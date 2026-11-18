//! The Postgres event store. Runtime-checked queries, so the crate builds
//! without a database.
//!
//! Appends hold `pg_advisory_xact_lock` on one key for their whole
//! transaction, so `bigserial` positions are handed out in commit order:
//! without it two writers could take 11 and 12, commit 12 first, and a
//! tailer that read 12 would never go back for 11. That serialises writes,
//! which is fine at this service's scale. The same transaction sends
//! `pg_notify('events', <position>)`, so a commit wakes the dispatcher.

use std::collections::HashMap;

use async_trait::async_trait;
use live_proto::sign::chain_hash;
use sqlx::Row;
use sqlx::postgres::{PgListener, PgPool, PgPoolOptions, PgRow};

use crate::store::{
    BatchDecoder, BatchRows, EventStore, ImportCheck, NewEvent, Query, RecordedEvent, StoreError,
    UsageRow, Wake,
};

const SCHEMA: &str = include_str!("schema.sql");
/// Any constant works; it only has to be ours.
const APPEND_LOCK: i64 = 0x6832_6c69_7665;
const SCHEMA_LOCK: i64 = 0x6832_6c69_7666;
pub const EVENTS_CHANNEL: &str = "events";

pub struct PostgresStore {
    pool: PgPool,
}

impl PostgresStore {
    /// Connects and creates the schema if it is missing.
    pub async fn connect(url: &str) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(url)
            .await?;
        // `create ... if not exists` races with itself; serialise it.
        let mut tx = pool.begin().await?;
        sqlx::query("select pg_advisory_xact_lock($1)")
            .bind(SCHEMA_LOCK)
            .execute(&mut *tx)
            .await?;
        sqlx::raw_sql(SCHEMA).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(PostgresStore { pool })
    }
}

impl PostgresStore {
    /// Connects for reading only, the source of a copy: no schema is
    /// created, and every transaction is read-only
    /// (`default_transaction_read_only`), so nothing is written to the
    /// database however it is used.
    pub async fn connect_read_only(url: &str) -> Result<Self, sqlx::Error> {
        let opts: sqlx::postgres::PgConnectOptions = url.parse()?;
        let opts = opts.options([("default_transaction_read_only", "on")]);
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect_with(opts)
            .await?;
        Ok(PostgresStore { pool })
    }

    /// [`PostgresStore::connect`], creating the database first if the
    /// server is up but the database does not exist yet.
    pub async fn connect_creating(url: &str) -> Result<Self, sqlx::Error> {
        match Self::connect(url).await {
            Err(e) if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("3D000") => {
                let opts: sqlx::postgres::PgConnectOptions = url.parse()?;
                let name = opts.get_database().unwrap_or("h2live").to_string();
                if !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                {
                    return Err(e);
                }
                let admin = PgPoolOptions::new()
                    .max_connections(1)
                    .connect_with(opts.database("postgres"))
                    .await?;
                // `create database` takes no parameters; the name is checked above.
                sqlx::raw_sql(&format!("create database \"{name}\""))
                    .execute(&admin)
                    .await?;
                admin.close().await;
                Self::connect(url).await
            }
            r => r,
        }
    }
}

/// The head of the log in the Postgres database at `url`, if it exists,
/// is reachable within a couple of seconds, and has an `events` table;
/// nothing is created. For telling an upgraded server where its log is.
pub async fn existing_log(url: &str) -> Option<u64> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(2))
        .connect(url)
        .await
        .ok()?;
    let head: Option<i64> = sqlx::query_scalar("select coalesce(max(position), 0) from events")
        .fetch_one(&pool)
        .await
        .ok();
    pool.close().await;
    head.map(|h| h as u64)
}

fn to_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

/// Stores `jws`: a single event's in its row; a batch's in `batches`,
/// once, its rows naming it. Returns the row's `jws` and `batch` columns.
async fn put_jws<'e>(
    tx: &mut sqlx::PgConnection,
    rows: &mut BatchRows,
    jws: &'e std::sync::Arc<str>,
    jws_index: Option<usize>,
) -> Result<(&'e str, Option<String>), StoreError> {
    let Some((key, first)) = rows.key(jws, jws_index) else {
        return Ok((jws, None));
    };
    if first {
        sqlx::query(
            "insert into batches (sha256, jws) values ($1, $2) on conflict (sha256) do nothing",
        )
        .bind(&key)
        .bind(&**jws)
        .execute(&mut *tx)
        .await?;
    }
    Ok(("", Some(key)))
}

