//! The event log in SQLite reached through [`Sql`]: the same tables and
//! statements as the SQLite store on a machine (`live_server::sql`). In
//! the Worker that is the `LiveHub` Durable Object's own storage
//! (`dosql`); in the tests here it is SQLite through sqlx, so the store is
//! tested against the shared store tests and the machine's own SQLite
//! store. Any other SQLite behind an atomic batch (D1) fits the same trait.
//!
//! An append reads the streams' heads, checks them, then writes every row
//! in one atomic batch. That is safe only with one writer: the hub, which
//! holds this store and appends one call at a time (`writer`).

use std::sync::Arc;

use async_trait::async_trait;
use live_proto::sign::chain_hash;
use live_server::sql;
use live_server::store::{
    BatchDecoder, BatchRows, EventStore, ImportCheck, NewEvent, Query, RecordedEvent, StoreError,
    UsageRow, Wake,
};
use serde_json::Value;
use tokio::sync::{Mutex, watch};

/// A bound parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Null,
    Int(i64),
    Text(String),
    Bytes(Vec<u8>),
}

impl From<&str> for Param {
    fn from(s: &str) -> Self {
        Param::Text(s.to_string())
    }
}

impl From<String> for Param {
    fn from(s: String) -> Self {
        Param::Text(s)
    }
}

impl From<u64> for Param {
    fn from(n: u64) -> Self {
        Param::Int(i64::try_from(n).unwrap_or(i64::MAX))
    }
}

impl<T: Into<Param>> From<Option<T>> for Param {
    fn from(v: Option<T>) -> Self {
        v.map_or(Param::Null, Into::into)
    }
}

/// A row, by column name. Integers may come back as JSON floats (from
/// JavaScript's numbers) and blobs as arrays of byte values.
pub type Row = serde_json::Map<String, Value>;

/// A statement and its parameters.
pub type Statement = (&'static str, Vec<Param>);

/// What the store needs of a database.
#[async_trait]
pub trait Sql: Send + Sync + 'static {
    /// Every row `sql` returns.
    async fn all(&self, sql: &str, params: Vec<Param>) -> Result<Vec<Row>, StoreError>;
    /// The statements in order, all or none; each one's rows.
    async fn batch(&self, statements: Vec<Statement>) -> Result<Vec<Vec<Row>>, StoreError>;
    /// Runs a script of statements without parameters (the schema).
    async fn script(&self, sql: &str) -> Result<(), StoreError>;
}

/// A script's statements, one by one, without its comments: JavaScript's
/// SQLite bindings prepare one statement at a time.
pub fn script_statements(script: &str) -> Vec<String> {
    let code: String = script
        .lines()
        .map(|l| l.split_once("--").map_or(l, |(code, _)| code))
        .collect::<Vec<_>>()
        .join("\n");
    code.split(';')
        .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|s| !s.is_empty())
        .collect()
}

pub struct SqlStore<S> {
    sql: S,
    /// One append or import at a time (see the module's docs).
    writer: Mutex<()>,
    head: watch::Sender<u64>,
}

fn corrupt(position: u64, reason: impl Into<String>) -> StoreError {
    StoreError::Corrupt {
        position,
        reason: reason.into(),
    }
}

/// An integer column, whether it came back as an integer or a float.
fn int(row: &Row, column: &str) -> Option<u64> {
    let v = row.get(column)?;
    v.as_u64().or_else(|| {
        v.as_f64()
            .filter(|f| *f >= 0.0 && f.fract() == 0.0)
            .map(|f| f as u64)
    })
}

/// A signed integer column (a usage counter may go down), as [`int`] reads.
fn signed(row: &Row, column: &str) -> Option<i64> {
    let v = row.get(column)?;
    v.as_i64()
        .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
}

/// The first column of the first row, as an integer (`coalesce(max(..))`).
fn scalar(rows: &[Row]) -> u64 {
    rows.first()
        .and_then(|r| r.values().next())
        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
        .unwrap_or(0)
}

fn text<'r>(row: &'r Row, column: &str) -> Option<&'r str> {
    row.get(column)?.as_str()
}

/// A blob column: an array of byte values (or, from a store that could,
/// text).
fn bytes(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::Array(items) => items
            .iter()
            .map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
            .collect(),
        Value::String(s) => Some(s.as_bytes().to_vec()),
        _ => None,
    }
}

