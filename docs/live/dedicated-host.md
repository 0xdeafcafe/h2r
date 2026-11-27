# Running Halo 2 as a dedicated host

**Partially feasible.** Zero-local-player matches run on Cloudflare hosts: an all-phantom sim lease boots from the private bundle, plays to `match_over` and reports ([measurements](notes/host-halving.md)). A remote human joining a dedicated host still needs a game run, on a Mac with `h2 lab run --dedicated` and on a Cloudflare host through the tunnel. Halo 2's simulation chooses the host by machine identity, but its normal lobby starts through a local player; the host lobby below supplies that start without one.

Live would choose and reserve the machine. The native game would run the match. With that arrangement every player has a network path to the authority, including the player who used to host. Fairness improves by removing that player's zero-network-delay advantage. Latency improves for a given player only if the new route and server perform better.

## What the host has to own

| Retail behaviour | Consequence for a dedicated process |
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
[Joining a dedicated host](live-matchmaking-search.md#joining-a-dedicated-host)

A peer host pays **0 ms of network RTT** to its own authority. For scale, Microsoft Research used **85 ms mean RTT** from Halo 3 console-pair probes. That sample includes candidate peers before final host selection. Saudi Arabia’s Q4 2023 fixed-line measurements to regional VALORANT servers showed **14–18 ms** by ISP, versus **76 ms** nationally to a European World of Warcraft service. These are different games, populations and routes. We have no measured h2recomp distribution or promised saving. Friends on the same LAN can already beat a regional provider. [Microsoft Research, Htrae](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/sigcomm09-265.pdf) · [CST, Game Mode Q4 2023, pp. 13–15](https://www.cst.gov.sa/-/media/cst-website-app/data/media/Reports/Data%20and%20Analysis/EN/Game%20Mode/Game_Mode_202304-en)

At 40 ms symmetric RTT, a shot takes about 20 ms to reach the host and confirmation about another 20 ms to return, plus tick scheduling, replication and presentation. Distributed clients predict local movement and reconcile with the host. The co-op pad schedule belongs to synchronous campaign play. Its input delays do not predict distributed multiplayer hit registration. Retail weapon behaviour also varies. A client can report a projectile impact in its first tick. At 30 Hz the Battle Rifle’s 400 world units/s gives about **13.3 units** of first-tick travel. Longer shots involve projectile flight. Bungie described original Halo 2 melee as awarding the first blow observed by the host. A lower-ping remote can still win a close contest. We have no evidence of a universal hitbox rewind or a verified per-weapon validation algorithm in this recomp. [Distributed versus synchronous prediction](live-coop-live.md#154-prediction) · [Halo developer notes on the original BR](https://support.halowaypoint.com/hc/en-us/articles/23707515298324-Halo-The-Master-Chief-Collection-MCC-Update-February-2024) · [Bungie’s original melee account](https://halo.bungie.org/bwu/index.html?item=163)

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
the cumulative runtime to that increment. For a settled packed lease,
it apportions provisioned memory and disk by the number of occupied
slots over time and uses measured process-group CPU when available.
A pack start carries one $0.001 planning allowance. Active reservations,
unmeasured leases and cleanup intervals use full-class estimates. Admission
reserves a 60-second idle hold for each launched Cloudflare lease. On release,
the pool records which lease emptied the pack. Only that lease pays the hold,
beginning at the pool's release timestamp. A retried stop returns the same
receipt. Older stop events without a receipt retain the full hold estimate.
Idle warm grants can overlap occupied intervals, so the estimate can run high.
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
certificates. The image holds public code only: the headless `h2` and its
entrypoint. The game module compiled from the owner's disc comes with the
private [game data bundle](dedicated-host-ops.md#game-data-bundle) at start, so CI builds and
pushes the image on every push to master
([Deploy from CI](live-worker-deploy.md#deploy-from-ci)).

`deploy/host/build.sh --h2` builds the headless `h2` with cargo-zigbuild
targeting `x86_64-unknown-linux-gnu.2.36`, on macOS or Linux, and stages
it in `deploy/host/stage/h2`; Docker copies it from there. On macOS,
`deploy/host/build.sh --module` builds the module with `h2 build --arch
x86_64-linux --tier clang` into `deploy/host/stage/image/` for the
bundle. Pass `--coverage FILE` to build the
[slim host profile](dedicated-host-build.md#slim-host). Clang and lld use
the Debian bookworm amd64 sysroot at `H2_LINUX_SYSROOT`. The module and
`h2` must come from the same generator revision
([The game module](dedicated-host-build.md#the-game-module)).
[Build and run instructions](../../deploy/host/README.md) · [Linux module build](../runtime/linux.md)

With `H2_PACK=1` the entrypoint runs `h2 host pack`, which starts empty
and takes matches over a private control port. This is how Cloudflare
runs it. [Packed hosts](#packed-hosts) Otherwise it passes `H2_LEASE`,
`H2_LIVE`, `H2_TOKEN`, `H2_DATA_URL`, `H2_DATA_SHA` and optional
`H2_REGION` to `h2 host run` for a single lease, as in a local Docker
smoke test. The host verifies the bundle hash, caches the data in
`/var/cache/h2` and runs the bundle's module from there. The image exposes UDP 31000–31069 for ten slots of game
ports, TCP 31099 for pack control and TCP 31100–31109 for the slots'
datagram tunnels.

## Packed hosts

One light match costs about 6–10 ms of CPU per 33 ms tick and about
340 MB RSS, much of it the shared module and maps. A container that runs
one match leaves its vCPU 70–80% idle. A pack runs several matches in one
container, each a separate `h2 host run` child with its own guest RAM,
writable data and ports. The module and maps are memory-mapped from the
same files, so the page cache shares them.

**Supervisor.** `h2 host pack` listens on `H2_PACK_CONTROL` (default
`127.0.0.1:31099`). Cloudflare packs explicitly bind their private container interface. Live adds a match with `POST /v1/pack/matches`, removes
one with `DELETE /v1/pack/matches/<lease>`, reads one with
`GET /v1/pack/matches/<lease>` and reads the whole pack with
`GET /v1/pack`. Each slot *n* uses UDP `31000 + 7n` to `31006 + 7n`,
tunnel `31100 + n` and private agent `14720 + n`. The add request carries
the lease, its token and the signed bundle URL. The supervisor passes them
to its child and never writes them down. Each child is its own process
group. A removal sends SIGTERM to the group, so `h2 host run` reports
`Stopped` to Live, and sends SIGKILL after 15 seconds. The children watch
a pipe from the supervisor and stop if it dies. A private `POST /v1/pack/warm` renews an empty supervisor’s idle
deadline. Unused supervisors exit after `H2_PACK_LINGER_S` (default 60 s).

**Admission.** The supervisor samples the match process group every two
seconds on Linux. The game agent supplies gameplay tick p99 on both homes.
On other platforms the pack uses the game process’s CPU and resident memory.
The pack estimates cold matches from the median of recent measured loads
on that map. Without history it reserves 10 ms per tick and 340 MiB per match.
Admission keeps 30% of each vCPU and 256 MiB of memory free. It accounts
for boot reservations even before resident memory grows. A rolling
30-second gameplay p99 over 36 ms also closes admission.

A Firefight lease (`HostLease.firefight`) takes a whole pack: the host
runs every actor's AI ([CPU on the host](../game/ai-networking.md#cpu-on-the-host)),
so the supervisor starts it only in an empty pack and refuses every lease
beside it until it ends (`SlotClass::Whole`, `admit_class`; refusals
`whole_needs_empty` and `taken_whole`). On a small pack that is one whole
vCPU. The pool routes it like any lease: a busy pack's refusal marks that
pack full until one of its leases ends, and the lease lands in a new or
empty pack.
[Admission](../../crates/live/live-proto/src/host_pack.rs) · [Supervisor](../../crates/apps/cli/src/host_cmd/pack.rs)

Small packs admit up to five slots on one vCPU within 1,536 MiB. Large
packs admit up to ten slots on two vCPUs within 3,072 MiB. Cloudflare
provisions and bills 3 and 6 GiB respectively under its capped scheduling
policy. The supervisor accepts explicit CPU, memory and slot limits.
[Sizing and qualification](host-halving.md#pack-density-and-admission)

**Routing.** On Cloudflare the hub still calls one room per lease. That
room asks the class's pool room for its location
(`pool-<hint>`, or `pool-any` when the region is not a Cloudflare location
hint) to place the lease. The pool sends the match to the fullest pack
with room. It starts a new pack room (`pool-<hint>/pack-<n>`) when
search demand exceeds free slots or a lease cannot be placed. A refusal marks that pack full until one of its leases
ends, and the pool tries the next pack, up to three new packs per lease.
The pack room passes its resource settings and configured feature switches to the container and waits up to 30 seconds for the control port. The lease
room forwards each tunnel upgrade to its pack room, which connects it to
the match's tunnel port. When Live stops a lease, the pool removes the
match from its pack. The final lease starts a 60-second idle hold. Each regional class allows
three speculative warm grants between new lease admissions, including
the first start and any renewals. Durable alarms expire leases and empty
holds across eviction.
The pool assigns a new name to every new pack.
`LIVE_HOST_PACK_MATCHES` sets matches per container on the Worker
(five for small, ten for large, or a lower configured limit).

Native Live uses the same pool logic and supervisor when
`LIVE_HOST_PACK_MATCHES` is set. It starts one local `h2 host pack` per
pack with its control port on loopback from `LIVE_HOST_PACK_CONTROL_PORT`
(default 32000) plus the pack's window. Each later window shifts its UDP,
tunnel and agent ports past the previous pack's slots. Its provider
references are `pack:<pack>/<lease>`. Set the slot limit to `auto` for
class-sized packs. Each child writes under `window-<n>/<lease>` in the
host data directory. Unset, each lease runs its own `h2 host run`.
[Warm capacity policy](host-halving.md#warm-capacity-from-demand)

The lease cap counts matches across both classes. The Worker deploy
renderer computes a separate container limit for each class from the cap,
its configured slots and the number of regions. See the
[capacity steps](host-halving.md#caps) and [cost model](#cost-and-budget).

## Light host

Moved to [Dedicated host: the light host](dedicated-host-light.md#light-host).

### Server maps

### Measure before pruning

### Starting one by hand

### `h2 host run`

### Compare against ordinary headless serve

## Lifecycle

Live requests one host per match. Before asking a provider to start it, Live
checks the monthly budget and counts hosts that are starting, serving a
match, or awaiting provider cleanup. `LIVE_MAX_HOSTS` defaults to 4 and accepts 0–4, 16, 64, 256 or 1,024. A refused request leaves matchmaking
free to use a peer host.

| Event on the `hosts` stream | What Live records |
|---|---|
| `HostRequested` | The lease, roster, provider, and time limits. The lease now occupies a slot. |
| `HostBooting` | The provider reference used to check or stop this instance. |
| `HostReady` | The endpoint registered with the lease's own credential and the time from request to registration. Clients can now join. |
| `HostPlaying` | The current player count. Live keeps the highest count for the match. |
| `HostEnded` | The reason, elapsed time, peak players and optional packed usage. A boot timeout or crash leaves the lease in `Failed`. |

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

Moved to [Dedicated host: data, routing, tunnel and providers](dedicated-host-ops.md#game-data-bundle).

### Upload to private R2

## Routing, assignment and arbitration

Moved to [Dedicated host: data, routing, tunnel and providers](dedicated-host-ops.md#routing-assignment-and-arbitration).

## Host tunnel

Moved to [Dedicated host: data, routing, tunnel and providers](dedicated-host-ops.md#host-tunnel).

## Playerless lobby

Moved to [Dedicated host: data, routing, tunnel and providers](dedicated-host-ops.md#playerless-lobby).

## Fly Machines provider

Moved to [Dedicated host: data, routing, tunnel and providers](dedicated-host-ops.md#fly-machines-provider).

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

To host a dedicated System Link game on the paired lab Mac for a joiner
on another Mac, use `h2 lab run --dedicated`
([Dedicated System Link host](../runtime/lab.md#dedicated-system-link-host)).
It starts the same light serve with `H2_HOST_LOBBY=1` on the lab Mac's LAN
address, can open the lobby itself, and takes agent calls through
`h2 lab agent`. The steps below run both ends by hand on one Mac.

The coordinator can test and build the CLI from this branch with the usual Cargo target directory. Point `H2_BIN` at the resulting release binary, or use the installed copy if it contains this change.

```sh
cargo test --locked -p cli --bin h2 boot_cmd::tests::system_link_enables_accountless_networking_only_on_request -- --exact
cargo build --locked --release -p cli --bin h2
export H2_BIN="${CARGO_TARGET_DIR:-$PWD/target}/release/h2"
```

Use that CLI, the coordinator’s own disc, and a matching prebuilt clang image. Set `H2_DISC` to the ISO, `H2_IMAGE` to its output directory, and `host_profile_dir` and `client_profile_dir` to different game data directories with distinct saved profiles. Choose unused UDP bases and loopback agent ports. Run these blocks in **one macOS shell**. The image check should succeed before either process starts.

```sh
H2_BIN=${H2_BIN:-"$HOME/.cargo/bin/h2"}
export H2_OPT=2 H2_UI_WATCH=1
unset H2_SIM H2_LEVEL H2_BOOT_GFX H2_INPUT_REPLAY H2_BOOT_INPUT
"$H2_BIN" build "${H2_DISC:?set the disc path}" --out "${H2_IMAGE:?set the image directory}" --tier clang --check
```

```sh
trial=$(mktemp -d "$TMPDIR/microhost.XXXXXX")
cp -cR "${host_profile_dir:?set host data directory}" "$trial/host"
cp -cR "${client_profile_dir:?set client data directory}" "$trial/client"
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

**Zero-local passes** only if that experiment or the host-mode implementation in step 2 meets all of the following checks. Repeat with the service-owned lobby and identical client. Require **two peers, exactly one session player**, and a world player with a live unit and no `local_user` on the host. On the client the same named player must be local. Verify remote input, a death and respawn, round results free of a host entrant, a second successful start and the last client leaving. Add a second remote for opponent damage, scoring and team objectives. The profile-based control uses the original lobby. The dedicated repeat uses the [playerless lobby methods](dedicated-host-ops.md#playerless-lobby) to create and start the game without a local owner. This design still awaits a two-console run. [Player reader](../../crates/extensions/agent/src/reader.rs) · [Agent API](../runtime/agent-api.md)

Stop the two processes from this shell when finished.

```sh
kill -TERM "$client_pid" "$host_pid"
wait "$client_pid" "$host_pid"
```
