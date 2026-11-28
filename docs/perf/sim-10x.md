# One sim match at 10 to 20 times real time

Status: Design. Of its lanes only B2 is built, unmeasured; the levers it ranks that are built are unmeasured.
Code: none of its own. Each lane below names its files. Tests: named per lane.
Switch: proposed per lane, default off; each joins [switches.md](../runtime/switches.md) when its lane lands.

`h2 sim run` plays one headless 2v2 bot match. On a quiet M-series Mac a
Lockout match with four phantom bots costs about 5.5 ms of host work per
33.3 ms tick, 6 times real time
([notes](notes/sim-10x.md)). The target is 10 times (3.3 ms) and then 20
times (1.7 ms) for one match; batches already exceed 20 times in aggregate
by running processes side by side ([sim.md §7](../runtime/sim.md#7-parallel-runs-ab-and-tuning)).
The rule that binds every lever: the same build and switches give the same
match ([sim.md §8](../runtime/sim.md#8-determinism)), and a lever that
replaces guest code is proven by equal state hashes against the lifted
code ([sim-rewrite.md](../engine/sim-rewrite.md), bar E), unless the user
signs it off as deterministic only.

Evidence labels are those of [docs-style.md](../meta/docs-style.md).
Every ms figure is **S** until its measurement in "Measurements" is taken.

## The budget

No Lockout profile and no phantom-roster profile exists. The table scales
the Waterworks attribution ([sim-attribution.md](sim-attribution.md)) to
Lockout: remove what four local seats ask for, remove vehicle object code,
scale to 5.50 ms. The arithmetic and its blind spots are in the
[note](notes/sim-10x.md); each row is ±30%.

| Family | ms/tick | Share |
|---|---:|---:|
| Objects and types (object core, units, bipeds, weapons, lights) | 1.59 | 29.0% |
| Havok library and glue | 0.91 | 16.5% |
| Sound sim (guest) | 0.56 | 10.1% |
| Bots brain (Rust, inside the pad hook) | 0.45 | 8.3% |
| Animation | 0.45 | 8.1% |
| Effects, particles, decals | 0.36 | 6.6% |
| Players and game engine | 0.28 | 5.2% |
| Cache, main loop, AI, scripts, render, unknown | 0.27 | 4.9% |
| Kernel, XAPI, feature hooks | 0.20 | 3.6% |
| Audio clock and DirectSound HLE | 0.17 | 3.2% |
| Collision | 0.16 | 2.8% |
| Networking | 0.10 | 1.8% |
| **Total** | **5.50** | |

The same tick by kind of work:

| Kind | ms/tick | What removes it |
|---|---:|---|
| Guest own code (lifted x86 doing the game's work) | 2.97 | a better compile of the same code, or a rewrite |
| mem: memory gate and slow path | 0.375 | flat RAM |
| direct, abi: call checks, exit, charge, acquire | 0.30 | thin transfers; the tier |
| sse: per-operation SSE helpers | 0.27 | a wider mode sample; the tier |
| x87: ext80 helpers | 0.29 | the tier; the inline binary64 path ([x87-native.md](../lifter/x87-native.md)) already takes what it can prove |
| Runner and scheduler | 0.47 | thin transfers; fewer hand-ups |
| Host only (brain 0.45, audio 0.17, kernel, input, hooks 0.20) | 0.82 | threads for the brain; no sound; small fixes |

**What 3.3 ms requires.** 2.2 ms must go. Every row except own code sums
to 2.53 ms, and not all of it can go (exact x87 stays, some runner entries
stay, the brain still reads the guest). A realistic take of the
non-own rows is 1.4 to 1.6 ms: flat RAM, the transfer slices, the brain on
worker threads, sound off, the small host fixes. The last 0.6 to 0.8 ms
has to come out of guest own code: about a quarter of it running three
times faster or more.

**What 1.7 ms requires.** With the host rows at 0.3 ms and no translation
tax at all, guest own code may cost 1.4 ms. It costs 2.97. So even a
perfect runtime needs every guest family, Havok, effects and networking
included, to run 2.1 times faster; with a realistic runtime, 3 times. No
subset of subsystems reaches it.

## Why the native subsystems save nothing as wired

The first run with `impl.sim.native` actually installed measured the same
speed as off ([notes](notes/sim-10x.md)). The code says why (**Code**).

- **The object bodies are ports of the machine, not of the work.**
  `crates/rewrites/engine/src/sim_objects` (96 bodies, about 31,700 lines)
  reproduces each function register by register over the retail stack
  frame: every push, every flag (`sim_physics::logic`, `arithmetic`),
  every load and store through `Guest::u32` (a page-table lookup and a
  bounds check each, `crates/runtime/runtime/src/guest.rs`,
  `read_bytes`). That is the callout contract of
  [sim-rewrite-objects.md](../engine/sim-rewrite-objects.md#bodies-segments-frames),
  and it makes the bodies swappable with the lifted code at any site. It
  also makes their own-code cost about that of the lifted code (*k* near
  1). Clang deletes dead flag writes in the lifted function; the Rust
  body computes them into `CpuAbi`.
- **What a fused body can save is small.** Inside `drive`
  (`sim_objects/mod.rs`) a call between two bodies skips the direct-call
  checks, the exit and the runner. For the object family that tax is about
  2.5 points of the thread, 0.14 ms on Lockout. That is the ceiling of
  this style with no callouts left.
- **Every callout costs more than the call it replaces.** A body that
  calls code with no body returns `TransferAbi::call(target, site)`. The
  host-call thunk continues inline only on a plain return
  ([contract §4.2](../runtime/contract-thin-transfers-flat-ram.md#42-transfers)),
  so a callout is a hand-up: every generated caller above the body loses
  its host frame. The callee then runs as an outermost handler (its own
  `h2abi_v1_fast_acquire`), its return goes through the runner's resume
  lookup to the body's installed closure
  (`crates/rewrites/sim-bridge/src/sim_objects.rs`, `install_table`),
  which takes a memory guard (`ctx.memory()`: a mutex, and under the paged
  model the gate and the dirty drain), checks the float environment,
  binary-searches the body table and re-enters the body's `match` on `pc`.
  The lifted code makes the same call in place.
- **Callouts are everywhere.** The bodies name about 100 distinct guest
  callees, among them the vector normalise `30bf0`, the frame-to-matrix
  `1420f0` and the CRT `_ftol2`. The unit shell alone makes nine calls.
- **Animation is a real kernel but a thin slice.**
  `engine::sim_anim::stream` samples from a decoded clip with no guest
  access ([anim-sampling.md](../engine/anim-sampling.md)). It replaces
  `2798a0` and below: 3.5% of the Waterworks thread, less the first-person
  share a phantom roster does not have. About 0.19 ms on Lockout all-in.
- **Collision takes two leaves.** `impl.sim.native.collision` installs
  point-to-leaf and location only; the vector and instance traces run the
  original while their float environments are observed
  (`crates/rewrites/sim-bridge/src/sim_native.rs`, `COLLISION_OBSERVED`).

So the expected result of turning the three on is animation's 0.1 ms,
plus or minus the callout cost of the objects. Census M2 measures both.

## Levers, ranked

Savings are on the 5.5 ms Lockout tick. "Ceiling" is what the lever gives
if everything in its row goes. Rank is expected ms per unit of lane work,
discounted by risk.

| # | Lever | Expected | Ceiling | Work | Risk | Verdict |
|---|---|---:|---:|---|---|---|
| 1 | Bot thinks on worker threads (`H2_BOT_THREADS`; `H2_BOT_PIPELINE`) | 0.2–0.3 batched, 0.35–0.4 pipelined | 0.45 | built; one small lane to make it the single-match default | low: batched equals serial by design; pipelined is a different, repeatable match | do first |
| 2 | Flat RAM (`H2_FLAT_RAM=1`, slice F3) | 0.25–0.35 | 0.45 | built, unmeasured; coordinator time and a fix lane if fixups are not zero | medium: the earlier flat model was slower and diverged | measure first |
| 3 | Thin transfers T1–T3 measured, T4 if earned | 0–0.3 | 0.6 | built; unknown whether the 5.5 ms image has them | low | measure first |
| 4 | Native animation on (`impl.sim.native.anim`) | 0.10–0.15 | 0.19 | built, never timed; its run ladder is not climbed | low | measure, then default on in sim |
| 5 | Small host fixes: map status poll, input sync with no pad, file path rebuild, per-tick component strings | 0.10–0.20 | 0.25 | two small lanes | low | do |
| 6 | Sound off in sims through the title's own no-device path | 0.5–0.65 | 0.73 | one static-reading step, one lane | medium: leaves bar E; needs sign-off unless the gameplay digest holds | blocked: the read found no complete no-device path; the per-map init calls DirectSound with the null interface ([note](notes/sim-10x.md#2026-11-22-lane-c1s-read-the-titles-no-sound-path)) |
| 7 | **The sim tier in the image**: tick closures recompiled with registers as locals, unchecked flat memory, host floats, exact fallback | 0.9 (leaf families), 1.7 (60% of guest time), 2.4–3.0 (85%) | 3.7 | six to eight lanes; [spec](sim-10x-tier.md) | high: its 5× gate has no number yet | the only route to 20×; start with its two measurements |
| 8 | Phantom seat residue: camera, first person, HUD and aim work a phantom host still runs ([sim-headless-skips.md](sim-headless-skips.md): the title gates the guest side off by itself; what is left is the phantoms' action build, measured at 1.15 ms of a 5.19 ms tick, [sim-phantom-build.md](sim-phantom-build.md)) | 0.8–0.95 | unknown | lanes P0 and P1 of [sim-phantom-build.md](sim-phantom-build.md) | medium | do |
| 9 | In-place callouts for Rust bodies ([spec](sim-10x-callouts.md)) | 0.10–0.25 with objects on | 0.3 | one lane | medium: runner invariants | do; the tier's call-out uses the same primitive |
| 10 | Emitter: sample the host float mode once per handler activation, not once per run of SSE operations (`h2sn_host_mode` is 1.4% of the Waterworks thread) | 0.05–0.15 | 0.27 | one emitter lane, one image rebuild | medium | batch with the next image rebuild |
| 11 | PGO image trained on sim matches (`H2_PGO`, [BUILD-CACHE.md](../lifter/BUILD-CACHE.md)) | 0.1–0.25 | unknown | built; two image builds | low | measure in a quiet gap |
| 12 | Effects and particles: phase 6 at bar E, or particles off under sign-off | 0.2 | 0.36 | weeks at bar E; one lane for the cut | high for the cut: particle setup draws from the shared RNG, effect parts deal damage | not by hand; the tier covers it |
| 13 | Finish native collision (slices S2–S7 of [sim-rewrite-collision.md](../engine/sim-rewrite-collision.md)) | 0.08–0.10 | 0.16 | six lanes | medium | not for this goal: a phantom roster has no camera asking |
| 14 | Havok by hand at bar E ([sim-rewrite-havok.md](../engine/sim-rewrite-havok.md)) | 0.55 | 0.91 | ten slices | very high | not for this goal; the tier gives its speed with no semantic risk |
| 15 | More register-level object bodies | 0 ± 0.1 | 0.14 | weeks | low | stop; keep the bodies as oracles and as the event source |
| 16 | Sound sim rewrite at bar E (phase 4) | 0.3–0.4 | 0.73 | weeks | medium | only if lever 6 is refused and the tier gate fails |
| 17 | Rust jobs inside a tick | 0 | small | — | determinism | no ([2026-11-20-sim.md](../reviews/2026-11-20-sim.md), 4.5) |

Notes on rows the ask names:

- **Runtime share.** The profiler's "Runtime 24%" is 6.8 points of bots
  brain, 8.9 of runner and scheduler, 5.4 of x87 and 2.6 of ABI
  (sim-attribution.md, "The runtime share"). On Lockout: 0.45, 0.47, 0.29
  and 0.14 ms. Levers 1, 3 and 9 take the first two; only the tier takes the x87 and
  ABI rows, because the inline binary64 path already serves every x87
  operation it can prove exact and the helper keeps the rest (2,049 of
  4,577 x87 samples are `h2x87_v1_unary`: transcendentals and unproven
  square roots).
- **Light profile.** `h2 sim run` already runs the light profile
  ([sim.md §10](../runtime/sim.md#10-expected-speed-up)):
  the packet clock and the world-frame skip are on. The one profile switch a sim leaves
  off is `host-light-native` (levers 4, 13 and 15). The tick pacer, mapped
  reads and sim stores do not change unpaced ms/tick.
- **Output-only work.** A sim has no viewer and no listener. Under the
  product contract a switch may drop work whose only consumer is a screen
  or a speaker. Two things stop a blanket cut: the tick reads some of that
  state back (voice completion, the view scale particles use, visit marks
  the camera's collision leaves), and particle setup draws from the shared
  RNG. So each cut has its own bar: equal `gameplay` digests against the
  uncut arm over the seed set of M5, or the user's sign-off on a match
  that is repeatable but different.
- **Phases 4 and 5.** Sound and Havok are 0.73 and 0.91 ms. By hand at
  bar E they return about 0.35 and 0.55 ms for many weeks of lanes. Lever
  6 takes most of sound in one lane; the tier takes Havok's.

## Is 20× reachable

Not by rewriting subsystems. Hand rewrites of animation, collision,
objects, sound, Havok and effects at *k* = 3 take the Waterworks model
from 14.12 to 7.65 ms (sim-attribution.md, cumulative table): 46% for six
phases. The same fraction of the 3.9 ms that is left after the cheap
levers would give 2.1 ms, but the model's saving includes the memory and
transfer tax those levers have already taken, so the honest figure is 2.6
to 2.9 ms, 11 to 13×, after all six phases. 20× needs the whole guest
tick, not six families of it, to run about three times faster.

It is reachable only if nearly all guest code in the tick stops running
under the reference contract. The sim tier does that without rewriting
any of it: it recompiles the same lifted IR
([sim-tier-experiment.md](../engine/sim-tier-experiment.md)) and falls
back to the reference image when it cannot vouch for a result. Its gate is
5× on five leaf roots, and the gate has no measured number.

The ladder on Lockout, each step with its honest ceiling:

| Step | Levers | Expected ms | Speed | Best case | If it underdelivers |
|---|---|---:|---:|---:|---|
| 0 | baseline | 5.50 | 6.1× | | |
| 1 | 1 (batched), 2, 3, 4, 5 | 4.5 | 7.3× | 4.0 (8.3×) | 5.0: flat RAM or transfers already in the baseline |
| 2 | 6 (sound off), 1 pipelined | 3.9 | 8.5× | 3.4 (9.8×) | 4.5: sound stays, rewrite or tier takes it later |
| 3 | 7, stage 1: leaf closures, 30% of guest time at 5× | 3.1 | 10.7× | 2.8 (12×) | 3.3 at 2.5× |
| 4 | 7, stage 2: call-outs, 60% of guest time; 9, 10 | 2.3 | 14.5× | 2.0 (16.5×) | 2.7 at 2.5× |
| 5 | 7, stage 3: tick phases as roots, 85% of guest time | 1.6 | 21× | 1.4 (24×) | 2.2 (15×) at 2.5× |

- **10× is reachable** with the built levers, sound off and the first tier
  stage. Without the tier it needs sound off, every row of step 1 at its
  best case and levers 8 to 11: about 3.3 to 3.5 ms, at the edge.
- **20× is reachable only at step 5**: at least 85% of guest time on the
  tier at 5× or better, the brain off the game thread, and no sound. At a
  measured tier factor of 3 the ceiling is about 16×. Below 2 the tier is
  not worth its lanes, and the ceiling without it is about 8.5× on the
  cheap levers and 11 to 13× after the six rewrite phases.
- What is left at 1.6 ms: uncovered guest code 0.5, covered guest code
  0.6, tier boundaries 0.15, the brain's guest reads and the rest of the
  host 0.35.

## Lanes

Each lane is one branch, sized for an Opus or Sonnet lane to write in two
hours without running anything. The coordinator checks, tests and measures.
"Proof" is what must pass before the switch defaults on.

| Lane | Scope | Files | Switch | Proof | Expected ms |
|---|---|---|---|---|---:|
| **A0** lab sim job | `h2 lab sim`: the host runs `h2 sim run` on a match file sent with the request, headless, and returns the report, log and profile. An allow-list of variables: `H2_FEATURES`, `H2_FLAT_RAM`, `H2_BOTS_ALL_PHANTOM`, `H2_BOT_THREADS`, `H2_BOT_PIPELINE`, `H2_DISPATCH_STATS`, `H2_ENTRY_STATS`, `H2_PERF_FROM`, `H2_PROFILE_HZ`, `H2_PROFILE_CPU`. `--passes N` runs the arm N times and reports the median `wall.ms_per_tick` | `crates/apps/cli/src/lab/proto.rs`, `lab/host.rs`, `lab/client.rs`, [lab.md](../runtime/lab.md) | none | proto tests: a variable off the list and a match file over 64 KiB are refused; the host's argv for a request (unit test) | 0; every measurement runs through it |
| **A1** timing report | Sim report gains `wall.bots_ms_per_tick` (game-thread time in observe, think and conclude), `dispatch_per_tick`, `flat.fixups_per_tick`; match key `components = false` leaves `state_components_v1` out for timing arms | `crates/live/sim-bench/src/metrics.rs`, `crates/extensions/game/src/sim_glue.rs`, `crates/apps/cli/src/sim_cmd.rs`, [sim.md](../runtime/sim.md) | none | metrics unit tests on the JSON; `h2 sim diff` on a report without components says so | ≤ 0.05 |
| **A2** profiler host category and `h2 profile attribute` | Slices 1 and 2 of [sim-attribution.md](sim-attribution.md#slices-a-lane-can-start) | `crates/tools/perf` | none | its symbolize tests; the tool reproduces the page's totals from the 2026-11-20 profile | 0; makes M1 one command |
| **B1** brain threads (measured: batched is 9% slower, so the default is serial) | A single `h2 sim run` thinks serially unless the match or the environment says otherwise; match keys `bot_threads` and `bot_pipeline`; the environment variables win | `crates/live/bots/src/think.rs` (`Mode`), `crates/apps/cli/src/sim_cmd.rs`, `crates/live/sim-bench` match file, sim.md, switches.md | match key, default batched | unit test of the precedence; `h2 sim diff` serial against batched prints nothing (M3) | 0.2–0.3 |
| **B2** small host fixes (built) | The read is in [cache-slots.md](../runtime/cache-slots.md#the-status-poll): the poll `0x163680` asks `0x213f10` two to five times a frame, and its answer is a pure function of the path, the slot paths, the copy requests, the queued paths and the CRT locale. `engine::map_status` keys the answer on all of them plus the entry ESP; a hit returns EAX, ECX, EDX and the arithmetic flags as the original left them. `InputManager::sync` with no backend, gate or pad only counts, and `service_input` skips the empty signal round trip. `vfs::host` builds each record path once (`RecordPaths`), with no allocation per byte | `crates/rewrites/engine/src/map_status.rs`, `crates/rewrites/sim-bridge/src/sim_status_poll.rs`, `crates/platform/input/src/device.rs`, `crates/platform/xapi/src/register.rs`, `crates/runtime/vfs/src/host.rs` | none for the hook, always on light hosts (1.3% on Lockout, [note](notes/sim-10x.md#2026-11-22-the-status-poll-on-the-quiet-laptop)); none for the host fixes, which change nothing the guest sees | `map_status_differential` (the status and the poll against the lifted original, `H2_XBE`); `h2 sim diff` on against off, arena hashes on | 0.10–0.15 |
| **C1** sound off | Step 0, read with `h2 disasm`: the path the title takes when DirectSound creation fails or reports no device, and the flag the sound manager then tests (`125b40`, `1293f0`). Under the switch the sim boots down that path, so the guest runs its own no-sound code. If the title has no such path, stop and report: do not stub entries. Read done: the path is partial, so the lane stopped ([note](notes/sim-10x.md#2026-11-22-lane-c1s-read-the-titles-no-sound-path)) | `crates/platform/dsound` (the refusal), `crates/extensions/game/src/sim_glue.rs` | `impl.sim.sound-off` | unit test of the refusal; M5: equal `gameplay` digests against sound on over 20 seeds, or the first differing component from `h2 sim diff` for the user to judge | 0.5–0.65 |
| **K1** in-place callouts (built) | [spec](sim-10x-callouts.md): `ExecCtx::call_inline`, used by `sim_objects::drive_in_place` | `crates/runtime/runtime/src/direct.rs`, `crates/rewrites/sim-bridge/src/sim_objects.rs` | none (direct calls off) | the spec's runtime tests; `sim_objects_differential` unchanged; `h2 sim diff` on against off, arena hashes on | 0.10–0.25 with objects on |
| **R1** Rust tick driver | A body for `GAME_TICK` (`137fe0`) that calls the retail phases in retail order through `call_inline`, with inclusive CPU time and an optional arena hash per phase in the report ([2026-11-20-sim.md](../reviews/2026-11-20-sim.md), 4.4). Needs K1 | new `crates/rewrites/engine/src/sim_tick.rs`, `crates/rewrites/sim-bridge` installer, `sim-bench` report | `impl.sim.tick-driver` | differential: the transcript of calls from `GAME_TICK` equals the lifted function's; `h2 sim diff` | 0; gives per-phase time, and the tier its stage 3 roots |
| **E1** SSE mode sample | `H2_FP_SAMPLE` (`crates/recompiler/emit/c/h2sse_native.h`) is taken once per handler activation and again after each call or control-word write, not at the first operation of each register-only run | `crates/recompiler/emit/c/h2sse_native.h`, `crates/recompiler/emit/src/c.rs` | none (image) | `emit` SSE tests; the sse-oracle sweep; hash gate on the rebuilt image | 0.05–0.15 |
| **T1–T8** the sim tier | [spec](sim-10x-tier.md), "Lanes" | `crates/recompiler/emit/src/sim.rs`, `emit/c/h2tier.h`, new `crates/runtime/runtime/src/tier.rs`, `crates/rewrites/sim-bridge` | `H2_SIM_TIER` at build, `impl.sim.tier` at run | the spec's ladder | 0.9, then 1.7, then 2.4–3.0 |

**Needs a Fable-level spec first:** T1–T8 ([sim-10x-tier.md](sim-10x-tier.md))
and K1 ([sim-10x-callouts.md](sim-10x-callouts.md)); both are written.
C1 and B2 need a static-reading step, written into the lane. R1 needs K1.
The other lanes can start from this table.

**Order.** A0, A1, B1 and B2 at once (no shared files). The coordinator
takes M0 to M4 and M6 to M7 while they run. K1 and T1 to T3 start as soon
as M6 has a number at or above 2. C1 starts when its read is done. R1
follows K1. E1 lands with the tier's first image build, so the image is rebuilt
once.

## Measurements

All timing on the quiet laptop. `h2 lab` has no sim job
(`crates/apps/cli/src/lab/proto.rs` builds `h2 play` jobs only); until
lane A0 lands, the arms below run in a shell on the laptop against the
image a lab build of the branch left there (`h2 lab build --branch <b>
--build-env H2_OPT=2`, [lab.md](../runtime/lab.md#builds-go-by-branch)).
With A0 each `arm` is `h2 lab sim --name <label> --host laptop --branch
<b> --match lockout.json --passes 3 --env ...`. Results go in
[notes/sim-10x.md](notes/sim-10x.md) with the commit.

Setup, once per shell. One optimised CLI built with `--features perf`,
one clang image per memory model, `H2_OPT=2` at build and run.

```bash
set -euo pipefail
out=$(mktemp -d "${RUSH_KEEP:?}/sim-10x.XXXXXX")
unset H2_PROFILE H2_TIMELINE H2_HOST_AUDIT H2_LOCKSTEP H2_HOST_MODULE H2_SERVER_MAP_DIR H2_FEATURES
for map in lockout ascension waterworks; do
    jq -n --arg map "$map" '{
        map: $map, game_type: "slayer", seed: 7, team_play: true, time_limit_s: 0,
        score_limit: 250, max_ticks: 9000, max_wall_s: 900, render_every: 0, pace: "unpaced",
        bot: [range(4) as $i | {name: (["Red 1","Blue 1","Red 2","Blue 2"][$i]),
            skill: "legendary", team: ($i % 2)}]
    }' > "$out/$map.json"
done

arm() (   # arm <map> <label> [VAR=value ...]
    map=$1; label=$2; shift 2
    scratch=$(mktemp -d "${TMPDIR:?}/h2-10x.XXXXXX"); trap 'rm -rf -- "$scratch"' EXIT
    ln -s "${H2_DATA:?}/bots" "$scratch/bots"
    env H2_OPT=2 H2_BOTS_ALL_PHANTOM=1 "$@" \
        h2 sim run "$out/$map.json" --disc "${H2_DISC:?}" --out "${H2_IMAGE:?}" --tier clang \
        --data "$scratch" --pace unpaced --report "$out/$map-$label.json" > "$out/$map-$label.log" 2>&1
    jq -e '.result.end == "tick_limit" and .ticks == 9000 and .nav == "loaded"' \
        "$out/$map-$label.json" > /dev/null
)
timed() {  # timed <map> <label> [VAR=value ...]: three passes, the median
    map=$1; label=$2; shift 2
    for p in 1 2 3; do arm "$map" "$label-t$p" "$@"; done
    jq -s --arg l "$label" '{arm: $l, median_ms_per_tick: (map(.wall.ms_per_tick) | sort | .[1])}' \
        "$out/$map-$label"-t{1,2,3}.json
}
hash=impl.sim.arena-hash
```

**M0. Baseline and repeatability.** Nothing else means anything until the
A/A diff is empty.

```bash
arm lockout aa1 H2_FEATURES=$hash; arm lockout aa2 H2_FEATURES=$hash
h2 sim diff "$out/lockout-aa1.json" "$out/lockout-aa2.json"      # must print nothing
for map in lockout ascension waterworks; do timed $map base; done
```

**M1. The phantom census** (replaces the budget table). Confirms or kills
lever 8 and sizes every other row.

```bash
arm lockout dispatch H2_DISPATCH_STATS=1
arm lockout entries  H2_ENTRY_STATS=1          # a `thunk` count means the image has T3
arm lockout profile  H2_PERF_FROM=level H2_PROFILE_HZ=500 H2_PROFILE_CPU=1 H2_PROFILE="$out/prof-lockout"
h2 analyze --functions "$out/functions.txt"
python3 -I scripts/sim-attribution/attribute.py --xbe "${H2_XBE:?}" \
    --functions "$out/functions.txt" --profile "$out/prof-lockout" --work "$out/work" \
    --objdump "$(brew --prefix llvm@22)/bin/llvm-objdump" \
    --ms "$(jq -s 'map(.wall.ms_per_tick) | sort | .[1]' "$out"/lockout-base-t{1,2,3}.json)" \
    --log "$out/lockout-profile.log" > "$out/attribution-lockout.txt"
```

Read: the camera, first person, UI and player rows (lever 8 is live if
they sum above 3% of the thread); the bots brain share (lever 1's
ceiling); runner and scheduler share (lever 3: under 3% means the
transfer slices are already in the baseline); sound and effects rows.

**M2. Native subsystems.** Needs `f50dc1ba6` (the install fix).

```bash
for s in anim collision objects; do
    arm lockout native-$s-hash H2_FEATURES=impl.sim.native.$s,$hash
    h2 sim diff "$out/lockout-aa1.json" "$out/lockout-native-$s-hash.json"
    timed lockout native-$s H2_FEATURES=impl.sim.native.$s
done
arm lockout native-objects-entries H2_FEATURES=impl.sim.native.objects H2_ENTRY_STATS=1
grep -i 'sim-native\|sim.native' "$out/lockout-native-objects-hash.log" | head   # the plan line: installed, not refused
```

Read: animation is confirmed at 0.08 ms or more and joins the sim
default. Objects is killed for sims (left off) unless it saves 0.1 ms;
runner entries per tick with objects on, against M1's, is the callout
count that sizes lane K1. If entries rise by more than 500 a tick, K1 is
worth its lane; under 100, it is not.

**M3. Brain threads.**

```bash
arm lockout serial-hash  H2_BOT_THREADS=0 H2_FEATURES=$hash
arm lockout batched-hash H2_BOT_THREADS=4 H2_FEATURES=$hash
h2 sim diff "$out/lockout-serial-hash.json" "$out/lockout-batched-hash.json"   # must print nothing
timed lockout serial H2_BOT_THREADS=0; timed lockout batched H2_BOT_THREADS=4
arm lockout piped1 H2_BOT_THREADS=4 H2_BOT_PIPELINE=1 H2_FEATURES=$hash
arm lockout piped2 H2_BOT_THREADS=2 H2_BOT_PIPELINE=1 H2_FEATURES=$hash
h2 sim diff "$out/lockout-piped1.json" "$out/lockout-piped2.json"              # must print nothing
timed lockout piped H2_BOT_THREADS=4 H2_BOT_PIPELINE=1
```

Kill: batched saves under 0.1 ms (the brain is mostly guest reads, which
stay on the game thread; then the bots lane cuts the reads instead).

**M4. Flat RAM.** A second image directory, built flat (contract doc, M4).

```bash
H2_FLAT_RAM=1 H2_OPT=2 H2_OBJ_CACHE=~/Library/Caches/h2recomp/obj h2 build "$H2_DISC" --out "$H2_IMAGE-flat"
( H2_IMAGE="$H2_IMAGE-flat"
  arm lockout flat-hash H2_FLAT_RAM=1 H2_FEATURES=$hash
  h2 sim diff "$out/lockout-aa1.json" "$out/lockout-flat-hash.json"            # must print nothing
  timed lockout flat H2_FLAT_RAM=1 )
```

Read `fixups` from `h2 agent profile.memory` on a paced run of the same
match, or from the report once A1 lands: it must be zero per tick. Kill:
flat is slower than paged, or the diff is not empty; then the finding
goes to the contract lane and flat stays off for sims.

**M5. Sound off** (after lane C1). Twenty seeds, `gameplay` digests.

```bash
for seed in $(seq 1 20); do
    jq --argjson s $seed '.seed = $s | .max_ticks = 3600' "$out/lockout.json" > "$out/lockout-s$seed.json"
    for a in on off; do
        f=''; [[ $a == off ]] && f=impl.sim.sound-off
        ( scratch=$(mktemp -d "${TMPDIR:?}/h2-10x.XXXXXX"); trap 'rm -rf -- "$scratch"' EXIT
          ln -s "$H2_DATA/bots" "$scratch/bots"
          H2_OPT=2 H2_BOTS_ALL_PHANTOM=1 H2_FEATURES="$f" h2 sim run "$out/lockout-s$seed.json" \
              --disc "$H2_DISC" --out "$H2_IMAGE" --tier clang --data "$scratch" --pace unpaced \
              --report "$out/snd-$a-s$seed.json" > "$out/snd-$a-s$seed.log" 2>&1 )
    done
    diff -q <(jq -c '.state_hashes_v1[] | [.tick, .gameplay]' "$out/snd-on-s$seed.json") \
            <(jq -c '.state_hashes_v1[] | [.tick, .gameplay]' "$out/snd-off-s$seed.json") \
        || h2 sim diff "$out/snd-on-s$seed.json" "$out/snd-off-s$seed.json"
done
timed lockout sound-off H2_FEATURES=impl.sim.sound-off
```

Confirmed outright if every digest series is equal. If they differ, the
first differing component and tick go to the user with the per-seat
kills, deaths and accuracy of both arms; the lever then needs sign-off.
Kill: under 0.3 ms saved.

**M6. The tier's gate** (decides lever 7). The existing bench, on the
laptop's checkout:

```bash
H2_XBE=/path/to/default.xbe cargo test -p engine --release --features differential \
  --test it sim_tier_differential:: -- --ignored --nocapture tier_bench
```

Fill the results table of
[sim-tier-experiment.md](../engine/sim-tier-experiment.md) from a dated
note. Go at a mean `reference - empty` over `tier` of 3 or more across the
five roots; at 2 to 3 build stage 1 only and re-measure in a match; under
2, kill the lever and record it in [notes/not-adopted.md](notes/not-adopted.md).

**M7. The tier's coverage.**

```bash
H2_XBE=/path/to/default.xbe cargo test -p emit --release --test sim_tier_writes -- --ignored --nocapture
```

Read from `target/tmp/sim-tier-writes.md`: the share of the tick closure
the tier compiles and the refusal reasons by count. Join the refused and
accepted function lists with M1's profile to get coverage weighted by
samples. Under 50% weighted, the top refusal reasons become lane T6
before any stage ships.

**M8. Inclusive phase times.** `arm lockout audit H2_HOST_AUDIT="$out/audit.json"`
([sim.md](../runtime/sim.md#host-audit-corpus)); after lane R1, the
report's per-phase block replaces it.

**M9. PGO.** The gen, train and use sequence of
[BUILD-CACHE.md](../lifter/BUILD-CACHE.md), trained on `arm lockout` and
`arm waterworks` runs; then `timed lockout pgo` on the `use` image. Kill
under 3%.

Every later lane is measured the same way: an arena-hash arm diffed
against M0's `aa1`, then `timed`, on Lockout and Waterworks.

## Rules and limits

- **Lockout is the yardstick, Waterworks the check.** A lever that helps
  Lockout and not Waterworks is reported as such; nothing ships on one map.
- **No silent fallback.** A lever that cannot engage (no flat window for
  the tier, a refused root, a title with no no-sound path) is a named line
  in the log and a field in the report.
- **One home for switches.** A lane adds its switch to switches.md in its
  own branch. This page names proposals only.
- **Numbers.** Every measured figure goes to the note; this page keeps the
  model until M1 replaces it, then links the note.

## Evidence

| Fact | Label | Source |
|---|---|---|
| 5.5 ms on Lockout, 6.2 ms on Ascension, four phantoms | Run | [notes/sim-10x.md](notes/sim-10x.md) |
| The family and kind rows | S | the note's arithmetic over [sim-attribution.md](sim-attribution.md); M1 |
| The native subsystems never installed before the point-shape fix; on equals off in speed under load | Run | the note |
| Object bodies keep retail frames, registers and flags; a callout returns a `CALL` transfer | Code | `crates/rewrites/engine/src/sim_objects/mod.rs` (`drive`, `Next`), `object.rs` |
| A thunk-run override continues inline only on a plain return | Code | `crates/runtime/runtime/src/direct.rs` module doc |
| T1–T3, F3–F5 built and unmeasured | Code | [contract](../runtime/contract-thin-transfers-flat-ram.md), first paragraph |
| The sim tier compiles five roots in a test dylib and is not in the image | Code | `crates/recompiler/emit/src/sim.rs`, `crates/rewrites/engine/tests/sim_tier_differential.rs` |
| `h2 lab` runs `h2 play` jobs only | Code | `crates/apps/cli/src/lab/proto.rs` (`RUN_ENV`, `check_play_args`) |
| A slice is 65,536 guest accesses | Code | `runtime::preempt::DEFAULT_SLICE` |
| Every expected and ceiling figure | S | its measurement above |

## Not verified

- The Lockout phantom mix: every row of the budget.
- Whether the 5.5 ms image carries the transfer slices.
- Which seat work a phantom host still runs.
- Whether sound state reaches the `gameplay` digest. (The title's
  no-sound path is partial: [note](notes/sim-10x.md#2026-11-22-lane-c1s-read-the-titles-no-sound-path).)
- The tier's factor on any root, its coverage of the tick, and how often
  a root aborts on a slice end.
- Whether the object bodies raise or lower runner entries per tick.
- That batched thinking pays on four brains, where the guest reads stay on
  the game thread.
- Whether flat RAM is faster than paged on this workload.

## Related

- [sim-10x-tier.md](sim-10x-tier.md) and [sim-10x-callouts.md](sim-10x-callouts.md): the two specs.
- [sim-headless-skips.md](sim-headless-skips.md): work a headless sim does for no consumer, classed for state, films and the viewer.
- [notes/sim-10x.md](notes/sim-10x.md): the measurements and the budget arithmetic.
- [sim-attribution.md](sim-attribution.md), [sim-budget.md](sim-budget.md): the profile this scales from.
- [sim-rewrite.md](../engine/sim-rewrite.md): the phases, the ownership rule and the exactness bars.
- [contract-thin-transfers-flat-ram.md](../runtime/contract-thin-transfers-flat-ram.md): levers 2 and 3.
- [2026-11-20-sim.md](../reviews/2026-11-20-sim.md): the review that proposed the tier and the tick driver.
- [host-halving.md](../live/host-halving.md): the same levers priced for a paced host.
- [sim-light.md](sim-light.md): memory, disk and setup time per match in a batch.
