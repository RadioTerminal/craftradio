//! The server: a TCP port, one thread per connection, and the routes of a mount — a
//! station's keyed source push in, its public listeners out, and the keyed title update
//! an MP3 source sends as it would to Icecast — the station's re-encoded outputs, and the
//! public stats of every live mount.
//! With a certificate and key in the config every connection is TLS (see [`crate::conn`]).

use crate::ServerError;
use crate::addr::{canonical, limit_key};
use crate::config::{
    Config, Output, Station, parse_saved_outputs, read_saved_outputs, saved_outputs_path,
};
use crate::conn::{Conn, server_config};
use crate::framing::Format;
use crate::http::{HttpError, Request, client_addr, read_body, read_request, respond};
use crate::icy::{IcyWriter, METAINT};
use crate::key::key_matches;
use crate::lockout::Lockout;
use crate::mount::{ListenError, Listener, ListenerCap, Mount, Received, SourceGuard};
use crate::pairing::{Document, PublicUrl};
use crate::stats::{StatsEntry, status_json};
use crate::transcode::{Outputs, Refusal, Transcoder, spawn};
use rustls::ServerConfig;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A source's body is read this much at a time. One [`crate::SourceGuard::write`] that
/// frames more chunks than a listener's queue holds would drop every listener.
const READ_BUFFER: usize = 4096;
/// How often a listener waiting on a silent source checks whether the server is stopping.
const LISTENER_POLL: Duration = Duration::from_millis(250);
/// Connection threads beyond the listener caps, for requests that are not streams and for
/// heads still arriving. Together with the caps they make the public part of the pool,
/// which anyone may fill; [`RESERVED_PER_STATION`] more are kept for sources.
const SPARE_CONNECTIONS: usize = 32;
/// Connection threads each station keeps beyond the public part (its source and a spare,
/// for a source that reconnects before its old connection is noticed gone). Only a request
/// with that station's key on its source route or title update takes one, and only once
/// the public part is full.
const RESERVED_PER_STATION: usize = 2;
/// How many connections from one address may be still sending their request head (or
/// TLS handshake) at a time. Not applied to the trusted proxy, which every client shares.
const HEADS_PER_ADDRESS: usize = 8;
/// How long a listener of an output waits for it to begin, once its station is on the air.
const OUTPUT_START_WAIT: Duration = Duration::from_secs(2);
/// How often that listener looks.
const OUTPUT_START_POLL: Duration = Duration::from_millis(20);
/// How long [`ServerHandle::stop`] waits for connection threads to finish.
const STOP_WAIT: Duration = Duration::from_secs(2);
/// The most of a refused request's rest that is read and thrown away (see
/// [`Timeouts::linger`]).
const LINGER_BYTES: usize = 64 << 10;
/// The shortest socket timeout; a zero timeout is an error to the OS.
const MIN_TIMEOUT: Duration = Duration::from_millis(1);
/// How long the accept loop pauses after a failed accept (for example out of file
/// descriptors), so it does not spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(10);
/// The largest body `PUT /api/station/outputs` takes.
const MAX_OUTPUTS_BODY: usize = 16 << 10;
/// What `PUT /api/station/outputs` answers on a server without a `state_dir`.
const NO_STATE_DIR: &[u8] = b"this server keeps no state_dir: outputs are set in its config";

const CORS: (&str, &str) = ("Access-Control-Allow-Origin", "*");
const LOCKED_FOR_SECS: &str = "300";
const CHALLENGE: (&str, &str) = ("WWW-Authenticate", "Basic realm=\"craftradio\"");
/// The body of a 503 for a request the full pool has no place for.
const FULL: &[u8] = b"the server has as many connections as it allows; try again later";
/// What Icecast answers a title update it took.
const METADATA_OK: &[u8] = b"<?xml version=\"1.0\"?><iceresponse><message>Metadata update successful</message><return>1</return></iceresponse>";

/// How long the server waits on its clients. Tests shorten them.
#[doc(hidden)]
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// For a whole request head to arrive.
    pub head: Duration,
    /// For a source's next byte before the source counts as gone.
    pub source_silence: Duration,
    /// For a listener to take the whole of each chunk (page or frame) before it counts as
    /// gone, however slowly it reads.
    pub listener_write: Duration,
    /// After a refusal, how long the rest of the request is read and thrown away, so that
    /// closing with unread bytes does not reset the connection before the reply is read.
    /// A connection doing this is answered: at the cap it is the first to be cut off.
    pub linger: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            head: Duration::from_secs(5),
            source_silence: Duration::from_secs(10),
            listener_write: Duration::from_secs(10),
            linger: Duration::from_millis(500),
        }
    }
}

/// Starts station servers.
pub struct Server;

impl Server {
    /// Binds `config.listen` (port 0 picks a free port) and serves every station in it.
    pub fn start(config: Config) -> Result<ServerHandle, ServerError> {
        Self::start_with(config, Timeouts::default())
    }

    #[doc(hidden)]
    pub fn start_with(config: Config, timeouts: Timeouts) -> Result<ServerHandle, ServerError> {
        // Read before binding, so a bad file leaves the port free.
        let tls = match (&config.tls_cert, &config.tls_key) {
            (Some(cert), Some(key)) => Some(server_config(cert, key)?),
            (None, None) => None,
            _ => {
                return Err(ServerError::Config(
                    "tls_cert and tls_key must be given together".into(),
                ));
            }
        };
        let public_url = PublicUrl::parse(&config.public_url).map_err(ServerError::Config)?;
        let mut config = config;
        apply_saved_outputs(&mut config)?;
        let listener = TcpListener::bind(config.listen)?;
        let local_addr = listener.local_addr()?;
        let mounts = config
            .stations
            .iter()
            .map(|station| (station.slug.clone(), StationMounts::new(station)))
            .collect();
        let public_connections = config
            .stations
            .iter()
            .fold(SPARE_CONNECTIONS, |sum, station| {
                sum.saturating_add(station.max_stream_listeners)
            });
        let max_connections = public_connections
            .saturating_add(config.stations.len().saturating_mul(RESERVED_PER_STATION));
        let shared = Arc::new(Shared {
            config,
            public_url,
            timeouts,
            tls,
            mounts,
            lockout: Lockout::default(),
            stopping: AtomicBool::new(false),
            connections: Mutex::default(),
            finished: Condvar::new(),
            max_connections,
            public_connections,
        });
        let accept = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("server-accept".into())
                .spawn(move || accept_loop(&shared, listener))?
        };
        Ok(ServerHandle {
            shared,
            local_addr,
            accept: Mutex::new(Some(accept)),
        })
    }
}

