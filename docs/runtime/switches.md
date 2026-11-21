# Switches

The settings, environment variables and feature switches used by the player,
compiler, service and tools. The tables group related variables. The source
paths name the full lists for settings, diagnostics and test inputs. Removed
render switches are recorded in
[not-adopted.md](../graphics/notes/not-adopted.md) and in each removal's
commit message.

`H2_FEATURES` takes a comma-separated list: `id`, `-id` or `id=on|off`.
An `impl.` name is an implementation switch
([`feature::switches`](../../crates/hooking/hooks/src/switches.rs)).

## Settings

| Switch | What |
|---|---|
| Every setting in `crates/extensions/game/src/setting_defs.rs` | the Settings menu's values; each has its env name there (`H2_FOV`, `H2_FPS`, `H2_LIGHTING`, `H2_SINGLE_COPY`, `H2_PAD_*`, `H2_VOICE_*`, `H2_TOUCH_*`, `H2_STEAM_*`...) |
| `H2_THEATER_RING_MIB` | Overrides the viewer’s `theater.ring_mib` setting for this launch. The [Theater ring](../game/theater-design.md#the-session-ring) defaults to 256 MiB. Set a persistent budget with `h2 settings set theater.ring_mib 128`. Zero disables ring capture. |
| `H2_RESOLUTION` | `native` (drawable aspect, up to a 3840 × 2160 pixel budget, the default) or a fixed `WxH`, each dimension 1–8192. Locks the Video screen's Resolution row. A headless GPU uses 640x480 for Native. |
| `H2_RENDER_SCALE` | Legacy resolution override from 1 to 6. A value of 2 means 1280x960. `H2_RESOLUTION` takes precedence. |
| `H2_HUD_POSITION` | `pinned` (default) anchors HUD elements to their screen edges. `centered` uses a centered safe area. Locks the HUD Position row. |
| `H2_LOD_DISTANCE` | Video’s LOD Distance (`graphics.lod_distance`). Choose 0.5, 1 (default), 1.5, 2, 3 or 4 times the resolution-adjusted distance, or `retail` for 640x480 distances at any resolution. |
| `H2_ICONS` | Prompt glyphs. `auto` follows the active device. `--icons` takes precedence ([button icons](../input/button-icons.md)). |
| `H2_AUDIO_LATENCY_MS` | Starting audio playout target, 10 to 500 ms (`crates/extensions/game/src/dsound_glue.rs`). |
| `H2_AIM_ASSIST` | aim-assist strength. Its Controls row appears on iOS and is editable from the main menu ([aim-assist.md](../game/aim-assist.md)) |
| `H2_PAD_MUTE` | DualSense mute button and LED. Locks the Mute Button row in Voice ([playstation-audio.md](../audio/playstation-audio.md)) |
| `--aspect source\|4:3\|16:9` | Desktop presentation shape. Source (default) follows the drawable in Native resolution. The fixed shapes fit inside it ([app.md](app.md)). |
| `H2_LEVEL`, `H2_DIFFICULTY` | boot straight into a level ([level-boot.md](level-boot.md)) |
| `H2_MAP_DIR` | Whole-map override directory for local play and editor testing (`crates/extensions/game/src/map_override.rs`). |
| `H2_MAPPACK=0`, `H2_HALO1=0` | Skip automatic import of the adjacent Map Pack or Halo 1 disc ([map pack](../game/map-pack.md), [Halo 1](../halo1/campaign.md)). |
| `H2_MAPPACK_ISO`, `H2_MAPPACK_DATA` | Editor Map Pack ISO override and installed Map Pack test-data root (`crates/tools/editor-core/src/mappack.rs`, `crates/apps/studio/src/app/map_pack_tests.rs`). |
| `H2_IMPORT_JOBS` | Parallel jobs for Halo 1 import (`crates/apps/cli/src/h1_import.rs`). |
| `H2_CLOUD_SAVES=0` | Keep saves on this machine during Live play (`crates/apps/cli/src/play_cloud.rs`). |
| `H2_LIVE_SERVER` | Service selected by `h2 play --live`. Use a URL or `off` ([app.md](app.md)). |
| `ui-back` | on by default. B or Back returns to Game Setup from its nested selectors. `H2_FEATURES=-ui-back` restores the retail selector Back behavior ([Back navigation](../game/ui-screens.md#back-navigation)) |
| `return-to-menu` | on in ordinary windowed runs. Shows the host loading picture on the return from a game ([level-boot.md](level-boot.md#returning-to-the-main-menu)) |
| `impl.render.native=off` | turns off the native GPU renderer, every stage together ([native-renderer.md](../graphics/native-renderer.md)) |
| `impl.render.reproject` | off by default. Enables rotation-only reprojection between guest frames above 30 fps ([high-framerate.md](../game/high-framerate.md)) |
| `audio.hrtf`, `audio.distance_reverb` | headphone binaural filtering; extra distance reverb |
| `lobby-options=off` | keeps the retail Quick Options lines in the custom game lobby ([ui-builder.md](../game/ui-builder.md#the-pregame-lobbys-quick-options)) |

## Build and code generation

Compiler flags contribute to the module stamp. Use matching settings for
build and run (`crates/recompiler/emit/src/compile.rs`,
`crates/runtime/recomp/src/tier.rs`).

| Switch | What |
|---|---|
| `H2_TIER` | Default for `h2 run` and `h2 play` when `--tier` is absent. `auto` uses a current clang build or starts on Cranelift and builds clang in the background. `fast` selects Cranelift only. `clang` waits for the optimised build ([fast tier](../lifter/cranelift-tier.md)). |
| `H2_BOOT_CACHE` | Boot analysis cache directory (default `$TMPDIR/h2boot-cache`). `off` disables it (`crates/runtime/recomp/src/fallback.rs`). |
| `H2_OPT`, `H2_DEBUG`, `H2_CPU`, `H2_PGO`, `H2_HOT`, `H2_OPT_COLD`, `H2_PASSES`, `H2_CLIF_OPT` | optimisation level, debug info, target CPU, PGO, tiers (on at `H2_OPT=2`/`3`; `H2_OPT_COLD=off` turns them off), IR passes ([building.md](building.md), [BUILD-CACHE.md](../lifter/BUILD-CACHE.md); `H2_PASSES=all,imprecise-faults` builds imprecise memory faults, [block-clock-imprecise-faults.md](block-clock-imprecise-faults.md)) |
| `H2_EMIT_TARGET`, `H2_XWIN`, `H2_LINUX_SYSROOT`, `H2_LLD`, `H2_LLVM`, `H2_OBJ_CACHE*` | cross targets, toolchains and the object cache |
| `H2_FASTMEM=0`, `H2_X87_NATIVE=0`, `H2_SSE_NATIVE=0`, `H2_ACCESS_COUNT=0`, `H2_DIRECT_CALLS=0` | exact reference builds for bisecting a divergence ([BOOT.md](BOOT.md)) |
| `H2_ACCESS_COUNT=block` | the block clock: the scheduler counts the plan's block costs, not guest memory accesses; module feature 64, so lockstep, films and sim-diff baselines differ from the access clock's ([block-clock-imprecise-faults.md](block-clock-imprecise-faults.md)) |
| `H2_PERF`, `H2_COLD_STATS` | generated-code counters for profiling |
| `H2_PGO_OUT` | Path for the raw profile from an instrumented clang module (`crates/recompiler/emit/src/compile/pgo.rs`). |
| `H2_CLIF_VERIFY`, `H2_TRACE` | Cranelift verifier; analysis diagnostics |

## Tools

`H2_THEATER_EXPORT_JOB` passes a private job directory from the watching
Theater process to its offline clip worker. The worker reports progress and
the final result through that directory.

| Switch | What |
|---|---|
| `h2 lighting split <map> [--bsp N] [--light ID] [--disc ISO] --out DIR` | Offline [lightmap comparison](../graphics/dynamic-lighting.md#the-split-offline). BSP and sky light default to 0. IDs are zero-based. A number selects a light in scenario sky order among skies used by the BSP; `placed:N` selects scenario lamp placement N. The disc follows the usual map lookup. Use a new or empty directory outside Git and open its `index.html`. |
| `h2 play --join URL`, `live-join-link` | open a [Live session link](../live/live-custom-games.md#join-in-halo-2) on the selected service during Live player runs. Film recording and playback disable joins. `H2_FEATURES=-live-join-link` disables game-side handling. |
| `H2_AGENT`, `agent-calls` | the agent API socket ([agent-api.md](agent-api.md)): a path or `tcp://HOST:PORT`; unset, `h2 play` opens `<cache>/h2recomp/agents/<pid>.sock` for editors to find ("Finding a game"); on macOS, a windowed game also publishes that socket when using a custom endpoint; `off` (or `0`, `none`, `false`, `no`) keeps every socket closed; `-agent-calls` leaves out the agent's calls into the game (`sim` teleports, give and spawn, and `maps.test.load`, [test-maps.md](../game/test-maps.md)) |
| `agent-net` | the observer that counts replication events by type for `net.replication` ([agent-api.md](agent-api.md#netreplication)); `-agent-net` leaves the counts at zero |
| `creature-probe` | off by default. `H2_FEATURES=creature-probe` registers the experimental slot-17 creature definition and selects its replication version for Live and System Link. Both peers use the same setting ([P7](../game/ai-networking.md#creature-slot-probe)) |
| `h2 host bundle --sim-stores DIR` | package verified shared resource stores; leased hosts select the bundle’s store directory ([dedicated-host-build.md](../live/dedicated-host-build.md#package-recorded-shared-resources)) |
| `impl.sim.mapped-reads` | use eligible immutable file ranges as private guest pages on a light host; disable for the copy arm ([host-halving.md](../live/host-halving.md#halving-the-memory)) |
| `H2_PACK_LINGER_S` | supervisor idle timeout in seconds, default 60; Live demand holds and cleanup last 60 seconds ([host-halving.md](../live/host-halving.md#warm-capacity-from-demand)) |
| `H2_PACK_MAX_MATCHES`, `H2_PACK_MEMORY_MIB`, `H2_PACK_VCPUS`, `LIVE_HOST_PACK_MATCHES` | supervisor slot, memory and CPU budgets; Live selects the class limit or a lower slot count ([dedicated-host.md](../live/dedicated-host.md#packed-hosts)) |
| `MALLOC_ARENA_MAX`, `MALLOC_TRIM_THRESHOLD_` | Linux glibc host lease defaults of 2 arenas and 131072 bytes; explicit allocator settings take precedence ([host-halving.md](../live/host-halving.md#halving-the-memory)) |
| `H2_STUDIO` | Editor agent socket path when `--socket` is absent (`crates/apps/studio-proto/src/lib.rs`, [editor API](../editor/agent-api.md)). |
| `H2_FFMPEG` | Selects the local ffmpeg executable for movie conversion (`crates/formats/movies/src/install.rs`). |
| `H2_LAB_HOST`, `H2_LAB_SOCKET` | default driving target and local lab socket ([lab.md](lab.md)) |
| `h2 lab host [install] --takeover-timeout SECONDS` | player consent answer timeout, 1 to 3600 seconds, default 60 ([Player consent](lab.md#player-consent-before-a-job)) |
| `h2 lab host [install] --takeover-cooldown SECONDS` | delay after a decline before asking again, default 1800 seconds, 0 lets every job ask ([Player consent](lab.md#player-consent-before-a-job)) |
| `lab-takeover`, `lab-takeover-dialog` | the windowed game's consent driver and dialog. Turning either off makes consent unavailable while the game window is open ([Player consent](lab.md#player-consent-before-a-job)) |
| `agent-audio` | with the agent and DirectSound active, records [`audio.loops`](agent-api.md#audioloops) history; switching it off leaves the current-state read available |
| `h2 live mods import --pack`, `--author`, `--source-url`, `--year`, `--version`, `--description`, `--cheating-pack`, `--not-for-online` | pack identity, credit and release metadata ([Importing a mod](../game/maps.md#importing-a-mod)) |
| `h2 live mods capture --disc`, `--out`, `--server`, `--as` | owner’s base disc, compiled image directory, Live endpoint and uploader account ([Mods](../live/live-art.md#mods)) |
| `attract` | during movie playback outside a film recording or replay, validates the idle movie before the music fades and keeps the music playing if unavailable ([Attract timer](../audio/bink-hle.md#attract-timer)) |
| `H2_SIM` and the sim tooling (`impl.sim.arena-hash`, `H2_DISPATCH_STATS`, `H2_ENTRY_STATS`) | headless matches ([sim.md](sim.md)) |
| `impl.sim.submix-fx`, `impl.sim.observer-resources`, `impl.sim.metrics-projection`, `audio.packet-clock` | on; `=off` is the reference the sim budget measures against ([sim-budget.md](../perf/sim-budget.md)) |
| `H2_BOOT_VERBOSE`, `H2_FEATURE_PLAN`, `H2_NATIVE_VERBOSE` | log verbosity |
| `H2_PERF_FROM`, `H2_PROFILE*`, `H2_TIMELINE*`, `H2_FRAME_STATS*`, `H2_HOST_PERF`, `H2_AUDIO_STATS`, `H2_SAMPLE_*`, `render-timing`, `render-census` | perf capture ([profiling.md](../perf/profiling.md)) |
| `H2_BOOT_GFX*`, `H2_BOOT_CAPTURE*`, `H2_BOOT_DRAWLOG*`, `H2_BOOT_UPLOAD_ALWAYS`, `H2_SWEEP_DIAG`, `H2_BOOT_STOP_AT_PRESENT`, `H2_BOOT_INPUT*` | headless rendering, captures and input |
| `H2_BOOT_FPS_FROM`, `H2_BOOT_REALTIME` | Headless frame-report start and wall-clock pacing (`crates/apps/boot/src/lib.rs`). |
| `H2_RENDER_CENSUS_LINES`, `H2_RENDER_CENSUS_OUT`, `H2_RENDER_TIMING_SITES` | Render reports' row budgets and census TSV output (`crates/rewrites/render-bridge/src/render_census.rs`, `crates/rewrites/render-bridge/src/render_timing.rs`). |
| `H2_BOOT_GFX_BETWEEN=N` | with in-between frames on in a headless `H2_BOOT_GFX` run (`crates/extensions/game/src/d3d_glue.rs`, `BetweenShots`): write the first N in-between frames as `frame-NNNNN-between-KK.png`, and the guest frames shown meanwhile as `frame-NNNNN.png`; ignored without in-between frames ([high-framerate.md](../game/high-framerate.md)) |
| `H2_BOOT_GFX_SIZE=WxH` | exact PNG size for [headless display clips](agent-api.md#rendercapture_clip), from a centered aspect-ratio crop of the GPU view. Unset uses the view's size. Both dimensions must fit the view at `H2_RENDER_SCALE` and the GPU's texture limit (`crates/extensions/game/src/d3d_glue/clip_png.rs`). |
| `H2_GFX_MERGE_CHECK`, `H2_D3D_WRITEBACK_TRACE`, `H2_RENDER_THREAD_STATS`, `H2_RENDER_THREAD=0`, `frame-scene.census` | render diagnostics; `H2_RENDER_THREAD=0` is the synchronous reference; `frame-scene.census` is the entry census `render.scene` reports ([framescene.md](../graphics/framescene.md), slice 4) |
| `frame-scene.layers` | off; with the native renderer on, the scene's layers 1, 5, 7, 8 and 20 draw their clusters' and placements' static items from the scene's plan, the rest on the D3D path; outcomes in `render.scene` as `layer_plan_*` ([framescene.md](../graphics/framescene.md), slice 4) |
| `frame-scene.sink` | off; with the native renderer on, the device's scene sink files each guest D3D op by the frame's slot, names side targets and counts readbacks; the D3D backend still draws everything. `render.scene` reports it as `carried` and `scene_carried_*`, `scene_readback*` ([framescene.md](../graphics/framescene.md), slice 6) |
| `H2_PIPELINE_CACHE`, `H2_GFX_COMPILE_THREADS`, `H2_VERTEX_CACHE_MB`, `H2_SMOOTH_TEXT`, `H2_RENDER_OVERRIDES` | D3D pipeline cache and sibling native descriptor archive, D3D compile workers, vertex cache budget, glyph smoothing, hot render overrides |
| `H2_LOCKSTEP*`, `H2_INPUT_RECORD`, `H2_INPUT_REPLAY` | lockstep and input recordings |
| `H2_FILM_OUT`, `H2_FILM_DIR` | Recording destination and film library location ([film format](../game/film-format.md)). |
| `H2_THEATER_MEASURE=1` | enables [Theater measurements](agent-api.md#theater-transport) on the recorder or player, including guest write-watch counts during recording |
| `H2_TELEMETRY=0`, `H2_TELEMETRY_FILE` | Disable telemetry or write a local capture (`crates/live/live-telemetry/src/config.rs`). |
| `H2_BACKGROUND_THREADS` | how many threads one background job (including nav, visibility and native pipeline builds) splits across; unset, `0` or not a number: a quarter of the cores, at least one; at most the cores ([background-work.md](background-work.md)) |
| `H2_BOTS_NAV_AUTO` | `0`, `off`, `false` or `no`: the game builds no bot nav meshes or visibility grids by itself; unset or anything else: on (`crates/live/bots/src/autonav.rs`, `ENV`) |
| `H2_WATERMARK`, `H2_DEBUG_MENU`, `H2_SNAPSHOT_PROBE`, `H2_UI_WATCH`, `H2_UI_PREVIEW_DIR`, `H2_UI_SHEET_DIR`, `H2_SKULLS_REVEAL` | snapshot, UI and play-test aids |
| `LIVE_MAX_HOSTS` | concurrent dedicated leases, default 4. Zero disables admission. Accepted caps are 0–4, 16, 64, 256 and 1,024. The Worker derives per-class container limits at deploy time ([capacity steps](../live/host-halving.md#caps)) |
| `h2 host run --pack-control IP:PORT` | private supervisor address supplied to the packed child to read its usage at match end ([packed hosts](../live/dedicated-host.md#packed-hosts)) |
| `h2 live invite grant <gamertag> [--open-cap N]` | grants invite access, optionally limiting a non-admin's open invites ([Live admins](../live/live-admin.md#5-the-dashboards-server-screen-and-admin-invites)) |
| `h2 live invite ungrant <gamertag>` | removes the explicit grant and closes a non-admin's open invites |
| `H2_LIVE_SCREENSHOT_CLASSIFIER_URL` | optional native screenshot classifier at a literal loopback HTTP address. Unset holds captures for review when no classifier is installed ([Screenshot review](../live/live-admin.md#screenshot-review)) |
| `H2_NO_COOP_ROW` | If present, hides the co-op row (`crates/apps/boot/src/registration.rs`). |
| `H2_HOST_*`, `H2_LEASE`, `H2_TOKEN`, `H2_LIVE`, `H2_DATA_*` | Dedicated host settings, leased credentials and bundle selection (`crates/apps/cli/src/host_cmd`, [host build](../live/dedicated-host-build.md)). |
| `H2_LIVE_*` | Live account, server and storage settings (`crates/apps/cli/src/play_live.rs`, `crates/live/live-server/src/standalone.rs`, `crates/live/live-worker/src/settings.rs`). |
| `H2_XNET_*`, `H2_COOP_*` | XNet endpoint and network diagnostics, co-op overrides and test controls (`crates/live/live-hle/src/network_config.rs`, `crates/extensions/game/src/coop_live.rs`, `crates/apps/cli/src/coop_test_cmd.rs`). |
| `H2_DATA`, `H2_DISC`, `H2_XBE`, `H2_BENCH_*` | Data directory, local disc and executable inputs, and parameters for individual test or benchmark harnesses. |

## Waiting for a measurement

These switches select alternate implementations for comparison. The
third column describes the remaining checks.

| Switch | What | Waiting for |
|---|---|---|
| `H2_THEATER_RING=1` | Opts a film player into [warm restores and the rewind ring](../game/theater-design.md#warm-restore-and-its-fallback). Off by default. | keyframe cost measurements and film replay verification |
| `actor-net` | Capture replicated AI actors and accepted dialogue after each distributed host tick ([AI over the network](../game/ai-networking.md#on-the-host-each-tick)) | P3 checks which presentation the client receives from retail |
| `H2_FLAT_RAM` | at build, the flat memory model: each guest access is one host instruction in the runtime's window, with a fixup to the helper (module feature 16; macOS and x86-64 Linux only); set it on the run too, or the image recompiles paged. At run time it also makes guest RAM aliasable from the start ([contract-thin-transfers-flat-ram.md](contract-thin-transfers-flat-ram.md), section 4.1, slice F3; [RUNTIME-CONTRACT.md](../../crates/recompiler/emit/RUNTIME-CONTRACT.md), "Flat RAM") | M4 of the contract doc |
| `H2_EMIT_FLAT_RAM`, `H2_FLAT_WRITE_WATCH` (+ `H2_FLAT_STATS_OWNER`) | at build, the opt-in span image instead (`H2_FLAT_SPAN`); with it, `H2_FLAT_RAM` at run time turns the span path and its write watch on or off for that image (RUNTIME-CONTRACT.md, "Opt-in flat RAM and first-write faults") | M0 of the contract doc |
| `H2_FLAT_STATS=1` | Records flat RAM protection statistics (`crates/runtime/runtime/src/flat/protection.rs`). | Flat RAM diagnostics |
| `H2_RAM_PAGE_CACHE` | reuses RAM pages within one gate hold | a full-image A/B |
| `H2_X87_TOP` | checked x87 TOP specialisation | a full-image A/B |
| `H2_ENTRY_GATE` | takes the RAM gate at handler entry; bench results in [guest-memory notes](../perf/notes/guest-memory.md) | an in-game profile ([guest-memory.md](../perf/guest-memory.md)) |
| `H2_DIRTY_INLINE` | tests the dirty bit at store sites | an in-game profile |
| `H2_DIRECT_GATE`, `H2_DIRECT_COUNT` | gate and count on the direct-call path; proven equivalent, untimed | a timing (they touch the `H2Direct` layout) |
| `H2_VIEW_CHARGE=0` | the per-call charge path; view charging is the default (`crates/recompiler/emit/src/compile.rs`), for counting modules | an in-game comparison of both paths |
| `impl.render.pipelined_frame` (+ `H2_PIPELINE_*` knobs) | render front-end one frame behind the sim on a worker ([pipelined-render.md](../graphics/pipelined-render.md)) | its documented next step |
| `impl.render.interpolate`, `impl.render.reproject_ahead` | in-between frames by replay; reprojection carried past N | E1 played at 60 ([high-framerate.md](../game/high-framerate.md)) |
| `host-light.tick-pacing`, `H2_HOST_TICK_PHASE_NS` | opt-in real-time light-host wake coalescing; the supervisor supplies the slot phase in nanoseconds | four-match tick p99, packet latency and paced hash parity ([host-halving.md](../live/host-halving.md#halving-the-cpu)) |
| `host-light-native` | light hosts opt into native animation, collision and objects together; explicit `impl.sim.native` or leaf settings take precedence | Waterworks and Lockout hash parity and Waterworks median CPU saving ([host-halving.md](../live/host-halving.md#halving-the-cpu)) |
| `impl.sim.native_*` (the individual ports are `crates/rewrites/engine/src/sim_physics`) | native simulation ports | a sim A/B |
| `impl.sim.native.collision` | BSP point and location queries in Rust, with FP observers for vector and instance traces ([collision queries](../engine/collision-queries.md)) | per-map parity, cross-architecture bail counts and timing |
| `impl.sim.native.objects` (`-impl.sim.native` turns it off with the other subsystems) | the object update in Rust, `crates/rewrites/engine/src/sim_objects` ([object-update.md](../engine/object-update.md)): the walks, the per-object updates, the type dispatchers, the wake leaves, function values with their imports, change colours, the damage update (with the tick's `Damage` and `Kill` events), the unit update's shell, movement, control, state, powered seats, timers, dialogue, team, camouflage and light with the unit animation state, the biped update and post-physics update, and the item, scenery, device, machine, crate, creature and projectile updates; the per-function ports at the points its bodies own stand down | run parity on the slices that add bodies ([sim-rewrite-objects.md](../engine/sim-rewrite-objects.md) section 12) |
| `impl.engine.datums`, `impl.engine.tags` | Rust datum and tag leaves (`impl.engine.locale` is on) | a play-test |
| `H2_SIGNIN_DIALOG` | the sign-in dialog window | a game run ([ui-builder.md](../game/ui-builder.md)) |
