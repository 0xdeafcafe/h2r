# Waterworks sim: every sample attributed

2026-11-20. This page takes the Waterworks profile behind
[sim-budget.md](sim-budget.md) (84,358 main-thread samples over 9,000 ticks,
four local bots, 14.12 ms/tick unprofiled) and gives every sample an owner.
It replaces the "Unreached" (37.5%) and "Multiple" (14.3%) rows of that
page's three-call census, splits the 23.8% the profiler calls "runtime", and
revises the phase order and the ms/tick projection in
[sim-rewrite.md](../engine/sim-rewrite.md).

Nothing was built or run for this. It is the existing `profile.folded` plus
static reading of the owner's XBE. A 1.00% share of the thread is 0.141
ms/tick, and one sample is 0.167 µs of each tick.

## What changed in the picture

1. **6.8 points of "runtime" are the bots' Rust brain.** The profiler gives
   `bots::`, `boot::` and `feature::` frames no category, so it charges them
   to the nearest categorised frame, `runtime::handler::execute`. Of that
   function's 5,273 self samples, 4,001 are the brain and 917 are its own
   code. The runner and scheduler together are 8.85% of the thread, not 23.8%.
2. **32% of the thread is translation tax**: work that exists only because
   the game runs as lifted x86. The guest's own instructions are 55.7%.
3. **Animation and pose is 15.3% all-in (2.16 ms), not ~5%.** Half of it was
   hidden in Unreached, in shared math and in emulation helpers.
4. **Local seats cost 16% of the thread.** Camera, first person, aim assist,
   HUD state and input for four seats that nobody watches, with the collision
   and animation they ask for. 82% of the Havok ray casts come from the
   observer camera. The product seats bots as phantoms, so this roster does
   not predict a host.
5. **Sound is 8.4% guest plus 2.5% HLE** (1.18 and 0.35 ms) on a silent run.
6. **Phases 1 to 4 of the old plan reach about 8.9 ms, not 7.5.** Its own
   exit criteria summed to 4.3 ms (9.8 ms/tick) while its last row claimed
   7.5. The target needs sound and effects as well, or the bots lane's share.

## Method

**Samples.** Each folded stack is classified again with the profiler's own
rule (`Category::of_symbol`), which reproduces its totals exactly: guest
59,690, runtime 20,050, HLE 2,816, kernel 1,633, other 123, D3D 46.

**What the stack shows.** A direct call between generated handlers nests on
the host stack, so its caller is visible. Any transfer through the runner
(an indirect call, a hooked point, a kernel or HLE call, a preemption)
returns every handler on the stack, and each ancestor is later resumed as a
new outermost frame. So 42,629 of the 66,470 samples with a guest frame show
one guest frame, and a function's visible callers are a sample of its
callers, not a census.

**Owner.** Every sampled guest function (1,968 of them) gets one code family
from an address-range map
([owner_map.py](../../scripts/sim-attribution/owner_map.py)). The image is
linked in translation-unit order, so a family is a handful of ranges. Range
edges come from named points, the engine map, the 68-slot lifecycle table at
`0x440dd8`, the object type descriptors at `0x468630`, Havok's timer names
and observed callers. A rewrite replaces code, so the owner is what a phase
removes.

**Driver.** Shared helpers (math, CRT, datum arrays, tag functions) and
service families (collision, animation, cache, model lookups) are also
charged to whoever asked. The nearest visible ancestor outside the family
decides. When the runner hid the ancestors, the outermost function's own
observed callers decide, in proportion, recursively. A function never seen
with a caller falls back to its static callers weighted by their inclusive
samples; object type callbacks are given the object dispatchers as callers
(the descriptor tables). The static graph is every direct call and tail jump
in `.text`, decoded with `llvm-objdump` from a scratch ELF at any depth.

**Kinds.** A sample with a guest frame is one of:

