//! The server's side: its storage endpoints (docs/live/live-api.md "Cloud
//! saves"), behind a trait so the engine is tested against a fake.

use std::time::Duration;

use live_proto::sign::{KeyPair, issue_token};
use live_proto::storage::{
    IF_MATCH, MODIFIED_HEADER, SHA256_HEADER, StorageList, VersionConflict, encode_name,
};
use live_proto::{Author, Xuid, lxm};

use crate::{Key, Sha256, StorageItem};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteError {
    /// Not reached, or busy (a transport error, 429 or 5xx): try later.
    Offline(String),
    /// The write named a version that is no longer current: the item as
    /// it is now (`None`: it never existed).
    Conflict(Option<Box<StorageItem>>),
    /// The server has no cloud saves (an older one).
    Unsupported,
    /// Refused for good: too large, over the quota, a bad name.
    Refused(u16, String),
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::Offline(why) => write!(f, "offline ({why})"),
            RemoteError::Conflict(Some(i)) => write!(f, "changed elsewhere (now version {})", i.version),
            RemoteError::Conflict(None) => write!(f, "deleted elsewhere"),
            RemoteError::Unsupported => write!(f, "this server keeps no cloud saves"),
            RemoteError::Refused(code, why) => write!(f, "refused ({code}: {why})"),
        }
    }
}

pub type RemoteResult<T> = Result<T, RemoteError>;

/// One account's storage on one server.
pub trait Remote: Send {
    fn list(&self) -> RemoteResult<Vec<StorageItem>>;
    fn get(&self, key: &Key) -> RemoteResult<Vec<u8>>;
    fn put(&self, key: &Key, bytes: &[u8], if_match: u64, modified: i64) -> RemoteResult<StorageItem>;
    fn delete(&self, key: &Key, if_match: u64) -> RemoteResult<StorageItem>;
}

/// [`Remote`] over HTTP, with a fresh signed token per request.
pub struct HttpRemote {
    /// The API base (`https://host/v1`).
    api: String,
    server_id: String,
    iss: String,
    xuid: Xuid,
    key: KeyPair,
    agent: ureq::Agent,
}

/// How long one request may take. Saves are small; a slow server must not
/// hold up the start of the game for long.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

