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

use crate::store::{EventStore, NewEvent, Query, RecordedEvent, StoreError, Wake};

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
        let pool = PgPoolOptions::new().max_connections(8).connect(url).await?;
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

fn to_i64(n: u64) -> i64 {
    i64::try_from(n).unwrap_or(i64::MAX)
}

fn decode(row: &PgRow) -> Result<RecordedEvent, StoreError> {
    let position = row.get::<i64, _>("position") as u64;
    let index = row.get::<Option<i32>, _>("jws_index").map(|i| i as usize);
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

#[async_trait]
impl EventStore for PostgresStore {
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
        for e in events {
            let head = heads.get_mut(&e.body.stream).expect("read above");
            let (version, hash) = (head.0, chain_hash(head.1.as_deref(), &e.jws));
            let b = &e.body;
            let position: i64 = sqlx::query_scalar(
                "insert into events (stream_id, version, type, data, at, author, kid, jws, jws_index, hash)
                 values ($1, $2, $3, $4, $5::timestamptz, $6, $7, $8, $9, $10)
                 returning position",
            )
            .bind(&b.stream)
            .bind(to_i64(version))
            .bind(&b.kind)
            .bind(&b.data)
            .bind(&b.at)
            .bind(&b.author)
            .bind(&b.kid)
            .bind(&e.jws)
            .bind(e.jws_index.map(|i| i as i32))
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
            "select position, version, hash, jws, jws_index from events
             where position > $1
               and ($2::text is null or stream_id = $2)
               and (cardinality($3::text[]) = 0 or type = any($3))
               and not (type = any($5))
             order by position limit $4",
        )
        .bind(to_i64(q.after))
        .bind(&q.stream)
        .bind(&q.types)
        .bind(to_i64(q.limit as u64))
        .bind(&q.exclude)
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

    #[tokio::test]
    async fn postgres_appends_race_safely() {
        let Some(store) = test_store().await else {
            return;
        };
        let store = std::sync::Arc::new(store);
        let key = std::sync::Arc::new(live_proto::sign::KeyPair::generate());
        let stream = format!("race-{}", live_proto::sign::random_id());
        // Twenty writers want version 0 of one stream: exactly one wins.
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
        // Concurrent unconstrained appends get unique, chained versions.
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
