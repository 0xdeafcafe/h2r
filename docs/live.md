# Xbox Live replacement

Status: design, with the first server slice implemented (accounts and title
files: `crates/live-proto`, `crates/live-server`, `h2 live ...` in
[4.2](#42-cli) and [4.3](#43-service)). The opt-in client sign-in HLE is
described in [Client HLE: sign-in](#client-hle-sign-in).

The goal is to make Halo 2 believe it is on Xbox Live: sign in with a
gamertag, show the message of the day and playlists, keep a friends list
with presence, and later matchmake and play. Accounts are made from the CLI
for now. The service behind it is ours, event sourced on Postgres.

## 1. What the title links

Halo 2 statically links `XONLINES` 5849, the secure Xbox Live client. It
lives in two code sections plus a read-only data section:

| section   | VA           | size    | analyzer starts |
|-----------|--------------|--------:|----------------:|
| `XONLINE` | `0x003a0620` | 182,708 | 913 |
| `XNET`    | `0x003ccfe0` |  78,880 | 381 |
| `XON_RD`  | `0x0056ade0` |   8,616 | data |

Signatures name only 13 `XOnline` and 10 `XNet` functions
([xdk-libraries.md](xdk-libraries.md)). Everything else has to be named by
hand.

**Boundary size.** A scan for direct `call rel32` from game code (`.text`,
`XPP`, `D3D`, `DSOUND`, `BINK`) into those two sections finds about 131
distinct targets from 185 call sites: ~94 in `XONLINE`, ~37 in `XNET`. That
is a raw byte scan, so a few are probably false positives, but it puts the
public API the game actually uses at roughly a hundred functions, not the
thousand in the library. `0x003cd718` (17 sites) is the busiest; the
`0x003cd0e1..0x003cd17d` cluster looks like the XNet socket calls (send,
receive, select).

**Today.** The kernel reports no cable (`crates/kernel/src/net.rs`,
`PhyGetLinkState` returns 0), so the title skips `XNetStartup` and every
Live path reports "network cable unplugged". Nothing in `XONLINE` runs.

## 2. What the real service looked like

Strings in `XON_RD` give the protocol map. The client gets Kerberos tickets
from `AS<n>.XBOXLIVE.COM`, `TGS<n>.XBOXLIVE.COM` and `MACS.XBOX.COM`
(machine account), opens an XSP tunnel to a security gateway, and then talks
HTTP (`POST %s HTTP/1.0`, `Content-type: xon/%x`) to services behind it:

| path prefix                    | service |
|--------------------------------|---------|
| `/xpnfront/xpresence.srf`      | presence, friends, game invites |
| `/xpnfront/xnotification.srf`, `xmessaging.srf` | notifications, messages |
| `/xpnfront/xteams.srf`         | teams (Halo 2 clans) |
| `/xmatch/xmatchhost.srf`, `xmatchclient.srf`, `xmatchhostdelete.srf` | session advertise and search |
| `/xqry/*.ashx`                 | generic query service (Search, Add, Update, Remove) |
| `/XArb/*.ashx`                 | arbitration (ranked results) |
| `/xstats/*.ashx`               | stats and leaderboards |
| `/stfd/readfile.ashx`, `writefile.ashx` | title and user storage |
| `/msgserver/*.ashx`, `/fbserver/fbserver.ashx` | string vetting, feedback |
| `/xsig/*.ashx`                 | content signing |
| `/xbos/ContentAvailable.ashx`  | downloadable content |

Bungie's own upload path, `/upload_server/stats.ashx` (multipart, boundary
`BUNGIEr0x0rz`), is game code, not XDK.

**Message of the day.** The game names two downloadable files,
`message_of_the_day.dat` (`0x00450f7c`) and `multiplayer_hopper.dat`
(`0x00453e60`, the playlists). Each is referenced from a table in `.data`
(`0x004672b8`, `0x0046dd5c`), which is presumably a list of title-storage
files to fetch after sign-in through `/stfd/readfile.ashx`. The format of
both files is unknown and is the first thing to decode.

## 3. Where to cut

Three places are possible. Decided (2026-11-15): C, the API boundary.

**A. The wire (what the Insignia revival does).** Leave `XONLINE` and `XNET`
as generated code, give XNet a real host socket, and write a server that
speaks Xbox Kerberos, XSP and the services above. It is the most faithful,
but it means reimplementing Microsoft's Kerberos variant and the XSP
crypto on the server side, and debugging crypto through generated code. Most
of the effort goes into protocol we then throw away.

**B. XNet only.** Replace XNet's sockets with plain host sockets and leave
`XONLINE` generated. The Kerberos and XSP exchanges still run, so we still
need a server that answers them. Same cost as A, less faithful.

**C. The XOnline API (recommended).** Replace the ~100 entry points the game
calls with Rust at the dispatch table, the same way D3D, DSound and Bink are
replaced ([ARCHITECTURE.md](ARCHITECTURE.md)). `XOnlineLogon`,
`XOnlineTaskContinue`, friends, presence, matchmaking, stats and storage
become calls to our own service with a plain protocol (HTTP + JSON, or
WebSocket for pushes). No Microsoft keys, no Kerberos, no XSP. The XDK's
public `xonline.h` gives argument shapes, and the task model is simple: each
call returns an `XONLINETASK_HANDLE`, the game pumps it with
`XOnlineTaskContinue` until it stops returning `XONLINETASK_S_RUNNING`, then
reads results.

C still needs an XNet replacement for game traffic: the sockets, `XNADDR`,
`XNKID`/`XNKEY` and the secure-address helpers (`XNetXnAddrToInAddr`,
`XNetInAddrToXnAddr`, `XNetRegisterKey`; the game logs
"XNetTsAddrToInAddr() failed in register_ip_and_key()"). Those values are
opaque to the game, so we can pack a host IP and port into them. This part is
shared with System Link, which is worth doing first: it needs no service at
all and proves the socket layer.

## 4. Pieces

### 4.1 Client (recomp side)

- **Link up.** `ConsoleConfig::phy_link` already exists; `--live` (or
  `H2_LIVE=<url>`) sets it so XNet starts.
- **XNet HLE.** Socket calls on host UDP and TCP; addresses packed as above.
- **XOnline HLE.** A `live` module in the runtime holding the task table,
  the signed-in users, and a client for the service. Tasks run on a host
  thread and complete into the handle the game polls, so guest code never
  blocks on the network.
- **Accounts.** The console's accounts come from `XOnlineGetUsers`. The HLE
  answers from a host file the CLI writes, so no HDD config-sector work is
  needed.

### 4.2 CLI

Implemented (crates/cli/src/live_cmd.rs; `h2 live --help` has every flag):

```
h2 live serve [--db URL] [--listen ADDR] [--data DIR] [--id HOST]
              [--private] [--invite-only]
              [--tls-cert PEM --tls-key PEM | --insecure-http]
h2 live account create <gamertag> [--server URL] [--invite CODE]
h2 live account list [--server URL]
h2 live motd set <line>... [--header TEXT] [--server URL] [--data DIR]
h2 live motd set --file PATH [--server URL] [--data DIR]
h2 live invite create [--days N] [--data DIR]
```

Defaults: `--db` is `$H2_LIVE_DB` or `postgres://localhost/h2live`,
`--listen` `127.0.0.1:8074`, `--server` `$H2_LIVE_SERVER` or
`http://127.0.0.1:8074`, `--data` the server directory below.

Host data lives in `~/Library/Application Support/h2recomp/live`, beside
the movies directory and the VFS data root ([storage.md](storage.md)):

- `accounts.json` (mode 0600): one entry per account made here, `{xuid,
  gamertag, id, server, server_id, kid, key}`, where `key` is the account's
  Ed25519 private key. `account create` generates the key locally, signs
  the creation request with it and stores it; the server only ever sees the
  public half. The game-side HLE answers `XOnlineGetUsers` from `xuid` and
  `gamertag`, so no HDD config-sector work is needed.
- `server/` (mode 0700): `h2 live serve`'s identity, `server.key` (0600)
  and `server.json` (`{"id"}`). The server id is set on first start
  (`--id`, default `localhost`) and cannot change, because every author id
  contains it.

Operator commands sign with the server key in `--data`, so they run on the
server's machine: `motd set` publishes `message_of_the_day.dat` (built from
up to 32 lines plus an optional header, in the format of
[live-motd.md](live-motd.md) §3.1, the same text in all nine language
slots; or a prebuilt `--file`), and `invite create` prints an invite code
for an `--invite-only` server. Clients find everything else from the base
URL's `/.well-known/h2live.json`, and refuse to send a token over plain
HTTP to anything but loopback.

Planned, not built: `h2 live friend add <gamertag> <gamertag>` (with
friends), `h2 play --live [url]` (boot with link up, signed in).

### 4.3 Service

Two crates. `crates/live-proto` is what clients and server share, with no
runtime: the wire types of [live-api.md](live-api.md) (XUIDs as 16 hex
digits, title IDs as 8, gamertags with Live's rules: 1..=15 letters, digits
and single spaces, starting with a letter, compared case-insensitively),
the signing code of [live-security.md](live-security.md) (`sign`: Ed25519
keys as JWK with RFC 7638 thumbprints, compact JWS, signed event bodies and
batches, self-issued tokens, invites, the hash chain; `jcs`: RFC 8785
canonical JSON) and the MOTD builder (`motd`). The game-side client and
the CLI sign with exactly this code. `crates/live-server` is the service
(axum, tokio, sqlx with runtime-checked queries; see its README).

The API is [live-api.md](live-api.md) on `127.0.0.1:8074`. Built: health,
discovery, accounts (create, list, one, keys by gamertag and by XUID),
presence, the event log (`GET /v1/events` with `from`/`cursor`, `type`,
`exclude`, `stream`, `limit`; `POST /v1/events` for one signed event), the
`/v1/stream` WebSocket (replay from `?from=`, `caught_up`, then live
events, with `types`/`exclude`), and title files. Games, ingest and map
outlines answer 501 `unimplemented` until they are built. Deviations and
choices the contract leaves open:

- `POST /v1/accounts` and `POST /v1/events` take the compact JWS as the
  raw body (`application/jose`). `POST /v1/events` is not in the contract's
  write table; it is how accounts write `KeyAdded`, `KeyRevoked` and
  presence.
- Title files are published by the operator only: a token signed by the
  server key. They are capped at 2 MiB, not the 64 KiB of security §6,
  because `multiplayer_hopper.dat` inflates to 1.4 MB. The bytes live in a
  `blobs` table keyed by SHA-256; the event carries `{title_id, name,
  sha256, size}`.
- Error codes are the contract's plus `too_large` (413).

**Security** ([live-security.md](live-security.md), all but §5
federation). Every event is a JWS by its author; the server writes its own
(`GamertagReserved`, `AccountCreated`, `KeyAdded` at creation, title
files) with its key. An append checks, all in memory: the bearer token
(EdDSA by a current key of `iss`, `aud` = server id, `exp - iat` ≤ 60 s,
`lxm`, `jti` unseen for two minutes), that the event's author is the
token's issuer, the event signature against the author's current key,
`at` within 5 minutes, the stream permission (an account writes only key
events on `account-<own xuid>` and presence on `presence-<own xuid>`),
the event's data (a `KeyAdded` key must be new, the last key cannot be
revoked, presence states are `online`/`away`/`playing`), and a fresh
nonce. Account creation is the one write without a token: the request is
signed by the key it registers, and `KeyAdded` keeps it as `proof`.
Invites are JWS by the server key, single use. Writes are rate limited
per author and method, account creation server-wide; bodies are capped at
64 KiB (title files 2 MiB, WebSocket messages 16 KiB). Reads are public
unless `--private`, which needs a `read` token on every read and as the
first stream message; discovery and `/healthz` stay public. CORS allows any
origin on GET and HEAD and nothing else. Binding to a non-loopback address
needs `--tls-cert`/`--tls-key` (rustls) or `--insecure-http`. The database
URL is never printed with its credentials.

