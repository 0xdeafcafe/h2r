# Matchmaking and custom games over Live

Status: design, nothing implemented. Every matchmaking entry on the Live
boundary is still a named stop ([live.md](live.md), remaining-stop table),
and the server has no session state. This page sets out, in order: what the
retail game calls on the matchmaking and custom-game paths and in what
order, how the hopper file drives it, a server design in the event-sourced
style of [live.md](live.md) §4.3 and [live-api.md](live-api.md), and a
phased plan. Sources: [live-boundary.md](live-boundary.md) (addresses,
confidences), [live.md](live.md) (Client HLE sections), [live-motd.md](live-motd.md)
and [live-motd-fetch.md](live-motd-fetch.md) (hoppers), [live-security.md](live-security.md)
and [live-api.md](live-api.md) (service contracts).

Retail context: session advertise/search/delete were three services behind
the security gateway — `/xmatch/xmatchhost.srf`, `xmatchclient.srf`,
`xmatchhostdelete.srf` ([live.md](live.md) §2) — which map one-to-one onto
`XOnlineMatchSessionCreate` / `XOnlineMatchSearch` /
`XOnlineMatchSessionDelete`. Under decision C (replace the API boundary) our
HTTP service replaces those services; nothing here speaks their wire format.

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
reads message properties through the same `0x0008e9e0` reader as the title
files; its remaining tags (`0x481`..`0x791`) serve the friends/invite UI
([live-motd-fetch.md](live-motd-fetch.md) §3).

On the receiving side: the invite notification arrives via
`XOnlineGetNotificationEx` (`0x003a0798`) and presence carries the session
XNKID by value in `XOnlineNotificationSetState` (`0x003a0774`); the player
answers with `XOnlineFriendsAnswerGameInvite` (`0x003a0811`, wrapper
`0x0008d050`) or `XOnlineGameInviteAnswer` (`0x003a834e`, wrapper
`0x0008d0b0`); `XOnlineFriendsJoinGame` (`0x003a081c`, UI `0x002395dc`)
joins the friend's session; and the menu polls
`XOnlineFriendsGetAcceptedGameInvite` (`0x003a0827`, site `0x0006d211`).
Today the HLE clears that call's 156-byte output and returns `E_FAIL`
("no invite"); the poll's check at `0x0006d218` treats *every nonnegative*
HRESULT as an invite, so a real invite must fill the record, not just
return success ([live.md](live.md), menu audit). *To confirm:* the invite
message's property set and the 156-byte accepted-invite record layout
(read `0x0008f7e0`, the `0x481`..`0x791` tag uses, and `0x0006d210`).

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
| Invite message properties; the 156-byte accepted-invite record | `0x0008f7e0`, `0x0008eda0` tag table (`0x0008ee44`), `0x0006d210` |
| Hopper types 1–5 and the meaning of the descriptor ranges | `0x00065530` (playlist UI filter), `0x00192eb0` (session setup states 6/7/8), `0x00231066` (matchmaking start) |

## 2. How hoppers drive matchmaking

`multiplayer_hopper.dat` is the playlist file, delivered after sign-in as a
message attachment of file class 4 (envelope and delivery:
[live-motd.md](live-motd.md) §5, [live-motd-fetch.md](live-motd-fetch.md)).
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
  valid file, whose record templates (`0x00444fc0`..`0x00445258` shapes)
  are the remaining format work.
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

Retail invites are messages, so reuse the message path the HLE already
serves (the title-file inbox). New stream `mailbox-<xuid>`:

- `GameInviteSent` `{from, session_id}` — written by the sender's account
  key through `POST /v1/events`. This needs one new stream-permission rule:
  today an account writes only its own streams; allow any signed-in account
  to append `GameInviteSent` to another account's mailbox when the session
  exists and is open (checked against the projection, rate-limited per
  author).
- `GameInviteAnswered` `{by, session_id, answer}` — written by the
  recipient, same route.

The HLE surfaces invites through `XOnlineMessageEnumerate` /
`XOnlineGetNotificationEx` (the title-file queue already drives both;
extend it to mix mailbox events in, refreshing at enumeration time the way
details/downloads re-fetch current bytes) and fills
`XOnlineFriendsGetAcceptedGameInvite`'s 156-byte record once §1.5 pins its
layout. The join is then the game's own flow: FindFromID → register key →
connect. Friend requests (`XOnlineFriendsRequestEx`) can later ride the
same mailbox; not needed here.

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
- **Session detail** — players as they join, linking to profiles and (once
  the game starts) the existing game pages; presence's `game_id` already
  ties a playing account to a game, and `PresenceChanged` can grow an
  optional `session_id` for the lobby state.

Presentation follows the web app's established Live-side style.

## 4. Phased plan

**Phase 0 — confirm the retail contracts** (§1.5's table; parallel, no
server work). The HLE result records can't be finalized before this.

**Phase 1 — custom games listed and joinable by invite** (the smallest
useful slice). Server: `session-*` streams, `POST /v1/sessions` and
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

- §1.5's confirmations, chiefly the session attribute list and the
  accepted-invite record layout — both gate phase 1's HLE records.
- The hopper record-template contents (§2) — the last format work before
  the server can author its own playlist file.
- Whether the 30 s presence ping is the right session heartbeat, or
  sessions need their own ping (the games slice uses ingest traffic;
  lobbies have none until the game starts).
- Carrying the base port in the secure address (§3.5) — needed before two
  consoles behind one NAT can both play with a third.
