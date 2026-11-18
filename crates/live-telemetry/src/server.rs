use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Duration, Instant};

use live_proto::game::{Frame, GameCreated, NewGame};
use live_proto::pack;
use live_proto::sign::{EventBody, KeyPair, issue_token, sign_batch};
use live_proto::{ClientMessage, Discovery, IngestReply, lxm};
use time::OffsetDateTime;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::config::TelemetryAccount;
use crate::event::GameEvent;
use crate::sink::Sink;

const MAX_INGEST_MESSAGE: usize = 16 * 1024;
const MAX_BATCH_EVENTS: usize = 4;
/// A packed keyframe every this many frames (ten seconds at 15 a second);
/// the frames between are deltas (docs/live/live-telemetry.md, Packed frames).
pub(crate) const FRAME_KEYFRAME_EVERY: u64 = 150;

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

struct Connection {
    socket: Socket,
    discovery: Discovery,
    author: String,
    last_auth: Instant,
    /// The connection's packed frame stream: a new connection starts a new
    /// stream with a keyframe.
    frames: pack::Encoder,
}

pub struct ServerSink {
    base_url: String,
    account: TelemetryAccount,
    key: KeyPair,
    connection: Option<Connection>,
    games: HashMap<String, String>,
    pending: Vec<GameEvent>,
}

impl ServerSink {
    pub fn new(base_url: String, account: TelemetryAccount) -> Result<Self, String> {
        if !live_proto::bearer_allowed(&base_url) {
            return Err("server credentials require HTTPS or a loopback URL".into());
        }
        let key = KeyPair::from_private_jwk(&account.key)
            .map_err(|e| format!("telemetry account key: {e}"))?;
        let mut account = account;
        account.xuid = live_proto::Xuid::parse(&account.xuid)
            .map_err(|e| format!("telemetry account XUID: {e}"))?
            .to_string();
        Ok(Self {
            base_url,
            account,
            key,
            connection: None,
            games: HashMap::new(),
            pending: Vec::new(),
        })
    }

    fn agent() -> ureq::Agent {
        ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout_read(Duration::from_secs(5))
            .timeout_write(Duration::from_secs(5))
            .build()
    }

    fn discover(&self) -> Result<Discovery, String> {
        let url = format!(
            "{}/.well-known/h2live.json",
            self.base_url.trim_end_matches('/')
        );
        Self::agent()
            .get(&url)
            .call()
            .map_err(|e| format!("telemetry discovery: {e}"))?
            .into_json::<Discovery>()
            .map_err(|e| format!("telemetry discovery JSON: {e}"))
    }

    fn token(&self, server_id: &str) -> String {
        let author = format!("acct:{}@{server_id}", self.account.xuid);
        issue_token(
            &self.key,
            &author,
            server_id,
            lxm::GAMES_WRITE,
            OffsetDateTime::now_utc().unix_timestamp(),
        )
    }

    fn connect(&mut self) -> Result<(), String> {
        if self.connection.is_some() {
            return Ok(());
        }
        let discovery = self.discover()?;
        let api = resolve_api(&self.base_url, &discovery.api);
        let ingest = format!("{}/ingest", api.trim_end_matches('/'));
        let socket_url = ingest
            .strip_prefix("http://")
            .map(|rest| format!("ws://{rest}"))
            .or_else(|| {
                ingest
                    .strip_prefix("https://")
                    .map(|rest| format!("wss://{rest}"))
            })
            .ok_or_else(|| "telemetry API URL must be HTTP(S)".to_owned())?;
        if !live_proto::bearer_allowed(&socket_url) {
            return Err("telemetry ingest credentials require WSS or loopback WS".into());
        }
        let (mut socket, _) = connect_bounded(&socket_url)?;
        set_timeouts(socket.get_mut())?;
        let author = format!("acct:{}@{}", self.account.xuid, discovery.id);
        let connection = Connection {
            socket,
            discovery,
            author,
            last_auth: Instant::now(),
            frames: pack::Encoder::new(0, FRAME_KEYFRAME_EVERY),
        };
        self.connection = Some(connection);
        self.authenticate()
    }

