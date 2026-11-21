//! What the Xbox Live replacement's clients (the game-side HLE, the CLI, the
//! web app's model) and its server (`crates/live/live-server`) share: wire types
//! (docs/live/live-api.md), signing and verification (docs/live/live-security.md,
//! [`sign`], [`jcs`]) and the title files the game downloads ([`motd`]).
//!
//! No runtime and no I/O, so the recomp can link it without tokio.

pub mod arbitration;
pub mod archive;
pub mod audit;
pub mod bans;
pub mod bots;
pub mod chunk;
pub mod film;
pub mod game;
pub mod hopper;
pub mod host_pack;
pub mod hosts; // dh-lease
pub mod jcs;
pub mod join_link;
pub use join_link::JoinLink;
pub mod mailbox;
pub mod map_details;
pub mod maps;
mod mod_packs;
pub mod motd;
pub mod mutes;
pub mod matchmaking;
pub mod optimatch;
pub mod pack;
pub mod rank;
pub mod recovery;
pub mod reports;
#[doc(hidden)]
pub mod samples;
pub mod session;
pub mod sign;
pub mod skulls;
pub mod storage;
pub mod team;
pub mod title_file;
pub mod tunnel;

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Halo 2's title ID, `4D530064`.
pub const HALO_2_TITLE_ID: TitleId = TitleId(0x4D530064);

/// The public Live service: what `h2 play --live` signs in to, and what the
/// tvOS, iOS and Xbox apps use unless told otherwise (docs/live/live.md
/// "Playing on Live"). The one place it is written; `$H2_LIVE_SERVER`
/// replaces it for a run.
pub const DEFAULT_SERVER: &str = "https://live.forbes.red";

/// What `$H2_LIVE_SERVER` says about the server Live would use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerOverride {
    /// Unset or blank: no change.
    Unset,
    /// `off`, `none` or `0`: Live off for this run.
    Off(String),
    /// This server instead (any trailing `/` dropped).
    Url(String),
}

impl ServerOverride {
    /// Reads the variable's value.
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim).filter(|v| !v.is_empty()) {
            None => ServerOverride::Unset,
            Some(v) if matches!(v.to_ascii_lowercase().as_str(), "off" | "none" | "0") => {
                ServerOverride::Off(v.to_string())
            }
            Some(v) => ServerOverride::Url(v.trim_end_matches('/').to_string()),
        }
    }
}

/// The downloadable file the game reads after sign-in (docs/live/live.md
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
/// `server:<server id>` (docs/live/live-security.md §2).
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

/// Stream names (docs/live/live-api.md, Events).
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
    /// Saving films and creating or deleting Theater clips.
    pub const FILMS_WRITE: &str = "films.write";
    pub const SESSIONS_WRITE: &str = "sessions.write";
    /// The co-op relay socket of a session the token's account is in.
    pub const SESSIONS_RELAY: &str = "sessions.relay";
    pub const TITLES_WRITE: &str = "titles.write";
    pub const INVITES_WRITE: &str = "invites.write";
    /// Listing only the caller's open invites.
    pub const INVITES_READ: &str = "invites.read";
    /// Admins and the operator setting an account's invite permission.
    pub const INVITE_GRANTS_WRITE: &str = "invite-grants.write";
    /// Reading the account's own recovery secret and current code.
    pub const RECOVERY_READ: &str = "recovery.read";
    /// Setting up or resetting the account's recovery secret.
    pub const RECOVERY_WRITE: &str = "recovery.write";
    /// Listing the account's own sign-ins (its current keys).
    pub const SIGNINS_READ: &str = "signins.read";
    /// Signing the account's other sign-ins out (revoking their keys).
    pub const SIGNINS_WRITE: &str = "signins.write";
    /// Content uploads, package publishing and reports (docs/game/maps.md §5).
    pub const CONTENT_WRITE: &str = "content.write";
    /// Admin moderation of uploaded content and hopper pins.
    pub const CONTENT_MODERATE: &str = "content.moderate";
    /// Reporting another player (docs/live/live-api.md "Player reports").
    pub const REPORTS_WRITE: &str = "reports.write";
    /// Admins reading and reviewing player reports.
    pub const REPORTS_MODERATE: &str = "reports.moderate";
    /// Changing one's own mute list (docs/live/live-voice.md §3.6).
    pub const MUTES_WRITE: &str = "mutes.write";
    /// Reading one's own cloud saves (docs/live/live-cloud-saves.md).
    pub const STORAGE_READ: &str = "storage.read";
    /// Writing and deleting one's own cloud saves.
    pub const STORAGE_WRITE: &str = "storage.write";
    /// Every read on a `--private` server, the stream included.
    pub const READ: &str = "read";
    /// The operator reading the log as stored, to copy it
    /// (`GET /v1/admin/log`, `h2 live store copy --from https://`).
    pub const LOG_READ: &str = "log.read";
    /// The operator importing a log copied from another server
    /// (`POST /v1/admin/log`, `h2 live store copy --to https://`).
    pub const LOG_WRITE: &str = "log.write";
}

