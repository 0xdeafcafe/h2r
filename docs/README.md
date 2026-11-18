# Docs

Every doc in this folder, grouped by area. **Start here** marks the entry
point for each area. "Design" means the doc describes something not built
(or only partly built); the doc's own status line says how far it got.

[ARCHITECTURE.md](ARCHITECTURE.md) is the entry point for the whole project:
goals, the two-stage plan, the crate table and milestones.

## Lifter: analysis, lifting and CPU semantics (`lifter/`)

- [LIFTER.md](lifter/LIFTER.md) (**start here**): the lifter's original design: x86 to IR to C11, the runtime contract, the validation plan. Where it and the code differ, the code wins
- [FLOAT.md](lifter/FLOAT.md): what floating point the game uses (measured with `h2 fpstats`) and the correctness-first plan
- [FLOAT-ABI.md](lifter/FLOAT-ABI.md): the x87 IR ops and `h2x87_v1_*` runtime helpers (`crates/x87`), #MF included; read when touching x87 lowering
- [x87-native.md](lifter/x87-native.md): the guarded native binary64 x87 path, on by default (`H2_X87_NATIVE=0` disables it): range proof, status bits, fallbacks
- [x87-transcendentals.md](lifter/x87-transcendentals.md): the `fast-trans` 128-bit first pass for FYL2X, F2XM1 and the trig ops, the table-driven FYL2X and F2XM1: guards and measurements
- [sse-native.md](lifter/sse-native.md): guarded native SSE arithmetic, on by default (`H2_SSE_NATIVE=0` disables it): eligibility and range proof
- [simd.md](lifter/simd.md): bit-exact SSE1 reference semantics (`crates/simd`), its C twin and how it is tested
- [oracle.md](lifter/oracle.md): independent x86 instruction oracle on Unicorn (`crates/oracle`)
- [sse-oracle.md](lifter/sse-oracle.md): SSE/MMX results measured on Rosetta 2 (`crates/sse-oracle`)
- [golden-trace.md](lifter/golden-trace.md): design for golden boot traces from Unicorn (the code is not on master)
- [dispatch-coverage.md](lifter/dispatch-coverage.md): indirect-dispatch coverage of the campaign's main tables, and how to reproduce it
- [xdk-libraries.md](lifter/xdk-libraries.md): how `h2 symbols` identifies statically linked XDK library code
- [BUILD-CACHE.md](lifter/BUILD-CACHE.md): generated-source and object caches, locking, iteration builds, `H2_EMIT_TARGET`; read before changing the build
- [build-perf.md](lifter/build-perf.md): build-time measurements behind those caches

## Runtime: booting and running the game (`runtime/`)

- [BOOT.md](runtime/BOOT.md) (**start here**): `h2 build` and `h2 run`, the boot runner's flags and environment, stopgaps, current state
- [building.md](runtime/building.md): the root `justfile`: building every target (Mac, Apple TV, iPhone, Windows, Xbox) from your disc, `just doctor`, `maps=demo|all`
- [app.md](runtime/app.md): the host shell, `h2 play` and `h2 play --follow` (`crates/app`): flags, pacing, frame stats, embedded hosts (tvOS, iOS)
- [level-boot.md](runtime/level-boot.md): `h2 play --level` / `H2_LEVEL`: start a campaign level or a local multiplayer game directly
- [level-preflight.md](runtime/level-preflight.md): static audit of what the first campaign levels need, ranked by risk
- [kernel-imports.md](runtime/kernel-imports.md): work list of every kernel import Halo 2 uses, with signatures
- [xapi.md](runtime/xapi.md): which XAPI services are replaced (`crates/xapi`) and which stay generated, native saves included
- [storage.md](runtime/storage.md): the Xbox storage boundary (`crates/vfs`): volumes, FATX names, file semantics, map overrides
- [agent-api.md](runtime/agent-api.md): design for an agent-facing API to the running game: UI and world views, profiling by owner, snapshots, input, overrides and A/B experiments (design only; none of it is on master)
- [lab.md](runtime/lab.md): `h2 lab`: start levels on a second Mac over SSH, build branches there, replay input recordings, fetch frame stats, profiles and logs
- [checkpoints.md](runtime/checkpoints.md): the checkpoint save and revert acceptance test
- [snapshots.md](runtime/snapshots.md): machine snapshots for `state.save`/`state.load`: what state lives outside guest RAM, the file format, phases
- [cache-slots.md](runtime/cache-slots.md): the campaign cache-slot integration test
- [tvos.md](runtime/tvos.md): building and deploying to Apple TV (`platform/tvos`, `H2_EMIT_TARGET=tvos`)
- [windows.md](runtime/windows.md): cross-building the runtime and the game module as a Windows DLL from a Mac (`x86_64-pc-windows-msvc`)
- [xbox.md](runtime/xbox.md): the UWP app for an Xbox in Dev Mode, packaged, signed and deployed from a Mac (`platform/xbox`, `H2_EMIT_TARGET=windows`)
- [title-updates.md](runtime/title-updates.md): design for title-update XBEs (1.1 and later): what assumes the 1.0 image, the address-map plan; design only, the one built part is `h2 import-mappack` copying the Map Pack's update aside

