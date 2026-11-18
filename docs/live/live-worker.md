# The Live server on Cloudflare Workers

Today the Live server (`live.forbes.red`) runs on a Mac at home. Every
request crosses the home network through a Cloudflare Tunnel, which costs
250–700 ms per request. This plan moves the whole server to Cloudflare:

- the routes run in a Worker;
- the event log lives in D1;
- blobs live in R2;
- whatever needs a single owner runs in a Durable Object.

No request reaches the Mac afterwards.

The code stays shared. The Worker runs live-server's own axum router, built
without the crate's `native` feature, so the routes are not forked.

## Where things live

| State | Home | Why |
|---|---|---|
| Event log (append order, stream versions, hash chain) | D1, written only by `LiveHub` | One writer keeps the append order. D1 has no interactive transactions, only atomic `batch`. |
| Read model, dispatcher, presence, sessions, invites, accounts, rate limits | `LiveHub`, one Durable Object named `live` | This is `Live` as it is today, single-owner and in memory, rebuilt from the log on start. |
| Event stream (`/v1/stream`), live frames, packed ticks | `LiveHub`, hibernatable WebSockets | The feed and frames come from the read model. Hibernation means idle viewers cost nothing. |
| Game ingest (`/v1/ingest`) | `LiveHub` | Frames feed the read model and the stream. A per-game object is the fallback if one hub's CPU is not enough. |
| Co-op relay (`/v1/sessions/{id}/relay`) | `CoopRelay`, one object per session id | Its traffic stays off the hub's thread, and the object can sit near the players (location hint). |
| Content, cloud saves, published art | R2 bucket `BLOBS` (published art in a public bucket) | `BlobStore` over R2 (`crates/live-worker/src/r2.rs`). |
| Title files | D1 `blobs` table today; move to R2 | A D1 row holds at most 2 MB, but a title file may be up to 2 MiB (`MAX_TITLE_FILE`). |
| Server key | Worker secret `LIVE_SERVER_KEY` | `ServerIdentity::from_secret`. The id is the variable `LIVE_SERVER_ID`. |
| Web app | Workers static assets (`web/dist`) | Served by Cloudflare ahead of the Worker; it is immutable and cached at the edge. |

### Open decision: D1 or the hub's own SQLite

Every D1 query from the hub is a network hop. An append is one read per
stream it touches, then one batch.

A Durable Object also has its own SQLite storage. That storage is
synchronous, transactional and in-process, so writes would cost microseconds
instead of milliseconds.

The store sits behind `d1store::Sql`, so the hub's storage is one more small
adapter. Slice 2 measures both and picks one.

D1's advantage is that other Workers can read it directly, for example stats
queries that skip the hub.

## What changed to make it build (slice 1)

### live-server

- **Features.** A `native` feature (on by default) holds everything that
  cannot build for `wasm32-unknown-unknown`:
  - `sqlx` (with `pg.rs`, `sqlite.rs`, `copy.rs`, `check.rs` and `standalone.rs`), `axum-server`, `rustls`, `reqwest` and `tower-http`;
  - object_store's file and S3 stores;
  - tokio's net, fs and multi-thread runtime;
  - axum's `ws`, `tokio`, `http1` and `http2`.

  `cargo check -p live-server --no-default-features --target wasm32-unknown-unknown` passes, and clippy is clean.
- **WebSocket routes.** Stream, ingest and relay are native-only (`http::sockets`). The relay's room logic stays shared.
- **`rt`.** Spawning, sleep, timeout, interval and `Instant` go through `rt`:
  - natively it is tokio;
  - in a Worker it is `spawn_local`, `setTimeout` and `performance.now()`.

  `std::time::Instant` panics in a Worker. A JavaScript timer is not `Send`, so it is wrapped (`send_wrapper`).
- **`ConnectInfo`.** A Worker has no socket, so `ConnectInfo` is never set there. Nothing counts as "this machine", which turns local sign-in and the loopback-only admin details off.
- **Blobs.** `BlobStore::from_store` takes any object store. `put_file` (uploads spooled to a temporary file) fails on a Worker; see slice 4.
- **Shared SQL.** The SQLite statements moved to `sql.rs`, shared by `SqliteStore` and the D1 store.
- **Public helpers.** `BatchRows`, `BatchDecoder` and `batch_key` are public. The shared store tests are public behind the `conformance` feature.

