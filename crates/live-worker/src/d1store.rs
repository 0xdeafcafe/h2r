//! The event log in D1 (Cloudflare's SQLite): the same tables and
//! statements as the SQLite store on a machine (`live_server::sql`), over
//! any [`Sql`] connection. In the Worker that is the D1 binding; in the
//! tests here it is SQLite through sqlx, so the store is tested against
//! the shared store tests and the machine's own SQLite store.
//!
//! D1 has no interactive transactions, only `batch`, which runs its
//! statements atomically. An append reads the streams' heads, checks them,
//! then writes every row in one batch. That is safe only with one writer:
//! the log's Durable Object, which holds this store and appends one call
//! at a time (`writer`).

use std::sync::Arc;

use async_trait::async_trait;
use live_proto::sign::chain_hash;
use live_server::sql;
use live_server::store::{
    BatchDecoder, BatchRows, EventStore, ImportCheck, NewEvent, Query, RecordedEvent, StoreError,
    Wake,
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

/// A row, by column name. Integers may come back as JSON floats (D1 hands
/// JavaScript numbers) and blobs as arrays of byte values.
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

/// A script's statements, one by one, without its comments: D1 prepares
/// one statement at a time (its `exec` wants each on one line).
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

pub struct D1Store<S> {
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

impl<S: Sql> D1Store<S> {
    /// Creates the tables if missing and reads the head.
    pub async fn open(sql: S) -> Result<Self, StoreError> {
        sql.script(sql::SCHEMA).await?;
        let head = scalar(&sql.all(sql::HEAD, vec![]).await?);
        Ok(D1Store {
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

struct D1Wake(watch::Receiver<u64>);

#[async_trait]
impl Wake for D1Wake {
    async fn wait(&mut self) {
        if self.0.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[async_trait]
impl<S: Sql> EventStore for D1Store<S> {
    fn kind(&self) -> &'static str {
        "d1"
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
        Ok(Box::new(D1Wake(self.head.subscribe())))
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_server::sqlite::SqliteStore;
    use live_server::store::conformance;
    use sqlx::sqlite::{SqlitePool, SqlitePoolOptions, SqliteRow};
    use sqlx::{Column, Row as _, TypeInfo, ValueRef};

    /// The D1 interface over SQLite: rows come back as D1 hands them (JSON
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
                    // As D1: a float that happens to be whole.
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
    async fn the_d1_store_conforms() {
        let path = temp_db("conforms");
        let store = D1Store::open(Sqlite(pool(&path).await)).await.unwrap();
        conformance::run(&store, "d1").await;
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The rows are the SQLite store's own: a log the Worker wrote opens
    /// on a machine (and the other way), which is how a log moves between
    /// them (`h2 live store copy`, docs/live/live-worker.md).
    #[tokio::test]
    async fn its_rows_are_the_sqlite_store_s() {
        let key = live_proto::sign::KeyPair::generate();
        let path = temp_db("shared");
        let d1 = D1Store::open(Sqlite(pool(&path).await)).await.unwrap();
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
        let mut written = d1
            .append(vec![conformance::event(&key, "acct-1", 0)], &[])
            .await
            .unwrap();
        written.extend(d1.append(batch, &[("game-1".into(), 0)]).await.unwrap());
        d1.save_checkpoint("stats", 4).await.unwrap();
        drop(d1);

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

        let d1 = D1Store::open(Sqlite(pool(&path).await)).await.unwrap();
        assert_eq!(d1.head().await.unwrap(), more[0].position);
        let tail = d1
            .read(&Query::after(written.len() as u64, 10))
            .await
            .unwrap();
        assert_eq!(tail, more);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_schema_splits_into_its_statements() {
        let s = script_statements(sql::SCHEMA);
        assert_eq!(s.len(), 5, "{s:#?}");
        assert!(s[0].starts_with("create table if not exists events ("));
        assert!(s[1].starts_with("create index if not exists events_type_position"));
        assert!(s.iter().all(|s| !s.contains("--") && !s.contains('\n')));
    }

    #[test]
    fn d1_s_numbers_read_as_integers() {
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
