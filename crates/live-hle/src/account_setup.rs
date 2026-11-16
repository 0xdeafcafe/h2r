//! In-game Live account setup (docs/live/live.md, "In-game account setup"):
//! create an account, recover one on this machine with its TOTP code, and
//! read the current recovery code, all host side with no guest memory.
//!
//! Each call is blocking HTTP; run it off the game thread ([`Job`]). New
//! entries go into the same accounts JSON the CLI keeps (`{xuid, gamertag,
//! id, server, server_id, kid, key}`, the key a private JWK), written mode
//! 0600 through a temporary file and a rename; other entries keep every
//! field they have. [`crate::Hle::reload_accounts`] then makes a saved entry
//! usable for sign-in without a restart.
//!
//! Nothing here formats a key, a server reply body or a typed code into an
//! error: [`SetupError`]'s text is a short line for the player.

use live_proto::recovery::{RecoverBody, RecoverRequest, RecoveryInfo};
use live_proto::sign::{AccountRequest, KeyPair, PrivateJwk, issue_token};
use live_proto::{Author, CreatedAccount, Discovery, ErrorBody, Xuid, lxm};
use serde_json::{Map, Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;
use url::Url;

/// How long one request may take.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// The largest reply read.
const MAX_REPLY: u64 = 64 * 1024;

/// Why setup failed, as the player reads it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SetupError {
    #[error("That gamertag is taken.")]
    GamertagTaken,
    #[error("That gamertag can't be used.")]
    BadGamertag,
    #[error("This server needs an invite code.")]
    NeedsInvite,
    #[error("That invite code isn't valid.")]
    InviteInvalid,
    #[error("No account has that gamertag.")]
    NoSuchGamertag,
    #[error("That account has no recovery set up.")]
    NoRecovery,
    #[error("Wrong code. Wait for the next one and try again.")]
    WrongCode,
    #[error("Too many wrong codes. Try again in {minutes} min.")]
    LockedOut { minutes: u64 },
    #[error("The server is busy. Try again shortly.")]
    Busy,
    #[error("Can't reach the server.")]
    Unreachable,
    /// Plain HTTP to a host that is not this machine: keys and secrets
    /// would cross the network in the clear.
    #[error("The server address must be https.")]
    Insecure,
    #[error("The server said no ({0}).")]
    Server(u16),
    #[error("Couldn't save the account on this machine.")]
    Save,
}

/// What a request is for, which decides how a refusal reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Create,
    Recover,
    RecoveryCode,
}

impl SetupError {
    /// A refusal: the HTTP status and the server's `message`.
    pub fn from_reply(action: Action, status: u16, message: &str) -> Self {
        let m = message.to_ascii_lowercase();
        match (action, status) {
            (_, 429) if m.contains("wrong code") => SetupError::LockedOut {
                minutes: lockout_minutes(&m),
            },
            (_, 429 | 503) => SetupError::Busy,
            (Action::Create, 409) => SetupError::GamertagTaken,
            (Action::Create, 403) if m.contains("needs an invite") => SetupError::NeedsInvite,
            (Action::Create, 403) => SetupError::InviteInvalid,
            (Action::Create | Action::Recover, 400) if m.contains("gamertag") => {
                SetupError::BadGamertag
            }
            (Action::Recover, 400) if m.contains("code") => SetupError::WrongCode,
            (Action::Recover, 404) => SetupError::NoSuchGamertag,
            (_, 403 | 404) if m.contains("no recovery") => SetupError::NoRecovery,
            (Action::Recover, 403) => SetupError::WrongCode,
            (_, s) => SetupError::Server(s),
        }
    }
}

/// `try again in N s` as whole minutes, rounded up, at least one.
fn lockout_minutes(message: &str) -> u64 {
    let secs = message
        .split("try again in ")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(60);
    secs.div_ceil(60).max(1)
}

