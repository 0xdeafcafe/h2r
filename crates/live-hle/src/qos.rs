//! HLE-only QoS side channel. This is not the Xbox encrypted wire protocol.
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
const QUERY: &[u8; 8] = b"H2QOS001";
const REPLY: &[u8; 8] = b"H2QOR001";
/// A co-op ping (docs/live/live-coop.md, "Ping"): magic, nonce, then the
/// sender's own last round trip to us in ms (0: none yet), so the host
/// learns its guest's measurement from the guest's next ping. Answered by
/// any responder, for any sender: it carries nothing but the echo.
const PING: &[u8; 8] = b"H2PNG001";
const PONG: &[u8; 8] = b"H2PNR001";
pub(crate) const PING_LEN: usize = 18;
pub(crate) const PONG_LEN: usize = 16;
/// The session each peer last asked about and got an answer for, by its
/// address: what a peer first heard from on a game channel is joining.
pub(crate) type Asked = Arc<Mutex<BTreeMap<Ipv4Addr, [u8; 8]>>>;
/// What pinging peers reported, by their address: (ms, when).
pub(crate) type Pings = Arc<Mutex<BTreeMap<Ipv4Addr, (u16, Instant)>>>;
pub(crate) fn ping_packet(nonce: u64, report: u16) -> [u8; PING_LEN] {
    let mut p = [0; PING_LEN];
    p[..8].copy_from_slice(PING);
    p[8..16].copy_from_slice(&nonce.to_le_bytes());
    p[16..18].copy_from_slice(&report.to_le_bytes());
    p
}
/// The responder's side of a ping: the echo to send back, after noting
/// what the sender reported.
pub(crate) fn answer_ping(input: &[u8], sender: Ipv4Addr, pings: &Pings) -> Option<[u8; PONG_LEN]> {
    if input.len() != PING_LEN || &input[..8] != PING {
        return None;
    }
    let report = u16::from_le_bytes([input[16], input[17]]);
    if report != 0 {
        pings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(sender, (report, Instant::now()));
    }
    let mut out = [0; PONG_LEN];
    out[..8].copy_from_slice(PONG);
    out[8..16].copy_from_slice(&input[8..16]);
    Some(out)
}
/// One ping to the responder at `address` (the peer's QoS port), reporting
/// `report`: the round trip, if the echo came back within `timeout`.
pub(crate) fn ping(address: SocketAddrV4, report: u16, timeout: Duration) -> Option<Duration> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect(address).ok()?;
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
    let started = Instant::now();
    socket.send(&ping_packet(nonce, report)).ok()?;
    let mut input = [0; 64];
    loop {
        let left = timeout.checked_sub(started.elapsed())?;
        socket
            .set_read_timeout(Some(left.max(Duration::from_millis(1))))
            .ok()?;
        let n = socket.recv(&mut input).ok()?;
        if n == PONG_LEN && &input[..8] == PONG && input[8..16] == nonce.to_le_bytes() {
            return Some(started.elapsed());
        }
    }
}
/// Probe the queue's targets concurrently on one socket. Accept an echo
/// only from the endpoint and for the nonce sent for that target.
pub(crate) fn measure(
    candidates: Vec<live_proto::matchmaking::Candidate>,
    timeout: Duration,
) -> std::io::Result<Vec<live_proto::matchmaking::Sample>> {
    use live_proto::matchmaking::Sample;
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    let started = Instant::now();
    let mut pending = BTreeMap::new();
    let mut samples = Vec::with_capacity(candidates.len());
    for target in candidates {
        let addr = SocketAddrV4::new(*target.endpoint.ip(), target.endpoint.port() + 2);
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        let sent = Instant::now();
        if socket.send_to(&ping_packet(nonce, 0), addr).is_ok() {
            pending.insert(nonce, (addr, sent, samples.len()));
        }
        samples.push(Sample {
            target: target.xuid,
            generation: target.generation,
            rtt_ms: None,
        });
    }
    let mut buffer = [0; 64];
    while !pending.is_empty() {
        let Some(left) = timeout.checked_sub(started.elapsed()) else {
            break;
        };
        socket.set_read_timeout(Some(left.max(Duration::from_millis(1))))?;
        let (len, sender) = match socket.recv_from(&mut buffer) {
            Ok(packet) => packet,
            Err(e) if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) => break,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if len != PONG_LEN || &buffer[..8] != PONG {
            continue;
        }
        let nonce = u64::from_le_bytes(buffer[8..16].try_into().unwrap());
        if let Some(&(addr, sent, index)) = pending.get(&nonce)
            && sender == SocketAddr::V4(addr)
        {
            samples[index].rtt_ms = Some(round_trip_ms(sent.elapsed()));
            pending.remove(&nonce);
        }
    }
    Ok(samples)
}

