# Live service API

Status: contract, being implemented. The service is `crates/live-server`
([live.md](live.md)). Everything here is JSON over HTTP on one port
(default `127.0.0.1:8074`), plus two WebSockets. Identity, signing, tokens
and discovery are in [live-security.md](live-security.md); this page refers
to it as §n. Every client is given a base URL and finds the rest from
`GET /.well-known/h2live.json`.

Three kinds of client use it:

- the game (the recomp's Live HLE and its telemetry tap),
- the web app (`web/`), served by the same server at `/`,
- agents and scripts, through `h2 live q` or plain HTTP.

## Conventions

- XUIDs are 16-digit hex strings (`"0009a1b2c3d4e5f6"`), title IDs 8-digit
  hex (`"4D530064"`), times RFC 3339 UTC, positions are the log's global
  `position` (an integer, strictly increasing).
- Errors: non-2xx with `{ "error": "<code>", "message": "..." }`. Codes:
  `invalid`, `not_found`, `conflict`, `unavailable`.
- Lists take `?limit=` (default 100, max 1000) and return
  `{ "items": [...], "next": <cursor or null> }`.

## Events

Every change is an event in the log ([live.md §4.3](live.md)). The wire
form, used by `/v1/events` and the stream:

```json
{ "position": 1042, "stream": "game-7f3c...", "version": 12,
  "type": "PlayerKilled", "at": "2026-11-15T20:14:03.120Z",
  "data": { ... },
  "author": "acct:0009a1b2c3d4e5f6@live.example.net", "kid": "...",
  "jws": "<compact JWS of the signed body>", "hash": "<hex sha256>" }
```

`data`, `type`, `stream` and `at` are copies of what the JWS signs, for
convenience; a verifying reader uses the JWS (security §2).

Event types so far (payload fields):

| stream | type | data |
|---|---|---|
| `account-<xuid>` | `AccountCreated` | `xuid`, `gamertag` |
| `gamertag-<lower>` | `GamertagReserved` | `xuid` |
| `title-<id>-file-<name>` | `TitleFilePublished` | `title_id`, `name`, `sha256`, `size` |
| `presence-<xuid>` | `SignedIn`, `PresenceChanged`, `SignedOut`, `PresenceExpired` | `xuid`, `state`, `game_id?` |
| `game-<id>` | `GameStarted` | `game_id`, `console`, `map`, `mode` (`campaign`/`multiplayer`), `variant?`, `team_play`, `score_limit?`, `time_limit_s?`, `players` |
| `game-<id>` | `PlayerJoined`, `PlayerLeft` | `player` (below) / `index` |
| `game-<id>` | `PlayerKilled` | `tick`, `victim`, `killer?`, `weapon?`, `kind` (`kill`/`suicide`/`betrayal`/`world`) |
| `game-<id>` | `ScoreChanged` | `tick`, `players: [{index, score, kills, deaths, assists}]`, `teams?: [{team, score}]` |
| `game-<id>` | `GameFrame` | a frame (below), recorded at 1 Hz for replays |
| `game-<id>` | `GameEnded` | `tick`, `reason` (`finished`/`quit`/`lost`), `winner?`, final `players` |

A player: `{ "index": 0, "name": "Sneaky Pete", "xuid": "...?", "team": 1,
"local": true }`. `index` is the game's player slot and is stable for the
game. `team` is the game's team index; colours are the web app's business.

A frame (`tick` is the game tick, 30 per second):

```json
{ "game_id": "7f3c...", "tick": 5400,
  "players": [ { "index": 0, "alive": true, "pos": [12.5, -3.25, 4.0],
                 "yaw": 1.57, "vehicle": null } ] }
```

`GameFrame` events hold a frame at 1 Hz. Live frames go out on the stream at
up to 15 Hz and are not stored.

## Read endpoints

| method and path | returns |
|---|---|
| `GET /healthz` | `{ "ok": true, "position": <head> }` |
| `GET /.well-known/h2live.json` | server id, API base, server keys (security §4) |
| `GET /v1/accounts/{gamertag}/keys` | account public keys with validity ranges |
| `GET /v1/accounts` | accounts: `{xuid, gamertag, created_at, online}` |
| `GET /v1/accounts/{gamertag}` | one account plus `recent_games` |
| `GET /v1/presence` | who is online now: `{xuid, gamertag, state, game_id?, since}` |
| `GET /v1/games?state=live\|ended` | game summaries: `{game_id, map, mode, variant, team_play, started_at, ended_at?, players, teams?}` |
| `GET /v1/games/{id}` | summary plus scoreboard and the last 50 kills |
| `GET /v1/games/{id}/frames?from_tick=&to_tick=` | recorded frames, for replay |
| `GET /v1/maps/{map}/outline` | top-down map outline (see below), 404 until a console has uploaded it |
| `GET /v1/events?from=&type=&stream=&limit=` | raw log, oldest first |
| `GET /v1/titles/{title}/files/{name}` | raw file bytes, `ETag` = sha256 |

## Write endpoints

Every write needs `Authorization: Bearer <self-issued JWT>` (security §3)
except account creation. Signed bodies are sent raw as a compact JWS with
`Content-Type: application/jose`. Event bodies are signed JWS (security §2):
`POST /v1/games/{id}/events` takes one JWS over the array.

| method and path | body | does |
|---|---|---|
| `POST /v1/accounts` | JWS signed by the new key over `{gamertag, jwk, invite?}` | create; 201 `{xuid, gamertag, id}`, 409 if taken |
| `PUT /v1/titles/{title}/files/{name}` | raw bytes, operator token | publish a title file (max 2 MiB) |
| `POST /v1/events` | one compact JWS (an event on a stream the author may write) | append; 201 `{position}` |
| `POST /v1/invites` | (operator token) | create a single-use invite |
| `POST /v1/games` | `GameStarted` data without `game_id` | starts a game; returns `{game_id}` |
| `POST /v1/games/{id}/events` | `[{type, data}]` | append game events in order |
| `PUT /v1/maps/{map}/outline` | outline JSON | store a map outline |

## Streams (WebSocket)

**`GET /v1/stream?from=<position>`** for readers. The server first replays
the log from `from` (omit it to start at the head), then sends new events
as they are appended, then interleaves live frames:

```json
{ "kind": "event", "event": { ...wire event... } }
{ "kind": "frame", "frame": { ...frame... } }
{ "kind": "caught_up", "position": 1042 }
```

`?games=<id>,<id>` limits frames to those games (events are always sent; they
are small). Without it, frames for every live game are sent.

**`GET /v1/ingest`** for the game. The first message is
`{ "kind": "auth", "token": "<jwt>" }`. Then the client sends
`{ "kind": "frame", "frame": {...} }` and
`{ "kind": "events", "game_id": "...", "events": [{type, data}] }`. The
server fans frames out to readers, records one per second per game as a
`GameFrame`, and appends events.

## Map outline

Map geometry comes from the user's own map files, so the server never ships
it. The game uploads an outline for its map the first time it starts a game
there:

```json
{ "map": "lockout", "bounds": { "min": [x,y,z], "max": [x,y,z] },
  "layers": [ { "z": [zmin, zmax], "polygons": [[[x,y], ...], ...] } ] }
```

Polygons are top-down projections of floor surfaces, grouped into height
layers so multi-storey maps can be drawn one floor at a time.

## Optional game art

A server may serve art extracted from its owner's own disc (`h2 live assets
extract`; nothing of it is in this repo). Clients must work without it.

- `GET /art/game/manifest.json`: the manifest, or 404 when the server has
  no game art or will not give it to this client. By default it is served
  only to clients on the server's own machine (a loopback peer with no
  `Forwarded`/`X-Forwarded-For`); an operator's `--share-game-art` serves
  it to everyone. A 404 means "use your own art".
- `GET /art/game/<file>`: a PNG named by the manifest, 404 likewise.

```json
{ "version": 1,
  "source": "<sha256 of the disc's map files>",
  "images":  { "<key>": { "file": "rank/12.png", "width": 28, "height": 26 } },
  "aliases": { "gametype/slayer": "gametype/0" },
  "emblem":  { ...colouring rules, below... },
  "palette": ["#ffffff", "..."],
  "skipped": { "<key or prefix>": "why it is missing" } }
```

`version` is the manifest format; it changes only when a client could no
longer read it (new fields and keys do not change it). `file` is relative
to the manifest. Keys (a client looks a key up in
`aliases` first, then `images`):

| Key | What |
|---|---|
| `emblem/fg/<0..63>` | emblem foreground layers |
| `emblem/bg/<0..31>` | emblem background layers |
| `rank/<1..50>` | rank (level) icons |
| `live/<0..31>` | Live status icons (friend, invite, voice, message...) |
| `gametype/<0..10>` | game-type icons; aliases `gametype/slayer`, `king_of_the_hill`, `oddball`, `juggernaut`, `ctf`, `assault`, `territories` (identified by eye; the others have only an index) |
| `weapon/<0..18>` | weapon silhouettes (small, single-mask: blue channel with matching alpha); aliases `weapon/energy_sword`, `plasma_pistol`, `plasma_rifle`, `sniper_rifle`, `rocket_launcher`, `smg`, `beam_rifle`, `sentinel_beam` (by eye; the others have only an index) |
| `medal/<0..31>` | medals, full colour; aliases `medal/double_kill`, `triple_kill`, `killtacular`, `kill_frenzy`, `killtrocity`, `killimanjaro`, `killing_spree`, `running_riot`, `rampage`, `berserker`, `overkill`, `sniper_kill`, `splatter`, `bone_cracker`, `assassin`, `stick_it`, `flag_carrier_kill`, `bomb_carrier_kill`, `flag_returned`, `bomb_returned` (by eye; the spree order is inferred from the medals' growing number of points) |
| `map/<map>/preview` | a multiplayer map's preview picture; `<map>` is the map file name without `.map` |

`palette` is the player colour palette, `#rrggbb` by colour index (the
colours are stored at 4 bits per channel); empty, with a `skipped` entry,
when not found.