/// A running server. Dropping it stops the server.
pub struct ServerHandle {
    shared: Arc<Shared>,
    local_addr: SocketAddr,
    accept: Mutex<Option<JoinHandle<()>>>,
}

impl ServerHandle {
    /// The address the server listens on, with the port it was given.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// The outputs of the station `slug`, for tests and diagnostics: a test holds an
    /// output's mount through it, so that output fails for the next source.
    #[doc(hidden)]
    pub fn outputs(&self, slug: &str) -> Option<Arc<Outputs>> {
        self.shared.mounts.get(slug).map(StationMounts::outputs)
    }

    /// Whether the station `slug` has a transcoder, for tests: it has one only while a
    /// source is live and the station has outputs.
    #[doc(hidden)]
    pub fn transcoding(&self, slug: &str) -> bool {
        self.shared
            .mounts
            .get(slug)
            .is_some_and(|mounts| mounts.slot().is_some())
    }

    /// Stops accepting, ends every source and listener, and waits (briefly) for their
    /// threads. Calling it again does nothing more.
    pub fn stop(&self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        {
            let mut connections = self.shared.connections();
            connections.stopping = true;
            // A source thread's read and a listener thread's write return at once; the
            // source drops its guard, which ends the mount for its listeners.
            for stream in connections.open.values() {
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        let accept = self
            .accept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(accept) = accept {
            // The accept loop sees the flag once one more connection arrives. Without
            // that connection it would never return, so it is left running rather than
            // joined; it ends at the next connection anyone makes.
            let woken = TcpStream::connect_timeout(&wake_addr(self.local_addr), STOP_WAIT);
            if woken.is_ok() {
                let _ = accept.join();
            }
        }
        let deadline = Instant::now()
            .checked_add(STOP_WAIT)
            .unwrap_or_else(Instant::now);
        let mut connections = self.shared.connections();
        while !connections.open.is_empty() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            connections = self
                .shared
                .finished
                .wait_timeout(connections, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Puts each station's saved outputs (see [`Shared::set_outputs`]) in place of the ones
/// its config gives. A file that can't be read or fails the checks is an error naming it.
fn apply_saved_outputs(config: &mut Config) -> Result<(), ServerError> {
    let Some(dir) = config.state_dir.clone() else {
        return Ok(());
    };
    if !dir.is_dir() {
        return Err(ServerError::Config(format!(
            "state_dir {}: is not a directory the server can use",
            dir.display()
        )));
    }
    for station in &mut config.stations {
        let path = saved_outputs_path(&dir, &station.slug);
        let saved = path.try_exists().map_err(|e| {
            ServerError::Config(format!("{}: can't be looked at: {e}", path.display()))
        })?;
        if saved {
            station.outputs = read_saved_outputs(&path)?;
        }
    }
    Ok(())
}

/// Writes `outputs` to the station's file in `dir`: to a temporary file beside it first,
/// then renamed over it, so the file is always whole.
fn save_outputs(dir: &Path, slug: &str, outputs: &[Output]) -> std::io::Result<()> {
    let path = saved_outputs_path(dir, slug);
    let temporary = dir.join(format!(".{slug}.outputs.json.tmp"));
    let text = serde_json::to_vec_pretty(outputs)?;
    let written = std::fs::File::create(&temporary).and_then(|mut file| {
        file.write_all(&text)?;
        file.sync_all()
    });
    let saved = written.and_then(|()| std::fs::rename(&temporary, &path));
    if saved.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    saved
}

/// Where `stop` connects to wake the accept loop: the listening address, with an
/// unspecified IP replaced by loopback.
fn wake_addr(local: SocketAddr) -> SocketAddr {
    let ip = match local.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, local.port())
}

struct Shared {
    config: Config,
    /// The config's `public_url`, taken apart for the pairing document.
    public_url: PublicUrl,
    timeouts: Timeouts,
    /// Set when the config names a certificate: every connection is then TLS.
    tls: Option<Arc<ServerConfig>>,
    mounts: HashMap<String, StationMounts>,
    lockout: Lockout,
    stopping: AtomicBool,
    connections: Mutex<Connections>,
    /// Signalled whenever a connection thread finishes.
    finished: Condvar,
    /// Every connection thread there may be.
    max_connections: usize,
    /// How many of them may hold anything but a station's reserved place.
    public_connections: usize,
}

/// A station's mounts: the stream it pushes and its re-encoded outputs, all under the
/// station's one listener limit.
///
/// The transcoder slot is the station's one lock for a source's start and end and for a
/// change of outputs: each takes it first and holds it throughout, so a station never has
/// two transcoders, and none is left once its source has ended. A source begins and ends
/// the pushed mount under it, and a change of outputs looks at the pushed mount and
/// replaces `outputs` under it; nothing takes the slot while holding either of those.
struct StationMounts {
    pushed: Arc<Mount>,
    cap: Arc<ListenerCap>,
    /// The current outputs. A change replaces the whole set; a reader clones the `Arc` and
    /// keeps to that set.
    outputs: Mutex<Arc<Outputs>>,
    /// The transcoder of the live source, if the station has outputs.
    transcoder: Mutex<Option<Transcoder>>,
}

impl StationMounts {
    fn new(station: &Station) -> StationMounts {
        let cap = ListenerCap::new(station.max_stream_listeners);
        StationMounts {
            pushed: Mount::new(Arc::clone(&cap)),
            outputs: Mutex::new(Outputs::new(&station.outputs, &cap)),
            cap,
            transcoder: Mutex::new(None),
        }
    }

    /// The current outputs. The lock only guards the swap of an `Arc`, so a poisoned one
    /// still holds a whole set.
    fn outputs(&self) -> Arc<Outputs> {
        Arc::clone(&self.outputs.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// The transcoder slot (see [`StationMounts`]). A thread that panicked holding it left
    /// a transcoder or none, either of which the next holder handles.
    fn slot(&self) -> MutexGuard<'_, Option<Transcoder>> {
        self.transcoder
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Finishes the transcoder in `slot`, if any, ending its outputs, and starts one on the
    /// current outputs while a source is live.
    fn restart(&self, slot: &mut Option<Transcoder>) {
        if let Some(old) = slot.take() {
            old.finish();
        }
        let outputs = self.outputs();
        if self.pushed.stats().live && !outputs.is_empty() {
            *slot = Some(spawn(Arc::clone(&self.pushed), outputs));
        }
    }
}

/// A live source: the pushed mount's guard, with the station's transcoder in its slot.
/// Dropping it, on every way out of [`source`] (unwinding included), finishes the
/// transcoder, ending its outputs, before the guard frees the mount for a new source, both
/// under the slot's lock, so a change of outputs can't start a transcoder in between.
struct OnAir<'a> {
    mounts: &'a StationMounts,
    guard: Option<SourceGuard>,
}

impl<'a> OnAir<'a> {
    /// Begins the station's pushed mount and starts its transcoder; `None` while another
    /// source is live.
    fn begin(
        mounts: &'a StationMounts,
        format: Format,
        name: String,
        kbps: Option<u32>,
    ) -> Option<OnAir<'a>> {
        let mut slot = mounts.slot();
        let guard = mounts.pushed.begin(format, name, kbps)?;
        mounts.restart(&mut slot);
        Some(OnAir {
            mounts,
            guard: Some(guard),
        })
    }

    fn write(&mut self, bytes: &[u8]) {
        if let Some(guard) = &mut self.guard {
            guard.write(bytes);
        }
    }
}

impl Drop for OnAir<'_> {
    fn drop(&mut self) {
        let mut slot = self.mounts.slot();
        if let Some(transcoder) = slot.take() {
            transcoder.finish();
        }
        drop(self.guard.take());
        drop(slot);
    }
}

/// Why a station's outputs were not changed.
enum SetOutputsError {
    /// The server has no `state_dir` to keep them in.
    NoStateDir,
    /// They could not be saved; nothing changed.
    Save(std::io::Error),
}

/// The open connections, so `stop` can close them, and what limits them.
#[derive(Default)]
struct Connections {
    stopping: bool,
    next_id: u64,
    /// Each connection's socket, shared with its thread: one descriptor a connection.
    open: HashMap<u64, Arc<TcpStream>>,
    /// The connections still sending their head (or TLS handshake), oldest first, with
    /// their peer address; `None` for the trusted proxy.
    in_head: BTreeMap<u64, Option<IpAddr>>,
    /// How many of `in_head` each address (an IPv6 address by its /64) has, the trusted
    /// proxy left out.
    heads: HashMap<IpAddr, usize>,
    /// Connections that were refused and answered, and are only reading what is left of
    /// the request before closing, oldest first. They hold a thread, but nothing else.
    lingering: BTreeSet<u64>,
    /// Connections cut off to make room (a head still arriving, or a refusal lingering):
    /// their sockets are shut and their threads about to end. They count toward no limit
    /// and are never let on.
    evicted: HashSet<u64>,
    /// The connections holding a station's reserved place: id to station slug.
    reserved: HashMap<u64, String>,
}

impl Connections {
    /// Connection threads that count toward the cap: every open one but those evicted.
    fn counted(&self) -> usize {
        self.open.len().saturating_sub(self.evicted.len())
    }

    /// Whether connection `id`, its head read, fits in the public part of the pool. A
    /// refusal that lingers does not hold a public place: it has been answered.
    fn public_room(&self, id: u64, public_connections: usize) -> bool {
        let public = self
            .open
            .keys()
            .filter(|open| {
                **open != id
                    && !self.reserved.contains_key(*open)
                    && !self.evicted.contains(*open)
                    && !self.lingering.contains(*open)
            })
            .count();
        public < public_connections
    }

    /// Gives connection `id` one of `slug`'s reserved places, if one is free.
    fn reserve(&mut self, id: u64, slug: &str) -> bool {
        let taken = self.reserved.values().filter(|held| *held == slug).count();
        if taken >= RESERVED_PER_STATION {
            return false;
        }
        self.reserved.insert(id, slug.to_owned());
        true
    }

    /// Connection `id` is no longer sending its head. False if it was evicted meanwhile.
    fn end_head(&mut self, id: u64) -> bool {
        if let Some(Some(addr)) = self.in_head.remove(&id)
            && let Some(heads) = self.heads.get_mut(&addr)
        {
            *heads = heads.saturating_sub(1);
            if *heads == 0 {
                self.heads.remove(&addr);
            }
        }
        !self.evicted.contains(&id)
    }

    /// Connection `id` was refused and answered, and lingers: it gives up any reserved
    /// place it held. False if it was cut off meanwhile.
    fn begin_linger(&mut self, id: u64) -> bool {
        if self.evicted.contains(&id) {
            return false;
        }
        self.reserved.remove(&id);
        self.lingering.insert(id);
        true
    }

    /// Makes room for a new connection at the cap by cutting off, oldest first, a refusal
    /// that lingers, else a head still arriving from a client address, else one from the
    /// trusted proxy. A connection whose head is read and that was not refused is never
    /// cut off. False if there is nothing to cut off.
    fn evict_one(&mut self) -> bool {
        let oldest = self.lingering.first().copied().or_else(|| {
            self.in_head
                .iter()
                .find(|(_, addr)| addr.is_some())
                .or_else(|| self.in_head.iter().next())
                .map(|(id, _)| *id)
        });
        let Some(id) = oldest else {
            return false;
        };
        self.end_head(id);
        self.lingering.remove(&id);
        self.evicted.insert(id);
        if let Some(stream) = self.open.get(&id) {
            // Its read fails at once, and its thread ends.
            let _ = stream.shutdown(Shutdown::Both);
        }
        true
    }
}

impl Shared {
    fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// Saves `outputs` (already checked) as station `slug`'s and puts them live: every
    /// current output ends (its listeners are cut off), and while the station is on the
    /// air its transcoder starts again on the new set. The pushed stream goes on. Gives
    /// the new set; on an error nothing has changed, on disk or live.
    fn set_outputs(
        &self,
        mounts: &StationMounts,
        slug: &str,
        outputs: &[Output],
    ) -> Result<Arc<Outputs>, SetOutputsError> {
        let dir = self
            .config
            .state_dir
            .as_deref()
            .ok_or(SetOutputsError::NoStateDir)?;
        // Saved and swapped under the slot's lock, so two changes at once leave the file
        // and the live set the same.
        let mut slot = mounts.slot();
        save_outputs(dir, slug, outputs).map_err(SetOutputsError::Save)?;
        let new = Outputs::new(outputs, &mounts.cap);
        *mounts
            .outputs
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Arc::clone(&new);
        // The old transcoder ends the old outputs and their listeners as it finishes.
        mounts.restart(&mut slot);
        Ok(new)
    }

    /// The connection list, even after a thread panicked while holding it: it only maps
    /// ids to sockets.
    fn connections(&self) -> MutexGuard<'_, Connections> {
        self.connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives an accepted connection its thread, or closes it when stopping or when its
    /// address already has as many heads coming as it may. At the cap, the oldest refusal
    /// still lingering, or else the oldest head still arriving, is cut off to make room, so
    /// a source always gets the chance to show its key; with neither, the new connection
    /// is closed.
    fn admit(self: &Arc<Self>, stream: TcpStream, peer: SocketAddr) {
        let stream = Arc::new(stream);
        let from_proxy = self
            .config
            .trusted_proxy
            .is_some_and(|proxy| canonical(proxy) == canonical(peer.ip()));
        let head_from = (!from_proxy).then_some(limit_key(peer.ip()));
        let id = {
            let mut connections = self.connections();
            // Checked under the same lock `stop` closes connections under, so none slips
            // in after it.
            if connections.stopping {
                return;
            }
            if let Some(addr) = head_from
                && connections.heads.get(&addr).copied().unwrap_or_default() >= HEADS_PER_ADDRESS
            {
                return;
            }
            if connections.counted() >= self.max_connections && !connections.evict_one() {
                return;
            }
            if let Some(addr) = head_from {
                *connections.heads.entry(addr).or_default() += 1;
            }
            let id = connections.next_id;
            connections.next_id += 1;
            connections.open.insert(id, Arc::clone(&stream));
            connections.in_head.insert(id, head_from);
            id
        };
        let guard = ConnectionGuard {
            shared: Arc::clone(self),
            id,
        };
        let spawned = std::thread::Builder::new()
            .name("server-connection".into())
            .spawn(move || {
                let guard = guard;
                serve(&guard, peer, stream);
            });
        // A thread that could not start drops its closure, and with it the guard.
        drop(spawned);
    }
}

/// Removes a connection from the list when its thread ends; the socket closes once the
/// thread's own handle is gone too.
struct ConnectionGuard {
    shared: Arc<Shared>,
    id: u64,
}

impl ConnectionGuard {
    /// The head is read (or failed): the connection no longer counts as one coming.
    /// False if it was cut off meanwhile to make room, and must not go on.
    fn end_head(&self) -> bool {
        self.shared.connections().end_head(self.id)
    }

    /// The connection was refused and answered; from now on it may be cut off to make
    /// room. False if it was cut off already.
    fn begin_linger(&self) -> bool {
        self.shared.connections().begin_linger(self.id)
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        {
            let mut connections = self.shared.connections();
            connections.end_head(self.id);
            connections.open.remove(&self.id);
            connections.lingering.remove(&self.id);
            connections.evicted.remove(&self.id);
            connections.reserved.remove(&self.id);
        }
        self.shared.finished.notify_all();
    }
}

fn accept_loop(shared: &Arc<Shared>, listener: TcpListener) {
    loop {
        let accepted = listener.accept();
        if shared.stopping() {
            break;
        }
        match accepted {
            Ok((stream, peer)) => shared.admit(stream, peer),
            Err(_) => std::thread::sleep(ACCEPT_BACKOFF),
        }
    }
}

fn serve(guard: &ConnectionGuard, peer: SocketAddr, tcp: Arc<TcpStream>) {
    let shared = Arc::clone(&guard.shared);
    let shared = &*shared;
    let head = shared.timeouts.head.max(MIN_TIMEOUT);
    // The TLS handshake and the request head both have to be in by then. Until then the
    // connection sets its socket's timeouts to what is left before each read and write,
    // so neither blocks past it, however slowly the bytes come.
    let deadline = Instant::now()
        .checked_add(head)
        .unwrap_or_else(Instant::now);
    let opened = match &shared.tls {
        None => Ok(Conn::plain(tcp, deadline)),
        Some(config) => Conn::tls(tcp, deadline, config),
    };
    // A handshake that failed or came too late: there is no HTTP to answer.
    let Ok(mut stream) = opened else {
        return;
    };
    let read = read_request(&mut stream, deadline);
    if !guard.end_head() {
        return;
    }
    if stream
        .end_head(head, shared.timeouts.listener_write.max(MIN_TIMEOUT))
        .is_err()
    {
        return;
    }
    answer(shared, guard.id, &mut stream, peer.ip(), read);
    if stream.write_shut() {
        linger(guard, &mut stream);
    }
}

/// Answers a request whose head has been read (or failed to parse).
fn answer(
    shared: &Shared,
    id: u64,
    stream: &mut Conn,
    peer: IpAddr,
    read: Result<Request, HttpError>,
) {
    let request = match read {
        Ok(request) => request,
        Err(HttpError::Malformed) => {
            return refuse(stream, 400, "Bad Request", &[], b"bad request");
        }
        Err(HttpError::TooLarge) => {
            return refuse(
                stream,
                431,
                "Request Header Fields Too Large",
                &[],
                b"request head too large",
            );
        }
        // Too slow, or gone: there is no one to answer.
        Err(HttpError::Timeout | HttpError::Io(_)) => return,
    };
    if gate(shared, id, stream, peer, &request) {
        route(shared, stream, peer, &request);
    }
}

/// Lets a request whose head is read go on: any request while the public part of the
/// pool has room, else only one with a station's key, on its source route or title
/// update, into one of that station's reserved places. Answers a request it stops.
fn gate(shared: &Shared, id: u64, stream: &mut Conn, peer: IpAddr, request: &Request) -> bool {
    if shared
        .connections()
        .public_room(id, shared.public_connections)
    {
        return true;
    }
    let Some(station) = keyed_station(shared, request) else {
        refuse(stream, 503, "Service Unavailable", &[CORS], FULL);
        return false;
    };
    // A wrong key is answered, and counted, as on any other keyed request.
    if !authorized(shared, stream, peer, request, station) {
        return false;
    }
    if !shared.connections().reserve(id, &station.slug) {
        refuse(stream, 503, "Service Unavailable", &[], FULL);
        return false;
    }
    true
}

/// The station a request would need the key of: a source push to its mount, or a title
/// update for it.
fn keyed_station<'a>(shared: &'a Shared, request: &Request) -> Option<&'a Station> {
    match request.method.as_str() {
        "PUT" | "SOURCE" => station_at(shared, &request.path),
        "GET" if request.path == "/admin/metadata" => station_at(shared, request.param("mount")?),
        _ => None,
    }
    .map(|(station, _)| station)
}

fn route(shared: &Shared, stream: &mut Conn, peer: IpAddr, request: &Request) {
    let method = request.method.as_str();
    match method {
        "GET" | "PUT" | "SOURCE" => {}
        "OPTIONS" => {
            let _ = respond(
                stream,
                204,
                "No Content",
                &[CORS, ("Access-Control-Allow-Methods", "GET, OPTIONS")],
                b"",
            );
            return;
        }
        _ => {
            return refuse(
                stream,
                405,
                "Method Not Allowed",
                &[("Allow", "GET, PUT, SOURCE, OPTIONS")],
                b"method not allowed",
            );
        }
    }
    if method == "GET" && request.path == "/admin/metadata" {
        return metadata(shared, stream, peer, request);
    }
    if method == "GET" && request.path == "/api/station" {
        return pairing(shared, stream, peer, request);
    }
    if method == "PUT" && request.path == "/api/station/outputs" {
        return put_outputs(shared, stream, peer, request);
    }
    if method == "GET" && request.path == "/status-json.xsl" {
        return stats(shared, stream);
    }
    if let Some((station, mounts)) = station_at(shared, &request.path) {
        return if method == "GET" {
            let titles = Titles::asked(request, station, &mounts.pushed);
            listen(shared, stream, &mounts.pushed, titles);
        } else {
            source(shared, stream, peer, request, station, mounts);
        };
    }
    // Only a listener may name an output: no key is looked at for one.
    if method == "GET"
        && let Some((slug, mounts, found)) = output_at(shared, &request.path)
    {
        let titles = shared
            .config
            .station(slug)
            .and_then(|station| Titles::asked(request, station, &mounts.pushed));
        return listen_output(shared, stream, slug, mounts, &found, titles);
    }
    refuse(stream, 404, "Not Found", &[CORS], b"not found")
}

/// The station whose pushed stream is at `path`, and its mounts. The one place a key is
/// taken: an output's path is never a station's here.
fn station_at<'a>(shared: &'a Shared, path: &str) -> Option<(&'a Station, &'a StationMounts)> {
    let (slug, "stream") = split_path(path)? else {
        return None;
    };
    let station = shared.config.station(slug)?;
    let mounts = shared.mounts.get(slug)?;
    Some((station, mounts))
}

