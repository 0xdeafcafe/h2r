//! Winsock's pathname AF_UNIX transport (Windows 10 1803 and later).

use socket2::{Domain, SockAddr, Socket, Type};
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub struct UnixListener(Socket);

impl UnixListener {
    pub fn bind(path: &Path) -> io::Result<Self> {
        let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        socket.bind(&SockAddr::unix(path)?)?;
        socket.listen(128)?;
        Ok(Self(socket))
    }

    pub fn incoming(&self) -> impl Iterator<Item = io::Result<UnixStream>> + '_ {
        std::iter::repeat_with(|| self.0.accept().map(|(s, _)| UnixStream(Arc::new(s))))
    }
}

/// Independent blocking reader and writer owners share one connected socket.
pub struct UnixStream(Arc<Socket>);

impl UnixStream {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        socket.connect(&SockAddr::unix(path)?)?;
        Ok(Self(Arc::new(socket)))
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(Self(Arc::clone(&self.0)))
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_read_timeout(timeout)
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        self.0.set_write_timeout(timeout)
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        self.0.shutdown(how)
    }
}

impl Read for UnixStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0.as_ref().read(buf)
    }
}

impl Write for UnixStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.as_ref().write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
