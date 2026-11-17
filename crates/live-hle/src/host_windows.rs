//! Windows stand-in for the socket adapter (host.rs). The Live HLE is opt-in
//! (`h2 play --live`, `--live-server` or `--live-local`) and has no Winsock
//! port yet, so every socket fails to open with WSASYSNOTREADY and the
//! game sees the network as down. The API matches host.rs so network.rs
//! compiles unchanged; a Winsock port (the same calls, `SOCKET` handles, `ioctlsocket`, `WSAPoll`, DWORD timeouts)
//! replaces this file.
use std::{io, net::SocketAddrV4};

/// What the guest sees when it asks for a socket here.
const WSASYSNOTREADY: u32 = 10091;

fn unsupported<T>() -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "Live sockets are not ported to Windows yet"))
}

pub struct Socket {
    pub datagram: bool,
    never: std::convert::Infallible,
}
impl Socket {
    pub fn new(_datagram: bool) -> io::Result<Self> {
        unsupported()
    }
    pub fn bind(&self, _a: SocketAddrV4) -> io::Result<()> {
        match self.never {}
    }
    pub fn connect(&self, _a: SocketAddrV4) -> io::Result<()> {
        match self.never {}
    }
    pub fn nonblocking(&self, _on: bool) -> io::Result<()> {
        match self.never {}
    }
    pub fn available(&self) -> io::Result<u32> {
        match self.never {}
    }
    pub fn send(&self, _data: &[u8], _target: Option<SocketAddrV4>, _flags: i32) -> io::Result<usize> {
        match self.never {}
    }
    pub fn recv(&self, _data: &mut [u8], _flags: i32) -> io::Result<(usize, SocketAddrV4)> {
        match self.never {}
    }
    pub fn shutdown(&self, _how: i32) -> io::Result<()> {
        match self.never {}
    }
    pub fn set_option(&self, _option: OptionName, _value: i32) -> io::Result<()> {
        match self.never {}
    }
    pub fn get_option(&self, _option: OptionName) -> io::Result<i32> {
        match self.never {}
    }
    pub fn linger(&self, _on: u16, _secs: u16) -> io::Result<()> {
        match self.never {}
    }
    pub fn timeout(&self, _receive: bool, _millis: u32) -> io::Result<()> {
        match self.never {}
    }
}

#[derive(Clone, Copy)]
pub enum OptionName {
    Reuse,
    Broadcast,
    SendBuffer,
    ReceiveBuffer,
    Error,
    NoDelay,
    KeepAlive,
}

pub fn wait(sockets: &[(&Socket, i16)], _millis: i32) -> io::Result<Vec<i16>> {
    match sockets.first() {
        Some((s, _)) => match s.never {},
        None => Ok(Vec::new()),
    }
}

/// A Winsock error for the guest. Native Windows errors already are one.
pub fn wsa(error: &io::Error) -> u32 {
    match error.raw_os_error() {
        Some(n @ 10000..=11999) => n as u32,
        _ if error.kind() == io::ErrorKind::Unsupported => WSASYSNOTREADY,
        _ => 10022,
    }
}