| Kind | Leaf | Share |
|---|---|---:|
| own | the handler's own code | 55.65% |
| mem | `h2_access_cold`, `h2_mark_dirty`, `h2_gate_try`, `h2_copy_width`: the first access of an activation takes the memory gate | 7.03% |
| direct | `h2_direct_enter`, `_exec`, `_back`: direct-call checks | 3.05% |
| sse | `h2sn_*`, `h2s_*`: packed and scalar SSE with exact flags, one lane at a time (`h2sn_host_mode` alone is 1,155 samples) | 5.03% |
| x87 | `h2x87_*`, `x87::*`: ext80 arithmetic, 2,049 of 4,577 under `h2x87_v1_unary` (square root and transcendentals) | 5.43% |
| abi | `h2abi_v1_exit`, `_charge`, `_fast_acquire`, `_budget` | 2.61% |

**Runner share.** Runner and scheduler samples have no guest frame. They are
divided by where `h2abi_v1_fast_acquire` samples fall (435 of them). A
handler entered by the runner acquires the memory view once; a direct callee
borrows its caller's. So acquire samples count runner entries per function.

**Reproduce.**

```sh
h2 analyze --functions "$TMPDIR/attr/functions.txt"
python3 -I scripts/sim-attribution/attribute.py --xbe "$H2_XBE" \
    --functions "$TMPDIR/attr/functions.txt" --profile "$PROFILE_DIR" \
    --work "$TMPDIR/attr/work" --objdump "$(brew --prefix llvm@22)/bin/llvm-objdump" \
    --ms 14.12 --log "$RUN_LOG"
```

`h2 analyze` reads the disc named by `H2_DISC`. The scratch ELF and
disassembly stay under `--work`. The script holds addresses and names only.

## The whole thread

| Part | Samples | Share | ms/tick |
|---|---:|---:|---:|
| Guest own code | 46,948 | 55.65% | 7.86 |
| Translation tax under a guest frame (mem, direct, sse, x87, abi) | 19,522 | 23.14% | 3.27 |
| Runner and scheduler | 7,462 | 8.85% | 1.25 |
| Bots brain (Rust, inside a hook) | 5,743 | 6.81% | 0.96 |
| Audio clock advance | 1,534 | 1.82% | 0.26 |
| Kernel export calls and file I/O | 1,279 | 1.52% | 0.21 |
| XAPI input sync | 632 | 0.75% | 0.11 |
| DirectSound and other HLE calls | 675 | 0.80% | 0.11 |
| Feature hooks, sim metrics, other host | 563 | 0.67% | 0.09 |

Removing every point of translation tax and the runner would leave 9.6
ms/tick. The target also needs the guest's own code to get faster.

## Attribution

All-in is own code plus tax plus the runner share. "Owner" charges a sample
to the family whose code was running. "Primary" moves math, CRT, datum and
tag-function helpers to the family that called them. Shares are of the
thread.

