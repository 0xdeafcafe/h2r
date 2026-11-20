//! The dedicated host's side of the Live datagram tunnel.
//!
//! Inject inbound packets through a local UDP bridge so Winsock's
//! native wait, receive timeouts and per-channel UDP buffering still apply.

use super::{
    Network, Op, Result as WsaResult, Socket, endpoint, lock, put_word, read, sockaddr, word,
    writable,
};
use crate::{host, qos, traffic};
use live_proto::{
    Xuid,
    tunnel::{Frame, Kind, decode, encode},
};
use runtime::Runtime;
use std::{
    collections::BTreeMap,
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket},
    sync::{
        Arc, Mutex, OnceLock, TryLockError,
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};
use tungstenite::{
    Message, WebSocket, accept_hdr_with_config,
    handshake::server::{Request, Response},
    protocol::WebSocketConfig,
};

const MAX_PEERS: usize = 16;
const MAX_PAYLOAD: usize = 1264;
const MAX_PORT: u16 = 6;
const QOS_PORT: u16 = 2;
const WIRE_HEADER: usize = 8;
const BRIDGE_HEADER: usize = 4;
const OUTBOX_DEPTH: usize = 64;
const SEND_BURST: usize = 16;
const RATE_PER_SECOND: u128 = 240;
const RATE_BURST: u128 = 64;
const TURN: Duration = Duration::from_millis(4);
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const PING_INTERVAL: Duration = Duration::from_secs(5);
const PONG_TIMEOUT: Duration = Duration::from_secs(15);
const HOST_IP: Ipv4Addr = Ipv4Addr::new(10, 77, 0, 1);
const MSG_PEEK: i32 = 2;
#[cfg(target_vendor = "apple")]
const MSG_DONTWAIT: i32 = 0x80;
#[cfg(not(target_vendor = "apple"))]
const MSG_DONTWAIT: i32 = 0x40;

fn peer_ip(id: u32) -> Ipv4Addr {
    let [high, low] = (id as u16).to_be_bytes();
    Ipv4Addr::new(10, 77, high, low)
}

fn address_id(ip: Ipv4Addr) -> Option<u32> {
    let [first, second, high, low] = ip.octets();
    (first == 10 && second == 77).then(|| u16::from_be_bytes([high, low]).into())
}

fn game_offset(port: u16) -> Option<u16> {
    port.checked_sub(1000).filter(|offset| *offset <= MAX_PORT)
}

fn channel_offset(port: u16, base: u16) -> Option<u16> {
    game_offset(port).or_else(|| port.checked_sub(base).filter(|offset| *offset <= MAX_PORT))
}

fn datagram_fits(port: u16, size: usize) -> bool {
    port <= MAX_PORT && size <= MAX_PAYLOAD
}

struct Rate {
    credit: u128,
    last: Instant,
}

impl Rate {
    const TOKEN: u128 = 1_000_000_000;

    fn new(now: Instant) -> Self {
        Self {
            credit: RATE_BURST * Self::TOKEN,
            last: now,
        }
    }

    fn take(&mut self, now: Instant) -> bool {
        let earned = now.saturating_duration_since(self.last).as_nanos() * RATE_PER_SECOND;
        self.last = self.last.max(now);
        self.credit = (self.credit + earned).min(RATE_BURST * Self::TOKEN);
        if self.credit < Self::TOKEN {
            return false;
        }
        self.credit -= Self::TOKEN;
        true
    }
}

struct Peer {
    out: SyncSender<Frame>,
    rate: Rate,
}

impl Peer {
    fn send(&mut self, id: u32, port: u16, payload: &[u8], now: Instant) {
        if self.rate.take(now) {
            // A full queue drops datagrams, as a busy UDP link would.
            let _ = self.out.try_send(Frame {
                kind: Kind::Datagram,
                port,
                peer: id,
                payload: payload.to_vec(),
            });
        }
    }
}

#[derive(Default)]
struct Peers {
    active: BTreeMap<u32, Peer>,
    last_id: u32,
}

impl Peers {
    fn register(&mut self, now: Instant) -> Option<(u32, Receiver<Frame>)> {
        if self.active.len() >= MAX_PEERS || self.last_id >= u32::from(u16::MAX - 1) {
            return None;
        }
        // .0.0 and .0.1 are reserved. Keep a closed peer's address out of
        // circulation for the rest of the lease, including queued packets.
        let id = (self.last_id + 1).max(2);
        self.last_id = id;
        let (out, incoming) = sync_channel(OUTBOX_DEPTH);
        self.active.insert(
            id,
            Peer {
                out,
                rate: Rate::new(now),
            },
        );
        Some((id, incoming))
    }

