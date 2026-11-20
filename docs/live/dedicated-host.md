# Running Halo 2 as a dedicated host

**Partially feasible.** A Mac can run the native title without a window. The question is whether Halo 2 will keep that machine in charge when every player is remote. Its simulation chooses the host by machine identity, but its normal lobby starts through a local player. The dedicated-host spike provides `h2 serve --system-link` for the first real client join. The join and zero-local-player lifecycle still need a game run.

Live would choose and reserve the machine. The native game would run the match. With that arrangement every player has a network path to the authority, including the player who used to host. Fairness improves by removing that player's zero-network-delay advantage. Latency improves for a given player only if the new route and server perform better.

## What the host has to own

| Retail behavior | Consequence for a dedicated process |
|---|---|
| With distributed protocol 2, the game compares the local peer index with the host peer index at start. The host gets game mode 5, simulation **world** mode 4. A client gets game 4, world 5. The host sends each connected remote machine a view of the running world. | Keep the local **peer** and its channels. The authority selector tests peer identity. It does not require the host to own a biped. |
| The party leader controls the lobby. The profile/players screen and Start Game act through a local controller. Each frame the session reconciles signed-in local users into session players. Each peer supplies map readiness and, for matchmaking, hopper status. The start waits for remote machine views. | A local profile normally becomes a player with a spawn, unit, team and score. Despawning it once or leaving its controller idle still leaves its membership. A machine-only owner needs a way to supply properties, accept a ready roster and start without the controller. |
| The session stores **16 peer records and 16 player slots**. A server consumes a peer even with an empty player list. | The existing layout accommodates at most **15 remote consoles**. Sixteen humans require at least two sharing one remote console and a variant allowing 16. An idle host player leaves at most 15 human slots. |

The host indices, local-user reconciliation, player configurations, countdown and remote-view waits are traced in the repo. A permanent nonspawning spectator role has no demonstrated retail path. A spectator workaround would need tests for respawn, team balance, objectives, score, results and capacity. The theater design proposes viewing a stream outside the session. [Phantom session implementation](../../crates/live/bots/src/phantom/session.rs) · [Local players and remote peers](../game/bots.md) · [Theater spectator options](../game/theater.md)

**Headless today.** The fast sim boots straight into a *local* map with one to four bot controllers. Ordinary `h2 serve` runs the full title without a window, including the guest render front end. `--system-link` enables XNet for the retail lobby. The light host reuses the sim’s world-render skip while running that same lobby. [Sim setup](../../crates/extensions/game/src/sim_glue.rs) · [Direct path and render costs](../runtime/sim.md) · [Headless CLI](../../crates/apps/cli/src/boot_cmd.rs)

**What Clankermatch proves.** Live supplies a hopper and bot roster. An empty search induces the *player console* to form the target session. The party and target can differ. The host can add a phantom as a remote machine and player, borrowing a local configuration and injecting its controls. Its channel is absent, so its special start handling bypasses waits that real clients must satisfy. Zero local **bots** still leaves a human profile. Runs that hit a controller-unplugged prompt demonstrate why a changed slot count cannot establish a working launch. [Clankermatch implementation](../../crates/extensions/game/src/clankermatch.rs) · [Hosting and observed start failures](../game/bots.md)

## What changes for the player

