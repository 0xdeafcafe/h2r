# Xbox Live replacement

Status: built, with the sections below marking what is not. The server
(`crates/live-proto`, `crates/live-server`, `h2 live ...` in
[4.2](#42-cli) and [4.3](#43-service)) has accounts with recovery codes,
title files, friends, teams, sessions, invites, stats, cloud saves, custom
maps, co-op sessions and a relay, and the client HLE (`crates/live-hle`)
starts at [Client HLE: sign-in](#client-hle-sign-in). It is on in the app
builds and opt-in for `h2 play` (`--live`). Sections 1 to 3 and 6 are the
original design and investigation; the HLE sections say what the game does
now. Not built: the in-game Create/Recover account menu
([In-game account setup](#in-game-account-setup)) and federation.

The goal is to make Halo 2 believe it is on Xbox Live: sign in with a
gamertag, show the message of the day and playlists, keep a friends list
with presence, and later matchmake and play. Accounts are made from the CLI
or the web app. The service behind it is ours, event sourced on SQLite
(Postgres if you prefer).

## 1. What the title links

Halo 2 statically links `XONLINES` 5849, the secure Xbox Live client. It
lives in two code sections plus a read-only data section:

| section   | VA           | size    | analyzer starts |
|-----------|--------------|--------:|----------------:|
| `XONLINE` | `0x003a0620` | 182,708 | 913 |
| `XNET`    | `0x003ccfe0` |  78,880 | 381 |
| `XON_RD`  | `0x0056ade0` |   8,616 | data |

Signatures name only 13 `XOnline` and 10 `XNet` functions
([xdk-libraries.md](../lifter/xdk-libraries.md)). Everything else has to be named by
hand.

**Boundary size.** A scan for direct `call rel32` from game code (`.text`,
`XPP`, `D3D`, `DSOUND`, `BINK`) into those two sections finds about 131
distinct targets from 185 call sites: ~94 in `XONLINE`, ~37 in `XNET`. That
is a raw byte scan, so a few are probably false positives, but it puts the
public API the game actually uses at roughly a hundred functions, not the
thousand in the library. `0x003cd718` (17 sites) is the busiest; the
`0x003cd0e1..0x003cd17d` cluster looks like the XNet socket calls (send,
receive, select).

**With Live off.** The kernel reports no cable (`crates/kernel/src/net.rs`,
`PhyGetLinkState` returns 0), so the title skips `XNetStartup` and every
Live path reports "network cable unplugged". Nothing in `XONLINE` runs.
With Live on, the HLE ([4.1](#41-client-recomp-side)) sets the link up and
answers the entry points instead.

## 2. What the real service looked like

Strings in `XON_RD` give the protocol map. The client gets Kerberos tickets
from `AS<n>.XBOXLIVE.COM`, `TGS<n>.XBOXLIVE.COM` and `MACS.XBOX.COM`
(machine account), opens an XSP tunnel to a security gateway, and then talks
HTTP (`POST %s HTTP/1.0`, `Content-type: xon/%x`) to services behind it:

| path prefix                    | service |
|--------------------------------|---------|
| `/xpnfront/xpresence.srf`      | presence, friends, game invites |
| `/xpnfront/xnotification.srf`, `xmessaging.srf` | notifications, messages |
| `/xpnfront/xteams.srf`         | teams (Halo 2 clans) |
| `/xmatch/xmatchhost.srf`, `xmatchclient.srf`, `xmatchhostdelete.srf` | session advertise and search |
| `/xqry/*.ashx`                 | generic query service (Search, Add, Update, Remove) |
| `/XArb/*.ashx`                 | arbitration (ranked results) |
| `/xstats/*.ashx`               | stats and leaderboards |
| `/stfd/readfile.ashx`, `writefile.ashx` | title and user storage |
| `/msgserver/*.ashx`, `/fbserver/fbserver.ashx` | string vetting, feedback |
| `/xsig/*.ashx`                 | content signing |
| `/xbos/ContentAvailable.ashx`  | downloadable content |

Bungie's own upload path, `/upload_server/stats.ashx` (multipart, boundary
`BUNGIEr0x0rz`), is game code, not XDK.

**Message of the day.** The game names two downloadable files,
`message_of_the_day.dat` (`0x00450f7c`) and `multiplayer_hopper.dat`
(`0x00453e60`, the playlists). Each is referenced from a table in `.data`
(`0x004672b8`, `0x0046dd5c`), which is presumably a list of title-storage
files to fetch after sign-in through `/stfd/readfile.ashx`. The format of
both files is unknown and is the first thing to decode.

## 3. Where to cut

Three places are possible. Decided (2026-11-15): C, the API boundary.

**A. The wire (what the Insignia revival does).** Leave `XONLINE` and `XNET`
as generated code, give XNet a real host socket, and write a server that
speaks Xbox Kerberos, XSP and the services above. It is the most faithful,
but it means reimplementing Microsoft's Kerberos variant and the XSP
crypto on the server side, and debugging crypto through generated code. Most
of the effort goes into protocol we then throw away.

**B. XNet only.** Replace XNet's sockets with plain host sockets and leave
`XONLINE` generated. The Kerberos and XSP exchanges still run, so we still
need a server that answers them. Same cost as A, less faithful.

**C. The XOnline API (recommended).** Replace the ~100 entry points the game
calls with Rust at the dispatch table, the same way D3D, DSound and Bink are
replaced ([ARCHITECTURE.md](../ARCHITECTURE.md)). `XOnlineLogon`,
`XOnlineTaskContinue`, friends, presence, matchmaking, stats and storage
become calls to our own service with a plain protocol (HTTP + JSON, or
WebSocket for pushes). No Microsoft keys, no Kerberos, no XSP. The XDK's
public `xonline.h` gives argument shapes, and the task model is simple: each
call returns an `XONLINETASK_HANDLE`, the game pumps it with
`XOnlineTaskContinue` until it stops returning `XONLINETASK_S_RUNNING`, then
reads results.

C still needs an XNet replacement for game traffic: the sockets, `XNADDR`,
`XNKID`/`XNKEY` and the secure-address helpers (`XNetXnAddrToInAddr`,
`XNetInAddrToXnAddr`, `XNetRegisterKey`; the game logs
"XNetTsAddrToInAddr() failed in register_ip_and_key()"). Those values are
opaque to the game, so we can pack a host IP and port into them. This part is
shared with System Link, which is worth doing first: it needs no service at
all and proves the socket layer.

## 4. Pieces

### 4.1 Client (recomp side)

- **Link up.** `ConsoleConfig::phy_link` already exists; Live on (`h2 play
  --live`, `--live-server` or `--live-local`, or an app build) sets it so
  XNet starts.
- **XNet HLE.** Socket calls on host UDP and TCP; addresses packed as above.
- **XOnline HLE.** A `live` module in the runtime holding the task table,
  the signed-in users, and a client for the service. Tasks run on a host
  thread and complete into the handle the game polls, so guest code never
  blocks on the network.
- **Accounts.** The console's accounts come from `XOnlineGetUsers`. The HLE
  answers from a host file the CLI writes, so no HDD config-sector work is
  needed.

### 4.2 CLI

Implemented (crates/cli/src/live_cmd.rs, live_accounts.rs, live_query.rs,
play_live.rs; `h2 live --help` has every flag):

```
h2 live serve [--db URL] [--blobs LOC] [--listen ADDR] [--data DIR] [--id HOST]
              [--private] [--invite-only] [--admin GAMERTAG]...
              [--game-art DIR] [--share-game-art]
              [--tls-cert PEM --tls-key PEM | --insecure-http]
              [--tunnel | --tunnel-name NAME | --public-url URL]
h2 live account create <gamertag> [--server URL] [--invite CODE]
h2 live account code [--as GAMERTAG] [--server URL]
h2 live account recovery [--reset] [--as GAMERTAG] [--server URL]
h2 live account recover <gamertag> [--code CODE] [--server URL]
h2 live account import <file> [--server URL]
h2 live account list [--server URL]
h2 live motd set <line>... [--header TEXT] [--server URL] [--data DIR]
h2 live motd set --file PATH [--server URL] [--data DIR]
h2 live account export <gamertag> [--file PATH] [--server URL]
h2 live admin grant|revoke <gamertag> [--server URL] [--data DIR]
h2 live hopper import <file> [--title ID] [--server URL] [--data DIR]
h2 live hopper defaults --out FILE [--raw] [--force]
h2 live friend add|accept|decline|remove|list ...
h2 live team ...
h2 live maps publish|list ...
h2 live assets extract [--disc PATH] [--out DIR] [--mappack ISO] [--installed DATA]
h2 live assets check [--out DIR]
h2 live store copy [--from DB --to DB] [--from-blobs LOC --to-blobs LOC]
h2 live menubar [--install | --uninstall | --status ...]
h2 live invite create [--days N] [--code TEXT] [--as GAMERTAG] [--server URL] [--data DIR]
h2 live invite list [--as GAMERTAG] [--server URL] [--data DIR]
h2 live invite revoke <jti> --as GAMERTAG [--server URL]
h2 live q <what> [--server URL] [--json|--jsonl]
h2 play <disc> [--live | --live-server URL | --live-local | --offline] [--live-listen ADDR] [--gamertag NAME] [--invite CODE]
```

`h2 live serve` serves the web app from `web/dist` (or `$H2_LIVE_WEB`) at
`/`, with `index.html` for any path that is not a file or under `/v1/`; it
prints a hint to run `cd web && bun run build` when there is none.

Defaults: `--db` is `$H2_LIVE_DB` or `sqlite://<--data>/live.db` (a
server with no `live.db` but a log in `postgres://localhost/h2live` keeps
that, with a notice saying how to copy it; [live-hosting.md](live-hosting.md#free-hosting-sqlite-and-cloudflare-r2)),
`--listen` `127.0.0.1:8074`, `--server` `$H2_LIVE_SERVER` or
`http://127.0.0.1:8074`, `--data` the server directory below.

The same server can also run as a Cloudflare Worker, with no machine of
yours in the path: [live-worker.md](live-worker.md) (in progress).

Host data lives in `~/Library/Application Support/h2recomp/live`, beside
the movies directory and the VFS data root ([storage.md](../runtime/storage.md)):

- `accounts.json` (mode 0600): one entry per account held here, `{xuid,
  gamertag, id, server, server_id, kid, key}`, where `key` is the account's
  Ed25519 private JWK (`{kty, crv, x, d}`; older files hold the bare
  base64url `d`, which still reads). `account create` generates the key
  locally, signs the creation request (with `aud` = the server id and
  `iat`) and stores it; the server only ever sees the public half.
  `account create` also prints the account's recovery code and
  authenticator key ([live-security.md](live-security.md) §1, Recovery).
  To use the account somewhere else, `h2 live account code` on a machine
  that has it (or the web dashboard's recovery screen, or an authenticator
  app) shows the current six-digit code, and `h2 live account recover
  <gamertag>` on the new machine (or Recover in the web app) makes a new
  key there and adds it with that code; no invite needed. `h2 live
  account recovery` shows the authenticator key again, sets recovery up
  on an account made before it existed, and with `--reset` replaces it.
  `account import` takes the entry the web app downloads after a browser
  sign-up (`{xuid, gamertag, server, key}`, or a list of them), checks that
  the key parses and is a current key of that XUID on the server
  (`/v1/accounts/by-xuid/{xuid}/keys`), and adds it, refusing an XUID that
  is already there or a gamertag already held for that server. The
  game-side HLE answers `XOnlineGetUsers` from `xuid` and `gamertag`, so no
  HDD config-sector work is needed.
- `play-accounts.json` (0600): the one account `h2 play` hands the
  game, since the HLE takes every entry of the file it is given.
- `server/` (mode 0700): `h2 live serve`'s identity, `server.key` (0600)
  and `server.json` (`{"id"}`). The server id is set on first start
  (`--id`, default `localhost`) and cannot change, because every author id
  contains it.

Operator commands sign with the server key in `--data`, so they run on the
server's machine: `motd set` publishes `message_of_the_day.dat` (built from
up to 32 lines plus an optional header, in the format of
[live-motd.md](live-motd.md) §3.1, the same text in all nine language
slots; or a prebuilt `--file`), and `invite create` prints an invite code
for an `--invite-only` server (`POST /v1/invites` with an operator
token). Clients find everything else from the base URL's
`/.well-known/h2live.json`, and refuse to send a token over plain HTTP to
anything but loopback.

#### Playing on Live

`h2 play` is offline unless a flag asks for Live, and contacts nothing to
decide (`choose` in `crates/cli/src/play_live.rs`). The app builds are the
other way round: on by default (below).

| `h2 play` flags | Plays | `$H2_LIVE_SERVER` |
|---|---|---|
| none, `--offline`, `--no-live` | offline | ignored |
| `--live` | signed in to `https://live.forbes.red` | a URL replaces it; `off` (`none`, `0`) plays offline with a line |
| `--live-server URL` | signed in to `URL` | ignored (the flag wins) |
| `--live-local` | an embedded server in the process | ignored |
| `--live-listen ADDR`, `--live-tunnel`, `--live-tunnel-name NAME` | as `--live-local` (they imply it) | ignored |
| `--live-local` with `--live-server` | an error: pick one | |

The default service is `live_proto::DEFAULT_SERVER`, the one place it is
written; the CLI (`DEFAULT_LIVE_SERVER`) and the app builds share it. It
prints `live: signing in to https://live.forbes.red (the default service;
--live-server URL names another)`. A Live flag that cannot sign in stops
`h2 play` with the reason; there is no fallback to offline. `--offline`
says the default out loud and does not go with any Live flag. `--gamertag
NAME` picks the account on whichever service is played on, so it needs a
Live flag.

**`h2 play --live-local`** starts a service inside the process on a background
runtime: Postgres from `$H2_LIVE_DB` or `postgres://localhost/h2live`
(creating the database if it is missing), else a file-backed log in the
data directory (`server/log`: `events.jsonl`, `blobs/`, `checkpoints.json`),
so accounts, friends, teams, games and title files survive a restart; the
in-memory store only if another server holds that directory, with a
warning. When no MOTD was ever published, the embedded server publishes a
default (`default_motd.rs`). It binds 127.0.0.1 on a free port (`--live-listen ADDR`
for a fixed one) with the identity in `live/server`, so its accounts are
`h2 live serve`'s. With `--live` or `--live-server URL` it uses a running
service instead. The
account is the first local one for that server whose key the server still
lists. Otherwise the player's identity moves with them: the gamertag is
`--gamertag NAME`, else the one last played on another server
(`last_played` in accounts.json), else one made from the OS user name
(letters and digits, a single space for anything else), and an account
of that gamertag on another server lends its key, so the player keeps one
key everywhere. If the gamertag is new to the server it is made there
(on an invite-only server with `--invite CODE`, or an invite asked for on
the terminal; long and 25-character codes both work). If the server has
it already (made from the web or another device), `h2 play` does not ask
on the terminal: it hands the game that gamertag marked `needs_code`, and
the game asks for the current recovery code (Account on the server's
`/dash`, or an authenticator app) when the player signs in to Xbox Live
([In-game sign-in](#in-game-sign-in)), then adds this machine's key
([live-security.md](live-security.md) §1, Recovery). `--signin-terminal`
asks on the terminal before the game starts instead, as before (and
without a terminal stops with the `h2 live account recover <tag> --server
URL` command); `h2 live account recover` still works on its own. On the embedded server (`--live-local`) a taken default name is
numbered instead. It prints the gamertag and, for the embedded server, the web app's
URL, then boots with `boot::LiveConfig {
enabled, accounts_path: play-accounts.json, server_url }`. The server stops
when the game does.

**The app builds** (tvOS and iOS, `crates/apple-host`; the Xbox app when
there is one, docs/runtime/windows.md) are on by default against the same
service (`live_hle::platform`). `live.toml` in the app's data root is the
switch:

```toml
enabled = false                    # play offline
server = "https://live.example"    # or sign in to another service
```

A file that does not parse is ignored with a line, so the default holds.
`$H2_LIVE_SERVER` (from the Xcode scheme, say) wins over the file for a
run: a URL replaces the server, `off` turns Live off. The accounts file is
`live/accounts.json` under the data root, which the in-game account setup
(below) writes; until it holds an account for the server, Live is on but
nothing is signed in and the game's Live notice says how to make one.
With one, the app's saves are synced as `h2 play`'s are
(`cloud_saves::session`, shared by both).

Signed in anywhere, the account's saves follow it
([live-cloud-saves.md](live-cloud-saves.md)).

With the embedded server it also prints a one-use link
(`/signin#once=…`, loopback only, ten minutes) that signs this machine's
browser in as the playing account, an admin there for the session; see
[live-admin.md](live-admin.md) §4.

For the embedded server only, before starting, it builds the web app (`bun install --frozen-lockfile`,
`bun run build`) when `web/dist` is missing or was built from a different
`web/` tree (`web/dist/.h2-web-tree` records it; with uncommitted changes
under `web/` an existing build is kept). `h2 play --follow` does the same
in each commit's build, so the banner's "Update ready" includes it. Without
bun it says how to install it and runs without the web app; `H2_LIVE_WEB`
points at a build of your own and skips this.

`h2 live friend add|accept|decline|remove|list` act as a local account
(`--as GAMERTAG`): each writes a signed friend event on that account's
stream (`crates/cli/src/live_cmd.rs`, `friend`).

#### Game art

`h2 live assets extract [--disc PATH] [--out DIR]` decodes the stats
site's art from your own disc into PNGs and a `manifest.json`. The default
`--out` is `game-art/` in the server directory. It covers emblem layers, rank
icons, Live icons, game-type icons, weapon silhouettes, medals, the player
colour palette, every multiplayer map's preview and every campaign level's
level-select picture (`map/<stem>/preview` too). A cold run takes well
under a second. Keys and emblem colouring are in
[live-api.md](live-api.md#optional-game-art).

- **How the art is found.** No asset is named in the code (CONTRIBUTING.md,
  [bitmaps.md](../formats/bitmaps.md)). Each category is the bitmap tag in the main menu
  or shared map with a known image count and image shape. Where several
  sizes of the same set exist, the largest wins. A map preview is the one
  single-image UI picture a multiplayer map has that the shared map does
  not. A campaign level's preview is the picture the main menu's level
  select names for it ([live-admin.md](live-admin.md#the-catalog)). Maps
  are told apart by the cache type in their header. Medals and
  the palette are icon glyphs in the fonts' private-use range
  ([fonts.md](../formats/fonts.md)): the medals are the 24-pixel-tall icons and the
  palette the flat colour swatches, from the first font in the engine's font
  table that has the full set.
- **Output.** It is deterministic for a disc. Anything not found or not
  decoded is listed under `skipped`. The manifest's `source` is a SHA-256
  over the extractor version and every map file's path, size, sector and
  first 2 KiB; this is the cache key.
- **Serving.** `h2 live serve` serves the art at `/art/game/` from
  `--game-art DIR`, or from `game-art/` in `--data` when it is there. By
  default it goes only to clients on the same machine, since it is your copy
  of the game's files. `--share-game-art` serves it to everyone and says at
  startup that this redistributes them.
- **`h2 play --live-local`** extracts the art once per disc (on first run, or when
  the cache key changes) and serves it from the embedded server. Without
  art, `/art/game/manifest.json` is a 404 and the web app uses its own art.
- **Map geometry.** The same run writes every multiplayer map's level
  geometry (`map/<stem>/map.glb`) and five pre-drawn, lit views with exact
  world-to-pixel matrices (`map/<stem>/views.json`, `views/*.png`), listed
  under the manifest's `maps`. This adds 2 to 3 seconds. Format and
  pipeline: [live-art.md](live-art.md).
- **Map pack maps.** `--mappack ISO` reads the map pack disc's maps too, and
  the extractor reads maps `h2 import-mappack` installed under the default
  data root (`--installed DATA` names another); they get the same pictures
  and geometry ([live-art.md](live-art.md)).
- **`h2 live assets check [--out DIR]`** exits 0 when the directory holds art
  from this `h2`'s extractor and 1 when it is missing or older, so a script
  knows when to extract again (`deploy/live/setup.sh` does).

#### For agents: `h2 live q`

Read-only queries. Output is JSON when stdout is not a terminal (or with
`--json`), one JSON value per line with `--jsonl` (each item of a list), a
compact table on a terminal. Lists follow every cursor, so they are whole.
`--server` defaults as above. Failures exit non-zero with the server's
`{error, message}` in the text.

| Command | Output |
|---|---|
| `q accounts` | `[{xuid, gamertag, created_at, online}]` |
| `q account <gamertag>` | `{xuid, gamertag, created_at, online, id, recent_games: [GameSummary], position}` |
| `q presence` | `[{xuid, gamertag, state, game_id?, since}]` |
| `q games [--live\|--ended]` | `[GameSummary]`, newest first: `{game_id, map, mode, variant, team_play, state, started_at, ended_at, players: [{index, name, xuid?, team, local?}], teams}` |
| `q game <id>` | `GameSummary` plus `{console, host, score_limit, time_limit_s, tick, last_tick, scoreboard: [{index, score, kills, deaths, assists}], kills: [{tick, victim, killer?, weapon?, kind, pos?, position, at}], winner, end_reason, position}` |
| `q game <id> --frames` | `[{game_id, tick, players: [{index, alive, pos, yaw, vehicle?}], source}]` (the 1 Hz recorded frames) |
| `q sessions`, `q session <id>` | the open session advertisements, or one |
| `q teams [gamertag]`, `q team <id>` | open teams (or one account's teams and pending recruits), or one team with members, properties and permissions |
| `q friends <gamertag>` | `{friends: [{xuid, gamertag, friends_since, presence?}], requests_in, requests_out}` |
| `q events [--from N] [--type T] [--stream S]` | `[{position, stream, version, type, at, data, author, kid, jws, jws_index?, hash}]` after position N |
| `q events ... --follow` | the same, then one event per line as they are appended (JSONL, from `/v1/stream`) |
| `q keys <gamertag>` | `{keys: [{kid, jwk, valid_from, valid_to}], position}` |
| `q verify [--from N]` | `{checked, anchored, bad: [{position, stream, type, problem}], position}`; exits 1 if `bad` is not empty |
| `q health` | `{ok, position}` |

`verify` re-checks the log on this side with `live_proto::audit`: each
event's JWS opens to exactly its `stream`, `type`, `data`, `author`, `kid`
and `at` (the batch element at `jws_index`), verifies under the author's
key at that position (the server's from discovery, an account's from its
keys with `valid_from`/`valid_to`), and its `hash` extends the stream's
chain. A stream whose first event read is not its first (version 0, with
`--from`) is taken as anchored there and counted in `anchored`.

### 4.3 Service

Two crates. `crates/live-proto` is what clients and server share, with no
runtime: the wire types of [live-api.md](live-api.md) (XUIDs as 16 hex
digits, title IDs as 8, gamertags with Live's rules: 1..=15 letters, digits
and single spaces, starting with a letter, compared case-insensitively),
the signing code of [live-security.md](live-security.md) (`sign`: Ed25519
keys as JWK with RFC 7638 thumbprints, compact JWS, signed event bodies and
batches, self-issued tokens, invites, the hash chain; `jcs`: RFC 8785
canonical JSON) and the MOTD builder (`motd`). The game-side client and
the CLI sign with exactly this code. `crates/live-server` is the service
(axum, tokio, sqlx with runtime-checked queries; see its README).

The API is [live-api.md](live-api.md) on `127.0.0.1:8074`. Built: health,
discovery, accounts (create, list, one, keys by gamertag and by XUID),
presence (with `POST /v1/presence/ping`), invites (`POST /v1/invites`,
operator), the event log (`GET /v1/events` with `from`/`cursor`, `type`,
`exclude`, `stream`, `limit`; `POST /v1/events` for one signed event), the
`/v1/stream` WebSocket (replay from `?from=`, `caught_up`, then live events
and game frames, with `types`/`exclude`/`stream`/`games`), title files,
games, the `/v1/ingest` WebSocket and map outlines. Deviations and choices
the contract leaves open:

- `POST /v1/accounts` and `POST /v1/events` take the compact JWS as the
  raw body (`application/jose`). `POST /v1/events` is not in the contract's
  write table; it is how accounts write `KeyAdded`, `KeyRevoked` and
  presence.
- Title files are published by the operator only: a token signed by the
  server key. They are capped at 2 MiB, not the 64 KiB of security §6,
  because `multiplayer_hopper.dat` inflates to 1.4 MB. The bytes live in a
  `blobs` table keyed by SHA-256; the event carries `{title_id, name,
  sha256, size}`.
- Error codes are the contract's plus `too_large` (413).
- `POST /v1/events` answers 201 with the whole stored event, a superset of
  `{position}`.

**Games** (crates/live-server/src/games.rs, live-proto `game`). `POST
/v1/games` (`games.write`) takes the game without an id; the server picks
a random 32-hex `game_id` and writes `GameStarted` itself, signed by the
server key, with `host` = the token's author, on `game-<id>`. Consoles
then post batches to `POST /v1/games/{id}/events`: one batch JWS of
`PlayerJoined`, `PlayerLeft`, `PlayerKilled`, `ScoreChanged` and
`GameEnded` bodies by the token's author, who must be the host or a
player's account. Each body is validated by type against the players as
the batch changes them (indexes exist and are unique, at most 16 players,
kill kinds and end reasons from the contract), `GameEnded` must be last,
and nothing of a refused batch is written; a replayed batch is 409, a
write to an ended game 409. `GET /v1/games` (newest first, `state=live|
ended`, cursor = the start position) and `GET /v1/games/{id}` (scoreboard,
team scores, the last 50 kills with `pos` and log position, winner, end
reason) come from the read model; `GET /v1/games/{id}/frames` pages the
recorded frames (`from_tick`, `to_tick`, 600 a page). Accounts list their
10 latest games as `recent_games`.

**Ingest.** `/v1/ingest` is the game's WebSocket: the first message is
`{kind: "auth", token}` (`games.write`), then `frame`, `events` (`{game_id,
jws}`: one batch JWS, answered `{kind: "appended", game_id, position}` or
`{kind: "error", error, message}`) and `ping`. Frames go out to stream
readers at most 15 a second per game; one a second per game is also
written as a server-signed `GameFrame` whose `data.source` is the sender.
The socket closes (4401) when its token expires unless a fresh `auth` by
the same author arrives.

**Map outlines.** `PUT /v1/maps/{map}/outline` (`games.write`, 2 MiB) stores
the outline as a blob and a server-signed `MapOutlineStored` (`{map,
sha256, size, source}`) on `map-<map>`. The first upload wins (409 after);
the operator may replace it. `GET` returns the bytes.

**Managers.** One sweeper task (every 5 s) ends what clients leave behind,
as the server: presence with no event or ping for 90 s gets
`PresenceExpired`, and a game with no frame or event for 5 minutes gets
`GameEnded {reason: "lost"}`. Pings (`POST /v1/presence/ping`, or `ping` on
the ingest socket) refresh the in-memory deadline only. Deadlines start
from the projection (the last event's `at`), so they survive a restart.

**Security** ([live-security.md](live-security.md), all but §5
federation). Every event is a JWS by its author; the server writes its own
(`GamertagReserved`, `AccountCreated`, `KeyAdded` at creation, title
files) with its key. An append checks, all in memory: the bearer token
(EdDSA by a current key of `iss`, `aud` = server id, `exp - iat` ≤ 60 s,
`lxm`, `jti` unseen for two minutes), that the event's author is the
token's issuer, the event signature against the author's current key,
`at` within 5 minutes, the stream permission (an account writes only key
events on `account-<own xuid>` and presence on `presence-<own xuid>`),
the event's data (a `KeyAdded` key must be new, the last key cannot be
revoked, presence states are `online`/`away`/`playing`), and a fresh
nonce. Account creation is the one write without a token: the request is
signed by the key it registers, and `KeyAdded` keeps it as `proof`.
Invites are JWS by the server key, single use. Writes are rate limited
per author and method, account creation server-wide; bodies are capped at
64 KiB (title files 2 MiB, WebSocket messages 16 KiB). Reads are public
unless `--private`, which needs a `read` token on every read and as the
first stream message; discovery and `/healthz` stay public. CORS allows any
origin on GET and HEAD and nothing else. Binding to a non-loopback address
needs `--tls-cert`/`--tls-key` (rustls) or `--insecure-http`. The database
URL is never printed with its credentials.

**The log.** SQLite by default, Postgres if asked (the description below is
Postgres's; SQLite's `sqlite_schema.sql` is the same table, and its appends
take an in-process lock and `BEGIN IMMEDIATE` for the same commit-order
positions). One append-only `events` table: `position bigserial`,
`stream_id`, `version`, `type`, `data jsonb`, `at`, `author`, `kid`, `jws`,
`jws_index` (batches), `hash`, `batch`, unique on `(stream_id, version)`.
The `jws` is the author's, untouched, and the source of truth: events are
decoded from it, and the other columns are copies for querying. A batch
JWS is stored once, in `batches (sha256, jws)`: its events' rows have an
empty `jws` and name it in `batch`, and reads join it back. Rows written
before that keep their own `jws` and a null `batch` (the column is added
on open), so an old log reads and verifies as it did; a log written this
way needs a server that knows `batches`. `hash` is
`hex(sha256(prev || jws))` per stream, computed in the append transaction.
Appends take a list of events and expected versions (0: the stream must
not exist) and are atomic across streams: account creation writes the
gamertag stream (expected 0, which is what makes a gamertag taken) and the
account stream in one transaction. They hold `pg_advisory_xact_lock` on a
fixed key for the whole transaction, so `bigserial` positions are handed
out in commit order (without it a tailer could read 12, miss an 11 that
commits later, and never go back for it), and send `pg_notify('events',
position)` before committing. Serialising appends caps write throughput,
which is fine at this scale. Schema creation takes its own advisory lock.
`subscriptions(name, position)` holds checkpoints for durable subscribers
(the store has `load_checkpoint`/`save_checkpoint`; none use them yet).
XUIDs are `0x0009 << 48 | random 48 bits`, the marker real Live account
XUIDs carry, with no guessable sequence; a collision fails the account
stream's version check and is retried.

**Subscriptions.** One `Dispatcher` tails the log by position and feeds
every `Subscriber` in order, woken by LISTEN (a watch channel on the
in-memory store) with a one-second poll as the fallback, so a lost wake
costs latency, not correctness. The read model (accounts, keys with their
valid-from/to positions, title files, presence, recent nonces and used
invites) is one subscriber, replayed from position 0 at startup before the
server listens. After each event the dispatcher publishes the applied
position and broadcasts the event: write handlers wait for their position
(read-your-writes), and stream readers subscribe to the broadcast before
replaying from the store, so nothing falls between the two; a reader that
lags re-reads from the store. Every read reply carries that applied
position.

SQL rather than Mongo: an event store needs ordered append with a
uniqueness check per stream, which is one table and one constraint. SQLite
is the default since 2026-11-16 (one file beside the key, no database
server, backed up to R2 by Litestream); Postgres stays supported, and
`h2 live store copy` moves a log between the two with positions kept.

Everything else is an event too, presence and matchmaking sessions
included (decided 2026-11-15), so the log is the full history of who was
online, where and with whom. To keep that from flooding the log:

- **Changes, not pings.** A presence stream records `SignedIn`,
  `PresenceChanged` (title, state, session), `SignedOut`. Keep-alive pings
  only refresh an in-memory deadline and are never written. (Built; see
  Managers above.)
- **Expiry is an event.** When a deadline passes, the server appends
  `PresenceExpired` (or `SessionExpired`), so a crashed client still ends
  its stream and a replay gives the same answer.
- **Session streams** per matchmaking session: `SessionCreated`,
  `SessionUpdated` (attributes, open slots), `PlayerJoined`, `PlayerLeft`,
  `SessionDeleted`. `XOnlineMatchSessionUpdate` calls that change nothing
  are dropped.
- **Pushes** to clients go out over a WebSocket from the same append path.

## 5. Order of work

1. **Name the boundary.** Done statically: [live-boundary.md](live-boundary.md)
   lists 130 entry points (62 confirmed, 68 likely), the sign-in sequence and
   how each logon result routes to a dialog or the dashboard. Sign-in also
   needs `XOnlineLogonTaskGetResults` (connected = `0x001510f0`, separate
   from task success `0x001500f0`), `XOnlineGetServiceInfo` for 13 services,
   and an `XNetQosServiceLookup` that completes; the logon task must stay
   open while signed in.
2. **Signed in, offline service.** Link up, XNet startup, `XOnlineStartup`,
   `XOnlineGetUsers`, `XOnlineLogon` succeed for a CLI account with a fixed
   in-process stub. Success is the game showing the gamertag as online.
3. **Service and MOTD.** Done: `h2 live serve`, accounts, title storage
   serving files by name, the `message_of_the_day.dat` and
   `multiplayer_hopper.dat` formats ([live-motd.md](live-motd.md),
   [live-admin.md](live-admin.md)) and the client HLE that fetches them.
4. **Friends and presence.** Done (server and HLE; [live-api.md](live-api.md#friends)).
5. **System Link over XNet HLE** (two local instances). Done
   ([Client HLE: sockets and System Link](#client-hle-sockets-and-system-link)).
6. **Matchmaking and custom games over Live**, then stats and clans. Done
   on tests ([live-matchmaking.md](live-matchmaking.md),
   [live-custom-games.md](live-custom-games.md), [live-telemetry.md](live-telemetry.md),
   [Teams](#client-hle-teams)); co-op over Live too
   ([live-coop.md](live-coop.md)). The HLE has been run in the game only
   where those pages say so.

## 6. Open questions

- The `.data` tables at `0x004672b8` and `0x0046dd5c` are answered in
  [live-motd.md](live-motd.md) §1 and §2.
- What `XOnlineLogon` failures the game checks for (title update required,
  service down) and which ones send it to the dashboard (`xonlinedash.xbe`
  is referenced from the image).
- Whether Halo 2 needs `XOnlineTitleUpdate`-style checks to pass before
  matchmaking; title updates themselves are covered in
  [title-updates.md](../runtime/title-updates.md).
- NAT: direct UDP works on a LAN. Across the internet the service will have
  to relay or punch holes; not needed for the first milestones.

## Client HLE: sign-in

`crates/live-hle` replaces the public entries in
[live-boundary.md](live-boundary.md) when the host enables Live. It uses
`Dispatch::register_override`, as XAPI and DSound do. Disabled is the default:
it registers nothing, reads no account file and leaves the console PHY setting
alone. These addresses apply to the retail 5849 image; they are not a title-update
matcher (see [title-updates.md](../runtime/title-updates.md) §1.1).

The host passes one `boot::LiveConfig` to `boot::run_with_live`, or to
`boot::run_hosted_with_live` for a windowed host. The existing `run` and
`run_hosted` keep Live disabled. The CLI supplies `enabled`, `accounts_path`
(`Option<PathBuf>`) and `server_url` (`Option<String>`). For example:

```rust,ignore
let live = boot::LiveConfig {
    enabled: true,
    accounts_path: Some(accounts_path),
    server_url: Some(server_base_url),
};
boot::run_hosted_with_live(&xbe, &dylib, &opts, host, &live)?;
```

`boot::live_glue::install` is the entry point for an embedding host that already
owns a runtime and kernel. It installs the boundary before guest execution and
sets the existing `ConsoleConfig::phy_link` to active, 100 Mbps, full duplex.
Configuration or registration errors leave the PHY setting unchanged.

The accounts file is a JSON array. Each entry needs a nonzero hexadecimal
`xuid` string and a gamertag of 1–15 printable ASCII bytes. Uppercase hex and
an optional `0x` prefix are accepted. Up to 16 distinct accounts are supported.
The optional `key` is a private Ed25519 JWK, parsed by
`live_proto::sign::KeyPair::from_private_jwk`. Keys stay in host memory; guest
records and configuration errors never contain them. The additional `server`
and future fields are ignored: `LiveConfig.server_url` chooses the server for
all calls. Missing or malformed keys allow local/System Link identities, but
server logon rejects them as invalid users. Invalid identity files fail
configuration without echoing their contents. No path means an empty account
list; the client does not invent an account.

With `server_url = None`, sign-in keeps its local-only behavior. With a URL,
logon authenticates against that service and publishes signed presence, as
specified in [Client HLE: service logon and title resources](#client-hle-service-logon-and-title-resources).

Transport startup succeeds through XNetStartup, WSAStartup and XOnlineStartup,
with paired reference-counted cleanup. GetUsers writes the fixed 16-record
buffer and an output count. Records are 112 bytes, with the XUID at offset 0,
gamertag at 12 and per-user HRESULT at 108. Logon accepts four controller slots,
checks their identities against the configured accounts and returns a persistent
SDK task handle. With a server, TaskContinue and LogonTaskGetResults remain
running (`0`) until verification and SignedIn finish. Success returns connection
established (`0x001510f0`). GetLogonUsers then returns a stable guest pointer to
four records with per-user HRESULT zero. Closing the connection clears that array
and invalidates its tasks. ChangeLogonUsers waits for its server requests, commits
the replacement array, then reports completion on the next pump. A rejected change
returns `0x80151300` and its result getter supplies four per-user HRESULTs; failed
validation preserves the previous users. ServiceInfo succeeds for every requested
service ID, with an optional endpoint output. These endpoints remain local XNet
placeholders; HTTP service traffic uses the configured URL.

XNetGetTitleXnAddr returns the static-IP bit (`4`), rather than pending. Its
36-byte XNADDR packs the host's default-route LAN IPv4 into `ina` and `inaOnline`
(offsets 0 and 4), in network byte order. `wPortOnline` at offset 8 is the configured host base port
(default 3074), also in network byte order. The Ethernet identity is synthetic; the remaining
online identity bytes are zero. Service endpoints use the same IP and port.
The host address comes from an OS route lookup using a UDP socket without sending
a datagram; a host with no IPv4 route falls back to loopback. Socket channel
translation uses that base port, as described below. Embedders and tests can pass
an explicit endpoint to `live_hle::Hle::with_address`, or channel and discovery
settings to `Hle::with_network`.

The sign-in screen also starts a QoS service lookup. The HLE supplies an XNQOS
header with one complete, uncontacted result and pending count zero, so polling
completes immediately. The full 24-byte XNQOSINFO entry follows the header because
the game reads entry zero without checking the result count. QosRelease invalidates
the record; a repeated release fails. It does not measure latency or contact the
service.

The immediate follow-up tasks have these local behaviors:

| Entry | Behavior |
|---|---|
| XOnlineFriendsStartup | Returns an empty task. Its first pump completes; later pumps return RUNNING_IDLE so the game's disposer can drain it. |
| XOnlineOfferingIsNewContentAvailable | Returns a task whose result is NO_NEW_CONTENT. |
| XOnlinePresenceInit | Returns a task; with a server it waits for the controller's presence request. |
| XOnlinePresenceAdd | Stores a bounded watch group of packed 12-byte XUIDs. |
| XOnlinePresenceSubmit | Queues the controller's current presence; its pump waits for the server acknowledgement. |
| XOnlineMutelistStartup | Returns an empty task that completes successfully. |
| XOnlineMutelistGet | Writes count zero and returns a completing task; no mute entries are written. |

Work-event handles other than NULL and unobserved QoS lookup flags stop with a
named unverified-contract diagnostic. Guest pointer faults use the runtime's
memory-fault path. Unknown or closed task handles fail rather than being reused.
The shared runtime HLE window supplies guest storage through `Guest::map_zeroed`;
existing mappings and claims are skipped. One claimed 64 KiB region holds the
stable user array, task handles and QoS headers. Tokens advance without reuse,
even after cleanup, so stale handles cannot name later tasks. Exhaustion returns
out-of-memory rather than growing indefinitely.

All 130 named boundary targets are registered when enabled: 116 have implemented
results, including deliberate empty views or failed tasks, and 14 remain named
stops. The menu audit and remaining-stop list appear below. A stop reports
`unimplemented Live API <name>` before reading arguments or executing the linked
library. There is no silent fallback to XONLINES. Tests establish these API and
host-transport contracts; they do not establish a playable Live session.

Synthetic tests call every implemented entry through the real runtime Runner
with guest stack arguments. They cover the packed records, stable pointers,
connection and change-user task lifetimes, requested service results, empty tasks,
QoS completion, invalid inputs, disabled registration, named stops and boot PHY
configuration. They require neither an XBE nor a built game image.

## Client HLE: sockets and System Link

The XNet boundary now uses host IPv4 sockets on macOS and Linux. Socket handles
are increasing guest integers, not host descriptors. `socket` accepts UDP,
TCP and Xbox VDP (protocol 254). VDP uses a UDP socket and carries the title's
payload unchanged. The host does not implement Xbox IPsec, VDP encryption,
Xbox broadcast authentication or interoperability with retail consoles.
Two HLE clients use the same plaintext transport.

The implemented socket entries are socket, bind, connect, send, sendto, recv,
recvfrom, select, __WSAFDIsSet, ioctlsocket, setsockopt, getsockopt, shutdown,
closesocket, WSAGetLastError, htonl and htons (including their inverse aliases).
TCP is a client transport here; the verified boundary has no listen/accept
entry. Sockets start blocking. FIONBIO (`0x8004667e`) switches modes, including
back to blocking. FIONREAD (`0x4004667f`) reports available TCP bytes or the next
UDP payload's size. A zero-length datagram remains readable. A short UDP receive
copies the prefix, consumes the datagram and returns WSAEMSGSIZE (`10040`).
MSG_PEEK leaves the datagram queued. Unsupported message flags and options
return their Winsock errors.

Socket failures, including an unpaired WSACleanup, return SOCKET_ERROR/INVALID_SOCKET (`0xffffffff`) and save a
translated WSA error under `ExecCtx::thread_id`. WSAGetLastError does not consume
it; successful calls leave it alone. Pointer errors return WSAEFAULT (`10014`).
Secure-address APIs return their error code directly. The full stdcall frame is
checked before execution, and malformed frames still use the runtime fault path.
Guest sockaddr_in is 16 bytes: little-endian AF_INET (`2`), network-order port,
four IPv4 octets and eight padding bytes. Guest fd_set is a 32-bit count followed
by up to 64 32-bit handles; it is unrelated to the host's bitmap representation.
select ignores nfds, filters the three guest sets and leaves timeval unchanged.
A failed nonblocking TCP connect appears in exceptfds; SO_ERROR returns the
translated error. Host waits release guest memory and HLE state locks. Close and
final transport cleanup wake pending operations and invalidate guest handles.

The title's socket path was checked at `0x00092bf0` (channel creation),
`0x00092ae0` (bind, FIONBIO and broadcast), `0x000b53e0` (protocol selection),
`0x000b4da0`/`0x000b4e70` (options), `0x000b52b0` (select),
`0x000b5060` (recvfrom) and `0x00093730`/`0x000b5110` (sendto).
The four transport channels are:

| Channel | Guest port | Protocol | Default host port |
|---|---:|---|---:|
| Main game data | 1000 | VDP / UDP | 3074 |
| System Link discovery | 1001 | UDP, SO_BROADCAST | 3075 |
| Additional game data | 1005 | VDP / UDP | 3079 |
| Additional game data | 1006 | VDP / UDP | 3080 |

Base+2 (3076) is the HLE QoS responder. Base+4 (3078) is reserved for the co-op
pad-exchange transport (`coop::link::UdpLink`, [live-coop.md](live-coop.md),
section 8), which would bind it on the host itself; nothing binds it today,
since Co-op over Live uses the game's own path, and no guest socket maps to it.

The option wrapper maps the game's options to SO_REUSEADDR (`4`), SO_LINGER
(`0x80`), SO_BROADCAST (`0x20`), SO_SNDBUF (`0x1001`), SO_RCVBUF (`0x1002`) and
Xbox's security option `0x4001`. Linger is a pair of guest 16-bit values,
translated to the host layout. Buffer sizes, broadcast, reuse and TCP_NODELAY affect the host
socket. The security option is accepted on UDP/TCP and retained for getsockopt;
VDP rejects it, matching the library's protocol check at `0x003ce4d7`. The game
sets it before TCP connects at `0x000b5205`. It does not change the HLE's plaintext
transport. Socket type, SO_ERROR, keepalive, timeout and exclusive-address/disable-linger
variants also have explicit handling. Unknown
options return WSAENOPROTOOPT (`10042`). These option and QoS layouts follow the
[public Winsock/XNet API declarations](https://github.com/Team-Resurgent/RXDK-SDK/blob/1dc30011768cf75366428f23380e099afa4dce0d/include/winsockx.h),
checked against the retail call sites.

Discovery sends the title's `broadcast-search` and `broadcast-reply` messages
through the UDP channel at guest port 1001, using INADDR_BROADCAST
(`255.255.255.255`) for both. `0x000b2ea0` sends search message 2 about every
1500 ms through the call at `0x000b2f29`; the reply call at `0x00094097` sends
message 3 to the same broadcast address and port. The message serializers are registered at `0x000ac800`; their payload encoding stays in the
recompiled game. `0x00093730` chooses port 1001 for transport channel 3, then
calls sendto. Receive polling drains the same channel with recvfrom. The HLE
does not decode or alter these title packets. It replaces XNet's secure-address
bookkeeping and console-specific broadcast wire format with host UDP. In LAN
mode, SO_BROADCAST enables a real limited broadcast to base+1. Hosts should use
the same base port and share a broadcast domain.

XNADDR keeps the 36-byte layout described above: both IPv4 fields contain the
host LAN address, wPortOnline contains the actual host base in network order,
and the synthetic Ethernet identity includes the IP and both port bytes so
local instances differ. The default base is 3074; guest ports 1000..1006 map to
base plus their offset from 1000. Other ports pass through. XNetXnAddrToInAddr
requires a registered XNKID and returns the peer's IN_ADDR, while retaining
its full XNADDR, key ID and host base for later routing. Peers are told apart
by IP and base port: a peer on another IP is known by its real IPv4; a peer on
this console's own IP (several games on one Mac, as `h2 coop test` runs them)
or on an IP another peer already holds is known by 169.254.(base >> 8).(base &
255) (`NetworkConfig::shared_ip_in_addr`), so each game is a peer of its own.
XNetInAddrToXnAddr returns that record and optional key ID. recvfrom also
learns a sender's base from the channel's source port before secure
registration, and reports the sender by the same IN_ADDR. Incoming channel
ports are translated back to the guest's virtual port.

XNetCreateKey fills an 8-byte system-link ID and a 16-byte key from host OS
randomness; XNetRandom fills the requested buffer. Register/unregister maintain
a bounded key table. Unregistering a key removes its address associations and
QoS listener. XNetConnect marks a known address connected immediately; status
is IDLE (`0`) before it, CONNECTED (`2`) afterwards and LOST (`3`) for an unknown
address. This is local address bookkeeping, not a handshake or reachability
claim. No encryption or NAT traversal is attempted.

For two instances on one machine, keep `LiveConfig.enabled = true` and use
base ports at least seven apart. Accounts may be absent for System Link.
Boot reads `H2_XNET_ADDR` and `H2_XNET_LOOPBACK_PEERS` only when Live is enabled;
the latter is a comma-separated list of peer base endpoints on this host IPv4.
With that list set, broadcast sends fan out to the listed endpoints, excluding
the sender, rather than emitting a LAN broadcast. Unicast uses learned peer
bases. The host routes these packets locally.

Use the machine's LAN IPv4 in this recipe, illustrated with `192.168.1.50`.
Halo treats `127.0.0.1` as an in-process loopback endpoint at `0x00093218` and
changes channel routing for it; it is suitable for synthetic HLE tests, not
for this title's two-instance setup. In the first instance's environment:

```sh
export H2_XNET_ADDR=192.168.1.50:31000
export H2_XNET_LOOPBACK_PEERS=192.168.1.50:31010
```

In the second instance's environment:

```sh
export H2_XNET_ADDR=192.168.1.50:31010
export H2_XNET_LOOPBACK_PEERS=192.168.1.50:31000
```

Launch each through a host that passes enabled LiveConfig to the boot entry
points above. The environment variables select transport settings; they do not
enable Live by themselves. The CLI client enable flag is outside this change.
Choose System Link, host on one instance and search on the other. No server URL
or account signing key participates in LAN traffic. An embedding host can avoid
environment variables by passing `live_hle::NetworkConfig` to
`Hle::with_network` or `boot::live_glue::install_with_network`.

XNetQosListen retains enabled/disabled state, up to 1020 bytes of listener data
and advertised bandwidth per key. It answers HLE-only UDP queries on base+2
(default 3076). XNetQosLookup copies all guest inputs before issuing bounded
queries, then returns an XNQOS header and 24-byte XNQOSINFO entries in guest
memory. Pending count is zero. Responding peers have COMPLETE, TARGET_CONTACTED
and, when present, DATA_RECEIVED flags. The actual listener data follows the
array with guest pointers; no host pointer escapes. RTT is a fixed 5 ms and
bandwidth defaults to 10 Mbit/s. Disabled listeners report TARGET_DISABLED.
An unreachable peer completes as uncontacted with no invented data, after at
most 50 ms per query. This is a compact HLE side protocol, not an implementation
of the encrypted Xbox QoS protocol. Non-null work events and unknown flags
return WSAEINVAL. Service QoS keeps its sign-in behavior from above.

Peer QoS allocations use a claimed 1 MiB guest arena. QosRelease returns blocks
to a coalescing free list; cleanup releases every outstanding record. Those
pointers have a C allocation lifetime and must not be used after release. Task
handles and service QoS keep the earlier monotonic 64 KiB allocation policy.
Tests cover full 64-peer data records and repeated lookup/release without
growing guest mappings.

Real loopback tests drive these APIs through synthetic guest stacks and the
runtime Runner. They exchange datagrams between two guests, discover and reply
through broadcast fan-out, verify TCP connect/data/EOF/error paths, FIONBIO,
FIONREAD, select, per-thread WSA errors, guest memory bounds, keys/address round
trips, QoS listener data, cleanup and disabled boot wiring. These tests need
permission to bind localhost sockets. No test builds or launches the game.
The remaining stops cover backend operations listed below, including
XNetTsAddrToInAddr and online matchmaking. Actual lobby/gameplay progression has
not been exercised.


## Client HLE: service logon and title resources

The existing boot configuration is sufficient: set `enabled = true`, pass the
CLI's accounts JSON path, and set `server_url` to the service base URL. Account
creation remains the CLI's responsibility. Boot glue installs the same HLE and
returns `Arc<Hle>` to an embedding host; no new boot argument is required.

One `h2-live-http` worker owns a blocking `ureq` client. Guest calls only enqueue
owned host data and poll completions. HTTP, signing, response parsing and waits
never hold guest memory or HLE locks. The bounded queue fails with TASK_BUSY if
full. Cleanup cancels the session without joining the thread; the worker sends
best-effort SignedOut events, including when its command channel disconnects.
Requests time out after five seconds. `Hle::with_server_options` permits shorter
timings for tests; production heartbeats are 30 seconds.

The worker reads `/.well-known/h2live.json`, then checks every selected account
at `/v1/accounts/by-xuid/{xuid}/keys`. Its active public key must match the
accounts file's private JWK. Only after all identities pass does it append
SignedIn events to `/v1/events`. Every request has a fresh, single-use bearer;
events also carry their own signed envelopes. Invalid/unknown/revoked identities
return `XONLINE_E_LOGON_INVALID_USER` (`0x80151006`); transport/protocol failures
return CANNOT_ACCESS_SERVICE (`0x80151001`); HTTP 429/503 returns SERVERS_TOO_BUSY
(`0x80151003`). These are task results, so the title's own error classifier sees
them. Failed logon publishes no guest user array. A failed heartbeat changes the
persistent connection task to CONNECTION_LOST (`0x80151004`).

As required by [live-security.md](live-security.md), remote services require
HTTPS; HTTP is allowed only for loopback. Discovery can select a same-origin API
path, and redirects are disabled. A discovered URL cannot move bearer tokens to
another origin. The per-account `server` field never overrides LiveConfig.

While connected, signed POSTs to `/v1/presence/ping` keep each distinct account
alive. NotificationSetState and PresenceInit/Submit queue PresenceChanged events:
a clear ONLINE bit means away, otherwise online. `Hle::set_game_id(Some(id))` is
the telemetry hook for playing with a server game ID; `None` returns to online.
The raw Xbox session ID is not used as a server game ID. Identical states are
coalesced by the worker. `Hle::server_status` exposes acknowledged ping count and
the last HRESULT without any account key data. The telemetry caller is not wired
in this slice.

Logon does not download title files or build a message catalog. The title's
resource manager requests MOTD and hopper files through title storage after
sign-in; the HTTP worker serves those requests as described below.

MessageEnumerate reports count zero without writing summaries. No title file
is presented as a player message. MessageDetails and MessageDelete return
MESSAGE_INVALID_ID; property/attachment getters have no valid inbox task and
return INVALID_ARGUMENT. Outgoing message capture is separate and unchanged.
GetNotificationEx continues to return FALSE and clear its optional outputs:
the poller at `0x002359ce` would otherwise raise an unopenable "message from
Live service" toast. The voice/text playback screen at `0x002c7410` selects
messages whose sender matches a player; its attachment and language properties
are not the title-resource mechanism.

Synthetic guest tests use the real embedded service to prove signed logon,
identity failures, unavailable/delayed servers, user changes, presence/pings,
connection loss and sign-out. Storage tests publish files and drive the actual
boundary calls, check returned buffers and lengths, validate their headers,
then inflate the MOTD/hopper payloads to exactly `0x25224`/`0x162cc0`.
A file-backed embedded-server test publishes a MOTD through the web-admin
endpoint, reads it through storage and repeats after a restart. No test runs
the title.

## Client HLE: menu polling and remaining stops

The polling audit starts from the game wrappers in
[live-boundary.md](live-boundary.md), and the per-frame message screen noted in
[live-motd.md](live-motd.md). The owner observed a real signed-in
menu stop at `0x003a0827`; the no-invite path now matches the public XDK contract
and the retail implementation. Other menu cases are statically audited and tested
through synthetic guest stacks; browsing every screen has not been run here.

| Game wrapper / call site | Menu-facing result |
|---|---|
| `0x0006d210` accepted invite poll | GetAcceptedGameInvite consumes a queued accepted invite into the 156-byte output; with none, it clears the output and returns E_FAIL (`0x80004005`). The check at `0x0006d218` treats every nonnegative HRESULT as an invite, so S_FALSE would be wrong. |
| `0x0008cc40`, `0x0008cd27`, `0x0008cddb`, disposer `0x0006b6a5` | FriendsStartup completes then idles, reporting RESULTS_AVAIL (`0x001500f1`) when its cached server snapshot changes. GetLatest returns 86-byte friend/request/invite records. EnumerateFinish lets the enumeration task finish before TaskClose. |
| `0x0006d080` / `0x0006d0d3` | GetNotificationEx emits one packed type-3 notification per received game invite, then BOOL FALSE with cleared info/state. Title files are never notifications. No HRESULT is returned in this BOOL slot. |
| `0x00058ab8`, `0x00058bd1` | MutelistStartup/Get complete; Get writes count 0. |
| `0x0008d4d0` | OfferingIsNewContentAvailable reports NO_NEW_CONTENT. |
| `0x0008c320` through `0x0008c59b` | PresenceInit/Add/Submit/Clear retain the bounded watch groups; GetLatest writes 61-byte records carrying the watched XUIDs and overlays cached friend/invite presence when available. |
| `0x0006b59d`, `0x0008d1da`, title checks `0x0008c28e` / `0x0008d27a` | FriendsGetTitleName writes UTF-16 “Halo 2” for this title and an empty terminated name for uncached titles. SameTitle/SamePublisher return BOOL comparisons. |
| `0x0006b565` | GetNatType returns the retail initial/unknown value 0. No NAT probing or claim of open NAT is made. |
| `0x000abc70` through `0x000ac2f2` | Team and member enumerations return snapshots from the local store described below. Unknown teams fail with TEAM_NOT_FOUND (`0x8015210b`); a missing item in a successful snapshot returns E_FAIL, as retail does. |
| `0x0008d3d0`, `0x000b388a` | FeedbackSend returns a failed task, SERVICE_TEMPORARILY_UNAVAILABLE (`0x80151102`). QuerySearch/GetResults returns QUERY_DATASET_NOT_FOUND (`0x80156109`). These optional views have no backend data yet. No feedback is claimed to be submitted. |
| `0x00092220` through `0x00092450`, manager `0x00074aa0` | StatWrite/GetResult accepts an owned local capture and completes successfully without attachments. StatRead/GetResult completes with missing (`NONE`) stats and zero values in the caller's arrays. See the local stats section below. |
| `0x0022387b` | SaveLogonState writes the public 516-byte launch-state layout with users and service IDs; it contains no signing key. |
| `0x0008e750`, manager `0x002c7410` | Message polling exposes pending game invitations. Title files use storage and never enter the player inbox. |

These 10 entries still stop by API name because their backend behavior is not
implemented. Their menu/game fallbacks are a separate follow-up slice; the
storage entries below no longer stop. Prefixes in the table are part of each
listed API name.

| Prefix | Remaining suffixes |
|---|---|
| `XOnline` | `TitleUpdate` |
| `XOnlineFriends` | `Remove`, `AnswerRequest` |
| `XOnlineMutelist` | `Add`, `Remove` |
| `XOnlineArbitration` | `CreateRoundID`, `Register`, `ExtendRound`, `Report` |
| `XNet` | `TsAddrToInAddr` |

Server-backed sessions, invitation delivery, clans and cached friend presence are
implemented in the Client HLE sections below. Feedback storage, friend mutations
outside the existing capture path, ranked arbitration and NAT traversal remain
unfinished. Synthetic tests prove invitation discovery through bidirectional
host UDP traffic; actual menu transitions and a full retail match are unconfirmed.

## Client HLE: title storage and profile fallback

The signed-in resource manager `0x0008e240`, called each network update by
`0x0008df50`, downloads the MOTD and hopper. The verified chain is
`0x0008e3f5` → `0x000b4450` → `0x000b4120` →
StorageCreateServerPath, then `0x000b4511` → StorageDownloadToMemory.
There is no StorageEnumerate call in the boundary inventory.

At sign-in, `0x0006bff0` arms the MOTD channel `0x00479780` and hopper
channel `0x0047d8f4` in slot table `0x004d8c28`. Requests use facility 4
(per-title), user/team XUID zero, controller index zero, flags zero and no work
event. The filename is the UTF-16 relative path `9809/<filename>`; console
check `0x0006bfa0` selects `test/<filename>` when byte `0x0050944e` is set.

| Formatted guest path | Caller buffer capacity | Server file under `/v1/titles/4D530064/files/` |
|---|---:|---|
| `//title.4d530064/t:4d530064/9809/message_of_the_day.dat` | `0x8000` | `message_of_the_day.dat` |
| `//title.4d530064/t:4d530064/9809/multiplayer_hopper.dat` | `0x20000` | `multiplayer_hopper.dat` |

The same two paths with `test/` in place of `9809/` are accepted. Only
these four complete paths, with facility 4, reach Service::file on the HTTP
worker. The guest call returns a running task; Continue polls the owned reply
and copies a complete file into checked guest memory. Oversize returns
XONLINE_E_OVERFLOW (`0x80150001`) without a partial copy. There is no
`0x499a` attachment limit on these requests: that cap belongs to the
unrelated playback screen.

StorageDownloadToMemoryGetResults has six arguments: task, optional buffer
pointer output, received count, total count, owner PUID and creation FILETIME.
On success it returns S_OK, the original caller buffer, and received == total.
Shared title owner is zero; creation time is zero because the endpoint does not
supply one. While pending it returns TASK_BUSY without publishing outputs.
GetProgress reports 100 percent and the completed byte counts after success.
Close/cleanup invalidate tasks; the worker never accesses guest pointers.
HTTP 404 becomes STORAGE_FILE_NOT_FOUND (`0x8015c004`); transport failures
remain service errors. Every fresh explicit request fetches current bytes.

The game completion chain is `0x0006c4a0` → `0x0008e5e0` → GetResults.
It requires equal received/total counts within the channel cap, then validator
`0x00081230` checks the file header. Completion `0x0008e0f0` →
`0x00081050` sets flags |= 2 and invokes `0x0008c790` (MOTD) or
`0x00194260` (hopper). The header and payload formats are in live-motd.md.

Channels arm on the signed-in transition and user-change completion
(`0x001513f1`), skipping channels that already contain data. Sign-out clears
them. There is no mid-session re-arm for a new publication or inbox event:
publish before sign-in, or sign out and back in to request the new files.
Errors, including FILE_NOT_FOUND and `0x8015c007`, use the existing fallback
without retry. Only received != total re-queues the transfer.

StorageCreateServerPath implements the retail formatter at `0x003c79f2`, without
any network request. It needs XOnlineStartup but not a signed-in user. The three
supported facilities produce these UTF-16 paths, where `title` is lowercase
8-digit hex (`4d530064`) and user/team IDs are lowercase 16-digit hex:

| Facility | Path |
|---|---|
| 3: teams | `//tteam.{title}/u:{team}/{title}/{user}/{filename}` |
| 4: per-title | `//title.{title}/t:{title}/{filename}` |
| 5: per-user/title | `//tuser.{title}/u:{user}/{title}/{filename}` |

The capacity and returned required size count UTF-16 code units including the
terminator, not bytes. NULL output supports a size query. Insufficient capacity
returns `0x8007007a` and updates the required count. If the buffer can hold the
prefix and terminator, retail writes that prefix before checking whether the
filename fits; the HLE preserves this behavior. Other facilities return
STORAGE_INVALID_FACILITY (`0x8015c009`) without changing outputs. The formatter
does not enforce the public 256-character path limit.

Other paths/facilities and package StorageDownload return tasks completing with
STORAGE_FILE_NOT_FOUND; receive/result buffers are untouched. This includes
profile storage, which remains future work. With no server configured even
the known title files are unavailable. UploadByServerPath and UploadFromMemory
complete with STORAGE_ACCESS_DENIED (`0x8015c002`); no upload is claimed to be
saved. Failed GetProgress reports zero bytes/percent with the task's error.
Guest paths never reach the host filesystem or become arbitrary HTTP paths.

The title classifies these errors: `0x0006c4a0` maps FILE_NOT_FOUND to
resource result 5 and ACCESS_DENIED to result 3. Tests cover both path prefixes,
both channel capacities, publication after logon followed by an explicit
request, missing/unknown paths, exact-capacity and oversized replies, packed
optional results, progress and task cleanup. The path formatter tests also
cover split IDs, UTF-16 size queries, Unicode and prefix-only overflow.

## Client HLE: local stats

The periodic stats manager `0x00074aa0`, called by the network update at
`0x0008df50`, reaches four public entries through wrappers `0x00092220`,
`0x00092330`, `0x000923c0` and `0x00092450`: StatRead, StatReadGetResult,
StatWrite and StatWriteGetResult. No StatReset or leader-enumeration call was
found in this chain or the verified boundary table; no new addresses are guessed.

StatWrite (`0x003a08c0`) validates the guest batch and copies its nested values
before returning a task. A spec is 24 bytes: a 12-byte XUID (64-bit ID plus flags),
leaderboard ID, stat count and guest array pointer. Each stat is 16 bytes: a
16-bit ID, two padding bytes, a 32-bit type and an eight-byte union. NONE, signed
LONG, signed LONGLONG, finite DOUBLE and terminated UTF-16 strings are supported.
The write batch allows at most 20 specs and 64 stats per spec; strings are bounded
to 32 UTF-16 units. Invalid types, malformed strings and oversized counts return
stat errors before accepting anything. All guest buffers are checked, including
nested arrays and strings; an invalid pointer follows the runtime's memory-fault
path without accepting a partial write or publishing a task handle.

TaskContinue returns XONLINETASK_S_SUCCESS (`0x001500f0`). Ordinary S_OK from the
starter alone would not complete a task. StatWriteGetResult (`0x003ab6a6`) returns
S_OK and clears any supplied server-file handle, attachment-reference pointer and
reference count. No attachment upload is needed. In the retail success branch at
`0x000754e8`, the title then calls `0x00080d70` to acknowledge its original stat
arrays and clear uploaded dirty state. Those input arrays remain unchanged by HLE.

Accepted writes remain in the Hle as owned StatWrite records, including sequence,
Unix capture time, user ID/flags, leaderboard, IDs and typed values. Closing tasks
or signing out does not discard them. `Hle::stat_writes()` returns a snapshot;
`Hle::drain_stat_writes()` transfers them to a future server-event forwarder.
`H2_BOOT_VERBOSE=1` emits the parsed records as debug diagnostics; account keys
are never part of the capture. The local journal is bounded to 256 batches and an
8 MiB value budget. When full, a new write fails with OUT_OF_MEMORY without
silently evicting an accepted batch. Draining frees the budget and preserves the
monotonic sequence. Nothing is sent to the server or persisted to disk yet.

StatRead (`0x003a08cb`) owns its query IDs, supports the manager's 32-spec batch
(up to 64 specs), and completes successfully. StatReadGetResult (`0x003a08d6`)
checks the caller-initialized specs/IDs before writing type NONE and a zero union
in each existing stat. Headers, IDs, padding and the unused extra buffer are
preserved. The consumer at `0x000751a9` treats NONE as zero. Reads currently do not
serve the local write journal as canonical leaderboard data.

Synthetic guest-stack tests exercise each entry, all five value types, copied
ownership, optional attachment outputs, successful task pumping/closing,
transactional rejection, pointer faults, the 32-spec read batch and capture
retention/draining. No test runs the title.

## Client HLE: Live not set up

`crates/boot/src/live_notice.rs` puts our text on the game's own "cannot
connect to Xbox Live" dialog, the one that offers the troubleshooter (UI
error `0x23`). The game draws it in its dialog and font; no tag or localized
string is edited. `boot::run_hosted_with_live` installs it after the Live
boundary and telemetry. `H2_NO_LIVE_NOTICE=1` leaves it out.

How the dialog is reached (static reading of the retail image, not a run):

- **With Live off** (`h2 play` without a Live flag). Nothing is registered, so the real XNet library runs.
  The default console reports no cable (`ConsoleConfig::phy_link = 0`), so
  `0x0008d690` never starts transport and `0x004d8b19` stays clear. Picking
  Xbox Live on the main menu runs the row handler `0x002308e0`: not signed
  in (`0x0019028d`) and transport unusable (`0x0008d7c0` at `0x00230928`), so
  it calls `0x0019b5af` with error `0x23` and the network-settings action
  `0x00236917` at `0x002309e1`. The dialog appears at once; there is no
  sign-in attempt to hang on, so no extra "not configured" mode is needed.
- **With Live on and a server.** If logon ends with CANNOT_ACCESS_SERVICE
  (`0x80151001`, the HLE's result for transport or protocol failures), the
  classifier gives game result 4 and `0x001486b8` shows the same error
  `0x23` (`0x001486fa`).

Dialog widgets (vtables `0x00454560`, `0x004545d0`, `0x00454640`) keep the
error ID at `+0x610`, then four 256-unit UTF-16 buffers: title `+0x614`,
message `+0x814`, first button `+0xa14`, second button `+0xc14`.
`0x0019b72a` (EAX = error ID, EBX = dialog, one stack word, `ret 4`) looks
the error up in the shared UI globals (`0x0023661f`) and resolves its
strings into those buffers; it is called from both dialog constructors
(`0x0019b56a`, `0x0019b5f3`) and from `0x001911ef`. The update method
`0x0019b682`, shared by all three vtables, copies the title and message
buffers into the text widgets every frame. The hook wraps `0x0019b72a`
(shadow entry `0xa0ff6000`): after the original fill, if `+0x610` is `0x23`,
it writes our message into `+0x814`, at most 255 units and a zero, so the
first button's buffer is never touched. Title and buttons keep the game's
text. Finding the dialog this way needs no asset names, string IDs or
string-table offsets.

The text depends on the boot's `LiveConfig`:

| Live | Server | Message |
|---|---|---|
| off | (any) | "Xbox Live isn't set up in h2recomp.", then how to start with `--live` or `--live-local` |
| on | URL, no account | "No Live account on this machine yet.", `h2 live account create <tag>`, or recover at `<URL>/dash/signup/recover` |
| on | URL | "Can't reach the h2 live server at" and the URL, then the same advice |
| on | none | stock text (local sign-in cannot fail to connect) |

```text
Xbox Live isn't set up in h2recomp.
Quit and start again with one of:
h2 play --live  (live.forbes.red)
h2 play --live-local  (this machine)
Accounts: h2 live account create <tag>
```

The message box on the OK/cancel error dialog is about 244 by 91 pixels,
five lines of the body font, so each line is kept under about 240 pixels.
Lines break on `\r\n`, as the game's own strings do. The URL is shortened
to 40 characters, and `|` (which starts a markup escape in the text scanner)
is replaced. The message does not mention `H2_LIVE_SERVER`, which `h2 play --live` and
the `h2 live` commands read.

The first button still offers the troubleshooter. It launches the
dashboard's network settings through `XLaunchNewImageA`, which ends the run
here. Other Live errors (servers busy `0x25`, invalid user `0x9a`,
connection lost `0x38`) keep the game's text.

Tests: `crates/boot/src/live_notice.rs` checks the text, the URL line and
the buffer bounds on synthetic guest memory; `crates/boot/tests/live_notice.rs`
runs the hook through the real Runner with the fixture module's leaf
standing in for the fill, and checks that only error `0x23` changes, that
install refuses to wrap an existing override and skips a missing original,
and that local-only Live installs nothing. They need no XBE or game build.

"No account" means the boot's accounts file (or its absence) holds no entry
with a private-JWK key (`live_hle::account_setup::load`); an unreadable file
keeps the "can't reach" text and is left to sign-in to report. Whether the
retail game reaches error `0x23` at all with zero accounts listed is not yet
checked in a run.

### In-game account setup

Making or recovering a Live account from inside the game. Built: the host
calls, and the sign-in
prompt below, which recovers an account made elsewhere and can create one
for an open server. Not built: the Create / Recover menu
(`boot::account_screen::AccountScreen`) as a screen; only its layout helpers
are used, by the prompt.

**Host side, `live_hle::account_setup`** (no guest memory; blocking HTTP, so
run each call on a `Job` thread and poll it from the frame loop):

- `create(server_url, gamertag, invite)` makes a key, signs an
  `AccountRequest` for the server id from discovery, and `POST`s
  `/v1/accounts`. It returns the `LocalEntry` and the `RecoveryInfo` to show
  once. The invite goes as typed (trimmed; empty means none).
- `recover(server_url, gamertag, code)` makes a key and `POST`s a signed
  `RecoverRequest` with the code to `/v1/accounts/{gamertag}/recover`.
- `recovery_code(server_url, entry)` and `reset_recovery` read or replace
  the secret with a self-issued `recovery.read` / `recovery.write` token.
- `needs_invite(server_url)` reads `invite_only` from `GET /v1/server`.
  It returns `None` when a private server won't say; a "needs an invite"
  refusal then turns the invite field on.
- `save(path, entry)` adds the entry to an accounts file in the CLI's shape
  (`{xuid, gamertag, id, server, server_id, kid, key}`). It replaces an entry
  with the same xuid, or one with the same gamertag on the same server. It
  writes other entries back with every field they had, and writes mode 0600
  through a temporary file and a rename. `shared_accounts_path` maps the
  per-session `play-accounts.json` to the `accounts.json` beside it.
- Each call refuses plain HTTP to anything but loopback (`Insecure`) before
  it makes a request. Its `SetupError` is one short line for the player:
  gamertag taken, needs an invite, invite not valid, no such gamertag, no
  recovery set up, wrong code, locked out for N minutes (from the server's
  429), busy, can't reach the server, or couldn't save. None of these
  messages includes a key, a reply body or the code that was typed.
- `Hle::reload_accounts()` reads the accounts file again, so a saved entry
  is listed by `XOnlineGetUsers` and signs in without a restart. Accounts
  that are signed in stay signed in, and if the file is broken nothing
  changes.

**Screen logic, `boot::account_screen::AccountScreen`** (pure Rust): a menu
with Create, Recover and Cancel. Create asks for a gamertag, then an invite
if the server needs one, then works, then shows "Write this down" (the
current code with its seconds left, the authenticator key in groups of
four, and how to recover). Recover asks for a gamertag, then the code, then
works. A picks, B goes back, and B never skips the secret. The glue opens
the keyboard for each `Effect::Keyboard` and answers with `typed`. It runs
each `Effect::Start` as a `Job`, saves the entry, and answers with
`finished`. On `Effect::Close { signed_up: true }` it calls
`reload_accounts`. Page text is at most five lines of 40 characters, broken
with `\r\n` and never holding `|`, as for the notice.

**Pending (needs facts from a run):** an entry point for the Create /
Recover menu, and the drawing. The sign-in prompt (below) is the entry for
an account that exists on the server. Either of these would do for the menu:

1. A list screen like Video and Skulls (`list_class`), opened from a row or
   from the error `0x23` dialog's button. This needs the retail handler for
   that button, or a menu to put the row on.
2. The keyboard as text entry: open the retail keyboard screen (factory
   `0x0023784f`, screen id `0xa7`) on demand with our buffer, and read the
   text back when it is accepted. `keyboard_glue` only types into a keyboard
   the game has already opened.

The game also has to choose which file to save into. It only knows
`LiveConfig.accounts_path`, which under `h2 play` with Live on is the session's
`play-accounts.json`, rewritten on every start. The in-game sign-in (below)
saves to that file and to `shared_accounts_path(..)`.


Tests: `crates/live-hle/tests/account_setup.rs` covers create, recover with
the code from the returned secret, a wrong code, a used code, an unknown
gamertag, lockout, reset, an unreachable server, saving with other entries
kept at 0600, and a saved account signing in after `reload_accounts`. All of
them run against the in-process memory server.
`crates/live-hle/src/account_setup.rs` maps refusals to errors.
`crates/boot/src/account_screen.rs` covers every transition and checks that
every page fits.

### In-game sign-in

When the player signs in to Live in the game with an identity this machine
has no key for on the server (made on the web, another device or another
server), the game asks for the 6-digit recovery code itself.

**The account list.** An accounts-file entry with `"needs_code": true` is
listed by `XOnlineGetUsers` like any other (the same 112-byte record), so
the player picks it in Halo 2's own sign-in. Its key, if any, is never
used, and it is dropped when a ready account of the same gamertag is in
the file (`live_hle::config`). `h2 play --live` writes such an entry into
`play-accounts.json` when the gamertag is on the server already
(`play_live::write_waiting`), with the server's xuid for it.

**The logon** (`live_hle::signin`). `XOnlineLogon` with a `needs_code` user
(one per logon, and only with a server; otherwise `INVALID_USER`) returns a
task and makes no request: `XOnlineTaskContinue` returns `0` (pending), so
the game stays on its signing-in step. The prompt answers through
`Hle::signin_submit(code)`, `signin_create()`, `signin_cancel()` and
`signin_dismiss()`, and reads `Hle::signin()`:

| Phase | Shows | A | B |
|---|---|---|---|
| Code | the code being typed, the last error | check it | cancel |
| Working | "Checking with HOST..." | | cancel |
| Create | the gamertag isn't on the server | make it there (open servers) | cancel |
| Created | the new account's recovery secret, once | close | close |
| Done | signed in | close | close |

A code that is not six digits is refused without a request. Each check runs
`account_setup::recover` on its own thread; a wrong code says how many
tries are left (the server's count), a lockout how long to wait, an
unreachable server says so, and the code is asked again. No such gamertag
offers Create, which runs `account_setup::create`; an invite-only server
says to make it on `/dash` or pass `h2 play --invite CODE` (there is no
in-game invite entry). On success the entry is saved to the session's file
and the shared `accounts.json`, the accounts are read again, the user's
record is replaced by the saved account's (the controller slot word kept),
and the service logon starts as for any account, ending in `CONNECTED`.
Cancel fails the logon with `INVALID_USER` (the user's HRESULT too).

**The prompt** (`crates/boot/src/signin_screen.rs`) is our own list class
(`list_class`) with one row and a description, on the Controls layout. The
d-pad types the code (up and down change the digit, left and right move;
direction events, kinds 1 to 4, from the d-pad or the left stick, as
`skulls-arrows` read the controller update), A on the row or Start checks
it, B backs out and closes. The text is served from guest buffers under our
string ids; each change goes into the other buffer of a pair, and the row
text (`Rows::live_text`) writes it and sets the description again. The
logon opens it: a wrapper around the HLE's `XOnlineTaskContinue`
(`0x003a0620`) asks `Hle::take_signin_open()` after each call, and the
first time a sign-in waits it writes the title and first page and calls
`0x149f1e(1 << controller, loader)`, as the Skulls page opens. A finished
page closes with `0x14800c` (the screen's `+0x78` slot and `+0x74` window).

Static reading (retail 5849): the main menu's Xbox Live row (`0x002308e0`),
not signed in and with transport, animates the menu out (`0x0022eeee`),
reads the accounts (`0x0018fc44`) and opens the account picker with loader
`0x0024b4a9`; the logon is the picker's. Not established without a run:
that the picker polls `XOnlineTaskContinue` each frame while signing in,
that a screen opened from inside that call draws on top and takes input,
that our screen's event hook sees the direction kinds before the list does, that a
list's row text runs every frame (so a finished check shows without a
press), and what the game shows for `INVALID_USER` after a cancel.

Switches: `H2_SIGNIN_PROMPT=0` leaves the screen out (a waiting sign-in is
cancelled at once, with a line pointing at `--signin-terminal`);
`H2_SIGNIN_TEXT_RECT=top,left,bottom,right` moves the description;
`features.signin-code=off` turns the class off (the sign-in is cancelled).
Every step logs a `live:` or `signin prompt:` line on stderr.

The app builds (iOS, tvOS) run the same HLE and boot glue, so an entry
marked `needs_code` in `live/accounts.json` asks in game the same way, and
they offer a new account in game (next).

#### A new account in game

On the app builds there is no terminal, so the account list itself offers
the way in. `live_hle::platform::config` sets
`LiveConfig::offer_new_account`, which does two things
(`live_hle::config`):

- A missing `live/accounts.json` is no accounts rather than an error (the
  first start), and
- the list gets one more identity, "New Account" (`NEW_ACCOUNT_TAG`, xuid
  `NEW_ACCOUNT_XUID`, `0x0009000100000000`, outside the stand-ins' range),
  marked as waiting.

Signing in with it starts a `Signin` that asks for a gamertag first
(`Signin::adding`). The prompt's phases before the code page:

| Phase | Shows | A | B |
|---|---|---|---|
| Gamertag | "A gamertag on HOST", the last error | open the keyboard | cancel |
| Invite | "HOST needs an invite to make TAG", the last error | open the keyboard | cancel |

The typed text goes to `Hle::signin_typed(Some(text))`, a backed-out
keyboard to `signin_typed(None)` (from the invite: back to the gamertag).
A gamertag that isn't one is refused without a request. Otherwise
`account_setup::create` runs on its own thread, with the invite when one
was typed:

- **Free:** the account is made and saved to the accounts files, the
  recovery secret is shown once (Created), and the logon goes on as the new
  account, as above.
- **Taken:** the gamertag is saved as waiting for its code
  (`account_setup::save_waiting`: `{xuid: stand_in_xuid(tag), gamertag,
  server, needs_code: true}`, added once, other entries kept), and the page
  becomes the code page for that gamertag. The right code signs in as the
  account itself. Backing out leaves the waiting entry, which the list
  shows from then on.
- **The server wants an invite:** the first refusal asks for one (Invite,
  no error), and a bad one says so and asks again.
- **Anything else** (unreachable, busy, server error) is said on the
  gamertag page.

**The keyboard** (`crates/boot/src/signin_keyboard.rs`). A on either page
opens the game's own on-screen keyboard. On the app builds the host's
system keyboard types into it (`keyboard_glue`). It is opened with the
retail opener `0x00238c21`: stdcall `(context, buffer, capacity)`, `ret
0xc`, the player in CL. The opener creates the screen through its loader
`0x0023784f` and edits our UTF-16 buffer (a page of ours) in place.
Static reading of the keyboard (retail 5849):

- `0x002381de` backs the text up at `+0x618` and points `+0x818` at the
  buffer.
- B copies the backup back (`0x00237fcc`).
- Done checks the text against the context's flags in the table at
  `0x0044a9f8` and then runs the context's case of the jump table at
  `0x00238468`.

Contexts 8 to 11 and 17 to 20 only close the screen on Done. Contexts 16 to
19 swap in the keyboard's own buffers when it is laid out (`0x00237a74`).
So:

- The gamertag uses context **8**: Done needs a letter or digit, and the
  capacity is cut to 17 units.
- The invite uses context **20**: no check, up to 150 units, and no title
  of its own (string id 0).

While one of ours is open, the prompt's string source answers that
context's title ids (`0x0d00044f` and `0x12000450`, or 0) with our words.
The buffer starts empty, so empty text when the keyboard closes means B.
The prompt's row text polls for that close: once the shared channel's
screen (`0x0054dba8 + 8`) is no longer the keyboard on our buffer, the text
is read and passed on. A keyboard that never shows within 120 polls counts
as backed out.

Not established without a run:

- what contexts 8 and 20 show around our title text;
- that opening the keyboard from our screen's row press draws it on top
  and takes input;
- that row text keeps being asked while the keyboard covers the prompt
  (the close is still seen when the prompt is back on top);
- that the host keyboard bridge finds it (it accepts contexts up to 20);
- how the game takes the signed-in user's xuid and gamertag changing from
  "New Account" to the new account's under it.

**Checklist for a run on tvOS or iOS** (fresh install, or
`live/accounts.json` deleted):

1. Xbox Live on the main menu lists "New Account". Pick it. Stderr:
   `live: New Account: asking in game for a gamertag on <url>` and
   `signin prompt: opening for controller 0`.
2. The prompt reads "New Xbox Live account". Press A: `signin prompt:
   opening the keyboard for the Gamertag (context 8, controller 0)`; the
   game's keyboard opens, titled "Your gamertag", and the system keyboard
   comes up with it.
3. Type a free gamertag, Done: `the Gamertag keyboard closed with text`,
   then `live: making <tag>`; the recovery secret page; A finishes signing
   in.
4. Again with a gamertag made elsewhere: the code page for it; enter the
   6-digit code from its Account page.
5. Against an invite-only server: the invite page; A opens the keyboard,
   titled "Invite code".
6. B on the keyboard: the page is asked again; B on the prompt cancels.

**Checklist for a game run** (`h2 play --live-server URL` with an account
made on that server's web dashboard and none on this machine):

1. `h2 play` prints `... needs a code from Account on URL/dash; the game
   asks for it when you sign in to Xbox Live` and starts without a terminal
   prompt.
2. Xbox Live on the main menu lists the gamertag; pick it. Stderr:
   `live: <tag> needs a code to sign in on <url>; asking in game`, then
   `signin prompt: opening for controller 0`.
3. The prompt is drawn over the signing-in step, titled "Xbox Live
   sign-in", the row reading "Sign in as <tag>", the description the code
   page. If the description is off-screen, move it with
   `H2_SIGNIN_TEXT_RECT`.
4. D-pad: each press logs `signin prompt: event K, button N (Some(Up))`
   and so on (K 1 to 4), and the digits change.

5. A wrong code: "Wrong code. 4 tries left." appears without another press.
6. The right code: "Signed in. A: done"; A closes the prompt and the game
   finishes signing in; `play-accounts.json` and `accounts.json` hold the
   key. The next `h2 play --live` signs straight in.
7. Again, with B on the prompt: note the game's sign-in error and that it
   returns to the menu.

Tests: `crates/live-hle/tests/signin_code.rs` (the logon pending through a
wrong code, the right code against the embedded server's real TOTP, both
files saved, the countdown to a lockout, cancel, create, unreachable, no
server; New Account making a free gamertag, a taken one kept waiting and
signed in with its code, backing out of one, an invite-only server);
`crates/live-hle/src/config.rs` (the list, the New Account row);
`crates/boot/tests/new_account_prompt.rs` (New Account opening the prompt
through the runner; A opening the retail keyboard on our buffer, with our
title); `crates/boot/src/signin_keyboard.rs` (reading the text back);
`crates/boot/src/signin_screen.rs` (the d-pad entry, every page fitting
five lines of 40); `crates/boot/src/list_class.rs` (a class's own buttons,
live text); `crates/cli/src/play_live.rs` (the hand-off).

## Client HLE: outgoing messages

The observed stop `0x003a9d8e` is reached from `0x0008f070`, through
`0x0008ef90` and the UI send action `0x002b7e69`. This is the by-name social
message composer. Its internal type 1 builds a Live type-2 friend request; its
internal type 2 builds a Live type-4 clan recruitment message, with the team
context flag. Both can carry text/language and voice data/codec/duration. The
backtrace alone does not include the type field, so it does not distinguish
these two actions. This path does not send feedback or the MOTD/hopper files.
The shared composer also builds type-3 game invitations and title-custom mail.

MessageCreate (`0x003a9d8e`) now returns an opaque guest handle. It retains the
message type, 64-bit context, flags and expiry. Property/count sizes are allocation
hints rather than hard limits on later setters. MessageSetProperty (`0x003a9dc3`)
validates and owns typed values: NULL, 8/16/32/64-bit integers, UTF-16 strings,
FILETIME, binary, attachment, BOOL and string ID. Replacing a property's identifier
replaces its stored value. Inline details are limited to 4096 bytes; voice/blob
attachments are copied into host-owned memory, bounded to 1 MiB each and 16 per
message. The non-exportable attachment flag is retained. Directory attachments
are not read from the host filesystem and return INVALID_ARGUMENT. Guest pointer
faults and rejected values leave the existing builder unchanged.

MessageSend (`0x003ab060`) snapshots the message and its array of 12-byte XUID
recipients. The composer uses typed send APIs too: FriendsRequestEx and
FriendsRequestByNameEx (`0x003a835d`/`0x003a838a`), TeamMemberRecruit and
TeamMemberRecruitByName (`0x003a8399`/`0x003a83d4`), and GameInviteSend
(`0x003a8321`). These capture the same owned message with the recipient XUID or
bounded ANSI gamertag, plus team/member properties or session ID as appropriate.
A null optional message on a typed send becomes an empty message of that type.
The sender must be signed in. With a server configured, GameInviteSend posts
signed mailbox invitations and its task waits for acknowledgment. Other generic
message/friend-request captures remain local; clan recruitment has its separate
server-backed Teams path.

Every accepted local send returns a task whose Continue result is
XONLINETASK_S_SUCCESS (`0x001500f0`), rather than the running value S_OK. The game
pumps these tasks through `0x0006c450` and `0x0006c670`; the completion callback
`0x0008f960` can finish and dispose its message. No SendGetResults or
SendGetProgress entry is used in this verified boundary. MessageDestroy
(`0x003a9db8`) frees the builder; TaskClose frees only the task. Each accepted
send is captured once, independently of later edits, destruction or task pumps.

`Hle::message_sends()` snapshots the accepted sends and
`Hle::drain_message_sends()` transfers them to a future server-event forwarder.
Records retain sequence/time, sender, send kind, recipients and built properties;
`H2_BOOT_VERBOSE=1` prints these as debug diagnostics. They contain no account
signing key or guest pointer. Captures survive cleanup/sign-out, while those
operations invalidate live builders and tasks. The local journal holds up to
256 sends and an 8 MiB value budget; builders have a separate 8 MiB budget and
64-handle limit. Exhaustion returns OUT_OF_MEMORY without discarding earlier
captures. Destroying builders and draining captures free the respective budgets.
Captures are in memory only. Local capture success does not establish delivery
or friendship. A server-backed game-invite task instead reports the real mailbox
write result; invitation acceptance is a separate operation.

Synthetic guest-stack tests cover all nine entries, the by-name composer flow,
all property types, packed XUID/member layouts, successful tasks, copied
ownership, validation/fault atomicity, destruction, cleanup and bounded journals.
They use no XBE and do not run the title.

## Client HLE: game invitations and joining

GameInviteSend now posts `GameInviteSent` to each recipient's server mailbox.
Its task stays running until the HTTP worker finishes, then reports success or
the server/transport error. The original owned message capture remains available;
text and voice attachments in that capture are not delivered in this slice.
FriendsGameInvite is the older no-task API: it copies the supplied 86-byte friend
array, queues the same writes and returns once queued. Backend failures on these
no-task calls are visible through `Hle::server_status()`, without blocking a frame.
Without a server URL, typed MessageSend captures retain their local behavior;
the new friend-invite actions return NO_SESSION.

FriendsStartup polling and the menu's GetLatest/notification/message reads queue
cached refreshes, at most once per 250 ms per controller after a reply. The worker
fetches actual friends/requests, open sessions and the caller's mailbox. No HTTP
runs under a guest-memory or HLE lock. FriendsGetLatest returns a DWORD count,
not an HRESULT. Its records include online/playing/joinable state and received
invite flags. PresenceGetLatest overlays that cached view on watched users.
These reads do not implement sending or accepting friend requests: establish the
friendship through the existing CLI or web UI first.

MessageEnumerate presents each pending invite as a type-3, 64-byte summary from
the real inviter XUID/gamertag. GetNotificationEx returns one type-3 notification
per invite; repeated polls return FALSE with cleared output. Details owns a
snapshot task, and DetailsGetResultsProperty returns the required session-ID
property `0x0581`. Other properties report PROPERTY_NOT_FOUND and no attachment
is invented. MessageDelete queues a remove (or block-sender) answer and hides
the item; an already handled known ID is acknowledged. Title resources remain
on the storage path and never become inbox messages or notifications.

FriendsAnswerGameInvite and GameInviteAnswer queue an authenticated recipient
answer, using the actual cached invite ID. YES/accept publishes a pending join
only after server acknowledgment; decline/remove never do. The newer peer
answer enum's NEVER value blocks later game invitations from that sender.
GameInviteAnswer owns a task; the older friend answer queues its work without a
task. FriendsJoinGame resolves an existing friendship and the friend's open
session before publishing the same pending join, even without an invitation.
This keeps a same-title join inside the current process instead of using the
retail cross-title dashboard launch path.

GetAcceptedGameInvite returns S_OK once for an accepted join, then E_FAIL with
a cleared output. Its 156-byte record is statically verified against
`0x003b49ce` and the consumer `0x0022f3b5`: packed InvitingFriend at +0 (86
bytes, padded to 88), accepted XUID/flags at +88, acceptance FILETIME at +100,
and four 12-byte logged-on XUID/flag records at +108. The friend's session ID is
at +40 and title ID at +48. The record expires after 15 minutes. Pointer faults
cannot consume it. Sign-out clears tasks, caches and pending joins. Answers for
a controller whose account changed cannot create a join for the new account.

The game can then use its existing FindFromID, search-result parsing, key
registration, XNADDR conversion, QoS and sockets. The host's exchange key is
never published; the remote key is zero for the current unencrypted HLE
transport. An embedded-server test follows this path through a successful QoS
probe and a two-way guest UDP exchange, with no broadcast peers configured.
The full retail invite menu/confirmation sequence, cancellation timing and a
complete custom game still need owner play testing. No game was built or run.

A two-instance recipe, using an already built `h2`, is:

1. Run one persistent server, for example `h2 live serve --db file --listen
   127.0.0.1:8074`. Both instances must use its URL.
2. Create accounts Alpha and Bravo with `h2 live account create NAME --server
   http://127.0.0.1:8074`. Establish friendship with `h2 live friend add Bravo
   --as Alpha --server http://127.0.0.1:8074`, then `h2 live friend accept Alpha
   --as Bravo --server http://127.0.0.1:8074` (or use the web UI).
3. Start Alpha with `H2_XNET_ADDR=127.0.0.1:31000 h2 play --live-server
   http://127.0.0.1:8074 --gamertag Alpha`. Wait for sign-in, then start Bravo
   with `H2_XNET_ADDR=127.0.0.1:32000 h2 play --live-server
   http://127.0.0.1:8074 --gamertag Bravo`. Sequential startup also avoids the
   CLI's shared play-accounts file being rewritten before the first HLE reads it.
4. Alpha hosts a custom lobby and invites Bravo from Friends; Bravo accepts.
   `H2_XNET_LOOPBACK_PEERS` is unnecessary: HTTP supplies discovery. Bases on one
   IP must differ by at least seven ports. Across a LAN use reachable LAN IPv4s;
   across the internet forward UDP base through base+6. An HTTP tunnel alone
   does not carry gameplay packets.

## Client HLE: sessions and server discovery

The host/search APIs in [live-matchmaking.md](live-matchmaking.md) sections
1.1, 1.2 and 4 use the background HTTP worker when `LiveConfig.server_url` is
configured. Without a server they retain the local host/empty-search behavior.
These APIs advertise and discover lobbies. They do not implement a matchmaker,
ranked arbitration or NAT traversal. Invitation delivery is described next.

Create copies the four slot counts and the attribute list before returning a
task. Update consumes only dirty attributes and replaces the slot counts;
NULL removes a value. The 16-byte XONLINE_ATTRIBUTE is ID, changed BOOL and an
eight-byte union: integer, UTF-16 string pointer, blob pointer/length, or NULL.
Limits remain 64 attributes, 400 UTF-16 units per string, 800 bytes per blob and
32 advertisements. Server-backed lobbies have at most 16 slots. Invalid guest
ranges leave outputs and dirty flags untouched. A request that was accepted by
the worker consumes the dirty flags even if the later network operation fails.

The worker creates a server session, signs subsequent update/delete batches,
and caches the accepted advertisement when the game pumps its task. Continue
returns running until HTTP completion, then XONLINETASK_S_SUCCESS or an error.
GetInfo returns the same eight-byte XNKID/session ID and locally generated
16-byte exchange key for create/update tasks. Closing the task keeps the lobby;
Delete removes it. Sign-out and controller removal attempt a signed delete;
server expiry handles unreachable clients. Presence pings refresh the host's
advertisements without writing events. No-op updates add no events.

The exchange key stays in the host HLE. Public session responses and events
contain the XNKID and host XNADDR, never the exchange key. Search records use a
zero exchange key because the current HLE sockets have no encryption. The
retail host wrapper at `0x0008fd90` gets the ID/key and then reads its XNADDR;
`0x00072f39` registers the returned pair before QoS listening. The separate
XNetCreateKey/System Link path does not supply this pair.

Search and FindFromID fetch open sessions and own their completed snapshots
until TaskClose. SearchGetResults returns an array of guest pointers. Each
84-byte header contains the ID at +4, XNADDR at +0x0c, zero key at +0x30, open
public/private counts at +0x40/+0x44, filled counts at +0x48/+0x4c, and column
count at +0x50. An offset directory follows. SearchParse decodes integer,
UTF-8 wire string to UTF-16 guest string, blob, and NULL columns into the
caller's specifications, checking all bounds before writing the output.
Result storage is reusable when a search task closes; handles remain monotonic.

The title's procedure 1 returns seven integer columns for attribute IDs
1, 2, 3, 5, 6, 7 and 8 (`0x000b4b80`, `0x000b4a40`). Missing values default to
zero. Its twelve positional integer/NULL query parameters are not yet fully
identified: this procedure currently searches all open sessions, without
inventing filters for anonymous ID-zero parameters. Other procedures accept
named attribute equality filters and return the stored columns in ID order.
FindFromID uses the seven-column title projection. SearchResultsLen retains
the retail capacity formula at `0x003b32f9`: 92 bytes plus the requested row
count times an 84-byte header and each column's eight-byte overhead/value
capacity. Limits are 1000 rows, 64 columns and 16 MiB; the mapped result pool
also has a finite allocation budget and reports OUT_OF_MEMORY when exhausted.

The host address is the configured XNet LAN/public IPv4 and base port. Set
`H2_XNET_ADDR` to an address peers can reach, with distinct ports for two local
instances as described above. The server's HTTP tunnel does not forward game
UDP. Internet peers need reachable UDP base through base+6; this slice adds no
relay or hole punching. Server clients that omit their declared IPv4 use the
trusted public-URL source-IP policy ([live-api.md](live-api.md#sessions)).

Synthetic tests cover the nine entries and their stack cleanup, local fallback,
typed values, dirty flags, stable IDs/keys, task lifetimes and capacity errors.
The server tests exercise authorization, signed batches, projection/replay,
filters, expiry and tunnel source trust. Two synthetic guests publish/search
through an embedded memory-backed server and read the rows and parsed values
that the retail wrappers consume. No test runs the game.

## Client HLE: Teams

The clan-create stop at `0x003afb80` is called by `0x000abe60` from the UI action
`0x00238bf1`. It passes a maximum of 100 members and creator privileges
`0xffffffff`. All thirteen linked XOnlineTeam entries use the same owned store and task seam:
create/delete, enumerate-by-user/results/details, member enumeration/results/
details, member property changes/removal, recruitment by XUID/name, and answers.
The game refreshes enumeration after mutations; no CreateGetResults or
TeamSetProperties thunk is called in the verified boundary. No guest address is
invented for those unused APIs. The host seam supports team-property changes.

The shared owned model/rules are in `live-proto::team`; guest marshalling is in
`live-hle::teams`. A team properties record is 1670 bytes (UTF-16 name 16 units,
description/motto/URL 256 each, WORD custom length and 100 bytes custom data).
A full team is 1698 bytes: 12-byte XUID, properties, DWORD flags, FILETIME and
DWORD active member count. A member is 146 bytes: 12-byte XUID, 16-byte ANSI
gamertag, 106-byte properties, DWORD flags and FILETIME. Outputs are checked and
written exactly to these bounds. The count in enumeration results is OUT, not
an input capacity; limits keep lists within eight teams and 100 members.

Each mutation returns a task; Continue reports success or the specific Teams
HRESULT. Enumerations own snapshots with their detail records. Missing items in
a successful snapshot return E_FAIL, matching retail. The store retains owned
strings/custom data after guest buffers are reused. It survives local sign-out;
cleanup invalidates tasks. With no server configured, recruitment resolves the host accounts file,
records a pending member and retains the outgoing message capture.
Pending teams appear for the recruit with the recruit flag; an accepted answer
moves the member into the roster, and decline removes the pending request.

Privilege bits are delete=1, modify-team=2, modify-member-permissions=4,
remove-member=8 and recruit=16. The title uses all bits for the leader, `0x1c`
for officers, and `0x10` or zero for lower ranks. The shared rules preserve one
creator/leader, reject orphaning/demoting it, require the appropriate permission
for mutations, prevent privilege escalation, and allow an ordinary member to
leave. Duplicate names, full teams, eight-team limits, unknown users and missing
teams produce their XDK errors. Local diagnostics use `H2_BOOT_VERBOSE=1`.

`Hle::local_teams()` snapshots the cache; `local_team_action` is the trusted local
host seam, including team-property edits where no guest thunk is linked. The trusted host action seam is available only without a configured server.
Local-only records are not persistent across runtimes. Synthetic guest
tests cover create/list/details/member editing/recruit/accept/decline/leave/
delete, packed layouts/canaries, snapshot ownership, validation/fault atomicity,
permissions and cleanup. No test builds or runs the title.

### Server-backed Teams

With `LiveConfig.server_url` set, the thirteen entries submit owned requests to
the existing HTTP worker. No HTTP work runs on the guest thread. Tasks remain
running until the response arrives; success snapshots populate the local cache,
and server permission/limit errors finish the task with the XDK Team HRESULT.
Transport failures report cannot-access-service without accepting a local write.
Closing a task drops its result; it does not undo an event already sent.

Creation, property changes, deletion, recruitment, answers and removal are
signed account events on `team-<id>`. The server owns the persistent projection
and validates the same rules as the offline store. Enumeration reads its account
and team endpoints. Recruitment resolves registered accounts by name or XUID,
so another user's private key need not be in the host accounts file. Guest
buffers and keys never enter diagnostics or the request queue as raw pointers.
Remote records are bounded and validated before packing SDK records.

A recruit also gets `TeamRecruitReceived` on their account stream. Pending
teams appear in their team enumeration with the recruit flag; accepting makes
them active members. The HLE does not yet translate that account event into a
Live inbox message or toast, and remote recruitment does not forward optional
voice/text message attachments. The team invitation and answer themselves are
persistent. Team-property edits are available through the server/CLI; the
verified title boundary has no linked TeamSetProperties entry to register.

`h2 live team` creates, lists, edits, recruits, accepts/declines, changes member
permissions, removes/leaves and deletes teams; `h2 live q teams [gamertag]`
reads the views. See live-api.md's Teams section for arguments and wire records.
Two simulated guests and an embedded server test this lifecycle, including
separate account files, pending flags, officer rights, denial and network
failure. Server tests cover signed authors, mailbox append, replay, concurrent
name conflicts and private reads; a CLI test drives signed commands through the
same endpoints. These tests do not build or run the title.
