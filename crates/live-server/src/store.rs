//! The event store: one append-only log of signed events, ordered by a
//! global `position`, with per-stream versions, optimistic concurrency and
//! a per-stream hash chain (docs/live-security.md §2). Postgres in
//! production ([`crate::pg`]), a directory for the embedded server without
//! one ([`crate::filestore`]), [`MemoryStore`] for tests.
//!
//! The store keeps each author's JWS untouched; everything a reader needs
//! is decoded from it, so the other columns are only indexes.

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use live_proto::WireEvent;
use live_proto::sign::{EventBody, SignError, SignedEvent, chain_hash};
use tokio::sync::watch;

/// An event ready to append: its JWS (already verified, or signed by the
/// server), which element of a batch JWS it is, and the body it signs.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    pub jws: String,
    pub jws_index: Option<usize>,
    pub body: EventBody,
}

impl NewEvent {
    /// Decodes a stored event's body from its JWS, without verifying (the
    /// server verified it on the way in).
    pub fn decode(jws: String, jws_index: Option<usize>) -> Result<Self, SignError> {
        let body = SignedEvent::body_at(&jws, jws_index)?;
        Ok(NewEvent {
            jws,
            jws_index,
            body,
        })
    }
}

/// One event as stored. `version` counts from 0 within its stream.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedEvent {
    pub position: u64,
    pub version: u64,
    pub hash: String,
    pub jws: String,
    pub jws_index: Option<usize>,
    pub body: EventBody,
}

impl RecordedEvent {
    pub fn to_wire(&self) -> WireEvent {
        let b = &self.body;
        WireEvent {
            position: self.position,
            stream: b.stream.clone(),
            version: self.version,
            kind: b.kind.clone(),
            at: b.at.clone(),
            data: b.data.clone(),
            author: b.author.clone(),
            kid: b.kid.clone(),
            jws: self.jws.clone(),
            jws_index: self.jws_index,
            hash: self.hash.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A stream was not at the version the append expected. Nothing was
    /// written.
    #[error("stream {stream} is at version {actual}, not {expected}")]
    Conflict {
        stream: String,
        expected: u64,
        actual: u64,
    },
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    /// The file-backed store ([`crate::filestore`]) could not read or
    /// write its directory.
    #[error("store files: {0}")]
    Io(std::io::Error),
    /// A stored row that no longer decodes: corruption, not a request
    /// problem.
    #[error("stored event at position {position} does not decode: {reason}")]
    Corrupt { position: u64, reason: String },
}

/// What [`EventStore::read`] returns: events with `position > after`,
/// oldest first, at most `limit`, optionally only one stream, only some
/// types, and not others.
#[derive(Debug, Clone, Default)]
pub struct Query {
    pub after: u64,
    pub limit: usize,
    pub stream: Option<String>,
    pub types: Vec<String>,
    pub exclude: Vec<String>,
}

impl Query {
    pub fn after(after: u64, limit: usize) -> Self {
        Query {
            after,
            limit,
            ..Query::default()
        }
    }

    pub fn matches(&self, e: &RecordedEvent) -> bool {
        e.position > self.after
            && self.stream.as_ref().is_none_or(|s| *s == e.body.stream)
            && (self.types.is_empty() || self.types.contains(&e.body.kind))
            && !self.exclude.contains(&e.body.kind)
    }
}

/// A hint that the log may have grown. Best effort: the dispatcher polls
/// as well, so a lost wake costs latency only.
#[async_trait]
pub trait Wake: Send {
    async fn wait(&mut self);
}

#[async_trait]
pub trait EventStore: Send + Sync + 'static {
    /// Appends `events` in order, each to its `body.stream`, atomically:
    /// either all are written or none. Each `(stream, version)` in `expect`
    /// must be that stream's next version (0: the stream must not exist),
    /// else [`StoreError::Conflict`]. Positions are handed out in commit
    /// order, so a tailer reading by position never skips one.
    async fn append(
        &self,
        events: Vec<NewEvent>,
        expect: &[(String, u64)],
    ) -> Result<Vec<RecordedEvent>, StoreError>;

    async fn read(&self, query: &Query) -> Result<Vec<RecordedEvent>, StoreError>;

    /// The highest position, 0 for an empty log.
    async fn head(&self) -> Result<u64, StoreError>;

    /// Content-addressed bytes (title files), keyed by hex SHA-256.
    async fn put_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError>;
    async fn blob(&self, sha256: &str) -> Result<Option<Vec<u8>>, StoreError>;

    /// Where a durable subscriber resumes (the `subscriptions` table).
    async fn load_checkpoint(&self, name: &str) -> Result<Option<u64>, StoreError>;
    async fn save_checkpoint(&self, name: &str, position: u64) -> Result<(), StoreError>;

    /// A wake source that fires after appends commit.
    async fn wake(&self) -> Result<Box<dyn Wake>, StoreError>;

    /// `postgres`, `file` or `memory`, for the admin status.
    fn kind(&self) -> &'static str {
        "unknown"
    }
}

