//! Client end of a dedicated host tunnel. The join worker performs the upgrade
//! and hello. A single I/O thread owns the WebSocket for the rest of the match.

use std::{
    collections::HashMap,
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, ToSocketAddrs, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    time::{Duration, Instant},
};

use live_proto::tunnel::{self, Frame, Kind, MAX_PAYLOAD, MAX_PORT};
use tungstenite::{
    Message, WebSocket,
    client::IntoClientRequest,
    handshake::client::Request,
    http::{HeaderValue, header::AUTHORIZATION},
    protocol::WebSocketConfig,
    stream::MaybeTlsStream,
};

const TURN: Duration = Duration::from_millis(2);
const CONNECT_WAIT: Duration = Duration::from_secs(5);
const KEEPALIVE: Duration = Duration::from_secs(5);
const QUEUE: usize = 256;

pub(crate) fn port_offset(base: SocketAddrV4, dest: SocketAddrV4) -> Option<u16> {
    if base.ip() != dest.ip() {
        return None;
    }
    dest.port()
        .checked_sub(base.port())
        .filter(|p| *p <= MAX_PORT)
}

/// Where the host's datagram enters a bound guest socket on this runtime.
pub(crate) fn channel_destination(
    config: &crate::NetworkConfig,
    bound: SocketAddrV4,
    offset: u16,
) -> Option<SocketAddrV4> {
    if offset > MAX_PORT || bound.port() != 1000 + offset {
        return None;
    }
    let mut dest = config.local(bound);
    if dest.ip().is_unspecified() {
        dest.set_ip(Ipv4Addr::LOCALHOST);
    }
    Some(dest)
}

#[derive(Clone, Copy)]
struct Hello {
    peer: u32,
    host: Ipv4Addr,
}

type Inject = Box<dyn Fn(&UdpSocket, u16, &[u8]) + Send + Sync>;

struct Shared {
    hello: Mutex<Hello>,
    inject: Mutex<Option<Inject>>,
    stopping: AtomicBool,
    closed: AtomicBool,
}

struct Client {
    out: SyncSender<Frame>,
    shared: Arc<Shared>,
    injector: SocketAddrV4,
    qos_base: SocketAddrV4,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.shared.stopping.store(true, Ordering::Release);
    }
}

/// The connection lives until its last handle is dropped, or the host ends it.
#[derive(Clone)]
pub struct TunnelHandle(Arc<Client>);

impl std::fmt::Debug for TunnelHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TunnelHandle")
            .field("host", &self.host_ip())
            .field("peer", &self.peer_id())
            .field("closed", &self.closed())
            .finish()
    }
}

impl TunnelHandle {
    pub fn host_ip(&self) -> Ipv4Addr {
        self.0.shared.hello.lock().unwrap().host
    }

    pub fn peer_id(&self) -> u32 {
        self.0.shared.hello.lock().unwrap().peer
    }

    pub fn closed(&self) -> bool {
        self.0.shared.closed.load(Ordering::Acquire)
            || self.0.shared.stopping.load(Ordering::Acquire)
    }

    /// Loopback base for the existing QoS lookup, including preflight before
    /// the join result reaches the HLE. The responder is at this base + 2.
    pub fn qos_base(&self) -> SocketAddrV4 {
        self.0.qos_base
    }

