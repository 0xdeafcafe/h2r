# Switches

What is left after the 2026-11-20 flag cleanup: the settings, the tools
still in use, and the experiments still waiting for a measurement. Each
remaining code path has one behaviour unless a row here names the other.
The tables give the variable and what it does; the linked docs have the
details. Removed switches, and why, are in
[render-switches.md](../graphics/render-switches.md) and in each removal's
commit message.

`H2_FEATURES` takes a comma-separated list: `id`, `-id` or `id=on|off`.
An `impl.` name is an implementation switch
([`feature::switches`](../../crates/feature/src/switches.rs)).

## Settings

| Switch | What |
|---|---|
| Every setting in `crates/boot/src/setting_defs.rs` | the Settings menu's values; each has its env name there (`H2_FOV`, `H2_FPS`, `H2_LIGHTING`, `H2_RENDER_SCALE`, `H2_SINGLE_COPY`, `H2_PAD_*`, `H2_VOICE_*`, `H2_TOUCH_*`, `H2_STEAM_*`...) |
| `H2_LEVEL`, `H2_DIFFICULTY` | boot straight into a level ([level-boot.md](level-boot.md)) |
| `H2_MAP_DIR`, `H2_MAPPACK*` | editor maps and the map pack ([map-pack.md](../game/map-pack.md)) |
| `impl.render.native=off` | turns off the native GPU renderer, every stage together ([native-renderer.md](../graphics/native-renderer.md)) |
| `impl.render.reproject` | in-between frames by reprojection, the 60 fps choice ([high-framerate.md](../game/high-framerate.md)) |
| `audio.hrtf`, `audio.distance_reverb` | headphone binaural filtering; extra distance reverb |

## Build and code generation

Compiler flags: part of the dylib stamp, so set them for both build and run.

| Switch | What |
|---|---|
| `H2_OPT`, `H2_DEBUG`, `H2_CPU`, `H2_PGO`, `H2_HOT`, `H2_OPT_COLD`, `H2_PASSES`, `H2_CLIF_OPT` | optimisation level, debug info, target CPU, PGO, tiers, IR passes ([building.md](building.md)) |
| `H2_EMIT_TARGET`, `H2_XWIN`, `H2_LINUX_SYSROOT`, `H2_LLD`, `H2_LLVM`, `H2_OBJ_CACHE*` | cross targets, toolchains and the object cache |
| `H2_FASTMEM=0`, `H2_X87_NATIVE=0`, `H2_SSE_NATIVE=0`, `H2_ACCESS_COUNT=0`, `H2_DIRECT_CALLS=0` | exact reference builds for bisecting a divergence ([BOOT.md](BOOT.md)) |
| `H2_PERF`, `H2_COLD_STATS` | generated-code counters for profiling |
| `H2_CLIF_VERIFY`, `H2_TRACE` | Cranelift verifier; analysis diagnostics |

## Tools

