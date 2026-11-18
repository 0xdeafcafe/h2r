//! The event store: one append-only log of signed events, ordered by a
//! global `position`, with per-stream versions, optimistic concurrency and
//! a per-stream hash chain (docs/live/live-security.md §2). SQLite by
//! default ([`crate::sqlite`]), Postgres when asked for ([`crate::pg`]), a
//! directory for the embedded server ([`crate::filestore`]), [`MemoryStore`]
//! for tests. [`crate::copy`] moves a log between them.
//!
//! The store keeps each author's JWS untouched; everything a reader needs
//! is decoded from it, so the other columns are only indexes. A batch JWS
//! (one signature over many events) is kept once, not once an event: in
//! memory its events share it ([`Arc`]); the databases store it in
//! `batches` under [`batch_key`] and its events' rows name it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use live_proto::WireEvent;
use live_proto::sign::{EventBody, SignError, SignedEvent, chain_hash};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

/// An event ready to append: its JWS (already verified, or signed by the
/// server), which element of a batch JWS it is, and the body it signs.
/// A batch's events share one JWS.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent {
    pub jws: Arc<str>,
    pub jws_index: Option<usize>,
    pub body: EventBody,
}

impl NewEvent {
    /// Decodes a stored event's body from its JWS, without verifying (the
    /// server verified it on the way in).
    pub fn decode(jws: impl Into<Arc<str>>, jws_index: Option<usize>) -> Result<Self, SignError> {
        let jws = jws.into();
        let body = SignedEvent::body_at(&jws, jws_index)?;
        Ok(NewEvent {
            jws,
            jws_index,
            body,
        })
    }
}

/// Opens stored events' JWS for their bodies, as [`NewEvent::decode`]
/// does, keeping the last batch it opened: a batch's events are adjacent
/// rows with the same JWS, so a page of them opens (base64 and JSON) the
/// batch once, not once a row, and shares one copy of its JWS.
#[derive(Default)]
pub struct BatchDecoder {
    batch: Option<(Arc<str>, Vec<EventBody>)>,
}

impl BatchDecoder {
    pub fn decode(&mut self, jws: &str, jws_index: Option<usize>) -> Result<NewEvent, SignError> {
        let Some(i) = jws_index else {
            return NewEvent::decode(jws, None);
        };
        if self.batch.as_ref().is_none_or(|(open, _)| **open != *jws) {
            self.batch = None;
            let bodies = SignedEvent::open_batch(jws)?.1;
            self.batch = Some((jws.into(), bodies));
        }
        let (jws, bodies) = self.batch.as_ref().expect("opened above");
        let body = bodies
            .get(i)
            .cloned()
            .ok_or(SignError::Malformed("jws_index out of range"))?;
        Ok(NewEvent {
            jws: Arc::clone(jws),
            jws_index,
            body,
        })
    }
}

/// The key a batch JWS is stored under in the databases' `batches` table:
/// hex(sha256(jws)).
pub fn batch_key(jws: &str) -> String {
    format!("{:x}", Sha256::digest(jws.as_bytes()))
}

/// Which `batches` row each event of an append (or import) names, and
/// whether that row still has to be written: once a batch, however many
/// events it has.
#[derive(Default)]
pub struct BatchRows {
    last: Option<(Arc<str>, String)>,
}

impl BatchRows {
    /// For an event about to be stored: `None` if it is a single event
    /// (its row keeps its own JWS), else its batch's key and whether this
    /// is the first of the batch's events here (so the batch row is due).
    pub fn key(&mut self, jws: &Arc<str>, jws_index: Option<usize>) -> Option<(String, bool)> {
        jws_index?;
        if let Some((last, key)) = &self.last
            && last == jws
        {
            return Some((key.clone(), false));
        }
        let key = batch_key(jws);
        self.last = Some((Arc::clone(jws), key.clone()));
        Some((key, true))
    }
}