**The log.** One append-only `events` table: `position bigserial`,
`stream_id`, `version`, `type`, `data jsonb`, `at`, `author`, `kid`, `jws`,
`jws_index` (batches), `hash`, unique on `(stream_id, version)`. The `jws`
is the author's, untouched, and the source of truth: events are decoded
from it, and the other columns are copies for querying. `hash` is
`hex(sha256(prev || jws))` per stream, computed in the append transaction.
Appends take a list of events and expected versions (0: the stream must
not exist) and are atomic across streams: account creation writes the
gamertag stream (expected 0, which is what makes a gamertag taken) and the
account stream in one transaction. They hold `pg_advisory_xact_lock` on a
fixed key for the whole transaction, so `bigserial` positions are handed
out in commit order (without it a tailer could read 12, miss an 11 that
commits later, and never go back for it), and send `pg_notify('events',
position)` before committing. Serialising appends caps write throughput,
which is fine at this scale. Schema creation takes its own advisory lock.
`subscriptions(name, position)` holds checkpoints for durable subscribers
(the store has `load_checkpoint`/`save_checkpoint`; none use them yet).
XUIDs are `0x0009 << 48 | random 48 bits`, the marker real Live account
XUIDs carry, with no guessable sequence; a collision fails the account
stream's version check and is retried.

