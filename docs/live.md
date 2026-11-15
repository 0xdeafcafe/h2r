# Xbox Live replacement

Status: design, with the first server slice implemented (accounts and title
files: `crates/live-proto`, `crates/live-server`, `h2 live ...` in
[4.2](#42-cli) and [4.3](#43-service)). The opt-in client sign-in HLE is
described in [Client HLE: sign-in](#client-hle-sign-in).

The goal is to make Halo 2 believe it is on Xbox Live: sign in with a
gamertag, show the message of the day and playlists, keep a friends list
with presence, and later matchmake and play. Accounts are made from the CLI
for now. The service behind it is ours, event sourced on Postgres.

## 1. What the title links

Halo 2 statically links `XONLINES` 5849, the secure Xbox Live client. It
lives in two code sections plus a read-only data section:

| section   | VA           | size    | analyzer starts |
|-----------|--------------|--------:|----------------:|
| `XONLINE` | `0x003a0620` | 182,708 | 913 |
| `XNET`    | `0x003ccfe0` |  78,880 | 381 |
| `XON_RD`  | `0x0056ade0` |   8,616 | data |

Signatures name only 13 `XOnline` and 10 `XNet` functions
([xdk-libraries.md](xdk-libraries.md)). Everything else has to be named by
hand.

**Boundary size.** A scan for direct `call rel32` from game code (`.text`,
`XPP`, `D3D`, `DSOUND`, `BINK`) into those two sections finds about 131
distinct targets from 185 call sites: ~94 in `XONLINE`, ~37 in `XNET`. That
is a raw byte scan, so a few are probably false positives, but it puts the
public API the game actually uses at roughly a hundred functions, not the
thousand in the library. `0x003cd718` (17 sites) is the busiest; the
`0x003cd0e1..0x003cd17d` cluster looks like the XNet socket calls (send,
receive, select).

**Today.** The kernel reports no cable (`crates/kernel/src/net.rs`,
`PhyGetLinkState` returns 0), so the title skips `XNetStartup` and every
Live path reports "network cable unplugged". Nothing in `XONLINE` runs.

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
replaced ([ARCHITECTURE.md](ARCHITECTURE.md)). `XOnlineLogon`,
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

- **Link up.** `ConsoleConfig::phy_link` already exists; `--live` (or
  `H2_LIVE=<url>`) sets it so XNet starts.
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
h2 live serve [--db URL] [--listen ADDR] [--data DIR] [--id HOST]
              [--private] [--invite-only]
              [--tls-cert PEM --tls-key PEM | --insecure-http]
h2 live account create <gamertag> [--server URL] [--invite CODE]
h2 live account import <file> [--server URL]
h2 live account list [--server URL]
h2 live motd set <line>... [--header TEXT] [--server URL] [--data DIR]
h2 live motd set --file PATH [--server URL] [--data DIR]
h2 live invite create [--days N] [--server URL] [--data DIR]
h2 live q <what> [--server URL] [--json|--jsonl]
h2 play <disc> --live | --live-server URL [--live-listen ADDR] [--gamertag NAME]
```

`h2 live serve` serves the web app from `web/dist` (or `$H2_LIVE_WEB`) at
`/`, with `index.html` for any path that is not a file or under `/v1/`; it
prints a hint to run `cd web && bun run build` when there is none.

Defaults: `--db` is `$H2_LIVE_DB` or `postgres://localhost/h2live`,
`--listen` `127.0.0.1:8074`, `--server` `$H2_LIVE_SERVER` or
`http://127.0.0.1:8074`, `--data` the server directory below.

Host data lives in `~/Library/Application Support/h2recomp/live`, beside
the movies directory and the VFS data root ([storage.md](storage.md)):

- `accounts.json` (mode 0600): one entry per account held here, `{xuid,
  gamertag, id, server, server_id, kid, key}`, where `key` is the account's
  Ed25519 private JWK (`{kty, crv, x, d}`; older files hold the bare
  base64url `d`, which still reads). `account create` generates the key
  locally, signs the creation request (with `aud` = the server id and
  `iat`) and stores it; the server only ever sees the public half.
  `account import` takes the entry the web app downloads after a browser
  sign-up (`{xuid, gamertag, server, key}`, or a list of them), checks that
  the key parses and is a current key of that XUID on the server
  (`/v1/accounts/by-xuid/{xuid}/keys`), and adds it, refusing an XUID that
  is already there or a gamertag already held for that server. The
  game-side HLE answers `XOnlineGetUsers` from `xuid` and `gamertag`, so no
  HDD config-sector work is needed.
- `play-accounts.json` (0600): the one account `h2 play --live` hands the
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

**`h2 play --live`** starts a service inside the process on a background
runtime: Postgres from `$H2_LIVE_DB` or `postgres://localhost/h2live`
(creating the database if it is missing), else the in-memory store with a
one-line warning. It binds 127.0.0.1 on a free port (`--live-listen ADDR`
for a fixed one) with the identity in `live/server`, so its accounts are
`h2 live serve`'s. `--live-server URL` uses a running service instead. The
account is the first local one for that server whose key the server still
lists, else a new one named `--gamertag NAME` or after the OS user
(letters and digits, a single space for anything else, numbered when
taken). It prints the gamertag and, for the embedded server, the web app's
URL (or the `bun run build` hint), then boots with `boot::LiveConfig {
enabled, accounts_path: play-accounts.json, server_url }`. The server stops
when the game does.

Planned, not built: `h2 live friend add <gamertag> <gamertag>` (with
friends).

#### Game art

`h2 live assets extract [--disc PATH] [--out DIR]` decodes the stats
site's art from your own disc into PNGs and a `manifest.json`. The default
`--out` is `game-art/` in the server directory. It covers emblem layers, rank
icons, Live icons, game-type icons, weapon silhouettes and every multiplayer
map's preview. Keys and emblem colouring are in
[live-api.md](live-api.md#optional-game-art).

- **How the art is found.** No asset is named in the code (CONTRIBUTING.md,
  [bitmaps.md](bitmaps.md)). Each category is the bitmap tag in the main menu
  or shared map with a known image count and image shape. Where several
  sizes of the same set exist, the largest wins. A map preview is the one
  single-image UI picture a multiplayer map has that the shared map does
  not.
- **Output.** It is deterministic for a disc. Anything not found or not
  decoded is listed under `skipped`. The manifest's `source` is a SHA-256
  over the extractor version and every map file's path, size, sector and
  first 2 KiB; this is the cache key.
- **Serving.** `h2 live serve` serves the art at `/art/game/` from
  `--game-art DIR`, or from `game-art/` in `--data` when it is there. By
  default it goes only to clients on the same machine, since it is your copy
  of the game's files. `--share-game-art` serves it to everyone and says at
  startup that this redistributes them.
- **`h2 play --live`** extracts the art once per disc (on first run, or when
  the cache key changes) and serves it from the embedded server. Without
  art, `/art/game/manifest.json` is a 404 and the web app uses its own art.

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

**The log.** One append-only `events` table: `position bigserial`,
`stream_id`, `version`, `type`, `data jsonb`, `at`, `author`, `kid`, `jws`,
`jws_index` (batches), `hash`, unique on `(stream_id, version)`. The `jws`
is the author's, untouched, and the source of truth: events are decoded
from it, and the other columns are copies for querying. `hash` is
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

Postgres rather than Mongo: an event store needs ordered append with a
uniqueness check per stream, which is one table and one constraint in
Postgres.

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
3. **Service and MOTD.** Server slice done (`h2 live serve`, accounts in
   Postgres, title storage serving files by name); what remains is decoding
   the `message_of_the_day.dat` and `multiplayer_hopper.dat` formats and
   having the client HLE fetch them.
4. **Friends and presence.**
5. **System Link over XNet HLE** (two local instances).
6. **Matchmaking and custom games over Live**, then stats and clans.

## 6. Open questions

- The `.data` tables at `0x004672b8` and `0x0046dd5c`: who reads them and
  which call fetches the file.
- What `XOnlineLogon` failures the game checks for (title update required,
  service down) and which ones send it to the dashboard (`xonlinedash.xbe`
  is referenced from the image).
- Whether Halo 2 needs `XOnlineTitleUpdate`-style checks to pass before
  matchmaking; title updates themselves are covered in
  [title-updates.md](title-updates.md).
- NAT: direct UDP works on a LAN. Across the internet the service will have
  to relay or punch holes; not needed for the first milestones.

## Client HLE: sign-in

`crates/live-hle` replaces the public entries in
[live-boundary.md](live-boundary.md) when the host enables Live. It uses
`Dispatch::register_override`, as XAPI and DSound do. Disabled is the default:
it registers nothing, reads no account file and leaves the console PHY setting
alone. These addresses apply to the retail 5849 image; they are not a title-update
matcher (see [title-updates.md](title-updates.md) §1.1).

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
The additional `server`, `key` and future fields are ignored; signing keys are
not retained in client state or included in diagnostics. Invalid files fail
configuration with an error that does not include their contents. No path means
an empty account list; the client does not invent an account. The configured
server base URL is stored for the next milestone. This milestone makes no
service requests and does not authenticate accounts against a server.

Transport startup succeeds through XNetStartup, WSAStartup and XOnlineStartup,
with paired reference-counted cleanup. GetUsers writes the fixed 16-record
buffer and an output count. Records are 112 bytes, with the XUID at offset 0,
gamertag at 12 and per-user HRESULT at 108. Logon accepts four controller slots,
checks their identities against the configured accounts and returns a persistent
SDK task handle. TaskContinue and LogonTaskGetResults return connection established
(`0x001510f0`) for that task. GetLogonUsers returns a stable guest pointer to the
four records with per-user HRESULT zero. Closing the connection clears that array
and invalidates its tasks. ChangeLogonUsers commits a replacement array on its
first pump and reports completion on the next; its result getter writes four
successful HRESULTs. ServiceInfo succeeds for every requested service ID, with
an optional endpoint output. NotificationSetState accepts the observed state
payload without publishing presence to a server.

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
| XOnlinePresenceInit | Returns an empty task that completes successfully. |
| XOnlinePresenceAdd | Validates the task and packed XUID input array; retains no presence data. |
| XOnlinePresenceSubmit | Accepts the presence task; its pump completes with no data. |
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

All 130 named boundary targets are registered when enabled. Sign-in and the
sockets/System Link entries below implement 55 of them. Each of the other 75,
including matchmaking, storage, stats and the remaining friend helpers,
stops with `unimplemented Live API <name>` before reading arguments or executing
the linked library. There is no silent fallback to XONLINES. Those stops are the
next milestones' boundaries. The tests establish the public API contracts and
host transport; the game has not been run to establish a playable session.

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
requires a registered XNKID and returns the peer's real IPv4, while retaining
its full XNADDR, key ID and host base for later routing. XNetInAddrToXnAddr
returns that record and optional key ID. recvfrom also learns a sender's base
from the channel's source port before secure registration. Incoming channel
ports are translated back to the guest's virtual port. A single IP can identify
one remote endpoint in an HLE at a time; registering another base on that IP
fails with WSAEADDRINUSE, instead of redirecting an existing peer.

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
The 75 remaining boundary entries still stop by name, including
XNetTsAddrToInAddr, online matchmaking, stats, storage, messages and most friend
operations. Actual lobby/gameplay progression has not been exercised.