| Subsystem | Family | Owner all-in | Primary all-in | Primary ms | Notes |
|---|---|---:|---:|---:|---|
| Objects and units | object core `b6000..c0b00`, `108a00..109300` | 7.71% | 9.27% | 1.310 | update walks, functions, markers, spatial marks |
| | units `c4000..d4de0`, `10dc70..115000` | 5.81% | 6.66% | 0.941 | unit update, aim, seats, unit pose |
| | bipeds `dc400..e7000` | 1.91% | 2.74% | 0.386 | |
| | vehicles `ee900..f8000`, `205000..207b00` | 2.42% | 2.85% | 0.403 | |
| | weapons, projectiles, equipment `f8000..108a00` | 1.44% | 1.46% | 0.206 | |
| | lights `c0b00..c4000`, `131c20..131ff0` | 1.21% | 1.33% | 0.188 | |
| | model nodes and markers `1d5000..1da000` | 0.92% | 1.68% | 0.237 | |
| | damage, devices, scenery | 0.76% | 0.77% | 0.108 | |
| Animation | manager, graph, sampling, codecs | 7.61% | 8.40% | 1.185 | 20% asked by first person |
| | first-person weapon `165000..168800` | 0.85% | 0.97% | 0.137 | |
| Collision | BSP, cluster walk, object query | 4.00% | 4.11% | 0.580 | plus 1.80% of Havok ray casts below |
| Havok | library `2d8000..320560` | 9.93% | 10.05% | 1.419 | |
| | game-side glue | 4.97% | 5.18% | 0.731 | world step, object bodies, shapes, character physics |
| AI, actors, pathfinding | | 0.28% | 0.28% | 0.040 | no actors on a multiplayer map |
| Effects and particles | effects, particles, decals | 4.64% | 5.46% | 0.772 | |
| HaloScript | | 0.19% | 0.19% | 0.026 | |
| Sound | sound manager, game sound, low level | 8.11% | 8.37% | 1.182 | plus 2.47% in the audio clock and DirectSound HLE |
| Networking, lockstep | session, simulation records | 1.37% | 1.46% | 0.207 | |
| Render front-end | | 0.32% | 0.32% | 0.045 | the world render is skipped |
| UI | UI update, per-user HUD state | 1.59% | 1.75% | 0.247 | |
| Players | player update, control, aim assist | 4.01% | 4.55% | 0.643 | |
| Camera | director, observer, cameras | 2.74% | 3.29% | 0.464 | |
| Game engine | modes, scoring, results | 1.67% | 2.00% | 0.283 | |
| Cache and resources | animation cache, predicted resources, cache files | 2.07% | 2.34% | 0.330 | |
| Main loop, input | tick, game time, input, rumble | 1.59% | 1.65% | 0.233 | |
| Shared math | `141350..146240`, geometry helpers, `30bf0` | 5.22% | (moved) | | 32% objects, 13% units, 11% animation, 9% camera |
| Shared CRT | `320560..338000`, `372000..379c60` | 3.29% | (moved) | | includes `printf` from a per-frame map status poll |
| Datum arrays, tag functions | | 1.04% | (moved) | | |
| Shared, caller unknown | | | 0.51% | 0.072 | |
| **Guest frames and runner** | | **87.64%** | **87.64%** | **12.375** | |
| Host only (table above) | | 12.36% | 12.36% | 1.745 | |

### Where Unreached and Multiple went

The three-call census reproduces from the same roots (Unreached 22,367 self
samples here against 22,370 published; Multiple 8,522 against 8,511).

| Now owned by | Of Unreached | Of Multiple |
|---|---:|---:|
| Havok library | 3,204 | 55 |
| Havok glue | 2,594 | 288 |
| Vehicles | 1,723 | 10 |
| Sound | 1,693 | 130 |
| Effects and particles | 1,597 | 24 |
| CRT | 1,595 | 364 |
| Animation | 1,591 | 489 |
| Players, aim assist | 1,189 | 21 |
| Units | 1,135 | 642 |
| Bipeds | 1,071 | 272 |
| Weapons | 562 | 101 |
| Camera | 506 | 72 |
| Shared math | 481 | 1,848 |
| UI | 440 | 6 |
| Object core | 176 | 2,359 |
| Tag functions, datum arrays | 3 | 668 |
| Everything else | 2,807 | 1,173 |

Unreached was mostly code behind an indirect call (Havok's agents and
solver, type callbacks for vehicles and bipeds, the sound and effect
records) and code more than three calls deep. Multiple was the object core
and math, each reached from several roots.

### Who asks for the shared work

| Family | Samples | Asked by |
|---|---:|---|
| Collision | 3,114 | camera 32%, objects 24% (spatial visit marks, cluster walk), sound 19% (listener leaf, obstruction), players 11% (aim assist), Havok glue 6%, effects 3% |
| Animation | 5,696 | units 29%, first person 20%, objects 13%, vehicles 5%, Havok glue 4%; 20% are codec leaves with no visible caller |
| Cache | 1,469 | animation 32% (`1236f0`, `1237e0`), camera 30% (predicted resources `16e5e0`), game engine 15% |
| Shared math | 4,235 | objects 32%, units 13%, animation 11%, camera 9%, effects 8%, bipeds 8% |
| Havok glue | 3,551 | bipeds 26%, tick 15%, objects 14%; 32% entered from Havok |

