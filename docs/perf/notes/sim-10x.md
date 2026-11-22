# Sim per-match speed: measurements and the budget model

Evidence for [../sim-10x.md](../sim-10x.md). Newest first.

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
