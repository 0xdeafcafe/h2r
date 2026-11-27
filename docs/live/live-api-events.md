# Live API: events and read endpoints

Part of [Live service API](live-api.md). Covers: Events; Read endpoints.

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
| `account-<xuid>` | `InviteGrantSet`, `InviteGrantRevoked` | `xuid`, `gamertag`, `by`, plus `open_cap` for `InviteGrantSet` or `keep_open: true` when revoking a current admin's explicit grant ([Server invites](live-api-sessions.md#server-invites)) |
| `account-<xuid>` | `FriendRequested`, `FriendAccepted`, `FriendDeclined`, `FriendRemoved` | `from`/`to`, or `by`/`friend` ([Friends](live-api-social.md#friends), below) |
| `gamertag-<lower>` | `GamertagReserved` | `xuid` |
| `title-<id>-file-<name>` | `TitleFilePublished` | `title_id`, `name`, `sha256`, `size` |
| `presence-<xuid>` | `SignedIn`, `PresenceChanged`, `SignedOut`, `PresenceExpired` | `xuid`, `state`, `game_id?`, `activity?`, `build?`, `capabilities?` (below) |
| `game-<id>` | `GameStarted` | `game_id`, `console`, `map`, `mode` (`campaign`/`multiplayer`), `variant?`, `team_play`, `score_limit?`, `time_limit_s?`, `players`, `kind?`, `playlist?`, `playlist_id?`, `difficulty?`, `online?` (below) |
| `game-<id>` | `PlayerJoined`, `PlayerLeft` | `player` (below) / `index` |
| `game-<id>` | `PlayerKilled` | `tick`, `victim`, `killer?`, `weapon?`, `headshot?`, `pos?`, `kind` (`kill`/`suicide`/`betrayal`/`world`; `betrayal` only for a teammate killed in a team game or campaign, `live_proto::game::is_betrayal`, and readers settle a stored `kill` or `betrayal` again by the teams, since older hosts sent free-for-all kills as betrayals) |
| `game-<id>` | `ActorKilled` | `tick`, `unit` (salted unit object datum), `character?` (salted character tag datum), `killer?` (credited player slot), `weapon?` (damage-source key). Published by the host. |
| `game-<id>` | `FirefightScore` | `tick`, `player` (credited roster slot), `points`, `reason` (`kill`/`assist`/`player_kill`; `player_kill` is a player of the other team killed, with two teams), `class?` (the kit class killed; on a kill only). Published by the host ([Firefight](../game/firefight.md#scoring-and-stats)). |
| `game-<id>` | `FirefightEnded` | `tick`, `set` (the wave set's name), `teams` (1 or 2), `outcome` (one team: `won`/`lost`; two teams: `team_won`, `draw` or `lost`), `team?` (the winning team, on `team_won` only), `scores` (each team's points, one per team), `rounds_cleared`, `sets_cleared?` (sets played through, bonus round included; an endless match counts every set, a finite one is 1 when won; left out when 0), `lives_used`. Published by the host, once, before `GameEnded`. |
| `game-<id>` | `Superbounce` | `tick`, `player` (a slot of the game), `chain` (the bounce's place in its chain, 1 and up), `monster` (true for a bounce that launches well above a normal one, the `monster-bounce` medal; absent reads as false, and a host leaves it out when false). Published by the host, for every player it watches, bots included ([superbounce.md](../game/superbounce.md#recording)). |
| `game-<id>` | `Bounced` | `tick`, `player`, `chain` (the count of the chain it ended, 0 if none; absent reads as 0): a death falling out of or through the map while trying a bounce, beside its `PlayerKilled`. Published by the host. |
| `game-<id>` | `Teabag` | `tick`, `player` (the crouching player's slot), `victim` (the slot of the player whose body it was; not `player`): a teabag. Published by the host, for every player it watches, bots included ([teabag.md](../game/teabag.md#recording)). |
| `game-<id>` | `GameMedal` | `tick`, `player` (the earner's slot), `medal` (the id of one of Halo 2's own medals; anything else is 400): a medal the game itself awarded. Published by the host, for every award the game makes there, bots' included ([Medals](live-api-theater-stats.md#medals)). |
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
page, no career stats ([Stats summary](live-api-theater-stats.md#stats-summary)), no rank of their
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
end). Entries are sorted by index, at most 255, one per index. A
machine-aware packed keyframe is a complete baseline, including at a
chunk start. V8 keys carry that baseline in their embedded v7 key.
An omitted index on a keyframe has unknown travel. An omitted index on a
delta holds the last value. A `cut` clears the preceding run's state.

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

From senders on packed format version 6, a frame may carry
`projectiles` and `items`, and vehicles their `vel`:

```json
"projectiles": [ { "id": "e90c0001", "kind": "plasma_grenade",
                   "pos": [4.0, 5.0, 6.0], "vel": [3.0, 0.0, 2.5] },
                 { "id": "e90c0002", "kind": "plasma_grenade",
                   "pos": [1.5, 2.0, 0.5], "parent": "90070007" } ],
"items": [ { "id": "a2f00f00", "kind": "flag", "pos": [20.0, -12.0, 0.5],
             "forward": [1.0, 0.0, 0.0], "up": [0.0, 0.0, 1.0] },
           { "id": "a10d0001", "kind": "overshield", "equipment": true,
             "pos": [0.0, 3.0, 0.2], "forward": [0.0, 1.0, 0.0],
             "up": [0.0, 0.0, 1.0] } ]
```

`projectiles` are projectiles in flight slower than 100 game units a
second (grenades, rockets, plasma, needles; bullets live a tick or two and
are drawn from the shooter's `fire` and `aim`) or stuck to something, at
most 64: `id` the object datum (new for each throw or shot, gone when it
detonates), `kind` the `proj` tag stem or `unknown`, `pos` the world
position, `vel` game units a second (absent when stuck), `parent` what it
is stuck to (a player's, actor's or vehicle's id), and then `pos` is that
object's position. `items` are weapons and equipment lying loose that no
scenario placement or netgame spawn made (those are `objects`): dropped
guns, the flag, the ball, the bomb, at most 128; `kind` is the `weap` stem,
or with `equipment: true` the `eqip` stem. A held flag, ball or bomb is the
carrier's `weapon` (`flag`, `ball`, `assault_bomb`). A vehicle's `vel` is
its velocity like a unit's. The server refuses non-finite positions or
velocities, velocities over 1000, zero `forward`/`up`, ids or kinds outside
`[a-z0-9_]{1,32}`, a projectile that is its own parent, and more than 64
projectiles or 128 items.

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

A `POST /v1/games` request can include these optional fields:

| field | values | rules |
|---|---|---|
| `kind` | `custom`, `matchmaking`, `clankermatch`, `campaign` | `clankermatch` is a matchmade game against bots (docs/game/bots.md §8.2); `campaign` exactly when `mode` is; absent means `campaign` for campaign mode, else `custom` |
| `playlist`, `playlist_id` | hopper name (1-32 characters, no control characters), id `[A-Za-z0-9_-]{1,64}` | matchmaking and clankermatch only; with no `playlist`, the server names it from the served hopper file when `playlist_id` is a descriptor index there |
| `session_id` | the Live session, 16 lowercase hex digits (the game's 8 bytes in order) | hosted multiplayer games, including custom lobbies; ranking finds the game's arbitration round by it ([Arbitration](live-api-details.md#arbitration)) |
| `difficulty` | `easy`, `normal`, `heroic`, `legendary` | campaign only |
| `online` | boolean | campaign only: co-op partners on other machines (Live co-op, not sent yet); split-screen co-op is more than one player without it |

Game summaries and `GET /v1/games/{id}` carry `kind` (inferred for old
games) and the reported playlist and campaign fields. When a start names a
session, the server records `GameSessionLinked {game_id, session_id}` on the
operator-only `lobby-links` stream and appends `GameStarted` without the ID
in the same transaction. Lobby visibility changes leave the signed game
event intact. Readers obtain a joinable session from `GET /v1/games/{id}/join`.

Live frames go out on the stream at about 15 Hz per game on average.
Batched ingest can deliver a bounded burst. Every frame sent is
recorded in the game's [frame chunks](#frame-chunks). A frame's `shots`
count since the previous frame sent (the sum over the frames the throttle
dropped, capped at 64 per player or actor); the same goes for `actions` and
`hits`. Games recorded before frame chunks have `GameFrame` events instead,
a frame at 1 Hz whose `shots`, `actions` and `hits` cover its whole second.

### Frame chunks

The server records a game's frames in chunks of ten seconds of game time
(`crates/live/live-proto/src/chunk.rs`): every frame sent in an aligned window
of 300 ticks, packed as one stream (a keyframe, then deltas; [packed
frames](live-telemetry-packed.md#packed-frames)) and compressed with raw deflate.
A chunk is stored when a frame lands in the next window, and when the game
ends (its `GameEnded`, or the idle sweep ending it as `lost`); there is no
timer. A server that stops mid-window loses at most that window.

The private blob store uses `frames/<game id>/<first tick>-<sha256>` for
new chunks and reads older chunks under `frames/<game id>/<first tick>`.
The digest keeps both originals when a checkpoint rewind repeats a chunk
start. Readers can pass the signed index entry’s `sha256` as a query parameter to disambiguate a repeated first tick. The Worker uses R2 and the native server uses its data directory.
With neither configured, the event store holds the bytes. A server-signed
`FrameChunk` event in the game's stream records its index entry.

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
check as the game. The binary response is the stored blob byte for byte.
An explicit `?format=json` returns a JSON array of its decoded frames. The
query gives each representation its own URL in the edge cache. A seek reads
the index once and fetches the one chunk whose window holds the tick.

## Read endpoints

| method and path | returns |
|---|---|
| `GET /healthz` | `{ "ok": true, "position": <head> }` |
| `GET /.well-known/h2live.json` | server id, API base, server keys (security §4), `url` with the public base URL when known ([live-hosting.md](live-hosting.md)), `ingest_frame_versions` for supported live codecs and `ingest_frame_batch` for batched ingest |
| `GET /v1/accounts/{gamertag}/keys` | account public keys with validity ranges |
| `GET /v1/accounts` | accounts with `xuid`, `gamertag`, `created_at`, `online`, `admin` and optional `invite_grant: {open_cap}` |
| `GET /v1/accounts/{gamertag}` | one account plus `recent_games` (without the co-op games the reader may not see: [Co-op games](live-api-sessions.md#co-op-games)) |
| `GET /v1/accounts/{gamertag}/friends` | friends with presence; pending requests in and out for the player and admins only ([Friends](live-api-social.md#friends), below) |
| `GET /v1/presence` | who is online now: `{xuid, gamertag, state, game_id?, activity?, since, build?, capabilities?}`; a player in a game or session the reader may not see shows no `game_id` and `activity: "In a game"` ([Co-op games](live-api-sessions.md#co-op-games), "Presence") |
| `GET /v1/coop/games?player=` | finished co-op campaign games, newest first ([Co-op games](live-api-sessions.md#co-op-games), below) |
| `GET /v1/games?state=live\|ended&map=&map_id=&skulls=` | game summaries: `{game_id, map, mode, kind, variant, playlist?, playlist_id?, difficulty?, online?, team_play, started_at, ended_at?, players, teams?, map_id?, skulls?, medals?}` (`medals`: [Medals](live-api-theater-stats.md#medals)); `map` keeps games on that base map, modded or not, `map_id` one exact map (`none`: games with no `map_id`), `skulls` (comma-separated indices, `4,6`) games that had every one of them on; without the co-op games the reader may not see ([Co-op games](live-api-sessions.md#co-op-games)) |
| `GET /v1/games/{id}` | summary plus scoreboard, the last 50 kills, and the last 100 superbounces (`bounces`: `tick`, `player`, `chain`, `monster` when true, `position`, `at`) and bounce deaths (`bounced`: `tick`, `player`, `chain`, `position`, `at`), and the last 100 teabags (`teabags`: `tick`, `player`, `victim`, `position`, `at`), each oldest first and empty in a game with none; `joined_late`, the slots of players in the game who joined after its first kill (left out when none; [teabag.md](../game/teabag.md#the-medal)); `medals` as in the summary; 404, as for no game, for a co-op game the reader may not see ([Co-op games](live-api-sessions.md#co-op-games)) |
| `GET /v1/games/{id}/join` | `{session_id}` for the game's current reader. The value is null unless the running game's own custom lobby or linked co-op session is joinable. The game and lobby each follow their own visibility rules. Hidden games return 404 and responses are private and uncached. |
| `GET /v1/games/{id}/chunks` | the game's [frame chunk](#frame-chunks) index: `{game_id, chunk_ticks, ended, items}`, `items` the `FrameChunk` data oldest first; empty for a game recorded before chunks |
| `GET /v1/games/{id}/chunks/{tick}` | one chunk by its first tick, `application/x-h2-chunk`; checked against its index entry's hash. Anonymous reads are `public, max-age=31536000, immutable` (it is never rewritten) |
| `GET /v1/games/{id}/frames?from_tick=&to_tick=` | a game recorded before frame chunks: its `GameFrame`s as an `application/x-h2-frames` container of packed keyframes (docs/live/live-telemetry.md, "Packed frames"), the next cursor in `x-next` and the head position in `x-position`. `format=json` returns the decoded page as `{items,next,position}` |
| `GET /v1/maps/{map}/outline` | top-down map outline (see below), 404 until an admin has uploaded it |
| `GET /v1/snapshot` | what a page needs on load, in one read at one log position: `{position, accounts, presence, live, ended, details, tail}`. `accounts` is `/v1/accounts?limit=1000`, `presence` is `/v1/presence`, `live` and `ended` are `/v1/games?state=…&limit=100`, `details` is `/v1/games/{id}` for every live game and the 40 newest ended ones, and `tail` is the last 300 log events up to `position` without `GameFrame` (as on the stream). Each part has the reader's visibility applied, as at its own endpoint ([Co-op games](live-api-sessions.md#co-op-games)) |
| `GET /v1/events?from=&type=&stream=&limit=` | raw log, oldest first, at most 200 a page; without the events of co-op games and sessions the reader may not see ([Co-op games](live-api-sessions.md#co-op-games), "The log"): a page can be short or empty with a `next`; without network addresses for anyone but the operator ([Network addresses](live-api-sessions.md#network-addresses)); invitations, friend events, content and player reports and bans only to whom they concern ([Private events](live-api-sessions.md#private-events)) |
| `GET /v1/titles/{title}/files/{name}` | raw file bytes, `ETag` = sha256 |
| `GET /v1/server` | the server's public facts: `{id, software, url, invite_only, private, federation, counts: {accounts, online, live_games, sessions}, position}` ([live-admin.md](live-admin.md) §5) |
| `GET /v1/stats/summary?window=today\|7d\|30d\|all` | server-wide totals, per-player rows, maps, weapons, playlists, skulls and hourly activity over a window ([Stats summary](live-api-theater-stats.md#stats-summary), below) |
| `GET /v1/accounts/{who}/medals` | `{xuid, medals: {id: count}, position}`: an account's medals over every game it played; `who` is a gamertag or XUID; 404 if unknown ([Medals](live-api-theater-stats.md#medals)) |
| `GET /v1/admin/status` | admins only (`admin.read` token): the facts plus tunnel, game art, admins, log, invites; `started_at` and `uptime_s` on native servers only, the data directory only to this machine (live-admin.md §5) |
| `GET /v1/admin/usage?days=` | admins only (`admin.read`): Live API requests and usage per UTC day, events by type, stored bytes ([Usage](live-api.md#usage), below) |
| `GET /v1/admin/usage/accounts?days=&sort=&limit=` | admins only (`admin.read`): the accounts using the most ([Usage](live-api.md#usage)) |
| `GET /v1/invites` | `admin.read` token from an admin or the operator. All invites, newest first ([Server invites](live-api-sessions.md#server-invites)) |
| `GET /v1/invites/mine` | `invites.read` token from a caller with invite access. Only their open invites ([Server invites](live-api-sessions.md#server-invites)) |

### Caching

Set by `crates/live/live-server/src/caching.rs` on GET and HEAD responses whose handler set no `Cache-Control` of its own:

- `/assets/*` (built, named by content hash): `public, max-age=31536000, immutable`.
- The web app's HTML (`/`, every app route, and a missing asset answered with the app): `no-cache`.
- Anonymous reads under `/v1/` of `snapshot`, `games`, `accounts`, `presence`, `stats`, `events`, `teams`, `ranks`, `page-meta` and retail map outlines that succeed: `public, max-age=0, s-maxage=5, stale-while-revalidate=30`, with a weak `ETag`. A matching `If-None-Match` gets a 304.
- Everything else under `/v1/` is `private, no-store`: any request with `Authorization`, any error, and every other route.
- Art (`/art/*`) and title files keep their own headers.

Every tagged `/v1/` answer also carries `Vary: Authorization`, so a shared cache keyed on the URL never answers a request that bears a token from the anonymous entry. A stray `auth` query mark from an older web client is ignored: the `Authorization` header alone says who reads.
