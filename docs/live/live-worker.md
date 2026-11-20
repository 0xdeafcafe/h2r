# The Live server on Cloudflare Workers

The Live service has a native deployment and a Cloudflare deployment.
The original Mac setup sent requests through a Cloudflare Tunnel and
measured 250–700 ms per request. The Cloudflare deployment serves the
same Live API from the edge:

- the routes run in a Worker;
- the event log lives in the hub Durable Object's own SQLite storage;
- blobs live in R2;
- whatever needs a single owner runs in a Durable Object.

The Worker deployment serves requests independently of the Mac.

The code stays shared. The Worker runs live-server's own axum router, built
without the crate's `native` feature, so the routes are not forked.

## Where things live

| State | Home | Why |
|---|---|---|
| Event log (append order, stream versions, hash chain) | `LiveHub`'s own SQLite storage (`ctx.storage.sql`) | In-process and synchronous, so a query costs no network hop. One writer keeps the append order. A batch commits whole inside `transactionSync`. |
| Read model, dispatcher, presence, sessions, invites, accounts, rate limits | `LiveHub`, one Durable Object named `live` | This is `Live` as it is today, single-owner and in memory, rebuilt from the log on start. |
| Event stream (`/v1/stream`), live frames, packed ticks | `LiveHub`, hibernatable WebSockets | The feed and frames come from the read model. Hibernation means idle viewers cost nothing. |
| Game ingest (`/v1/ingest`) | `LiveHub` | Frames feed the read model and the stream. A per-game object is the fallback if one hub's CPU is not enough. |
| Co-op relay (`/v1/sessions/{id}/relay`) | `CoopRelay`, one object per session id | Its traffic stays off the hub's thread, and the object can sit near the players (location hint). |
| Content, cloud saves, published art | R2 bucket `BLOBS` (published art in a public bucket) | `BlobStore` over R2 (`crates/live/live-worker/src/r2.rs`). |
| Title files | R2, under `title/<sha256>` (`live_server::titleblobs`) | A Durable Object's SQLite row holds at most 2 MB, but a title file may be up to 2 MiB (`MAX_TITLE_FILE`). Ones already in the log's `blobs` table are still read from there. |
| Server key | Worker secret `H2_LIVE_SERVER_KEY` | `ServerIdentity::from_secret`. The id is the variable `H2_LIVE_ID` ([Settings](#settings)). |
| Web app | Workers static assets (`web/dist`) | Served by Cloudflare ahead of the Worker; it is immutable and cached at the edge. |

Anonymous web Theater reads of a complete public time chunk use the Worker
Cache API. The key includes the origin, game, immutable upload revision and
exact byte span. A miss goes to LiveHub. A successful 206 carrying the hub's
immutable film policy can populate the cache at that edge location. The Worker
restores the original Content-Range on a hit. Private homes, bearer and cookie
requests, mutable visibility and metadata reads continue through LiveHub.

### Decision: the hub's own SQLite, not D1

Decided in slice 2. The log lives in `LiveHub`'s own SQLite storage. That
storage is in-process and synchronous, and the hub is its only writer, so an
append is a few local statements. D1 would cost a network hop for every
query: one read per stream an append touches, then one batch.

The store is `sqlstore::SqlStore` over the `Sql` trait:
- in the Worker it runs on the hub's storage (`dosql.rs`);
- in the tests it runs on sqlx's SQLite.

Anything else that runs SQLite statements in an atomic batch, D1 included,
fits the same trait. D1 is out of the hot path, and migration does not need
it either ([Migrating the current data](#migrating-the-current-data)).

What is given up: other Workers cannot query the log directly. Everything
goes through the hub, as it does on a machine today.

### Decision: analytics without Postgres

Decided 2026-11-18. The hub's SQLite stays the source of truth, for the
log and the read model. Heavy historical analytics go K2 → Pipelines → R2
Iceberg tables, queried with R2 SQL. Nothing a user sees waits on that
path.

Why not Postgres (through Hyperdrive):
- **No extra vendor or bill.** Everything stays on the Cloudflare account.
- **No network hop in the request path.** The hub's SQLite answers
  in-process; analytics run beside it, never in front of it.
- **R2 storage prices for history.** Old events and frames cost R2's rate,
  not a database's.

Postgres stays supported for a native `h2 live serve`, as it is today.

## Settings

Worker variables (`[vars]` in wrangler.toml) and secrets. The names are
`h2-live-server`'s environment names where it has them
(`crates/live/live-worker/src/settings.rs`).

| Name | Kind | Meaning |
|---|---|---|
| `H2_LIVE_ID` | variable | The server id: the host in every author id. It must stay what the machine's server had. |
| `H2_LIVE_SERVER_KEY` | secret | The server's key, base64 as in `server.key`. |
| `H2_LIVE_PUBLIC_URL` | variable | The URL clients use, shown in discovery. Default `https://<id>`. |
| `H2_LIVE_ADMINS` | variable | Admin gamertags, comma-separated. |
| `H2_LIVE_INVITE_ONLY` | variable | `1`: account creation needs an invite. |
| `H2_LIVE_PRIVATE` | variable | `1`: reads need a token too. |
| `H2_LIVE_GAME_ART_URL` | variable | Published game art, `<public bucket URL>/art/<stamp>` (what `h2 live assets publish` prints). `/art/game/*` redirects there and `/art/emblem/*` reads its manifest and masks. With it unset, both paths return 404 and the web app uses its own art pack. |

A missing id or key, or a bad value, fails the hub's start. The request then
answers 500 with the reason; the hub never runs as some other server.

**The client address.** A Worker has no peer address, and every request comes
through Cloudflare's edge, which sets `CF-Connecting-IP` itself. The hub
marks its public URL as at the edge (`PublicUrl::set_edge`), so that header
is the client's address.

On a machine the header still counts only from loopback behind a tunnel;
anywhere else it can be forged.

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

- **`sqlstore`** (named `d1store` in slice 1): an `EventStore` over `Sql`. It builds and is tested natively against SQLite:
  - it passes the shared store tests;
  - its rows are `SqliteStore`'s own, checked in both directions on one file.
- **`dosql`** (slice 2; it replaced slice 1's `d1`): the hub's own storage as `Sql`.
- **`r2`**: R2 as an `ObjectStore`. It covers get, range, head, put (overwrite, create-only, if-match), delete and list; multipart and copy are not done yet.
- **`entry`**: the fetch handler and the `LiveHub` object, which starts `Live` on its first request and answers with the router.
- **`wrangler.toml`**: the local defaults and the `staging`, `cutover` and `production` environments ([Deploy](#deploy)).

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
multiplayer and campaign recordings. [Telemetry measurements](../engine/live-telemetry.md#finished-replay-archives)

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
smoke test signs up as it to publish the title file.

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

## The stream on the hub

The hub and native server advertise the same v8 ingest support in
discovery. Packed subscribers opt in separately. The hibernation
attachment retains each subscriber's version preference.
[Packed frames](live-telemetry.md#version-8-live-wire)

`live_server::stream` holds `/v1/stream` with no socket underneath it:
- `StreamFilter` is the query string.
- `StreamSession` turns what happens into messages for the reader (`Out`:
  text, binary, or a close). The things that happen are: the reader being let
  in, its auth message, a new event, a JSON frame, a packed tick, falling
  behind, and a token running out.
- `Resume` is a session's lasting state as plain data: the filter, the last
  position the reader has, who it is and when its token ends.

Two drivers run the session:
- **axum** (`http::run_stream`), on a machine. It is one task per socket,
  selecting over the socket, the feeds and the token deadline, as before.
- **The hub** (`crates/live/live-worker/src/sockets.rs`).
  - Each socket is accepted with `state.accept_web_socket`, and its `Resume`
    is kept as the socket's attachment. The attachment is written when that
    state changes (let in, a new token), not per event: every open session
    is level with the log whenever the hub is idle, the only time it
    hibernates, so a session rebuilt after a wake starts at the log's head.
    Writing it per event cost about 3 ms an append at p50 and 12 ms at p90
    with 100 sockets ([Slice 5 results](#slice-5-results)).
  - One pump task fans the hub's feeds out to every session. Expired tokens,
    and readers that sent no auth message within 10 s, are closed from the
    hub's alarm ([Timers and hibernation](#timers-and-hibernation)).
  - Reader messages come in through `websocket_message`. A socket's
    messages are handled one at a time and in order, each session behind an
    async lock, so none is lost while an earlier one awaits the store.
  - The pump skips a session that is busy. Before the session is let go it
    is caught up again until it is level with the log (`settle`).
  - The code is generic over a `Socket` trait, so it is tested on a machine
    with a fake socket.
  - After a wake (a new instance with the sockets still open), `adopt`
    rebuilds each session from its attachment.

`/v1/ingest` works the same way: `live_server::ingest::IngestSession` is
the game socket's logic, driven by axum's task on a machine and by the hub
(`crates/live/live-worker/src/ingest.rs`). The hub handles each ingest socket's
messages one at a time, behind an async lock, since a message's work awaits
the store. The shared session checks the 16 KiB message cap before parsing
auth or decoding frames, and accepts up to 16 frame records per message
in batched mode. The attachment keeps the author, token expiry and
ingest mode. After a wake, the sender's next keyframe restarts the packed
decoder. [Ingest wire format](live-api.md#streams-websocket)

## Timers and hibernation

A Durable Object with a timer pending does not hibernate. On a machine
the server spawns loops: the dispatcher's poll, the sweep every 5 s and the
100 ms packed tick. The hub starts the server with `Timers::Driven`
instead, which spawns none of them:
- The dispatcher waits only for the store's wake. The hub's store has one
  writer, and its wake sees every append.
- After every request, socket message and alarm, the hub sets its one
  alarm (`LiveHub::schedule`) for the earliest of:
  - `Live::sweep_due`: the next presence to expire, game to end as lost,
    or co-op session to expire;
  - each stream or ingest socket's auth deadline or token end.

  With nothing due there is no alarm. The alarm runs the sweep and the
  sockets' checks, then sets the next one.
- The packed tick runs only while a game is live (`Live::packed_active`),
  and stops by itself when none is.

So an idle hub, sockets open or not, has nothing pending, and hibernates.

## Deploys and retries

A deploy resets every Durable Object: calls in flight throw ("Durable
Object reset because its code was updated", "Network connection lost") with
`retryable` set, and the hub replays its log on the next call. The fetch
handler (`entry::fetch_retrying`, decisions in `doretry`, unit-tested on
this machine) answers such a throw by:
- calling again, 100 ms then 400 ms later (3 calls in all), if the request is
  safe to repeat: GET, HEAD, OPTIONS, PUT, DELETE, `POST /v1/presence/ping`,
  and the signed-event POSTs (`/v1/events`, `/v1/sessions/{id}/events`), which
  the hub takes once by their nonce, a guard rebuilt from the log;
- otherwise, or when the calls run out, `503` with `Retry-After: 2` and the
  API's error body (`unavailable`), not a bare 500, so a client knows to
  come back;
- an overloaded object (`overloaded`) gets the 503 at once: calling again
  would only add to its load.

A call is made again with a copy of the request (its body teed), and the
runtime holds the copy's bytes in the isolate until the first call ends.
A body over 8 MiB (`doretry::KEEP_BODY_BYTES`), or one without a
`Content-Length`, is sent once without a copy, so a 95 MiB content upload
never sits twice in a 128 MB isolate; a reset under it is the 503 above,
and the client sends it again.

`POST /v1/presence/ping/batch` goes to the hub's shared HTTP router.
The edge returns 503 on a transient hub failure so the client can issue
new per-account tokens for its next attempt.

Anything else thrown surfaces as before. The single-use token guard (`jti`)
is in the hub's memory and is lost on a reset, so a repeat with the same
token is accepted; that is why only requests that are idempotent or nonce-
guarded are repeated here. The game client has its own retries and a grace
before it calls the connection lost ([live.md](live.md) "Outages"), and the
restarted hub renews presence, session and game deadlines, so a deploy does
not expire anyone who is still pinging.

Sockets (`/v1/stream`, ingest, the co-op relay) are closed by a deploy;
their attachments survive hibernation, not a deploy. The relay is not used
by the game at runtime yet; for the telemetry ingest socket's reconnect,
see [live-telemetry.md](live-telemetry.md).

## The co-op relay

`/v1/sessions/{id}/relay` begins at the hub. The upgrade request carries a
`sessions.relay` bearer token, just as on a machine. After admission, a
`CoopRelay` Durable Object named by the session id forwards datagrams.

- **Membership.** The hub spends the token's `jti`, checks that the
  account is the host or an accepted guest of an open co-op session,
  and signs a 15-second ticket containing the session, XUID, author and
  token expiry. It then forwards the upgrade to the relay object, which
  verifies the ticket before accepting the socket. An `auth` message
  later refreshes the token through `POST /v1/sessions/{id}/relay/ticket`.
- **Rechecks.** On a machine the relay rechecks membership every 5 s. Here,
  a member whose ticket has run out gets a new one from the hub with the
  old ticket (`{"ticket": ...}`, no token), in the background, so no
  datagram waits for it. If the hub says no, the socket closes with 4403.
- **The room** (`crates/live/live-worker/src/relayroom.rs`, generic and tested on
  a machine): two accounts; the same account again replaces its old socket
  (4409); a third is refused (4409); messages over 1,200 bytes close with
  4413, faster than `relay_rate` with 4429; `{kind: "peer", present}`
  when the other arrives or leaves.
- **Hibernation.** Sockets are hibernatable, with the ticket as the
  attachment. The room is rebuilt after a wake, and the object's alarm
  closes sockets when their tokens expire.

The ticket endpoint can also issue a ticket directly to an authenticated
member, and accepts a current ticket for renewal.

## Dedicated host containers

When Live grants a Cloudflare lease, the hub calls the room named by the
lease ID. Its private `start` request asks the class's pool room to place
the match on a pack container with room. Only when no pack has room does a
new pack room launch the image entrypoint with `H2_PACK=1`, which executes
`/opt/h2/h2 host pack`. The pack starts the match's `h2 host run`.
[Packed hosts](dedicated-host.md#packed-hosts) The
host receives the lease ID, Live URL, lease-scoped token, signed bundle URL
and SHA-256. `H2_HOST_LEASE_JSON` carries the complete lease so the
supervisor can check its ID and configure the map and lobby before registration. It fetches the private bundle and registers with Live over
outbound HTTPS. The room keeps the boot credentials out of Durable Object
storage. Registration opens the match for joins.
[Lease and boot contracts](dedicated-host-build.md)

Wrangler configures two container applications in each environment. A
capped application has a fixed size, so `HostRoom` handles Small leases
and `LargeHostRoom` handles Large leases. Each uses the same Dockerfile
and room implementation. Both use 12 GB of scratch disk. Small reserves
1 vCPU and 3 GiB, Large reserves 2 vCPU and 6 GiB. Those memory values
meet the default scheduling policy's 3 GiB per vCPU minimum.
[Scheduling policies](https://developers.cloudflare.com/containers/configuration/scheduling-policy/) ·
[Custom instance constraints](https://developers.cloudflare.com/changelog/post/2026-09-29-remove-disk-to-memory-ratio/)

| Worker setting | Purpose |
|---|---|
| `LIVE_MAX_HOSTS` | Shared cap across both classes. Defaults to 4. Set each application's numeric `max_instances` to the same value when deploying. |
| `LIVE_HOST_PACK_MATCHES` | Matches per host container. Defaults to 3, at most 8. Each container still admits a match only when its measured load leaves room. |
| `LIVE_HOST_PRICES` | Optional JSON price table used to estimate lease costs, including when replaying past host events. |
| `LIVE_HOST_BUDGET_USD` | Estimated dedicated-host spend cap in USD per UTC calendar month. Defaults to $25. Set to `0` to disable new leases. Falls back to `LIVE_HOST_BUDGET_USD_MONTH` when unset. |
| `LIVE_HOST_LIVE_URL` | URL the host can reach for registration and reports. Defaults to Live's public URL. |
| `LIVE_HOST_DATA_URL` | Set as an environment-specific Worker secret containing a signed download URL for the owner's private bundle. Renew before the URL expires. |
| `LIVE_HOST_DATA_SHA256` | SHA-256 of that bundle. Update alongside its signed URL. |

To set the production cap explicitly, add `LIVE_HOST_BUDGET_USD = "25"`
under `[env.production.vars]` in `crates/live/live-worker/wrangler.toml`.
Set the variable separately for each environment that starts containers.
Live reserves budget for the whole match before calling the room to
start. [Cost calculation](dedicated-host.md#cost-and-budget)

Before enabling hosts, create a **$20 budget alert** in the Cloudflare
dashboard under **Manage Account > Billing > Billable Usage**. Enter the
threshold in USD and add the operator's email address. The alert monitors
account-wide usage-based spend over Cloudflare's billing period. Live
enforces its own cap on estimated host spend over the UTC calendar month.
[Cloudflare budget alerts](https://developers.cloudflare.com/billing/manage/budget-alerts/)

`GET /v1/hosts/{lease}/tunnel` takes a fresh `sessions.relay` account
bearer on each WebSocket upgrade. LiveHub checks the roster and match
state, then sends a fresh upgrade to the lease's room. The account bearer
stays in LiveHub. The room forwards the upgrade to TCP port 31100 and
returns the WebSocket response. The supervisor uses the same port through
`H2_TUNNEL_LISTEN=0.0.0.0:31100`. It drives the game through a private
Unix socket. The image's `H2_AGENT` fallback stays on container loopback.
Each binary message carries one tunnel frame end to end. The host assigns
peer IDs and handles the game ports.
[WebSocket forwarding](https://developers.cloudflare.com/containers/examples/websocket/)

Only provider start can launch a container. A liveness check reads
`container.running`. On stop or match end, the provider calls the room to
destroy the running instance. The room also arms an alarm at the lease's
hard deadline before starting compute. It restores the container's
inactivity timeout on wake so bot-only matches can run while the room is
quiet. The game supervisor and Live reaper measure match idle time.
[Container lifecycle API](https://developers.cloudflare.com/containers/api/durable-object-container/)

Before deploying, stage the owner's Linux binary and module under
`deploy/host/stage/`. Wrangler uses `deploy/host/` as the build context
and uploads the resulting image to the account's private registry.
[Preparing the host image](../../deploy/host/README.md)

In staging, exercise both sizes with a private bundle. Check
that the host registers, a rostered client can exchange binary datagrams,
and a different account receives 403. End one lease through a normal match
and stop another from the admin page. Confirm each instance stops in
Cloudflare and that a subsequent tunnel request leaves it stopped.

To check the first boot alarm, use a staging host that starts but withholds
registration. After the cohort forms and the queue responds, stop all client
polls. Once the 60-second boot deadline has passed, check the host log for
`HostEnded` with reason `boot_timeout` and confirm the container stopped.
The event timestamp should precede the first verification request.
[Coordinator integration pass](dedicated-host-build.md#coordinators-integration-pass)

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
   - The `live-worker` crate exists, with the store tested natively, R2 and the hub.
2. **The hub serves HTTP under `wrangler dev` (done).**
   - The log moved into the hub's own storage ([Decision](#decision-the-hubs-own-sqlite-not-d1)).
   - Built with `worker-build`, run with `bunx wrangler@4 dev --local`.
   - [Settings](#settings) come from variables and secrets, and `CF-Connecting-IP` is trusted at the edge.
   - The smoke test passes, and the cold start is measured ([Slice 2 results](#slice-2-results)).
3. **The event stream (done).**
   - Hibernatable WebSockets on the hub ([The stream on the hub](#the-stream-on-the-hub)).
   - One stream implementation, `live_server::stream::StreamSession`, run by axum's per-socket task and by the hub.
   - JSON and packed readers, auth messages and token expiry.
4. **Ingest, timers and uploads (done).**
   - The game's ingest socket on the hub, one session shared with axum.
   - No timers on the hub: an alarm plus work done on demand ([Timers and hibernation](#timers-and-hibernation)). A real hibernation wake is shown under `wrangler dev`.
   - Content and cloud-save uploads stream into the blob store (R2 on the hub, multipart past 8 MiB) instead of a temporary file, on a machine too.
   - Title files are in R2.
   - Results: [Slice 4 results](#slice-4-results).
5. **The co-op relay object (done).**
   - Stream sockets got ingest's per-socket lock; attachments are written on a change of state only.
   - `CoopRelay` per session, membership by the hub's signed ticket ([The co-op relay](#the-co-op-relay)).
   - The forwarding, rate buckets and close codes of `relay.rs`.
   - Two relay clients in the smoke test: [Slice 5 results](#slice-5-results).
6. **Cold start.**
   - Snapshot the read model at a log position in the hub's storage (or R2), and replay only the tail.
   - Needed if the replay is slow at today's log size; see [Slice 2 results](#slice-2-results) for the rate.
7. **Migration tooling and a rehearsal (done).**
   - The operator log endpoints, with writes held while a copy goes in ([Migrating the current data](#migrating-the-current-data)).
   - `store copy` takes `https://HOST` as a source or a destination, signing with the server key.
   - Blobs go to R2 through the `s3://` path.
   - Rehearsed against `wrangler dev` with a read-only copy of the real log ([Rehearsal](#rehearsal-2026-11-18)).
8. **Staging, then cutover.** [Deploy](#deploy), then the [Cutover runbook](#cutover-runbook).

## Deploy

`crates/live/live-worker/deploy.sh` builds and deploys; never run a bare
`wrangler deploy`. Each environment in wrangler.toml is its own Worker, with
its own Durable Objects (so its own log), bucket and key:

| Environment | Worker | Id | Bucket | Reached at |
|---|---|---|---|---|
| `staging` | `h2-live-staging` | `live-staging.forbes.red` (fresh; nothing from production is valid there) | `h2-live-blobs-staging` | `https://h2-live-staging.<subdomain>.workers.dev` |
| `cutover` | `h2-live` | `live.forbes.red` (the Mac's, from its discovery document) | `h2-live-blobs` | workers.dev only: no route |
| `production` | `h2-live` | `live.forbes.red` | `h2-live-blobs` | the route `live.forbes.red/*`, and workers.dev |

`cutover` and `production` are one Worker deployed two ways: `cutover`
without the route (to copy the log into before the switch), `production`
with it (the switch). Both need `--yes-production`.

Every environment declares the dedicated host containers, built from
`deploy/host/Dockerfile` with the image `deploy/host/build.sh` stages. Until
that image is staged, deploy with `--no-hosts`: the script writes
`wrangler.nohosts.toml` beside `wrangler.toml` without the `[[containers]]`
blocks and with `LIVE_MAX_HOSTS = "0"`, so Live refuses host launches and
everything else deploys. Without the image and without the flag it stops
before deploying.

`deploy.sh` prints each step as it goes:

1. **Toolchain.** Rust from rust-toolchain.toml with the wasm32 target and
   `worker-build` 0.9.0, in a rustup of its own under
   `~/.cache/h2-live-worker` (`$H2_WORKER_HOME`). `~/.rustup` and
   `~/.cargo` are not touched. The first run installs it.
2. **Web.** `bun install --frozen-lockfile` and `bun run build` into
   `web/dist`, the Worker's static assets.
3. **Wasm.** `worker-build --release` into `crates/live/live-worker/build`.
4. **Sign-in.** `wrangler whoami`; if you are not signed in, it stops and
   says to run `bunx wrangler@4 login`.
5. **Bucket.** It creates the environment's R2 bucket if it is missing.
6. **Key.** It asks for the key only if the Worker has no
   `H2_LIVE_SERVER_KEY` secret:
   - **staging:** it makes a fresh one;
   - **cutover and production:** it reads `$H2_LIVE_KEY_FILE`, or asks for
     the Mac's `server.key` without echoing it. It then checks that the
     key's public half is the one `https://live.forbes.red/.well-known/h2live.json`
     publishes.

   The key goes to `wrangler secret put` on stdin, after the deploy (the
   Worker has to exist to hold it). It is never printed.
7. **Deploy.** `wrangler deploy --env <env>`.
8. **Health.** `/healthz` and the discovery document's id.

`--dry-run` builds and bundles without signing in or uploading.
`--skip-build` deploys what `build/` and `web/dist` already hold.

### Staging

```
crates/live/live-worker/deploy.sh staging
# discovery's url: the workers.dev address the first deploy printed
H2_LIVE_PUBLIC_URL=https://h2-live-staging.<subdomain>.workers.dev \
  crates/live/live-worker/deploy.sh staging --skip-build
```

Staging is invite-only with `afr` as its admin, like production. Its log
starts empty, until slice 7's copy fills it.

### The site

What the Mac's server serves, and where it comes from on the Worker:

| Path | Mac | Worker |
|---|---|---|
| `/v1/*`, `/healthz`, `/.well-known/h2live.json` | the router | the router, in the hub (`run_worker_first`) |
| `/assets/*` | `web/dist`, `public, max-age=31536000, immutable` | static assets, the same header from `web/public/_headers` |
| App routes | `web/dist/index.html` with page metadata, `no-cache` | the Worker injects the page metadata into the static `index.html`, `no-cache` |
| `/art/*` (the web's own art pack) | `web/dist` | static assets |
| `/art/game/*` (art from the owner's disc) | served from `game-art/` (`--share-game-art`) | a redirect to the published copy (`H2_LIVE_GAME_ART_URL`), answered by the Worker without waking the hub, else a JSON 404 |
| `/art/emblem/*` | composed from local game art, under the same sharing rules | composed from the published art in the Worker, without waking the hub |

A game page's header model travels with the hub's existing
`GET /v1/page-meta` reply. If the hub supplies a game preview without a
header, the Worker reads `GET /v1/games/{id}` anonymously and builds the
model from the matching game detail. The hub's generic metadata response
skips the fallback, and the detail read enforces the same anonymous
visibility rules. Both homes use the same renderer to put the header
and its emblem URLs in an inert template in the initial HTML. Game Details
mounts it below the breadcrumbs.
[Worker page rendering](../../crates/live/live-worker/src/site.rs)

The Worker routes `/art/emblem/*` ahead of static assets. The stable
appearance URL redirects to a revision derived from the published art
directory and the PNG renderer version. On a cache hit the Worker serves
the PNG directly. On a miss it reads the manifest and two mask PNGs from
`H2_LIVE_GAME_ART_URL` and composes them with the native server's Rust
renderer. The Worker caches successful PNGs under their full revisioned
URL, per data center, with `public, max-age=31536000, immutable`. Changing
the published directory changes the revision. The stable redirect
revalidates and errors are uncached.
[Workers Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/)

The `/v1/` cache rules (`public, … s-maxage=5` with an `ETag` for
anonymous reads, `private, no-store` for the rest) are the router's, so
they hold on the Worker too.

The Worker's source art is the published copy. Extracting everything the
viewer and the stats site use
(Halo 2's maps, the map pack's, the Halo 1 maps converted from the Halo 1
disc, models, pictures) and publishing it once from the Mac:

```
ART=$TMPDIR/live-art
h2 live assets extract --disc disc/halo2.iso --mappack disc/mappack.iso \
  --halo1 disc/halo1.iso --out "$ART"
export H2_LIVE_S3_PUBLIC_ENDPOINT=https://<account id>.r2.cloudflarestorage.com \
       H2_LIVE_S3_PUBLIC_ACCESS_KEY_ID=… H2_LIVE_S3_PUBLIC_SECRET_ACCESS_KEY=…
h2 live assets publish --to s3://live-art --url https://live-art.forbes.red --out "$ART"
# prints what it holds (maps, maps (halo 1), textures, views, models, ...)
# and: H2_LIVE_GAME_ART_URL = "https://live-art.forbes.red/art/<stamp>"
```

`--halo1` converts the Halo 1 maps into the runtime data root (`h2 play`'s;
`--installed DATA` names another) first, only what is missing. Publishing
refuses art whose manifest names a file it lacks (the web app would ask
for it).

What goes up ([live-art.md](live-art.md#packing)): every file under
`art/<stamp>/`, the stamp a hash of the whole extraction, each object with
its `Content-Type` and `Cache-Control: public, max-age=31536000,
immutable` (a new extraction is a new stamp, so nothing is ever replaced
in place); glbs and JSON as their Brotli copies with `Content-Encoding:
br`, pictures (WebP, PNG) as they are. The Worker's redirect for
`/art/game/<file>` is `307` with `Cache-Control: public, max-age=300` (the
manifest and anything unversioned) or `public, max-age=31536000,
immutable` (a `?v=<stamp>` address). The web app then fetches every other
file straight from the bucket, from the directory the manifest was
redirected to (`web/src/lib/gameart.ts` `artBase`), so a map's dozens of
pictures cost no redirects.

Put that line in each environment's `vars` in wrangler.toml and deploy
again. The bucket needs public access (its r2.dev address or a custom
domain), and a CORS rule allowing `GET` from any origin. The web app
fetches `manifest.json` with `fetch`, which follows the redirect
cross-origin:

```
bunx wrangler@4 r2 bucket cors set live-art --file crates/live/live-worker/art-cors.json
```

A new extraction is a new stamp: publish it again and change the variable.

## Migrating the current data

The hub's storage can be reached only through the Worker, so the log goes
in through the hub's operator endpoints (`crates/live/live-server/src/logio.rs`):

| Route | Token | |
|---|---|---|
| `GET /v1/admin/log?after=N&limit=M` | operator, `log.read` | events after N as stored: position, version, hash and JWS (a batch's JWS once a page) |
| `POST /v1/admin/log` | operator, `log.write` | imports a page. Each event must follow the head and continue its stream's version and hash chain (`EventStore::import`), or the page is refused whole with a 409 that says why. It answers once the read model has the page. |
| `POST /v1/admin/log/release` | operator, `log.write` | ends the copy: the server writes again |
| `GET /v1/admin/log/meta` | operator, `log.read` | the head, whether writes are held, the title-file blobs, the checkpoints |
| `GET`/`PUT /v1/admin/log/blobs/{sha256}`, `PUT /v1/admin/log/checkpoints/{name}` | | title-file blobs and checkpoints |

An operator token is signed with the server's own key, so only whoever holds
`server.key` can read the log whole or write into it.

**Writes are held while a log is copied in** (`crates/live/live-server/src/hold.rs`).
The first import holds every write of the server's own, and of its clients,
until `--release`. Without it, the hub's sweep sees a copied sign-in or game
gone quiet and appends `PresenceExpired` or `GameEnded` at its head. The
rehearsal showed this: two such events after the first page, so the next
page no longer followed. The hold is a checkpoint row, so it survives the
hub being evicted. It is not part of the log a copy takes. While held,
writes answer 503 "writes are held while the log is copied in".

`h2 live store copy` (`h2-live-server store copy`) takes a server as either
end:

```
h2 live store copy --from postgres://localhost/h2live \
  --to https://h2-live.<subdomain>.workers.dev --operator "<data dir>" [--release]
```

- It goes in pages of 500 events, one request each. The hub checks positions, stream versions and hash chains.
- **Resumable.** Run it again and it carries on from the hub's head, once it has checked that the hub's last event is the source's at that position. A different log is refused, not merged.
- **Checked at the end.** It ends by comparing the destination's last event (JWS, hash, version) with the source's at that position, and prints both heads.
- **`--operator DIR`** is the destination server's `server.key` and `server.json`. It defaults to `--data`. The copy first checks that the server's discovery document names that id and key.
- **A `postgres://` source is opened read-only:** no schema, every transaction `default_transaction_read_only`.
- **`--release`** releases the hub once the copy has caught up with the source. It refuses if the source has moved past it.
- **Back out:** `--from https://… --to postgres://…`, for a rollback.
- **Blobs** (content, cloud saves) go straight to R2 over its S3 API: `--from-blobs DIR --to-blobs s3://BUCKET`, with `H2_LIVE_S3_ENDPOINT`, `H2_LIVE_S3_ACCESS_KEY_ID` and `H2_LIVE_S3_SECRET_ACCESS_KEY`. A blob already there at the same size is skipped.

### Rehearsal (2026-11-18)

The source was a read-only copy of the real log: Postgres to a SQLite file
in scratch, 6264 events (92 MB, the largest JWS 18 KB, well under a
Durable Object row's 2 MB), copied in 22.5 s. The destination was the
Worker under `wrangler dev`, with id `live.forbes.red` and a dev key.

| Step | Result |
|---|---|
| Copy, killed after 1500 events | the hub at 1500, nothing of its own written (held) |
| Run again | carried on at 1501: 4764 events in 4 s; verified at 6264 |
| Public reads against live.forbes.red (also at 6264) | `/v1/accounts`, `/v1/games?state=ended`, `/v1/teams`, `/v1/maps`, `/v1/sessions`: identical. `/v1/stats/summary`: identical except the order of tied map variants. That order was not deterministic; the map id now breaks the tie (`stats.rs`). |
| Back out to a fresh SQLite file | 6264 events in 16 s; every position and hash the same as the source |
| `--release` | the hub writes again |
| Blobs, the Mac's data dir to a directory | 10 files, 4.2 MB; a second run copied nothing |
| Cold start on that log, hub restarted | 1.38 s to replay 6264 events (`server-timing: start;dur=1376`) |

The first attempt, without the hold, failed exactly as described above.
That is why the hold exists.

### Staging rehearsal (the user's step)

Staging has its own id, so the copied accounts can't sign in there; their
author ids name live.forbes.red. The copy, the checks and the reads are
what this rehearses. `deploy.sh staging` keeps staging's key as
`~/.config/h2-live-worker/staging/server.key`, for `--operator`.

```
crates/live/live-worker/deploy.sh staging
~/.cargo/bin/h2 live store copy --from postgres://localhost/h2live \
  --to https://h2-live-staging.<subdomain>.workers.dev \
  --operator ~/.config/h2-live-worker/staging --release
curl -s https://h2-live-staging.<subdomain>.workers.dev/healthz
```

`--operator ~/.config/h2-live-worker/staging` is refused with "is server
\"live-staging.forbes.red\"…" if staging's key was made before deploy.sh
kept it. Fix that with `bunx wrangler@4 secret delete H2_LIVE_SERVER_KEY
--env staging`, then `deploy.sh staging` again.

To start staging over, delete it with `bunx wrangler@4 delete --env staging`,
which removes its Durable Objects and so its log, and deploy again.

## Cutover runbook

Run these in order on the Mac, from the main checkout on master. `<sub>`
is the account's workers.dev subdomain, and DATA is the Mac server's data
directory.

```
DATA="$HOME/Library/Application Support/h2recomp/live/server"
```

**0. Install the h2 binary that knows the new commands.**
```
lockf -k $TMPDIR/cargo.lock cargo install --locked --force --path crates/apps/cli --bin h2
~/.cargo/bin/h2 live --help | grep -E "assets publish|store copy"
bunx wrangler@4 login
```

**1. Publish the disc art to a public bucket.** This is once, and the Mac
keeps serving meanwhile.
1. Make the bucket, public with CORS:
   ```
   bunx wrangler@4 r2 bucket create live-art
   bunx wrangler@4 r2 bucket domain add live-art --domain live-art.forbes.red --zone-id <forbes.red zone id>
   bunx wrangler@4 r2 bucket cors set live-art --file crates/live/live-worker/art-cors.json
   ```
   `bunx wrangler@4 r2 bucket dev-url enable live-art` gives an r2.dev
   address instead of a custom domain; it is rate limited, so it is for
   trying it out.
2. Make an R2 API token with Object Read & Write on `live-art` and
   `h2-live-blobs` (dashboard: R2, Manage API tokens).
3. Extract (with the Halo 1 maps) and upload, with the token's keys:
   ```
   ~/.cargo/bin/h2 live assets extract --disc disc/halo2.iso --mappack disc/mappack.iso \
     --halo1 disc/halo1.iso --out "$DATA/game-art"
   export H2_LIVE_S3_ENDPOINT=https://<account id>.r2.cloudflarestorage.com
   export H2_LIVE_S3_ACCESS_KEY_ID=… H2_LIVE_S3_SECRET_ACCESS_KEY=…
   ~/.cargo/bin/h2 live assets publish --to s3://live-art --url https://live-art.forbes.red --out "$DATA/game-art"
   ```
4. It prints `H2_LIVE_GAME_ART_URL = "https://live-art.forbes.red/art/<stamp>"`.
   Put that line under `[env.cutover.vars]`, `[env.production.vars]` and
   `[env.staging.vars]` in `crates/live/live-worker/wrangler.toml`.
5. Check: `curl -sI -H 'Origin: https://live.forbes.red' -H 'Accept-Encoding: br'
   https://live-art.forbes.red/art/<stamp>/manifest.json` answers 200 with
   `content-encoding: br`, `cache-control: public, max-age=31536000,
   immutable`, `content-type: application/json` and
   `access-control-allow-origin`.

**2. Deploy the Worker without the route.** It holds the real key, from
the Mac's `server.key`, which deploy.sh checks against the public key at
https://live.forbes.red/.well-known/h2live.json before putting it:
```
H2_LIVE_KEY_FILE="$DATA/server.key" crates/live/live-worker/deploy.sh cutover --yes-production
curl -s https://h2-live.<sub>.workers.dev/healthz        # {"ok":true,"position":0}
curl -s https://h2-live.<sub>.workers.dev/.well-known/h2live.json   # id live.forbes.red, the Mac's key
```

**3. Bulk copy, with the Mac still serving.** This holds the Worker's
writes until step 6:
```
~/.cargo/bin/h2 live store copy --from postgres://localhost/h2live \
  --to https://h2-live.<sub>.workers.dev --operator "$DATA"
~/.cargo/bin/h2 live store copy --from-blobs "$DATA" --to-blobs s3://h2-live-blobs
```
Run either again if it stops; each carries on. On today's log this takes
under a minute.

**4. Stop the Mac's writes.** This starts the downtime: from here until
step 7, live.forbes.red answers errors (the tunnel has no server behind it).
The user does this, not an agent. The tunnel can stay up until step 7. From
a terminal, stop the server and keep a login from starting it again:
```
launchctl bootout gui/$(id -u)/red.forbes.h2live.server
launchctl disable gui/$(id -u)/red.forbes.h2live.server
```
Check it is stopped: `curl -s http://127.0.0.1:8074/healthz` fails to
connect.

**5. Copy the tail.** This is the same command plus `--release`. It copies
what the Mac wrote since step 3, verifies, and lets the Worker write again:
```
~/.cargo/bin/h2 live store copy --from postgres://localhost/h2live \
  --to https://h2-live.<sub>.workers.dev --operator "$DATA" --release
~/.cargo/bin/h2 live store copy --from-blobs "$DATA" --to-blobs s3://h2-live-blobs
```
It must end with "verified: the logs agree up to N … the source is at N"
and "released". If it says the source is past the head, the Mac is still
writing: go back to step 4.

**6. Switch: deploy with the route.**
```
crates/live/live-worker/deploy.sh production --yes-production --skip-build
```
The route `live.forbes.red/*` puts the Worker in front of the proxied
record. The DNS record and the tunnel's configuration stay as they are.

**7. Verify.**
```
curl -s https://live.forbes.red/healthz                 # the position from step 5, or just past it
curl -sI https://live.forbes.red/ | grep -i cache-control   # public, max-age=0, must-revalidate (static assets)
curl -s https://live.forbes.red/.well-known/h2live.json  # id live.forbes.red
curl -sI https://live.forbes.red/art/game/manifest.json | grep -i location   # https://live-art.forbes.red/art/<stamp>/…
```
Then sign in from the game (`h2 play`) and from the web, and watch a game
or a co-op session go through. Finally, in the dashboard under Workers &
Pages, h2-live, Observability:
- the requests from just now show in **Logs**;
- a request shows in **Traces**.

`bunx wrangler@4 tail --env production` shows them live.

**8. Leave the Mac's server and tunnel stopped.** They are the rollback.
Stop the tunnel (`launchctl bootout` and `launchctl disable` for its agent, as
in step 4) once the Worker has served for a day.

## Rollback

1. **Remove the route.** Dashboard: Workers & Pages, h2-live, Settings,
   Domains & Routes; delete `live.forbes.red/*`. Traffic falls through to
   the tunnel again (start it first if step 8 stopped it). Redeploying
   `cutover` does not remove a route.
2. **Copy the Worker's writes back to Postgres.** Do this before starting
   the Mac's server:
   ```
   ~/.cargo/bin/h2 live store copy --from https://h2-live.<sub>.workers.dev \
     --to postgres://localhost/h2live --operator "$DATA"
   ~/.cargo/bin/h2 live store copy --from-blobs s3://h2-live-blobs --to-blobs "$DATA"
   ```
   This is a continuation of the Mac's own log, so it is accepted. A log
   that forked is refused. Events the Worker writes after this copy (its
   sweeps, with no clients left) are not carried back. They are server
   housekeeping that the Mac's own sweep redoes.
3. **Start the Mac's server:** `launchctl enable` then `launchctl bootstrap
   gui/$(id -u) ~/Library/LaunchAgents/red.forbes.h2live.server.plist`,
   or run `h2 live serve` by hand.
4. Check `curl -s https://live.forbes.red/healthz` shows the position
   from step 2.

Only one side may write at a time. The hold (step 3 to step 5) and
stopping the Mac (step 4) keep it that way. Never run both against the
same hostname.

## Observability

Set in wrangler.toml, per environment (`[env.<env>.observability]`):

| | staging | cutover | production |
|---|---|---|---|
| Workers Logs (`head_sampling_rate`) | 1 | 1 | 1 |
| Invocation logs | on | on | on |
| Traces (`observability.traces.head_sampling_rate`) | 1 | 1 | 0.1 |

- **Logs** are what the Worker writes with `console.log`, plus one
  invocation log a request.
  - Workers Paid includes 20 million log events a month.
  - Today's traffic (a few players, viewers polling) is far below that,
    so production starts at 1. Lower it when the dashboard's monthly count
    passes about half the quota.
- **Traces** show each request's path through the Worker, the Durable
  Object call (hub or relay) and R2.
- **Live:** `bunx wrangler@4 tail --env production` (add
  `--format pretty`, or `--search <text>`).

Not done yet; this is the next slice after the cutover:
- structured JSON log lines from the Rust side (level, route, author where
  safe, game or session id, duration, log position; never tokens, keys,
  codes or addresses);
- our own spans or trace-id lines inside the hub;
- OTLP export to an external backend, as a setting that is off unless set;
- Analytics Engine counters wired into the admin server screen.

## What the Mac does afterwards

Nothing, as a server:
- the game (`h2 play`) talks to `live.forbes.red` as before;
- the web app is served by Cloudflare.

The Mac still does these:
- **Publishing game art.** It needs the disc, and it uploads to the public bucket through the existing `s3://` path.
- **Deploys.** `wrangler deploy` from a checkout.

`h2 live serve` keeps working for local and offline servers, where the
machine's SQLite or Postgres log stays the default.
