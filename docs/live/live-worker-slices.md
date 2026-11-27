# Live Worker: build slices and results

Part of [The Live server on Cloudflare Workers](live-worker.md). Covers: What changed to make it build (slice 1); Finished replay archive smoke; Slice 2 results; Slice 3 results; Slice 4 results; Slice 5 results; Slices (serial, about 2 hours each).

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
- **Shared SQL.** The SQLite statements moved to `sql.rs`, shared by `SqliteStore` and the Worker's store.
- **Public helpers.** `BatchRows`, `BatchDecoder` and `batch_key` are public. The shared store tests are public behind the `conformance` feature.

### live-proto

On wasm32 it uses getrandom's JavaScript backends (0.2 `js` and 0.3 `wasm_js`) and `time`'s `wasm-bindgen` clock.

### crates/live/live-worker

- **`sqlstore`** (named `d1store` in slice 1): a `Store` (`EventStore`, `TitleBlobStore` and `UsageStore`) over `Sql`. It builds and is tested natively against SQLite:
  - it passes the shared store tests for events, blobs and usage (`conformance::run`);
  - its rows are `SqliteStore`'s own, checked in both directions on one file.
- **`dosql`** (slice 2; it replaced slice 1's `d1`): the hub's own storage as `Sql`.
- **`r2`**: R2 as an `ObjectStore`. It covers get, range, head, put (overwrite, create-only, if-match), delete and list; multipart and copy are not done yet.
- **`entry`**: the fetch handler and the `LiveHub` object, which starts `Live` on its first request and answers with the router.
- **`wrangler.toml`**: the local defaults and the `staging`, `cutover` and `production` environments ([Deploy](live-worker-deploy.md#deploy)).

### Building and running on this machine

The repository pins Rust 1.98.1 (rust-toolchain.toml). Homebrew's 1.98.1 has
no wasm32 standard library, so the Worker is built with rustup's 1.98.1
plus the `wasm32-unknown-unknown` target and `worker-build`. That toolchain
lives in a home of its own, so the machine's toolchain is untouched:

```
export RUSTUP_HOME=$TMPDIR/lw2/rustup CARGO_HOME=$TMPDIR/lw2/cargo   # rustup-init, once
rustup target add wasm32-unknown-unknown; cargo install worker-build
cd crates/live/live-worker && worker-build --release     # build/index.js, build/index_bg.wasm
crates/live/live-worker/smoke/run.sh 1000                # wrangler dev, the smoke test, a restart
```

For the install only, `CARGO_HOME` points at the private home; the builds
then run with `RUSTUP_HOME` alone, so cargo keeps the shared registry.

The bundle is 4.4 MB of wasm, 1.35 MB gzipped. Workers allow 3 MB on the
free plan and 10 MB on paid.

`smoke/run.sh` runs `bunx wrangler@4 dev --local`: workerd and miniflare on
this machine, with no account. It writes its own wrangler config and a dev
server key under `$TMPDIR/live-worker-dev`.

`smoke/smoke.ts` uses the web app's own sign-up, token and event code. It
checks, in order:
- the first request (the cold start);
- an anonymous `/v1/snapshot`;
- sign-up;
- a token-signed read (sign-in);
- an event append, read back from the log;
- the stream: a socket subscribed before the append receives the event live,
  a replay from a position, a token-authed reader, a bad token closed with
  4401, and a plain GET refused with 400;
- with `WAKE=S`, a hibernation wake with a socket open;
- game ingest to JSON and packed readers, then game close, alarm-driven archive publication and reads of both stored segments;
- R2: a multipart content upload, a cloud save and a 2 MiB title file;
- the same screenshot HTTP contract as native Live, from proof rejection
  and private preview through approval, reporting and removal;
- warm timings;
- with `--seed N`, 2N more events so a restart has a log to replay.

## Finished replay archive smoke

The local workerd smoke used Worker build `3719470d` and harness `b4b49436`.
It ingested frames at ticks 30 and 960 to force two independent segments.
A WebSocket reader observed publication. The harness fetched each segment
over HTTP with Brotli content encoding and checked every decoded field against
the original H2CK response. Admin verification checked the retained originals
and both index hashes. Repeating the final verification cursor returned the
same receipt.

| Operation on the two-frame recording | Local elapsed time |
|---|---:|
| Signed game close, HTTP round trip | 16.5 ms |
| Start of close to publication observed on the stream | 622.7 ms |
| HTTP fetch and decode of each archived segment | 13.8 ms each |
| Verify both segments and retry the final cursor | 106.8 ms |

These are wall times from a single local run with Durable Object storage and
emulated R2. The full-match size and browser CPU measurements use the seeded
multiplayer and campaign recordings. [Telemetry measurements](notes/2026-11-19-telemetry-precision.md#finished-replay-archives)

## Slice 2 results

These numbers come from `smoke/run.sh 1000` under local `wrangler dev`
(workerd on this Mac), so they are not timings at Cloudflare's edge.

| What | Result |
|---|---|
| Cold start, empty log | 40 ms to the first answer; the hub's start took 4 ms |
| Cold start, 2,007 events | 84 ms to the first answer; replaying the log took 49 ms, about 24 µs an event |
| Anonymous `/v1/snapshot` | 7 ms cold, 5 ms warm (p50) on an empty log; 14 ms warm with 2,007 events (283 KB) |
| Sign-up, two accounts | 32 ms |
| Sign-in (a token-signed read) | 6 ms, `private, no-store` |
| Event append | 15 ms round trip; in the hub 5 ms p50, 7 ms p90 over 2,001 appends |

The cache headers are slice 1's:
- anonymous reads: `public, s-maxage=5` with an ETag;
- signed reads: `private, no-store`.

**Replay at today's log size.** `live.forbes.red`'s log is at position 6,196.
At this rate a full replay takes about 150 ms. It costs more if the log is
heavy with GameFrames, which are larger to decode. Read-model snapshots
(slice 6) can wait until the log is far larger.

The hub reports its own start on the request that caused it, in a
`server-timing: start;dur=<ms>, events;desc=<head>` header.

## Slice 3 results

These come from `smoke/run.sh 200` under local `wrangler dev`, as in slice 2.

| What | Result |
|---|---|
| Open `/v1/stream` and get `caught_up` | 24 ms |
| Append to the event reaching the subscribed socket | 23 ms, the same as the append's own round trip |
| Replay from a position (one event, then `caught_up`) | 8 ms |
| `?auth=1` with a fresh read token | let in; a bad token closes with 4401 |

Read tokens are single-use, so a reader needs a fresh one for each socket.

## Slice 4 results

These come from `WAKE=15 smoke/run.sh 20` under local `wrangler dev`, with
R2 simulated locally by miniflare.

| What | Result |
|---|---|
| Hibernation wake | With a stream socket open and 15 s idle, the next request found a new hub (`start;dur=7`), and the socket got the next event, its session rebuilt from the attachment |
| Ingest | A game started, a frame sent on `/v1/ingest`, the frame reached a JSON reader and a packed reader (the packed tick on demand): 79 ms in all |
| Content upload, 9 MiB | 636 ms streamed into a two-part upload; read back whole in 541 ms; a body that does not hash to its path is refused and not stored |
| Cloud save, 300 KiB | 62 ms to write and read back |
| Title file, 2 MiB | 243 ms to publish and read back (too big for a SQLite row) |

`run.sh` makes a fresh admin gamertag each run (`H2_LIVE_ADMINS`); the
smoke test uses the local dev operator key to register it and publish the title file as that account.

## Slice 5 results

Under local `wrangler dev`, as before.

**Attachments on a busy stream** (`BUSY=100`: 100 sockets follow the log
while 60 events are appended; one run each, so the tails are noisy):

| Attachment written | Append p50 | Append p90 | Append max |
|---|---|---|---|
| Per event (slice 4) | 21.3 ms | 35.2 ms | 210 ms |
| On a change of state only | 18.4 ms | 23.4 ms | 32.1 ms |
| The same, in the full run below | 13.2 ms | 34.0 ms | 121 ms |

Every socket had each event within 0.3 ms of the append's answer.

**The co-op relay** (`WAKE=15 BUSY=100 smoke/run.sh 20`: two members of
a real co-op session, through the `CoopRelay` object):

| What | Result |
|---|---|
| Both members in and told of each other | 296 ms (two tickets from the hub) |
| A stranger | closed with 4403 |
| Round trip, 300 datagrams | p50 6.0 ms, p99 11.3 ms |
| 30 a second each way for 17 s | 1,020 of 1,020 delivered; each member's ticket renewed once (`relay: renewed` in the log) |
| 200 a second each way for 5 s | 2,000 of 2,000 delivered |

The relay's own ceiling was not reached: 200 a second each way is the
sender's pacing, under `relay_rate` (240 a second a socket).

## Slices (serial, about 2 hours each)

1. **Feasibility (done).**
   - live-server builds for wasm32 behind `native`.
   - The `live-worker` crate exists, with the store tested natively, R2 and the hub.
2. **The hub serves HTTP under `wrangler dev` (done).**
   - The log moved into the hub's own storage ([Decision](live-worker.md#decision-the-hubs-own-sqlite-not-d1)).
   - Built with `worker-build`, run with `bunx wrangler@4 dev --local`.
   - [Settings](live-worker.md#settings) come from variables and secrets, and `CF-Connecting-IP` is trusted at the edge.
   - The smoke test passes, and the cold start is measured ([Slice 2 results](#slice-2-results)).
3. **The event stream (done).**
   - Hibernatable WebSockets on the hub ([The stream on the hub](live-worker.md#the-stream-on-the-hub)).
   - One stream implementation, `live_server::stream::StreamSession`, run by axum's per-socket task and by the hub.
   - JSON and packed readers, auth messages and token expiry.
4. **Ingest, timers and uploads (done).**
   - The game's ingest socket on the hub, one session shared with axum.
   - No timers on the hub: an alarm plus work done on demand ([Timers and hibernation](live-worker.md#timers-and-hibernation)). A real hibernation wake is shown under `wrangler dev`.
   - Content and cloud-save uploads stream into the blob store (R2 on the hub, multipart past 8 MiB) instead of a temporary file, on a machine too.
   - Title files are in R2.
   - Results: [Slice 4 results](#slice-4-results).
5. **The co-op relay object (done).**
   - Stream sockets got ingest's per-socket lock; attachments are written on a change of state only.
   - `CoopRelay` per session, membership by the hub's signed ticket ([The co-op relay](live-worker.md#the-co-op-relay)).
   - The forwarding, rate buckets and close codes of `relay.rs`.
   - Two relay clients in the smoke test: [Slice 5 results](#slice-5-results).
6. **Cold start.**
   - Snapshot the read model at a log position in the hub's storage (or R2), and replay only the tail.
   - Needed if the replay is slow at today's log size; see [Slice 2 results](#slice-2-results) for the rate.
7. **Migration tooling and a rehearsal (done).**
   - The operator log endpoints, with writes held while a copy goes in ([Migrating the current data](live-worker-deploy.md#migrating-the-current-data)).
   - `store copy` takes `https://HOST` as a source or a destination, signing with the server key.
   - Blobs go to R2 through the `s3://` path.
   - Rehearsed against `wrangler dev` with a read-only copy of the real log ([Rehearsal](live-worker-deploy.md#rehearsal)).
8. **Staging, then cutover.** [Deploy](live-worker-deploy.md#deploy), then the [Cutover runbook](live-worker-deploy.md#cutover-runbook).