/// One account this machine holds a key for: an accounts-file entry.
#[derive(Clone)]
pub struct LocalEntry {
    pub xuid: Xuid,
    pub gamertag: String,
    /// `acct:<xuid>@<server id>`.
    pub id: String,
    /// The base URL it was made or recovered on.
    pub server: String,
    pub server_id: String,
    pub key: Arc<KeyPair>,
}

impl std::fmt::Debug for LocalEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEntry")
            .field("xuid", &self.xuid)
            .field("gamertag", &self.gamertag)
            .field("server_id", &self.server_id)
            .field("kid", &self.key.kid())
            .finish_non_exhaustive()
    }
}

impl LocalEntry {
    fn new(created: &CreatedAccount, server: &str, server_id: &str, key: KeyPair) -> Self {
        LocalEntry {
            xuid: created.xuid,
            gamertag: created.gamertag.to_string(),
            id: created.id.clone(),
            server: server.trim_end_matches('/').into(),
            server_id: server_id.into(),
            key: Arc::new(key),
        }
    }

    /// The entry as the accounts file holds it (the CLI's shape).
    pub fn to_json(&self) -> Value {
        json!({
            "xuid": self.xuid,
            "gamertag": self.gamertag,
            "id": self.id,
            "server": self.server,
            "server_id": self.server_id,
            "kid": self.key.kid(),
            "key": self.key.private_jwk(),
        })
    }

    /// Reads an accounts-file entry with a private-JWK key; `None` for one
    /// this cannot use (no key, the old seed form, missing fields). Never
    /// reports why: the input holds a private key.
    pub fn from_json(v: &Value) -> Option<Self> {
        let s = |k: &str| v.get(k).and_then(Value::as_str);
        let xuid = Xuid::parse(s("xuid")?).ok()?;
        let gamertag = s("gamertag")?.to_string();
        let jwk: PrivateJwk = serde_json::from_value(v.get("key")?.clone()).ok()?;
        let key = KeyPair::from_private_jwk(&jwk).ok()?;
        if s("kid").is_some_and(|k| k != key.kid()) {
            return None;
        }
        let server_id = s("server_id")?.to_string();
        Some(LocalEntry {
            xuid,
            id: s("id").map_or_else(|| author_id(xuid, &server_id), str::to_string),
            gamertag,
            server: s("server").unwrap_or_default().to_string(),
            server_id,
            key: Arc::new(key),
        })
    }
}