/// One output as a listener found it: the station's set of outputs then, the output and
/// its own mount in that set.
struct FoundOutput {
    outputs: Arc<Outputs>,
    output: Output,
    mount: Arc<Mount>,
}

/// The output at `path`, `/<slug>/<mount>`, in the station's current outputs: the slug,
/// the station's mounts and the output.
fn output_at<'a, 'p>(
    shared: &'a Shared,
    path: &'p str,
) -> Option<(&'p str, &'a StationMounts, FoundOutput)> {
    let (slug, name) = split_path(path)?;
    let mounts = shared.mounts.get(slug)?;
    let outputs = mounts.outputs();
    let (output, mount) = outputs
        .each()
        .find(|(output, _)| output.mount == name)
        .map(|(output, mount)| (output.clone(), Arc::clone(mount)))?;
    Some((
        slug,
        mounts,
        FoundOutput {
            outputs,
            output,
            mount,
        },
    ))
}

/// The slug and mount of `/<slug>/<mount>`, both one non-empty path segment.
fn split_path(path: &str) -> Option<(&str, &str)> {
    let (slug, mount) = path.strip_prefix('/')?.split_once('/')?;
    (!slug.is_empty() && !mount.is_empty() && !mount.contains('/')).then_some((slug, mount))
}

fn source(
    shared: &Shared,
    stream: &mut Conn,
    peer: IpAddr,
    request: &Request,
    station: &Station,
    mounts: &StationMounts,
) {
    if !authorized(shared, stream, peer, request, station) {
        return;
    }
    let Some(format) = request
        .header("Content-Type")
        .and_then(Format::from_content_type)
    else {
        return refuse(
            stream,
            415,
            "Unsupported Media Type",
            &[],
            b"the stream must be audio/ogg or audio/mpeg",
        );
    };
    let name = request
        .header("Ice-Name")
        .filter(|name| !name.is_empty())
        .unwrap_or(&station.name)
        .to_owned();
    // Only a whole number above zero: anything else gives no bitrate.
    let kbps = request
        .header("Ice-Bitrate")
        .and_then(|kbps| kbps.parse::<u32>().ok())
        .filter(|kbps| *kbps > 0);
    // Taken before the reply, so a listener who comes the moment the source has its 200
    // finds the source live.
    let Some(mut on_air) = OnAir::begin(mounts, format, name, kbps) else {
        return refuse(
            stream,
            403,
            "Forbidden",
            &[],
            b"this mount already has a source",
        );
    };
    if stream.write_all(b"HTTP/1.0 200 OK\r\n\r\n").is_err()
        || stream.flush().is_err()
        || stream
            .set_read_timeout(shared.timeouts.source_silence.max(MIN_TIMEOUT))
            .is_err()
    {
        return;
    }
    let mut buffer = [0; READ_BUFFER];
    while !shared.stopping() {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => on_air.write(buffer.get(..read).unwrap_or_default()),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            // A read timeout is the source's silence: it is gone.
            Err(_) => break,
        }
    }
    // Ends the outputs, then the stream for every listener, and frees the mount.
    drop(on_air);
}