    fn authenticate(&mut self) -> Result<(), String> {
        let token = self.token(&self.connection.as_ref().unwrap().discovery.id);
        self.connection.as_mut().unwrap().last_auth = Instant::now();
        self.send(ClientMessage::Auth { token })
    }

    fn send(&mut self, message: ClientMessage) -> Result<(), String> {
        if self
            .connection
            .as_ref()
            .is_some_and(|c| c.last_auth.elapsed() >= Duration::from_secs(45))
        {
            self.authenticate()?;
        }
        let text = serde_json::to_string(&message).map_err(|e| e.to_string())?;
        self.send_message(Message::Text(text.into()))
    }

    /// Sends a text or binary message of at most [`MAX_INGEST_MESSAGE`]
    /// bytes and drains replies already waiting.
    fn send_message(&mut self, message: Message) -> Result<(), String> {
        if message.len() > MAX_INGEST_MESSAGE {
            return Err(format!(
                "telemetry ingest message is {} bytes (limit {MAX_INGEST_MESSAGE})",
                message.len()
            ));
        }
        let connection = self.connection.as_mut().unwrap();
        connection
            .socket
            .send(message)
            .map_err(|e| format!("telemetry ingest send: {e}"))?;
        // Replies are optional for frames. Drain what is already available,
        // but never wait for an acknowledgement on the guest's behalf.
        for _ in 0..4 {
            match connection.socket.read() {
                Ok(Message::Text(text)) => {
                    let reply: IngestReply = serde_json::from_str(&text)
                        .map_err(|e| format!("telemetry ingest reply: {e}"))?;
                    if let IngestReply::Error { message, .. } = reply {
                        return Err(format!("telemetry ingest refused: {message}"));
                    }
                }
                Ok(Message::Ping(bytes)) => connection
                    .socket
                    .send(Message::Pong(bytes))
                    .map_err(|e| format!("telemetry ingest pong: {e}"))?,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::WouldBlock => break,
                Err(tungstenite::Error::Io(e)) if e.kind() == ErrorKind::TimedOut => break,
                Err(e) => return Err(format!("telemetry ingest receive: {e}")),
            }
        }
        Ok(())
    }

    fn create_game(&mut self, local_id: &str, game: &NewGame) -> Result<(), String> {
        self.connect()?;
        let api = resolve_api(
            &self.base_url,
            &self.connection.as_ref().unwrap().discovery.api,
        );
        let url = format!("{}/games", api.trim_end_matches('/'));
        let token = self.token(&self.connection.as_ref().unwrap().discovery.id);
        let created = Self::agent()
            .post(&url)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .send_json(game)
            .map_err(|e| format!("create telemetry game: {e}"))?
            .into_json::<GameCreated>()
            .map_err(|e| format!("create telemetry game JSON: {e}"))?;
        self.games.insert(local_id.to_owned(), created.game_id);
        Ok(())
    }

    fn flush_events(&mut self) -> Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.connect()?;
        let pending = std::mem::take(&mut self.pending);
        let mut remaining = pending.as_slice();
        while !remaining.is_empty() {
            let count = remaining.len().min(MAX_BATCH_EVENTS);
            let batch = &remaining[..count];
            let result = (|| {
                let game_id = batch[0].game_id.clone();
                let server_id = self
                    .games
                    .get(&game_id)
                    .cloned()
                    .ok_or_else(|| format!("no server game for {game_id}"))?;
                let author = self.connection.as_ref().unwrap().author.clone();
                let message = signed_events_message(
                    &self.key,
                    &author,
                    &server_id,
                    batch,
                    OffsetDateTime::now_utc(),
                )?;
                self.send(message)
            })();
            if let Err(error) = result {
                self.pending = remaining.to_vec();
                return Err(error);
            }
            remaining = &remaining[count..];
        }
        Ok(())
    }
}

