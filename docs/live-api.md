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
| `account-<xuid>` | `KeyAdded`, `KeyRevoked` | `jwk`, `kid` (security §1) |
| `account-<xuid>` | `FriendRequested`, `FriendAccepted`, `FriendDeclined`, `FriendRemoved` | `from`/`to`, or `by`/`friend` ([Friends](#friends), below) |
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

A player may also carry `appearance`, read from a local player's signed-in
profile; it is left out when nothing is known, and so is any field in it:

```json
"appearance": { "primary": 5, "secondary": 9, "tertiary": 2, "quaternary": 11,
                "emblem_foreground": 30, "emblem_background": 7 }
```

`primary`..`quaternary` index the art manifest's 18-colour `palette`;
`emblem_foreground` (0..63) and `emblem_background` (0..31) index
`emblem/fg/<n>` and `emblem/bg/<n>`. Remote players have none yet, and there
is no rank (docs/live-telemetry.md, "Player appearance").

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
| `GET /.well-known/h2live.json` | server id, API base, server keys (security §4), `url`: the public base URL when known ([live-hosting.md](live-hosting.md)) |
| `GET /v1/accounts/{gamertag}/keys` | account public keys with validity ranges |
| `GET /v1/accounts` | accounts: `{xuid, gamertag, created_at, online}` |
| `GET /v1/accounts/{gamertag}` | one account plus `recent_games` |
| `GET /v1/accounts/{gamertag}/friends` | friends with presence, pending requests in and out ([Friends](#friends), below) |
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
| `POST /v1/local/signin` | `{"once": token}`, no token | embedded server and loopback peers only: answers once with the browser account entry `h2 play --live` offered; otherwise 404 (live-admin.md §4) |

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

**`GET /v1/accounts/{gamertag}/friends`** (public like the other reads)
is everything a friends list needs in one document, so clients enumerate
with one call:

```json
{ "friends": [
    { "xuid": "0009a1b2c3d4e5f6", "gamertag": "Sneaky Pete",
      "friends_since": "2026-11-15T20:14:03.120Z",
      "presence": { "state": "playing", "game_id": "7f3c...", "since": "..." } } ],
  "requests_in":  [ { "xuid": "...", "gamertag": "...", "at": "..." } ],
  "requests_out": [ { "xuid": "...", "gamertag": "...", "at": "..." } ],
  "position": 1042 }
```

The lists are sorted by gamertag. `presence` is the friend's current
presence and is absent when the friend is offline; a pending request
carries no presence (the two are not friends yet), just the other account
and when the request was made. `friends_since` is the `FriendAccepted`
event's `at`.

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
`custom`, also `hopper` or `ranked`), and optional `hopper_id`, `map`, `name`,
`language`. Attribute values are `{type: "integer", value: <u64>}`,
`{type: "string", value: "..."}`, `{type: "blob", value: [bytes]}` or
`{type: "null"}`. The ID's datatype bits must agree. Limits are 16 total slots,
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

- `SessionUpdated`: optional slots, attributes (full replacement), name or map.
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

Lists accept `state` (default `open`), `hopper` (numeric ID), `map`, `host`
(author or canonical gamertag), and `attributes` (URL-encoded JSON with exact
value comparisons). Filters combine. `limit` is 1..1000, default 100; `cursor`
is the previous `next` string. Ordering is by session ID. CLI queries are
`h2 live q sessions [--state open|deleted|expired] [--hopper ID] [--map NAME]
[--host GAMERTAG]` and `h2 live q session ID`, with the usual JSON formats.

The HTTP service advertises endpoints; it does not relay game packets, create
a ranked match or traverse NAT. Direct internet play requires reachable UDP
base through base+6. A Cloudflare HTTP tunnel alone provides no gameplay path.

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
- `GET /art/game/catalog.json`: the playlist builder's catalog of the
  game's maps, playlist texts and built-in variants
  ([live-admin.md](live-admin.md), "The catalog"), 404 likewise.
- `GET /art/game/map/<stem>/map.glb` (`model/gltf-binary`),
  `.../views.json` (`application/json`) and `.../views/<id>.png`: a
  multiplayer map's level geometry and pre-drawn views, named by the
  manifest's `maps` ([live-art.md](live-art.md)), 404 likewise.

```json
{ "version": 1,
  "source": "<sha256 of the disc's map files>",
  "images":  { "<key>": { "file": "rank/12.png", "width": 28, "height": 26 } },
  "aliases": { "gametype/slayer": "gametype/0" },
  "emblem":  { ...colouring rules, below... },
  "palette": ["#ffffff", "..."],
  "skipped": { "<key or prefix>": "why it is missing" },
  "maps":    { "<stem>": { "geometry": "map/<stem>/map.glb",
                           "views": "map/<stem>/views.json" } } }
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
  ping; a game with no events or frames for 5 minutes ends as `lost`. That
  `GameEnded` carries the last tick seen, but its `at` (the game's
  `ended_at`) is when the server gave up, so a game's length is
  `(GameEnded.tick - GameStarted.tick) / 30`, not `ended_at - started_at`.
- **`recent_games`** on an account is the latest 10.
- **Account creation** payload is `{gamertag, jwk, invite?, aud, iat}`;
  `aud` is the server id from the well-known document and `iat` must be
  within 5 minutes of the server's clock. The fixed test vector is
  `account_request_vector` in `crates/live-proto/src/sign.rs`.