/// Checks the station key of a keyed request for `station`, and answers the request
/// itself when it is refused (see [`keyed`]).
fn authorized(
    shared: &Shared,
    stream: &mut Conn,
    peer: IpAddr,
    request: &Request,
    station: &Station,
) -> bool {
    keyed(shared, stream, peer, request, |key| {
        key_matches(key, &station.key_hash).then_some(())
    })
    .is_some()
}

/// Checks the key of a keyed request with `find`, which gives what the key opens, and
/// answers the request itself when it is refused. A locked-out address is refused before
/// its key is looked at. A request that presents no `Authorization` header at all is
/// refused without counting toward the lockout, so stray requests from a shared address
/// cannot lock a station out; any header that does not carry a right key (a wrong key,
/// another scheme, or one that does not decode) counts as a wrong key.
fn keyed<T>(
    shared: &Shared,
    stream: &mut Conn,
    peer: IpAddr,
    request: &Request,
    find: impl FnOnce(&str) -> Option<T>,
) -> Option<T> {
    let addr = client_addr(peer, request, shared.config.trusted_proxy);
    let now = Instant::now();
    if shared.lockout.blocked(addr, now) {
        refuse(
            stream,
            429,
            "Too Many Requests",
            &[("Retry-After", LOCKED_FOR_SECS)],
            b"too many wrong keys from this address; try again later",
        );
        return None;
    }
    if request.header("Authorization").is_none() {
        refuse(stream, 401, "Unauthorized", &[CHALLENGE], b"no key");
        return None;
    }
    let found = request.basic_password().and_then(|key| find(&key));
    if found.is_none() {
        shared.lockout.failed(addr, now);
        refuse(stream, 401, "Unauthorized", &[CHALLENGE], b"wrong key");
    }
    found
}