| Switch | What |
|---|---|
| `H2_AGENT` | the agent API socket ([agent-api.md](agent-api.md)) |
| `H2_SIM` and the sim tooling (`sim.arena-hash`, `H2_DISPATCH_STATS`, `H2_ENTRY_STATS`) | headless matches ([sim.md](sim.md)) |
| `sim.static-light`, `sim.submix-fx`, `sim.observer-resources`, `sim.metrics-projection`, `audio.packet-clock` | on; `=off` is the reference the sim budget measures against ([sim-budget.md](../perf/sim-budget.md)) |
| `H2_BOOT_VERBOSE`, `H2_FEATURE_PLAN`, `H2_NATIVE_VERBOSE` | log verbosity |
| `H2_PERF_FROM`, `H2_PROFILE*`, `H2_TIMELINE*`, `H2_FRAME_STATS*`, `H2_HOST_PERF`, `H2_AUDIO_STATS`, `H2_SAMPLE_*`, `render-timing`, `render-census` | perf capture ([profiling.md](../perf/profiling.md)) |
| `H2_BOOT_GFX*`, `H2_BOOT_CAPTURE*`, `H2_BOOT_DRAWLOG*`, `H2_BOOT_UPLOAD_ALWAYS`, `H2_SWEEP_DIAG`, `H2_BOOT_STOP_AT_PRESENT`, `H2_BOOT_INPUT*` | headless rendering, captures and input |
| `H2_GFX_MERGE_CHECK`, `H2_D3D_WRITEBACK_TRACE`, `H2_RENDER_THREAD_STATS`, `H2_RENDER_THREAD=0` | render diagnostics; `H2_RENDER_THREAD=0` is the synchronous reference |
| `H2_PIPELINE_CACHE`, `H2_GFX_COMPILE_THREADS`, `H2_VERTEX_CACHE_MB`, `H2_SMOOTH_TEXT`, `H2_RENDER_OVERRIDES` | pipeline cache, compile workers, cache budget, glyph smoothing, hot render overrides |
| `H2_LOCKSTEP*`, `H2_INPUT_RECORD`, `H2_INPUT_REPLAY` | lockstep and input recordings |
| `H2_WATERMARK`, `H2_DEBUG_MENU`, `H2_SNAPSHOT_PROBE`, `H2_UI_WATCH`, `H2_UI_PREVIEW_DIR`, `H2_UI_SHEET_DIR`, `H2_SKULLS_REVEAL` | snapshot, UI and play-test aids |
| Host, Live, deploy and test-data env (`H2_HOST_*`, `H2_LEASE`, `H2_TOKEN`, `H2_LIVE_*`, `H2_DATA*`, `H2_DISC`, `H2_XBE`, `H2_BENCH_*`...) | dedicated hosts, Live, deploys and test data |

## Waiting for a measurement

Off by default. Each is the owner's call once the A/B named is run.

| Switch | What | Waiting for |
|---|---|---|
| `H2_FLAT_RAM`, `H2_FLAT_WRITE_WATCH` (+ `H2_FLAT_STATS_OWNER`) | guest RAM as one flat host mapping, and its write watch | today's full-image A/B |
| `H2_RAM_PAGE_CACHE` | reuses RAM pages within one gate hold | the same |
| `H2_X87_TOP` | checked x87 TOP specialisation | the same |
| `H2_ENTRY_GATE` | takes the RAM gate at handler entry; 11% off a runner entry in benches | an in-game profile ([guest-memory.md](../perf/guest-memory.md)) |
| `H2_DIRTY_INLINE` | tests the dirty bit at store sites; 0.4 ns a store under a watch | an in-game profile |
| `H2_DIRECT_GATE`, `H2_DIRECT_COUNT` | gate and count on the direct-call path; proven equivalent, untimed | a timing (they touch the `H2Direct` layout) |
| `H2_VIEW_CHARGE=0` | the per-call charge path; view charging became the default on 2026-11-20 with no recorded measurement | an in-game profile of the new default |
| `impl.render.pipelined_frame` (+ `H2_PIPELINE_*` knobs) | render front-end one frame behind the sim on a worker ([pipelined-render.md](../graphics/pipelined-render.md)) | its documented next step |
| `impl.render.interpolate`, `impl.render.reproject_ahead` | in-between frames by replay; reprojection carried past N | E1 played at 60 ([high-framerate.md](../game/high-framerate.md)) |
| `impl.render.occlusion` | Hi-Z occlusion culling of object setups | a first in-game check ([occlusion-culling.md](../graphics/occlusion-culling.md)) |
| `impl.sim.physics`, `impl.sim.native_*` | native simulation ports | today's sim A/B |
| `impl.engine.datums`, `impl.engine.tags` | Rust datum and tag leaves (`impl.engine.locale` is on) | a play-test |
| `H2_SIGNIN_DIALOG` | the sign-in dialog window | a game run ([ui-builder.md](../game/ui-builder.md)) |
| `H2_NO_COOP_ROW` | hides the co-op row | the co-op lane (read in files this cleanup could not touch) |