/// Whether a client may send a bearer token to `base_url`: always over
/// HTTPS, over plain HTTP only to a loopback host (docs/live/live-security.md §6).
pub fn bearer_allowed(base_url: &str) -> bool {
    let Ok(url) = url::Url::parse(base_url) else {
        return false;
    };
    match url.scheme() {
        "https" | "wss" => url.has_host(),
        "http" | "ws" => match url.host() {
            Some(url::Host::Domain(name)) => name == "localhost",
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        _ => false,
    }
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
    /// Additional packed ingest versions this server accepts beyond the v7 baseline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ingest_frame_versions: Vec<u8>,
    /// Private sim combat previews are accepted on the authenticated ingest socket.
    #[serde(default)]
    pub sim_combat: bool,
    pub id: String,
    pub api: String,
    pub keys: Vec<sign::Jwk>,
    pub software: String,
    pub federation: bool,
    /// The server's public base URL when it knows one (a tunnel or proxy,
    /// docs/live/live-hosting.md). Informational: clients keep the base URL they
    /// were given, and `api` stays relative to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// `POST /v1/accounts` reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatedAccount {
    pub xuid: Xuid,
    pub gamertag: Gamertag,
    /// The author id: `acct:<xuid>@<server id>`.
    pub id: String,
    /// On creation: the account's new recovery secret and current code
    /// ([`recovery`]), shown to the user once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<recovery::RecoveryInfo>,
}

/// An account's explicit permission to create invites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InviteGrant {
    /// The maximum number of open invites, or no cap.
    pub open_cap: Option<std::num::NonZeroU32>,
}

/// An entry of `GET /v1/accounts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccountSummary {
    pub xuid: Xuid,
    pub gamertag: Gamertag,
    pub created_at: String,
    pub online: bool,
    /// Whether the account is an admin (docs/live/live-admin.md): configured at
    /// startup or granted. Only this server's admins, not a federation view.
    #[serde(default)]
    pub admin: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite_grant: Option<InviteGrant>,
    /// A bot account (docs/game/bots.md §9.5): issued by the server for
    /// Clankers, never signed in with a key, never recovered or claimed.
    /// Absent for a human.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bot: bool,
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

/// Presence states (docs/live/live-api.md, Clarifications 5). `playing` carries
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
    /// What the player is doing, in words ("Co-op: Outskirts"); only some
    /// activities say. Left out when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    pub since: String,
    /// The build the player's console runs (its git short hash), when it
    /// says; [`valid_build`]. Shown to every reader, as `state` is: it
    /// names software, not where the player is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// What that build can do ([`capability`]); [`valid_capabilities`].
    /// Shown to every reader, as `build` is.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