    fn send(&mut self, id: u32, port: u16, payload: &[u8], now: Instant) -> bool {
        let Some(peer) = self.active.get_mut(&id) else {
            return false;
        };
        peer.send(id, port, payload, now);
        true
    }

    fn broadcast(&mut self, port: u16, payload: &[u8], now: Instant) -> bool {
        for (&id, peer) in &mut self.active {
            peer.send(id, port, payload, now);
        }
        !self.active.is_empty()
    }

    fn source(&self, id: u32, base: u16, port: u16) -> Option<SocketAddrV4> {
        if port > MAX_PORT || !self.active.contains_key(&id) {
            return None;
        }
        Some(SocketAddrV4::new(peer_ip(id), base.checked_add(port)?))
    }

    fn contains_ip(&self, ip: Ipv4Addr) -> bool {
        address_id(ip).is_some_and(|id| self.active.contains_key(&id))
    }

    fn remove(&mut self, id: u32) {
        self.active.remove(&id);
    }
}

#[derive(Default)]
struct Channel {
    reader: Mutex<()>,
    // The OS sees the local bridge as the connected peer. XNet sees this one.
    destination: Mutex<Option<SocketAddrV4>>,
}

struct TunnelHost {
    address: SocketAddrV4,
    bridge: UdpSocket,
    bridge_address: SocketAddrV4,
    peers: Mutex<Peers>,
    channels: Mutex<BTreeMap<u32, Arc<Channel>>>,
    listeners: qos::Listeners,
    pings: qos::Pings,
    asked: qos::Asked,
}

