//! Where an account is signed in, and signing the others out
//! (docs/live/live-api.md "Sign-ins", docs/live/live-security.md §3).
//!
//! A sign-in is one of the account's current keys. Every device makes its
//! own key (a game install at `h2 live account create` or with a sign-in
//! code, a browser at sign-up or with a code), and every request token is
//! signed by it, so signing a device out is revoking its key: the server
//! appends a `KeyRevoked` it signs itself, naming the key that asked
//! (`by`), and [`Live::authenticate`](crate::app::Live::authenticate)
//! refuses a revoked key's tokens from the next request on (it looks the
//! key up in the read model on every request). Nothing new is checked per
//! request and no loop runs.
//!
//! What a sign-in is (a browser or the game, and which) comes from the
//! User-Agent of the request that made the key, kept in the server's
//! `KeyAdded` as `device`, sealed like the sign-in code secret: the log is
//! public, a browser and OS per account need not be. Keys from before
//! that are named when they list their own sign-ins. When each was last
//! used is kept in memory only: it starts empty when the server does.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::Response;
use live_proto::recovery::seal;
use live_proto::sign::Token;
use live_proto::{Author, Xuid, lxm, streams, types};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::format_description::well_known::Rfc3339;

use crate::app::{ApiError, Live, conflict, forbidden, invalid, not_found};
use crate::dispatch::Subscriber;
use crate::store::RecordedEvent;

/// The label of the key a device label is sealed under.
pub const DEVICE_LABEL: &str = "h2live-device-seal-v1";
/// The longest device name kept, in characters.
const MAX_NAME: usize = 40;

/// What made a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Browser,
    /// The game (or the `h2` CLI, which keeps the game's accounts).
    Game,
    Other,
}

/// A device as a person would name it: "Firefox on Windows", "Halo 2".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub kind: Kind,
    pub name: String,
}

/// A device from a User-Agent; none without one.
pub fn describe(ua: Option<&str>) -> Option<Device> {
    let ua = ua.map(str::trim).filter(|u| !u.is_empty())?;
    if ua.starts_with("Mozilla/") {
        let browser = [
            ("Edg/", "Edge"),
            ("OPR/", "Opera"),
            ("SamsungBrowser/", "Samsung Internet"),
            ("Firefox/", "Firefox"),
            ("FxiOS/", "Firefox"),
            ("CriOS/", "Chrome"),
            ("Chrome/", "Chrome"),
            ("Safari/", "Safari"),
        ]
        .iter()
        .find(|(tag, _)| ua.contains(tag))
        .map_or("A browser", |(_, name)| name);
        let os = [
            ("iPhone", "iPhone"),
            ("iPad", "iPad"),
            ("Android", "Android"),
            ("CrOS", "ChromeOS"),
            ("Mac OS X", "macOS"),
            ("Macintosh", "macOS"),
            ("Windows", "Windows"),
            ("Linux", "Linux"),
        ]
        .iter()
        .find(|(tag, _)| ua.contains(tag))
        .map(|(_, name)| *name);
        return Some(Device {
            kind: Kind::Browser,
            name: match os {
                Some(os) => format!("{browser} on {os}"),
                None => browser.to_string(),
            },
        });
    }
    // The game and the CLI speak through ureq, or name themselves.
    let product = ua.split([' ', '/']).next().unwrap_or("");
    if matches!(product, "ureq" | "h2" | "h2recomp") {
        return Some(Device {
            kind: Kind::Game,
            name: "Halo 2".into(),
        });
    }
    let name: String = ua
        .split(' ')
        .next()
        .unwrap_or(ua)
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect();
    Some(Device {
        kind: Kind::Other,
        name,
    })
}

/// The device of a request, from its User-Agent.
pub fn device_of(headers: &HeaderMap) -> Option<Device> {
    describe(
        headers
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok()),
    )
}

