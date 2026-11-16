# Hosting a Live server for friends (Cloudflare Tunnel)

`h2 live serve` binds loopback and refuses anything else without TLS
([live-security.md](live-security.md) §6). The easy way to let friends in
over the internet, with HTTPS and no port forwarding, is Cloudflare
Tunnel: `cloudflared` runs on your machine, connects out to Cloudflare, and
forwards each request (WebSockets included) to the server on loopback.
`--tunnel` runs it for you.

```
brew install cloudflared

h2 live serve --tunnel                   # quick tunnel: https://<random>.trycloudflare.com
h2 play disc.iso --live --tunnel         # the embedded server, published the same way

# friends
h2 play disc.iso --live-server https://<random>.trycloudflare.com
```

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

## Named tunnel (`--tunnel-name NAME --hostname HOST`)

One-time setup with a Cloudflare account and a domain on it:

```
cloudflared tunnel login
cloudflared tunnel create h2live
cloudflared tunnel route dns h2live live.example.net

h2 live serve --tunnel-name h2live --hostname live.example.net
```

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
  Cloudflare tunnel is configured and the request came from loopback;
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
- `crates/cli/src/play_live.rs`: `h2 play --live --tunnel` with a fake
  cloudflared: discovery shows the URL, the game and sign-in link stay on
  loopback, a tunnelled redeem is refused, cloudflared stops with the
  session.
- `crates/live-server/tests/tunnel.rs`, `art.rs`, `local_signin.rs`,
  `public.rs`: proxy and CF headers refused for game art and local
  sign-in through the router; discovery's `url`; `CF-Connecting-IP` only
  trusted behind a tunnel; unclaimed admin names.
