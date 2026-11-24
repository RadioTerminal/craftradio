//! A connection: plain TCP, or server TLS over it when the config names a certificate.
//!
//! Its socket is shared with the list `stop` walks, so that each connection holds one file
//! descriptor, not a socket and a clone of it; `stop` only ever shuts that socket, never
//! writes TLS on it.

use crate::ServerError;
use rustls::pki_types::pem::{self, PemObject};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a clean end waits to send TLS `close_notify`.
const CLOSE_NOTIFY_WAIT: Duration = Duration::from_millis(500);

/// The TLS settings for `cert` and `key` (PEM files), with the ring provider. An error
/// names the file and never carries what is in it.
pub(crate) fn server_config(cert: &Path, key: &Path) -> Result<Arc<ServerConfig>, ServerError> {
    let certs = CertificateDer::pem_file_iter(cert)
        .and_then(|certs| certs.collect::<Result<Vec<_>, _>>())
        .map_err(|error| pem_error(cert, "certificate", &error))?;
    if certs.is_empty() {
        return Err(pem_error(cert, "certificate", &pem::Error::NoItemsFound));
    }
    let key_der =
        PrivateKeyDer::from_pem_file(key).map_err(|error| pem_error(key, "private key", &error))?;
    let config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|error| ServerError::Tls(format!("no TLS version to offer: {error}")))?
            .with_no_client_auth()
            .with_single_cert(certs, key_der)
            .map_err(|error| {
                ServerError::Tls(format!(
                    "{} and {} are not a usable certificate and key: {error}",
                    cert.display(),
                    key.display()
                ))
            })?;
    Ok(Arc::new(config))
}

/// Says what is wrong with a PEM file in fixed words: the parser's own messages can quote
/// a line of the file, which in a key file is key material.
fn pem_error(path: &Path, what: &str, error: &pem::Error) -> ServerError {
    let path = path.display();
    ServerError::Tls(match error {
        pem::Error::Io(error) => format!("{path}: cannot read the {what}: {error}"),
        pem::Error::NoItemsFound => format!("{path}: holds no {what}"),
        _ => format!("{path}: the {what} file is not valid PEM"),
    })
}

/// The socket under a connection. Until its head is read, every read and write is held to
/// the head's deadline, the TLS handshake's included, however slowly the bytes come.
pub(crate) struct Socket {
    tcp: Arc<TcpStream>,
    deadline: Option<Instant>,
    /// The write side is shut: the reply has been sent in full.
    write_shut: bool,
}

impl Socket {
    /// What is left before the deadline, if there is one; an error once it has passed.
    fn left(&self) -> std::io::Result<Option<Duration>> {
        let Some(deadline) = self.deadline else {
            return Ok(None);
        };
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(std::io::Error::new(
                ErrorKind::TimedOut,
                "request head deadline",
            ));
        }
        Ok(Some(left))
    }
}

impl Read for Socket {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(left) = self.left()? {
            self.tcp.set_read_timeout(Some(left))?;
        }
        (&*self.tcp).read(buf)
    }
}

impl Write for Socket {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Some(left) = self.left()? {
            self.tcp.set_write_timeout(Some(left))?;
        }
        (&*self.tcp).write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        (&*self.tcp).flush()
    }
}

/// One accepted connection, as its thread reads and writes it.
pub(crate) enum Conn {
    Plain(Socket),
    Tls(Box<StreamOwned<ServerConnection, Socket>>),
}

impl Conn {
    /// `tcp` is the same socket the connection list holds; its head must be read by
    /// `deadline`.
    pub(crate) fn plain(tcp: Arc<TcpStream>, deadline: Instant) -> Conn {
        Conn::Plain(Socket {
            tcp,
            deadline: Some(deadline),
            write_shut: false,
        })
    }

    /// Completes the TLS handshake by `deadline`, which the head that follows must meet
    /// too.
    pub(crate) fn tls(
        tcp: Arc<TcpStream>,
        deadline: Instant,
        config: &Arc<ServerConfig>,
    ) -> std::io::Result<Conn> {
        let mut socket = Socket {
            tcp,
            deadline: Some(deadline),
            write_shut: false,
        };
        let mut connection =
            ServerConnection::new(Arc::clone(config)).map_err(std::io::Error::other)?;
        while connection.is_handshaking() {
            connection.complete_io(&mut socket)?;
        }
        Ok(Conn::Tls(Box::new(StreamOwned::new(connection, socket))))
    }

    fn socket(&self) -> &Socket {
        match self {
            Conn::Plain(socket) => socket,
            Conn::Tls(tls) => &tls.sock,
        }
    }

    fn socket_mut(&mut self) -> &mut Socket {
        match self {
            Conn::Plain(socket) => socket,
            Conn::Tls(tls) => &mut tls.sock,
        }
    }

    /// The head is read: from now on the socket's own timeouts apply.
    pub(crate) fn end_head(&mut self, read: Duration, write: Duration) -> std::io::Result<()> {
        let socket = self.socket_mut();
        socket.deadline = None;
        socket.tcp.set_read_timeout(Some(read))?;
        socket.tcp.set_write_timeout(Some(write))
    }

    pub(crate) fn set_read_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.socket().tcp.set_read_timeout(Some(timeout))
    }

    /// Ends the write side: the client reads the end of the reply (after TLS
    /// `close_notify`, so a TLS client sees a clean end, not a cut connection).
    pub(crate) fn shutdown_write(&mut self) -> std::io::Result<()> {
        if let Conn::Tls(tls) = self {
            tls.conn.send_close_notify();
            tls.flush()?;
        }
        let socket = self.socket_mut();
        socket.tcp.shutdown(Shutdown::Write)?;
        socket.write_shut = true;
        Ok(())
    }

    /// Whether [`Conn::shutdown_write`] has ended the write side.
    pub(crate) fn write_shut(&self) -> bool {
        self.socket().write_shut
    }
}

impl Drop for Conn {
    /// Ends a TLS connection cleanly, waiting only briefly; once `stop` has shut the
    /// socket the write fails at once.
    fn drop(&mut self) {
        if let Conn::Tls(tls) = self {
            tls.sock.deadline = None;
            if tls
                .sock
                .tcp
                .set_write_timeout(Some(CLOSE_NOTIFY_WAIT))
                .is_ok()
            {
                tls.conn.send_close_notify();
                let _ = tls.flush();
            }
        }
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(socket) => socket.read(buf),
            Conn::Tls(tls) => tls.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(socket) => socket.write(buf),
            Conn::Tls(tls) => tls.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Plain(socket) => socket.flush(),
            Conn::Tls(tls) => tls.flush(),
        }
    }
}
