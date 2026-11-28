//! LAN authentication for h2 lab (docs/runtime/lab.md). The pairing code carries
//! the TLS server's pin and a one-time secret. Each paired connection proves
//! its key against a fresh challenge before speaking the lab protocol.

mod credentials;
mod tls;

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use ring::digest;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use subtle::ConstantTimeEq;

use self::credentials::{Peer, Store};
use super::service;

const AUTH: u8 = 1;
const PAIR: u8 = 2;
const ACCEPTED: u8 = 1;
const DENIED: u8 = 0;
const MAX_CONNECTIONS: usize = 64;
const MAX_HANDSHAKES: usize = 32;
const MAX_HANDSHAKES_PER_IP: usize = 4;
const SERVER_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_ATTEMPT_SOURCES: usize = 1024;
const MAX_FAILURES: usize = 8;
const FAILURE_WINDOW: Duration = Duration::from_secs(60);

type Key = [u8; 32];
type PairSecret = [u8; 16];

#[derive(Clone, Debug)]
pub struct Listen {
    pub addr: SocketAddr,
    pub name: Option<String>,
}

pub fn new_pairing(root: &Path) -> Result<String> {
    let store = Store::new(root)?;
    let (pin, _) = tls::identity(store.identity()?)?;
    let secret = store.offer(credentials::now()?)?;
    service::access_event(root, "pairing code issued locally");
    Ok(pair_code(pin, secret))
}

pub fn revoke(root: &Path) -> Result<()> {
    Store::new(root)?.revoke()?;
    service::access_event(root, "pairing revoked locally");
    Ok(())
}

pub struct Server {
    root: PathBuf,
    store: Arc<Store>,
    tls: Arc<rustls::ServerConfig>,
    connections: Arc<AtomicUsize>,
    handshakes: Arc<Mutex<Handshakes>>,
    attempts: Mutex<Attempts>,
}

impl Server {
    pub fn new(root: &Path) -> Result<Self> {
        let store = Arc::new(Store::new(root)?);
        let (_, tls) = tls::identity(store.identity()?)?;
        Ok(Self {
            root: root.to_owned(),
            store,
            tls,
            connections: Arc::new(AtomicUsize::new(0)),
            handshakes: Arc::default(),
            attempts: Mutex::default(),
        })
    }

    /// Reserve capacity before starting a socket worker. The accept loop drops
    /// the reservation after the handshake, including on failure.
    pub(super) fn reserve_handshake(&self, ip: IpAddr) -> Option<HandshakeReservation> {
        let ip = ip.to_canonical();
        let mut state = self.handshakes.lock().unwrap();
        if state.total >= MAX_HANDSHAKES {
            return None;
        }
        let active = state.by_ip.entry(ip).or_default();
        if *active >= MAX_HANDSHAKES_PER_IP {
            return None;
        }
        *active += 1;
        state.total += 1;
        Some(HandshakeReservation {
            ip,
            handshakes: Arc::clone(&self.handshakes),
        })
    }

