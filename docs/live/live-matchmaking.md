# Matchmaking and custom games over Live

Status: phase 1 server sessions and HLE invitations are implemented. Tests use
router memory stores and two synthetic guests against `start_embedded`, through
HTTP delivery, acceptance, FindFromID, QoS and bidirectional UDP. The owner
has Live working in the game (2026-11-17).
Search also returns real sessions, with the anonymous procedure-1 filters still
unmapped. The server matcher, ranked arbitration and NAT traversal remain design.
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
   `xmatchclient.srf` search. *To confirm:* the procedure values the game
   passes and the attribute ids it queries with (read `0x0008fa80`).
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
overrides ([live-motd.md](live-motd.md) §1.2). All of these are stops
today (stat reads answer `STAT_USER_NOT_FOUND`, query searches
`QUERY_DATASET_NOT_FOUND`); none is needed for the first phases.

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
| Attribute list written at session create/update; how the host address and key id are published | `0x0008fcc0`, `0x0008fe20`, `0x0008fd90` |
| Search procedure values and query attributes | `0x0008fa80` |
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
  {2, 4, 5}; the matchmaking start flow (`0x00231066`) reads the selected
  hopper's block, and the session state machine (`0x00192eb0`, states
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
  Session records, seven-column parsing and accepted-invite packing are checked;
  procedure-1 positional filter meanings still need identification.
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

Matchmaking start (`0x00231066`) and the custom-game list
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

`HopperFile::default_set()` builds a complete file from nothing: eight
playlists in hoppers 0..7, playlist ids 1..8, status 0, the game's skill
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
