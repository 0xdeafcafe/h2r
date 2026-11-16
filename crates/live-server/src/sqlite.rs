//! The SQLite event store, the default (`--db sqlite://<data>/live.db`):
//! one file beside the server's key, no database server to run. The same
//! tables and semantics as [`crate::pg`]: positions in commit order,
//! optimistic stream versions, the hash chain, checkpoints and title-file
//! blobs.
//!
//! - **WAL** journal, so reads (the HTTP routes, the dispatcher) never wait
//!   for a write and a write never waits for reads; `synchronous=NORMAL`,
//!   which in WAL mode loses nothing on a process crash and at most the last
//!   commits on a power cut, never corrupts. Litestream (docs/live/
//!   live-hosting.md, "Free hosting") ships the WAL to R2 as it grows.
//! - **One writer.** Appends take an in-process lock and then `BEGIN
//!   IMMEDIATE`, SQLite's write lock, for the whole transaction, so positions
//!   are handed out in commit order (the same reason [`crate::pg`] holds an
//!   advisory lock) and another process writing the file (`h2 live store
//!   copy` into it) waits its turn instead of failing.
//! - **Wakes** come from the in-process lock's holder after each commit; a
//!   commit by another process is seen by the dispatcher's poll.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use live_proto::sign::chain_hash;
use sqlx::Row;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteRow,
    SqliteSynchronous,
};
use tokio::sync::watch;

use crate::store::{EventStore, ImportCheck, NewEvent, Query, RecordedEvent, StoreError, Wake};

const SCHEMA: &str = include_str!("sqlite_schema.sql");

/// The file a `sqlite:` URL names: `sqlite://<path>` or `sqlite:<path>`,
/// any `?options` dropped. The path is taken as written (spaces and all),
/// not percent-decoded; `sqlite:///abs` and `sqlite:/abs` are both `/abs`.
pub fn path_of(url: &str) -> Option<PathBuf> {
    let rest = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))?;
    let path = rest.split('?').next().unwrap_or(rest);
    (!path.is_empty() && path != ":memory:").then(|| PathBuf::from(path))
}

/// `sqlite://<path>`, the URL [`path_of`] reads back.
pub fn url_for(path: &Path) -> String {
    format!("sqlite://{}", path.display())
}

pub struct SqliteStore {
    pool: SqlitePool,
    path: PathBuf,
    /// Held across each write transaction (see the module notes).
    writer: tokio::sync::Mutex<()>,
    head: watch::Sender<u64>,
}

fn to_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn decode(row: &SqliteRow) -> Result<RecordedEvent, StoreError> {
    let position = row.get::<i64, _>("position") as u64;
    let index = row.get::<Option<i64>, _>("jws_index").map(|i| i as usize);
    let e = NewEvent::decode(row.get("jws"), index).map_err(|e| StoreError::Corrupt {
        position,
        reason: e.to_string(),
    })?;
    Ok(RecordedEvent {
        position,
        version: row.get::<i64, _>("version") as u64,
        hash: row.get("hash"),
        jws: e.jws,
        jws_index: e.jws_index,
        body: e.body,
    })
}

/// A list as a JSON array, for `json_each`.
fn json_list(items: &[String]) -> String {
    serde_json::to_string(items).expect("strings serialize")
}

impl SqliteStore {
    /// Opens the database at `path`, creating it (and its directory) and
    /// the schema if missing.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(StoreError::Io)?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(10))
            .foreign_keys(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(options)
            .await?;
        sqlx::raw_sql(SCHEMA).execute(&pool).await?;
        let store = SqliteStore {
            pool,
            path: path.to_path_buf(),
            writer: tokio::sync::Mutex::new(()),
            head: watch::channel(0).0,
        };
        let head = store.head().await?;
        store.head.send_replace(head);
        Ok(store)
    }