/// The station whose key is `key`. Every station's hash is checked, with no early exit,
/// so how long the answer takes does not tell which slug the key belongs to. The config
/// gives each station a hash of its own, so at most one matches.
fn station_with_key<'a>(shared: &'a Shared, key: &str) -> Option<&'a Station> {
    let mut found = None;
    for station in &shared.config.stations {
        if key_matches(key, &station.key_hash) {
            found = Some(station);
        }
    }
    found
}

/// `GET /api/station`: the pairing document of the station whose key the request carries,
/// as its Basic password with any user name.
fn pairing(shared: &Shared, stream: &mut Conn, peer: IpAddr, request: &Request) {
    let Some(station) = keyed(shared, stream, peer, request, |key| {
        station_with_key(shared, key)
    }) else {
        return;
    };
    let Some(mounts) = shared.mounts.get(&station.slug) else {
        return refuse(stream, 404, "Not Found", &[], b"not found");
    };
    send_document(shared, stream, station, &mounts.outputs());
}

/// Answers 200 with the pairing document of `station`, whose outputs are `outputs`.
fn send_document(shared: &Shared, stream: &mut Conn, station: &Station, outputs: &Outputs) {
    let outputs: Vec<Output> = outputs.each().map(|(output, _)| output.clone()).collect();
    let document = Document::for_station(
        &shared.public_url,
        station,
        &outputs,
        shared.config.state_dir.is_some(),
    );
    let Ok(body) = serde_json::to_vec(&document) else {
        return refuse(
            stream,
            500,
            "Internal Server Error",
            &[],
            b"the pairing document could not be written",
        );
    };
    let _ = respond(
        stream,
        200,
        "OK",
        &[
            ("Content-Type", "application/json"),
            ("Cache-Control", "no-store"),
        ],
        &body,
    );
}