impl TunnelHost {
    fn start(network: &Network, bind: &str) -> io::Result<Arc<Self>> {
        let address = network.config.address;
        if address_id(*address.ip()).is_some() || !(1..=65529).contains(&address.port()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid XNet tunnel base",
            ));
        }
        let listener = TcpListener::bind(bind)?;
        let local = listener.local_addr()?;
        // Use the game IP for local delivery so a connected wildcard UDP
        // socket keeps receiving on the same interface as an unconnected one.
        let bridge = UdpSocket::bind((*address.ip(), 0))?;
        bridge.set_nonblocking(true)?;
        let SocketAddr::V4(bridge_address) = bridge.local_addr()? else {
            unreachable!("the bridge binds an IPv4 socket");
        };
        let tunnel = Arc::new(Self {
            address,
            bridge,
            bridge_address,
            peers: Mutex::default(),
            channels: Mutex::default(),
            listeners: network.listeners.clone(),
            pings: network.pings.clone(),
            asked: network.asked.clone(),
        });
        let worker = tunnel.clone();
        thread::Builder::new()
            .name("h2-tunnel-listener".into())
            .spawn(move || worker.listen(listener))?;
        eprintln!("live: host tunnel listening on {local}");
        Ok(tunnel)
    }

    fn listen(self: Arc<Self>, listener: TcpListener) {
        let mut connections: Vec<thread::JoinHandle<()>> = Vec::new();
        for stream in listener.incoming() {
            let stream = match stream {
                Ok(stream) => stream,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    eprintln!("live: host tunnel accept failed: {error}");
                    break;
                }
            };
            connections.retain(|connection| !connection.is_finished());
            if connections.len() >= MAX_PEERS {
                continue;
            }
            let tunnel = self.clone();
            match thread::Builder::new()
                .name("h2-tunnel-peer".into())
                .spawn(move || {
                    if let Err(error) = tunnel.serve(stream) {
                        eprintln!("live: host tunnel connection ended: {error}");
                    }
                }) {
                Ok(thread) => connections.push(thread),
                Err(error) => eprintln!("live: host tunnel could not start connection: {error}"),
            }
        }
    }

    fn serve(&self, stream: TcpStream) -> Result<(), Box<dyn std::error::Error>> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(HELLO_TIMEOUT))?;
        stream.set_write_timeout(Some(Duration::from_secs(1)))?;
        let limit = Some(WIRE_HEADER + MAX_PAYLOAD);
        let config = WebSocketConfig::default()
            .read_buffer_size(4096)
            .write_buffer_size(0)
            .max_write_buffer_size(4096)
            .max_frame_size(limit)
            .max_message_size(limit);
        use live_proto::matchmaking::{PACK_ECHO_PATH, REGION_ECHO};
        let mut xuid = None;
        let mut echo = false;
        let mut ws = accept_hdr_with_config(
            stream,
            |request: &Request, mut response: Response| {
                if request.uri().path() == PACK_ECHO_PATH {
                    echo = true;
                    if request.headers().get("sec-websocket-protocol").and_then(|h| h.to_str().ok()) == Some(REGION_ECHO) {
                        response.headers_mut().insert("sec-websocket-protocol", REGION_ECHO.parse().unwrap());
                    }
                    return Ok(response);
                }
                xuid = request
                    .headers()
                    .get("x-h2-tunnel-peer")
                    .and_then(|header| header.to_str().ok())
                    .and_then(|value| Xuid::parse(value).ok());
                Ok(response)
            },
            Some(config),
        )
        .map_err(|error| error.to_string())?;
        if echo {
            ws.get_mut().set_read_timeout(Some(Duration::from_secs(1)))?;
            if let Message::Text(text) = ws.read()?
                && text.as_str() == REGION_ECHO
            {
                ws.send(Message::Text(text))?;
            }
            return Ok(());
        }
        ws.get_mut().set_read_timeout(Some(TURN))?;
        wait_for_hello(&mut ws)?;

        let (id, out) = lock(&self.peers)
            .register(Instant::now())
            .ok_or("host tunnel is full")?;
        let peer = ConnectedPeer {
            tunnel: self,
            id,
            xuid,
        };
        eprintln!(
            "live: host tunnel joined peer {id} ({}, xuid {peer})",
            peer_ip(id)
        );
        write_frame(&mut ws, &hello(id))?;
        self.exchange(&mut ws, id, out)
    }

    fn exchange(
        &self,
        ws: &mut WebSocket<TcpStream>,
        id: u32,
        out: Receiver<Frame>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut ingress = Rate::new(Instant::now());
        let mut last_ping = Instant::now();
        let mut last_pong = last_ping;
        loop {
            let now = Instant::now();
            if now.duration_since(last_pong) >= PONG_TIMEOUT {
                return Err("tunnel pong timed out".into());
            }
            if now.duration_since(last_ping) >= PING_INTERVAL {
                ws.send(Message::Ping(Default::default()))?;
                last_ping = now;
            }
            for frame in out.try_iter().take(SEND_BURST) {
                write_frame(ws, &frame)?;
            }
            match ws.read() {
                Ok(Message::Binary(bytes)) => {
                    let frame = decode(&bytes).map_err(|_| "invalid client tunnel frame")?;
                    if !valid_client_frame(&frame) {
                        return Err("invalid client tunnel frame".into());
                    }
                    match frame.kind {
                        Kind::Datagram if ingress.take(Instant::now()) => {
                            self.deliver(id, &frame);
                        }
                        Kind::Datagram => {}
                        Kind::Bye => {
                            lock(&self.peers).remove(id);
                            write_frame(ws, &control(Kind::Bye, id, Vec::new()))?;
                            let _ = ws.close(None);
                            return Ok(());
                        }
                        Kind::Hello => return Err("duplicate client tunnel hello".into()),
                    }
                }
                Ok(Message::Pong(_)) => last_pong = Instant::now(),
                Ok(Message::Ping(_)) => ws.flush()?,
                Ok(Message::Close(_)) => {
                    lock(&self.peers).remove(id);
                    let _ = ws.flush();
                    return Ok(());
                }
                Ok(_) => return Err("expected a binary tunnel frame".into()),
                Err(error) if pending(&error) => {}
                Err(tungstenite::Error::ConnectionClosed) => return Ok(()),
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn deliver(&self, id: u32, frame: &Frame) {
        if frame.port == QOS_PORT
            && let Some(reply) = self.qos_reply(peer_ip(id), &frame.payload)
        {
            if let Some(payload) = reply {
                lock(&self.peers).send(id, QOS_PORT, &payload, Instant::now());
            }
            return;
        }

        // This tag lives only on the local UDP bridge. It identifies a
        // pending datagram even if its tunnel has closed before the game reads.
        let mut bytes = [0; BRIDGE_HEADER + MAX_PAYLOAD];
        bytes[..BRIDGE_HEADER].copy_from_slice(&id.to_le_bytes());
        let len = BRIDGE_HEADER + frame.payload.len();
        bytes[BRIDGE_HEADER..len].copy_from_slice(&frame.payload);
        let to = SocketAddrV4::new(*self.address.ip(), self.address.port() + frame.port);
        let _ = self.bridge.send_to(&bytes[..len], to);
    }

    /// Preserve the existing QoS response and key selection on port +2, with
    /// the same synthetic address as the peer's game channels.
    fn qos_reply(&self, ip: Ipv4Addr, input: &[u8]) -> Option<Option<Vec<u8>>> {
        if let Some(pong) = qos::answer_ping(input, ip, &self.pings) {
            return Some(Some(pong.to_vec()));
        }
        if input.len() != 24 || &input[..8] != b"H2QOS001" {
            return None;
        }
        let kid: [u8; 8] = input[16..24].try_into().unwrap();
        Some(
            qos::answer(&lock(&self.listeners), &kid).map(|(enabled, bandwidth, data)| {
                lock(&self.asked).insert(ip, kid);
                let mut reply = Vec::with_capacity(23 + data.len());
                reply.extend_from_slice(b"H2QOR001");
                reply.extend_from_slice(&input[8..16]);
                reply.push(if enabled {
                    3 | if data.is_empty() { 0 } else { 8 }
                } else {
                    7
                });
                reply.extend_from_slice(&bandwidth.to_le_bytes());
                reply.extend_from_slice(&(data.len() as u16).to_le_bytes());
                reply.extend_from_slice(&data);
                reply
            }),
        )
    }

    fn recv_udp(
        &self,
        socket: &Socket,
        channel: &Channel,
        offset: u16,
        data: &mut [u8],
        flags: i32,
    ) -> io::Result<(usize, SocketAddrV4, usize)> {
        receive_with_bridge(
            self.bridge_address,
            data,
            flags,
            |data, flags| socket.host.recv(data, flags),
            |id| {
                let expected = *lock(&channel.destination);
                lock(&self.peers)
                    .source(id, self.address.port(), offset)
                    .filter(|source| expected.is_none_or(|to| to.ip() == source.ip()))
            },
        )
    }

    fn channel(&self, id: u32) -> Arc<Channel> {
        lock(&self.channels).entry(id).or_default().clone()
    }

    fn destination(&self, id: u32) -> Option<SocketAddrV4> {
        let channel = lock(&self.channels).get(&id)?.clone();
        
        *lock(&channel.destination)
    }

    fn reap_addresses(&self, network: &Network) {
        let mut state = lock(&network.state);
        let peers = lock(&self.peers);
        let keep = |ip: Ipv4Addr| address_id(ip).is_none() || peers.contains_ip(ip);
        state
            .peers
            .retain(|_, peer| endpoint(&peer.xna).is_ok_and(|address| keep(*address.ip())));
        state
            .traffic
            .retain(|(ip, _, _)| keep(Ipv4Addr::from(ip.to_le_bytes())));
    }
}

fn receive_with_bridge(
    bridge: SocketAddrV4,
    data: &mut [u8],
    mut flags: i32,
    mut read: impl FnMut(&mut [u8], i32) -> io::Result<(usize, SocketAddrV4)>,
    mut source: impl FnMut(u32) -> Option<SocketAddrV4>,
) -> io::Result<(usize, SocketAddrV4, usize)> {
    loop {
        let (len, from) = read(data, flags)?;
        if from != bridge {
            return Ok((len, from, 0));
        }
        if len >= BRIDGE_HEADER {
            let id = u32::from_le_bytes(data[..BRIDGE_HEADER].try_into().unwrap());
            if let Some(address) = source(id) {
                return Ok((len - BRIDGE_HEADER, address, BRIDGE_HEADER));
            }
        }
        if flags & MSG_PEEK != 0 {
            // The caller holds the channel's read lock. Discard the stale
            // packet and keep the next live datagram intact for a peek.
            read(data, (flags & !MSG_PEEK) | MSG_DONTWAIT)?;
        }
        // A stale packet may have woken select. Avoid blocking again if
        // the socket has no live datagram behind it.
        flags |= MSG_DONTWAIT;
    }
}

struct ConnectedPeer<'a> {
    tunnel: &'a TunnelHost,
    id: u32,
    xuid: Option<Xuid>,
}

impl std::fmt::Display for ConnectedPeer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.xuid {
            Some(xuid) => write!(f, "{xuid}"),
            None => f.write_str("unknown"),
        }
    }
}