impl Sink for ServerSink {
    fn event(&mut self, event: GameEvent) -> Result<(), String> {
        if event.kind == live_proto::game::GAME_STARTED {
            self.flush_events()?;
            let started: live_proto::game::GameStarted = serde_json::from_value(event.data.clone())
                .map_err(|e| format!("GameStarted data: {e}"))?;
            self.create_game(&event.game_id, &started.game)
        } else {
            self.pending.push(event);
            Ok(())
        }
    }

    fn frame(&mut self, mut frame: Frame) -> Result<(), String> {
        self.connect()?;
        let server_id = self
            .games
            .get(&frame.game_id)
            .cloned()
            .ok_or_else(|| format!("no server game for {}", frame.game_id))?;
        frame.game_id = server_id;
        if self
            .connection
            .as_ref()
            .is_some_and(|c| c.last_auth.elapsed() >= Duration::from_secs(45))
        {
            self.authenticate()?;
        }
        let message = frame_message(&mut self.connection.as_mut().unwrap().frames, frame)?;
        self.send_message(message)
    }

    fn flush(&mut self) -> Result<(), String> {
        self.flush_events()
    }
}

impl Drop for ServerSink {
    fn drop(&mut self) {
        if let Some(connection) = &mut self.connection {
            let _ = connection.socket.close(None);
        }
    }
}

/// A frame as an ingest message: one packed record in a binary message, or
/// (a frame the format cannot carry, which leaves the stream as it was)
/// the JSON `Frame` message; the server takes both.
fn frame_message(frames: &mut pack::Encoder, frame: Frame) -> Result<Message, String> {
    match frames.encode(&frame) {
        Ok((record, _)) => {
            let mut bytes = Vec::with_capacity(record.len() + 3);
            pack::push_record(&mut bytes, &record);
            Ok(Message::Binary(bytes.into()))
        }
        Err(_) => {
            let message = ClientMessage::Frame {
                frame: Box::new(frame),
            };
            let text = serde_json::to_string(&message).map_err(|e| e.to_string())?;
            Ok(Message::Text(text.into()))
        }
    }
}

fn connect_bounded(
    url: &str,
) -> Result<(Socket, tungstenite::handshake::client::Response), String> {
    let url = url.to_owned();
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("h2-telemetry-connect".into())
        .spawn(move || {
            let _ = sender.send(connect_bounded_io(&url));
        })
        .map_err(|e| format!("spawn telemetry connector: {e}"))?;
    receiver
        .recv_timeout(Duration::from_secs(20))
        .map_err(|_| "telemetry connect timed out".to_owned())?
}

