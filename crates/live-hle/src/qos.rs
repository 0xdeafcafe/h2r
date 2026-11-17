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
    ) -> std::io::Result<Self> {
        let socket = UdpSocket::bind((address.ip().to_owned(), address.port() + 2))?;
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("h2-xnet-qos".into())
            .spawn(move || {
                let mut input = [0; 24];
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
                    let entry = listeners
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get(&kid)
                        .cloned();
                    let Some(entry) = entry else {
                        continue;
                    };
                    let data = if entry.enabled {
                        entry.data.as_slice()
                    } else {
                        &[]
                    };
                    let mut response = Vec::with_capacity(23 + data.len());
                    response.extend_from_slice(REPLY);
                    response.extend_from_slice(&input[8..16]);
                    response.push(if entry.enabled {
                        3 | if data.is_empty() { 0 } else { 8 }
                    } else {
                        7
                    });
                    response.extend_from_slice(&entry.bandwidth.to_le_bytes());
                    response.extend_from_slice(&(data.len() as u16).to_le_bytes());
                    response.extend_from_slice(data);
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
pub(crate) fn query(address: SocketAddrV4, kid: [u8; 8], probes: u16) -> Info {
    let attempt = || -> std::io::Result<Info> {
        let socket = UdpSocket::bind((std::net::Ipv4Addr::UNSPECIFIED, 0))?;
        socket.connect((address.ip().to_owned(), address.port() + 2))?;
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;
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
    };
    attempt().unwrap_or(Info {
        flags: 1,
        ..Info::default()
    })
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
    fn round_trips_are_whole_milliseconds_from_one() {
        assert_eq!(round_trip_ms(Duration::from_micros(300)), 1);
        assert_eq!(round_trip_ms(Duration::from_millis(42)), 42);
        assert_eq!(
            round_trip_ms(Duration::from_secs(60)),
            live_proto::session::PING_MAX_MS
        );
    }
}