Charging the services to who asked gives the end consumers. Seats are the
surprise: camera 6.20%, players 4.63%, first person 3.16%, UI 1.48%, input
0.56%, together **16.0% of the thread (2.26 ms)** before their runner share,
for four local seats. The others: objects 10.84%, units 8.73%, sound 8.20%,
Havok 7.78% (world step, ray casts excluded), effects 5.32%, Havok glue
5.11%, vehicles 3.09%, bipeds 2.94%, game engine 2.26%.

### Havok by stage

Stacks are classed by the innermost stage marker on them.

| Stage | Samples | Share | Asked by |
|---|---:|---:|---|
| Convex agents (`2daf70`, `2ee610`, `3048a0`) | 1,609 | 1.91% | world step |
| Ray casts through the float MOPP (`244980`, `2df720`, `301800`, `3009e0`) | 1,516 | 1.80% | camera 82%, players 9%, units, damage, weapons |
| BV-tree and weld agent (`2fa2f0`, `2fa64e`) | 1,043 | 1.24% | world step |
| Solver and integrate (`31c100`, `31c940`, `31f620`) | 814 | 0.96% | world step |
| Character physics (`1f19c0`, `1f2250`, `1e55d0`) | 673 | 0.80% | bipeds |
| Integer MOPP box query (`2ff8b0`, `300150`) | 666 | 0.79% | world step |
| Structure shape callbacks (`1edd00..1eff40`) | 645 | 0.76% | world step, 8% ray casts |
| Broadphase and collide driver (`3175e0`, `317950`) | 226 | 0.27% | world step |
| Other library code | 2,202 | 2.61% | world step |
| Other glue (object bodies `1ce000..1d5000`, `2268e0`) | 2,129 | 2.52% | tick 26%, objects 22%, bipeds 11% |

The ray casts belong to the collision phase: `1697c0` reaches them through
`244980`, and the rest of Havok never calls them.

## The runtime share

| Profiler row | Samples | What it is |
|---|---:|---|
| `handler::execute` self | 5,273 | 4,001 bots brain, 150 sim metrics, 205 other hooks, 917 its own code |
| `Runner::run_from` self | 3,833 | the runner loop |
| resolve (`resolve_resume`, `resolve`, `resolve_miss`) | 1,339 | 660 of it resuming a returned frame |
| x87 under a guest frame | 4,577 | sound 780, Havok 906, camera 515, units 482, players 460, effects 387 (primary) |
| exit, charge, acquire, budget | 2,203 | once per handler activation |
| Other bots frames, scheduler, `ExecCtx::new`, replay, checks | 2,825 | |

### Which call patterns cause the runner's share

Runner entries, from the 435 acquire samples:

| Cause | Share | Where |
|---|---:|---|
| Indirect entry: an address-taken function with no direct caller | 19% | object type callbacks (`bcc20`, `c6b90`, `c6b30`), animation codecs (`28c470`), Havok virtual methods, Halo shape callbacks (`1edf50`, `1ef950`) |
| Resume in a function that makes indirect calls | 13% | `108d90`, `108c60` (type dispatch), node evaluators `27ace0..280ac0` (codec table), `2fa2f0`, `177040` |
| Hooked points and their callers | 3% | `GAME_TICK`, `OBJECT_DAMAGE_UPDATE` (per object, per tick, from h1-health) |
| Resume after a deeper unwind, a preemption or a refused direct call | 65% | everywhere: `bd090`, `ff5f0`, `bab40`, `166d75`, `1c66a0` |