    pub(crate) fn same_connection(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn injector(&self) -> SocketAddrV4 {
        self.0.injector
    }

    pub(crate) fn on_datagram(&self, inject: Inject) {
        *self.0.shared.inject.lock().unwrap() = Some(inject);
    }

    pub(crate) fn stop(&self) {
        self.0.shared.stopping.store(true, Ordering::Release);
    }

    pub(crate) fn send(&self, port: u16, payload: &[u8]) -> Result<usize, u32> {
        if self.closed() {
            return Err(10054);
        }
        if port > MAX_PORT {
            return Err(10022);
        }
        if payload.len() > MAX_PAYLOAD {
            return Err(10040); // WSAEMSGSIZE
        }
        self.0
            .out
            .try_send(Frame {
                kind: Kind::Datagram,
                port,
                peer: 0,
                payload: payload.to_vec(),
            })
            .map_err(|e| match e {
                TrySendError::Full(_) => 10035u32,      // WSAEWOULDBLOCK
                TrySendError::Disconnected(_) => 10054, // WSAECONNRESET
            })?;
        Ok(payload.len())
    }
}

/// Opens a connection on the caller's thread. Use the Live join worker, so
/// TCP, TLS and the host's hello never hold up a guest socket call.
pub fn connect_tunnel(url: &str, bearer: &str) -> Result<TunnelHandle, String> {
    connect(url, bearer).map_err(|e| e.to_string())
}

fn connect(url: &str, bearer: &str) -> io::Result<TunnelHandle> {
    let mut socket = open(url, bearer)?;
    let hello = handshake(&mut socket)?;
    let injector = loopback_socket()?;
    let injection_addr = local_addr(&injector)?;
    let qos = QosBridge::new()?;
    let qos_base = qos.base;
    let (out, rx) = sync_channel(QUEUE);
    let shared = Arc::new(Shared {
        hello: Mutex::new(hello),
        inject: Mutex::new(None),
        stopping: AtomicBool::new(false),
        closed: AtomicBool::new(false),
    });
    let worker = shared.clone();
    let (url, bearer) = (url.to_owned(), bearer.to_owned());
    std::thread::Builder::new()
        .name("xnet-tunnel".into())
        .spawn(move || {
            let mut qos = qos;
            run_with_retry(
                socket,
                hello,
                || open(&url, &bearer),
                |socket, hello| {
                    *worker.hello.lock().unwrap() = hello;
                    run(socket, hello, &worker, &rx, &injector, &mut qos)
                },
                || worker.stopping.load(Ordering::Acquire),
            );
            worker.closed.store(true, Ordering::Release);
        })?;
    Ok(TunnelHandle(Arc::new(Client {
        out,
        shared,
        injector: injection_addr,
        qos_base,
    })))
}

fn loopback_socket() -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

fn local_addr(socket: &UdpSocket) -> io::Result<SocketAddrV4> {
    match socket.local_addr()? {
        SocketAddr::V4(addr) => Ok(addr),
        SocketAddr::V6(_) => Err(io::Error::other("expected IPv4 loopback socket")),
    }
}

fn request(url: &str, bearer: &str) -> io::Result<Request> {
    let mut request = url
        .into_client_request()
        .map_err(|_| invalid("invalid tunnel URL"))?;
    let mut header = HeaderValue::try_from(format!("Bearer {bearer}"))
        .map_err(|_| invalid("invalid tunnel authorization header"))?;
    header.set_sensitive(true);
    request.headers_mut().insert(AUTHORIZATION, header);
    Ok(request)
}

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

fn open(url: &str, bearer: &str) -> io::Result<Ws> {
    open_with_timeouts(url, bearer, CONNECT_WAIT, TURN)
}

/// Measure only the first application frame on a fresh authenticated upgrade.
pub(crate) fn region_echo(url: &str, bearer: &str) -> Option<u16> {
    use live_proto::matchmaking::REGION_ECHO;
    let mut socket = open_with_timeouts(url, bearer, Duration::from_millis(1000), Duration::from_millis(450)).ok()?;
    let started = Instant::now();
    socket.send(Message::Text(REGION_ECHO.into())).ok()?;
    match socket.read().ok()? {
        Message::Text(text) if text.as_str() == REGION_ECHO => {
            Some(crate::qos::round_trip_ms(started.elapsed()))
        }
        _ => None,
    }
}

fn open_with_timeouts(url: &str, bearer: &str, connect_wait: Duration, read_wait: Duration) -> io::Result<Ws> {
    let req = request(url, bearer)?;
    let uri = req.uri();
    let default_port = match uri.scheme_str() {
        Some("ws") => 80,
        Some("wss") => 443,
        _ => return Err(invalid("expected ws or wss tunnel URL")),
    };
    let name = uri.host().ok_or_else(|| invalid("missing tunnel host"))?;
    let name = name.trim_start_matches('[').trim_end_matches(']');
    let addrs = (name, uri.port_u16().unwrap_or(default_port)).to_socket_addrs()?;
    let deadline = Instant::now() + connect_wait;
    let mut result = Err(io::Error::other("tunnel host has no address"));
    for addr in addrs {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        result = TcpStream::connect_timeout(&addr, left);
        if result.is_ok() {
            break;
        }
    }
    let stream = result?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(connect_wait))?;
    stream.set_write_timeout(Some(connect_wait))?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(tunnel::HEADER_LEN + MAX_PAYLOAD))
        .max_frame_size(Some(tunnel::HEADER_LEN + MAX_PAYLOAD));
    // A single upgrade to this URL. A redirect must never receive the bearer.
    let (mut socket, _) = tungstenite::client_tls_with_config(req, stream, Some(config), None)
        .map_err(|err| match err {
            tungstenite::handshake::HandshakeError::Failure(tungstenite::Error::Http(reply)) => {
                io::Error::other(format!(
                    "dedicated tunnel upgrade returned HTTP {}",
                    reply.status()
                ))
            }
            tungstenite::handshake::HandshakeError::Failure(tungstenite::Error::Io(err)) => err,
            _ => io::Error::other("dedicated tunnel upgrade failed"),
        })?;
    let tcp = match socket.get_mut() {
        MaybeTlsStream::Plain(tcp) => tcp,
        MaybeTlsStream::Rustls(tls) => &mut tls.sock,
        _ => return Err(io::Error::other("unsupported tunnel TLS backend")),
    };
    tcp.set_read_timeout(Some(read_wait))?;
    Ok(socket)
}