impl Drop for ConnectedPeer<'_> {
    fn drop(&mut self) {
        lock(&self.tunnel.peers).remove(self.id);
        let ip = peer_ip(self.id);
        lock(&self.tunnel.pings).remove(&ip);
        lock(&self.tunnel.asked).remove(&ip);
        eprintln!(
            "live: host tunnel left peer {} ({ip}, xuid {self})",
            self.id
        );
    }
}

fn control(kind: Kind, peer: u32, payload: Vec<u8>) -> Frame {
    Frame {
        kind,
        port: 0,
        peer,
        payload,
    }
}

fn hello(peer: u32) -> Frame {
    control(Kind::Hello, peer, HOST_IP.octets().to_vec())
}

fn valid_client_frame(frame: &Frame) -> bool {
    frame.peer == 0
        && match frame.kind {
            Kind::Datagram => datagram_fits(frame.port, frame.payload.len()),
            Kind::Hello | Kind::Bye => frame.port == 0 && frame.payload.is_empty(),
        }
}

fn wait_for_hello(ws: &mut WebSocket<TcpStream>) -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    while started.elapsed() < HELLO_TIMEOUT {
        match ws.read() {
            Ok(Message::Binary(bytes)) => {
                let frame = decode(&bytes).map_err(|_| "invalid client tunnel hello")?;
                if matches!(frame.kind, Kind::Hello) && valid_client_frame(&frame) {
                    return Ok(());
                }
                return Err("expected client tunnel hello".into());
            }
            Ok(Message::Ping(_)) => ws.flush()?,
            Ok(Message::Pong(_)) => {}
            Ok(_) => return Err("expected client tunnel hello".into()),
            Err(error) if pending(&error) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err("client tunnel hello timed out".into())
}

fn write_frame(ws: &mut WebSocket<TcpStream>, frame: &Frame) -> tungstenite::Result<()> {
    ws.send(Message::binary(encode(frame)))
}

fn pending(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(error)
        if matches!(error.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock))
}

