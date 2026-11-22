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
| Every setting in `crates/extensions/game/src/setting_defs.rs` | the Settings menu's values; each has its env name there (`H2_FOV`, `H2_LIGHTING`, `H2_SINGLE_COPY`, `H2_PAD_*`, `H2_VOICE_*`, `H2_TOUCH_*`, `H2_STEAM_*`...) |
| `H2_THEATER_RING_MIB` | Overrides the viewer’s `theater.ring_mib` setting for this launch. The [Theater ring](../game/theater-design.md#the-session-ring) defaults to 256 MiB. `H2_THEATER_RING=0` turns the ring and its warm restores off ([warm restore](../game/theater-design.md#warm-restore-and-its-fallback)); the CLI's `theater verify` and export set it explicitly. Set a persistent budget with `h2 settings set theater.ring_mib 128`. Zero disables ring capture. |
| `H2_FPS`, `h2 play --fps` | Window presentation rate. Choose 30 (default), 60, 120 or uncapped. All four use real-time guest pacing in normal play. |
| `h2 play --unpaced` | Disables the shell's guest pacing for throughput measurements. A fast host can simulate more than 30 ticks per wall second. |
| `H2_RESOLUTION` | `native` (drawable aspect, up to a 3840 × 2160 pixel budget, the default) or a fixed `WxH`, each dimension 1–8192. Locks the Video screen's Resolution row. A headless GPU uses 640x480 for Native. |
| `H2_RENDER_SCALE` | Legacy resolution override from 1 to 6. A value of 2 means 1280x960. `H2_RESOLUTION` takes precedence. |
| `H2_HUD_POSITION` | `pinned` (default) anchors HUD elements to their screen edges. `centered` uses a centered safe area. Locks the HUD Position row. |
| `H2_LOD_DISTANCE` | Video’s LOD Distance (`graphics.lod_distance`). Choose 0.5, 1 (default), 1.5, 2, 3 or 4 times the resolution-adjusted distance, or `retail` for 640x480 distances at any resolution. |
| `H2_ICONS` | Prompt glyphs. `auto` follows the active device. `--icons` takes precedence ([button icons](../input/button-icons.md)). |
| `H2_AUDIO_LATENCY_MS` | Starting audio playout target, 10 to 500 ms (`crates/extensions/game/src/dsound_glue.rs`). |
| `H2_AIM_ASSIST` | aim-assist strength. Its Controls row appears on iOS and is editable from the main menu ([aim-assist.md](../game/aim-assist.md)) |
| `H2_SUPERBOUNCE=1` | turns the [superbounce game type](../game/superbounce.md#the-superbounce-game-type) (the HUD, spot lines, `superbounce.state`) on in any multiplayer or campaign game, not only the Superbounce variant. The `superbounce` feature (`H2_FEATURES`) switches the whole mode off, and `superbounce-assist` the hold-X fall assist ([superbounce.md](../game/superbounce.md#hold-x-the-fall-assist)), which also needs the player's `superbounce.fall_assist` setting on |
| `H2_SUPERBOUNCE_TUTORIAL=1` | shows the [superbounce HUD's tutorial steps](../game/superbounce.md#the-superbounce-game-type) the next time a game starts, even when the `superbounce.tutorial_seen` setting says they have been seen |
| `H2_PAD_MUTE` | DualSense mute button and LED. Locks the Mute Button row in Voice ([playstation-audio.md](../audio/playstation-audio.md)) |
| `--aspect source\|4:3\|16:9` | Desktop presentation shape. Source (default) follows the drawable in Native resolution. The fixed shapes fit inside it ([app.md](app.md)). |
| `H2_STEAM_KEYBOARD=0` | on Linux under Steam, keeps Steam's on-screen keyboard closed when the game's virtual keyboard gains focus after controller input ([host-keyboard.md](../input/host-keyboard.md#steams-on-screen-keyboard)) |
| `H2_LEVEL`, `H2_DIFFICULTY` | boot straight into a level ([level-boot.md](level-boot.md)) |
| `H2_MAP_DIR` | Startup directory for disc map overrides. The first lookup, including a directory listing, selects the source for each map. A paired editor can request a separate play copy. [Map overrides](storage.md#map-overrides) |
| `H2_MAPPACK=0`, `H2_HALO1=0` | Skip automatic import of the adjacent Map Pack or Halo 1 disc ([map pack](../game/map-pack.md), [Halo 1](../halo1/campaign.md)). |
| `H2_MAPPACK_ISO`, `H2_MAPPACK_DATA` | Editor Map Pack ISO override and installed Map Pack test-data root (`crates/tools/editor-core/src/mappack.rs`, `crates/apps/studio/src/app/map_pack_tests.rs`). |
| `H2_IMPORT_JOBS` | Parallel jobs for Halo 1 import (`crates/apps/cli/src/h1_import.rs`). |
| `H2_CLOUD_SAVES=0` | Keep saves on this machine during Live play (`crates/apps/cli/src/play_cloud.rs`). |
| `variants.port.script`, `H2_VARIANTS_PORT_SCRIPT` | How saved variants are ported into the game-variant store at boot: `keep` (default; their engine's own rules) or `builtin` (the engine's built-in rules script named and carried). ([Live game variants in the game](../game/rules-engine.md#live-game-variants-in-the-game)) |
| `game-variants` | On by default; `-game-variants` turns it off. Writes a game-variant store variant's options and script reference on the game at its start ([Live game variants in the game](../game/rules-engine.md#live-game-variants-in-the-game)). |
| `H2_LIVE_SERVER` | Service selected by `h2 play --live`. Use a URL or `off` ([app.md](app.md)). |
| `forge` | On by default; `-forge` turns it off. The [local Forge session](../game/forge-design.md#the-ui) on offline multiplayer maps: D-pad Up or Tab toggles the monitor. The party lobby's mode strip has a FORGE tab while it is on ([the lobbies](../game/forge-firefight-ui.md#the-lobbies)); `-forge` removes the tab. Light hosts, film replays and lockstep runs never have it. |
| `forge-screens`; `forge-objects`, `forge-properties`, `forge-tools`, `forge-settings`, `forge-link`, `forge-question`, `forge-options`, `forge-controls` | on with `forge`. The Forge screens in the game and GAME MENU's Forge rows; each screen's class is its own feature. With `-forge-screens` the session refuses every screen by name ([forge-firefight-ui.md](../game/forge-firefight-ui.md#the-screens)) |
| `tweakers` | on by default. Adds the Tweakers row under Territories in the game type list and lists Superbounce there instead of under Slayer ([community-gametypes.md](../game/community-gametypes.md#tweakers)). `H2_FEATURES=-tweakers` leaves the retail list, with Superbounce under Slayer |
| `teabag` | on by default. Watches every game for teabags and records them to the film and Live, where they take the Incel medal ([teabag.md](../game/teabag.md)). `H2_FEATURES=-teabag` records none, so Live and the film give Incel to everyone who completes a game |
| `ui-back` | on by default. B or Back returns to Game Setup from its nested selectors. `H2_FEATURES=-ui-back` restores the retail selector Back behavior ([Back navigation](../game/ui-screens.md#back-navigation)) |
| `return-to-menu` | on in ordinary windowed runs. Shows the host loading picture on the return from a game ([level-boot.md](level-boot.md#returning-to-the-main-menu)) |
| `impl.render.native=off` | turns off the native GPU renderer, every stage together ([native-renderer.md](../graphics/native-renderer.md)) |
| `impl.render.interpolate`, `impl.render.reproject` | Host frames above 30 fps; both generate up to 60 frames per second ([high-framerate.md](../game/high-framerate.md)). Interpolate, on by default, replays geometry with blended transforms; `-impl.render.interpolate` turns it off. Reproject, off by default, warps the world image for camera rotation and takes its place when set. At 30 fps neither acts. |
| `audio.hrtf`, `audio.distance_reverb` | on by default: headphone binaural filtering; extra distance reverb. `audio.hrtf=off` and `audio.distance_reverb=off` turn them off ([audio.md](../audio/audio.md#headphones-3d)). |
| `actor-net` | on; `-actor-net` stops capturing replicated AI actors and accepted dialogue after each distributed host tick ([AI over the network](../game/ai-networking.md#on-the-host-each-tick)) |
| `firefight`, `firefight-row`, `firefight-options` | on; `-firefight` turns off the [Firefight](../game/firefight.md#the-host-runtime) director and host log (in each multiplayer game whose map carries the kit; any other map gets the refusal and its reason in `firefight.state`). `-firefight-row` removes the Game Setup row, `-firefight-options` the [Firefight Options...](../game/firefight.md#game-setup) page. With `firefight` on, the party lobby's mode strip has a FIREFIGHT tab ([the lobby](../game/firefight.md#the-lobby)). |
| `rules` | off; `rules` turns on the [rules script host runtime](../game/rules-engine.md#the-host-runtime): at each multiplayer game's start it runs the script the record names (a built-in by hash prefix; any other is refused by name until the Live fetch exists) on the machine `GAME_ENGINE_AUTHORITATIVE` answers true for, keeps its state as the `RULES` record of `SimExt`, puts its hash on a Live host's session and advertises `rules/1`. A refused script is logged and the game plays its own engine's rules; `rules.state` says which. The `rules` cargo feature of `game` and `boot` (on by default, and in the cli's `host`) compiles it in |
| `impl.engine.datums`, `impl.engine.tags`, `impl.engine.locale` | on; `=off` turns off the Rust datum, tag and locale leaves, each group or one function (`impl.engine.tags.tag_get_checked=off`) |
| `lobby-options=off` | keeps the retail Quick Options lines in the custom game lobby ([ui-builder.md](../game/ui-builder.md#the-pregame-lobbys-quick-options)); the party lobby's mode strip has its own hook on that update and is drawn either way ([party-lobby.md](../game/party-lobby.md#the-strip)) |

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
| `H2_FIELD_NAMES` | Assembly's plugin folder (or its `Halo2Xbox` child) for the ignored `every_plugin_merges_against_our_definitions` developer test. The test reads `Halo2Xbox`, its `Halo2` fallback, and the other retail folders as hints. With the variable unset, the test returns immediately. [Field-name comparison](../editor/notes/tag-field-names.md) |
| `h2 lighting split <map> [--bsp N] [--light ID] [--disc ISO] --out DIR` | Offline [lightmap comparison](../graphics/dynamic-lighting.md#the-split-offline). BSP and sky light default to 0. IDs are zero-based. A number selects a light in scenario sky order among skies used by the BSP; `placed:N` selects scenario lamp placement N. The disc follows the usual map lookup. Use a new or empty directory outside Git and open its `index.html`. |
| `h2 play --join URL`, `live-join-link` | open a [Live session link](../live/live-custom-games.md#join-in-halo-2) on the selected service during Live player runs. Film recording and playback disable joins. `H2_FEATURES=-live-join-link` disables game-side handling. |
| `H2_AGENT`, `agent-calls` | the agent API socket ([agent-api.md](agent-api.md)): a path or `tcp://HOST:PORT`; unset, `h2 play` opens `<cache>/h2recomp/agents/<pid>.sock` for editors to find ("Finding a game"); on macOS, a windowed game also publishes that socket when using a custom endpoint; `off` (or `0`, `none`, `false`, `no`) keeps every socket closed; `-agent-calls` leaves out the agent's calls into the game (`sim` teleports, give and spawn, and `maps.test.load`, [test-maps.md](../game/test-maps.md)) |
| `agent-edit-lifecycle` | Permits on-demand editor hooks for live object writes and map reloads. The first authorized write adopts the game. Opening the agent socket or displaying the pairing indicator leaves its hooks untouched. [Object edits](agent-api.md#object-edits) |
| `H2_AGENT_OUT=DIR` | game-side root for `traces/`, `flights/` and `inputs/` ([agent-api.md](agent-api.md#tick-traces-and-the-flight-recorder)). Defaults to the game's cache directory when unset or empty. Lab sets it to the job's run directory. |
| `agent-net` | the observer that counts replication events by type for `net.replication` ([agent-api.md](agent-api.md#netreplication)); `-agent-net` leaves the counts at zero |
| `creature-probe` | on by default; `-creature-probe` turns it off. Registers the slot-17 creature definition and selects its replication version for Live and System Link. A peer with a different version is refused as `replication_version_mismatch`, so both peers use the same setting ([P7](../game/ai-networking.md#creature-slot-probe)) |
| `h2 host bundle --data DIR`, `--no-bots` | where the bundle's bot nav meshes and visibility grids come from (`DIR/bots`, built there when stale; default: the data root `h2 play` uses); `--no-bots` leaves them out ([dedicated-host.md](../live/dedicated-host.md#game-data-bundle)) |
| `h2 host bundle --sim-stores DIR` | package verified shared resource stores; leased hosts select the bundle’s store directory ([dedicated-host-build.md](../live/dedicated-host-build.md#package-recorded-shared-resources)) |
| `impl.sim.mapped-reads` | use eligible immutable file ranges as private guest pages on a light host; disable for the copy arm ([host-halving.md](../live/host-halving.md#halving-the-memory)) |
| `impl.sim.page-census` | on; hashes guest RAM by 4 KiB page on the tick after counting begins and at the end for `wall.guest_pages_written_4k` and `_16k`; `=off` leaves them out of the report, for a timing arm ([sim.md](sim.md#6-metrics)) |
| `H2_PACK_LINGER_S` | supervisor idle timeout in seconds, default 60; Live demand holds and cleanup last 60 seconds ([host-halving.md](../live/host-halving.md#warm-capacity-from-demand)) |
| `H2_PACK_MAX_MATCHES`, `H2_PACK_MEMORY_MIB`, `H2_PACK_VCPUS`, `LIVE_HOST_PACK_MATCHES` | supervisor slot, memory and CPU budgets; Live selects the class limit or a lower slot count ([dedicated-host.md](../live/dedicated-host.md#packed-hosts)) |
| `MALLOC_ARENA_MAX`, `MALLOC_TRIM_THRESHOLD_` | Linux glibc host lease defaults of 2 arenas and 131072 bytes; explicit allocator settings take precedence ([host-halving.md](../live/host-halving.md#halving-the-memory)) |
| `H2_STUDIO` | Editor agent socket path when `--socket` is absent (`crates/apps/studio-proto/src/lib.rs`, [editor API](../editor/agent-api.md)). |
| `H2_FFMPEG` | Selects the local ffmpeg executable for movie conversion (`crates/formats/movies/src/install.rs`). |
| `H2_LAB_JOB_ID`, `H2_LAB_JOB_NAME` | The lab passes its run ID (for example, `0007`) and job label to the game for its window title and memory reports. |
| `H2_MEM_SOFT` | Process footprint at which the game, editor or lab host writes a memory report. Defaults to 8 GiB. Sizes accept binary units such as `12G`. Zero disables both the soft limit and sustained-growth alerts. |
| `H2_MEM_HARD` | Process footprint at which the watchdog writes a report and exits. Defaults to the lesser of 16 GiB and 75% of physical RAM. Zero disables the hard limit. |
| `H2_MEM_SAMPLE` | Mean allocation interval for Poisson stack sampling, in bytes or a binary size such as `1M`. Unset or zero disables sampled stacks. Live bytes and allocation counts per subsystem are always tracked in watched processes ([Finding a leak after the fact](agent-api.md#finding-a-leak-after-the-fact)). |
| `H2_MEM_REPORT_DIR` | Directory for watchdog JSON reports and rotating memory flight recorders. Defaults to `logs/mem/` under the selected h2 data directory. |
| `h2 lab run --own-data` | Play on the host Mac's own data root, settings and Live accounts, as `h2 play` there would, and keep its `H2_DATA`, `H2_LIVE_*` and `H2_CLOUD_SAVES`. Refused with `--fresh-data`. [Playing on your own data](lab.md#playing-on-your-own-data) |
| `h2 lab run --mem-limit SIZE` | Cap on the game child's footprint, 12 GiB by default. [Memory limits and reports](lab.md#memory-limits-and-reports) |
| `h2 lab host [install] --obj-cache-limit SIZE` | Size budget for the lab's shared object cache after each build attempt, 8 GiB by default. The host prunes the least recently used entries first. Zero retains no entries. |
| `h2 lab host [install] --target-limit SIZE` | Size budget for the shared Cargo target directory, 32 GiB by default. The host clears an oversized target after the build. Zero retains no Cargo outputs. |
| `h2 lab run --dedicated [--host-bots] [--map MAP] [--variant NAME] [--max-players N] [--base-port PORT]` | a headless dedicated System Link host on the lab Mac's LAN IPv4, XNet base 3174 by default (a base reaching the player's 3074 to 3080 is refused), with its lobby opened when `--map` is given ([Dedicated System Link host](lab.md#dedicated-system-link-host)) |
| `h2 lab sim --arm NAME=VALUE`, `--repeat N`, `--parallel N` | one arm of a sim benchmark and the `H2_FEATURES` its runs get, runs per arm, matches at once in a batch (default 1) ([Sim benchmarks](lab.md#sim-benchmarks)) |
| `H2_LAB_HOST`, `H2_LAB_SOCKET` | default driving target and local lab socket ([lab.md](lab.md)) |
| `h2 lab host [install] --max-jobs 1` | active job cap; only 1 is accepted, since the host runs one job at a time ([lab.md](lab.md#host-files-and-flags)) |
| `h2 lab host watch [--root DIR]`, `h2 lab logs <id\|current> [--follow]` | watch job and service logs on the host or through the paired lab connection ([Watching jobs](lab.md#watching-jobs)) |
| `h2 lab host [install] --takeover-timeout SECONDS` | player consent answer timeout, 1 to 3600 seconds, default 60; also how long the player has to answer a running job's question ([Player consent](lab.md#player-consent-before-a-job), [When the player opens a game](lab.md#when-the-player-opens-a-game)) |
| `h2 lab host [install] --takeover-cooldown SECONDS` | delay after a decline before asking again, default 1800 seconds, 0 lets every job ask ([Player consent](lab.md#player-consent-before-a-job)) |
| `lab-takeover`, `lab-takeover-dialog`, `lab-running-dialog` | the windowed game's consent driver and its two dialogs. Turning off either of the first two makes consent unavailable while the game window is open; turning off `lab-running-dialog` leaves a running job's question unanswered, so the job finishes and the queue waits ([Player consent](lab.md#player-consent-before-a-job), [When the player opens a game](lab.md#when-the-player-opens-a-game)) |
| `agent-audio` | with the agent and DirectSound active, records [`audio.loops`](agent-api.md#audioloops) history; switching it off leaves the current-state read available |
| `h2 live mods import --pack`, `--author`, `--source-url`, `--year`, `--version`, `--description`, `--cheating-pack`, `--not-for-online` | pack identity, credit and release metadata ([Importing a mod](../game/maps.md#importing-a-mod)) |
| `h2 live mods capture --disc`, `--out`, `--server`, `--as` | owner’s base disc, compiled image directory, Live endpoint and uploader account ([Mods](../live/live-art.md#mods)) |
| `attract` | during movie playback outside a film recording or replay, validates the idle movie before the music fades and keeps the music playing if unavailable ([Attract timer](../audio/bink-hle.md#attract-timer)) |
| `H2_SIM` and the sim tooling (`impl.sim.arena-hash`, `H2_DISPATCH_STATS`, `H2_ENTRY_STATS`) | headless matches ([sim.md](sim.md)) |
| `impl.sim.submix-fx`, `impl.sim.observer-resources`, `impl.sim.metrics-projection`, `audio.packet-clock` | on; `=off` is the reference the sim budget measures against ([sim-budget.md](../perf/sim-budget.md)) |
| `H2_BOOT_VERBOSE`, `H2_FEATURE_PLAN`, `H2_NATIVE_VERBOSE` | log verbosity |
| `H2_PERF_FROM`, `H2_PROFILE*`, `H2_TIMELINE*`, `H2_FRAME_STATS*`, `H2_HOST_PERF`, `H2_AUDIO_STATS`, `H2_SAMPLE_*`, `render-timing`, `render-census` | perf capture ([profiling.md](../perf/profiling.md)) |
| `H2_BOOT_GFX*`, `H2_BOOT_CAPTURE*`, `H2_BOOT_DRAWLOG*`, `H2_BOOT_UPLOAD_ALWAYS`, `H2_BOOT_STOP_AT_PRESENT`, `H2_BOOT_INPUT*` | headless rendering, captures and input |
| `H2_BOOT_FPS_FROM`, `H2_BOOT_REALTIME` | Headless frame-report start and wall-clock pacing (`crates/apps/boot/src/lib.rs`). |
| `H2_RENDER_CENSUS_LINES`, `H2_RENDER_CENSUS_OUT`, `H2_RENDER_TIMING_SITES` | Render reports' row budgets and census TSV output (`crates/rewrites/render-bridge/src/render_census.rs`, `crates/rewrites/render-bridge/src/render_timing.rs`). |
| `H2_BOOT_GFX_BETWEEN=N` | with in-between frames on in a headless `H2_BOOT_GFX` run (`crates/extensions/game/src/d3d_glue.rs`, `BetweenShots`): write the first N in-between frames as `frame-NNNNN-between-KK.png`, and the guest frames shown meanwhile as `frame-NNNNN.png`; ignored without in-between frames ([high-framerate.md](../game/high-framerate.md)) |
| `H2_BOOT_GFX_SIZE=WxH` | exact PNG size for [headless display clips](agent-api.md#rendercapture_clip), from a centered aspect-ratio crop of the GPU view. Unset uses the view's size. Both dimensions must fit the view at `H2_RENDER_SCALE` and the GPU's texture limit (`crates/extensions/game/src/d3d_glue/clip_png.rs`). |
| `H2_GFX_MERGE_CHECK`, `H2_D3D_WRITEBACK_TRACE`, `H2_RENDER_THREAD_STATS`, `H2_RENDER_THREAD=0`, `frame-scene.census` | render diagnostics; `H2_RENDER_THREAD=0` is the synchronous reference; `frame-scene.census` is the entry census `render.scene` reports ([framescene.md](../graphics/framescene.md), slice 4) and places `render.residual`'s producer observers ([retire-d3d.md](../graphics/retire-d3d.md), slice R0) |
| `frame-scene.layers` | on with the native renderer; the scene's layers 1, 5, 7, 8 and 20 draw their clusters' and placements' static items from the scene's plan under the device's stencil, the rest on the D3D path; `=off` leaves every item to the D3D path; outcomes in `render.scene` as `layer_plan_*` ([framescene.md](../graphics/framescene.md), slice 4) |
| `frame-scene.sink` | off. Records carried guest work by view and slot. With `frame-scene.census`, the original renderer presents every frame and the sink records target and readback history. With the native frame step enabled and guest rendering on the game thread, a GPU display supplies sealed scenes for private draws. The headless lighting A/B uses its own pair of views. A display with a render thread and interpolation off can present eligible frames directly at 1x or through its single copy at a uniform integer render scale; other resolutions keep the original renderer. [FrameScene slice 6](../graphics/framescene.md#slice-6-the-renderer-owns-game-view-frames) |
| `H2_PIPELINE_CACHE`, `H2_GFX_COMPILE_THREADS`, `H2_VERTEX_CACHE_MB`, `H2_SMOOTH_TEXT`, `H2_RENDER_OVERRIDES` | D3D pipeline cache and sibling native descriptor archive, D3D compile workers, vertex cache budget, glyph smoothing, hot render overrides |
| `WGPU_BACKEND` | the wgpu backends a window, headless GPU context, tool or GPU test may use, a comma list (`vulkan`, `metal`, `dx12`, `gl`); unset, by `gfx::GpuHost::backends`: Vulkan on Linux and other Unix, every backend on Apple and Windows. The UWP app (`GpuHost::WindowsApp`) ignores it and uses DX12 alone (`gfx::instance_for`, [linux.md](linux.md#desktop-build)) |
| `H2_PREWARM_PROGRAMS` | most D3D programs the pipeline cache prewarms at startup and at each map load, most recently seen first; unset or not a number: 1024 (`PREWARM_PROGRAMS`); `0` or `off`: none ([Persisted keys, prewarmed](../graphics/graphics.md)) |
| `H2_LOCKSTEP*` | Lockstep settings |
| `H2_INPUT_RECORD`, `H2_INPUT_REPLAY` | Record or replay the guest's input reads. A boot with either tap excludes editor object edits and map reloads. [Input](agent-api.md#input-act-like-a-player) |
| `H2_FILM_OUT`, `H2_FILM_DIR` | Recording destination and film library location ([film format](../game/film-format.md)). |
| `H2_THEATER_MEASURE=1` | enables [Theater measurements](agent-api.md#theater-transport) on the recorder or player, including guest write-watch counts during recording |
| `H2_TELEMETRY=0`, `H2_TELEMETRY_FILE` | Disable telemetry or write a local capture (`crates/live/live-telemetry/src/config.rs`). |
| `H2_BACKGROUND_THREADS` | how many threads one background job (including nav, visibility and native pipeline builds) splits across; unset, `0` or not a number: a quarter of the cores, at least one; at most the cores ([background-work.md](background-work.md)) |
| `H2_BOTS_MAP_INLINE` | `1`: a map's bot data (nav mesh, walls, visibility grid) is read on the game thread before the frame's pads instead of on a worker, so the tick the bots first have it does not depend on the host's load. `h2 sim` sets it (`crates/extensions/game/src/bot_glue.rs`, `MAP_INLINE_ENV`) |
| `H2_BOT_THREADS` | Bot think workers: a count, `auto` (one fewer than the cores, at most 8), or unset, `0`, `off` for serial. Batched pads equal serial pads. `h2 sim run` sets it from the match's `bot_threads`, else to `min(phantoms, cores - 1)`; a variable already set wins, and `h2 sim batch` children stay serial (`crates/live/bots/src/think.rs`, `THREADS_ENV`; `crates/apps/cli/src/sim_cmd.rs`, `think_env`; [sim.md](sim.md#bot-thinking-across-cores)) |
| `H2_BOT_PIPELINE` | `1`, `on`, `true` or `yes`: the thinks overlap the guest's tick and their pads are the next tick's, a different but repeatable match. `h2 sim run` sets it from the match's `bot_pipeline`; a variable already set wins (`think.rs`, `PIPELINE_ENV`) |
| `H2_PHANTOM_BUILD_CHECK=1` | builds every phantom seat's action on both paths, the alias on live RAM and the copy-on-write view, keeps the `impl.sim.phantom-alias` path's result, and counts the seats whose outcome, action bytes or history differ in the sim report's `wall.phantom_build_mismatches` (`crates/extensions/game/src/bot_glue/retail_input.rs`, `check_mode`; [sim-phantom-build.md](../perf/sim-phantom-build.md)). A diagnostic: every seat costs both builds |
| `H2_SIM_CACHE_TEMPLATE=0` | `0`, `off`, `false` or `no`: `h2 sim batch` runs start with an empty Z: and an unseeded N:, and copy every map (`shared`, `single_player_shared`, the main menu, the match's map) from the disc, instead of cloning the image's template (`crates/apps/cli/src/sim_cmd/cache_template.rs`, [Sim batches: a warm Z:](cache-slots.md#sim-batches-a-warm-z), [a warm N:](cache-slots.md#sim-batches-a-warm-n)) |
| `H2_SIM_CACHE_SEED` | set by `h2 sim batch` on each child: the template's N: slot directories, joined as `PATH` is. On a light host the VFS clones them into N: straight after the boot's format (`vfs::CACHE_SEED_ENV`, `vfs::Config::cache_seed`, [a warm N:](cache-slots.md#sim-batches-a-warm-n)); not for users |
| `map-copy.lobby` | on for light hosts (sims and dedicated hosts), off elsewhere: the main loop's background map copy (the call returning to `0x12b571`, the one the System Link lobby waits on) runs up to 16 steps a frame, the same calls in the same order. `H2_FEATURES=-map-copy.lobby` is the A/B arm (`crates/extensions/game/src/map_copy.rs`, [Copy speed on the loading screen](cache-slots.md#copy-speed-on-the-loading-screen)) |
| `H2_BOTS_NAV_AUTO` | `0`, `off`, `false` or `no`: the game builds no bot nav meshes or visibility grids by itself; unset or anything else: on (`crates/live/bots/src/autonav.rs`, `ENV`) |
| `H2_WATERMARK`, `H2_DEBUG_MENU`, `H2_SNAPSHOT_PROBE`, `H2_UI_WATCH`, `H2_UI_PREVIEW_DIR`, `H2_UI_SHEET_DIR`, `H2_SKULLS_REVEAL` | snapshot, UI and play-test aids |
| `LIVE_MAX_HOSTS` | concurrent dedicated leases, default 4. Zero disables admission. Accepted caps are 0–4, 16, 64, 256 and 1,024. The Worker derives per-class container limits at deploy time ([capacity steps](../live/host-halving.md#caps)) |
| `h2 host run --pack-control IP:PORT` | private supervisor address supplied to the packed child to read its usage at match end ([packed hosts](../live/dedicated-host.md#packed-hosts)) |
| `h2 live invite grant <gamertag> [--open-cap N]` | grants invite access, optionally limiting a non-admin's open invites ([Live admins](../live/live-admin.md#5-the-dashboards-server-screen-and-admin-invites)) |
| `h2 live invite ungrant <gamertag>` | removes the explicit grant and closes a non-admin's open invites |
| `H2_LIVE_SCREENSHOT_CLASSIFIER_URL` | optional native screenshot classifier at a literal loopback HTTP address. Unset holds captures for review when no classifier is installed ([Screenshot review](../live/live-admin.md#screenshot-review)) |
| `H2_NO_COOP_ROW` | If present, hides the co-op row (`crates/apps/boot/src/registration.rs`). |
| `H2_HOST_*`, `H2_LEASE`, `H2_TOKEN`, `H2_LIVE`, `H2_DATA_*` | Dedicated host settings, leased credentials and bundle selection (`crates/apps/cli/src/host_cmd`, [host build](../live/dedicated-host-build.md)). |
| `H2_IMAGE` (`h2 host run --out`) | the game module directory a dedicated host runs. Unset: the verified host-data bundle's `image/`; the host stops with a named error when the bundle has none or its stamp does not match this `h2` ([The game module](../live/dedicated-host-build.md#the-game-module)) |
| `H2_HOST_COVERAGE` | `deploy/host/build.sh --module`: the coverage list for a slim module ([slim host](../live/dedicated-host-build.md#slim-host)) |
| `H2_LIVE_*` | Live account, server and storage settings (`crates/apps/cli/src/play_live.rs`, `crates/live/live-server/src/standalone.rs`, `crates/live/live-worker/src/settings.rs`). |
| `H2_XNET_*`, `H2_COOP_*` | XNet endpoint and network diagnostics, co-op overrides and test controls (`crates/live/live-hle/src/network_config.rs`, `crates/extensions/game/src/coop_live.rs`, `crates/apps/cli/src/coop_test_cmd.rs`). |
| `H2_DATA`, `H2_DISC`, `H2_XBE`, `H2_BENCH_*` | Data directory, local disc and executable inputs, and parameters for individual test or benchmark harnesses. |

## Boot runner

The environment the boot runner reads ([BOOT.md](BOOT.md)).

| Switch | What |
|---|---|
| `H2_FASTMEM=0` | **On by default.** Generated code does inline RAM checks/copies; unsupported accesses use Rust. `0` compiles without them; set it for both `h2 build` and `h2 run`/`play`, since changing compiler flags invalidates the dylib stamp. This is the gated inline path, not fault-driven fastmem. |
| `H2_X87_NATIVE=0` | **On by default.** Generated code runs range-guarded x87 arithmetic in native binary64 (see [x87-native.md](../lifter/x87-native.md)). `0` keeps every operation on the `h2x87_v1_*` helpers. A compiler flag: part of the dylib stamp and object cache keys, so set it for both build and run. |
| `H2_SSE_NATIVE=0` | **On by default.** Generated code runs guarded SSE add, subtract, multiply, divide and square root in native SIMD ([sse-native.md](../lifter/sse-native.md)). `0` uses the integer `h2s_*` helpers for every operation. A compiler flag, like `H2_X87_NATIVE`: set it for both build and run. |
| `H2_ACCESS_COUNT=0` | **On by default.** Generated handlers count their own guest memory accesses and charge them in bulk (see crates/recompiler/emit/RUNTIME-CONTRACT.md). `0` lets the runtime charge each accessor call; `block` selects the block clock, which counts planned block costs instead of accesses ([block-clock-imprecise-faults.md](block-clock-imprecise-faults.md)). It changes the C: part of the dylib stamp and the emit cache key, as `H2_PASSES` is. |
| `H2_BOOT_VERBOSE=1` | log boot notes, fallback analysis and cache diagnostics |
| `H2_BOOT_GFX=<dir>` | run D3D on a headless `GfxBackend` with the nv2a shaders and write presented frames to `<dir>/frame-NNNNN.png` |
| `H2_BOOT_UPLOAD_ALWAYS` | Any value, even `0`, counts as set. D3D re-reads every resource on every use, instead of trusting the write barrier (`UploadPolicy::Tracked`) |
| `H2_BOOT_GFX_EVERY=N` | with `H2_BOOT_GFX`, keep the first frame and every Nth (default 30); `never` renders on the GPU but writes no PNGs |
| `H2_BOOT_STOP_AT_PRESENT=N` | end the run once N frames are presented |
| `H2_BOOT_FPS_FROM=N` | measure the report's fps window from present N (default: the last 100) |
| `H2_BOOT_REALTIME=1` | pace the scheduler to wall-clock time (`Pacer::real_time`); off by default, so runs go as fast as the host allows |
| `H2_BOOT_CACHE=<dir>` or `off` | the boot cache (default `$TMPDIR/h2boot-cache`), shared by every `--out`: the fallback's unread jump-table targets, and the image's analysis (`analysis-<hash>-<size>-<analyzer>.h2a`), keyed on the XBE hash and size and on `ANALYSIS_SOURCE_HASH` (xbe and analyze sources, the `Cargo.lock` entries they reach, rustc version). `h2 build` writes the analysis and the fallback reads it at run time; `H2_TRACE` skips it. `off` caches nothing |
| `H2_BOOT_INPUT=BUTTONS@WHEN,...` | scripted pad timeline through the keyboard pad and XInputGetState. BUTTONS: `A B X Y BLACK WHITE LT RT UP DOWN LEFT RIGHT START BACK LS RS LUP LDOWN LLEFT LRIGHT` or a key name, joined with `\|`. WHEN: `T`, `T+D` or `T-U`, a slice or (`f` prefix) a present count; held 4 presents or 100 slices by default. Each press is printed. |
| `H2_BOOT_INPUT_FILE=<file>` | the same entries, one per line, `#` comments; merged with `H2_BOOT_INPUT` |
| `H2_BOOT_CAPTURE=<dir>`, `H2_BOOT_CAPTURE_FRAME=N` | record frame N for offline replay (see `crates/extensions/game/src/d3d_diag.rs`): every texture upload, surface, guest write, and the frame's `ResolvedDraw`s with their streams |
| `H2_BOOT_CAPTURE_SNAP=K[-L],...` | with a capture, read the colour target back after those draws of frame N (`snap-K.bin`, guest BGRA bytes) |
| `H2_BOOT_CAPTURE_SKIP=K[-L],...` | with a capture, drop those draws of frame N |
| `H2_BOOT_CAPTURE_LITE=1` | with a capture, record only the frame's draws, surfaces and snapshots (no uploads or guest writes); for snapshots |
| `H2_BOOT_CAPTURE_TEX=K[-L],...` | with a lite capture, still keep these texture uploads' bytes (`tex-K-L.bin`; ids from `textures.txt` and `frame.txt`, stable across `--fresh-data` runs) |
| `H2_BOOT_DRAWLOG=<file>` | log every backend call of the frames in `H2_BOOT_DRAWLOG_FRAMES` (present counts, default `240`; frame N matches `frame-N.png`) |
| `H2_BOOT_DRAWLOG_VERTS=1` | with `H2_BOOT_DRAWLOG`, add the vertex layout and the first 8 vertices of each stream |

## Profiling

`perf::Session::from_env` reads these ([profiling.md](../perf/profiling.md)).

| Switch | What |
|---|---|
| `H2_PROFILE=<dir>` | sample the calling thread, write the profile there |
| `H2_PROFILE_HZ` | rate (default 1000) |
| `H2_PROFILE_TOP` | rows per table (default 25) |
| `H2_PROFILE_CPU=1` | on-CPU samples only |
| `H2_TIMELINE=<file.json>` | record a timeline and write it there |
| `H2_TIMELINE_CATS` | categories, e.g. `all`, `default`, `frame,d3d,gfx`, `all,-hle` |

## Live Worker settings

Worker variables (`[vars]` in wrangler.toml) and secrets. The names are `h2-live-server`'s environment names where it has them (`crates/live/live-worker/src/settings.rs`; [live-worker.md](../live/live-worker.md#settings)).

| Switch | What |
|---|---|
| `H2_LIVE_ID` | Worker variable. The server id: the host in every author id. It must stay what the machine's server had. |
| `H2_LIVE_SERVER_KEY` | Worker secret. The server's key, base64 as in `server.key`. |
| `H2_LIVE_PUBLIC_URL` | Worker variable. The URL clients use, shown in discovery. Default `https://<id>`. |
| `H2_LIVE_ADMINS` | Worker variable. Admin gamertags, comma-separated. |
| `H2_LIVE_INVITE_ONLY` | Worker variable. `1`: account creation needs an invite. |
| `H2_LIVE_PRIVATE` | Worker variable. `1`: reads need a token too. |
| `H2_LIVE_BUILD` | Worker variable. The deployed commit, set by `deploy.sh` with `--var`. `/healthz` sends it as the `x-h2-build` header and the deploy check compares it. Unset on a deploy that bypasses the script. |
| `H2_LIVE_FORCE_CONTAINERS` | `deploy.sh`. `1`: roll the host container apps out even when the host image tag is unchanged (needed for a new `LIVE_MAX_HOSTS` or class change). |
| `H2_LIVE_GAME_ART_URL` | Worker variable. Published game art, `<public bucket URL>/art/<stamp>` (what `h2 live assets publish` prints). `/art/game/*` redirects there and `/art/emblem/*` reads its manifest and masks. With it unset, both paths return 404 and the web app uses its own art pack. |

## Cross-build target directories

The cross builds use Cargo target directories of their own, because `RUSTC_BOOTSTRAP` and `-Zbuild-std` would otherwise invalidate the host build's artifacts ([building.md](building.md)).

| Switch | What |
|---|---|
| `H2_APPLE_TARGET_DIR` | the `h2host` runtime cross-built for tvOS or iOS (`platform/apple/build.sh`). Default `target/<tvos\|ios>-cross`. |
| `H2_APPLE_HOST_TARGET_DIR` | the host-side `h2` and packer that `build.sh` runs for the disc subset and game module. Default `target/apple-host-pack`. |
| `H2_WINDOWS_TARGET_DIR` | the Windows cross build (`platform/windows/env.sh`, so `windows-dll` and `check-windows`). Default `target/windows-cross`. |

## DualSense speaker and mute

Set, each locks its row in the Settings menu ([playstation-audio.md](../audio/playstation-audio.md)).

| Switch | What |
|---|---|
| `H2_PAD_SPEAKER` | `0` off, `1` on with defaults, otherwise a TOML file of the keys below (on unless it says `enabled = false`). Set, it locks the in-game row (below). Default: unset: the in-game row decides, on until turned off. |
| `H2_PAD_SPEAKER_DEVICE` | Output device name (substring, case-insensitive); `auto` tries DualSense, then a DS4, then a Steam Controller's trackpad haptics when one is connected; `haptics` (or `steam`) asks for those alone ([steam-controller.md](../input/steam-controller.md)). Default: `auto`. |
| `H2_PAD_SPEAKER_EXCLUSIVE` | `1` plays routed sounds only on the controller, `0` on both. Default: `1`. |
| `H2_PAD_MUTE` | `1` the mute button toggles mute and its LED, `0` leaves the button alone. Set, it locks the Voice screen's Mute Button row (`[pad] mute_button`; a row press reaches the HID thread at once through `PadHid::mute_switch`: on sets the LED for the current state, off unmutes and puts the LED out). Default: `1` (acts once a DualSense is found). |

## Build metadata and platform scripts

Set by the platform scripts, the justfile and CI; the game reads none of them.

| Switch | What |
|---|---|
| `H2_BUILD_SHA`, `H2_BUILD_BRANCH`, `H2_BUILD_UNIX` | Build identity shown in the watermark and the lab host's report. `H2_BUILD_SHA` and `H2_BUILD_BRANCH` override what `crates/apps/app/build.rs` reads from git; `H2_BUILD_UNIX` is the build time it records. |
| `H2_ISO` | `just iso=PATH` default: the disc image, else `H2_DISC`, else `disc/halo2.iso` (`justfile`). |
| `H2_BIN` | An existing `h2` binary for `deploy/host/build.sh` and `scripts/h1-parity.sh`, instead of building one. |
| `H2_OPT_VALUE` | The `H2_OPT` a Buck module rule exports to the build (`platform/linux/BUCK`). |
| `H2_MAKEMSIX`, `H2_XBSYMDB` | Paths to the `makemsix` tool (`scripts/doctor.sh`) and the XbSymbolDatabase checkout (`crates/recompiler/symbols/src/lib.rs`, default `.tools/XbSymbolDatabase`). |
| `H2_APPLE_TARGET_DIR`, `H2_APPLE_HOST_TARGET_DIR`, `H2_WINDOWS_TARGET_DIR` | Cargo target directories for the tvOS and iOS cross build, its host-side packer, and the Windows cross build ([building.md](building.md)). |
| `H2_XBOX` | The console's address for the `just` Xbox recipes. |
| `H2_XBOX_OUT`, `H2_XBOX_TARGET_DIR`, `H2_XBOX_HOST_TARGET_DIR`, `H2_XBOX_KEYS`, `H2_XBOX_BUILD_DRY_RUN`, `H2_XBOX_API_CHECK` | `platform/xbox/build.sh`: package output directory, the two Cargo target directories, the signing key directory, a dry run that prints the plan, and `warn` to package despite imports missing on the console. |
| `H2_XBOX_CER`, `H2_XBOX_PACKAGE_NAME`, `H2_XBOX_USER` | `platform/xbox/deploy.sh` and `xbox.sh`: the certificate to install, the package name (default `h2recomp.Halo2`) and the Device Portal user. |
| `H2_XBOX_AGENT_CMD`, `H2_XBOX_SYMBOLIZE_CMD`, `H2_XBOX_SYMBOLS`, `H2_XBOX_PROFILE_DIR` | `xbox.sh profile`: the agent and symbolizer commands, the symbol directory and where profiles are written. |
| `H2_WORKER_HOME` | The Worker toolchain's home for `crates/live/live-worker/deploy.sh` and the deploy workflow (default `~/.cache/h2-live-worker`). |
| `H2_CLOUDFLARED` | The `cloudflared` binary the tunnel helper starts (`crates/apps/cli/src/tunnel.rs`); default `cloudflared` on `PATH`. |

## Dedicated host, theater and sim plumbing

Variables one `h2` process sets for a child it starts. They are listed so a
person reading a process environment can tell what set them.

| Switch | What |
|---|---|
| `H2_PACK`, `H2_REGION` | `deploy/host/entrypoint.sh`: `1` starts the pack supervisor instead of a single host; the region passed to it as `--region`. |
| `H2_PACK_CONTROL`, `H2_BASE_PORT`, `H2_PACK_DATA`, `H2_PACK_AGENT_PORT`, `H2_TUNNEL_LISTEN` | Environment form of the `h2 host pack` flags `--control-listen`, `--base-port`, `--data`, `--agent-port` and `--tunnel-listen`; a flag wins (`crates/apps/cli/src/host_cmd/pack.rs`). |
| `H2_PUBLIC_ADDR` | The public IPv4 address or `ADDR:PORT` the host advertises for XNet (`crates/apps/cli/src/host_cmd/process.rs`). |
| `H2_TUNNEL_URL` | The public tunnel URL the supervisor passes to a packed child. The theater child drops both tunnel variables. |
| `H2_FILM_GAME_ID`, `H2_FILM_PENDING_RECEIPT` | A dedicated host child's game id and the path of its pending film receipt (`crates/apps/cli/src/host_cmd/process.rs`). |
| `H2_THEATER_WORK`, `H2_THEATER_VIEW`, `H2_THEATER_RANGE`, `H2_THEATER_FILM_ID` | The watching Theater process starts a clip or replay child with a private work directory, a JSON initial view, a `start:end` tick range and the film id (`crates/apps/cli/src/theater_cmd.rs`). |
| `H2_SIM_BATCH_ID`, `H2_SIM_BATCH_LOCKS` | `h2 sim batch` tells each child its batch id and the lock files its slots hold (`crates/apps/cli/src/sim_cmd.rs`). |
| `H2_SIM_REPORT` | Path the sim's report is written to when a run finishes (`crates/live/sim-bench/src/lib.rs`). |
| `H2_SIM_STORE`, `H2_SIM_STORE_RECORD` | A directory of verified map bytes served instead of the disc, and a file that records the bytes a run's guest read to build one (`crates/runtime/vfs/src/sim_store.rs`, `h2 sim store`). |
| `H2_SERVER_MAP_DIR` | Where a light host reads server-side map overrides (`crates/apps/boot/src/lib.rs`). |
| `H2_SNAPSHOTS` | Snapshot store root; default `snapshots` in the cache directory (`crates/runtime/snapshot/src/store.rs`). |

## Game overrides and diagnostics

| Switch | What |
|---|---|
| `H2_HALO1_SANDBOX`, `H2_HALO1_ASSETS`, `H2_HALO1_HUD` | The Halo 1 options' environment names; each takes `halo1` or `halo2` ([campaign.md](../halo1/campaign.md)). |
| `H2_HEALTH_MODEL` | `halo1` or `halo2`: debug override of the health model chosen per map (`crates/extensions/game/src/health.rs`). |
| `H2_BOT_AIM_MODEL` | `code` or `net`: the aim model for bots whose tuning names none; anything else keeps `code` (`crates/live/bots/src/tuning.rs`). |
| `H2_PHANTOM_LOBBY` | `0`: phantom bots join only a running game and leave when it ends, not the lobby ([bots.md](../game/bots.md)). |
| `H2_RENDER_CENSUS_EVERY`, `H2_RENDER_CENSUS_TOP` | Render census report interval in frames (default 300) and rows per list (default 20) (`crates/rewrites/render-bridge/src/render_census.rs`). |
| `H2_EDITOR_PROBE` | With the editor's headless screenshot runner, print the windows and widgets a script can aim at (`crates/apps/studio/src/headless.rs`). |
| `H2_BOOT_DYLIB`, `H2_RETAIL_MODULE` | The generated game module for the ignored `boot` and bot-input tests. |

## Test, bench and disc-backed check inputs

Read only by ignored tests, examples and benches. Most need the disc (`H2_DISC`).

| Switch | What |
|---|---|
| `H2_BENCH_FRAMES` | frames per run (the default differs per bench). Read by: `cargo bench -p d3d` (`pushbuf`, `streams`, `vertex_fetch`) and the d3d benchmark tests (`--ignored`). |
| `H2_BENCH_DRAWS` | draws per frame (defaults 1200, 1000 and 1000 in the benches). Read by: the `pushbuf`, `streams` and `vertex_fetch` benches and some d3d benchmark tests. |
| `H2_BENCH_GUEST_ONLY` | set: run only the guest-memory dirty-tracking benchmark, then exit. Read by: `pushbuf`. |
| `H2_BENCH_MODES` | `cpu` or `gpu`: time one fetch mode only, e.g. to profile it. Read by: `vertex_fetch`. |
| `H2_BENCH_FILLS`, `H2_BENCH_SMALL`, `H2_BENCH_ROUNDS` | fills per visit (4), small draws per visit (0), rounds, the median reported (3). Read by: the ignored `render_churn` test in `crates/platform/d3d/tests/render_churn.rs`, its game-frame section. |
| `H2_DIFF_SEEDS` | comma-separated seeds; the layer test defaults to `1,7,99`, `differential` to its first default seed. Read by: `crates/rewrites/engine/tests/{differential,layer_differential}.rs` (`--features differential`, need `H2_XBE`). |
| `H2_DIFF_SCALE` | a multiplier on the cases per seed (default 1). Read by: `crates/rewrites/engine/tests/differential.rs`. |
| `H2_EMIT_FULL` | `1`: build the whole image through generated C instead of only the functions the entry run lifted (minutes, cached afterwards). Read by: `crates/recompiler/emit/tests/entry.rs` (needs `H2_XBE`). |
| `H2_BENCH_IMMEDIATE`, `H2_BENCH_DYNAMIC`, `H2_BENCH_ONLY`, `H2_BENCH_OUTPUT`, `H2_BENCH_PASSES`, `H2_BENCH_TRACE` | Further knobs of the d3d benches and ignored tests: draws drawn immediate-mode, dynamic vertex buffers, a name filter, an output file, pass count and a per-frame trace (`crates/platform/d3d/benches`, `crates/platform/d3d/tests`). |
| `H2_PERF_ITERS`, `H2_PERF_TRIALS`, `H2_PERF_KIND`, `H2_PERF_MAPS`, `H2_EXECUTOR_ROUNDS` | Emit tests: iterations, trials, kind filter and mapping count of `perf.rs`; rounds of the second-executor test. |
| `H2_CLIF_JOBS`, `H2_ORACLE_FIXTURES`, `H2_ORACLE_PASSES`, `H2_X87_WIDE_CASES` | Compile threads for the Cranelift image test; fixture list (`xtask` sets it) and pass spec for the lift oracle; case count for the wide x87 test (default 200,000 per width). |
| `H2_SCHEMA_MAPS`, `H2_SCHEMA_OUT`, `H2_SCHEMA_PROBE`, `H2_SCHEMA_WHY`, `H2_SCHEMA_ODD`, `H2_SHUFFLE_LAYOUT`, `H2_TRANSPLANT`, `H2_TRANSPLANT_FROM` | `crates/formats/tags/tests/tag_schema.rs`: maps to infer from, schema output file, root-word probe (`hlmt:0x58`), diagnostics, child layout, and the tag to transplant and its source. |
| `H2_DEFS_GROUPS`, `H2_DEFS_MAPS`, `H2_DEFS_SKELETON`, `H2_DEFS_ANNOTATE`, `H2_DEFS_EXTRA`, `H2_DEFS_DUMP`, `H2_DEFS_DUMP_NODES` | `crates/formats/tags/tests/tag_defs.rs`: tag groups and maps to check, skeleton and annotated modes, extra definition files, and dumps. |
| `H2_BUILD_MAPS`, `H2_GROW_TAGS`, `H2_REPO_MAPS`, `H2_REPO_DIR`, `H2_REPO_COUNT_RAW`, `H2_PROBE`, `H2_PROBE_MAP` | Tag-repository and map-build tests: maps to use, extra tags to add (default 500), where to keep the repository, sizing without writing, and BSP root probes. |
| `H2_HS_MAP`, `H2_HS_TICKS` | The disc script-VM test (`crates/formats/hs/tests/vm_disc.rs`): limit the map and set ticks per phase (default 3000). |
| `H2_SAVED_VARIANT` | A game-written saved variant payload to check (`crates/formats/variants/tests/disc.rs`). |
| `H2_MAP`, `H2_MAPS`, `H2_BASE`, `H2_STEMS`, `H2_LEVELS` | Map stems for the Halo 1 converter and asset tests (defaults such as `lockout`, `beavercreek`, `05a_deltaapproach`). |
| `H2_SHADER_CORPUS`, `H2_FONT_HD_PNG` | The extracted shader directory for the pipeline-cost test, and the output directory for the glyph test's 4x images (`crates/platform/d3d/tests`). |
| `H2_TITLE_CHECK`, `H2_PREVIEW_CHECK`, `H2_GEOMETRY_CHECK`, `H2_GEOMETRY_BEFORE`, `H2_LOOKS_MAPS`, `H2_CATALOG_CHECK` | Output directories and map list for the Live art disc checks (`crates/apps/cli/src/live_*.rs`). |
| `H2_ARCHIVE_DUMP`, `H2_V8_DUMP` | A recorded archive or v8 packed-record file for the Live protocol tests and `web/bench`. |
| `H2_UPDATE_GOLDEN` | `1` rewrites the committed golden files of the Live protocol tests. |
| `H2_S3_TEST_ENDPOINT`, `H2_S3_TEST_ACCESS_KEY`, `H2_S3_TEST_SECRET_KEY`, `H2_S3_TEST_BUCKET`, `H2_S3_TEST_REGION` | An S3-compatible bucket for `crates/live/live-server/tests/s3_server.rs`; the test skips without the first four. Region defaults to `auto`. |
| `H2_LEAK`, `H2_TEST_FLAT_FAULT_CHILD` | Sentinels that the lab and flat-RAM tests set to check that the environment does or does not reach a child. |

## Web mock server

`web/mock/server.ts` serves a fake Live service for the web app.

| Switch | What |
|---|---|
| `H2_MOCK_SESSIONS`, `H2_MOCK_HISTORY_MIN`, `H2_MOCK_QUIET` | Open sessions kept (default 5), minutes of history (default 20), and `1` for no new games. |
| `H2_MOCK_STATIC`, `H2_MOCK_TAMPER` | A built app directory to serve (SPA fallback); corrupt every nth event's data to show bad signatures. |
| `H2_MOCK_GAME_ART`, `H2_MOCK_GAME_ART_DIR`, `H2_MOCK_MAP_GEOMETRY`, `H2_MOCK_MODELS`, `H2_MOCK_FURNITURE`, `H2_MOCK_INDOOR` | Game-art manifest (fake, or a real extraction), map geometry (`0` off), stand-in models (`1` or `static`), map furniture (`0` off) and indoor arenas. |
| `H2_MOCK_ACTION`, `H2_MOCK_ACTORS`, `H2_MOCK_ADMIN`, `H2_MOCK_CUSTOM_MAPS`, `H2_MOCK_HOPPER`, `H2_MOCK_RULES`, `H2_MOCK_STATS_ARCHIVE` | Stances, actions and damage (`0` off); AI and vehicles in every game; every signed-up account an admin; custom-map games (`0` none); the playlist stand-in; the rules tools (`0` off); the stats archive (`0` off). |

## Family members

The family rows in the tables here cover these names.

| Family | Members |
|---|---|
| `H2_COOP_*` | `H2_COOP_MAX_PLAYERS`, `H2_COOP_JOIN_IN_PROGRESS`, `H2_COOP_LOOK_PREDICTION` (the co-op settings' env names); `H2_COOP_JIP_LIFT` and `H2_COOP_JIP_LOG` (`0` keeps retail's join-in-progress check, or drops the stage lines); `H2_COOP_JOIN_SCREEN` (`0` leaves joining to the Co-op Campaign row); `H2_COOP_LIST_UI` (`1` opens the list screen from the lobby); `H2_COOP_LATENCY` (`0` drops the latency line); `H2_COOP_AUTO_INVITE`, `H2_COOP_AUTO_ACCEPT`, `H2_COOP_AUTO_ACCEPT_DELAY` (test automation that invites or accepts the named gamertags, the accept held back by the delay in seconds; `H2_COOP_AUTO` is their common prefix); `H2_COOP_PROBE`, `H2_COOP_PROBE_MAP`, `H2_COOP_PROBE_DIFFICULTY` (the retail campaign over System Link probe); `H2_COOP_LOOK_CAPTURE`, `H2_COOP_LOOK_INPUT` (recordings the co-op look tests read) ([live-coop.md](../live/live-coop.md)). |
| `H2_LIVE_*` | Player side: `H2_LIVE_SERVER` (service URL), `H2_LIVE_TOKEN` (admin token for `h2 sim` cloud runs), `H2_LIVE_DIR` (the player's Live data directory), `H2_LIVE_VISIBILITY` (the setting's env name), `H2_LIVE_NAT` and `H2_LIVE_PING_MATCHMAKING` (`1` turns on ping-based matchmaking with the given NAT type), `H2_LIVE_UPLOAD_KBPS` (upload bandwidth hint for it). Server side: `H2_LIVE_DATA` (data directory), `H2_LIVE_DB` (Postgres URL, else SQLite in the data directory), `H2_LIVE_WEB` (the built web app), `H2_LIVE_BLOBS`, `H2_LIVE_PUBLIC_BLOBS`, `H2_LIVE_PUBLIC_BLOBS_URL` (blob stores), `H2_LIVE_S3_ENDPOINT`, `H2_LIVE_S3_ACCESS_KEY_ID`, `H2_LIVE_S3_SECRET_ACCESS_KEY`, `H2_LIVE_S3_REGION` and the `H2_LIVE_S3_PUBLIC_*` twins read first by the public store, `H2_LIVE_ID`, `H2_LIVE_SERVER_KEY`, `H2_LIVE_PUBLIC_URL`, `H2_LIVE_ADMINS`, `H2_LIVE_INVITE_ONLY`, `H2_LIVE_PRIVATE`, `H2_LIVE_GAME_ART_URL`, `H2_LIVE_SCREENSHOT_CLASSIFIER_URL`; tests: `H2_LIVE_TEST_DB`. The Worker's variables are under [Live Worker settings](#live-worker-settings). |
| `H2_HOST_*` | Set by the supervisor on a host child: `H2_HOST_LEASE_JSON`, `H2_HOST_TOKEN`, `H2_HOST_LIVE_URL`, `H2_HOST_ARCHIVE`, `H2_HOST_LOBBY`, `H2_HOST_LOBBY_MACHINE_START`, `H2_HOST_PARENT_PIPE`, `H2_HOST_AGENT_TCP` (the second agent listener's TCP address), `H2_HOST_CACHE` and `H2_HOST_CACHE_MAX_BYTES` (the data cache and its cap in bytes), `H2_HOST_TICK_PHASE_NS`. Build and diagnostics: `H2_HOST_AUDIT` (the light-host audit report path), `H2_HOST_MODULE` (the light host's module list; needs the clang tier and the light host profile), `H2_HOST_COVERAGE`, `H2_HOST_PERF`. |
| `H2_DATA_*` | `H2_DATA_URL` and `H2_DATA_SHA`: the host-data bundle's signed URL and its SHA-256 (`h2 host`). |
| `H2_XNET_*` | `H2_XNET_ADDR` (the XNet bind address, `ADDR:PORT`), `H2_XNET_LOOPBACK_PEERS` (comma list of same-machine `IPv4:port` peers), `H2_XNET_LOWDELAY` (`1`: low-delay UDP sockets). |
| `H2_LOCKSTEP*` | `H2_LOCKSTEP=FILE` hashes guest RAM at the end of every tick into FILE; `H2_LOCKSTEP_TICKS=0` keeps only the present records (`h2 lockstep A B` compares two traces). |
| `H2_BOOT_*` | The headless boot names: `H2_BOOT_GFX`, `_GFX_EVERY`, `_GFX_BETWEEN`, `_GFX_SIZE`, `_GFX_SHADOWS` (also write the first N frames that sample shadows), `_GFX_LIGHTING_AB` (draw a retail and an E3 view of one run), `H2_BOOT_CAPTURE`, `_CAPTURE_FRAME`, `_CAPTURE_SNAP`, `_CAPTURE_SKIP`, `_CAPTURE_LITE`, `_CAPTURE_TEX`, `H2_BOOT_DRAWLOG`, `_DRAWLOG_FRAMES`, `_DRAWLOG_VERTS`, `H2_BOOT_INPUT`, `_INPUT_FILE` ([BOOT.md](BOOT.md)). |
| `H2_PROFILE*`, `H2_TIMELINE*`, `H2_FRAME_STATS*`, `H2_SAMPLE_*` | `H2_PROFILE` (output directory), `_HZ`, `_TOP`, `_CPU`; `H2_TIMELINE` (output file), `_CATS`; `H2_FRAME_STATS`, `_CSV`, `_FROM`; `H2_SAMPLE_DIR`, `H2_SAMPLE_SECONDS` ([profiling.md](../perf/profiling.md)). |
| `H2_PAD_*`, `H2_STEAM_*`, `H2_TOUCH_*`, `H2_VOICE_*` | Setting env names defined in `crates/extensions/game/src/setting_defs.rs`; `H2_PAD_SPEAKER*` and `H2_PAD_MUTE` are in [playstation-audio.md](../audio/playstation-audio.md), `H2_PAD_CHECKPOINT` is a test input. |
| `H2_OBJ_CACHE*` | `H2_OBJ_CACHE` (the shared clang object directory) and `H2_OBJ_CACHE_MAX` (its size cap). |

## Waiting for a measurement

These switches select alternate implementations for comparison. The
third column describes the remaining checks.

| Switch | What | Waiting for |
|---|---|---|
| `H2_FLAT_RAM` | at build, the flat memory model: each guest access is one host instruction in the runtime's window, with a fixup to the helper (module feature 16; macOS and x86-64 Linux only); set it on the run too, or the image recompiles paged. At run time it also makes guest RAM aliasable from the start ([contract-thin-transfers-flat-ram.md](contract-thin-transfers-flat-ram.md), section 4.1, slice F3; [RUNTIME-CONTRACT.md](../../crates/recompiler/emit/RUNTIME-CONTRACT.md), "Flat RAM") | M4 of the contract doc |
| `H2_EMIT_FLAT_RAM`, `H2_FLAT_WRITE_WATCH` (+ `H2_FLAT_STATS_OWNER`) | at build, the opt-in span image instead (`H2_FLAT_SPAN`); with it, `H2_FLAT_RAM` at run time turns the span path and its write watch on or off for that image (RUNTIME-CONTRACT.md, "Opt-in flat RAM and first-write faults") | M0 of the contract doc |
| `H2_FLAT_STATS=1` | Records flat RAM protection statistics (`crates/runtime/runtime/src/flat/protection.rs`). | Flat RAM diagnostics |
| `impl.render.reproject`, `impl.render.reproject_ahead` | reprojection of the world image; carried past N | E1 played at 60 against the replay ([high-framerate.md](../game/high-framerate.md)) |
| `host-light.tick-pacing`, `H2_HOST_TICK_PHASE_NS` | opt-in real-time light-host wake coalescing; the supervisor supplies the slot phase in nanoseconds | four-match tick p99, packet latency and paced hash parity ([host-halving.md](../live/host-halving.md#halving-the-cpu)) |
| `host-light-native` | light hosts opt into native animation, collision and objects together; explicit `impl.sim.native` or leaf settings take precedence | Waterworks and Lockout hash parity and Waterworks median CPU saving ([host-halving.md](../live/host-halving.md#halving-the-cpu)) |
| `impl.sim.status-poll` | on light hosts, the map cache status `0x213f10` answered from its last result while nothing it reads has changed ([cache-slots.md](cache-slots.md#the-status-poll)) | `map_status_differential`, `h2 sim diff` on against off with `impl.sim.arena-hash`, and its timing on Lockout and Waterworks ([sim-10x.md](../perf/sim-10x.md), lane B2) |
| `impl.sim.phantom-alias` | on; the phantoms' action build runs on live RAM through `SecondExecutor::alias`, every store journaled and undone before the seat's gather returns; `-impl.sim.phantom-alias`, and any flat-RAM image, build in a copy-on-write view ([sim-phantom-build.md](../perf/sim-phantom-build.md), lane P1; [second-executor.md](second-executor.md#the-alias-and-its-journal)) | `H2_PHANTOM_BUILD_CHECK=1` over the eight Lockout seeds with no mismatch, `h2 sim diff` on against off with `impl.sim.arena-hash`, and equal film `BTAP` series |
| `impl.sim.native_*` (the individual ports are `crates/rewrites/engine/src/sim_physics`) | native simulation ports | a sim A/B |
| `impl.sim.native.collision` | BSP point and location queries in Rust, with FP observers for vector and instance traces ([collision queries](../engine/collision-queries.md)) | per-map parity, cross-architecture bail counts and timing |
| `impl.sim.native.objects` (`-impl.sim.native` turns it off with the other subsystems) | the object update in Rust, `crates/rewrites/engine/src/sim_objects` ([object-update.md](../engine/object-update.md)): the walks, the per-object updates, the type dispatchers, the wake leaves, function values with their imports, change colours, the damage update (with the tick's `Damage` and `Kill` events), the unit update's shell, movement, control, state, powered seats, timers, dialogue, team, camouflage and light with the unit animation state, the biped update and post-physics update, and the item, scenery, device, machine, crate, creature and projectile updates; the per-function ports at the points its bodies own stand down | run parity on the slices that add bodies ([sim-rewrite-objects.md](../engine/sim-rewrite-objects.md) section 12) |
| `H2_SIGNIN_DIALOG` | the sign-in dialog window | a game run ([ui-builder.md](../game/ui-builder.md)) |
