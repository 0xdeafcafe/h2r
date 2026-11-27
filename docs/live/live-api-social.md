# Live API: friends, teams, moderation, saves and relay

Part of [Live service API](live-api.md). Covers: Friends; Teams; Player reports; Bans; Mute list; Game-invite mailboxes; Cloud saves; Co-op relay; Streams (WebSocket); Presence heartbeats; Map outline.

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
options, docs/game/game-options.md section 3; `rules/1` runs a host's rules
script, docs/game/rules-engine.md "Versioning", and no build advertises it
yet). A bad one refuses the event
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
Member properties are `{privileges,data}` with the same byte limit.

A team
contains `id`, `creator`, `properties`, `max_members`, `created_at`, `members`,
`recruits` and `deleted`; rosters map XUIDs to
`{identity:{xuid,gamertag},properties,joined_at}`. Times are RFC 3339.

The creator is the one leader (`0xffffffff`). Permission bits are delete=1,
modify-team=2, modify-member=4, remove-member=8 and recruit=16. Halo 2 uses
`0x1c` for officers and `0x10` or zero for lower ranks.

Members cannot grant
rights they do not have, create a second leader, demote/remove the creator,
or delete a team they do not lead. Members can leave; the leader must delete
the team. Names are unique ignoring case, a team reserves at most 100 active
and recruited places, and an account may have eight teams including pending
recruits.

A deleted ID remains reserved, but its name becomes available.
Team writes are serialised through validation and projection to enforce these
limits across streams.

Recruit targets must be registered accounts with matching XUID/gamertag.
Recruitment atomically appends a server-signed `TeamRecruitReceived` event to
`account-<recruit-xuid>` with `{team_id,to,from}` (`from` is the recruiting
author). It appears on the ordinary event feed.