/// One sign-in, as `GET .../signins` lists it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SignIn {
    pub kid: String,
    /// `browser`, `game`, `other`, or `unknown` for a key made before
    /// devices were named that hasn't listed its sign-ins since.
    pub kind: String,
    pub name: Option<String>,
    /// How the key came: `signup`, `code` (a sign-in code), `key` (added
    /// by another of the account's keys).
    pub via: String,
    /// When the key was added (RFC 3339).
    pub created_at: String,
    /// The last request it signed since the server started (RFC 3339).
    pub last_seen: Option<String>,
    /// The key that signed this request.
    pub current: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SignIns {
    pub items: Vec<SignIn>,
    pub position: u64,
}

/// The reply to a sign-out: the keys revoked.
#[derive(Debug, Serialize, Deserialize)]
pub struct SignedOut {
    pub revoked: Vec<String>,
    pub position: u64,
}

struct Entry {
    xuid: Xuid,
    created_at: String,
    via: &'static str,
    /// Sealed [`Device`] JSON.
    device: Option<String>,
}

/// The account keys' labels, from the log, and when each was last used.
pub struct Registry {
    key: [u8; 32],
    keys: RwLock<HashMap<String, Entry>>,
    seen: Mutex<HashMap<String, i64>>,
    /// Devices learnt from a key's own requests, for keys whose `KeyAdded`
    /// names none. Memory only.
    learnt: Mutex<HashMap<String, Device>>,
    /// Holds a sign-out's checks and its append together.
    pub(crate) writes: tokio::sync::Mutex<()>,
}

impl Registry {
    pub fn new(identity: &crate::identity::ServerIdentity) -> Self {
        Registry {
            key: identity.derive(DEVICE_LABEL),
            keys: RwLock::default(),
            seen: Mutex::default(),
            learnt: Mutex::default(),
            writes: Default::default(),
        }
    }

    /// `device` sealed, for a `KeyAdded`'s `device`.
    pub fn seal(&self, device: &Device) -> String {
        seal::seal(
            &self.key,
            &serde_json::to_vec(device).expect("a device serializes"),
        )
    }

    fn open(&self, sealed: &str) -> Option<Device> {
        serde_json::from_slice(&seal::open(&self.key, sealed)?).ok()
    }

    /// `kid` signed a request at `now`.
    pub fn seen(&self, kid: &str, now: i64) {
        let mut seen = self.seen.lock().unwrap();
        if let Some(t) = seen.get_mut(kid) {
            *t = now;
        } else if self.keys.read().unwrap().contains_key(kid) {
            seen.insert(kid.to_string(), now);
        }
    }

    /// Names `kid` from its own request when its `KeyAdded` didn't.
    pub fn learn(&self, kid: &str, device: Option<Device>) {
        let Some(device) = device else { return };
        let named = self
            .keys
            .read()
            .unwrap()
            .get(kid)
            .map(|e| e.device.is_some());
        if named == Some(false) {
            self.learnt.lock().unwrap().insert(kid.to_string(), device);
        }
    }

    /// Whether `kid` is a current key of `xuid`.
    pub fn is_current(&self, xuid: Xuid, kid: &str) -> bool {
        self.keys
            .read()
            .unwrap()
            .get(kid)
            .is_some_and(|e| e.xuid == xuid)
    }

    /// `xuid`'s current keys but `except`.
    pub fn others(&self, xuid: Xuid, except: &str) -> Vec<String> {
        let mut kids: Vec<String> = self
            .keys
            .read()
            .unwrap()
            .iter()
            .filter(|(kid, e)| e.xuid == xuid && kid.as_str() != except)
            .map(|(kid, _)| kid.clone())
            .collect();
        kids.sort();
        kids
    }