// A dedicated process owns one XNet configuration and one lease. Initialize
// lazily at the first XNet call and keep the listener for the lease's lifetime.
static HOST: OnceLock<Option<Arc<TunnelHost>>> = OnceLock::new();

fn enabled(network: &Network) -> Option<&'static TunnelHost> {
    let tunnel = HOST.get_or_init(|| {
        let bind = std::env::var("H2_TUNNEL_LISTEN").ok()?;
        match TunnelHost::start(network, &bind) {
            Ok(tunnel) => Some(tunnel),
            Err(error) => {
                eprintln!("live: cannot start host tunnel at {bind}: {error}");
                None
            }
        }
    });
    tunnel
        .as_deref()
        .filter(|tunnel| tunnel.address == network.config.address)
}

/// One Winsock dispatch hook. Operations that need no tunnel work continue
/// down XNet's ordinary path, including select's native fd wait.
pub(super) fn dispatch(
    network: &Network,
    op: Op,
    a: &[u32],
    rt: &Runtime,
) -> Option<WsaResult<u32>> {
    let tunnel = enabled(network)?;
    match op {
        Op::Send | Op::SendTo => send(tunnel, network, op, a, rt).transpose(),
        Op::Recv | Op::RecvFrom => recv(tunnel, network, op, a, rt).transpose(),
        Op::Ioctl if a[1] == 0x4004667f => available(tunnel, network, a, rt).transpose(),
        Op::ConnectSocket => connect(tunnel, network, a, rt).transpose(),
        Op::Socket => {
            let state = lock(&network.state);
            lock(&tunnel.channels).retain(|id, _| state.sockets.contains_key(id));
            None
        }
        Op::Close => {
            lock(&tunnel.channels).remove(&a[0]);
            None
        }
        Op::XnToIn | Op::InToXn | Op::ConnectAddress | Op::ConnectStatus => {
            tunnel.reap_addresses(network);
            if op == Op::XnToIn {
                return reject_closed_address(tunnel, a, rt).transpose();
            }
            None
        }
        _ => None,
    }
}

fn reject_closed_address(tunnel: &TunnelHost, a: &[u32], rt: &Runtime) -> WsaResult<Option<u32>> {
    let xna = read(rt, a[0], 36)?.try_into().unwrap();
    let address = endpoint(&xna)?;
    if address_id(*address.ip()).is_some() && !lock(&tunnel.peers).contains_ip(*address.ip()) {
        return Err(10065);
    }
    Ok(None)
}

fn connect(
    tunnel: &TunnelHost,
    network: &Network,
    a: &[u32],
    rt: &Runtime,
) -> WsaResult<Option<u32>> {
    let socket = network.socket(a[0])?;
    if !socket.host.datagram {
        return Ok(None);
    }
    let guest = sockaddr(rt, a[1], a[2])?;
    let target = network.target(guest);
    let peer = address_id(*target.ip());
    let prior = tunnel.destination(a[0]);
    if peer.is_none() && prior.is_none() {
        return Ok(None);
    }
    if let Some(peer) = peer {
        if !lock(&tunnel.peers).active.contains_key(&peer) {
            return Err(10065);
        }
        channel_offset(guest.port(), tunnel.address.port()).ok_or(10049u32)?;
        super::host(socket.host.connect(tunnel.bridge_address))?;
        *lock(&tunnel.channel(a[0]).destination) =
            Some(SocketAddrV4::new(*target.ip(), guest.port()));
    } else {
        super::host(socket.host.connect(target))?;
        *lock(&tunnel.channel(a[0]).destination) = None;
    }
    Ok(Some(0))
}

