//! What the Xbox Live replacement's clients (the game-side HLE, the CLI, the
//! web app's model) and its server (`crates/live-server`) share: wire types
//! (docs/live-api.md), signing and verification (docs/live-security.md,
//! [`sign`], [`jcs`]) and the title files the game downloads ([`motd`]).
//!
//! No runtime and no I/O, so the recomp can link it without tokio.

pub mod audit;
pub mod game;
pub mod hopper;
pub mod jcs;
pub mod motd;
pub mod sign;
pub mod team;

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Halo 2's title ID, `4D530064`.
pub const HALO_2_TITLE_ID: TitleId = TitleId(0x4D530064);

/// The downloadable file the game reads after sign-in (docs/live.md
/// section 2). Published with [`HALO_2_TITLE_ID`] by `h2 live motd set`.
pub const HALO_2_MOTD_FILE: &str = "message_of_the_day.dat";

/// An Xbox Live user ID: serialized as 16 lowercase hex digits
/// (`0009000012345678`). Server XUIDs carry the `0x0009` high marker; see
/// `live_server::generate_xuid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Xuid(pub u64);

impl Xuid {
    pub fn parse(s: &str) -> Result<Self, ParseIdError> {
        let t = s
            .strip_prefix("0x")
            .or_else(|| s.strip_prefix("0X"))
            .unwrap_or(s);
        if t.len() != 16 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ParseIdError(s.to_string()));
        }
        Ok(Xuid(
            u64::from_str_radix(t, 16).map_err(|_| ParseIdError(s.to_string()))?,
        ))
    }
}

impl fmt::Display for Xuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

impl Serialize for Xuid {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Xuid {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Xuid::parse(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// A title ID: serialized as 8 uppercase hex digits (`4D530064`); parsed
/// case-insensitively with or without a `0x` prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TitleId(pub u32);

impl TitleId {
    pub fn parse(s: &str) -> Result<Self, ParseIdError> {
        let t = s
            .strip_prefix("0x")
            .or_else(|| s.strip_prefix("0X"))
            .unwrap_or(s);
        if t.len() != 8 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ParseIdError(s.to_string()));
        }
        Ok(TitleId(
            u32::from_str_radix(t, 16).map_err(|_| ParseIdError(s.to_string()))?,
        ))
    }
}

impl fmt::Display for TitleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:08X}", self.0)
    }
}