When OptiMatch assigns a dedicated host, you stay in matchmaking while
Live starts it. The status line first says “Starting a dedicated host…”
and then `Joining dedicated host (<region>)`. Your console joins
automatically. If Live switches to a player host before the join, the
usual matchmaking flow resumes.
[Joining a dedicated host](live-matchmaking.md#joining-a-dedicated-host)

A peer host pays **0 ms of network RTT** to its own authority. For scale, Microsoft Research used **85 ms mean RTT** from Halo 3 console-pair probes. That sample includes candidate peers before final host selection. Saudi Arabia’s Q4 2023 fixed-line measurements to regional VALORANT servers showed **14–18 ms** by ISP, versus **76 ms** nationally to a European World of Warcraft service. These are different games, populations and routes. We have no measured h2recomp distribution or promised saving. Friends on the same LAN can already beat a regional provider. [Microsoft Research, Htrae](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/sigcomm09-265.pdf) · [CST, Game Mode Q4 2023, pp. 13–15](https://www.cst.gov.sa/-/media/cst-website-app/data/media/Reports/Data%20and%20Analysis/EN/Game%20Mode/Game_Mode_202304-en)

At 40 ms symmetric RTT, a shot takes about 20 ms to reach the host and confirmation about another 20 ms to return, plus tick scheduling, replication and presentation. Distributed clients predict local movement and reconcile with the host. The co-op pad schedule belongs to synchronous campaign play. Its input delays do not predict distributed multiplayer hit registration. Retail weapon behavior also varies. A client can report a projectile impact in its first tick. At 30 Hz the Battle Rifle’s 400 world units/s gives about **13.3 units** of first-tick travel. Longer shots involve projectile flight. Bungie described original Halo 2 melee as awarding the first blow observed by the host. A lower-ping remote can still win a close contest. We have no evidence of a universal hitbox rewind or a verified per-weapon validation algorithm in this recomp. [Distributed versus synchronous prediction](live-coop.md#154-prediction) · [Halo developer notes on the original BR](https://support.halowaypoint.com/hc/en-us/articles/23707515298324-Halo-The-Master-Chief-Collection-MCC-Update-February-2024) · [Bungie’s original melee account](https://halo.bungie.org/bwu/index.html?item=163)

An operator-controlled host keeps a player from choosing the authoritative clock, changing the match result on that machine or ending everyone’s game by quitting. Its telemetry can corroborate damage and kills. Input automation, information available to modified clients, packet loss and differing routes remain. A donor controls their machine and can falsify the authoritative record. Start donated servers as unranked. Central hosting also trades the former host’s immediate authority and availability of local play for dependence on capacity, routing and operator uptime. Preserve ordinary custom games and abort an interrupted ranked round without awarding a partial result. [Live trust boundary](live-security.md) · [Gameplay telemetry](live-telemetry.md)

**Light host (in progress).** Keep the network lobby and authoritative
simulation running at real-time pace while skipping steady-state world
drawing and using a silent audio clock. Make bot support optional. The
goal is to reduce tick cost and memory enough to host matches on a small
machine. The zero-local trial and the load measurements below are still
needed. [Headless simulation](../runtime/sim.md)

## Where the game could run

The game host needs a native process. The current native game module is
about **276 MB**. The former Workers Paid script limit was about 10 MB.
The current Worker bundle limit is **64 MiB**, with **128 MB** of memory
per isolate. Workers also lack inbound UDP for the game's sockets.
[Worker limits](https://developers.cloudflare.com/workers/platform/limits/) ·
[Worker size change](https://developers.cloudflare.com/changelog/post/2026-09-04-increased-worker-size-limit/)

Choose between **Cloudflare Containers** and a **small VPS** after the
zero-local trial. The Container path needs a Linux x86-64 game build,
a Durable Object to allocate and route to it, and a WebSocket/TCP tunnel
for the game's datagrams through our XNet HLE. The VPS path can expose
the game's UDP ports directly. Compare real route and tick timings before
choosing. [Container architecture](https://developers.cloudflare.com/containers/concepts/architecture/) ·
[WebSocket ingress](https://developers.cloudflare.com/containers/examples/websocket/)

**Asset rights decide whether to build a fleet.** The repo expects each user’s own legally obtained disc. The runtime loads a native module derived from its XBE and needs maps and shared disc data. Microsoft’s fan-content rules do not establish permission here to copy those assets into a hosted registry, backups or regional replicas, or operate them concurrently for other people. A private bucket or a disc upload does not settle those questions. Obtain a rights determination covering the actual copies, operators and service model before a managed pilot. Owner-provisioned hardware keeps the disc on its owner’s machine. Separately review public donated hosting. [Project requirements](../../README.md) · [Boot inputs](../runtime/BOOT.md) · [Microsoft Game Content Usage Rules](https://www.xbox.com/en-US/developers/rules)

The native emitter targets arm64 and x86-64 macOS, arm64 iOS/tvOS and x86-64 Windows MSVC. The Windows desktop target has cross-build evidence. A dedicated desktop run still needs qualification. The native game emitter lists no Linux target. A Linux Live service or a Worker build does not produce a Linux game module. Both cloud candidates need an x86-64 Linux ELF module, validated loading/ABI, headless runtime dependencies and an end-to-end join. [Target triples](../../crates/recompiler/emit/src/compile.rs) · [Windows status](../runtime/windows.md) · [Worker implementation](live-worker.md)

| Host | Path to players and readiness | Illustrative USD per occupied match-hour |
|---|---|---|
| Home Mac or PC | Mac is the first test platform. Expose direct UDP on a reachable address for WAN. Availability depends on power, upload and NAT. | Assuming 30 W incremental load and $0.30/kWh, **$0.009/h** electricity, plus hardware, broadband and idle time. |
| Donated host | Owner installs their own disc data. Admit by build, available slots and measured route. CGNAT needs a relay or a different host. | Same power calculation paid by the donor. Service still pays for coordination, optional relay and support. |
| Fly Machines (direct UDP cost example) | Pick a region. UDP needs dedicated IPv4, binding to `fly-global-services` and matching external/internal ports. Give each match a stable endpoint or route all its clients to one machine. | In iad/ewr, performance-1x/2 GB is $33 per 720 h, **$0.046/h**. Performance-2x/4 GB is **$0.092/h**. Add $2/month per dedicated IPv4, storage and NA/EU egress at $0.02/GB. [Pricing](https://docs.fly.io/about/pricing) · [UDP](https://docs.fly.io/networking/udp-and-tcp) |
| Cloudflare Containers | A Worker can allocate a Linux/amd64 Container through a Durable Object. End users cannot send raw UDP directly. An adapter through a Worker and Durable Object adds a routing leg. A WebSocket can also delay later updates behind a lost packet. Measure that path to the actual container region. A Worker isolate cannot execute this native game. | Standard-2, 1 vCPU/6 GiB/12 GB. After plan allowances, 6 × $0.009 memory + 12 × $0.000252 disk + 0.5–1 active CPU × $0.072 = **$0.093–0.129/h**. Add Workers Paid, Durable Objects, transfer and idle allocation as applicable. [Pricing](https://developers.cloudflare.com/containers/platform/pricing/) · [Ingress and lifecycle](https://developers.cloudflare.com/containers/concepts/architecture/) |

Rates checked November 19, 2026. The figures assume one running instance per occupied match and exclude taxes. Divide running compute cost by occupancy to account for a warm pool. For example, 50% occupancy doubles compute per occupied hour. At an *assumed* aggregate game egress of 1 Mbit/s, 0.45 GB leaves per hour, another $0.009 on Fly in NA/EU. Telemetry ingest is a different stream. Fly describes low double-digit seconds to create a VM and typically under a second to start a prepared Machine. Cloudflare reports cold starts often around 1–3 s. Both exclude title boot, disc reads, lobby readiness and start-to-spawn. In the Mac co-op harness with seeded data it took about 19 s from opening the agent socket to the main menu. A joinable distributed lobby and playable spawn come later. Cache a matching module ahead of allocation. Measure cold and warm allocation-to-joinable and join-to-first-playable p50/p95 separately before choosing a warm pool. [Co-op startup observations, §12.6](live-coop.md) · [Fly lifecycle](https://docs.fly.io/machines/overview) · [Container lifecycle](https://developers.cloudflare.com/containers/concepts/architecture/)

**Sizing is provisional.** The two-bot result supplied for this design, 2.8× unpaced at 30 ticks/s, gives 84 ticks/s or **11.9 ms/tick**. If CPU-bound, that uses about 36% of one comparable serial core when paced. If 10–20% of that work scales linearly with player count, sixteen seats cost 11.9 × (1 + 7f) with f = 0.10–0.20, or **20–29 ms/tick** (about **0.61–0.86 of a comparable core**), before network replication. At about 26% scalable work the 33.3 ms budget is already exhausted. Remote humans replace bot thinking with real replication and acknowledgement work. The sim design budgets roughly **1 GB/process** including guest RAM, module and caches. Sixteen-player CPU and peak RSS are unmeasured. Trial one sustained core and 2 GiB, then two cores and 4 GiB. Extra cores can handle other threads. One slow simulation thread can still miss its deadline. [Sim cost model and report](../runtime/sim.md#10-expected-speed-up) · [Report fields](../../crates/live/sim-bench/src/metrics.rs)

## Cost and budget

Live estimates the cost of each dedicated match from the server-signed
`hosts` stream, stored in LiveHub's Durable Object on Cloudflare and in
the event store on native Live. For Worker-launched containers, Live
starts counting at the lease reservation, before calling the provider.
It keeps counting until the provider confirms the stop, even if the
start response was lost or the game already reported the match ended.
For leases without a reserved provider address, Live starts counting
at the first evidence of runtime and settles from the recorded
`HostEnded`.

| Provider | Small | Large | Estimated egress |
|---|---|---|---|
| Local | $0 | $0 | $0 |
| Cloudflare Containers | 1 vCPU, 3 GiB, 12 GB disk | 2 vCPU, 6 GiB, 12 GB disk | $0.025/GB |
| Fly Machines | Performance-1x, 2 GB, about $0.046/h | Performance-2x, 4 GB, about $0.092/h | Compute only in the estimate |

Cloudflare bills active containers in 10 ms increments. Live rounds up
the cumulative runtime to that increment for CPU, memory and disk. It
assumes full use of the allocated vCPUs, memory and disk, and adds a flat
$0.001 estimate per start. The flat amount is a planning allowance.
Fly uses the class's hourly rate divided by 3,600 and multiplies it by
runtime in seconds. The estimate covers the dedicated hosts in one Live
deployment. Cloudflare plan fees, Workers, Durable Objects, actual
egress and included allowances can all affect the account invoice.
[Cloudflare pricing](https://developers.cloudflare.com/containers/platform/pricing/) ·
[Fly pricing](https://docs.fly.io/about/pricing)

Estimated egress is peak players × runtime in minutes ×
0.0005 GB per player-minute for Cloudflare. The budget check uses the
lease's full player capacity for the reserved runtime. The
optional `LIVE_HOST_PRICES` JSON object can override any of these defaults.
Omitted fields keep their defaults. Rates must be finite and non-negative.

```json
{
  "egress_gb_per_player_minute": 0.0005,
  "cloudflare": {
    "usd_per_vcpu_second": 0.00002,
    "usd_per_gib_second": 0.0000025,
    "usd_per_gb_disk_second": 0.00000007,
    "usd_per_egress_gb": 0.025,
    "usd_per_start": 0.001
  },
  "fly": {
    "small_usd_per_second": 0.000012777777777777778,
    "large_usd_per_second": 0.000025555555555555555
  }
}
```

Set `LIVE_HOST_BUDGET_USD` to the limit in USD per UTC calendar month.
The Worker defaults to $25 and native Live defaults to unlimited.
`LIVE_HOST_BUDGET_USD_MONTH` still works when the new setting is absent.
Set the limit to zero to disable new dedicated host requests.

Before granting a lease, Live checks month-to-date spend, the remaining
commitments on existing leases and the cost of one more full-length
match. It also checks commitments in the next month when a lease crosses
the UTC boundary. If there is insufficient budget, the admin start
returns HTTP 503 with a budget error and matchmaking can use a peer
host. The Live reaper and each room's deadline alarm enforce the maximum
lease duration.

The admin Hosts board and `GET /v1/admin/hosts` show estimated spend
since midnight UTC on the first of the month (`month_to_date_usd`) and
the cap (`budget_usd`, which is `null` for unlimited).

For 10 minutes of total runtime, including any boot and lobby time,
the defaults give these estimates in USD.

| Match | Estimated egress | Cloudflare Containers | Fly Machines |
|---|---:|---:|---:|
| 2 players, Small | 0.010 GB | **$0.01825** | $0.046 ÷ 6 = **$0.00767** |
| 16 players, Large | 0.080 GB | **$0.03650** | $0.092 ÷ 6 = **$0.01533** |

## Container image

Both cloud providers use the same Linux/amd64 image. It runs as a non-root
user in a scratch rootfs with bookworm's ELF runtime libraries and CA
certificates. Ship the image only
through a private registry because it contains a module compiled from the
owner's disc.

On macOS, `deploy/host/build.sh` cross-builds the headless `h2` with
cargo-zigbuild targeting `x86_64-unknown-linux-gnu.2.36` and builds the
module with `h2 build --arch x86_64-linux --tier clang`. Pass
`--coverage FILE` to stage the [slim host profile](dedicated-host-build.md#slim-host).
Clang and lld
use the Debian bookworm amd64 sysroot at `H2_LINUX_SYSROOT`. Docker copies
`h2`, `libh2game.so` and its stamp from `deploy/host/stage/`. The matching
`default.xbe` and maps come from private runtime provisioning. [Build and run instructions](../../deploy/host/README.md) · [Linux module build](../runtime/linux.md)

With `H2_PACK=1` the entrypoint runs `h2 host pack`, which starts empty
and takes matches over a private control port. This is how Cloudflare
runs it. [Packed hosts](#packed-hosts) Otherwise it passes `H2_LEASE`,
`H2_LIVE`, `H2_TOKEN`, `H2_DATA_URL`, `H2_DATA_SHA` and optional
`H2_REGION` to `h2 host run` for a single lease, as in a local Docker
smoke test. The host verifies the bundle hash and caches the data in
`/var/cache/h2`. The image exposes UDP 31000–31020 for three slots of game
ports, TCP 31099 for pack control and TCP 31100–31102 for the slots'
datagram tunnels.

## Packed hosts

One light match costs about 6–10 ms of CPU per 33 ms tick and about
340 MB RSS, much of it the shared module and maps. A container that runs
one match leaves its vCPU 70–80% idle. A pack runs several matches in one
container, each a separate `h2 host run` child with its own guest RAM,
writable data and ports. The module and maps are memory-mapped from the
same files, so the page cache shares them.

**Supervisor.** `h2 host pack` listens on `H2_PACK_CONTROL` (default
`0.0.0.0:31099`). Live adds a match with `POST /v1/pack/matches`, removes
one with `DELETE /v1/pack/matches/<lease>`, reads one with
`GET /v1/pack/matches/<lease>` and reads the whole pack with
`GET /v1/pack`. Each slot *n* uses UDP `31000 + 7n` to `31006 + 7n`,
tunnel `31100 + n` and private agent `14720 + n`. The add request carries
the lease, its token and the signed bundle URL. The supervisor passes them
to its child and never writes them down. Each child is its own process
group. A removal sends SIGTERM to the group, so `h2 host run` reports
`Stopped` to Live, and sends SIGKILL after 15 seconds. The children watch
a pipe from the supervisor and stop if it dies. A pack that has run
nothing for `H2_PACK_LINGER_S` (default 60 s) exits.

**Admission.** The supervisor samples each child's process group every
2 seconds from `/proc`: CPU time per 33 ms of wall time and proportional
resident memory (`Pss`). After three samples a match's cost is the higher
of its smoothed and latest value. Until then it counts at 10 ms per tick
and 340 MiB. A new match is refused when the pack already runs
`H2_PACK_MAX_MATCHES` (default 3), when the summed cost plus 10 ms for the
new match would pass 70% of 33.3 ms per vCPU, or when memory plus 340 MiB
would pass the container limit less 256 MiB. Memory comes from the cgroup's
`memory.current` less inactive file pages. Without a cgroup, it uses the
matches' own figures. `H2_PACK_VCPUS` and `H2_PACK_MEMORY_MIB` set the
size; otherwise the cgroup's `cpu.max` and `memory.max` do. On one vCPU,
two matches measured at 6 ms leave room for a third (22 ms of a 23.3 ms
budget). Two at 9 ms do not. Off Linux nothing is measured, so the
defaults and the match cap decide.

**Routing.** On Cloudflare the hub still calls one room per lease. That
room asks the class's pool room for its location
(`pool-<hint>`, or `pool-any` when the region is not a Cloudflare location
hint) to place the lease. The pool sends the match to the fullest pack
with room and starts a new pack room (`pool-<hint>/pack-<n>`) only when
none has room. A refusal marks that pack full until one of its leases
ends, and the pool tries the next pack, up to three new packs per lease.
The pack room starts its container with only the pack's size in the
environment and waits up to 30 seconds for the control port. The lease
room forwards each tunnel upgrade to its pack room, which connects it to
the match's tunnel port. When Live stops a lease, the pool removes the
match from its pack. When that was the pack's last lease, the pack room
destroys the container instead. Pack names are never reused. Each room
keeps an alarm at its latest lease deadline as a backstop for a lost stop.
`LIVE_HOST_PACK_MATCHES` sets matches per container on the Worker
(default 3, at most 8).

Native Live uses the same pool logic and supervisor when
`LIVE_HOST_PACK_MATCHES` is set. It starts one local `h2 host pack` per
pack with its control port on loopback from `LIVE_HOST_PACK_CONTROL_PORT`
(default 32000) plus the pack's window. Each later window shifts its UDP,
tunnel and agent ports past the previous pack's slots. Its provider
references are `pack:<pack>/<lease>`. Unset, each lease runs its own
`h2 host run` as before.

Live's cost estimate still counts a full container for each lease, and
`LIVE_MAX_HOSTS` still counts leases. Packing three matches per container
lets the same `max_instances` serve three times the leases. The estimate
does not yet reflect that saving.

## Light host

Run `h2 serve --system-link --host-light` with the existing prebuilt image.
Clanker Training uses the same profile, adds bot seats, and defaults to
unpaced. An operator still uses the retail lobby to create and start a
dedicated game. The
machine-only lobby and remote join lifecycle above still need a game run.

| Area | What the profile does and why |
|---|---|
| World, HUD, effects | `sim-frame` skips every steady-state world call to `0x00223240`. This drops the world, HUD, and effect draw paths reached there. It calls `0x0012b2e0` for the frame clock, advances counters at `0x00485aa0` and `0x004ba034`, and clears `0x004e6098`. Guest effect updates stay because the tick reads the counters. The software D3D shim keeps display timing and device ABI, with no GPU, display presentation, or render thread. [Skip](../../crates/rewrites/sim-bridge/src/sim_frame.rs) · [Read-back census](../runtime/sim.md#2-what-can-be-skipped-and-what-cant) |
| UI and movies | Bink takes the existing skip return, including intro, attract and credits. The lobby and loading frames keep the guest menu path and cache housekeeping. The menu layer at `0x00147af1` dispatches virtual callbacks, so a blanket guest-side skip needs a write audit. Animation has a tick-side request at `0x001236f0` and a render-side load at `0x001237e0`. Test animation-driven hit and melee behaviour in a real match. [Movie mode](../../crates/extensions/game/src/movies_glue.rs) · [Animation dependency](../graphics/notes/pipelined-render.md#the-new-dependency-sites) |
| Audio | The guest polls buffer status and position and waits for stream events. `StreamGetStatus` at `0x0037f591` has a pause bit read at `0x002ae4c4`. The same logical 256-frame grid advances cursors and delivers status, events and callbacks with no sample mixing, output queue, sink or output thread. `SetBufferData` and PCM/ADPCM stream packets validate the guest ranges without reading their sample bytes in DirectSound. Machine snapshots are unavailable with this audio profile because their audio section contains samples. [DirectSound state](../../crates/platform/dsound/src/state.rs) · [Output clock](../../crates/platform/dsound/src/output.rs) |
| Map I/O | The world skip avoids render-initiated texture streaming in steady state. The D3D shim takes the descriptor-only texture path before reading or decoding texels, including lobby and loading frames. ISO reads and immutable DVD-directory `.map` reads can use read-only file-backed mappings. Eligible immutable ranges can also back guest pages directly. Writes to those pages are private to the match. Sim reports count mapped and copied read bytes. Keep the source files intact for the lifetime of each process. The retail copy to the writable cache, loading-screen bitmap reads, and guest sound-cache and WMA reads still run. Shared cache calls serve several resource types, and the audio shim only owns the payload after the guest submits it. [DVD reads](../../crates/runtime/vfs/src/disc.rs) · [Directory reads](../../crates/runtime/vfs/src/dir_disc.rs) · [Cache copy](../../crates/extensions/game/src/map_copy.rs) · [Codec](../../crates/platform/dsound/src/xwma.rs) |
| Memory | The guest has 64 MiB of RAM per process. The OS can share clean native code pages when processes open the same dylib or DLL. The loader registers pointers and selectors, and dispatch tables, guest RAM, cache metadata and network state are private. Use separate `--data` directories and the same immutable image. The light profile omits kernel-call trace wrappers and frame-time history, including during a long lobby wait. [Guest RAM](../../crates/runtime/runtime/src/guest.rs) · [Module loading and boot](../../crates/apps/boot/src/lib.rs) |
| Native simulation | Light hosts can opt into the three native animation, collision and object implementations together. Individual subsystem overrides take precedence. [Controls](../runtime/switches.md#waiting-for-a-measurement) |
| Clock and network | The kernel sleeps when logical time leads wall time. It wakes for vblank, packet-related guest work and audio completion deadlines as needed on the way to the next 30 Hz gameplay tick. The optional pack pacer groups intermediate deadlines on a slot-specific 30 Hz grid and wakes for socket traffic. XNet performs one receive per guest poll and uses an OS wait for timed `select`. [Pacer](../../crates/runtime/kernel/src/pacer.rs) · [XNet sockets](../../crates/live/live-hle/src/network.rs) |
| Bots and telemetry | `--host-bots` enables explicit phantom adds. The configured telemetry feature follows the regular boot and encoder path, including packed v8 where the build and ingest negotiate it. Accountless System Link can select a local capture with `H2_TELEMETRY_FILE`. [Feature registration](../../crates/apps/boot/src/registration.rs) · [Telemetry configuration](../../crates/live/live-telemetry/src/config.rs) |
| Completed match archive | `h2 host run --archive dual\|film` or `H2_HOST_ARCHIVE`. For provisioned leases, set `LIVE_HOST_ARCHIVE` on native Live or the Worker. A pack passes the archive mode to each lease and keeps each match’s film in its own directory. The default `dual` sends live telemetry and uploads the sealed film. In `film` mode the host uploads one film at match end, and Live derives the stats and web replay from that upload. [Cutover](live-api.md#switching-a-host-to-film-ingestion) |

For a quiet, low-population game, use **12 ms of game-thread CPU per tick**
as the first sizing target (about **0.36 of one core** at 30 Hz) and
**1 GiB RSS per process** as the provisional memory allowance. These are
planning figures from the two-bot sim and the earlier process allowance.
Replication, map loads and the map itself can move either number. RSS counts
shared pages in each process. Measure the fleet’s incremental RAM separately
before sizing by game count.

Measured 2026-11-20, one waterworks 2v2 sim process (macOS `vmmap`,
`/usr/bin/time -l`): peak RSS about **981 MiB**, of which **791 MiB** is
the resident pages of the whole mmapped disc image (the game copies
`shared` (183 MiB), `single_player_shared` (429 MiB) and the level map
from `D:` at load); footprint (dirty) **143 MiB** — Malloc Large 103 MiB
(guest RAM is 64 MiB of that), Malloc Small 35 MiB — plus shared
`__TEXT` 414 MiB. On a container host the resident image pages are
charged per container. Three changes cut that, all behaviour-identical:

- Bulk reads through the disc mapping drop their pages again after the
  copy out (`msync(MS_KILLPAGES)` on macOS, `madvise(MADV_DONTNEED)` +
  `posix_fadvise(DONTNEED)` on Linux): the data lands in the title's
  cache copy or guest RAM, so the image's pages stop accumulating.
- The nav builder on a light host scans only the booted map at boot
  (was: every map on the disc, reading each one's BSPs); later maps
  build on demand as they load.
- The descriptor-only D3D path creates no vertex or index caches (was:
  128 + 32 MiB of LRU budget).

**Sim stores** ([Sim stores](../runtime/sim.md#61-sim-stores-the-map-without-the-map))
take it further: with `H2_SIM_STORE` naming a directory of built
stores, the host serves each `maps\<map>.map` from the store — only the
bytes a recording saw the guest read — and never streams the retail
map. Two hosts on different maps still share `shared.map`'s store pages
and the dylib's code; two on the same map can also share guest RAM
itself, each over a sparse copy-on-write base image
(`state.load` with `"cow": true`). Measurement plan for the fleet:
`wall.peak_rss_mib` and `wall.footprint_mib` in each run's report;
`vmmap`/`smaps` for shared-versus-private per process; the reports'
`wall.guest_written_pages` over `wall.guest_pages` for how much of the
shared base a match keeps.

### Server maps

`h2 server-map strip ORIGINAL.map OUTPUT.map` writes a sparse copy. Use
`h2 server-map verify ORIGINAL.map OUTPUT.map` to compare SHA-256 over
every retained byte, including all tag headers, tag blocks, BSP metadata,
strings, and the cache header. The verifier checks the original cache
checksum and bounds too. Set `H2_SERVER_MAP_DIR` to the directory of
verified copies. Name disc and official Map Pack copies by the original
filename and packaged mod copies by the original map ID followed by
`.map`. If a copy is missing or its header or length differs, the light
host uses the original. Keep matched files immutable while mounted.

The stripper clears known local bitmap LOD and low-detail payloads and
classifiable PCM and Xbox ADPCM sample spans. It keeps WMA, IMA, unknown
or externally referenced ranges, and other raw resources. The guest can
read codec bytes before DirectSound sees a packet (WMA decoder
`0x0038a0c5`–`0x0038a1cc`). Render also feeds the animation cache used
from the tick. Keep the original cache alongside its verified override.
The result has the same logical length, with allocated size reduced by
the page-aligned holes. The command prints both sizes. Container savings
depend on the captured files and on the layer format preserving holes or
compressing zeros. Use the provisional RSS allowance above until measured.
[Stripper](../../crates/apps/cli/src/server_map.rs) · [Mount](../../crates/runtime/vfs/src/map_override.rs)

### Measure before pruning

Set `H2_HOST_AUDIT` to a report path and query `h2 agent hostaudit` before
terminating a live host. The [sim corpus](../runtime/sim.md#host-audit-corpus)
collects the same guest function coverage, tag reads, and subsystem CPU
totals. Keep normal light-host traffic and fleet sizing runs unaudited.

Candidate removals cover effects and lights, first-person animation and
camera shake, sound objects, cloth and antenna updates, and HUD and menu
state. Require a read/write trace and tick-hash parity for each switch.
Effect dispatch at `0x0017a8a0` can apply damage or spawn objects, and
particles share RNG and counters. Audio status, positions and completion
events are gameplay-visible. Keep those paths and the rest of the guest
tick until their cosmetic work can be isolated. A missing tag read or
entry in one corpus is evidence to investigate. Map sub-tag pruning stays
off until indirect reads and uncovered equipment are accounted for. The
optional clang host module faults with a guest address on an uncovered
entry. Use the full light image for live games.

### Starting one by hand

`h2 live hosts start` is the admin's way to run a custom dedicated host
outside matchmaking. It signs an `admin.write` request — with the server
key in `--data` (the operator, the default) or an admin account's key with
`--as GAMERTAG` — and `POST`s `/v1/admin/hosts`
([API](live-api.md#dedicated-hosts)):

```sh
h2 live hosts start --map lockout --variant 'Slayer' --max-players 8 \
  [--class small|large] [--region lhr] [--transport udp|tunnel] [--no-launch] \
  [--live URL] [--data DIR | --as GAMERTAG]
```

Without `--no-launch` the server reserves the lease (budget,
`LIVE_MAX_HOSTS` and cost rows all apply, exactly as for a match) and its
configured provider starts the container; the command prints the lease
JSON. With `--no-launch` no provider is started: the server records the
lease as `requested` and answers its host token, and the command prints
shell lines a manual container run can `eval` — the only place the token
is ever shown:

```sh
eval "$(h2 live hosts start --map lockout --no-launch --live http://127.0.0.1:8074)"
```

sets `H2_LEASE`, `H2_HOST_LEASE_JSON`, `H2_TOKEN` and `H2_LIVE` (the JSON
single-quoted so `eval` cannot split on it). Export the data-bundle values
the same way (`H2_DATA_URL`, `H2_DATA_SHA` from `h2 host bundle`), then run
the image as [deploy/host/README.md](../../deploy/host/README.md) shows:

```sh
docker run --rm --name h2host-smoke --platform linux/amd64 \
  -p 31000-31006:31000-31006/udp \
  -e H2_LEASE -e H2_HOST_LEASE_JSON -e H2_LIVE -e H2_TOKEN \
  -e H2_DATA_URL -e H2_DATA_SHA \
  -e H2_REGION -e H2_OPT -e H2_BASE_PORT=31000 -e H2_PUBLIC_ADDR \
  h2host:local
```

The manual container owns the same lifecycle as a launched one: it
registers its endpoint with the token (moving the lease straight from
`requested` to `joinable` — no provider boot event exists to wait for),
reports playing and ended, and a lease whose container never registers
times out on the same boot deadline as any `requested` lease. Pre-pull the
image: the boot budget is 60 seconds from `hosts start`.

### `h2 host run`

Start one host for an existing Live lease. The provider supplies the full
`HostLease` JSON object in `H2_HOST_LEASE_JSON`, along with the lease ID,
its bearer token and the signed URL and SHA-256 of the owner's private
game-data bundle. Every required flag can also come from the environment.

| Flag | Environment | Value |
|---|---|---|
| `--lease` | `H2_LEASE` | Live host lease ID |
| `--live` | `H2_LIVE` | Live HTTPS base URL or loopback HTTP URL |
| `--token` | `H2_TOKEN` | Lease-scoped host bearer |
| `--data-url` | `H2_DATA_URL` | Signed bundle URL |
| `--data-sha` | `H2_DATA_SHA` | Bundle SHA-256 |
| `--region` | `H2_REGION` | Optional provider region label |
| `--base-port` | `H2_BASE_PORT` | XNet base port, default 31000 |
| `--tunnel-listen` | `H2_TUNNEL_LISTEN` | Internal tunnel listener, default `0.0.0.0:31100` |
| `--out` | `H2_IMAGE` | Prebuilt game module directory, default `$TMPDIR/h2boot` |
| `--data` | `H2_DATA` | Writable game data directory, default private per-run scratch space |
|  | `H2_AGENT` | Optional TCP endpoint for the child's agent, as `tcp://HOST:PORT` |

On Unix, the supervisor checks that the JSON lease ID matches `--lease`
or `H2_LEASE` before provisioning. It provisions the verified bundle under
`H2_HOST_CACHE` (default `host-data` in the user cache) and starts a child
`h2 serve --system-link --host-light`. The bundle's disc directory must
contain `default.xbe` and the maps. The module must match the XBE and its
build settings, including `H2_OPT`.

The child gets `H2_HOST_LOBBY=1`, its own private Unix agent socket,
and the provider's `H2_HOST_TOKEN`, `H2_HOST_LIVE_URL` and `H2_HOST_LEASE_JSON`.
The supervisor opens the lease's map and variant with `host.lobby.open`
and `live: true`. Open returns the session ID acknowledged by Live. The
supervisor registers that ID and the endpoint once `host.lobby.state` shows a lobby. It requests
start when the lobby is ready and has at least the reserved player count,
or one player for an open lobby. Lease roster authorization belongs to Live.

When the supervisor receives a TCP `H2_AGENT` address, the child also
binds that address to the same agent. Both listeners share the same
commands, state and controller claim. The child reports a bind failure
as a startup error. Cloudflare sets `tcp://127.0.0.1:14720` for
container-local diagnostics. A pack gives each slot its own agent port
from 14720 and tunnel port from 31100. The Local provider drops any inherited `H2_AGENT` from Live so
its leases use their own Unix sockets.

For UDP, the child uses the default route's IPv4 or the IP from
`H2_XNET_ADDR`, with the configured base port. Live receives that address
unless `H2_PUBLIC_ADDR` specifies a public IPv4 or IPv4:port. Keep base
through base+6 available and assign each concurrent host its own range.
For tunnel leases, Live receives the WebSocket URL derived from
`--live` at `/v1/hosts/{lease}/tunnel`, and the child listens at
`--tunnel-listen`.

The supervisor samples lobby state once a second. It reports `playing`
on the first playing sample and reports `ended` on postgame
(`MatchOver`), last departure (`Empty`), an unvisited lobby's timeout
(`Idle`), or the lease's hard duration limit (`MaxDuration`). These
ends close the lobby and return 0. A child exit before a terminal lobby
state reports `Crashed` and returns 1. The child has up to 60 seconds
after spawn to reach the lobby. The provider also enforces its boot
deadline from the original lease request.

The local provider sets `H2_HOST_PARENT_PIPE=1` and holds the write end of
the supervisor's stdin pipe for the lease. When the provider exits, EOF
requests the same shutdown as SIGTERM, so the supervisor closes the lobby
and reaps its game child. Cloud providers leave this variable unset.

Bot-only leases with a `sim-<run_id>` assignment use the existing
unpaced simulator in a dedicated child process. The supervisor registers
the lease and starts the child with the verified disc and prebuilt clang
module. The child fetches its match from `GET /v1/hosts/{lease}/sim` and uses
the lease bearer to upload the recording and report through Live's
private sim routes. The supervisor observes gameplay through `match.status`.
A successful child exit reports `MatchOver` to the host API. A failed child
exit reports `Crashed`.

### Compare against ordinary headless serve

Use the image, profile and port setup from the [Mac spike](#mac-system-link-spike).
Use the same prebuilt image and map, matching warm cache state, roster and
telemetry configuration for both trials. Stop both processes between trials
and start the same client again. Run the control with `start_trial control`
and the light trial with `start_trial light --host-light`.

```sh
start_trial() {
  label=$1
  shift
  H2_BOOT_REALTIME=1 H2_XNET_ADDR="${lan}:31000" H2_XNET_LOOPBACK_PEERS="${lan}:31010"     "$H2_BIN" serve "$H2_DISC" --system-link "$@" --out "$trial/host-image" --tier clang     --data "$trial/host" --no-halo1 --no-mappack --socket tcp://127.0.0.1:14720     >"$trial/$label.log" 2>&1 &
  host_pid=$!
}
host_agent() { "$H2_BIN" agent --socket tcp://127.0.0.1:14720 --timeout 100 "$@"; }
```

After joining and starting the same map in each trial, collect five
non-overlapping 30-tick windows after the first 300 ticks.

```sh
for ticks in 300 360 420 480 540; do
  host_agent wait "{\"phase\":\"gameplay\",\"map_ticks\":$ticks}" 90000 >/dev/null &&
    host_agent hostmetrics || break
done | tee "$trial/$label-metrics.txt"
```

Compare `game_cpu_ms_per_tick`, `process_cpu_ms_per_tick`, `cpu_percent`
and `rss_bytes`. The game agent reports the gameplay tick p99 over 30 seconds
and counts intervals over 36 ms. On Linux it also reports proportional,
private dirty and shared clean bytes. The pack reports group memory for each slot. Check `wall_ms_per_tick` against 33.3 ms. The match
check is remote movement, hits, death and respawn, results, and a second
round on the same host. The render consumes shared RNG at
`0x001032a0`, so the rendered control and the light host can diverge in
lockstep. The hash also includes guest texture and sample buffers. Compare
repeat runs from a clean seed and use the same hash to gate each candidate
prune. Cross-profile parity is still open. See the [sim checks](../runtime/sim.md#8-determinism).

## Lifecycle

Live requests one host per match. Before asking a provider to start it, Live
checks the monthly budget and counts hosts that are starting, serving a
match, or awaiting provider cleanup. `LIVE_MAX_HOSTS` defaults to 4. A refused request leaves matchmaking
free to use a peer host.

| Event on the `hosts` stream | What Live records |
|---|---|
| `HostRequested` | The lease, roster, provider, and time limits. The lease now occupies a slot. |
| `HostBooting` | The provider reference used to check or stop this instance. |
| `HostReady` | The endpoint registered with the lease's own credential and the time from request to registration. Clients can now join. |
| `HostPlaying` | The current player count. Live keeps the highest count for the match. |
| `HostEnded` | The reason, elapsed time and peak players. A boot timeout or crash leaves the lease in `Failed`. |

On startup Live replays these server-signed events before it serves requests.
The host token and signed game-data URL are passed directly to the provider.
The boot deadline is 60 seconds from the request. After registration, the
idle timeout defaults to 120 seconds and the hard limit to 1,800 seconds from
the request. An occupied, playing host can run through to the hard limit.
When a supervisor reports the match ended, it has five seconds to exit.
Live then checks the provider and stops a lingering instance before releasing
its slot. A stored cursor lets cleanup resume after a restart.

Set `LIVE_HOST_DATA_URL` to the signed URL for the owner's bundle and
`LIVE_HOST_DATA_SHA256` to its 64-character SHA-256 digest. Set
`LIVE_HOST_LIVE_URL` to the Live base URL reachable from the host when it
differs from the server's public URL. This includes local servers listening
on a port other than the default 8074 without a public URL.

Locally the server launches `h2 host run` with a writable directory under
`<server-data>/hosts/<lease-id>`. A standalone `h2-live-server` deployment
sets `LIVE_HOST_BIN` to the host-capable `h2` binary. Otherwise the provider
uses the current executable. The supervisor receives the full lease in
`H2_HOST_LEASE_JSON` and checks that its ID matches `--lease` before
provisioning. Concurrent local hosts use separate seven-port UDP ranges,
starting at `H2_BASE_PORT` (default 31000). Each tunnel host also gets a
separate TCP listener. On Unix, Live starts the supervisor in its own
process group so an operator stop reaches both the supervisor and its game.

## Game data bundle

Build the bundle from your own Halo 2 disc image or an extracted DVD
directory. Keep the archive, manifest and extracted cache private. Keep
the bundle out of Git and container images.

```sh
h2 host bundle --disc "$H2_DISC" --out "$H2_BUNDLE_DIR"
```

| Disc files | Why the host needs them |
|---|---|
| `default.xbe` | Boot reads the original executable and uses its full hash to identify the prebuilt module. |
| `maps/mainmenu.map` | The light host still boots the retail menu and loads the multiplayer lobby. |
| `maps/shared.map`, `maps/single_player_shared.map` | The map loader can resolve references into these shared caches. |
| Multiplayer `maps/*.map` | The host loads these maps for matches. The bundle selects them by their cache headers. |
| `maps/fonts/` | The retail menu reads the font tables and packed glyphs from the DVD. |

The light host skips Bink movies. Campaign levels and dashboard media are
outside the multiplayer boot path. See [boot](../../crates/apps/boot/src/lib.rs)
and [disc fonts](../formats/fonts.md).

To use server copies, make them with `h2 server-map strip ORIGINAL.map
OUTPUT.map`, keeping the original filenames, and add `--stripped-maps DIR`.
A same-named copy must match the original map header and logical length.
Maps without a replacement come from the disc.

The command writes `host-data.tar.zst` and `host-data.json`. The JSON gives
the archive's `sha256` and the list of payload paths, sizes in bytes and
SHA-256 digests. Keep the two files together.

### Upload to private R2

Create an R2 bucket with public access disabled. Create an R2 API token
with Object Read & Write permission scoped to the bucket, then configure
your AWS CLI profile with its Access Key ID and Secret Access Key. Set
`R2_ACCOUNT_ID`, `R2_BUCKET`, `AWS_PROFILE` and `H2_BUNDLE_DIR` before
running these commands with the AWS CLI and `jq`.
[Cloudflare R2 S3 setup](https://developers.cloudflare.com/r2/get-started/s3/)

```sh
R2_ENDPOINT="https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
H2_DATA_SHA=$(jq -er '.sha256' "$H2_BUNDLE_DIR/host-data.json")
R2_BUNDLE="s3://$R2_BUCKET/host-data/$H2_DATA_SHA"

aws s3 cp "$H2_BUNDLE_DIR/host-data.tar.zst" "$R2_BUNDLE/host-data.tar.zst" \
  --endpoint-url "$R2_ENDPOINT" --region auto
aws s3 cp "$H2_BUNDLE_DIR/host-data.json" "$R2_BUNDLE/host-data.json" \
  --endpoint-url "$R2_ENDPOINT" --region auto
```

Create a signed GET URL for the archive and pass it as `HostBoot.data_url`
(`H2_DATA_URL` for a direct host launch). Pass the manifest's archive SHA-256
as `HostBoot.data_sha256` (`H2_DATA_SHA`). Refresh the URL before starting
hosts after it expires and keep it out of logs and shared configuration.
[Cloudflare R2 presigned URLs](https://developers.cloudflare.com/r2/api/s3/presigned-urls/)

```sh
H2_DATA_URL=$(aws s3 presign "$R2_BUNDLE/host-data.tar.zst" \
  --endpoint-url "$R2_ENDPOINT" --region auto --expires-in 3600)
export H2_DATA_URL H2_DATA_SHA
```

At startup the host streams the archive to `<cache>/<sha256>.partial`,
checks the archive digest, verifies the files during extraction and
publishes `<cache>/<sha256>/` atomically. A complete cache entry works
after the signed URL expires. `H2_HOST_CACHE` overrides the cache location.
`H2_HOST_CACHE_MAX_BYTES` sets the cache budget in uncompressed bytes
(default 8 GiB). Provisioning evicts older entries by last use and keeps
the requested entry even if it alone exceeds the budget. When running
different bundle hashes concurrently, size the cache to hold the live
bundles or give each deployment its own cache.

## Routing, assignment and arbitration

**Game traffic.** System Link discovers pregame hosts by UDP broadcast. Live discovers through a session ID, XNADDR/key and QoS description. Both reach the game over XNet’s UDP mapping. Keep base through base+6 reachable (game channels +0/+1/+5/+6, QoS +2). On a home router this generally requires port forwarding. The HLE provides no general NAT traversal or transport encryption. Existing public/friends listings use the observed HTTP source address. A successful XNetConnect only records an address mapping. Cloud egress and public UDP ingress can differ. Verify the advertised endpoint externally and allow an authenticated managed ingress separate from the local bind address. The existing WebSocket relay admits an open **co-op** host and one guest for messages up to 1,200 bytes. It cannot carry a retail multiplayer match today. A multiplayer fallback needs authenticated multi-peer channel mapping, datagram boundaries, expiry, limits and real route measurements. [XNet transport](live.md#client-hle-sockets-and-system-link) · [Join and address restrictions](live-custom-games.md) · [Relay](../../crates/live/live-server/src/relay.rs) · [Worker counterpart](../../crates/live/live-worker/src/relayroom.rs)

**Assignment.** [Ping-aware OptiMatch](live-matchmaking.md#9-ping-aware-optimatch-assignments)
can reserve an opted-in solo cohort for a registered dedicated endpoint.
Each client measures the actual UDP route with the existing QoS echo.
A registered endpoint can use its existing account. For an ad hoc host the
matchmaker binds the lease to the assignment before the provider starts the
child. Live attaches that assignment, including `matchmaking.host_lease`, to
the machine's session. A roster member checks the lease and the registered
session ID before joining.
[QoS implementation](../../crates/live/live-hle/src/qos.rs)

**Ranked admission.** A peer-hosted match uses the session host’s game
record and its latest unused arbitration round registered before the game
started. The existing human registration and bot roster rules apply.

When the published assignment has `host_lease`, Live binds the session to
the lease subject and checks the assignment’s members against the lease
roster. The session has its own ID and retains the matchmaking assignment
ID, leaving that ID available for a peer fallback. Only the lease subject
can write the authoritative game’s frames and event batches. A client receiving `Queue::Join.frames_from_host` skips
its own frame upload for that game.

A dedicated match counts for Ranked when the lease holder signs a
`GameEnded` with reason `finished` in the assigned ranked hopper. Every
recorded player, including anyone who left, must have an XUID on the
lease roster. Reserved players who never joined receive no result. A
seat without an XUID or a player outside the reservation makes the whole
result ineligible. The current guest-memory projection gives ordinary remote
humans no XUID. A real game with any such seat therefore remains unranked
until the runtime can identify that player. Gamertags and roster order
cannot establish account identity. Live spends an eligible assignment once
and ranks the recorded entrants. The service identity has no player slot,
score, or standing.
[Ranking predicates](../../crates/live/live-server/src/ranks.rs) ·
[Arbitration](../../crates/live/live-server/src/arbitration.rs) ·
[Identity projection](../../crates/live/live-telemetry/src/session.rs)



The planned trusted source is the dedicated host. It signs telemetry
with a service identity separate from the players. Live verifies that
the assigned host signed the completed game and ties the record to the
reservation and eligible joined players before it counts for Ranked.
Preserve single-use results, reconcile disconnects against the
reserved roster and keep the service account out of ranks and scores.

## Host tunnel

Set `H2_TUNNEL_LISTEN=0.0.0.0:31100` in the dedicated game process.
The listener starts on its first initialized XNet call and serves one
lease. Keep this port on the private network between the game host and
the upstream proxy. Live authenticates and admits the client first. The
host accepts the upstream-authenticated upgrade at its original
`/v1/hosts/{lease}/tunnel` path and logs the optional
`x-h2-tunnel-peer` XUID.

The client sends an empty binary hello with port 0 and peer 0. The host
replies on port 0 with an assigned nonzero peer ID and the four IPv4
octets for `10.77.0.1`. The host reserves `10.77/16` for this lease.
The low 16 bits of a client's synthetic address hold its peer ID,
beginning at `10.77.0.2`. A reconnect gets a fresh ID and address. All
client frames carry peer 0. Host datagrams and bye replies carry the
recipient's ID. The host sends a WebSocket Ping every five seconds,
answers incoming pings, and drops connections after 15 seconds without
a Pong.

Datagrams go to XNet guest port `1000 + offset` with a source of the
client's synthetic IPv4 and the host's XNet base plus that offset. The
configured XNet IPv4 must be local to the host. The local UDP bridge
lets ordinary socket reads and `select` wake on tunnel traffic. Sends
to synthetic peers go back over their WebSockets. System Link broadcast
fans out to all active tunnel peers and still goes out over native UDP.
The tunnel answers HLE QoS queries on offset +2 using the same advertised
data as the UDP responder.

The host allows 16 concurrent connections, including pending upgrades.
The payload ceiling is 1,264 bytes. Each peer can send and receive up to
240 datagrams per second, with a burst of 64 in each direction. An
outbound queue holds 64 datagrams per peer. Rate limits and full queues
drop datagrams. Bye or WebSocket close makes the address unreachable.
The next receive skips pending packets from that peer and the next XNet
address operation drops any cached mapping.

## Playerless lobby

For the zero-local repeat of the Mac spike, start the host with
`H2_HOST_LOBBY=1` and `serve --system-link --host-light`. Leave the real
client on its usual System Link path. Use the host agent to open a custom
game and read its roster even when nobody is connected.

```sh
host_agent host.lobby.open '{"map":"lockout","variant":"slayer","max_players":2,"live":false}'
host_agent host.lobby.state '{}'
```

After a remote console joins, expect one remote player and two peers
(count peers with `sim session`). Once `ready` is true, use
`host_agent host.lobby.start '{}'`. Check for world mode 4 on the host and
5 on the client. Verify movement, death and respawn from the host agent,
then the absence of a local player in the result. Close through
`host_agent host.lobby.close '{}'` and repeat the open and join. A second
remote on a separate console tests independent inputs. Two split-screen
users on one remote test the distinction between a player slot and a peer.

The implementation keeps the local **peer** as host and party leader.
It blocks local profile attachment for this dedicated run. A signed-in
bot-style profile would be reconciled into a real session player by
`0x63d80 → 0x65770`. Omitting that row from agent output would still
consume a slot and could affect teams, spawns and results. No owner
profile or player record is manufactured.

The first expected profile-specific refusal is the controller countdown
at `0x19a78e`. It reads the pressing controller's local-user index,
which is −1 on the playerless host. The machine fallback requests stage
2 through `0x5a220` after the remote roster, map properties, ownership
and retail start check pass. It calls `0x5a220` as the countdown
finisher does at `0x6e4d7`: the session in ESI, the stage in EDI, no
stack arguments, plain `ret`. The map check is the finisher's: every
peer's status above 2. `host.lobby.state` reports `ready` only then. Set `H2_HOST_LOBBY_MACHINE_START=0` to
observe the original refusal with the same roster. Remote-view
handshakes still have to finish before the match can run.

| Retail boundary | What the coordinator should confirm |
|---|---|
| Tick `0x137fe0`, profile sign-in `0x18fb34` (callee pops 12 bytes) | Main-menu ticks service RPCs before a profile is chosen. Networking byte `[0x527330]` and lobby byte `[0x4d8ba0]` become nonzero. All four slots from `0x54e8e0` (stride `0xc70`, profile flag `0x10`, local-user index at `+4`) remain signed out through reopen and postgame. |
| System Link Create `0x199df9(0,0)`, Live preparation `0x199e2e(1)` and Create `0x59890(2,0,0)`, custom mode `0x199a03(2)` | Creation returns success in AL. Verify that the post-create squad pointer `S = [0x527364]` identifies the same session through close. Check host state 5 (`S+0x741c`), class 1 or 2 (`+0x18`), peer count (`+0x54`), and that the machine index (`+0x72d8`) equals both host and leader (`+0x40/+0x50`). Confirm discovery and a remote join on each transport. |
| Map setter `0x19a6f2` (ESI = map ID, stack = −1), default variant `0x19d220(out,type)`, variant setter `0x19a728(out)` | Check map (`S+0x4d0c`), distributed protocol 2 (`+0x4dac`), and the full `0x130`-byte rules at `+0x4db0`. Check the limit at variant `+0x78` and session `+0x4994`. Verify that bumping parameter revision `+0x4978` publishes the limit to the client and that retail refuses an extra join. |
| Start check `0x19a2ce(0)`, countdown `0x19a78e` (EAX = port 0, stack = 10, 3), machine request `0x5a220` (ESI = session, EDI = 2) | Verify start state 9 with a remote player and map status 3 or 4 for every peer (`S+0x58+peer*0x10c+0x88`). Check the fallback's calling convention and the stage change at `S+0x4980` against retail's countdown finisher `0x6e410`. Exercise both flag settings and verify remote controls move a unit. |
| Roster and phase | Verify count/mask at `S+0x1118/+0x111c`, records at `S+0x1120+slot*0x13c`, peer at `+0x0c`, and effective UTF-16 name at `+0xa8`. Cross-check lifecycle `[0x527334]` (1/2 lobby, 3 game, 4 postgame), countdown `S+0x49a8`, and game globals `[0x4e6948]` (mode `+8`, active `+0x1120`, ended `+0x1128`). Confirm world `[0x4cf77c]` reaches distributed-server mode 4 at `+8` and running state 5 at `+0x18` before gameplay is reported. |
| Leave sessions `0x593e0()` and return to menu `0x18f1c0(0)` | The target pointer `[0x52736c]` must hold no separate active matchmaking session. Check that remote clients leave, the network squad retires, the host returns to the menu, and the specific Live listing disappears. Repeat open and join on System Link. Use a fresh lease for each Live match. Exercise close during countdown and during play. |

On a leased host, `XOnlineStartup` starts machine logon before the first
retail advertisement. The HLE checks the token against Live discovery and
publishes as `host:<lease_id>`. Live allocates one dedicated session ID for
the lease. Retail updates and deletion use the host bearer and event batches
signed with the lease key. The machine has zero signed-in users and publishes
no gamertag, friends or presence. Plain `h2 serve --system-link` still runs
with local networking only.

For a real run, compare the ID returned by `host.lobby.open` with the
registered endpoint and the roster member's session detail. Check
`matchmaking.host_lease` against the assigned lease, join from a second
console, and use `host.lobby.close` to verify withdrawal of the same ID.
Only remote players should appear in the roster.

The supervisor uses the agent API for these repeatable operations. The
guest boundaries above come from static traces. The two-console lifecycle
and the first refusal still need a game run. [Agent methods](../runtime/agent-api.md#hostlobby)

## Fly Machines provider

`FlyProvider` ([fly.rs](../../crates/live/live-server/src/hosts/fly.rs)) runs
each lease as one Fly Machine: created when matchmaking forms the lobby,
destroyed with the match. On the native Live server, set
`LIVE_HOST_PROVIDER=fly` to select Fly or leave it unset to use `local`.
The native provider calls the Machines API through its reqwest adapter.
The Worker selects Cloudflare Containers through `HostRoom`. A Worker
deployment that selects Fly would need a platform-fetch implementation of
`FlyHttp`.

Setup, from the environment:

| Variable | Meaning |
|---|---|
| `LIVE_FLY_TOKEN` | Fly API token, sent as the bearer on every call. Required. |
| `LIVE_FLY_APP` | The Fly app the machines live in. Required. |
| `LIVE_FLY_IMAGE` | The host image each machine runs (the `linux/amd64` image from `deploy/host`). Required. |
| `LIVE_FLY_REGION` | Where a lease with no region runs. Default `iad`. |
| `LIVE_FLY_PUBLIC_IP` | The app's dedicated IPv4 advertised to clients. Required for UDP leases. |
| `LIVE_FLY_PORTS` | The UDP range lease windows are picked from, as `30000-44000`. Default `30000-44000`. |
| `LIVE_FLY_API` | The API base, so a test can point the provider at a fake. Default `https://api.fly.io`. |

**UDP needs the app's dedicated IPv4.** Allocate one
(`fly ips allocate-v4`, $2/month) and the machine's host process must
bind `fly-global-services`. The provider sets `H2_XNET_ADDR` to that
name and the window's base port, `H2_BASE_PORT` to the base port, and
`H2_PUBLIC_ADDR` to the app's dedicated IPv4. The supervisor resolves
the bind name before passing a numeric address to the game.
Each machine claims a 7-port window
(`base..base+6`, the game's channels and QoS) as one UDP service per
port with external matching internal, chosen deterministically from the
lease id and aligned inside `LIVE_FLY_PORTS`, so a restarted lease comes
back on the same ports and two concurrent leases either take disjoint
windows or the second create is refused by Fly — an error the lifecycle
falls back from. A Tunnel lease claims no ports: its datagrams arrive
through the Live tunnel instead.

The machine: guest `performance` 1×2048 MiB for a Small lease, 2×4096
for Large; `auto_destroy` so a host that ends by itself (match over,
last client gone) removes its machine; `restart: no` so a crashed host
is not restarted — the lifecycle reads the failure and falls back. The
machine env also carries `H2_LEASE`, `H2_HOST_LEASE_JSON`, `H2_LIVE`,
`H2_TOKEN`, `H2_DATA_URL` and `H2_DATA_SHA`. Its metadata names
the lease for `fly machines status`. `start` returns the machine id as
the provider ref — the host registers itself and answers the lease when
it is up; nothing waits. `stop` deletes the machine with force; a
machine already gone is stopped. `alive` reads the machine state
(booting and running are alive, destroyed or missing is not).

Costs follow the [Fly row above](#where-the-game-could-run): the class's
per-second rate while the machine exists, the dedicated IPv4, storage
and egress.

## Order of experiments

1. Run the Mac control below. Advance only after a real remote client joins and its actions reach the authoritative world.
2. Decouple machine ownership/ready/start from local player reconciliation in an isolated host mode. Exercise join, load, combat, results, a second round and last-client departure. If retail requires an owner profile, identify the first failing transition. A retained spectator needs explicit evidence for every slot and gameplay consequence.
3. Reuse the sim’s gameplay render skip with network pacing. Compare normal and skipped runs on the same maps at 1, 4, 8 and 15 remote machines, including a 16-human split-screen case. Record p50/p95/p99 tick work, missed 33.3 ms deadlines, host CPU, RSS, packets/bytes and both startup intervals. Use agent `hostperf`, `capture`, `loads` and host OS resource counters (macOS `/usr/bin/time -l` for whole-process peak RSS and CPU).
4. After the zero-local trial, choose the Container or VPS path. Once
   rights and Linux support are established, pilot unranked games in one
   region. Compare routes and hit outcomes against peer hosts. Introduce
   fleet assignment, then require verified host telemetry for Ranked
   after the remote identity and crash/no-contest checks pass.

## Mac System Link spike

The coordinator can test and build the CLI from this branch with the usual Cargo target directory. Point `H2_BIN` at the resulting release binary, or use the installed copy if it contains this change.

```sh
cargo test --locked -p cli --bin h2 boot_cmd::tests::system_link_enables_accountless_networking_only_on_request -- --exact
cargo build --locked --release -p cli --bin h2
export H2_BIN="${CARGO_TARGET_DIR:-$PWD/target}/release/h2"
```

Use that CLI, the coordinator’s own disc, and a matching prebuilt clang image. Set `H2_DISC` to the ISO, `H2_IMAGE` to its output directory, and `H2_PROFILE_A` and `H2_PROFILE_B` to different game data directories with distinct saved profiles. Choose unused UDP bases and loopback agent ports. Run these blocks in **one macOS shell**. The image check should succeed before either process starts.

```sh
H2_BIN=${H2_BIN:-"$HOME/.cargo/bin/h2"}
export H2_OPT=2 H2_UI_WATCH=1
unset H2_SIM H2_LEVEL H2_BOOT_GFX H2_INPUT_REPLAY H2_BOOT_INPUT
"$H2_BIN" build "${H2_DISC:?set the disc path}" --out "${H2_IMAGE:?set the image directory}" --tier clang --check
```

```sh
trial=$(mktemp -d "$TMPDIR/microhost.XXXXXX")
cp -cR "${H2_PROFILE_A:?set host data directory}" "$trial/host"
cp -cR "${H2_PROFILE_B:?set client data directory}" "$trial/client"
cp -cR "$H2_IMAGE" "$trial/host-image"
iface=$(route -n get default | awk '/interface:/ {print $2}')
lan=$(ipconfig getifaddr "$iface")
```

Use the Mac’s active LAN IPv4 for `lan` if its default route is a VPN. Retail treats 127.0.0.1 specially for game channels. The opposite bases fan out discovery on one machine. The cloned output isolates the host’s kernel trace. Keep this test short. Ordinary serve retains an unbounded in-memory trace and writes a TSV. Bound tracing before measuring fleet RSS.

```sh
H2_BOOT_REALTIME=1 H2_HOST_PERF=1 H2_XNET_ADDR="${lan:?set LAN IPv4}:31000" \
  H2_XNET_LOOPBACK_PEERS="${lan}:31010" "$H2_BIN" serve "$H2_DISC" --system-link \
  --out "$trial/host-image" --tier clang --data "$trial/host" \
  --no-halo1 --no-mappack --socket tcp://127.0.0.1:14720 >"$trial/host.log" 2>&1 &
host_pid=$!
host_agent() { "$H2_BIN" agent --socket tcp://127.0.0.1:14720 --timeout 100 "$@"; }
```

Once `host_agent methods` responds, start the independent windowed client. Its embedded Live instance enables the XNet HLE. Its first launch may build the web app and extract private disc art. Exclude that one-time setup from game-ready timings. Select **System Link** in the title.

```sh
H2_AGENT=tcp://127.0.0.1:14721 H2_XNET_ADDR="${lan}:31010" \
  H2_XNET_LOOPBACK_PEERS="${lan}:31000" H2_LIVE_DIR="$trial/client-live" \
  H2_LIVE_DB=file H2_CLOUD_SAVES=0 "$H2_BIN" play "$H2_DISC" --out "$H2_IMAGE" \
  --tier clang --data "$trial/client" --no-halo1 --no-mappack --no-movies \
  --live-local --gamertag LanClient >"$trial/client.log" 2>&1 &
client_pid=$!
client_agent() { "$H2_BIN" agent --socket tcp://127.0.0.1:14721 --timeout 100 "$@"; }
```

Use `host_agent ui`, `host_agent press start` and `host_agent press a` as the saved profile requires to reach the main menu. Do the same through `client_agent`. From the main menu’s first row, open the players screen (mode 2) and then Available Games (screen 13) on each.

```sh
host_agent sim lobby mode system_link
host_agent sim lobby press a '{"screen":13}'
client_agent sim lobby mode system_link
client_agent sim lobby press a '{"screen":13}'
```

On an empty host browser choose **Create New Game** (`host_agent press a` when selected). Leave the default Slayer lobby open. In the client browser select the named host with the direction pad and A (`client_agent press down`, then `client_agent press a` if Create is highlighted above it). The UI method reports the screen. Browser row order depends on available games. Check `host_agent sim session` and `client_agent sim session` for two peers before pressing **Start Game** on the host. Two one-peer lobbies mean the client also selected Create.

```sh
host_agent press a
host_agent wait '{"phase":"gameplay","map_ticks":30}' 90000
client_agent wait '{"phase":"gameplay","map_ticks":30}' 90000
host_agent sim session
client_agent sim session
host_agent sim view at player
client_agent sim view at player
client_agent stick left 0 1 1000
host_agent wait '{"ticks":60}' 10000
host_agent sim view at player
```

**Control passes** when the host reports distributed `mp_host` (world 4, host true), the client reports `mp_client` (world 5, host false), both count two peers and two players, and the named client’s live unit moves on the host. Move into open space if blocked. An unattended local player is expected on the host. If discovery fails, verify addresses, opposite UDP bases and host lobby state before investigating game start. [Agent CLI](../../crates/apps/cli/src/agent_cmd.rs) · [Menu steps](../../crates/extensions/agent-proto/src/sim/menu.rs) · [World-mode mapping](../../crates/extensions/agent-proto/src/sim/mod.rs)

On a disposable repeat, try the ordinary local player’s leave/sign-out action through the host agent UI after the client joins pregame. Check both session roles, peer and player counts immediately afterward. A refusal, a disbanded lobby, or migration to the client identifies a failing retail transition. If the original host survives without a player, try starting the round.

**Zero-local passes** only if that experiment or the host-mode implementation in step 2 meets all of the following checks. Repeat with the service-owned lobby and identical client. Require **two peers, exactly one session player**, and a world player with a live unit and no `local_user` on the host. On the client the same named player must be local. Verify remote input, a death and respawn, round results free of a host entrant, a second successful start and the last client leaving. Add a second remote for opponent damage, scoring and team objectives. The profile-based control uses the original lobby. The dedicated repeat uses the [playerless lobby methods](#playerless-lobby) to create and start the game without a local owner. This design still awaits a two-console run. [Player reader](../../crates/extensions/agent/src/reader.rs) · [Agent API](../runtime/agent-api.md)

Stop the two processes from this shell when finished.

```sh
kill -TERM "$client_pid" "$host_pid"
wait "$client_pid" "$host_pid"
```