impl HttpRemote {
    /// `api` is the discovery document's API base. Refuses to send tokens
    /// over plain HTTP to anything but loopback (docs/live/live-security.md §6).
    pub fn new(api: &str, server_id: &str, xuid: Xuid, key: KeyPair) -> Result<Self, String> {
        let api = api.trim_end_matches('/').to_string();
        if !live_proto::bearer_allowed(&api) {
            return Err(format!("refusing to send tokens over plain HTTP to {api}"));
        }
        Ok(HttpRemote {
            iss: Author::Account {
                xuid,
                server: server_id.into(),
            }
            .to_string(),
            api,
            server_id: server_id.into(),
            xuid,
            key,
            agent: ureq::AgentBuilder::new()
                .timeout_connect(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build(),
        })
    }

    fn bearer(&self, method: &str) -> String {
        let now = crate::now_ms() / 1000;
        format!(
            "Bearer {}",
            issue_token(&self.key, &self.iss, &self.server_id, method, now)
        )
    }

    fn list_url(&self) -> String {
        format!("{}/accounts/{}/storage", self.api, self.xuid)
    }

    fn item_url(&self, key: &Key) -> String {
        format!("{}/{}/{}", self.list_url(), key.kind, encode_name(&key.name))
    }
}

/// A failed request as a [`RemoteError`].
fn error(e: ureq::Error) -> RemoteError {
    match e {
        ureq::Error::Transport(t) => RemoteError::Offline(t.to_string()),
        ureq::Error::Status(code, resp) => {
            let text = resp.into_string().unwrap_or_default();
            match code {
                409 => match serde_json::from_str::<VersionConflict>(&text) {
                    Ok(c) => RemoteError::Conflict(c.current.map(Box::new)),
                    Err(_) => RemoteError::Refused(code, text),
                },
                429 | 500.. => RemoteError::Offline(format!("HTTP {code}")),
                _ => {
                    let message = serde_json::from_str::<serde_json::Value>(&text)
                        .ok()
                        .and_then(|v| v["message"].as_str().map(str::to_string))
                        .unwrap_or(text);
                    RemoteError::Refused(code, message)
                }
            }
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(resp: ureq::Response) -> RemoteResult<T> {
    let text = resp
        .into_string()
        .map_err(|e| RemoteError::Offline(e.to_string()))?;
    serde_json::from_str(&text).map_err(|e| RemoteError::Refused(200, format!("an unexpected reply: {e}")))
}

impl Remote for HttpRemote {
    fn list(&self) -> RemoteResult<Vec<StorageItem>> {
        let req = self
            .agent
            .get(&self.list_url())
            .set("authorization", &self.bearer(lxm::STORAGE_READ));
        match req.call() {
            Ok(resp) => parse::<StorageList>(resp).map(|l| l.items),
            Err(ureq::Error::Status(404, _)) => Err(RemoteError::Unsupported),
            Err(e) => Err(error(e)),
        }
    }

    fn get(&self, key: &Key) -> RemoteResult<Vec<u8>> {
        let resp = self
            .agent
            .get(&self.item_url(key))
            .set("authorization", &self.bearer(lxm::STORAGE_READ))
            .call()
            .map_err(error)?;
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes)
            .map_err(|e| RemoteError::Offline(e.to_string()))?;
        Ok(bytes)
    }

    fn put(&self, key: &Key, bytes: &[u8], if_match: u64, modified: i64) -> RemoteResult<StorageItem> {
        let resp = self
            .agent
            .put(&self.item_url(key))
            .set("authorization", &self.bearer(lxm::STORAGE_WRITE))
            .set(IF_MATCH, &if_match.to_string())
            .set(SHA256_HEADER, &Sha256::of(bytes).to_string())
            .set(MODIFIED_HEADER, &modified.max(1).to_string())
            .set("content-type", "application/octet-stream")
            .send_bytes(bytes)
            .map_err(error)?;
        parse(resp)
    }

    fn delete(&self, key: &Key, if_match: u64) -> RemoteResult<StorageItem> {
        let resp = self
            .agent
            .delete(&self.item_url(key))
            .set("authorization", &self.bearer(lxm::STORAGE_WRITE))
            .set(IF_MATCH, &if_match.to_string())
            .set(MODIFIED_HEADER, &crate::now_ms().to_string())
            .call()
            .map_err(error)?;
        parse(resp)
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A server in memory, with the real one's version rules, that can be
    //! switched off.
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Default)]
    pub struct Inner {
        pub items: BTreeMap<Key, (StorageItem, Vec<u8>)>,
        pub offline: bool,
        /// Every write, in order: (key, version written).
        pub writes: Vec<(Key, u64)>,
    }

    #[derive(Clone, Default)]
    pub struct FakeRemote(pub Arc<Mutex<Inner>>);

    impl FakeRemote {
        pub fn set_offline(&self, offline: bool) {
            self.0.lock().unwrap().offline = offline;
        }

        /// Another machine's write.
        pub fn seed(&self, name: &str, bytes: &[u8], modified: i64) -> StorageItem {
            let key = Key::save(name);
            let version = self.0.lock().unwrap().items.get(&key).map_or(0, |(i, _)| i.version);
            let was = self.0.lock().unwrap().offline;
            self.set_offline(false);
            let item = self.put(&key, bytes, version, modified).unwrap();
            self.set_offline(was);
            self.0.lock().unwrap().writes.pop();
            item
        }

        pub fn bytes(&self, name: &str) -> Option<Vec<u8>> {
            self.0
                .lock()
                .unwrap()
                .items
                .get(&Key::save(name))
                .filter(|(i, _)| !i.deleted)
                .map(|(_, b)| b.clone())
        }

        pub fn writes(&self) -> Vec<(Key, u64)> {
            self.0.lock().unwrap().writes.clone()
        }

        fn up(&self) -> RemoteResult<std::sync::MutexGuard<'_, Inner>> {
            let g = self.0.lock().unwrap();
            if g.offline {
                return Err(RemoteError::Offline("switched off".into()));
            }
            Ok(g)
        }
    }

    impl Remote for FakeRemote {
        fn list(&self) -> RemoteResult<Vec<StorageItem>> {
            Ok(self.up()?.items.values().map(|(i, _)| i.clone()).collect())
        }

        fn get(&self, key: &Key) -> RemoteResult<Vec<u8>> {
            self.up()?
                .items
                .get(key)
                .filter(|(i, _)| !i.deleted)
                .map(|(_, b)| b.clone())
                .ok_or(RemoteError::Refused(404, "no such item".into()))
        }

        fn put(&self, key: &Key, bytes: &[u8], if_match: u64, modified: i64) -> RemoteResult<StorageItem> {
            let mut g = self.up()?;
            let current = g.items.get(key).map(|(i, _)| i.clone());
            if current.as_ref().map_or(0, |i| i.version) != if_match {
                return Err(RemoteError::Conflict(current.map(Box::new)));
            }
            let item = StorageItem {
                kind: key.kind.clone(),
                name: key.name.clone(),
                version: if_match + 1,
                sha256: Some(Sha256::of(bytes)),
                size: bytes.len() as u64,
                modified,
                stored_at: String::new(),
                deleted: false,
            };
            g.items.insert(key.clone(), (item.clone(), bytes.to_vec()));
            g.writes.push((key.clone(), item.version));
            Ok(item)
        }

        fn delete(&self, key: &Key, if_match: u64) -> RemoteResult<StorageItem> {
            let mut g = self.up()?;
            let Some((current, _)) = g.items.get(key).cloned().filter(|(i, _)| !i.deleted) else {
                return Err(RemoteError::Refused(404, "no such item".into()));
            };
            if current.version != if_match {
                return Err(RemoteError::Conflict(Some(Box::new(current))));
            }
            let tomb = StorageItem {
                version: if_match + 1,
                sha256: None,
                size: 0,
                deleted: true,
                ..current
            };
            g.items.insert(key.clone(), (tomb.clone(), Vec::new()));
            g.writes.push((key.clone(), tomb.version));
            Ok(tomb)
        }
    }
}