impl Serialize for TitleId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TitleId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        TitleId::parse(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// A malformed XUID or title ID on the wire.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("expected hex id, got {0:?}")]
pub struct ParseIdError(pub String);

/// A gamertag under original Xbox Live rules: 1..=15 ASCII letters, digits
/// and single spaces, starting with a letter. Comparison and hashing are
/// case-insensitive; the original spelling is kept for display.
#[derive(Debug, Clone)]
pub struct Gamertag(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GamertagError {
    #[error("gamertag must be 1 to 15 characters")]
    Length,
    #[error("gamertag must start with a letter")]
    Start,
    #[error("gamertag may contain only letters, digits and single spaces")]
    Characters,
}

impl Gamertag {
    /// Validates and adopts `s`, keeping its original case.
    pub fn new(s: &str) -> Result<Self, GamertagError> {
        let n = s.len();
        if !(1..=15).contains(&n) {
            return Err(GamertagError::Length);
        }
        let bytes = s.as_bytes();
        if !bytes[0].is_ascii_alphabetic() {
            return Err(GamertagError::Start);
        }
        for (i, b) in bytes.iter().enumerate() {
            if b.is_ascii_alphanumeric() {
                continue;
            }
            // Spaces only between words: never doubled (single spaces) and
            // never trailing.
            let single_space =
                *b == b' ' && bytes.get(i + 1).is_some_and(|n| n.is_ascii_alphanumeric());
            if !single_space {
                return Err(GamertagError::Characters);
            }
        }
        Ok(Gamertag(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The case-insensitive key accounts and gamertag streams are filed
    /// under (also the stream-name spelling on the server).
    pub fn key(&self) -> String {
        self.0.to_ascii_lowercase()
    }
}

impl fmt::Display for Gamertag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Case-insensitive equality and hashing; use `as_str` to compare spelling.
impl PartialEq for Gamertag {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}
impl Eq for Gamertag {}
impl std::hash::Hash for Gamertag {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.key().hash(state);
    }
}

impl Serialize for Gamertag {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Gamertag {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Gamertag::new(&String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

/// Who wrote an event or signed a token: `acct:<xuid>@<server id>` or
/// `server:<server id>` (docs/live-security.md §2).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Author {
    Account { xuid: Xuid, server: String },
    Server(String),
}

impl Author {
    pub fn parse(s: &str) -> Option<Self> {
        if let Some(host) = s.strip_prefix("server:") {
            return (!host.is_empty()).then(|| Author::Server(host.into()));
        }
        let (xuid, host) = s.strip_prefix("acct:")?.split_once('@')?;
        let xuid = Xuid::parse(xuid).ok()?;
        (!host.is_empty()).then(|| Author::Account {
            xuid,
            server: host.into(),
        })
    }

    /// The server this author belongs to.
    pub fn server(&self) -> &str {
        match self {
            Author::Account { server, .. } | Author::Server(server) => server,
        }
    }
}

impl fmt::Display for Author {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Author::Account { xuid, server } => write!(f, "acct:{xuid}@{server}"),
            Author::Server(server) => write!(f, "server:{server}"),
        }
    }
}

/// Stream names (docs/live-api.md, Events).
pub mod streams {
    use super::{Gamertag, TitleId, Xuid};

    pub fn account(xuid: Xuid) -> String {
        format!("account-{xuid}")
    }
    pub fn gamertag(tag: &Gamertag) -> String {
        format!("gamertag-{}", tag.key())
    }
    pub fn presence(xuid: Xuid) -> String {
        format!("presence-{xuid}")
    }
    pub fn title_file(title: TitleId, name: &str) -> String {
        format!("title-{title}-file-{name}")
    }
}

/// Event type names.
pub mod types {
    pub const ACCOUNT_CREATED: &str = "AccountCreated";
    pub const GAMERTAG_RESERVED: &str = "GamertagReserved";
    pub const KEY_ADDED: &str = "KeyAdded";
    pub const KEY_REVOKED: &str = "KeyRevoked";
    pub const TITLE_FILE_PUBLISHED: &str = "TitleFilePublished";
    pub const SIGNED_IN: &str = "SignedIn";
    pub const PRESENCE_CHANGED: &str = "PresenceChanged";
    pub const SIGNED_OUT: &str = "SignedOut";
    pub const PRESENCE_EXPIRED: &str = "PresenceExpired";
    pub const FRIEND_REQUESTED: &str = "FriendRequested";
    pub const FRIEND_ACCEPTED: &str = "FriendAccepted";
    pub const FRIEND_DECLINED: &str = "FriendDeclined";
    pub const FRIEND_REMOVED: &str = "FriendRemoved";
}

/// Token `lxm` values: the method a token is good for.
pub mod lxm {
    pub const EVENTS_WRITE: &str = "events.write";
    /// Game events, `POST /v1/games`, the ingest socket, map outlines.
    pub const GAMES_WRITE: &str = "games.write";
    pub const TITLES_WRITE: &str = "titles.write";
    pub const INVITES_WRITE: &str = "invites.write";
    /// Every read on a `--private` server, the stream included.
    pub const READ: &str = "read";
}

/// Whether a client may send a bearer token to `base_url`: always over
/// HTTPS, over plain HTTP only to a loopback host (docs/live-security.md §6).
pub fn bearer_allowed(base_url: &str) -> bool {
    let lower = base_url.to_ascii_lowercase();
    if lower.starts_with("https://") || lower.starts_with("wss://") {
        return true;
    }
    let Some(rest) = lower
        .strip_prefix("http://")
        .or_else(|| lower.strip_prefix("ws://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = if let Some(v6) = host_port.strip_prefix('[') {
        v6.split_once(']').map_or("", |(h, _)| h)
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// One event as the API serves it (`/v1/events`, the stream). `stream`,
/// `type`, `data` and `at` are copies of what `jws` signs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireEvent {
    pub position: u64,
    pub stream: String,
    pub version: u64,
    #[serde(rename = "type")]
    pub kind: String,
    pub at: String,
    pub data: serde_json::Value,
    pub author: String,
    pub kid: String,
    pub jws: String,
    /// The element of a batch JWS this event is; absent for a single event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jws_index: Option<usize>,
    pub hash: String,
}

/// A list response. `next` is opaque: pass it back as `?cursor=`.
/// `position` is the log head when the list was read, so a client can open
/// the stream with `?from=<position>` after loading it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
    pub position: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Health {
    pub ok: bool,
    pub position: u64,
}

/// `GET /.well-known/h2live.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Discovery {
    pub id: String,
    pub api: String,
    pub keys: Vec<sign::Jwk>,
    pub software: String,
    pub federation: bool,
}

/// `POST /v1/accounts` reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatedAccount {
    pub xuid: Xuid,
    pub gamertag: Gamertag,
    /// The author id: `acct:<xuid>@<server id>`.
    pub id: String,
}

/// An entry of `GET /v1/accounts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountSummary {
    pub xuid: Xuid,
    pub gamertag: Gamertag,
    pub created_at: String,
    pub online: bool,
    /// Whether the account is an admin (docs/live-admin.md): configured at
    /// startup or granted. Only this server's admins, not a federation view.
    #[serde(default)]
    pub admin: bool,
}

/// `GET /v1/accounts/{gamertag}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountDetail {
    #[serde(flatten)]
    pub summary: AccountSummary,
    pub id: String,
    /// The latest games the account played in, newest first.
    pub recent_games: Vec<game::GameSummary>,
    pub position: u64,
}

/// One key of `GET /v1/accounts/{gamertag}/keys`: valid for events at
/// positions `valid_from <= p < valid_to` (`valid_to` null while current).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountKey {
    pub kid: String,
    pub jwk: sign::Jwk,
    pub valid_from: u64,
    pub valid_to: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountKeys {
    pub keys: Vec<AccountKey>,
    pub position: u64,
}

/// Presence states (docs/live-api.md, Clarifications 5). `playing` carries
/// a `game_id`; offline is the absence of presence.
pub const PRESENCE_STATES: [&str; 3] = ["online", "away", "playing"];

/// An entry of `GET /v1/presence`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Presence {
    pub xuid: Xuid,
    pub gamertag: Gamertag,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_id: Option<String>,
    pub since: String,
}

/// The data a `FriendRequested`, `FriendAccepted` or `FriendDeclined` event
/// signs: `from` is the requester, `to` the account the request is
/// addressed to. Requests are directional; the friendship, once accepted,
/// is mutual (docs/live-api.md, Friends).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendEventData {
    pub from: Xuid,
    pub to: Xuid,
}