fn decode(row: &Row, batches: &mut BatchDecoder) -> Result<RecordedEvent, StoreError> {
    let position = int(row, "position").ok_or_else(|| corrupt(0, "a row without a position"))?;
    let jws = text(row, "jws").ok_or_else(|| corrupt(position, "no jws"))?;
    let index = int(row, "jws_index").map(|i| i as usize);
    let e = batches
        .decode(jws, index)
        .map_err(|e| corrupt(position, e.to_string()))?;
    Ok(RecordedEvent {
        position,
        version: int(row, "version").ok_or_else(|| corrupt(position, "no version"))?,
        hash: text(row, "hash")
            .ok_or_else(|| corrupt(position, "no hash"))?
            .to_string(),
        jws: e.jws,
        jws_index: e.jws_index,
        body: e.body,
    })
}

/// An event row's parameters after its position: stream, version, type,
/// data, at, author, kid, jws, jws_index, hash, batch.
fn event_params(
    e: &NewEvent,
    version: u64,
    hash: &str,
    jws: &str,
    batch: Option<String>,
) -> Vec<Param> {
    let b = &e.body;
    vec![
        b.stream.as_str().into(),
        version.into(),
        b.kind.as_str().into(),
        b.data.to_string().into(),
        b.at.as_str().into(),
        b.author.as_str().into(),
        b.kid.as_str().into(),
        jws.into(),
        e.jws_index.map(|i| i as u64).into(),
        hash.into(),
        batch.into(),
    ]
}

impl<S: Sql> SqlStore<S> {
    /// Creates the tables if missing and reads the head.
    pub async fn open(sql: S) -> Result<Self, StoreError> {
        sql.script(sql::SCHEMA).await?;
        let head = scalar(&sql.all(sql::HEAD, vec![]).await?);
        Ok(SqlStore {
            sql,
            writer: Mutex::new(()),
            head: watch::channel(head).0,
        })
    }

    /// `stream`'s next version and last hash.
    async fn stream_head(&self, stream: &str) -> Result<(u64, Option<String>), StoreError> {
        let rows = self.sql.all(sql::STREAM_HEAD, vec![stream.into()]).await?;
        Ok(rows.first().map_or((0, None), |r| {
            (
                int(r, "version").unwrap_or(0) + 1,
                text(r, "hash").map(str::to_string),
            )
        }))
    }

    /// The `batches` row an event needs first, if any, and its row's `jws`
    /// and `batch` columns.
    fn jws_columns(
        rows: &mut BatchRows,
        e: &NewEvent,
        statements: &mut Vec<Statement>,
    ) -> (String, Option<String>) {
        match rows.key(&e.jws, e.jws_index) {
            None => (e.jws.to_string(), None),
            Some((key, first)) => {
                if first {
                    statements.push((
                        sql::INSERT_BATCH,
                        vec![key.as_str().into(), (*e.jws).into()],
                    ));
                }
                (String::new(), Some(key))
            }
        }
    }

    fn woke(&self, position: u64) {
        self.head.send_if_modified(|h| {
            let grew = position > *h;
            *h = (*h).max(position);
            grew
        });
    }
}

struct SqlWake(watch::Receiver<u64>);