The last row is the first two multiplied. One indirect call at depth *d*
costs the call, the return and one resume for each of the *d* ancestors,
because every ancestor lost its host frame. Preemption adds little: 87,424
preemptions over 9,000 ticks is under ten a tick. By family the entries fall
on objects 15%, animation 10%, units 9%, sound 8%, Havok 8%, Havok glue 6%,
effects 6%, players 5%.

The image has 3,735 indirect call sites. Havok holds 1,081, the CRT 607, UI
544, networking 310, animation 278 and Havok glue 206.

The exact counts need a run: `H2_DISPATCH_STATS=1` and `H2_ENTRY_STATS=1`
(census below). The sample estimate has a standard error near 2 points on
the 19% row.

### What a rewrite removes

A subsystem taken over at one entry runs as plain Rust inside the boundary:

- **mem, direct, abi and the runner share go completely** for calls inside
  the boundary. That is 21.5% of the thread across all families.
- **sse goes.** Native lanes do the same operation in one instruction. The
  existing ports already prove the bit-exact form.
- **x87 mostly stays under bar E.** The Rust kernel needs the `x87` crate's
  exact operation unless the double result is proven equal. The projection
  keeps 40%. Sound (dB and distance curves) and Havok hold most of it.
- **Own code gets faster by an unknown factor *k*.** The lifted code pays
  for guest stack traffic, flags and 32-bit pointer chasing. Phase 1a's
  measured delta calibrates *k*; the projection shows 2, 3 and 4.
- **A boundary cost stays** for callbacks into guest code and for guest
  callers of Rust entries. The projection keeps 10% of each phase's all-in.

Per-function ports pay the opposite way. A hooked leaf leaves the
direct-call path (the feature plan prints that warning for hot points), so
its caller chain unwinds on every call. This profile has no ports installed
and records no timing of them.

## Revised phase order and projection

All-in cost per phase scope on this profile, and the modelled saving:

| Phase | Scope | All-in | ms | Saves, *k* = 2 / 3 / 4 |
|---|---|---:|---:|---|
| 1 | Animation and pose: sampling, codecs, manager, graph resolve, node matrices, unit pose, first person, markers | 15.27% | 2.156 | 1.29 / 1.50 / 1.61 |
| 2 | Collision and queries: BSP, cluster walk, object query, the Havok ray path | 6.33% | 0.894 | 0.46 / 0.56 / 0.61 |
| 3 | Objects and types: object core, units, bipeds, vehicles, weapons, lights, damage, tag functions | 20.59% | 2.907 | 1.60 / 1.92 / 2.08 |
| 4 | Sound sim | 8.37% | 1.182 | 0.64 / 0.77 / 0.83 |
| 5 | Havok world and its glue | 13.27% | 1.873 | 1.00 / 1.21 / 1.32 |
| 6 | Effects and particles | 5.46% | 0.772 | 0.43 / 0.51 / 0.55 |
| 7 | Seats and rules: players, aim assist, camera, HUD state, input, game engine | 12.19% | 1.722 | 0.96 / 1.13 / 1.22 |
| | Networking (stays) | 1.46% | 0.207 | |
| | Cache, main loop, render | 4.22% | 0.596 | 0.36 / 0.42 / 0.45 |
| | AI and HaloScript | 0.47% | 0.066 | campaign census |

Cumulative ms/tick from 14.12:

| After phase | *k* = 2 | *k* = 3 | *k* = 4 |
|---|---:|---:|---:|
| 1 | 12.83 | 12.62 | 12.51 |
| 2 | 12.37 | 12.06 | 11.90 |
| 3 | 10.77 | 10.14 | 9.82 |
| 4 (sound) | 10.13 | 9.37 | 8.99 |
| 5 (Havok) | 9.13 | 8.16 | 7.67 |
| 6 (effects) | 8.70 | 7.65 | 7.12 |
| 7 (seats) | 7.74 | 6.52 | 5.90 |