/// The data a `FriendRemoved` event signs: `by` is the account writing it,
/// `friend` the other side of the friendship or pending request it ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FriendRemovedData {
    pub by: Xuid,
    pub friend: Xuid,
}

/// A friend's current presence, folded into a friends read. Offline is the
/// absence of `presence` on the friend, not a state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FriendPresence {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_id: Option<String>,
    pub since: String,
}

/// One entry of the `friends` list of `GET /v1/accounts/{gamertag}/friends`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Friend {
    pub xuid: Xuid,
    pub gamertag: Gamertag,
    /// When they became friends: the `FriendAccepted` event's `at`.
    pub friends_since: String,
    /// Current presence; absent when the friend is offline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presence: Option<FriendPresence>,
}

/// A pending request of `GET /v1/accounts/{gamertag}/friends`: the other
/// account (the requester for `requests_in`, the target for `requests_out`)
/// and when the request was made.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FriendRequest {
    pub xuid: Xuid,
    pub gamertag: Gamertag,
    pub at: String,
}

/// `GET /v1/accounts/{gamertag}/friends`: an account's friends (with
/// presence, sorted by gamertag) and its pending requests in and out —
/// everything the game's friends list needs in one document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Friends {
    pub friends: Vec<Friend>,
    pub requests_in: Vec<FriendRequest>,
    pub requests_out: Vec<FriendRequest>,
    pub position: u64,
}