fn send(
    tunnel: &TunnelHost,
    network: &Network,
    op: Op,
    a: &[u32],
    rt: &Runtime,
) -> WsaResult<Option<u32>> {
    let (ip, port) = if op == Op::SendTo {
        let guest = sockaddr(rt, a[4], a[5])?;
        let ip = if guest.ip().is_broadcast() {
            *guest.ip()
        } else {
            *network.target(guest).ip()
        };
        (ip, guest.port())
    } else if let Some(target) = tunnel.destination(a[0]) {
        (*target.ip(), target.port())
    } else {
        return Ok(None);
    };
    let peer = address_id(ip);
    let broadcast = ip.is_broadcast();
    if peer.is_none() && !broadcast {
        return Ok(None);
    }
    let socket = network.socket(a[0])?;
    if !socket.host.datagram {
        return Ok(None);
    }
    if (a[2] as i32) < 0 {
        return Err(10022);
    }
    if a[3] & !7 != 0 {
        return Err(10045);
    }
    let offset = channel_offset(port, tunnel.address.port());
    if broadcast && (offset.is_none() || a[2] as usize > MAX_PAYLOAD) {
        return Ok(None);
    }
    let offset = offset.ok_or(10049u32)?;
    if !datagram_fits(offset, a[2] as usize) {
        return Err(10040);
    }
    let data = read(rt, a[1], a[2] as usize)?;
    let now = Instant::now();
    if let Some(peer) = peer {
        if !lock(&tunnel.peers).send(peer, offset, &data, now) {
            return Err(10065);
        }
    } else {
        if super::host(socket.host.get_option(host::OptionName::Broadcast))? == 0 {
            return Err(10013);
        }
        if !lock(&tunnel.peers).broadcast(offset, &data, now) {
            return Ok(None);
        }
        for dest in network.config.broadcasts(port) {
            let _ = socket.host.send(&data, Some(dest), a[3] as i32);
        }
    }
    traffic::sent(data.len());
    Ok(Some(data.len() as u32))
}

fn recv(
    tunnel: &TunnelHost,
    network: &Network,
    op: Op,
    a: &[u32],
    rt: &Runtime,
) -> WsaResult<Option<u32>> {
    let socket = network.socket(a[0])?;
    let Some(offset) = bound_offset(&socket) else {
        return Ok(None);
    };
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
    let mut data = traffic::take_buffer();
    let channel = tunnel.channel(a[0]);
    let got = {
        let _read = lock(&channel.reader);
        tunnel.recv_udp(&socket, &channel, offset, &mut data, a[3] as i32)
    };
    let start = got.as_ref().map_or(0, |&(_, _, start)| start);
    let result = network.received(
        got.map(|(n, from, _)| (n, from)),
        &data[start..],
        &socket,
        a,
        rt,
        source,
    );
    traffic::give_buffer(data);
    result.map(Some)
}

fn available(
    tunnel: &TunnelHost,
    network: &Network,
    a: &[u32],
    rt: &Runtime,
) -> WsaResult<Option<u32>> {
    let socket = network.socket(a[0])?;
    let Some(offset) = bound_offset(&socket) else {
        return Ok(None);
    };
    writable(rt, a[2], 4)?;
    let channel = tunnel.channel(a[0]);
    let _read = match channel.reader.try_lock() {
        Ok(guard) => guard,
        Err(TryLockError::Poisoned(error)) => error.into_inner(),
        Err(TryLockError::WouldBlock) => {
            put_word(rt, a[2], 0)?;
            return Ok(Some(0));
        }
    };
    let mut data = traffic::take_buffer();
    let size = tunnel.recv_udp(
        &socket,
        &channel,
        offset,
        &mut data,
        MSG_PEEK | MSG_DONTWAIT,
    );
    traffic::give_buffer(data);
    let size = match size {
        Ok((size, _, _)) => size as u32,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => 0,
        Err(error) => return Err(host::wsa(&error)),
    };
    put_word(rt, a[2], size)?;
    Ok(Some(0))
}