**Emblems.** An emblem is a background layer with a foreground layer drawn
over it. Each layer image holds two masks, not colours: mask *a* is the red
channel (green equals it, so it looks yellow) and mask *b* the blue channel.
The image's alpha copies mask *b* and is not coverage. Per pixel, with
channels in 0..1:

```
rgb      = colour_a * red + colour_b * blue
coverage = background: 1;  foreground: max(red, blue)
```

`emblem.colours` in the manifest gives the profile colour (an index into
`palette`) for each mask
(foreground: primary for *a*, secondary for *b*; background: background
colour for *a*, a second background colour for *b*). The masks were
measured on the disc. Which profile colour fills which mask has not been
confirmed against the game.

## Clarifications (2026-11-15)

Settled after the first client was built against this page.

1. **Positions.** The log starts at position 1. `?from=N` is exclusive:
   the stream and `/v1/events` return events with `position > N`, so
   `from=0` is the whole log and a client resumes with the last position it
   applied.
2. **Snapshots join the stream.** Every read response carries
   `"position": <head when it was read>`, so a client can load a snapshot
   and open the stream with `?from=<that position>`. The stream also takes
   `?exclude=GameFrame` (comma-separated types to leave out) and
   `?types=` (only these types).
3. **Lists** are `{ "items": [...], "next": <string or null>, "position": n }`
   everywhere. `next` is opaque; pass it back as `?cursor=`. For
   `/v1/events` it happens to be the next `from`.