    /// `xuid`'s sign-ins, `current` first, then the most recently used.
    pub fn list(&self, xuid: Xuid, current: &str) -> Vec<SignIn> {
        let keys = self.keys.read().unwrap();
        let seen = self.seen.lock().unwrap();
        let learnt = self.learnt.lock().unwrap();
        let mut out: Vec<(Option<i64>, SignIn)> = keys
            .iter()
            .filter(|(_, e)| e.xuid == xuid)
            .map(|(kid, e)| {
                let device = e
                    .device
                    .as_deref()
                    .and_then(|s| self.open(s))
                    .or_else(|| learnt.get(kid).cloned());
                let last = seen.get(kid).copied();
                let sign_in = SignIn {
                    kid: kid.clone(),
                    kind: device.as_ref().map_or("unknown".into(), |d| {
                        serde_json::to_value(d.kind)
                            .ok()
                            .and_then(|v| v.as_str().map(String::from))
                            .unwrap_or_default()
                    }),
                    name: device.map(|d| d.name),
                    via: e.via.into(),
                    created_at: e.created_at.clone(),
                    last_seen: last.and_then(rfc3339),
                    current: kid == current,
                };
                (last, sign_in)
            })
            .collect();
        out.sort_by(|(la, a), (lb, b)| {
            b.current
                .cmp(&a.current)
                .then(lb.cmp(la))
                .then(b.created_at.cmp(&a.created_at))
                .then(a.kid.cmp(&b.kid))
        });
        out.into_iter().map(|(_, s)| s).collect()
    }
}

fn rfc3339(t: i64) -> Option<String> {
    time::OffsetDateTime::from_unix_timestamp(t)
        .ok()?
        .format(&Rfc3339)
        .ok()
}

impl Subscriber for Registry {
    fn name(&self) -> &str {
        "signins"
    }

    fn apply(&self, event: &RecordedEvent) {
        let b = &event.body;
        let Some(xuid) = b
            .stream
            .strip_prefix("account-")
            .and_then(|x| Xuid::parse(x).ok())
        else {
            return;
        };
        match b.kind.as_str() {
            types::KEY_ADDED => {
                let Some(kid) = key_id(&b.data) else { return };
                let by_server = matches!(Author::parse(&b.author), Some(Author::Server(_)));
                let via = match (by_server, b.data.get("via").and_then(Value::as_str)) {
                    (true, Some("totp")) => "code",
                    (true, _) => "signup",
                    (false, _) => "key",
                };
                let mut keys = self.keys.write().unwrap();
                keys.entry(kid).or_insert(Entry {
                    xuid,
                    created_at: b.at.clone(),
                    via,
                    device: b
                        .data
                        .get("device")
                        .and_then(Value::as_str)
                        .map(String::from),
                });
            }
            types::KEY_REVOKED => {
                let Some(kid) = b.data.get("kid").and_then(Value::as_str) else {
                    return;
                };
                let mut keys = self.keys.write().unwrap();
                if keys.get(kid).is_some_and(|e| e.xuid == xuid) {
                    keys.remove(kid);
                    self.seen.lock().unwrap().remove(kid);
                    self.learnt.lock().unwrap().remove(kid);
                }
            }
            _ => {}
        }
    }
}

/// A `KeyAdded`'s kid: as given, else the thumbprint of its `jwk`.
fn key_id(data: &Value) -> Option<String> {
    let jwk = serde_json::from_value(data.get("jwk")?.clone()).ok()?;
    live_proto::sign::PublicKey::from_jwk(&jwk)
        .ok()
        .map(|k| k.kid().to_string())
}

/// The caller, who must be the account `xuid` itself; its key's kid.
fn own(live: &Live, bearer: Option<&str>, method: &str, xuid: Xuid) -> Result<String, ApiError> {
    match live.authenticate(bearer, method)? {
        Author::Account { xuid: me, .. } if me == xuid => {}
        _ => return Err(forbidden("only the account itself")),
    }
    // authenticate parsed it already.
    Token::parse(bearer.unwrap_or(""))
        .ok()
        .and_then(|t| t.jws.kid().map(String::from))
        .ok_or_else(|| invalid("token has no kid"))
}

/// `GET /v1/accounts/by-xuid/{xuid}/signins`.
pub fn list(
    live: &Live,
    bearer: Option<&str>,
    xuid: Xuid,
    device: Option<Device>,
) -> Result<SignIns, ApiError> {
    let kid = own(live, bearer, lxm::SIGNINS_READ, xuid)?;
    live.signins.learn(&kid, device);
    Ok(SignIns {
        items: live.signins.list(xuid, &kid),
        position: live.position(),
    })
}

/// Which sign-ins to end.
pub enum Target {
    One(String),
    Others,
}

