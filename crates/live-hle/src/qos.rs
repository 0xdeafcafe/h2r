//! HLE-only QoS side channel. This is not the Xbox encrypted wire protocol.
use std::{
    collections::BTreeMap,
    net::{SocketAddrV4, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};
const QUERY: &[u8; 8] = b"H2QOS001";
const REPLY: &[u8; 8] = b"H2QOR001";
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
    pub fn start(address: SocketAddrV4, listeners: Listeners) -> std::io::Result<Self> {
        let socket = UdpSocket::bind((address.ip().to_owned(), address.port() + 2))?;
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("h2-xnet-qos".into())
            .spawn(move || {
                let mut input = [0; 24];
                while !flag.load(Ordering::Acquire) {
                    let Ok((24, sender)) = socket.recv_from(&mut input) else {
                        continue;
                    };
                    if &input[..8] != QUERY {
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