fn connect_bounded_io(
    url: &str,
) -> Result<(Socket, tungstenite::handshake::client::Response), String> {
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| "telemetry ingest URL has no scheme".to_owned())?;
    let authority = rest.split('/').next().unwrap_or("");
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let secure = match scheme {
        "ws" => false,
        "wss" => true,
        _ => return Err("telemetry ingest URL must use ws or wss".into()),
    };
    let default_port = if secure { 443 } else { 80 };
    let (host, address) = if let Some(bracketed) = host_port.strip_prefix('[') {
        let (host, suffix) = bracketed
            .split_once(']')
            .ok_or_else(|| "invalid IPv6 telemetry host".to_owned())?;
        let address = if suffix.is_empty() {
            format!("{host_port}:{default_port}")
        } else {
            host_port.to_owned()
        };
        (host, address)
    } else if host_port.contains(':') {
        (
            host_port.split(':').next().unwrap_or(""),
            host_port.to_owned(),
        )
    } else {
        (host_port, format!("{host_port}:{default_port}"))
    };
    let addresses: Vec<_> = address
        .to_socket_addrs()
        .map_err(|e| format!("resolve telemetry ingest: {e}"))?
        .collect();
    if addresses.is_empty() {
        return Err("telemetry ingest resolved no address".into());
    }
    let mut last_error = "telemetry ingest TCP connect failed".to_owned();
    for socket_addr in addresses {
        match TcpStream::connect_timeout(&socket_addr, Duration::from_secs(5)) {
            Ok(tcp) => {
                tcp.set_read_timeout(Some(Duration::from_secs(5)))
                    .map_err(|e| e.to_string())?;
                tcp.set_write_timeout(Some(Duration::from_secs(5)))
                    .map_err(|e| e.to_string())?;
                return if secure {
                    let mut roots = rustls::RootCertStore::empty();
                    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
                    let config = Arc::new(
                        rustls::ClientConfig::builder_with_provider(
                            rustls::crypto::ring::default_provider().into(),
                        )
                        .with_safe_default_protocol_versions()
                        .map_err(|e| format!("telemetry TLS versions: {e}"))?
                        .with_root_certificates(roots)
                        .with_no_client_auth(),
                    );
                    let server_name = rustls::pki_types::ServerName::try_from(host.to_owned())
                        .map_err(|e| format!("telemetry TLS server name: {e}"))?;
                    let connection = rustls::ClientConnection::new(config, server_name)
                        .map_err(|e| format!("telemetry TLS setup: {e}"))?;
                    tungstenite::client::client(
                        url,
                        MaybeTlsStream::Rustls(rustls::StreamOwned::new(connection, tcp)),
                    )
                    .map_err(|e| format!("telemetry ingest handshake: {e}"))
                } else {
                    tungstenite::client::client(url, MaybeTlsStream::Plain(tcp))
                        .map_err(|e| format!("telemetry ingest handshake: {e}"))
                };
            }
            Err(error) => last_error = format!("telemetry ingest TCP connect: {error}"),
        }
    }
    Err(last_error)
}

fn set_timeouts(stream: &mut MaybeTlsStream<TcpStream>) -> Result<(), String> {
    let tcp = match stream {
        MaybeTlsStream::Plain(tcp) => tcp,
        MaybeTlsStream::Rustls(tls) => &mut tls.sock,
        _ => return Err("unsupported telemetry TLS stream".into()),
    };
    tcp.set_read_timeout(Some(Duration::from_millis(50)))
        .map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())
}

fn resolve_api(base_url: &str, api: &str) -> String {
    if api.starts_with("http://") || api.starts_with("https://") {
        return api.trim_end_matches('/').to_owned();
    }
    if api.starts_with('/') {
        let Some(scheme) = base_url.find("://") else {
            return api.trim_end_matches('/').to_owned();
        };
        let authority = scheme + 3;
        let end = base_url[authority..]
            .find('/')
            .map_or(base_url.len(), |i| authority + i);
        return format!("{}{}", &base_url[..end], api.trim_end_matches('/'));
    }
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        api.trim_matches('/')
    )
}