fn bound_offset(socket: &Socket) -> Option<u16> {
    if !socket.host.datagram {
        return None;
    }
    game_offset(lock(&socket.bound).map_or(0, |a| a.port()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::mpsc::TryRecvError};

    const BASE: u16 = 31000;

    #[test]
    fn hello_and_bye_use_the_contract_wire() {
        assert_eq!(
            encode(&hello(0x0102)),
            [1, 1, 0, 0, 2, 1, 0, 0, 10, 77, 0, 1],
        );
        assert_eq!(
            encode(&control(Kind::Bye, 0x0102, Vec::new())),
            [1, 2, 0, 0, 2, 1, 0, 0],
        );

        let client_hello = decode(&[1, 1, 0, 0, 0, 0, 0, 0]).unwrap();
        assert!(matches!(client_hello.kind, Kind::Hello));
        assert!(valid_client_frame(&client_hello));
        assert!(!valid_client_frame(&hello(0x0102)));
        assert!(!valid_client_frame(&control(Kind::Hello, 0, vec![1])));
        assert!(!valid_client_frame(&control(Kind::Bye, 0x0102, Vec::new())));
        assert!(valid_client_frame(&control(Kind::Bye, 0, Vec::new())));
    }

    #[test]
    fn regional_echo_accepts_one_text_frame_and_never_registers_a_game_peer() {
        use live_proto::matchmaking::{PACK_ECHO_PATH, REGION_ECHO};
        use tungstenite::client::IntoClientRequest;

        let bridge = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let SocketAddr::V4(bridge_address) = bridge.local_addr().unwrap() else { unreachable!() };
        let host = TunnelHost {
            address: SocketAddrV4::new(Ipv4Addr::LOCALHOST, BASE),
            bridge, bridge_address, peers: Mutex::default(), channels: Mutex::default(),
            listeners: Default::default(), pings: Default::default(), asked: Default::default(),
        };
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        thread::scope(|scope| {
            let server = scope.spawn(|| {
                for _ in 0..2 {
                    let (stream, _) = listener.accept().unwrap();
                    host.serve(stream).unwrap();
                }
            });
            let addr = listener.local_addr().unwrap();
            for text_frame in [true, false] {
                let mut req = format!("ws://{addr}{PACK_ECHO_PATH}").into_client_request().unwrap();
                req.headers_mut().insert("sec-websocket-protocol", REGION_ECHO.parse().unwrap());
                let stream = TcpStream::connect(addr).unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let (mut client, response) = tungstenite::client(req, stream).unwrap();
                assert_eq!(response.headers()["sec-websocket-protocol"], REGION_ECHO);
                if text_frame {
                    client.send(Message::Text(REGION_ECHO.into())).unwrap();
                    assert_eq!(client.read().unwrap(), Message::Text(REGION_ECHO.into()));
                } else {
                    client.send(Message::Binary(encode(&control(Kind::Hello, 0, vec![])).into())).unwrap();
                    assert!(client.read().is_err());
                }
            }
            server.join().unwrap();
        });
        assert_eq!(lock(&host.peers).last_id, 0);
    }

    #[test]
    fn maps_each_peer_to_all_seven_xnet_channels() {
        let mut peers = Peers::default();
        let now = Instant::now();
        let (first, _) = peers.register(now).unwrap();
        let (second, _) = peers.register(now).unwrap();
        let first_ip = Ipv4Addr::new(10, 77, 0, 2);
        let second_ip = Ipv4Addr::new(10, 77, 0, 3);
        assert_eq!(address_id(first_ip), Some(first));
        assert_eq!(address_id(second_ip), Some(second));

        for (id, ip) in [(first, first_ip), (second, second_ip)] {
            for port in 0..=MAX_PORT {
                assert_eq!(
                    peers.source(id, BASE, port),
                    Some(SocketAddrV4::new(ip, BASE + port))
                );
                assert_eq!(channel_offset(1000 + port, BASE), Some(port));
                assert_eq!(channel_offset(BASE + port, BASE), Some(port));
            }
            assert_eq!(peers.source(id, BASE, MAX_PORT + 1), None);
        }
        assert!(!peers.contains_ip(HOST_IP));
        assert_eq!(channel_offset(BASE + MAX_PORT + 1, BASE), None);
    }

    #[test]
    fn addressable_unicast_and_discovery_fan_out_with_recipient_ids() {
        let mut peers = Peers::default();
        let now = Instant::now();
        let (first, first_rx) = peers.register(now).unwrap();
        let (second, second_rx) = peers.register(now).unwrap();

        assert!(peers.send(second, 6, b"game", now));
        assert!(matches!(first_rx.try_recv(), Err(TryRecvError::Empty)));
        let frame = second_rx.try_recv().unwrap();
        assert!(matches!(frame.kind, Kind::Datagram));
        assert_eq!(
            (frame.peer, frame.port, frame.payload.as_slice()),
            (second, 6, &b"game"[..])
        );

        assert!(peers.broadcast(5, b"discovery", now));
        for (id, rx) in [(first, first_rx), (second, second_rx)] {
            let frame = rx.try_recv().unwrap();
            assert!(matches!(frame.kind, Kind::Datagram));
            assert_eq!(
                (frame.peer, frame.port, frame.payload.as_slice()),
                (id, 5, &b"discovery"[..])
            );
        }
    }

    #[test]
    fn frame_size_channel_rate_and_outbox_are_bounded() {
        let mut datagram = Frame {
            kind: Kind::Datagram,
            port: MAX_PORT,
            peer: 0,
            payload: vec![0; MAX_PAYLOAD],
        };
        assert!(valid_client_frame(&datagram));
        assert!(valid_client_frame(&decode(&encode(&datagram)).unwrap()));
        datagram.peer = 2;
        assert!(!valid_client_frame(&datagram));
        datagram.peer = 0;
        datagram.port += 1;
        assert!(!valid_client_frame(&datagram));
        datagram.port = MAX_PORT;
        datagram.payload.push(0);
        assert!(!valid_client_frame(&datagram));

        let mut peers = Peers::default();
        let now = Instant::now();
        let (first, first_rx) = peers.register(now).unwrap();
        let (second, second_rx) = peers.register(now).unwrap();
        for _ in 0..RATE_BURST + 1 {
            assert!(peers.send(first, 0, b"game", now));
        }
        assert_eq!(first_rx.try_iter().count(), RATE_BURST as usize);
        assert!(peers.send(first, 0, b"rate limited", now));
        assert!(matches!(first_rx.try_recv(), Err(TryRecvError::Empty)));
        assert!(peers.send(second, 0, b"other peer", now));
        assert_eq!(
            second_rx.try_recv().unwrap().payload.as_slice(),
            b"other peer"
        );

        // Refill the first peer's bucket twice without draining its queue.
        for second in [1, 2] {
            let at = now + Duration::from_secs(second);
            for _ in 0..RATE_BURST {
                assert!(peers.send(first, 0, b"queue", at));
            }
        }
        assert_eq!(first_rx.try_iter().count(), OUTBOX_DEPTH);
        let at = now + Duration::from_secs(3);
        assert!(peers.send(first, 0, b"resumed", at));
        assert_eq!(first_rx.try_recv().unwrap().payload.as_slice(), b"resumed");
    }

    #[test]
    fn removing_a_peer_frees_a_slot_and_retires_its_address() {
        let mut peers = Peers::default();
        let now = Instant::now();
        let slots: Vec<_> = (0..MAX_PEERS)
            .map(|_| peers.register(now).unwrap())
            .collect();
        assert!(peers.register(now).is_none());

        let (old, old_rx) = &slots[0];
        let old_ip = peer_ip(*old);
        peers.remove(*old);
        let (replacement, _) = peers.register(now).unwrap();
        assert_ne!(*old, replacement);
        assert!(!peers.contains_ip(old_ip));
        assert_eq!(peers.source(*old, BASE, 5), None);
        assert!(!peers.send(*old, 5, b"gone", now));
        assert!(peers.broadcast(5, b"remaining", now));
        assert!(matches!(old_rx.try_recv(), Err(TryRecvError::Disconnected)));
        assert_eq!(
            slots[1].1.try_recv().unwrap().payload.as_slice(),
            b"remaining"
        );
    }

    #[test]
    fn stale_ingress_is_skipped_without_another_blocking_wait() {
        let mut peers = Peers::default();
        let (old, _) = peers.register(Instant::now()).unwrap();
        peers.remove(old);
        let bridge = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 43210);
        let mut flags_seen = Vec::new();
        let result = receive_with_bridge(
            bridge,
            &mut [0; 8],
            0,
            |buffer, flags| {
                flags_seen.push(flags);
                if flags_seen.len() == 1 {
                    buffer[..4].copy_from_slice(&old.to_le_bytes());
                    Ok((4, bridge))
                } else {
                    Err(io::ErrorKind::WouldBlock.into())
                }
            },
            |id| peers.source(id, BASE, 5),
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::WouldBlock);
        assert_eq!(flags_seen, [0, MSG_DONTWAIT]);
    }

    #[test]
    fn peek_discards_stale_ingress_and_leaves_the_live_datagram_queued() {
        let mut peers = Peers::default();
        let now = Instant::now();
        let (old, _) = peers.register(now).unwrap();
        let (current, _) = peers.register(now).unwrap();
        peers.remove(old);
        let bridge = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 43210);
        let packet = |id: u32, byte: u8| [id.to_le_bytes().as_slice(), &[byte]].concat();
        let mut input = VecDeque::from([packet(old, 1), packet(current, 2)]);
        let mut data = [0; 8];
        let result = receive_with_bridge(
            bridge,
            &mut data,
            MSG_PEEK,
            |buffer, flags| {
                let packet = input.front().unwrap();
                let len = packet.len();
                buffer[..len].copy_from_slice(packet);
                if flags & MSG_PEEK == 0 {
                    input.pop_front();
                }
                Ok((len, bridge))
            },
            |id| peers.source(id, BASE, 6),
        )
        .unwrap();
        assert_eq!(
            result,
            (
                1,
                SocketAddrV4::new(peer_ip(current), BASE + 6),
                BRIDGE_HEADER
            )
        );
        assert_eq!(data[BRIDGE_HEADER], 2);
        assert_eq!(input, VecDeque::from([packet(current, 2)]));
    }
}