trait Wire {
    fn send(&mut self, message: Message) -> io::Result<()>;
    fn read(&mut self) -> io::Result<Option<Message>>;
    fn close(&mut self);
}

fn ws_error(err: tungstenite::Error) -> io::Error {
    match err {
        tungstenite::Error::Io(err) => err,
        tungstenite::Error::ConnectionClosed
        | tungstenite::Error::AlreadyClosed
        | tungstenite::Error::Protocol(
            tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
        ) => io::ErrorKind::UnexpectedEof.into(),
        _ => invalid("invalid WebSocket message"),
    }
}

impl Wire for Ws {
    fn send(&mut self, message: Message) -> io::Result<()> {
        WebSocket::send(self, message).map_err(ws_error)
    }

    fn read(&mut self) -> io::Result<Option<Message>> {
        match WebSocket::read(self) {
            Ok(message) => {
                // tungstenite queued the pong / close response during read.
                if matches!(message, Message::Ping(_) | Message::Close(_)) {
                    self.flush().map_err(ws_error)?;
                }
                Ok(Some(message))
            }
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(ws_error(e)),
        }
    }

    fn close(&mut self) {
        let _ = WebSocket::close(self, None);
        let _ = self.flush();
    }
}

fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

fn client_control(kind: Kind) -> Message {
    Message::binary(tunnel::encode(&Frame {
        kind,
        port: 0,
        peer: 0,
        payload: vec![],
    }))
}

fn handshake(socket: &mut impl Wire) -> io::Result<Hello> {
    socket.send(client_control(Kind::Hello))?;
    let until = Instant::now() + CONNECT_WAIT;
    while Instant::now() < until {
        match socket.read()? {
            None | Some(Message::Ping(_) | Message::Pong(_)) => continue,
            Some(Message::Binary(bytes)) => {
                let frame =
                    tunnel::decode(&bytes).map_err(|_| invalid("malformed tunnel hello"))?;
                if frame.kind == Kind::Hello
                    && frame.port == 0
                    && frame.peer != 0
                    && let [10, 77, x, y] = frame.payload.as_slice()
                {
                    return Ok(Hello {
                        peer: frame.peer,
                        host: Ipv4Addr::new(10, 77, *x, *y),
                    });
                }
            }
            _ => {}
        }
        return Err(invalid(
            "expected host hello with assigned peer and synthetic IPv4",
        ));
    }
    Err(io::ErrorKind::TimedOut.into())
}

enum Incoming {
    Datagram(Frame),
    Bye,
    Control,
}

fn incoming(hello: Hello, message: Message) -> io::Result<Incoming> {
    match message {
        Message::Binary(bytes) => {
            let frame = tunnel::decode(&bytes).map_err(|_| invalid("malformed tunnel frame"))?;
            if frame.peer != hello.peer {
                return Err(invalid("tunnel peer mismatch"));
            }
            match frame.kind {
                Kind::Datagram => Ok(Incoming::Datagram(frame)),
                Kind::Bye if frame.port == 0 && frame.payload.is_empty() => Ok(Incoming::Bye),
                _ => Err(invalid("unexpected tunnel control frame")),
            }
        }
        Message::Close(None) => Err(io::ErrorKind::UnexpectedEof.into()),
        Message::Close(Some(reason)) => match u16::from(reason.code) {
            1001 | 1011 | 1012 | 1013 => Err(io::ErrorKind::ConnectionAborted.into()),
            _ => Ok(Incoming::Bye),
        },
        Message::Ping(_) | Message::Pong(_) => Ok(Incoming::Control),
        _ => Err(invalid("expected binary tunnel frame")),
    }
}