impl Presence {
    /// Whether the console announced `capability`.
    pub fn has(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
}

/// The data of a `SignedIn`, `PresenceChanged`, `SignedOut` or
/// `PresenceExpired` event: what a console sets for one of its players.
/// `build` and `capabilities` ride on the first two; an older server
/// ignores them, an older console leaves them out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresenceData {
    pub xuid: Xuid,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub game_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

/// Capabilities a console's build announces on presence, `name/version`
/// (docs/game/game-options.md section 3, "Older builds").
pub mod capability {
    /// Applies a Live host's variant options: the extension block in the
    /// record's spare bytes, taken from the host's session (slice 4).
    pub const VARIANT_EXT: &str = "variant-ext/1";
}

/// The most capabilities one presence carries.
pub const PRESENCE_CAPABILITIES_MAX: usize = 16;
/// The longest capability, in bytes.
pub const CAPABILITY_MAX: usize = 32;

/// Whether `build` may be a presence's build: a git short hash, 7 to 12
/// lowercase hex digits.
pub fn valid_build(build: &str) -> bool {
    (7..=12).contains(&build.len())
        && build
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether `c` may be a capability: `name/version`, the name lowercase
/// letters, digits and `-`, the version digits, at most [`CAPABILITY_MAX`]
/// bytes.
pub fn valid_capability(c: &str) -> bool {
    let Some((name, version)) = c.split_once('/') else {
        return false;
    };
    c.len() <= CAPABILITY_MAX
        && !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !version.is_empty()
        && version.bytes().all(|b| b.is_ascii_digit())
}

/// Whether `c` may be a presence's capabilities: at most
/// [`PRESENCE_CAPABILITIES_MAX`], each [`valid_capability`].
pub fn valid_capabilities(c: &[String]) -> bool {
    c.len() <= PRESENCE_CAPABILITIES_MAX && c.iter().all(|c| valid_capability(c))
}

/// The players among `xuids` whose presence lacks `capability`, in the
/// order of `xuids`. A player with no presence (a bot, someone signed out)
/// is left out: nothing says what it runs. The host's check of its guests
/// before a game with variant options (docs/game/game-options.md section
/// 3); `xuids` may come from the session's roster or the game's own
/// player list.
pub fn lacking<'a>(
    capability: &str,
    xuids: &[Xuid],
    presence: &'a [Presence],
) -> Vec<&'a Presence> {
    xuids
        .iter()
        .filter_map(|x| presence.iter().find(|p| p.xuid == *x))
        .filter(|p| !p.has(capability))
        .collect()
}

/// The longest presence `activity`, in characters.
pub const PRESENCE_ACTIVITY_MAX: usize = 64;

/// Whether `activity` may be a presence's activity: some text, at most
/// [`PRESENCE_ACTIVITY_MAX`] characters, no control characters.
pub fn valid_activity(activity: &str) -> bool {
    !activity.trim().is_empty()
        && activity.chars().count() <= PRESENCE_ACTIVITY_MAX
        && !activity.chars().any(char::is_control)
}

