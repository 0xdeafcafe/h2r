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
| Screenshot classification | Workers AI binding `SCREENSHOT_AI` | A shared server classifier adapter sends only the normalised WebP to Clef Flash and reads its NSFW score. Without a valid result, the image stays pending. |
| Title files | R2, under `title/<sha256>` (`live_server::titleblobs`) | A Durable Object's SQLite row holds at most 2 MB, but a title file may be up to 2 MiB (`MAX_TITLE_FILE`). Ones already in the log's `blobs` table are still read from there. |
| Server key | Worker secret `H2_LIVE_SERVER_KEY` | `ServerIdentity::from_secret`. The id is the variable `H2_LIVE_ID` ([Settings](#settings)). |
| Web app | Workers static assets (`web/dist`) | Served by Cloudflare ahead of the Worker; it is immutable and cached at the edge. |

Anonymous web Theater reads of a complete public time chunk use the Worker
Cache API. The key includes the origin, game, immutable upload revision and
exact byte span. A miss goes to LiveHub. A successful 206 carrying the hub's
immutable film policy can populate the cache at that edge location.

The Worker
restores the original Content-Range on a hit. Private homes, bearer and cookie
requests, mutable visibility and metadata reads continue through LiveHub.

### Decision: the hub's own SQLite, not D1

Decided in slice 2. The log lives in `LiveHub`'s own SQLite storage. That
storage is in-process and synchronous, and the hub is its only writer, so an
append is a few local statements.

D1 would cost a network hop for every
query: one read per stream an append touches, then one batch.

The store is `sqlstore::SqlStore` over the `Sql` trait:
- in the Worker it runs on the hub's storage (`dosql.rs`);
- in the tests it runs on sqlx's SQLite.

Anything else that runs SQLite statements in an atomic batch, D1 included,
fits the same trait. D1 is out of the hot path, and migration does not need
it either ([Migrating the current data](live-worker-deploy.md#migrating-the-current-data)).

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

The variables and secrets are listed under Live Worker settings in [switches.md](../runtime/switches.md#live-worker-settings).

A missing id or key, or a bad value, fails the hub's start. The request then
answers 500 with the reason; the hub never runs as some other server.

**The client address.** A Worker has no peer address, and every request comes
through Cloudflare's edge, which sets `CF-Connecting-IP` itself. The hub
marks its public URL as at the edge (`PublicUrl::set_edge`), so that header
is the client's address.

On a machine the header still counts only from loopback behind a tunnel;
anywhere else it can be forged.

## What changed to make it build (slice 1)

Moved to [Live Worker: build slices and results](live-worker-slices.md#what-changed-to-make-it-build-slice-1).

### live-server

### live-proto

### crates/live/live-worker

### Building and running on this machine

## The stream on the hub

The hub and native server advertise the same v8 ingest support in
discovery. Packed subscribers opt in separately. The hibernation
attachment retains each subscriber's version preference.
[Packed frames](live-telemetry-packed.md#version-8-live-wire)

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
    with 100 sockets ([measurements](notes/2026-11-18-worker-slice-results.md#slice-5-commit-725ba630d-2026-11-18)).
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
in batched mode.

The attachment keeps the author, token expiry and
ingest mode. After a wake, the sender's next keyframe restarts the packed
decoder. [Ingest wire format](live-api-social.md#streams-websocket)

## Timers and hibernation

In-memory timers keep a Durable Object awake. On a machine
the server spawns loops: the dispatcher's poll, the sweep every 5 s and the
100 ms packed tick. The hub starts the server with `Timers::Driven`
instead, which spawns none of them:
- The dispatcher waits only for the store's wake. The hub's store has one
  writer, and its wake sees every append.
- After every request, socket message and alarm, the hub sets its one
  alarm (`LiveHub::schedule`) for the earliest of:
  - `Live::sweep_due_with_hosts`: the next presence, game, session, film,
    dedicated host or usage deadline;
  - each stream or ingest socket's auth deadline or token end.

  With nothing due there is no alarm. The alarm runs the sweep and the
  sockets' checks, then sets the next one.
- Metered [usage](live-api.md#usage) accumulates in memory and flushes to
  SQLite after five seconds or 256 meter increments, whichever comes first.
  Requests and ingest messages both arm the sweep while a batch is pending.
  The flush deadline is shorter than Cloudflare's usual ten-second idle
  period before hibernation. A deploy or abrupt reset can discard a pending
  batch. [Durable Object lifecycle](https://developers.cloudflare.com/durable-objects/concepts/durable-object-lifecycle/)
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
the match on a pack container with room. An eligible search can start a pack
before a lease exists. When placement finds no room, a new pack room launches
the image entrypoint with `H2_PACK=1`, which executes
`/opt/h2/h2 host pack`.

The pack starts the match's `h2 host run`.
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
and room implementation.

Both use 12 GB of scratch disk. Small reserves
1 vCPU and 3 GiB, Large reserves 2 vCPU and 6 GiB. Those memory values
meet the default scheduling policy's 3 GiB per vCPU minimum.
[Scheduling policies](https://developers.cloudflare.com/containers/configuration/scheduling-policy/) ·
[Custom instance constraints](https://developers.cloudflare.com/changelog/post/2026-09-29-remove-disk-to-memory-ratio/)

Both applications run one image from the account's Cloudflare registry,
`h2-live-host`, tagged with a hash of what `deploy/host/Dockerfile`
copies: the Dockerfile, the entrypoint and the binaries `deploy/host/build.sh` stages.
`crates/live/live-worker/deploy.sh` computes the tag and checks the
registry. A tag the registry holds is deployed as it is, so a deploy
needs Docker only when the host binaries change; then it builds and
pushes the image first (`wrangler containers build --push`) and stops
with a named error if Docker is not running. The generated config names
`registry.cloudflare.com/<account>/h2-live-host:<tag>` in place of the
Dockerfile. [Image management](https://developers.cloudflare.com/containers/platform-details/image-management/)

| Worker setting | Purpose |
|---|---|
| `LIVE_MAX_HOSTS` | Shared lease cap across both classes. The default is 4. Operators may select 0–4 or a measured fleet step (16, 64, 256, 1,024). Deployment calculates a separate container limit for each class. [Fleet caps](host-halving.md#caps) |
| `LIVE_HOST_PACK_MATCHES` | Optional slot cap per container. Small packs have five slots and large packs ten. A numeric value lowers the class cap. Admission also checks gameplay tick p99, CPU and memory. |
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
account-wide usage-based spend over Cloudflare's billing period.

Live
enforces its own cap on estimated host spend over the UTC calendar month.
[Cloudflare budget alerts](https://developers.cloudflare.com/billing/manage/budget-alerts/)

`GET /v1/hosts/{lease}/tunnel` takes a fresh `sessions.relay` account
bearer on each WebSocket upgrade. LiveHub checks the roster and match
state, then sends a fresh upgrade to the lease's room. The account bearer
stays in LiveHub.

The room forwards the upgrade to TCP port 31100 and
returns the WebSocket response. The supervisor uses the same port through
`H2_TUNNEL_LISTEN=0.0.0.0:31100`. It drives the game through a private
Unix socket.

The image's `H2_AGENT` fallback stays on container loopback.
Each binary message carries one tunnel frame end to end. The host assigns
peer IDs and handles the game ports.

LiveHub forwards the optional
`x-h2-actor-channel: 1` upgrade offer. The host's binary hello acknowledges
the offer and the same socket carries actor frames.
[Host tunnel](dedicated-host-ops.md#host-tunnel)
[WebSocket forwarding](https://developers.cloudflare.com/containers/examples/websocket/)

Only provider start can launch a container. A liveness check reads
`container.running`. On stop or match end, the provider calls the room to
destroy the running instance.

The room also arms an alarm at the lease's
hard deadline before starting compute. It restores the container's
inactivity timeout on wake so bot-only matches can run while the room is
quiet. The game supervisor and Live reaper measure match idle time.
[Container lifecycle API](https://developers.cloudflare.com/containers/api/durable-object-container/)

Before deploying, stage the Linux `h2` in `deploy/host/stage/h2`
(`deploy/host/build.sh --h2`; CI does this on every run). Wrangler uses
`deploy/host/` as the build context and uploads the resulting image to
the account's registry. The image holds public code only; hosts take the
game module from the private host-data bundle.
[Preparing the host image](../../deploy/host/README.md) · [The game module](dedicated-host-build.md#the-game-module)

In staging, exercise both sizes with a private bundle. Check
that the host registers, a rostered client can exchange binary datagrams,
and a different account receives 403. End one lease through a normal match
and stop another from the admin page.

Confirm each instance stops in
Cloudflare and that a subsequent tunnel request leaves it stopped.

To check the first boot alarm, use a staging host that starts but withholds
registration. After the cohort forms and the queue responds, stop all client
polls. Once the 60-second boot deadline has passed, check the host log for
`HostEnded` with reason `boot_timeout` and confirm the container stopped.

The event timestamp should precede the first verification request.

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


## Deploy

Moved to [Live Worker: deploy, cutover and rollback](live-worker-deploy.md#deploy).

### Deploy from CI

### Staging

### The site

## Migrating the current data

Moved to [Live Worker: deploy, cutover and rollback](live-worker-deploy.md#migrating-the-current-data).

### Rehearsal

### Staging rehearsal (the user's step)

## Cutover runbook

Moved to [Live Worker: deploy, cutover and rollback](live-worker-deploy.md#cutover-runbook).

## Rollback

Moved to [Live Worker: deploy, cutover and rollback](live-worker-deploy.md#rollback).

## Observability

Moved to [Live Worker: deploy, cutover and rollback](live-worker-deploy.md#observability).

## What the Mac does afterwards

Moved to [Live Worker: deploy, cutover and rollback](live-worker-deploy.md#what-the-mac-does-afterwards).