**7.48 ms/tick on this roster needs phases 1 to 6 at *k* ≥ 3.3.** At
*k* = 3 it needs phases 1 to 6 plus 0.17 ms from elsewhere (a fifth of the
bots brain), or phases 1 to 5 plus the seat work. Phases 1 to 3 and Havok
alone end near 8.9 ms at *k* = 3.

Changes to the order, and why:

- **Phase 1 grows a slice, 1d.** The animation manager (`1c6440..1c7450`,
  `1cb000..1ce000`) and graph resolve (`1daea0`, `1dd9d0`, `1ddb40`) are
  3.4% of the thread, as much as sampling and codecs (3.5%). The old plan
  left them guest. Node-matrix build is 2.9% and unit pose, first person and
  markers 3.9%.
- **Sound moves ahead of Havok.** It is 1.18 ms guest plus 0.35 ms of HLE,
  every host runs it silent, and it carries far less exactness risk than a
  bit-exact Havok. Its BSP queries arrive with phase 2.
- **Havok stays a single late phase** but its ray path moves into phase 2
  (1.80%). Without the ray path it is 13.3% all-in, in line with the old
  1.5 to 2.5 ms guess.
- **Seat work becomes a named phase.** Its order waits for the phantom
  census: if phantom seats skip the camera, first person, HUD and aim
  assist, phase 7 is already paid for on hosts and matters only to players.
- **Three small fixes need no rewrite:**
  - the per-frame map status poll `163680 → 213f10 → 214db0` formats a path
    with `sprintf` and compares it every frame: 265 visible samples (0.31%)
    plus about 0.5% in the CRT's `printf` and string compare beneath it;
  - `OBJECT_DAMAGE_UPDATE` (`d5de0`) is hooked before and after by h1-health
    for every object every tick, so `bc470` unwinds on each one;
  - the profiler should give `bots::`, `boot::`, `feature::`, `sim::`,
    `input::` and `engine::` frames a category of their own (`host`) so
    "runtime" means the runtime.

## Phase 0 census for the coordinator

Use one optimised CLI built with `--features perf` and one full clang image
for every arm. Set `H2_DISC`, `H2_IMAGE`, `H2_DATA` to absolute paths and
`H2_XBE` to the owner's `default.xbe`. Each arm gets a fresh writable data
directory with the bots' navigation files linked in, as in sim-budget.md.
Profile at 500 Hz: at 1,000 Hz the sampler's pause doubled the wall time.

### A. Waterworks, both rosters (unattended, about 40 minutes)

The same match as the 2026-11-20 profile (`waterworks.toml` there: team
Slayer, seed 20261119, 9,000 ticks, two legendary and two heroic bots,
unpaced, `render_every = 0`), once with four local seats and once with four
phantoms.

```bash
set -euo pipefail
out=$(mktemp -d "${RUSH_KEEP:?}/census.XXXXXX")
unset H2_PROFILE H2_TIMELINE H2_HOST_AUDIT H2_LOCKSTEP H2_HOST_MODULE H2_SERVER_MAP_DIR H2_FEATURES
match=${MATCH:?the waterworks.toml of the 2026-11-20 profile}

arm() (   # arm <roster> <label> [VAR=value ...]
    roster=$1; label=$2; shift 2
    scratch=$(mktemp -d "${TMPDIR:?}/h2-census.XXXXXX"); trap 'rm -rf -- "$scratch"' EXIT
    ln -s "${H2_DATA:?}/bots" "$scratch/bots"
    phantom=0; [[ $roster == phantom ]] && phantom=1
    env "$@" H2_OPT=2 H2_BOTS_ALL_PHANTOM=$phantom \
        h2 sim run "$match" --disc "${H2_DISC:?}" --out "${H2_IMAGE:?}" --tier clang \
        --data "$scratch" --pace unpaced --report "$out/ww-$roster-$label.json" \
        > "$out/ww-$roster-$label.log" 2>&1
    jq -e '.result.end == "tick_limit" and .ticks == 9000 and .nav == "loaded"' \
        "$out/ww-$roster-$label.json" > /dev/null
)

for roster in local phantom; do
    for pass in 1 2 3; do arm $roster time$pass; done
    arm $roster dispatch H2_DISPATCH_STATS=1
    arm $roster entries  H2_ENTRY_STATS=1
    arm $roster profile  H2_PERF_FROM=level H2_PROFILE_HZ=500 H2_PROFILE_CPU=1 \
        H2_PROFILE="$out/prof-ww-$roster"
    jq -s --arg r $roster '{roster: $r, median_ms_per_tick: (map(.wall.ms_per_tick) | sort | .[1])}' \
        "$out"/ww-$roster-time{1,2,3}.json
done
```

