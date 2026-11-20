//! A server's log reached over HTTPS, through its operator endpoints
//! ([`crate::logio`]), as an [`EventStore`] that [`crate::copy::copy_log`]
//! reads from and imports into: `h2 live store copy --to https://…` (into
//! the Worker's hub) and `--from https://…` (back out, for a rollback).
//!
//! Every request carries a fresh operator token signed with the server's
//! key, so this needs that server's identity (`server.key` and
//! `server.json`, [`ServerIdentity::load`]); [`HttpStore::connect`] checks
//! the server's discovery document names that id and key before anything
//! else. Only what a copy uses is here: reading by position, the head,
//! import, title-file blobs and checkpoints. Appending is refused.

use std::time::Duration;

use async_trait::async_trait;
use live_proto::sign::issue_token;
use live_proto::{Discovery, lxm};

use crate::identity::ServerIdentity;
use crate::logio::{Checkpoint, LogMeta, LogPage, PAGE_MAX};
use crate::store::{EventStore, NewEvent, Query, RecordedEvent, StoreError, Wake};

/// Reads are retried this many times on a network error or a 5xx (a
/// Worker's hub restarting); an import is not (it is checked against the
/// head, so running the copy again carries on).
const READ_TRIES: u32 = 4;

pub struct HttpStore {
    base: String,
    identity: ServerIdentity,
    client: reqwest::Client,
}

fn backend(e: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(e.to_string())
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// The error a server sent, as text.
async fn failure(resp: reqwest::Response) -> String {
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let message = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_string))
        .unwrap_or(body);
    format!("{status}: {message}")
}

impl HttpStore {
    /// `base` is the server's URL (`https://host`, or `http://` to a
    /// loopback address); `identity` the operator's (that server's) id and
    /// key.
    pub async fn connect(base: &str, identity: ServerIdentity) -> anyhow::Result<Self> {
        let base = base.trim_end_matches('/').to_string();
        if !live_proto::bearer_allowed(&base) {
            anyhow::bail!("{base}: the server key's tokens go over https:// only (or to loopback)");
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?;
        let url = format!("{base}/.well-known/h2live.json");
        let resp = client.get(&url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("{url}: {}", failure(resp).await);
        }
        let discovery: Discovery = serde_json::from_slice(&resp.bytes().await?)
            .map_err(|e| anyhow::anyhow!("{url}: not a discovery document ({e})"))?;
        let kid = identity.key.kid();
        let has_key = discovery.keys.iter().any(|k| k.kid.as_deref() == Some(kid));
        if discovery.id != identity.id || !has_key {
            anyhow::bail!(
                "{base} is server {:?}{}; the operator key given is server {:?}'s",
                discovery.id,
                if has_key { "" } else { " with another key" },
                identity.id
            );
        }
        Ok(HttpStore {
            base,
            identity,
            client,
        })
    }

    fn token(&self, method: &str) -> String {
        issue_token(
            &self.identity.key,
            &self.identity.author(),
            &self.identity.id,
            method,
            now(),
        )
    }

    /// A GET with retries; `None` on a 404.
    async fn get(&self, path: &str) -> Result<Option<bytes::Bytes>, StoreError> {
        let url = format!("{}{path}", self.base);
        let mut last = String::new();
        for attempt in 0..READ_TRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500 << attempt)).await;
            }
            let sent = self
                .client
                .get(&url)
                .bearer_auth(self.token(lxm::LOG_READ))
                .send()
                .await;
            match sent {
                Ok(r) if r.status().is_success() => {
                    return r.bytes().await.map(Some).map_err(backend);
                }
                Ok(r) if r.status() == reqwest::StatusCode::NOT_FOUND => return Ok(None),
                Ok(r) if r.status().is_server_error() => last = failure(r).await,
                Ok(r) => return Err(backend(format!("GET {url}: {}", failure(r).await))),
                Err(e) => last = e.to_string(),
            }
        }
        Err(backend(format!("GET {url}: {last}")))
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, StoreError> {
        let bytes = self
            .get(path)
            .await?
            .ok_or_else(|| backend(format!("{}{path}: 404", self.base)))?;
        serde_json::from_slice(&bytes).map_err(|e| backend(format!("{}{path}: {e}", self.base)))
    }

    async fn put(&self, path: &str, body: Vec<u8>) -> Result<(), StoreError> {
        let url = format!("{}{path}", self.base);
        let r = self
            .client
            .put(&url)
            .bearer_auth(self.token(lxm::LOG_WRITE))
            .body(body)
            .send()
            .await
            .map_err(backend)?;
        if !r.status().is_success() {
            return Err(backend(format!("PUT {url}: {}", failure(r).await)));
        }
        Ok(())
    }

    /// `POST /v1/admin/log/release`: the server's own writes start again.
    pub async fn release(&self) -> Result<u64, StoreError> {
        let url = format!("{}/v1/admin/log/release", self.base);
        let r = self
            .client
            .post(&url)
            .bearer_auth(self.token(lxm::LOG_WRITE))
            .send()
            .await
            .map_err(backend)?;
        if !r.status().is_success() {
            return Err(backend(format!("POST {url}: {}", failure(r).await)));
        }
        let done: crate::logio::Imported =
            serde_json::from_slice(&r.bytes().await.map_err(backend)?).map_err(backend)?;
        Ok(done.head)
    }

    /// Whether the server holds its own writes ([`crate::hold`]).
    pub async fn held(&self) -> Result<bool, StoreError> {
        Ok(self.meta().await?.held)
    }

    async fn meta(&self) -> Result<LogMeta, StoreError> {
        self.get_json("/v1/admin/log/meta").await
    }
}

