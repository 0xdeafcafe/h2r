//! Where co-op datagrams go.
//!
//! A [`Link`] is an unreliable datagram path to the one other machine:
//! [`UdpLink`] directly, `relay::RelayLink` through the Live server,
//! [`Fallback`] over the pair, and [`sim_pair`] for tests. A host with
//! several machines around it serves them all from one bound socket
//! ([`UdpPort`]). Nothing above a link assumes a datagram arrives, arrives
//! once or arrives in order.

use std::collections::{HashMap, VecDeque};
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The co-op channel's host UDP port, counted from the XNet base port
/// (`live_hle::NetworkConfig::address`). The title's own channels use base
/// +0, +1, +5 and +6 and the QoS responder +2 (docs/live/live.md, "Client HLE:
/// sockets and System Link"), so a host that forwards base..base+6 for
/// System Link already forwards this one.
pub const COOP_PORT_OFFSET: u16 = 4;

/// A datagram path to the other machine.
pub trait Link {
    /// Sends one datagram, or drops it. Never blocks.
    fn send(&mut self, datagram: &[u8]);
    /// The next datagram that has arrived, if any. Never blocks.
    fn recv(&mut self) -> Option<Vec<u8>>;
}

impl<L: Link + ?Sized> Link for Box<L> {
    fn send(&mut self, datagram: &[u8]) {
        (**self).send(datagram)
    }
    fn recv(&mut self) -> Option<Vec<u8>> {
        (**self).recv()
    }
}

/// Microseconds on some clock: wall time in a game, a counter in tests.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// A clock tests move by hand.
#[derive(Clone, Debug, Default)]
pub struct FakeClock(Arc<AtomicU64>);

impl FakeClock {
    pub fn now(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }

    pub fn advance(&self, micros: u64) {
        self.0.fetch_add(micros, Ordering::Relaxed);
    }

    pub fn clock(&self) -> Clock {
        let c = self.clone();
        Arc::new(move || c.now())
    }
}

/// Wall time since the first call, for real links.
pub fn wall_clock() -> Clock {
    let start = std::time::Instant::now();
    Arc::new(move || start.elapsed().as_micros() as u64)
}

/// The direct path: one UDP socket, talking to one peer.
pub struct UdpLink {
    socket: UdpSocket,
    peer: SocketAddr,
}

impl UdpLink {
    /// Binds `local` and talks to `peer`. Datagrams from anyone else are
    /// dropped.
    pub fn bind(local: SocketAddr, peer: SocketAddr) -> std::io::Result<UdpLink> {
        let socket = UdpSocket::bind(local)?;
        socket.set_nonblocking(true)?;
        Ok(UdpLink { socket, peer })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Changes who this link talks to (the peer's port is known only once
    /// its end is bound).
    pub fn set_peer(&mut self, peer: SocketAddr) {
        self.peer = peer;
    }
}

impl Link for UdpLink {
    fn send(&mut self, datagram: &[u8]) {
        // An error here (no route, a full buffer) is a lost datagram.
        let _ = self.socket.send_to(datagram, self.peer);
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        let mut buf = [0u8; 2048];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((n, from)) if from == self.peer => return Some(buf[..n].to_vec()),
                Ok(_) => continue,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                // WouldBlock, or an ICMP error surfaced by the socket.
                Err(_) => return None,
            }
        }
    }
}

/// Datagrams one [`UdpPort`] queue holds for a peer before new ones drop.
const PORT_QUEUE: usize = 256;
/// Datagrams one `recv` routes from the socket, so a busy peer cannot
/// starve the others of their turn.
const PORT_ROUTE: usize = 64;

struct UdpShared {
    socket: UdpSocket,
    queues: Mutex<HashMap<SocketAddr, VecDeque<Vec<u8>>>>,
}

