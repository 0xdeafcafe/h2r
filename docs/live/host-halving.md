# Halving the dedicated host, and scaling it

Status: Implementation and qualification. All-phantom sim matches run on Cloudflare hosts at 11 ms a tick ([notes](notes/host-halving.md)); the per-match pack targets (**S**) still await a full-pack measurement.
Code: `crates/apps/cli/src/host_cmd/pack.rs` (the pack supervisor), `crates/live/live-worker/src/hostroom.rs` and `crates/live/live-server/src/hosts/local_pack.rs` (the pool), `crates/live/live-server/src/hosts/lifecycle.rs` (leases and caps). Tests: `crates/apps/cli/src/host_cmd/pack_tests.rs`, `crates/live/live-worker/tests/hostroom.rs`, `crates/live/live-server/src/hosts/local_pack_tests.rs`.
Controls and experimental profile settings are listed in [switches.md](../runtime/switches.md).

A light match costs 6–10 ms of CPU per 33 ms tick and about 340 MB of
resident memory in the original three-match, 1 vCPU / 3 GiB sizing model
([Packed hosts](dedicated-host.md#packed-hosts)). The targets are to halve both
figures per match, start a match in under 2 s from a template process, and
let the fleet follow lease demand from one match to a thousand. One rule
holds throughout: a host is ad hoc.

Searchers and active leases create demand. An empty pack
expires after 60 seconds without demand on both local Live and Cloudflare.

Targets, per match in a full pack:

| | Today | Target | Projected (**S**) |
|---|---:|---:|---:|
| `game_cpu_ms_per_tick` | 6–10 | ≤ 3–5 | 3.5–5.5 |
| Memory (Pss: private plus its share of shared pages) | about 210 MiB (**S**; 340 MiB resident) | ≤ 170 MiB | 75–115 MiB |
| Matches per vCPU | 3 | 4–6 | 4 at 5 ms, 5 at 4 ms |
| Allocation to joinable, pack running | about 20 s | < 2 s | 1–2 s |
| Cloudflare cost per match-hour, full pack | $0.027 | ≤ $0.014 | $0.018–0.020 |

Single-match all-phantom results on Cloudflare are in [notes/host-halving.md](notes/host-halving.md). The full-pack baseline and acceptance results are pending; collect the CPU figures on the quiet laptop.

## Where the cost is today

**CPU.** The only full attribution is a Waterworks match with four *local*
bots at 14.12 ms/tick ([sim-attribution.md](../perf/sim-attribution.md)).
A host seats bots as phantoms, so it does not pay that roster's 16% for
local seats (cameras, first person, aim assist, HUD state). Scaled to the
rest, a host tick is roughly: objects and units 26%, animation 11%, Havok
18%, sound 13% (guest sound code plus the audio clock), effects 6.5%,
collision 5%, the bots' Rust brain up to 8%, runner and scheduler 10%
(**S**: the phantom-roster census is phase 0 of
[sim-rewrite.md](../engine/sim-rewrite.md) and is not taken yet).

**Memory.** One sim process on macOS has a 143 MiB dirty footprint: 64 MiB
of guest RAM, 39 MiB of other large allocations and 35 MiB of small ones.
Its 414 MiB of module text is shared
([notes/sim.md](../runtime/notes/sim.md)). The pack reserves 340 MiB for a match on an unmeasured map.

This
resident-size estimate counts shared pages in each process.

**Start.** A lease waits for the container (1–3 s on Cloudflare), the
bundle fetch on a new container, the title's boot to its main menu (about
19 s in the Mac co-op harness), then the lobby, the machine logon and the
registration ([dedicated-host.md](dedicated-host.md#where-the-game-could-run)).

**What the light profile already skips** (**Code**,
[Light host](dedicated-host-light.md#light-host)): every steady-state world draw
with the HUD and effect draws under it, texel reads and decodes, vertex and
index caches, PCM mixing and output, movies, the kernel trace, and the
disc image's resident pages after a bulk read. A gameplay frame is already
one tick: the frame wait spans two vblanks and nothing interpolates
([sim.md §1](../runtime/sim.md#1-what-a-headless-run-does-today)).

**What is left** that nobody on a host sees or hears: the guest's sound
sim (voice admission, listener queries, submix state), effect, particle
and decal updates, light updates, the observer's resource prediction, menu
and HUD state, and the wakes between ticks.

## Halving the CPU

Each row is measured the same way: the phantom-only Lockout and Waterworks
fixtures of [sim-budget.md](../perf/sim-budget.md#matched-light-profile-ab),
seed 7, three passes per arm, median `wall.ms_per_tick` unpaced; then the
same arms paced on a host, five 30-tick windows of `h2 agent hostmetrics`
after tick 300, comparing `game_cpu_ms_per_tick` and
`process_cpu_ms_per_tick` and checking `wall_ms_per_tick` against 33.3.
`h2 agent hostperf` gives the per-frame line while a run is up. Parity is
`state_hashes_v1` against the `off` arm after an A/A repeat is clean. The
quiet laptop runs every benchmark; a local run is a smoke check.

| Candidate | Saving per tick (**S**) | Risk | Parity bar |
|---|---:|---|---|
| Native subsystems on in the light profile: `impl.sim.native.anim`, `impl.sim.native.objects` (collision is always native) | 1.5–2.5 ms | Low once each switch's ladder is clean; the work is turning on what exists | hashes equal to `-impl.sim.native` |
| Sound sim in Rust on the silent clock (planned phase 4) | 0.5–0.8 ms | Medium: voice starts and completion events are gameplay-visible | hashes, plus voice start and completion order per tick |
| Effects, particles and decals in Rust (planned phase 6) | 0.2–0.35 ms | High if pruned, medium if rewritten: effect dispatch can deal damage and spawn objects, and particles share the random stream | hashes |
| Havok world and glue (planned phase 5) | 0.4–0.7 ms | High: its own acceptance bar in the rewrite plan | behaviour suites, cross-architecture hashes |
| One wake per tick, tick phases staggered across a pack | 0.1–0.3 ms | Low | hashes; inbound packet delay unchanged |
| Bot thinks off the game thread, on a pool the pack sizes | 0.4–0.9 ms of game-thread time on a match with phantoms; process CPU unchanged | Low: `H2_BOT_THREADS` batching is hash-identical to serial by design | batched equal to serial at every tick |
| Film archive only (`H2_HOST_ARCHIVE=film`) in place of `dual` | 0.1–0.3 ms | Low: stats arrive at match end, not live | none; a Live behaviour change |
| Slim module | 0, or a loss where the interpreter serves a missed owner | Medium | `h2 agent fallbacks` reads zero misses |

From a midpoint of 8 ms, the first, second, fifth and seventh rows reach
about 4.5–5.5 ms, and effects and Havok reach 3.5–5 ms. Small maps start
near 6 ms and land at 3–4 ms. Without the sound phase the target is missed
on large maps.

**Native subsystems.** The projection is the rewrite plan's own model at
*k* = 2 (10.77 of 14.12 ms after phase 3), with the local-seat share
removed from the denominator. Phase 7 (seats) is skipped for hosts.
Animation, collision and the object update always run in Rust, on hosts
as everywhere, with no profile switch.
[sim-10x.md](../perf/sim-10x.md#why-the-native-subsystems-save-nothing-as-wired)
reads the bodies and expects far less from them as wired: the object
bodies keep the lifted code's work and pay a runner round trip per
callout.

**Sound and effects.** These are the rewrite plan's phases 4 and 6, not
new skip switches. A host has no listener that matters, but which voices
start and when their packets complete feeds back into the tick
([sim-budget.md](../perf/sim-budget.md#headless-guest-dependencies)), so
the Rust body keeps admission, pitch and completion order and drops the
work that only shapes samples. Cosmetic pruning by switch stays off for the
reasons in [Measure before pruning](dedicated-host-light.md#measure-before-pruning).

**Tick rate.** The sim is fixed at 30 Hz and clients expect that rate, so
there is no lower "network rate" to drop to, and there are no in-between
frames to remove. The experimental light-host pacer groups host waits on a
30 Hz monotonic wall-clock grid. Inbound guest UDP, including datagrams from
the tunnel bridge, interrupts the grouped wait.

The guest processes its
original logical deadlines in order. Slot *n* of capacity *m* uses a phase
offset of 33.3 × *n* / *m* ms across processes. The switch defaults off
pending paced hash parity, lower process CPU, unchanged packet latency and
a four-match wall p99 under 36 ms on one vCPU.

**Bots.** `H2_BOT_THREADS=auto` sizes a pool per process, which
oversubscribes a 1 vCPU pack. Bot thinking defaults to serial. Explicit worker settings
apply per process.

A pack-wide allowance still needs a measured brain
cost and the pipeline's parity gate. Admission reads process CPU because
moving work onto another thread still consumes container CPU. A brain service shared between
processes (one pool, perception passed over shared memory) is not designed
further: it is worth building only if the brain measures above 20% of a
host tick after the rows above.

## Halving the memory

On Linux the pack supervisor reports process-group Pss, private dirty and shared clean bytes in each match’s `load` at `GET /v1/pack`. It also reports the cgroup's
`memory.current` less inactive file pages. The sim report's
`wall.peak_rss_mib`, `wall.footprint_mib` and
`wall.guest_written_pages` over `wall.guest_pages` give the same split per
run on macOS, and `rss_bytes` in `hostmetrics` tracks it live.

A figure is
taken in the lobby and at tick 1,800 of the match, for 1, 2 and 4 matches
on one map and 4 matches on four maps.

| Candidate | Saving per match (**S**) | Risk | What moves |
|---|---:|---|---|
| Level data mapped into guest RAM, not copied | 40–54 MiB private | Medium: alignment decides whether it applies | `guest_written_pages`, `Private_Dirty` |
| Template process: module tables and the menu state shared copy-on-write | 10–20 MiB private | Medium | `Private_Dirty` of a forked match |
| Heap census and budgets (74 MiB of non-guest allocations to 40) | 30–40 MiB private | Low | `footprint_mib` |
| Allocator: two arenas, freed pages returned | 5–15 MiB private | Low | `Private_Dirty`, heap mappings in `smaps` |
| Sim stores in the bundle (`H2_SIM_STORE`) | 20–40 MiB of each match's share, and a smaller bundle | Low: a miss is a named error | `Shared_Clean`, bundle bytes |
| Slim module | 10–30 MiB of each match's share, and a smaller image | Medium: coverage | resident `libh2game.so` text in `smaps` |

Private memory goes from about 143 MiB to 45–70 MiB, and a pack's shared
pages (text and stores, 120–180 MiB in all) are paid once, so a match in a
pack of four costs 75–115 MiB.

**Mapped reads.** Halo reads a level's tag data from the map into guest
RAM at load, so every match holds a private copy of bytes that are
identical across matches on that map. When the guest reads a run of whole
pages from an immutable source (a sim store, a server map or the read-only
disc mapping) into a guest address that is page-congruent with the file
offset, the VFS maps that file range private over the guest pages instead
of copying (`crates/runtime/vfs/src/sim_store.rs`,
`crates/runtime/runtime/src/guest.rs`). The guest sees the same bytes.

A
page it later writes becomes private; the rest stay one set in the page
cache for every match on the host. A read that is not congruent, is
shorter than 16 pages, or targets a writable source is copied as before,
and the report counts mapped and copied bytes. The store builder pads large ranges for congruence on 4 KiB hosts; 16 KiB hosts (Apple
silicon) qualify less often.

The mapping is private and preserves guest writes. A live alias, a write watch that needs the original backing, or an unsupported RAM layout can require copying. Compare the mapped and copied byte counters and hashes in matched runs, and require at least 40 MiB less private dirty memory per match on one map.

**Private stores and slim image.** The bundle accepts verified stores for
the two shared resource maps. The menu and playable maps remain contiguous
for native catalogue and navigation readers. Both host child paths mount bundled
stores.

The Linux staging script accepts a matching slim coverage export.
Qualification requires zero store misses and zero coverage fallbacks on
the light-host corpus and the lobby sequence.
[Build and size checks](dedicated-host-build.md#package-recorded-shared-resources)

**Guest RAM from a snapshot.** `state.load` with `"cow": true` already
maps a saved RAM image private copy-on-write
([sim.md §6.2](../runtime/sim-metrics.md#62-sharing-guest-ram-with-machine-snapshots)).
The proposed template uses it for the menu state. A *loaded-level*
template (one per map and variant, forked after load) only fits a lease
that boots straight into its map: a `sim-<run_id>` lease.

It needs the
phantom peers' host state in the snapshot, which is missing today. A pack
keeps at most two level templates, least recently used first out, and only
while a batch is queued.

**Heap.** Light hosts start interpreter analysis on the first miss and a slim
host retains one lifted cold owner. The optional dense bot visibility index
is capped at 256 KiB. Larger grids look up cells in the sorted list already
present in the navigation data. Linux lease children and the glibc image
default to two malloc arenas and a 128 KiB trim threshold.

The heap census and footprint gate still need a 2v2 match. Collect a heap
profile and Linux mapping totals, then check the macOS Waterworks
`wall.footprint_mib` against 80 MiB before budgeting additional consumers.

## How it works

### Template processes

The proposed fork slice gives a pack one template: a game process that has loaded the module,
built its dispatch tables and mapped the menu snapshot's RAM image, and
then stopped before it starts a helper thread, opens a socket or logs on
to Live. One host thread drives every guest thread
([snapshots.md](../runtime/snapshots.md)), so at that point the process has
one thread and can fork.

1. The pack supervisor starts the template at pack start
   (`h2 serve --system-link --host-light` with the template flag) and
   keeps a pipe to it.
2. `POST /v1/pack/matches` sends the slot's ports, data directory, lease
   and token down the pipe. The token is never written down, as today.
3. The template forks. The child becomes its own process group, takes the
   slot's settings, re-bases guest time and takes fresh key material (see
   Rules), starts its helper threads, binds its agent socket and tunnel
   port, and runs on from the main menu.
4. The supervisor drives it exactly as it drives a booted child:
   `host.lobby.open`, registration, start.

The menu snapshot is taken before the machine logon, so it holds no lease,
no session and no credential, and one snapshot serves every map. It comes
from the bundle when `h2 host bundle` was given one, keyed by the module
stamp, the bundle hash and the switch set. Otherwise the pack's first
start boots the title once (the 20 s path), saves the snapshot into the
host cache and becomes the template.

A snapshot that does not match is a
named error in the pack's log and in `GET /v1/pack` (`template: "bundle"`,
`"built"` or `"none: <reason>"`), and that pack boots each match the slow
way. The light profile's clock-only audio has no snapshot section today;
the template needs one that saves cursors and pending events and no
samples (`crates/platform/dsound/src/state.rs`).

On the local home the same supervisor runs the same steps. Where the
platform cannot fork the process safely (a macOS `h2` that has touched the
windowing frameworks), the supervisor spawns a fresh child that loads the
same snapshot with `"cow": true`. That shares the RAM image and costs
about a second more.

### Pack density and admission

The supervisor refuses a match above the slot limit, when any running
match has a gameplay tick p99 above 36 ms, when projected CPU exceeds 70%
of 33.3 ms per vCPU, or when projected memory leaves less than 256 MiB
free. It estimates new loads from recent measured gameplay on the same map,
using 10 ms and 340 MiB in the absence of observations. Newly allocated slots
reserve memory during boot, before their pages appear in the cgroup.

| Pack class | vCPUs | Supervisor memory limit | Slots | Provisioned Cloudflare memory |
|---|---:|---:|---:|---:|
| Small | 1 | 1,536 MiB | 5 | 3 GiB |
| Large | 2 | 3,072 MiB | 10 | 6 GiB |

Both Cloudflare classes reserve 12 GB of disk. The deployment uses the
scheduling policy that enforces Wrangler’s `max_instances` and requires
at least 3 GiB of memory per vCPU. The smaller limits are admission and
qualification budgets. Billing uses the provisioned memory.

The density
gate runs four real-time matches on one vCPU with a physical 1,536 MiB
limit for 30 minutes, checking every pack sample for a tick p99 under 36 ms.
[Cloudflare instance constraints](https://developers.cloudflare.com/changelog/post/2026-09-29-remove-disk-to-memory-ratio/) · [Scheduling policies](https://developers.cloudflare.com/containers/configuration/scheduling-policy/)

### Warm capacity from demand

Each class and region has a pool. Matchmaking counts unexpired search
tickets in dedicated-enabled hoppers by each player’s best region. Active
leases in that class and region also count as demand.

Placement fills the
most occupied pack with capacity first.

The pool estimates spare slots as twice the observed five-minute lease
arrival rate times its median cold start duration. Its minimum is one slot
for the first searcher. It subtracts free slots in occupied packs, renews
only the required empty packs, and starts at most one more per reconciliation.

Each regional class allows three speculative starts or renewal rounds between
newly admitted leases. A failed attempt consumes a round. A duplicate pulse,
an empty plan or a retried lease leaves the counter unchanged.

The counter
survives pack expiry and Worker room eviction. A native service restart
resets the in-memory pool.
A supervisor can start before a cohort forms.

Its game child and data
provisioning start with the lease. The under-two-second join gate still
depends on the proposed template.

Without searchers or active leases, the wanted count falls to zero. The
empty supervisor lingers for 60 seconds. Native Live reaps it and the Worker
persists cleanup deadlines across room eviction.

Before starting or renewing
an empty Cloudflare pack, Live records its paid interval against the host
budget. A lost response can leave a conservative reservation. A zero lease
cap or exhausted budget prevents new capacity.

### Region placement by ping

The queue advertises running managed UDP endpoints, configured UDP probes
and a ticket-authenticated WebSocket echo. Native Live answers the service
echo itself. On Cloudflare a regional pool tries an active pack's tunnel
and otherwise answers in the room.

Neither fallback starts a host. The
matchmaker prefers the running UDP sample, then configured UDP, then the
service echo. It selects the lowest worst-member RTT among regions sampled
by every member.

A warm slot within 10 ms wins. An unranked match uses its reserved
peer when a strict majority of the cohort measures over 120 ms to the region
and a faster route to that peer. Ranked matches keep the dedicated placement.

Incomplete regional coverage uses the configured default.

The host agent exposes live per-peer RTT for the same-client comparison.
A historical network median still needs a trusted association between
the measured guest peer, its Live account and its network at match time.
[Probe protocol and selection](live-matchmaking-search.md#ad-hoc-dedicated-hosts)

### Caps

`LIVE_MAX_HOSTS` counts leases and Wrangler's `max_instances` counts
containers per class. They are different numbers once matches are packed:

    max_instances = ⌈LIVE_MAX_HOSTS ÷ matches per pack × 1.1⌉ + regions in use

The deploy renderer uses the configured slots for each class, up to five
small or ten large, and counts an unconfigured region table as one pool.
Live defaults to four leases. Zero disables admission.

Caps one through
four support smaller deployments. Larger steps require an explicit setting.
The monthly Worker budget defaults to $25.

A capacity increase still
requires an independent budget decision. Unfilled packs incur provisioned
memory and disk charges, including paid idle intervals.

| Lease cap | Regions in this example | Maximum small / large containers | Qualification before raising the fleet cap |
|---:|---:|---:|---|
| 4 | 1 | 2 / 2 | Default |
| 16 | 1 | 5 / 3 | Slices 1–5 measured and 16 phantom matches for 30 minutes with no pack marked late |
| 64 | 1 | 16 / 9 | Monthly estimate within 20% of the invoice and warm starts under 2 s at p95 |
| 256 | 2 | 59 / 31 | Two regions in use and a pack killed mid-match releases its leases and slots within 30 s |
| 1,024 | 2 | 228 / 115 | A 1,000-lease phantom soak with flat pool and hub latency |

The largest accepted cap is 1,024. Qualification at scale is pending.
A managed fleet at any step still needs the rights determination in
[dedicated-host.md](dedicated-host.md#where-the-game-could-run).

## Interfaces

| Interface | Change |
|---|---|
| `GET /v1/pack` (`pack.rs`) | reports each match’s private and shared bytes, sampled peak Pss, CPU seconds and gameplay tick p99; the template field is reserved for the template work |
| `POST /v1/pack/matches` | starts a supervised game child for the lease |
| `host.metrics` (`crates/extensions/game/src/host_metrics.rs`, [agent API](../runtime/agent-api-host.md#hostmetrics)) | reports the Linux resident split, CPU seconds, and a rolling 30-second gameplay tick p99 and count above 36 ms |
| Sim report `wall` block (`crates/live/sim-bench/src/metrics.rs`) | adds `mapped_read_bytes` and `copied_read_bytes` |
| Pool `place` (`hostroom.rs`, `local_pack.rs`) | reconciles demand independently of placement; the Worker records whether a placement reused a running supervisor |
| `HostEnded` (`live-proto`) | optionally carries measured group CPU time, peak Pss, the slot lifetime, its share of occupied container time and the start-charge flag |
| `host_costs.rs` | charges settled, measured occupancy and CPU. Missing measurements, active reservations and cleanup use full-class estimates |
| `FlyProvider` (`crates/live/live-server/src/hosts/fly.rs`) | runs one Machine per lease. Multi-slot Machines need a private supervisor control path on both homes |

The agent API is the measuring instrument for every gate. `host.metrics`
is extended because the memory split and tick lateness are read again by
every later slice and by admission.

## Rules and limits

- **Ad hoc.** A new pack requires a searcher or lease in its region. A paid idle
  hold expires after the one-minute linger. A stopped Fly Machine is not running.
- **Two homes.** The supervisor and admission logic are shared by local Live and Cloudflare. A provider-specific piece (the pool
  room's echo, custom instance sizes, Fly's stopped Machines) has a local
  counterpart or is only an optimisation.
- **No shared randomness.** Every fork of one template starts from the
  same guest state. Before the lobby opens, the child re-bases guest wall
  time and the TSC from the host clock and takes new XNet key material and
  a new session ID from Live. A test forks two children and requires
  different session nonces and different first-tick random state.
- **Loud fallback.** A template that cannot be used, a mapped read that is
  refused and a store miss are each reported by name. None silently takes
  the slow path.
- **Parity.** A switch that changes the tick ships only with equal
  `state_hashes_v1` against its off arm. Clients are not in lockstep with
  a dedicated host, but the hash is the only proof the gameplay is the
  same.
- **Tick deadline over density.** Admission refuses on lateness before it
  refuses on CPU.

### Provider limits

| | Cloudflare Containers | Fly Machines |
|---|---|---|
| Instance | up to 4 vCPU, 12 GiB, 20 GB disk; custom sizes | presets; performance-1x/2 GB and 2x/4 GB in use |
| Account | 15,000 vCPU, 60 TiB memory, 300 TB disk at once; 50 GB of images | soft per-organisation Machine limits, raised on request (**S**) |
| Billing | CPU on use; memory and disk as provisioned, in 10 ms steps, until the instance sleeps | the whole Machine per second while running; a stopped Machine pays rootfs storage only |
| Player path | WebSocket tunnel through the Worker; no inbound UDP | UDP on the app's dedicated IPv4; 14,000 ports is 2,000 seven-port windows per address |
| Start | 1–3 s cold, then the bundle fetch on a new instance | tens of seconds to create, about a second to start a stopped Machine |
| Current deployment | up to 5/10 slots per small/large pack | one Machine per lease, native Live only |

[Cloudflare limits](https://developers.cloudflare.com/containers/platform-details/limits/) ·
[Cloudflare pricing](https://developers.cloudflare.com/containers/pricing/) ·
[Fly pricing](https://docs.fly.io/about/pricing)

On Fly a pack per Machine is the whole saving, because an idle core is
billed. Packed Fly Machines and a stopped-Machine pool are proposed. Fly runs
one Machine per lease on native Live and has no Worker provider.

### Cost per player-hour

All **S**. Eight players a match; the projected 5 ms and four matches per
1 vCPU / 3 GiB / 12 GB pack; rates from
[Cost and budget](dedicated-host.md#cost-and-budget). A Cloudflare pack is
$0.0300 an hour for memory and disk plus $0.0108 per match for CPU. Fill
is 85% at 100 matches and 90% at 1,000, for regions and churn.

| Concurrent matches | Containers | Cloudflare per match-hour | Cloudflare per player-hour | Today's design per player-hour | Fly, packed, per player-hour |
|---:|---:|---:|---:|---:|---:|
| 1 | 1 | $0.041 | $0.0051 | $0.0059 | $0.0058 |
| 10 | 3 | $0.020 | $0.0025 | $0.0037 | $0.0017 |
| 100 | 30 | $0.020 | $0.0025 | $0.0037 | $0.0017 |
| 1,000 | 278 | $0.019 | $0.0024 | $0.0036 | $0.0016 |

Add egress on both: at the estimate's 0.03 GB per player-hour it is
$0.00075 on Cloudflare after the included terabyte and $0.0006 on Fly, a
quarter of the total at scale. A thousand matches cost about $19 an hour
in compute and $6 in egress. Fly today, one Machine per lease, is $0.0058
per player-hour at any size. "Today's design" is priced as if its caps
allowed the count.

Cloudflare is cheaper when packs are part full, Fly
when they are full, and Fly has no tunnel leg.

## Evidence

| Fact | Label | Source |
|---|---|---|
| 6–10 ms and 340 MB per light match; admission constants | Run / Code | [Packed hosts](dedicated-host.md#packed-hosts), `host_cmd/pack.rs` |
| 143 MiB footprint, 64 MiB guest RAM, 414 MiB shared text | Run | [notes/sim.md](../runtime/notes/sim.md) |
| Subsystem shares of the tick | Run, local-seat roster | [sim-attribution.md](../perf/sim-attribution.md) |
| One host thread drives every guest thread | Code | [snapshots.md](../runtime/snapshots.md) |
| The tunnel upgrade is returned with its socket, so frames bypass the rooms | Code | `forward_tunnel` in `hostroom.rs` |
| One frame is one tick on a light host | Code | [sim.md](../runtime/sim.md#where-the-frame-loop-meets-time) |
| Cloudflare bills CPU on use and takes custom sizes | S | the provider pages above; confirm on an invoice |
| Every saving, the start time and every cost in this page | S | the gate in its slice |

## Not verified

- What share of a phantom-roster host tick each subsystem takes. The CPU
  table scales a local-seat profile.
- Whether the 6–10 ms figure was taken with any `impl.sim.native` switch
  on. If it was, the first CPU row is smaller.
- Whether the level's tag read is page-congruent, on 4 KiB and on 16 KiB
  hosts, and how many of those pages a match then writes. The store
  recording shows the offsets; it needs the destination address added.
- Whether the title reaches its main menu without a wall-clock or logon
  dependency that a shared snapshot would freeze.
- Whether a forked child's first match draws its random state from
  something the re-base covers.
- Whether a macOS `h2 serve` process is safe to fork at the template point.
- The bundle fetch time on a new Cloudflare instance, and whether its disk
  survives between starts of one instance.
- Whether a Durable Object that holds a passed-through tunnel upgrade is
  billed duration for the match. At the published rate that is about
  $0.006 per room-hour, which would be a third of a match's cost and
  argues for one room per tunnel, not two.
- Whether one Fly performance vCPU and one Cloudflare vCPU run a tick in
  the same time.
- The real egress per player. The estimate's 0.0005 GB per player-minute
  is a planning figure.

## Related

- [dedicated-host.md](dedicated-host.md): the host, packs, cost and lifecycle this page changes.
- [dedicated-host-build.md](dedicated-host-build.md): the slim host and the provider contracts.
- [sim.md](../runtime/sim.md): the light profile, stores, snapshots and the report fields.
- [sim-light.md](../perf/sim-light.md): the memory levers for sim batches, and why mapped reads and stores do not reach a sim's cache slots.
- [sim-budget.md](../perf/sim-budget.md) and [sim-attribution.md](../perf/sim-attribution.md): where the tick goes, and the A/B method.
- [sim-rewrite.md](../engine/sim-rewrite.md): the phases the CPU half depends on.
- [live-matchmaking.md](live-matchmaking-search.md#ad-hoc-dedicated-hosts): region choice and the peer fallback.
