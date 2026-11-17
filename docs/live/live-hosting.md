# Hosting a Live server for friends (Cloudflare Tunnel)

`h2 live serve` binds loopback and refuses anything else without TLS
([live-security.md](live-security.md) §6). The easy way to let friends in
over the internet, with HTTPS and no port forwarding, is Cloudflare
Tunnel: `cloudflared` runs on your machine, connects out to Cloudflare, and
forwards each request (WebSockets included) to the server on loopback.
`--live-tunnel` (`h2 play`) and `--tunnel` (`h2 live serve`) run it for you.

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

Implemented in `crates/cli/src/tunnel.rs` (flags in `live_cmd.rs`,
`play_live.rs`); the server side is `crates/live-server/src/public.rs`.

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

**Use a different tunnel and host name from the Docker deploy.** The
Docker setup below serves `live.forbes.red` through its own tunnel
(`h2live`, token-run). Pointing `h2 play` at the same tunnel name or host
would put two servers with different ids behind one address. Pick another
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
cloudflared you run yourself (a system service, or the Docker setup below)
that reaches the server on loopback. The server then treats it like its
own named tunnel: `CF-Connecting-IP` names the client on requests from
loopback, the dashboard shows a named tunnel, and on a first start with no
`--id` the server id becomes `HOST` (not for a `*.trycloudflare.com` URL,
an `http://` URL or one with a port). It does not go with `--tunnel`. Only
use it when cloudflared is the one thing that can reach the port: anything
else on this machine could send its own `CF-Connecting-IP`.

## Docker with a named tunnel