**Subscriptions.** One `Dispatcher` tails the log by position and feeds
every `Subscriber` in order, woken by LISTEN (a watch channel on the
in-memory store) with a one-second poll as the fallback, so a lost wake
costs latency, not correctness. The read model (accounts, keys with their
valid-from/to positions, title files, presence, recent nonces and used
invites) is one subscriber, replayed from position 0 at startup before the
server listens. After each event the dispatcher publishes the applied
position and broadcasts the event: write handlers wait for their position
(read-your-writes), and stream readers subscribe to the broadcast before
replaying from the store, so nothing falls between the two; a reader that
lags re-reads from the store. Every read reply carries that applied
position.

Postgres rather than Mongo: an event store needs ordered append with a
uniqueness check per stream, which is one table and one constraint in
Postgres.

Everything else is an event too, presence and matchmaking sessions
included (decided 2026-11-15), so the log is the full history of who was
online, where and with whom. To keep that from flooding the log:

- **Changes, not pings.** A presence stream records `SignedIn`,
  `PresenceChanged` (title, state, session), `SignedOut`. Keep-alive pings
  only refresh an in-memory deadline and are never written. (Built:
  presence events and the projection. Not built: the deadline, so nothing
  expires yet.)
- **Expiry is an event.** When a deadline passes, the server appends
  `PresenceExpired` (or `SessionExpired`), so a crashed client still ends
  its stream and a replay gives the same answer.
