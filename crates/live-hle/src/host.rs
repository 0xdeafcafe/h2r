//! Small Unix socket adapter. Xbox constants and guest pointers never reach C.
//! Keep platform layout/unsafe code here; the rest of the HLE uses owned values.
#![allow(unsafe_code)]
#[cfg(not(any(target_vendor = "apple", target_os = "linux")))]
compile_error!("Live host sockets currently support Apple platforms and Linux");

use std::{
    ffi::{c_int, c_ulong, c_void},
    io,
    mem::size_of,
    net::{Ipv4Addr, SocketAddrV4},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
};

#[repr(C)]
struct Address {
    #[cfg(target_vendor = "apple")]
    len: u8,
    #[cfg(target_vendor = "apple")]
    family: u8,
    #[cfg(target_os = "linux")]
    family: u16,
    port: u16,
    ip: [u8; 4],
    zero: [u8; 8],
}
impl Address {
    fn new(a: SocketAddrV4) -> Self {
        Self {
            #[cfg(target_vendor = "apple")]
            len: 16,
            family: 2,
            port: a.port().to_be(),
            ip: a.ip().octets(),
            zero: [0; 8],
        }
    }
    fn endpoint(&self) -> SocketAddrV4 {
        SocketAddrV4::new(Ipv4Addr::from(self.ip), u16::from_be(self.port))
    }
}
#[repr(C)]
struct PollFd {
    fd: c_int,
    events: i16,
    revents: i16,
}
unsafe extern "C" {
    fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
    fn bind(fd: c_int, address: *const Address, len: u32) -> c_int;
    fn connect(fd: c_int, address: *const Address, len: u32) -> c_int;
    fn send(fd: c_int, data: *const c_void, len: usize, flags: c_int) -> isize;
    fn sendto(
        fd: c_int,
        data: *const c_void,
        len: usize,
        flags: c_int,
        address: *const Address,
        alen: u32,
    ) -> isize;
    fn recvfrom(
        fd: c_int,
        data: *mut c_void,
        len: usize,
        flags: c_int,
        address: *mut Address,
        alen: *mut u32,
    ) -> isize;
    fn setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, len: u32) -> c_int;
    fn getsockopt(fd: c_int, level: c_int, name: c_int, value: *mut c_void, len: *mut u32)
    -> c_int;
    fn shutdown(fd: c_int, how: c_int) -> c_int;
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
    fn fcntl(fd: c_int, command: c_int, ...) -> c_int;
    fn poll(fds: *mut PollFd, count: PollCount, timeout: c_int) -> c_int;
}
#[cfg(target_vendor = "apple")]
type PollCount = u32;
#[cfg(target_os = "linux")]
type PollCount = usize;
#[cfg(target_vendor = "apple")]
const SOL_SOCKET: i32 = 0xffff;
#[cfg(target_os = "linux")]
const SOL_SOCKET: i32 = 1;
#[cfg(target_vendor = "apple")]
const FIONBIO: c_ulong = 0x8004667e;
#[cfg(target_os = "linux")]
const FIONBIO: c_ulong = 0x5421;
#[cfg(target_vendor = "apple")]
const FIONREAD: c_ulong = 0x4004667f;
#[cfg(target_os = "linux")]
const FIONREAD: c_ulong = 0x541b;
#[cfg(target_os = "linux")]
const SEND_FLAGS: i32 = 0x4000; // MSG_NOSIGNAL
#[cfg(target_vendor = "apple")]
const SEND_FLAGS: i32 = 0;

fn cvt(v: i32) -> io::Result<()> {
    if v == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn count(v: isize) -> io::Result<usize> {
    if v == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(v as usize)
    }
}

