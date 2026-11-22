# Sim per-match speed: measurements and the budget model

Evidence for [../sim-10x.md](../sim-10x.md). Newest first.

## 2026-11-22: the phantom alias against the view on the laptop

Lab job 0065, commit b925c10dd (integ-sim7), `H2_OPT=2`, Lockout 2v2
(`docs/runtime/sim-examples/lockout-2v2.toml`), arms
`alias=impl.sim.phantom-alias` and `view=-impl.sim.phantom-alias`, three
repeats each. Reproduce: `h2 lab sim --host laptop --branch <b> --repeat 3
--arm alias=impl.sim.phantom-alias --arm view=-impl.sim.phantom-alias
--build-env H2_OPT=2 docs/runtime/sim-examples/lockout-2v2.toml`.

| Run | Speed | ms/tick | Wall |
|---|---|---|---|
| alias-r1 | 10.90× | 3.06 | 37.1 s |
| view-r1 | 8.94× | 3.73 | 38.3 s |
| view-r2 | 1.78× | 18.73 | 173.2 s (laptop under memory pressure) |

The host stopped the job during alias-r2 for machine memory pressure, so
one clean pair stands: the alias is 22% faster per match than the view
(3.06 against 3.73 ms a tick). `state_hashes_v1` are identical across all
three finished runs and both arms. A repeat on a quiet laptop is still
needed for a median.

## 2026-11-22: eight at once with the warm Z:, and the phantom build's cost

Lab job 0060 on the laptop, commit `e64535b4d` (warm Z: template and lane
H0's report fields), `H2_OPT=2`: the same eight Lockout matches as jobs
0055 to 0057, all at once. Job 0058 (four at once on the warm-Z: branch)
ran while the game was open on the laptop, so its speeds are not used;
its state hashes equal job 0056's for all eight seeds (`h2 sim diff`:
"9000 ticks, every component agrees").

- **Eight at once now fit.** One run makes the Z: template and seven clone
  it. The job takes 139 s for 2,400 game seconds: a batch speed of 17.3×,
  against 13.7× at four and 7.5× at one.