- **Session streams** per matchmaking session: `SessionCreated`,
  `SessionUpdated` (attributes, open slots), `PlayerJoined`, `PlayerLeft`,
  `SessionDeleted`. `XOnlineMatchSessionUpdate` calls that change nothing
  are dropped.
- **Pushes** to clients go out over a WebSocket from the same append path.

## 5. Order of work

1. **Name the boundary.** Done statically: [live-boundary.md](live-boundary.md)
   lists 130 entry points (62 confirmed, 68 likely), the sign-in sequence and
   how each logon result routes to a dialog or the dashboard. Sign-in also
   needs `XOnlineLogonTaskGetResults` (connected = `0x001510f0`, separate
   from task success `0x001500f0`), `XOnlineGetServiceInfo` for 13 services,
   and an `XNetQosServiceLookup` that completes; the logon task must stay
   open while signed in.
2. **Signed in, offline service.** Link up, XNet startup, `XOnlineStartup`,
   `XOnlineGetUsers`, `XOnlineLogon` succeed for a CLI account with a fixed
   in-process stub. Success is the game showing the gamertag as online.
3. **Service and MOTD.** Server slice done (`h2 live serve`, accounts in
   Postgres, title storage serving files by name); what remains is decoding
   the `message_of_the_day.dat` and `multiplayer_hopper.dat` formats and
   having the client HLE fetch them.
4. **Friends and presence.**
5. **System Link over XNet HLE** (two local instances).
6. **Matchmaking and custom games over Live**, then stats and clans.

## 6. Open questions

- The `.data` tables at `0x004672b8` and `0x0046dd5c`: who reads them and
  which call fetches the file.
- What `XOnlineLogon` failures the game checks for (title update required,
  service down) and which ones send it to the dashboard (`xonlinedash.xbe`
  is referenced from the image).
- Whether Halo 2 needs `XOnlineTitleUpdate`-style checks to pass before
  matchmaking; title updates themselves are covered in
  [title-updates.md](title-updates.md).
- NAT: direct UDP works on a LAN. Across the internet the service will have
  to relay or punch holes; not needed for the first milestones.

## Client HLE: sign-in

`crates/live-hle` replaces the public entries in
[live-boundary.md](live-boundary.md) when the host enables Live. It uses
`Dispatch::register_override`, as XAPI and DSound do. Disabled is the default:
it registers nothing, reads no account file and leaves the console PHY setting
alone. These addresses apply to the retail 5849 image; they are not a title-update
matcher (see [title-updates.md](title-updates.md) §1.1).

