# Live matchmaking: OptiMatch search and ping-aware assignments

Ping-aware OptiMatch assignments.

## 7. OptiMatch search: procedure 1, decoded

Read statically from the 5849 XBE; nothing was run. The code that applies
it is `crates/live/live-proto/src/optimatch.rs`; the HLE decodes the game's
parameters (`crates/live/live-hle/src/sessions.rs`, `Op::Search`) and the
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
session (likely; the hosting branch is the next thing to read).

So
"find or form" needs no server matcher for OptiMatch: the server
filters and orders, and an empty answer makes the searching console the
host that the next searcher finds.

### 7.4 Server and HLE

- `live_proto::optimatch::Query` is the decoded search: `hopper`,
  `average`, `lowest`/`highest` (the window, absent when widened out),
  `party`, and the optional Live field `has_map_pack`.
  `Query::from_params` reads the twelve retail parameters. Other shapes
  fall back to an unfiltered search.
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

## 9. Ping-aware OptiMatch assignments

The HLE uses the queue for solo OptiMatch searches on hoppers with Map
Pack entries. Set `H2_LIVE_PING_MATCHMAKING=1` to use it on hoppers with
disc maps alone. Parties, Clankermatch and the custom-games browser use
their existing search paths.

The server uses the active hopper's **minimum** player count
and the entrants' current server ranks for that hopper. Team hoppers fill
equal-sized teams by distributing the highest-to-lowest ranks in alternating
team order. The planned teams are in the assignment.

The retail title still
owns the final in-game team placement.