/// `PUT /v1/titles/{title}/files/{name}` reply, and the
/// `TitleFilePublished` event data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishedFile {
    pub title_id: TitleId,
    pub name: String,
    /// Lowercase hex SHA-256 of the bytes; also the file's ETag.
    pub sha256: String,
    pub size: usize,
}

/// Messages a `/v1/stream` reader receives.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamMessage {
    Event {
        event: Box<WireEvent>,
    },
    /// A live telemetry frame (not stored; up to 15 Hz per game).
    Frame {
        frame: Box<game::Frame>,
    },
    CaughtUp {
        position: u64,
    },
}

/// Messages a WebSocket client sends. Readers of `/v1/stream` send only
/// `auth`; the game's `/v1/ingest` sends all of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClientMessage {
    Auth {
        token: String,
    },
    Frame {
        frame: Box<game::Frame>,
    },
    /// One batch JWS over game event bodies (security §2, Batches).
    Events {
        game_id: String,
        jws: String,
    },
    /// Keeps the sender's presence alive.
    Ping,
}

/// What the ingest socket sends back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IngestReply {
    /// A batch was appended; `position` is its last event's.
    Appended { game_id: String, position: u64 },
    /// A message was refused; the socket stays open unless auth failed.
    Error { error: ErrorCode, message: String },
}

/// Every non-2xx body: `{"error": "<code>", "message": "..."}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub error: ErrorCode,
    pub message: String,
}

impl ErrorBody {
    pub fn new(error: ErrorCode, message: impl Into<String>) -> Self {
        ErrorBody {
            error,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// 400.
    Invalid,
    /// 401: missing or bad token or signature.
    Unauthorized,
    /// 403: authenticated but not allowed.
    Forbidden,
    NotFound,
    /// 409: taken, replayed, or a version race.
    Conflict,
    /// 413.
    TooLarge,
    /// 429.
    RateLimited,
    /// 501: in the contract, not built yet.
    Unimplemented,
    /// 503 (and 500).
    Unavailable,
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = serde_json::to_value(self).map_err(|_| fmt::Error)?;
        f.write_str(v.as_str().unwrap_or("?"))
    }
}

/// Title-storage file names: `[A-Za-z0-9_.-]{1,64}`. The name is also a
/// stream-id component on the server, so it stays deliberately tight.
pub fn valid_file_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamertag_accepts_live_spellings() {
        for ok in [
            "a",
            "Player1",
            "Foo Bar",
            "Xbox Live User",
            "abcdefghijklmno",
        ] {
            assert_eq!(Gamertag::new(ok).unwrap().as_str(), ok, "{ok}");
        }
        // Display case is kept.
        assert_eq!(Gamertag::new("NoIcE").unwrap().to_string(), "NoIcE");
        // ...but comparison is case-insensitive.
        assert_eq!(
            Gamertag::new("NoIcE").unwrap(),
            Gamertag::new("noice").unwrap()
        );
        assert_eq!(
            Gamertag::new("Xbox Live User").unwrap().key(),
            "xbox live user"
        );
    }

    #[test]
    fn gamertag_rejects_bad_ones() {
        for (bad, err) in [
            ("", GamertagError::Length),
            ("abcdefghijklmnoX", GamertagError::Length),
            ("1abc", GamertagError::Start),
            (" abc", GamertagError::Start),
            ("ab_c", GamertagError::Characters),
            ("ab-c", GamertagError::Characters),
            ("ab cé", GamertagError::Characters),
            ("a  b", GamertagError::Characters),
            ("ab ", GamertagError::Characters),
            ("a ", GamertagError::Characters),
        ] {
            assert_eq!(Gamertag::new(bad), Err(err), "{bad:?}");
        }
    }

