# Live Worker: deploy, cutover and rollback

Part of [The Live server on Cloudflare Workers](live-worker.md). Covers: Deploy; Migrating the current data; Cutover runbook; Rollback; Observability; What the Mac does afterwards.

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

Every environment declares the dedicated host containers. They run the
registry image `h2-live-host` at the content tag of the staged `h2`
([Dedicated host containers](live-worker.md#dedicated-host-containers)).
`deploy.sh --no-hosts` (with the environment name) omits the container
applications and sets the lease cap to zero. For a host
deployment, edit the environment's variables in `wrangler.toml`.
`python3 crates/live/live-worker/host_capacity.py` prints the effective
config, including each application's container limit.

The deploy script
passes this config to Wrangler and removes its temporary copy on exit.

`deploy.sh` prints each step as it goes:

1. **Toolchain.** Rust from rust-toolchain.toml with the wasm32 target and
   `worker-build` 0.9.0, in a rustup of its own under
   `~/.cache/h2-live-worker` (`$H2_WORKER_HOME`). `~/.rustup` and
   `~/.cargo` are not touched. The first run installs it.
2. **Web.** `bun install --frozen-lockfile` and `bun run build` into
   `web/dist`, the Worker's static assets.
3. **Wasm.** `worker-build --release` into `crates/live/live-worker/build` (build output).
4. **Sign-in.** `wrangler whoami`; if you are not signed in, it stops and
   says to run `bunx wrangler@4 login`.
5. **Host image.** The tag is a hash of the Dockerfile, the entrypoint
   and the Linux `h2` that `deploy/host/build.sh --h2` stages in
   `deploy/host/stage/h2`. A tag the registry lacks is built and pushed
   (this needs Docker; without it the script stops and says so). Without
   a staged `h2` the script stops and says to stage one or pass
   `--no-hosts`. Skipped when the environment's lease cap is zero.
6. **Bucket.** It creates the environment's R2 bucket if it is missing.
7. **Key.** It asks for the key only if the Worker has no
   `H2_LIVE_SERVER_KEY` secret:
   - **staging:** it makes a fresh one;
   - **cutover and production:** it reads `$H2_LIVE_KEY_FILE`, or asks for
     the Mac's `server.key` without echoing it. It then checks that the
     key's public half is the one `https://live.forbes.red/.well-known/h2live.json`
     publishes.

   The key goes to `wrangler secret put` on stdin, after the deploy (the
   Worker has to exist to hold it). It is never printed.
8. **Deploy.** `wrangler deploy --env <env>`; its failure stops the script.
9. **Health.** `/healthz` and the discovery document's id.

`--dry-run` builds and bundles without signing in or uploading.
`--skip-build` deploys what `build/` and `web/dist` already hold.

### Deploy from CI

`.github/workflows/live-deploy.yml` runs `deploy.sh production
--yes-production` on each push to master once the Linux workflow
(`linux.yml`) passes for it, and on demand. The macOS and other platform
workflows do not hold it back: the hosts run Linux. Before it, the
job installs Rust from `rust-toolchain.toml`, Zig and cargo-zigbuild and
runs `deploy/host/build.sh --h2`, so the deploy builds and pushes the
host image for that commit's `h2` when the registry lacks its tag.

The
image holds public code only. The game module stays in the private
host-data bundle; when a commit changes the generator, the owner
refreshes it there
([The game module](dedicated-host-build.md#the-game-module)). The job reads two repository secrets:
`CLOUDFLARE_API_TOKEN` (a token with Workers Scripts, Workers Routes,
Containers and R2 edit, and Account Settings read) and
`CLOUDFLARE_ACCOUNT_ID`.

Without the token it skips and deploys nothing.
The server key and the host-data secrets stay on the Worker.

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
visibility rules.

Both homes use the same renderer to put the header
and its emblem URLs in an inert template in the initial HTML. Game Details
mounts it below the breadcrumbs.
[Worker page rendering](../../crates/live/live-worker/src/site.rs)

The Worker routes `/art/emblem/*` ahead of static assets. The stable
appearance URL redirects to a revision derived from the published art
directory and the PNG renderer version. On a cache hit the Worker serves
the PNG directly.

On a miss it reads the manifest and two mask PNGs from
`H2_LIVE_GAME_ART_URL` and composes them with the native server's Rust
renderer. The Worker caches successful PNGs under their full revisioned
URL, per data centre, with `public, max-age=31536000, immutable`; a
request that bears `Authorization` never reads from or fills that cache,
so a cache key holds no auth. Changing the published directory changes the
revision.

The stable redirect revalidates and errors are uncached.
[Workers Cache API](https://developers.cloudflare.com/workers/runtime-apis/cache/)

The `/v1/` cache rules (`public, … s-maxage=5` with an `ETag` and
`Vary: Authorization` for anonymous reads, `private, no-store` for the rest) are
the router's, so they hold on the Worker too.

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

What goes up ([live-art.md](live-art-pipeline.md#packing)): every file under
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
page did not follow.

The hold is a checkpoint row, so it survives the
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

### Rehearsal

The copy, resume, back-out and hold behaviour above was rehearsed on a
read-only copy of the real log; the steps and timings are in
[the Worker rehearsal note](notes/2026-11-18-worker-rehearsal.md).

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
- **Refusals:** every 4xx and 5xx the Worker returns is logged once, at
  its fetch handler (`crates/live/live-worker/src/entry.rs`), as
  `refused <METHOD> <path> <status> code=<error> message="<message>"`: the
  path keeps its ids and drops the query string, and the code and message
  come from the `ApiError` body (cut at 300 characters). 5xx go to
  `console.error`. Headers, tokens, query strings and request bodies are
  never logged. `h2 live serve` writes the same line to stderr
  (`live_server::refusals`).
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
