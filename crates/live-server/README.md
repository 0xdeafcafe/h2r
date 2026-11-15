# live-server

The Xbox Live replacement service ([docs/live.md](../../docs/live.md) §4.3):
signed events in an append-only Postgres log, projected into memory and
served as HTTP + JSON and a WebSocket, so the game-side HLE never speaks
Kerberos or XSP. The contract is [docs/live-api.md](../../docs/live-api.md);
identity, signing and tokens are
[docs/live-security.md](../../docs/live-security.md). Everything clients
share with the server (wire types, signing, the MOTD builder) is in
`crates/live-proto`.

## Run it

```
h2 live serve                                   # 127.0.0.1:8074, postgres://localhost/h2live
h2 live serve --db postgres://localhost/other --id live.example.net \
    --listen 0.0.0.0:443 --tls-cert cert.pem --tls-key key.pem
h2 live account create "Sneaky Pete"            # key made and kept locally
h2 live motd set "Welcome back" "Double XP this weekend" --header "Halo 2"
h2 live serve --invite-only & h2 live invite create
```

The database URL comes from `--db`, then `H2_LIVE_DB`, then the default; it
is only ever printed without credentials. The schema is created at startup
if missing. The server's key and id live in `--data` (default
`~/Library/Application Support/h2recomp/live/server`, mode 0700, key 0600),
never in Postgres; the id is fixed on first start. A non-loopback `--listen`
needs `--tls-cert`/`--tls-key` (rustls) or `--insecure-http`. `--private`
makes reads need a token; `--invite-only` makes account creation need an
invite from `h2 live invite create`.

## API

What is built of [live-api.md](../../docs/live-api.md):

| request | notes |
|---|---|
| `GET /healthz` | `{ok, position}`, position = log head |
| `GET /.well-known/h2live.json` | `{id, api: "/v1", keys, software, federation: false}` |
| `POST /v1/accounts` | body: compact JWS by the new key over `{gamertag, jwk, invite?}`; 201 `{xuid, gamertag, id}` |
| `GET /v1/accounts` | list of `{xuid, gamertag, created_at, online}` |
| `GET /v1/accounts/{gamertag}` | plus `id`, `recent_games` (empty until games exist) |
| `GET /v1/accounts/{gamertag}/keys`, `/v1/accounts/by-xuid/{xuid}/keys` | `{keys: [{kid, jwk, valid_from, valid_to}]}` |
| `GET /v1/presence` | list of `{xuid, gamertag, state, game_id?, since}` |
| `GET /v1/events?from=&cursor=&type=&exclude=&stream=&limit=` | the log, oldest first, `from` exclusive |
| `POST /v1/events` | body: one signed event (compact JWS), bearer token `events.write` |
| `GET /v1/stream?from=&types=&exclude=` | WebSocket: replay, `caught_up`, live |
| `PUT /v1/titles/{title}/files/{name}` | operator only (token by the server key, `titles.write`), ≤ 2 MiB |
| `GET /v1/titles/{title}/files/{name}` | raw bytes, `ETag`, 304 on `If-None-Match` |
| `/v1/games*`, `/v1/ingest`, `/v1/maps/{map}/outline` | 501 `unimplemented` |

Lists are `{items, next, position}`; pass `next` back as `?cursor=`. Every
read reply carries `position`, the last position the read model applied.
Errors are `{"error": "<code>", "message"}` with codes `invalid`,
`unauthorized`, `forbidden`, `not_found`, `conflict`, `too_large`,
`rate_limited`, `unimplemented`, `unavailable`.

Accounts may append, on their own streams only: `KeyAdded` / `KeyRevoked`
on `account-<xuid>` (a new key must be unregistered; the last key cannot be
revoked) and `SignedIn` / `PresenceChanged` / `SignedOut` on
`presence-<xuid>` (states `online`, `away`, `playing` with `game_id`).

## Design

- **`store`**: `EventStore` (Postgres in `pg`, `MemoryStore` for tests).
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
- **`identity`**: the server key and id on disk.

The event's JWS is stored untouched and is the source of truth; events are
decoded from it on read. The other columns are copies for querying.

## Tests

```
cargo test -p live-proto -p live-server
```

runs the unit tests, the router tests (`tests/http.rs`, tower `oneshot`
over the in-memory store) and the WebSocket tests (`tests/stream.rs`, a
real socket). The Postgres tests skip with a note unless `H2_LIVE_TEST_DB`
is set:

```
createdb h2live_test_0
H2_LIVE_TEST_DB=postgres://localhost/h2live_test_0 cargo test -p live-server
dropdb h2live_test_0
```

Queries are runtime-checked (`sqlx::query`, not `query!`), so no
`DATABASE_URL` is needed to build.

## Known gaps

- Games, ingest, frames and map outlines are not built (501).
- Presence never expires: the keep-alive deadline and `PresenceExpired`
  are not built.
- Federation (security §5) is not built; authors from another server are
  refused.
- Single instance per database: the in-memory read model, replay sets and
  rate limits assume one server process.
- A restart forgets token `jti`s (they live two minutes) but not event
  nonces, which are re-seeded from the log.
- The projections are in memory; a restart replays the whole log. The
  `subscriptions` checkpoint table is there for when that gets slow.
