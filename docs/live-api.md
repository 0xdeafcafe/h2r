# Live service API

Status: contract, being implemented. The service is `crates/live-server`
([live.md](live.md)). Everything here is JSON over HTTP on one port
(default `127.0.0.1:8074`), plus two WebSockets. No auth yet: the service
is meant for localhost and a trusted LAN.

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
  "data": { ... } }
```

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
| `GET /v1/accounts` | accounts: `{xuid, gamertag, created_at, online}` |
| `GET /v1/accounts/{gamertag}` | one account plus `recent_games` |
| `GET /v1/presence` | who is online now: `{xuid, gamertag, state, game_id?, since}` |
| `GET /v1/games?state=live\|ended` | game summaries: `{game_id, map, mode, variant, started_at, ended_at?, players, teams?}` |
| `GET /v1/games/{id}` | summary plus scoreboard and the last 50 kills |
| `GET /v1/games/{id}/frames?from_tick=&to_tick=` | recorded frames, for replay |
| `GET /v1/maps/{map}/outline` | top-down map outline (see below), 404 until a console has uploaded it |
| `GET /v1/events?from=&type=&stream=&limit=` | raw log, oldest first |
| `GET /v1/titles/{title}/files/{name}` | raw file bytes, `ETag` = sha256 |

## Write endpoints

| method and path | body | does |
|---|---|---|
| `POST /v1/accounts` | `{gamertag}` | create; 201, 409 if taken |
| `PUT /v1/titles/{title}/files/{name}` | raw bytes | publish a title file |
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

**`GET /v1/ingest`** for the game. The client sends
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
