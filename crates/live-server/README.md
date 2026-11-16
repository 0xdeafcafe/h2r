# live-server

The Xbox Live replacement service ([docs/live/live.md](../../docs/live/live.md) §4.3):
signed events in an append-only log (SQLite by default, Postgres if asked
for), projected into memory and
served as HTTP + JSON and a WebSocket, so the game-side HLE never speaks
Kerberos or XSP. The contract is [docs/live/live-api.md](../../docs/live/live-api.md);
identity, signing and tokens are
[docs/live/live-security.md](../../docs/live/live-security.md). Everything clients
share with the server (wire types, signing, the MOTD builder) is in
`crates/live-proto`.

## Run it

```
h2 live serve                                   # 127.0.0.1:8074, sqlite://<data>/live.db
h2 live serve --blobs s3://h2live               # content and saves in R2 (docs/live/live-hosting.md)
h2 live store copy --from postgres://localhost/h2live --to sqlite://<data>/live.db
h2 live serve --db postgres://localhost/other --id live.example.net \
    --listen 0.0.0.0:443 --tls-cert cert.pem --tls-key key.pem
h2 live account create "Sneaky Pete"            # key made and kept locally
h2 live motd set "Welcome back" "Double XP this weekend" --header "Halo 2"
h2 live serve --invite-only & h2 live invite create
h2 live q games --live                          # JSON off a terminal
h2 play disc.iso --live-local                   # an embedded server, signed in
h2 play disc.iso --live                         # signed in to live.forbes.red
h2 live serve --tunnel                          # public via cloudflared (docs/live/live-hosting.md)
```

