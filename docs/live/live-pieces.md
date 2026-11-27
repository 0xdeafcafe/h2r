# Live: the pieces

Part of [Xbox Live replacement](live.md). Covers: 4. Pieces.

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

Implemented (crates/apps/cli/src/live_cmd.rs, live_accounts.rs, live_query.rs,
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
h2 live mods import INPUT --pack NAME --author NAME --source-url URL
h2 live mods capture PACK_OR_MAP_ID
h2 live assets extract [--disc PATH] [--out DIR] [--mappack ISO] [--installed DATA]
h2 live assets check [--out DIR]
h2 live store copy [--from DB --to DB] [--from-blobs LOC --to-blobs LOC]
h2 live invite grant <gamertag> [--open-cap N] [--server URL] [--data DIR]
h2 live invite ungrant <gamertag> [--server URL] [--data DIR]
h2 live invite create [--days N] [--code TEXT] [--as GAMERTAG] [--server URL] [--data DIR]
h2 live invite list [--as GAMERTAG] [--server URL] [--data DIR]
h2 live invite revoke <jti> --as GAMERTAG [--server URL]
h2 live q <what> [--server URL] [--json|--jsonl]
h2 play <disc> [--live | --live-server URL | --live-local | --offline] [--live-listen ADDR] [--gamertag NAME] [--invite CODE] [--join URL]
```

`h2 live serve` serves the web app from `web/dist` (or `$H2_LIVE_WEB`) at
`/`, with `index.html` for any path that is not a file or under `/v1/`; it
prints a hint to run `cd web && bun run build` when there is none.

Defaults: `--db` is `$H2_LIVE_DB` or `sqlite://<--data>/live.db` (a
server with no `live.db` but a log in `postgres://localhost/h2live` keeps
that, with a notice saying how to copy it; [live-hosting.md](live-hosting.md#free-hosting-sqlite-and-cloudflare-r2)),
`--listen` `127.0.0.1:8074`, `--server` `$H2_LIVE_SERVER` or
`http://127.0.0.1:8074`, `--data` the server directory below.

The same service runs on Cloudflare Workers. The architecture and the
deployment steps are in [live-worker.md](live-worker.md).

Host data lives in `%LOCALAPPDATA%\h2recomp\live` on Windows and
`~/Library/Application Support/h2recomp/live` on Unix, beside
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
decide (`choose` in `crates/apps/cli/src/play_live.rs`). The app builds are the
other way round: on by default (below).

| `h2 play` flags | Plays | `$H2_LIVE_SERVER` |
|---|---|---|
| none, `--offline`, `--no-live` | offline | ignored |
| `--live` | signed in to `https://live.forbes.red` | a URL replaces it; `off` (`none`, `0`) plays offline with a line |
| `--join URL` | opens a [session link](live-custom-games.md#join-in-halo-2) on the service selected by the ordinary Live flags; implies `--live` | as for `--live` |
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
([In-game sign-in](live-client-hle-offline.md#in-game-sign-in)), then adds this machine's key
([live-security.md](live-security.md) §1, Recovery). `--signin-terminal`
asks on the terminal before the game starts instead, as before (and
without a terminal stops with the `h2 live account recover <tag> --server
URL` command); `h2 live account recover` still works on its own. On the embedded server (`--live-local`) a taken default name is
numbered instead. It prints the gamertag and, for the embedded server, the web app's
URL, then boots with `boot::LiveConfig {
enabled, accounts_path: play-accounts.json, server_url }`. The server stops
when the game does.

**The app builds** (tvOS and iOS, `crates/apps/h2host`; the Xbox app when
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
in each commit's build, so the banner's "Update ready" includes it, and
`h2 play --watch --live-local` rebuilds it in the checkout when `web/` changes. Without
bun it says how to install it and runs without the web app; `H2_LIVE_WEB`
points at a build of your own and skips this.

`h2 live friend add|accept|decline|remove|list` act as a local account
(`--as GAMERTAG`): each writes a signed friend event on that account's
stream (`crates/apps/cli/src/live_cmd.rs`, `friend`).

#### Game art

`h2 live assets extract [--disc PATH] [--out DIR]` decodes the stats
site's art from your own disc into PNGs and a `manifest.json`. The default
`--out` is `game-art/` in the server directory. It covers emblem layers, rank
icons, Live icons, game-type icons, weapon silhouettes, medals, the player
colour palette, every multiplayer map's preview and every campaign level's
level-select picture (`map/<stem>/preview` too), and the game's title and
save images from `default.xbe` (`title/image`, `title/save-image`: the
`$$XTIMAGE` and `$$XSIMAGE` sections, `crates/platform/assets/src/xpr.rs`). A cold run takes well
under a second. Keys and emblem colouring are in
[live-api.md](live-api-details.md#optional-game-art).

- **How the art is found.** No asset is named in the code (CONTRIBUTING.md,
  [bitmaps.md](../formats/bitmaps.md)). Each category is the bitmap tag in the main menu
  or shared map with a known image count and image shape. Where several
  sizes of the same set exist, the largest wins. A map preview is the one
  single-image UI picture a multiplayer map has that the shared map does
  not. A campaign level's preview is the picture the main menu's level
  select names for it ([live-admin.md](live-admin.md#the-catalogue)). Maps
  are told apart by the cache type in their header. Medals and
  the palette are icon glyphs in the fonts' private-use range
  ([fonts.md](../formats/fonts.md)): the medals are the 24-pixel-tall icons and the
  palette the flat colour swatches, from the first font in the engine's font
  table that has the full set.
- **Output.** It is deterministic for a disc. Anything not found or not
  decoded is listed under `skipped`. The manifest's `source` is a SHA-256
  over the extractor version and every map file's path, size, sector and
  first 2 KiB; this is the cache key.
- **Stats banners.** `bun run banner` in `web/` draws the stats site's
  header collages from the extracted maps into the same directory and
  manifest (`banner/home`, `banner/strip`); run it after each extraction.
  See [live-art.md](live-art.md#stats-banners).
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
  knows when to extract again.
- **Map names and pictures on the site.** Wherever a map is named, the
  site reads the disc's name from `catalog.json` (Halo 2's multiplayer maps
  and campaign levels), then the manifest's `maps.<stem>.name` (converted
  Halo 1 maps), then the stem title-cased. A thumbnail is the map's
  `map/<stem>/preview`, the picture the game's Select Map screen shows; a
  game page's header shows it too for a finished game. A Halo 1 map reported
  by its bare name (`bloodgulch`) finds its `h1_` entry when the server
  knows no map by that stem. The catalogue is fetched on its own, so names
  hold when the manifest is missing. A disc variant whose name doesn't say
  its game type gets it from the catalogue ("Rockets (Slayer)"). Halo 1's
  campaign levels are told apart by stem (`a10`..`d40`) even without art,
  so their unpublished packages never read as "modded". With no art (a 404, or art served only to
  this machine and the page opened from another), names are the title-cased
  stems and a finished game's thumbnail is a card with that name. On the
  Worker the art and catalogue come from the published copy
  ([live-worker.md](live-worker-deploy.md#the-site)); nothing of them is in the
  repository or the Worker's bundle.

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

Two crates. `crates/live/live-proto` is what clients and server share, with no
runtime: the wire types of [live-api.md](live-api.md) (XUIDs as 16 hex
digits, title IDs as 8, gamertags with Live's rules: 1..=15 letters, digits
and single spaces, starting with a letter, compared case-insensitively),
the signing code of [live-security.md](live-security.md) (`sign`: Ed25519
keys as JWK with RFC 7638 thumbprints, compact JWS, signed event bodies and
batches, self-issued tokens, invites, the hash chain; `jcs`: RFC 8785
canonical JSON) and the MOTD builder (`motd`). The game-side client and
the CLI sign with exactly this code. `crates/live/live-server` is the service
(axum, tokio, sqlx with runtime-checked queries; see its README).

The API is [live-api.md](live-api.md) on `127.0.0.1:8074`. Built: health,
discovery, accounts (create, list, one, keys by gamertag and by XUID),
presence ([heartbeats](live-api-social.md#presence-heartbeats)), [server invites](live-api-sessions.md#server-invites),
the event log (`GET /v1/events` with `from`/`cursor`, `type`,
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

**Games** (crates/live/live-server/src/games.rs, live-proto `game`). `POST
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
reason) come from the read model. The chunk index serves newer
recordings and their verified archives. The `/frames` endpoint serves
recordings made before chunks. Accounts list their 10 latest games as
`recent_games`. [Game reads](live-api-events.md#frame-chunks)

**Ingest.** `/v1/ingest` is the game's WebSocket. After authentication
the client sends packed frames and signed event batches. When discovery
advertises batched ingest, the client combines packed frames in a
single message. The server sends admitted frames to readers at about
15 frames per second per game on average and records them in H2CK
chunks. Finished recordings are queued for H2AR publication. The
socket closes (4401) when its token expires unless the same author
reauthenticates.
[Stream and ingest API](live-api-social.md#streams-websocket) ·
[Archives](live-api-details.md#finished-replay-archives)

**Map outlines.** `PUT /v1/maps/{map}/outline` (`games.write`, 2 MiB) stores
the outline as a blob and a server-signed `MapOutlineStored` (`{map,
sha256, size, source}`) on `map-<map>`. Admins and the operator can
upload and replace outlines. `GET` returns the bytes.

**Managers.** One sweeper task (every 5 s) ends what clients leave behind,
as the server: presence with no event or ping for 90 s gets
`PresenceExpired`, and a game with no frame or event for 5 minutes gets
`GameEnded {reason: "lost"}`. HTTP heartbeats and `ping` on the ingest
socket refresh the in-memory deadline. Deadlines start
from the projection (the last event's `at`), so they survive a restart; a
restart then renews every presence, open session and live game to a full
deadline from the moment it finishes (`Model::renew_deadlines`). Pings and
frames are not in the log, so without that a player who sat in a lobby for
longer than the TTL would be expired by the first sweep after a restart
(every Worker deploy restarts the hub) while still connected. Whoever really
is gone still expires a TTL after the restart.
Until the deadline passes, a game that has gone quiet still reads as live.
The 5 minutes is generous on purpose. Across the live.forbes.red log on
2026-11-18 (46 games), a game being played never went more than 17 s
between recorded frames (p99.9 10 s). Half the games (23) ended `lost`,
each 301-306 s after its last frame, and 9 of those never sent a frame at
all. How long a paused game goes without a frame is not measured, so the
wait stays well above it. The web shows a `lost` game as ended, labelled
"Unfinished", with its length on the game clock (0 with no frames).

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