pub(crate) fn signed_events_message(
    key: &KeyPair,
    author: &str,
    game_id: &str,
    events: &[GameEvent],
    now: OffsetDateTime,
) -> Result<ClientMessage, String> {
    if events.is_empty() {
        return Err("cannot sign an empty telemetry batch".into());
    }
    let bodies: Vec<EventBody> = events
        .iter()
        .map(|event| {
            EventBody::new(
                key,
                author,
                &live_proto::game::stream(game_id),
                &event.kind,
                event.data.clone(),
                now,
            )
        })
        .collect();
    Ok(ClientMessage::Events {
        game_id: game_id.to_owned(),
        jws: sign_batch(key, &bodies),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::sign::{KeyPair, SignedEvent, Token};

    #[test]
    fn signed_ingest_batch_verifies_with_live_proto() {
        let key = KeyPair::from_secret([7; 32]);
        let author = "acct:0009000000000001@localhost";
        let events = vec![
            GameEvent::new(
                "server-game",
                "PlayerJoined",
                live_proto::game::PlayerJoined {
                    player: live_proto::game::Player {
                        index: 0,
                        name: "A".into(),
                        xuid: None,
                        team: 0,
                        local: Some(true),
                        appearance: None,
                        bot: None,
                    },
                },
            ),
            GameEvent::new(
                "server-game",
                "ScoreChanged",
                live_proto::game::ScoreChanged {
                    tick: 30,
                    players: vec![],
                    teams: None,
                },
            ),
        ];
        let now = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let message = signed_events_message(&key, author, "server-game", &events, now).unwrap();
        let ClientMessage::Events { game_id, jws } = message else {
            panic!()
        };
        assert_eq!(game_id, "server-game");
        let (parsed, bodies) = SignedEvent::open_batch(&jws).unwrap();
        parsed.verify(key.public()).unwrap();
        assert_eq!(bodies.len(), 2);
        assert_eq!(bodies[0].kind, "PlayerJoined");
        assert_eq!(bodies[1].kind, "ScoreChanged");
        assert!(
            bodies
                .iter()
                .all(|b| b.stream == "game-server-game" && b.author == author)
        );
    }

    #[test]
    fn frames_go_as_packed_records_the_server_decodes() {
        let mut frames = pack::Encoder::new(0, FRAME_KEYFRAME_EVERY);
        let mut decoder = pack::Decoder::new();
        let sample = live_proto::samples::multiplayer(4);
        for (n, frame) in sample.iter().enumerate() {
            let Message::Binary(bytes) = frame_message(&mut frames, frame.clone()).unwrap() else {
                panic!("frame {n} not packed");
            };
            assert!(bytes.len() < MAX_INGEST_MESSAGE);
            let records: Vec<_> = pack::records(&bytes).collect::<Result<_, _>>().unwrap();
            assert_eq!(records.len(), 1);
            let decoded = decoder.decode(records[0]).unwrap();
            assert_eq!(decoded.keyframe, n == 0);
            assert_eq!(decoded.frame, pack::quantize(frame).unwrap());
        }
        // A frame the format cannot carry goes as JSON, and the stream
        // carries on.
        let mut odd = sample[0].clone();
        odd.players[0].pos[0] = f64::NAN;
        assert!(matches!(
            frame_message(&mut frames, odd).unwrap(),
            Message::Text(_)
        ));
        let Message::Binary(bytes) = frame_message(&mut frames, sample[3].clone()).unwrap() else {
            panic!("not packed");
        };
        let record = pack::records(&bytes).next().unwrap().unwrap();
        assert_eq!(
            decoder.decode(record).unwrap().frame,
            pack::quantize(&sample[3]).unwrap()
        );
    }

    #[test]
    fn request_token_verifies_with_live_proto() {
        let key = KeyPair::from_secret([9; 32]);
        let now = 1_800_000_000;
        let token = issue_token(&key, "acct:a@localhost", "localhost", lxm::GAMES_WRITE, now);
        Token::parse(&token)
            .unwrap()
            .verify(key.public(), "localhost", lxm::GAMES_WRITE, now + 1)
            .unwrap();
    }

    #[test]
    fn account_xuids_are_normalized_for_signed_authors() {
        let account = TelemetryAccount {
            xuid: "0009A1B2C3D4E5F6".into(),
            gamertag: "A".into(),
            key: KeyPair::from_secret([3; 32]).private_jwk(),
        };
        let sink = ServerSink::new("http://localhost:8074".into(), account).unwrap();
        assert_eq!(sink.account.xuid, "0009a1b2c3d4e5f6");
    }

    #[test]
    fn api_resolution_handles_absolute_relative_and_root_paths() {
        assert_eq!(
            resolve_api("http://localhost:8074", "/v1"),
            "http://localhost:8074/v1"
        );
        assert_eq!(
            resolve_api("http://localhost:8074/base", "v1"),
            "http://localhost:8074/base/v1"
        );
        assert_eq!(
            resolve_api("http://localhost:8074/base", "https://live.test/v1"),
            "https://live.test/v1"
        );
    }
}
