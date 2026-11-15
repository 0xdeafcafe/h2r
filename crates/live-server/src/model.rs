//! The in-memory read model: every projection the HTTP API and the
//! authenticator read, built by one [`Subscriber`] from the whole log at
//! startup and kept current by the dispatcher.
//!
//! It trusts the log: the server validated every event on the way in, so
//! an event whose data does not parse is skipped with a log line.

use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;

use live_proto::sign::{Jwk, MAX_EVENT_SKEW_S, PublicKey, parse_time};
use live_proto::{AccountKey, Gamertag, PublishedFile, TitleId, Xuid, types};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::dispatch::Subscriber;
use crate::guard::ReplayGuard;
use crate::store::RecordedEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub xuid: Xuid,
    pub gamertag: Gamertag,
    pub created_at: String,
    pub position: u64,
}

#[derive(Debug, Clone)]
pub struct Key {
    pub xuid: Xuid,
    pub key: PublicKey,
    pub from: u64,
    pub to: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub meta: PublishedFile,
    pub position: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Presence {
    pub state: String,
    pub game_id: Option<String>,
    pub since: String,
}

#[derive(Default)]
pub struct State {
    pub accounts: HashMap<Xuid, Account>,
    by_tag: HashMap<String, Xuid>,
    /// Creation position -> account, for paging in creation order.
    by_position: BTreeMap<u64, Xuid>,
    pub keys: HashMap<String, Key>,
    pub files: HashMap<(TitleId, String), File>,
    pub presence: HashMap<Xuid, Presence>,
}

impl State {
    pub fn account_by_tag(&self, tag: &Gamertag) -> Option<&Account> {
        self.by_tag
            .get(&tag.key())
            .and_then(|x| self.accounts.get(x))
    }

    /// Accounts created after `after`, in creation order.
    pub fn accounts_after(&self, after: u64) -> impl Iterator<Item = &Account> {
        self.by_position
            .range(after + 1..)
            .filter_map(|(_, x)| self.accounts.get(x))
    }

    /// An account's keys, oldest first.
    pub fn keys_of(&self, xuid: Xuid) -> Vec<AccountKey> {
        let mut keys: Vec<_> = self
            .keys
            .values()
            .filter(|k| k.xuid == xuid)
            .map(|k| AccountKey {
                kid: k.key.kid().into(),
                jwk: k.key.jwk(),
                valid_from: k.from,
                valid_to: k.to,
            })
            .collect();
        keys.sort_by_key(|k| k.valid_from);
        keys
    }

    /// The current key `kid` of account `xuid`.
    pub fn active_key(&self, xuid: Xuid, kid: &str) -> Option<&PublicKey> {
        self.keys
            .get(kid)
            .filter(|k| k.xuid == xuid && k.to.is_none())
            .map(|k| &k.key)
    }

    pub fn active_key_count(&self, xuid: Xuid) -> usize {
        self.keys
            .values()
            .filter(|k| k.xuid == xuid && k.to.is_none())
            .count()
    }
}

/// The read model plus the replay sets the log seeds: event nonces still
/// inside the `at` window, and used invites.
#[derive(Default)]
pub struct Model {
    state: RwLock<State>,
    pub nonces: ReplayGuard,
}

#[derive(Deserialize)]
struct AccountCreated {
    xuid: Xuid,
    gamertag: Gamertag,
    #[serde(default)]
    invite: Option<String>,
}

#[derive(Deserialize)]
struct KeyAdded {
    jwk: Jwk,
}

#[derive(Deserialize)]
struct KeyRevoked {
    kid: String,
}

#[derive(Deserialize)]
struct PresenceData {
    xuid: Xuid,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    game_id: Option<String>,
}

fn data<T: DeserializeOwned>(e: &RecordedEvent) -> Option<T> {
    match serde_json::from_value(e.body.data.clone()) {
        Ok(d) => Some(d),
        Err(err) => {
            eprintln!("live: skipping {} at {}: {err}", e.body.kind, e.position);
            None
        }
    }
}

/// The xuid of an `account-<xuid>` stream.
fn account_of(stream: &str) -> Option<Xuid> {
    Xuid::parse(stream.strip_prefix("account-")?).ok()
}

pub fn nonce_key(author: &str, nonce: &str) -> String {
    format!("nonce {author} {nonce}")
}

pub fn invite_key(jti: &str) -> String {
    format!("invite {jti}")
}

impl Model {
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap()
    }