/// The data a `FriendRequested`, `FriendAccepted` or `FriendDeclined` event
/// signs: `from` is the requester, `to` the account the request is
/// addressed to. Requests are directional; the friendship, once accepted,
/// is mutual (docs/live/live-api.md, Friends).
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
    /// As [`Presence::activity`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<String>,
    pub since: String,
    /// As [`Presence::build`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build: Option<String>,
    /// As [`Presence::capabilities`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
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
    /// Latest combat diagnostics for one bot in a running private sim.
    SimCombat { game_id: String, sample: Box<game::SimCombatSample> },
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
    /// 410: there was such a thing, and an admin took it down (hidden,
    /// deleted or blocked content, docs/live/live-admin.md).
    Gone,
    /// 413.
    TooLarge,
    /// 429.
    RateLimited,
    /// 403: the account is banned (docs/live/live-admin.md "Bans"); the
    /// message says until when.
    Banned,
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
    fn the_server_variable_overrides_or_turns_live_off() {
        assert_eq!(DEFAULT_SERVER, "https://live.forbes.red");
        assert_eq!(ServerOverride::parse(None), ServerOverride::Unset);
        assert_eq!(ServerOverride::parse(Some("  ")), ServerOverride::Unset);
        for off in ["off", "OFF", "none", " 0 "] {
            assert!(
                matches!(ServerOverride::parse(Some(off)), ServerOverride::Off(_)),
                "{off}"
            );
        }
        assert_eq!(
            ServerOverride::parse(Some(" https://live.example/ ")),
            ServerOverride::Url("https://live.example".into())
        );
    }

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
            r"http://remote.invalid\@localhost/",
            "http://localhost:invalid",
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
                    activity: None,
                    since: "2026-11-15T20:15:00.000Z".into(),
                    build: None,
                    capabilities: Vec::new(),
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

    #[test]
    fn presence_carries_a_build_and_capabilities_checked_and_optional() {
        for ok in ["a65fa01", "a65fa010", "0123456789ab"] {
            assert!(valid_build(ok), "{ok}");
        }
        for bad in [
            "",
            "a65fa0",
            "0123456789abc",
            "A65FA01",
            "a65fa0g",
            "unknown",
        ] {
            assert!(!valid_build(bad), "{bad}");
        }
        for ok in [capability::VARIANT_EXT, "a/0", "x-2/10"] {
            assert!(valid_capability(ok), "{ok}");
        }
        let long = format!("{}/1", "a".repeat(CAPABILITY_MAX - 1));
        for bad in [
            "",
            "variant-ext",
            "variant-ext/",
            "/1",
            "Variant-ext/1",
            "variant_ext/1",
            "variant-ext/1a",
            "variant-ext/1/2",
            long.as_str(),
        ] {
            assert!(!valid_capability(bad), "{bad}");
        }
        let many = vec![capability::VARIANT_EXT.to_string(); PRESENCE_CAPABILITIES_MAX];
        assert!(valid_capabilities(&many));
        let too_many = vec![capability::VARIANT_EXT.to_string(); PRESENCE_CAPABILITIES_MAX + 1];
        assert!(!valid_capabilities(&too_many));
        assert!(valid_capabilities(&[]));

        // Left out when there are none, so an older reader sees the same
        // bytes; read back when there are.
        let bare: PresenceData = serde_json::from_value(serde_json::json!({
            "xuid": "0009000000000001", "state": "online", "game_id": null
        }))
        .unwrap();
        assert_eq!((bare.build.as_deref(), bare.capabilities.len()), (None, 0));
        let v = serde_json::to_value(&bare).unwrap();
        assert!(v.get("build").is_none() && v.get("capabilities").is_none());
        let full: PresenceData = serde_json::from_value(serde_json::json!({
            "xuid": "0009000000000001", "state": "online", "game_id": null,
            "build": "a65fa01", "capabilities": ["variant-ext/1"]
        }))
        .unwrap();
        assert_eq!(full.build.as_deref(), Some("a65fa01"));
        assert_eq!(full.capabilities, [capability::VARIANT_EXT]);
    }

    #[test]
    fn lacking_names_the_players_whose_build_cannot_and_skips_the_unknown() {
        let p = |x: u64, tag: &str, caps: &[&str]| Presence {
            xuid: Xuid(x),
            gamertag: Gamertag::new(tag).unwrap(),
            state: "playing".into(),
            game_id: None,
            activity: None,
            since: "2026-11-18T20:00:00.000Z".into(),
            build: None,
            capabilities: caps.iter().map(|c| c.to_string()).collect(),
        };
        let presence = [
            p(1, "Host", &[capability::VARIANT_EXT]),
            p(2, "New", &["other/1", capability::VARIANT_EXT]),
            p(3, "Old", &[]),
            p(4, "Older", &["variant-ext/0"]),
            p(9, "Elsewhere", &[]),
        ];
        let named: Vec<_> = lacking(
            capability::VARIANT_EXT,
            &[Xuid(4), Xuid(2), Xuid(3), Xuid(7)],
            &presence,
        )
        .iter()
        .map(|p| p.gamertag.as_str().to_string())
        .collect();
        // 7 has no presence (a bot): nothing says what it runs.
        assert_eq!(named, ["Older", "Old"]);
        assert!(lacking(capability::VARIANT_EXT, &[], &presence).is_empty());
    }
}
