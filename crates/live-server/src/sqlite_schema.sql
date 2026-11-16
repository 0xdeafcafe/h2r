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
    jws text not null,
    jws_index integer,
    hash text not null,
    unique (stream_id, version)
);
create index if not exists events_type_position on events (type, position);

create table if not exists subscriptions (
    name text primary key,
    position integer not null
);

create table if not exists blobs (
    sha256 text primary key,
    bytes blob not null
);