#[async_trait]
impl Wake for SqlWake {
    async fn wait(&mut self) {
        if self.0.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[async_trait]
impl<S: Sql> EventStore for SqlStore<S> {
    fn kind(&self) -> &'static str {
        "sql"
    }

    async fn append(
        &self,
        events: Vec<NewEvent>,
        expect: &[(String, u64)],
    ) -> Result<Vec<RecordedEvent>, StoreError> {
        let _writer = self.writer.lock().await;
        let mut heads = std::collections::HashMap::new();
        let streams = expect
            .iter()
            .map(|(s, _)| s.clone())
            .chain(events.iter().map(|e| e.body.stream.clone()));
        for stream in streams {
            if let std::collections::hash_map::Entry::Vacant(v) = heads.entry(stream) {
                let head = self.stream_head(v.key()).await?;
                v.insert(head);
            }
        }
        for (stream, expected) in expect {
            let actual = heads[stream].0;
            if actual != *expected {
                return Err(StoreError::Conflict {
                    stream: stream.clone(),
                    expected: *expected,
                    actual,
                });
            }
        }
        let mut statements = Vec::new();
        let mut inserts = Vec::new();
        let mut rows = BatchRows::default();
        let mut placed = Vec::with_capacity(events.len());
        for e in &events {
            let head = heads.get_mut(&e.body.stream).expect("read above");
            let (version, hash) = (head.0, chain_hash(head.1.as_deref(), &e.jws));
            let (jws, batch) = Self::jws_columns(&mut rows, e, &mut statements);
            inserts.push(statements.len());
            statements.push((
                sql::INSERT_EVENT,
                event_params(e, version, &hash, &jws, batch),
            ));
            *head = (version + 1, Some(hash.clone()));
            placed.push((version, hash));
        }
        let results = self.sql.batch(statements).await?;
        let mut out = Vec::with_capacity(events.len());
        for ((e, (version, hash)), i) in events.into_iter().zip(placed).zip(inserts) {
            let position = results
                .get(i)
                .and_then(|r| r.first())
                .and_then(|r| int(r, "position"))
                .ok_or_else(|| StoreError::Backend("an insert returned no position".into()))?;
            out.push(RecordedEvent {
                position,
                version,
                hash,
                jws: e.jws,
                jws_index: e.jws_index,
                body: e.body,
            });
        }
        if let Some(last) = out.last() {
            self.woke(last.position);
        }
        Ok(out)
    }

    async fn read(&self, q: &Query) -> Result<Vec<RecordedEvent>, StoreError> {
        let rows = self
            .sql
            .all(
                sql::READ,
                vec![
                    q.after.into(),
                    q.stream.clone().into(),
                    sql::json_list(&q.types).into(),
                    (q.limit as u64).into(),
                    sql::json_list(&q.exclude).into(),
                ],
            )
            .await?;
        let mut batches = BatchDecoder::default();
        rows.iter().map(|row| decode(row, &mut batches)).collect()
    }

    async fn head(&self) -> Result<u64, StoreError> {
        Ok(scalar(&self.sql.all(sql::HEAD, vec![]).await?))
    }

    async fn put_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError> {
        self.sql
            .all(
                sql::PUT_BLOB,
                vec![sha256.into(), Param::Bytes(bytes.to_vec())],
            )
            .await?;
        Ok(())
    }

    async fn blob(&self, sha256: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let rows = self.sql.all(sql::BLOB, vec![sha256.into()]).await?;
        Ok(rows.first().and_then(|r| r.get("bytes")).and_then(bytes))
    }

    async fn load_checkpoint(&self, name: &str) -> Result<Option<u64>, StoreError> {
        let rows = self
            .sql
            .all(sql::LOAD_CHECKPOINT, vec![name.into()])
            .await?;
        Ok(rows.first().and_then(|r| int(r, "position")))
    }

    async fn save_checkpoint(&self, name: &str, position: u64) -> Result<(), StoreError> {
        self.sql
            .all(sql::SAVE_CHECKPOINT, vec![name.into(), position.into()])
            .await?;
        Ok(())
    }

    async fn wake(&self) -> Result<Box<dyn Wake>, StoreError> {
        Ok(Box::new(SqlWake(self.head.subscribe())))
    }

    async fn import(&self, events: &[RecordedEvent]) -> Result<(), StoreError> {
        let Some(last) = events.last().map(|e| e.position) else {
            return Ok(());
        };
        let _writer = self.writer.lock().await;
        let mut check = ImportCheck::new(self.head().await?);
        let mut statements = Vec::new();
        let mut rows = BatchRows::default();
        for e in events {
            if !check.knows(&e.body.stream) {
                let (next, hash) = self.stream_head(&e.body.stream).await?;
                check.seed(&e.body.stream, next, hash);
            }
            check.check(e)?;
            let new = NewEvent {
                jws: Arc::clone(&e.jws),
                jws_index: e.jws_index,
                body: e.body.clone(),
            };
            let (jws, batch) = Self::jws_columns(&mut rows, &new, &mut statements);
            let mut params = vec![e.position.into()];
            params.extend(event_params(&new, e.version, &e.hash, &jws, batch));
            statements.push((sql::IMPORT_EVENT, params));
        }
        self.sql.batch(statements).await?;
        self.woke(last);
        Ok(())
    }

    async fn blob_names(&self) -> Result<Vec<String>, StoreError> {
        let rows = self.sql.all(sql::BLOB_NAMES, vec![]).await?;
        Ok(rows
            .iter()
            .filter_map(|r| text(r, "sha256").map(str::to_string))
            .collect())
    }

    async fn checkpoints(&self) -> Result<Vec<(String, u64)>, StoreError> {
        let rows = self.sql.all(sql::CHECKPOINTS, vec![]).await?;
        Ok(rows
            .iter()
            .filter_map(|r| Some((text(r, "name")?.to_string(), int(r, "position")?)))
            .collect())
    }

    /// One atomic batch: every add, then the cut.
    async fn add_usage(&self, rows: &[UsageRow], keep_from: &str) -> Result<(), StoreError> {
        let mut statements: Vec<Statement> = rows
            .iter()
            .map(|r| {
                (
                    sql::ADD_USAGE,
                    vec![
                        r.day.as_str().into(),
                        r.account.as_str().into(),
                        r.metric.as_str().into(),
                        Param::Int(r.n),
                    ],
                )
            })
            .collect();
        statements.push((sql::PRUNE_USAGE, vec![keep_from.into()]));
        self.sql.batch(statements).await?;
        Ok(())
    }

    async fn usage(&self) -> Result<Vec<UsageRow>, StoreError> {
        let rows = self.sql.all(sql::USAGE, vec![]).await?;
        Ok(rows
            .iter()
            .filter_map(|r| {
                Some(UsageRow {
                    day: text(r, "day")?.to_string(),
                    account: text(r, "account")?.to_string(),
                    metric: text(r, "metric")?.to_string(),
                    n: signed(r, "n")?,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_server::sqlite::SqliteStore;
    use live_server::store::conformance;
    use sqlx::sqlite::{SqlitePool, SqlitePoolOptions, SqliteRow};
    use sqlx::{Column, Row as _, TypeInfo, ValueRef};

    /// The trait over sqlx's SQLite: rows come back as JavaScript hands them (JSON
    /// values by column name, blobs as byte arrays), and a batch is one
    /// transaction.
    struct Sqlite(SqlitePool);

    fn bind<'q>(
        mut q: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
        params: Vec<Param>,
    ) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
        for p in params {
            q = match p {
                Param::Null => q.bind(None::<i64>),
                Param::Int(n) => q.bind(n),
                Param::Text(s) => q.bind(s),
                Param::Bytes(b) => q.bind(b),
            };
        }
        q
    }

    fn row(r: &SqliteRow) -> Row {
        let mut out = Row::new();
        for (i, c) in r.columns().iter().enumerate() {
            let raw = r.try_get_raw(i).unwrap();
            let v = if raw.is_null() {
                Value::Null
            } else {
                match raw.type_info().name() {
                    "INTEGER" => Value::from(r.get::<i64, _>(i)),
                    // As JavaScript: a float that happens to be whole.
                    "REAL" => Value::from(r.get::<f64, _>(i)),
                    "BLOB" => Value::from(r.get::<Vec<u8>, _>(i)),
                    _ => Value::from(r.get::<String, _>(i)),
                }
            };
            out.insert(c.name().to_string(), v);
        }
        out
    }

    fn db(e: sqlx::Error) -> StoreError {
        StoreError::Backend(e.to_string())
    }

    #[async_trait]
    impl Sql for Sqlite {
        async fn all(&self, sql: &str, params: Vec<Param>) -> Result<Vec<Row>, StoreError> {
            let rows = bind(sqlx::query(sql), params)
                .fetch_all(&self.0)
                .await
                .map_err(db)?;
            Ok(rows.iter().map(row).collect())
        }

        async fn batch(&self, statements: Vec<Statement>) -> Result<Vec<Vec<Row>>, StoreError> {
            let mut tx = self.0.begin().await.map_err(db)?;
            let mut out = Vec::new();
            for (sql, params) in statements {
                let rows = bind(sqlx::query(sql), params)
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(db)?;
                out.push(rows.iter().map(row).collect());
            }
            tx.commit().await.map_err(db)?;
            Ok(out)
        }

        async fn script(&self, sql: &str) -> Result<(), StoreError> {
            sqlx::raw_sql(sql).execute(&self.0).await.map_err(db)?;
            Ok(())
        }
    }

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "live-worker-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("live.db")
    }

    async fn pool(path: &std::path::Path) -> SqlitePool {
        let url = format!("sqlite://{}?mode=rwc", path.display());
        SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&url)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_sql_store_conforms() {
        let path = temp_db("conforms");
        let store = SqlStore::open(Sqlite(pool(&path).await)).await.unwrap();
        conformance::run(&store, "sql").await;
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The rows are the SQLite store's own: a log the Worker wrote opens
    /// on a machine (and the other way), which is how a log moves between
    /// them (`h2 live store copy`, docs/live/live-worker.md).
    #[tokio::test]
    async fn its_rows_are_the_sqlite_store_s() {
        let key = live_proto::sign::KeyPair::generate();
        let path = temp_db("shared");
        let store = SqlStore::open(Sqlite(pool(&path).await)).await.unwrap();
        let (jws, bodies) = conformance::batch(&key, "game-1", 3);
        let batch: Vec<NewEvent> = bodies
            .into_iter()
            .enumerate()
            .map(|(i, body)| NewEvent {
                jws: Arc::clone(&jws),
                jws_index: Some(i),
                body,
            })
            .collect();
        let mut written = store
            .append(vec![conformance::event(&key, "acct-1", 0)], &[])
            .await
            .unwrap();
        written.extend(store.append(batch, &[("game-1".into(), 0)]).await.unwrap());
        store.save_checkpoint("stats", 4).await.unwrap();
        drop(store);

        let sqlite = SqliteStore::open(&path).await.unwrap();
        let all = sqlite.read(&Query::after(0, 100)).await.unwrap();
        assert_eq!(all, written);
        assert_eq!(sqlite.load_checkpoint("stats").await.unwrap(), Some(4));
        let more = sqlite
            .append(
                vec![conformance::event(&key, "acct-1", 1)],
                &[("acct-1".into(), 1)],
            )
            .await
            .unwrap();
        drop(sqlite);

        let store = SqlStore::open(Sqlite(pool(&path).await)).await.unwrap();
        assert_eq!(store.head().await.unwrap(), more[0].position);
        let tail = store
            .read(&Query::after(written.len() as u64, 10))
            .await
            .unwrap();
        assert_eq!(tail, more);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// `GET /v1/rank-stats` on the Worker's own composition: the SQL
    /// store, title files through `BlobsElsewhere`, driven timers, the
    /// shared router. No hopper file is published, so the default one
    /// names the ranked leaderboards (docs/live/live-matchmaking.md §8.3).
    #[tokio::test]
    async fn the_games_rank_read_answers_on_the_worker() {
        use axum::{body::Body, http::Request};
        use http_body_util::BodyExt;
        use live_server::{
            Live, Policy, ServerIdentity,
            app::Timers,
            blobs::{BlobStore, Blobs},
            titleblobs::BlobsElsewhere,
        };
        use tower::ServiceExt;

        let path = temp_db("rank-stats");
        let store = SqlStore::open(Sqlite(pool(&path).await)).await.unwrap();
        let private = BlobStore::memory();
        let store = BlobsElsewhere::new(Arc::new(store), private.clone());
        let identity = ServerIdentity::open(path.parent().unwrap(), Some("live.test")).unwrap();
        let live = Live::start_with(Arc::new(store), identity, Policy::default(), Timers::Driven)
            .await
            .unwrap();
        live.set_blobs(Blobs {
            private,
            public: None,
        });
        let get = |uri: &str| {
            let live = live.clone();
            let req = Request::get(uri).body(Body::empty()).unwrap();
            async move {
                let r = live_server::http::router(live).oneshot(req).await.unwrap();
                let status = r.status();
                let body = r.into_body().collect().await.unwrap().to_bytes();
                (
                    status,
                    serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null),
                )
            }
        };
        let (status, body) = get("/v1/rank-stats?leaderboards=99,2,1&xuids=0009000000000001").await;
        assert_eq!(status, 200, "{body}");
        let rows: Vec<(u64, &str, u64)> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["leaderboard"].as_u64().unwrap(),
                    i["playlist"].as_str().unwrap(),
                    i["standing"]["level"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(rows, [(1, "0", 1), (2, "1", 1)]);
        let (status, _) = get("/v1/rank-stats?leaderboards=1&xuids=nope").await;
        assert_eq!(status, 400);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A host's own Clankers on the Worker's composition, as on a machine
    /// (live-server's `tests/bots.rs`, docs/game/bots.md §6): `bot: true`
    /// players with no xuid are taken at the start and on a join, the game
    /// record keeps them, and the stats summary gives them no row and
    /// credits nobody's row with a kill or death against them.
    #[tokio::test]
    async fn a_hosts_own_clankers_are_game_only_bots_on_the_worker() {
        use axum::{body::Body, http::Request};
        use http_body_util::BodyExt;
        use live_proto::sign::{
            AccountRequest, EventBody, KeyPair, format_unix, issue_token, parse_time, sign_batch,
        };
        use live_proto::{Xuid, lxm};
        use live_server::{Live, Policy, ServerIdentity, app::Timers};
        use tower::ServiceExt;

        let path = temp_db("clankers");
        let store = SqlStore::open(Sqlite(pool(&path).await)).await.unwrap();
        let identity = ServerIdentity::open(path.parent().unwrap(), Some("live.test")).unwrap();
        let live = Live::start_with(Arc::new(store), identity, Policy::default(), Timers::Driven)
            .await
            .unwrap();
        let call = |method: &'static str, uri: String, auth: Option<String>, body: String| {
            let live = live.clone();
            async move {
                let mut req = Request::builder().method(method).uri(uri);
                if let Some(a) = auth {
                    req = req.header("authorization", format!("Bearer {a}"));
                }
                let r = live_server::http::router(live)
                    .oneshot(req.body(Body::from(body)).unwrap())
                    .await
                    .unwrap();
                let status = r.status().as_u16();
                let body = r.into_body().collect().await.unwrap().to_bytes();
                (
                    status,
                    serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null),
                )
            }
        };
        let now = live_server::app::now;
        let key = KeyPair::generate();
        let (status, created) = call(
            "POST",
            "/v1/accounts".into(),
            None,
            AccountRequest::sign(&key, "Host", None, "live.test", now()),
        )
        .await;
        assert_eq!(status, 201, "{created}");
        let xuid = Xuid::parse(created["xuid"].as_str().unwrap()).unwrap();
        let author = created["id"].as_str().unwrap().to_string();
        let token = || issue_token(&key, &author, "live.test", lxm::GAMES_WRITE, now());
        let clanker = |index: u32| serde_json::json!({"index": index, "name": format!("Clanker {index}"), "team": 1, "local": true, "bot": true});
        let game = serde_json::json!({
            "console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Slayer",
            "team_play": false, "tick": 0, "kind": "custom",
            "players": [
                {"index": 0, "name": "Host", "xuid": xuid, "team": 0, "local": true},
                clanker(1)
            ]
        });
        let (status, started) =
            call("POST", "/v1/games".into(), Some(token()), game.to_string()).await;
        assert_eq!(status, 201, "{started}");
        let id = started["game_id"].as_str().unwrap().to_string();
        let at = parse_time(&format_unix(now())).unwrap();
        let bodies: Vec<EventBody> = [
            ("PlayerJoined", serde_json::json!({"player": clanker(2)})),
            (
                "PlayerKilled",
                serde_json::json!({"tick": 30, "victim": 1, "killer": 0, "kind": "kill"}),
            ),
            (
                "PlayerKilled",
                serde_json::json!({"tick": 60, "victim": 0, "killer": 2, "kind": "kill"}),
            ),
            (
                "GameEnded",
                serde_json::json!({"tick": 900, "reason": "finished", "winner": {"player": 0}}),
            ),
        ]
        .into_iter()
        .map(|(kind, data)| EventBody::new(&key, &author, &format!("game-{id}"), kind, data, at))
        .collect();
        let (status, body) = call(
            "POST",
            format!("/v1/games/{id}/events"),
            Some(token()),
            sign_batch(&key, &bodies),
        )
        .await;
        assert_eq!(status, 201, "{body}");

        let (_, g) = call("GET", format!("/v1/games/{id}"), None, String::new()).await;
        let players = g["players"].as_array().unwrap();
        assert_eq!(players.len(), 3, "{g}");
        assert!(
            players[1..]
                .iter()
                .all(|p| p["bot"] == true && p["xuid"].is_null())
        );
        let (_, v) = call(
            "GET",
            "/v1/stats/summary?window=all".into(),
            None,
            String::new(),
        )
        .await;
        assert_eq!(v["totals"]["kills"], 2);
        let rows = v["players"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(
            (rows[0]["kills"].as_u64(), rows[0]["deaths"].as_u64()),
            (Some(0), Some(0))
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Bot accounts on the Worker's composition, as on a machine
    /// (live-server's `tests/bots.rs`, docs/game/bots.md §9.5): a roster
    /// for the host's session issues bot accounts that read by gamertag
    /// with `bot: true` and stay out of the accounts list.
    #[tokio::test]
    async fn rostered_bots_read_as_bot_accounts_on_the_worker() {
        use axum::{body::Body, http::Request};
        use http_body_util::BodyExt;
        use live_proto::lxm;
        use live_proto::sign::{AccountRequest, KeyPair, issue_token};
        use live_server::{Live, Policy, ServerIdentity, app::Timers};
        use tower::ServiceExt;

        let path = temp_db("bot-accounts");
        let store = SqlStore::open(Sqlite(pool(&path).await)).await.unwrap();
        let identity = ServerIdentity::open(path.parent().unwrap(), Some("live.test")).unwrap();
        let live = Live::start_with(Arc::new(store), identity, Policy::default(), Timers::Driven)
            .await
            .unwrap();
        let call = |method: &'static str, uri: String, auth: Option<String>, body: String| {
            let live = live.clone();
            async move {
                let mut req = Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json");
                if let Some(a) = auth {
                    req = req.header("authorization", format!("Bearer {a}"));
                }
                let r = live_server::http::router(live)
                    .oneshot(req.body(Body::from(body)).unwrap())
                    .await
                    .unwrap();
                let status = r.status().as_u16();
                let body = r.into_body().collect().await.unwrap().to_bytes();
                (
                    status,
                    serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null),
                )
            }
        };
        let now = live_server::app::now;
        let key = KeyPair::generate();
        let (status, created) = call(
            "POST",
            "/v1/accounts".into(),
            None,
            AccountRequest::sign(&key, "Host", None, "live.test", now()),
        )
        .await;
        assert_eq!(status, 201, "{created}");
        let author = created["id"].as_str().unwrap().to_string();
        let token = |m: &str| issue_token(&key, &author, "live.test", m, now());
        let lobby = serde_json::to_value(live_proto::session::NewSession {
            addr: Some("192.168.2.4".parse().unwrap()),
            port: 3074,
            host_address: vec![0; 36],
            slots: live_proto::session::Slots {
                public_filled: 1,
                public_open: 7,
                ..Default::default()
            },
            attributes: Default::default(),
            flags: "custom".into(),
            hopper_id: None,
            map: Some("lockout".into()),
            name: Some("Bots".into()),
            language: None,
            visibility: Default::default(),
            map_id: None,
            skulls: None,
            campaign: None,
            ping_ms: None,
            variant_ext: None,
        })
        .unwrap();
        let (status, session) = call(
            "POST",
            "/v1/sessions".into(),
            Some(token(lxm::SESSIONS_WRITE)),
            lobby.to_string(),
        )
        .await;
        assert_eq!(status, 201, "{session}");
        let session_id = session["session_id"].as_str().unwrap();
        let (status, roster) = call(
            "POST",
            "/v1/games/bots".into(),
            Some(token(lxm::GAMES_WRITE)),
            serde_json::json!({"session_id": session_id, "count": 1}).to_string(),
        )
        .await;
        assert_eq!(status, 201, "{roster}");
        let bot = &roster["bots"][0];
        let tag = bot["gamertag"].as_str().unwrap();
        let (status, v) = call("GET", format!("/v1/accounts/{}", tag.replace(' ', "%20")), None, String::new()).await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["bot"], true);
        assert_eq!(v["xuid"], bot["xuid"]);
        let (_, list) = call("GET", "/v1/accounts".into(), None, String::new()).await;
        let items = list["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        assert_eq!(items[0]["gamertag"], "Host");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Signing a device out on the Worker's own composition: the revoked
    /// key's next request gets 401, and so it does after the hub restarts
    /// and rebuilds from its log (docs/live/live-api.md "Sign-ins").
    #[tokio::test]
    async fn a_signed_out_key_is_refused_on_the_worker() {
        use axum::{body::Body, http::Request};
        use http_body_util::BodyExt;
        use live_proto::recovery::{RecoverRequest, TotpSecret, format_code};
        use live_proto::sign::{AccountRequest, KeyPair, issue_token};
        use live_proto::{Xuid, lxm};
        use live_server::{Live, Policy, ServerIdentity, app::Timers};
        use tower::ServiceExt;

        let path = temp_db("signins");
        let dir = path.parent().unwrap().to_path_buf();
        let boot = || async {
            let store = SqlStore::open(Sqlite(pool(&path).await)).await.unwrap();
            let identity = ServerIdentity::open(&dir, Some("live.test")).unwrap();
            Live::start_with(Arc::new(store), identity, Policy::default(), Timers::Driven)
                .await
                .unwrap()
        };
        let call = |live: Arc<Live>,
                    method: &'static str,
                    uri: String,
                    auth: Option<String>,
                    ua: &'static str,
                    body: String| async move {
            let mut req = Request::builder()
                .method(method)
                .uri(uri)
                .header("user-agent", ua);
            if let Some(a) = auth {
                req = req.header("authorization", format!("Bearer {a}"));
            }
            let r = live_server::http::router(live)
                .oneshot(req.body(Body::from(body)).unwrap())
                .await
                .unwrap();
            let status = r.status().as_u16();
            let body = r.into_body().collect().await.unwrap().to_bytes();
            (
                status,
                serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null),
            )
        };
        let now = || {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
        };
        let browser = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Firefox/140.0";

        let live = boot().await;
        let laptop = KeyPair::generate();
        let (status, created) = call(
            live.clone(),
            "POST",
            "/v1/accounts".into(),
            None,
            browser,
            AccountRequest::sign(&laptop, "Drifter", None, "live.test", now()),
        )
        .await;
        assert_eq!(status, 201, "{created}");
        let xuid = Xuid::parse(created["xuid"].as_str().unwrap()).unwrap();
        let iss = created["id"].as_str().unwrap().to_string();
        let secret =
            TotpSecret::from_base32(created["recovery"]["secret"].as_str().unwrap()).unwrap();
        let game = KeyPair::generate();
        let recover = serde_json::json!({
            "request": RecoverRequest::sign(&game, "Drifter", "live.test", now()),
            "code": format_code(secret.code_at(now())),
        });
        let (status, body) = call(
            live.clone(),
            "POST",
            "/v1/accounts/Drifter/recover".into(),
            None,
            "ureq/2.12.1",
            recover.to_string(),
        )
        .await;
        assert_eq!(status, 201, "{body}");

        let token = |key: &KeyPair, m: &str| issue_token(key, &iss, "live.test", m, now());
        let signins = format!("/v1/accounts/by-xuid/{xuid}/signins");
        let (status, body) = call(
            live.clone(),
            "GET",
            signins.clone(),
            Some(token(&game, lxm::SIGNINS_READ)),
            "ureq/2.12.1",
            String::new(),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        let kinds: Vec<&str> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["game", "browser"], "{body}");

        let (status, body) = call(
            live.clone(),
            "DELETE",
            format!("{signins}/{}", game.kid()),
            Some(token(&laptop, lxm::SIGNINS_WRITE)),
            browser,
            String::new(),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        let (status, _) = call(
            live.clone(),
            "GET",
            signins.clone(),
            Some(token(&game, lxm::SIGNINS_READ)),
            "ureq/2.12.1",
            String::new(),
        )
        .await;
        assert_eq!(status, 401);
        let (status, _) = call(
            live.clone(),
            "GET",
            signins.clone(),
            Some(token(&laptop, lxm::SIGNINS_READ)),
            browser,
            String::new(),
        )
        .await;
        assert_eq!(status, 200);

        // The hub wakes again, its read model rebuilt from its own log.
        drop(live);
        let live = boot().await;
        let (status, _) = call(
            live.clone(),
            "GET",
            signins.clone(),
            Some(token(&game, lxm::SIGNINS_READ)),
            "ureq/2.12.1",
            String::new(),
        )
        .await;
        assert_eq!(status, 401);
        let (status, body) = call(
            live.clone(),
            "GET",
            signins,
            Some(token(&laptop, lxm::SIGNINS_READ)),
            browser,
            String::new(),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["items"].as_array().unwrap().len(), 1);
        assert_eq!(body["items"][0]["name"], "Firefox on Windows");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_schema_splits_into_its_statements() {
        let s = script_statements(sql::SCHEMA);
        assert_eq!(s.len(), 6, "{s:#?}");
        assert!(s[5].starts_with("create table if not exists usage ("));
        assert!(s[0].starts_with("create table if not exists events ("));
        assert!(s[1].starts_with("create index if not exists events_type_position"));
        assert!(s.iter().all(|s| !s.contains("--") && !s.contains('\n')));
    }

    #[test]
    fn javascript_numbers_read_as_integers() {
        let mut r = Row::new();
        r.insert("position".into(), Value::from(42.0));
        r.insert("version".into(), Value::from(3));
        r.insert("bad".into(), Value::from(1.5));
        assert_eq!(int(&r, "position"), Some(42));
        assert_eq!(int(&r, "version"), Some(3));
        assert_eq!(int(&r, "bad"), None);
        assert_eq!(
            bytes(&Value::from(vec![1u8, 2, 255])),
            Some(vec![1, 2, 255])
        );
    }
}