/// One UDP socket serving every peer of a session: the co-op channel's
/// host end. Each [`link_to`](UdpPort::link_to) is a [`Link`] to one peer;
/// a datagram from an address no link was made for is dropped, as
/// [`UdpLink`] drops a stranger's.
///
/// The host binds its channel once (the XNet base port plus
/// [`COOP_PORT_OFFSET`]); a peer's link sends to that peer's own channel.
/// Any link's `recv` takes datagrams off the shared socket and files each
/// by its source address, so every link is served as long as its peer is
/// pumped.
pub struct UdpPort {
    shared: Arc<UdpShared>,
}

/// One peer's lane of a [`UdpPort`].
pub struct UdpPortLink {
    shared: Arc<UdpShared>,
    peer: SocketAddr,
}

impl UdpPort {
    /// Binds `local` (never blocks; the socket is non-blocking).
    pub fn bind(local: SocketAddr) -> std::io::Result<UdpPort> {
        let socket = UdpSocket::bind(local)?;
        socket.set_nonblocking(true)?;
        Ok(UdpPort {
            shared: Arc::new(UdpShared {
                socket,
                queues: Mutex::new(HashMap::new()),
            }),
        })
    }

    /// The link to `peer`: its sends go out on the shared socket, and its
    /// `recv` answers only with datagrams that came from `peer`.
    pub fn link_to(&self, peer: SocketAddr) -> UdpPortLink {
        self.shared
            .queues
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(peer)
            .or_default();
        UdpPortLink {
            shared: Arc::clone(&self.shared),
            peer,
        }
    }

    /// The bound address (a port of 0 was given to the OS to pick).
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.shared.socket.local_addr()
    }
}

impl Link for UdpPortLink {
    fn send(&mut self, datagram: &[u8]) {
        // An error here (no route, a full buffer) is a lost datagram.
        let _ = self.shared.socket.send_to(datagram, self.peer);
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        let mut buf = [0u8; 2048];
        let mut queues = self.shared.queues.lock().unwrap_or_else(|e| e.into_inner());
        for _ in 0..PORT_ROUTE {
            match self.shared.socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    let queue = queues.entry(from).or_default();
                    if queue.len() < PORT_QUEUE {
                        queue.push_back(buf[..n].to_vec());
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                // WouldBlock, or an ICMP error surfaced by the socket.
                Err(_) => break,
            }
        }
        queues
            .get_mut(&self.peer)
            .and_then(|queue| queue.pop_front())
    }
}

/// Which path a [`Fallback`] is using.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    Direct,
    Relay,
}

/// The direct path while it works, the relay while it does not.
///
/// Every datagram goes out on the direct path, so it is found again the
/// moment it starts working. While nothing has come in on it for `timeout`
/// (and from the start, until something has), every datagram also goes out
/// on the relay. Datagrams are taken from both. The protocol above ignores
/// duplicates, so sending on both for a moment is harmless.
pub struct Fallback<D, R> {
    direct: D,
    relay: R,
    clock: Clock,
    timeout: u64,
    last_direct: Option<u64>,
    /// Datagrams sent on the relay.
    pub relayed: u64,
}

impl<D: Link, R: Link> Fallback<D, R> {
    /// `timeout` is in microseconds of `clock`.
    pub fn new(direct: D, relay: R, clock: Clock, timeout: u64) -> Self {
        Fallback {
            direct,
            relay,
            clock,
            timeout,
            last_direct: None,
            relayed: 0,
        }
    }

    pub fn path(&self) -> Path {
        match self.last_direct {
            Some(at) if (self.clock)().saturating_sub(at) <= self.timeout => Path::Direct,
            _ => Path::Relay,
        }
    }
}

impl<D: Link, R: Link> Link for Fallback<D, R> {
    fn send(&mut self, datagram: &[u8]) {
        self.direct.send(datagram);
        if self.path() == Path::Relay {
            self.relay.send(datagram);
            self.relayed += 1;
        }
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        if let Some(d) = self.direct.recv() {
            self.last_direct = Some((self.clock)());
            return Some(d);
        }
        self.relay.recv()
    }
}