/// A usage counter's change ([`crate::usage`]): `n` added to the counter
/// for (`day`, `account`, `metric`). `day` is a UTC day, `YYYY-MM-DD`, or
/// empty for a counter with no day (a running total such as cloud saves
/// stored); `account` a xuid in hex, or empty for the server's own.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct UsageRow {
    pub day: String,
    pub account: String,
    pub metric: String,
    pub n: i64,
}

/// Adds `rows` to a map of counters and drops the days before `keep_from`
/// (never the dayless ones): the in-memory stores' [`EventStore::add_usage`].
pub(crate) fn add_usage_to(
    map: &mut std::collections::BTreeMap<(String, String, String), i64>,
    rows: &[UsageRow],
    keep_from: &str,
) {
    for r in rows {
        *map.entry((r.day.clone(), r.account.clone(), r.metric.clone()))
            .or_default() += r.n;
    }
    map.retain(|(day, _, _), _| day.is_empty() || day.as_str() >= keep_from);
}

pub(crate) fn usage_rows(
    map: &std::collections::BTreeMap<(String, String, String), i64>,
) -> Vec<UsageRow> {
    map.iter()
        .map(|((day, account, metric), n)| UsageRow {
            day: day.clone(),
            account: account.clone(),
            metric: metric.clone(),
            n: *n,
        })
        .collect()
}

/// One event as stored. `version` counts from 0 within its stream.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedEvent {
    pub position: u64,
    pub version: u64,
    pub hash: String,
    pub jws: Arc<str>,
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
            jws: self.jws.to_string(),
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
    #[cfg(feature = "native")]
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    /// A database reached some other way (the Worker's D1), its error as
    /// text.
    #[error("database: {0}")]
    Backend(String),
    /// The file-backed store ([`crate::filestore`]) could not read or
    /// write its directory.
    #[error("store files: {0}")]
    Io(std::io::Error),
    /// A stored row that no longer decodes: corruption, not a request
    /// problem.
    #[error("stored event at position {position} does not decode: {reason}")]
    Corrupt { position: u64, reason: String },
    /// Writes are held while a log is copied in ([`crate::hold`]).
    #[error("writes are held while the log is copied in (h2 live store copy --release)")]
    Held,
    /// [`EventStore::import`] was given an event that does not follow the
    /// log it is going into: nothing of that batch was written.
    #[error("cannot import position {position}: {reason}")]
    Import { position: u64, reason: String },
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

/// A hint that the log may have grown. Best effort where the dispatcher
/// polls as well (a lost wake costs latency only); a store run with
/// `Timers::Driven` must wake on every append, as one writer's can.
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

    /// `sqlite`, `postgres`, `file` or `memory`, for the admin status.
    fn kind(&self) -> &'static str {
        "unknown"
    }

    /// Writes events exactly as another store recorded them, positions,
    /// versions and hashes kept, atomically (`h2 live store copy`). Each must
    /// come after this log's head and continue its stream's version and hash
    /// chain ([`ImportCheck`]); checked, not trusted. Stores that number
    /// positions themselves cannot take them.
    async fn import(&self, events: &[RecordedEvent]) -> Result<(), StoreError> {
        let position = events.first().map_or(0, |e| e.position);
        Err(StoreError::Import {
            position,
            reason: format!("a {} store cannot be copied into", self.kind()),
        })
    }

    /// Every title-file blob's SHA-256, for copying them.
    async fn blob_names(&self) -> Result<Vec<String>, StoreError>;

    /// Every durable subscriber's checkpoint, for copying them.
    async fn checkpoints(&self) -> Result<Vec<(String, u64)>, StoreError>;

    /// Adds each row's `n` to its usage counter (made at 0 if new), then
    /// drops every counter for a day before `keep_from` (dayless ones
    /// stay), in one go ([`crate::usage`]). A store that keeps no counters
    /// takes them and forgets them: the server's own copy, in memory,
    /// still counts until it restarts.
    async fn add_usage(&self, rows: &[UsageRow], keep_from: &str) -> Result<(), StoreError> {
        let _ = (rows, keep_from);
        Ok(())
    }

    /// Every usage counter, read once when the server starts.
    async fn usage(&self) -> Result<Vec<UsageRow>, StoreError> {
        Ok(Vec::new())
    }
}

