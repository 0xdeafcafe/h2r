//! LAN authentication for h2 lab (docs/runtime/lab.md). The pairing code carries
//! the TLS server's pin and a one-time secret. Each paired connection proves
//! its key against a fresh challenge before speaking the lab protocol.

mod credentials;
mod tls;

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use ring::digest;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use subtle::ConstantTimeEq;

use self::credentials::{Peer, Store};

const AUTH: u8 = 1;
const PAIR: u8 = 2;
const ACCEPTED: u8 = 1;
const DENIED: u8 = 0;
const MAX_CONNECTIONS: usize = 64;
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
    Ok(pair_code(pin, secret))
}

pub fn revoke(root: &Path) -> Result<()> {
    Store::new(root)?.revoke()
}

pub struct Server {
    store: Arc<Store>,
    tls: Arc<rustls::ServerConfig>,
    connections: Arc<AtomicUsize>,
    attempts: Mutex<Attempts>,
}

impl Server {
    pub fn new(root: &Path) -> Result<Self> {
        let store = Arc::new(Store::new(root)?);
        let (_, tls) = tls::identity(store.identity()?)?;
        Ok(Self {
            store,
            tls,
            connections: Arc::new(AtomicUsize::new(0)),
            attempts: Mutex::default(),
        })
    }

    /// Returns a stream only when the caller proves the current driver key.
    /// Pairing and authentication failures both end after the handshake.
    pub fn authenticate(&self, socket: TcpStream) -> Result<Option<Authenticated>> {
        let slot = ConnectionSlot::take(&self.connections)?;
        let ip = socket.peer_addr()?.ip();
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
                stream.sock.authenticated()?;
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
                send(&mut stream, &[if accepted { ACCEPTED } else { DENIED }])?;
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
    let peer = credentials::read_peer(host)?;
    let mut stream = authenticate(addr, &peer)?;
    stream.sock.authenticated()?;
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

fn authenticate(addr: SocketAddr, peer: &Peer) -> Result<tls::ClientStream> {
    let mut stream = tls::connect(addr, peer.pin)?;
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
    let mut stream = tls::connect(addr, peer.pin)?;
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

struct ConnectionSlot(Arc<AtomicUsize>);

impl ConnectionSlot {
    fn take(connections: &Arc<AtomicUsize>) -> Result<Self> {
        connections
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
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
        let full = self.failures.len() == MAX_ATTEMPT_SOURCES;
        let failures = self.failures.get_mut(&ip);
        if failures
            .as_ref()
            .map_or(full, |(_, count)| *count >= MAX_FAILURES)
        {
            return Ok(None);
        }

        let result = check()?;
        if result.is_none() {
            match failures {
                Some((_, count)) => *count += 1,
                None => {
                    self.failures.insert(ip, (now, 1));
                }
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
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
        let error = authenticate(addr, &wrong)
            .err()
            .expect("the wrong key must fail");
        assert!(error.to_string().contains("authentication refused"));
        assert!(task.join().unwrap().unwrap().is_none());

        let (addr, task) = serve_once(&server);
        let mut client = tls::connect(addr, pin).unwrap();
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
        let mut replay = tls::connect(addr, pin).unwrap();
        send(&mut replay, &[AUTH]).unwrap();
        assert_ne!(read_array::<32>(&mut replay).unwrap(), challenge);
        send(&mut replay, first_proof.as_ref()).unwrap();
        assert_eq!(read_array::<1>(&mut replay).unwrap(), [DENIED]);
        assert!(task.join().unwrap().unwrap().is_none());

        revoke(&root.0).unwrap();
        assert!(!session.is_authorized().unwrap());
        let (addr, task) = serve_once(&server);
        assert!(authenticate(addr, &peer).is_err());
        assert!(task.join().unwrap().unwrap().is_none());
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