/// What a simulated link does to datagrams. Times are microseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Conditions {
    /// One-way delay every datagram gets.
    pub latency: u64,
    /// Extra delay, uniform in `0..=jitter`, per datagram. Jitter larger
    /// than the gap between sends reorders them.
    pub jitter: u64,
    /// The share of datagrams dropped, 0 to 1.
    pub loss: f64,
    /// The share of datagrams held back a further `latency + jitter`, so
    /// they arrive after later ones.
    pub reorder: f64,
}

struct Wire {
    /// (deliver at, datagram), in send order.
    queue: VecDeque<(u64, Vec<u8>)>,
    rng: u64,
}

struct SimShared {
    clock: FakeClock,
    conditions: Mutex<Conditions>,
    down: AtomicBool,
    /// Datagrams sent, dropped (loss or down) and delivered, both ways.
    sent: AtomicU64,
    dropped: AtomicU64,
    delivered: AtomicU64,
}

/// One end of a simulated link.
pub struct SimLink {
    shared: Arc<SimShared>,
    out: Arc<Mutex<Wire>>,
    inbound: Arc<Mutex<Wire>>,
}

/// A handle on a simulated link, to change it while the ends are in use.
#[derive(Clone)]
pub struct SimControl(Arc<SimShared>);

impl SimControl {
    /// While down, every datagram sent is dropped.
    pub fn set_down(&self, down: bool) {
        self.0.down.store(down, Ordering::Relaxed);
    }

    pub fn set_conditions(&self, c: Conditions) {
        *self.0.conditions.lock().unwrap() = c;
    }

    pub fn sent(&self) -> u64 {
        self.0.sent.load(Ordering::Relaxed)
    }

    pub fn dropped(&self) -> u64 {
        self.0.dropped.load(Ordering::Relaxed)
    }

    pub fn delivered(&self) -> u64 {
        self.0.delivered.load(Ordering::Relaxed)
    }
}

/// Two ends of a link with `conditions`, on `clock`. The same `seed` gives
/// the same drops and delays.
pub fn sim_pair(
    clock: &FakeClock,
    conditions: Conditions,
    seed: u64,
) -> (SimLink, SimLink, SimControl) {
    let shared = Arc::new(SimShared {
        clock: clock.clone(),
        conditions: Mutex::new(conditions),
        down: AtomicBool::new(false),
        sent: AtomicU64::new(0),
        dropped: AtomicU64::new(0),
        delivered: AtomicU64::new(0),
    });
    let wire = |salt: u64| {
        Arc::new(Mutex::new(Wire {
            queue: VecDeque::new(),
            rng: (seed ^ salt) | 1,
        }))
    };
    let (ab, ba) = (wire(0x9e37_79b9_7f4a_7c15), wire(0xc2b2_ae3d_27d4_eb4f));
    (
        SimLink {
            shared: Arc::clone(&shared),
            out: Arc::clone(&ab),
            inbound: Arc::clone(&ba),
        },
        SimLink {
            shared: Arc::clone(&shared),
            out: ba,
            inbound: ab,
        },
        SimControl(shared),
    )
}

/// xorshift64*: a value in 0..1.
fn unit(rng: &mut u64) -> f64 {
    *rng ^= *rng >> 12;
    *rng ^= *rng << 25;
    *rng ^= *rng >> 27;
    (rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11) as f64 / (1u64 << 53) as f64
}