    fn apply_event(&self, e: &RecordedEvent) {
        let b = &e.body;
        if let Some(at) = parse_time(&b.at) {
            let at = at.unix_timestamp();
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            self.nonces
                .insert_new(nonce_key(&b.author, &b.nonce), at + MAX_EVENT_SKEW_S, now);
        }
        let mut s = self.state.write().unwrap();
        match b.kind.as_str() {
            types::ACCOUNT_CREATED => {
                let Some(d) = data::<AccountCreated>(e) else {
                    return;
                };
                if let Some(jti) = d.invite {
                    self.nonces.insert_new(invite_key(&jti), i64::MAX, 0);
                }
                s.by_tag.insert(d.gamertag.key(), d.xuid);
                s.by_position.insert(e.position, d.xuid);
                let account = Account {
                    xuid: d.xuid,
                    gamertag: d.gamertag,
                    created_at: b.at.clone(),
                    position: e.position,
                };
                s.accounts.insert(d.xuid, account);
            }
            types::KEY_ADDED => {
                let (Some(xuid), Some(d)) = (account_of(&b.stream), data::<KeyAdded>(e)) else {
                    return;
                };
                let Ok(key) = PublicKey::from_jwk(&d.jwk) else {
                    return;
                };
                s.keys.entry(key.kid().to_string()).or_insert(Key {
                    xuid,
                    key,
                    from: e.position,
                    to: None,
                });
            }
            types::KEY_REVOKED => {
                let (Some(xuid), Some(d)) = (account_of(&b.stream), data::<KeyRevoked>(e)) else {
                    return;
                };
                if let Some(k) = s
                    .keys
                    .get_mut(&d.kid)
                    .filter(|k| k.xuid == xuid && k.to.is_none())
                {
                    k.to = Some(e.position);
                }
            }
            types::TITLE_FILE_PUBLISHED => {
                let Some(meta) = data::<PublishedFile>(e) else {
                    return;
                };
                s.files.insert(
                    (meta.title_id, meta.name.clone()),
                    File {
                        meta,
                        position: e.position,
                    },
                );
            }
            types::SIGNED_IN | types::PRESENCE_CHANGED => {
                let Some(d) = data::<PresenceData>(e) else {
                    return;
                };
                let since = match s.presence.get(&d.xuid) {
                    Some(p) => p.since.clone(),
                    None => b.at.clone(),
                };
                let state = d.state.unwrap_or_else(|| "online".into());
                s.presence.insert(
                    d.xuid,
                    Presence {
                        state,
                        game_id: d.game_id,
                        since,
                    },
                );
            }
            types::SIGNED_OUT | types::PRESENCE_EXPIRED => {
                let Some(d) = data::<PresenceData>(e) else {
                    return;
                };
                s.presence.remove(&d.xuid);
            }
            _ => {}
        }
    }
}

impl Subscriber for Model {
    fn name(&self) -> &str {
        "read-model"
    }

    fn apply(&self, event: &RecordedEvent) {
        self.apply_event(event);
    }
}

#[cfg(test)]
mod tests {
    use live_proto::sign::{EventBody, KeyPair};
    use serde_json::{Value, json};

    use super::*;

    struct Log {
        key: KeyPair,
        position: u64,
        model: Model,
    }

