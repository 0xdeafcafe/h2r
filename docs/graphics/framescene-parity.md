# FrameScene parity: corpus, pixel bar and the sim hash gate

Status: Partly built. Section 9 of [framescene.md](framescene.md): how a slice proves the native frame equals the D3D frame and the tick is unchanged. `render.compare`, `render.capture`, `render.scene` and `sim.hashes` are built; `render.parity` is built for the carried path.
Code: `crates/extensions/agent/src/{render,compare}.rs`, `crates/extensions/agent/src/sim/hashes.rs`, `crates/extensions/agent-proto/src/render.rs`.

| Gate | Tool | Bar |
|---|---|---|
| Pixels, two launches | `render.compare` over `H2_BOOT_GFX` PNGs | `max_channel <= 8`, `differing` at most 0.01% |
| Pixels, one launch | `render.parity {arm}` | the same bar, on armed frames |
| At `alpha = 1` | `render.compare` | `max_channel == 0` |
| The sim | `sim.hashes` of both launches | identical series |

## Corpus

- **Gate corpus** (every slice): Old Mombasa (`03a_oldmombasa`) from boot, cutscene and gameplay, presents 1 to 5000; one multiplayer map with four phantoms (Lockout or Waterworks, the sim benchmarks); the main menu with a blurred overlay open; a two-view split-screen start. At render scale 1 and 4.
- **Census corpus** (before an entry is replaced, and before slice 6): every campaign level and every multiplayer map on the disc, 3,000 presents each from boot, counters only.

## Pixels

Two launches of the same image from boot, one with `impl.render.native=off`, write PNGs at the same presents (`H2_BOOT_GFX`; the all-native push got identical frames through present 4500). The agent API has these methods ([agent-api.md](../runtime/agent-api.md)):

| Method | Params and result |
|---|---|
| `render.compare {a, b, diff?}` | two PNG paths in; `{size, differing, max_channel, mean, first: [x, y]}` out, and a difference image at `diff` if asked. `h2 agent frame compare A B` runs it locally with no socket |
| `render.capture {tick?, pass?}` | the frame of sim tick `tick` (default: the next one) as a PNG, with its tick, present number and the scene's summary hash. It waits until nothing is unresident. On the D3D path the frame of a tick is the present that follows it. Captures are tick-addressed, which present numbers stop being once the render thread free-runs |
| `render.scene {dump?}` | the last sealed scene's counts per view (clusters, instances, parts, rows, lights, shadow maps, carried ops per slot, by `CameraUse`), drops, unresident and held counts; `dump` writes it whole as JSON under the cache directory |
| `render.frame` | D3D passes; from slice 6 the native pass graph in the same shape (`passes`, `totals`, `snap` by pass number), so `h2 agent frame` keeps working |

The methods exist because every slice and every later render lane needs the same check.

**Bar.** Per compared frame: `max_channel <= 8` and `differing` (any channel off by more than 2) at most 0.01% of the pixels. The all-native push measured 0 on nine of ten frames and 6 on one. A frame over the bar fails the slice until explained; an accepted difference is recorded with its difference image in the slice's report, never by loosening the bar.

**In-process reference** (slice 4 to slice 8). `render.parity {arm}` makes the next frame step run the guest's whole front end into the D3D backend's own targets, and also extracts and draws the same tick natively into private targets. It compares the two on the render thread and reports as `render.compare` does.

- The guest front end is a superset of the replaced entries' tick-visible writes, so the sim cannot tell (the hash gate has already shown that).
- It is armed for one frame from the agent API and costs one relaxed load a frame otherwise.
- It is the one place two paths draw the same tick. It is a diagnostic and is deleted with the D3D backend.

**Interpolated frames.** At `alpha = 1` the frame must equal the 30 fps frame of that tick exactly (`max_channel == 0`). In-between frames are checked by counters and by eye, as in [high-framerate.md](../game/high-framerate.md).

## The sim

- `h2 lockstep A B` on `H2_LOCKSTEP` traces of the two launches is the whole-RAM check. It differs where only the render writes (visit marks, class a), so it localises but does not gate.
- **`sim.hashes {from_tick?}`** returns the `state_hashes_v1` rows (gameplay digest, and the arena hash under `impl.sim.arena-hash`) of the running game. Sim mode writes this series to its report but refuses a GPU, so a rendered run has no other way to produce it. The gate compares the series of the two launches row by row and names the first differing tick.
- Slice 0 runs this on master first. If `native=on` and `=off` differ, that is a bug in the tenant path and is fixed before slice 1.
- The super-bounce regression test ([high-framerate.md](../game/high-framerate.md), section 4) runs at 30, 60 and 120 once slice 8 lands.

How to run each check: [framescene-measure.md](framescene-measure.md).