/// The in-memory log, for tests.
pub struct MemoryStore {
    inner: Mutex<MemoryLog>,
    head: watch::Sender<u64>,
}

#[derive(Default)]
struct MemoryLog {
    events: Vec<RecordedEvent>,
    /// stream -> (next version, last hash)
    streams: HashMap<String, (u64, String)>,
    blobs: HashMap<String, Vec<u8>>,
    checkpoints: HashMap<String, u64>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        MemoryStore {
            inner: Mutex::default(),
            head: watch::channel(0).0,
        }
    }
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every saved checkpoint, for a store that persists them.
    pub(crate) fn checkpoints(&self) -> HashMap<String, u64> {
        self.inner.lock().unwrap().checkpoints.clone()
    }
}

struct MemoryWake(watch::Receiver<u64>);

#[async_trait]
impl Wake for MemoryWake {
    async fn wait(&mut self) {
        if self.0.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[async_trait]
impl EventStore for MemoryStore {
    fn kind(&self) -> &'static str {
        "memory"
    }

    async fn append(
        &self,
        events: Vec<NewEvent>,
        expect: &[(String, u64)],
    ) -> Result<Vec<RecordedEvent>, StoreError> {
        let mut log = self.inner.lock().unwrap();
        for (stream, expected) in expect {
            let actual = log.streams.get(stream).map_or(0, |s| s.0);
            if actual != *expected {
                return Err(StoreError::Conflict {
                    stream: stream.clone(),
                    expected: *expected,
                    actual,
                });
            }
        }
        let mut out = Vec::with_capacity(events.len());
        for e in events {
            let (version, prev) = log.streams.get(&e.body.stream).cloned().unwrap_or_default();
            let hash = chain_hash((version > 0).then_some(prev.as_str()), &e.jws);
            log.streams
                .insert(e.body.stream.clone(), (version + 1, hash.clone()));
            let rec = RecordedEvent {
                position: log.events.len() as u64 + 1,
                version,
                hash,
                jws: e.jws,
                jws_index: e.jws_index,
                body: e.body,
            };
            log.events.push(rec.clone());
            out.push(rec);
        }
        let head = log.events.len() as u64;
        drop(log);
        self.head.send_replace(head);
        Ok(out)
    }

    async fn read(&self, query: &Query) -> Result<Vec<RecordedEvent>, StoreError> {
        let log = self.inner.lock().unwrap();
        let start = (query.after as usize).min(log.events.len());
        Ok(log.events[start..]
            .iter()
            .filter(|e| query.matches(e))
            .take(query.limit)
            .cloned()
            .collect())
    }

    async fn head(&self) -> Result<u64, StoreError> {
        Ok(self.inner.lock().unwrap().events.len() as u64)
    }

    async fn put_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError> {
        self.inner
            .lock()
            .unwrap()
            .blobs
            .insert(sha256.into(), bytes.to_vec());
        Ok(())
    }

    async fn blob(&self, sha256: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.inner.lock().unwrap().blobs.get(sha256).cloned())
    }

    async fn load_checkpoint(&self, name: &str) -> Result<Option<u64>, StoreError> {
        Ok(self.inner.lock().unwrap().checkpoints.get(name).copied())
    }

    async fn save_checkpoint(&self, name: &str, position: u64) -> Result<(), StoreError> {
        self.inner
            .lock()
            .unwrap()
            .checkpoints
            .insert(name.into(), position);
        Ok(())
    }

    async fn wake(&self) -> Result<Box<dyn Wake>, StoreError> {
        Ok(Box::new(MemoryWake(self.head.subscribe())))
    }
}

/// The behaviour every store must have; run against both implementations.
#[cfg(test)]
pub(crate) mod conformance {
    use super::*;
    use live_proto::sign::KeyPair;
    use serde_json::json;

    pub fn event(key: &KeyPair, stream: &str, n: u64) -> NewEvent {
        let body = EventBody::new(
            key,
            "server:test",
            stream,
            "Test",
            json!({"n": n}),
            time::OffsetDateTime::now_utc(),
        );
        NewEvent {
            jws: body.sign(key),
            jws_index: None,
            body,
        }
    }