4. **Errors** are exactly `{ "error": "<code>", "message": "..." }` with the
   codes in Conventions (plus `unauthorized`, `forbidden`, `rate_limited`,
   `too_large`, `unimplemented`).
5. **Presence states** are `online`, `away` and `playing` (`game_id` set);
   `SignedOut` and `PresenceExpired` mean offline.
6. **`GameEnded.winner`** is `{ "team": n }` in team games,
   `{ "player": index }` otherwise, or null for a draw or quit.
7. **`GameEnded.players`** is the final scoreboard: `[{index, score, kills,
   deaths, assists}]`.
8. **Frames** are `GET /v1/games/{id}/frames?from_tick=&to_tick=&cursor=`
   returning a list (item 3) of frames, at most 600 per page, oldest first.
   `GameFrame.data` is the frame plus `source` (the ingesting author id).
9. **`PlayerKilled`** carries `pos`, the victim's position.
10. **`GameStarted`** carries `tick`, the game tick it started at.
11. **Keys.** `GET /v1/accounts/{gamertag}/keys` and
    `GET /v1/accounts/by-xuid/{xuid}/keys` both return
    `{ "keys": [ { "kid", "jwk", "valid_from": <position>,
    "valid_to": <position or null> } ] }`. The hash chain, batch JWS and
    `at` rules are in [live-security.md §2](live-security.md).
12. **Well-known `api`** may be relative (`"/v1"`); resolve it against the
    well-known document's URL.
13. **Axes.** World coordinates are the game's: x and y horizontal, z up,
    in world units. `yaw` is radians counter-clockwise from +x. Map outlines
    use the same axes.
14. **`console`** is a free display string (the console's name); clients
    may show it or not.

## As built (2026-11-15)

Decisions made while building the games slice; they override the tables
above where they differ.

- **`POST /v1/games`** returns `{game_id}`; the server picks the id (32 hex)
  and writes `GameStarted` itself, signed by the server key, with an extra
  `host` field naming the token's author. Later game events are written by
  the host or a player in the game.
- **Ingest `events`** messages are `{ "kind": "events", "game_id": "...",
  "jws": "<one batch JWS>" }`, answered with `{kind: "appended", position}`
  or `{kind: "error", ...}`. `{kind: "ping"}` refreshes presence.
- **`POST /v1/events`** returns the stored wire event (a superset of
  `{position}`).
- **Map outlines**: the first upload for a map wins; the operator can replace
  it.
- **`POST /v1/presence/ping`** needs `lxm: events.write` and returns 404 if
  the caller isn't signed in. Presence expires 90 s after the last event or
  ping; a game with no events or frames for 5 minutes ends as `lost`.
- **`recent_games`** on an account is the latest 10.
- **Account creation** payload is `{gamertag, jwk, invite?, aud, iat}`;
  `aud` is the server id from the well-known document and `iat` must be
  within 5 minutes of the server's clock. The fixed test vector is
  `account_request_vector` in `crates/live-proto/src/sign.rs`.