fn author_id(xuid: Xuid, server_id: &str) -> String {
    Author::Account {
        xuid,
        server: server_id.into(),
    }
    .to_string()
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// A server's base URL, its discovery and API root.
struct Server {
    base: String,
    api: String,
    id: String,
    agent: ureq::Agent,
}

impl Server {
    /// Discovery, with the same checks the sign-in worker makes: http(s)
    /// only, https unless the host is this machine, no credentials in the
    /// URL, and an API on the same origin.
    fn open(base: &str) -> Result<Self, SetupError> {
        let base = base.trim().trim_end_matches('/').to_string();
        let url = Url::parse(&base).map_err(|_| SetupError::Unreachable)?;
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(SetupError::Unreachable);
        }
        if !live_proto::bearer_allowed(&base) {
            return Err(SetupError::Insecure);
        }
        let agent = ureq::AgentBuilder::new()
            .timeout(TIMEOUT)
            .redirects(0)
            .build();
        let discovery: Discovery = read_json(
            agent.get(&format!("{base}/.well-known/h2live.json")),
            None,
            Action::Create,
        )
        .map_err(|_| SetupError::Unreachable)?;
        let api = Url::parse(&format!("{base}/"))
            .and_then(|d| d.join(&discovery.api))
            .map_err(|_| SetupError::Unreachable)?;
        if api.origin() != url.origin() || api.query().is_some() || api.fragment().is_some() {
            return Err(SetupError::Unreachable);
        }
        Ok(Server {
            base,
            api: api.as_str().trim_end_matches('/').into(),
            id: discovery.id,
            agent,
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.api)
    }
}

fn read_json<T: serde::de::DeserializeOwned>(
    req: ureq::Request,
    body: Option<(&str, &str)>,
    action: Action,
) -> Result<T, SetupError> {
    let reply = match body {
        Some((content_type, b)) => req.set("content-type", content_type).send_string(b),
        None => req.call(),
    };
    let reply = reply.map_err(|e| match e {
        ureq::Error::Status(status, r) => {
            let mut data = Vec::new();
            let _ = r.into_reader().take(4096).read_to_end(&mut data);
            let message = serde_json::from_slice::<ErrorBody>(&data)
                .map(|b| b.message)
                .unwrap_or_default();
            SetupError::from_reply(action, status, &message)
        }
        ureq::Error::Transport(_) => SetupError::Unreachable,
    })?;
    let mut data = Vec::new();
    reply
        .into_reader()
        .take(MAX_REPLY)
        .read_to_end(&mut data)
        .map_err(|_| SetupError::Unreachable)?;
    serde_json::from_slice(&data).map_err(|_| SetupError::Unreachable)
}

/// Whether the server at `server_url` needs an invite to create an
/// account (`GET /v1/server`, `invite_only`). `None` when it does not say
/// (a private server answers only signed-in callers): ask for an invite
/// when creation answers [`SetupError::NeedsInvite`].
pub fn needs_invite(server_url: &str) -> Result<Option<bool>, SetupError> {
    let server = Server::open(server_url)?;
    match read_json::<Value>(
        server.agent.get(&server.url("/server")),
        None,
        Action::Create,
    ) {
        Ok(v) => Ok(v.get("invite_only").and_then(Value::as_bool)),
        Err(SetupError::Unreachable) => Err(SetupError::Unreachable),
        Err(_) => Ok(None),
    }
}

/// Creates `gamertag` on the server with a new key; `invite` is sent as
/// typed (trimmed; empty is none). Returns the entry to [`save`] and the
/// recovery secret to show the player once.
pub fn create(
    server_url: &str,
    gamertag: &str,
    invite: Option<&str>,
) -> Result<(LocalEntry, Option<RecoveryInfo>), SetupError> {
    let server = Server::open(server_url)?;
    let key = KeyPair::generate();
    let invite = invite
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let jws = AccountRequest::sign(&key, gamertag.trim(), invite, &server.id, now());
    let created: CreatedAccount = read_json(
        server.agent.post(&server.url("/accounts")),
        Some(("application/jose", &jws)),
        Action::Create,
    )?;
    let recovery = created.recovery.clone();
    Ok((
        LocalEntry::new(&created, &server.base, &server.id, key),
        recovery,
    ))
}

/// Adds a new key on this machine to `gamertag`'s account with its current
/// recovery code (`123-456` or `123456`).
pub fn recover(server_url: &str, gamertag: &str, code: &str) -> Result<LocalEntry, SetupError> {
    let code = code.trim();
    if live_proto::recovery::parse_code(code).is_none() {
        return Err(SetupError::WrongCode);
    }
    let gamertag = gamertag.trim();
    if live_proto::Gamertag::new(gamertag).is_err() {
        return Err(SetupError::BadGamertag);
    }
    let server = Server::open(server_url)?;
    let key = KeyPair::generate();
    let body = RecoverBody {
        request: RecoverRequest::sign(&key, gamertag, &server.id, now()),
        code: code.into(),
    };
    let body = serde_json::to_string(&body).map_err(|_| SetupError::Unreachable)?;
    let path = format!("/accounts/{}/recover", path_segment(gamertag));
    let created: CreatedAccount = read_json(
        server.agent.post(&server.url(&path)),
        Some(("application/json", &body)),
        Action::Recover,
    )?;
    Ok(LocalEntry::new(&created, &server.base, &server.id, key))
}

/// The account's recovery secret and current code, to show on a signed-in
/// machine so another can recover. [`SetupError::NoRecovery`] when the
/// account never had one.
pub fn recovery_code(server_url: &str, entry: &LocalEntry) -> Result<RecoveryInfo, SetupError> {
    recovery_call(server_url, entry, "GET", lxm::RECOVERY_READ)
}

/// A new recovery secret: the old one stops working.
pub fn reset_recovery(server_url: &str, entry: &LocalEntry) -> Result<RecoveryInfo, SetupError> {
    recovery_call(server_url, entry, "POST", lxm::RECOVERY_WRITE)
}

fn recovery_call(
    server_url: &str,
    entry: &LocalEntry,
    method: &str,
    method_lxm: &str,
) -> Result<RecoveryInfo, SetupError> {
    let server = Server::open(server_url)?;
    let token = issue_token(
        &entry.key,
        &author_id(entry.xuid, &server.id),
        &server.id,
        method_lxm,
        now(),
    );
    let req = server
        .agent
        .request(
            method,
            &server.url(&format!("/accounts/by-xuid/{}/recovery", entry.xuid)),
        )
        .set("authorization", &format!("Bearer {token}"));
    let body = (method == "POST").then_some(("application/json", "{}"));
    read_json(req, body, Action::RecoveryCode)
}

/// A gamertag as one URL path segment (letters, digits and spaces are all
/// a gamertag holds; anything else is escaped too).
fn path_segment(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Adds `entry` to the accounts file at `path`, replacing an entry with
/// the same xuid and a stale one with the same gamertag on the same server.
/// Other entries are written back as they were, unknown fields included.
/// Writes mode 0600 via a temporary file and a rename.
pub fn save(path: &Path, entry: &LocalEntry) -> Result<(), SetupError> {
    let mut entries = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Vec<Map<String, Value>>>(&bytes)
            .map_err(|_| SetupError::Save)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return Err(SetupError::Save),
    };
    let tag = entry.gamertag.to_ascii_lowercase();
    entries.retain(|e| {
        let s = |k: &str| e.get(k).and_then(Value::as_str);
        let same_xuid = s("xuid")
            .and_then(|x| Xuid::parse(x).ok())
            .is_some_and(|x| x == entry.xuid);
        let same_tag = s("gamertag").is_some_and(|g| g.to_ascii_lowercase() == tag)
            && s("server_id").is_some_and(|id| id == entry.server_id);
        !same_xuid && !same_tag
    });
    let Value::Object(new) = entry.to_json() else {
        unreachable!("an entry is an object")
    };
    entries.push(new);
    let text = serde_json::to_string_pretty(&entries).map_err(|_| SetupError::Save)?;
    write_private(path, format!("{text}\n").as_bytes()).map_err(|_| SetupError::Save)
}

/// Every usable entry in the accounts file at `path` (see
/// [`LocalEntry::from_json`]); a missing file has none.
pub fn load(path: &Path) -> Result<Vec<LocalEntry>, SetupError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice::<Vec<Value>>(&bytes)
            .map_err(|_| SetupError::Save)?
            .iter()
            .filter_map(LocalEntry::from_json)
            .collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(_) => Err(SetupError::Save),
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
    if let Some(dir) = dir {
        std::fs::create_dir_all(dir)?;
    }
    let tmp: PathBuf = {
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(format!(".{}.tmp", std::process::id()));
        path.with_file_name(name)
    };
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let result = (|| {
        let mut f = options.open(&tmp)?;
        #[cfg(unix)]
        f.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The CLI's own accounts file beside the per-session file `h2 play --live`
/// hands the game (`live/play-accounts.json` → `live/accounts.json`), so an
/// account made in game is there next time. `None` for any other path.
pub fn shared_accounts_path(session_accounts: &Path) -> Option<PathBuf> {
    (session_accounts.file_name()? == "play-accounts.json")
        .then(|| session_accounts.with_file_name("accounts.json"))
}

/// One setup call on its own thread, polled from the game's frame loop.
pub struct Job<T> {
    result: Mutex<mpsc::Receiver<T>>,
}

impl<T: Send + 'static> Job<T> {
    pub fn spawn(work: impl FnOnce() -> T + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("h2-live-setup".into())
            .spawn(move || {
                let _ = tx.send(work());
            })
            .expect("spawning the setup thread");
        Job {
            result: Mutex::new(rx),
        }
    }

    /// The result once it is in; `None` while it runs.
    pub fn poll(&self) -> Option<T> {
        self.result
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .try_recv()
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusals_read_as_short_lines() {
        use Action::*;
        let r = SetupError::from_reply;
        assert_eq!(
            r(Create, 409, "gamertag is taken"),
            SetupError::GamertagTaken
        );
        assert_eq!(
            r(Create, 403, "this server needs an invite code"),
            SetupError::NeedsInvite
        );
        assert_eq!(
            r(Create, 403, "invite: no such code"),
            SetupError::InviteInvalid
        );
        assert_eq!(
            r(Create, 403, "invite already used"),
            SetupError::InviteInvalid
        );
        assert_eq!(
            r(Create, 400, "gamertag must be 1-15 characters"),
            SetupError::BadGamertag
        );
        assert_eq!(r(Recover, 403, "wrong code"), SetupError::WrongCode);
        assert_eq!(
            r(
                Recover,
                403,
                "that code was used already; wait for the next one"
            ),
            SetupError::WrongCode
        );
        assert_eq!(
            r(Recover, 403, "this account has no recovery set up"),
            SetupError::NoRecovery
        );
        assert_eq!(
            r(RecoveryCode, 404, "this account has no recovery set up"),
            SetupError::NoRecovery
        );
        assert_eq!(
            r(Recover, 404, "no such gamertag"),
            SetupError::NoSuchGamertag
        );
        assert_eq!(
            r(
                Recover,
                429,
                "too many wrong codes for this gamertag; try again in 61 s"
            ),
            SetupError::LockedOut { minutes: 2 }
        );
        assert_eq!(
            r(
                Recover,
                429,
                "too many wrong codes for this gamertag; try again in 1 s"
            ),
            SetupError::LockedOut { minutes: 1 }
        );
        assert_eq!(r(Create, 429, "too many new accounts"), SetupError::Busy);
        assert_eq!(r(Create, 500, ""), SetupError::Server(500));
        assert_eq!(
            SetupError::LockedOut { minutes: 3 }.to_string(),
            "Too many wrong codes. Try again in 3 min."
        );
    }

    #[test]
    fn plain_http_to_another_host_is_refused_before_any_request() {
        assert_eq!(
            create("http://example.invalid", "Pete", None).unwrap_err(),
            SetupError::Insecure
        );
        assert_eq!(
            recover("http://example.invalid", "Pete", "123456").unwrap_err(),
            SetupError::Insecure
        );
        assert_eq!(
            create("ftp://x", "Pete", None).unwrap_err(),
            SetupError::Unreachable
        );
        // Codes and gamertags are checked before the network.
        assert_eq!(
            recover("http://example.invalid", "Pete", "12345").unwrap_err(),
            SetupError::WrongCode
        );
    }

    #[test]
    fn session_files_map_to_the_shared_file() {
        assert_eq!(
            shared_accounts_path(Path::new("/x/live/play-accounts.json")),
            Some(PathBuf::from("/x/live/accounts.json"))
        );
        assert_eq!(
            shared_accounts_path(Path::new("/x/live/accounts.json")),
            None
        );
        assert_eq!(path_segment("Mr Chief"), "Mr%20Chief");
    }

    #[test]
    fn entries_never_debug_their_key() {
        let key = KeyPair::generate();
        let secret = key.secret_b64();
        let e = LocalEntry {
            xuid: Xuid(0x9000000000001),
            gamertag: "Pete".into(),
            id: author_id(Xuid(0x9000000000001), "live.test"),
            server: "http://127.0.0.1:1".into(),
            server_id: "live.test".into(),
            key: Arc::new(key),
        };
        assert!(!format!("{e:?}").contains(&secret));
        let back = LocalEntry::from_json(&e.to_json()).unwrap();
        assert_eq!(back.key.kid(), e.key.kid());
        assert_eq!(back.id, "acct:0009000000000001@live.test");
        let mut v = e.to_json();
        v["kid"] = "other".into();
        assert!(LocalEntry::from_json(&v).is_none());
    }
}
