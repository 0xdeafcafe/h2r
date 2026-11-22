//! TLS 1.3 with the host's Ed25519 public key pinned during pairing.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use rustls::client::Resumption;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::ring::default_provider;
use rustls::pki_types::{
    CertificateDer, PrivatePkcs8KeyDer, ServerName, SubjectPublicKeyInfoDer, UnixTime,
};
use rustls::server::AlwaysResolvesServerRawPublicKeys;
use rustls::sign::CertifiedKey;
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, ServerConfig, ServerConnection,
    SignatureScheme, StreamOwned, version,
};

use super::{Key, fingerprint};

const ALPN: &[u8] = b"h2lab/1";
const AUTH_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) type ServerStream = StreamOwned<ServerConnection, Socket>;
pub(super) type ClientStream = StreamOwned<ClientConnection, Socket>;

pub(super) fn identity(private_key: Vec<u8>) -> Result<(Key, Arc<ServerConfig>)> {
    let provider = Arc::new(default_provider());
    let key = provider
        .key_provider
        .load_private_key(PrivatePkcs8KeyDer::from(private_key).into())?;
    let public_key = key.public_key().context("getting the lab TLS public key")?;
    let pin = fingerprint(public_key.as_ref());
    let raw_key = CertificateDer::from(public_key.as_ref().to_vec());
    let certified = Arc::new(CertifiedKey::new(vec![raw_key], key));
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&version::TLS13])?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(AlwaysResolvesServerRawPublicKeys::new(certified)));
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.send_tls13_tickets = 0;
    Ok((pin, Arc::new(config)))
}

pub(super) fn accept(config: Arc<ServerConfig>, socket: TcpStream) -> Result<ServerStream> {
    let mut stream = StreamOwned::new(ServerConnection::new(config)?, Socket::new(socket)?);
    stream.conn.complete_io(&mut stream.sock)?;
    ensure!(
        stream.conn.alpn_protocol() == Some(ALPN),
        "incompatible lab TLS protocol"
    );
    Ok(stream)
}

pub(super) fn connect(addr: SocketAddr, pins: Vec<Key>) -> Result<(ClientStream, Key)> {
    let mut config = ClientConfig::builder_with_provider(default_provider().into())
        .with_protocol_versions(&[&version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedKeys(pins)))
        .with_no_client_auth();
    config.enable_sni = false;
    config.alpn_protocols = vec![ALPN.to_vec()];
    config.resumption = Resumption::disabled();

    // The pin is the server identity, independent of an address or Bonjour name.
    let conn = ClientConnection::new(config.into(), ServerName::try_from("h2lab")?)?;
    let socket = TcpStream::connect_timeout(&addr, AUTH_TIMEOUT)
        .with_context(|| format!("connecting to the lab at {addr}"))?;
    let mut stream = StreamOwned::new(conn, Socket::new(socket)?);
    stream
        .conn
        .complete_io(&mut stream.sock)
        .context("verifying the lab host's pinned TLS key")?;
    ensure!(
        stream.conn.alpn_protocol() == Some(ALPN),
        "incompatible lab TLS protocol"
    );
    let key = stream
        .conn
        .peer_certificates()
        .and_then(|keys| keys.first())
        .context("the lab host's TLS handshake returned no verified key")?;
    let pin = fingerprint(key.as_ref());
    Ok((stream, pin))
}

#[derive(Debug)]
struct PinnedKeys(Vec<Key>);

impl ServerCertVerifier for PinnedKeys {
    fn verify_server_cert(
        &self,
        raw_key: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if intermediates.is_empty() && self.0.contains(&fingerprint(raw_key.as_ref())) {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General(
                "lab host TLS key does not match the pairing".into(),
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _key: &CertificateDer<'_>,
        _signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("the lab requires TLS 1.3".into()))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        raw_key: &CertificateDer<'_>,
        signature: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature_with_raw_key(
            message,
            &SubjectPublicKeyInfoDer::from(raw_key.as_ref()),
            signature,
            &default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![SignatureScheme::ED25519]
    }

    fn requires_raw_public_keys(&self) -> bool {
        true
    }
}

// Keep the handshake deadline across TLS records and short reads. Lab
// requests can take as long as the build or game requires after authentication.
pub(super) struct Socket {
    tcp: TcpStream,
    deadline: Option<Instant>,
}

impl Socket {
    fn new(tcp: TcpStream) -> io::Result<Self> {
        tcp.set_nodelay(true)?;
        Ok(Self {
            tcp,
            deadline: Some(Instant::now() + AUTH_TIMEOUT),
        })
    }

    pub(super) fn authenticated(&mut self, idle_timeout: Option<Duration>) -> io::Result<()> {
        self.deadline = None;
        self.tcp.set_read_timeout(idle_timeout)?;
        self.tcp.set_write_timeout(idle_timeout)
    }

    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .and_then(|deadline| deadline.checked_duration_since(Instant::now()))
            .filter(|duration| !duration.is_zero())
            .ok_or(io::ErrorKind::TimedOut.into())
    }
}

impl Read for Socket {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if self.deadline.is_some() {
            self.tcp.set_read_timeout(Some(self.remaining()?))?;
        }
        self.tcp.read(bytes)
    }
}

impl Write for Socket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.deadline.is_some() {
            self.tcp.set_write_timeout(Some(self.remaining()?))?;
        }
        self.tcp.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}
