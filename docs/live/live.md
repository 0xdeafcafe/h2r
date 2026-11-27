# Xbox Live replacement

Status: built, with the sections below marking what is not. The server
(`crates/live/live-proto`, `crates/live/live-server`, `h2 live ...` in
[4.2](live-pieces.md#42-cli) and [4.3](live-pieces.md#43-service)) has accounts with recovery codes,
title files, friends, teams, sessions, invites, stats, cloud saves, custom
maps, co-op sessions and a relay, and the client HLE (`crates/live/live-hle`)
starts at [Client HLE: sign-in](live-client-hle.md#client-hle-sign-in). It is on in the app
builds and opt-in for `h2 play` (`--live`). Sections 1 to 3 and 6 are the
original design and investigation; the HLE sections say what the game does
now. Not built: the in-game Create/Recover account menu
([In-game account setup](live-client-hle-offline.md#in-game-account-setup)) and federation.

Players sign in with a gamertag, see the message of the day, join
friends, and play Custom Games, campaign co-op, OptiMatch and
Clankermatch. Accounts are created in the CLI or web app. The native
service uses SQLite or Postgres, and the Cloudflare deployment shares
its router with a Durable Object event log and R2 blobs.
[Worker architecture](live-worker.md)

- **Matchmaking and Ranked.** Served hoppers supply the playlist menu
  and rules. The web Ranked page shows standings, levels, XP and bot
  marks. Game results use the server's arbitration rules.
  [Matchmaking](live-matchmaking-ranks.md#8-ranks-what-the-game-does-and-what-the-server-does)
- **Mods.** Browse releases by pack, expand their maps and install them
  together. Choosing a map in a pack selects its installed map slot. The
  lobby advertises the member's exact package ID so joining consoles
  can fetch its missing layers.
  [Maps](../game/maps.md#5-sharing-live)
- **Spectating and replays.** Live ingest negotiates v8 in discovery.
  V7 readers and recordings remain supported. The viewer opens the
  pre-rendered game header, watches live frames or seeks into a verified
  archive for a finished match. [Telemetry](live-telemetry.md) ·
  [Replay fidelity](replay-fidelity.md)
- **Clankers.** Bot looks follow the gamertag, and the Live player options
  card shows a Clanker's emblem, skill and style when available. Sim runs
  can upload to Uplink's Clanker Training. [Bots](../game/bots.md#clanker-appearance-and-live-player-options) ·
  [Training](../runtime/sim.md#watching-in-uplink)
- **Game hosting.** The dedicated-host and light-host work is in progress.
  [Dedicated host](dedicated-host.md)

## 1. What the title links

Halo 2 statically links `XONLINES` 5849, the secure Xbox Live client. It
lives in two code sections plus a read-only data section:

| section   | VA           | size    | analyser starts |
|-----------|--------------|--------:|----------------:|
| `XONLINE` | `0x003a0620` | 182,708 | 913 |
| `XNET`    | `0x003ccfe0` |  78,880 | 381 |
| `XON_RD`  | `0x0056ade0` |   8,616 | data |

Signatures name only 13 `XOnline` and 10 `XNet` functions
([xdk-libraries.md](../lifter/xdk-libraries.md)). Everything else has to be named by
hand.

**Boundary size.** A scan for direct `call rel32` from game code (`.text`,
`XPP`, `D3D`, `DSOUND`, `BINK`) into those two sections finds about 131
distinct targets from 185 call sites: ~94 in `XONLINE`, ~37 in `XNET`. That
is a raw byte scan, so a few are probably false positives, but it puts the
public API the game actually uses at roughly a hundred functions, not the
thousand in the library. `0x003cd718` (17 sites) is the busiest; the
`0x003cd0e1..0x003cd17d` cluster looks like the XNet socket calls (send,
receive, select).

**With Live off.** The kernel reports no cable (`crates/runtime/kernel/src/net.rs`,
`PhyGetLinkState` returns 0), so the title skips `XNetStartup` and every
Live path reports "network cable unplugged". Nothing in `XONLINE` runs.
With Live on, the HLE ([4.1](live-pieces.md#41-client-recomp-side)) sets the link up and
answers the entry points instead.

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
replaced ([ARCHITECTURE.md](../ARCHITECTURE.md)). `XOnlineLogon`,
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

Moved to [Live: the pieces](live-pieces.md#4-pieces).

### 4.1 Client (recomp side)

### 4.2 CLI

#### Playing on Live

#### Game art

#### For agents: `h2 live q`

### 4.3 Service

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
3. **Service and MOTD.** Done: `h2 live serve`, accounts, title storage
   serving files by name, the `message_of_the_day.dat` and
   `multiplayer_hopper.dat` formats ([live-motd.md](live-motd.md),
   [live-admin.md](live-admin.md)) and the client HLE that fetches them.
4. **Friends and presence.** Done (server and HLE; [live-api.md](live-api-social.md#friends)).
5. **System Link over XNet HLE** (two local instances). Done
   ([Client HLE: sockets and System Link](live-client-hle.md#client-hle-sockets-and-system-link)).
6. **Matchmaking and custom games over Live**, then stats and clans. Done
   on tests ([live-matchmaking.md](live-matchmaking.md),
   [live-custom-games.md](live-custom-games.md), [live-telemetry.md](live-telemetry.md),
   [Teams](live-client-hle-offline.md#client-hle-teams)); co-op over Live too
   ([live-coop.md](live-coop.md)). The HLE has been run in the game only
   where those pages say so.

## 6. Open questions

- The `.data` tables at `0x004672b8` and `0x0046dd5c` are answered in
  [live-motd.md](live-motd.md) §1 and §2.
- What `XOnlineLogon` failures the game checks for (title update required,
  service down) and which ones send it to the dashboard (`xonlinedash.xbe`
  is referenced from the image).
- Whether Halo 2 needs `XOnlineTitleUpdate`-style checks to pass before
  matchmaking; title updates themselves are covered in
  [title-updates.md](../runtime/title-updates.md).
- NAT: direct UDP works on a LAN. Across the internet the service will have
  to relay or punch holes; not needed for the first milestones.

## Client HLE: sign-in

Moved to [Live: client HLE, sign-in, sockets and logon](live-client-hle.md#client-hle-sign-in).

## Client HLE: sockets and System Link

Moved to [Live: client HLE, sign-in, sockets and logon](live-client-hle.md#client-hle-sockets-and-system-link).

## Client HLE: service logon and title resources

Moved to [Live: client HLE, sign-in, sockets and logon](live-client-hle.md#client-hle-service-logon-and-title-resources).

### Outages

## Client HLE: menu polling and remaining stops

Moved to [Live: client HLE, sign-in, sockets and logon](live-client-hle.md#client-hle-menu-polling-and-remaining-stops).

## Client HLE: player feedback

Moved to [Live: client HLE, player services](live-client-hle-services.md#client-hle-player-feedback).

## Client HLE: mute list

Moved to [Live: client HLE, player services](live-client-hle-services.md#client-hle-mute-list).

## Client HLE: bans

Moved to [Live: client HLE, player services](live-client-hle-services.md#client-hle-bans).

## Client HLE: title storage and profile fallback

Moved to [Live: client HLE, player services](live-client-hle-services.md#client-hle-title-storage-and-profile-fallback).

## Client HLE: local stats

Moved to [Live: client HLE, player services](live-client-hle-services.md#client-hle-local-stats).

## Client HLE: Live not set up

Moved to [Live: client HLE, offline, sessions and teams](live-client-hle-offline.md#client-hle-live-not-set-up).

### In-game account setup

### In-game sign-in

#### A new account in game

## Client HLE: outgoing messages

Moved to [Live: client HLE, player services](live-client-hle-services.md#client-hle-outgoing-messages).

## Client HLE: game invitations and joining

Moved to [Live: client HLE, player services](live-client-hle-services.md#client-hle-game-invitations-and-joining).

## Client HLE: sessions and server discovery

Moved to [Live: client HLE, offline, sessions and teams](live-client-hle-offline.md#client-hle-sessions-and-server-discovery).

## Client HLE: Teams

Moved to [Live: client HLE, offline, sessions and teams](live-client-hle-offline.md#client-hle-teams).

### Server-backed Teams

