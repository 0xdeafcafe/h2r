# Live API: sessions, invites, sign-ins and hosts

Part of [Live service API](live-api.md). Covers: Sessions; Server invites; Sign-ins; Dedicated hosts; Dedicated hosts; Dedicated host tunnel.

## Sessions

Session advertisements are separate from recorded games. The stream is
`session-<id>`; the server chooses a random eight-byte online-peer XNKID and
uses its 16 lowercase hex digits as the id. There is no exchange key in the
public API. HLE keys stay local while sockets use unencrypted host traffic.

| endpoint | authentication | result |
|---|---|---|
| `POST /v1/sessions` | account token, `lxm: sessions.write`; or a lease token | 201 `{session_id, session, position}`; a leased host's also has `identifiers`, the seat and machine slots of its game (as `POST /v1/games` returns) |
| `POST /v1/sessions/{id}/events` | host token, `lxm: sessions.write`; batch JWS | 201 `{position}` |
| `GET /v1/sessions` | public, or `read` on a private server | `{items, next, position}` |
| `GET /v1/sessions/{id}` | same read rule | `{session, position}`; 404 if unknown |

Create takes `addr` (IPv4 or null), `port`, `host_address` (36 XNADDR bytes),
`slots` (`public_filled`, `public_open`, `private_filled`, `private_open`),
`attributes` (an object keyed by decimal attribute ID), `flags` (default
`custom`, also `hopper`, `ranked` or `coop`), and optional `hopper_id`, `map`, `name`,
`language`. Attribute values are `{type: "integer", value: <u64>}` (a
number up to 2^53 and a decimal string above it, since signed events use
canonical JSON whose numbers are doubles; a reader takes either),
`{type: "string", value: "..."}`, `{type: "blob", value: [bytes]}` or
`{type: "null"}`, and optional `visibility` (`public`, the default and
omitted on the wire; `friends`; `invite`) and `map_id` (64 hex digits,
exactly which map: see "Custom maps" below; omitted when none) and `skulls`
(the host's multiplayer skulls, ascending indices, only 4 and 6; omitted
when none; docs/game/skulls.md "In multiplayer") and `variant_ext`
(`{block, key}`: the game variant's extension block for the host's next
game, 72 lowercase hex digits, and the variant's key, 16; omitted when the
host sends none; docs/game/game-options.md section 3) and `rules` (the
SHA-256 of the rules IR blob the host's next game runs, 64 lowercase hex
digits; omitted when none; docs/game/rules-engine.md "Carrying a script")
and `firefight` (`{set, id, teams, difficulty, kit}`: the Firefight match
the host's next game plays, its wave set's name, the definition's SHA-256
as 64 lowercase hex digits, 1 or 2 player teams, difficulty 0 to 3 and
the kit version from 1; omitted when the next game is not Firefight;
docs/game/firefight.md "Live").
A session whose flags are `ranked` and whose `rules` is set names a saved
or built-in script that the ranked policy takes
(docs/game/rules-engine.md "Roles, powers and capabilities"): an unknown
hash is 400, and a script declaring a capability ranked refuses (any
`forge.*`) is 403, naming it. A `SessionUpdated` that sets `rules` is
checked the same way.
The ID's datatype bits must agree. Limits are 16 total slots,
64 attributes, 400 UTF-16 units per string, 800 bytes per blob, 256 bytes per
metadata string and 32 open advertisements per host. Unknown input fields are
rejected. Addresses must be unicast IPv4 and ports 1..65529.

An optional `browser` object holds discovery metadata. Each of its fields
can be omitted independently. `clankers` is a boolean for whether the host
plans bot seats. `game_type` is the multiplayer engine index. `variant`
is the host's full variant name. `map_category` names the map source as
`halo2`, `map_pack`, `mods` or `halo_ce`.

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
  visibility, `map_id`, `skulls`, `variant_ext`, `rules` or `firefight`
  (absent keeps the current one). Use `clear_map_id: true` when a host
  returns to retail, and `clear_firefight: true` when its next game is not
  Firefight (with `firefight` in the same update it is refused).
  `skulls: []` turns them all off. A `variant_ext` replaces the last.
  A server from before `skulls` refuses an update that has it, so the HLE
  sends it only when the host's set changed. The same applies to
  `variant_ext` and `rules`. An update with `browser` replaces the discovery metadata
  as a whole. `browser: {}` clears it.
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

Presence pings refresh, in memory, the open advertisements the host created
since its last `SignedIn` (`Model::touch_host_sessions` in
`crates/live/live-server/src/model.rs`); one left by an earlier run of the
game gets no refresh. A `SignedIn` starts a new run: presence `since` resets
to its time, while `PresenceChanged` keeps it. Five minutes
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

### Browser filters