#[async_trait]
impl EventStore for HttpStore {
    async fn append(
        &self,
        _events: Vec<NewEvent>,
        _expect: &[(String, u64)],
    ) -> Result<Vec<RecordedEvent>, StoreError> {
        Err(backend(
            "a server over HTTPS is appended to through its API",
        ))
    }

    /// By position only, in pages of at most [`PAGE_MAX`].
    async fn read(&self, query: &Query) -> Result<Vec<RecordedEvent>, StoreError> {
        if query.stream.is_some() || !query.types.is_empty() || !query.exclude.is_empty() {
            return Err(backend("a server over HTTPS is read by position only"));
        }
        let mut out = Vec::new();
        let mut after = query.after;
        while out.len() < query.limit {
            let limit = (query.limit - out.len()).min(PAGE_MAX);
            let page: LogPage = self
                .get_json(&format!("/v1/admin/log?after={after}&limit={limit}"))
                .await?;
            let events = page.into_events().map_err(backend)?;
            let Some(last) = events.last().map(|e| e.position) else {
                break;
            };
            after = last;
            let full = events.len() == limit;
            out.extend(events);
            if !full {
                break;
            }
        }
        Ok(out)
    }

    async fn head(&self) -> Result<u64, StoreError> {
        Ok(self.meta().await?.head)
    }

    async fn put_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError> {
        self.put(&format!("/v1/admin/log/blobs/{sha256}"), bytes.to_vec())
            .await
    }

    async fn blob(&self, sha256: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .get(&format!("/v1/admin/log/blobs/{sha256}"))
            .await?
            .map(|b| b.to_vec()))
    }

    async fn load_checkpoint(&self, name: &str) -> Result<Option<u64>, StoreError> {
        Ok(self
            .meta()
            .await?
            .checkpoints
            .into_iter()
            .find_map(|(n, p)| (n == name).then_some(p)))
    }

    async fn save_checkpoint(&self, name: &str, position: u64) -> Result<(), StoreError> {
        let body = serde_json::to_vec(&Checkpoint { position }).map_err(backend)?;
        let name = percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC);
        self.put(&format!("/v1/admin/log/checkpoints/{name}"), body)
            .await
    }

    async fn wake(&self) -> Result<Box<dyn Wake>, StoreError> {
        Err(backend("a server over HTTPS has no wake"))
    }

    fn kind(&self) -> &'static str {
        "https"
    }

    /// One page per request; the server checks each event follows its
    /// head, stream versions and hash chains, and answers once its read
    /// model has them.
    async fn import(&self, events: &[RecordedEvent]) -> Result<(), StoreError> {
        let Some(first) = events.first().map(|e| e.position) else {
            return Ok(());
        };
        let url = format!("{}/v1/admin/log", self.base);
        let page = LogPage::new(0, events);
        let body = serde_json::to_vec(&page).map_err(backend)?;
        let r = self
            .client
            .post(&url)
            .bearer_auth(self.token(lxm::LOG_WRITE))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(backend)?;
        if r.status() == reqwest::StatusCode::CONFLICT {
            return Err(StoreError::Import {
                position: first,
                reason: failure(r).await,
            });
        }
        if !r.status().is_success() {
            return Err(backend(format!("POST {url}: {}", failure(r).await)));
        }
        Ok(())
    }

    async fn blob_names(&self) -> Result<Vec<String>, StoreError> {
        Ok(self.meta().await?.blobs)
    }

    async fn checkpoints(&self) -> Result<Vec<(String, u64)>, StoreError> {
        Ok(self.meta().await?.checkpoints)
    }
}