Then, for each roster:

```bash
h2 analyze --functions "$out/functions.txt"
python3 -I scripts/sim-attribution/attribute.py --xbe "${H2_XBE:?}" \
    --functions "$out/functions.txt" --profile "$out/prof-ww-$roster" \
    --work "$out/work" --objdump "$(brew --prefix llvm@22)/bin/llvm-objdump" \
    --ms "$median" --log "$out/ww-$roster-profile.log" > "$out/attribution-ww-$roster.txt"
```

Wanted from it: the local roster's table should match this page within a
point per row (same build, same seed). The phantom table shows which seat
rows vanish and what networking grows to. `dispatch_per_tick` and the entry
census replace the acquire-sample estimate above.

### B. Co-op campaign (two windows, about 25 minutes, needs the machine)

Level `03a_oldmombasa`, heroic, two players: an opening cutscene, then
squads, vehicles and scripts. Profile the host for ten minutes of play from
level load. Then repeat on `05a_deltaapproach` for a vehicle-heavy level.

```bash
root=$(mktemp -d "${RUSH_KEEP:?}/census-coop.XXXXXX")
h2 coop test --disc "${H2_DISC:?}" --out "${H2_IMAGE:?}" --root "$root" \
    --players 2 --auto --drive --dry-run
```

`--dry-run` prints each game's command line with its variables. Run the
Live server and Bravo's line as printed, and Alpha's line with this in
front, so only the host is profiled:

```bash
H2_PERF_FROM=level H2_PROFILE_HZ=500 H2_PROFILE_CPU=1 H2_PROFILE="$root/prof-coop-03a" \
H2_FRAME_STATS=1 H2_FRAME_STATS_CSV="$root/frames-03a.csv" H2_DISPATCH_STATS=1
```

With no `H2_LEVEL`, `H2_PERF_FROM=level` opens the session at the first game
that is not the main menu, which is the co-op level. Pick Old Mombasa and
Heroic on the host's Co-op Campaign screen, start, play both players through
the first two encounters for ten minutes, then quit from the pause menu so
the session writes its files. Run the attribution command above on
`$root/prof-coop-03a` with `--ms` set to the frame stats' mean guest time.

Read from it: the AI, actor, pathfinding and HaloScript rows (0.5% here),
the co-op session's networking row, and whether object, animation and Havok
shares hold with thirty actors on the map. That fixes the order after
phase 3.

Not verified here: that `h2 coop test --dry-run` prints lines that work when
started by hand without the harness's driver, and that `h2 play` writes the
profile when quit from the pause menu. If either fails, the one change
needed is a `--host-env KEY=VALUE` option on `h2 coop test` that adds
variables to the host game only (the CLI belongs to the co-op session).
Without it, `H2_PROFILE` set in the shell reaches both games and they write
the same directory.

### C. Inclusive phase times (optional, unattended)

