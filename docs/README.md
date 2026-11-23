# Docs

Every doc in this folder, by area. **Start here** marks the entry point for
each area. A page's own status line says whether it describes built code,
partly built code or a design. Pages are written to the rules in
[meta/docs-style.md](meta/docs-style.md).

[ARCHITECTURE.md](ARCHITECTURE.md) is the entry point for the whole project:
goals, targets, the pipeline and the crate table. Dated reviews of the code
are under [Reviews](#reviews).

[features.md](features.md) lists every feature beyond the retail game with its state and its page, the measured speed numbers and the roadmap.

## Lifter: analysis, lifting and CPU semantics (`lifter/`)

- [LIFTER.md](lifter/LIFTER.md) (**start here**): the lifter: x86 to IR to C11, the runtime contract, the validation plan. Where it and the code differ, the code wins
- [FLOAT.md](lifter/FLOAT.md): what floating point the game uses (measured with `h2 fpstats`) and the correctness-first plan
- [FLOAT-ABI.md](lifter/FLOAT-ABI.md): the x87 IR ops and `h2x87_v1_*` runtime helpers (`crates/recompiler/x87`), #MF included; read when touching x87 lowering
- [x87-native.md](lifter/x87-native.md): the guarded native binary64 x87 path, on by default (`H2_X87_NATIVE=0` disables it): range proof, status bits, fallbacks
- [x87-transcendentals.md](lifter/x87-transcendentals.md): the guarded 128-bit first pass for FYL2X, F2XM1 and the trig ops, and the table-driven FYL2X and F2XM1: guards and rules
- [sse-native.md](lifter/sse-native.md): guarded native SSE arithmetic, on by default (`H2_SSE_NATIVE=0` disables it): eligibility and range proof
- [simd.md](lifter/simd.md): bit-exact SSE1 reference semantics (`crates/recompiler/simd`), its C twin and how it is tested
- [oracle.md](lifter/oracle.md): independent x86 instruction oracle on Unicorn (`crates/recompiler/oracle`)
- [sse-oracle.md](lifter/sse-oracle.md): SSE/MMX results measured on Rosetta 2 (`crates/recompiler/sse-oracle`)
- [cranelift-tier.md](lifter/cranelift-tier.md): Cranelift fast start, the background clang build and the shared module contract
- [dispatch-coverage.md](lifter/dispatch-coverage.md): indirect-dispatch coverage of the campaign's main tables, and how to reproduce it
- [xdk-libraries.md](lifter/xdk-libraries.md): how `h2 symbols` identifies statically linked XDK library code
- [BUILD-CACHE.md](lifter/BUILD-CACHE.md): generated-source and object caches, locking, iteration builds, `H2_EMIT_TARGET`; read before changing the build
- Notes: [build-perf.md](lifter/notes/build-perf.md) (build-time measurements behind the caches), [fp-census.md](lifter/notes/fp-census.md) (floating-point census of the retail image), [not-adopted.md](lifter/notes/not-adopted.md) (lifter experiments that lost), [precedents.md](lifter/notes/precedents.md) (public lifter projects read), [x87-native.md](lifter/notes/x87-native.md) (guarded native x87 measurements), [x87-transcendentals.md](lifter/notes/x87-transcendentals.md) (guarded transcendental measurements)

## Runtime: booting and running the game (`runtime/`)

- [BOOT.md](runtime/BOOT.md) (**start here**): `h2 build` and `h2 run`, the boot runner's flags and environment, stopgaps, current state
- [switches.md](runtime/switches.md): every env var and `H2_FEATURES` switch left, one line each, and the experiments still waiting for a measurement
- [cli-tools.md](runtime/cli-tools.md): `h2-tools`, the offline map, tag and script tools in a binary without the game; `h2 <tool>` runs it, `cargo h2-tools` builds only it
- [building.md](runtime/building.md): the root `justfile`: building every target (Mac, Apple TV, iPhone, Windows, Xbox) from your disc, `just doctor`, `maps=demo|all`
- [app.md](runtime/app.md): the host shell, `h2 play`, `h2 play --follow` and `h2 play --watch` (`crates/apps/app`): flags, pacing, frame stats, embedded hosts (tvOS, iOS)
- [level-boot.md](runtime/level-boot.md): `h2 play --level` / `H2_LEVEL`: start a campaign level or a local multiplayer game directly
- [kernel-imports.md](runtime/kernel-imports.md): work list of every kernel import Halo 2 uses, with signatures
- [xapi.md](runtime/xapi.md): which XAPI services are replaced (`crates/platform/xapi`) and which stay generated, native saves included
- [storage.md](runtime/storage.md): the Xbox storage boundary (`crates/runtime/vfs`): volumes, FATX names, file semantics, map overrides
- [background-work.md](runtime/background-work.md): work nothing on the frame waits for (nav meshes, film hashes, uploads) on threads below the game's priority, and the worker cap (`crates/runtime/background`, `H2_BACKGROUND_THREADS`)
- [agent-api.md](runtime/agent-api.md): the agent API to the running game (`h2 serve`, `h2 agent`): UI and world views, profiling by owner, snapshots, input, overrides, sim matches and tick traces
- [debug-menu.md](runtime/debug-menu.md): our in-game debug menu: an overlay over the game image, in the 8x8 font, over the same registries as `h2 agent` (built and unit tested, not yet seen in the game)
- [contract-thin-transfers-flat-ram.md](runtime/contract-thin-transfers-flat-ram.md): spec for the first recompiler-contract change: calls and host calls that stay off the runner, and one-instruction guest memory with a fixup table and card marking; why the opt-in flat RAM is slower; slices, gates and measurements (design only)
- [block-clock-imprecise-faults.md](runtime/block-clock-imprecise-faults.md): the fault census (`profile.faults`: which guest memory faults need a precise exit), the block clock (the default; the scheduler counts planned block costs, not accesses) and imprecise memory faults (the default)
- [lab.md](runtime/lab.md): `h2 lab`: start levels on a second Mac over the LAN, build branches there, replay input recordings, fetch frame stats, profiles and logs
- The [agent API skill](../.claude/skills/h2-agent-api/SKILL.md) and [lab skill](../.claude/skills/h2-lab/SKILL.md) guide coding agents through live checks and remote runs.
- [checkpoints.md](runtime/checkpoints.md): the checkpoint save and revert acceptance test
- [sim.md](runtime/sim.md): headless bot matches, tuning comparisons and uploads to Clanker Training in Uplink
- [bounce-search.md](runtime/bounce-search.md): the forked bounce search: superbounce trials from a saved machine, the region that bounces on each seam and the seams ranked by ease
- [snapshots.md](runtime/snapshots.md): machine snapshots for `state.save`/`state.load`: what state lives outside guest RAM, the file format, the quiescence rule
- [cache-slots.md](runtime/cache-slots.md): the campaign cache-slot integration test
- [tvos.md](runtime/tvos.md): building and deploying to Apple TV (`platform/tvos`, `H2_EMIT_TARGET=tvos`)
- [windows.md](runtime/windows.md): cross-building the runtime and the game module as a Windows DLL from a Mac (`x86_64-pc-windows-msvc`)
- [linux.md](runtime/linux.md): the Linux desktop `h2 play` (Buck2 build, Vulkan, XDG data, a prebuilt module on a machine without clang) and the Linux x86-64 game module (`libh2game.so`) the dedicated host runs
- [steamos.md](runtime/steamos.md): playing on Linux and on a Steam Machine in Developer Mode through Buck (`//platform/linux:play`, `//platform/steamos:run`), the disc from `.buckconfig.local`
- [xbox.md](runtime/xbox.md): the UWP app for an Xbox in Dev Mode, packaged, signed and deployed from a Mac (`platform/xbox`, `H2_EMIT_TARGET=windows`)
- [title-updates.md](runtime/title-updates.md): design for title-update XBEs (1.1 and later): what assumes the 1.0 image, the address-map plan; design only, the one built part is `h2 import-mappack` copying the Map Pack's update aside
- [second-executor.md](runtime/second-executor.md): a second guest executor, the runtime building block the phantoms' action build runs on
- [ios.md](runtime/ios.md): the iOS app: building, deploying and the touch controls (`platform/ios`, `input::touch`)
- Notes: [boot-log.md](runtime/notes/boot-log.md) (the boot milestone log), [sim.md](runtime/notes/sim.md) (sim mode's speed-up estimate and per-process memory), [snapshots.md](runtime/notes/snapshots.md) (snapshot quiescence probe runs), [steamos-boot.md](runtime/notes/steamos-boot.md) (the first Steam Machine boot)

## Development (`dev/`)

- [buck.md](dev/buck.md): the Buck2 build on Linux: the devenv shell, `.buckconfig.local`, the generated BUCK files (`scripts/update-buck.py`), Reindeer fixups, the SteamOS sysroot mode
- [build-speed.md](dev/build-speed.md): the Rust build and test loop: the dev profile, one test binary per crate (`tests/it.rs`), building many worktrees (target directories, the cache), `cargo xtask` (the affected packages, the test ledger, one nextest run per branch); the measurement plan and what is left
- Notes: [2026-11-20-nightly-rust.md](dev/notes/2026-11-20-nightly-rust.md) (nightly Rust, Cranelift and `-Zthreads` timed against stable), [not-adopted.md](dev/notes/not-adopted.md) (developer tooling experiments that lost)

## Input (`input/`)

- [input.md](input/input.md) (**start here**): XInput replacement, device model, backends, bindings and input timelines (`crates/platform/input`)
- [input-default.toml](input/input-default.toml): the default binding file; [input-menu-example.toml](input/input-menu-example.toml): an example input timeline
- [host-keyboard.md](input/host-keyboard.md): host typing in the game's virtual keyboard
- [controller-layouts.md](input/controller-layouts.md): the retail button layouts and the extra ones we add
- [button-icons.md](input/button-icons.md): button glyphs that follow the device in use (Xbox 360, PlayStation, keyboard; `H2_ICONS`)
- [steam-controller.md](input/steam-controller.md): Steam Controllers over raw HID, trackpad aim and haptics (`H2_STEAM_*`)
- [keyboard-charset.md](input/keyboard-charset.md): design for extending the on-screen keyboard's characters (not built)

## Graphics (`graphics/`)

- [graphics.md](graphics/graphics.md) (**start here**): D3D8 state and textures on wgpu (`crates/platform/gfx`): batching, merged passes, the pipeline cache
- [d3d.md](graphics/d3d.md): the guest-facing Direct3D 8 replacement (`crates/platform/d3d`): entries, device model, push buffer, render targets, vblank
- [framescene.md](graphics/framescene.md): design for `FrameScene` and a native renderer that owns the frame: the snapshot type, threads, 60/120 fps from two snapshots, the order D3D is cut and deleted, slices with tests and gates
- [retire-d3d.md](graphics/retire-d3d.md): design for retiring the guest D3D path: what the native stages leave on D3D by caller, the residual census (`render.residual`), what replaces each guest render subsystem, the slices and the zero-residual gate before `crates/platform/d3d` is deleted
- [shaders.md](graphics/shaders.md): NV2A vertex microcode, register combiners, fog and shadow maps to WGSL (`crates/formats/shaders`)
- [render-thread.md](graphics/render-thread.md): the render thread (on by default, `H2_RENDER_THREAD=0` turns it off): what the guest can observe
- [render-loop.md](graphics/render-loop.md): the view and pass loops of the retail renderer read, and the render-state list ported to Rust
- [occlusion-culling.md](graphics/occlusion-culling.md): how a view's objects are chosen, the per-object skip point, and the game's W-buffer scale
- [layer-draw.md](graphics/layer-draw.md): the layer draw and its entry loop, read and ported, with the `render-timing` breakdown
- [hot-iteration.md](graphics/hot-iteration.md): live tag edits (`edit.*`) and hot WGSL overrides, parameters and toggles for the host renderer (`render.*`), without a rebuild or restart
- [high-resolution.md](graphics/high-resolution.md): native and fixed output sizes, per-axis raster scaling, and widescreen projection
- [native-renderer.md](graphics/native-renderer.md): the native GPU renderer (`impl.render.native`, `crates/rewrites/native-render`): the frame, the stages and how a frame is checked
- [native-world.md](graphics/native-world.md): native renderer stage 1: the static world
- [native-objects.md](graphics/native-objects.md): native renderer stage 2: objects and skinning
- [native-decorators.md](graphics/native-decorators.md): native decorators: the BSP's model decorators typed into the scene and drawn instanced
- [native-transparents.md](graphics/native-transparents.md): native renderer stage 3: transparents
- [native-lights.md](graphics/native-lights.md): native renderer stage 4: lights and shadows
- [native-water-fog.md](graphics/native-water-fog.md): native renderer stage 4b: water, fog and camouflage
- [native-post-hud.md](graphics/native-post-hud.md): native renderer stage 5: post and HUD
- [native-shadows.md](graphics/native-shadows.md): native shadows: caster enumeration, projection, and the interface to the light graph
- [native-shadow-layers.md](graphics/native-shadow-layers.md): shadow layers 10, 11 and 14 drawn from the scene's gathered groups (design): the compiler from tags, the raster state, the guest targets and the object filter, the slices and their checks
- [native-gels.md](graphics/native-gels.md): native shader gel quads through `DrawAdmission`
- [native-particles.md](graphics/native-particles.md): native particles, contrails, beams and the other immediate effects
- [native-decals-research.md](graphics/native-decals-research.md): native decals: the dynamic decal path read from the guest code and its draw admission
- [e3-lighting.md](graphics/e3-lighting.md): E3 2003-style lighting on the native renderer over FrameScene (design): sun shadows from objects, the renderer's own choice of lights, shadows from every cone light, the sharp edge, the budget and the slices; retail light-path findings
- [e3-lighting-setting.md](graphics/e3-lighting-setting.md): the Lighting row as built on the D3D path (`H2_LIGHTING`, scaled one-tap shadow maps in the view)
- [pstencil.md](graphics/pstencil.md): the Digsite E3 tag set's lights (study copies, never committed), corrections to earlier readings of them, and why shadows stay shadow maps rather than stencil volumes
- [pstencil-history.md](graphics/pstencil-history.md): the history of the cut E3 2003 stencil lighting from public sources: how it worked, why it was cut, where to see it, and the look to tune against (§4)
- Notes: [e3-lighting.md](graphics/notes/e3-lighting.md) (shadow-prototype timings, in-level Lighting runs, the budget arithmetic and the reading of Sev's shaders), [editor-guest-reads.md](graphics/notes/editor-guest-reads.md) (which of the renderer's guest reads are tag data and which are run state), [decorator-stages.md](graphics/notes/decorator-stages.md) (the lightmap page a decorator samples and what engine-driven states need), [native-ab.md](graphics/notes/native-ab.md) (the all-native A/B record behind the native renderer defaults), [not-adopted.md](graphics/notes/not-adopted.md) (removed render switches and experiments that lost), [pipeline-cache.md](graphics/notes/pipeline-cache.md) (the unbounded pipeline-cache prewarm that ran out of memory at boot), [pipelined-render.md](graphics/notes/pipelined-render.md) (pipelined rendering: census, prototype and measurements), [render-thread.md](graphics/notes/render-thread.md) (render thread measurements), [scene-pass-timing.md](graphics/notes/scene-pass-timing.md) (scene pass timing)
- [dynamic-lighting.md](graphics/dynamic-lighting.md): design (investigation) for an opt-in live sun over the baked lightmaps: what lighting data the disc holds, the approaches with costs on an M3 at 4x, the offline lightmap split, what FrameScene carries, and the slices
- [live-gi.md](graphics/live-gi.md): design for the `dynamic` mode's indirect light computed every sim tick: the technique choice with costs on an M3 at 4x, wgpu's Metal ray queries as built, transport traced offline and relit live, the fit to the retail lightmap and the fill, objects, leaks and caches, the engine shared with E3 lighting, the slices and the go or stop checkpoint

## Audio and movies (`audio/`)

- [audio.md](audio/audio.md) (**start here**): the mixer behind DirectSound (`crates/platform/audio`): voices, ADPCM, mix bins, effects, output
- [dsound.md](audio/dsound.md): the guest-facing DirectSound shim (`crates/platform/dsound`): call sites, buffers, streams, audio stats
- [wma.md](audio/wma.md): ASF demuxing and native WMAv2 decoding (`crates/formats/asf`, `crates/platform/wma`)
- [playstation-audio.md](audio/playstation-audio.md): DualSense/DS4 speaker routing, mute button and microphone design
- [bink-hle.md](audio/bink-hle.md): the Bink player replacement (`crates/platform/bink-hle`): playback of installed movies with sound, skip mode
- [movies.md](audio/movies.md): optional Bink movie conversion with your own ffmpeg (`h2 movies`)

## Performance (`perf/`)

- [README.md](perf/README.md) (**start here**): where the frame rate stands, which doc answers which question, and the order to read them in
- [profiling.md](perf/profiling.md): the profiler, timeline traces and benchmarks (`h2 profile`, `h2 bench`, `crates/tools/perf`, `crates/runtime/timeline`); test and bench knobs
- [guest-memory.md](perf/guest-memory.md): guest memory accessors, the inline RAM path (`H2_FASTMEM`, on by default), direct calls, the one exit call, the lent view and runtime counters
- [sim-budget.md](perf/sim-budget.md): the Waterworks and Lockout simulation tick budget: where an unrendered sim's main thread spends its time
- [sim-attribution.md](perf/sim-attribution.md): every sample of that Waterworks sim given an owner, the runtime share split, and the rewrite phase order it implies
- [sim-10x.md](perf/sim-10x.md): one sim match at 10 to 20 times real time: the Lockout budget, every lever ranked, the lanes, the ceiling of each step and the measurements that confirm or kill each
- [sim-10x-tier.md](perf/sim-10x-tier.md): spec for the sim tier in the image: roots, journaled segments, the exact clock, call-outs and the fallback to the reference image
- [sim-10x-callouts.md](perf/sim-10x-callouts.md): spec for `ExecCtx::call_inline`: a Rust body calls lifted code in place and leaves to the runner only when the runner has work
- [sim-headless-skips.md](perf/sim-headless-skips.md): what a headless sim still computes for a screen, a speaker or a local player, what the title already skips for a phantom roster, and each candidate skip classed for game state, films and the viewer
- [sim-phantom-build.md](perf/sim-phantom-build.md): the phantoms' action build call by call, where its 1.15 ms goes, and the design that runs it on live RAM with an undo journal
- [sim-light.md](perf/sim-light.md): RAM, disk and setup time per headless match: why mapped reads never fire in a sim, the levers ranked, and the lanes for a warm N:, frozen slots and lean reports
- Notes: [device-cost.md](perf/notes/device-cost.md) (D3D device CPU cost), [fastmem-proto.md](perf/notes/fastmem-proto.md) (the Mach write-watch prototype), [gameplay-budget.md](perf/notes/gameplay-budget.md) (where an in-level frame goes), [guest-memory.md](perf/notes/guest-memory.md) (guest memory and runner measurements), [menu-budget.md](perf/notes/menu-budget.md) (the main-menu frame budget), [steammachine-perf.md](perf/notes/steammachine-perf.md) (the Steam Machine profile and the music hangs), [sim-10x.md](perf/notes/sim-10x.md) (phantom-roster sim speeds and the Lockout budget arithmetic), [not-adopted.md](perf/notes/not-adopted.md) (performance experiments that lost)

## Data formats and tags (`formats/`)

- [map-format.md](formats/map-format.md) (**start here**): the cache file (v8) format and the `map` reader; read before the tag docs
- [map-builder.md](formats/map-builder.md): writing whole maps from a tag set (`map::build`): layout, fixups, retail round trips
- [tag-schema.md](formats/tag-schema.md): tag layouts inferred from every retail map; relocating any retail tag (`tags::schema`), shuffle proof, coverage
- [tag-definitions.md](formats/tag-definitions.md): typed tag definitions (`tags::defs`): every field named or an explicit unknown span; decode, encode, defaults, diff; disc round trip and coverage per group; scenery rebuilt with no template bytes
- [tag-repo.md](formats/tag-repo.md): the loose tag repository: one position-independent file per tag (`tags::repo`), `h2 tags extract` / `h2 tags build`, variants, rebuild proof
- [animation.md](formats/animation.md): animation graphs (`jmad`) and frame decompression (`h2 anim`)
- [bitmaps.md](formats/bitmaps.md): bitmap tags to textures and PNG (`crates/platform/assets`, `h2 bitmap`)
- [collision.md](formats/collision.md): collision and physics model tags (`h2 collision`)
- [effects.md](formats/effects.md): the effects family (`effe`, `prt3`, `pmov` and more; `h2 effect`)
- [fonts.md](formats/fonts.md): packed bitmap fonts and the text rasteriser (`crates/formats/font`, `h2 font`)
- [haloscript.md](formats/haloscript.md): HaloScript syntax, builtins and decoding (`crates/formats/hs`, `h2 hs`)
- [lighting.md](formats/lighting.md): level lighting, lightmaps and atmosphere
- [localization.md](formats/localization.md): string IDs and localized text
- [mopp.md](formats/mopp.md): Havok MOPP code in structure BSPs: the engine's opcodes, a query model, and the writer (`crates/formats/mopp`)
- [objects.md](formats/objects.md): object tags (`obje` family, `hlmt`, `itmc`, `vehc`; `h2 object`)
- [pathfinding.md](formats/pathfinding.md): AI pathfinding data in structure BSPs, and generating it from collision (`crates/formats/pathfind`)
- [shader-instances.md](formats/shader-instances.md): shader instance, template and pass tags (`shad`, `stem`, `spas`)
- [sound.md](formats/sound.md): sound tags (`snd!`, `ugh!`) and `h2 sound`
- [structure-bsp.md](formats/structure-bsp.md): render model and structure BSP geometry, lightmaps, and the viewer
- [ui.md](formats/ui.md): user-interface tags (`wgit`, `wgtz`, `wigl`, `skin`)

## Engine (`engine/`)

- [engine-map.md](engine/engine-map.md) (**start here**): survey of the retail engine's subsystems and where to start overriding
- [feature-engine.md](engine/feature-engine.md): the feature engine: Rust subsystems at clean boundaries, hooks, settings; what is built and what is design
- [guest-layout.md](engine/guest-layout.md): `crates/hooking/guest-layout`: guest structure offsets and globals, the `impl.*` switch table, hook point shapes
- [anim-sampling.md](engine/anim-sampling.md): animation sampling in Rust (`sim-anim`) behind `impl.sim.native.anim`: the contract, the decoded clips and the disc sweep
- [object-update.md](engine/object-update.md): the object update's contracts read from the lifted code (callouts, flag setters, register liveness) and how the phase 3 bodies behind `impl.sim.native.objects` keep them
- [sim-review.md](engine/sim-review.md): typed simulation state and the experimental leaf compiler contract
- [sim-tier-experiment.md](engine/sim-tier-experiment.md): the sim-tier experiment: the tick's leaves compiled under a looser contract (`emit::sim`), checked against the interpreter and timed against the hand ports
- [sim-tier-writes.md](engine/sim-tier-writes.md): a static census of every store the game tick can make
- [README.md](engine/README.md): contracts for the engine functions rewritten in Rust (`crates/rewrites/engine`) and their differential tests, indexing [datum_new.md](engine/datum_new.md), [tag_get_checked.md](engine/tag_get_checked.md), [locale_string_id_find.md](engine/locale_string_id_find.md), [memory_pool.md](engine/memory_pool.md) and the HaloScript VM, [hs_vm.md](engine/hs_vm.md)
- [shadow-exec.md](engine/shadow-exec.md): shadow execution: native ports checked against the lifted original on the states the game reaches (`shadow.*`, `scripts/shadow-gate.sh`), and the shared `lifted` crate
- [havok-runtime.md](engine/havok-runtime.md): how the retail game keeps its Havok world in guest memory: world, islands, bodies, shapes, contacts, agents
- [collision-queries.md](engine/collision-queries.md): the native collision world behind `impl.sim.native.collision`: entries, contract and bail reasons
- [sim-rewrite.md](engine/sim-rewrite.md): plan for rewriting the simulation as Rust subsystems (not built), with the phase specs [sim-rewrite-collision.md](engine/sim-rewrite-collision.md) (phase 2), [sim-rewrite-objects.md](engine/sim-rewrite-objects.md) (phase 3) and [sim-rewrite-havok.md](engine/sim-rewrite-havok.md) (phase 4)
- Notes: [2026-11-21-engine-vtables.md](engine/notes/2026-11-21-engine-vtables.md) (the retail game engines' vtables and the slots that make rules), [2026-11-22-script-scoring-and-roles.md](engine/notes/2026-11-22-script-scoring-and-roles.md) (the carriers' scoring sites, Juggernaut's role and Oddball's ball return, for script games), [2026-11-23-round-start-and-player-quit.md](engine/notes/2026-11-23-round-start-and-player-quit.md) (the round reset and the player-left notice that deliver `on round_start` and `on quit`), [feature-engine.md](engine/notes/feature-engine.md) (feature engine and engine contracts evidence)

## Game features and enhancements (`game/`)

- [settings-menu.md](game/settings-menu.md) (**start here**): the in-game settings screens (Video, Voice, pause menus), FOV, the settings schema and `h2 settings`; how Halo 2's own settings screens work
- [teabag.md](game/teabag.md): teabags in every game (two crouches over a fresh body), the `Teabag` event and the Incel medal for a game without one
- [pogo-stick.md](game/pogo-stick.md): Superbounce's Pogo Stick: a weapon of our own in three looks built from `art/pogo/` into each multiplayer map, the secondary weapon by a script option, carried on the ground and ridden in the air with a trick for each bounce of a chain, the chase camera, `pogo.state` and `pogo.test`
- [superbounce.md](game/superbounce.md): how a superbounce happens: a landing in a floor crease next to a seam, the Havok push, the forced-crouch and crouch-height code, per-tick traces on Ascension; the scan for spots in a map's collision (`h2 superbounce scan`); the bounce medals (design)
- [aim-assist.md](game/aim-assist.md): the Aim Assist setting (Auto, Default, Strong, Very Strong), the weapon and globals fields it scales at map load
- [ui-builder.md](game/ui-builder.md): the UI builder for native screens: pages as pure state machines, one `View` drawn offline from the disc, layout checks, the fidelity loop against game captures
- [ui-screens.md](game/ui-screens.md): the UI kit for list screens of ours: ids, input, previews, the checklist
- [ui-in-maps.md](game/ui-in-maps.md): our screens as generated `wgit` tags in the mainmenu and shared maps, listed in the `wgtz`, cached by disc map and docs hash
- [hud-kit.md](game/hud-kit.md): the HUD kit (design): what Halo 2's HUD is made of (widgets, messages, training prompts, colours, fonts, anchors), HUD elements of ours drawn by the game's own HUD code, `h2 hud preview` and its checks, the superbounce HUD rebuilt on it
- [skulls.md](game/skulls.md): campaign skulls per profile, the skulls menu, and our own skulls
- [community-gametypes.md](game/community-gametypes.md): community classics (Zombies, SWAT and more) as extra built-in game variants
- [game-options.md](game/game-options.md): game variant options retail doesn't allow (no-weapon starts, instant respawn, gravity) and names with any character (design)
- [maps.md](game/maps.md): map packages, patches applied at load, separate mod slots, and Live downloads
- [test-maps.md](game/test-maps.md): test maps from recipes (spawn grid, weapon range, superbounce patch...), built from the disc and loaded through the agent API's `maps.test.*`
- [bots.md](game/bots.md): Clanker navigation, beliefs, personality, phantom seats, appearance, player options, and Clankermatch
- [bot-training.md](game/bot-training.md): tuning the Clanker table from sim matches: what is searched, the fitness against people, throughput, the admin's Training views and promoting a table (design)
- [bounce-search.md](game/bounce-search.md): the bots learn each superbounce spot by a per-spot knob search in headless sims (design): the knobs, the search, `<map>.learned`, `h2 sim train`
- [map-pack.md](game/map-pack.md): the Multiplayer Map Pack disc, `h2 import-mappack` and how 1.0 lists its maps
- [map-select.md](game/map-select.md): Change Map categories for Halo 2, Map Pack, Mods and Halo: CE, with separate retail and mod slots
- [high-framerate.md](game/high-framerate.md): 60 and 120 fps with the 30 Hz tick exact (`H2_FPS`): the determinism harness, the interpolated replay and reprojection (built, not yet run in game)
- [theater.md](game/theater.md): the in-game film library, host-film playback and seeking, with real-host verification pending
- [Theater seek measurements](game/notes/theater-seek.md): cold launch, keyframe capture and tape size procedure
- [film-format.md](game/film-format.md): the `.h2film` container a host records and Theater replays: layout, keyframes, seeking, determinism checks
- [forge.md](game/forge.md): what Halo 3's Forge is and what retail Halo 2 has for one to sit on: placement blocks, the netgame equipment spawner, object creation
- [forge-design.md](game/forge-design.md): Forge documents, edit logs, local map builds and the design for in-game tools and sharing
- [Forge probes](game/notes/forge-probes.md) records the runtime measurement procedure and results for Forge.
- [forge-features.md](game/forge-features.md): design for each Forge feature: monitor bodies, objects, links and triggers, emitters, barriers, nav, AI, geometry and paint, character import (not built)
- [forge-monitor-appearance.md](game/forge-monitor-appearance.md): design for what a Forge monitor looks like: its model from the disc, colours, tint and light, the picker and preview, how a session agrees on it, and how Theater shows it (not built)
- [ai-networking.md](game/ai-networking.md): AI actors in network games: host actors in the distributed model, the synchronous model for campaign, what retail replicates, the actor channel, budgets, probes and slices
- [firefight.md](game/firefight.md): Firefight: waves of AI on multiplayer maps for up to 16 players, the director, the offline map kit, wave-set definitions and the design for scoring, lives, skulls and Live
- [forge-editing.md](game/forge-editing.md): what a Forge monitor can do, after Halo 3, Reach and Infinite: controls, tools, snapping and Coordinates, the object menu, properties by kind, groups and prefabs (partly built)
- [forge-map-variants.md](game/forge-map-variants.md): design for Forge as a scenario editor: edits of the map's own placements named by identity with a print, the fields by kind, game types, the map variant file, how a lobby, a playlist entry and a game variant name one, materialising, Live, the named refusals, slices (design)
- [forge-map-variants-ui.md](game/forge-map-variants-ui.md): design for that editor's UI on the kits: Properties rows by kind from a YAML doc, the bottom-left readout, world marks, the budget, the map variant save, load and rename screens, previews, slices (design)
- [Forge prior art](game/notes/forge-prior-art.md) records what Halo 3, Reach and Infinite Forge do, with sources.
- [forge-firefight-ui.md](game/forge-firefight-ui.md): Forge and Firefight UI as Halo 2 draws it: an audit of each screen, HUD piece and overlay, the retail screen or HUD element each copies, the six-row main menu, the lobby modes, and the slices (design)
- [party-lobby.md](game/party-lobby.md): the party lobby's modes (Multiplayer, Custom Games, Forge, Firefight, Co-op): the header that names the mode, the leader's GAME MODE picker on X, the Xbox Live menu's mode rows, where each mode runs, what a switch does, and its slices
- [rules-engine.md](game/rules-engine.md): scripted game types: a typed rules language and Forge's logic graph compiled to one verified IR, a host-only VM over named engine entries, script ports of the seven retail engines (design)
- [rules-language-types.md](game/rules-language-types.md): the rules language with Forge labels as declared, typed names and map requirements; nullable handles with narrowing, `time`, `zone` and `weapon` types, `match`, constants; diagnostics with fixes; the IR's label table; the slices (design)
- [rules-start-gate.md](game/rules-start-gate.md): a map that breaks a script's label requirements blocks the game: the check in the lobby, on the host at game start and on Live's hopper edits (design)
- [rules-variants.md](game/rules-variants.md): a game variant as one rules script: the `settings` block for record settings, `quick` marks for the lobby's Quick Options, `scoring carrier`, the community variants as scripts in place of the Rust table, the slices (design)
- [rules-world-api.md](game/rules-world-api.md): the world API for rules scripts: who runs a script in each session kind, the authenticated show channel for HUD, messages and sounds, entries for objects, drop ships, AI squads, bots, audio, HUD and physics, security and limits, per-entry costs and bench gates, splitting a script, the slices (design)
- [rules-world-limits.md](game/rules-world-limits.md): the world API's security, limits and performance: what an attacker controls in each direction, verifier refusals, caps on every write, the show channel's validation, the hopper gate, a property test per entry, per-entry costs and bench gates (design)
- [rules-world-modes.md](game/rules-world-modes.md): Firefight and Superbounce as plain scripts over the world API: what each needs, what is engine code today, a sketch of each, and when the Rust oracles go (design)
- [theater-design.md](game/theater-design.md): design for the full in-game Theater: stats is the summary and the film is the simulation; rewind and jumps from sidecar keyframes, warm restore, cameras, clips, sharing through Live and the web-to-game link; slices in build order (design)
- [input-film.md](game/input-film.md): what a film holds (seed, per-seat inputs, joins, digests), every host answer replay depends on, the Halo 3 reference, the column encoding, sizes and memory (design)
- Notes: [not-adopted.md](game/notes/not-adopted.md) (game designs built and removed), [2026-11-23-firefight-kit-headroom.md](game/notes/2026-11-23-firefight-kit-headroom.md) (where the Firefight kit's tag memory goes on Zanzibar and the other large maps), [2026-11-22-retail-settings-dialogs.md](game/notes/2026-11-22-retail-settings-dialogs.md) (where retail's settings dialogs put the window's line and a setting's help), [high-framerate.md](game/notes/high-framerate.md) (prior art and measurements for 60 and 120 fps), [halo3-film.md](game/notes/halo3-film.md) (Halo 3's saved film layout and sizes, from public sources), [ai-networking.md](game/notes/ai-networking.md) (what retail's distributed replication carries, read from the image)
- [creature-entity.md](game/notes/creature-entity.md) (retail creature-slot registration, scheduler bounds, wire dispatch and session admission)
- [2026-11-21-h2-custom-hud-mods.md](game/notes/2026-11-21-h2-custom-hud-mods.md) (how Halo 2 mods did custom HUD work and how an `nhdt` meter is made, from public sources), [superbounce-spots.md](game/notes/superbounce-spots.md) (the pair word tick by tick round a forced crouch, the time to sleep, and what a stricter crouch filter keeps per map), [2026-11-21-known-superbounce-spots.md](game/notes/2026-11-21-known-superbounce-spots.md) (the community's documented superbounce spots per map and technique facts, set against our scan), [2026-11-21-bot-bounce-sim.md](game/notes/2026-11-21-bot-bounce-sim.md) (the bots' superbounce practice and tuning sweeps in sim mode: commands and results tables), [2026-11-22-seam-drop-depth.md](game/notes/2026-11-22-seam-drop-depth.md) (seam drops with a carried velocity: the Ascension seam bounces when the last free tick ends deep enough), [2026-11-23-pogo-static-reads.md](game/notes/2026-11-23-pogo-static-reads.md) (the Pogo Stick's static reads: the starting weapon pick and the spawn's weapon pass in the image, what can refuse a weapon, the camera route, the node rows), [2026-11-23-item-game-types.md](game/notes/2026-11-23-item-game-types.md) (the item spawner's game-type slots, the matcher and the respawn modes, read from the image), [2026-11-23-bots-natural-round1.md](game/notes/2026-11-23-bots-natural-round1.md) (bots fighting like people: Midship Slayer and Juggernaut activity, accuracy by range and melee before the fixes)

## Halo 1 (`halo1/`)

Halo 1 (Xbox) running in the Halo 2 engine: its data read from your own `halo1.iso` and converted offline into Halo 2 maps. [README.md](halo1/README.md) (**start here**) says where each part stands.

- [rebuild.md](halo1/rebuild.md): the Halo 1 converter writing objects from typed definitions: scenery, vehicles, bipeds, devices, physics, effects, weapons, particles
- [campaign.md](halo1/campaign.md): Halo 1's campaign in the Halo 2 engine: the decisions, the pipeline, and every step. All ten levels convert offline at import (`h2 import halo1`); a30 has run headless, none has been played yet
- [multiplayer.md](halo1/multiplayer.md): Halo 1's 13 disc multiplayer maps converted onto Halo 2 base maps (`h2 h1 convert`): collision, structure BSP, MOPP, placements, packages with variants
- [campaign-picker.md](halo1/campaign-picker.md): how 1.0's menus list campaign levels and keep progress, and the campaign picker and per-campaign progress built on them (confirmed in the game)
- [parity.md](halo1/parity.md): headless behavioural checks of converted Blood Gulch and a30 against the Halo 1 disc, driven through the agent API's `probe.*` methods (`h2 parity`)
- [health.md](halo1/health.md): Halo 1's health model on Halo 2's engine: no body regeneration, health packs, the meter in Halo 2's HUD; picked from the map at game start (`H2_HEALTH_MODEL` overrides)
- [hud.md](halo1/hud.md): Halo 1's HUD on converted maps: the unit HUD is built and the default; weapon HUDs, grenades and the HUD globals are not converted yet
- [script-builtins.md](halo1/script-builtins.md): Halo 1 unit, object and miscellaneous script builtins: retail equivalents and the hooks on converted campaign maps (`boot::h1_script_builtins`)
- [shared-map.md](halo1/shared-map.md): design for a shared Halo 1 cache and the Halo 1 sandbox on every map: the sandbox built once and added to a map as a layer, its raw in an appendix to `shared.map`'s offset space (not built)
- Notes: [shared-map.md](halo1/notes/shared-map.md) (what the converted maps repeat, and the room in the shared caches)

Reading and converting Halo 1 data:

- [map-format.md](halo1/map-format.md): Halo 1 Xbox cache files (v5, zlib) and the `h1map` reader (`h2 h1 info|tags|convert`, `h2 import halo1`)
- [models.md](halo1/models.md): Halo 1 Xbox models and animation graphs converted to Halo 2 `mode` and `jmad` (`crates/tools/h1-assets`)
- [bitmaps.md](halo1/bitmaps.md): Halo 1 Xbox bitmaps: where the pixels are, decoding with Halo 2's decoder, conversion to Halo 2 `bitm` and lightmap pages (`crates/tools/h1-assets`, `h2 h1 bitmap`)
- [sounds.md](halo1/sounds.md): Halo 1 Xbox sounds: where the samples are, decoding to PCM, conversion to Halo 2 `snd!` plus `ugh!` gestalt entries, the whole gestalt layout, class mapping (`crates/tools/h1-assets`, `h2 h1 sound`)
- [objects.md](halo1/objects.md): Halo 1 sandbox objects (`weap`, `proj`, `eqip`, `bipd`, `vehi`, `coll`, `phys`) read from Halo 1 and built as Halo 2 tags on retail templates, with the tuning carried over (`crates/tools/h1objects`)
- [role-audit.md](halo1/role-audit.md): Halo 2 values left in converted tags (role words, counterpart references), each allowed or a bug
- [ai.md](halo1/ai.md): Halo 1 AI data (actors, encounters, command lists) read, Halo 2's scenario AI blocks and `char` decoded, and the conversion between them (`crates/tools/h1campaign`)
- [ai-control.md](halo1/ai-control.md): Halo 1 encounter script controls bound to Halo 2 evaluators (`h1scripts::aicontrol`, `h1ai::control`)
- [scripts.md](halo1/scripts.md): Halo 1 campaign scripts decoded, decompiled, translated to Halo 2 builtins and compiled to Halo 2 nodes (`crates/tools/h1campaign`)
- [sky.md](halo1/sky.md): Halo 1 and Halo 2 `sky ` tags: layouts, the disc census, and conversion (`crates/tools/h1-assets`)
- [text.md](halo1/text.md): Halo 1 `ustr` and `hmt ` text: reading UTF-16 lists and HUD messages, converting them to Halo 2 `unic` locale rows (`crates/tools/h1-assets`)

## Xbox Live (`live/`)

- [live.md](live/live.md) (**start here**): the Xbox Live replacement: overview, CLI, service and every client HLE section
- [live-api.md](live/live-api.md): the service's HTTP and WebSocket contract
- [live-security.md](live/live-security.md): identity, signing, token checks, recovery, rate limits and the federation design
- [live-cloud-saves.md](live/live-cloud-saves.md): cloud saves: profiles, variants, campaign saves and profile settings follow the account; the merge and conflict rules
- [live-admin.md](live/live-admin.md): who may publish title files, and the web MOTD editor and playlist builder
- [web-boundary.md](live/web-boundary.md): what goes on the dashboard and what on the stats site, and the dashboard's dense style for admin and ops screens
- [web-app.md](live/web-app.md): the Live web app: its pages, the mock server and its `H2_MOCK_*` knobs, derived stats, sign-up, social features, link previews and the state architecture
- [web-viewer.md](live/web-viewer.md): the web app's optional game art and the game page's 2D map and 3D viewer
- [viewer-overlay.md](live/viewer-overlay.md): the 3D view's opening camera, Halo 2-style overlay, name tags and progressive reveal (design only, not built)
- [viewer-effects.md](live/viewer-effects.md): how damage reads in the viewer, and its combat effects: shields, explosions, impacts, melee, teleporters and deaths, what each derives from and the new telemetry two need (design only, not built)
- [Telemetry cost](live/notes/2026-11-19-telemetry-cost.md): frame sizes and CPU per version, with the benchmark commands.
- [Replay: the original 1 Hz recording](live/notes/2026-11-18-replay-original-1hz.md): stage rates, fields and sizes of the first recording.
- [Live Worker rehearsal](live/notes/2026-11-18-worker-rehearsal.md): the migration rehearsal on a copy of the real log.
- [MOTD first reading](live/notes/2026-11-16-motd-first-reading.md): the superseded XONLINE call-chain reading, kept for its addresses.
- [Dedicated host measurements](live/notes/host-halving.md): the evidence for halving the dedicated host.
- [Telemetry precision and replay measurements](live/notes/2026-11-19-telemetry-precision.md) for the Live v8 stream: the workload, byte counts per player and the finished-archive limits.
- [Grenade trajectories](live/notes/grenade-trajectories.md) compares the telemetry viewer’s grenade arcs with recorded Lockout projectiles.
- [dash-concepts.md](live/dash-concepts.md): the Live dashboard as it is now: the orb, the menu blades, the Games screen and its Halo 2 panel
- [live-hosting.md](live/live-hosting.md): hosting a server for friends (Cloudflare Tunnel, docker compose)
- [live-worker.md](live/live-worker.md): the Live service on Workers, Durable Objects and R2, including the game header and cutover runbook
- [dedicated-host.md](live/dedicated-host.md): native game hosting, zero-local trial, light host, cloud choices and Ranked telemetry design
- [dedicated-host-build.md](live/dedicated-host-build.md): the shared contract of the parallel dedicated-host build: the slim host, contracts and file ownership
- [host-halving.md](live/host-halving.md): dedicated host resource profiles, pack admission and fleet sizing, measurement gates and the proposed template start
- [live-boundary.md](live/live-boundary.md): static inventory of the retail XOnline/XNet entry points and the sign-in sequence
- [live-motd.md](live/live-motd.md): how retail fetches and parses `message_of_the_day.dat` and `multiplayer_hopper.dat`
- [live-matchmaking.md](live/live-matchmaking.md): sessions, invites, arbitration, the Ranked page and ping-aware host selection in progress
- [live-custom-games.md](live/live-custom-games.md): the custom games browser, filters, visibility and joining
- [live-coop.md](live/live-coop.md): campaign co-op over Live: options, decisions, and what is built and probed (Co-op over Live built and confirmed in the game)
- [live-voice.md](live/live-voice.md): voice chat, voice messages and the mute list: how the title does voice, and the host design (design only, not built)
- [live-gamestate.md](live/live-gamestate.md): retail game-state addresses and layouts for the scoreboard and map
- [live-telemetry.md](live/live-telemetry.md): capture hooks, events, machine travel, negotiated v8 live frames and the archive handoff
- [replay-fidelity.md](live/replay-fidelity.md): current recording and playback, the original 1 Hz measurements and the spectator track design
- [replay-migration.md](live/replay-migration.md): migrate, verify and roll back finished-game archives in both homes
- [live-art.md](live/live-art.md): map geometry and pre-drawn views extracted from the disc for the stats site

## Editor (`editor/`)

- [editor-guide.md](editor/editor-guide.md) (**start here**): user guide to `h2-editor`: map placements, tags, variants, playlists, UI screens, HUDs and Halo 1 maps
- [editor.md](editor/editor.md): the editor for developers (`crates/apps/studio`, Dear ImGui + wgpu): window, the tools, Halo 1 maps and converting, the playlist format rules, code layout, scripted headless screenshots (`--do`, `--crop`, `--callout`), viewport gaps
- [scenario-editor.md](editor/scenario-editor.md): the scenario placement editor (`crates/tools/scenario-edit`): blocks, fields, save rules, packages
- [rules-editors.md](editor/rules-editors.md): game type scripts in VS Code and Zed: the extensions in `editors/`, `h2 rules editors`, the language server's features, CI and troubleshooting
- [variants.md](editor/variants.md): the game variant record, its schema and saved files (`crates/formats/variants`)
- [hud-editor.md](editor/hud-editor.md): the HUD tool (`nhdt` tags): placement, checks, the save, what is not modelled
- [agent-api.md](editor/agent-api.md): the editor's socket API: JSON methods over maps, tags, placements, variants, playlists and UI screens
- [live-mirror.md](editor/live-mirror.md): paired game discovery, live world state, conditional edit batches, undo and conflict handling
- [click-to-edit.md](editor/click-to-edit.md): placement-to-tag navigation, editable tag documents, references and the remaining unified Map work
- [tag-editing.md](editor/tag-editing.md): design for the tag field panel: rows in plain words, when a change takes effect, folded unnamed fields, lists, pickers, search, the naming workflow and the API additions
- [rich-editors.md](editor/rich-editors.md): design for editors past raw fields: colour pickers, curves, the shader tab and its preview, script source and compiler, sounds with mouth curves, recordings, and what each can write
- [modeling.md](editor/modeling.md): mesh kernel and the design for editing geometry, collision, lightmaps and textures
- [modeling-targets.md](editor/modeling-targets.md): design for the Halo 2 round trip of each modeling target: read path, edit model, write classes, what is recomputed, limits and risks for `sbsp` render geometry and collision, portals, lightmaps, `mode`, `coll` and `phmo`
- [realtime-edits.md](editor/realtime-edits.md): design for sending geometry, placement add and remove, collision and texture edits to the running game without a reload: the raw overlay, the edit arena, the agent methods and the build slices
- Notes: [2026-11-23-realtime-survey.md](editor/notes/2026-11-23-realtime-survey.md) (what a live edit of each tag group does today and what realtime needs, sized), [tag-field-names.md](editor/notes/tag-field-names.md) (the sources of tag field names, their links and licences)

## Working on the repository (`dev/`)

- [crate-structure.md](dev/crate-structure.md): crate domains, dependency checks, composition and the crate table

## Reviews

Dated reviews of the code or a plan, frozen on their date. The design pages
hold what was done about their findings.

- [2026-11-16-frame-time.md](reviews/2026-11-16-frame-time.md): why gameplay runs at about 18 fps against a 33.3 ms budget, with ranked structural changes
- [2026-11-20-architecture.md](reviews/2026-11-20-architecture.md): a read of the whole repository (recompiler, runtime, hooks, renderers, crate structure) and an ordered list of ten changes
- [2026-11-20-e3-lighting.md](reviews/2026-11-20-e3-lighting.md): the E3 lighting design against the public record of the 2003 renderer and against FrameScene: what holds, what is stale, what was missing
- [2026-11-20-sim.md](reviews/2026-11-20-sim.md): how the Halo 2 sim works, why it is slow, and a critique of the rewrite plan

## About these docs (`meta/`)

- [docs-style.md](meta/docs-style.md): how docs here are written, where each kind goes, the evidence labels and the checker