pub struct Socket {
    fd: OwnedFd,
    pub datagram: bool,
}
impl Socket {
    pub fn new(datagram: bool) -> io::Result<Self> {
        // SAFETY: no pointers; a successful descriptor is owned exactly once.
        let fd = unsafe { socket(2, if datagram { 2 } else { 1 }, 0) };
        if fd == -1 {
            return Err(io::Error::last_os_error());
        }
        let s = Self {
            fd: unsafe { OwnedFd::from_raw_fd(fd) },
            datagram,
        };
        // SAFETY: F_SETFD (2) takes FD_CLOEXEC (1) as an integer on both OSes.
        cvt(unsafe { fcntl(s.fd.as_raw_fd(), 2, 1) })?;
        #[cfg(target_vendor = "apple")]
        s.set_int(SOL_SOCKET, 0x1022, 1)?; // SO_NOSIGPIPE
        Ok(s)
    }
    pub fn bind(&self, a: SocketAddrV4) -> io::Result<()> {
        let a = Address::new(a);
        // SAFETY: a is a live, initialized native sockaddr_in with the given size.
        cvt(unsafe { bind(self.fd.as_raw_fd(), &a, 16) })
    }
    pub fn connect(&self, a: SocketAddrV4) -> io::Result<()> {
        let a = Address::new(a);
        // SAFETY: same as bind; the OS does not retain the pointer.
        cvt(unsafe { connect(self.fd.as_raw_fd(), &a, 16) })
    }
    pub fn nonblocking(&self, on: bool) -> io::Result<()> {
        let mut value = i32::from(on);
        // SAFETY: FIONBIO accepts a pointer to an initialized native int.
        cvt(unsafe { ioctl(self.fd.as_raw_fd(), FIONBIO, &mut value) })
    }
    pub fn available(&self) -> io::Result<u32> {
        if self.datagram {
            // BSD FIONREAD includes queued sockaddr overhead for datagrams.
            // A nonblocking peek gives Winsock's next-datagram payload size.
            if wait(&[(self, 1)], 0)?[0] & 1 == 0 {
                return Ok(0);
            }
            let mut data = vec![0; 65536];
            let flags = 2 | if cfg!(target_vendor = "apple") {
                0x80
            } else {
                0x40
            };
            return match self.recv(&mut data, flags) {
                Ok((n, _)) => Ok(n as u32),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(0),
                Err(e) => Err(e),
            };
        }
        let mut value = 0i32;
        // SAFETY: FIONREAD writes one native int.
        cvt(unsafe { ioctl(self.fd.as_raw_fd(), FIONREAD, &mut value) })?;
        Ok(value.max(0) as u32)
    }
    pub fn send(&self, data: &[u8], target: Option<SocketAddrV4>, flags: i32) -> io::Result<usize> {
        let flags = flags | SEND_FLAGS;
        // SAFETY: data is readable for len; optional address is live throughout C.
        count(unsafe {
            match target {
                Some(a) => {
                    let a = Address::new(a);
                    sendto(
                        self.fd.as_raw_fd(),
                        data.as_ptr().cast(),
                        data.len(),
                        flags,
                        &a,
                        16,
                    )
                }
                None => send(self.fd.as_raw_fd(), data.as_ptr().cast(), data.len(), flags),
            }
        })
    }
    pub fn recv(&self, data: &mut [u8], flags: i32) -> io::Result<(usize, SocketAddrV4)> {
        let mut a = Address::new(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
        let mut len = 16;
        // SAFETY: each output is initialized and writable for its specified size.
        let n = count(unsafe {
            recvfrom(
                self.fd.as_raw_fd(),
                data.as_mut_ptr().cast(),
                data.len(),
                flags,
                &mut a,
                &mut len,
            )
        })?;
        Ok((n, a.endpoint()))
    }
    pub fn shutdown(&self, how: i32) -> io::Result<()> {
        // SAFETY: valid owned fd; how was checked against 0..=2 by the caller.
        cvt(unsafe { shutdown(self.fd.as_raw_fd(), how) })
    }
    fn set_int(&self, level: i32, name: i32, value: i32) -> io::Result<()> {
        // SAFETY: this helper is used only with integer-valued native options.
        cvt(unsafe {
            setsockopt(
                self.fd.as_raw_fd(),
                level,
                name,
                (&value as *const i32).cast(),
                4,
            )
        })
    }
    fn get_int(&self, level: i32, name: i32) -> io::Result<i32> {
        let mut value = 0i32;
        let mut len = 4;
        // SAFETY: integer options return at most four bytes into value.
        cvt(unsafe {
            getsockopt(
                self.fd.as_raw_fd(),
                level,
                name,
                (&mut value as *mut i32).cast(),
                &mut len,
            )
        })?;
        Ok(value)
    }
    pub fn set_option(&self, option: OptionName, value: i32) -> io::Result<()> {
        let (level, name) = option.native();
        self.set_int(level, name, value)
    }
    pub fn get_option(&self, option: OptionName) -> io::Result<i32> {
        let (level, name) = option.native();
        self.get_int(level, name)
    }
    pub fn linger(&self, on: u16, secs: u16) -> io::Result<()> {
        let value = [on as i32, secs as i32];
        let name = if cfg!(target_vendor = "apple") { 0x80 } else { 13 };
        // SAFETY: native linger is two ints on both supported platforms.
        cvt(unsafe {
            setsockopt(
                self.fd.as_raw_fd(),
                SOL_SOCKET,
                name,
                value.as_ptr().cast(),
                size_of::<[i32; 2]>() as u32,
            )
        })
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
impl OptionName {
    fn native(self) -> (i32, i32) {
        use OptionName::*;
        if matches!(self, NoDelay) {
            return (6, 1);
        }
        let name = match self {
            Reuse => {
                if cfg!(target_vendor = "apple") {
                    4
                } else {
                    2
                }
            }
            Broadcast => {
                if cfg!(target_vendor = "apple") {
                    0x20
                } else {
                    6
                }
            }
            SendBuffer => {
                if cfg!(target_vendor = "apple") {
                    0x1001
                } else {
                    7
                }
            }
            ReceiveBuffer => {
                if cfg!(target_vendor = "apple") {
                    0x1002
                } else {
                    8
                }
            }
            Error => {
                if cfg!(target_vendor = "apple") {
                    0x1007
                } else {
                    4
                }
            }
            KeepAlive => {
                if cfg!(target_vendor = "apple") {
                    8
                } else {
                    9
                }
            }
            NoDelay => unreachable!(),
        };
        (SOL_SOCKET, name)
    }
}
/// Returns native poll revents for each socket. No HLE or guest lock is held.
pub fn wait(sockets: &[(&Socket, i16)], millis: i32) -> io::Result<Vec<i16>> {
    let mut fds: Vec<_> = sockets
        .iter()
        .map(|(s, events)| PollFd {
            fd: s.fd.as_raw_fd(),
            events: *events,
            revents: 0,
        })
        .collect();
    // SAFETY: fds is writable for count native entries, including an empty slice.
    cvt(unsafe { poll(fds.as_mut_ptr(), fds.len() as PollCount, millis) })?;
    Ok(fds.into_iter().map(|x| x.revents).collect())
}

/// Explicit errno translation; Unix values must never escape to the Xbox.
pub fn wsa(error: &io::Error) -> u32 {
    use io::ErrorKind::*;
    match error.kind() {
        WouldBlock => 10035,
        Interrupted => 10004,
        PermissionDenied => 10013,
        AddrInUse => 10048,
        AddrNotAvailable => 10049,
        ConnectionRefused => 10061,
        ConnectionReset => 10054,
        ConnectionAborted => 10053,
        NotConnected => 10057,
        TimedOut => 10060,
        BrokenPipe => 10058,
        OutOfMemory => 10055,
        _ => match error.raw_os_error() {
            Some(n) if n == if cfg!(target_vendor = "apple") { 36 } else { 115 } => 10035, // EINPROGRESS
            Some(n) if n == if cfg!(target_vendor = "apple") { 37 } else { 114 } => 10037,
            Some(n) if n == if cfg!(target_vendor = "apple") { 56 } else { 106 } => 10056,
            Some(n) if n == if cfg!(target_vendor = "apple") { 40 } else { 90 } => 10040,
            Some(n) if n == if cfg!(target_vendor = "apple") { 50 } else { 100 } => 10050,
            Some(n) if n == if cfg!(target_vendor = "apple") { 51 } else { 101 } => 10051,
            Some(n) if n == if cfg!(target_vendor = "apple") { 65 } else { 113 } => 10065,
            _ => 10022,
        },
    }
}

#[repr(C)]
struct Timeval {
    seconds: std::ffi::c_long,
    #[cfg(target_vendor = "apple")]
    micros: i32,
    #[cfg(target_os = "linux")]
    micros: std::ffi::c_long,
}
impl Socket {
    pub fn timeout(&self, receive: bool, millis: u32) -> io::Result<()> {
        let value = Timeval {
            seconds: (millis / 1000) as _,
            micros: ((millis % 1000) * 1000) as _,
        };
        let name = if cfg!(target_vendor = "apple") {
            if receive { 0x1006 } else { 0x1005 }
        } else if receive {
            20
        } else {
            21
        };
        // SAFETY: timeout options accept a native timeval with this layout.
        cvt(unsafe {
            setsockopt(
                self.fd.as_raw_fd(),
                SOL_SOCKET,
                name,
                (&value as *const Timeval).cast(),
                size_of::<Timeval>() as u32,
            )
        })
    }
}
