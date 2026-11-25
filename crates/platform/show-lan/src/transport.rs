use std::io;
use std::net::{SocketAddr, UdpSocket};

/// What the carrier rides: a datagram socket that never blocks.
pub trait Datagrams {
    fn send_to(&self, bytes: &[u8], to: SocketAddr) -> io::Result<()>;
    /// The next waiting datagram, or none.
    fn recv_from(&self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>>;
}

/// A bound socket, set nonblocking by the caller.
impl Datagrams for UdpSocket {
    fn send_to(&self, bytes: &[u8], to: SocketAddr) -> io::Result<()> {
        UdpSocket::send_to(self, bytes, to).map(|_| ())
    }

    fn recv_from(&self, buf: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
        match UdpSocket::recv_from(self, buf) {
            Ok(got) => Ok(Some(got)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
}