Clients keep the existing XNet QoS responder available and measure each
candidate's actual round trip at its XNet base port plus two. This uses the
[co-op echo](live-coop-live.md#106-ping) with a 450 ms reply window. It leaves the
title's separate lobby-data lookup intact.

Set `H2_LIVE_NAT` to `open`,
`moderate` or `strict` and `H2_LIVE_UPLOAD_KBPS` to the known upload rate.
Omit unknown values. The HLE does not infer NAT type or capacity from a
successful small-packet echo.

A cohort needs one eligible host that every member can reach. The oldest
matchable ticket starts with a five-level skill spread and a 90 ms RTT
limit. Every five seconds of wait add three levels and 20 ms, up to 49
levels and 350 ms.

A strict NAT or known upload below 1,000 kbit/s makes a
candidate ineligible to host. Missing measurements wait for a probe.
Explicit timeouts exclude that route.

When all routes fail or exceed the
maximum window, the clients continue searching until the search ends.

Once Live has formed a cohort and planned its teams, it minimises worst
RTT, then median over remote members, then host XUID. A registered
[dedicated host](dedicated-host-ops.md#routing-assignment-and-arbitration)
wins over a peer only by improving worst RTT. The chosen peer receives
the empty search that lets the title advertise its lobby. The other
clients keep their async searches pending until that advertisement and
its title QoS lobby data are ready. Opted-out clients continue to search
the legacy lobby list without discovering queued assignments.

### Joining a dedicated host

The OptiMatch search stays pending while Live reserves and boots a
dedicated host. The matchmaking screen shows “Starting a dedicated host…”.
The client allows time for Live’s 60-second boot window and fallback,
including for a cohort formed near the end of the search.

Even if an
assignment names the local account as a prospective host, `host_lease`
keeps that console on the join path.

When the queue returns `Queue::Join`, the Live HLE looks up the published
session. The session’s `matchmaking.host_lease` identifies the lease even
for clients whose first queue reply is `Queue::Join`. The assignment must
match the queued hopper, include the client in its roster, and name the
session’s host XUID. If the session detail has no assignment, a client
that saw the lease while waiting can use that assignment when the queue
sets `frames_from_host`.

A waiting assignment can carry XUID zero until
the host publishes. The HLE reads
`GET /v1/hosts/{lease}` with the roster member’s `read` bearer. The lease’s
`assignment_id` must match `Assignment.id` and the endpoint’s `session_id`
must match `Queue::Join.session_id`.

The dedicated session can have its own
ID, leaving the assignment ID available for peer fallback. The matchmaking
screen then shows `Joining dedicated host (<region>)`. Local hosts without
a region show `local`.

- With `Udp`, the HLE uses `HostEndpoint.udp` as the game’s base address
  and keeps the identity bytes from `HostEndpoint.xnaddr`.
- With `Tunnel`, the HLE opens `HostEndpoint.tunnel_url` on the Live API
  using a fresh `sessions.relay` bearer. It checks lobby QoS through the
  tunnel and gives the game the host’s published synthetic XNADDR.

Both routes hand the session to the existing XNKID, QoS and XNet join
path. The HLE keeps `Queue::Join.frames_from_host` with that published
session ID so the uploader can leave frame uploads to the dedicated
host. When a host ends or fails before the join completes, the client
refreshes the queue for the server’s fallback assignment.

A fresh
assignment without a host lease uses the peer path. If that assignment
elects this console with `Queue::Host`, the title receives its normal
empty search and advertises the lobby.

[Dedicated host contracts](dedicated-host-build.md#contracts)

### Queue API

Both the native router and the Worker's LiveHub serve the same routes.
Each write uses a fresh `sessions.write` token.

- `PUT /v1/matchmaking/queue` refreshes the signed account's ticket with
  `hopper`, the XNet base `port`, and optional `has_map_pack`,
  `connectivity` and `samples`.
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
  planned teams, the per-member RTT table and the boolean
  `has_map_pack` for the whole assignment. A peer host has zero RTT
  to itself. The public debug record omits network addresses.

### Map Pack rotations

The playlist catalogue reads map IDs from the owner's Map Pack. The default
rotations include all nine in suitable human and clanker playlists:
Containment (1000), Warlock (1010), Sanctuary (1020), Turf (1030), Backwash
(1040), Elongation (1050), Relic (1060), Terminal (1070) and Gemini (1080).
Every such playlist keeps base-disc entries too.

The same hopper and
playlist ID serve every player. The server offers older
clients a disc-only file. The HLE requests the full rotation, detects
hoppers with add-on entries and supplies a disc-only download to a
console without the pack.

Solo searches on those hoppers enter the QoS
queue. At the peer host's weighted pick, the block contains Map Pack
entries only for a current all-owner assignment. Unverified host
lobbies, including legacy and party searches, use the disc rotation.

A downgrade invalidates the pending assignment and withdraws any
dedicated host still starting it (`matchmaking::Matchmaker::submit` and
`Live::withdraw_pending_match_host`). A configured Map Pack map on an
ad-hoc dedicated host requires the pack on every assigned console and in
the host's data bundle. A registered dedicated candidate on a configured
Map Pack map reports its own capability and can serve an all-owner
cohort.

Mixed cohorts can still match on eligible peers
(`matchmaking::selection`).

`live_proto::hopper::retain_block_maps` compacts full entry records so
their weights, variants and opaque fields stay intact. The native hook
checks each map ID against the active menu’s multiplayer level list when available and
filters the result of the hopper block read before the picker consumes
the count and weights. Capability uses the installed content, as
`h2 import-mappack` lays it out, and covers every local seat on that
console.

The search screen explains the disc rotation if the pack is
missing. The website shows the same limitation beside a hopper with Map
Pack entries when the signed-in account's recent search reported it.
[Live API](live-api-sessions.md#map-pack-matchmaking)

### Ad-hoc dedicated hosts

Set `LIVE_DEDICATED=1` in the native server environment or in the Live
Worker's variables. Set `LIVE_DEDICATED_HOPPERS` to a JSON object keyed
by hopper index. A hopper is opted in when its entry supplies the map
scenario and optionally a variant.

For example, enable hopper 1 with
`{"1":{"map":"lockout","variant":"Team Slayer"}}`. The global switch
and each individual hopper default to off. Choose a map and variant
supported by the playlist and the host's data bundle.

For regional placement, set `LIVE_HOST_REGIONS` to a JSON object
mapping provider region codes to IPv4 QoS base endpoints. For example,
`{"lhr":"192.0.2.10:31000","ams":"192.0.2.20:31000","iad":null}`.
Each endpoint answers the same UDP echo as a peer at base port plus
two.

Use `null` for a region without a configured UDP target. Set
`LIVE_HOST_DEFAULT_REGION` to a key in that list. With both settings
omitted, the provider chooses the location.

The queue includes regional probes in its `probing` response within the
32-target limit. It accepts only samples for the current generation.
For a dedicated Map Pack map, only pack owners receive regional probes
and contribute to host pool warming.

On native Live, a running
provider-managed match in the region advertises its UDP QoS endpoint.
A searcher can also probe the configured UDP endpoint. With local and
Cloudflare providers, the queue advertises up
to nine `region_echoes` on the client's own Live origin.

The client
upgrades `/v1/hosts/regions/{region}/echo` using its account's
`sessions.relay` credential and measures a single echo after the
handshake. The account needs a current dedicated search ticket. On the
Worker the region's small-class pool forwards to an active pack's
echo-only tunnel endpoint when available and otherwise answers the echo
in its own room, without starting a container.

On local Live the server
answers the WebSocket. A successful live UDP sample takes precedence
over configured UDP and WebSocket samples for that member.

Among regions measured by every cohort member, Live takes the lowest
worst-member RTT. A region with a recent free small-class slot wins if
its worst RTT is within 10 ms of the best. Equal scores prefer the
configured default and then region code order. If no region has samples
from every member, Live uses the configured default.

In an unranked hopper, each member whose
chosen-region RTT exceeds 120 ms and whose route to the reserved peer
is faster counts towards a veto. A strict majority of the entire cohort
starts the match on the peer. Missing samples abstain, and a tie keeps
the dedicated host.

Ranked hoppers keep the dedicated placement regardless
of these votes. Regional probes occupy no player seats.

Live reserves a cohort with an eligible peer fallback and requests
the dedicated host as soon as the cohort forms. The forming request
waits for the allocation result while other members can keep polling.
This lets the Worker schedule the first lease sweep even if polling
stops.

The playerless host occupies a machine slot, so an ad-hoc
assignment can take at most 15 solo entrants. Cohorts reachable only
through a registered host use the existing registered-host path.
All ad-hoc members receive `waiting` during boot.

Live binds
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