`GET /v1/sessions` accepts an optional URL-encoded JSON object in
`filters`. Omit any field to leave that criterion unset.

| Field | Values |
|---|---|
| `kind` | `multiplayer` or `coop` |
| `clankers` | `true` with Clankers, `false` without |
| `game_type` | `1` CTF, `2` Slayer, `3` Oddball, `4` King of the Hill, `7` Juggernaut, `8` Territories, `9` Assault |
| `variant` | Full multiplayer variant name, matched case-insensitively after trimming |
| `map_category` | `halo2`, `map_pack`, `mods` or `halo_ce` |
| `has_space` | `true` requires a public opening and room in the advertised capacity. `false` leaves room unrestricted |
| `campaign` | `halo2` or `halo_ce`, inferred from a known co-op level |
| `difficulty` | Co-op difficulty, `0` Easy through `3` Legendary |

Each active criterion excludes known mismatches. For example, a host
with unknown Clanker or variant metadata remains eligible for either
value. Co-op sessions are excluded by multiplayer game type or variant
criteria, and multiplayer sessions by campaign or difficulty criteria.
The map filter can recover a category from package provenance or a
recognised map name when the host omits it. The server applies the filters
before the page limit and retains the visibility and endpoint rules above.
Send the same filters with each cursor.

With `browser_variants=true`, the first page also has a `variants` array
with up to 64 distinct multiplayer variant names. It comes from sessions
visible to the reader and eligible for the list, before the browser
criteria and limit. The game uses this with the retail custom browser's
search query. The next cursor page omits `variants`. List items include advertised
`browser` metadata when the request uses `filters` or
`browser_variants=true`. Plain lists and session details use the legacy
advertisement shape.

### Network addresses