    /// Returns a stream only when the caller proves the current driver key.
    /// Pairing and authentication failures both end after the handshake.
    pub fn authenticate(&self, socket: TcpStream) -> Result<Option<Authenticated>> {
        let slot = ConnectionSlot::take(&self.connections)?;
        let ip = socket.peer_addr()?.ip().to_canonical();
        let mut stream = tls::accept(Arc::clone(&self.tls), socket)?;
        match read_array::<1>(&mut stream)?[0] {
            AUTH => {
                let challenge = random::<32>()?;
                send(&mut stream, &challenge)?;
                let proof: Key = read_array(&mut stream)?;
                let key = self
                    .attempts
                    .lock()
                    .unwrap()
                    .check(ip, Instant::now(), || {
                        Ok(self
                            .store
                            .key()?
                            .filter(|key| verify(key, &challenge, &proof)))
                    })?;
                send(
                    &mut stream,
                    &[if key.is_some() { ACCEPTED } else { DENIED }],
                )?;
                let Some(key) = key else { return Ok(None) };
                stream.sock.authenticated(Some(SERVER_IDLE_TIMEOUT))?;
                Ok(Some(Authenticated {
                    stream,
                    store: Arc::clone(&self.store),
                    key,
                    _slot: slot,
                }))
            }
            PAIR => {
                let secret = read_array(&mut stream)?;
                let key = read_array(&mut stream)?;
                let accepted = self
                    .attempts
                    .lock()
                    .unwrap()
                    .check(ip, Instant::now(), || {
                        Ok(self
                            .store
                            .pair(&secret, key, credentials::now()?)?
                            .then_some(()))
                    })?
                    .is_some();
                let response = send(&mut stream, &[if accepted { ACCEPTED } else { DENIED }]);
                if accepted {
                    service::access_event(&self.root, "driver paired");
                }
                response?;
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

pub struct Authenticated {
    stream: tls::ServerStream,
    store: Arc<Store>,
    key: Key,
    _slot: ConnectionSlot,
}

impl Authenticated {
    /// Re-check between requests so local unpair also revokes open connections.
    pub fn is_authorized(&self) -> Result<bool> {
        Ok(self
            .store
            .key()?
            .is_some_and(|key| bool::from(key.ct_eq(&self.key))))
    }
}

impl Read for Authenticated {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.stream.read(bytes)
    }
}

impl Write for Authenticated {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.stream.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stream.flush()
    }
}

pub fn connect(host: &str, addr: SocketAddr) -> Result<impl Read + Write + Send + use<>> {
    let peers = credentials::trusted_peers(host)?;
    let mut stream = authenticate(addr, &peers)?;
    stream.sock.authenticated(None)?;
    Ok(stream)
}

pub fn pair(host: &str, addr: SocketAddr, code: &str) -> Result<()> {
    let (pin, secret) = parse_pair_code(code)?;
    let peer = Peer {
        pin,
        key: random()?,
    };
    let pending = credentials::stage_peer(host, &peer)?;
    pair_with(addr, &peer, &secret)?;
    pending.publish()
}

fn authenticate(addr: SocketAddr, peers: &[Peer]) -> Result<tls::ClientStream> {
    let pins = peers.iter().map(|peer| peer.pin).collect();
    let (mut stream, verified_pin) = tls::connect(addr, pins)?;
    let peer = peers
        .iter()
        .find(|peer| peer.pin == verified_pin)
        .context("the lab host's TLS key does not match a pairing")?;
    send(&mut stream, &[AUTH])?;
    let challenge = read_array(&mut stream)?;
    send(&mut stream, proof(&peer.key, &challenge).as_ref())?;
    ensure!(
        read_array::<1>(&mut stream)?[0] == ACCEPTED,
        "lab authentication refused (pair again if the key was revoked)"
    );
    Ok(stream)
}

fn pair_with(addr: SocketAddr, peer: &Peer, secret: &PairSecret) -> Result<()> {
    let (mut stream, _) = tls::connect(addr, vec![peer.pin])?;
    let mut request = [0; 1 + size_of::<PairSecret>() + size_of::<Key>()];
    request[0] = PAIR;
    request[1..1 + secret.len()].copy_from_slice(secret);
    request[1 + secret.len()..].copy_from_slice(&peer.key);
    send(&mut stream, &request)?;
    ensure!(
        read_array::<1>(&mut stream)?[0] == ACCEPTED,
        "lab pairing refused (the code may be invalid, expired, used, or rate-limited)"
    );
    Ok(())
}

fn proof(key: &Key, challenge: &Key) -> hmac::Tag {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::sign(&key, challenge)
}

fn verify(key: &Key, challenge: &Key, proof: &Key) -> bool {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::verify(&key, challenge, proof).is_ok()
}

fn pair_code(pin: Key, secret: PairSecret) -> String {
    let mut code = [0; size_of::<Key>() + size_of::<PairSecret>()];
    code[..pin.len()].copy_from_slice(&pin);
    code[pin.len()..].copy_from_slice(&secret);
    live_proto::pack::to_base64(&code)
}

fn parse_pair_code(code: &str) -> Result<(Key, PairSecret)> {
    ensure!(code.trim().len() == 64, "invalid lab pairing code");
    let bytes = live_proto::pack::from_base64(code.trim())
        .map_err(|_| anyhow::anyhow!("invalid lab pairing code"))?;
    let bytes: [u8; size_of::<Key>() + size_of::<PairSecret>()] = bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid lab pairing code"))?;
    let (pin, secret) = bytes.split_at(size_of::<Key>());
    Ok((pin.try_into().unwrap(), secret.try_into().unwrap()))
}

fn fingerprint(bytes: &[u8]) -> Key {
    digest::digest(&digest::SHA256, bytes)
        .as_ref()
        .try_into()
        .unwrap()
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("getting randomness for the lab handshake"))?;
    Ok(bytes)
}

fn read_array<const N: usize>(stream: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn send(stream: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    stream.write_all(bytes)?;
    stream.flush()
}

#[derive(Default)]
struct Handshakes {
    total: usize,
    by_ip: HashMap<IpAddr, usize>,
}

pub(super) struct HandshakeReservation {
    ip: IpAddr,
    handshakes: Arc<Mutex<Handshakes>>,
}

impl Drop for HandshakeReservation {
    fn drop(&mut self) {
        let mut state = self.handshakes.lock().unwrap();
        state.total -= 1;
        let active = state.by_ip.get_mut(&self.ip).unwrap();
        *active -= 1;
        if *active == 0 {
            state.by_ip.remove(&self.ip);
        }
    }
}

struct ConnectionSlot(Arc<AtomicUsize>);

impl ConnectionSlot {
    fn take(connections: &Arc<AtomicUsize>) -> Result<Self> {
        connections
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                (n < MAX_CONNECTIONS).then_some(n + 1)
            })
            .ok()
            .context("too many lab connections")?;
        Ok(Self(Arc::clone(connections)))
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Attempts {
    failures: HashMap<IpAddr, (Instant, usize)>,
}

impl Attempts {
    fn check<T>(
        &mut self,
        ip: IpAddr,
        now: Instant,
        check: impl FnOnce() -> Result<Option<T>>,
    ) -> Result<Option<T>> {
        self.failures
            .retain(|_, (start, _)| now.duration_since(*start) < FAILURE_WINDOW);
        if self
            .failures
            .get(&ip)
            .is_some_and(|(_, count)| *count >= MAX_FAILURES)
        {
            return Ok(None);
        }

        let result = check()?;
        if result.is_none() {
            if let Some((_, count)) = self.failures.get_mut(&ip) {
                *count += 1;
            } else {
                if self.failures.len() == MAX_ATTEMPT_SOURCES
                    && let Some((&oldest, _)) =
                        self.failures.iter().min_by_key(|(_, (start, _))| *start)
                {
                    self.failures.remove(&oldest);
                }
                self.failures.insert(ip, (now, 1));
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::{Ipv6Addr, TcpListener};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    use std::thread::{self, JoinHandle};

    use super::*;

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new() -> Self {
            let id = crate::lab::proto::to_hex(&random::<16>().unwrap());
            let path = std::env::temp_dir().join(format!("h2lab-auth-{id}"));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn serve_once(server: &Arc<Server>) -> (SocketAddr, JoinHandle<Result<Option<Authenticated>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = Arc::clone(server);
        let task = thread::spawn(move || server.authenticate(listener.accept()?.0));
        (addr, task)
    }

    #[test]
    fn lan_pairing_code_is_single_use_and_expires() {
        let root = TestRoot::new();
        let store = Store::new(&root.0).unwrap();
        let secret = store.offer(1000).unwrap();
        let key = [42; 32];
        assert!(!store.pair(&[0; 16], key, 1001).unwrap());
        assert!(!store.pair(&secret, key, 999).unwrap());
        assert!(!store.pair(&secret, key, 1600).unwrap());
        assert!(store.key().unwrap().is_none());

        let fresh = store.offer(2000).unwrap();
        assert!(store.pair(&fresh, key, 2599).unwrap());
        assert!(!store.pair(&fresh, [29; 32], 2599).unwrap());
        assert_eq!(store.key().unwrap(), Some(key));
        let mode = fs::metadata(root.0.join("auth/pairing.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn lan_tls_authentication_rejects_a_wrong_pin_wrong_key_and_replay() {
        let root = TestRoot::new();
        let server = Arc::new(Server::new(&root.0).unwrap());
        let code = new_pairing(&root.0).unwrap();
        let (pin, secret) = parse_pair_code(&code).unwrap();
        let peer = Peer { pin, key: [3; 32] };

        let (addr, task) = serve_once(&server);
        let mut wrong_pin = pin;
        wrong_pin[0] ^= 1;
        let wrong = Peer {
            pin: wrong_pin,
            key: peer.key,
        };
        assert!(pair_with(addr, &wrong, &secret).is_err());
        assert!(task.join().unwrap().is_err());

        let (addr, task) = serve_once(&server);
        pair_with(addr, &peer, &secret).unwrap();
        assert!(task.join().unwrap().unwrap().is_none());

        let (addr, task) = serve_once(&server);
        assert!(
            pair_with(addr, &peer, &secret)
                .unwrap_err()
                .to_string()
                .contains("pairing refused")
        );
        assert!(task.join().unwrap().unwrap().is_none());

        let (addr, task) = serve_once(&server);
        let wrong = Peer { pin, key: [9; 32] };
        let error = authenticate(addr, std::slice::from_ref(&wrong))
            .err()
            .expect("the wrong key must fail");
        assert!(error.to_string().contains("authentication refused"));
        assert!(task.join().unwrap().unwrap().is_none());

        let (addr, task) = serve_once(&server);
        let (mut client, _) = tls::connect(addr, vec![pin]).unwrap();
        send(&mut client, &[AUTH]).unwrap();
        let challenge = read_array(&mut client).unwrap();
        let first_proof = proof(&peer.key, &challenge);
        send(&mut client, first_proof.as_ref()).unwrap();
        assert_eq!(read_array::<1>(&mut client).unwrap(), [ACCEPTED]);
        let mut session = task.join().unwrap().unwrap().unwrap();
        assert!(session.is_authorized().unwrap());
        send(&mut client, b"request").unwrap();
        assert_eq!(read_array::<7>(&mut session).unwrap(), *b"request");

        let (addr, task) = serve_once(&server);
        let (mut replay, _) = tls::connect(addr, vec![pin]).unwrap();
        send(&mut replay, &[AUTH]).unwrap();
        assert_ne!(read_array::<32>(&mut replay).unwrap(), challenge);
        send(&mut replay, first_proof.as_ref()).unwrap();
        assert_eq!(read_array::<1>(&mut replay).unwrap(), [DENIED]);
        assert!(task.join().unwrap().unwrap().is_none());

        revoke(&root.0).unwrap();
        assert!(!session.is_authorized().unwrap());
        let (addr, task) = serve_once(&server);
        assert!(authenticate(addr, std::slice::from_ref(&peer)).is_err());
        assert!(task.join().unwrap().unwrap().is_none());
        let access = root.0.join("logs/host-access.log");
        assert_eq!(
            fs::read_to_string(&access).unwrap(),
            concat!(
                "lab access: pairing code issued locally\n",
                "lab access: driver paired\n",
                "lab access: pairing revoked locally\n",
            ),
        );
        assert_eq!(
            fs::metadata(access).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    fn paired_server(key: Key) -> (TestRoot, Arc<Server>, Peer) {
        let root = TestRoot::new();
        let code = new_pairing(&root.0).unwrap();
        let (pin, secret) = parse_pair_code(&code).unwrap();
        assert!(
            Store::new(&root.0)
                .unwrap()
                .pair(&secret, key, credentials::now().unwrap())
                .unwrap()
        );
        let server = Arc::new(Server::new(&root.0).unwrap());
        (root, server, Peer { pin, key })
    }

    fn save_peer(dir: &Path, host: &str, peer: &Peer) {
        let file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(credentials::peer_path(dir, host))
            .unwrap();
        serde_json::to_writer(file, peer).unwrap();
    }

    #[test]
    fn lan_numeric_host_uses_only_the_tls_matched_peer() {
        let (_first_root, first, first_peer) = paired_server([32; 32]);
        let (_second_root, second, second_peer) = paired_server([64; 32]);
        let (_third_root, third, _) = paired_server([128; 32]);
        let peers = TestRoot::new();
        save_peer(&peers.0, "first", &first_peer);
        save_peer(&peers.0, "second", &second_peer);

        for (server, with_port) in [(&first, false), (&second, true)] {
            let (addr, task) = serve_once(server);
            let host = if with_port {
                addr.to_string()
            } else {
                addr.ip().to_string()
            };
            let trusted = credentials::trusted_peers_in(&peers.0, &host).unwrap();
            assert_eq!(trusted.len(), 2);
            assert!(authenticate(addr, &trusted).is_ok());
            assert!(task.join().unwrap().unwrap().is_some());
        }

        // A numeric address provides no trust when the host presents a new key.
        let (addr, task) = serve_once(&third);
        let trusted = credentials::trusted_peers_in(&peers.0, &addr.to_string()).unwrap();
        assert!(authenticate(addr, &trusted).is_err());
        assert!(task.join().unwrap().is_err());

        // The pin recorded for a name stays binding even for another paired host.
        assert!(credentials::trusted_peers_in(&peers.0, "unknown").is_err());
        let (addr, task) = serve_once(&second);
        let trusted = credentials::trusted_peers_in(&peers.0, "FIRST").unwrap();
        assert_eq!(trusted.len(), 1);
        assert!(authenticate(addr, &trusted).is_err());
        assert!(task.join().unwrap().is_err());

        // An exact address entry also stays binding.
        let (addr, task) = serve_once(&second);
        save_peer(&peers.0, &addr.to_string(), &first_peer);
        let trusted = credentials::trusted_peers_in(&peers.0, &addr.to_string()).unwrap();
        assert_eq!(trusted.len(), 1);
        assert!(authenticate(addr, &trusted).is_err());
        assert!(task.join().unwrap().is_err());
    }

    #[test]
    fn lan_handshakes_reserve_per_ip_and_global_capacity() {
        let root = TestRoot::new();
        let server = Server::new(&root.0).unwrap();
        let ip = "192.0.2.1".parse().unwrap();
        let mut reservations = Vec::new();
        for _ in 0..MAX_HANDSHAKES_PER_IP {
            reservations.push(server.reserve_handshake(ip).unwrap());
        }
        let mapped_ip = "::ffff:192.0.2.1".parse().unwrap();
        assert!(server.reserve_handshake(mapped_ip).is_none());
        for n in reservations.len()..MAX_HANDSHAKES {
            let ip = Ipv6Addr::from(n as u128).into();
            reservations.push(server.reserve_handshake(ip).unwrap());
        }
        let other = "192.0.2.2".parse().unwrap();
        assert!(server.reserve_handshake(other).is_none());

        drop(reservations.pop());
        assert!(server.reserve_handshake(other).is_some());
        drop(reservations);
        assert!(server.reserve_handshake(mapped_ip).is_some());
        assert!(server.handshakes.lock().unwrap().by_ip.is_empty());
    }

    #[test]
    fn lan_attempt_cache_evicts_old_sources_without_blocking_new_ones() {
        let mut attempts = Attempts::default();
        let at = Instant::now();
        for n in 0..MAX_ATTEMPT_SOURCES {
            let ip = Ipv6Addr::from(n as u128).into();
            attempts
                .check(ip, at + Duration::from_nanos(n as u64), || Ok(None::<()>))
                .unwrap();
        }
        let oldest = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
        assert!(attempts.failures.contains_key(&oldest));

        let new_ip = "192.0.2.1".parse().unwrap();
        let now = at + Duration::from_millis(1);
        assert!(
            attempts
                .check(new_ip, now, || Ok(Some(())))
                .unwrap()
                .is_some()
        );
        attempts.check(new_ip, now, || Ok(None::<()>)).unwrap();

        assert_eq!(attempts.failures.len(), MAX_ATTEMPT_SOURCES);
        assert!(attempts.failures.contains_key(&new_ip));
        assert!(!attempts.failures.contains_key(&oldest));
    }

    #[test]
    fn lan_failed_attempts_are_rate_limited_by_source() {
        let mut attempts = Attempts::default();
        let at = Instant::now();
        let ip = "127.0.0.1".parse().unwrap();
        for _ in 0..MAX_FAILURES {
            assert!(attempts.check(ip, at, || Ok(None::<()>)).unwrap().is_none());
        }

        assert!(attempts.check(ip, at, || Ok(Some(()))).unwrap().is_none());
        let other = "127.0.0.2".parse().unwrap();
        assert!(
            attempts
                .check(other, at, || Ok(Some(())))
                .unwrap()
                .is_some()
        );
        assert!(
            attempts
                .check(ip, at + FAILURE_WINDOW, || Ok(Some(())))
                .unwrap()
                .is_some()
        );
    }
}
