# Features, speed and roadmap

Status: Built, as a reference. Each row's state is the state its own page gives; where they differ, the linked page wins.
Code: the pages linked from each row.
Switch: none of its own; the switches it names are listed in [switches.md](runtime/switches.md).

Everything h2recomp adds to or replaces in the retail game, in one table,
with its state and the page that describes it. Then the speed numbers,
kept in three groups: measured, targets and work in flight. The README
gives the short version; this page is the full list.

**Built**: works in the game or the tools. **Partly built**: some of it
works, and its page says which part. **Design**: written up, not built.

## Features

| Area | Feature | State | Docs |
| --- | --- | --- | --- |
| Recompiler | XBE analysis (about 18,700 functions), x86 to IR to C11, clang build with source and object caches | Built | [LIFTER](lifter/LIFTER.md), [cache](lifter/BUILD-CACHE.md) |
| | Native x87 and SSE fast paths, inline guest RAM, direct calls (all on by default) | Built | [x87](lifter/x87-native.md), [memory](perf/guest-memory.md) |
| Runtime | Xbox kernel, XAPI, saves and storage replaced in Rust, on a deterministic logical clock with checkpoints | Built | [BOOT](runtime/BOOT.md), [storage](runtime/storage.md) |
| | Direct3D 8 on wgpu (push buffer, NV2A shaders to WGSL), render thread, merged passes | Built | [d3d](graphics/d3d.md), [graphics](graphics/graphics.md) |
| | DirectSound mixer, native WMA, Bink replacement (cutscenes skipped; optional MP4 conversion) | Built | [audio](audio/audio.md), [movies](audio/movies.md) |
| Player | `h2 play`: window, sound, keyboard, mouse, pads; any level or map from the command line; `--follow` rebuilds as master moves | Built | [app](runtime/app.md), [levels](runtime/level-boot.md) |
| Agent API | `h2 serve` runs the game headless with a JSON-RPC socket; `h2 agent` queries state, drives input, waits on conditions, saves and restores state, replays scenarios | Built | [agent API](runtime/agent-api.md) |
| Sim and hosts | Headless simulation: Halo 2 matches without a window, bots on a host | Built | [sim](runtime/sim.md) |
| | Dedicated and light hosts for Live matches | Partly built | [dedicated host](live/dedicated-host.md) |
| Platforms | macOS (Metal) | Built | [building](runtime/building.md) |
| | iPhone and iPad app, with touch controls | Built | [iOS](runtime/ios.md) |
| | Apple TV app: boots to the menu and plays the bundled levels, slowly | Partly built | [tvOS](runtime/tvos.md) |
| | Windows: the runtime and game module cross-build from a Mac; not run, no host app | Partly built | [windows](runtime/windows.md) |
| | Xbox in Developer Mode (UWP app): runs on a Series X and presents frames while the title boots; pads and levels are not verified | Partly built | [xbox](runtime/xbox.md) |
| Graphics | Native GPU renderer, on by default: world, objects, lights, shadows, transparents, decals, particles, fog, water, post, HUD and UI. Scene lists are built in Rust, the guest's opaque handlers are skipped, geometry is cached across frames, and anything it can't take falls back to D3D (`impl.render.native=off` turns it all off) | Built | [native renderer](graphics/native-renderer.md) |
| | Render scale 1x to 6x (the Resolution row), one copy rendered at the scaled size | Built | [high-res](graphics/high-resolution.md) |
| | Pipelined frames: tick N+1 overlapping frame N (off by default, `impl.render.pipelined_frame`) | Built, off | [pipelined](graphics/pipelined-render.md) |
| | E3 2003-style lighting: the Lighting row (sharper shadow maps, D3D path) | Built | [Lighting row](graphics/e3-lighting-setting.md) |
| | 60 and 120 fps over the 30 Hz tick: the setting, determinism harness, interpolated replay and reprojection | Partly built | [high-framerate](game/high-framerate.md) |
| Game | Expanded settings screens: Video, Controls, FOV, voice | Built | [settings](game/settings-menu.md) |
| | Campaign skulls per profile, a Skulls page, our own skulls | Built | [skulls](game/skulls.md) |
| | 13 community game variants (Zombies, SWAT and more) as built-ins | Built | [variants](game/community-gametypes.md) |
| | Aim assist strength setting | Built | [aim assist](game/aim-assist.md) |
| | Multiplayer bots (Clankers) join hosted matches as phantom peers, and controller ports stay for people; their nav meshes build in the background, again when a map or the generator changes (`H2_BOTS_NAV_AUTO=0` turns that off) | Built | [bots](game/bots.md) |
| | Multiplayer Map Pack installed from your own disc (`h2 import-mappack`, or `mappack.iso` beside the disc) | Partly built | [map pack](game/map-pack.md) |
| | New, edited and patched maps as packages | Built | [maps](game/maps.md) |
| | More than one campaign: picker and per-campaign progress | Built | [campaigns](halo1/campaign-picker.md) |
| | Halo 1 health model and meter | Built | [health](halo1/health.md) |
| | Theater: `.h2film` films recorded by dedicated hosts, played back with the game's renderer and director cameras | Partly built | [theater](game/theater.md), [film format](game/film-format.md) |
| | Forge | Design | [forge](game/forge.md), [design](game/forge-design.md) |
| Halo 1 | Halo 1 campaign levels convert offline into Halo 2 maps for the Halo 2 engine, with Halo 1 assets and a Halo 2 setting | Partly built | [campaign](halo1/campaign.md) |
| | The 13 Halo 1 multiplayer maps convert to Halo 2 maps (collision, BSP, MOPP, objects, shaders) | Built | [h1 multiplayer](halo1/multiplayer.md) |
| | Halo 1 maps, bitmaps, models and animations read and converted to Halo 2 tags | Built | [h1 format](halo1/map-format.md), [models](halo1/models.md) |
| Input | Remappable bindings, input timelines, typing into the game's keyboard | Built | [input](input/input.md) |
| | Button icons that follow the pad (Xbox, PlayStation, keyboard); extra button layouts | Built | [icons](input/button-icons.md), [layouts](input/controller-layouts.md) |
| | DualSense: speaker and mute button | Built | [PlayStation](audio/playstation-audio.md) |
| | Steam Controller | Built | [Steam](input/steam-controller.md) |
| Live | Service: accounts, friends, teams, invites, stats, title files, cloud saves, custom maps, a relay | Built | [live](live/live.md), [API](live/live-api.md) |
| | In the game: sign-in, matchmaking, custom games browser | Built | [matchmaking](live/live-matchmaking.md), [custom games](live/live-custom-games.md) |
| | Co-op campaign over Live | Built | [co-op](live/live-coop.md) |
| | Cloud saves: profiles, campaign progress, variants and settings follow the account | Built | [cloud saves](live/live-cloud-saves.md) |
| | Web app: dashboard, stats site, live top-down map, replays, admin | Built | [web](../web/README.md), [web-app](live/web-app.md), [admin](live/live-admin.md), [art](live/live-art.md) |
| | Hosting: Docker with Cloudflare Tunnel, SQLite and R2; or Cloudflare Workers | Built | [hosting](live/live-hosting.md), [Workers](live/live-worker.md) |
| Editor | `h2-editor`: 3D viewport, placements, tags, game variants, playlists, UI screens, Halo 1 maps, with its own socket API | Built | [guide](editor/editor-guide.md), [API](editor/agent-api.md) |
| Tools | Tag decoders (`h2 bitmap`, `sound`, `ui`, `hs`, ...), map builder, profiler, benchmarks | Built | [formats](README.md#data-formats-and-tags-formats), [perf](perf/profiling.md) |

## Speed

Three different things, kept apart. Only the first table holds numbers
anyone has taken.

### Measured

| What | Before | After | Where |
| --- | --- | --- | --- |
| Native world prep | 5.9 ms/frame | 0.19 ms/frame | Headless Metal, Old Mombasa gameplay, 4x, 2026-11-20 |
| World upload | 3.1 MB/frame | 38 KB/frame | same |
| 1x reference copy | 295 draws | 0 draws | same |
| Guest opaque handlers skipped | 0 | 124 handlers (291 draws) per frame | same |
| Dedicated host, Waterworks, 4 bots | 14.96 ms/tick | 14.12 ms/tick | Five-minute run, same seed, 2026-11-20 |
| Dedicated host, Lockout | 9.07 ms/tick | 8.49 ms/tick | same |
| Campaign frame rate | | steady 30 fps, dips with 6+ enemies | Owner's M3, 4K, 4x |
| Headless gameplay, 600 ticks, native on vs off | 34.3 fps (off) | 32.4 fps (on) | Old Mombasa, 4x, M3, quiet machine, 2026-11-20 |
| Flat guest RAM, Waterworks 2v2 | 10.11 ms/tick (off) | 21.67 ms/tick (on), state diverges at tick 6 | 3600 ticks, same seed, 2026-11-20 |

Two of those rows are regressions. The native renderer (on by default) is
about 6% slower than the D3D path at 4x in that run (its submit is
1.4 ms/frame dearer, with object and light fallbacks still drawing through
the guest), and flat guest RAM (off by default) doubles the tick and breaks
determinism. The host numbers are one run each, and same-build
repeatability on Waterworks is open ([sim-budget.md](perf/sim-budget.md)).
The A/B record of the native renderer, including the rounds that were
slower than D3D, is in [native-ab.md](graphics/notes/native-ab.md); the
gameplay frame budget is in
[gameplay-budget.md](perf/notes/gameplay-budget.md).

<p align="center">
  <img src="images/readme/measured-costs.svg" alt="Bar chart of the measured costs before and after" width="100%">
</p>

### Targets

| Target | Measured |
| --- | --- |
| Uncapped frame rates; a locked 30 fps at 4x on an M3 first | 32-34 fps headless, dips with 6+ enemies |
| Sim about 2x faster per tick: Waterworks at 7.48 ms/tick | 14.12 ms/tick |
| One render path: sim, then native render, nothing drawn twice | Native on by default; 0 reference-copy draws in the measured scene, other scenes unchecked |

### In flight

Built or being built, not measured. The figures are estimates, not timings.

| Work | Estimate |
| --- | --- |
| 32 native sim functions replacing guest code | 0.5-0.6 ms/tick for each batch |
| SIMD transform and collision | not estimated |
| Pipelined frames (tick N+1 overlapping frame N) | not estimated |

<p align="center">
  <img src="images/readme/render-pipeline.svg" alt="Render pipeline before and after: guest front-end, D3D translate, two copies; then native scene lists and one native render" width="100%">
</p>

## Roadmap

No dates. Items move when they are measured, not when they are written down.

<p align="center">
  <img src="images/readme/roadmap.svg" alt="Roadmap: now, next, later" width="100%">
</p>

**Now**

- A locked 30 fps at 4x on an M3: the rest of the gameplay frame and the
  residual render callers
  ([native-renderer.md](graphics/native-renderer.md),
  [gameplay-budget.md](perf/notes/gameplay-budget.md)).
- Measure the in-flight sim work (flat guest RAM, the native sim batches) on
  the Waterworks and Lockout cases ([sim-budget.md](perf/sim-budget.md)).
- Fidelity gaps, such as point sprites and the open items in
  [BOOT.md](runtime/BOOT.md) and [shaders.md](graphics/shaders.md).

**Next**

- Pipelined frames on by default, once hash parity holds
  ([pipelined-render.md](graphics/pipelined-render.md)).
- SIMD transform and collision; rewrites of the hot guest families in
  [sim-budget.md](perf/sim-budget.md), toward 7.48 ms/tick on Waterworks.
- One render path: every scene native, the D3D fallback for leftovers only.

**Later**

- 60 and 120 fps over the 30 Hz tick in the game, and the rest of E3 lighting.
- Theater and Forge.
- A Windows and Xbox host app.

## Not verified

- The rows marked Partly built list their open items on their own pages.
- The frame rates above 30 fps are not measured on this page; the
  60 and 120 fps work's own record is [high-framerate.md](game/high-framerate.md).

## Related

[ARCHITECTURE.md](ARCHITECTURE.md), [building.md](runtime/building.md),
[app.md](runtime/app.md), [live.md](live/live.md).
