# Live: client HLE, sign-in, sockets and logon

Part of [Xbox Live replacement](live.md). Covers: Client HLE: sign-in; Client HLE: sockets and System Link; Client HLE: service logon and title resources; Client HLE: menu polling and remaining stops.

## Client HLE: sign-in

`crates/live/live-hle` replaces the public entries in
[live-boundary.md](live-boundary.md) when the host enables Live. It uses
`Dispatch::register_override`, as XAPI and DSound do. Disabled is the default:
it registers nothing, reads no account file and leaves the console PHY setting
alone.

These addresses apply to the retail 5849 image; they are not a title-update
matcher (see [title-updates.md](../runtime/title-updates.md) §1.1).

The host passes one `boot::LiveConfig` to `boot::run_with_live`, or to
`boot::run_hosted_with_live` for a windowed host. The existing `run` and
`run_hosted` keep Live disabled. The CLI supplies `enabled`, `accounts_path`
(`Option<PathBuf>`) and `server_url` (`Option<String>`).

For example:

```rust,ignore
let live = boot::LiveConfig {
    enabled: true,
    accounts_path: Some(accounts_path),
    server_url: Some(server_base_url),
};
boot::run_hosted_with_live(&xbe, &dylib, &opts, host, &live)?;
```

`boot::live_glue::install` is the entry point for an embedding host that already
owns a runtime and kernel. It installs the boundary before guest execution and
sets the existing `ConsoleConfig::phy_link` to active, 100 Mbps, full duplex.
Configuration or registration errors leave the PHY setting unchanged.

The accounts file is a JSON array. Each entry needs a nonzero hexadecimal
`xuid` string and a gamertag of 1–15 printable ASCII bytes. Uppercase hex and
an optional `0x` prefix are accepted.

Up to 16 distinct accounts are supported.
The optional `key` is a private Ed25519 JWK, parsed by
`live_proto::sign::KeyPair::from_private_jwk`. Keys stay in host memory; guest
records and configuration errors never contain them.

The additional `server`
and future fields are ignored: `LiveConfig.server_url` chooses the server for
all calls. Missing or malformed keys allow local/System Link identities, but
server logon rejects them as invalid users. Invalid identity files fail
configuration without echoing their contents.

No path means an empty account
list; the client does not invent an account.

