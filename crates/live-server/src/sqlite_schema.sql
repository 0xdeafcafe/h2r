-- The SQLite event store (crates/live-server/src/sqlite.rs): the same tables
-- as schema.sql (Postgres), in SQLite's types. `autoincrement` so a position
-- is never handed out twice, even after the last row is gone.
create table if not exists events (
    position integer primary key autoincrement,
    stream_id text not null,
    version integer not null,
    type text not null,
    -- JSON text; `jws` is the source of truth.
    data text not null,
    -- RFC 3339, as the author wrote it.
    at text not null,
    author text not null,
    kid text not null,
    -- Empty for an element of a batch: `batch` names its JWS.
    jws text not null,
    jws_index integer,
    hash text not null,
    -- `batches.sha256` of a batch's JWS, null for a single event. Added to
    -- older logs by SqliteStore::open; their rows keep their own `jws`.
    batch text,
    unique (stream_id, version)
);
create index if not exists events_type_position on events (type, position);

-- Each batch JWS once (its events' rows name it), by hex(sha256(jws)).
create table if not exists batches (
    sha256 text primary key,
    jws text not null
);

create table if not exists subscriptions (
    name text primary key,
    position integer not null
);

create table if not exists blobs (
    sha256 text primary key,
    bytes blob not null
);