/// `PUT /api/station/outputs`: replaces the outputs of the station whose key the request
/// carries (as `GET /api/station` takes it) with the JSON list in the body, saved in
/// `state_dir` and put live at once; answers with the new pairing document. A refused
/// change changes nothing.
fn put_outputs(shared: &Shared, stream: &mut Conn, peer: IpAddr, request: &Request) {
    let Some(station) = keyed(shared, stream, peer, request, |key| {
        station_with_key(shared, key)
    }) else {
        return;
    };
    let Some(mounts) = shared.mounts.get(&station.slug) else {
        return refuse(stream, 404, "Not Found", &[], b"not found");
    };
    if shared.config.state_dir.is_none() {
        return refuse(stream, 403, "Forbidden", &[], NO_STATE_DIR);
    }
    let Some(length) = request.header("Content-Length") else {
        return refuse(
            stream,
            411,
            "Length Required",
            &[],
            b"the request must say its Content-Length",
        );
    };
    let Ok(length) = length.parse::<usize>() else {
        return refuse(
            stream,
            400,
            "Bad Request",
            &[],
            b"the Content-Length is not a number",
        );
    };
    if length > MAX_OUTPUTS_BODY {
        let body = format!("the list of outputs may be at most {MAX_OUTPUTS_BODY} bytes");
        return refuse(stream, 413, "Content Too Large", &[], body.as_bytes());
    }
    let deadline = Instant::now()
        .checked_add(shared.timeouts.head.max(MIN_TIMEOUT))
        .unwrap_or_else(Instant::now);
    let Ok(body) = read_body(stream, length, deadline) else {
        return refuse(
            stream,
            400,
            "Bad Request",
            &[],
            b"the body did not all arrive",
        );
    };
    let outputs = match parse_saved_outputs(&body) {
        Ok(outputs) => outputs,
        Err(reason) => return refuse(stream, 400, "Bad Request", &[], reason.as_bytes()),
    };
    match shared.set_outputs(mounts, &station.slug, &outputs) {
        Ok(current) => send_document(shared, stream, station, &current),
        Err(SetOutputsError::NoStateDir) => refuse(stream, 403, "Forbidden", &[], NO_STATE_DIR),
        Err(SetOutputsError::Save(error)) => {
            let body = format!("the outputs could not be saved, so nothing changed: {error}");
            refuse(stream, 500, "Internal Server Error", &[], body.as_bytes());
        }
    }
}