/// What [`EventStore::import`] checks of each event, given each stream's
/// last version and hash in the log being written (looked up once per
/// stream by `last`).
#[derive(Default)]
pub struct ImportCheck {
    head: u64,
    streams: HashMap<String, (u64, Option<String>)>,
}

impl ImportCheck {
    pub fn new(head: u64) -> Self {
        ImportCheck {
            head,
            streams: HashMap::new(),
        }
    }

    /// Whether `stream`'s head is known yet (else call [`Self::seed`]).
    pub fn knows(&self, stream: &str) -> bool {
        self.streams.contains_key(stream)
    }

    /// `stream`'s next version and last hash in the log.
    pub fn seed(&mut self, stream: &str, next: u64, hash: Option<String>) {
        self.streams.insert(stream.to_string(), (next, hash));
    }

    /// Checks `e` follows what came before, and records it.
    pub fn check(&mut self, e: &RecordedEvent) -> Result<(), StoreError> {
        let fail = |reason: String| StoreError::Import {
            position: e.position,
            reason,
        };
        if e.position <= self.head {
            return Err(fail(format!("the log is already at {}", self.head)));
        }
        let (next, prev) = self
            .streams
            .get(&e.body.stream)
            .cloned()
            .expect("seeded before checking");
        if e.version != next {
            return Err(fail(format!(
                "{} is at version {next}, the event says {}",
                e.body.stream, e.version
            )));
        }
        let hash = chain_hash(prev.as_deref(), &e.jws);
        if hash != e.hash {
            return Err(fail(format!("the hash chain of {} breaks", e.body.stream)));
        }
        self.head = e.position;
        self.streams
            .insert(e.body.stream.clone(), (next + 1, Some(hash)));
        Ok(())
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
    usage: std::collections::BTreeMap<(String, String, String), i64>,
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
    pub(crate) fn checkpoint_map(&self) -> HashMap<String, u64> {
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

    /// Positions here are indexes, so only a log with no gaps imports.
    async fn import(&self, events: &[RecordedEvent]) -> Result<(), StoreError> {
        let mut log = self.inner.lock().unwrap();
        let len = log.events.len() as u64;
        let mut check = ImportCheck::new(len);
        for (i, e) in events.iter().enumerate() {
            if e.position != len + 1 + i as u64 {
                return Err(StoreError::Import {
                    position: e.position,
                    reason: "an in-memory log has no gaps in its positions".into(),
                });
            }
            if !check.knows(&e.body.stream) {
                let (next, hash) = log
                    .streams
                    .get(&e.body.stream)
                    .cloned()
                    .map_or((0, None), |(n, h)| (n, Some(h)));
                check.seed(&e.body.stream, next, hash);
            }
            check.check(e)?;
        }
        for e in events {
            log.streams
                .insert(e.body.stream.clone(), (e.version + 1, e.hash.clone()));
            log.events.push(e.clone());
        }
        let head = log.events.len() as u64;
        drop(log);
        self.head.send_replace(head);
        Ok(())
    }

    async fn blob_names(&self) -> Result<Vec<String>, StoreError> {
        let mut names: Vec<_> = self.inner.lock().unwrap().blobs.keys().cloned().collect();
        names.sort();
        Ok(names)
    }

    async fn checkpoints(&self) -> Result<Vec<(String, u64)>, StoreError> {
        let mut all: Vec<_> = self.checkpoint_map().into_iter().collect();
        all.sort();
        Ok(all)
    }

    async fn add_usage(&self, rows: &[UsageRow], keep_from: &str) -> Result<(), StoreError> {
        add_usage_to(&mut self.inner.lock().unwrap().usage, rows, keep_from);
        Ok(())
    }

    async fn usage(&self) -> Result<Vec<UsageRow>, StoreError> {
        Ok(usage_rows(&self.inner.lock().unwrap().usage))
    }
}

/// The behaviour every store must have; run against every implementation
/// (the Worker's D1 store too, through the `conformance` feature).
#[cfg(any(test, feature = "conformance"))]
pub mod conformance {
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
            jws: body.sign(key).into(),
            jws_index: None,
            body,
        }
    }