The pending roster is the
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
([Private events](live-api-sessions.md#private-events)). Admins review reports through
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
`0x80151200`.

The player reads their own ban:

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
  and the operator ([Private events](live-api-sessions.md#private-events)). Nobody posts them by
  hand.
- The dashboard lists the signed-in player's mutes on their own screen
  with Unmute, and offers Mute or Unmute on other players' screens. The
  game reads the list at sign-in, so a change made there reaches it then.

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
answers may clear a stale invite.

Block also refuses later invitations from
that sender; no unblock endpoint is included here. Duplicate pending invitations
to the same session are conflicts. At most 100 invitations to open sessions may
be pending per recipient.

Unknown fields, sender/recipient spoofing and foreign
or closed sessions are refused. Timestamp/key/signature/nonces and rate limits
follow the ordinary event rules. Validation and append share the session-write
lock, preventing duplicate concurrent answers or a race with session deletion.

`GET /v1/accounts/by-xuid/{xuid}/mailbox` always requires a `read` token signed
by that recipient, including on a public server. It returns
`{items, messages, next: null, position}` with at most 100 pending invitations to open
sessions, newest first, and at most 100 text messages ([below](#text-messages)). Each item is
`{id, from, to, gamertag, session_id, sent_at, answer: null}`; `sent_at` is Unix
seconds.

Sender names come from the account projection. Answered or
closed/expired-session invitations disappear from this list. The projection
rebuilds from the log on restart.

As with the rest of this service, raw events
follow the server's general public/private log policy; mailbox authorisation is
not a promise of encrypted or secret event contents.

### Text messages

Friends message each other through the same mailboxes, as signed events on
`mailbox-<recipient xuid>` with an `events.write` token.

| event | signed data | authority |
|---|---|---|
| `MessageSent` | `{from, text}` | A signed-in account messaging a friend of its own |
| `MessageDeleted` | `{by, message_id}` | The mailbox owner deleting one message |

`text` is 1 to 512 characters (`live_proto::mailbox::TEXT_MAX`), not blank, with no
control characters but newline and tab. The sent time is the signed event's own
time. `from` is the signing account and not the recipient, and the two are
friends (`State::is_friends`): otherwise 403. Bad text or an unknown field is
400, a recipient with 100 messages waiting is 409, a Clanker recipient 403.
`message_id` is the sent event's log position; a message that is not in the
owner's mailbox is 400.

Voice messages are not supported.

The mailbox read lists each waiting message as
`{id, from, to, gamertag, text, sent_at}` (`sent_at` is Unix seconds), newest
first. Deleted messages disappear; the projection rebuilds from the log on
restart. The log shows `MessageSent` only to its sender and recipient and
`MessageDeleted` only to the recipient (admins and the operator see both),
like the invitation events.

The HLE fetches this mailbox with the current account's key. Successful delivery
is not acceptance or a joined player: an accepted invite becomes a local launch
record, and actual connection uses FindFromID and the guest's QoS/UDP path.
`PlayerJoined` remains a host-reported session event, not an inferred side effect
of answering an invitation. No message attachments, notification acknowledgments,
ranked matcher or UDP relay are added by these endpoints.

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

- The upgrade request carries `Authorization: Bearer <jwt>` with
  `lxm: sessions.relay`. A missing, wrong or expired token gets HTTP 401.
  After the upgrade, `{ "kind": "auth", "token": "<jwt>" }` from the same
  account refreshes the socket. Expiry closes it with 4401.
- The session must be open and created with `flags: "coop"` or
  `flags: "forge"` (a shared Forge session, [forge-design.md](../game/forge-design.md)).
  The account
  must be its host or a guest who accepted an invitation. The upgrade
  gets HTTP 403 if membership is refused or 404 if the session is missing.
  The relay rechecks membership after connection and closes with 4403
  if membership ends.
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
readers allowed to see it ([Co-op games](live-api-sessions.md#co-op-games)). On a public server
the token's expiry closes only explicitly scoped [sim streams](live-api-theater-stats.md#clanker-training).
Without `auth`, a public stream is anonymous and leaves those events out.

Legacy subscribers request `?frames=packed` for v7. The viewer requests
`?frames=packed&frame_version=8` and recognises the version in each binary
record, including v7 from older servers. Both homes batch each game's
length-prefixed records for up to 100 ms.

Readers of the same version
share its encoder and batches. Events and `caught_up` use JSON text.

A new v8 stream starts with an independent key. A reader joining an
existing stream receives the key and all subsequent records through the
latest published frame. That suffix restores the adaptive model used by
the next delta.

A v8 key interval spans at most 150 samples. The server
also uses this history to catch up a slow reader and suppresses records
that it has already sent. A hibernated Worker socket retains its requested
version across wake-up.

**`GET /v1/ingest`** opens a game socket. The first message is
`{ "kind": "auth", "token": "<jwt>" }` with a `games.write` account
token or a `telemetry.ingest` host lease. If discovery advertises
`ingest_frame_batch: true`, the client can send `auth_batched` as the
message kind, including on token refresh. A socket carries one stream of
packed frame records, oldest first, in binary messages.

In batched mode
each message can hold up to 16 records, and each game can spend up to 16
frames of accumulated credit, replenished at about 15 frames per second.
The same budget applies across the game's publishers. Each text or
binary message is capped at 16 KiB.

Clients can also send `{ "kind": "frame", "frame": {...} }` for a JSON
frame. Frames are recorded in [frame chunks](live-api-events.md#frame-chunks) and sent to
readers. Only refused frames get a reply, with kind `frame_error` in
batched mode and `error` on legacy sockets.

An event message carries
`{ "kind": "events", "game_id": "...", "jws": "<one batch JWS>" }` and
gets `{kind: "appended", game_id, position}` or `{kind: "error", error,
message}`. A `{kind: "ping"}` refreshes presence for an account or
session activity for a leased host. Both deployments use
`crates/live/live-server/src/ingest.rs`.

A `GameFrame` recorded before frame chunks holds `{tick, source, packed}`:
`packed` is the frame as one packed keyframe, base64url without padding, so
the event's signature covers the packed bytes. The oldest hold the JSON
frame plus `source`; both read the same through `/frames`.

## Presence heartbeats

| Endpoint | Request | Response |
|---|---|---|
| `POST /v1/presence/ping` | One `events.write` bearer token | `{ok: true, expires_in}`, or 404 if the account has no presence |
| `POST /v1/presence/ping/batch` | JSON `{tokens: [...]}` containing 1–4 single-use `events.write` tokens | HTTP 200 with `{results: [...]}` in token order. Each result is `{ttl}` in seconds or `{status, error, message}` for that token |

The batch handler in `crates/live/live-server/src/http.rs` returns 400 for
an invalid body or token count and 413 for a body over 64 KiB. Each token
is verified independently. The client issues fresh tokens on retries and
uses individual pings if the batch endpoint returns HTTP 404. An
individual result with status 404 tells the client to sign that account
in again.

Presence expires 90 seconds after the last event or ping under
the default policy.

## Map outline

An admin or the operator can upload an outline derived from their own map
files with a `games.write` token. Each map name has one outline, shared
by every game on that map. A new upload replaces the current outline.

```json
{ "map": "lockout", "bounds": { "min": [x,y,z], "max": [x,y,z] },
  "layers": [ { "z": [zmin, zmax], "polygons": [[[x,y], ...], ...] } ] }
```

Polygons are top-down projections of floor surfaces, grouped into height
layers so multi-storey maps can be drawn one floor at a time.
