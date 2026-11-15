# Xbox Live replacement

Status: investigation and design. Nothing here is implemented.

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

Three places are possible. The recommendation is the API boundary.

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

```
h2 live serve [--db postgres://...]        # run the service
h2 live account create <gamertag>          # register, write the local account
h2 live account list
h2 live friend add <gamertag> <gamertag>   # until the in-game request flow works
h2 live motd set <file>                    # publish message_of_the_day.dat
h2 play --live [http://localhost:...]      # boot with link up, signed in
```

### 4.3 Service

A new crate (`crates/live-server`, axum + sqlx) on Postgres, event sourced:

- One append-only `events` table: `stream_id`, `version`, `type`,
  `payload jsonb`, `at`. Unique on `(stream_id, version)` for optimistic
  concurrency.
- Streams per account (`AccountCreated`, `FriendRequested`,
  `FriendAccepted`, `FriendRemoved`), per title-storage file
  (`FilePublished`), later per clan and per stats board.
- Projections rebuilt from events into ordinary tables for reads (accounts by
  gamertag, friend lists).

Not everything belongs in the event log. Presence and matchmaking sessions
change every few seconds and nobody needs their history, so they live in
memory with a TTL and are pushed to clients over a WebSocket. Events where
history matters (accounts, friends, clans, match results, stats); memory
where it doesn't.

Postgres rather than Mongo: an event store needs ordered append with a
uniqueness check per stream, which is one table and one constraint in
Postgres.

## 5. Order of work

1. **Name the boundary.** Turn the 131 call targets into a named table
   (address, XDK name, arguments) with Ghidra and `xonline.h`, starting from
   sign-in. Checked in as addresses and names only, as with every other
   hook.
2. **Signed in, offline service.** Link up, XNet startup, `XOnlineStartup`,
   `XOnlineGetUsers`, `XOnlineLogon` succeed for a CLI account with a fixed
   in-process stub. Success is the game showing the gamertag as online.
3. **Service and MOTD.** `h2 live serve`, accounts in Postgres, title storage
   serving `message_of_the_day.dat` and `multiplayer_hopper.dat`. Needs the
   formats decoded.
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