- **Setup does not shrink:** 11.7 to 14.5 s a run. The lobby still waits
  for the N: copies (`docs/runtime/cache-slots.md`, "Sim batches: a warm
  Z:").
- **The phantoms' action build is the largest single cost found so far.**
  Per tick:

| run | ms/tick | build | of which snapshots | brain |
|---|---|---|---|---|
| lockout-s1 | 5.19 | 1.15 | 0.12 (remap) | 0.25 |
| the other seven (median) | 7.0 | 1.5 | 0.17 (remap) | 0.45 |

  The build is 22% of the tick; the copy-on-write snapshots are only a
  tenth of it, about one per tick, all remapped. The rest is the guest
  calls, reverts and view writes in `Builder::sample`
  (`crates/extensions/game/src/bot_glue/retail_input.rs`).
- **The census confirms the title already skips the presentation work:**
  local users 0 and the local visibility set empty on every tick; decals
  and particles 0; effects mean 8 (peak 11); lights mean 30 (peak 37);
  particle systems mean 0.03.

Reproduce: the batch of the 2026-11-22 throughput entry with `--parallel 8`
on a commit with lane H0.

## 2026-11-22: batch throughput at --parallel 1, 4 and 8 on the quiet laptop

Lab jobs 0055 to 0057, commit `1c331062f`, `H2_OPT=2`: one batch of eight
Lockout 2v2 matches (seeds 1 to 8, 9,000 ticks each, 2,400 game seconds in
all) at each setting. "Batch speed" is game seconds over the job's wall
time, setup and lab overhead included.

| --parallel | per-match speed (median, min to max) | ms/tick | job wall | batch speed |
|---|---|---|---|---|
| 1 | 10.70× (9.91 to 11.09) | 3.12 | 321 s | 7.5× |
| 4 | 6.55× (6.32 to 6.79) | 5.09 | 175 s | 13.7× |
| 8 | failed | – | 198 s | – |

- Four at once slow each match by 39%, so four matches buy 1.8× the
  throughput of one.
- At eight, every match failed before its first tick: the laptop's disk
  filled. Each run's data directory gets its own copy of the map cache
  partitions (`data/disk/Partition4`, `Partition5`, `cache00N.map`), which
  the title writes while the System Link lobby waits for every peer's map
  status to reach 3 ("vfs: write-behind write ... No space left on
  device"). That copy is also part of each match's 9 to 12 s setup.

Reproduce: a batch file with `match = "lockout-2v2.toml"`, `maps =
["lockout"]`, `seeds = [1, ..., 8]`, then `h2 lab sim --host laptop --branch
master --wait --parallel N --build-env H2_OPT=2 <batch>`.

## 2026-11-22: the status poll on the quiet laptop

Lab job 0054, the same setup as job 0053 below (commit `1c331062f`,
`H2_OPT=2`, Lockout 2v2 seed 7, three interleaved runs per arm, nav
loaded).

| arm | speedup (median, min to max) | ms/tick |
|---|---|---|
| `impl.sim.status-poll` on | 10.43× (10.21 to 10.50) | 3.20 |
| off | 10.30× (10.15 to 10.43) | 3.24 |

State hashes are identical across all six runs. The gain is 1.3%, about
0.04 ms a tick against the 0.10 to 0.15 ms the plan expected, and the
ranges overlap. It is exact, so the switch defaults on.

Reproduce: the job 0053 command with `--arm poll=impl.sim.status-poll
--arm base=-impl.sim.status-poll`.

## 2026-11-22: native on against off on the quiet laptop, bots playing

Lab job 0053 on the laptop, commit `1c331062f`, `H2_OPT=2`, Lockout 2v2
(`docs/runtime/sim-examples/lockout-2v2.toml`, seed 7, 9,000 ticks), three
runs per arm, interleaved. Every run has `nav = "loaded"` and real kills
(Red 1 7, Blue 1 4, Red 2 1, Blue 2 0 in each). Earlier laptop jobs (0050)
had no nav mesh, so their bots stood still and their ~20× was not a bot
match.

| arm | speedup (median, min to max) | ms/tick | bots ms/tick |
|---|---|---|---|
| `impl.sim.native` on | 9.95× (9.92 to 10.01) | 3.35 | 0.24 |
| off | 10.23× (10.21 to 10.27) | 3.26 | 0.24 |

- State hashes are identical across all six runs and both arms.
- The native subsystems cost 2.7% per tick as wired, so they stay off.
- The bots take 0.24 of 3.3 ms a tick (7%); the guest's tick is the rest.
- Setup is 9 to 12 s of a 38 to 42 s process, so a batch's process speed
  is about 7 to 8×.

Reproduce: `h2 lab sim --host laptop --branch master --wait --repeat 3
--arm on=impl.sim.native --arm off=-impl.sim.native --build-env H2_OPT=2
docs/runtime/sim-examples/lockout-2v2.toml`, with an `h2` that uploads the
nav mesh (it refuses a map without one).

## 2026-11-22: lane C1's read, the title's no-sound path

Commit: `4386d06aa`. Method: static reading of retail `default.xbe`
(5849) with `h2 disasm`, plus a call graph from a linear sweep of `.text`
(`h2 disasm 0x12000 1300000`). Reproduce with the addresses below.

**Where creation failure goes.** The sound core's init `0x21e4b0` calls
`DirectSoundCreate` at `0x21e516` and branches on a negative HRESULT
(`jl 0x21eacc`, `0x21e51d`). That exit tears down through `0x21eae0`, which
null-checks the interface at `[[0x51ebe4] + 0x2ab0]` before releasing it,
and returns 0. The real library writes `*ppDS` only on success (the store
at `0x37d7c5` is skipped by `jl 0x37d7c7`), so the slot stays the zero the
init cleared. The sound manager's init `0x1253d0` (lifecycle slot 44 at
`0x440dd8 + 44 * 0x24`) then skips to `0x1255e9`: the manager's
initialized byte `[[0x4e6380] + 0x78]` stays 0 and its record count
`+0x208` stays 0.

**What the manager tests afterwards.** The byte at `+0x78` gates the
service `0x125b40` (`0x125b4f`, skipping to `0x125d17`), the active test
`0x125a60` (which gates the periodic path `0x125d60`), and the entries at
`0x1257b0`, `0x125920`, `0x125f70`, `0x126400`, `0x1264c0` and `0x126c30`.
`0x1293f0` has no test of its own; its record loop runs zero times when
`+0x208` is 0, and it is reached only from gated callers.

**What does not test it.** Two paths call DirectSound with the handles a
failed creation never filled:

- **Every map load.** Lifecycle slot 44 is `{0x1253d0, 0x125600,
  0x125690, 0x1257b0, 0x125920, 0x175f40}`. The third pointer, the
  per-map init `0x125690`, has no gate and calls `0x191270` at
  `0x12576e`, which passes `[[0x51ebe4] + 0x2ab0]` (zero) to
  `IDirectSound_SetEffectData` (`0x37b60d`) three times. The library's
  wrapper keeps a null `this` null and `CDirectSound::SetEffectData`
  (`0x37a2ed`) reads `[this + 0xc]` at `0x37a31b`. Its only early exit is
  the library's fatal flag `[0x386b0c]`, set solely at `0x37ef53`, not by a
  failed create. On the console this is a null read, a fault.
- **Pause and unpause.** `0x125a90` (from `0x1482e2`, `0x18fa48` and
  `0x232087`: the pause screen, unpause and the controller check
  `0x18f9be`) tests only `+0x7b`. Pausing calls `0x21f570`, unpausing
  `0x21f5a0`, and both call `IDirectSoundBuffer_Pause` (`0x37b73f`) on
  `[[0x51ebe4] + 0x2a90]` and `+0x2a94`, buffers made only on the success
  path.

**Verdict.** The title has a partial no-sound path: the service and the
play entries are gated, but its own per-map init dereferences the
DirectSound interface it never created. A sim booted with creation refused
would stop at the first map load, in the HLE as a `BadHandle` stop on
`IDirectSound_SetEffectData`, as a fault on the console. Making it run
would need the HLE to answer calls on a null interface, behaviour the
retail library does not have, or a stub of the guest's per-map init. Lane
C1 stops here; `impl.sim.sound-off` is not built.

The other three `DirectSoundCreate` callers (`0x33334f`, `0x3338b0`,
`0x334167`) are the XHV voice engine, not the sound manager.

## 2026-11-22: the Lockout budget model (arithmetic, not a measurement)

Commit: none; computed from the tables of
[../sim-attribution.md](../sim-attribution.md) (Waterworks, four local
bots, 14.12 ms/tick, 84,358 samples). Method: static arithmetic. No Lockout
profile and no phantom-roster profile exists; census M1 of the design page
replaces every row below.

**Step 1: the Waterworks rows by code family** ("Primary all-in", share of
the thread): objects and types 26.76 (object core 9.27, units 6.66, bipeds
2.74, vehicles 2.85, weapons 1.46, lights 1.33, model nodes 1.68, damage
and small types 0.77); animation 9.37 (8.40 plus first-person weapon 0.97);
collision 4.11; Havok 15.23 (library 10.05, glue 5.18); effects 5.46; sound
8.37; players 4.55; camera 3.29; game engine 2.00; cache 2.34; main loop
and input 1.65; UI 1.75; networking 1.46; AI, scripts, render, unknown
1.30. Guest frames and runner 87.64. Host only 12.36 (bots brain 6.81,
audio clock and DirectSound HLE 2.62, kernel 1.52, XAPI 0.75, hooks 0.67).

**Step 2: remove what four local seats ask for** (the attribution's "Who
asks for the shared work"): camera 3.29; UI 1.75; half of players 2.28
(assumed: the player update and the action builder stay for a phantom, aim
assist and local control go); input 0.56 of the main loop; collision asked
by camera and players, 43% of 4.11 = 1.77; animation asked by first
person, 20% of 8.40 = 1.68, and the first-person weapon row 0.97; Havok ray
casts asked by camera and players, 91% of 1.80 = 1.64; cache asked by the
camera, 30% of 2.34 = 0.70. Total removed 14.64 points; 85.36 remain.

**Step 3: remove vehicles** (Lockout has none): 2.85 points of object
code. Vehicle physics inside the Havok rows is not separable and stays.
82.52 remain.

**Step 4: scale to 5.50 ms.** One point of the original thread is
5.50 / 82.52 = 0.0667 ms.

| Family | Points | Share of the Lockout tick | ms |
|---|---:|---:|---:|
| Objects and types | 23.91 | 29.0% | 1.59 |
| Havok library and glue | 13.59 | 16.5% | 0.91 |
| Sound sim (guest) | 8.37 | 10.1% | 0.56 |
| Bots brain (Rust) | 6.81 | 8.3% | 0.45 |
| Animation | 6.72 | 8.1% | 0.45 |
| Effects, particles, decals | 5.46 | 6.6% | 0.36 |
| Players and game engine | 4.27 | 5.2% | 0.28 |
| Cache, main loop, AI, scripts, render, unknown | 4.03 | 4.9% | 0.27 |
| Kernel, XAPI, feature hooks | 2.94 | 3.6% | 0.20 |
| Audio clock and DirectSound HLE | 2.62 | 3.2% | 0.17 |
| Collision | 2.34 | 2.8% | 0.16 |
| Networking | 1.46 | 1.8% | 0.10 |
| **Total** | **82.52** | **100%** | **5.50** |

**Step 5: the same tick by kind.** Host-only work is 12.37 of 82.52 =
0.82 ms. The other 4.68 ms is split in the Waterworks ratio own 55.65 :
tax 23.14 : runner 8.85.

| Kind | ms |
|---|---:|
| Guest own code | 2.97 |
| Memory gate and slow path (mem) | 0.375 |
| Direct-call checks (direct) | 0.163 |
| SSE helpers (sse) | 0.269 |
| x87 helpers (x87) | 0.290 |
| Exit, charge, acquire, budget (abi) | 0.139 |
| Runner and scheduler | 0.47 |
| Host only | 0.82 |
| **Total** | **5.50** |

**What the model cannot know.**

- The Lockout mix. Lockout has fewer objects, lights and BSP leaves than
  Waterworks, so the object, light and collision rows are probably
  smaller and Havok, sound and the brain larger.
- Which of the seat rows a phantom keeps. Step 2 assumes half of the
  player row.
- Whether the 5.5 ms image had thin transfers T2 and T3
  ([contract](../../runtime/contract-thin-transfers-flat-ram.md)). If it
  did, the runner, direct and abi rows are already smaller than shown and
  own code is larger.
- The check: Lockout with four local bots measured 8.49 ms on 2026-11-20
  and four phantoms about 5.5 ms on 2026-11-21, a drop of 35%. Step 2
  explains 15 to 18 points of that. The rest is unattributed: bot
  navigation changes, the transfer slices, the light-profile skips and a
  different machine state all sit between the two numbers.

Treat each row as ±30%.

## 2026-11-21 and 2026-11-22: phantom-roster speeds and the first sim-native run

Reported by the coordinator; reports not kept in the repository. Commit:
timing runs not recorded; the install fix is `f50dc1ba6` on branch
`sim-seed-early`. Machine: M-series Mac. Reproduce: census M0 and M2 of
the design page.

- **Quiet machine, 2v2, four phantom bots, unpaced, light profile:**
  Lockout about 6× real time (about 5.5 ms of host work per 33.3 ms tick);
  Ascension about 5.4× (about 6.2 ms).
- **`sim-native` never installed before `f50dc1ba6`.** The point-shape
  check (`crates/rewrites/engine/src/point_shapes.rs`) refused
  `WEAPON_UPDATE` (`0xfdfb0`, which follows a jump table), so the whole
  feature failed to install in every earlier run. No native subsystem
  (`impl.sim.native.anim`, `.collision`, `.objects`) has a timing from a
  real match before this date, and every earlier "on" arm measured the
  lifted code.
- **First on/off pair, loaded machine:** Lockout seed 3, 9,000 ticks,
  `impl.sim.native` on against off: bit-identical state hashes at every
  tick, 3.20× against 3.22×. The machine was busy, so the speeds are a
  hint only: the native bodies as wired save nothing measurable.
- **Same-seed repeatability** was broken by spawn words in the game
  options (fixed by `1df5198d5`), and the bots' map data loaded on a
  worker thread (fixed on the same branch, `sim-seed-early`).