    /// Usage counters add up, keep their dayless totals and lose the days
    /// before the cut. Rows are on this run's own account, on days no
    /// server writes (1970), so a shared store's other rows don't matter.
    pub async fn usage(store: &dyn EventStore, prefix: &str) {
        let acct = format!("{prefix}-usage");
        let row = |day: &str, metric: &str, n: i64| UsageRow {
            day: day.into(),
            account: acct.clone(),
            metric: metric.into(),
            n,
        };
        let mine = |rows: Vec<UsageRow>| {
            let mut r: Vec<_> = rows.into_iter().filter(|r| r.account == acct).collect();
            r.sort();
            r
        };
        store
            .add_usage(
                &[
                    row("1970-01-01", "up.frames", 10),
                    row("1970-01-02", "up.frames", 5),
                    row("", "stored.saves", 7),
                ],
                "1970-01-01",
            )
            .await
            .unwrap();
        store
            .add_usage(
                &[
                    row("1970-01-02", "up.frames", 1),
                    row("", "stored.saves", -2),
                ],
                "1970-01-01",
            )
            .await
            .unwrap();
        assert_eq!(
            mine(store.usage().await.unwrap()),
            [
                row("", "stored.saves", 5),
                row("1970-01-01", "up.frames", 10),
                row("1970-01-02", "up.frames", 6)
            ]
        );
        // The cut drops whole days, at once with the write; never the dayless.
        store
            .add_usage(&[row("1970-01-03", "events", 1)], "1970-01-02")
            .await
            .unwrap();
        assert_eq!(
            mine(store.usage().await.unwrap()),
            [
                row("", "stored.saves", 5),
                row("1970-01-02", "up.frames", 6),
                row("1970-01-03", "events", 1)
            ]
        );
        store.add_usage(&[], "1970-01-04").await.unwrap();
        assert_eq!(
            mine(store.usage().await.unwrap()),
            [row("", "stored.saves", 5)]
        );
    }

    /// A batch JWS of `n` events on `stream`, and their bodies.
    pub fn batch(key: &KeyPair, stream: &str, n: u64) -> (Arc<str>, Vec<EventBody>) {
        let bodies: Vec<EventBody> = (0..n).map(|i| event(key, stream, 100 + i).body).collect();
        (live_proto::sign::sign_batch(key, &bodies).into(), bodies)
    }

    /// A log as a server before `batches` wrote it, every row with its own
    /// JWS: a single event, then a batch of three on `stream`, chained.
    pub fn legacy_log(key: &KeyPair, stream: &str) -> Vec<RecordedEvent> {
        let (jws, bodies) = batch(key, stream, 3);
        let mut rows: Vec<NewEvent> = vec![event(key, stream, 1)];
        rows.extend(bodies.into_iter().enumerate().map(|(i, body)| NewEvent {
            jws: Arc::clone(&jws),
            jws_index: Some(i),
            body,
        }));
        let mut prev: Option<String> = None;
        rows.into_iter()
            .enumerate()
            .map(|(i, e)| {
                let hash = chain_hash(prev.as_deref(), &e.jws);
                prev = Some(hash.clone());
                RecordedEvent {
                    position: i as u64 + 1,
                    version: i as u64,
                    hash,
                    jws: e.jws,
                    jws_index: e.jws_index,
                    body: e.body,
                }
            })
            .collect()
    }