    #[test]
    fn gamertag_serde_round_trips_and_rejects() {
        #[derive(Serialize, Deserialize)]
        struct Wrap {
            tag: Gamertag,
        }
        let w: Wrap = serde_json::from_str(r#"{"tag":"Foo Bar"}"#).unwrap();
        assert_eq!(w.tag.as_str(), "Foo Bar");
        assert_eq!(
            serde_json::to_value(&w).unwrap(),
            serde_json::json!({"tag": "Foo Bar"})
        );
        assert!(serde_json::from_str::<Wrap>(r#"{"tag":"a  b"}"#).is_err());
    }

    #[test]
    fn ids_are_hex_strings() {
        let x = Xuid(0x0009_1234_5678_9abc);
        assert_eq!(
            serde_json::to_value(x).unwrap(),
            serde_json::json!("0009123456789abc")
        );
        assert_eq!(x.to_string(), "0009123456789abc");
        assert_eq!(Xuid::parse("0009123456789abc").unwrap(), x);
        assert_eq!(Xuid::parse("0x0009123456789ABC").unwrap(), x);
        assert_eq!(
            serde_json::from_value::<Xuid>(serde_json::json!("0009123456789abc")).unwrap(),
            x
        );

        let t = HALO_2_TITLE_ID;
        assert_eq!(
            serde_json::to_value(t).unwrap(),
            serde_json::json!("4D530064")
        );
        assert_eq!(TitleId::parse("4D530064").unwrap(), t);
        assert_eq!(TitleId::parse("0x4d530064").unwrap(), t);

        for bad in ["", "123", "zzzz", "0009123456789abc0", "0x123"] {
            assert!(Xuid::parse(bad).is_err(), "{bad}");
            assert!(TitleId::parse(bad).is_err(), "{bad}");
        }
        assert!(serde_json::from_value::<Xuid>(serde_json::json!(16)).is_err());
    }

    #[test]
    fn file_names() {
        for ok in [
            "message_of_the_day.dat",
            "multiplayer_hopper.dat",
            "a",
            "A-b_C.0",
            ".hidden",
        ] {
            assert!(valid_file_name(ok), "{ok}");
        }
        let long = "x".repeat(65);
        for bad in ["", "a/b", "a\\b", "a b", long.as_str()] {
            assert!(!valid_file_name(bad), "{bad:?}");
        }
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn authors() {
        let a = Author::parse("acct:0009000000000001@live.example").unwrap();
        assert_eq!(
            a,
            Author::Account {
                xuid: Xuid(0x0009_0000_0000_0001),
                server: "live.example".into()
            }
        );
        assert_eq!(a.to_string(), "acct:0009000000000001@live.example");
        assert_eq!(
            Author::parse("server:h").unwrap(),
            Author::Server("h".into())
        );
        for bad in [
            "",
            "server:",
            "acct:1@h",
            "acct:0009000000000001@",
            "acct:0009000000000001",
            "user:x",
        ] {
            assert_eq!(Author::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn bearer_only_over_tls_or_loopback() {
        for ok in [
            "https://live.example.net",
            "http://127.0.0.1:8074",
            "http://127.5.0.1",
            "http://localhost:8074/v1",
            "http://[::1]:8074",
            "ws://localhost/v1/stream",
            "HTTP://LOCALHOST",
        ] {
            assert!(bearer_allowed(ok), "{ok}");
        }
        for bad in [
            "http://live.example.net",
            "http://192.168.1.2:8074",
            "http://localhost.evil.com",
            "http://127.0.0.1@evil.com",
            "http://[::2]",
            "ftp://localhost",
            "localhost",
        ] {
            assert!(!bearer_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn error_body_shape() {
        let b = ErrorBody::new(ErrorCode::Unimplemented, "later");
        assert_eq!(
            serde_json::to_value(&b).unwrap(),
            serde_json::json!({"error": "unimplemented", "message": "later"})
        );
        assert_eq!(ErrorCode::RateLimited.to_string(), "rate_limited");
    }

    #[test]
    fn stream_messages() {
        let m = StreamMessage::CaughtUp { position: 7 };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({"kind": "caught_up", "position": 7})
        );
        let c: ClientMessage = serde_json::from_str(r#"{"kind":"auth","token":"t"}"#).unwrap();
        assert_eq!(c, ClientMessage::Auth { token: "t".into() });
    }

    #[test]
    fn friend_event_data_is_two_xuids() {
        assert_eq!(
            serde_json::to_value(FriendEventData {
                from: Xuid(0x0009_0000_0000_0001),
                to: Xuid(0x0009_0000_0000_0002),
            })
            .unwrap(),
            serde_json::json!({"from": "0009000000000001", "to": "0009000000000002"})
        );
        assert_eq!(
            serde_json::from_value::<FriendEventData>(serde_json::json!({
                "from": "0009000000000001", "to": "0009000000000002"
            }))
            .unwrap(),
            FriendEventData {
                from: Xuid(0x0009_0000_0000_0001),
                to: Xuid(0x0009_0000_0000_0002)
            }
        );
        assert_eq!(
            serde_json::to_value(FriendRemovedData {
                by: Xuid(0x0009_0000_0000_0001),
                friend: Xuid(0x0009_0000_0000_0002),
            })
            .unwrap(),
            serde_json::json!({"by": "0009000000000001", "friend": "0009000000000002"})
        );
    }

    #[test]
    fn friends_document_round_trips() {
        let friends = Friends {
            friends: vec![Friend {
                xuid: Xuid(0x0009_0000_0000_0002),
                gamertag: Gamertag::new("NoIcE").unwrap(),
                friends_since: "2026-11-15T20:14:03.120Z".into(),
                presence: Some(FriendPresence {
                    state: "playing".into(),
                    game_id: Some("7f3c".into()),
                    since: "2026-11-15T20:15:00.000Z".into(),
                }),
            }],
            requests_in: vec![FriendRequest {
                xuid: Xuid(0x0009_0000_0000_0003),
                gamertag: Gamertag::new("Sneaky Pete").unwrap(),
                at: "2026-11-15T20:10:00.000Z".into(),
            }],
            requests_out: Vec::new(),
            position: 1042,
        };
        assert_eq!(
            serde_json::to_value(&friends).unwrap(),
            serde_json::json!({
                "friends": [{
                    "xuid": "0009000000000002", "gamertag": "NoIcE",
                    "friends_since": "2026-11-15T20:14:03.120Z",
                    "presence": {
                        "state": "playing", "game_id": "7f3c",
                        "since": "2026-11-15T20:15:00.000Z"
                    }
                }],
                "requests_in": [{
                    "xuid": "0009000000000003", "gamertag": "Sneaky Pete",
                    "at": "2026-11-15T20:10:00.000Z"
                }],
                "requests_out": [],
                "position": 1042
            })
        );
        // An offline friend has no `presence`, an online one no `game_id`
        // unless playing.
        let back = serde_json::from_value::<Friends>(serde_json::json!({
            "friends": [{
                "xuid": "0009000000000002", "gamertag": "NoIcE",
                "friends_since": "2026-11-15T20:14:03.120Z",
                "presence": {"state": "online", "since": "2026-11-15T20:15:00.000Z"}
            }],
            "requests_in": [], "requests_out": [], "position": 7
        }))
        .unwrap();
        assert_eq!(
            back.friends[0].presence.as_ref().map(|p| p.game_id.clone()),
            Some(None)
        );
        let gone = serde_json::from_value::<Friends>(serde_json::json!({
            "friends": [{
                "xuid": "0009000000000002", "gamertag": "NoIcE",
                "friends_since": "2026-11-15T20:14:03.120Z"
            }],
            "requests_in": [], "requests_out": [], "position": 7
        }))
        .unwrap();
        assert!(gone.friends[0].presence.is_none());
    }
}