The host passes one `boot::LiveConfig` to `boot::run_with_live`, or to
`boot::run_hosted_with_live` for a windowed host. The existing `run` and
`run_hosted` keep Live disabled. The CLI supplies `enabled`, `accounts_path`
(`Option<PathBuf>`) and `server_url` (`Option<String>`). For example:

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
an optional `0x` prefix are accepted. Up to 16 distinct accounts are supported.
The additional `server`, `key` and future fields are ignored; signing keys are
not retained in client state or included in diagnostics. Invalid files fail
configuration with an error that does not include their contents. No path means
an empty account list; the client does not invent an account. The configured
server base URL is stored for the next milestone. This milestone makes no
service requests and does not authenticate accounts against a server.

Transport startup succeeds through XNetStartup, WSAStartup and XOnlineStartup,
with paired reference-counted cleanup. GetUsers writes the fixed 16-record
buffer and an output count. Records are 112 bytes, with the XUID at offset 0,
gamertag at 12 and per-user HRESULT at 108. Logon accepts four controller slots,
checks their identities against the configured accounts and returns a persistent
SDK task handle. TaskContinue and LogonTaskGetResults return connection established
(`0x001510f0`) for that task. GetLogonUsers returns a stable guest pointer to the
four records with per-user HRESULT zero. Closing the connection clears that array
and invalidates its tasks. ChangeLogonUsers commits a replacement array on its
first pump and reports completion on the next; its result getter writes four
successful HRESULTs. ServiceInfo succeeds for every requested service ID, with
an optional endpoint output. NotificationSetState accepts the observed state
payload without publishing presence to a server.

XNetGetTitleXnAddr returns the static-IP bit (`4`), rather than pending. Its
36-byte XNADDR packs the host's default-route LAN IPv4 into `ina` and `inaOnline`
(offsets 0 and 4), in network byte order. `wPortOnline` at offset 8 is port 3074,
also in network byte order. The Ethernet identity is synthetic; the remaining
online identity bytes are zero. Service endpoints use the same IP and port.
The host address comes from an OS route lookup using a UDP socket without sending
a datagram; a host with no IPv4 route falls back to loopback. The port is an
address convention here, not a socket listener. Embedders and tests can pass an
explicit endpoint to `live_hle::Hle::with_address`.

The sign-in screen also starts a QoS service lookup. The HLE supplies an XNQOS
header with one complete, uncontacted result and pending count zero, so polling
completes immediately. The full 24-byte XNQOSINFO entry follows the header because
the game reads entry zero without checking the result count. QosRelease invalidates
the record; a repeated release fails. It does not measure latency or contact the
service.

The immediate follow-up tasks have these local behaviors:

| Entry | Behavior |
|---|---|
| XOnlineFriendsStartup | Returns an empty task. Its first pump completes; later pumps return RUNNING_IDLE so the game's disposer can drain it. |
| XOnlineOfferingIsNewContentAvailable | Returns a task whose result is NO_NEW_CONTENT. |
| XOnlinePresenceInit | Returns an empty task that completes successfully. |
| XOnlinePresenceAdd | Validates the task and packed XUID input array; retains no presence data. |
| XOnlinePresenceSubmit | Accepts the presence task; its pump completes with no data. |
| XOnlineMutelistStartup | Returns an empty task that completes successfully. |
| XOnlineMutelistGet | Writes count zero and returns a completing task; no mute entries are written. |

Work-event handles other than NULL and unobserved QoS lookup flags stop with a
named unverified-contract diagnostic. Guest pointer faults use the runtime's
memory-fault path. Unknown or closed task handles fail rather than being reused.
The shared runtime HLE window supplies guest storage through `Guest::map_zeroed`;
existing mappings and claims are skipped. One claimed 64 KiB region holds the
stable user array, task handles and QoS headers. Tokens advance without reuse,
even after cleanup, so stale handles cannot name later tasks. Exhaustion returns
out-of-memory rather than growing indefinitely.

All 130 named boundary targets are registered when enabled. The 27 entries above
and in the sign-in/transport path are implemented. Each of the other 103 entries,
including sockets, matchmaking, storage, stats and the remaining friend helpers,
stops with `unimplemented Live API <name>` before reading arguments or executing
the linked library. There is no silent fallback to XONLINES. Those stops are the
next milestones' boundaries; this change does not claim that later online screens
or gameplay work.

Synthetic tests call every implemented entry through the real runtime Runner
with guest stack arguments. They cover the packed records, stable pointers,
connection and change-user task lifetimes, requested service results, empty tasks,
QoS completion, invalid inputs, disabled registration, named stops and boot PHY
configuration. They require neither an XBE nor a built game image.
