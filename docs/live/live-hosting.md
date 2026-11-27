# Hosting a Live server for friends (Cloudflare Tunnel)

`h2 live serve` binds loopback and refuses anything else without TLS
([live-security.md](live-security.md) §6). The easy way to let friends in
over the internet, with HTTPS and no port forwarding, is Cloudflare
Tunnel: `cloudflared` runs on your machine, connects out to Cloudflare, and
forwards each request (WebSockets included) to the server on loopback.
`--live-tunnel` (`h2 play`) and `--tunnel` (`h2 live serve`) run it for you.
For a hosted setup with no Mac involved, see [live-worker.md](live-worker.md) (Cloudflare Workers).

```
brew install cloudflared

h2 live serve --tunnel                   # quick tunnel: https://<random>.trycloudflare.com
h2 play disc.iso --live-tunnel           # the embedded server, published the same way

# a lasting address, set up on first use (Cloudflare account and domain needed)
h2 play disc.iso --live-tunnel --live-tunnel-name h2play --live-tunnel-url https://play.forbes.red

# friends
h2 play disc.iso --live-server https://<random>.trycloudflare.com
```

On `h2 play`, `--live-tunnel` implies `--live-local` (the embedded server;
it does not go with `--live` or `--live-server`). The older spellings still
work: `--tunnel`, `--tunnel-name NAME` for `--live-tunnel-name`,
`--hostname HOST` for `--live-tunnel-url https://HOST`. The URL must be
`https://HOST` with no port and no path beyond `/`.

Implemented in `crates/apps/cli/src/tunnel.rs` (flags in `live_cmd.rs`,
`play_live.rs`); the server side is `crates/live/live-server/src/public.rs`.

## Quick tunnel (`--tunnel`)

No Cloudflare account. `serve` starts

```
cloudflared tunnel --no-autoupdate --grace-period 1s --url http://127.0.0.1:<port>
```

reads the `https://<name>.trycloudflare.com` URL from its log, and:

- prints it (`h2 live: public at https://...`) with the warnings below;
- publishes it in discovery as `url` (`GET /.well-known/h2live.json`).
  `api` stays `/v1`, relative, so a client on this machine keeps talking
  to loopback and one outside keeps talking to the tunnel;