## Input (`input/`)

- [input.md](input/input.md) (**start here**): XInput replacement, device model, backends, bindings and input timelines (`crates/input`)
- [input-default.toml](input/input-default.toml): the default binding file; [input-menu-example.toml](input/input-menu-example.toml): an example input timeline
- [host-keyboard.md](input/host-keyboard.md): host typing in the game's virtual keyboard
- [controller-layouts.md](input/controller-layouts.md): the retail button layouts and the extra ones we add
- [button-icons.md](input/button-icons.md): button glyphs that follow the device in use (Xbox 360, PlayStation, keyboard; `H2_ICONS`)
- [steam-controller.md](input/steam-controller.md): Steam Controllers over raw HID, trackpad aim and haptics (`H2_STEAM_*`)
- [ios.md](runtime/ios.md#controls): the iOS touch controls (`input::touch`, `TouchControls.swift`) and their settings
- [keyboard-charset.md](input/keyboard-charset.md): design for extending the on-screen keyboard's characters (not built)

## Graphics (`graphics/`)

- [graphics.md](graphics/graphics.md) (**start here**): D3D8 state and textures on wgpu (`crates/gfx`): batching, merged passes, the pipeline cache
- [d3d.md](graphics/d3d.md): the guest-facing Direct3D 8 replacement (`crates/d3d`): entries, device model, push buffer, render targets, vblank
- [shaders.md](graphics/shaders.md): NV2A vertex microcode, register combiners, fog and shadow maps to WGSL (`crates/shaders`)
- [render-thread.md](graphics/render-thread.md): the render thread (on by default, `H2_RENDER_THREAD=0` turns it off): what the guest can observe, measured gain
- [render-loop.md](graphics/render-loop.md): stage 7a, the view and pass loops ported to Rust: what is built and what is next
- [occlusion-culling.md](graphics/occlusion-culling.md): how a view's objects are chosen, the per-object skip point, and Hi-Z occlusion culling against the previous frame's depth (`impl.render.occlusion`)
- [pipelined-render.md](graphics/pipelined-render.md): the render front-end one frame behind the simulation on its own thread: design, and the `render-census` that decides it
- [layer-draw.md](graphics/layer-draw.md): the layer draw and its entry loop, read and ported, with the `render-timing` breakdown
- [hot-iteration.md](graphics/hot-iteration.md): live tag edits (`edit.*`) and hot WGSL overrides, parameters and toggles for the host renderer (`render.*`), without a rebuild or restart
- [high-resolution.md](graphics/high-resolution.md): 4K and other internal resolutions (`H2_RENDER_SCALE`, the Resolution row); phases 1 and 2 built
- [e3-lighting.md](graphics/e3-lighting.md): design for E3 2003-style stencil shadows and per-pixel lighting; phases 1 and 3 built (`H2_LIGHTING`, the Lighting row: sharper one-tap shadow maps at the render scale)
- [pstencil.md](graphics/pstencil.md): the Digsite E3 tag set's lights (study copies, never committed), corrections to earlier readings of them, and why shadows stay shadow maps rather than stencil volumes
- [pstencil-history.md](graphics/pstencil-history.md): the history of the cut E3 2003 stencil lighting from public sources: how it worked, why it was cut, where to see it, and the look to tune against (§4)

## Audio and movies (`audio/`)

- [audio.md](audio/audio.md) (**start here**): the mixer behind DirectSound (`crates/audio`): voices, ADPCM, mix bins, effects, output
- [dsound.md](audio/dsound.md): the guest-facing DirectSound shim (`crates/dsound`): call sites, buffers, streams, audio stats
- [wma.md](audio/wma.md): ASF demuxing and native WMAv2 decoding (`crates/asf`, `crates/wma`)
- [playstation-audio.md](audio/playstation-audio.md): DualSense/DS4 speaker routing, mute button and microphone design
- [bink-hle.md](audio/bink-hle.md): the Bink player replacement (`crates/bink-hle`): playback of installed movies with sound, skip mode
- [movies.md](audio/movies.md): optional Bink movie conversion with your own ffmpeg (`h2 movies`)

## Performance (`perf/`)

- [perf.md](perf/perf.md) (**start here**): the profiler, timeline traces and benchmarks (`h2 profile`, `h2 bench`, `crates/perf`, `crates/timeline`); test and bench knobs
- [performance.md](perf/performance.md): guest memory accessors, the inline RAM path (`H2_FASTMEM`, on by default), direct calls, the one exit call, the lent view and runtime counters
- [frame-budget.md](perf/frame-budget.md): where an in-level frame's time goes, the worst frames and what a worst frame under 33 ms needs
- [perf-budget.md](perf/perf-budget.md): the main-menu frame budget: the latest flyover numbers and the original plan to 30 fps
- [architecture-review.md](perf/architecture-review.md): a structural review of the gameplay frame-time gap, with ranked changes and their status
- [fastmem-4k.md](perf/fastmem-4k.md): the 4 KiB guest translation experiment (branch only; slower than the inline path, not adopted)
- [fastmem-proto.md](perf/fastmem-proto.md): the Mach exception write-watch prototype (`crates/fastmem-proto`) and its results

## Data formats and tags (`formats/`)

- [map-format.md](formats/map-format.md) (**start here**): the cache file (v8) format and the `map` reader; read before the tag docs
- [map-builder.md](formats/map-builder.md): writing whole maps from a tag set (`map::build`): layout, fixups, retail round trips
- [tag-schema.md](formats/tag-schema.md): tag layouts inferred from every retail map; relocating any retail tag (`tags::schema`), shuffle proof, coverage
- [tag-definitions.md](formats/tag-definitions.md): typed tag definitions (`tags::defs`): every field named or an explicit unknown span; decode, encode, defaults, diff; disc round trip and coverage per group; scenery rebuilt with no template bytes
- [tag-repo.md](formats/tag-repo.md): the loose tag repository: one position-independent file per tag (`tags::repo`), `h2 tags extract` / `h2 tags build`, variants, rebuild proof
- [animation.md](formats/animation.md): animation graphs (`jmad`) and frame decompression (`h2 anim`)
- [bitmaps.md](formats/bitmaps.md): bitmap tags to textures and PNG (`crates/assets`, `h2 bitmap`)
- [collision.md](formats/collision.md): collision and physics model tags (`h2 collision`)
- [effects.md](formats/effects.md): the effects family (`effe`, `prt3`, `pmov` and more; `h2 effect`)
- [fonts.md](formats/fonts.md): packed bitmap fonts and the text rasteriser (`crates/font`, `h2 font`)
- [h1-map-format.md](formats/h1-map-format.md): Halo 1 Xbox cache files (v5, zlib) and the `h1map` reader (`h2 h1 info|tags|convert`, `h2 import halo1`)
- [h1-models.md](formats/h1-models.md): Halo 1 Xbox models and animation graphs converted to Halo 2 `mode` and `jmad` (`crates/h1models`)
- [h1-ai.md](formats/h1-ai.md): Halo 1 AI data (actors, encounters, command lists) read, Halo 2's scenario AI blocks and `char` decoded, and the conversion between them (`crates/h1ai`)
- [h1-bitmaps.md](formats/h1-bitmaps.md): Halo 1 Xbox bitmaps: where the pixels are, decoding with Halo 2's decoder, conversion to Halo 2 `bitm` and lightmap pages (`crates/h1bitmaps`, `h2 h1 bitmap`)
- [h1-sounds.md](formats/h1-sounds.md): Halo 1 Xbox sounds: where the samples are, decoding to PCM, conversion to Halo 2 `snd!` plus `ugh!` gestalt entries, the whole gestalt layout, class mapping (`crates/h1sounds`, `h2 h1 sound`)
- [h1-objects.md](formats/h1-objects.md): Halo 1 sandbox objects (`weap`, `proj`, `eqip`, `bipd`, `vehi`, `coll`, `phys`) read from Halo 1 and built as Halo 2 tags on retail templates, with the tuning carried over (`crates/h1objects`)
- [h1-sky.md](formats/h1-sky.md): Halo 1 and Halo 2 `sky ` tags: layouts, the disc census, and conversion (`crates/h1sky`)
- [h1-text.md](formats/h1-text.md): Halo 1 `ustr` and `hmt ` text: reading UTF-16 lists and HUD messages, converting them to Halo 2 `unic` locale rows (`crates/h1text`)
- [haloscript.md](formats/haloscript.md): HaloScript syntax, builtins and decoding (`crates/hs`, `h2 hs`)
- [h1-scripts.md](formats/h1-scripts.md): Halo 1 campaign scripts decoded, decompiled, translated to Halo 2 builtins and compiled to Halo 2 nodes (`crates/h1scripts`)
- [lighting.md](formats/lighting.md): level lighting, lightmaps and atmosphere
- [localization.md](formats/localization.md): string IDs and localized text
- [mopp.md](formats/mopp.md): Havok MOPP code in structure BSPs: the engine's opcodes, a query model, and the writer (`crates/mopp`)
- [objects.md](formats/objects.md): object tags (`obje` family, `hlmt`, `itmc`, `vehc`; `h2 object`)
- [pathfinding.md](formats/pathfinding.md): AI pathfinding data in structure BSPs, and generating it from collision (`crates/pathfind`)
- [shader-instances.md](formats/shader-instances.md): shader instance, template and pass tags (`shad`, `stem`, `spas`)
- [sound.md](formats/sound.md): sound tags (`snd!`, `ugh!`) and `h2 sound`
- [structure-bsp.md](formats/structure-bsp.md): render model and structure BSP geometry, lightmaps, and the viewer
- [ui.md](formats/ui.md): user-interface tags (`wgit`, `wgtz`, `wigl`, `skin`)

## Engine (`engine/`)

- [engine-map.md](engine/engine-map.md) (**start here**): survey of the retail engine's subsystems and where to start overriding
- [feature-engine.md](engine/feature-engine.md): the feature engine: Rust subsystems at clean boundaries, hooks, settings; design and build log (phases 0 to 3 built, phase 4 in part)
- [README.md](engine/README.md): contracts for the engine functions rewritten in Rust (`crates/engine`) and their differential tests, indexing:
  [datum_new.md](engine/datum_new.md), [tag_get_checked.md](engine/tag_get_checked.md),
  [locale_string_id_find.md](engine/locale_string_id_find.md), [memory_pool.md](engine/memory_pool.md)
  and the HaloScript VM, [hs_vm.md](engine/hs_vm.md)

## Game features and enhancements (`game/`)

- [settings-menu.md](game/settings-menu.md) (**start here**): the in-game settings screens (Video, Controls), FOV, the settings schema, `h2 settings` and the `H2_*` flag map
- [aim-assist.md](game/aim-assist.md): the Aim Assist setting (Auto, Default, Strong, Very Strong), the weapon and globals fields it scales at map load
- [ui-builder.md](game/ui-builder.md): the UI builder for native screens: pages as pure state machines, one `View` drawn offline from the disc, layout checks, the fidelity loop against game captures
- [ui-screens.md](game/ui-screens.md): the UI kit for list screens of ours: ids, input, previews, the checklist
- [skulls.md](game/skulls.md): campaign skulls per profile, the skulls menu, and our own skulls
- [community-gametypes.md](game/community-gametypes.md): community classics (Zombies, SWAT and more) as extra built-in game variants
- [maps.md](game/maps.md): new, edited, patched and shared maps: packages, patches, `MapId`; foundation built
- [campaigns.md](game/campaigns.md): design for more than one campaign, starting with Halo 1's levels in the Halo 2 engine; the converter is built for multiplayer maps only
- [health.md](game/health.md): Halo 1's health model on Halo 2's engine: no body regeneration, health packs, a health meter (`H2_HEALTH_MODEL`; the map-load switch for Halo 1 maps is not wired yet)
- [bots.md](game/bots.md): design for multiplayer bots (Clankers): one brain feeding controller input, local slots then phantom peers, the Clankers Game Setup page, Clankermatch, and the bot roster interface with the server
- [multiple-campaigns.md](game/multiple-campaigns.md): how 1.0's menus list campaign levels and keep progress, and the campaign picker and per-campaign progress built on them
- [h1-multiplayer.md](game/h1-multiplayer.md): Halo 1's 13 disc multiplayer maps converted onto Halo 2 base maps (`h2 h1 convert`): collision, structure BSP, MOPP, placements, packages with variants; the Halo 1 sandbox and map list are not built
- [map-pack.md](game/map-pack.md): the Multiplayer Map Pack disc, `h2 import-mappack` and how 1.0 lists its maps
- [high-framerate.md](game/high-framerate.md): 60 and 120 fps keeping the 30 Hz tick exact (`H2_FPS`); the determinism harness is built
- [theater.md](game/theater.md): design for saved films, free camera and spectating (not built)
- [forge.md](game/forge.md): design for Forge on Halo 2 multiplayer maps (not built)

## Xbox Live (`live/`)

- [live.md](live/live.md) (**start here**): the Xbox Live replacement: overview, CLI, service and every client HLE section
- [live-api.md](live/live-api.md): the service's HTTP and WebSocket contract
- [live-security.md](live/live-security.md): identity, signing, tokens and federation (built except federation)
- [live-cloud-saves.md](live/live-cloud-saves.md): cloud saves: profiles, variants, campaign saves and profile settings follow the account; the merge and conflict rules
- [live-admin.md](live/live-admin.md): who may publish title files, and the web MOTD editor and playlist builder
- [live-hosting.md](live/live-hosting.md): hosting a server for friends (Cloudflare Tunnel, docker compose)
- [live-boundary.md](live/live-boundary.md): static inventory of the retail XOnline/XNet entry points and the sign-in sequence
- [live-motd.md](live/live-motd.md): how retail fetches and parses `message_of_the_day.dat` and `multiplayer_hopper.dat`
- [live-matchmaking.md](live/live-matchmaking.md): matchmaking, sessions and invites: the retail call trace and what is built
- [live-custom-games.md](live/live-custom-games.md): the custom games browser, its menu row, visibility and joining
- [live-coop.md](live/live-coop.md): campaign co-op over Live: options, decisions, and what is built and probed (Co-op over Live built and confirmed in the game)
- [live-voice.md](live/live-voice.md): voice chat, voice messages and the mute list: how the title does voice, and the host design (design only, not built)
- [live-gamestate.md](live/live-gamestate.md): retail game-state addresses and layouts for the scoreboard and map
- [live-telemetry.md](live/live-telemetry.md): `crates/live-telemetry`: hooks into the game, events and frames
- [live-art.md](live/live-art.md): map geometry and pre-drawn views extracted from the disc for the stats site

## Editor (`editor/`)

- [editor-guide.md](editor/editor-guide.md) (**start here**): user guide to `h2-editor`: map objects, game variants, playlists and Halo 1 maps, step by step with 74 screenshots
- [editor.md](editor/editor.md): the editor for developers (`crates/studio`, Dear ImGui + wgpu): window, the tools, Halo 1 maps and converting, the playlist format rules, code layout, scripted headless screenshots (`--do`, `--crop`, `--callout`), viewport gaps
- [scenario-editor.md](editor/scenario-editor.md): the scenario placement editor (`crates/scenario-edit`): blocks, fields, save rules, packages
- [variants.md](editor/variants.md): the game variant record, its schema and saved files (`crates/variants`)