With `server_url = None`, sign-in keeps its local-only behaviour. With a URL,
logon authenticates against that service and publishes signed presence, as
specified in [Client HLE: service logon and title resources](#client-hle-service-logon-and-title-resources).

Transport startup succeeds through XNetStartup, WSAStartup and XOnlineStartup,
with paired reference-counted cleanup. GetUsers writes the fixed 16-record
buffer and an output count. Records are 112 bytes, with the XUID at offset 0,
gamertag at 12 and per-user HRESULT at 108.

Logon accepts four controller slots,
checks their identities against the configured accounts and returns a persistent
SDK task handle. With a server, TaskContinue and LogonTaskGetResults remain
running (`0`) until verification and SignedIn finish. Success returns connection
established (`0x001510f0`).

GetLogonUsers then returns a stable guest pointer to
four records with per-user HRESULT zero. Closing the connection clears that array
and invalidates its tasks. ChangeLogonUsers waits for its server requests, commits
the replacement array, then reports completion on the next pump.

A rejected change
returns `0x80151300` and its result getter supplies four per-user HRESULTs; failed
validation preserves the previous users. ServiceInfo succeeds for every requested
service ID, with an optional endpoint output. These endpoints remain local XNet
placeholders; HTTP service traffic uses the configured URL.

XNetGetTitleXnAddr returns the static-IP bit (`4`), rather than pending. Its
36-byte XNADDR packs the host's default-route LAN IPv4 into `ina` and `inaOnline`
(offsets 0 and 4), in network byte order. `wPortOnline` at offset 8 is the configured host base port
(default 3074), also in network byte order. The Ethernet identity is synthetic; the remaining
online identity bytes are zero.

Service endpoints use the same IP and port.
The host address comes from an OS route lookup using a UDP socket without sending
a datagram; a host with no IPv4 route falls back to loopback. Socket channel
translation uses that base port, as described below.

Embedders and tests can pass
an explicit endpoint to `live_hle::Hle::with_address`, or channel and discovery
settings to `Hle::with_network`.

The sign-in screen also starts a QoS service lookup. The HLE supplies an XNQOS
header with one complete, uncontacted result and pending count zero, so polling
completes immediately. The full 24-byte XNQOSINFO entry follows the header because
the game reads entry zero without checking the result count.

QosRelease invalidates
the record; a repeated release fails. It does not measure latency or contact the
service.

The immediate follow-up tasks have these local behaviours:

| Entry | Behaviour |
|---|---|
| XOnlineFriendsStartup | Returns an empty task. Its first pump completes; later pumps return RUNNING_IDLE so the game's disposer can drain it. |
| XOnlineOfferingIsNewContentAvailable | Returns a task whose result is NO_NEW_CONTENT. |
| XOnlinePresenceInit | Returns a task; with a server it waits for the controller's presence request. |
| XOnlinePresenceAdd | Stores a bounded watch group of packed 12-byte XUIDs. |
| XOnlinePresenceSubmit | Queues the controller's current presence; its pump waits for the server acknowledgement. |
| XOnlineMutelistStartup | Loads every signed-in player's mute list from the server; the task completes when they are in. See "Client HLE: mute list". |
| XOnlineMutelistGet | Writes the controller's mute list and count, at once when it is loaded, else when it loads. |
| XOnlineMutelistAdd, XOnlineMutelistRemove | Change the list at once and send the change to the server behind it. |

Work-event handles other than NULL and unobserved QoS lookup flags stop with a
named unverified-contract diagnostic. Guest pointer faults use the runtime's
memory-fault path. Unknown or closed task handles fail rather than being reused.

The shared runtime HLE window supplies guest storage through `Guest::map_zeroed`;
existing mappings and claims are skipped. One claimed 64 KiB region holds the
stable user array, task handles and QoS headers. Tokens advance without reuse,
even after cleanup, so stale handles cannot name later tasks.

Exhaustion returns
out-of-memory rather than growing indefinitely.

All 130 named boundary targets are registered when enabled: 116 have implemented
results, including deliberate empty views or failed tasks, and 14 remain named
stops. The menu audit and remaining-stop list appear below. A stop reports
`unimplemented Live API <name>` before reading arguments or executing the linked
library.

There is no silent fallback to XONLINES. Tests establish these API and
host-transport contracts; they do not establish a playable Live session.

Synthetic tests call every implemented entry through the real runtime Runner
with guest stack arguments. They cover the packed records, stable pointers,
connection and change-user task lifetimes, requested service results, empty tasks,
QoS completion, invalid inputs, disabled registration, named stops and boot PHY
configuration. They require neither an XBE nor a built game image.

## Client HLE: sockets and System Link

The XNet boundary now uses host IPv4 sockets on macOS and Linux. Socket handles
are increasing guest integers, not host descriptors. `socket` accepts UDP,
TCP and Xbox VDP (protocol 254). VDP uses a UDP socket and carries the title's
payload unchanged.

The host does not implement Xbox IPsec, VDP encryption,
Xbox broadcast authentication or interoperability with retail consoles.
Two HLE clients use the same plaintext transport.

The implemented socket entries are socket, bind, connect, send, sendto, recv,
recvfrom, select, __WSAFDIsSet, ioctlsocket, setsockopt, getsockopt, shutdown,
closesocket, WSAGetLastError, htonl and htons (including their inverse aliases).
TCP is a client transport here; the verified boundary has no listen/accept
entry. Sockets start blocking.

FIONBIO (`0x8004667e`) switches modes, including
back to blocking. FIONREAD (`0x4004667f`) reports available TCP bytes or the next
UDP payload's size. A zero-length datagram remains readable.

A short UDP receive
copies the prefix, consumes the datagram and returns WSAEMSGSIZE (`10040`).
MSG_PEEK leaves the datagram queued. Unsupported message flags and options
return their Winsock errors.

Socket failures, including an unpaired WSACleanup, return SOCKET_ERROR/INVALID_SOCKET (`0xffffffff`) and save a
translated WSA error under `ExecCtx::thread_id`. WSAGetLastError does not consume
it; successful calls leave it alone. Pointer errors return WSAEFAULT (`10014`).

Secure-address APIs return their error code directly. The full stdcall frame is
checked before execution, and malformed frames still use the runtime fault path.
Guest sockaddr_in is 16 bytes: little-endian AF_INET (`2`), network-order port,
four IPv4 octets and eight padding bytes.

Guest fd_set is a 32-bit count followed
by up to 64 32-bit handles; it is unrelated to the host's bitmap representation.
select ignores nfds, filters the three guest sets and leaves timeval unchanged.
A failed nonblocking TCP connect appears in exceptfds; SO_ERROR returns the
translated error. Host waits release guest memory and HLE state locks.

Close and
final transport cleanup wake pending operations and invalidate guest handles.

The title's socket path was checked at `0x00092bf0` (channel creation),
`0x00092ae0` (bind, FIONBIO and broadcast), `0x000b53e0` (protocol selection),
`0x000b4da0`/`0x000b4e70` (options), `0x000b52b0` (select),
`0x000b5060` (recvfrom) and `0x00093730`/`0x000b5110` (sendto).
The four transport channels are:

| Channel | Guest port | Protocol | Default host port |
|---|---:|---|---:|
| Main game data | 1000 | VDP / UDP | 3074 |
| System Link discovery | 1001 | UDP, SO_BROADCAST | 3075 |
| Additional game data | 1005 | VDP / UDP | 3079 |
| Additional game data | 1006 | VDP / UDP | 3080 |

Base+2 (3076) is the HLE QoS responder. Its worker sleeps until a query
arrives or network cleanup signals it to stop, then releases the socket.
Base+4 (3078) is reserved for the co-op
pad-exchange transport (`coop::link::UdpLink`, [live-coop.md](live-coop.md),
section 8), which would bind it on the host itself; nothing binds it today,
since Co-op over Live uses the game's own path, and no guest socket maps to it.

The option wrapper maps the game's options to SO_REUSEADDR (`4`), SO_LINGER
(`0x80`), SO_BROADCAST (`0x20`), SO_SNDBUF (`0x1001`), SO_RCVBUF (`0x1002`) and
Xbox's security option `0x4001`. Linger is a pair of guest 16-bit values,
translated to the host layout. Buffer sizes, broadcast, reuse and TCP_NODELAY affect the host
socket.

The security option is accepted on UDP/TCP and retained for getsockopt;
VDP rejects it, matching the library's protocol check at `0x003ce4d7`. The game
sets it before TCP connects at `0x000b5205`. It does not change the HLE's plaintext
transport.

Socket type, SO_ERROR, keepalive, timeout and exclusive-address/disable-linger
variants also have explicit handling. Unknown
options return WSAENOPROTOOPT (`10042`). These option and QoS layouts follow the
[public Winsock/XNet API declarations](https://github.com/Team-Resurgent/RXDK-SDK/blob/1dc30011768cf75366428f23380e099afa4dce0d/include/winsockx.h),
checked against the retail call sites.

Discovery sends the title's `broadcast-search` and `broadcast-reply` messages
through the UDP channel at guest port 1001, using INADDR_BROADCAST
(`255.255.255.255`) for both. `0x000b2ea0` sends search message 2 about every
1500 ms through the call at `0x000b2f29`; the reply call at `0x00094097` sends
message 3 to the same broadcast address and port. The message serialisers are registered at `0x000ac800`; their payload encoding stays in the
recompiled game. `0x00093730` chooses port 1001 for transport channel 3, then
calls sendto. Receive polling drains the same channel with recvfrom.

The HLE
does not decode or alter these title packets. It replaces XNet's secure-address
bookkeeping and console-specific broadcast wire format with host UDP. In LAN
mode, SO_BROADCAST enables a real limited broadcast to base+1. Hosts should use
the same base port and share a broadcast domain.

XNADDR keeps the 36-byte layout described above: both IPv4 fields contain the
host LAN address, wPortOnline contains the actual host base in network order,
and the synthetic Ethernet identity includes the IP and both port bytes so
local instances differ. The default base is 3074; guest ports 1000..1006 map to
base plus their offset from 1000. Other ports pass through.

XNetXnAddrToInAddr
requires a registered XNKID and returns the peer's IN_ADDR, while retaining
its full XNADDR, key ID and host base for later routing. Peers are told apart
by IP and base port: a peer on another IP is known by its real IPv4; a peer on
this console's own IP (several games on one Mac, as `h2 coop test` runs them)
or on an IP another peer already holds is known by 169.254.(base >> 8).(base &
255) (`NetworkConfig::shared_ip_in_addr`), so each game is a peer of its own.
XNetInAddrToXnAddr returns that record and optional key ID. recvfrom also
learns a sender's base from the channel's source port before secure
registration, and reports the sender by the same IN_ADDR. Incoming channel
ports are translated back to the guest's virtual port.

For a dedicated host with `transport = "tunnel"`, the join code opens
`connect_tunnel(url, bearer)` using the advertised WebSocket URL and a
`sessions.relay` bearer. The host's hello supplies its synthetic IPv4
address and the client's peer ID. The join uses `handle.qos_base()` to
probe the host's lobby on channel +2 and installs the transport with
`hle.install_tunnel(advertised, handle)`. The advertised XNADDR supplies
the host base port.

Datagrams addressed to that host on base through
base+6 travel on the WebSocket with their port offset and payload. On
the return path the worker delivers the payload to the UDP socket bound to
guest port `1000 + offset`. This makes it readable by `select`, `recv`
and `recvfrom`.

The guest sees the host's game address as the sender.
LAN discovery broadcasts and sends to other peers use the existing UDP path.

The socket thread sends a WebSocket Ping every five seconds. A dropped
connection gets one reconnect and a fresh hello. On leave,
`hle.clear_tunnel()` removes the route.

The worker sends a bye and closes
the WebSocket when the route is cleared or the last handle drops. See the
[dedicated host tunnel wire format](live-api-sessions.md#dedicated-host-tunnel).

XNetCreateKey fills an 8-byte system-link ID and a 16-byte key from host OS
randomness; XNetRandom fills the requested buffer. Register/unregister maintain
a bounded key table. Unregistering a key removes its address associations and
QoS listener.

XNetConnect marks a known address connected immediately; status
is IDLE (`0`) before it, CONNECTED (`2`) afterwards and LOST (`3`) for an unknown
address. This is local address bookkeeping, not a handshake or reachability
claim. No encryption or NAT traversal is attempted.

For two instances on one machine, keep `LiveConfig.enabled = true` and use
base ports at least seven apart. Accounts may be absent for System Link.
Boot reads `H2_XNET_ADDR` and `H2_XNET_LOOPBACK_PEERS` only when Live is enabled;
the latter is a comma-separated list of peer base endpoints on this host IPv4.

With that list set, broadcast sends fan out to the listed endpoints, excluding
the sender, rather than emitting a LAN broadcast. Unicast uses learned peer
bases. The host routes these packets locally.

Use the machine's LAN IPv4 in this recipe, illustrated with `192.168.1.50`.
Halo treats `127.0.0.1` as an in-process loopback endpoint at `0x00093218` and
changes channel routing for it; it is suitable for synthetic HLE tests, not
for this title's two-instance setup. In the first instance's environment:

```sh
export H2_XNET_ADDR=192.168.1.50:31000
export H2_XNET_LOOPBACK_PEERS=192.168.1.50:31010
```

In the second instance's environment:

```sh
export H2_XNET_ADDR=192.168.1.50:31010
export H2_XNET_LOOPBACK_PEERS=192.168.1.50:31000
```

Launch each through a host that passes enabled LiveConfig to the boot entry
points above. The environment variables select transport settings; they do not
enable Live by themselves. For headless LAN play, `h2 serve --system-link`
enables the boundary with no account or service URL. See the
[dedicated host experiment](dedicated-host.md#mac-system-link-spike).

Choose System Link, host on one instance and search on the other. No server URL
or account signing key participates in LAN traffic. An embedding host can avoid
environment variables by passing `live_hle::NetworkConfig` to
`Hle::with_network` or `boot::live_glue::install_with_network`.

XNetQosListen retains enabled/disabled state, up to 1020 bytes of listener data
and advertised bandwidth per key. It answers HLE-only UDP queries on base+2
(default 3076). XNetQosLookup copies all guest inputs before issuing bounded
queries, then returns an XNQOS header and 24-byte XNQOSINFO entries in guest
memory.

Pending count is zero. Responding peers have COMPLETE, TARGET_CONTACTED
and, when present, DATA_RECEIVED flags. The actual listener data follows the
array with guest pointers; no host pointer escapes.

RTT is a fixed 5 ms and
bandwidth defaults to 10 Mbit/s. Disabled listeners report TARGET_DISABLED.
An unreachable peer completes as uncontacted with no data. The guest caller
waits while the HLE tries each endpoint in order.

A silent endpoint can take
three 100 ms receive timeouts before the next endpoint is tried
(`crates/live/live-hle/src/qos.rs`). This is a compact HLE side protocol, not an implementation
of the encrypted Xbox QoS protocol. Non-null work events and unknown flags
return WSAEINVAL. Service QoS keeps its sign-in behaviour from above.

Peer QoS allocations use a claimed 1 MiB guest arena. QosRelease returns blocks
to a coalescing free list; cleanup releases every outstanding record. Those
pointers have a C allocation lifetime and must not be used after release.

Task
handles and service QoS keep the earlier monotonic 64 KiB allocation policy.
Tests cover full 64-peer data records and repeated lookup/release without
growing guest mappings.

Real loopback tests drive these APIs through synthetic guest stacks and the
runtime Runner. They exchange datagrams between two guests, discover and reply
through broadcast fan-out, verify TCP connect/data/EOF/error paths, FIONBIO,
FIONREAD, select, per-thread WSA errors, guest memory bounds, keys/address round
trips, QoS listener data, cleanup and disabled boot wiring. These tests need
permission to bind localhost sockets.

No test builds or launches the game.
The remaining stops cover backend operations listed below, including
XNetTsAddrToInAddr and online matchmaking. Actual lobby/gameplay progression has
not been exercised.

## Client HLE: service logon and title resources

The existing boot configuration is sufficient: set `enabled = true`, pass the
CLI's accounts JSON path, and set `server_url` to the service base URL. Account
creation remains the CLI's responsibility. Boot glue installs the same HLE and
returns `Arc<Hle>` to an embedding host; no new boot argument is required.

After `map.reload {file}` binds an editor map, the process refuses external
Live sign-in until restart. The game's cannot-connect dialog says “An editor
map was loaded. Restart the game to sign in to Xbox Live.” The HLE contacts
no service for that logon.

A DVD reservation in progress returns `TASK_BUSY`.
A pending sign-in or an external logon that has signed in an account
refuses a file reload. After a connection loss, the worker can still
announce that account on a heartbeat until the game closes the logon.

A failed initial sign-in allows binding as soon as the worker finishes,
even before the game polls the failure. The embedded service continues
to allow local play copies.
See `crates/live/live-hle/src/state.rs` and
`crates/extensions/game/src/live_notice.rs`.

One `h2-live-http` worker owns a blocking `ureq` client. Guest calls only enqueue
owned host data and poll completions. HTTP, signing, response parsing and waits
never hold guest memory or HLE locks.

The bounded queue fails with TASK_BUSY if
full. Cleanup cancels the session without joining the thread; the worker sends
best-effort SignedOut events, including when its command channel disconnects.
Requests time out after five seconds. `Hle::with_server_options` permits shorter
timings for tests; production heartbeats are 30 seconds.

The worker reads `/.well-known/h2live.json`, then checks every selected account
at `/v1/accounts/by-xuid/{xuid}/keys`. Its active public key must match the
accounts file's private JWK. Only after all identities pass does it append
SignedIn events to `/v1/events`. Every request has a fresh, single-use bearer;
events also carry their own signed envelopes.

Invalid/unknown/revoked identities
return `XONLINE_E_LOGON_INVALID_USER` (`0x80151006`); transport/protocol failures
return CANNOT_ACCESS_SERVICE (`0x80151001`); HTTP 429/503 returns SERVERS_TOO_BUSY
(`0x80151003`). These are task results, so the title's own error classifier sees
them. Failed logon publishes no guest user array.

A heartbeat that keeps
failing changes the persistent connection task to CONNECTION_LOST
(`0x80151004`), after a grace (below).

### Outages

A Worker deploy restarts the hub, and a network blip drops a request; neither
should sign anyone out. The client rides out a short outage at two levels:

- **Retries.** A request that is safe to repeat is retried on a transport
  failure, 5xx or 429: 3 attempts, 0.5 s then 1 s apart, or the server's
  `Retry-After` (capped at 5 s) (`ServerOptions::retry`). Each attempt gets
  a fresh bearer, since a token is spent by its first use. Safe to repeat:
  every GET, mute PUT/DELETE, the heartbeat, and signed events (presence,
  Teams, mailbox, session events). The server takes an event once by its
  nonce, so a resend of one whose answer was lost comes back 409 "nonce
  already used", and after a retry that counts as sent. Session creation,
  reports and arbitration are sent once. When retries run out, the server's
  own last answer (503: SERVERS_TOO_BUSY) is reported over a later failure
  to reach it.
- **Grace.** A failed heartbeat (after its retries) is not a lost
  connection. The connection is lost once heartbeats have failed for
  `ServerOptions::connection_grace` (90 s: the fourth failed beat at 30 s),
  and any answered heartbeat starts it over. A presence change that fails
  for the same reasons counts the same way, and is kept and sent after the
  next good heartbeat (unless a newer one went through first). A refused
  account is not an outage: a ban (ACCOUNT_REQUIRES_MANAGEMENT) or 401/403
  (INVALID_USER) ends the connection at once.
- **Forgotten presence.** If the heartbeat comes back 404 ("not signed in"),
  the server expired the player's presence while it could not be reached.
  The player never left: the client appends `SignedIn` again, then the
  presence it last had, and carries on.

The Worker retries too ([live-worker.md](live-worker.md) "Deploys and
retries"), and the server renews deadlines on restart (Managers, above).
Tests: `crates/live/live-hle/tests/server.rs` drives a proxy (`Flaky`) in front of
the embedded server that fails requests on cue: an outage shorter than the
grace, then a real one; a lost answer to a presence event (one event on the
server); dropped connections and a deploy under logon and storage reads;
retries running out; a forgotten presence; presence set while down.

As required by [live-security.md](live-security.md), remote services require
HTTPS; HTTP is allowed only for loopback. Discovery can select a same-origin API
path, and redirects are disabled. A discovered URL cannot move bearer tokens to
another origin.

The per-account `server` field never overrides LiveConfig.

While connected, the worker sends one heartbeat per console every 30
seconds. It uses `POST /v1/presence/ping/batch` for multiple accounts and
individual `POST /v1/presence/ping` requests for a single account or a
server without the batch route. NotificationSetState and
PresenceInit/Submit queue PresenceChanged events:
a clear ONLINE bit means away, otherwise online. `Hle::set_game_id(Some(id))` is
the telemetry hook for playing with a server game ID; `None` returns to online.

The raw Xbox session ID is not used as a server game ID. Identical states are
coalesced by the worker. `Hle::server_status` exposes acknowledged ping count and
the last HRESULT without any account key data. The telemetry caller is not wired
in this slice.

Logon does not download title files or build a message catalogue. The title's
resource manager requests MOTD and hopper files through title storage after
sign-in; the HTTP worker serves those requests as described below.

MessageEnumerate reports count zero without writing summaries. No title file
is presented as a player message. MessageDetails and MessageDelete return
MESSAGE_INVALID_ID; property/attachment getters have no valid inbox task and
return INVALID_ARGUMENT. Outgoing message capture is separate and unchanged.

GetNotificationEx continues to return FALSE and clear its optional outputs:
the poller at `0x002359ce` would otherwise raise an unopenable "message from
Live service" toast. The voice/text playback screen at `0x002c7410` selects
messages whose sender matches a player; its attachment and language properties
are not the title-resource mechanism.

Synthetic guest tests use the real embedded service to prove signed logon,
identity failures, unavailable/delayed servers, user changes, presence/pings,
connection loss and sign-out. Storage tests publish files and drive the actual
boundary calls, check returned buffers and lengths, validate their headers,
then inflate the MOTD/hopper payloads to exactly `0x25224`/`0x162cc0`.
A file-backed embedded-server test publishes a MOTD through the web-admin
endpoint, reads it through storage and repeats after a restart.

No test runs
the title.

## Client HLE: menu polling and remaining stops

The polling audit starts from the game wrappers in
[live-boundary.md](live-boundary.md), and the per-frame message screen noted in
[live-motd.md](live-motd.md). The owner observed a real signed-in
menu stop at `0x003a0827`; the no-invite path now matches the public XDK contract
and the retail implementation. Other menu cases are statically audited and tested
through synthetic guest stacks; browsing every screen has not been run here.

| Game wrapper / call site | Menu-facing result |
|---|---|
| `0x0006d210` accepted invite poll | GetAcceptedGameInvite consumes a queued accepted invite into the 156-byte output; with none, it clears the output and returns E_FAIL (`0x80004005`). The check at `0x0006d218` treats every nonnegative HRESULT as an invite, so S_FALSE would be wrong. |
| `0x0008cc40`, `0x0008cd27`, `0x0008cddb`, disposer `0x0006b6a5` | FriendsStartup completes then idles, reporting RESULTS_AVAIL (`0x001500f1`) when its cached server snapshot changes. GetLatest returns 86-byte friend/request/invite records. EnumerateFinish lets the enumeration task finish before TaskClose. |
| `0x0006d080` / `0x0006d0d3` | GetNotificationEx emits one packed type-3 notification per received game invite, then BOOL FALSE with cleared info/state. Title files are never notifications. No HRESULT is returned in this BOOL slot. |
| `0x00058ab8`, `0x00058bd1` | MutelistStartup/Get complete; Get writes count 0. |
| `0x0008ce2f`, `0x0008ce97` | FriendsAnswerRequest (answer 0 yes, 1 no, 2 never) and FriendsRemove read the friend's XUID from the first eight bytes of the 86-byte record, queue a signed `FriendAccepted`, `FriendDeclined` or `FriendRemoved` event on the player's account stream and return S_OK. Never is a decline: the server has no block event. The cached friends snapshot is re-read as soon as the event is sent. |
| `0x0008d4d0` | OfferingIsNewContentAvailable reports NO_NEW_CONTENT. |
| `0x0008c320` through `0x0008c59b` | PresenceInit/Add/Submit/Clear retain the bounded watch groups; GetLatest writes 61-byte records carrying the watched XUIDs and overlays cached friend/invite presence when available. |
| `0x0006b59d`, `0x0008d1da`, title checks `0x0008c28e` / `0x0008d27a` | FriendsGetTitleName writes UTF-16 “Halo 2” for this title and an empty terminated name for uncached titles. SameTitle/SamePublisher return BOOL comparisons. |
| `0x0006b565` | GetNatType returns the retail initial/unknown value 0. No NAT probing or claim of open NAT is made. |
| `0x000abc70` through `0x000ac2f2` | Team and member enumerations return snapshots from the local store described below. Unknown teams fail with TEAM_NOT_FOUND (`0x8015210b`); a missing item in a successful snapshot returns E_FAIL, as retail does. |
| `0x0008d3d0` | FeedbackSend sends a player report to the server and completes its task when the server has it; see "Client HLE: player feedback" below. |
| `0x000b388a` | QuerySearch/GetResults returns QUERY_DATASET_NOT_FOUND (`0x80156109`). This optional view has no backend data yet. |
| `0x00092220` through `0x00092450`, manager `0x00074aa0` | StatWrite/GetResult accepts an owned local capture and completes successfully without attachments. StatRead/GetResult completes with missing (`NONE`) stats and zero values in the caller's arrays. See the local stats section below. |
| `0x0022387b` | SaveLogonState writes the public 516-byte launch-state layout with users and service IDs; it contains no signing key. |
| `0x0008e750`, manager `0x002c7410` | Message polling exposes pending game invitations. Title files use storage and never enter the player inbox. |

These 2 entries still stop by API name because their backend behaviour is not
implemented. Their menu/game fallbacks are a separate follow-up slice; the
storage entries below do not stop. Prefixes in the table are part of each
listed API name.

| Prefix | Remaining suffixes |
|---|---|
| `XOnline` | `TitleUpdate` |
| `XNet` | `TsAddrToInAddr` |

Server-backed sessions, invitation delivery, clans and cached friend presence are
implemented in the Client HLE sections below. Comparing ranked arbitration reports
([live-matchmaking.md](live-matchmaking.md) §8.4) and NAT traversal
remain unfinished. Synthetic tests prove invitation discovery through bidirectional
host UDP traffic; actual menu transitions and a full retail match are unconfirmed.