A session holds its host's network addresses: the declared
`advertisement.addr` and `port`, the XNADDR in `advertisement.host_address`,
and the `endpoint` the server observed (often the player's public IP). They
are what a game joins by, so `GET /v1/sessions` and `GET /v1/sessions/{id}`
give them to a signed-in reader (a `read` bearer: the game's matchmaking
and join calls) and leave them out for an anonymous one.

A signed-in reader whose address, as the server sees it
(`policy.public.client_ip`, an IPv4-mapped IPv6 address counting as its
IPv4), is the session's `endpoint.addr` also gets
`lan: {addr, port, source: "lan"}`: the address and port in the host's
XNADDR (`host_address` bytes 0–3 and 8–9). The two consoles sit behind
one NAT, which seldom loops a public address back inside, so the joiner
reaches the host at its LAN address. `lan` is given only when that
address is a private, link-local or shared (100.64/10) one other than
the endpoint, the port is 1..65529, and the endpoint itself is not a
local address (`Session::lan_for`). It is computed for each read and is
never stored or logged; an anonymous reader never gets it.

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
| `MessageSent` | the message's sender and recipient |
| `MessageDeleted` | the recipient |
| `FriendRequested`, `FriendAccepted`, `FriendDeclined`, `FriendRemoved` | the two accounts and the current friends of either |
| `ContentReported` | admins only |
| `PlayerReported`, `ReportReviewed` | admins only ([Player reports](live-api-social.md#player-reports)) |
| `AccountBanned`, `AccountUnbanned` | the banned account ([Bans](live-api-social.md#bans)) |
| `PlayerMuted`, `PlayerUnmuted` | the muting account ([Mute list](live-api-social.md#mute-list)) |

Admin accounts and the operator read all of them; anonymous readers none,
so an anonymous read stays safe for the edge cache. Other readers get the
log without them, the same way as hidden co-op events (a page can be short
with a `next`). The game and the web read these through their own calls
(`/v1/accounts/by-xuid/{xuid}/mailbox` for the owner,
`/v1/accounts/{gamertag}/friends`, `/v1/admin/content` and
`/v1/admin/reports` for admins), not
the log. The friends read keeps friendships public and pending requests
for the player ([Friends](live-api-social.md#friends)).

Lists accept `state` (default `open`), `hopper` (numeric ID), `map`, `host`
(author or canonical gamertag), `flags` (`custom`, `hopper`, `ranked` or
`coop`: `?flags=coop` lists co-op campaign sessions), `visibility`, and `attributes` (URL-encoded
JSON with exact value comparisons). Filters combine. `limit` is 1..1000, default 100; `cursor`
is the previous `next` string. Ordering is by session ID. `optimatch`
(URL-encoded JSON `{hopper, average, lowest?, highest?, party?, has_map_pack?}`) is the
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

**Stats.** `GET /v1/stats/summary` and `GET /v1/accounts/{who}/medals`
count every game for admins. Everyone else gets the public fold: a Live co-op game linked to a session is
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

### Map Pack matchmaking

`PUT /v1/matchmaking/queue` (a `sessions.write` token) accepts an optional
boolean `has_map_pack`. It reports whether the requesting console has
all nine maps installed. Omission counts as false for matchmaking.
The assignment's `has_map_pack` is true when every assigned console and
the host can use the Map Pack. Older assignments omit it. A Map Pack map
on an assigned session requires this capability.

`GET /v1/sessions?optimatch=<json>` accepts the same optional member in
the decoded search. Live includes legacy Map Pack advertisements only
for an authenticated solo search (`party: 1`) with the capability. This
includes published maps that require a Map Pack base. Custom-game
browsing keeps its own map availability rules.

`GET /v1/matchmaking/availability` (an account's `read` token) returns
`{"has_map_pack": true}`, `{"has_map_pack": false}` or
`{"has_map_pack": null}`. It describes the reader's last console search.
Only an explicit report from an authenticated search or queue refresh
counts. The report expires after 30 minutes and the response is private
and uncached.

`GET /v1/titles/4D530064/files/multiplayer_hopper.dat` serves a disc-only
rotation to legacy clients. A client with runtime rotation filtering
requests `?map_pack=1` for the full file. Each response's ETag identifies
the bytes served for that variant. `GET /v1/titles/4D530064/hopper`
always describes the published file. A hopper with Map Pack entries
includes `has_map_pack_maps: true` in this JSON. A rotation with add-on
entries needs at least one Halo 2 disc map when published or imported.
[Map Pack rotations](live-matchmaking-search.md#map-pack-rotations)

## Server invites

An admin or the operator can enable **can invite** on an existing account.
Both grant events are server-signed and include the acting author's id
in `by`. The cap is a positive 32-bit integer, or `null` for no cap.
Omitting `open_cap` also sets no cap. The account list, account detail and
snapshot include `invite_grant: {open_cap}` only when the explicit grant
is active. Admins have invite permission through their role.

An authenticated non-admin account without a grant receives HTTP 403
for `POST /v1/invites`:

```json
{"error":"forbidden","message":"an admin must enable invite access for this account"}
```

For a non-admin with a grant, creating at the open invite cap returns 409. Exceeding the
creation rate returns 429. Non-admin accounts with a grant have a burst
of three, with one invite slot replenished per minute. Admins have a
burst of ten and the same refill rate. Redemption or revocation frees
open-cap space. Lowering a cap leaves existing invites open. Revoking the
admin role permanently closes the account's open invites. A ban blocks
redemption of a grantee's codes while it is in force.

Both list endpoints return `{items, position}`. Each item has `jti`,
`by`, `at`, `exp`, `state` (`open`, `used`, `revoked` or `expired`) and
`custom`. It can also have `by_gamertag`, `used_by` and
`used_by_gamertag`. Open generated invites carry the `invite` code.
See [Invite codes people type](live-admin.md#invite-codes-people-type).

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

## Dedicated hosts

A `HostLease` reserves a process for one match. Its `id` is `hl-` plus
16 random bytes in unpadded base64url. The `assignment_id` is the
matchmaking assignment ID, `sim-<id>` or `custom-<id>`. The lease records
the `provider` (`local`, `cloudflare` or `fly`), `region`, `class` (`small`
for 1 vCPU and 2 GiB, `large` for 2 vCPUs and 4 GiB), `map`, `map_id`,
`variant`, `hopper`, `max_players` (1–16), `roster` (XUIDs), `bots`
(seat names), `transport` (`udp` or `tunnel`) and `created_ms` (Unix
milliseconds). `idle_timeout_s` defaults to 120 and `max_match_s` to 1800.
A Firefight match's lease also carries `firefight`, the match as a
session advertises it (`set`, `id`, `teams`, `difficulty`, `kit`); it is
left out otherwise. Its pack runs nothing beside it, and the host plays
the map's kit from its game data
([Packed hosts](dedicated-host.md#packed-hosts)).

A `HostEndpoint` contains `transport`, `udp` (address and base port),
`tunnel_url`, `xnaddr` (hex of the advertised XNADDR) and `session_id`.
UDP uses base through base+6. Host states are `requested`, `booting`,
`joinable`, `playing`, `ended` and `failed`.

The server signs these lifecycle events on stream `hosts`.

| type | data |
|---|---|
| `HostRequested` | `{lease}` (the full `HostLease`) |
| `HostBooting` | `{lease_id, provider_ref}` (the provider's container, machine or process ID) |
| `HostReady` | `{lease_id, endpoint, boot_ms}` (a `HostEndpoint` and milliseconds to readiness) |
| `HostPlaying` | `{lease_id, players}` (player count) |
| `HostEnded` | `{lease_id, reason, ran_ms, players_peak, pack_usage?}`. Packed supervisors report the elapsed `slot_us`, integrated `container_share_us`, `charge_start`, and sampled `cpu_us` and `peak_pss_bytes` when available. |
| `HostStopped` | `{lease_id, pack_release?}`. A provider-confirmed stop can carry the pool receipt `"no_linger"` or `{"linger":{"released_ms":...}}` (Unix milliseconds). |

End reasons are `match_over`, `empty`, `idle`, `max_duration`, `stopped`,
`boot_timeout`, `crashed` and `budget`. `ran_ms` is elapsed milliseconds.

The matchmaking assignment includes `host_lease` when an ad hoc dedicated
host serves it. A queue reply with `state: "join"` includes `session_id`
and `frames_from_host`. When true, the dedicated host supplies telemetry
frames for the match. Older replies that omit the flag read as false.

## Dedicated hosts

| Route | Auth | Result |
|---|---|---|
| `POST /v1/hosts/{lease}/register` | Lease's host bearer, `hosts.register` | Takes a [`HostEndpoint`](dedicated-host-build.md#1-lease-live-proto-owned-by-lane-dh-lease) JSON body up to 8 KiB. Records `HostReady` and returns `200` with the `HostLease`. |
| `POST /v1/hosts/{lease}/report` | Lease's host bearer, `hosts.report` | Takes `{state, players, reason?, pack_usage?}` JSON up to 1 KiB. A packed supervisor includes its usage in the ended report. Live bounds slot time by elapsed time since the lease request, requires a container share from one tenth of slot time (rounded down to whole microseconds) through the full slot time, and bounds CPU time by slot time times allocated vCPUs. A zero duration or out-of-bounds reading ends the lease with a full-class estimate. `state` is `"playing"` or `"ended"`, `players` is the player count and `reason` is an optional `EndReason`. Records `HostPlaying` or `HostEnded` and returns `204`. |
| `GET /v1/hosts/{lease}` | Roster member or admin with a `read` bearer | Returns `200` with `{lease, state, endpoint}`. The endpoint is available after registration. |
| `GET /v1/admin/hosts` | Admin with an `admin.read` bearer | Returns `200` with running and recent hosts and their costs. |
| `POST /v1/admin/hosts` | Admin with an `admin.write` bearer | Takes `{map, variant?, max_players? (1–16, default 8), class? ("small"\|"large", default "small"), region?, transport? ("udp"\|"tunnel", default per provider), bots? (0 to `max_players` − 1, default 0), launch? (default `true`)}` JSON up to 1 KiB, with `map` 1–64 ASCII characters. Reserves a `custom-<id>` lease with an empty roster and `bots` Clanker names in its `bots` through the same budget, `LIVE_MAX_HOSTS` cap and cost rows as a match. With `launch` the configured provider starts it and the reply is `201` with `{lease}`; with `launch: false` no provider is started, the lease stays `Requested` until the operator's container registers, and the reply is `201` with `{lease, token}` - the **only** route that returns the host token. With no provider configured the reply is `503`. |
| `POST /v1/admin/hosts/{lease}/stop` | Admin with an `admin.write` bearer | Takes no body, stops the host with reason `stopped` and returns `204`. |
| `GET /v1/hosts/{lease}/tunnel` | Roster member with a `sessions.relay` bearer on upgrade | Reserved for the [WebSocket datagram tunnel](dedicated-host-build.md#6-tunnel-wire-live-proto-owned-by-dh-tunnel-client). The native stub returns `501`. |

Invalid JSON bodies return `400`. Requests exceeding a body cap return
`413`. Lease and inventory reads use the shared per-IP API rate limit.

## Dedicated host tunnel

A client joins a tunnel endpoint with a WebSocket upgrade at
`GET /v1/hosts/{lease}/tunnel`. The upgrade carries the roster member's
`sessions.relay` bearer in the `Authorization` header. The client uses
the endpoint URL directly and sends one frame per binary message.

| Byte offset | Size | Field |
|---|---|---|
| 0 | 1 | Version, currently 1 |
| 1 | 1 | Kind, 0 for datagram, 1 for hello, 2 for bye |
| 2 | 2 | Port offset 0 through 6, little-endian |
| 4 | 4 | Peer ID, little-endian |
| 8 | 0–1264 | Payload bytes |

The client starts each connection with an empty hello on port 0 and
peer 0. The host replies with a hello on port 0, the client's assigned
nonzero peer ID and the **host's** synthetic IPv4 address as four octets
in network order (`10.77.x.y`). All client frames have peer 0. Host
datagrams carry the recipient's assigned ID. The port names the game
channel in both directions. A zero-length datagram is valid.

The client sends a WebSocket Ping every five seconds. On transport loss
it attempts one fresh upgrade and hello. A bye has port 0 and an empty
payload. The sender follows it with a WebSocket close. The host uses
the recipient's ID for its bye. A bye ends the session.