`h2-live-server` (`src/bin`, flags in `standalone.rs`) is the same server
without the rest of `h2`, for the Docker image in `deploy/live/`
([live-hosting.md](../../docs/live/live-hosting.md#docker-with-a-named-tunnel)):
`--data` (or `H2_LIVE_DATA`), the web app from `H2_LIVE_WEB`, and no
tunnel of its own; `--public-url https://HOST --behind-cloudflared` says a
cloudflared beside it delivers the requests.

`h2 live serve` also serves the web app from `web/dist` (`H2_LIVE_WEB`
overrides) at `/`, falling back to `index.html` for client routes.
`start_embedded` is the same server on a background runtime for `h2 play
--live-local`: Postgres if reachable (database created if missing), else the
file store (`filestore.rs`: an append-only `events.jsonl`, a `blobs/`
directory and `checkpoints.json` under `<data>/log`, replayed into memory
on open), else in-memory with a warning if that directory is held by
another server. Unless the store is in-memory it publishes a default
message of the day (`default_motd.rs`) when none was ever published.

The database URL comes from `--db`, then `H2_LIVE_DB`, then the default; it
is only ever printed without credentials. The schema is created at startup
if missing. The server's key and id live in `--data` (default
`~/Library/Application Support/h2recomp/live/server`, mode 0700, key 0600),
never in Postgres; the id is fixed on first start. A non-loopback `--listen`
needs `--tls-cert`/`--tls-key` (rustls) or `--insecure-http`. `--private`
makes reads need a token; `--invite-only` makes account creation need an
invite from `h2 live invite create`.

## API

What is built of [live-api.md](../../docs/live/live-api.md):

| request | notes |
|---|---|
| `GET /healthz` | `{ok, position}`, position = log head |
| `GET /.well-known/h2live.json` | `{id, api: "/v1", keys, software, federation: false, url?}`; `url` is the public base URL when tunnelled or `--public-url` ([live-hosting.md](../../docs/live/live-hosting.md)) |
| `POST /v1/accounts` | body: compact JWS by the new key over `{gamertag, jwk, invite?, aud, iat}` (`aud` = server id, `iat` within 5 min); 201 `{xuid, gamertag, id}` |
| `GET /v1/accounts` | list of `{xuid, gamertag, created_at, online}` |
| `GET /v1/accounts/{gamertag}` | plus `id`, `recent_games` (the 10 latest) |
| `GET /v1/accounts/{gamertag}/keys`, `/v1/accounts/by-xuid/{xuid}/keys` | `{keys: [{kid, jwk, valid_from, valid_to}]}` |
| `GET /v1/accounts/{gamertag}/friends` | friends with presence, pending requests in and out ([live-api.md, Friends](../../docs/live/live-api.md#friends)) |
| `GET /v1/presence` | list of `{xuid, gamertag, state, game_id?, since}` |
| `POST /v1/presence/ping` | `events.write`; refreshes the 90 s deadline, writes nothing; `{ok, expires_in}` |
| `POST /v1/invites?days=&code=` | operator (`invites.write`), or an admin's signed `InviteCreated` (optionally `{event, code}`); 201 `{invite, custom, jti, exp, position}` |
| `POST /v1/accounts/{gamertag}/recover` | `{request, code}`: add a new device key with a recovery code (src/recovery.rs) |
| `GET`/`POST /v1/accounts/by-xuid/{xuid}/recovery` | the account's own token: its recovery secret and current code; `POST` resets it |
| `GET /v1/events?from=&cursor=&type=&exclude=&stream=&limit=` | the log, oldest first, `from` exclusive |
| `POST /v1/events` | body: one signed event (compact JWS), bearer token `events.write` |
| `GET /v1/stream?from=&types=&exclude=&stream=&games=` | WebSocket: replay, `caught_up`, live events and `frame`s |
| `PUT /v1/titles/{title}/files/{name}` | operator only (token by the server key, `titles.write`), ≤ 2 MiB |
| `GET /v1/titles/{title}/files/{name}` | raw bytes, `ETag`, 304 on `If-None-Match` |
| `POST /v1/games` | `games.write`; body: the game without an id; 201 `{game_id, position}` (server writes `GameStarted` with `host`) |
| `GET /v1/games?state=live\|ended&cursor=&limit=` | `GameSummary` list, newest first |
| `GET /v1/games/{id}` | summary plus scoreboard, last 50 kills, winner, end reason |
| `GET /v1/games/{id}/frames?from_tick=&to_tick=&cursor=&limit=` | recorded frames, oldest first, 600 a page |
| `POST /v1/games/{id}/events` | `games.write`; body: one batch JWS by the host or a player |
| `GET /v1/ingest` | WebSocket: `auth`, then `frame` / `events` / `ping` |
| `PUT /v1/maps/{map}/outline` | `games.write`, ≤ 2 MiB; first upload wins, operator may replace |
| `GET /v1/maps/{map}/outline` | the outline JSON |

Lists are `{items, next, position}`; pass `next` back as `?cursor=`. Every
read reply carries `position`, the last position the read model applied.
Errors are `{"error": "<code>", "message"}` with codes `invalid`,
`unauthorized`, `forbidden`, `not_found`, `conflict`, `too_large`,
`rate_limited`, `unimplemented`, `unavailable`.

Accounts may append, on their own streams only: `KeyAdded` / `KeyRevoked`
and the friend events (`FriendRequested` from oneself, `FriendAccepted` /
`FriendDeclined` by the request's recipient, `FriendRemoved` by either
side) on `account-<xuid>`, and `SignedIn` / `PresenceChanged` /
`SignedOut` on `presence-<xuid>` (states `online`, `away`, `playing` with
`game_id`).

## Design

- **`store`**: `EventStore` (SQLite in `sqlite`, Postgres in `pg`, the
  embedded server's `filestore`, `MemoryStore` for tests; one conformance
  suite runs over all of them). SQLite runs in WAL mode; appends take an
  in-process lock and `BEGIN IMMEDIATE`, so positions are handed out in
  commit order as Postgres's advisory lock does.
- **`blobs`**: content, cloud saves and published art through the
  `object_store` crate: the data directory (the old layout), an
  S3-compatible bucket (R2), or memory for tests. A private store for
  everything, and an optional public bucket that shared game art is
  published to and redirected to (`art.rs`).
- **`copy`**: `store copy`, moving the log (positions, versions and hashes
  kept, checked on the way in) and the blobs between backends, resumable.
  Appends are a list of events plus expected versions, atomic across
  streams, and compute the per-stream hash chain
  `hex(sha256(prev || jws))`. In Postgres every append holds
  `pg_advisory_xact_lock` on one key, so `bigserial` positions are handed
  out in commit order (a tailer reading by position can never miss a late
  commit), and sends `pg_notify('events', position)`. That serialises
  writes, which is fine at this scale.
- **`dispatch`**: the one tailer, woken by LISTEN with a 1 s poll fallback.
  It feeds each event to every `Subscriber`, then publishes the applied
  position (writes wait on it: read-your-writes) and broadcasts the event
  (the WebSocket follows it, re-reading from the store if it lags).
- **`model`**: the read model, replayed from 0 at startup: accounts, keys
  with validity positions, title files, presence, recent nonces and used
  invites.
- **`app`**: all checks, in memory: tokens (EdDSA, `aud`, `exp - iat` ≤
  60 s, `lxm`, `jti` remembered two minutes), event signatures against the
  author's current key, `at` within 5 minutes, nonces, stream permissions,
  rate limits per author and method.
- **`http`**: routes, body caps (64 KiB, title files 2 MiB, WebSocket
  messages 16 KiB), CORS (any origin for GET/HEAD, none for writes).
- **`games`**, **`ingest`**: game writes and reads, frames (15 Hz fan-out,
  1 Hz recorded), outlines, and the sweeper that writes `PresenceExpired`
  and `GameEnded {reason: "lost"}`.
- **`identity`**: the server key and id on disk.

The event's JWS is stored untouched and is the source of truth; events are
decoded from it on read. The other columns are copies for querying.

## Tests

```
cargo test -p live-proto -p live-server
```

runs the unit tests, the router tests (`tests/http.rs`, `tests/games.rs`,
`tests/friends.rs`, tower `oneshot` over the in-memory store, sharing
`tests/common`) and the
WebSocket tests (`tests/stream.rs`, a real socket, ingest included). SQLite
runs the event-store conformance and race tests on a temporary file, and
the blob store runs its conformance over a directory, memory and an S3
fake (`blobs.rs`, path-style requests signed for region `auto`). The
Postgres tests skip with a note unless `H2_LIVE_TEST_DB` is set:

```
createdb h2live_test_0
H2_LIVE_TEST_DB=postgres://localhost/h2live_test_0 cargo test -p live-server
dropdb h2live_test_0
```

`examples/fake_game.rs` drives a running server the way a console would
(accounts, presence pings, a finished game and a live one over ingest), for
the web app and for trying the API:

```
cargo run -p live-server --example fake_game -- http://127.0.0.1:8074 --seconds 60
```

Queries are runtime-checked (`sqlx::query`, not `query!`), so no
`DATABASE_URL` is needed to build.

## Known gaps

- Ingest throttling and ping deadlines are in memory, per process.
- Federation (security §5) is not built; authors from another server are
  refused.
- Single instance per database: the in-memory read model, replay sets and
  rate limits assume one server process.
- A restart forgets token `jti`s (they live two minutes) but not event
  nonces, which are re-seeded from the log.
- The projections are in memory; a restart replays the whole log. The
  `subscriptions` checkpoint table is there for when that gets slow.