fn decode(row: &PgRow, batches: &mut BatchDecoder) -> Result<RecordedEvent, StoreError> {
    let position = row.get::<i64, _>("position") as u64;
    let index = row.get::<Option<i32>, _>("jws_index").map(|i| i as usize);
    let e = batches
        .decode(row.get::<&str, _>("jws"), index)
        .map_err(|e| StoreError::Corrupt {
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

#[async_trait]
impl EventStore for PostgresStore {
    fn kind(&self) -> &'static str {
        "postgres"
    }

    async fn append(
        &self,
        events: Vec<NewEvent>,
        expect: &[(String, u64)],
    ) -> Result<Vec<RecordedEvent>, StoreError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("select pg_advisory_xact_lock($1)")
            .bind(APPEND_LOCK)
            .execute(&mut *tx)
            .await?;

        // stream -> (next version, last hash), read once per stream.
        let mut heads: HashMap<String, (u64, Option<String>)> = HashMap::new();
        let streams = expect
            .iter()
            .map(|(s, _)| s)
            .chain(events.iter().map(|e| &e.body.stream));
        for stream in streams {
            if heads.contains_key(stream) {
                continue;
            }
            let row = sqlx::query(
                "select version, hash from events where stream_id = $1 order by version desc limit 1",
            )
            .bind(stream)
            .fetch_optional(&mut *tx)
            .await?;
            let head = row.map_or((0, None), |r| {
                (r.get::<i64, _>("version") as u64 + 1, Some(r.get("hash")))
            });
            heads.insert(stream.clone(), head);
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

        let mut out = Vec::with_capacity(events.len());
        let mut rows = BatchRows::default();
        for e in events {
            let head = heads.get_mut(&e.body.stream).expect("read above");
            let (version, hash) = (head.0, chain_hash(head.1.as_deref(), &e.jws));
            let (jws, batch) = put_jws(&mut tx, &mut rows, &e.jws, e.jws_index).await?;
            let b = &e.body;
            let position: i64 = sqlx::query_scalar(
                "insert into events (stream_id, version, type, data, at, author, kid, jws, jws_index, hash, batch)
                 values ($1, $2, $3, $4, $5::timestamptz, $6, $7, $8, $9, $10, $11)
                 returning position",
            )
            .bind(&b.stream)
            .bind(to_i64(version))
            .bind(&b.kind)
            .bind(&b.data)
            .bind(&b.at)
            .bind(&b.author)
            .bind(&b.kid)
            .bind(jws)
            .bind(e.jws_index.map(|i| i as i32))
            .bind(&hash)
            .bind(batch)
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
        if let Some(last) = out.last() {
            sqlx::query("select pg_notify($1, $2)")
                .bind(EVENTS_CHANNEL)
                .bind(last.position.to_string())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(out)
    }

    async fn read(&self, q: &Query) -> Result<Vec<RecordedEvent>, StoreError> {
        let rows = sqlx::query(
            "select e.position, e.version, e.hash, coalesce(b.jws, e.jws) as jws, e.jws_index
             from events e left join batches b on b.sha256 = e.batch
             where e.position > $1
               and ($2::text is null or e.stream_id = $2)
               and (cardinality($3::text[]) = 0 or e.type = any($3))
               and not (e.type = any($5))
             order by e.position limit $4",
        )
        .bind(to_i64(q.after))
        .bind(&q.stream)
        .bind(&q.types)
        .bind(to_i64(q.limit as u64))
        .bind(&q.exclude)
        .fetch_all(&self.pool)
        .await?;
        let mut batches = BatchDecoder::default();
        rows.iter().map(|row| decode(row, &mut batches)).collect()
    }

    async fn head(&self) -> Result<u64, StoreError> {
        let p: i64 = sqlx::query_scalar("select coalesce(max(position), 0) from events")
            .fetch_one(&self.pool)
            .await?;
        Ok(p as u64)
    }

    async fn put_blob(&self, sha256: &str, bytes: &[u8]) -> Result<(), StoreError> {
        sqlx::query(
            "insert into blobs (sha256, bytes) values ($1, $2) on conflict (sha256) do nothing",
        )
        .bind(sha256)
        .bind(bytes)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn blob(&self, sha256: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(
            sqlx::query_scalar("select bytes from blobs where sha256 = $1")
                .bind(sha256)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    async fn load_checkpoint(&self, name: &str) -> Result<Option<u64>, StoreError> {
        let p: Option<i64> =
            sqlx::query_scalar("select position from subscriptions where name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?;
        Ok(p.map(|p| p as u64))
    }

    async fn save_checkpoint(&self, name: &str, position: u64) -> Result<(), StoreError> {
        sqlx::query(
            "insert into subscriptions (name, position) values ($1, $2)
             on conflict (name) do update set position = excluded.position",
        )
        .bind(name)
        .bind(to_i64(position))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn import(&self, events: &[RecordedEvent]) -> Result<(), StoreError> {
        let Some(last) = events.last() else {
            return Ok(());
        };
        let mut tx = self.pool.begin().await?;
        sqlx::query("select pg_advisory_xact_lock($1)")
            .bind(APPEND_LOCK)
            .execute(&mut *tx)
            .await?;
        let head: i64 = sqlx::query_scalar("select coalesce(max(position), 0) from events")
            .fetch_one(&mut *tx)
            .await?;
        let mut check = ImportCheck::new(head as u64);
        let mut rows = BatchRows::default();
        for e in events {
            if !check.knows(&e.body.stream) {
                let row = sqlx::query(
                    "select version, hash from events where stream_id = $1 order by version desc limit 1",
                )
                .bind(&e.body.stream)
                .fetch_optional(&mut *tx)
                .await?;
                let (next, hash) = row.map_or((0, None), |r| {
                    (r.get::<i64, _>("version") as u64 + 1, Some(r.get("hash")))
                });
                check.seed(&e.body.stream, next, hash);
            }
            check.check(e)?;
            let (jws, batch) = put_jws(&mut tx, &mut rows, &e.jws, e.jws_index).await?;
            let b = &e.body;
            sqlx::query(
                "insert into events (position, stream_id, version, type, data, at, author, kid, jws, jws_index, hash, batch)
                 values ($1, $2, $3, $4, $5, $6::timestamptz, $7, $8, $9, $10, $11, $12)",
            )
            .bind(to_i64(e.position))
            .bind(&b.stream)
            .bind(to_i64(e.version))
            .bind(&b.kind)
            .bind(&b.data)
            .bind(&b.at)
            .bind(&b.author)
            .bind(&b.kid)
            .bind(jws)
            .bind(e.jws_index.map(|i| i as i32))
            .bind(&e.hash)
            .bind(batch)
            .execute(&mut *tx)
            .await?;
        }
        // Positions given here bypass the sequence; the next append must
        // still come after them.
        sqlx::query("select setval(pg_get_serial_sequence('events', 'position'), $1)")
            .bind(to_i64(last.position))
            .execute(&mut *tx)
            .await?;
        sqlx::query("select pg_notify($1, $2)")
            .bind(EVENTS_CHANNEL)
            .bind(last.position.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
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

    async fn add_usage(&self, rows: &[UsageRow], keep_from: &str) -> Result<(), StoreError> {
        let mut tx = self.pool.begin().await?;
        for r in rows {
            sqlx::query(
                "insert into usage (day, account, metric, n) values ($1, $2, $3, $4)
                 on conflict (day, account, metric) do update set n = usage.n + excluded.n",
            )
            .bind(&r.day)
            .bind(&r.account)
            .bind(&r.metric)
            .bind(r.n)
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("delete from usage where day <> '' and day < $1")
            .bind(keep_from)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn usage(&self) -> Result<Vec<UsageRow>, StoreError> {
        let rows: Vec<(String, String, String, i64)> =
            sqlx::query_as("select day, account, metric, n from usage")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|(day, account, metric, n)| UsageRow {
                day,
                account,
                metric,
                n,
            })
            .collect())
    }

    async fn wake(&self) -> Result<Box<dyn Wake>, StoreError> {
        let mut listener = PgListener::connect_with(&self.pool).await?;
        listener.listen(EVENTS_CHANNEL).await?;
        Ok(Box::new(PgWake(listener)))
    }
}

/// Wakes on the NOTIFY appends send. A dropped connection is retried by
/// `PgListener` itself; meanwhile the dispatcher's poll covers.
struct PgWake(PgListener);

#[async_trait]
impl Wake for PgWake {
    async fn wait(&mut self) {
        if let Err(e) = self.0.recv().await {
            eprintln!("live: LISTEN failed ({e}); polling");
            tokio::time::sleep(crate::dispatch::POLL).await;
        }
    }
}

/// Integration tests against a real database: set
/// `H2_LIVE_TEST_DB=postgres://localhost/<throwaway>`; unset, they skip.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub async fn test_store() -> Option<PostgresStore> {
        let Some(url) = std::env::var("H2_LIVE_TEST_DB")
            .ok()
            .filter(|s| !s.is_empty())
        else {
            eprintln!("skipping: H2_LIVE_TEST_DB is not set");
            return None;
        };
        Some(
            PostgresStore::connect(&url)
                .await
                .expect("connecting to H2_LIVE_TEST_DB"),
        )
    }

    #[tokio::test]
    async fn postgres_store_conforms() {
        let Some(store) = test_store().await else {
            return;
        };
        let prefix = format!("pg{}-{}", std::process::id(), live_proto::sign::random_id());
        crate::store::conformance::run(&store, &prefix).await;
    }

    /// A log moved SQLite -> Postgres -> SQLite comes back the same, and
    /// Postgres's sequence carries on after the imported positions. Uses a
    /// fresh database beside `H2_LIVE_TEST_DB`'s, dropped afterwards.
    #[tokio::test]
    async fn the_log_copies_through_postgres_and_back() {
        let Some(base) = std::env::var("H2_LIVE_TEST_DB")
            .ok()
            .filter(|s| !s.is_empty())
        else {
            eprintln!("skipping: H2_LIVE_TEST_DB is not set");
            return;
        };
        let opts: sqlx::postgres::PgConnectOptions = base.parse().unwrap();
        let name = format!("h2live_copy_{}", std::process::id());
        let url = {
            let (head, _) = base.rsplit_once('/').unwrap();
            format!("{head}/{name}")
        };
        let pg = PostgresStore::connect_creating(&url).await.unwrap();
        let key = live_proto::sign::KeyPair::generate();
        let a = crate::sqlite::tests::temp_db("pg-a");
        let src = crate::sqlite::SqliteStore::open(&a).await.unwrap();
        for i in 0..6 {
            src.append(
                vec![crate::store::conformance::event(
                    &key,
                    &format!("s{}", i % 2),
                    i,
                )],
                &[],
            )
            .await
            .unwrap();
        }
        src.put_blob("t", b"title").await.unwrap();
        src.save_checkpoint("c", 3).await.unwrap();
        let r = crate::copy::copy_log(&src, &pg, |_, _| {}).await.unwrap();
        assert_eq!((r.events, r.blobs, r.checkpoints), (6, 1, 1));
        let q = Query::after(0, 1000);
        assert_eq!(pg.read(&q).await.unwrap(), src.read(&q).await.unwrap());
        let next = pg
            .append(vec![crate::store::conformance::event(&key, "s0", 9)], &[])
            .await
            .unwrap();
        assert_eq!(next[0].position, 7, "the sequence follows the import");
        assert_eq!(next[0].version, 3);

        let b = crate::sqlite::tests::temp_db("pg-b");
        let back = crate::sqlite::SqliteStore::open(&b).await.unwrap();
        crate::copy::copy_log(&pg, &back, |_, _| {}).await.unwrap();
        assert_eq!(back.read(&q).await.unwrap(), pg.read(&q).await.unwrap());
        assert_eq!(
            back.blob("t").await.unwrap().as_deref(),
            Some(&b"title"[..])
        );
        assert_eq!(pg.checkpoints().await.unwrap(), [("c".to_string(), 3)]);

        pg.pool.close().await;
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(opts.database("postgres"))
            .await
            .unwrap();
        sqlx::raw_sql(&format!("drop database \"{name}\""))
            .execute(&admin)
            .await
            .unwrap();
        for p in [a, b] {
            std::fs::remove_dir_all(p.parent().unwrap()).unwrap();
        }
    }

    /// A log written before batches were stored once (no `batch` column,
    /// a batch's JWS in each of its rows) connects: the column is added,
    /// the old rows read and verify as before, new batches are stored
    /// once. Uses a fresh database beside `H2_LIVE_TEST_DB`'s.
    #[tokio::test]
    async fn a_log_from_before_batches_were_stored_once_still_verifies() {
        let Some(base) = std::env::var("H2_LIVE_TEST_DB")
            .ok()
            .filter(|s| !s.is_empty())
        else {
            eprintln!("skipping: H2_LIVE_TEST_DB is not set");
            return;
        };
        let opts: sqlx::postgres::PgConnectOptions = base.parse().unwrap();
        let name = format!("h2live_legacy_{}", std::process::id());
        let admin = PgPoolOptions::new()
            .max_connections(1)
            .connect_with(opts.clone().database("postgres"))
            .await
            .unwrap();
        sqlx::raw_sql(&format!("create database \"{name}\""))
            .execute(&admin)
            .await
            .unwrap();
        let key = live_proto::sign::KeyPair::generate();
        let legacy = crate::store::conformance::legacy_log(&key, "game-legacy");
        {
            let old = PgPoolOptions::new()
                .max_connections(1)
                .connect_with(opts.clone().database(&name))
                .await
                .unwrap();
            sqlx::raw_sql(LEGACY_EVENTS).execute(&old).await.unwrap();
            for e in &legacy {
                let b = &e.body;
                sqlx::query(
                    "insert into events (position, stream_id, version, type, data, at, author, kid, jws, jws_index, hash)
                     values ($1, $2, $3, $4, $5, $6::timestamptz, $7, $8, $9, $10, $11)",
                )
                .bind(e.position as i64)
                .bind(&b.stream)
                .bind(e.version as i64)
                .bind(&b.kind)
                .bind(&b.data)
                .bind(&b.at)
                .bind(&b.author)
                .bind(&b.kid)
                .bind(&*e.jws)
                .bind(e.jws_index.map(|i| i as i32))
                .bind(&e.hash)
                .execute(&old)
                .await
                .unwrap();
            }
            sqlx::query("select setval(pg_get_serial_sequence('events', 'position'), $1)")
                .bind(legacy.len() as i64)
                .execute(&old)
                .await
                .unwrap();
            old.close().await;
        }
        let url = {
            let (head, _) = base.rsplit_once('/').unwrap();
            format!("{head}/{name}")
        };
        let store = PostgresStore::connect(&url).await.unwrap();
        crate::store::conformance::upgraded(&store, &key, &legacy).await;
        let rows: Vec<(String, Option<String>)> =
            sqlx::query_as("select jws, batch from events order by position")
                .fetch_all(&store.pool)
                .await
                .unwrap();
        for (row, e) in rows.iter().zip(&legacy) {
            assert_eq!((row.0.as_str(), row.1.as_deref()), (&*e.jws, None));
        }
        let new: Vec<_> = rows[legacy.len()..].iter().collect();
        assert_eq!(new.len(), 2);
        assert!(new.iter().all(|r| r.0.is_empty() && r.1 == new[0].1));
        store.pool.close().await;
        // Connecting again finds the column there.
        let again = PostgresStore::connect(&url).await.unwrap();
        assert_eq!(again.head().await.unwrap(), legacy.len() as u64 + 2);
        again.pool.close().await;
        sqlx::raw_sql(&format!("drop database \"{name}\""))
            .execute(&admin)
            .await
            .unwrap();
    }

    /// `events` as servers before `batches` made it.
    const LEGACY_EVENTS: &str = "
        create table events (
            position bigserial primary key,
            stream_id text not null,
            version bigint not null,
            type text not null,
            data jsonb not null,
            at timestamptz not null,
            author text not null,
            kid text not null,
            jws text not null,
            jws_index int,
            hash text not null,
            unique (stream_id, version)
        );";

    #[tokio::test]
    async fn postgres_appends_race_safely() {
        let Some(store) = test_store().await else {
            return;
        };
        race(std::sync::Arc::new(store)).await;
    }

    /// Twenty writers want version 0 of one new stream: exactly one wins.
    /// Then twenty unconstrained appends get unique, chained versions.
    pub(crate) async fn race(store: std::sync::Arc<dyn EventStore>) {
        let key = std::sync::Arc::new(live_proto::sign::KeyPair::generate());
        let stream = format!("race-{}", live_proto::sign::random_id());
        let tasks: Vec<_> = (0..20)
            .map(|n| {
                let (store, key, stream) = (store.clone(), key.clone(), stream.clone());
                tokio::spawn(async move {
                    let e = crate::store::conformance::event(&key, &stream, n);
                    store.append(vec![e], &[(stream.clone(), 0)]).await
                })
            })
            .collect();
        let mut won = 0;
        for t in tasks {
            match t.await.unwrap() {
                Ok(_) => won += 1,
                Err(StoreError::Conflict { .. }) => {}
                Err(e) => panic!("{e}"),
            }
        }
        assert_eq!(won, 1);
        let tasks: Vec<_> = (0..20)
            .map(|n| {
                let (store, key, stream) = (store.clone(), key.clone(), stream.clone());
                tokio::spawn(async move {
                    store
                        .append(
                            vec![crate::store::conformance::event(&key, &stream, n)],
                            &[],
                        )
                        .await
                        .unwrap()
                })
            })
            .collect();
        for t in tasks {
            t.await.unwrap();
        }
        let q = Query {
            stream: Some(stream.clone()),
            ..Query::after(0, 100)
        };
        let all = store.read(&q).await.unwrap();
        assert_eq!(
            all.iter().map(|e| e.version).collect::<Vec<_>>(),
            (0..21).collect::<Vec<_>>()
        );
        for w in all.windows(2) {
            assert_eq!(w[1].hash, chain_hash(Some(&w[0].hash), &w[1].jws));
            assert!(w[1].position > w[0].position);
        }
    }
}