    /// `prefix` keeps streams unique so a reused database still passes.
    pub async fn run(store: &dyn EventStore, prefix: &str) {
        let key = KeyPair::generate();
        let a = format!("{prefix}-a");
        let b = format!("{prefix}-b");
        let base = store.head().await.unwrap();

        // Versions per stream, positions global and consecutive.
        let first = store
            .append(
                vec![event(&key, &a, 0), event(&key, &a, 1)],
                &[(a.clone(), 0)],
            )
            .await
            .unwrap();
        assert_eq!(first.iter().map(|e| e.version).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(first[1].position, first[0].position + 1);
        let second = store.append(vec![event(&key, &b, 2)], &[]).await.unwrap();
        assert_eq!(second[0].version, 0);
        assert!(second[0].position > first[1].position);
        assert!(store.head().await.unwrap() >= second[0].position);

        // The hash chain links each stream's events.
        assert_eq!(first[0].hash, chain_hash(None, &first[0].jws));
        assert_eq!(
            first[1].hash,
            chain_hash(Some(&first[0].hash), &first[1].jws)
        );
        assert_eq!(second[0].hash, chain_hash(None, &second[0].jws));
        let third = store
            .append(vec![event(&key, &a, 3)], &[(a.clone(), 2)])
            .await
            .unwrap();
        assert_eq!(
            third[0].hash,
            chain_hash(Some(&first[1].hash), &third[0].jws)
        );

        // A wrong expectation on any stream writes nothing at all.
        let count = |stream: &str| {
            let q = Query {
                stream: Some(stream.to_string()),
                ..Query::after(base, 100)
            };
            async move { store.read(&q).await.unwrap().len() }
        };
        for wrong in [0, 2, 9] {
            let err = store
                .append(
                    vec![event(&key, &b, 4), event(&key, &a, 5)],
                    &[(b.clone(), 1), (a.clone(), wrong)],
                )
                .await
                .unwrap_err();
            assert!(
                matches!(err, StoreError::Conflict { actual: 3, .. }),
                "{err:?}"
            );
        }
        assert_eq!((count(&a).await, count(&b).await), (3, 1));

        // Reads: exclusive cursor, limit, stream and type filters. (Other
        // writers may share the database, hence the stream filters.)
        let only_a = Query {
            stream: Some(a.clone()),
            ..Query::after(base, 100)
        };
        let mine = store.read(&only_a).await.unwrap();
        assert_eq!(mine, [first[0].clone(), first[1].clone(), third[0].clone()]);
        let after_first = store
            .read(&Query {
                after: first[0].position,
                limit: 1,
                ..only_a.clone()
            })
            .await
            .unwrap();
        assert_eq!(after_first, vec![first[1].clone()]);
        let all = store.read(&Query::after(base, 1000)).await.unwrap();
        assert!(all.windows(2).all(|w| w[0].position < w[1].position));
        assert!(all.iter().all(|e| e.position > base));
        let typed = Query {
            stream: Some(b.clone()),
            types: vec!["Nope".into()],
            ..Query::after(base, 100)
        };
        assert!(store.read(&typed).await.unwrap().is_empty());
        let typed = Query {
            types: vec!["Nope".into(), "Test".into()],
            ..typed
        };
        assert_eq!(store.read(&typed).await.unwrap().len(), 1);
        let excluded = Query {
            exclude: vec!["Test".into()],
            ..only_a.clone()
        };
        assert!(store.read(&excluded).await.unwrap().is_empty());

        // Blobs and checkpoints.
        let sha = format!("{prefix}-sha");
        assert_eq!(store.blob(&sha).await.unwrap(), None);
        store.put_blob(&sha, b"bytes").await.unwrap();
        store.put_blob(&sha, b"bytes").await.unwrap();
        assert_eq!(
            store.blob(&sha).await.unwrap().as_deref(),
            Some(&b"bytes"[..])
        );
        let cp = format!("{prefix}-cp");
        assert_eq!(store.load_checkpoint(&cp).await.unwrap(), None);
        store.save_checkpoint(&cp, 41).await.unwrap();
        store.save_checkpoint(&cp, 42).await.unwrap();
        assert_eq!(store.load_checkpoint(&cp).await.unwrap(), Some(42));

        // Appends wake a waiting tailer.
        let mut wake = store.wake().await.unwrap();
        let waiting = tokio::spawn(async move { wake.wait().await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        store.append(vec![event(&key, &b, 6)], &[]).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), waiting)
            .await
            .expect("woken")
            .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_store_conforms() {
        conformance::run(&MemoryStore::new(), "mem").await;
    }
}