/// `GET /admin/metadata?mount=/<slug>/stream&mode=updinfo&song=<title>`: an MP3 source's
/// title, which its stream does not carry.
fn metadata(shared: &Shared, stream: &mut Conn, peer: IpAddr, request: &Request) {
    let Some(path) = request.param("mount") else {
        return refuse(
            stream,
            400,
            "Bad Request",
            &[],
            b"the request names no mount",
        );
    };
    let Some((station, mounts)) = station_at(shared, path) else {
        return refuse(stream, 404, "Not Found", &[], b"not found");
    };
    if !authorized(shared, stream, peer, request, station) {
        return;
    }
    if request.param("mode") != Some("updinfo") {
        return refuse(
            stream,
            400,
            "Bad Request",
            &[],
            b"only mode=updinfo is supported",
        );
    }
    let Some(song) = request.param("song") else {
        return refuse(stream, 400, "Bad Request", &[], b"the request has no song");
    };
    // The mount cuts it to 512 characters.
    if !mounts.pushed.set_title(song.to_owned()) {
        return refuse(
            stream,
            400,
            "Bad Request",
            &[],
            b"this station is not on the air",
        );
    }
    let _ = respond(
        stream,
        200,
        "OK",
        &[("Content-Type", "text/xml")],
        METADATA_OK,
    );
}

/// `GET /status-json.xsl`: public and unkeyed, for the listener page's "now playing".
/// Each station's pushed mount comes first, then its outputs in the order they were set, so the page,
/// which falls back to the first entry, finds the pushed stream. An output carries the
/// pushed mount's title and artist; one whose source is refused, or that failed, is left
/// out.
fn stats(shared: &Shared, stream: &mut Conn) {
    let mut entries = Vec::new();
    for station in &shared.config.stations {
        let Some(mounts) = shared.mounts.get(&station.slug) else {
            continue;
        };
        let mut pushed = mounts.pushed.stats();
        if pushed.name.is_empty() {
            pushed.name.clone_from(&station.name);
        }
        let current = mounts.outputs();
        let outputs: Vec<StatsEntry> = if current.refusal().is_some() {
            Vec::new()
        } else {
            current
                .each()
                .filter(|(output, _)| !current.failed(&output.mount))
                .map(|(output, mount)| {
                    let mut stats = mount.stats();
                    if stats.name.is_empty() {
                        stats.name.clone_from(&pushed.name);
                    }
                    stats.title.clone_from(&pushed.title);
                    stats.artist.clone_from(&pushed.artist);
                    StatsEntry {
                        path: format!("{}/{}", station.slug, output.mount),
                        stats,
                    }
                })
                .collect()
        };
        entries.push(StatsEntry {
            path: format!("{}/stream", station.slug),
            stats: pushed,
        });
        entries.extend(outputs);
    }
    let body = status_json(&shared.config.public_url, &entries);
    let _ = respond(
        stream,
        200,
        "OK",
        &[
            ("Content-Type", "application/json"),
            ("Cache-Control", "no-cache, no-store"),
            CORS,
        ],
        body.as_bytes(),
    );
}

/// A listener of a station's pushed stream.
fn listen(shared: &Shared, stream: &mut Conn, mount: &Arc<Mount>, titles: Option<Titles>) {
    match mount.listen() {
        Ok(listener) => play(shared, stream, &listener, titles),
        Err(ListenError::NoSource) => off_air(stream),
        Err(ListenError::Full) => full(stream),
    }
}

/// A listener of one of a station's outputs. With the station on the air but the output
/// not begun yet (its transcoder is still reading the source's first audio), it waits up
/// to [`OUTPUT_START_WAIT`] for it. Outputs that can't run, an output that failed, and one
/// whose station's outputs were changed meanwhile, are refused at once with the reason.
fn listen_output(
    shared: &Shared,
    stream: &mut Conn,
    slug: &str,
    mounts: &StationMounts,
    found: &FoundOutput,
    titles: Option<Titles>,
) {
    let FoundOutput {
        outputs,
        output,
        mount,
    } = found;
    let deadline = Instant::now()
        .checked_add(OUTPUT_START_WAIT)
        .unwrap_or_else(Instant::now);
    loop {
        if !Arc::ptr_eq(outputs, &mounts.outputs()) {
            return refuse(
                stream,
                503,
                "Service Unavailable",
                &[CORS],
                b"this station's outputs were just changed; try again",
            );
        }
        if let Some(refusal) = outputs.refusal() {
            return refuse(
                stream,
                503,
                "Service Unavailable",
                &[CORS],
                refused(refusal).as_bytes(),
            );
        }
        // Checked before joining: a failed output's mount may still be held.
        if outputs.failed(&output.mount) {
            let body = format!(
                "the {} output ({} kbit/s) failed for this broadcast and is off until the \
                 source reconnects; the station's main stream still plays at /{slug}/stream",
                output.format.name(),
                output.kbps
            );
            return refuse(stream, 503, "Service Unavailable", &[CORS], body.as_bytes());
        }
        match mount.listen() {
            Ok(listener) => return play(shared, stream, &listener, titles),
            Err(ListenError::Full) => return full(stream),
            Err(ListenError::NoSource) => {}
        }
        if !mounts.pushed.stats().live {
            return off_air(stream);
        }
        // Stopping has closed the connection already.
        if shared.stopping() {
            return;
        }
        if Instant::now() >= deadline {
            let body = format!(
                "the {} output ({} kbit/s) is starting and not playing yet; try again in a moment",
                output.format.name(),
                output.kbps
            );
            return refuse(stream, 503, "Service Unavailable", &[CORS], body.as_bytes());
        }
        std::thread::sleep(OUTPUT_START_POLL);
    }
}

