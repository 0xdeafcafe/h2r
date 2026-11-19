# Live service API

Status: contract, implemented apart from what the sections mark as not
built (federation among them). The service is `crates/live-server`
([live.md](live.md)). Everything here is JSON over HTTP on one port
(default `127.0.0.1:8074`), plus three WebSockets (stream, ingest and the
co-op relay). Identity, signing, tokens
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
| `account-<xuid>` | `KeyAdded`, `KeyRevoked` | `jwk`, `kid` (security §1) |
| `account-<xuid>` | `FriendRequested`, `FriendAccepted`, `FriendDeclined`, `FriendRemoved` | `from`/`to`, or `by`/`friend` ([Friends](#friends), below) |
| `gamertag-<lower>` | `GamertagReserved` | `xuid` |
| `title-<id>-file-<name>` | `TitleFilePublished` | `title_id`, `name`, `sha256`, `size` |
| `presence-<xuid>` | `SignedIn`, `PresenceChanged`, `SignedOut`, `PresenceExpired` | `xuid`, `state`, `game_id?`, `activity?`, `build?`, `capabilities?` (below) |
| `game-<id>` | `GameStarted` | `game_id`, `console`, `map`, `mode` (`campaign`/`multiplayer`), `variant?`, `team_play`, `score_limit?`, `time_limit_s?`, `players`, `kind?`, `playlist?`, `playlist_id?`, `difficulty?`, `online?` (below) |
| `game-<id>` | `PlayerJoined`, `PlayerLeft` | `player` (below) / `index` |
| `game-<id>` | `PlayerKilled` | `tick`, `victim`, `killer?`, `weapon?`, `kind` (`kill`/`suicide`/`betrayal`/`world`) |
| `game-<id>` | `ScoreChanged` | `tick`, `players: [{index, score, kills, deaths, assists}]`, `teams?: [{team, score}]` |
| `game-<id>` | `FrameChunk` | a stored chunk of the game's recorded frames ([Frame chunks](#frame-chunks)) |
| `game-<id>` | `GameFrame` | a frame (below), recorded at 1 Hz for replays by servers before frame chunks |
| `game-<id>` | `GameEnded` | `tick`, `reason` (`finished`/`quit`/`lost`), `winner?`, final `players` |

A player: `{ "index": 0, "name": "Sneaky Pete", "xuid": "...?", "team": 1,
"local": true }`. `index` is the game's player slot and is stable for the
game. `team` is the game's team index; colours are the web app's business.

A Clanker bot adds `"bot": true` ([bots.md](../game/bots.md) §6, §9). A
bot from a server roster carries its rostered `xuid`, accepted only for the
game's `session_id` roster; a host's own bot (a local slot, or a phantom it
made) has no `xuid` and needs no session. Bots are game-only: no player
page, no career stats ([Stats summary](#stats-summary)), no rank of their
own unless rostered as persistent.

A player may also carry `appearance`, read from a local player's signed-in
profile; it is left out when nothing is known, and so is any field in it:

```json
"appearance": { "primary": 5, "secondary": 9, "tertiary": 2, "quaternary": 11,
                "emblem_foreground": 30, "emblem_background": 7 }
```

`primary`..`quaternary` index the art manifest's 18-colour `palette`;
`emblem_foreground` (0..63) and `emblem_background` (0..31) index
`emblem/fg/<n>` and `emblem/bg/<n>`. Remote players have none yet, and there
is no rank (docs/live/live-telemetry.md, "Player appearance").

A frame (`tick` is the game tick, 30 per second; it only goes up within a
game, as do the ticks of its events, and a frame lists only players it can
place: a player whose unit cannot be read and who has no body or last place
is left out, never sent at the origin):

```json
{ "game_id": "7f3c...", "tick": 5400,
  "players": [ { "index": 0, "alive": true, "pos": [12.5, -3.25, 4.0],
                 "yaw": 1.57, "species": "elite", "aim": [0.7, 0.7, 0.1],
                 "weapon": "battle_rifle", "shots": 3 } ] }
```

A frame player may also carry, each left out when unknown (older senders
send none of them; draw what is there):

| field | meaning |
|---|---|
| `vehicle` | the vehicle's object datum (hex) when riding one |
| `species` | `"spartan"` or `"elite"`, from the biped's tag; another biped passes through as its short id, and readers fall back to spartan |
| `aim` | the unit's aim direction in game coordinates (z up), unit length |
| `weapon` | the held weapon's tag file stem, lower case: `battle_rifle`, `smg`, `energy_blade` (the sword), `brute_shot`, ... Exactly the stem lower-cased, as the art manifest names weapon models |
| `shots` | shots the held weapon fired since the player's previous frame; `0` is sent as absent, at most 64 |

The server refuses a frame whose `aim` is not finite or is zero, whose
`species` or `weapon` is not `[a-z0-9_]{1,32}`, or whose `shots` is over 64,
and stores `aim` normalised to unit length. A frame the server drops for the
15 Hz limit hands its `shots` on to the next frame it sends, so shots are
counted since the previous *delivered* frame.

A frame may also carry `vehicles` and `actors`, each left out when empty:

```json
"vehicles": [ { "id": "90040004", "kind": "ghost", "pos": [20.0, 0.0, 1.0],
                "forward": [0.0, 1.0, 0.0], "up": [0.0, 0.0, 1.0] } ],
"actors": [ { "id": "90070007", "species": "grunt", "pos": [5.0, 6.0, 7.0],
              "yaw": 1.0, "aim": [0.0, 1.0, 0.0], "weapon": "plasma_pistol",
              "shots": 2, "team": "covenant", "vehicle": "90040004" } ]
```

`vehicles` are every vehicle in the world, occupied ones first, at most
64, each with `occupied` (someone, player or AI, is in it; absent reads as
true, since older senders listed only occupied vehicles); `id` is the
object datum in hex, the value `FramePlayer.vehicle` and `FrameActor.vehicle`
hold. `kind` is the `vehi` tag file stem (`warthog`, `ghost`, `banshee`,
...), `forward` and `up` unit vectors in game coordinates. A turret (the
Warthog's chaingun, the Scorpion's cannon) is a vehicle of its own with
`parent`, the id of the vehicle it is mounted on (its `pos`, `forward`
and `up` are then relative to that one); any vehicle may carry `aim`,
where it points, a unit vector (normalised, refused when not finite or
zero, as a player's). `actors` are the
live AI-controlled bipeds, nearest to a player first, at most 128.
`species` is the directory of the biped tag (`grunt`, `jackal`, `elite`,
`marine`, `bugger`, `floodcombat_human`, `tartarus`, ...), lower case; `team` in campaign is `player`,
`human`, `covenant`, `flood`, `sentinel`, `heretic`, `prophet` or
`default`, else the team index as text. `kind` and `species` are `unknown`
when the map's tag names are not available; `aim`, `weapon`, `shots`,
`team` and `vehicle` are left out when not known. The server refuses
non-finite positions or vectors, zero `forward`/`up`/`aim`, ids outside
`[a-z0-9_]{1,32}`, more than 64 vehicles or 128 actors, and stores the
vectors normalised; actors' shots carry over dropped frames like players'.

Players and actors may carry how they move and what they just did:

```json
"stance": "crouch", "speed": 4.5, "actions": ["reload"], "grenade": "frag"
```

`stance` is `stand`, `crouch`, `airborne` (jumping or falling), `vehicle`
or `dead`; `speed` horizontal game units a second (0 to 1000); `actions`
the one-shot actions since the previous frame, oldest first, at most 8, each
`grenade` (a throw), `melee`, `reload`, `jump`, `switch` (weapon swap) or
`pickup`; `grenade` the selected type, `frag` or `plasma`.

They may also carry, from senders that read them (packed format version 3):

```json
"vel": [1.5, -0.25, 0.0], "fire": 5, "body": 1.0, "shield": 0.5, "seat": 0,
"actions_ago": [1]
```

`vel` is the velocity in game units a second; `fire` the shots by tick, bit
k a shot k ticks before the frame's tick (a 32-tick window); `body` the body
vitality 0..1 and `shield` the shield 0..4 (over 1 is an overshield);
`seat` the seat index in its vehicle, at most 63; `actions_ago` how many
ticks before the frame's tick each of `actions` happened, one each, at most
900, and only beside `actions`.

From senders that read it (packed format version 4), they may carry the
animation they play:

```json
"anim": { "name": "combat:rifle:any:move_front", "frame": 12.5 }
```

`name` is the animation's name in the unit's graph, `[a-z0-9_:-]{1,64}`;
`frame` the frame in it, 0 to 32767, kept to a sixteenth.

A frame may also carry `hits`, `bsp`, `machines` and `machine_positions`:

```json
"hits": [ { "from": "p0", "to": "90070007", "weapon": "battle_rifle",
            "pos": [5.0, 6.0, 7.0], "amount": 0.75, "kind": "bullet" } ],
"bsp": 1,
"machines": [ { "node": "scarab_0", "id": "90090009", "kind": "scarab",
                "pos": [0.0, 0.0, 10.0], "forward": [1.0, 0.0, 0.0],
                "up": [0.0, 0.0, 1.0] } ],
"machine_positions": [ { "i": 7, "p": 128 } ]
```

`hits` is the damage dealt since the previous frame, oldest first, at most
64: `from` and `to` are `p<index>` for a player or an object datum in hex
(an actor's or vehicle's `id`), absent for nobody (falls); `weapon` the
weapon's tag stem when known, else the damage effect's; `pos` the victim's
position; `amount` body plus shield damage as fractions of the victim's
vitality (1 is a full body); `kind` `bullet`, `explosion`, `grenade`,
`melee`, `fall`, `collision`, `vehicle`, `turret`, `sentinel`,
`teleporter`, `guardians` or `unknown`; `ago` (optional) how many ticks
before the frame's tick it landed, at most 900. `bsp` is the structure BSP the game
has loaded (campaign switches between zones). `machines` are device
machines and other big scripted movers (doors, elevators, the scarab): those
that moved since the previous frame, all of them in a game's first frame,
at most 128. `node` is map.glb's `machines` child for it,
`<tag stem>_<scenario placement index>`, absent when the scenario did not
place it. `kind` is the `mach` tag stem. Older frames may carry an
additional `position` (0..1) on each entry.

`machine_positions` gives the changed device travel values independently
of the world poses. `i` is the scenario machine index (0..32767) and `p`
is its position byte (0 closed or at the start, 255 fully open or at the
end). Entries are sorted by index, at most 255, one per index. Packed
keyframes and recorded chunk starts can repeat the latest values. An
omitted entry holds the last value unless a `cut` starts a new run.

The server refuses unknown stances, actions and grenades, negative or
non-finite speeds and amounts, more than 8 actions, 64 hits or 128 machine
poses, invalid machine node names, and legacy positions outside 0..1.

A frame may also carry `objects`, the scenario's placed objects that are
not where the scenario put them (docs/live/live-telemetry.md, "Scenario
objects"):

```json
"objects": [ { "k": "bloc", "i": 12, "p": [3.0, 4.5, 1.0],
               "f": [0.0, 1.0, 0.0], "u": [0.0, 0.0, 1.0] },
             { "k": "netgame", "i": 3, "gone": true } ]
```

`k` is the placement's kind as map.glb's `scenario` node names it (`bipd`,
`vehi`, `weap`, `eqip`, `scen`, `mach`, `ctrl`, `lifi`, `ssce`, `bloc`,
`crea`, `netgame`), `i` its index in that kind's scenario block (at most
`0x7fff`). A moved object has `p` (finite) and non-zero `f` and `u`; a gone
one (destroyed, or a weapon someone picked up) has `gone: true` and none of
them. Everything not listed is at its placement. The server refuses more
than 2048 entries, entries not in (kind, index) order or repeated, and
entries that break those rules.

A frame may carry `cut`: a campaign checkpoint revert or restart happened
just before it, and the engine's clock came back to that tick (the game's
ticks keep counting; see below). Only the first frame of the new run has it.

A frame may carry `cinematic: true`: a campaign cutscene is playing. The
game sends such frames on its clock as ever, with the tick and `bsp` (and a
`cut` when one is due) but no players, actors, vehicles, hits, machines or
objects, and sends no kills meanwhile (docs/live/live-telemetry.md,
"Cinematics"). A viewer shows a cutscene and draws nothing from the frames
either side of it into it. Absent means false.

Shots, actions and hits of frames that are dropped (the 15 Hz throttle, a
full sink queue) go to the next delivered frame (`fire`, `actions_ago` and
hits' `ago` placed again against that frame's tick, those older than they
can say dropped, and a `cut` kept), and a recorded frame holds the values admitted by the throttle. The first
frame of a recorded chunk repeats the latest machine poses and travel.

Campaign plays as one game per mission (mode `campaign`, no scores): a
checkpoint revert or restart of the same mission continues it, and its
ticks keep counting from where the game had got to although the engine's
clock went back. Recordings from older senders run backwards there; a
reader takes their frames and events in log order and starts each run
after the one before (the web viewer's `GameClock`).

A game start (`POST /v1/games`, `GameStarted`) may also say what kind of
game it is; all five fields are optional and old senders leave them out:

| field | values | rules |
|---|---|---|
| `kind` | `custom`, `matchmaking`, `clankermatch`, `campaign` | `clankermatch` is a matchmade game against bots (docs/game/bots.md §8.2); `campaign` exactly when `mode` is; absent means `campaign` for campaign mode, else `custom` |
| `playlist`, `playlist_id` | hopper name (1-32 characters, no control characters), id `[A-Za-z0-9_-]{1,64}` | matchmaking and clankermatch only; with no `playlist`, the server names it from the served hopper file when `playlist_id` is a descriptor index there |
| `session_id` | the Live session, 16 lowercase hex digits (the game's 8 bytes in order) | matchmaking only; ranking finds the game's arbitration round by it ([Arbitration](#arbitration)) |
| `difficulty` | `easy`, `normal`, `heroic`, `legendary` | campaign only |
| `online` | boolean | campaign only: co-op partners on other machines (Live co-op, not sent yet); split-screen co-op is more than one player without it |

Game summaries and `GET /v1/games/{id}` always carry `kind` (inferred for old
games) and the others when the start had them.

Live frames go out on the stream at up to 15 Hz, and every frame sent is
recorded in the game's [frame chunks](#frame-chunks). A frame's `shots`
count since the previous frame sent (the sum over the frames the throttle
dropped, capped at 64 per player or actor); the same goes for `actions` and
`hits`. Games recorded before frame chunks have `GameFrame` events instead,
a frame at 1 Hz whose `shots`, `actions` and `hits` cover its whole second.

### Frame chunks

The server records a game's frames in chunks of ten seconds of game time
(`crates/live-proto/src/chunk.rs`): every frame sent in an aligned window
of 300 ticks, packed as one stream (a keyframe, then deltas; [packed
frames](live-telemetry.md#packed-frames)) and compressed with raw deflate.
A chunk is stored when a frame lands in the next window, and when the game
ends (its `GameEnded`, or the idle sweep ending it as `lost`); there is no
timer. A server that stops mid-window loses at most that window.

Each chunk is an immutable blob in the private blob store,
`frames/<game id>/<first tick>` (R2 on the Worker, the data directory
natively; the event store's blobs on a server with neither), and a
server-signed `FrameChunk` event in the game's stream is its index entry:

```json
{ "from_tick": 9000, "to_tick": 9298, "frames": 150, "bytes": 38211,
  "sha256": "…", "codec": "deflate", "raw_bytes": 41022, "versions": [4],
  "players": [0, 1, 2], "cuts": [9120], "hits": 37, "shots": 210 }
```

`versions` are the packed frame versions inside (a reader that knows none
skips the chunk), `players` every player index a frame lists, `cuts` the
ticks of frames that carry a `cut`, `hits` and `shots` totals. The
signature covers `sha256`, so a chunk is verifiable. Kills, scores and the
rest stay ordinary events: stats never read frames.

A chunk's bytes:

| field | bytes |
|---|---|
| magic | `H2CK` |
| format | `u8`, 1 |
| codec | `u8`: 0 stored, 1 raw deflate (RFC 1951) |
| raw length, frames, first tick, last tick | LEB128 each |
| body | the codec's bytes of a container of packed frame records |

A chunk carries packed records as they are, so a new packed frame version
changes nothing here. A chunk inflates to at most 16 MiB. `GET
/v1/games/{id}/chunks` lists a game's chunks and `GET
/v1/games/{id}/chunks/{first tick}` returns one, with the same visibility
check as the game. A seek reads the index once and fetches the one chunk
whose window holds the tick.

## Read endpoints

| method and path | returns |
|---|---|
| `GET /healthz` | `{ "ok": true, "position": <head> }` |
| `GET /.well-known/h2live.json` | server id, API base, server keys (security §4), `url`: the public base URL when known ([live-hosting.md](live-hosting.md)) |
| `GET /v1/accounts/{gamertag}/keys` | account public keys with validity ranges |
| `GET /v1/accounts` | accounts: `{xuid, gamertag, created_at, online}` |
| `GET /v1/accounts/{gamertag}` | one account plus `recent_games` (without the co-op games the reader may not see: [Co-op games](#co-op-games)) |
| `GET /v1/accounts/{gamertag}/friends` | friends with presence; pending requests in and out for the player and admins only ([Friends](#friends), below) |
| `GET /v1/presence` | who is online now: `{xuid, gamertag, state, game_id?, activity?, since, build?, capabilities?}`; a player in a game or session the reader may not see shows no `game_id` and `activity: "In a game"` ([Co-op games](#co-op-games), "Presence") |
| `GET /v1/coop/games?player=` | finished co-op campaign games, newest first ([Co-op games](#co-op-games), below) |
| `GET /v1/games?state=live\|ended&map=&map_id=&skulls=` | game summaries: `{game_id, map, mode, kind, variant, playlist?, playlist_id?, difficulty?, online?, team_play, started_at, ended_at?, players, teams?, map_id?, skulls?}`; `map` keeps games on that base map, modded or not, `map_id` one exact map (`none`: games with no `map_id`), `skulls` (comma-separated indices, `4,6`) games that had every one of them on; without the co-op games the reader may not see ([Co-op games](#co-op-games)) |
| `GET /v1/games/{id}` | summary plus scoreboard and the last 50 kills; 404, as for no game, for a co-op game the reader may not see ([Co-op games](#co-op-games)) |
| `GET /v1/games/{id}/chunks` | the game's [frame chunk](#frame-chunks) index: `{game_id, chunk_ticks, ended, items}`, `items` the `FrameChunk` data oldest first; empty for a game recorded before chunks |
| `GET /v1/games/{id}/chunks/{tick}` | one chunk by its first tick, `application/x-h2-chunk`; checked against its index entry's hash. Anonymous reads are `public, max-age=31536000, immutable` (it is never rewritten) |
| `GET /v1/games/{id}/frames?from_tick=&to_tick=` | a game recorded before frame chunks: its `GameFrame`s as an `application/x-h2-frames` container of packed keyframes (docs/live/live-telemetry.md, "Packed frames"), the next cursor in `x-next` and the head position in `x-position`. The JSON read (`format=json`) is gone: `400` |
| `GET /v1/maps/{map}/outline` | top-down map outline (see below), 404 until a console has uploaded it |
| `GET /v1/snapshot` | what a page needs on load, in one read at one log position: `{position, accounts, presence, live, ended, details, tail}`. `accounts` is `/v1/accounts?limit=1000`, `presence` is `/v1/presence`, `live` and `ended` are `/v1/games?state=…&limit=100`, `details` is `/v1/games/{id}` for every live game and the 40 newest ended ones, and `tail` is the last 300 log events up to `position` without `GameFrame` (as on the stream). Each part has the reader's visibility applied, as at its own endpoint ([Co-op games](#co-op-games)) |
| `GET /v1/events?from=&type=&stream=&limit=` | raw log, oldest first, at most 200 a page; without the events of co-op games and sessions the reader may not see ([Co-op games](#co-op-games), "The log"): a page can be short or empty with a `next`; without network addresses for anyone but the operator ([Network addresses](#network-addresses)); invitations, friend events, content and player reports and bans only to whom they concern ([Private events](#private-events)) |
| `GET /v1/titles/{title}/files/{name}` | raw file bytes, `ETag` = sha256 |
| `GET /v1/server` | the server's public facts: `{id, software, url, invite_only, private, federation, counts: {accounts, online, live_games, sessions}, position}` ([live-admin.md](live-admin.md) §5) |
| `GET /v1/stats/summary?window=today\|7d\|30d\|all` | server-wide totals, per-player rows, maps, weapons, playlists, skulls and hourly activity over a window ([Stats summary](#stats-summary), below) |
| `GET /v1/admin/status` | admins only (`admin.read` token): the facts plus uptime, tunnel, game art, admins, log, invites; the data directory only to this machine (live-admin.md §5) |
| `GET /v1/admin/usage?days=` | admins only (`admin.read`): the server's usage per UTC day, events by type, stored bytes ([Usage](#usage), below) |
| `GET /v1/admin/usage/accounts?days=&sort=&limit=` | admins only (`admin.read`): the accounts using the most ([Usage](#usage)) |
| `GET /v1/invites` | admins only (`admin.read` token): the invites admins made, newest first, open ones with their code (live-admin.md §5) |

### Caching

Set by `crates/live-server/src/caching.rs` on GET and HEAD responses whose handler set no `Cache-Control` of its own:

- `/assets/*` (built, named by content hash): `public, max-age=31536000, immutable`.
- The web app's HTML (`/`, every app route, and a missing asset answered with the app): `no-cache`.
- Anonymous reads under `/v1/` of `snapshot`, `games`, `accounts`, `presence`, `stats`, `events`, `teams` and `maps` that succeed: `public, max-age=0, s-maxage=5, stale-while-revalidate=30`, with a weak `ETag`. A matching `If-None-Match` gets a 304.
- Everything else under `/v1/` is `private, no-store`: any request with `Authorization` or an `auth` query mark (the web app adds `auth=1` to every read it sends with a token, because a CDN's cache key leaves out `Authorization`), any error, and every other route.
- Art (`/art/*`) and title files keep their own headers.

## Stats summary

`GET /v1/stats/summary?window=` (default `7d`) feeds the stats site's home
page. It is a projection of the game streams (`crates/live-server/src/stats.rs`),
kept in UTC-day buckets for the last 30 days, hour buckets for the same span,
and a running all-time bucket, so an answer merges at most 30 buckets and
never reads the log. Answers are cached until the log moves or the hour
turns. `window` is `today` (the current UTC day), `7d` or `30d` (that many
UTC days, today included) or `all`; anything else is 400.

```json
{ "window": "7d", "from": "2026-11-10T00:00:00Z", "to": "2026-11-16T15:04:05Z", "position": 1042,
  "totals":   { "games": 41, "multiplayer": 33, "campaign": 8, "matchmaking": 20, "custom": 13,
                "coop": 3, "kills": 1210, "deaths": 1302, "betrayals": 17, "suicides": 75,
                "campaign_deaths": 40, "player_seconds": 302400, "legendary": 2 },
  "last_24h": { ...the same counts, over the last 24 hours },
  "all_time": { ...the same counts, over the whole log },
  "players":  [ { "xuid": "...", "name": "Sneaky Pete", "games": 9, "multiplayer": 8, "campaign": 1,
                  "wins": 5, "kills": 120, "deaths": 88, "betrayals": 1, "best_spree": 9,
                  "seconds": 7200, "legendary": 1 } ],
  "maps":      [ { "map": "lockout", "map_id": "...?", "campaign": false, "games": 12, "kills": 480 } ],
  "weapons":   [ { "weapon": "battle_rifle", "kills": 310 } ],
  "playlists": [ { "playlist": "Team Slayer", "games": 14 } ],
  "skulls":    [ { "skull": 4, "games": 3 } ],
  "activity":  [ { "hour": "2026-11-16T14:00:00Z", "games": 3, "kills": 95 } ] }
```

The rules match the web app's: `kills` (kind `kill` with a killer other
than the victim), `deaths` (every death), `betrayals`, `suicides`, `wins`
and `best_spree` (scoring kills without dying, within one game) are
multiplayer's; campaign has `campaign_deaths` and `legendary` (levels
finished on Legendary: per game in totals, distinct levels per player).
A game counts on the day it started; kills on the day they happened; time
played (`player_seconds`, a player's `seconds`) on the day the game ended,
capped at 12 hours a game. `players` lists accounts only (a player with an
`xuid`), at most 500, by kills then games; totals count everyone.
Bots (`bot: true` on a game's player, [bots.md](../game/bots.md) §6) are
game-only: no row of their own (a rostered bot's xuid included), and a
knockout with a bot on either side counts in the totals, maps and weapons
but in nobody's row: a human's kill of a bot is not a career kill, nor a
death to a bot a career death. The game record keeps every one of them.
`activity` lists hours with any game starts or kills, within the window
and at most 30 days back. There are no headshots or medals: the events do
not carry them (the web app derives medals itself).

## Usage

The Live admin's usage screens (`/dash/admin/usage`,
[web-boundary.md](web-boundary.md) §5) read two endpoints, both for
admins only: an `admin.read` token from an admin account, or the
operator's (401 without one, 403 for anyone else). The numbers live in
`crates/live-server/src/usage.rs`; a read walks only the days it asks
for, in memory, and never the log.

**Where the counts come from.** Two books, merged when read:

- *From the log*, folded as events are applied and replayed at startup
  like the read model: `events` (every event, by its author; the
  server's own under no account), `up.events` (the bytes of each event an
  account signed, its JWS as received; a batch once), `messages.*`
  (`friend_requests`: `FriendRequested`; `invites`: `GameInviteSent`, for
  its sender), `games` (once per game, for the host and the players it
  began with or who joined), `sessions_hosted` (`SessionCreated`),
  `sign_ins` (`SignedIn`), `reports.sent` and `reports.received`
  (`PlayerReported`, for its `by` and its `target`), `stored.events` and
  `stored.content` (`ContentStored`, for its uploader). Other events the
  server signs for itself (`GameFrame`, `RankChanged`, `ReportReviewed`,
  `PresenceExpired`, ...) count as events by type and nothing more.
- *Metered* at the request, for bytes that never reach the log, each the
  accepted body's size as received: `up.frames` (live frames on
  `/v1/ingest`, text and packed), `up.content` (content uploads, title
  files, map outlines), `up.saves` (cloud save bodies), and
  `stored.saves` (a running total: a write adds its size less what it
  replaced, a delete takes its size off; saves written before counting
  began are not in it). These are counters in the store (the `usage`
  table; `usage.json` in a file store), added to in memory at once and
  written at most once a minute by whichever write or sweep comes next.
  Nothing runs on a timer, so a Durable Object still sleeps; a crash loses
  at most that minute. `h2 live store copy` does not copy them yet: the
  log's book rebuilds on the other side, the metered one starts again.

Days are UTC, from an event's signed time (`at`) or the server's clock
when metered, and are kept for 400 days; the write on a new day drops
older ones. Stored totals have no day and are never dropped. An account
is *active* on a day when anything above counts for it, other than what
it stores or reports made about it. The co-op relay is not metered: in
the Worker it is a Durable Object of its own, with no line to the hub's
store. A later sink (Cloudflare Analytics Engine) could take the metered
counts behind the same call, beside the in-process ones.

### `GET /v1/admin/usage?days=30`

`days` is 1 to 400 (default 30), the range ending today.

```json
{ "from": "2026-10-20", "to": "2026-11-18", "keep_days": 400,
  "days": [ { "day": "2026-11-18",
              "up": { "events": 1145, "frames": 133, "content": 0, "saves": 0 },
              "events": 5, "messages": { "friend_requests": 1, "invites": 0 },
              "games": 1, "sessions_hosted": 0, "sign_ins": 1, "reports": 0,
              "active_accounts": 1 } ],
  "totals": { "day": "", "up": { ... }, "events": 5, ...,
              "active_accounts": 1 },
  "accounts_active": 1,
  "event_types": [ { "type": "GameStarted", "count": 1 } ],
  "stored": { "events": 2290, "content": 0, "saves": 0 } }
```

`days` lists only days with something in them, oldest first (fill the
rest with zeros). `totals` sums them (its `active_accounts` is a sum of
days; `accounts_active` counts distinct accounts). `event_types` is
largest first, counts only. `stored` is what is stored now, over every
account.

### `GET /v1/admin/usage/accounts?days=30&sort=uploaded&limit=50`

Every account with anything in the range or anything stored, sorted
largest first by `sort` (`uploaded`, `stored`, `events`, `messages`,
`games`, `sessions_hosted`, `sign_ins`, `reports_sent`,
`reports_received`, `active_days`; ties by xuid), at most `limit` (1 to
500, default 50). `total` counts them all.

```json
{ "days": 30, "sort": "uploaded", "total": 1,
  "items": [ { "xuid": "00097a94c4494c72", "gamertag": "Alpha",
               "up": { "events": 1145, "frames": 133, "content": 0, "saves": 0 }, "uploaded": 1278,
               "stored": { "events": 1145, "content": 0, "saves": 0 }, "stored_total": 1145,
               "events": 2, "messages": { "friend_requests": 1, "invites": 0 }, "messages_total": 1,
               "games": 1, "sessions_hosted": 0, "sign_ins": 1,
               "reports_sent": 0, "reports_received": 0,
               "active_days": 1, "last_active": "2026-11-18" } ] }
```

## Write endpoints

Every write needs `Authorization: Bearer <self-issued JWT>` (security §3)
except account creation. Signed bodies are sent raw as a compact JWS with
`Content-Type: application/jose`. Event bodies are signed JWS (security §2):
`POST /v1/games/{id}/events` takes one JWS over the array.

| method and path | body | does |
|---|---|---|
| `POST /v1/accounts` | JWS signed by the new key over `{gamertag, jwk, invite?}` | create; 201 `{xuid, gamertag, id, recovery}` (`recovery`: the TOTP secret, `otpauth` URI and current code, live-security.md §1 Recovery), 409 if taken |
| `POST /v1/accounts/{gamertag}/recover` | `{"request": <JWS h2live-recover by the new key over {gamertag, jwk, aud, iat, nonce}>, "code": "123456"}` | sign in on a new device: add the new key with a current sign-in code; 201 `{xuid, gamertag, id}`; 403 wrong code, 429 locked out (live-security.md §1) |
| `GET`/`POST /v1/accounts/by-xuid/{xuid}/recovery` | the account's own `recovery.read` / `recovery.write` token | the sign-in code's secret and current code; `POST` makes a new one (set up or reset) |
| `GET /v1/accounts/by-xuid/{xuid}/signins` | the account's own `signins.read` token | where the account is signed in ([Sign-ins](#sign-ins)) |
| `DELETE /v1/accounts/by-xuid/{xuid}/signins/{kid}` | the account's own `signins.write` token | sign one other device out ([Sign-ins](#sign-ins)) |
| `POST /v1/accounts/by-xuid/{xuid}/signins/sign-out-others` | the account's own `signins.write` token | sign out every device but the caller ([Sign-ins](#sign-ins)) |
| `PUT /v1/titles/{title}/files/{name}` | raw bytes, operator token | publish a title file (max 2 MiB) |
| `POST /v1/events` | one compact JWS (an event on a stream the author may write) | append; 201 `{position}` |
| `POST /v1/invites` | none with an operator token (`?days=&code=`); or a JWS `InviteCreated` event `{jti, exp}` by an admin account with its `invites.write` token, bare or as `{"event": <jws>, "code": "..."}` | create a single-use invite; 201 `{invite, custom, jti, exp, position}`: `invite` is the 25-character code or the custom one (live-admin.md §5, "Invite codes people type") |
| `POST /v1/invites/{jti}/revoke` | JWS `InviteRevoked` event `{jti}` by an admin | end an open invite an admin made |
| `POST /v1/games` | `GameStarted` data without `game_id` | starts a game; returns `{game_id}` |
| `POST /v1/games/{id}/events` | `[{type, data}]` | append game events in order |
| `PUT /v1/maps/{map}/outline` | outline JSON | store a map outline |
| `POST /v1/local/signin` | `{"once": token}`, no token | embedded server and loopback peers only: answers once with the browser account entry `h2 play --live-local` offered; otherwise 404 (live-admin.md §4) |

## Sign-ins

Where an account is signed in, and signing any other device out. The API
and the log say "recovery" for what people are shown as the **sign-in
code** (the rotating six-digit TOTP code, live-security.md §1); the field
and path names stay.

A sign-in is one of the account's current keys. Each device has its own:
a browser makes one at sign-up or when it signs in with a code, a copy of
the game (and `h2 live account create`, whose accounts file the game
reads) the same. Every request token is signed by that key (security
§3), so signing a device out is revoking its key.

`GET /v1/accounts/by-xuid/{xuid}/signins` with the account's own
`signins.read` token:

```json
{"items": [
  {"kid": "Xc0…", "kind": "browser", "name": "Chrome on macOS", "via": "signup",
   "created_at": "2026-11-01T18:30:00Z", "last_seen": "2026-11-18T12:00:00Z", "current": true},
  {"kid": "p9Q…", "kind": "game", "name": "Halo 2", "via": "code",
   "created_at": "2026-11-15T20:02:11Z", "last_seen": null, "current": false}
], "position": 4211}
```

- `kind`: `browser`, `game` (the game or the CLI), `other`, or `unknown`
  for a key from before devices were named that hasn't listed its own
  sign-ins since (doing so names it, in memory, from that request).
- `name`: from the User-Agent of the request that made the key
  (`POST /v1/accounts` or `/recover`): browser and OS for a browser,
  "Halo 2" for the game. The server keeps it in its `KeyAdded` as
  `device`, sealed like the sign-in code secret (HKDF label
  `h2live-device-seal-v1`), because the log is public.
- `via`: `signup` (the key made the account), `code` (signed in with a
  sign-in code) or `key` (added by another of the account's keys).
- `last_seen`: the last request the key signed, since the server (or the
  Worker's hub) started; kept in memory only, `null` until then.
- `current`: the key that signed this request. It is listed first, then
  the most recently used.

`DELETE /v1/accounts/by-xuid/{xuid}/signins/{kid}` and
`POST /v1/accounts/by-xuid/{xuid}/signins/sign-out-others`, with the
account's own `signins.write` token, revoke the key or every key but the
caller's. The server appends one `KeyRevoked {kid, by, via: "signout"}`
per key on the account stream, signed by itself (`by` is the kid of the
key that asked), all in one append, and answers once they are applied:
200 `{"revoked": [kid, ...], "position": n}` (`revoked` is empty when
there was nothing else to sign out). Revoking the caller's own key is 409
(sign out on that device instead); a kid that is not a current key of
the account is 404; another account's token is 403.

A revoked key's next request is refused: every token check looks the key
up in the read model (security §3), so `authenticate` answers 401 `token
key is unknown or revoked` from the request after the append, in a
native server and in the Worker alike, and after a restart (the
revocation is in the log). An open WebSocket closes when its token
expires (at most 60 s) because a fresh one is refused. The device signs
in again with the gamertag and a sign-in code.

Signing others out is open to a banned account (`signins.read` and
`signins.write` are not barred), so someone locked out can still end a
stolen sign-in. Writes share the per-author write rate limit.

## Friends

Friendship is event sourced like everything else, on the account streams.
A request is directional; once accepted, the friendship is mutual, and
either side can end it. Each event names both accounts by XUID and is
written by the acting account on **its own** account stream, signed with
its key and sent like other account events (`POST /v1/events`, token with
`lxm: events.write`):

| type | data | written by | does |
|---|---|---|---|
| `FriendRequested` | `{from, to}` | `from` | asks `to`; pending until answered |
| `FriendAccepted` | `{from, to}` | `to` | accepts; the two are friends from this event's `at` |
| `FriendDeclined` | `{from, to}` | `to` | declines; the request is gone |
| `FriendRemoved` | `{by, friend}` | either of the two | ends the friendship, or cancels a pending request between them |

The server refuses (`invalid` or `conflict`) a request to oneself or to
an account that does not exist, a second request while one is pending in
either direction or the two are already friends, an answer on any account
but the request's recipient or to a request that is not pending, and a
removal without a friendship or pending request between the two.

**`GET /v1/accounts/{gamertag}/friends`** is everything a friends list
needs in one document, so clients enumerate with one call. The friendships
are public like the other reads; the pending requests (`requests_in`,
`requests_out`) are filled in only for the player themselves (a `read`
bearer of that account), admin accounts and the operator. Anyone else,
anonymous included, gets both lists empty, so the anonymous response the
edge caches holds no requests.

```json
{ "friends": [
    { "xuid": "0009a1b2c3d4e5f6", "gamertag": "Sneaky Pete",
      "friends_since": "2026-11-15T20:14:03.120Z",
      "presence": { "state": "playing", "game_id": "7f3c...", "activity": "Co-op: Outskirts", "since": "..." } } ],
  "requests_in":  [ { "xuid": "...", "gamertag": "...", "at": "..." } ],
  "requests_out": [ { "xuid": "...", "gamertag": "...", "at": "..." } ],
  "position": 1042 }
```

The lists are sorted by gamertag. `presence` is the friend's current
presence and is absent when the friend is offline; a pending request
carries no presence (the two are not friends yet), just the other account
and when the request was made. `friends_since` is the `FriendAccepted`
event's `at`.

`activity` is an optional short line the game sets on a presence event
("Co-op: Outskirts"): 1 to 64 characters, not blank, no control
characters (`live_proto::valid_activity`); a bad one refuses the event with
400. It rides `SignedIn` and `PresenceChanged`, is served in `GET
/v1/presence` and in a friend's `presence`, and is absent when unset. A
presence event without it clears the previous one.

`build` and `capabilities` say what software the player's console runs:
its git short hash (7 to 12 lowercase hex digits, `live_proto::valid_build`)
and up to 16 `name/version` strings of at most 32 characters
(`live_proto::valid_capabilities`; `variant-ext/1` applies a host's variant
options, docs/game/game-options.md section 3). A bad one refuses the event
with 400. They ride `SignedIn` and `PresenceChanged`, are served in `GET
/v1/presence` and in a friend's `presence` to every reader (they name
software, not where the player is), and are absent when unset.

The game side's friends enumeration makes exactly this call with the
signed-in account's gamertag and reads `friends` (gamertag, xuid,
presence); the CLI is `h2 live friend add|accept|decline|remove
<gamertag>` and `h2 live friend list`, and agents can query the same
document with `h2 live q friends <gamertag>`. Friend events reach
`/v1/stream` readers like every event (watch `FriendRequested` and
`FriendAccepted` for the toast), and a client that misses some resumes
from the snapshot's `position`.

## Teams

Teams use signed account events on `team-<id>` streams. IDs are sixteen hex
digits; clients generate an ID before creation (our clients use a random
48-bit value under `000a`). The server applies the shared `live-proto::team`
rules before appending, then derives the roster from the event log on replay.
Writes use `POST /v1/events`, `lxm: events.write`, the account's token and its
signed event, as friends do. The event's author is the acting member; callers
cannot choose another actor in the data. The stream ID, data ID, event type
and operation must agree.

| type | data (`id` is the team ID) |
|---|---|
| `TeamCreated` | `{operation:"create", id, properties, member, max_members}` |
| `TeamPropertiesChanged` | `{operation:"properties", id, properties}` |
| `TeamDeleted` | `{operation:"delete", id}` |
| `TeamMemberRecruited` | `{operation:"recruit", id, target:{xuid,gamertag}, properties}` |
| `TeamRecruitAccepted` | `{operation:"answer", id, accept:true}` |
| `TeamRecruitDeclined` | `{operation:"answer", id, accept:false}` |
| `TeamMemberPropertiesChanged` | `{operation:"member_properties", id, user, properties}` |
| `TeamMemberRemoved` | `{operation:"remove", id, user}` (also used for leaving) |

Team properties are `{name,description,motto,url,data}`; omitted strings and
data default to empty. Names must be nonblank and fit 15 UTF-16 units; the
other strings fit 255 units, and custom data is an array of at most 100 bytes.
Member properties are `{privileges,data}` with the same byte limit. A team
contains `id`, `creator`, `properties`, `max_members`, `created_at`, `members`,
`recruits` and `deleted`; rosters map XUIDs to
`{identity:{xuid,gamertag},properties,joined_at}`. Times are RFC 3339.

The creator is the one leader (`0xffffffff`). Permission bits are delete=1,
modify-team=2, modify-member=4, remove-member=8 and recruit=16. Halo 2 uses
`0x1c` for officers and `0x10` or zero for lower ranks. Members cannot grant
rights they do not have, create a second leader, demote/remove the creator,
or delete a team they do not lead. Members can leave; the leader must delete
the team. Names are unique ignoring case, a team reserves at most 100 active
and recruited places, and an account may have eight teams including pending
recruits. A deleted ID remains reserved, but its name becomes available.
Team writes are serialized through validation and projection to enforce these
limits across streams.

Recruit targets must be registered accounts with matching XUID/gamertag.
Recruitment atomically appends a server-signed `TeamRecruitReceived` event to
`account-<recruit-xuid>` with `{team_id,to,from}` (`from` is the recruiting
author). It appears on the ordinary event feed. The pending roster is the
current invitation state: only its recipient can accept or decline; a member
with remove permission can cancel it. Acceptance moves the recruit into the
member roster, decline removes it, and deletion ends all memberships.

| read | response |
|---|---|
| `GET /v1/teams?limit=50&cursor=<id>` | `Page<Team>`, active teams in ID order, limit 1..100 |
| `GET /v1/teams/{id}` | `{team,position}`, or 404 for missing/deleted |
| `GET /v1/accounts/{gamertag}/teams` | `Page<Team>` including pending recruits, at most eight, no next page |
| `GET /v1/accounts/by-xuid/{xuid}/teams` | same account list by XUID |
| `GET /v1/accounts/by-xuid/{xuid}` | `{xuid,gamertag}` for canonical recruit lookup |

Reads use the normal public/private read policy (`lxm: read` on a private
server). Team failures add a `team_error` enum to the ordinary error body,
which the HLE maps to the corresponding XDK HRESULT. Invalid data is 400,
missing teams/users 404, membership/permission failures 403, and conflicts or
limits 409. Rejected writes append nothing.

CLI commands take `--server <base-url>` and `--as <local-gamertag>`:
`h2 live team create <name>`, `list`, `show <id>`, `edit <id>` with
`--name`, `--description`, `--motto` or `--url`, `delete <id>`,
`recruit <id> <gamertag>` (optional `--privileges`), `accept <id>`,
`decline <id>`, `member <id> <gamertag> --privileges <mask>`,
`remove <id> <gamertag>` and `leave <id>`. Masks accept decimal or `0x` hex.
`h2 live q teams [gamertag]` and `h2 live q team <id>` read the same views.

## Sessions

Session advertisements are separate from recorded games. The stream is
`session-<id>`; the server chooses a random eight-byte online-peer XNKID and
uses its 16 lowercase hex digits as the id. There is no exchange key in the
public API. HLE keys stay local while sockets use unencrypted host traffic.

| endpoint | authentication | result |
|---|---|---|
| `POST /v1/sessions` | account token, `lxm: sessions.write` | 201 `{session_id, session, position}` |
| `POST /v1/sessions/{id}/events` | host token, `lxm: sessions.write`; batch JWS | 201 `{position}` |
| `GET /v1/sessions` | public, or `read` on a private server | `{items, next, position}` |
| `GET /v1/sessions/{id}` | same read rule | `{session, position}`; 404 if unknown |

Create takes `addr` (IPv4 or null), `port`, `host_address` (36 XNADDR bytes),
`slots` (`public_filled`, `public_open`, `private_filled`, `private_open`),
`attributes` (an object keyed by decimal attribute ID), `flags` (default
`custom`, also `hopper`, `ranked` or `coop`), and optional `hopper_id`, `map`, `name`,
`language`. Attribute values are `{type: "integer", value: <u64>}`,
`{type: "string", value: "..."}`, `{type: "blob", value: [bytes]}` or
`{type: "null"}`, and optional `visibility` (`public`, the default and
omitted on the wire; `friends`; `invite`) and `map_id` (64 hex digits,
exactly which map: see "Custom maps" below; omitted when none) and `skulls`
(the host's multiplayer skulls, ascending indices, only 4 and 6; omitted
when none; docs/game/skulls.md "In multiplayer") and `variant_ext`
(`{block, key}`: the game variant's extension block for the host's next
game, 72 lowercase hex digits, and the variant's key, 16; omitted when the
host sends none; docs/game/game-options.md section 3). The ID's datatype bits must agree. Limits are 16 total slots,
64 attributes, 400 UTF-16 units per string, 800 bytes per blob, 256 bytes per
metadata string and 32 open advertisements per host. Unknown input fields are
rejected; addresses must be unicast IPv4 and ports 1..65529.

The server writes a server-signed `SessionCreated` with the full session. Its
`host`, `host_xuid` and `host_gamertag` come from the authenticated account;
`endpoint` contains `{addr, port, source}`. An explicit address has source
`declared`. When `addr` is null, the observed address comes from
`policy.public.client_ip`: `CF-Connecting-IP` is trusted only with a configured
Cloudflare tunnel and a loopback peer, otherwise the TCP peer wins. Unknown
or IPv6-only observations require an explicit IPv4 address. XNADDR bytes are
retained; the HLE result encoder substitutes this endpoint in its address/port
fields.

Only the authenticated host may append events, signed with its registered key:

- `SessionUpdated`: optional slots, attributes (full replacement), name, map,
  visibility, `map_id`, `skulls` or `variant_ext` (absent keeps the current
  one; `skulls: []` turns them all off; a `variant_ext` replaces the last).
  A server from before `skulls` refuses an update that has it, so the HLE
  sends it only when the host's set changed; the same for `variant_ext`.
- `PlayerJoined`: `{xuid, slot?}`; the account must exist and not be in the
  roster, which is bounded to 16. Slot is an optional display string.
- `PlayerLeft`: `{xuid}`; the account must be in the roster.
- `SessionDeleted`: `{reason: "closed" | "host_left"}`; must be last in a batch.

Batches are validated in order before any append. Stream, author, signature,
key, five-minute timestamp window, nonce replay and payload checks apply.
Closed sessions reject writes with 409. Unchanged updates are successful but
add no event. These streams are reserved: `POST /v1/events` cannot bypass their
host checks or forge creation/expiry. `PlayerJoined/Left` are available to the
host API; this HLE slice does not infer those events from UDP traffic.

Presence pings refresh the host's open advertisements in memory. Five minutes
without a ping or changed session event produces a server-signed
`SessionExpired`. Pings are not persisted; after restart the deadline comes
from the last recorded event. Deleted/expired sessions remain readable by ID
and replay from the log, but are absent from the default list.

Lists show a session only to the readers its visibility allows: `public` to
everyone, `friends` to the host and the host's friends, `invite` to the host
alone. The reader is the account of an optional `read` bearer (checked when
present, even on a public server); without one, only public sessions. A
listed session must also be listable: its endpoint was observed, or its
declared address is a local-network one. When a listed session is created
from a non-loopback IPv4, the observed address replaces any declared one
([live-custom-games.md](live-custom-games.md) §3–§4). Reading one session by
ID ignores visibility. `SessionUpdated` may also set `visibility`.

### Network addresses

A session holds its host's network addresses: the declared
`advertisement.addr` and `port`, the XNADDR in `advertisement.host_address`,
and the `endpoint` the server observed (often the player's public IP). They
are what a game joins by, so `GET /v1/sessions` and `GET /v1/sessions/{id}`
give them to a signed-in reader (a `read` bearer: the game's matchmaking
and join calls) and leave them out for an anonymous one.

The log never shows them to anyone but the operator (a server-key token).
Every other reader of `GET /v1/events`, `/v1/stream` and the snapshot's
event tail, anonymous, signed in or an admin account, gets each event with
every network address removed at any depth (`addr` and the `port` beside
it, `endpoint`, `host_address`, `xnaddr`, `ip`, `observed`). The stored log
is unchanged. A changed event carries `redacted: true` in its data and an
empty `jws` (no `jws_index`): its signature covers the data as stored, so
it would not verify against the redacted view; only the operator's read
can be checked against signatures. Today only `SessionCreated` carries
addresses. The web's signature check shows such an event as "redacted"
(addresses hidden, signature checked by the server only), not as unsigned.

### Private events

Some events in the log show only to the accounts they concern, on every
log read (`GET /v1/events`, `/v1/stream` replay and live, the snapshot's
event tail). The stored log is unchanged.

| Events | Who reads them |
|---|---|
| `GameInviteSent`, `GameInviteAnswered` | the invitation's sender and recipient |
| `FriendRequested`, `FriendAccepted`, `FriendDeclined`, `FriendRemoved` | the two accounts and the current friends of either |
| `ContentReported` | admins only |
| `PlayerReported`, `ReportReviewed` | admins only ([Player reports](#player-reports)) |
| `AccountBanned`, `AccountUnbanned` | the banned account ([Bans](#bans)) |
| `PlayerMuted`, `PlayerUnmuted` | the muting account ([Mute list](#mute-list)) |

Admin accounts and the operator read all of them; anonymous readers none,
so an anonymous read stays safe for the edge cache. Other readers get the
log without them, the same way as hidden co-op events (a page can be short
with a `next`). The game and the web read these through their own calls
(`/v1/accounts/by-xuid/{xuid}/mailbox` for the owner,
`/v1/accounts/{gamertag}/friends`, `/v1/admin/content` and
`/v1/admin/reports` for admins), not
the log. The friends read keeps friendships public and pending requests
for the player ([Friends](#friends)).

Lists accept `state` (default `open`), `hopper` (numeric ID), `map`, `host`
(author or canonical gamertag), `flags` (`custom`, `hopper`, `ranked` or
`coop`: `?flags=coop` lists co-op campaign sessions), `visibility`, and `attributes` (URL-encoded
JSON with exact value comparisons). Filters combine. `limit` is 1..1000, default 100; `cursor`
is the previous `next` string. Ordering is by session ID. `optimatch`
(URL-encoded JSON `{hopper, average, lowest?, highest?, party?}`) is the
game's decoded OptiMatch search ([live-matchmaking.md](live-matchmaking.md)
§7): it keeps sessions on that playlist whose level window and party room
fit, orders them by closeness to `average`, and returns one page with a
null `next`. CLI queries are
`h2 live q sessions [--state open|deleted|expired] [--hopper ID] [--map NAME]
[--host GAMERTAG]` and `h2 live q session ID`, with the usual JSON formats.

A `coop` session may carry a `campaign` (`{level, name?, difficulty}`: the
map file stem, the level's name as the game shows it, and 0 easy to 3
legendary); no other kind may. A `SessionUpdated` with `campaign` makes the
session co-op, and one with `flags: "custom"` drops it. Co-op sessions are
listed, filtered and hidden by `visibility` like any other (the game creates
them `friends` by default).

### Co-op games

Campaign co-op posts no game result, so finished co-op games are derived
from session events. A `coop` session that closed (`SessionDeleted` or
`SessionExpired`) after at least one guest joined is one finished game; a
session nobody joined is not. Nothing new is written.

`GET /v1/coop/games` (public, or `read` on a private server; an optional
`read` bearer says who reads, see below) returns
`{items, next, position}`, newest closed first. `limit` is 1..1000
(default 100); `cursor` is the previous `next`, the log position of the
closing event. `player` (an XUID or a gamertag) keeps the games that
account hosted or joined; an unknown gamertag gives an empty page. An item:

```json
{ "session_id": "8f3a09c2d1e47b65",
  "host": { "xuid": "0009a1b2c3d4e5f6", "gamertag": "Sneaky Pete" },
  "players": [ { "xuid": "0009a1b2c3d4e5f6", "gamertag": "Sneaky Pete" },
               { "xuid": "0009a1b2c3d4e5f7", "gamertag": "Moss" } ],
  "campaign": { "level": "03a_oldmombasa", "name": "Outskirts", "difficulty": 2 },
  "started": "2026-11-16T10:05:41.220Z",
  "ended": "2026-11-16T10:47:09.004Z",
  "end_reason": "closed",
  "visibility": "friends" }
```

`players` is the host first, then every guest that ever joined in join
order, including ones who left before the end. `started` is the first
guest's join time, `ended` the closing event's. `end_reason` is `closed`,
`host_left` or `expired`. `gamertag` is null for an account the server no
longer knows. The campaign is the one on the session when it closed.

**Who sees a finished co-op game.** `visibility` is the session's when it
closed. A `public` one shows to everyone. A `friends` one shows only to
the readers who were in it (the host or a guest) and the host's friends;
an `invite` one only to those who were in it. Admins (the operator's
token, or an admin account's) see every game. The reader is the account
of an optional `read` bearer, checked when present even on a public
server; without one, only public games show. The same rule holds for a
Live co-op campaign game's telemetry (`kind: "campaign"`, `online: true`):
when it starts, the server links it to the open `coop` session its host
(or, failing that, a player) is in and records that session's host and
visibility, and `GET /v1/games`, `GET /v1/games/{id}`, its `/frames`, `/chunks` and
an account's `recent_games` leave it out for readers who may not see it
(those who were in it are its players). A campaign game with no such
session stays public. While the game runs it follows its session: when the
host changes the session's visibility, the game's changes with it. Once
the game ends it keeps the visibility it had then.

**The log.** `GET /v1/events` and `/v1/stream` apply the same rule per
reader: a hidden game's events (its `GameStarted`, players, kills,
recorded `GameFrame`s and `GameEnded`) and its live frames go only to
readers who may see the game, and a friends-only or invite-only session's
events only to its host, whoever is or was in it, the host's friends
(friends-only) and admins. Positions are the log's own, so a filtered
reader sees gaps. `GET /v1/events` reads `limit` events of the log and
leaves out the hidden ones, so a page can be short or empty with a
`next`: read on until `next` is null. `/v1/stream` skips hidden events
but its place still moves past them, and `caught_up` names the log's
head. A stream reader says who it is with `?auth=1` and a first message
`{"kind": "auth", "token": "<read token>"}` (see Streams); without it, a
public server's stream is anonymous.

**Presence.** A player's game id and activity follow the same rule. In
`GET /v1/presence`, a friends list (`GET /v1/accounts/{gamertag}/friends`)
and the log's `SignedIn`/`PresenceChanged` events (`/v1/events`,
`/v1/stream`), a reader who may not see the player's game, or every
friends-only or invite-only session the player was in when it set that
presence (and, for the current presence, is in now), gets the player's
state with no `game_id` and `activity: "In a game"` (none when the
player said neither). The player itself and admins always get the real
values. Such a log event carries `redacted: true` in its data and an
empty `jws`: its signature covers the real data, so it is withheld.

**Stats.** `GET /v1/stats/summary` counts every game for admins. Everyone
else gets the public summary: a Live co-op game linked to a session is
left out while it runs and counted when it ends if it ended public. A
friend who may see a friends-only game still gets the public summary.

The HTTP service advertises endpoints; it does not relay game packets, create
a ranked match or traverse NAT. Direct internet play requires reachable UDP
base through base+6. A Cloudflare HTTP tunnel alone provides no gameplay path.

A `map_id` on a create or update is refused with 410 when an admin hid,
deleted or blocked that package or one of its layers, and with 403 when the
session's `flags` are `hopper` or `ranked` and the id is neither a retail
map's (from the operator's retail list) nor pinned on the session's
`hopper_id` (`custom` and `coop` sessions take any map). A `map_id` the
server never saw is fine in a custom game: retail maps and local edits have
ids too.

## Custom maps

Custom and modded maps are shared as content: the layers (file patches,
content packs, user-made maps, preview images) by SHA-256, and the package
manifest by `MapId` ([maps.md](../game/maps.md) §1, §5). Retail map bytes are never
stored: players use their own disc for the base.

| endpoint | authentication | result |
|---|---|---|
| `PUT /v1/content/{sha256}` | account token, `lxm: content.write`; the raw bytes | 201 `{sha256, size, whole_map, stored: true}`; 200 `{..., stored: false}` if already kept |
| `GET /v1/content/{sha256}` | public, or `read` on a private server | the bytes (`ETag` the hash, cached immutably); 404; 410 if taken down |
| `POST /v1/content/{sha256}/report` | account token, `lxm: content.write`; `{reason}` | 201 `{ok, position}` |
| `PUT /v1/maps` | account token, `lxm: content.write`; a signed `MapPublished` event (compact JWS) | 201 `{map, missing, position}`; 200 when the author republishes it |
| `GET /v1/maps?sort=&base=&author=&category=&q=&limit=&cursor=` | read rule | `{items: [summary], next, position}`, newest first or most downloaded first (see "The mod repository") |
| `GET /v1/maps/{map_id}` | read rule; a `read` token counts a download | `{map: summary + manifest, missing, position}`; 404; 410 if taken down |
| `GET /v1/maps/{map_id}/image` | read rule | the preview (PNG or JPEG, its content type); 404 if none |
| `POST /v1/maps/{map_id}/report` | account token, `lxm: content.write`; `{reason}` | 201 `{ok, position}` |
| `PUT /v1/maps/{map_id}/credit` | admin or operator token, `lxm: content.moderate`; a credit `{authors?, release?, released?, source_url?, notes?}` | 200 `{ok, position, map: summary}`; `{}` clears the admin's credit; 403 for others; 404 if not published; 400 when malformed |
| `PUT /v1/maps/{map_id}/details` | admin or operator token, `lxm: content.moderate`; details `{description?, images?: [{sha256, caption?, source_url?, credit?, licence?}], contents?, features?, pack?}` | 200 `{ok, position, map: summary}`; `{}` clears the admin's details; each image must be stored content that is a PNG or JPEG; 403 for others; 404 if not published; 400 when malformed |
| `GET /v1/maps/{map_id}/images/{n}` | read rule | the details' `n`th image (from 0), its content type; 404 if none |
| `GET /v1/mappacks` | read rule | `{items: [{id, name, description?, authors?, released?, source_url?, images?}], position}` |
| `PUT /v1/mappacks/{id}` | admin or operator token, `lxm: content.moderate`; a pack `{name, description?, authors?, released?, source_url?, images?}`, or `null` to clear | 200 `{ok, position}`; ids are 1 to 64 of a-z, 0-9 and -; 400 when malformed |
| `GET /v1/mappacks/{id}/images/{n}` | read rule | the pack's `n`th image; 404 if none |
| `GET /v1/hoppers/pins` | read rule | `{pins: [{hopper_id, map_ids}], position}` |
| `GET /v1/hoppers/clankers` | read rule | `{hoppers: [{hopper_id, playlist_id, players, teams, ranks_as?}]}`: the served file's clanker hoppers (live-matchmaking.md §6.8) |

Uploads (`PUT /v1/content`) stream to `<data>/tmp`, hashed as they
arrive, and the SHA-256 of the body must be the path's (400 otherwise).
The blob is then kept once, as `content/<first two hex digits>/<sha256>`
in the server's private blob store (`<data>/content/...` by default, or
an R2 bucket with `--blobs s3://BUCKET`, [live-hosting.md](live-hosting.md#free-hosting-sqlite-and-cloudflare-r2)),
and never changed. It is always served through the server, never by a
redirect (the game's client follows none, and a takedown must be
immediate); a server-signed `ContentStored`
event (`content-<sha256>` stream; `{sha256, size, uploader}`) records it.
A body starting with a map's `daeh` header is a whole map; anything else
is a layer (a patch, pack or image). Both are capped at 95 MiB by
default (413 over either; `h2 live serve --content-layer-mib N
--content-map-mib N`): the largest whole MiB under the 100 MB request
body Cloudflare passes to a Worker (`live_server::content::EDGE_BODY_BYTES`;
the edge refuses larger bodies with its own 413 before the Worker runs),
so the native server and the Worker take the same uploads. A layer can
be large: Night Ops' delta over Containment is 68 MiB. Each account stores at most
1 GiB (`--content-quota-mib N`), counting what it uploaded first and not
deleted (413 over it). Uploads and publishes are rate limited per account
(a burst of 20, then one every two seconds; 429). A hash on the operator's
retail list is refused (403), and so is a hash an admin blocked (410). A
server with no data directory stores nothing (503).

A package is published as an event the author signs with their own key,
like the account's other events: stream `package-<map_id>`, type
`MapPublished`, data `{manifest, image?, features?, gametypes?, version?, category?, source_url?, credit?}`. `manifest` is the package
manifest as JSON (the TOML's tables and keys, with `[[patch]]` and
`[[pack]]` as the arrays `patch` and `pack`); `image` is the SHA-256 of the
preview, uploaded like a layer, given exactly when the manifest names one.
`category` (one of `gameplay`, `weapons`, `vehicles`, `visuals`, `maps`,
`other`) and `source_url` (an `http` or `https` URL of at most 300
characters, where the mod came from) are the repository's display data,
outside the `MapId` like the manifest's name (400 when malformed). So are
the in-game Mods page's (docs/game/maps.md §5, "A mod's page"):
`features` (what the mod changes, each once, of `vehicles`, `weapons`,
`objects`, `textures`, `shaders`, `scripts`, `geometry`, `ai`;
`h2 live maps import` derives them), `gametypes` (each once, of `slayer`,
`ctf`, `assault`, `oddball`, `koth`, `juggernaut`, `territories`; none:
any) and `version` (one line, at most 24 characters); each optional and
left out when empty. A server from before them refuses a document that
has them (the document allows no unknown keys): update the server first.
`credit` is who made the mod when the publisher did not: `{authors: [..],
release?, released?, source_url?, notes?}`, `authors` the original
creators as the release credits them (at most 12, each 1 to 64 characters
on one line), `release` the original release's name (at most 120),
`released` its date (`YYYY`, `YYYY-MM` or `YYYY-MM-DD`), `source_url` as
above, `notes` at most 500 characters. Without it the manifest's
`[package] author` (one name) and `source_url` stand in. Servers before
credits refuse a document that has it (unknown field: 400).
The server checks the manifest as the runtime does ([maps.md](../game/maps.md) §7,
"Manifest checks"), computes `MapId::of` and requires the stream to be that
id's; refuses a manifest with no layers (only a retail base: 403) or naming a
retail hash (403); refuses it while any layer or the image is not stored
(409, naming the files); and checks the image is at most 512 KiB and starts
with a PNG or JPEG signature. The first publisher of a `MapId` is its
author (another account: 409). The usual signature, key, five-minute clock
and nonce checks apply.

A summary is `{map_id, name, description?, author: {xuid, gamertag}, base:
{map, sha256?, derived}, layers: [{sha256, size, kind: "patch"|"pack"}],
size, image?, features?, gametypes?, version?, geometry?, published_at,
position, downloads, credit?, category?, source_url?, credits?}`: `size` is the layers' bytes (what a player
without them downloads), `image` the preview's hash, `geometry` the
manifest's `[result] geometry` (set when the mod changed the map's
geometry), `position` the `MapPublished` event's log position, `downloads`
how many accounts fetched the manifest. `author` is the account that
published the package (its key signed the manifest), never who made the
mod: that is `credits`, an admin's correction (below) if there is one,
else the publisher's `credit`, else the manifest's `[package] author`;
`credit` is its authors on one line (`A`, `A & B`, `A, B & C`) for
readers that know only that, and `source_url` is the credit's when it
has one. An admin's details (`PUT /v1/maps/{id}/details`) add `images`
(`[{sha256, caption?, source_url?, credit?, licence?}]`, served at
`/v1/maps/{id}/images/{n}`), `contents` (the report's lines) and `pack`
and `pack_name`; they replace `description` and `features` when set, and
the first image is `image` when the package has no preview. A list page
then has `packs: [{id, name, ...}]` for the packs its items name; a
map's own answer has `report` (the whole contents report:
`{lines, classes, swaps, stats, placements, scripts}`) and `pack_info`.
`missing` lists
layers the server no longer holds (deleted by an admin). Lists leave out
taken-down packages. Servers before the repository send no `position` or
`downloads`; read them as 0.

### The mod repository

The web's Mods pages (the stats site's "Mods" tab) and the game's Xbox
Live menu browse the same list. The exact request, for clients:

```
GET /v1/maps?sort=new&limit=20
GET /v1/maps?sort=new&limit=20&cursor=<the previous page's next>
```

```json
{
  "items": [
    {
      "map_id": "<64 hex>", "name": "Lockout Snipers",
      "description": "...", "author": {"xuid": "0009...", "gamertag": "Modder"},
      "credit": "who made it", "category": "weapons",
      "source_url": "https://...",
      "credits": {"authors": ["who made it"], "released": "2005-03",
                  "source_url": "https://..."},
      "base": {"map": "lockout", "sha256": "<64 hex>", "derived": false},
      "layers": [{"sha256": "<64 hex>", "size": 412345, "kind": "patch"}],
      "size": 412345, "image": "<64 hex>",
      "published_at": "2026-11-18T10:00:00Z",
      "position": 4211, "downloads": 17
    }
  ],
  "next": "4211",
  "position": 4300
}
```

- The id is `map_id` and the cursor is `next`, as everywhere in this API.
  `next` is null on the last page (one shorter than `limit`); a page as
  long as `limit` gives one, which may lead to an empty page.
- `sort`: `new` (the default: newest first, by `position`) or `popular`
  (most `downloads` first, ties newest first). Anything else is 400.
- Filters, all optional, empty ones ignored: `base` (the base map's name,
  any case), `author` (the publisher's gamertag; one no account has
  matches nothing), `category` (400 when not one of the six), `q` (a
  case-insensitive substring of the name, description, credited authors
  or release name, base map name or publisher's gamertag).
- `limit` 1 to 1000 (default 100). `cursor` is opaque: for `new` it is the
  last item's position (`"4211"`), for `popular` its downloads and
  position (`"17:4211"`); one of the other sort's form is 400.
- **New since last time** needs no per-account state: a client keeps the
  highest `position` it has seen and counts the items above it as new.
  `position` is the package's own, so a republish by its author (same
  `MapId`) does not move it; a package an admin deleted and its author
  published again is new again.
- **Downloads.** `GET /v1/maps/{map_id}` with `Authorization: Bearer
  <token>` (an account's `read` token; tokens are single use, so a fresh
  one per request) counts that account once per package: the first time,
  the server appends a server-signed `MapDownloaded` event (stream
  `package-<map_id>`, data `{map_id, by}`) and answers with the new count.
  Without a token, or with one that does not check out on a public server,
  the read is answered and counts nothing (the web never sends one there).
  A private server needs the token anyway. `MapDownloaded` events, like
  reports, show in the log to admins only. The count is kept by the log's
  projection, replayed at startup, so both homes (native and the Worker)
  count the same way, with no background task.
- The picture is `GET /v1/maps/{map_id}/image` when `image` is set, else
  the base map's art.
- **Details and packs.** `PUT /v1/maps/{map_id}/details` (the same
  tokens) appends a server-signed `MapDetailed` event (stream
  `package-<map_id>`, data `{map_id, details?, by?}`): a description of
  up to 4000 characters, up to 8 images already uploaded as content (each
  checked to be a PNG or JPEG), a contents report, a features list and a
  pack id. `{}` clears it. `PUT /v1/mappacks/{id}` appends `MapPackSet`
  (stream `mappack-<id>`, data `{id, pack?, by?}`); `null` clears it.
  Both are display data only, like the credit. `h2 live maps details
  --file docs/game/mod-details.toml` applies a file of them, and `h2 live
  maps analyze` writes the contents reports.
- **Credits.** `PUT /v1/maps/{map_id}/credit` (an admin's account token or
  the operator's, `lxm: content.moderate`) sets who made a published
  package, over what its publisher said: the body is a credit as above
  (trimmed; blank fields dropped). The server appends a server-signed
  `MapCredited` event (stream `package-<map_id>`, data `{map_id, credit?,
  by?}`, public) and answers with the new summary. An empty body object
  (`{}`) clears it, and the publisher's credit shows again. The manifest
  the publisher signed, its `MapId` and every session on it are untouched:
  this is display data only. The correction is kept by `MapId`, so a
  package deleted and published again keeps it. `h2 live maps credit`
  sends it, one package by flags or many from a file
  (`docs/game/mod-credits.toml`).

The client side (`h2 live maps publish DIR`, `h2 live maps import FILE`,
`h2 live maps list`, and the join-time download) is in
[maps.md](../game/maps.md) §5, "As built" and "Importing a mod".

## Player reports

A player reports another, from the game's player-details screens (the Live
HLE's `XOnlineFeedbackSend`, [live.md](live.md) "Client HLE: player
feedback") or any client:

| endpoint | authentication | result |
|---|---|---|
| `POST /v1/accounts/by-xuid/{xuid}/report` | account token, `lxm: reports.write`; `{kind, reason?, game_id?}` | 201 `{ok, position, duplicate: false}`; 200 `{ok, position, duplicate: true}` for a repeat |

`kind` is one of `gamertag`, `gameplay`, `screaming`, `harassment`,
`lewdness` (complaints), `good_attitude`, `good_session` (commendations),
or `type_10`, `type_11`, `type_12` (complaints the game offers whose XDK
names are not confirmed; see [live-boundary.md](live-boundary.md)).
`reason` is the reporter's words, at most 500 characters (the game sends
none); `game_id` the game they were in.

- A player can't report themselves (400); an unknown XUID is 404; the
  operator's token is 403 (reports come from accounts).
- A repeat of a report still open (same reporter, player and kind) appends
  nothing and answers 200 with the first report's position. Once an admin
  has reviewed it, the same complaint can be filed again. Commendations are
  never reviewed, so each counts once per pair of players.
- Each account files at most ten reports at once, then one every six
  minutes (429), on top of the usual per-method write limit. Repeats do not
  count.

The report is a server-signed `PlayerReported` event (`{target, by, kind,
reason?, game_id?}`) on `reports-<target xuid>`, readable by admins only
([Private events](#private-events)). Admins review reports through
`/v1/admin/reports` ([live-admin.md](live-admin.md) §8). No one else learns
of a report: not the reported player, and not the reporter what became of
it.

The two commendation kinds are counted in public:

| endpoint | authentication | result |
|---|---|---|
| `GET /v1/accounts/{gamertag or xuid}/commendations` | as any read | `{xuid, good_attitude, good_session}`: how many players commended the account, by kind |

## Bans

Admins ban accounts ([live-admin.md](live-admin.md) §9). While a ban is in
force, every token the account presents for a scope other than `read`,
`recovery.read`, `signins.read`, `signins.write`, `storage.read` and `log.read` is refused with 403
`{"error": "banned", "message": "this account is banned until <until>: <reason>"}`
(or "banned: <reason>" for a ban until lifted). Clients treat `banned`
as final for the session; the Live HLE reports it to the game as
`0x80151200`. The player reads their own ban:

| endpoint | authentication | result |
|---|---|---|
| `GET /v1/accounts/by-xuid/{xuid}/ban` | `read` token of the account itself or an admin | `{banned: false}` or `{banned: true, ban: {id, reason, until?, at}}` |

The ban events, `AccountBanned` and `AccountUnbanned` on
`bans-<xuid>`, are server-signed; nobody posts them by hand.

## Mute list

The players an account has muted (docs/live/live-voice.md §3.6), kept by
the server so the list follows the player. The game's voice mute matrix is
built from it, and a mute holds both ways: neither player hears the other.

| endpoint | authentication | result |
|---|---|---|
| `GET /v1/accounts/by-xuid/{xuid}/mutes` | `read` token of the account itself or an admin | `{items: [{xuid, gamertag}], position}`, oldest first |
| `PUT /v1/accounts/by-xuid/{xuid}/mutes/{target}` | the account's own token, `lxm: mutes.write` | 201 `{ok, position, changed: true}`; 200 `{ok, changed: false}` when already muted |
| `DELETE /v1/accounts/by-xuid/{xuid}/mutes/{target}` | as `PUT` | 200 `{ok, position, changed: true}`, or `{ok, changed: false}` when not muted |

- A player can't mute themselves (400); an unknown target is 404; another
  account's list, or the operator, is 403.
- A list holds at most 250 players, the size of the game's
  `XOnlineMutelistGet` buffer; one more is 409.
- Each change is a server-signed `PlayerMuted` or `PlayerUnmuted` event
  (`{by, target}`) on `mutes-<by>`, readable by the muting account, admins
  and the operator ([Private events](#private-events)). Nobody posts them by
  hand.
- The dashboard lists the signed-in player's mutes on their own screen
  with Unmute, and offers Mute or Unmute on other players' screens. The
  game reads the list at sign-in, so a change made there reaches it then.

## Cloud saves

Each account's own save storage ([live-cloud-saves.md](live-cloud-saves.md)):
private on every server, versioned, and kept by content hash. `{xuid}` is
the owner's; any other caller is refused.

| endpoint | authentication | result |
|---|---|---|
| `GET /v1/accounts/{xuid}/storage` | the owner's token, `lxm: storage.read` | `{items: [item], used_bytes, quota_bytes, item_bytes}` (tombstones included) |
| `GET /v1/accounts/{xuid}/storage/{kind}/{name}` | the owner's token, `lxm: storage.read` | the bytes; `ETag: "<version>"`, `X-H2-Version`, `X-H2-Modified`, `X-H2-SHA256`; 404 if absent or deleted |
| `PUT /v1/accounts/{xuid}/storage/{kind}/{name}` | the owner's token, `lxm: storage.write`; `If-Match: <version>`, `X-H2-SHA256: <hex>`, `X-H2-Modified: <unix ms>` (optional); the raw bytes | 201 (new) or 200 `item`; 409 `{error: "conflict", message, current: item or null}` |
| `DELETE /v1/accounts/{xuid}/storage/{kind}/{name}` | the owner's token, `lxm: storage.write`; `If-Match: <version>` | 200 the tombstone; 409 as for `PUT`; 404 if absent or already deleted |

An item is `{kind, name, version, sha256?, size, modified, stored_at,
deleted?}`: `version` starts at 1 and rises by one with each write or
delete, `modified` is the writer's own time of the change (Unix
milliseconds; the server's time when the writer gives none), `stored_at`
the server's (RFC 3339). A tombstone has `deleted: true`, no `sha256` and
size 0.

- `kind` is `save` (U:'s files) or `settings` (one profile's settings); any
  other is 404. `name` is 1 to 4 `/`-separated segments of printable ASCII
  without `" * + , : ; < = > ? \ | %`, never `.` or `..`, each at most 64
  bytes, 250 in all (400 otherwise); percent-encode it in the URL.
- `If-Match` is required on writes: the item's current version (a
  tombstone's too), `0` for an item that never existed, bare or quoted as an
  ETag. Anything else is a 409 carrying the current item, checked both
  before and after the body is read.
- The body's SHA-256 must be `X-H2-SHA256` (400 otherwise). Bytes are kept
  once per account by hash under `storage/<xuid>/blobs/` in the private
  blob store (`<data>/storage/...`, or the private bucket), and removed
  when no item names them.
- Caps (`Policy::storage`, `StorageLimits`): 8 MiB per item, 64 MiB per
  account counting live items only (a replacement counts its new size), and
  2048 items per account, tombstones included; 413 over any. Writes share the
  per-author write rate (429). A server with no data directory keeps none
  (503); a server older than this answers 404 for the list.
- Responses are `Cache-Control: private, no-store`. Nothing about cloud
  saves goes into the event log or the stream.

## Co-op relay

**`GET /v1/sessions/{id}/relay`** is a WebSocket that hands each binary
message from one member of a co-op session to the other
([live-coop.md](live-coop.md), section 8). It exists for two machines that
cannot reach each other directly; the direct path is UDP on the XNet base
port + 4.

- The first message is `{ "kind": "auth", "token": "<jwt>" }` with
  `lxm: sessions.relay`. A missing, wrong or expired token closes the
  socket with 4401, as on the other sockets. Sending a fresh `auth`
  message later extends the socket; it must be the same account's.
- The session must be open and created with `flags: "coop"`. The account
  must be its host, or one whose invitation to this session was answered
  `accept`. Otherwise the socket closes with 4403. Membership is checked
  again every few seconds, so a closed session ends its relay.
- A room has two members. The same account connecting again replaces its
  older socket (which closes with 4409); a third account gets 4409.
- Each member receives `{ "kind": "peer", "present": true|false }` when
  the other arrives or leaves.
- A binary message is at most 1200 bytes (close 4413). A socket may send
  240 messages a second with a burst of 480 (`Policy::relay_rate`; close
  4429).
- The server does not read, store or log message contents, and it does
  not queue for an absent member: a message with nobody to take it, or
  with more than 256 waiting for a slow member, is dropped. The co-op
  protocol repeats what was lost. Nothing is appended to the event log.

## Game-invite mailboxes

These are gameplay invitations, separate from the operator's account-registration
invite codes at `POST /v1/invites`. Both gameplay events use `POST /v1/events`,
one compact JWS and an `events.write` bearer token, on `mailbox-<recipient xuid>`.

| event | signed data | authority |
|---|---|---|
| `GameInviteSent` | `{from, session_id}` | A signed-in account inviting another existing account to its own open session |
| `GameInviteAnswered` | `{by, session_id, invite_id, answer}` | The mailbox owner answering that exact pending invitation |

`invite_id` is the sent event's global position. Answers are `accept`, `decline`,
`remove`, or `block`. Accept requires the session still be open; the other
answers may clear a stale invite. Block also refuses later invitations from
that sender; no unblock endpoint is included here. Duplicate pending invitations
to the same session are conflicts. At most 100 invitations to open sessions may
be pending per recipient. Unknown fields, sender/recipient spoofing and foreign
or closed sessions are refused. Timestamp/key/signature/nonces and rate limits
follow the ordinary event rules. Validation and append share the session-write
lock, preventing duplicate concurrent answers or a race with session deletion.

`GET /v1/accounts/by-xuid/{xuid}/mailbox` always requires a `read` token signed
by that recipient, including on a public server. It returns
`{items, next: null, position}` with at most 100 pending invitations to open
sessions, newest first. Each item is
`{id, from, to, gamertag, session_id, sent_at, answer: null}`; `sent_at` is Unix
seconds. Sender names come from the account projection. Answered or
closed/expired-session invitations disappear from this list. The projection
rebuilds from the log on restart. As with the rest of this service, raw events
follow the server's general public/private log policy; mailbox authorization is
not a promise of encrypted or secret event contents.

The HLE fetches this mailbox with the current account's key. Successful delivery
is not acceptance or a joined player: an accepted invite becomes a local launch
record, and actual connection uses FindFromID and the guest's QoS/UDP path.
`PlayerJoined` remains a host-reported session event, not an inferred side effect
of answering an invitation. No message attachments, notification acknowledgments,
ranked matcher or UDP relay are added by these endpoints.

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

`?auth=1` makes the first client message `{ "kind": "auth", "token":
"<read token>" }` on a public server too (a `--private` server always asks
for it), and the stream then sends what that account may see: the events
and frames of a friends-only or invite-only co-op game or session go only to
readers allowed to see it ([Co-op games](#co-op-games)). On a public server
the token's expiry does not close the socket. Without `auth`, a public
stream is anonymous and leaves those events out.

`?frames=packed` sends frames as binary messages instead: each a container
of packed frame records (docs/live/live-telemetry.md, "Packed frames"), one
stream per game, batched up to 100 ms (the server batches every game's
frames once a tick and shares the batch among readers). A reader that joins
or falls behind first gets each game's frames since its last keyframe. Each
stream's frames come in order, each once: never a frame older than one
already sent, nor a delta without its base. Events and `caught_up` stay
JSON text messages.

**`GET /v1/ingest`** for the game. The first message is
`{ "kind": "auth", "token": "<jwt>" }`. Then the client sends
`{ "kind": "frame", "frame": {...} }` and
`{ "kind": "events", "game_id": "...", "events": [{type, data}] }`. Frames
may also come as binary messages: containers of packed frame records, one
stream per socket. The server fans frames out to readers, records them in
[frame chunks](#frame-chunks), and appends events.

A `GameFrame` recorded before frame chunks holds `{tick, source, packed}`:
`packed` is the frame as one packed keyframe, base64url without padding, so
the event's signature covers the packed bytes. The oldest hold the JSON
frame plus `source`; both read the same through `/frames`.

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
- `GET /art/game/catalog.json`: the playlist builder's catalog of the
  game's maps, playlist texts and built-in variants
  ([live-admin.md](live-admin.md), "The catalog"), 404 likewise.
- `GET /art/game/map/<stem>/map.glb` (`model/gltf-binary`),
  `.../views.json` (`application/json`) and `.../views/<id>.png`: a
  map's or campaign level's geometry and pre-drawn views, named by the
  manifest's `maps` ([live-art.md](live-art.md)), 404 likewise. Converted
  Halo 1 maps are there too, keyed by the name games report them by
  (`h1_<map>`, or the level's), their entries marked `"game": "halo1"` with
  a `"name"`.

```json
{ "version": 1,
  "source": "<sha256 of the disc's map files>",
  "images":  { "<key>": { "file": "rank/12.png", "width": 28, "height": 26 } },
  "aliases": { "gametype/slayer": "gametype/0" },
  "emblem":  { ...colouring rules, below... },
  "palette": ["#ffffff", "..."],
  "skipped": { "<key or prefix>": "why it is missing" },
  "maps":    { "<stem>": { "geometry": "map/<stem>/map.glb",
                           "views": "map/<stem>/views.json",
                           "kind": "multiplayer", "bsps": 1,
                           "game": "halo1", "name": "..." } } }
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
| `title/image` | the game's title image, from the executable's `$$XTIMAGE` section (128x128): what the original dashboard showed as the game's tile |
| `title/save-image` | the game's save image, from `$$XSIMAGE` (64x64): the picture of a save that has no `SaveImage.xbx` of its own |

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
   codes in Conventions (plus `unauthorized`, `forbidden`, `gone` (410), `rate_limited`,
   `too_large`, `unimplemented`).
5. **Presence states** are `online`, `away` and `playing` (`game_id` set);
   `SignedOut` and `PresenceExpired` mean offline.
6. **`GameEnded.winner`** is `{ "team": n }` in team games,
   `{ "player": index }` otherwise, or null for a draw or quit.
7. **`GameEnded.players`** is the final scoreboard: `[{index, score, kills,
   deaths, assists}]`.
8. **Frames** are [frame chunks](#frame-chunks) (`GET
   /v1/games/{id}/chunks`). Games recorded before them are read with `GET
   /v1/games/{id}/frames?from_tick=&to_tick=&cursor=`, packed, at most 600
   per page, oldest first.
9. **`PlayerKilled`** carries `pos`, the victim's position.
10. **Who writes a game** (2026-11-18). The server trusts the host's
    record. Only the host ends the game (`GameEnded`; 403 from anyone
    else, who leaves with `PlayerLeft`). A `PlayerJoined` from anyone but
    the host must be the author's own account, not a bot (403). An xuid
    can be in a game once at a time: a `NewGame` naming one twice, or a
    `PlayerJoined` for an xuid already in, is a 400.
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
  ping; a game with no events or frames for 5 minutes ends as `lost`. That
  `GameEnded` carries the last tick seen, but its `at` (the game's
  `ended_at`) is when the server gave up, so a game's length is
  `(GameEnded.tick - GameStarted.tick) / 30`, not `ended_at - started_at`.
  A `lost` game that never got past its start tick (the console went quiet
  before its first frame) has length 0: its wall clock is only the
  server's wait, and stats count none of it as time played.
- **`recent_games`** on an account is the latest 10.
- **Account creation** payload is `{gamertag, jwk, invite?, aud, iat}`;
  `aud` is the server id from the well-known document and `iat` must be
  within 5 minutes of the server's clock. The fixed test vector is
  `account_request_vector` in `crates/live-proto/src/sign.rs`.

## Ranks

Per-playlist ranks ([live-matchmaking.md](live-matchmaking.md) §8): a
level 1..50 and experience for each player in each ranked playlist. The
server writes them, as server-signed `RankChanged` events on
`rank-<xuid>`, when a finished ranked matchmaking game ends; nobody else
may write those streams. A standing is
`{level, experience, highest, games, wins}`. Reads are public, or need
`read` on a private server.

| endpoint | result |
|---|---|
| `GET /v1/accounts/{who}/ranks` | `{xuid, ranks: {playlist: standing}, position}`; `who` is a gamertag or XUID; 404 if unknown |
| `GET /v1/accounts/{who}/ranks/{playlist}` | `{xuid, playlist, standing, history, position}`: `history` is the last 100 changes, newest first, `{game_id, at, position, before, after}` |
| `GET /v1/ranks/{playlist}?limit=` | `{playlist, items, position}`: `{xuid, gamertag, ...standing}`, with `bot: true` on a bot's row, by level then experience; `limit` 1..1000, default 100 |
| `GET /v1/rank-stats?leaderboards=&xuids=&session=` | `{items, position}`: the game's stats read. `leaderboards` are playlist ids (a descriptor's `+0x08`), `xuids` players, both comma-separated, at most 32 and 64 (400 beyond). One `{xuid, leaderboard, playlist, standing}` per player per leaderboard that is a ranked playlist in the served hopper file; a player with no games there is level 1; other leaderboards have no rows. Optional `session` (16 lowercase hex digits, else 400): a made-up bot of that session's latest roster, on the roster's playlist, answers its roster level |

`playlist` is the hopper descriptor index, as sessions and games name it.
A leaderboard is the playlist id the hopper file gives that descriptor,
which is what the game's `XOnlineStatRead` names.
A game is ranked only for the players who registered its arbitration
round ([Arbitration](#arbitration)); each `RankChanged` names that round
as `round`, `{session}-{round}`, and a round ranks one game. Only the
session host's own record is ranked (`GameStarted.host` is the session's
`host_xuid`), by the latest round it registered before the game started.
A bot
rostered in the game's session (`POST /v1/games/bots`) needs no
registration: a persistent bot is ranked, its `RankChanged` marked
`bot: true`; a made-up bot is an opponent at its roster level and gets
none. A bot's rank pages answer by its xuid or gamertag, with `bot: true`.

## Arbitration

The game's `XOnlineArbitration` calls, posted by the Live HLE once for
each account signed in on the console
([live-matchmaking.md](live-matchmaking.md) §8.4). Each takes a
`games.write` token of the account. `session` and `round` are 16
lowercase hex digits. The server writes `ArbitrationRegistered`,
`ArbitrationExtended` or `ArbitrationReported`, server-signed, on
`round-<session>-<round>`, with `{session_id, round_id, xuid}` and the
call's fields; `xuid` is the token's account. The session must be one the
server knows (404 otherwise). When a game is ranked by a round, the
server also writes `ArbitrationUsed` `{session_id, round_id, game_id}`
there, whether or not anyone's standing moved; the round ranks no other
game. Nobody writes these through `/v1/events`.

| endpoint | body | result |
|---|---|---|
| `POST /v1/arbitration/{session}/{round}/register` | `{duration, flags}` | 201 with `{position}`; 200, writing nothing, when the account already registered |
| `POST /v1/arbitration/{session}/{round}/extend` | `{duration}` | 201; 409 unless the account registered the round |
| `POST /v1/arbitration/{session}/{round}/report` | `{flags, procedures}`: the stat procedure ids, at most 1000 | 201; 200 when the account already reported; 409 unless it registered |

An id that is not 16 lowercase hex digits, or another call name, is a
404.

