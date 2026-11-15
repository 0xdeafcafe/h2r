-- The service's whole persistent state (docs/live.md §4.3). Created at
-- startup if missing. `events` is append-only; `jws` is the author's
-- signed body, untouched, and the source of truth: the other columns are
-- copies for querying.
create table if not exists events (
    position bigserial primary key,
    stream_id text not null,
    version bigint not null,
    type text not null,
    data jsonb not null,
    at timestamptz not null,
    author text not null,
    kid text not null,
    jws text not null,
    -- The element of a batch JWS this event is; null for a single event.
    jws_index int,
    -- hex(sha256(previous hash in the stream || jws)).
    hash text not null,
    unique (stream_id, version)
);
create index if not exists events_type_position on events (type, position);

-- Where a durable subscriber resumes: the last position it processed.
create table if not exists subscriptions (
    name text primary key,
    position bigint not null
);

-- Content-addressed bytes (title files); events refer to them by sha256.
create table if not exists blobs (
    sha256 text primary key,
    bytes bytea not null
);