### live-proto

On wasm32 it uses getrandom's JavaScript backends (0.2 `js` and 0.3 `wasm_js`) and `time`'s `wasm-bindgen` clock.

### crates/live-worker

- **`d1store`**: an `EventStore` over `Sql`. It builds and is tested natively against SQLite:
  - it passes the shared store tests;
  - its rows are `SqliteStore`'s own, checked in both directions on one file.
- **`d1`**: the D1 binding.
- **`r2`**: R2 as an `ObjectStore`. It covers get, range, head, put (overwrite, create-only, if-match), delete and list; multipart and copy are not done yet.
- **`entry`**: the fetch handler and the `LiveHub` object, which starts `Live` on its first request and answers with the router.
- **`wrangler.toml`**: a sketch with no ids in it.

### Building for wasm32 on this machine

Homebrew's Rust has no wasm32 standard library. Checks build it from
`rust-src`:

```
RUSTC_BOOTSTRAP=1 cargo check -p live-worker --target wasm32-unknown-unknown -Zbuild-std=std,panic_abort
```

A deploy needs a real wasm32 toolchain (rustup's target) and `worker-build`.

## Cost of the relay

Co-op sends about 30 packets a second per player, so a two-player session is
60 messages a second into the relay object. The prices below are as
published in 2025; check them again before relying on them.

- **Requests.** Incoming WebSocket messages bill as requests at 20 messages
  to 1, and outgoing messages are free.
  - An hour is 216,000 messages, which bills as 10,800 requests.
- **Duration.** A busy relay never hibernates. It bills 128 MB × 3,600 s =
  450 GB-s per hour.

| Plan | Included | Two-player co-op it covers | After that |
|---|---|---|---|
| Free (SQLite-backed Durable Objects only) | 100,000 requests and 13,000 GB-s a day | About 9 hours a day by requests (duration allows about 28). Everything else the server does shares the same 100,000. | Requests fail until the next day. |
| Paid ($5 a month) | 1,000,000 requests and 400,000 GB-s a month | About 90 hours a month | About $0.0016 an hour in requests plus $0.0056 an hour in duration, under a cent an hour. |

Game ingest and the stream add to this:
- 1 Hz frames per live game in;
- packed ticks out to viewers, which are free.

The paid plan is the one to use. On the free plan, an evening of co-op uses a
large share of the day's requests.

## Slices (serial, about 2 hours each)

1. **Feasibility (done).**
   - live-server builds for wasm32 behind `native`.
   - The `live-worker` crate exists, with the D1 store tested natively, R2 and the hub.
2. **The hub serves HTTP under `wrangler dev`.**
   - Run it locally: `bunx wrangler dev --local` from `$TMPDIR`, with no account.
   - Build with `worker-build`.
   - `Policy` from variables and secrets: admins, invite-only, private.
   - Trust `CF-Connecting-IP` always (a Worker is always behind Cloudflare).
   - Drive the existing HTTP tests' requests against it.
   - Measure the cold start: replaying the log from D1.
   - Decide D1 or the hub's own SQLite (above).
3. **The event stream.**
   - Hibernatable WebSockets on the hub.
   - `run_stream`, `catch_up` and the frame feeds go behind a small socket trait, so axum's WebSocket and the object's share one implementation.
   - JSON and packed readers, auth messages and token expiry.
4. **Ingest, timers and uploads.**
   - The game's ingest socket on the hub.
   - The spawned loops (dispatcher poll, presence sweep, the 100 ms packed tick) become an alarm plus work done on demand, so an idle hub can be evicted.
   - Content and cloud-save uploads stream into R2 instead of a temporary file.
   - Title files move to R2.
5. **The co-op relay object.**
   - `CoopRelay` per session.
   - Membership: the hub checks it once and signs a short ticket; the relay checks the ticket.
   - The forwarding, rate buckets and close codes of `relay.rs`.
   - Tested with two `coop` peers against `wrangler dev`.
6. **Cold start.**
   - Snapshot the read model at a log position in the hub's storage (or R2), and replay only the tail.
   - Needed if slice 2 shows the full replay is slow at today's log size.
7. **Migration tooling and a rehearsal.**
   - `store copy` into a D1 import (below), and blobs to R2 through the existing `s3://` path.
   - Import into a local D1 and compare heads and hash chains with the source.
8. **Staging, then cutover.**
   - Deploy to a staging hostname (`live-staging.forbes.red`), playing against a copy.
   - Then cut over, as below.
9. **Later.** The menu bar app gets a "worker" mode: deploy, tail and status in place of a local server.

## Migrating the current data

The server's log is in Postgres today.

1. **Copy the log.** `h2 live store copy --from postgres://… --to sqlite://<dir>/live.db`. The copy checks hash chains and continues an earlier copy as the source grows.
2. **Turn it into SQL.** Dump the four tables, `events`, `batches`, `subscriptions` and `blobs`, as insert statements (`sqlite3 live.db .dump <table>`, without `sqlite_sequence`).
3. **Import into D1.** Apply the dump with `wrangler d1 execute h2-live --remote --file`. Large batch JWS rows must stay under D1's statement limit; slice 7's tool splits the dump to fit.
4. **Copy blobs.** `h2 live store copy --from-blobs <data dir> --to-blobs s3://h2-live-blobs`, with the R2 S3 credentials ([live-hosting.md](live-hosting.md), "Free hosting"). The public art bucket stays as it is.
5. **Move the identity.**
   - `wrangler secret put LIVE_SERVER_KEY` with the contents of `server.key`.
   - `LIVE_SERVER_ID` is the id in `server.json`. It must not change: every account's author id contains it.
6. **Check.**
   - The Worker's `/healthz` position equals the source head.
   - `/v1/accounts` and `/v1/games` match the old server's answers.

## Cutover, with the DNS name kept

`live.forbes.red` stays a proxied DNS record. A Worker route on
`live.forbes.red/*` runs before the record's origin (the tunnel), so adding
the route moves traffic, and removing it moves it back. The DNS record does
not change.

1. **Rehearse.** Run a full rehearsal on the staging hostname (slice 8).
2. **Stop writes on the Mac.** Stop the Mac's server. Clients retry; this is the downtime window, minutes at most.
3. **Copy the tail.** Copy the log's last events, from the head of the earlier copy, and import them into D1.
4. **Add the route** `live.forbes.red/*` → `h2-live` (with the static assets).
5. **Smoke test.** Check `/healthz`, sign in, the stream and a co-op relay.
6. **Leave the tunnel stopped.** It is the rollback path; it no longer serves anything.

## Rollback

1. **Remove the Worker route.** Traffic falls through to the tunnel again.
2. **Bring the Worker's writes back to the Mac.** Before starting the Mac's server:
   - `wrangler d1 export h2-live --remote` to a SQLite file;
   - `h2 live store copy` from that file into the Mac's log, which takes the events after its head.

   The copy refuses a log that is not a continuation of its own, so a forked log cannot be merged by mistake.
3. **Start the Mac's server and the tunnel.**

Two things keep this safe:
- Only one side writes at a time. Never run the Mac's server and the Worker against the same hostname together.
- R2 blobs written while on the Worker (uploads, cloud saves) copy back with `store copy --from-blobs s3://h2-live-blobs --to-blobs <data dir>`.

## What the Mac does afterwards

Nothing, as a server:
- the game (`h2 play`) talks to `live.forbes.red` as before;
- the web app is served by Cloudflare.

The Mac still does these:
- **Publishing game art.** It needs the disc, and it uploads to the public bucket through the existing `s3://` path.
- **Deploys.** `wrangler deploy` from a checkout, until the menu bar app's "worker" mode (slice 9).

`h2 live serve` keeps working for local and offline servers, where the
machine's SQLite or Postgres log stays the default.
