//! Guest Winsock and secure-address translation. Copy inputs/check outputs under
//! the guest lock, then release it before any host wait or network operation.
pub(crate) mod film;
use crate::{NetworkConfig, host, qos, qos_memory::QosMemory, traffic};
use runtime::{Runtime, guest::Access};
use std::{
    collections::BTreeMap,
    io::{self, Read},
    net::{Ipv4Addr, SocketAddrV4},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Socket,
    Bind,
    ConnectSocket,
    Send,
    SendTo,
    Recv,
    RecvFrom,
    Select,
    FdIsSet,
    Ioctl,
    SetOption,
    GetOption,
    Shutdown,
    Close,
    LastError,
    Htonl,
    Htons,
    XnToIn,
    InToXn,
    CreateKey,
    RegisterKey,
    UnregisterKey,
    UnregisterAddress,
    ConnectAddress,
    ConnectStatus,
    Random,
    QosListen,
    QosLookup,
}
impl Op {
    fn winsock(self) -> bool {
        use Op::*;
        matches!(
            self,
            Socket
                | Bind
                | ConnectSocket
                | Send
                | SendTo
                | Recv
                | RecvFrom
                | Select
                | FdIsSet
                | Ioctl
                | SetOption
                | GetOption
                | Shutdown
                | Close
        )
    }
}
type Result<T> = std::result::Result<T, u32>;
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn host<T>(r: io::Result<T>) -> Result<T> {
    r.map_err(|e| host::wsa(&e))
}
fn read(rt: &Runtime, p: u32, size: usize) -> Result<Vec<u8>> {
    if size > 16 * 1024 * 1024 {
        return Err(10055);
    }
    if p == 0 && size != 0 {
        return Err(10014);
    }
    let m = rt.memory();
    m.check(p, size, Access::Read, 1).map_err(|_| 10014u32)?;
    let mut data = vec![0; size];
    m.read_bytes(p, &mut data, 1).map_err(|_| 10014u32)?;
    Ok(data)
}
fn writable(rt: &Runtime, p: u32, size: usize) -> Result<()> {
    if p == 0 && size != 0 {
        return Err(10014);
    }
    rt.memory()
        .check(p, size, Access::Write, 1)
        .map_err(|_| 10014)
}
fn write(rt: &Runtime, p: u32, data: &[u8]) -> Result<()> {
    if p == 0 && !data.is_empty() {
        return Err(10014);
    }
    rt.memory().write_bytes(p, data, 1).map_err(|_| 10014)
}
fn word(rt: &Runtime, p: u32) -> Result<u32> {
    Ok(u32::from_le_bytes(read(rt, p, 4)?.try_into().unwrap()))
}
fn put_word(rt: &Runtime, p: u32, v: u32) -> Result<()> {
    write(rt, p, &v.to_le_bytes())
}
fn sockaddr(rt: &Runtime, p: u32, size: u32) -> Result<SocketAddrV4> {
    if (size as i32) < 16 {
        return Err(10014);
    }
    let data = read(rt, p, 16)?;
    if u16::from_le_bytes(data[..2].try_into().unwrap()) != 2 {
        return Err(10047);
    }
    Ok(SocketAddrV4::new(
        Ipv4Addr::new(data[4], data[5], data[6], data[7]),
        u16::from_be_bytes([data[2], data[3]]),
    ))
}
fn address_bytes(a: SocketAddrV4) -> [u8; 16] {
    let mut b = [0; 16];
    b[..2].copy_from_slice(&2u16.to_le_bytes());
    b[2..4].copy_from_slice(&a.port().to_be_bytes());
    b[4..8].copy_from_slice(&a.ip().octets());
    b
}
pub(crate) fn xnaddr(a: SocketAddrV4) -> [u8; 36] {
    let mut b = [0; 36];
    b[..4].copy_from_slice(&a.ip().octets());
    b[4..8].copy_from_slice(&a.ip().octets());
    b[8..10].copy_from_slice(&a.port().to_be_bytes());
    // Synthetic locally administered identity, unique across local instances.
    b[10] = 2;
    let ip = a.ip().octets();
    b[11] = ip[0] ^ ip[1];
    b[12..14].copy_from_slice(&ip[2..4]);
    b[14..16].copy_from_slice(&a.port().to_be_bytes());
    b
}
fn endpoint(xna: &[u8; 36]) -> Result<SocketAddrV4> {
    let ip = Ipv4Addr::new(xna[0], xna[1], xna[2], xna[3]);
    let port = u16::from_be_bytes([xna[8], xna[9]]);
    if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() || port == 0 || port > 65529 {
        return Err(10049);
    }
    Ok(SocketAddrV4::new(ip, port))
}
fn ip_word(ip: Ipv4Addr) -> u32 {
    u32::from_le_bytes(ip.octets())
}
#[derive(Clone)]
struct Peer {
    xna: [u8; 36],
    kid: [u8; 8],
    connected: bool,
}
struct Socket {
    host: host::Socket,
    protocol: u32,
    options: Mutex<BTreeMap<(u32, u32), Vec<u8>>>,
    guest_port: Mutex<u16>,
    pending: Mutex<Option<u32>>,
    nonblocking: AtomicBool,
}
#[derive(Default)]
struct State {
    net: u32,
    wsa: u32,
    next: u32,
    sockets: BTreeMap<u32, Arc<Socket>>,
    errors: BTreeMap<u32, u32>,
    keys: BTreeMap<[u8; 8], [u8; 16]>,
    peers: BTreeMap<u32, Peer>,
    qos: QosMemory,
    responder: Option<qos::Responder>,
    /// Game traffic already said in the log: (peer IN_ADDR, guest port,
    /// sent).
    traffic: std::collections::BTreeSet<(u32, u16, bool)>,
}
pub(crate) struct Network {
    pub config: NetworkConfig,
    pub tap: std::sync::OnceLock<Arc<dyn ::film::tape::Tap>>,
    state: Mutex<State>,
    listeners: qos::Listeners,
    pings: qos::Pings,
    asked: qos::Asked,
    /// The last QoS lookup said in the log, so a title that repeats one
    /// (a browser refreshing) logs it once.
    said: Mutex<String>,
}
pub(crate) struct Cleanup {
    sockets: BTreeMap<u32, Arc<Socket>>,
    responder: Option<qos::Responder>,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        for socket in self.sockets.values() {
            let _ = socket.host.shutdown(2);
        }
        drop(self.responder.take());
    }
}
impl Network {
    pub fn new(config: NetworkConfig) -> Self {
        Self {
            config,
            tap: Default::default(),
            state: Mutex::new(State {
                next: 0x100,
                ..State::default()
            }),
            listeners: Arc::default(),
            pings: Arc::default(),
            asked: Arc::default(),
            said: Mutex::default(),
        }
    }
    /// Update transport references while the caller still owns the sign-in
    /// state lock. This preserves lifecycle order between guest threads. Return
    /// resources to destroy only after all guest/HLE locks have been released.
    pub fn lifecycle(&self, op: crate::entries::Operation) -> Option<Cleanup> {
        use crate::entries::Operation::*;
        let mut s = lock(&self.state);
        match op {
            NetStartup => s.net = s.net.saturating_add(1),
            WsaStartup => s.wsa = s.wsa.saturating_add(1),
            NetCleanup => s.net = s.net.saturating_sub(1),
            WsaCleanup => s.wsa = s.wsa.saturating_sub(1),
            _ => return None,
        }
        if (op == NetCleanup && s.net == 0) || (op == WsaCleanup && s.wsa == 0) {
            if op == NetCleanup {
                s.keys.clear();
                s.peers.clear();
                s.traffic.clear();
                s.qos.clear();
                lock(&self.listeners).clear();
            }
            Some(Cleanup {
                sockets: std::mem::take(&mut s.sockets),
                responder: if op == NetCleanup {
                    s.responder.take()
                } else {
                    None
                },
            })
        } else {
            None
        }
    }
    /// The XNKIDs registered with `XNetRegisterKey`, in key order.
    /// Starts the QoS responder if the title has not yet (a co-op host
    /// answers its guest's pings from the lobby on).
    pub fn ensure_responder(&self) -> std::io::Result<()> {
        if self.tap.get().is_some_and(|t| t.replaying()) {
            return Ok(());
        }
        let mut s = lock(&self.state);
        if s.responder.is_none() {
            s.responder = Some(qos::Responder::start(
                self.config.address,
                self.listeners.clone(),
                self.pings.clone(),
                self.asked.clone(),
            )?);
        }
        Ok(())
    }
    /// The round trips pinging peers reported within `fresh`, by address.
    pub fn reported_pings(&self, fresh: std::time::Duration) -> Vec<(Ipv4Addr, u16)> {
        lock(&self.pings)
            .iter()
            .filter(|(_, (_, at))| at.elapsed() <= fresh)
            .map(|(a, (ms, _))| (*a, *ms))
            .collect()
    }
    pub fn registered_keys(&self) -> Vec<[u8; 8]> {
        lock(&self.state).keys.keys().copied().collect()
    }
    pub fn set_error(&self, thread: u32, error: u32) {
        lock(&self.state).errors.insert(thread, error);
    }
    pub fn release_qos(&self, handle: u32) -> bool {
        lock(&self.state).qos.release(handle)
    }
    pub fn invoke(&self, op: Op, a: &[u32], thread: u32, rt: &Runtime) -> u32 {
        let result = match self.tap.get() {
            Some(tap) => self.filmed(tap.as_ref(), op, a, thread, rt),
            None => self.run(op, a, thread, rt),
        };
        match result {
            Ok(v) => v,
            Err(e) if op.winsock() => {
                self.set_error(thread, e);
                u32::MAX
            }
            Err(e) => e,
        }
    }
    fn socket(&self, id: u32) -> Result<Arc<Socket>> {
        lock(&self.state).sockets.get(&id).cloned().ok_or(10038)
    }
    fn target(&self, a: SocketAddrV4) -> SocketAddrV4 {
        let s = lock(&self.state);
        match s
            .peers
            .get(&ip_word(*a.ip()))
            .and_then(|p| endpoint(&p.xna).ok())
        {
            Some(peer) => SocketAddrV4::new(
                *peer.ip(),
                NetworkConfig::channel_port(peer.port(), a.port()),
            ),
            None => a,
        }
    }
    /// The IN_ADDR the title knows the peer whose XNet base is `remote`
    /// by: the one it has; else its IP, unless that is this console's own
    /// or another peer's (games on one Mac share an IP: 31000, 31010,
    /// 31020 on 10.0.3.7), when one is made from the port
    /// ([`NetworkConfig::shared_ip_in_addr`]). Peers are told apart by
    /// IP and port, so each game on a Mac is a player of its own.
    fn in_addr(&self, peers: &BTreeMap<u32, Peer>, remote: SocketAddrV4) -> u32 {
        if let Some((ina, _)) = peers
            .iter()
            .find(|(_, p)| endpoint(&p.xna).ok() == Some(remote))
        {
            return *ina;
        }
        let real = ip_word(*remote.ip());
        if remote.ip() != self.config.address.ip() && !peers.contains_key(&real) {
            return real;
        }
        ip_word(NetworkConfig::shared_ip_in_addr(remote.port()))
    }
    fn source(&self, mut a: SocketAddrV4, socket: &Socket) -> SocketAddrV4 {
        let guest_port = *lock(&socket.guest_port);
        if (1000..=1006).contains(&guest_port) {
            let offset = guest_port - 1000;
            if let Some(base) = a
                .port()
                .checked_sub(offset)
                .filter(|p| *p > 0 && *p <= 65529)
            {
                let mut s = lock(&self.state);
                let from = SocketAddrV4::new(*a.ip(), base);
                let ina = self.in_addr(&s.peers, from);
                // A peer heard first on a game channel is joining the
                // session it asked QoS about: the title asks for its key
                // (XNetInAddrToXnAddr) to tell its own session's players.
                let asked = lock(&self.asked).get(a.ip()).copied().unwrap_or([0; 8]);
                let peer = s.peers.entry(ina).or_insert_with(|| Peer {
                    xna: xnaddr(from),
                    kid: [0; 8],
                    connected: false,
                });
                if peer.kid == [0; 8] {
                    peer.kid = asked;
                }
                let kid = peer.kid;
                a = SocketAddrV4::new(Ipv4Addr::from(ina.to_le_bytes()), guest_port);
                if s.traffic.insert((ina, guest_port, false)) {
                    eprintln!(
                        "live: xnet: first packet from {from} (as {}) on port {guest_port}, \
                         session {}",
                        a.ip(),
                        hex(&kid)
                    );
                }
            }
        }
        a
    }
    fn run(&self, op: Op, a: &[u32], thread: u32, rt: &Runtime) -> Result<u32> {
        use Op::*;
        match op {
            LastError => return Ok(*lock(&self.state).errors.get(&thread).unwrap_or(&0)),
            Htonl => return Ok(a[0].swap_bytes()),
            Htons => return Ok((a[0] as u16).swap_bytes() as u32),
            FdIsSet => return Ok(u32::from(fdset(rt, a[1])?.contains(&a[0]))),
            _ => {}
        }
        {
            let s = lock(&self.state);
            if s.net == 0 || (op.winsock() && s.wsa == 0) {
                return if op == ConnectStatus {
                    Ok(3)
                } else {
                    Err(10093)
                };
            }
        }
        match op {
            Socket => {
                if a[0] != 2 {
                    return Err(10047);
                }
                let datagram = match a[1] {
                    1 => false,
                    2 => true,
                    _ => return Err(10044),
                };
                if !matches!((datagram, a[2]), (true, 0 | 17 | 254) | (false, 0 | 6)) {
                    return Err(10043);
                }
                let native = host(host::Socket::new(datagram))?;
                if datagram && traffic::low_delay() {
                    // Best effort: a refusal leaves the default class.
                    let _ = native.low_delay();
                }
                let socket = Arc::new(self::Socket {
                    host: native,
                    protocol: a[2],
                    options: Mutex::default(),
                    guest_port: Mutex::new(0),
                    pending: Mutex::new(None),
                    nonblocking: AtomicBool::new(false),
                });
                let mut s = lock(&self.state);
                if s.sockets.len() >= 256 {
                    return Err(10024);
                }
                let id = s.next;
                s.next = s
                    .next
                    .checked_add(1)
                    .filter(|p| *p != u32::MAX)
                    .ok_or(10024u32)?;
                s.sockets.insert(id, socket);
                Ok(id)
            }
            Close => {
                let socket = lock(&self.state).sockets.remove(&a[0]).ok_or(10038u32)?;
                // The Arc keeps the fd valid for an in-flight operation. Shutdown
                // wakes a blocked reader before its final Arc closes the fd.
                let _ = socket.host.shutdown(2);
                Ok(0)
            }
            Bind => {
                let socket = self.socket(a[0])?;
                let address = sockaddr(rt, a[1], a[2])?;
                let actual = self.config.local(address);
                host(socket.host.bind(actual))?;
                *lock(&socket.guest_port) = address.port();
                Ok(0)
            }
            ConnectSocket => {
                let socket = self.socket(a[0])?;
                let target = self.target(sockaddr(rt, a[1], a[2])?);
                let result = host(socket.host.connect(target));
                if matches!(result, Err(10035 | 10037)) {
                    *lock(&socket.pending) = Some(0);
                }
                result.map(|()| 0)
            }
            Shutdown => {
                let socket = self.socket(a[0])?;
                if a[1] > 2 {
                    return Err(10022);
                }
                host(socket.host.shutdown(a[1] as i32))?;
                Ok(0)
            }
            Send | SendTo => self.send(op, a, rt),
            Recv | RecvFrom => self.recv(op, a, rt),
            Select => self.select(a, rt),
            FdIsSet => Ok(u32::from(fdset(rt, a[1])?.contains(&a[0]))),
            Ioctl => {
                let socket = self.socket(a[0])?;
                match a[1] {
                    0x8004667e => {
                        let value = word(rt, a[2])?;
                        host(socket.host.nonblocking(value != 0))?;
                        socket.nonblocking.store(value != 0, Ordering::Release);
                    }
                    0x4004667f => {
                        writable(rt, a[2], 4)?;
                        put_word(rt, a[2], host(socket.host.available())?)?;
                    }
                    _ => return Err(10022),
                };
                Ok(0)
            }
            SetOption | GetOption => self.option(op, a, rt),
            Random => {
                if a[1] > 1024 * 1024 {
                    return Err(10022);
                }
                writable(rt, a[0], a[1] as usize)?;
                write(rt, a[0], &random(a[1] as usize)?)?;
                Ok(0)
            }
            CreateKey => {
                writable(rt, a[0], 8)?;
                writable(rt, a[1], 16)?;
                let mut bytes = random(24)?;
                bytes[0] &= 0x0f; // SYSTEM_LINK
                write(rt, a[0], &bytes[..8])?;
                write(rt, a[1], &bytes[8..])?;
                Ok(0)
            }
            RegisterKey => {
                let kid = read(rt, a[0], 8)?.try_into().unwrap();
                let key = read(rt, a[1], 16)?.try_into().unwrap();
                let mut s = lock(&self.state);
                if s.keys.contains_key(&kid) {
                    return Err(10048);
                }
                if s.keys.len() >= 64 {
                    return Err(10055);
                }
                s.keys.insert(kid, key);
                eprintln!("live: xnet: key for session {} registered", hex(&kid));
                Ok(0)
            }
            UnregisterKey => {
                let kid: [u8; 8] = read(rt, a[0], 8)?.try_into().unwrap();
                let mut s = lock(&self.state);
                if s.keys.remove(&kid).is_none() {
                    return Err(10022);
                }
                s.peers.retain(|_, p| p.kid != kid);
                lock(&self.listeners).remove(&kid);
                eprintln!("live: xnet: key for session {} unregistered", hex(&kid));
                Ok(0)
            }
            XnToIn => {
                let xna: [u8; 36] = read(rt, a[0], 36)?.try_into().unwrap();
                let remote = endpoint(&xna)?;
                let kid: [u8; 8] = read(rt, a[1], 8)?.try_into().unwrap();
                writable(rt, a[2], 4)?;
                let mut s = lock(&self.state);
                let ip = self.in_addr(&s.peers, remote);
                if !s.keys.contains_key(&kid) {
                    eprintln!(
                        "live: join: {remote} for session {}: no key registered for it",
                        hex(&kid)
                    );
                    return Err(10022);
                }
                if !s.peers.contains_key(&ip) {
                    eprintln!(
                        "live: join: {remote} for session {}: address made ({})",
                        hex(&kid),
                        Ipv4Addr::from(ip.to_le_bytes())
                    );
                }
                if let Some(old) = s.peers.get(&ip)
                    && endpoint(&old.xna).ok().is_some_and(|old| old != remote)
                {
                    return Err(10048);
                }
                s.peers.insert(
                    ip,
                    Peer {
                        xna,
                        kid,
                        connected: false,
                    },
                );
                drop(s);
                put_word(rt, a[2], ip)?;
                Ok(0)
            }
            InToXn => {
                writable(rt, a[1], 36)?;
                if a[2] != 0 {
                    writable(rt, a[2], 8)?;
                }
                let p = lock(&self.state).peers.get(&a[0]).cloned();
                let Some(p) = p else {
                    eprintln!(
                        "live: xnet: the game asked about {}, which is no peer",
                        Ipv4Addr::from(a[0].to_le_bytes())
                    );
                    return Err(10022);
                };
                write(rt, a[1], &p.xna)?;
                if a[2] != 0 {
                    write(rt, a[2], &p.kid)?;
                }
                Ok(0)
            }
            UnregisterAddress => {
                if lock(&self.state).peers.remove(&a[0]).is_none() {
                    return Err(10022);
                }
                Ok(0)
            }
            ConnectAddress => {
                let mut s = lock(&self.state);
                let p = s.peers.get_mut(&a[0]).ok_or(10022u32)?;
                if !p.connected {
                    eprintln!(
                        "live: join: connecting to {} for session {}",
                        endpoint(&p.xna).map_or_else(|_| "?".to_owned(), |e| e.to_string()),
                        hex(&p.kid)
                    );
                }
                p.connected = true;
                Ok(0)
            }
            ConnectStatus => Ok(lock(&self.state)
                .peers
                .get(&a[0])
                .map(|p| if p.connected { 2 } else { 0 })
                .unwrap_or(3)),
            QosListen => self.qos_listen(a, rt),
            QosLookup => self.qos_lookup(a, rt),
            _ => unreachable!(),
        }
    }
    fn send(&self, op: Op, a: &[u32], rt: &Runtime) -> Result<u32> {
        let socket = self.socket(a[0])?;
        if (a[2] as i32) < 0 {
            return Err(10022);
        }
        if a[3] & !7 != 0 {
            return Err(10045);
        }
        let data = read(rt, a[1], a[2] as usize)?;
        let target = if op == Op::SendTo {
            Some(sockaddr(rt, a[4], a[5])?)
        } else {
            None
        };
        if let Some(target) = target
            && target.ip().is_broadcast()
        {
            if !socket.host.datagram {
                return Err(10045);
            }
            if host(socket.host.get_option(host::OptionName::Broadcast))? == 0 {
                return Err(10013);
            }
            for dest in self.config.broadcasts(target.port()) {
                host(socket.host.send(&data, Some(dest), a[3] as i32))?;
            }
            return Ok(data.len() as u32);
        }
        let routed = target.map(|p| self.target(p));
        let result = host(socket.host.send(&data, routed, a[3] as i32));
        if let (Some(to), Some(at)) = (target, routed)
            && to != at
        {
            self.say_sent(to, at, &result);
        }
        let sent = result?;
        if socket.host.datagram {
            traffic::sent(sent);
        }
        Ok(sent as u32)
    }
    /// The first datagram the title sends a peer on each port, said once
    /// (and a failed one each time): a join's first packets.
    fn say_sent(&self, to: SocketAddrV4, at: SocketAddrV4, result: &Result<usize>) {
        let first = lock(&self.state)
            .traffic
            .insert((ip_word(*to.ip()), to.port(), true));
        match result {
            Ok(n) if first => eprintln!("live: xnet: first packet to {to} (at {at}): {n} bytes"),
            Err(e) => eprintln!("live: xnet: a packet to {to} (at {at}) failed: error {e}"),
            _ => {}
        }
    }
    fn recv(&self, op: Op, a: &[u32], rt: &Runtime) -> Result<u32> {
        let socket = self.socket(a[0])?;
        if (a[2] as i32) < 0 || a[2] > 16 * 1024 * 1024 {
            return Err(10022);
        }
        if a[3] & !3 != 0 {
            return Err(10045);
        }
        writable(rt, a[1], a[2] as usize)?;
        let source = op == Op::RecvFrom && a[4] != 0;
        if source {
            if (word(rt, a[5])? as i32) < 16 {
                return Err(10014);
            }
            writable(rt, a[5], 4)?;
            writable(rt, a[4], 16)?;
        }
        if a[2] == 0 && !socket.host.datagram {
            return Ok(0);
        }
        // A datagram is read whole (to report truncation) into a buffer
        // kept per thread, not one allocated and zeroed per call: the game
        // polls its sockets every frame and most polls find nothing.
        let mut data = if socket.host.datagram {
            traffic::take_buffer()
        } else {
            vec![0; a[2] as usize]
        };
        let got = socket.host.recv(&mut data, a[3] as i32);
        let result = self.received(got, &data, &socket, a, rt, source);
        if socket.host.datagram {
            traffic::give_buffer(data);
        }
        result
    }
    fn received(
        &self,
        got: io::Result<(usize, SocketAddrV4)>,
        data: &[u8],
        socket: &Socket,
        a: &[u32],
        rt: &Runtime,
        source: bool,
    ) -> Result<u32> {
        let (len, sender) = got.map_err(|e| {
            if e.kind() == io::ErrorKind::WouldBlock
                && !socket.nonblocking.load(Ordering::Acquire)
                && lock(&socket.options)
                    .get(&(0xffff, 0x1006))
                    .is_some_and(|b| b.iter().any(|v| *v != 0))
            {
                10060
            } else {
                host::wsa(&e)
            }
        })?;
        if socket.host.datagram {
            traffic::received(len);
        }
        let copy = len.min(a[2] as usize);
        write(rt, a[1], &data[..copy])?;
        if source {
            let sender = self.source(sender, socket);
            write(rt, a[4], &address_bytes(sender))?;
            put_word(rt, a[5], 16)?;
        }
        if len > a[2] as usize {
            return Err(10040);
        }
        Ok(copy as u32)
    }
    fn option(&self, op: Op, a: &[u32], rt: &Runtime) -> Result<u32> {
        let socket = self.socket(a[0])?;
        let option = match (a[1], a[2]) {
            (0xffff, 4) => Some(host::OptionName::Reuse),
            (0xffff, 0x20) => Some(host::OptionName::Broadcast),
            (0xffff, 8) => Some(host::OptionName::KeepAlive),
            (0xffff, 0x1001) => Some(host::OptionName::SendBuffer),
            (0xffff, 0x1002) => Some(host::OptionName::ReceiveBuffer),
            (0xffff, 0x1007) => Some(host::OptionName::Error),
            (6, 1) => Some(host::OptionName::NoDelay),
            (0xffff, 0x80 | 0xffffff7f | 0xfffffffb | 0x1005 | 0x1006 | 0x1008) => None,
            (0xffff, 0x4001) if socket.protocol != 254 => None,
            _ => return Err(10042),
        };
        if op == Op::SetOption {
            if a[4] < 4 || (a[4] as i32) < 0 {
                return Err(10014);
            }
            let data = read(rt, a[3], 4)?;
            let value = i32::from_le_bytes(data[..4].try_into().unwrap());
            if let Some(option) = option {
                host(socket.host.set_option(option, value))?;
            } else {
                match a[2] {
                    0x80 => host(socket.host.linger(
                        u16::from_le_bytes([data[0], data[1]]),
                        u16::from_le_bytes([data[2], data[3]]),
                    ))?,
                    0xffffff7f => host(socket.host.linger(u16::from(value == 0), 0))?,
                    0x1008 => return Err(10042),
                    0x1005 | 0x1006 => {
                        if value < 0 {
                            return Err(10022);
                        }
                        host(socket.host.timeout(a[2] == 0x1006, value as u32))?;
                    }
                    0xfffffffb => host(
                        socket
                            .host
                            .set_option(host::OptionName::Reuse, i32::from(value == 0)),
                    )?,
                    0x4001 => {} // Xbox security-bypass option; HLE transport is already plaintext.
                    _ => unreachable!(),
                }
            }
            lock(&socket.options).insert((a[1], a[2]), data);
            Ok(0)
        } else {
            let len = word(rt, a[4])?;
            if (len as i32) < 4 {
                return Err(10014);
            }
            writable(rt, a[3], 4)?;
            writable(rt, a[4], 4)?;
            let data = if let Some(option) = option {
                let mut value = host(socket.host.get_option(option))?;
                if matches!((a[1], a[2]), (0xffff, 4 | 8 | 0x20) | (6, 1)) {
                    value = i32::from(value != 0);
                }
                if a[2] == 0x1007 {
                    if let Some(pending) = lock(&socket.pending).take()
                        && pending != 0
                    {
                        value = pending as i32;
                    }
                    if value != 0 && value < 10000 {
                        value = host::wsa(&io::Error::from_raw_os_error(value)) as i32;
                    }
                }
                value.to_le_bytes().to_vec()
            } else if a[2] == 0x1008 {
                (if socket.host.datagram { 2u32 } else { 1 })
                    .to_le_bytes()
                    .to_vec()
            } else {
                lock(&socket.options)
                    .get(&(a[1], a[2]))
                    .cloned()
                    .unwrap_or(vec![0; 4])
            };
            write(rt, a[3], &data)?;
            put_word(rt, a[4], 4)?;
            Ok(0)
        }
    }
    fn select(&self, a: &[u32], rt: &Runtime) -> Result<u32> {
        if a[1..4].iter().all(|p| *p == 0) {
            return Err(10022);
        }
        let mut sets = Vec::new();
        let mut sockets = BTreeMap::new();
        for p in &a[1..4] {
            let set = if *p == 0 { Vec::new() } else { fdset(rt, *p)? };
            if *p != 0 {
                writable(rt, *p, 4 + set.len() * 4)?;
            }
            for id in &set {
                sockets.entry(*id).or_insert(self.socket(*id)?);
            }
            sets.push(set);
        }
        let millis = if a[4] == 0 {
            -1
        } else {
            let b = read(rt, a[4], 8)?;
            let sec = i32::from_le_bytes(b[..4].try_into().unwrap());
            let micros = i32::from_le_bytes(b[4..].try_into().unwrap());
            if sec < 0 || !(0..1000000).contains(&micros) {
                return Err(10022);
            }
            (sec as i64 * 1000 + (micros as i64 + 999) / 1000).min(i32::MAX as i64) as i32
        };
        let poll: Vec<_> = sockets
            .iter()
            .map(|(id, s)| {
                let mut events = 0;
                if sets[0].contains(id) {
                    events |= 1;
                }
                if sets[1].contains(id) {
                    events |= 4;
                }
                if sets[2].contains(id) {
                    events |= 2;
                }
                // A failed nonblocking connect is delivered in exceptfds, even if
                // only that set was supplied. POLLOUT completes successful connects.
                if lock(&s.pending).is_some() {
                    events |= 4;
                }
                (&s.host, events)
            })
            .collect();
        let events = host(host::wait(&poll, millis))?;
        let mut ready = [Vec::new(), Vec::new(), Vec::new()];
        for ((&id, socket), flags) in sockets.iter().zip(events) {
            if flags & 0x20 != 0 {
                return Err(10038);
            } // POLLNVAL
            let mut failure = false;
            let pending = *lock(&socket.pending);
            if let Some(error) = pending {
                if error != 0 {
                    failure = true;
                } else if flags != 0 {
                    let code = host(socket.host.get_option(host::OptionName::Error))?;
                    if code != 0 {
                        *lock(&socket.pending) =
                            Some(host::wsa(&io::Error::from_raw_os_error(code)));
                        failure = true;
                    } else {
                        *lock(&socket.pending) = None;
                    }
                }
            }
            for (i, set) in sets.iter().enumerate() {
                if set.contains(&id)
                    && match i {
                        0 => flags & (1 | 8 | 16) != 0,
                        1 => flags & 4 != 0 && !failure,
                        _ => flags & 2 != 0 || failure,
                    }
                {
                    ready[i].push(id);
                }
            }
        }
        let count = ready.iter().map(Vec::len).sum::<usize>();
        for (p, set) in a[1..4].iter().zip(ready) {
            if *p != 0 {
                let mut data = Vec::new();
                data.extend_from_slice(&(set.len() as u32).to_le_bytes());
                for id in set {
                    data.extend_from_slice(&id.to_le_bytes());
                }
                write(rt, *p, &data)?;
            }
        }
        Ok(count as u32)
    }
    fn qos_listen(&self, a: &[u32], rt: &Runtime) -> Result<u32> {
        let kid: [u8; 8] = read(rt, a[0], 8)?.try_into().unwrap();
        if a[2] > 1020 || a[4] & !0x1f != 0 || a[4] & 3 == 3 {
            return Err(10022);
        }
        let data = if a[4] & 4 != 0 {
            Some(read(rt, a[1], a[2] as usize)?)
        } else {
            None
        };
        let mut s = lock(&self.state);
        if a[4] & 0x10 != 0 {
            if lock(&self.listeners).remove(&kid).is_some() {
                eprintln!("live: qos: stopped listening for session {}", hex(&kid));
            }
            return Ok(0);
        }
        if !s.keys.contains_key(&kid) {
            eprintln!(
                "live: qos: can't listen for session {}: no key registered for it",
                hex(&kid)
            );
            return Err(10022);
        }
        if s.responder.is_none() {
            let started = qos::Responder::start(
                self.config.address,
                self.listeners.clone(),
                self.pings.clone(),
                self.asked.clone(),
            );
            if let Err(e) = &started {
                eprintln!(
                    "live: qos: can't answer on {}:{}: {e}; guests will see network problems",
                    self.config.address.ip(),
                    self.config.address.port() + 2
                );
            }
            s.responder = Some(host(started)?);
        }
        let mut listeners = lock(&self.listeners);
        let entry = listeners.entry(kid).or_insert_with(|| qos::Listener {
            bandwidth: 10_000_000,
            ..qos::Listener::default()
        });
        let was = (entry.enabled, entry.data.len());
        if a[4] & 1 != 0 {
            entry.enabled = true;
        }
        if a[4] & 2 != 0 {
            entry.enabled = false;
        }
        if let Some(data) = data {
            if !data.is_empty() {
                entry.data_set = DATA_SET.fetch_add(1, Ordering::Relaxed);
            }
            entry.data = data;
        }
        if a[4] & 8 != 0 {
            entry.bandwidth = a[3];
        }
        if (entry.enabled, entry.data.len()) != was {
            eprintln!(
                "live: qos: listening for session {} on {}:{}: {}, {} bytes of data",
                hex(&kid),
                self.config.address.ip(),
                self.config.address.port() + 2,
                if entry.enabled { "on" } else { "off" },
                entry.data.len()
            );
        }
        Ok(0)
    }
    /// One log line per lookup, unless it repeats the last.
    fn say_lookup(
        &self,
        targets: &[(SocketAddrV4, [u8; 8])],
        results: &[std::io::Result<qos::Info>],
    ) {
        let line = lookup_line(targets, results);
        let mut said = lock(&self.said);
        if *said != line {
            eprintln!("{line}");
            *said = line;
        }
    }
    fn qos_lookup(&self, a: &[u32], rt: &Runtime) -> Result<u32> {
        if a[0] > 64
            || a[4] > 64
            || a[0] + a[4] > 64
            || a[7] == 0
            || a[7] > 65535
            || a[9] != 0
            || a[10] != 0
        {
            return Err(10022);
        }
        writable(rt, a[11], 4)?;
        let xnas = pointers(rt, a[1], a[0])?;
        let kids = pointers(rt, a[2], a[0])?;
        let keys = pointers(rt, a[3], a[0])?;
        let mut targets = Vec::new();
        for ((xna, kid), key) in xnas.into_iter().zip(kids).zip(keys) {
            let xna: [u8; 36] = read(rt, xna, 36)?.try_into().unwrap();
            let endpoint = endpoint(&xna)?;
            let kid: [u8; 8] = read(rt, kid, 8)?.try_into().unwrap();
            read(rt, key, 16)?;
            targets.push((endpoint, kid));
        }
        let ips = pointers(rt, a[5], a[4])?;
        read(rt, a[6], a[4] as usize * 4)?;
        for ip in ips {
            let peer = lock(&self.state).peers.get(&ip).cloned();
            if let Some(peer) = peer {
                targets.push((endpoint(&peer.xna)?, peer.kid));
            } else {
                targets.push((
                    SocketAddrV4::new(Ipv4Addr::from(ip.to_le_bytes()), 3074),
                    [0; 8],
                ));
            }
        }
        // All guest input is copied and outputs checked before the bounded I/O.
        let results: Vec<_> = targets
            .iter()
            .map(|&(address, kid)| qos::query_why(address, kid, a[7] as u16))
            .collect();
        self.say_lookup(&targets, &results);
        let infos: Vec<_> = results
            .into_iter()
            .map(|r| {
                r.unwrap_or(qos::Info {
                    flags: 1,
                    ..qos::Info::default()
                })
            })
            .collect();
        let size = 8 + infos.len() * 24 + infos.iter().map(|i| i.data.len()).sum::<usize>();
        let mut m = rt.memory();
        let mut s = lock(&self.state);
        let ptr = s.qos.alloc(&mut m, size as u32).ok_or(10055u32)?;
        let mut out = vec![0; size];
        out[..4].copy_from_slice(&(infos.len() as u32).to_le_bytes());
        let mut offset = 8 + infos.len() * 24;
        for (i, info) in infos.into_iter().enumerate() {
            let at = 8 + i * 24;
            out[at] = info.flags;
            out[at + 2..at + 4].copy_from_slice(&(a[7] as u16).to_le_bytes());
            out[at + 4..at + 6].copy_from_slice(&info.probes.to_le_bytes());
            out[at + 6..at + 8].copy_from_slice(&(info.data.len() as u16).to_le_bytes());
            if !info.data.is_empty() {
                out[at + 8..at + 12].copy_from_slice(&(ptr + offset as u32).to_le_bytes());
                out[offset..offset + info.data.len()].copy_from_slice(&info.data);
                offset += info.data.len();
            }
            out[at + 12..at + 14].copy_from_slice(&info.rtt.to_le_bytes());
            out[at + 14..at + 16].copy_from_slice(&info.rtt.to_le_bytes());
            out[at + 16..at + 20].copy_from_slice(&info.bandwidth.to_le_bytes());
            out[at + 20..at + 24].copy_from_slice(&info.bandwidth.to_le_bytes());
        }
        m.write_bytes(ptr, &out, 1).map_err(|_| 10014u32)?;
        m.set_u32(a[11], ptr).map_err(|_| 10014u32)?;
        Ok(0)
    }
}
/// Orders listeners' data ([`qos::answer`]).
static DATA_SET: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
fn hex(kid: &[u8; 8]) -> String {
    live_proto::sign::hex(kid)
}
/// What a QoS lookup found, per target: `contacted` with the data the
/// host listens with, `off` for a host that stopped listening, or why no
/// answer came (the title shows "network problems" for those).
pub(crate) fn lookup_line(
    targets: &[(SocketAddrV4, [u8; 8])],
    results: &[std::io::Result<qos::Info>],
) -> String {
    let each: Vec<_> = targets
        .iter()
        .zip(results)
        .map(|((address, kid), r)| {
            let qos = SocketAddrV4::new(*address.ip(), address.port().saturating_add(2));
            let what = match r {
                Ok(i) if i.flags & 4 != 0 => "contacted, not listening (off)".to_owned(),
                Ok(i) => format!("contacted, {} bytes of data", i.data.len()),
                Err(e) if e.kind() == io::ErrorKind::ConnectionRefused => {
                    "no answer: nothing on that port".to_owned()
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    format!(
                        "no answer in {} tries (the host doesn't listen for that session, or \
                         isn't there)",
                        qos::QUERY_TRIES
                    )
                }
                Err(e) => format!("no answer: {e}"),
            };
            format!("{qos} session {}: {what}", hex(kid))
        })
        .collect();
    format!("live: join: qos lookup: {}", each.join("; "))
}
pub(crate) fn random(len: usize) -> Result<Vec<u8>> {
    film::state_random(len, || {
        let mut bytes = vec![0; len];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut bytes))
            .map_err(|_| 10055u32)?;
        Ok(bytes)
    })
}
fn pointers(rt: &Runtime, p: u32, count: u32) -> Result<Vec<u32>> {
    Ok(read(rt, p, count as usize * 4)?
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect())
}
fn fdset(rt: &Runtime, p: u32) -> Result<Vec<u32>> {
    let count = word(rt, p)?;
    if count > 64 {
        return Err(10022);
    }
    let ptr = p.checked_add(4).ok_or(10014u32)?;
    let mut values = pointers(rt, ptr, count)?;
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lookup_says_per_target_whether_the_host_answered() {
        let host = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 31000);
        let kid = [0x86, 0x87, 0xb4, 0x10, 0x9b, 0x1c, 0x02, 0x94];
        let targets = [(host, kid), (host, kid), (host, kid), (host, kid)];
        let results = [
            Ok(qos::Info {
                flags: 0xb,
                data: vec![0; 12],
                ..qos::Info::default()
            }),
            Ok(qos::Info {
                flags: 7,
                ..qos::Info::default()
            }),
            Err(io::Error::from(io::ErrorKind::WouldBlock)),
            Err(io::Error::from(io::ErrorKind::ConnectionRefused)),
        ];
        let line = lookup_line(&targets, &results);
        let at = "127.0.0.1:31002 session 8687b4109b1c0294";
        assert_eq!(
            line,
            format!(
                "live: join: qos lookup: {at}: contacted, 12 bytes of data; \
                 {at}: contacted, not listening (off); \
                 {at}: no answer in 3 tries (the host doesn't listen for that session, or isn't \
                 there); {at}: no answer: nothing on that port"
            )
        );
    }
}
