//! The server: a TCP port, one thread per connection, and the routes of a mount — a
//! station's keyed source push in, its public listeners out, and the keyed title update
//! an MP3 source sends as it would to Icecast — and the public stats of every live mount.

use crate::ServerError;
use crate::config::{Config, Station};
use crate::conn::Conn;
use crate::framing::Format;
use crate::http::{HttpError, Request, client_addr, read_request, respond};
use crate::key::key_matches;
use crate::lockout::Lockout;
use crate::mount::{ListenError, Mount, Received};
use crate::stats::status_json;
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
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
/// How long [`ServerHandle::stop`] waits for connection threads to finish.
const STOP_WAIT: Duration = Duration::from_secs(2);
/// After a refusal, how long the rest of the request is read and thrown away, so that
/// closing with unread bytes does not reset the connection before the reply is read.
const LINGER: Duration = Duration::from_millis(500);
const LINGER_BYTES: usize = 64 << 10;
/// The shortest socket timeout; a zero timeout is an error to the OS.
const MIN_TIMEOUT: Duration = Duration::from_millis(1);
/// How long the accept loop pauses after a failed accept (for example out of file
/// descriptors), so it does not spin.
const ACCEPT_BACKOFF: Duration = Duration::from_millis(10);

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
    /// For a listener to take the next write before it counts as gone.
    pub listener_write: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Timeouts {
            head: Duration::from_secs(5),
            source_silence: Duration::from_secs(10),
            listener_write: Duration::from_secs(10),
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
        let listener = TcpListener::bind(config.listen)?;
        let local_addr = listener.local_addr()?;
        let mounts = config
            .stations
            .iter()
            .map(|station| {
                (
                    station.slug.clone(),
                    Mount::new(station.max_stream_listeners),
                )
            })
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
            timeouts,
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
    timeouts: Timeouts,
    mounts: HashMap<String, Arc<Mount>>,
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

/// The open connections, so `stop` can close them, and what limits them.
#[derive(Default)]
struct Connections {
    stopping: bool,
    next_id: u64,
    /// Each connection's socket, shared with its thread: one descriptor a connection.
    open: HashMap<u64, Arc<TcpStream>>,
    /// Connections still sending their head, by peer address (the trusted proxy's left
    /// out).
    heads: HashMap<IpAddr, usize>,
    /// The connections holding a station's reserved place: id to station slug.
    reserved: HashMap<u64, String>,
}

impl Connections {
    /// Whether connection `id`, its head read, fits in the public part of the pool.
    fn public_room(&self, id: u64, public_connections: usize) -> bool {
        let public = self
            .open
            .keys()
            .filter(|open| **open != id && !self.reserved.contains_key(*open))
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
}

impl Shared {
    fn stopping(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// The connection list, even after a thread panicked while holding it: it only maps
    /// ids to sockets.
    fn connections(&self) -> MutexGuard<'_, Connections> {
        self.connections
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Gives an accepted connection its thread, or closes it when stopping, when every
    /// connection thread is taken, or when its address already has as many heads coming
    /// as it may.
    fn admit(self: &Arc<Self>, stream: TcpStream, peer: SocketAddr) {
        let stream = Arc::new(stream);
        let head_from = (self.config.trusted_proxy != Some(peer.ip())).then_some(peer.ip());
        let id = {
            let mut connections = self.connections();
            // Checked under the same lock `stop` closes connections under, so none slips
            // in after it.
            if connections.stopping || connections.open.len() >= self.max_connections {
                return;
            }
            if let Some(addr) = head_from {
                let heads = connections.heads.entry(addr).or_default();
                if *heads >= HEADS_PER_ADDRESS {
                    return;
                }
                *heads += 1;
            }
            let id = connections.next_id;
            connections.next_id += 1;
            connections.open.insert(id, Arc::clone(&stream));
            id
        };
        let guard = ConnectionGuard {
            shared: Arc::clone(self),
            id,
            head_from,
        };
        let spawned = std::thread::Builder::new()
            .name("server-connection".into())
            .spawn(move || {
                let mut guard = guard;
                serve(&mut guard, peer, Conn::new(stream));
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
    /// The address whose head count this connection is in, until its head is read.
    head_from: Option<IpAddr>,
}

impl ConnectionGuard {
    /// The head is read (or failed): the connection no longer counts as one coming.
    fn end_head(&mut self) {
        if let Some(addr) = self.head_from.take() {
            end_head(&mut self.shared.connections(), addr);
        }
    }
}

fn end_head(connections: &mut Connections, addr: IpAddr) {
    if let Some(heads) = connections.heads.get_mut(&addr) {
        *heads = heads.saturating_sub(1);
        if *heads == 0 {
            connections.heads.remove(&addr);
        }
    }
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        {
            let mut connections = self.shared.connections();
            if let Some(addr) = self.head_from.take() {
                end_head(&mut connections, addr);
            }
            connections.open.remove(&self.id);
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

fn serve(guard: &mut ConnectionGuard, peer: SocketAddr, mut stream: Conn) {
    let shared = Arc::clone(&guard.shared);
    let shared = &*shared;
    let head = shared.timeouts.head.max(MIN_TIMEOUT);
    // `read_request` tries a read that times out again until its deadline; without a
    // timeout a read could block past it, and a non-blocking socket would spin.
    let timeouts_set = stream.set_read_timeout(head).is_ok()
        && stream
            .set_write_timeout(shared.timeouts.listener_write.max(MIN_TIMEOUT))
            .is_ok();
    if !timeouts_set {
        return;
    }
    let deadline = Instant::now()
        .checked_add(head)
        .unwrap_or_else(Instant::now);
    let read = read_request(&mut stream, deadline);
    guard.end_head();
    let request = match read {
        Ok(request) => request,
        Err(HttpError::Malformed) => {
            return refuse(&mut stream, 400, "Bad Request", &[], b"bad request");
        }
        Err(HttpError::TooLarge) => {
            return refuse(
                &mut stream,
                431,
                "Request Header Fields Too Large",
                &[],
                b"request head too large",
            );
        }
        // Too slow, or gone: there is no one to answer.
        Err(HttpError::Timeout | HttpError::Io(_)) => return,
    };
    if gate(shared, guard.id, &mut stream, peer.ip(), &request) {
        route(shared, stream, peer.ip(), &request);
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

fn route(shared: &Shared, mut stream: Conn, peer: IpAddr, request: &Request) {
    let method = request.method.as_str();
    match method {
        "GET" | "PUT" | "SOURCE" => {}
        "OPTIONS" => {
            let _ = respond(
                &mut stream,
                204,
                "No Content",
                &[CORS, ("Access-Control-Allow-Methods", "GET, OPTIONS")],
                b"",
            );
            return;
        }
        _ => {
            return refuse(
                &mut stream,
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
    if method == "GET" && request.path == "/status-json.xsl" {
        return stats(shared, stream);
    }
    let Some((station, mount)) = station_at(shared, &request.path) else {
        return refuse(&mut stream, 404, "Not Found", &[CORS], b"not found");
    };
    if method == "GET" {
        listen(shared, stream, mount);
    } else {
        source(shared, stream, peer, request, station, mount);
    }
}

/// The station and mount whose stream is at `path`.
fn station_at<'a>(shared: &'a Shared, path: &str) -> Option<(&'a Station, &'a Arc<Mount>)> {
    let slug = stream_slug(path)?;
    let station = shared.config.station(slug)?;
    let mount = shared.mounts.get(slug)?;
    Some((station, mount))
}

/// The slug of `/<slug>/stream`.
fn stream_slug(path: &str) -> Option<&str> {
    let (slug, rest) = path.strip_prefix('/')?.split_once('/')?;
    (rest == "stream" && !slug.is_empty()).then_some(slug)
}

fn source(
    shared: &Shared,
    mut stream: Conn,
    peer: IpAddr,
    request: &Request,
    station: &Station,
    mount: &Arc<Mount>,
) {
    if !authorized(shared, &mut stream, peer, request, station) {
        return;
    }
    let Some(format) = request
        .header("Content-Type")
        .and_then(Format::from_content_type)
    else {
        return refuse(
            &mut stream,
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
    // Taken before the reply, so a listener who comes the moment the source has its 200
    // finds the source live.
    let Some(mut guard) = mount.begin(format, name) else {
        return refuse(
            &mut stream,
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
            Ok(read) => guard.write(buffer.get(..read).unwrap_or_default()),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            // A read timeout is the source's silence: it is gone.
            Err(_) => break,
        }
    }
    // Dropping the guard ends the stream for every listener and frees the mount.
    drop(guard);
}

/// Checks the station key of a keyed request, and answers the request itself when it is
/// refused. A locked-out address is refused before its key is looked at. A request that
/// presents no `Authorization` header at all is refused without counting toward the
/// lockout, so stray requests from a shared address cannot lock a station out; any
/// header that does not carry the right key (a wrong key, another scheme, or one that
/// does not decode) counts as a wrong key.
fn authorized(
    shared: &Shared,
    stream: &mut Conn,
    peer: IpAddr,
    request: &Request,
    station: &Station,
) -> bool {
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
        return false;
    }
    if request.header("Authorization").is_none() {
        refuse(stream, 401, "Unauthorized", &[CHALLENGE], b"no key");
        return false;
    }
    let right_key = request
        .basic_password()
        .is_some_and(|key| key_matches(&key, &station.key_hash));
    if !right_key {
        shared.lockout.failed(addr, now);
        refuse(stream, 401, "Unauthorized", &[CHALLENGE], b"wrong key");
        return false;
    }
    true
}

/// `GET /admin/metadata?mount=/<slug>/stream&mode=updinfo&song=<title>`: an MP3 source's
/// title, which its stream does not carry.
fn metadata(shared: &Shared, mut stream: Conn, peer: IpAddr, request: &Request) {
    let Some(path) = request.param("mount") else {
        return refuse(
            &mut stream,
            400,
            "Bad Request",
            &[],
            b"the request names no mount",
        );
    };
    let Some((station, mount)) = station_at(shared, path) else {
        return refuse(&mut stream, 404, "Not Found", &[], b"not found");
    };
    if !authorized(shared, &mut stream, peer, request, station) {
        return;
    }
    if request.param("mode") != Some("updinfo") {
        return refuse(
            &mut stream,
            400,
            "Bad Request",
            &[],
            b"only mode=updinfo is supported",
        );
    }
    let Some(song) = request.param("song") else {
        return refuse(
            &mut stream,
            400,
            "Bad Request",
            &[],
            b"the request has no song",
        );
    };
    // The mount cuts it to 512 characters.
    if !mount.set_title(song.to_owned()) {
        return refuse(
            &mut stream,
            400,
            "Bad Request",
            &[],
            b"this station is not on the air",
        );
    }
    let _ = respond(
        &mut stream,
        200,
        "OK",
        &[("Content-Type", "text/xml")],
        METADATA_OK,
    );
}

/// `GET /status-json.xsl`: public and unkeyed, for the listener page's "now playing".
fn stats(shared: &Shared, mut stream: Conn) {
    let mounts: Vec<_> = shared
        .config
        .stations
        .iter()
        .filter_map(|station| {
            let mut stats = shared.mounts.get(&station.slug)?.stats();
            if stats.name.is_empty() {
                stats.name.clone_from(&station.name);
            }
            Some((station.slug.clone(), stats))
        })
        .collect();
    let body = status_json(&shared.config.public_url, &mounts);
    let _ = respond(
        &mut stream,
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

fn listen(shared: &Shared, mut stream: Conn, mount: &Arc<Mount>) {
    let listener = match mount.listen() {
        Ok(listener) => listener,
        Err(ListenError::NoSource) => {
            return refuse(
                &mut stream,
                404,
                "Not Found",
                &[CORS],
                b"this station is not on the air",
            );
        }
        Err(ListenError::Full) => {
            return refuse(
                &mut stream,
                503,
                "Service Unavailable",
                &[CORS],
                b"this station has as many listeners as it allows",
            );
        }
    };
    let head = format!(
        "HTTP/1.0 200 OK\r\nContent-Type: {}\r\nCache-Control: no-cache, no-store\r\n\
         Access-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
        listener.format().content_type()
    );
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    loop {
        match listener.recv(LISTENER_POLL) {
            Received::Chunk(chunk) => {
                if stream.write_all(&chunk).is_err() {
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

/// Answers with an error and closes gently: the write side first, then whatever the
/// client still sends is read for a moment, so its unread bytes don't reset the
/// connection and lose the reply.
fn refuse(stream: &mut Conn, status: u16, reason: &str, headers: &[(&str, &str)], body: &[u8]) {
    if respond(stream, status, reason, headers, body).is_err() {
        return;
    }
    if stream.shutdown_write().is_err() || stream.set_read_timeout(LINGER).is_err() {
        return;
    }
    let until = Instant::now()
        .checked_add(LINGER)
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