    /// [`Self::open`] on a `sqlite:` URL ([`path_of`]).
    pub async fn connect(url: &str) -> Result<Self, StoreError> {
        let path = path_of(url).ok_or_else(|| {
            StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{url:?} is not sqlite://<path>"),
            ))
        })?;
        Self::open(&path).await
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    #[cfg(test)]
    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// `stream`'s next version and last hash, inside a write transaction.
    async fn stream_head(
        tx: &mut sqlx::SqliteConnection,
        stream: &str,
    ) -> Result<(u64, Option<String>), StoreError> {
        let row = sqlx::query(
            "select version, hash from events where stream_id = ?1 order by version desc limit 1",
        )
        .bind(stream)
        .fetch_optional(&mut *tx)
        .await?;
        Ok(row.map_or((0, None), |r| {
            (r.get::<i64, _>("version") as u64 + 1, Some(r.get("hash")))
        }))
    }

    /// Runs `body` in a `BEGIN IMMEDIATE` transaction under the writer lock;
    /// commits if it succeeds (a dropped transaction rolls back).
    async fn write<T>(
        &self,
        body: impl for<'c> FnOnce(
            &'c mut sqlx::SqliteConnection,
        ) -> futures_util::future::BoxFuture<'c, Result<T, StoreError>>,
    ) -> Result<T, StoreError> {
        let _writer = self.writer.lock().await;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let out = body(&mut tx).await?;
        tx.commit().await?;
        Ok(out)
    }

    fn woke(&self, position: u64) {
        self.head.send_if_modified(|h| {
            let grew = position > *h;
            *h = (*h).max(position);
            grew
        });
    }
}

struct SqliteWake(watch::Receiver<u64>);

