//! The server: a TCP port, one thread per connection, and the routes of a mount — a
//! station's keyed source push in, its public listeners out, and the keyed title update
//! an MP3 source sends as it would to Icecast — and the public stats of every live mount.

use crate::ServerError;
use crate::config::{Config, Station};
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
/// Connection threads beyond the listener caps and two per station (a source and a
/// spare), for requests that are not streams.
const SPARE_CONNECTIONS: usize = 32;
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
        let max_connections = config
            .stations
            .iter()
            .fold(SPARE_CONNECTIONS, |sum, station| {
                sum.saturating_add(station.max_stream_listeners)
                    .saturating_add(2)
            });
        let shared = Arc::new(Shared {
            config,
            timeouts,
            mounts,
            lockout: Lockout::default(),
            stopping: AtomicBool::new(false),
            connections: Mutex::default(),
            finished: Condvar::new(),
            max_connections,
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
    max_connections: usize,
}

/// The open connections, so `stop` can close them.
#[derive(Default)]
struct Connections {
    stopping: bool,
    next_id: u64,
    open: HashMap<u64, TcpStream>,
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

    /// Gives an accepted connection its thread, or closes it when stopping or when every
    /// connection thread is taken.
    fn admit(self: &Arc<Self>, stream: TcpStream) {
        let id = {
            let mut connections = self.connections();
            // Checked under the same lock `stop` closes connections under, so none slips
            // in after it.
            if connections.stopping || connections.open.len() >= self.max_connections {
                return;
            }
            let Ok(clone) = stream.try_clone() else {
                return;
            };
            let id = connections.next_id;
            connections.next_id += 1;
            connections.open.insert(id, clone);
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
                serve(&guard.shared, stream);
            });
        // A thread that could not start drops its closure, and with it the guard.
        drop(spawned);
    }
}

/// Removes a connection from the list when its thread ends, closing the list's copy of
/// the socket.
struct ConnectionGuard {
    shared: Arc<Shared>,
    id: u64,
}

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.shared.connections().open.remove(&self.id);
        self.shared.finished.notify_all();
    }
}

fn accept_loop(shared: &Arc<Shared>, listener: TcpListener) {
    for incoming in listener.incoming() {
        if shared.stopping() {
            break;
        }
        match incoming {
            Ok(stream) => shared.admit(stream),
            Err(_) => std::thread::sleep(ACCEPT_BACKOFF),
        }
    }
}

fn serve(shared: &Shared, mut stream: TcpStream) {
    let Ok(peer) = stream.peer_addr() else {
        return;
    };
    let head = shared.timeouts.head.max(MIN_TIMEOUT);
    // `read_request` tries a read that times out again until its deadline; without a
    // timeout a read could block past it, and a non-blocking socket would spin.
    let timeouts_set = stream.set_read_timeout(Some(head)).is_ok()
        && stream
            .set_write_timeout(Some(shared.timeouts.listener_write.max(MIN_TIMEOUT)))
            .is_ok();
    if !timeouts_set {
        return;
    }
    let deadline = Instant::now()
        .checked_add(head)
        .unwrap_or_else(Instant::now);
    let request = match read_request(&mut stream, deadline) {
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
    route(shared, stream, peer.ip(), &request);
}

fn route(shared: &Shared, mut stream: TcpStream, peer: IpAddr, request: &Request) {
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
    mut stream: TcpStream,
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
            .set_read_timeout(Some(shared.timeouts.source_silence.max(MIN_TIMEOUT)))
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
    stream: &mut TcpStream,
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
fn metadata(shared: &Shared, mut stream: TcpStream, peer: IpAddr, request: &Request) {
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
fn stats(shared: &Shared, mut stream: TcpStream) {
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

fn listen(shared: &Shared, mut stream: TcpStream, mount: &Arc<Mount>) {
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
fn refuse(
    stream: &mut TcpStream,
    status: u16,
    reason: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) {
    if respond(stream, status, reason, headers, body).is_err() {
        return;
    }
    if stream.shutdown(Shutdown::Write).is_err() || stream.set_read_timeout(Some(LINGER)).is_err() {
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