`deploy/live/` runs the server and cloudflared with Docker Compose (the
log is SQLite in `data/`; blobs there or in R2, [below](#free-hosting-sqlite-and-cloudflare-r2)): the API, the web dashboard and the stats site at one lasting
address (here `https://live.forbes.red`), with the game art shared. The
image has no disc and no art in it; you extract the art once on a machine
with the disc and the containers read it from there.

- `deploy/live/Dockerfile`: builds the web app with bun and
  `h2-live-server` (`crates/live-server/src/bin`, `standalone.rs`: the
  server and its `serve` flags without the rest of `h2`, so it builds on
  Linux in a few minutes), into a distroless image that runs as a non-root
  user. The web app is at `/srv/web` (`H2_LIVE_WEB`); `/data` is the
  server's key and id, `/art` the art.
- `deploy/live/compose.yaml`: two services, and a third for backups.
  - `cloudflared` runs `tunnel run` with the token in `TUNNEL_TOKEN`.
  - `server` runs in cloudflared's network namespace
    (`network_mode: service:cloudflared`) and listens on `127.0.0.1:8074`
    with `--share-game-art --public-url https://live.forbes.red
    --behind-cloudflared`; admins come from `H2_LIVE_ADMINS`, and
    `H2_LIVE_INVITE_ONLY=1` (setup.sh asks; off by default) makes sign-up
    need an invite. Its root filesystem is read-only and it has no
    capabilities.
  - `litestream` (only with `COMPOSE_PROFILES=backup`) copies `live.db`
    to R2 as it is written ([Backups](#backups-to-r2-litestream)).
  All of them restart unless stopped. `compose.postgres.yaml` adds a
  Postgres 17 container and points the server at it, for a server that
  stays on Postgres (`docker compose -f compose.yaml -f
  compose.postgres.yaml up -d`, `POSTGRES_PASSWORD` in `.env`).
- `deploy/live/.env.example`: the settings; copy it to `.env`, which git
  ignores along with `data/` and `art/`.

Why the server shares cloudflared's namespace instead of listening on
`0.0.0.0`: on the compose network every container, and Docker's own
gateway, could reach it, and a request from one of them is not from
loopback, so the server could not tell "this machine" from a neighbour.
`0.0.0.0` also needs TLS or `--insecure-http` (security §6). In a shared
namespace the server keeps loopback and plain HTTP, nothing is published on
the host, cloudflared is the one thing that can reach port 8074, and every
request it delivers carries Cloudflare's headers, so local sign-in and
other this-machine-only routes refuse it exactly as they do behind
`h2 live serve --tunnel`. The server joins cloudflared's namespace rather
than the other way round because the server is the one more likely to
restart (a new image); a restarted server rejoins the
namespace, while a container joined to a restarted one is left with a dead
namespace. If cloudflared itself is recreated, Compose restarts the server
with it (`depends_on: restart: true`).

### One-time Cloudflare setup

The domain (`forbes.red`) must be on your Cloudflare account.

1. In the Cloudflare dashboard, Zero Trust → Networks → Tunnels → Create a
   tunnel. Choose **Cloudflared**, name it (`h2live`), save.
2. On the install page, copy the token: the long `eyJ...` string after
   `--token` in any of the install commands. Do not run the install
   command; Docker runs cloudflared. Put the token in `.env` as
   `TUNNEL_TOKEN`. It is a secret: anyone with it can serve your tunnel.
3. Add a public hostname: subdomain `live`, domain `forbes.red`, service
   type **HTTP**, URL `127.0.0.1:8074` (not `localhost`, which may resolve
   to `::1`; the server binds IPv4 loopback). Cloudflare makes the DNS
   record. WebSockets need nothing: they are on by default.

### Game art

On the Mac with the disc and a built `h2`, from the repository root:

```
h2 live assets extract --disc /path/to/halo2.iso --out deploy/live/art
```

(`--disc` defaults to `$H2_DISC`, else the ISO in `disc/`.) It writes PNGs,
the map models and views and `manifest.json` ([live.md](live.md#game-art),
[live-art.md](live-art.md)). Compose mounts the directory read-only at
`/art`; the server will not start if `--game-art` has no `manifest.json`.
With `--share-game-art` anyone can download these files: you are
redistributing your copy of the game's art, so decide that knowingly. Run
the command again after updating `h2` if the extractor changed; the server
reads the files as they are.

### Start it

```
cd deploy/live
cp .env.example .env          # TUNNEL_TOKEN, H2_LIVE_ADMINS; R2 optional
mkdir -p data
docker compose up -d --build
docker compose logs -f server
```

Make `data/` before the first start: if Docker creates it, it may be owned
by root and the server, which runs as uid 65532, cannot write to it. On
Linux set `H2_LIVE_UID`/`H2_LIVE_GID` in `.env` to your own ids so the
files stay yours (Docker Desktop on macOS maps ownership for you).

`H2_LIVE_DATA_DIR` and `H2_LIVE_ART_DIR` in `.env` (default `./data` and
`./art`; `setup.sh` writes both) say where on the host the state and the
extracted art live; compose mounts them at `/data` and `/art`.

The first start creates `data/server.key` and `data/server.json` with the
id `live.forbes.red`, from `--public-url` (`--behind-cloudflared`, above).
Among the first lines of the log:

```
h2 live: public at https://live.forbes.red (through cloudflared)
h2 live: live.forbes.red on http://127.0.0.1:8074, log at position 0 (sqlite sqlite:///data/live.db, public reads)
h2 live: blobs in /data
h2 live: web app from /srv/web
h2 live: serving game art from /art to everyone: ...
```

The id never changes after this. If the first start had the wrong
`H2_LIVE_PUBLIC_URL`, stop it, delete `data/` (and, on Postgres, the `pg`
volume: `docker compose down -v`), and start again before anyone signs up.

### The first admin account

Sign up in the web app or with `h2 live account create`. If you made the
server invite-only, the first account needs an invite. The operator
commands sign with the key in `data/` and work against the public URL from
the machine that has it:

```
h2 live invite create --server https://live.forbes.red --data deploy/live/data
h2 live account create afr --server https://live.forbes.red --invite <code>
```

or sign up in the web app at `https://live.forbes.red` with the invite.
If `afr` is in `H2_LIVE_ADMINS`, that account is an admin as soon as it
exists; until it does, whoever registers the name first would be, which is
why the server warns about unclaimed admin names (with `--invite-only`
only someone holding an invite can). Otherwise grant it:

```
h2 live admin grant afr --server https://live.forbes.red --data deploy/live/data
```

Admins then make further invites in the dashboard (Settings → Server →
Invites, [live-admin.md](live-admin.md) §5). To change invite-only, set
`H2_LIVE_INVITE_ONLY` in `.env`; to add `--private`, edit the server's
`command` in `compose.yaml`.

### Backups

What holds the server's state:

- `data/server.key`, `data/server.json`: the server key and id. Lose them
  and every token and author id the server has issued is orphaned. Copy
  them somewhere private (the key is a secret). They never go to R2.
- `data/live.db`: the event log (accounts, friends, games, title files).
  With `COMPOSE_PROFILES=backup` Litestream keeps a copy in R2
  ([below](#backups-to-r2-litestream)); otherwise snapshot it while the
  server runs with `sqlite3 data/live.db "vacuum into 'h2live.db'"` (a
  plain file copy of a database in WAL mode can miss recent writes). On
  Postgres: `docker compose exec -T db pg_dump -U h2live h2live > h2live.sql`.
- Blobs (custom content, cloud saves): `data/content/` and `data/storage/`,
  or the private R2 bucket, which is its own copy.

The art can always be extracted again.

### Updating

```
git pull
cd deploy/live
docker compose up -d --build
docker image prune -f
```

The server rebuilds and restarts; the data, the database and the id are
kept. `docker compose pull` updates cloudflared and Litestream (and
Postgres if you use it: stay on major version 17 unless you dump and
restore). If a server restart ever leaves
the site down while cloudflared is up, `docker compose restart cloudflared
server` puts both back in one namespace.

### What Docker does not change

Only the service goes through the tunnel. Game traffic is UDP straight
between players' machines ([below](#what-the-tunnel-does-not-carry)), not
through this server, so hosting a game still needs UDP 3074-3080 reachable
on the host player's side.

## Menu bar app

`h2 live menubar` is an optional menu bar (tray) app for a server that
launchd runs on this Mac: it shows whether the server and tunnel are up,
starts and stops them, makes invites, and decides what runs at login. It
has no Dock icon and no window. It never runs the server itself; closing it
leaves the server running.

It manages three LaunchAgents, named in
`~/Library/Application Support/h2recomp/live/menubar.toml`:

| Agent | Default label | What runs |
| --- | --- | --- |
| server | `org.h2recomp.live.server` | `h2 live serve ...` or `h2-live-server ...` |
| tunnel | `org.h2recomp.live.tunnel` | `cloudflared tunnel ... run` |
| menu bar | `org.h2recomp.live.menubar` | `h2 live menubar` at login |

The file also holds `local_url` (where the menu bar reaches the server,
default `http://127.0.0.1:8074`), `public_url` (the tunnel's URL, if any),
`data_dir` (the server's key, which signs the status and invite requests)
and `logs_dir`. A missing file is the defaults.

### Install

Already running the server and cloudflared as LaunchAgents of your own?
Adopt them by label; their plists are read, never rewritten:

```
h2 live menubar --adopt-existing red.forbes.h2live.server red.forbes.h2live.tunnel
h2 live menubar --status      # check what the menu will say
h2 live menubar --install     # open the menu bar app at every login
h2 live menubar               # and open it now
```

Adopting reads the server agent's `--public-url`, `--data` and `--listen`
into `menubar.toml`, and its `StandardErrorPath` for the logs folder. Pass
`none` as the tunnel label for a server with no tunnel. `--public-url URL`,
`--data DIR` and `--local-url URL` set those keys by hand.

Starting from nothing, the menu bar can write the agents too:

```
h2 live menubar --install-server -- --public-url https://live.example.net --behind-cloudflared --invite-only
h2 live menubar --install-tunnel --token-file ~/.cloudflared/live.token
h2 live menubar --install
```

`--install-server` writes and loads an agent running `h2 live serve` with
the flags after `--` (KeepAlive, log in `live/logs/server.log`);
`--install-tunnel` one running `cloudflared tunnel --no-autoupdate --url
<local_url> run --token-file FILE` (cloudflared from `PATH` or
`--cloudflared PATH`, log in `live/logs/tunnel.log`). `--uninstall`
removes the menu bar's own login item; the server and tunnel agents stay.

### The menu

The icon is the app icon's "2" as a template glyph
([art/app-icon](../../art/app-icon/README.md)): plain when the server
answers, with a small ring beside it while it starts or an action runs,
dimmed when it is stopped, and slashed through on an error (the agent
failed, or the server answers but will not give its status).

- **Status lines**: the server's state and local URL; uptime, players
  online, live games and accounts (from `GET /v1/admin/status`, what the
  dashboard's Server screen reads, signed with the server key); the tunnel
  agent's state, the public URL and whether it answers `/healthz`; and the
  last action's result or error. These refresh every 4 s off the UI thread.
- **Start / Stop / Restart Server**, **Start / Stop Tunnel**: `launchctl
  bootstrap`, `bootout` and `kickstart -k` on the agent. Stop unloads it,
  so KeepAlive does not bring it back. A server something else started (in
  a terminal) shows as "not started by this app" and is not started or
  stopped from here.
- **Open Dashboard**, **Open Stats Site**: the public URL (else the local
  one), and its `/stats`.
- **Copy New Invite**: what `h2 live invite create` does (a 7-day invite,
  signed with the server key in `data_dir`), copied to the clipboard and
  shown in the menu.
- **Open Logs Folder**, **View Server Log**: the folder holding the server
  agent's log, and the log in Console.
- **Run Server at Login**: launchd's enable/disable override for the server
  agent (`launchctl enable|disable gui/<uid>/<label>`). A KeepAlive agent
  starts as soon as it loads whatever `RunAtLoad` says, so the toggle stops
  it loading at login instead, and leaves the plist alone. Start from the
  menu still works while it is off; the server then runs until you stop it
  or log out.
- **Open Menu Bar App at Login**: writes or removes the menu bar's own
  agent, as `--install` and `--uninstall` do.
- **Quit Menu Bar**: quits; the server and tunnel keep running.
  **Stop Everything and Quit**: stops the tunnel and the server, then quits.
  Agents left to run at login start again at the next login.

### Updates

Opt-in: the menu bar can follow a branch of a git clone, rebuild what
changed and install it, so the server tracks master without you running
builds. Use a clone of its own, never the checkout you work in: updates
stop the moment a checkout has uncommitted changes, sits on another
branch, or has commits the remote lacks, and they never reset or stash
your changes (the one exception is below: restoring after the updater's
own failed merge). The web app's dashboard art is in Git LFS, so install
`git-lfs` (`brew install git-lfs && git lfs install`) before cloning.

```
git clone git@github.com:0xdeafcafe/h2r.git ~/Library/Application\ Support/h2recomp/live/src
h2 live menubar --update-repo ~/Library/Application\ Support/h2recomp/live/src
h2 live menubar --update-disc /path/to/halo2.iso    # optional: re-extract art when the extractor changes
h2 live menubar --update-now                        # first build and install, in this terminal
~/Library/Application\ Support/h2recomp/live/bin/h2 live menubar --install
```

The last line makes the login item run the installed `h2`, so the menu
bar restarts itself on the new one when an update replaces it. The
server agent should run `bin/h2-live-server` from the install dir, with
`H2_LIVE_WEB` pointing at its `web/` (as the red.forbes.h2live.server
agent does).

`[update]` in `menubar.toml`:

| Key | Default | |
| --- | --- | --- |
| `repo` | none (off) | the clone to follow |
| `branch`, `remote` | `master`, `origin` | what to follow |
| `interval_minutes` | 10 | how often to check |
| `install_dir` | `live/` | holds `bin/` (h2-live-server, h2), `web/` and the build's `target/` |
| `disc`, `mappack` | none | re-extract game art from these when needed |
| `cargo`, `bun`, `git` | found on PATH, `~/.cargo/bin`, `~/.bun/bin`, Homebrew | the tools, if elsewhere |
| `path` | none | extra directories put first on every child's PATH |

A LaunchAgent starts with only `/usr/bin:/bin:/usr/sbin:/sbin`, so every
child the menu bar runs (git and its `git-lfs` filter, cargo, bun, rsync,
nice) gets the inherited PATH plus `/opt/homebrew/bin`, `/usr/local/bin`,
`~/.cargo/bin` and `~/.bun/bin`, once each, after anything in `path`. The
agents `--install`, `--install-server` and `--install-tunnel` write set the
same `PATH` in their plists.

Each check (on the timer, or Update Now):

1. If the repository uses Git LFS (`filter=lfs` in `.gitattributes`),
   `git lfs version` must work; if not the check is blocked with
   "git-lfs not found" and nothing is touched. Then `git fetch`; when
   `<remote>/<branch>` moved and fast-forwards, `git merge --ff-only`. A
   dirty, diverged or off-branch checkout is reported in the menu and left
   alone.
2. The files changed since the installed revision (kept in
   `update-state.json`) pick the builds: live-server and its path
   dependencies (from `cargo metadata`) build `h2-live-server`; the CLI's
   crates build `h2`; `web/` runs `bun install --frozen-lockfile` and
   `bun run build`. The workspace's `Cargo.toml`, `Cargo.lock`,
   `rust-toolchain.toml` and `.cargo/` rebuild both binaries. Nothing
   installed yet builds all three.
3. Builds run under `nice -n 10`, one at a time, into `install_dir/target`,
   with their output in `logs/update.log`. Each binary is copied in as
   `.new` and renamed over the old one; the web build is synced into
   `web/` (`rsync -a --delete`).
4. When `EXTRACTOR_VERSION` in `crates/cli/src/live_art.rs` changed, the
   new `h2 live assets check` decides whether the art is stale; with a
   `disc` it re-extracts, without one the menu says the art needs
   re-extracting.
5. The server agent, if loaded, is restarted (`kickstart -k`) when its
   binary, the web app or the art changed. A stopped server stays
   stopped. When `h2` changed and the menu bar is that `h2`, it re-executes
   itself.

When the updater resets the clone. Only in two cases, both about its own
merge, never for changes you made. (a) If `git merge --ff-only` fails
(git's stderr is shown as the failure and logged), it runs
`git reset --hard <the old HEAD>` and `git clean -fd` in the update clone
so the half-applied merge does not linger, and the next check retries.
(b) If that restore itself failed, `update-state.json` keeps
`merge_failed` = that HEAD; a later check that finds the clone dirty at
exactly that HEAD does the same restore before going on. A dirty clone
with no such record, or at any other HEAD, is the refusal above:
"checkout has uncommitted changes", no reset, no stash. This is why the
clone must be dedicated: `clean -fd` removes untracked files in it.

A revision whose build failed is not retried on the timer, only by Update
Now (fix it upstream and the next revision builds as usual).

The menu shows "Updates: up to date (abc1234, 5 min ago)", what is
building, or why updates are blocked; the last error; **Update Now**; and
**Pause Updates** (kept in `update-state.json`; Update Now still works
while paused). `h2 live menubar --update-now` runs one check in a terminal,
and `--update-repo none` turns updates off.

The menu bar is `crates/live-menubar` (tray-icon and muda on a winit event
loop). macOS is complete; elsewhere the tray runs but launchd's items fail
with a message.

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

(`H2_LIVE_S3_PUBLIC_ENDPOINT`, `H2_LIVE_S3_PUBLIC_ACCESS_KEY_ID`,
`H2_LIVE_S3_PUBLIC_SECRET_ACCESS_KEY` and `H2_LIVE_S3_PUBLIC_REGION` override
the matching one for the public bucket, for a token scoped to it;
`H2_LIVE_S3_REGION` defaults to `auto`.) Then:

```
h2 live serve --blobs s3://h2live \
  --public-blobs s3://h2live-public --public-blobs-url https://art.forbes.red ...
```

or the same in `.env` for Docker (`H2_LIVE_BLOBS`, `H2_LIVE_PUBLIC_BLOBS`,
`H2_LIVE_PUBLIC_BLOBS_URL`; `deploy/live/setup.sh` asks for them). Requests
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

(`h2-live-server store check --blobs s3://h2live` does the same in the
Docker image; `H2_LIVE_S3_REGION` stays `auto` for R2, and `--blobs` falls
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
"refused" hint. A token scoped to the private bucket cannot check the
public one: give the public bucket its own token and the
`H2_LIVE_S3_PUBLIC_*` variables. Typical failures: *refused* is the token's
permission or bucket scope; *no such bucket* is a wrong bucket name or
account id in the endpoint; *signature* errors are a wrong key or secret.

**Published art.** With shared art (`--share-game-art`) and a public
bucket, the server uploads the art directory once in the background to
`art/<stamp>/...`, where the stamp is 16 hex digits of a hash over every
file's path and size and the manifest: a new extraction is a new set of
addresses. Each object carries its content type and `Cache-Control:
public, max-age=31536000, immutable`; a file with a `.gz` copy is uploaded
as that copy with `Content-Encoding: gzip`. An interrupted upload resumes
on the next start (files already there are skipped), and a published
extraction costs one request to recognise. Once it is all there, a request
for `/art/game/<file>` is a `307` to `<public url>/art/<stamp>/<file>`,
carrying the cache policy the file itself had: a year for a versioned
`?v=` address, `no-cache` (asked each time) otherwise. Until then, for a
client that does not take gzip, and on a `--private` server, the server
serves the files itself. Art that is not shared is never published. Old
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
$4.50 per million Class A operations. Published art behind a custom domain
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

- **Docker**: `COMPOSE_PROFILES=backup` and `LITESTREAM_BUCKET=h2live` in
  `.env` start the `litestream` service beside the server.
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
skipped. Only `content/` and `storage/` are copied, never the key.

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
  `--invite-only` requires an invite code (`h2 live invite create`, run on
  this machine, or an admin's dashboard: Settings → Server → Invites,
  [live-admin.md](live-admin.md) §5). `serve` warns when a tunnel runs without it.
- **Who may read.** Reads (games, accounts, presence, the log) are public
  by default; `--private` needs a token for every read and on the stream.
- **Admin names.** `--admin`/`H2_LIVE_ADMINS` name gamertags, and a
  configured gamertag nobody holds yet makes whoever registers it first an
  admin. With a public URL, `serve` and `play --tunnel` warn about such
  names: create those accounts first, use `--invite-only`, or grant an
  existing account with `h2 live admin grant`.

## What the tunnel does not carry

Only the service (HTTP and WebSockets). Game traffic between consoles is
UDP straight between players and never goes through the server
([live-matchmaking.md](live-matchmaking.md) §3.6): the host still needs
UDP 3074-3080 reachable for internet play.

## Tests

- `crates/cli/src/tunnel.rs`: URL parsing over real-looking cloudflared
  logs (the boxed banner, JSON log format, older releases, and look-alikes
  such as `api.trycloudflare.com` that must not match); flags; arguments;
  lookup on `PATH`; lifecycle with a fake `cloudflared` shell script
  (URL reported, process stopped on drop, failure output shown, timeout
  kills it, a dead one restarted with its new URL).
- `crates/cli/src/tunnel.rs`, named tunnel setup with a fake
  `cloudflared` that logs its calls: no certificate → `tunnel login` runs
  (and a login that writes nothing, or a missing `$TUNNEL_ORIGIN_CERT`, is
  an error); an existing tunnel is not created again, a missing one is;
  `route dns` "already configured" is success and an existing record is a
  conflict naming the dashboard fix; `--no-tunnel-setup` (and a name
  without a host) goes straight to `run`; `tunnel list` JSON (deleted
  tunnels, `null`); `--live-tunnel-url` parsing; the origin certificate
  search.
- `crates/cli/src/play_live.rs`: `h2 play --live-tunnel` with a fake
  cloudflared: discovery shows the URL, the game and sign-in link stay on
  loopback, a tunnelled redeem is refused, cloudflared stops with the
  session. `--live-tunnel` flags and their aliases (`--live-tunnel`
  implies `--live-local`); a named `--live-tunnel` session logs in, creates,
  routes and then runs, and only runs with `--no-tunnel-setup`.
- `crates/live-server/src/standalone.rs`: `h2-live-server`'s flags,
  including the command line `deploy/live/compose.yaml` runs (loopback,
  the id from `--public-url` on a first start only, `CF-Connecting-IP`
  trusted, the art shared, the web app from `H2_LIVE_WEB`); `public.rs`,
  `identity.rs`, `art.rs`: the lasting host, the first-start id and
  `--game-art` checks both binaries share; `crates/cli/src/live_cmd.rs`:
  `serve --behind-cloudflared` names a new server only.
- `crates/live-menubar`: `menubar.toml` defaults, partial files and round
  trips; plist XML (escaping, keys, and `plutil -lint`/`-extract` on
  macOS); `launchctl print`/`print-disabled` parsing; start, stop,
  restart, install, uninstall and the login toggle against a fake launchd
  (a disabled agent is enabled for the bootstrap and disabled again; an
  adopted plist is never rewritten); `/v1/admin/status` parsing from a
  recorded reply; snapshots from scripted health; the menu model (labels
  and which items are enabled when running, starting, failed, stopped,
  busy, not installed, started elsewhere); every item's action; the icon's
  states; `--adopt-existing`, `--install`, `--install-server`,
  `--install-tunnel`.
- `crates/live-menubar/src/update.rs`: against a fake git and builder,
  a first run builds and installs everything and restarts a loaded server
  (a stopped one stays stopped); then web-only, CLI-only, shared-crate,
  lockfile and docs-only changes build just what they need across one or
  several commits; dirty, off-branch and diverged checkouts are left
  alone (no fetch, merge or build); a failed build is retried only by
  Update Now; pause; an extractor version change checks the art and
  re-extracts from the configured disc; a rewritten history builds
  everything. The real git runs against scratch repositories with a local
  bare remote (fetch, ancestry, fast-forward, diff, show, dirty).
  `cargo metadata` path closures, atomic installs, the update menu lines,
  `[update]` config, and `--update-repo`/`--update-disc`/`--update-now`.
- `crates/live-server/src/sqlite.rs`, `pg.rs`, `store.rs`: the one
  event-store conformance suite over SQLite (a temporary file), memory,
  the file store and Postgres (`H2_LIVE_TEST_DB`), the concurrent-append
  race on SQLite and Postgres, two handles on one SQLite file, WAL mode.
- `crates/live-server/src/blobs.rs`: the blob-store conformance suite
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
- `crates/live-server/tests/s3_server.rs`: the blob store against a real S3
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
- `crates/live-server/tests/tunnel.rs`, `art.rs`, `local_signin.rs`,
  `public.rs`: proxy and CF headers refused for game art and local
  sign-in through the router; discovery's `url`; `CF-Connecting-IP` only
  trusted behind a tunnel; unclaimed admin names.