#[async_trait]
impl Wake for SqliteWake {
    async fn wait(&mut self) {
        if self.0.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[async_trait]
impl EventStore for SqliteStore {
    fn kind(&self) -> &'static str {
        "sqlite"
    }

    async fn append(
        &self,
        events: Vec<NewEvent>,
        expect: &[(String, u64)],
    ) -> Result<Vec<RecordedEvent>, StoreError> {
        let expect = expect.to_vec();
        let out = self
            .write(move |tx| {
                Box::pin(async move {
                    let mut heads = std::collections::HashMap::new();
                    let streams = expect
                        .iter()
                        .map(|(s, _)| s.clone())
                        .chain(events.iter().map(|e| e.body.stream.clone()))
                        .collect::<Vec<_>>();
                    for stream in streams {
                        if let std::collections::hash_map::Entry::Vacant(v) = heads.entry(stream) {
                            let head = Self::stream_head(tx, v.key()).await?;
                            v.insert(head);
                        }
                    }
                    for (stream, expected) in &expect {
                        let actual = heads[stream].0;
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
                        let head = heads.get_mut(&e.body.stream).expect("read above");
                        let (version, hash) = (head.0, chain_hash(head.1.as_deref(), &e.jws));
                        let b = &e.body;
                        let position: i64 = sqlx::query_scalar(
                            "insert into events (stream_id, version, type, data, at, author, kid, jws, jws_index, hash)
                             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                             returning position",
                        )
                        .bind(&b.stream)
                        .bind(to_i64(version))
                        .bind(&b.kind)
                        .bind(b.data.to_string())
                        .bind(&b.at)
                        .bind(&b.author)
                        .bind(&b.kid)
                        .bind(&e.jws)
                        .bind(e.jws_index.map(|i| i as i64))
                        .bind(&hash)
                        .fetch_one(&mut *tx)
                        .await?;
                        *head = (version + 1, Some(hash.clone()));
                        out.push(RecordedEvent {
                            position: position as u64,
                            version,
                            hash,
                            jws: e.jws,
                            jws_index: e.jws_index,
                            body: e.body,
                        });
                    }
                    Ok(out)
                })
            })
            .await?;
        if let Some(last) = out.last() {
            self.woke(last.position);
        }
        Ok(out)
    }

    async fn read(&self, q: &Query) -> Result<Vec<RecordedEvent>, StoreError> {
        let rows = sqlx::query(
            "select position, version, hash, jws, jws_index from events
             where position > ?1
               and (?2 is null or stream_id = ?2)
               and (json_array_length(?3) = 0 or type in (select value from json_each(?3)))
               and type not in (select value from json_each(?5))
             order by position limit ?4",
        )
        .bind(to_i64(q.after))
        .bind(&q.stream)
        .bind(json_list(&q.types))
        .bind(to_i64(q.limit as u64))
        .bind(json_list(&q.exclude))
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(decode).collect()
    }

    async fn head(&self) -> Result<u64, StoreError> {
        let p: i64 = sqlx::query_scalar("select coalesce(max(position), 0) from events")
            .fetch_one(&self.pool)
            .await?;
        Ok(p as u64)
    }

    async fn put_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError> {
        sqlx::query(
            "insert into blobs (sha256, bytes) values (?1, ?2) on conflict (sha256) do nothing",
        )
        .bind(sha256)
        .bind(bytes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn blob(&self, sha256: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(
            sqlx::query_scalar("select bytes from blobs where sha256 = ?1")
                .bind(sha256)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    async fn load_checkpoint(&self, name: &str) -> Result<Option<u64>, StoreError> {
        let p: Option<i64> =
            sqlx::query_scalar("select position from subscriptions where name = ?1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        Ok(p.map(|p| p as u64))
    }

    async fn save_checkpoint(&self, name: &str, position: u64) -> Result<(), StoreError> {
        sqlx::query(
            "insert into subscriptions (name, position) values (?1, ?2)
             on conflict (name) do update set position = excluded.position",
        )
        .bind(name)
        .bind(to_i64(position))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn wake(&self) -> Result<Box<dyn Wake>, StoreError> {
        Ok(Box::new(SqliteWake(self.head.subscribe())))
    }

    async fn import(&self, events: &[RecordedEvent]) -> Result<(), StoreError> {
        let Some(last) = events.last().map(|e| e.position) else {
            return Ok(());
        };
        let events = events.to_vec();
        self.write(move |tx| {
            Box::pin(async move {
                let head: i64 = sqlx::query_scalar("select coalesce(max(position), 0) from events")
                    .fetch_one(&mut *tx)
                    .await?;
                let mut check = ImportCheck::new(head as u64);
                for e in &events {
                    if !check.knows(&e.body.stream) {
                        let (next, hash) = Self::stream_head(tx, &e.body.stream).await?;
                        check.seed(&e.body.stream, next, hash);
                    }
                    check.check(e)?;
                    let b = &e.body;
                    sqlx::query(
                        "insert into events (position, stream_id, version, type, data, at, author, kid, jws, jws_index, hash)
                         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                    )
                    .bind(to_i64(e.position))
                    .bind(&b.stream)
                    .bind(to_i64(e.version))
                    .bind(&b.kind)
                    .bind(b.data.to_string())
                    .bind(&b.at)
                    .bind(&b.author)
                    .bind(&b.kid)
                    .bind(&e.jws)
                    .bind(e.jws_index.map(|i| i as i64))
                    .bind(&e.hash)
                    .execute(&mut *tx)
                    .await?;
                }
                Ok(())
            })
        })
        .await?;
        self.woke(last);
        Ok(())
    }

    async fn blob_names(&self) -> Result<Vec<String>, StoreError> {
        Ok(
            sqlx::query_scalar("select sha256 from blobs order by sha256")
                .fetch_all(&self.pool)
                .await?,
        )
    }

    async fn checkpoints(&self) -> Result<Vec<(String, u64)>, StoreError> {
        let rows: Vec<(String, i64)> =
            sqlx::query_as("select name, position from subscriptions order by name")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|(n, p)| (n, p as u64)).collect())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::store::conformance;

    pub fn temp_db(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "h2live-sqlite-{name}-{}-{}",
            std::process::id(),
            live_proto::sign::random_id()
        ));
        dir.join("live.db")
    }

    #[test]
    fn urls_name_files() {
        assert_eq!(path_of("sqlite://live.db"), Some("live.db".into()));
        assert_eq!(
            path_of("sqlite:///srv/live.db"),
            Some("/srv/live.db".into())
        );
        assert_eq!(
            path_of("sqlite:/srv/live.db?mode=rwc"),
            Some("/srv/live.db".into())
        );
        assert_eq!(
            path_of("sqlite:///Users/me/Library/Application Support/h2/live.db"),
            Some("/Users/me/Library/Application Support/h2/live.db".into())
        );
        assert_eq!(path_of("sqlite::memory:"), None);
        assert_eq!(path_of("postgres://localhost/h2live"), None);
        let p = Path::new("/a b/live.db");
        assert_eq!(path_of(&url_for(p)).as_deref(), Some(p));
    }

    #[tokio::test]
    async fn sqlite_store_conforms() {
        let path = temp_db("conform");
        let store = SqliteStore::open(&path).await.unwrap();
        conformance::run(&store, "sq").await;
        // A second run on the same file: the conformance suite tolerates a
        // shared log, as Postgres's does.
        conformance::run(&store, "sq2").await;
        drop(store);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[tokio::test]
    async fn the_log_survives_reopening_in_wal_mode() {
        let path = temp_db("reopen");
        let key = live_proto::sign::KeyPair::generate();
        let first = {
            let store = SqliteStore::open(&path).await.unwrap();
            let mode: String = sqlx::query_scalar("pragma journal_mode")
                .fetch_one(&store.pool)
                .await
                .unwrap();
            assert_eq!(mode, "wal");
            store
                .append(vec![conformance::event(&key, "s", 0)], &[("s".into(), 0)])
                .await
                .unwrap()
        };
        let store = SqliteStore::open(&path).await.unwrap();
        assert_eq!(store.head().await.unwrap(), first[0].position);
        let next = store
            .append(vec![conformance::event(&key, "s", 1)], &[("s".into(), 1)])
            .await
            .unwrap();
        assert_eq!(next[0].version, 1);
        assert_eq!(next[0].hash, chain_hash(Some(&first[0].hash), &next[0].jws));
        drop(store);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    /// The race test Postgres runs, on SQLite: one winner for version 0,
    /// and concurrent appends chained without gaps or repeats.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn sqlite_appends_race_safely() {
        let path = temp_db("race");
        let store = std::sync::Arc::new(SqliteStore::open(&path).await.unwrap());
        crate::pg::tests::race(store.clone()).await;
        drop(store);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    /// A second process writing the same file (a copy into a running
    /// server's database) waits for the lock rather than failing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_handles_on_one_file_take_turns() {
        let path = temp_db("two");
        let a = std::sync::Arc::new(SqliteStore::open(&path).await.unwrap());
        let b = std::sync::Arc::new(SqliteStore::open(&path).await.unwrap());
        let key = std::sync::Arc::new(live_proto::sign::KeyPair::generate());
        let tasks: Vec<_> = (0..20u64)
            .map(|n| {
                let store = if n % 2 == 0 { a.clone() } else { b.clone() };
                let key = key.clone();
                tokio::spawn(async move {
                    store
                        .append(vec![conformance::event(&key, "shared", n)], &[])
                        .await
                        .unwrap()
                })
            })
            .collect();
        for t in tasks {
            t.await.unwrap();
        }
        let all = a
            .read(&Query {
                stream: Some("shared".into()),
                ..Query::after(0, 100)
            })
            .await
            .unwrap();
        assert_eq!(
            all.iter().map(|e| e.version).collect::<Vec<_>>(),
            (0..20).collect::<Vec<_>>()
        );
        for w in all.windows(2) {
            assert_eq!(w[1].hash, chain_hash(Some(&w[0].hash), &w[1].jws));
        }
        drop((a, b));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