The stack cannot show a tick phase's inclusive time. One more Waterworks arm
with `H2_HOST_AUDIT="$out/audit.json"` gives CPU per hooked phase
([sim.md](../runtime/sim.md#host-audit-corpus)) to check the driver split.

## Confidence

| Claim | Confidence | Why |
|---|---|---|
| Category and kind totals | exact | the profiler's own rule, totals reproduced |
| Bots brain share | exact | `bots::` frames on the stack |
| Owner of a guest self sample | 78.8% high, 16.9% medium, 4.3% low | high: a named point, documented address or type table in the range; medium: lifecycle slot and callers; low: neighbourhood only |
| Driver of shared work | medium | callers are visible for 36% of guest-frame samples; the rest assume a function's hidden callers match its visible ones |
| Runner share per family | medium | 435 samples; about ±1.7 points on a 15% row |
| Runner cause split | low to medium | static class of the entered function, not a transfer log |
| Projection | low | *k*, the x87 share kept and the boundary cost are assumptions until phase 1a lands |

Low-confidence ranges with more than 50 samples: the lifecycle-39 records at
`17b7b0..17c900` and the frame update at `17cc60..17d220` (both called
effects), `2266a0..22a000` (called Havok glue from its callees and globals),
`180000..183e40` (Havok glue, called from vehicle code), `a0000..a8000`
(vector validity checks, called math), and `1e6a40..1eb550` (per-player
weapon HUD and target state, called UI).

## Slices a lane can start

1. **Profiler categories.** `crates/perf/src/symbolize.rs`: a `host`
   category for `bots::`, `boot::`, `feature::`, `sim::`, `input::`,
   `engine::`, `live_telemetry::` frames; update the tests beside it and
   the table in profiling.md.
2. **Attribution in the tool.** Port `attribute.py` to `h2 profile attribute`
   in `crates/perf`, reading the owner map as data, so a census prints this
   page's tables.
3. **Map status poll.** Read `163680` and `213f10`: find why the status is
   asked every frame in a game, and answer it from the last result when the
   path and slot generation are unchanged.
4. **Damage hook.** Move h1-health off `d5de0`'s before and after pair onto
   the damage application points, or gate it on the object being a player
   biped before leaving the direct path.
5. **Phase 1a to 1d** as in sim-rewrite.md, with 1d added.

## Risks and open points

- **The roster.** Four local seats are not the product. Every seat row and
  the bots row may shrink or move under phantoms. Do not fix the order after
  phase 3 before census A's phantom arm.
- **Campaign.** AI is 0.3% here and unknown there. Census B decides whether
  AI, pathfinding or HaloScript jumps ahead of sound and Havok.
- **Hidden callers.** A function reached only through the runner has no
  visible caller in any sample. Its driver comes from static callers, which
  cannot weigh cold against hot paths.
- **Range edges.** The owner map assumes link order. A helper placed in
  another unit's range is misfiled; the caller split bounds the damage.
- **The model.** The saving per phase is arithmetic on assumptions. Only a
  measured phase calibrates it.
- **x87 under bar E.** Sound and Havok hold 0.13 and 0.16 ms of ext80 work.
  The model keeps 40% of it; all of it stays unless each sequence is proven
  equal in double.
- **Not resolved:** which guest thread makes the 579 file I/O samples (the
  stack has no guest frame; the sound and animation caches are the static
  candidates), and what `2268e0` does each tick.

## Summary

1. All 84,358 samples have an owner; Unreached and Multiple are gone.
2. Guest own code is 55.7%, translation tax 32.0%, host-only work 12.4%.
3. "Runtime 23.8%" is 6.8 bots brain, 8.9 runner and scheduler, 5.4 x87, 2.6 ABI.
4. Runner entries come from indirect calls and the resumes they force.
5. Animation and pose is 15.3% all-in, the best first phase by size and risk.
6. Objects and types are 20.6%, Havok 13.3%, sound 8.4% plus 2.5% HLE.
7. Local seats drive 16% of the thread; the camera asks for 82% of ray casts.
8. Old phases 1 to 4 end near 8.9 ms; 7.48 needs sound and effects too.
9. New order: animation, collision, objects, sound, Havok, effects, seats.
10. Census A (phantom roster, dispatch counts) and B (co-op 03a) set the rest.