/// Why a station's outputs are off, in words.
fn refused(refusal: Refusal) -> String {
    match refusal {
        Refusal::Unsupported { rate, channels } => format!(
            "this station's outputs are off: its source sends {rate} Hz audio in {channels} \
             channel(s), and the outputs need 48000 Hz stereo; the stream it pushes is still \
             at its stream path"
        ),
        Refusal::Unreadable => "this station's outputs are off: its source's audio can't be \
                                decoded; the stream it pushes is still at its stream path"
            .to_owned(),
    }
}

fn off_air(stream: &mut Conn) {
    refuse(
        stream,
        404,
        "Not Found",
        &[CORS],
        b"this station is not on the air",
    );
}

fn full(stream: &mut Conn) {
    refuse(
        stream,
        503,
        "Service Unavailable",
        &[CORS],
        b"this station has as many listeners as it allows",
    );
}

/// Where an MP3 listener who asked for ICY titles gets them: the station's pushed mount,
/// whose title and artist its outputs carry too.
struct Titles<'a> {
    pushed: &'a Mount,
    /// For `icy-name`: the source's name, else the station's.
    name: String,
}

impl<'a> Titles<'a> {
    /// Set when the request asks with `Icy-MetaData: 1`.
    fn asked(request: &Request, station: &Station, pushed: &'a Mount) -> Option<Titles<'a>> {
        if request.header("Icy-MetaData").map(str::trim) != Some("1") {
            return None;
        }
        let mut name = pushed.stats().name;
        if name.is_empty() {
            name.clone_from(&station.name);
        }
        // A header value holds no line breaks or other control characters.
        let name = name
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        Some(Titles { pushed, name })
    }

    /// `artist - title`, or the title alone with no artist.
    fn now(&self) -> String {
        let stats = self.pushed.stats();
        if stats.artist.is_empty() {
            stats.title
        } else {
            format!("{} - {}", stats.artist, stats.title)
        }
    }
}

/// A listener's stream. Each chunk must reach the client within
/// [`Timeouts::listener_write`]; a listener that falls further behind than its queue holds
/// is dropped by the mount and ended at once, so a slow reader frees its connection thread.
/// An MP3 listener who asked for titles gets them interleaved (see [`crate::icy`]); any
/// other listener gets the bytes as they are.
fn play(shared: &Shared, stream: &mut Conn, listener: &Listener, titles: Option<Titles>) {
    let mut titles = titles
        .filter(|_| listener.format() == Format::Mp3)
        .map(|titles| (titles, IcyWriter::new(), Vec::new()));
    let icy = titles.as_ref().map_or_else(String::new, |(titles, _, _)| {
        format!("icy-metaint: {METAINT}\r\nicy-name: {}\r\n", titles.name)
    });
    let head = format!(
        "HTTP/1.0 200 OK\r\nContent-Type: {}\r\nCache-Control: no-cache, no-store\r\n\
         Access-Control-Allow-Origin: *\r\nConnection: close\r\n{icy}\r\n",
        listener.format().content_type()
    );
    let write = shared.timeouts.listener_write.max(MIN_TIMEOUT);
    if stream.write_all_within(head.as_bytes(), write).is_err() {
        return;
    }
    loop {
        match listener.recv(LISTENER_POLL) {
            Received::Chunk(chunk) => {
                let bytes = match &mut titles {
                    Some((titles, writer, out)) => {
                        out.clear();
                        // The title is looked up only for a chunk that reaches a block.
                        let now = if writer.due(chunk.len()) {
                            titles.now()
                        } else {
                            String::new()
                        };
                        writer.write(&chunk, &now, out);
                        out.as_slice()
                    }
                    None => &chunk[..],
                };
                if stream.write_all_within(bytes, write).is_err() {
                    break;
                }
            }
            Received::Timeout => {
                if shared.stopping() {
                    break;
                }
            }
            Received::Ended => break,
        }
    }
}

/// Answers with an error and ends the write side. [`serve`] then lingers (see [`linger`]).
fn refuse(stream: &mut Conn, status: u16, reason: &str, headers: &[(&str, &str)], body: &[u8]) {
    if respond(stream, status, reason, headers, body).is_ok() {
        let _ = stream.shutdown_write();
    }
}

/// Closes a refused connection gently: whatever the client still sends is read for a
/// moment, so its unread bytes don't reset the connection and lose the reply. Meanwhile
/// the connection may be cut off to make room for a newcomer.
fn linger(guard: &ConnectionGuard, stream: &mut Conn) {
    let linger = guard.shared.timeouts.linger.max(MIN_TIMEOUT);
    if !guard.begin_linger() || stream.set_read_timeout(linger).is_err() {
        return;
    }
    let until = Instant::now()
        .checked_add(linger)
        .unwrap_or_else(Instant::now);
    let mut thrown = 0;
    let mut buffer = [0; READ_BUFFER];
    while thrown < LINGER_BYTES && Instant::now() < until {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => thrown += read,
        }
    }
}
