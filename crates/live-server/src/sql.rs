//! The SQLite event store's schema and statements, shared by
//! [`crate::sqlite::SqliteStore`] (sqlx, on this machine) and the Worker's
//! D1 store (crates/live-worker), so both read and write the same rows the
//! same way. Parameters are numbered (`?1`), which both bind.

/// The tables, created if missing.
pub const SCHEMA: &str = include_str!("sqlite_schema.sql");

/// A batch's JWS, once: `?1` its key ([`crate::store::batch_key`]), `?2`
/// the JWS.
pub const INSERT_BATCH: &str =
    "insert into batches (sha256, jws) values (?1, ?2) on conflict (sha256) do nothing";

/// Stream `?1`'s last row: `version`, `hash`.
pub const STREAM_HEAD: &str =
    "select version, hash from events where stream_id = ?1 order by version desc limit 1";

/// A new event; returns its `position`.
pub const INSERT_EVENT: &str =
    "insert into events (stream_id, version, type, data, at, author, kid, jws, jws_index, hash, batch)
     values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
     returning position";

/// An imported event, at its own position (`?1`).
pub const IMPORT_EVENT: &str =
    "insert into events (position, stream_id, version, type, data, at, author, kid, jws, jws_index, hash, batch)
     values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)";

/// A page of the log ([`crate::store::Query`]): after `?1`, stream `?2`
/// (or any), types in the JSON array `?3` (or any), at most `?4`, types
/// not in the JSON array `?5`. Rows: `position`, `version`, `hash`, `jws`,
/// `jws_index`.
pub const READ: &str =
    "select e.position, e.version, e.hash, coalesce(b.jws, e.jws) as jws, e.jws_index
     from events e left join batches b on b.sha256 = e.batch
     where e.position > ?1
       and (?2 is null or e.stream_id = ?2)
       and (json_array_length(?3) = 0 or e.type in (select value from json_each(?3)))
       and e.type not in (select value from json_each(?5))
     order by e.position limit ?4";

/// The last position, 0 for an empty log.
pub const HEAD: &str = "select coalesce(max(position), 0) from events";

pub const PUT_BLOB: &str =
    "insert into blobs (sha256, bytes) values (?1, ?2) on conflict (sha256) do nothing";
pub const BLOB: &str = "select bytes from blobs where sha256 = ?1";
pub const BLOB_NAMES: &str = "select sha256 from blobs order by sha256";

pub const LOAD_CHECKPOINT: &str = "select position from subscriptions where name = ?1";
pub const SAVE_CHECKPOINT: &str = "insert into subscriptions (name, position) values (?1, ?2)
     on conflict (name) do update set position = excluded.position";
pub const CHECKPOINTS: &str = "select name, position from subscriptions order by name";

/// A list as a JSON array, for `json_each`.
pub fn json_list(items: &[String]) -> String {
    serde_json::to_string(items).expect("strings serialize")
}
