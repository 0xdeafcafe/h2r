# Matchmaking and custom games over Live

Status: phase 1 server sessions and HLE invitations are implemented. Tests use
router memory stores and two synthetic guests against `start_embedded`, through
HTTP delivery, acceptance, FindFromID, QoS and bidirectional UDP. The owner
has Live working in the game (2026-11-17).
Search returns real sessions, filtered by the decoded procedure-1
parameters: playlist, the party's level window and room for the party,
closest mean level first (§7). The game widens its own window and hosts
when nothing is found. Solo clients can opt into the ping-aware server
queue in [§9](#9-ping-aware-optimatch-assignments). Ranked games
are ranked by the server from the games log, for the players who
registered the game's arbitration round (§8.4); comparing arbitration
reports, and NAT traversal, remain design.
Current contracts are in [live-api.md](live-api.md#sessions) and
[live.md](live.md#client-hle-game-invitations-and-joining). The sections below
retain the static call trace and phased design; implemented details there defer
to those Client HLE/API sections.

Retail context: session advertise/search/delete were three services behind
the security gateway — `/xmatch/xmatchhost.srf`, `xmatchclient.srf`,
`xmatchhostdelete.srf` ([live.md](live.md) §2) — which map one-to-one onto
`XOnlineMatchSessionCreate` / `XOnlineMatchSearch` /
`XOnlineMatchSessionDelete`. Under decision C (replace the API boundary) our
HTTP service replaces those services; nothing here speaks their wire format.

The in-game custom games browser (the System Link browser's dormant Live
mode) and session visibility are in [live-custom-games.md](live-custom-games.md).

## Ping-aware host selection (in progress)

SBMM is adding a host-selection step that considers measured RTT as well
as skill. A proposed group needs an eligible host that every member can
reach. The server prototype widens the skill and latency windows with
queue time, then prefers the lowest worst-player RTT. On equal worst
RTT it prefers a peer host, then the lower median. A dedicated host
must improve the worst route to displace a peer. The current OptiMatch
search and join path is described below. [Dedicated host](dedicated-host.md)

## 1. What the retail game calls

Confidences are [live-boundary.md](live-boundary.md)'s (`confirmed` is
statically identified from ABI and behaviour; `likely` is a consistent
proposed name). "Known" below means the docs establish it; "to confirm"
needs a reading of the retail code, with the addresses to read. Addresses
are guest VAs in the retail 5849 image.

### 1.1 Hosting: create, advertise, update, delete

The host path, in call order. Steps 1–2 and 5–6 are established pairings;
the exact interleaving of 3/4 with them is inferred and marked.

1. **Make and register a key.** `XNetCreateKey` (`0x003cd0cb`, confirmed;
   game wrapper `0x0007a8a0`, site `0x0007a8ce`) fills an 8-byte XNKID and a
   16-byte XNKEY. `XNetRegisterKey` (`0x003cd0d6`, confirmed; wrapper
   `0x0007a920`, site `0x0007a951`) registers it. Create must precede
   register (the second register site, `0x000b3a86`, is on the join path,
   §1.2). Both HLE entries are already implemented with host randomness.
   *To confirm:* whether key creation happens before or after the session is
   created (both orders appear plausible from the wrapper cluster; read
   `0x0007a8a0`'s callers).
2. **Create the session.** `XOnlineMatchSessionCreate` (`0x003a06a8`,
   confirmed; wrapper `0x0008fcc0`, site `0x0008fd25`): filled and open slot
   counts plus an attribute list, eight stack words. This is the
   `xmatchhost.srf` advertise.
3. **Read the session id and the host address.** On task completion,
   wrapper `0x0008fd90` calls `XOnlineMatchSessionGetInfo` (`0x003a06e7`,
   confirmed; site `0x0008fdef`) for the session id and exchange key, then —
   in the same wrapper — `XNetGetTitleXnAddr` (`0x003cd17d`, confirmed;
   site `0x0008fdfc`) to fill the local 36-byte XNADDR. Known: these two
   reads happen together right after creation, so the host's address is
   published with the session. *To confirm:* whether the address and key id
   go in as session attributes (the title's attribute-type vocabulary,
   [live-motd.md](live-motd.md) §5.4, includes `secure-address`) or travel
   another way; read the attribute list built by `0x0008fcc0` and what
   `0x0008fd90` does with the XNADDR.
4. **Advertise QoS.** `XNetQosListen` (`0x003cd147`, confirmed): key id,
   up to 1020 bytes of listener data, advertised bandwidth, flags. Five
   sites in the session lifecycle wrapper `0x000731d0` (`0x000731f9`,
   `0x00073231`, `0x0007324c`, `0x000732bb`, `0x000732dc` — enable, update
   and disable variants) and two more (`0x0005fb57`, `0x0005fb7c`) beside
   `XNetUnregisterKey` at `0x0005fb97`. The HLE side
   protocol (UDP on base+2) is already implemented. *To confirm:* when
   listening starts relative to session creation.
5. **Update as the lobby changes.** `XOnlineMatchSessionUpdate`
   (`0x003a06b7`, confirmed; wrapper `0x0008fe20`, site `0x0008fea4`): the
   8-byte session id by value, slot counts and attributes; the game calls it
   as players join, leave and the game starts. Our server drops updates
   that change nothing (already decided, [live.md](live.md) §4.3).
6. **Delete.** `XOnlineMatchSessionDelete` (`0x003a06f2`, likely; wrapper
   `0x00090020`, site `0x00090089`), then `XNetUnregisterKey`
   (`0x003cd0e1`; shutdown also unregisters every key at `0x0008d9b9`).

### 1.2 Searching and joining

1. **Size and start the search.** Wrapper `0x0008fa80` calls
   `XOnlineMatchSearchResultsLen` (`0x003a0753`, confirmed; site
   `0x0008fad4`) to size the result buffer, then `XOnlineMatchSearch`
   (`0x003a072e`, confirmed; site `0x0008faea`): a procedure, a result
   limit, attributes (the query), buffer size, event/task. This is the
   `xmatchclient.srf` search. Procedure 1 with twelve positional
   parameters, decoded in §7.
2. **Read and parse results.** `XOnlineMatchSearchGetResults`
   (`0x003a073d`, confirmed; site `0x0008fba7` in wrapper `0x0008fb30`)
   yields the result array; `XOnlineMatchSearchParse` (`0x003a0748`,
   likely; site `0x0008fc5c`) decodes each result into title attributes.
   *To confirm:* the result layout and which attributes the game reads
   (map, variant, player counts, the host's secure address and key id) —
   read `0x0008fc5c` and the consumers of its output.
3. **Probe candidates.** `XNetQosLookup` (`0x003cd156`, confirmed; wrapper
   `0x0007b4c0`, site `0x0007b558`) probes the candidates' addresses and
   keys (twelve args, 48-byte caller cleanup); results are released with
   `XNetQosRelease` (`0x003cd172`, site `0x0007b5a6` in the same wrapper).
   The HLE already implements lookup/listen between HLE peers with
   contacted/data-received flags, so uncontacted hosts can be skipped the
   retail way.
4. **Join a known session.** The invite/quick-join chain is wrappers
   `0x000900e0`/`0x000900ff` → `XOnlineMatchSessionFindFromID`
   (`0x003a0710`, likely; site `0x00090121`, same four-word ABI as Delete:
   the 8-byte session id by value) → `XOnlineMatchSearchGetResults` (site
   `0x000901e5`, wrapper `0x00090160`) → `XOnlineMatchSearchParse` (site
   `0x00090282`). So joining by id reuses the search result machinery on
   one session.
5. **Register the found address and key.** The query-result cluster at
   `0x000b37f0`–`0x000b3b42`: `XOnlineQuerySearch` (`0x003b0254`, likely;
   site `0x000b388a`) and `XOnlineQuerySearchGetResults` (`0x003b0263`,
   confirmed; site `0x000b39b3`), then wrapper `0x000b3a70` converts a
   title-service address from the result to a secure address with
   `XNetTsAddrToInAddr` (`0x003cd103`, confirmed; site `0x000b3aa7`),
   registers the session key (`XNetRegisterKey`, site `0x000b3a86`) and
   unregisters the previous one (`XNetUnregisterKey` `0x000b3ad7`,
   `XNetUnregisterInAddr` `0x000b3acb`/`0x000b3b2f`) — the game's own
   diagnostic names this `register_ip_and_key`
   ([live.md](live.md) §3). *To confirm:* whether match-search results also
   carry a TSADDR or only query-service results do; read what feeds
   `0x000b3a70`.
6. **Connect.** The existing secure-address path:
   `XNetXnAddrToInAddr` (`0x003cd0ec`, wrapper `0x0007adf0`),
   `XNetInAddrToXnAddr` (`0x003cd32d`), `XNetConnect` (`0x003cd34f`) and
   `XNetGetConnectStatus` (`0x003cd35a`), then the game's own sockets
   (guest ports 1000/1005/1006, VDP over UDP). All implemented for System
   Link ([live.md](live.md), sockets section). `XNetTsAddrToInAddr` is the
   one stop on this path.

### 1.3 Invitations

Invites are Live messages: the sender builds one with
`XOnlineMessageCreate`/`SetProperty` (`0x003a9d8e`/`0x003a9dc3`, likely;
wrappers `0x0008f070`/`0x0008f4b0`) and sends it with
`XOnlineGameInviteSend` (`0x003a8321`, likely; wrapper `0x0008f7e0`, site
`0x0008f852`: XUID recipient list, the session id by value, the message
handle) or, for one friend, `XOnlineFriendsGameInvite` (`0x003a07e5`,
likely; wrapper `0x0008cfe0`, site `0x0008d03a`). The friends/invite UI
reads message properties through the `0x0008e9e0` reader; its tags
(`0x481`..`0x791`) serve the friends/invite UI. The title files do not use
this path ([live-motd.md](live-motd.md) §2).

On the receiving side: the invite notification arrives via
`XOnlineGetNotificationEx` (`0x003a0798`) and presence carries the session
XNKID by value in `XOnlineNotificationSetState` (`0x003a0774`); the player
answers with `XOnlineFriendsAnswerGameInvite` (`0x003a0811`, wrapper
`0x0008d050`) or `XOnlineGameInviteAnswer` (`0x003a834e`, wrapper
`0x0008d0b0`); `XOnlineFriendsJoinGame` (`0x003a081c`, UI `0x002395dc`)
joins the friend's session; and the menu polls
`XOnlineFriendsGetAcceptedGameInvite` (`0x003a0827`, site `0x0006d211`).
The HLE consumes a real 156-byte accepted record once; with none it clears the
output and returns `E_FAIL` ("no invite"); the poll's check at `0x0006d218` treats *every nonnegative*
HRESULT as an invite, so a real invite must fill the record, not just
return success ([live.md](live.md), menu audit). The session property is `0x0581`. The record layout is verified at
`0x003b49ce`: friend86/padding2, accepted XUID12 at88, FILETIME at100,
logged-on XUIDs48 at108. The full retail UI sequence remains unconfirmed.

### 1.4 Ranked results: arbitration and stats

For ranked hoppers the game runs arbitration before results count:
`XOnlineArbitrationCreateRoundID` (`0x003adc4c`, confirmed; wrapper
`0x00073ae0`: eight random bytes, local), `XOnlineArbitrationRegister`
(`0x003ae32b`, likely; wrapper `0x000924e0`: round id, duration, flags),
`ExtendRound` (`0x003ae33a`), `Report` (`0x003ae36a`: round id, stat
procedures, report data). Stats go out with `XOnlineStatWrite`
(`0x003a08c0`, wrapper `0x000923c0`) and `WriteGetResult` (`0x003ab6a6`);
leaderboard reads use `XOnlineStatRead` and the query service above. The
compiled-in matchmaking attribute names at `0x0046ddc0` (`games-played`,
`games-quit`, ...) are the per-player stats schema the hopper file
overrides ([live-motd.md](live-motd.md) §1.2). Stat writes are
accepted and kept locally; stat reads of a playlist's leaderboard get
the player's rank from the server (§8.3), other stats read as absent;
arbitration is accepted and kept locally (§8.3); query searches answer
`QUERY_DATASET_NOT_FOUND`.

### 1.5 Known versus to confirm

Established by the docs: the entries, addresses, ABIs and confidences
above; the GetInfo-plus-`XNetGetTitleXnAddr` pairing in `0x0008fd90`;
ResultsLen-before-Search in `0x0008fa80`; the
GetResults-then-Parse pairs (`0x0008fb30`→`0x0008fc5c`,
`0x00090160`→`0x00090282`); FindFromID's place before them on the
join-by-id path; the QoS lookup/release pairing; and the
`register_ip_and_key` cluster. Session ids are 8 bytes on the wire (two
stack words by value).

To confirm in the retail code, with addresses:

| question | read |
|---|---|
| ~~Attribute list written at session create/update~~: decoded (§7.1); how the host address and key id are published | `0x0008fcc0`, `0x0008fe20`, `0x0008fd90` |
| ~~Search procedure values and query attributes~~: decoded (§7.2) | `0x0008fa80` |
| Search result layout; which attributes the game reads back | `0x0008fb30`, `0x0008fc5c`, `0x00090160`, `0x00090282` |
| Whether match results carry a TSADDR, or only query results do | `0x000b3a70` and its callers |
| Order of key create/register versus session create; when QoS listening starts | `0x0007a8a0`, `0x0007a920`, `0x000731d0`, the `0x0005fb57`/`0x0005fb7c` sites |
| Optional invite voice/text playback and full retail UI handoff (accepted-record layout confirmed) | `0x0008f7e0`, `0x0008eda0` tag table (`0x0008ee44`), `0x0006d210` |
| ~~Hopper types 1–5 and the meaning of the descriptor ranges~~ answered in §6 | `0x00065530`, `0x00192eb0`, `0x00231066` |

## 2. How hoppers drive matchmaking

`multiplayer_hopper.dat` is the playlist file, delivered after sign-in through
per-title storage facility 4 (envelope and delivery:
[live-motd.md](live-motd.md) §2 and §5).
What matters for matchmaking:

- The tag `matchmaking_hopper_descriptions` defines the universe of
  playlists; the file's 16 hopper entries (0x614-byte descriptors) each
  reference up to 100 records in it by id and supply live-tunable
  parameters (0x15cb8-byte blocks). Unknown record ids mark the slot as
  error, so **the server may only advertise playlists the tag knows**.
- The playlist UI (`0x00065530`) iterates the descriptors filtering types
  {2, 4, 5}; Quickmatch's start (`0x00231066`, the profile's last
  hopper) and the playlist screen's press (`0x002b2149` → `0x002b2181`,
  docs/game/bots.md §8.3) read the selected hopper's block, and the session state machine (`0x00192eb0`, states
  6/7/8) consumes it during setup. The hopper therefore decides the search
  parameters, party size limits (descriptor range fields) and the
  matchmaking timeouts — the defaults carry 30/45/120/150/300 s constants
  with field ids `0x7a4`..`0x7ab`.
- If the file is missing or fails validation the game silently runs the
  compiled-in defaults (`0x00194160`), so matchmaking works without our
  server publishing anything — but then the server's hopper list and the
  game's can disagree, which the matcher must tolerate (see §3.4).
- The operator already publishes the file through the title-file API
  (`PUT /v1/titles/...`, `h2 live motd set`-style tooling; 2 MiB cap covers
  the 1.46 MB inflated payload). The server can parse the same bytes it
  publishes to learn the hopper list — it is the source of the file, so it
  never needs to reverse the retail defaults; it only needs to *author* a
  valid file, which it now does (§6.8: the format, field by field, and the
  built-in lineup served when nothing is published).
- The attribute-type vocabulary beside the hopper descriptor
  (`none`...`axes`, including `secure-address`, at `0x0046dd80`) is the
  title's online-attribute schema; matchmaking attributes and the stats
  schema (`games-played` and friends at `0x0046ddc0`) draw from it.

## 3. Server design

Same shape as the games slice: everything is an event in the log, the read
model is a projection, writes are signed, pushes ride the stream
WebSocket, and a sweeper ends what clients abandon
([live.md](live.md) §4.3).

### 3.1 Sessions as events

Stream `session-<id>` (the id is 16 hex = 64 bits, matching the 8-byte
by-value ABI; the server picks it, as it does for games):

| type | data | author |
|---|---|---|
| `SessionCreated` | `session_id`, `host` (xuid), `xnkid`, `endpoint {addr, port, source}`, `flags` (`custom`/`hopper`/`ranked`), `hopper_id?`, `name?`, `language?`, `slots {filled, open}`, `attributes` (the title's typed attribute map) | server (with `host` = the creating token's author) |
| `SessionUpdated` | changed `slots` / `attributes` | host |
| `PlayerJoined` / `PlayerLeft` | `xuid`, `slot?` | host |
| `SessionDeleted` | `reason` (`closed`/`host_left`) | host |
| `SessionExpired` | – | server (sweeper) |

Principles already decided and kept: changes, not pings — the host's
30 s presence ping refreshes an in-memory deadline and writes nothing;
no-op `SessionUpdate`s are dropped; expiry is an event, so a crashed host
still ends its stream and a replay gives the same answer. The deadline
seeds from the projection (last event `at`) like games. Only the host
writes membership in the first slice: joins are learned over the game
network, exactly as the retail host re-advertises filled slots through
`xmatchhost.srf`. (Games allow players to write too; sessions can loosen
later without a schema change.)

The XNKID/XNKEY pair stays host-local (`XNetCreateKey`); the session record
carries only the XNKID (the public half), which joiners register with
`XNetRegisterKey` before connecting — mirroring the retail
`register_ip_and_key` flow. The XNKEY never leaves the host.

The projection holds: open sessions by hopper and id, slot counts,
endpoints, and per-hopper population (searching counts, §3.4).

### 3.2 HTTP API

Following [live-api.md](live-api.md)'s conventions (`{items, next,
position}`, public reads, `read` token on `--private` servers):

| method and path | body / notes |
|---|---|
| `POST /v1/sessions` | `SessionCreated` data without `session_id`; server signs it with `host` = author; 201 `{session_id, position}` |
| `POST /v1/sessions/{id}/events` | one batch JWS by the host (`SessionUpdated`, `PlayerJoined`, `PlayerLeft`, `SessionDeleted`), validated like game batches: membership checked as the batch applies it, nothing written from a refused batch, replay 409, writes to an expired/deleted session 409 |
| `GET /v1/sessions?state=open&hopper=&map=&limit=` | session summaries: host gamertag, name, hopper, slots, attributes, endpoint, `since` — this is what `XOnlineMatchSearch` and the web app read |
| `GET /v1/sessions/{id}` | one session — the `XOnlineMatchSessionFindFromID` answer |
| `h2 live q sessions` / `q session <id>` | the agent queries, alongside `q games` |

The HLE implements the boundary entries against these: `SessionCreate`
POSTs and hands the returned id to `GetInfo`; `Search` GETs with the
procedure mapped to a filter set; `SearchGetResults`/`SearchParse`
fabricate result records from the JSON items (the attribute map round-trips
as-is); `SessionUpdate`/`SessionDelete` POST events; `FindFromID` GETs one.
The search "procedure" the game passes is an opaque number to the server
until §1.5 confirms its values; the HLE can translate whichever values the
retail code uses into filters.

### 3.3 Invitations as mailbox messages

Implemented on `mailbox-<xuid>` streams with signed `GameInviteSent` and
`GameInviteAnswered` through `POST /v1/events`. The host must be signed in and
own the open session; only the recipient answers. The sent position is the
invite ID. Answers select accept, decline, remove or block. The recipient reads
pending items with its own `read` token at
`GET /v1/accounts/by-xuid/{xuid}/mailbox`. See
[live-api.md](live-api.md#game-invite-mailboxes) for payloads and limits.

The HLE refreshes friends, invitations and sessions on its HTTP worker.
MessageEnumerate and GetNotificationEx expose only real player invitations;
title files use storage. YES creates the consumed-on-read accepted record after
the server acknowledges the answer. FriendsJoinGame resolves an existing friend
and session before publishing the same join. Session resolution, key/address
registration, QoS and guest sockets then use the existing transport.

### 3.4 Hopper matching

Searching players are event-sourced too, on `matchmaking-<xuid>` streams:
`SearchStarted` `{hopper_id, attributes}` and `SearchStopped` (or the
stream simply ends at sign-out; the sweeper expires a stale search with the
presence). A server-side matcher — a subscriber beside the read model, no
new infrastructure — groups waiting parties by hopper, respecting the
hopper's size limits and timeouts from the published file, and writes a
server-signed `MatchProposed` `{match_id, host, members, hopper_id}` on
`match-<id>`. Each named console sees itself as host or joiner on the
stream WebSocket and proceeds: the host runs the §1.1 flow (create session,
key, QoS listen), joiners run §1.2. The log ends up recording who searched
and every match made.

If the game's hopper file is stale (defaults cached) its hopper ids can
disagree with the server's; the matcher only ever proposes hoppers it
published, and a console that answers "unknown hopper" is simply
re-searched — the failure is the retail silent-defaults path, not a crash.

### 3.5 How consoles connect to each other

No new transport: the System Link networking already built carries Live
games ([live.md](live.md), sockets section). The host's XNADDR packs its
IPv4 and base port; the four game channels are plain UDP on the host
(base 3074 + channel offset, VDP payload untouched); joiners resolve the
host through `XNetRegisterKey` + `XNetXnAddrToInAddr` and connect, with
`XNetGetConnectStatus` for liveness. What Live adds over System Link is
only *discovery*: the broadcast channel (guest port 1001, base+1) is not
used — the joiner learns the host's endpoint and XNKID from the session
record instead of from a LAN broadcast. QoS probing between HLE peers
(UDP base+2) already reports contacted/data-received flags, so search
results can be ordered by probe outcome the retail way.

Two known limits to carry forward: one HLE maps one remote endpoint per IP
(a second base on the same IP fails `WSAEADDRINUSE`), so two consoles
behind one NAT cannot both be peers of a third today — fixing it means
carrying the base port in the secure address, a self-contained HLE change;
and the loopback recipe (`H2_XNET_LOOPBACK_PEERS`) is for two instances on
one machine, not production.

### 3.6 NAT and addressing assumptions (self-hosted)

The server itself is easy by design: it needs only its HTTP(S) port, since
game traffic never relays through it. The assumptions for phase 1:

- **Same LAN:** works today, unchanged (System Link recipe).
- **Across the internet, phase 1 (direct):** the host must be reachable on
  UDP base..base+6 (3074–3080 by default); that means a forwarded port or
  a routable address. Joiners need only outbound UDP to the host and
  outbound HTTPS to the server. Symmetric-NAT hosts cannot host; they can
  still join.
- **The advertised endpoint:** `XNetGetTitleXnAddr` reports the host's
  default-route LAN address, which is useless off-LAN. The session record
  therefore carries an explicit endpoint: configured (`H2_XNET_ADDR`
  already exists for this), declared by the host at session create, or —
  as a fallback — the source address the server observed on the host's
  HTTP requests (right IP for non-symmetric NATs; the port must still be
  declared). `SessionCreated.endpoint.source` records which.
- **Later phases:** server-coordinated UDP hole-punch (the stream
  WebSocket already reaches both sides to coordinate a simultaneous
  open), or an opt-in relay. Live.md's open NAT question stands; nothing
  in the event design depends on which is chosen, because the endpoint is
  data in the session record.

`XOnlineGetNatType` keeps returning the retail unknown value (0); no NAT
probing is claimed.

### 3.7 Signing and lxm, per write

| write | signed by | token |
|---|---|---|
| `POST /v1/sessions` (`SessionCreated`) | server key | `sessions.write` (new) |
| `POST /v1/sessions/{id}/events` | host account key (batch JWS) | `sessions.write` |
| `SessionExpired`, `MatchProposed` | server key (sweeper / matcher; no token) | – |
| `GameInviteSent` / `GameInviteAnswered` on `mailbox-*` | sender / recipient account key | `events.write` + the §3.3 permission rule |
| `SearchStarted` / `SearchStopped` on `matchmaking-<own>` | the player's account key | `events.write` (own stream, like presence) |
| reads (`GET /v1/sessions...`) | – | public; `read` on `--private` |

One new `lxm` (`sessions.write`) keeps the method-per-endpoint-family
pattern of `games.write`. Event bodies, batches, the hash chain and token
rules are exactly [live-security.md](live-security.md) §2–§3; no new
crypto.

### 3.8 What the web app can show

- **Open sessions** — a live list (host, name, hopper or custom, map and
  variant from the attributes, slots filled/open, since), updating from
  the stream the way the games list does.
- **Hopper populations** — players searching per hopper, matches proposed
  over time, from the `matchmaking-*`/`match-*` projections.
- **Co-op campaign** — a `coop` session shows as "Co-op campaign" with
  its level name, difficulty and players (host and who joined), in the
  dashboard's find-a-game list and the stats home. Finished co-op games
  come from `GET /v1/coop/games` and show on the games page's campaign tab
  and a player's history (Co-op tab); one from a friends-only or
  invite-only session shows only to who may see it (live-api.md, "Co-op
  games"), so the web asks with a `read` token when signed in. A session's
  `ping_ms` (the host's round trip to its guest) shows as "42 ms" in the
  row and the detail. Friends' presence shows its
  `activity` line ("Co-op: Outskirts") when the game set one.
- **Session detail** — players as they join, linking to profiles and (once
  the game starts) the existing game pages; presence's `game_id` already
  ties a playing account to a game, and `PresenceChanged` can grow an
  optional `session_id` for the lobby state.

Presentation follows the web app's established Live-side style.

## 4. Phased plan

**Phase 0 — confirm the retail contracts** (§1.5's table; parallel, no
server work). The HLE result records can't be finalized before this.

**Phase 1 — custom games listed and joinable by invite** (implemented and
synthetically tested; retail play confirmation pending). Server: `session-*` streams, `POST /v1/sessions` and
`/events`, `GET /v1/sessions{,/{id}}`, the sweeper, the `mailbox-*`
invite pair, `sessions.write`, `q sessions`; web: the session list. HLE
(replaces these stops): `XOnlineMatchSessionCreate`, `SessionUpdate`,
`SessionGetInfo`, `SessionDelete`, `SessionFindFromID`,
`XOnlineGameInviteSend`, `XOnlineGameInviteAnswer`,
`XOnlineFriendsGameInvite`/`AnswerGameInvite`/`JoinGame`, and a real
`XOnlineFriendsGetAcceptedGameInvite` (E_FAIL stays the no-invite answer,
which the poll already handles). Success: two local instances, one invites
the other over the server, they play — the System Link recipe with the
server in place of the broadcast. Tests: router tests over the in-memory
store and synthetic guest tests through `start_embedded`, in the style of
the sign-in/message tests; no game build.

**Phase 2 — search and hoppers.** HLE: `XOnlineMatchSearch`,
`SearchGetResults`, `SearchParse`, `SearchResultsLen` over
`GET /v1/sessions` (in-game custom-game browse), QoS probing of candidates
over the existing side protocol. Server: the matcher, `matchmaking-*` and
`match-*` streams, hopper populations on the web, and our own authored
hopper file so game and server agree on playlists.

**Phase 3 — ranked results.** Arbitration (`Register`/`ExtendRound`/
`Report`) checked against the games log the consoles already write, stat
writes, and leaderboards through the query service — at which point the
`XNetTsAddrToInAddr` stop and the stats stops retire.

The HLE halves of each phase belong to `crates/live-hle` (Codex's lane);
the server halves to `crates/live-server`/`crates/live-proto` and the web
app.

## 5. Open questions

- The complete retail invite UI/confirmation transition remains untested.
  Session records, seven-column parsing and accepted-invite packing are checked.
  Procedure 1's parameters are decoded (§7); the hosting branch after an
  empty search (`0x0005a220(6)`) is read only as far as "likely".
- ~~The hopper record-template contents (§2)~~: decoded (§6); the server
  authors its own file (§6.8).
- Whether the 30 s presence ping is the right session heartbeat, or
  sessions need their own ping (the games slice uses ingest traffic;
  lobbies have none until the game starts).
- Carrying the base port in the secure address (§3.5) — needed before two
  consoles behind one NAT can both play with a third.

## 6. The hopper file, field by field

`multiplayer_hopper.dat` is now decoded whole, from the retail game's own
readers and its compiled-in defaults (read statically from the 5849 XBE;
nothing was run). The code is `crates/live-proto/src/hopper/`:
`layout.rs` (every field, the validator port, the accessor ports),
`variant.rs` (the game variant an entry carries) and `defaults.rs` (the
lineup a server offers with no file). The envelope is unchanged
([live-motd.md](live-motd.md) §5.1). Evidence: **confirmed** means a reader
was seen using the field that way; **likely** means a reader was seen but
the meaning is inferred from how it is used; **unused** means only the
game's defaults write it and no reader was found, so we write 0.

### 6.1 Image

```
+0x000000  16 x 0x614     descriptors (copied to 0x00551ae8 by 0x00193ff0)
+0x006140  16 x 0x15cb8   data blocks (read one at a time by 0x001943d0)
```

A descriptor that fails the validator (`0x00193560` and the per-type
check behind `0x001934f0`) is no hopper to the game: every reader goes
through `0x001934f0` first (`0x00192db0`, `0x00192df0`, `0x00192e60`,
`0x001943d0`, `0x001945c0`, `0x00194610`). An all-zero slot is therefore
simply absent.

### 6.2 Descriptor (0x614 bytes)

| Offset | Type | Field | Readers | Evidence |
|---|---|---|---|---|
| `+0x000` | i32 | hopper type 1..5 (§6.3) | validator; `0x00193470`/`90`/`b0`; every per-type accessor | confirmed |
| `+0x004` | i32 | status: 0 not checked yet, 1 ok, 2 error, 3 no hopper; validator 0..3 | `0x00192df0` (0 → `0x00194660` checks the entries' maps and writes 1 or 2), `0x00065530` wants 1 | confirmed |
| `+0x008` | i32 | playlist id, validator 1..=100; unique per playlist (the game's defaults use 1, 2, 3, 4, 25, 26, 5) | `0x000741f0` reads it with `+0x20` and `+0x40` beside it as stats ids | range confirmed, meaning likely |
| `+0x00c` | 9 x 16 B | title per language, UTF-8, NUL-terminated | `0x001944c0`; written by `0x001936e0` | confirmed |
| `+0x09c` | 9 x 128 B | description per language (the ninth ends at `+0x51b`) | `0x00194540` | confirmed |
| `+0x51c` | 128 x i8 | skill ceiling: entry `i` is the highest skill matched with skill `i`; validator `i <= entry <= 0x7f` | `0x001931a0` walks it down for the lowest skill matched (used by `0x00073530` for the session's skill range) | range confirmed, meaning likely |
| `+0x59c` | u32 | search timeout, ms: a team arrangement older than this is dropped | `0x00071c7a` (elapsed + it vs now) | likely |
| `+0x5a0` | u32 | base wait, ms: the defaults' input to the start-wait table | none (`0x001931d0` indexes from `+0x5a4`) | unused |
| `+0x5a4` | 16 x u32 | start wait for 1..16 players, ms; `0x7fffffff` never | `0x001931d0` (answers never outside the type's player range, 0 at the most players, else the table); `0x000719f5` compares it with time searched and shows seconds left (/1000) | confirmed |
| `+0x5e4` | i32 | minimum rank: a party member below it cannot search | `0x00066150` (per-player signed byte, error 0xf); `0x002b2090` (a profile dword at `0x0054eae4 + 0xc70 n`) | reader confirmed, meaning likely |
| `+0x5e8` | u8 | list the hopper whatever the rank | `0x002b2070` | likely |
| `+0x5e9` | 3 B | | none | unused |
| `+0x5ec` | 10 x u32 | type parameters (§6.4) | validator, accessors `0x001931d0`..`0x001934d0`, `0x0006620d` | confirmed |

The older notes' "ASCII name at `+0x51b`" and "record count at `+0x08`"
were wrong: there is no name (the bytes are the last description byte and
the skill table), and the record count is the data block's own.

### 6.3 Hopper types

| Type | Meaning | Party limit (`0x00193250`) | Smallest party (`0x0006620d`) |
|---|---|---|---|
| 1 | free for all, unranked | most players | 1 |
| 2 | free for all, ranked | 1 (alone) | 1 |
| 3 | teams, unranked | teams x most per team | 1 |
| 4 | teams, ranked | `[5]` | `[4]` |
| 5 | clan match: a party is a team | most per team | fewest per team |

Ranked is {2, 4, 5} (`0x00193470`), unranked {1, 3} (`0x00193490`),
teams {3, 4, 5} (`0x001934b0`). The playlist screen's pass at
`0x00065530` (the "types {2, 4, 5}" filter of §1.5) walks the ranked
hoppers to find a player's best skill. A party outside the limits gets
error 6 (too small) or 7 (too big) from `0x00066050`.

### 6.4 Type parameters (`+0x5ec`, ten dwords)

| Type | `[0]` | `[1]` | `[2]` | `[3]` | `[4]`/`[5]` | `[6]` | `[7]`/`[8]` | `[9]` |
|---|---|---|---|---|---|---|---|---|
| 1, 2 | fewest players | most players | | | | | | |
| 3 | teams | fewest a team | most a team | u8: uneven teams allowed | | | | |
| 4 | teams | fewest a team | most a team | largest size difference | party size | u8: keep parties together | party sizes kept as one team | unused |
| 5 | teams | fewest a team | most a team | largest size difference | | | | |

Validator rules (`0x00193610`, `0x00193630`, `0x001936a0`), signed:

- 1, 2: `1 <= [0] <= [1] <= 16`.
- 3, 5: `2 <= [0] <= 16`, `1 <= [1] <= [2] <= 16 / [0]`.
- 4: `2 <= [0] <= 16`, `2 <= [1] <= [2] <= 16 / [0]`,
  `1 <= [4] <= [5] <= [2]`, `0 <= [3] <= [2] - [1]`.

Readers: fewest/most players `0x001932c0`/`0x00193300` (teams: `[0]`
times the team size), team size `0x00193340`/`0x00193370`, teams
`0x00193440`, size difference `0x00193400` (type 3 reads `[3]` as a byte
and answers 0 or 1), party-as-team `0x001934d0` with
`0x001933d0`/`0x001933a0` (type 5 always, with the team size; type 4 when
`[6]`, with `[7]`/`[8]`). These feed team assembly (`0x00071730` →
`0x00091fc0`). The game's defaults write `[9] = 2` on one hopper; nothing
reads it.

### 6.5 Data block (0x15cb8 bytes) and entry (0x37c bytes)

| Offset | Type | Field | Readers | Evidence |
|---|---|---|---|---|
| block `+0x00` | 0x44 B | | none | unused |
| block `+0x44` | i32 | entry count; 0 leaves nothing to pick, the game has no upper check but the block holds 100 | `0x00194660`, `0x00192eb0` | confirmed |
| block `+0x48` | 100 x 0x37c | entries | | |
| entry `+0x000` | u32 | | none | unused |
| entry `+0x004` | u32 | map id: a level in the globals tag's multiplayer list (`0x0019c5f0`); an unknown one marks the whole hopper status 2 | `0x00194660`, `0x00192eb0` | confirmed |
| entry `+0x008` | 9 x 32 u16 | map name per language (the defaults copy a compiled-in English name into all nine) | none | unused |
| entry `+0x248` | i32 | weight: session setup picks an entry at random by weight, each clamped to 1..=1000 (0 when the map is unknown) | `0x00192f12` | confirmed |
| entry `+0x24c` | 0x130 B | game variant, the saved-variant record ([variants.md](../editor/variants.md)); copied out whole with the map id for the entry picked | `0x00192fc0` | confirmed |

Quickmatch's start (`0x00231066`), the playlist screen's press
(`0x002b2181`) and the custom-game list
(`0x002b20a4`, `0x002b2198`) read a block only to see that it loads.

The game's default entries build each variant from a stock record per
game type (`0x0019d220`) plus a few changes per built-in matchmaking
variant (`0x00212f40`, 28 of them), then clear settings 12..14
(`0x00193010`). `variant.rs` ports the stock record and the clean-up
limits (`0x0019d650`) a variant is held to.

### 6.6 The game's own defaults

With no file, `0x001940d0` builds seven hoppers (`0x00193c70`): free for
all (type 2, 4..8 players), small team (type 4, 2 x 3..4, parties 1..4
kept together at 3..4), head to head (type 2, 2..2), big team (type 4,
2 x 5..8, parties 1..5), two clan matches (type 5, 2 x 3..4 and 2 x 6..8)
and small team unranked (type 3, 2 x 2..4, uneven teams). Waits are 30 s
base with a 150 s timeout (120 s for head to head and the clan matches),
45 s and 300 s for big team. Titles come from the
`matchmaking_hopper_descriptions` strings (`ffa_title` ...
`small_team_unranked_description`, string ids `0x7a4`..`0x7b1`).

### 6.7 What a session advertises as its hopper

The **descriptor index** (0..15). The matchmaking state keeps the
selected hopper as an index (`[state + 0x49c8]`), passes it straight to
`0x00192e60` (which indexes the 16 descriptors), and writes it into the
session description at `+0x70` (`0x00073530`). The playlist id (`+0x08`)
is separate and goes to stats; an entry's "record id" is a map id. So
`hopper_id` in sessions and pins (`PUT /v1/hoppers/{hopper_id}/pins`,
[maps.md](../game/maps.md)) is the index, which is what the web builder's pins use
(`HopperPins` with `h().index`).

### 6.8 The default set

`HopperFile::default_set()` builds a complete file from nothing: eleven
playlists in hoppers 0..10, playlist ids 1..11 (eight human playlists, then
three clanker ones), status 0, the game's skill
ceiling and wait/timeout values, the start-wait table computed as the game
computes it, only retail maps (map pack maps are left out: an unknown map
errors the whole hopper and the format has no "only when installed" flag),
and variants from the stock records.

| # | Title | Type | Size | Entries |
|---|---|---|---|---|
| 0 | Rumble Pit | 2 | 4..8 players, alone | Rumble Slayer, Oddball, King of the Hill |
| 1 | Team Slayer | 4 | 2 x 3..4, parties 1..4 (3..4 kept as a team) | Team Slayer |
| 2 | Team Skirmish | 4 | 2 x 2, parties 1..2 (2 kept as a team) | Team Slayer, Teamball |
| 3 | Team Snipers | 4 | 2 x 3..4, parties 1..4 | Team Snipers |
| 4 | Big Team Battle | 4 | 2 x 5..8, parties 1..5 | Team Slayer, Classic CTF, Assault, Territories |
| 5 | Team Objective | 4 | 2 x 3..4, parties 1..4 (3..4 kept as a team) | Classic CTF, Assault, Territories, Team King, Teamball |
| 6 | Free for All | 1 | 2..8, parties welcome | Rumble Slayer, Oddball, King of the Hill |
| 7 | Unranked Teams | 3 | 2 x 2..4, uneven allowed | Team Slayer, Classic CTF, Team King, Teamball |
| 8 | Clanker Slayer | 1 | 2..8, the rest bots | Rumble Slayer |
| 9 | Clanker Teams | 3 | 2 x 1..4, uneven allowed, the rest bots | Team Slayer |
| 10 | Clanker Doubles | 3 | 2 x 1..2, uneven allowed, the rest bots | Team Slayer |

Hoppers 8..10 are **clanker hoppers** (Clankermatch, docs/game/bots.md
§8): one human or a party, and bots the host runs in every other seat.
Titles fit the descriptor's 16-byte title slot (15 bytes and a NUL), so
"Clanker Doubles" stands for 2 x 2. They are unranked types because the
ranked types' rules are for filling a game with people (ranked teams need
two a team and bound parties; ranked free for all is solo); a lone human
with bots needs a team of one and uneven teams while the bots join. The
server still ranks a Clankermatch game from its record (below). The mark
is not a game field: it is `Playlist::clankers` in the server's source
(`Clankers { ranks_as }`, bots.md §9.4 item 1), all three with their own
standing (`ranks_as` unset).

`GET /v1/hoppers/clankers` (read like `/v1/hoppers/pins`) serves the marks
as `ClankerHoppers` (`crates/live-proto/src/hopper/clankers.rs`):
`{"hoppers": [{"hopper_id", "playlist_id", "players", "teams",
"ranks_as"?}]}`, by descriptor index, for the hopper file served now. A
published file keeps a mark only where it has the default's playlist id
at that index; with no file, or one that does not parse, the list is
empty. live-hle reads it for the Clankermatch row's filter, the search
and the lobby fill. A game recorded with `kind: "clankermatch"` (and the
hopper's `playlist_id`) is its own kind in the read model and the stats
totals (`clankermatch`, apart from `matchmaking`), counts in its playlist
and every career number, and ranks like a ranked matchmaking game
(§8.4, the same arbitration round) in its clanker hopper's playlist, or
`ranks_as`'s; the game's stats read (§8.3) answers a clanker hopper's
playlist id with that standing.

Tests check it against the validator port, the per-entry checks
(`HopperFile::problems`: retail map ids, weights, the variant clean-up),
a byte-exact build → parse → build, and that every field reads back from
its offset. The server serves it while no file is published (§6.9);
`h2 live hopper defaults --out FILE [--raw]` writes it.

### 6.9 Serving

`Live::file` answers Halo 2's `multiplayer_hopper.dat` with the default
set while none is published (`crates/live-server/src/default_hopper.rs`),
so the game's title-storage download, `GET /v1/titles/{title}/hopper` and
the builder's `PUT` (which then publishes the edited default) all work on
a fresh server. Nothing is written to the log until someone publishes;
`h2 live hopper import` still replaces it with the owner's own file.

### 6.10 Building playlists, and custom maps in them

The 16 slots are fixed; a slot of type 0 is no hopper, and the default
set leaves indexes 11..15 so (playlists 12..16 in the builders). The web builder (`web/src/admin/HopperBuilder.tsx`)
and the editor's playlist tool (`hopper_edit::model`) work the same way:

- **A new playlist** is made in an unused slot from a template
  (`defaults::templates`, the eight human playlists of §6.8): its whole
  descriptor (type, sizes, waits, timeout, skill ceiling, a free playlist
  id, `HopperFile::create_from`) and one entry, the template's first. The
  raw fields cannot build a descriptor the validator takes (the skill
  ceiling and most type parameters are not in them), which is why a title
  typed into an unused slot used to fail with "type must be 1 to 5" and
  "1 to 100 entries, got 0". "Change kind" applies another template to a
  playlist, keeping its text and entries. A playlist can be cleared back to
  an unused slot (`HopperFile::clear_hopper`).
- **Only changed playlists are checked**, in both editors and on the
  server, so untouched unused slots never block a publish; each problem is
  shown at its playlist with a revert or discard action.
- **An entry's game variant** is picked per entry: a built-in one
  (`defaults::builtin_variants`, keyed; the server writes its 0x130 bytes)
  or one already in the file (its slot is copied with `from`/
  `from_hopper`). The map is any map of the catalog (or `RETAIL_MAPS`).
- **Custom maps** are allowed per playlist by pins (`HopperPinsSet`,
  [maps.md §5](../game/maps.md)); the builder's "Custom maps allowed"
  section says, for each pinned map, whether the playlist has an entry on
  its base map and offers to add one.

What a playlist cannot do yet: **make a game play a custom map.** An
entry is a map id and a variant; the format has no room for a `MapId`, and
session setup (`0x00192e60`/`0x00192eb0`) picks an entry and loads
`maps\<base>.map`. A pin permits a custom content ID in the session
advertisement. Custom games pick the package's separate slot under Mods.
For a
playlist entry to choose a custom map, three pieces are missing:

1. **Data:** a per-entry binding kept by the server beside the file (for
   example pins extended to `(hopper_id, entry index) -> MapId`, signed
   and replayed like `HopperPinsSet`), served with the pins.
2. **Runtime:** a hook at the host's entry pick (after `0x00192eb0`
   chooses the entry, before the map load) that reads the binding, makes
   the package ready through `MapGate` (download, base check) and selects
   its content offer for that game.
3. **Fallback:** what a host that cannot get the package does (play the
   base, or skip to another entry), and that every peer agrees before the
   load, since lockstep diverges on any byte.

## 7. OptiMatch search: procedure 1, decoded

Read statically from the 5849 XBE; nothing was run. The code that applies
it is `crates/live-proto/src/optimatch.rs`; the HLE decodes the game's
parameters (`crates/live-hle/src/sessions.rs`, `Op::Search`) and the
server filters `GET /v1/sessions?optimatch=` with it.

### 7.1 What a session advertises

Every session the title creates or updates carries fifteen integer
attributes, built by `0x000b4b80` from a seven-dword block that a virtual
method of the session object fills (`0x000733f0` for playlist sessions,
`0x00073860` for custom games):

| id | playlist session (`0x000733f0`, from `0x00073530` and `0x0007e210`) | custom game (`0x00073860`) |
|---|---|---|
| 1 | descriptor index (`[state+0x49c8]`, §6.7) | -1 |
| 2 | mean level of the session's players | -1 |
| 3 | public slots open | public slots open |
| 5 | lowest level, raised to the floor of the highest player (below) | -1 |
| 6 | highest level | -1 |
| 7 | time since the lobby formed (`0x00321aae() - [state+0x78b4]`) | 0 |
| 8 | room mask: bit `n-1` set when a party of `n` fits | `(1 << open) - 1` |
| 9..15 | bit 0..6 of id 8, as 0 or 1 | same |
| 16 | 1 when any bit 7+ of id 8 is set | same |

Levels here are the per-player values the lobby summary (`0x0007e210`)
averages, maxes and mins over players whose flags `& 3` are clear. For a
playlist session it computes the room mask itself from the hopper's
party and team rules (`0x00091f00` per party size; the unranked
fallback is all sizes up to the open count).

The **floor** of a level `L` is `0x001931a0(hopper, L)`: walk down from
`L` while the hopper's skill ceiling table (`+0x51c`, §6) still lets the
lower level play with `L`; the floor is the lowest such level. So
`ceiling[i] >= L` for every `i` from the floor up to `L - 1`.

### 7.2 What a search sends

`0x00090880` starts a search from the party state (`0x00070ea0`, the
OptiMatch screen): the playlist, the party size `[+0x7b4]`, and the
party's mean, highest and lowest level (`[+0x960]`, `[+0x958]`,
`[+0x95c]`, summed at `0x00072540`). It precomputes, with `hi` the
party's highest level and `lo` its lowest:

- `floor = floor(hi)`; `low = max(lo, floor)`; `cap = ceiling[low]`;
- `max_widen = max(low - floor, cap - hi)`; the widening step starts at 0.

`0x00090b30` then builds procedure 1's twelve parameters through
`0x000b4a90` and calls `XOnlineMatchSearch(1, 50, params, 12, ..)`:

| # | value | compared with |
|---|---|---|
| 0 | playlist (descriptor index) | id 1, equal |
| 1 | party mean level | id 2: results ordered by distance |
| 2 | `max(low - step, floor)`, or null when the step is -1 | id 5 must be at least this |
| 3 | `min(hi + step, cap)`, or null when the step is -1 | id 6 must be at most this |
| 4..10 | 1 at position `3 + n` for a party of `n` (1..7), else null | id `8 + n` must be 1 |
| 11 | 1 for a party of 8 or more, else null | id 16 must be 1 |

The custom games browser (`0x000b3100`) uses the same call with
playlist -1, mean 0 and every other parameter null, which finds custom
games (id 1 is -1) and nothing from a playlist.

The comparison column is our reading: the parameters pair one-to-one
with the advertised ids, and the window is exactly the levels the
hopper's ceiling table lets the party play with. Retail Live's stored
procedure is not in the image; the ordering by mean level is our
choice.

### 7.3 Widening, and who hosts

The game widens, not the server. `0x00090f70` runs after each search:
while the step is below `max_widen` it adds one and searches again; at
`max_widen` an unranked playlist (type 1 or 3, `[+0x14]` clear) sets
the step to -1, which drops the window entirely, and a ranked one stays
at the widest window. A search also repeats while it returned a full
page (50). When the searches end, `0x00091400` picks a result and joins
it (`0x00059e50`); with none, the party state moves on with
`0x0005a220(6)`, which is where the console goes on to host its own
session (likely; the hosting branch is the next thing to read). So
"find or form" needs no server matcher for OptiMatch: the server
filters and orders, and an empty answer makes the searching console the
host that the next searcher finds.

### 7.4 Server and HLE

- `live_proto::optimatch::Query` is the decoded search: `hopper`,
  `average`, `lowest`/`highest` (the window, absent when widened out),
  `party`. `Query::from_params` reads the twelve parameters; any other
  shape falls back to an unfiltered search, as before.
- `GET /v1/sessions?optimatch=<json>` keeps the listed sessions that
  match and returns up to `limit`, closest mean level first, as one page
  (`next` is null). The other filters still apply.
- A session the title did not advertise itself (no id 1, such as a co-op
  lobby) only shows to the browser's -1.
- The HLE logs each distinct search once per 30 s: `live: optimatch:
  searching playlist P for a party of N, mean level M, levels A..B` (or
  `any level`), so a run log shows the window widen.

Tests: `optimatch` unit tests in live-proto (parameter round trip,
window, room, browser, ordering), the router test
`optimatch_searches_filter_by_playlist_window_and_room`, and the guest
test `optimatch_searches_find_sessions_in_the_partys_level_window`
(two synthetic guests through `start_embedded`).

## 8. Ranks: what the game does, and what the server does

The stats site's **Ranked** page at `/games/halo2/ranks` builds its
playlist menu from the served hopper file and the Clankermatch marks.
The menu groups Ranked, Unranked and Clankermatch. The selected playlist
shows its game types and size rules. The board shows levels and XP
progress and marks bot accounts. Unranked playlists have no standings
and can link to recent games. [Ranked page](../../web/src/stats/Ranks.tsx)

### 8.1 In the retail game (static reading)

Read statically from the 5849 XBE; nothing was run. Addresses are guest
VAs.

- **Where a rank lives.** Xbox Live stats: one leaderboard per playlist,
  whose id is the descriptor's playlist id (`+0x08`, §6), with six stats,
  ids 1..6. Stat 1 is experience (0..2^30-1), stat 2 the level (stored
  0..49; the 1..50 shown is likely stored + 1), stat 3 the highest level
  reached (likely), and stats 4..6 per-player game counters whose meaning
  is not decoded.
- **The cache.** `0x00074aa0`, the stats manager, keeps a per-user,
  per-hopper rank cache at `0x00509454` (4 users x 16 hoppers). For every
  valid hopper of type 1..5 not yet read, or stale for longer than the
  network configuration's re-read age, it adds a `XONLINE_STAT_SPEC`
  (leaderboard = playlist id, stats 1..6) to its one `XOnlineStatRead`
  batch, beside the recent-players read on leaderboard `0x61`
  (`0x00080940`). Results return through `0x00092330`; `0x00075790` maps a
  leaderboard back to a hopper. `0x00073a10` reads a cached stat
  (0 experience, 1 level).
- **Into the lobby.** `0x00065530` copies the selected hopper's level and
  experience (and the best level across ranked hoppers) into the local
  player's properties block (`0x004cd918 + user * 0xd0`): hopper index
  at `+0x84`, level at `+0x8a`, experience at `+0x8c`. The session's
  player record carries those properties at `+0x18`, which is how
  `0x00072540` reads them at record `+0x12c/+0x132/+0x134` (§7.2). The
  network encoder `0x0007c5a0` sends the level in 7 bits and experience
  in 30.
- **After a ranked game the console works out the change itself.**
  `0x00073ca0` (only for the hopper and round that were arbitrated)
  builds per player: experience earned or lost against each opponent by
  level difference, scaled by the player's level, averaged over
  opponents and clamped (`0x00074800`, `0x00074720`, `0x000746b0`); the
  level from experience through a threshold table with hysteresis (a
  one-level drop only below the midpoint, `0x00074970`). The tuning
  tables come from the downloaded `network_configuration.dat`
  (`0x004ce040`, defaults from `0x00066330`), not the hopper file. It
  updates its own cache, marks it stale (re-read soon after) and sends
  the result with `XOnlineArbitrationReport` (`0x000925c0`): a flat stat
  array per player on the playlist's leaderboard (and playlist id + 0x20,
  + 0x40) and stat procedures `0x8001`, `0x8003` and `0x8007` (shapes
  read, meanings not mapped). Experience and counters likely go as
  increments, the level as an absolute value.
- Not found yet: the lobby, post-game and service-record rank icon
  readers.

### 8.2 On this server

The server is authoritative: it ranks from the games log, not from the
console's arithmetic. The rules are in `crates/live-proto/src/rank.rs`:
the game's mechanism (points per opponent by level difference, scaled
by level, averaged, clamped to ±200; level from a threshold table with
the same drop rule), with our own numbers rather than the game's
tables. Every player starts each ranked playlist at level 1.

- When a game batch ends a game (`GameEnded`) with `reason: "finished"`,
  `kind: "matchmaking"` and a `playlist_id` (the hopper descriptor index)
  whose hopper is ranked (type 2, 4 or 5) in the served hopper file, the
  same request ranks it: one server-signed `RankChanged`
  `{xuid, playlist, game_id, before, after}` on `rank-<xuid>` for each
  player with an account whose standing moved. Players who left before
  the end count as last. A game is ranked once. Nothing runs in the
  background.
- No one else may write `rank-*` streams or `RankChanged`.
- The read model keeps every standing per playlist and the last 100
  changes per player and playlist; a replay rebuilds them from the
  events without re-running the rules.
- Reads: `GET /v1/accounts/{who}/ranks`,
  `GET /v1/accounts/{who}/ranks/{playlist}` and `GET /v1/ranks/{playlist}`
  ([live-api.md](live-api.md#ranks)). The stats site shows them at
  `/games/halo2/ranks/{playlist}` and `/games/halo2/players/{tag}/ranks`.

### 8.3 The game's side: stat reads, playlist ids, arbitration

**Stat reads.** `XOnlineStatRead` asks the server when a query names a
leaderboard 1..100 (a possible playlist id) and any of stats 1..6:
`GET /v1/rank-stats` ([live-api.md](live-api.md#ranks); the shared router, so
the Worker answers it too) with every such leaderboard and player,
through the Live worker, so the call never waits. The task reports 0
until the answer comes; `XOnlineStatReadGetResult` answers
`RESULTS_UNAVAILABLE` until then.
Rank stats come back as LONGs from `rank::live_stat`: 1 experience, 2
the level and 3 the highest level, both stored from 0 as the game does,
and 4..6 (counters not decoded) as 0. Everything else, and every stat
of a read the server failed, is NONE, which the game reads as zero:
level 1, as before reads reached the server.

**Playlist ids on games.** At each multiplayer start telemetry asks the
boot's playlist locator, which asks `Hle::playlist`: the hopper index
(session attribute 1) of this console's own advertisement, else of the
session it joined, as last read. Neither waits nor takes guest memory.
A game with one is `kind: "matchmaking"` with that `playlist_id`; the
server names the playlist from its hopper file. The rules in §8.2 then
rank it.

**Arbitration.** Read from the retail call sites, not yet checked
against a running ranked game:

| entry | arguments | what we do |
|---|---|---|
| `CreateRoundID` `0x003adc4c` | out pointer | eight random bytes, nonzero |
| `Register` `0x003ae32b` | pointer to 16 bytes (session id, round id), a duration-like word, flags (`0xc` or `0x1c`), event (0), task out | posted to the server (§8.4) |
| `ExtendRound` `0x003ae33a` | the same 16 bytes, a duration, event (0), task out (likely) | posted to the server |
| `Report` `0x003ae36a` | the same 16 bytes, count (≤ 1000), `0x88`-byte stat procedure records (u16 id `0x8001`/`0x8003`/`0x8007`, a pointer at `+0x18`), 0, flags (bits 0 and 1), event (0), task out | posted to the server with the procedure ids; their data is not decoded |

The HLE also keeps every call (`Hle::arbitration`, the last 256),
owned, with no guest pointers. §8.4 has how the server uses them.

### 8.4 Slice 3: bots in ranks, and arbitration

Status: agreed with the coordinator and the bots lane (2026-11-18;
their side is [bots.md](../game/bots.md) §9.4). The arbitration half is
built. Clankermatch uses its own game kind and the hopper's
`ranks_as` mark to resolve the leaderboard. Bot accounts and their
events are described in [bots.md](../game/bots.md) §9.

The user's rules (2026-11-18):

1. Bot stats are recorded separately but counted together for now.
2. Bot rank is matched to yours.

#### Bots in ranks

**Who is ranked.** A finished ranked game (`kind` `matchmaking` or
`clankermatch`, a ranked playlist) ranks every entrant through the same
`rank::apply`, bots included: a bot is an opponent like any other
(rule 1, counted together).

- A **persistent bot** (`persistent: true` in its roster) has a standing
  of its own that rises and falls like a human's. Its change is a
  `RankChanged` on `rank-<bot xuid>` with `bot: true`, which is how it
  is recorded separately.
- A **made-up bot** (`persistent: false`) has no standing to keep. It
  counts as an opponent at the level its roster showed, and gets no
  `RankChanged`.
- Host-made bots (`live_proto::bots::is_local_xuid`, System Link and
  offline) never reach a ranked game: the server refuses them in a game
  record (bots lane).

**Separate but together.** The read model keeps bots' standings in the
same per-playlist table and tags them as bots. Leaderboards
(`GET /v1/ranks/{playlist}`), rank history and the stats site include
them with `bot: true` on the row, so a later "humans only" view is a
filter, not a migration. Career totals count them as well; `kind:
"clankermatch"` keeps those games apart in the games log.

**Clanker playlists.** The server publishes clanker playlists as their
own hoppers (bots.md §8.3), each with a playlist id of its own. The
mark is in the server's hopper source, not in the game's descriptor:
`clankers: { ranks_as: Option<u32> }` (bots.md §9.4). `ranks_as` is
the playlist id of the human hopper whose standing it shares; that
hopper is its *rank playlist*. A clankermatch game is ranked under the
rank playlist, and `GET /v1/rank-stats` answers the clanker playlist's
id with the rank playlist's standing, so the lobby shows one rank for
both (rule 1). With `ranks_as: None` the clanker hopper is its own rank
playlist. (An earlier draft shared the descriptor id `+0x08` instead;
two descriptors with one id is untested against the game's validator.)

**Matched to yours (rule 2).** The roster picks bots for the human's
standing in the rank playlist:

- `rank::bot_skill(level) -> f32`, `(level - 1) / 49`: 0 at level 1, 1
  at level 50, continuous, as bots.md §8.2 asks. The roster's `skill`.
- `Ranks::nearest(playlist, level, n, bots) -> Vec<Xuid>` (built): up to
  `n` of the persistent bot accounts `bots`, closest level first, then
  fewest games; a bot that never played the playlist stands at level 1.
  The roster endpoint passes its bot accounts, and fills any gap with
  made-up bots shown at the humans' level.
- With more than one human, "yours" is the mean level of the humans in
  the match.

The roster's `rank.level` is then the bot's real level (persistent) or
the humans' (made up). A made-up bot has no standing, so its stat read
needs the session. `GET /v1/rank-stats` takes an optional `session=`
(16 lowercase hex digits, else a 400). The HLE fills it on the stats
read with the session this console hosts, else the one it joined. It
answers a made-up bot of that session's latest roster with its roster
level, when the roster's playlist is the leaderboard's, so the lobby's
stat read and the roster agree. A persistent bot answers with its
standing, as anyone else does.

**The bots lane's lookups** (bots.md §9.4, built), on `State.bots`:

- `bot(xuid) -> Option<&Issued>`, from `BotIssued` on `bot-<xuid>`;
- `rostered(session_id, xuid, before) -> Option<(&Rostered,
  &RosteredBot { persistent, level, team, skill })>`, from
  `BotsRostered` on `bots-<session_id>`, keyed by the same
  `NewGame.session_id` that arbitration uses. `Rostered.playlist` is
  the roster's playlist.

A session can roster more than once (one game after another). The
roster `rank_game` reads should then be the one in force when the game
started: the latest `BotsRostered` before its `GameStarted`. The game
record's acceptance check (bots.md §9.3) refuses any `bot: true` entry
that is not rostered, so `rank_game` sees only accepted bots.

**Built** (`crates/live-server/src/ranks.rs`, tested in
`tests/ranks.rs`):

1. In `rank_game`: an entrant rostered in the game's session, in the
   roster in force at its `GameStarted` (`before` is the game's
   position), needs no arbitration registration. A persistent bot is
   ranked with `RankChanged { bot: true }`. A made-up bot is an
   opponent at its roster level and gets no event.
2. `GET /v1/rank-stats` takes `session=` for made-up bots; the HLE
   sends it.
3. Leaderboard rows, `GET /v1/accounts/{bot}/ranks` and its history
   carry `bot: true` for a bot, with the bot's gamertag; a bot's xuid
   or gamertag finds its pages.

**Still to build, once the bots lane has them:** `kind:
"clankermatch"` (not yet in `GAME_KINDS`) ranked under its rank
playlist, and clanker playlist ids answered on `GET /v1/rank-stats`
with the `ranks_as` standing (the mark is not in the hopper source yet).

#### Arbitration (built)

A round is linked to a server game by its session, so this does not
need `Hle::set_game_id`. The 16 bytes the game passes to Register,
ExtendRound and Report start with the session id, which is also the
Live session's id. Ids are written as the game's 8 bytes in order, 16
lowercase hex digits.

1. **Game record.** `NewGame.session_id`, matchmaking games only. The
   boot's playlist locator answers `Hle::matchmade`: the playlist and
   the session, both from this console's own advertisement, else from
   the joined session as last read. Telemetry puts both on the game.
2. **HLE to server.** Register, ExtendRound and Report post through the
   Live worker, never waiting, once for every account signed in on the
   console:
   `POST /v1/arbitration/{session}/{round}/register` `{duration, flags}`,
   `.../extend` `{duration}` and `.../report` `{flags, procedures}` (the
   procedure ids only, until their data is decoded). The task reports 0
   until the server answers. It then succeeds, or completes with the
   failure's status as the Live worker maps it (an unreachable server is
   `CANNOT_ACCESS_SERVICE`). With no server (System Link, offline), the
   calls are only kept and complete at once.
3. **Server.** `ArbitrationRegistered`, `ArbitrationExtended` and
   `ArbitrationReported` are server-signed on
   `round-<session>-<round>`, carrying the token's account as `xuid`.
   Nobody posts them through `/v1/events`. The session must be one the
   server knows (404 otherwise). Any account may register its round:
   the session's `players` are not checked, because the HLE does not post
   session membership (`PlayerJoined` on `session-<id>`) yet. A
   registration alone ranks nobody; see the next item.
   - A second registration or report by the same account writes
     nothing.
   - Extending or reporting a round the account did not register is a
     409.
4. **The check, in `rank_game`.** Only the session host's own game
   record is ranked: its `GameStarted.host` must be the session's
   `host_xuid`. The server trusts that record (who played, who won); the
   host is the console that ran the game. A player can be added to it
   only by the host, or by its own account (a guest re-joining); only
   the host ends it; an xuid is in it once (live-api.md, Clarifications
   10). The game's round is the latest round in its session that the
   host registered before the game's `GameStarted` and that has not
   ranked a game yet. `ArbitrationUsed` on the round's stream marks it
   spent once the game is ranked, even when nobody's standing moved (one
   side only, or fewer than two ranked entrants), and
   `RankChanged.round` (`{session}-{round}`) names it, so a round ranks
   one game. Only the entrants who registered that round
   are ranked, and only they count as opponents. A game with no
   `session_id`, no such round, or fewer than two ranked entrants is
   not ranked. There is no switch to rank without arbitration: every
   client is this recomp.
   - Rostered bots need no registration, because the host vouches for
     its bots (bots.md §9.2, §9.3); see "Bots in ranks".
   - Reports are recorded and linked, but not yet compared. Comparing
     them needs the `0x8001`/`0x8003`/`0x8007` records decoded.
5. **Order.** A report may land after `GameEnded`. Ranking runs at
   `GameEnded`, on the registrations, which the game makes before play
   starts.

What this stops: a host's record can no longer move a human's rank
unless that human's own console registered the round. What it does not
stop yet: a host misreporting the score. That waits for the decoded
reports.

**Game-run check (the user).** Run with `H2_BOOT_VERBOSE=1` and play a
ranked matchmaking game on two consoles. Each console logs
`live arbitration: session <id> round <id> (advertising <id>): …` for
Register, and again for Report.

- On the host, `session` should equal `advertising`.
- On the joining console, `session` should be the host's `advertising`
  id.
- The game's record on the server should carry that same `session_id`,
  and the game should rank both players.

If `session` matches neither, the first 8 bytes are not the session id.
Then rounds cannot be linked this way, and nobody is ranked: send the
log lines.

## 9. Ping-aware OptiMatch assignments

Enable the queue on each participating game client with
`H2_LIVE_PING_MATCHMAKING=1`. The HLE uses it for solo OptiMatch searches.
Parties, Clankermatch and the custom-games browser use their existing
search paths. The server uses the active hopper's **minimum** player count
and the entrants' current server ranks for that hopper. Team hoppers fill
equal-sized teams by distributing the highest-to-lowest ranks in alternating
team order. The planned teams are in the assignment. The retail title still
owns the final in-game team placement.

Clients keep the existing XNet QoS responder available and measure each
candidate's actual round trip at its XNet base port plus two. This uses the
[co-op echo](live-coop.md#106-ping) with a 450 ms reply window. It leaves the
title's separate lobby-data lookup intact. Set `H2_LIVE_NAT` to `open`,
`moderate` or `strict` and `H2_LIVE_UPLOAD_KBPS` to the known upload rate.
Omit unknown values. The HLE does not infer NAT type or capacity from a
successful small-packet echo.

A cohort needs one eligible host that every member can reach. The oldest
matchable ticket starts with a five-level skill spread and a 90 ms RTT
limit. Every five seconds of wait add three levels and 20 ms, up to 49
levels and 350 ms. A strict NAT or known upload below 1,000 kbit/s makes a
candidate ineligible to host. Missing measurements wait for a probe.
Explicit timeouts exclude that route. When all routes fail or exceed the
maximum window, the clients continue searching until the search ends.

Once Live has formed a cohort and planned its teams, it minimises worst
RTT, then median over remote members, then host XUID. A registered
[dedicated host](dedicated-host.md#routing-assignment-and-arbitration)
wins over a peer only by improving worst RTT. The chosen peer receives
the empty search that lets the title advertise its lobby. The other
clients keep their async searches pending until that advertisement and
its title QoS lobby data are ready. Opted-out clients continue to search
the legacy lobby list without discovering queued assignments.

### Queue API

Both the native router and the Worker's LiveHub serve the same routes.
Each write uses a fresh `sessions.write` token.

- `PUT /v1/matchmaking/queue` refreshes the signed account's ticket with
  `hopper`, the XNet base `port`, optional `connectivity` and `samples`.
  The server takes the IPv4 address from the connection. The `probing`
  response gives each target's XUID, endpoint and ticket generation.
  Report `target`, `generation` and `rtt_ms` for each target. Use
  `rtt_ms: null` for a timeout. Replies from a replaced endpoint or
  expired generation are ignored.
- An operator can enable a service account as a dedicated candidate with
  `PUT /v1/matchmaking/dedicated/{xuid}`. That account keeps its endpoint
  available by refreshing a ticket with `dedicated: true`. It occupies no
  entrant seat. Dedicated registration lasts one hour and is renewed by the
  operator. The game HLE queues as a player. A dedicated process must use
  the service API and bring up a compatible game session when elected.
- An elected account receives `state: host` and an `assignment` from the
  queue. It publishes with the normal `POST /v1/sessions` and the header
  `X-H2-Match: <assignment.id>`. Live checks the publishing identity,
  probed endpoint, hopper and available seats. Retrying with the same ID
  returns the existing session. Members receive `state: waiting` until
  the session is published, then `state: join` and its session ID.
- Tickets expire after ten seconds without a refresh. An assignment is
  reserved for sixty seconds. Each hopper admits up to 32 live candidates.
  Tickets, assignments awaiting publication and dedicated registration
  are transient, so a restart starts a fresh search. Published sessions
  retain `matchmaking` in their signed `SessionCreated` event and session
  detail. It includes the host, whether it was dedicated, the roster,
  planned teams and the per-member RTT table. A peer host has zero RTT
  to itself. The public debug record omits network addresses.

### Ad-hoc dedicated hosts

Set `LIVE_DEDICATED=1` in the native server environment or in the Live
Worker's variables. Set `LIVE_DEDICATED_HOPPERS` to a JSON object keyed
by hopper index. A hopper is opted in when its entry supplies the map
scenario and optionally a variant. For example, enable hopper 1 with
`{"1":{"map":"lockout","variant":"Team Slayer"}}`. The global switch
and each individual hopper default to off. Choose a map and variant
supported by the playlist and the host's data bundle.

For regional placement, set `LIVE_HOST_REGIONS` to a JSON object
mapping provider region codes to IPv4 QoS base endpoints. For example,
`{"lhr":"192.0.2.10:31000","ams":"192.0.2.20:31000","iad":null}`.
Each endpoint answers the same UDP echo as a peer at base port plus
two. Use `null` for a region without a probe. Set
`LIVE_HOST_DEFAULT_REGION` to a key in that list. If both region
settings are omitted, Live leaves placement to the provider.

The queue includes configured regional probes in its existing
`probing` response when there is room within the candidate limit. Live
uses measurements from the current probe generation. Among regions
measured successfully by every cohort member, it chooses the lowest
worst-member RTT. Ties prefer the configured default and then region
code order. If no region has measurements from every member, Live
uses the configured default. Regional probes occupy no player seats.

Live reserves a cohort with an eligible peer fallback and requests
the dedicated host as soon as the cohort forms. The forming request
waits for the allocation result while other members can keep polling.
This lets the Worker schedule the first lease sweep even if polling
stops. The playerless host occupies a machine slot, so an ad-hoc
assignment can take at most 15 solo entrants. Cohorts reachable only
through a registered host use the existing registered-host path.
All ad-hoc members receive `waiting` during boot. Live binds
`host_lease` before starting the process, so the host can publish its
lobby under that lease. The dedicated lobby has a session ID distinct
from the assignment ID.
After `HostReady` supplies the session ID, the queue sends `join`
with that ID and `frames_from_host: true`.

If allocation fails, including when the budget is exhausted, or if
the host fails to register within 60 seconds of cohort formation, the
same assignment ID, roster, and teams move to the reserved peer. The
elected peer receives `host` and publishes as usual. The other players
keep polling the same search and join when the peer's lobby is
available, with `frames_from_host: false`.