/// Revokes `target`'s keys with server-signed `KeyRevoked`s, appended
/// together; answers once they are applied, so the next request with one
/// of them is refused.
pub async fn sign_out(
    live: &Live,
    bearer: Option<&str>,
    xuid: Xuid,
    target: Target,
) -> Result<SignedOut, ApiError> {
    let me = own(live, bearer, lxm::SIGNINS_WRITE, xuid)?;
    let author = Author::Account {
        xuid,
        server: live.identity.id.clone(),
    };
    live.rate_limit(&author, lxm::SIGNINS_WRITE)?;
    let _write = live.signins.writes.lock().await;
    let kids = match target {
        Target::One(kid) if kid == me => {
            return Err(conflict(
                "that is the sign-in making this request; sign out on this device instead",
            ));
        }
        Target::One(kid) if live.signins.is_current(xuid, &kid) => vec![kid],
        Target::One(_) => return Err(not_found("no such sign-in on this account")),
        Target::Others => live.signins.others(xuid, &me),
    };
    if kids.is_empty() {
        return Ok(SignedOut {
            revoked: kids,
            position: live.position(),
        });
    }
    let events = kids
        .iter()
        .map(|kid| {
            live.server_event(
                streams::account(xuid),
                types::KEY_REVOKED,
                json!({"kid": kid, "by": me, "via": "signout"}),
            )
        })
        .collect();
    let recorded = live.store.append(events, &[]).await?;
    let last = recorded.last().map_or(0, |e| e.position);
    live.wait_applied(last).await?;
    Ok(SignedOut {
        revoked: kids,
        position: last,
    })
}

// HTTP handlers.

type Shared = State<Arc<Live>>;

fn xuid_of(raw: &str) -> Result<Xuid, ApiError> {
    Xuid::parse(raw).map_err(|_| invalid("not a XUID"))
}

pub async fn get_signins(
    State(live): Shared,
    headers: HeaderMap,
    Path(xuid): Path<String>,
) -> Result<Response, ApiError> {
    let out = list(
        &live,
        crate::http::bearer(&headers),
        xuid_of(&xuid)?,
        device_of(&headers),
    )?;
    Ok(crate::http::json(StatusCode::OK, &out))
}

pub async fn delete_signin(
    State(live): Shared,
    headers: HeaderMap,
    Path((xuid, kid)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let out = sign_out(
        &live,
        crate::http::bearer(&headers),
        xuid_of(&xuid)?,
        Target::One(kid),
    )
    .await?;
    Ok(crate::http::json(StatusCode::OK, &out))
}

pub async fn post_sign_out_others(
    State(live): Shared,
    headers: HeaderMap,
    Path(xuid): Path<String>,
) -> Result<Response, ApiError> {
    let out = sign_out(
        &live,
        crate::http::bearer(&headers),
        xuid_of(&xuid)?,
        Target::Others,
    )
    .await?;
    Ok(crate::http::json(StatusCode::OK, &out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agents_name_devices() {
        let d = |ua: &str| describe(Some(ua)).unwrap();
        let mac_chrome = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
        assert_eq!(
            d(mac_chrome),
            Device {
                kind: Kind::Browser,
                name: "Chrome on macOS".into()
            }
        );
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 19_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/19.0 Mobile/15E148 Safari/604.1";
        assert_eq!(d(iphone).name, "Safari on iPhone");
        let edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36 Edg/140.0";
        assert_eq!(d(edge).name, "Edge on Windows");
        let firefox = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";
        assert_eq!(d(firefox).name, "Firefox on Linux");
        assert_eq!(d("ureq/2.12.1").kind, Kind::Game);
        assert_eq!(d("h2/0.1 (macOS)").name, "Halo 2");
        assert_eq!(
            d("curl/8.7.1"),
            Device {
                kind: Kind::Other,
                name: "curl/8.7.1".into()
            }
        );
        assert_eq!(describe(None), None);
        assert_eq!(describe(Some("  ")), None);
        assert!(d(&"x".repeat(500)).name.chars().count() <= MAX_NAME);
    }
}