/// A round trip in whole milliseconds as sessions carry it: at least 1.
pub fn round_trip_ms(d: Duration) -> u16 {
    u16::try_from(d.as_millis())
        .unwrap_or(u16::MAX)
        .clamp(1, live_proto::session::PING_MAX_MS)
}
static NONCE: AtomicU64 = AtomicU64::new(1);
#[derive(Clone, Default)]
pub(crate) struct Listener {
    pub enabled: bool,
    pub data: Vec<u8>,
    pub bandwidth: u32,
    /// When its data was last set non-empty (a running count), so the
    /// newest lobby data wins ([`answer`]).
    pub data_set: u64,
}
/// What the responder answers about `kid`: (enabled, bandwidth, data), or
/// None for a session this console doesn't listen for.
///
/// A host's lobby has two ids: the XNKID the title made for its squad and
/// listens on with no data, and the session id the server gave its
/// advertisement, which gets the lobby data. A joining guest probes the
/// squad's id and the title's join (`0x0006f5f5`) fails on no data
/// ("network problems", `error_join_data_decode_failure`). So an enabled
/// listener without data answers with the newest data this console
/// listens with.
pub(crate) fn answer(
    listeners: &BTreeMap<[u8; 8], Listener>,
    kid: &[u8; 8],
) -> Option<(bool, u32, Vec<u8>)> {
    let entry = listeners.get(kid)?;
    if !entry.enabled {
        return Some((false, entry.bandwidth, Vec::new()));
    }
    let data = if entry.data.is_empty() {
        listeners
            .values()
            .filter(|l| l.enabled && !l.data.is_empty())
            .max_by_key(|l| l.data_set)
            .map(|l| l.data.clone())
            .unwrap_or_default()
    } else {
        entry.data.clone()
    };
    Some((true, entry.bandwidth, data))
}
#[derive(Default)]
pub(crate) struct Info {
    pub flags: u8,
    pub probes: u16,
    pub rtt: u16,
    pub bandwidth: u32,
    pub data: Vec<u8>,
}
pub(crate) type Listeners = Arc<Mutex<BTreeMap<[u8; 8], Listener>>>;
pub(crate) struct Responder {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Responder {
    pub fn start(
        address: SocketAddrV4,
        listeners: Listeners,
        pings: Pings,
        asked: Asked,
    ) -> std::io::Result<Self> {
        let socket = UdpSocket::bind((address.ip().to_owned(), address.port() + 2))?;
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("h2-xnet-qos".into())
            .spawn(move || {
                let mut input = [0; 24];
                // Each session a peer asked for that this console doesn't
                // listen for, said once: the usual cause of the title's
                // "network problems" join failure.
                let mut unknown = std::collections::BTreeSet::new();
                while !flag.load(Ordering::Acquire) {
                    let Ok((n, sender)) = socket.recv_from(&mut input) else {
                        continue;
                    };
                    if n == PING_LEN {
                        if let SocketAddr::V4(v4) = sender
                            && let Some(out) = answer_ping(&input[..n], *v4.ip(), &pings)
                        {
                            let _ = socket.send_to(&out, sender);
                        }
                        continue;
                    }
                    if n != 24 || &input[..8] != QUERY {
                        continue;
                    }
                    let kid: [u8; 8] = input[16..24].try_into().unwrap();
                    let entry = answer(&listeners.lock().unwrap_or_else(|e| e.into_inner()), &kid);
                    if entry.is_some()
                        && let SocketAddr::V4(v4) = sender
                    {
                        asked
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .insert(*v4.ip(), kid);
                    }
                    let Some((enabled, bandwidth, data)) = entry else {
                        if unknown.insert(kid) {
                            let ours: Vec<_> = listeners
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .keys()
                                .map(|k| live_proto::sign::hex(k))
                                .collect();
                            eprintln!(
                                "live: qos: {sender} asked about session {}, which this console \
                                 isn't listening for (listening: {}); no answer",
                                live_proto::sign::hex(&kid),
                                if ours.is_empty() {
                                    "none".to_owned()
                                } else {
                                    ours.join(", ")
                                }
                            );
                        }
                        continue;
                    };
                    let mut response = Vec::with_capacity(23 + data.len());
                    response.extend_from_slice(REPLY);
                    response.extend_from_slice(&input[8..16]);
                    response.push(if enabled {
                        3 | if data.is_empty() { 0 } else { 8 }
                    } else {
                        7
                    });
                    response.extend_from_slice(&bandwidth.to_le_bytes());
                    response.extend_from_slice(&(data.len() as u16).to_le_bytes());
                    response.extend_from_slice(&data);
                    let _ = socket.send_to(&response, sender);
                }
            })?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for Responder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
/// Tries per QoS lookup target. A host under load (three games on one Mac
/// run at a few frames a second) can miss one short window; a host that
/// doesn't listen for the session never answers.
pub(crate) const QUERY_TRIES: u32 = 3;
const QUERY_WAIT: Duration = Duration::from_millis(100);
/// Asks the responder at `address` (its QoS port, + 2) about session
/// `kid`; the error is the last try's.
pub(crate) fn query_why(address: SocketAddrV4, kid: [u8; 8], probes: u16) -> std::io::Result<Info> {
    let mut last = std::io::Error::from(std::io::ErrorKind::TimedOut);
    for _ in 0..QUERY_TRIES {
        match query_once(address, kid, probes) {
            Ok(info) => return Ok(info),
            // Nothing listens there: another try can't help.
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => return Err(e),
            Err(e) => last = e,
        }
    }
    Err(last)
}
fn query_once(address: SocketAddrV4, kid: [u8; 8], probes: u16) -> std::io::Result<Info> {
    {
        let socket = UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))?;
        socket.connect((address.ip().to_owned(), address.port() + 2))?;
        socket.set_read_timeout(Some(QUERY_WAIT))?;
        let mut request = [0; 24];
        request[..8].copy_from_slice(QUERY);
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed).to_le_bytes();
        request[8..16].copy_from_slice(&nonce);
        request[16..24].copy_from_slice(&kid);
        socket.send(&request)?;
        let mut response = [0; 1043];
        let len = socket.recv(&mut response)?;
        if len < 23 || &response[..8] != REPLY || response[8..16] != nonce {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let data_len = u16::from_le_bytes(response[21..23].try_into().unwrap()) as usize;
        if data_len > 1020 || 23 + data_len != len {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        Ok(Info {
            flags: response[16],
            probes,
            rtt: 5,
            bandwidth: u32::from_le_bytes(response[17..21].try_into().unwrap()),
            data: response[23..len].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ping_is_echoed_and_its_report_noted() {
        let pings = Pings::default();
        let sender = Ipv4Addr::new(192, 0, 2, 7);
        let echo = answer_ping(&ping_packet(9, 0), sender, &pings).unwrap();
        assert_eq!(&echo[..8], PONG);
        assert_eq!(echo[8..16], 9u64.to_le_bytes());
        assert!(pings.lock().unwrap().is_empty(), "0 reports nothing");
        answer_ping(&ping_packet(10, 37), sender, &pings).unwrap();
        assert_eq!(pings.lock().unwrap()[&sender].0, 37);
        // Anything else is not a ping.
        assert!(answer_ping(&[0; PING_LEN], sender, &pings).is_none());
        assert!(answer_ping(&ping_packet(1, 1)[..17], sender, &pings).is_none());
    }

    #[test]
    fn matchmaking_measures_the_same_echo_as_coop() {
        use live_proto::{Xuid, matchmaking::Candidate};

        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let base = socket.local_addr().unwrap().port() - 2;
        let responder = std::thread::spawn(move || {
            let mut bytes = [0; PING_LEN];
            let (len, sender) = socket.recv_from(&mut bytes).unwrap();
            let SocketAddr::V4(sender) = sender else { unreachable!() };
            let pings = Pings::default();
            let response = answer_ping(&bytes[..len], *sender.ip(), &pings).unwrap();
            socket.send_to(&response, sender).unwrap();
            assert!(pings.lock().unwrap().is_empty());
        });
        let samples = measure(
            vec![Candidate {
                xuid: Xuid(1),
                endpoint: SocketAddrV4::new(Ipv4Addr::LOCALHOST, base),
                generation: "8000000000000001".into(),
            }],
            Duration::from_secs(2),
        )
        .unwrap();
        responder.join().unwrap();
        assert_eq!(samples[0].target, Xuid(1));
        assert_eq!(samples[0].generation, "8000000000000001");
        assert!(samples[0].rtt_ms.is_some_and(|ms| ms > 0));
    }

    #[test]
    fn a_listener_without_data_answers_with_the_newest_lobby_data() {
        let squad = [0x86, 0x99, 0xd9, 0x7a, 0xea, 0xb8, 0x46, 0xec];
        let session = [0x83, 0xbd, 0x67, 0x94, 0x78, 0x74, 0x7d, 0xf9];
        let old = [0x82, 1, 2, 3, 4, 5, 6, 7];
        let on = |data: &[u8], data_set| Listener {
            enabled: true,
            data: data.to_vec(),
            bandwidth: 7,
            data_set,
        };
        let mut l = BTreeMap::new();
        l.insert(squad, on(&[], 0));
        // Nothing to lend yet: no data.
        assert_eq!(answer(&l, &squad), Some((true, 7, vec![])));
        l.insert(old, on(b"older lobby", 1));
        l.insert(session, on(&[0xab; 172], 2));
        assert_eq!(answer(&l, &squad), Some((true, 7, vec![0xab; 172])));
        assert_eq!(answer(&l, &session).unwrap().2.len(), 172);
        assert_eq!(answer(&l, &old).unwrap().2, b"older lobby");
        // Disabled: no data, and lends none.
        l.get_mut(&session).unwrap().enabled = false;
        assert_eq!(answer(&l, &session), Some((false, 7, vec![])));
        assert_eq!(answer(&l, &squad).unwrap().2, b"older lobby");
        assert_eq!(answer(&l, &[9; 8]), None);
    }

    #[test]
    fn round_trips_are_whole_milliseconds_from_one() {
        assert_eq!(round_trip_ms(Duration::from_micros(300)), 1);
        assert_eq!(round_trip_ms(Duration::from_millis(42)), 42);
        assert_eq!(
            round_trip_ms(Duration::from_secs(60)),
            live_proto::session::PING_MAX_MS
        );
    }
}
