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

On `h2 play`, `--live-tunnel` implies `--live`. The older spellings still
work: `--live --tunnel`, `--tunnel-name NAME` for `--live-tunnel-name`,
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

`deploy/live/` runs the server, its Postgres and cloudflared with Docker
Compose: the API, the web dashboard and the stats site at one lasting
address (here `https://live.forbes.red`), with the game art shared. The
image has no disc and no art in it; you extract the art once on a machine
with the disc and the containers read it from there.

- `deploy/live/Dockerfile`: builds the web app with bun and
  `h2-live-server` (`crates/live-server/src/bin`, `standalone.rs`: the
  server and its `serve` flags without the rest of `h2`, so it builds on
  Linux in a few minutes), into a distroless image that runs as a non-root
  user. The web app is at `/srv/web` (`H2_LIVE_WEB`); `/data` is the
  server's key and id, `/art` the art.
- `deploy/live/compose.yaml`: three services.
  - `cloudflared` runs `tunnel run` with the token in `TUNNEL_TOKEN`.
  - `server` runs in cloudflared's network namespace
    (`network_mode: service:cloudflared`) and listens on `127.0.0.1:8074`
    with `--share-game-art --public-url https://live.forbes.red
    --behind-cloudflared --invite-only`; admins come from
    `H2_LIVE_ADMINS`. Its root filesystem is read-only and it has no
    capabilities.
  - `db` is Postgres 17, on a named volume, reachable only on the compose
    network.
  All three restart unless stopped.
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
restart (Postgres not up yet, a new image); a restarted server rejoins the
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
cp .env.example .env          # TUNNEL_TOKEN, POSTGRES_PASSWORD (openssl rand -hex 24), H2_LIVE_ADMINS
mkdir -p data
docker compose up -d --build
docker compose logs -f server
```

Make `data/` before the first start: if Docker creates it, it may be owned
by root and the server, which runs as uid 65532, cannot write to it. On
Linux set `H2_LIVE_UID`/`H2_LIVE_GID` in `.env` to your own ids so the
files stay yours (Docker Desktop on macOS maps ownership for you).

The first start creates `data/server.key` and `data/server.json` with the
id `live.forbes.red`, from `--public-url` (`--behind-cloudflared`, above).
Among the first lines of the log:

```
h2 live: public at https://live.forbes.red (through cloudflared)
h2 live: live.forbes.red on http://127.0.0.1:8074, log at position 0 (postgres://db:5432/h2live, public reads)
h2 live: web app from /srv/web
h2 live: serving game art from /art to everyone: ...
```

The id never changes after this. If the first start had the wrong
`H2_LIVE_PUBLIC_URL`, stop it, delete `data/` and the `pg` volume
(`docker compose down -v`), and start again before anyone signs up.

### The first admin account

The server is invite-only, so the first account needs an invite. The
operator commands sign with the key in `data/` and work against the public
URL from the machine that has it:

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
Invites, [live-admin.md](live-admin.md) §5). To drop `--invite-only` or add
`--private`, edit the server's `command` in `compose.yaml`.

### Backups

Two things hold the server's state, and you need both:

- `data/`: the server key and id. Lose it and every token and author id
  the server has issued is orphaned. Copy it somewhere private (it holds a
  secret key): `tar czf h2live-data.tar.gz -C deploy/live data`.
- Postgres: the event log (accounts, friends, games, title files).
  `docker compose exec -T db pg_dump -U h2live h2live > h2live.sql`
  while it runs. Restore into a fresh volume with
  `docker compose up -d db` then
  `docker compose exec -T db psql -U h2live h2live < h2live.sql`, then
  start the rest.

The art can always be extracted again.

### Updating

```
git pull
cd deploy/live
docker compose up -d --build
docker image prune -f
```

The server rebuilds and restarts; the data, the database and the id are
kept. `docker compose pull` updates cloudflared and Postgres (stay on major
version 17 unless you dump and restore). If a server restart ever leaves
the site down while cloudflared is up, `docker compose restart cloudflared
server` puts both back in one namespace.

### What Docker does not change

Only the service goes through the tunnel. Game traffic is UDP straight
between players' machines ([below](#what-the-tunnel-does-not-carry)), not
through this server, so hosting a game still needs UDP 3074-3080 reachable
on the host player's side.

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
  name). The `h2 play --live --tunnel` sign-in link stays a loopback URL.
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
- `crates/cli/src/play_live.rs`: `h2 play --live --tunnel` with a fake
  cloudflared: discovery shows the URL, the game and sign-in link stay on
  loopback, a tunnelled redeem is refused, cloudflared stops with the
  session. `--live-tunnel` flags and their aliases (`--live-tunnel`
  implies `--live`); a named `--live-tunnel` session logs in, creates,
  routes and then runs, and only runs with `--no-tunnel-setup`.
- `crates/live-server/src/standalone.rs`: `h2-live-server`'s flags,
  including the command line `deploy/live/compose.yaml` runs (loopback,
  the id from `--public-url` on a first start only, `CF-Connecting-IP`
  trusted, the art shared, the web app from `H2_LIVE_WEB`); `public.rs`,
  `identity.rs`, `art.rs`: the lasting host, the first-start id and
  `--game-art` checks both binaries share; `crates/cli/src/live_cmd.rs`:
  `serve --behind-cloudflared` names a new server only.
- `crates/live-server/tests/tunnel.rs`, `art.rs`, `local_signin.rs`,
  `public.rs`: proxy and CF headers refused for game art and local
  sign-in through the router; discovery's `url`; `CF-Connecting-IP` only
  trusted behind a tunnel; unclaimed admin names.
