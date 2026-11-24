//! A connection's socket, shared with the list `stop` walks so that each connection
//! holds one file descriptor, not a socket and a clone of it.

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Arc;
use std::time::Duration;

/// One accepted connection, as its thread reads and writes it.
pub(crate) struct Conn {
    tcp: Arc<TcpStream>,
}

impl Conn {
    /// `tcp` is the same socket the connection list holds.
    pub(crate) fn new(tcp: Arc<TcpStream>) -> Conn {
        Conn { tcp }
    }

    pub(crate) fn set_read_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.tcp.set_read_timeout(Some(timeout))
    }

    pub(crate) fn set_write_timeout(&self, timeout: Duration) -> std::io::Result<()> {
        self.tcp.set_write_timeout(Some(timeout))
    }

    /// Ends the write side: the client reads the end of the reply.
    pub(crate) fn shutdown_write(&mut self) -> std::io::Result<()> {
        self.tcp.shutdown(Shutdown::Write)
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        (&*self.tcp).read(buf)
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        (&*self.tcp).write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        (&*self.tcp).flush()
    }
}