    /// `store` was opened on [`legacy_log`]'s rows, written the old way:
    /// they read back unchanged (their chain verifies), and the stream
    /// carries on with a batch stored the new way.
    pub async fn upgraded(store: &dyn EventStore, key: &KeyPair, legacy: &[RecordedEvent]) {
        let stream = legacy[0].body.stream.clone();
        let q = Query {
            stream: Some(stream.clone()),
            ..Query::after(0, 100)
        };
        let old = store.read(&q).await.unwrap();
        assert_eq!(old, legacy);
        let mut prev: Option<String> = None;
        for e in &old {
            assert_eq!(e.hash, chain_hash(prev.as_deref(), &e.jws));
            prev = Some(e.hash.clone());
        }
        let (jws, bodies) = batch(key, &stream, 2);
        let next = store
            .append(
                bodies
                    .into_iter()
                    .enumerate()
                    .map(|(i, body)| NewEvent {
                        jws: Arc::clone(&jws),
                        jws_index: Some(i),
                        body,
                    })
                    .collect(),
                &[(stream, legacy.len() as u64)],
            )
            .await
            .unwrap();
        let all = store.read(&q).await.unwrap();
        assert_eq!(all[..legacy.len()], *legacy);
        assert_eq!(all[legacy.len()..], next);
        for w in all.windows(2) {
            assert_eq!(w[1].hash, chain_hash(Some(&w[0].hash), &w[1].jws));
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

        // A batch's events (one JWS) read back whole, each with its own
        // body and a chain link over the batch's JWS, around a single
        // event and across two appends of the same batch's events.
        let c = format!("{prefix}-c");
        let batch = batch(&key, &c, 3);
        let at = |i: usize| NewEvent {
            jws: Arc::clone(&batch.0),
            jws_index: Some(i),
            body: batch.1[i].clone(),
        };
        let mut written = store
            .append(vec![at(0), event(&key, &c, 7), at(1)], &[(c.clone(), 0)])
            .await
            .unwrap();
        written.extend(store.append(vec![at(2)], &[]).await.unwrap());
        let read = store
            .read(&Query {
                stream: Some(c.clone()),
                ..Query::after(base, 100)
            })
            .await
            .unwrap();
        assert_eq!(read, written);
        assert_eq!(
            read.iter().map(|e| e.jws_index).collect::<Vec<_>>(),
            [Some(0), None, Some(1), Some(2)]
        );
        let mut prev: Option<String> = None;
        for e in &read {
            assert_eq!(e.hash, chain_hash(prev.as_deref(), &e.jws));
            assert_eq!(
                NewEvent::decode(Arc::clone(&e.jws), e.jws_index)
                    .unwrap()
                    .body,
                e.body
            );
            prev = Some(e.hash.clone());
        }
        assert_eq!(*read[0].jws, *batch.0);
        assert_eq!(*read[3].jws, *batch.0);

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

        usage(store, prefix).await;

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

    #[test]
    fn batch_decoder_opens_what_each_row_would() {
        let key = live_proto::sign::KeyPair::generate();
        let bodies = |n: u64| -> Vec<EventBody> {
            (0..3)
                .map(|i| conformance::event(&key, "s", n * 10 + i).body)
                .collect()
        };
        let one = live_proto::sign::sign_batch(&key, &bodies(1));
        let two = live_proto::sign::sign_batch(&key, &bodies(2));
        let single = conformance::event(&key, "s", 99).jws.to_string();
        // Rows as a page holds them: a batch, a single event, the batch
        // again (not adjacent), another batch.
        let rows = [
            (one.clone(), Some(0)),
            (one.clone(), Some(1)),
            (single, None),
            (one.clone(), Some(2)),
            (two.clone(), Some(0)),
            (two, Some(2)),
        ];
        let mut batches = BatchDecoder::default();
        for (jws, index) in rows {
            let expected = NewEvent::decode(jws.clone(), index).unwrap();
            assert_eq!(batches.decode(&jws, index).unwrap(), expected);
        }
        assert!(batches.decode(&one, Some(3)).is_err(), "out of range");
    }
}