- stops cloudflared when the server stops (also on Ctrl-C: it shares the
  terminal's process group);
- restarts cloudflared with backoff (2 s doubling to 60 s) if it dies. A
  restarted quick tunnel gets a new URL, which is printed and put in
  discovery.

`--listen` must stay loopback with a fixed port (default
`127.0.0.1:8074`), and `--tunnel` does not go with `--tls-cert`:
cloudflared terminates HTTPS at Cloudflare and talks plain HTTP to
loopback.

Cloudflare's terms for quick tunnels: no uptime guarantee, for trying
things out. They carry WebSockets (the event stream and the game's ingest
socket) as they are; nothing needs configuring.

### The URL changes every run

That does not break accounts. Accounts are tied to the **server id**, not
the URL: the id is fixed in `server/server.json` on first start, it is the
`aud` of every token and the host in every author id
(`acct:<xuid>@<id>`), and the CLI matches local accounts by `server_id`
(`LocalAccount::is_on`). A friend who made an account through yesterday's
URL signs in through today's with the same account.

What does change: friends need the new URL each time (`--live-server`, the
web app's server switcher), and old links stop working. For a server that
lasts, use a named tunnel.

## Named tunnel (`--live-tunnel-name NAME --live-tunnel-url https://HOST`)

You need a Cloudflare account with a domain on it. Then one command does
the rest:

```
h2 play disc.iso --live-tunnel --live-tunnel-name h2play --live-tunnel-url https://play.forbes.red
h2 live serve --tunnel-name h2live --hostname live.example.net      # the same for serve
```

Before starting the tunnel it sets up whatever is missing, one line per
step:

```
live: no Cloudflare login on this machine; running `cloudflared tunnel login`: open the link it prints and pick the domain for the tunnel
live: logged in to Cloudflare (/Users/you/.cloudflared/cert.pem)
live: creating tunnel h2play
live: routing play.forbes.red to tunnel h2play
live: starting cloudflared (/opt/homebrew/bin/cloudflared) for http://127.0.0.1:52811
```

1. **Login.** With no origin certificate, it runs `cloudflared tunnel
   login` on your terminal and waits while you open the link and pick the
   domain. The certificate is found as cloudflared finds it:
   `$TUNNEL_ORIGIN_CERT` if set (then only that file, and a missing one is
   an error rather than a login, since login writes elsewhere), else
   `cert.pem` in `~/.cloudflared`, `~/.cloudflare-warp`,
   `~/cloudflare-warp`, `/etc/cloudflared` or `/usr/local/etc/cloudflared`.
2. **Tunnel.** If `cloudflared tunnel list --output json` has no tunnel of
   that name (or id), `cloudflared tunnel create NAME`. Its credentials go
   to `~/.cloudflared/<id>.json`; a tunnel made on another machine needs
   that file copied here (`run` says "credentials file not found"
   otherwise).
3. **DNS.** `cloudflared tunnel route dns NAME HOST`, every time: it adds a
   CNAME, or says the host already routes to this tunnel, which is fine.
   If HOST already has another record (an A record, or a CNAME to another
   tunnel), cloudflared will not replace it and neither will `h2`: the
   error says so and asks you to delete the old record in the Cloudflare
   dashboard (your domain → DNS → Records), then run again.

`--no-tunnel-setup` skips all three and only runs the tunnel, for a tunnel
you manage yourself. A name without a URL also skips them and just runs
the tunnel; the server cannot know its URL then, so it is not printed or
published.

**Use a different tunnel and host name for each server.** Pointing
`h2 play` at the tunnel name or host another server already uses (say the
one `live.forbes.red` is served through) would put two servers with
different ids behind one address. Pick another
pair, such as `h2play` and `play.forbes.red`.

`serve` runs `cloudflared tunnel --no-autoupdate --grace-period 1s run --url
http://127.0.0.1:<port> h2live`, waits for a registered connection, and
uses `https://live.example.net` as the public URL. Without `--hostname`
the tunnel still runs, but the server cannot know its URL, so it is not
printed or published.

On a **first start** (no `server/server.key` yet) with no `--id`, the
server id becomes the host name (`live.example.net`) instead of
`localhost`: ids cannot change later, and a lasting address is the natural
one (it matters for federation, [live-security.md](live-security.md) §5).
An existing server keeps its id.

If your `~/.cloudflared/config.yml` has `ingress` rules, cloudflared will
not take `--url`; point the rule for the host name at
`http://127.0.0.1:8074` instead.

## Your own proxy (`--public-url URL`)

Behind a reverse proxy you run yourself, `--public-url https://...` only
sets the URL shown in discovery and the startup lines. It does not trust
any proxy header.

`--public-url https://HOST --behind-cloudflared` says the proxy is a
cloudflared you run yourself (a system service)
that reaches the server on loopback. The server then treats it like its
own named tunnel: `CF-Connecting-IP` names the client on requests from
loopback, the dashboard shows a named tunnel, and on a first start with no
`--id` the server id becomes `HOST` (not for a `*.trycloudflare.com` URL,
an `http://` URL or one with a port). It does not go with `--tunnel`.

Only
use it when cloudflared is the one thing that can reach the port: anything
else on this machine could send its own `CF-Connecting-IP`.

## Free hosting (SQLite and Cloudflare R2)

A server is one process: the event log is SQLite (`live.db` beside the
key), and what is bulky and written once can live in Cloudflare R2, whose
free tier covers a friends' server. So a small VM, a Raspberry Pi or the
Mac you play on runs it with no database server, and nothing it stores is
lost with the machine.

| what | where | why |
|---|---|---|
| the event log | `<data>/live.db` (SQLite, WAL mode), backed up to R2 by Litestream | small, written constantly, needs ordered appends: a local file is fastest and free |
| the server key and id | `<data>/server.key`, `server.json` | a secret; never uploaded |
| custom content, cloud saves and their indexes | the private blob store: `<data>` (default) or a private R2 bucket (`--blobs s3://BUCKET`) | bulky, written once: object storage |
| shared game art | the art directory, published to a public R2 bucket (`--public-blobs`) and redirected to | big, public and cacheable: the bytes skip the server and the tunnel |

Content (maps, packs, preview images) is always served through the server,
from whichever store holds it: the game's client follows no redirects, and
an admin's takedown has to be immediate. Cloud saves always go through the
server with the owner's token. Only shared art is redirected.

### R2 setup

In the Cloudflare dashboard (any account; R2 needs a payment method on file
even on the free tier):

1. **R2 → Create bucket**: `h2live` (private: content, cloud saves,
   Litestream's backups). Location: automatic.
2. **Optionally a second bucket for the art**, `h2live-public`. It must be
   its own bucket: a public R2 bucket is public as a whole, and a cloud
   save must never be fetchable by hash. In its **Settings → Public
   access**, connect a custom domain on your Cloudflare zone (say
   `art.forbes.red`; cached at the edge) or allow the `r2.dev` URL (rate
   limited, for trying it out). Then **Settings → CORS policy**, so the
   web app can load the art across origins (it reads emblem pixels and
   loads models with `fetch`):

   ```json
   [{"AllowedOrigins": ["*"], "AllowedMethods": ["GET", "HEAD"], "AllowedHeaders": ["*"], "MaxAgeSeconds": 86400}]
   ```
3. **R2 → Manage API tokens → Create API token**: permission **Object Read
   & Write**, applied to those buckets only. Copy the **Access Key ID** and
   **Secret Access Key** (shown once) and the S3 endpoint,
   `https://<account id>.r2.cloudflarestorage.com` (on the R2 overview page).

The server reads the keys from the environment, never the command line:

```
H2_LIVE_S3_ENDPOINT=https://<account id>.r2.cloudflarestorage.com
H2_LIVE_S3_ACCESS_KEY_ID=...
H2_LIVE_S3_SECRET_ACCESS_KEY=...
```

For the public bucket, `S3Config::from_env` in
`crates/live/live-server/src/blobs.rs` tries the `H2_LIVE_S3_PUBLIC_*`
prefix first for each of `ENDPOINT`, `ACCESS_KEY_ID`,
`SECRET_ACCESS_KEY` and `REGION`. The region defaults to `auto`.

```
h2 live serve --blobs s3://h2live \
  --public-blobs s3://h2live-public --public-blobs-url https://art.forbes.red ...
```

or the same in the environment (`H2_LIVE_BLOBS`, `H2_LIVE_PUBLIC_BLOBS`,
`H2_LIVE_PUBLIC_BLOBS_URL`). Requests
are path-style, signed with SigV4 for region `auto`; `s3://BUCKET/PREFIX`
keeps a server's keys under a prefix. Any S3-compatible store works the
same way (MinIO, Backblaze B2).

### Check the bucket

Run this once with the real token before pointing a server at a bucket (or
after changing the token). It needs no server and no data directory:

```
export H2_LIVE_S3_ENDPOINT=https://<account id>.r2.cloudflarestorage.com
export H2_LIVE_S3_ACCESS_KEY_ID=...        # the token's Access Key ID
export H2_LIVE_S3_SECRET_ACCESS_KEY=...    # the token's Secret Access Key
h2 live store check --blobs s3://h2live
```

(`h2-live-server store check --blobs s3://h2live` does the same on a
machine without `h2`; `H2_LIVE_S3_REGION` stays `auto` for R2, and `--blobs` falls
back to `$H2_LIVE_BLOBS`.) For the public art bucket, with the same or a
scoped token:

```
h2 live store check --blobs s3://h2live-public --public --public-url https://art.forbes.red
```

`--public` reads the `H2_LIVE_S3_PUBLIC_*` variables first, and
`--public-url` also fetches the test object from that address with no
credentials, which is what a browser will do. `--quick` skips the 12 MiB
multipart upload (a few requests, well inside the free tier).

It writes under `.h2-store-check/<random>/` in the bucket and prints one
line per step, `[ ok ]` or `[FAIL]` with the reason, never a key or secret:

```
store check: s3://h2live (endpoint https://<id>.r2.cloudflarestorage.com, region auto)
  [ ok ]  write                  47 bytes
  [ ok ]  read                   47 bytes, identical
  [ ok ]  head and content type  size 47, content type and cache control kept
  [ ok ]  list                   1 object(s) under the scratch prefix
  [ ok ]  range read             the first 8 bytes
  [ ok ]  conditional write      create-only and if-match both enforced
  [ ok ]  multipart upload       12 MiB uploaded in parts and read back identical
  [ ok ]  delete                 scratch objects removed, prefix empty
store check: all 8 steps passed; s3://h2live is ready for --blobs
```

The exit status is 1 if any step failed, and the scratch objects are
deleted either way. What each step proves: *write*, *read*, *list*, *range
read* and *delete* are what content, cloud saves and art use; *conditional
write* is the `If-None-Match`/`If-Match` that keeps two servers from undoing
each other's save indexes (a store that ignores them fails this step);
*multipart upload* is how an upload over 10 MiB (a custom map) is stored;
*public URL* shows the bucket's public address serves the object.

What the token needs: **Object Read & Write** on the bucket (R2 → Manage API
tokens), nothing else. That covers every operation above (PutObject,
GetObject, HeadObject, DeleteObject, ListObjectsV2 and multipart). The
server never creates buckets or changes their settings, so *Admin* tokens
are not needed, and an *Object Read only* token fails *write* with a
"refused" hint.

A token scoped to the private bucket cannot check the
public one: give the public bucket its own token and the
`H2_LIVE_S3_PUBLIC_*` variables. Typical failures: *refused* is the token's
permission or bucket scope; *no such bucket* is a wrong bucket name or
account id in the endpoint; *signature* errors are a wrong key or secret.

**Published art.** With shared art (`--share-game-art`) and a public
bucket, the server uploads the art directory once in the background to
`art/<stamp>/...`, where the stamp is 16 hex digits of a hash over every
file's path and size and the manifest: a new extraction is a new set of
addresses. Each object carries its content type and `Cache-Control:
public, max-age=31536000, immutable`; a file with a compressed copy is
uploaded as that copy, its `.br` one with `Content-Encoding: br` (every glb
and JSON file of a current extraction, [live-art.md](live-art-pipeline.md#packing)),
else its `.gz` one with `Content-Encoding: gzip`. An interrupted upload resumes
on the next start (files already there are skipped), and a published
extraction costs one request to recognise.

Once it is all there, a request
for `/art/game/<file>` is a `307` to `<public url>/art/<stamp>/<file>`,
carrying the cache policy the file itself had: a year for a versioned
`?v=` address, `no-cache` (asked each time) otherwise. Until then, for a
client that does not take the published copy's encoding (Brotli, or gzip
for older extractions), and on a `--private` server, the server serves the
files itself, the same bytes: the `.br` copy to a browser that takes
Brotli, the `.gz` one to one that takes gzip, else the file. Art that is not shared is never published.

Old
extractions' prefixes stay in the bucket; delete them in the dashboard if
space matters.

**Cloud save indexes** live beside the saves (`storage/<xuid>/index.json`):
they cannot be rebuilt from the blobs, so they move with them. In a bucket
each index write is conditional on the copy the server read (`If-Match`,
which R2 honours), so two servers on one bucket cannot silently undo each
other's writes.

### Costs

R2's free tier, per month: 10 GB stored, 1 million Class A operations
(writes, lists), 10 million Class B operations (reads), and no charge for
egress, ever. A friends' server stays well inside it: art is a few hundred
MB and published once, a cloud save write is two Class A operations (the
bytes and the index), a read one Class B, and Litestream writes at most one
small object a second while the log is busy (its default sync interval),
plus a daily snapshot. Past the free tier R2 bills $0.015/GB-month and
$4.50 per million Class A operations.

Published art behind a custom domain
is served from Cloudflare's cache, so repeat views are not R2 reads at all.

### Backups to R2 (Litestream)

[Litestream](https://litestream.io) copies `live.db`'s WAL to R2 as it is
written, so a lost machine loses at most the last second. It is a separate
process, not part of the server: it is battle-tested at exactly this, and
it reads the database file without the server's help. (A built-in periodic
`VACUUM INTO` snapshot uploaded to R2 would be simpler to ship but would
lose up to a whole interval of accounts, friends and games; Litestream is
one more process for a much better recovery point.) Its settings are
`deploy/live/litestream.yml`, which takes everything from the environment:
the same `H2_LIVE_S3_*` keys, `LITESTREAM_BUCKET` (the private bucket) and
`LITESTREAM_DB` (the database file). Backups go under `litestream/` in the
bucket, a daily snapshot is kept for a week.

- **Bare metal or a Mac**: `brew install litestream` (or the release
  binary), then run, beside the server,

  ```
  LITESTREAM_DB="$HOME/Library/Application Support/h2recomp/live/server/live.db" \
  LITESTREAM_BUCKET=h2live H2_LIVE_S3_ENDPOINT=... H2_LIVE_S3_ACCESS_KEY_ID=... \
  H2_LIVE_S3_SECRET_ACCESS_KEY=... litestream replicate -config deploy/live/litestream.yml
  ```

  As a launchd agent: a second plist like the server's, with
  `ProgramArguments` `/opt/homebrew/bin/litestream replicate -config
  <repo>/deploy/live/litestream.yml`, those variables in
  `EnvironmentVariables`, `KeepAlive` and `RunAtLoad` true. (Keep the
  plist's mode 0600: it holds the secret key.)
- **Restore** (the server stopped, `live.db` absent):
  `litestream restore -config deploy/live/litestream.yml -if-replica-exists "$LITESTREAM_DB"`,
  then start the server; with the server key and id from your own backup,
  it is the same server.

### Moving an existing server

`h2 live store copy` (or `h2-live-server store copy` on a machine without
`h2`) moves the log between databases and the blobs between the data
directory and a bucket. The log keeps every event's position, version and
hash (clients' cursors and checkpoints stay valid) and is checked on the
way in; the copy is idempotent and resumable: run it again and it carries
on after what the destination has, and it refuses a destination that holds
a different log. Blobs already at the destination with the same size are
skipped.

The blob copy includes custom content, cloud saves, frame chunks,
sim reports and `films/`. It preserves each private host recording, including
multipart manifests, their parts and pending stats-import pages. The copy
also transfers private `film-staging/` objects, including validation cursors
and reservation details.

It refreshes staged objects even when their size
matches. Quiesce writes during the final copy so journals, parts and mutable
cursors represent the same cutover. The destination sweeper resumes accepted
films after the signed log and private objects have both been restored.

An upload still awaiting its next part can resume when its host reconnects.

For live.forbes.red (a launchd agent running `h2-live-server --data
"~/Library/Application Support/h2recomp/live/server" --db
postgres://localhost/h2live`):

```
D="$HOME/Library/Application Support/h2recomp/live/server"
launchctl bootout gui/$(id -u)/red.forbes.h2live.server     # stop it
h2-live-server store copy --from postgres://localhost/h2live --to "sqlite://$D/live.db"
# with R2 (the H2_LIVE_S3_* variables set in this shell):
h2-live-server store copy --from-blobs "$D" --to-blobs s3://h2live
```

then change the plist's `ProgramArguments`: drop `--db
postgres://localhost/h2live` (the default is now `live.db` in `--data`;
or say `--db sqlite://$D/live.db`), and for R2 add `--blobs s3://h2live`
(and `--public-blobs s3://h2live-public --public-blobs-url
https://art.forbes.red` for the art) with the three `H2_LIVE_S3_*`
variables in `EnvironmentVariables`. Start it again (`launchctl bootstrap
gui/$(id -u) ~/Library/LaunchAgents/red.forbes.h2live.server.plist`); its
first line says `log at position N (sqlite ...)` with the same N Postgres
had. Keep the Postgres database until you are happy; running the copy
again after a mistaken start on Postgres picks up what was added.

## Speed through Cloudflare

The server marks what may be cached (live-api.md, "Caching"). By default Cloudflare caches only static file extensions, so the JSON reads come back `DYNAMIC` and every one costs a round trip through the tunnel (250 to 700 ms measured on live.forbes.red, against about 1 ms at the origin). To let the edge answer them, add a Cache Rule on the zone:

- **When:** hostname is your Live host, the URI path starts with `/v1/`, and the method is `GET`.
- **Then:** eligible for cache, edge TTL "use cache-control header if present", browser TTL "respect origin".

The rule is safe for signed-in readers. Cloudflare bypasses its cache for any request that carries an `Authorization` header, so a signed-in read is never answered from the anonymous entry, and its `private, no-store` answer is never stored. Cloudflare ignores `Vary: Authorization` unless a Cache Rules Vary setting lists it; the header is for other shared caches.

Assets are `immutable` and named by hash, so they are cached anyway; purge the zone once after the first deploy that carries the new headers.

Tunnel: `cloudflared` picks QUIC on UDP 7844 and opens 4 connections to the nearest Cloudflare locations it can reach. If its log shows QUIC dials failing ("no route to host"), or the connections land far from your readers, try `--protocol http2` (TCP 443). Argo Smart Routing shortens the path from the reader's edge to the tunnel.

The edge cache does more than either, because a cached read never reaches the tunnel.

## cloudflared not found, or failing

cloudflared is looked up as `$H2_CLOUDFLARED`, else `cloudflared` on
`PATH`. Missing: the error says `brew install cloudflared` (or Cloudflare's
downloads page). If it exits before the tunnel is up, or does not bring it
up within 45 s, the error includes its last lines of output (no
credentials for a named tunnel, an unknown tunnel name, no network).

## Security with a public server

The server is on the internet now. What protects it:

- **Signed requests, not network position.** Every write is a JWS by its
  author's key and a single-use token whose `aud` is the server id
  ([live-security.md](live-security.md) §2, §3). The tunnel changes
  nothing about that.
- **Operator and admin endpoints need keys.** Invites, admin grant and
  revoke, title files and hoppers need a token signed by the server key in
  `--data` (or, for title files and invites, an admin account's key,
  [live-admin.md](live-admin.md) §5). None of them trusts loopback.
- **Rate limits are on by default**: writes per author and method (10/s,
  burst 30), account creation server-wide (1/s, burst 10), local sign-in
  redeems. Bodies are capped (64 KiB, title files 2 MiB, socket messages
  16 KiB). They are keyed by author, not address, so all traffic arriving
  from cloudflared on loopback is not lumped together.
- **This-machine-only routes stay this-machine-only.** Through a tunnel
  every request comes from `127.0.0.1`, so the peer address cannot tell a
  stranger from you. Game art (`/art/game/*`, unless `--share-game-art`)
  and the one-use sign-in link (`POST /v1/local/signin`) also refuse any
  request carrying a proxy header (`Forwarded`, any `X-Forwarded-*`,
  `X-Real-IP`, `True-Client-IP`, any `CF-*`): cloudflared adds
  `CF-Connecting-IP`, `CF-Ray`, `CF-Visitor` and `X-Forwarded-For` to
  every request it forwards, and a visitor cannot remove them. Local
  sign-in further needs a loopback `Host` (the tunnel sends its own host
  name). The `h2 play --live-tunnel` sign-in link stays a loopback URL.
- **The client's address**, for code that needs it (the session endpoint's
  observed-source fallback, [live-matchmaking.md](live-matchmaking.md)
  §3.6): `PublicUrl::client_ip` reads `CF-Connecting-IP` only while a
  Cloudflare tunnel is configured (`--tunnel`, `--tunnel-name`, or
  `--behind-cloudflared`) and the request came from loopback;
  otherwise the header is ignored, since anyone can send it.

What you should decide:

- **Who may create accounts.** By default anyone with the URL can.
  `--invite-only` requires an invite code. The operator, admins and
  accounts with the **can invite** grant can make codes
  ([live-admin.md](live-admin.md) §5). `serve` warns when a tunnel runs
  without this setting.
- **Who may read.** Reads (games, accounts, presence, the log) are public
  by default; `--private` needs a token for every read and on the stream.
- **Admin names.** New accounts for configured gamertags require the
  operator credential at registration. Use `h2 live account create` with
  `--operator-data DIR`. For an existing account, verify its identity
  before using `h2 live admin grant`.

## What the tunnel does not carry

Only the service (HTTP and WebSockets). Game traffic between consoles is
UDP straight between players and never goes through the server
([live-matchmaking.md](live-matchmaking.md) §3.6): the host still needs
UDP 3074-3080 reachable for internet play.

## Tests

- `crates/apps/cli/src/tunnel.rs`: URL parsing over real-looking cloudflared
  logs (the boxed banner, JSON log format, older releases, and look-alikes
  such as `api.trycloudflare.com` that must not match); flags; arguments;
  lookup on `PATH`; lifecycle with a fake `cloudflared` shell script
  (URL reported, process stopped on drop, failure output shown, timeout
  kills it, a dead one restarted with its new URL).
- `crates/apps/cli/src/tunnel.rs`, named tunnel setup with a fake
  `cloudflared` that logs its calls: no certificate → `tunnel login` runs
  (and a login that writes nothing, or a missing `$TUNNEL_ORIGIN_CERT`, is
  an error); an existing tunnel is not created again, a missing one is;
  `route dns` "already configured" is success and an existing record is a
  conflict naming the dashboard fix; `--no-tunnel-setup` (and a name
  without a host) goes straight to `run`; `tunnel list` JSON (deleted
  tunnels, `null`); `--live-tunnel-url` parsing; the origin certificate
  search.
- `crates/apps/cli/src/play_live.rs`: `h2 play --live-tunnel` with a fake
  cloudflared: discovery shows the URL, the game and sign-in link stay on
  loopback, a tunnelled redeem is refused, cloudflared stops with the
  session. `--live-tunnel` flags and their aliases (`--live-tunnel`
  implies `--live-local`); a named `--live-tunnel` session logs in, creates,
  routes and then runs, and only runs with `--no-tunnel-setup`.
- `crates/live/live-server/src/standalone.rs`: `h2-live-server`'s flags,
  including the `--behind-cloudflared` command line (loopback,
  the id from `--public-url` on a first start only, `CF-Connecting-IP`
  trusted, the art shared, the web app from `H2_LIVE_WEB`); `public.rs`,
  `identity.rs`, `art.rs`: the lasting host, the first-start id and
  `--game-art` checks both binaries share; `crates/apps/cli/src/live_cmd.rs`:
  `serve --behind-cloudflared` names a new server only.
  `cargo metadata` path closures, atomic installs, the update menu lines,
  `[update]` config, and `--update-repo`/`--update-disc`/`--update-now`.
- `crates/live/live-server/src/sqlite.rs`, `pg.rs`, `store.rs`: the
  store conformance suites (events, title blobs, usage counters) over SQLite (a temporary file), memory,
  the file store and Postgres (`H2_LIVE_TEST_DB`), the concurrent-append
  race on SQLite and Postgres, two handles on one SQLite file, WAL mode.
- `crates/live/live-server/src/blobs.rs`: the blob-store conformance suite
  (put, streamed file uploads, ranged reads, list by prefix, conditional
  writes) over a directory, memory and a fake S3 endpoint that checks the
  requests are path-style and signed for region `auto`; `check.rs`: the
  `store check` steps over memory and a directory, scrubbing of secrets,
  argument errors; `copy.rs`: logs
  with position gaps, resume after growth, a different log refused, a
  broken chain refused, the file store into SQLite, blobs without the key
  or temporary files; `art.rs`: publishing (not when unshared, gzip
  copies, resumable) and the redirects with their cache policy;
  `tests/cloud_saves.rs`: saves in a bucket, and an index changed by
  another writer is not overwritten.
- `crates/live/live-server/tests/s3_server.rs`: the blob store against a real S3
  protocol server (the `s3s` library over a directory, in process, with
  SigV4 checked): put, get, head, list, delete, a range read from the
  middle and across a 10 MiB part boundary, a 25 MiB multipart upload,
  create-only and `If-Match` writes refused when stale, content type,
  encoding and cache control kept, wrong credentials and a missing bucket
  as errors; the server's own paths over it (a small and a 12 MiB content
  upload and download, a cloud save with its index, published art with its
  headers, `store copy` into and out of a bucket, twice); and `store
  check` passing, failing on a non-public URL and on a wrong secret.
  Set `H2_S3_TEST_ENDPOINT`, `H2_S3_TEST_ACCESS_KEY`,
  `H2_S3_TEST_SECRET_KEY` and `H2_S3_TEST_BUCKET` (an existing bucket;
  `H2_S3_TEST_REGION` if not `auto`) to run the same tests against MinIO or
  a scratch bucket instead; each test keeps to a random prefix and deletes
  what it wrote.
- `crates/live/live-server/tests/tunnel.rs`, `art.rs`, `local_signin.rs`,
  `public.rs`: proxy and CF headers refused for game art and local
  sign-in through the router; discovery's `url`; `CF-Connecting-IP` only
  trusted behind a tunnel; unclaimed admin names.