fn run_with_retry<W: Wire>(
    mut socket: W,
    hello: Hello,
    open: impl FnOnce() -> io::Result<W>,
    mut session: impl FnMut(&mut W, Hello) -> io::Result<()>,
    stopping: impl Fn() -> bool,
) {
    if let Err(e) = session(&mut socket, hello)
        && e.kind() != io::ErrorKind::InvalidData
        && !stopping()
    {
        drop(socket);
        if let Ok(mut fresh) = open()
            && let Ok(next) = handshake(&mut fresh)
            && next.host == hello.host
        {
            let _ = session(&mut fresh, next);
        }
    }
}

struct Keepalive {
    next_ping: Instant,
    last_seen: Instant,
}

impl Keepalive {
    fn new(now: Instant) -> Self {
        Self {
            next_ping: now + KEEPALIVE,
            last_seen: now,
        }
    }

    fn heard(&mut self, now: Instant) {
        self.last_seen = now;
    }

    fn tick(&mut self, socket: &mut impl Wire, now: Instant) -> io::Result<()> {
        if now.duration_since(self.last_seen) >= KEEPALIVE * 3 {
            return Err(io::ErrorKind::TimedOut.into());
        }
        if now >= self.next_ping {
            socket.send(Message::Ping(Default::default()))?;
            self.next_ping = now + KEEPALIVE;
        }
        Ok(())
    }
}

fn run(
    socket: &mut impl Wire,
    hello: Hello,
    shared: &Shared,
    out: &Receiver<Frame>,
    injector: &UdpSocket,
    qos: &mut QosBridge,
) -> io::Result<()> {
    let mut keepalive = Keepalive::new(Instant::now());
    while !shared.stopping.load(Ordering::Acquire) {
        // Bound each turn so continuous game traffic cannot starve receives.
        for frame in out.try_iter().take(32) {
            socket.send(Message::binary(tunnel::encode(&frame)))?;
        }
        qos.poll(socket)?;
        keepalive.tick(socket, Instant::now())?;
        if let Some(msg) = socket.read()? {
            keepalive.heard(Instant::now());
            match incoming(hello, msg)? {
                Incoming::Control => {}
                Incoming::Bye => {
                    socket.close();
                    return Ok(());
                }
                Incoming::Datagram(f) => {
                    if f.port == 2 && qos.reply(&f.payload) {
                        continue;
                    }
                    if let Some(deliver) = &*shared.inject.lock().unwrap() {
                        deliver(injector, f.port, &f.payload);
                    }
                }
            }
        }
    }
    let result = socket.send(client_control(Kind::Bye));
    socket.close();
    result
}

// The existing title QoS lookup uses an ephemeral UDP socket for each probe.
// Forward its known request on +2; concurrent replies return to the probe
// with the matching nonce, even when replies arrive out of order.
struct QosBridge {
    socket: UdpSocket,
    base: SocketAddrV4,
    pending: HashMap<[u8; 8], (SocketAddr, Instant)>,
}

impl QosBridge {
    fn new() -> io::Result<Self> {
        let socket = loopback_socket()?;
        let mut base = local_addr(&socket)?;
        base.set_port(
            base.port()
                .checked_sub(2)
                .ok_or_else(|| invalid("invalid local QoS port"))?,
        );
        Ok(Self {
            socket,
            base,
            pending: HashMap::new(),
        })
    }

    fn poll(&mut self, wire: &mut impl Wire) -> io::Result<()> {
        // One byte past the valid size detects a truncated/oversized request.
        let mut buf = [0; 25];
        match self.socket.recv_from(&mut buf) {
            Ok((24, addr)) if addr.ip().is_loopback() && &buf[..8] == b"H2QOS001" => {
                self.pending
                    .retain(|_, (_, at)| at.elapsed() < CONNECT_WAIT);
                if self.pending.len() < 64 {
                    self.pending
                        .insert(buf[8..16].try_into().unwrap(), (addr, Instant::now()));
                    wire.send(Message::binary(tunnel::encode(&Frame {
                        kind: Kind::Datagram,
                        port: 2,
                        peer: 0,
                        payload: buf[..24].to_vec(),
                    })))?;
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(e),
        }
        Ok(())
    }

    fn reply(&mut self, payload: &[u8]) -> bool {
        if payload.len() < 16 || &payload[..8] != b"H2QOR001" {
            return false;
        }
        let nonce: [u8; 8] = payload[8..16].try_into().unwrap();
        if let Some((addr, at)) = self.pending.remove(&nonce) {
            if at.elapsed() < CONNECT_WAIT {
                let _ = self.socket.send_to(payload, addr);
            }
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests;