impl Link for SimLink {
    fn send(&mut self, datagram: &[u8]) {
        let s = &self.shared;
        s.sent.fetch_add(1, Ordering::Relaxed);
        let c = *s.conditions.lock().unwrap();
        let mut wire = self.out.lock().unwrap();
        // Draw the same numbers whether or not the link is down, so taking
        // it down does not change what happens to later datagrams.
        let (lose, jitter, hold) = (
            unit(&mut wire.rng),
            unit(&mut wire.rng),
            unit(&mut wire.rng),
        );
        if s.down.load(Ordering::Relaxed) || lose < c.loss {
            s.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let mut delay = c.latency + (jitter * (c.jitter + 1) as f64) as u64;
        if hold < c.reorder {
            delay += c.latency + c.jitter;
        }
        wire.queue
            .push_back((s.clock.now() + delay, datagram.to_vec()));
    }

    fn recv(&mut self) -> Option<Vec<u8>> {
        let now = self.shared.clock.now();
        let mut wire = self.inbound.lock().unwrap();
        // The earliest due datagram, which need not be the first sent.
        let (at, _) = wire
            .queue
            .iter()
            .enumerate()
            .filter(|(_, (due, _))| *due <= now)
            .min_by_key(|(i, (due, _))| (*due, *i))?;
        self.shared.delivered.fetch_add(1, Ordering::Relaxed);
        wire.queue.remove(at).map(|(_, d)| d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(l: &mut impl Link) -> Vec<Vec<u8>> {
        std::iter::from_fn(|| l.recv()).collect()
    }

    #[test]
    fn a_simulated_link_delays_by_its_latency() {
        let clock = FakeClock::default();
        let (mut a, mut b, _) = sim_pair(
            &clock,
            Conditions {
                latency: 30_000,
                ..Default::default()
            },
            1,
        );
        a.send(b"one");
        clock.advance(29_999);
        assert_eq!(b.recv(), None);
        clock.advance(1);
        assert_eq!(b.recv().as_deref(), Some(&b"one"[..]));
        assert_eq!(b.recv(), None);
        assert_eq!(a.recv(), None, "nothing comes back on its own");
        b.send(b"two");
        clock.advance(30_000);
        assert_eq!(a.recv().as_deref(), Some(&b"two"[..]));
    }

    #[test]
    fn loss_jitter_and_reordering_happen_and_repeat_by_seed() {
        let run = |seed| {
            let clock = FakeClock::default();
            let (mut a, mut b, control) = sim_pair(
                &clock,
                Conditions {
                    latency: 20_000,
                    jitter: 30_000,
                    loss: 0.25,
                    reorder: 0.1,
                },
                seed,
            );
            let mut got = Vec::new();
            for n in 0..400u32 {
                a.send(&n.to_le_bytes());
                clock.advance(5_000);
                got.extend(drain(&mut b));
            }
            clock.advance(1_000_000);
            got.extend(drain(&mut b));
            let got: Vec<u32> = got
                .iter()
                .map(|d| u32::from_le_bytes(d[..].try_into().unwrap()))
                .collect();
            assert_eq!(control.sent(), 400);
            assert_eq!(control.delivered() + control.dropped(), 400);
            got
        };
        let got = run(7);
        assert!(
            (250..350).contains(&got.len()),
            "about a quarter lost: {}",
            got.len()
        );
        assert!(
            got.windows(2).any(|w| w[0] > w[1]),
            "some arrive out of order"
        );
        let mut sorted = got.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), got.len(), "nothing is duplicated");
        assert_eq!(run(7), got, "the same seed repeats");
        assert_ne!(run(8), got);
    }

    #[test]
    fn a_link_that_is_down_drops_everything() {
        let clock = FakeClock::default();
        let (mut a, mut b, control) = sim_pair(&clock, Conditions::default(), 3);
        control.set_down(true);
        a.send(b"lost");
        control.set_down(false);
        a.send(b"kept");
        assert_eq!(drain(&mut b), [b"kept".to_vec()]);
        assert_eq!(control.dropped(), 1);
    }

    #[test]
    fn udp_links_talk_only_to_their_peer() {
        let any: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut a = UdpLink::bind(any, any).unwrap();
        let mut b = UdpLink::bind(any, a.local_addr().unwrap()).unwrap();
        a.set_peer(b.local_addr().unwrap());
        let stranger = UdpSocket::bind(any).unwrap();
        stranger.send_to(b"hello", a.local_addr().unwrap()).unwrap();
        b.send(b"from b");
        let mut got = None;
        for _ in 0..200 {
            got = a.recv();
            if got.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got.as_deref(), Some(&b"from b"[..]));
        assert_eq!(a.recv(), None, "the stranger's datagram was dropped");
    }

    #[test]
    fn a_port_files_datagrams_by_their_source_and_drops_strangers() {
        let any: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let port = UdpPort::bind(any).unwrap();
        let mut to_first = port.link_to(any);
        let mut to_second = port.link_to(any);
        let mut first = UdpLink::bind(any, port.local_addr().unwrap()).unwrap();
        let mut second = UdpLink::bind(any, port.local_addr().unwrap()).unwrap();
        to_first.peer = first.local_addr().unwrap();
        to_second.peer = second.local_addr().unwrap();
        // Each peer's datagram answers only its own link, whoever recv'd.
        first.send(b"one");
        second.send(b"two");
        let stranger = UdpSocket::bind(any).unwrap();
        stranger.send_to(b"who", port.local_addr().unwrap()).unwrap();
        let drain = |l: &mut UdpPortLink| std::iter::from_fn(|| l.recv()).collect::<Vec<_>>();
        assert_eq!(drain(&mut to_second), [b"two".to_vec()]);
        assert_eq!(drain(&mut to_first), [b"one".to_vec()]);
        // A link's send goes to its own peer, from the one shared socket.
        to_first.send(b"back one");
        to_second.send(b"back two");
        assert_eq!(first.recv().as_deref(), Some(&b"back one"[..]));
        assert_eq!(second.recv().as_deref(), Some(&b"back two"[..]));
        assert_eq!(first.recv(), None);
        assert_eq!(to_first.recv(), None, "the stranger's datagram was dropped");
    }

    #[test]
    fn a_port_link_send_fits_the_datagram_limit() {
        // One co-op datagram must cross a port link whole (wire::MAX_DATAGRAM).
        let any: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let port = UdpPort::bind(any).unwrap();
        let mut link = port.link_to(any);
        let mut peer = UdpLink::bind(any, port.local_addr().unwrap()).unwrap();
        link.peer = peer.local_addr().unwrap();
        let datagram = vec![0xabu8; crate::wire::MAX_DATAGRAM];
        link.send(&datagram);
        assert_eq!(peer.recv(), Some(datagram));
    }

    #[test]
    fn fallback_uses_the_relay_only_while_direct_is_silent() {
        let clock = FakeClock::default();
        let (da, db, direct) = sim_pair(&clock, Conditions::default(), 1);
        let (ra, rb, relay) = sim_pair(&clock, Conditions::default(), 2);
        let mut a = Fallback::new(da, ra, clock.clock(), 500_000);
        let mut b = Fallback::new(db, rb, clock.clock(), 500_000);
        direct.set_down(true);
        // Nothing has been heard on direct: both paths carry it.
        assert_eq!(a.path(), Path::Relay);
        a.send(b"1");
        assert_eq!(drain(&mut b), [b"1".to_vec()]);
        assert_eq!((b.path(), relay.delivered()), (Path::Relay, 1));
        // Direct comes up: the first datagram heard on it switches over.
        direct.set_down(false);
        a.send(b"2");
        assert_eq!(
            drain(&mut b),
            [b"2".to_vec(), b"2".to_vec()],
            "once on each path"
        );
        assert_eq!(b.path(), Path::Direct);
        let before = relay.sent();
        b.send(b"3");
        assert_eq!(relay.sent(), before, "direct only");
        assert_eq!(drain(&mut a), [b"3".to_vec()]);
        // Direct goes quiet for longer than the timeout: back to the relay.
        direct.set_down(true);
        clock.advance(500_001);
        assert_eq!(b.path(), Path::Relay);
        b.send(b"4");
        assert_eq!(drain(&mut a), [b"4".to_vec()]);
        assert!(b.relayed >= 1);
    }
}