    impl Log {
        fn new() -> Self {
            Log {
                key: KeyPair::generate(),
                position: 0,
                model: Model::default(),
            }
        }
        fn push(&mut self, stream: &str, kind: &str, data: Value) -> u64 {
            self.position += 1;
            let body = EventBody::new(
                &self.key,
                "server:h",
                stream,
                kind,
                data,
                time::OffsetDateTime::now_utc(),
            );
            let e = RecordedEvent {
                position: self.position,
                version: 0,
                hash: String::new(),
                jws: body.sign(&self.key),
                jws_index: None,
                body,
            };
            self.model.apply(&e);
            self.position
        }
    }

    const X: &str = "0009000000000001";

    #[test]
    fn accounts_keys_files_presence() {
        let mut log = Log::new();
        let k1 = KeyPair::generate();
        let k2 = KeyPair::generate();
        let created = log.push(
            &format!("account-{X}"),
            "AccountCreated",
            json!({"xuid": X, "gamertag": "NoIcE", "invite": "inv1"}),
        );
        let added = log.push(
            &format!("account-{X}"),
            "KeyAdded",
            json!({"kid": k1.kid(), "jwk": k1.public().jwk()}),
        );
        let added2 = log.push(
            &format!("account-{X}"),
            "KeyAdded",
            json!({"kid": k2.kid(), "jwk": k2.public().jwk()}),
        );
        let revoked = log.push(
            &format!("account-{X}"),
            "KeyRevoked",
            json!({"kid": k1.kid()}),
        );
        // A revocation on another account's stream does nothing.
        log.push(
            "account-0009000000000002",
            "KeyRevoked",
            json!({"kid": k2.kid()}),
        );
        // Malformed data is skipped.
        log.push(&format!("account-{X}"), "KeyAdded", json!({"jwk": 7}));

        let xuid = Xuid::parse(X).unwrap();
        {
            let s = log.model.read();
            let a = s.account_by_tag(&Gamertag::new("noice").unwrap()).unwrap();
            assert_eq!(
                (a.xuid, a.position, a.gamertag.as_str()),
                (xuid, created, "NoIcE")
            );
            let keys = s.keys_of(xuid);
            assert_eq!(keys.len(), 2);
            assert_eq!(
                (keys[0].valid_from, keys[0].valid_to),
                (added, Some(revoked))
            );
            assert_eq!((keys[1].valid_from, keys[1].valid_to), (added2, None));
            assert!(s.active_key(xuid, k1.kid()).is_none());
            assert!(s.active_key(xuid, k2.kid()).is_some());
            assert!(s.active_key(Xuid(5), k2.kid()).is_none());
            assert_eq!(s.accounts_after(0).count(), 1);
            assert_eq!(s.accounts_after(created).count(), 0);
        }
        assert!(log.model.nonces.contains(&invite_key("inv1"), 0));

        let t = "4D530064";
        log.push(
            &format!("title-{t}-file-motd"),
            "TitleFilePublished",
            json!({"title_id": t, "name": "motd", "sha256": "aa", "size": 1}),
        );
        log.push(
            &format!("title-{t}-file-motd"),
            "TitleFilePublished",
            json!({"title_id": t, "name": "motd", "sha256": "bb", "size": 2}),
        );
        assert_eq!(
            log.model.read().files[&(live_proto::HALO_2_TITLE_ID, "motd".into())]
                .meta
                .sha256,
            "bb"
        );

        log.push(
            &format!("presence-{X}"),
            "SignedIn",
            json!({"xuid": X, "state": "online"}),
        );
        log.push(
            &format!("presence-{X}"),
            "PresenceChanged",
            json!({"xuid": X, "state": "playing", "game_id": "g"}),
        );
        {
            let s = log.model.read();
            let p = &s.presence[&xuid];
            assert_eq!(
                (p.state.as_str(), p.game_id.as_deref()),
                ("playing", Some("g"))
            );
        }
        log.push(&format!("presence-{X}"), "SignedOut", json!({"xuid": X}));
        assert!(log.model.read().presence.is_empty());
    }
}
