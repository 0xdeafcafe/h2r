# Crate structure: the target map and the moves to it

Status: Design. No crate has moved. Built: `scripts/crate-table.py` (the
crate table in [ARCHITECTURE.md](../ARCHITECTURE.md)) and
`scripts/crate-graph.py` (every number on this page).
Measured at: `21ca66709` (master `100f13cef` plus docs), 2026-11-20, with
`scripts/crate-graph.py`. Nothing was compiled or run.

This page carries out section 2 and item 8 of
[architecture-review-2026-11.md](../architecture-review-2026-11.md): where
every crate ends up, why, and the order of moves that gets there while
about eight lanes keep merging. It has three rules. A move is mechanical
(`git mv`, path fixes, no logic). A move is a script, so a lane replays it
on its own branch instead of resolving conflicts. Lists that every lane
appends to become lists that each crate keeps for itself.

Build and test speed (profiles, one test binary per crate, nextest) is
`build-1120`'s work in `docs/dev/build-speed.md` on that branch. This page
takes its follow-ups 3 to 6 (split `boot`, thin `cli`, the commit hash in
`app`, `Cargo.lock` in `boot`'s build script) and gives them an order.

## The current graph

87 packages, 1,151,360 lines of Rust (952,617 under `src/`), 390
integration test binaries. `scripts/crate-graph.py sizes` prints the row
for every crate.

| Family | Crates | Lines | Members (thousand lines) |
|---|---:|---:|---|
| Disc and formats | 18 | 99,063 | `tags` 43.1, `shaders` 11.0, `hs` 7.0, `content` 7.0, `map` 5.3, `variants` 4.3, `font` 4.0, `timeline` 3.7, `mopp` 2.8, `wma` 2.4, `movies` 1.9, `settings` 1.6, `pathfind` 1.4, `asf` 1.2, `assets` 1.1, `xiso` 0.5, `xbe` 0.5, `app-icon` 0.1 |
| Recompiler core | 12 | 58,232 | `emit` 15.4, `x87` 8.7, `analyze` 8.5, `lift` 7.9, `ir` 3.7, `fastmem-proto` 2.6, `oracle` 2.5, `simd` 2.2, `clif` 2.1, `sse-oracle` 2.1, `opt` 1.4, `symbols` 1.3 |
| Runtime | 7 | 63,761 | `kernel` 21.3, `runtime` 18.1, `vfs` 14.4, `perf` 6.1, `lockstep` 1.9, `snapshot` 1.8, `runtime-test-probe` 0.1 |
| Hooks and ports | 3 | 75,101 | `engine` 36.9, `feature` 36.1, `hot-tags` 2.1 |
| Guest-facing HLE | 8 | 140,448 | `d3d` 68.5, `gfx` 18.2, `dsound` 14.4, `input` 14.0, `audio` 10.5, `xapi` 9.7, `bink-hle` 3.0, `voice` 2.3 |
| Native render | 3 | 37,199 | `native-render` 28.5, `viewer` 7.3, `hot-render` 1.4 |
| Agent API | 3 | 23,085 | `agent` 15.0, `agent-proto` 5.2, `debug-menu` 2.9 |
| Game product | 3 | 171,986 | `boot` 158.2, `app` 8.9, `sim` 4.9 |
| Bots, co-op, Live (other session) | 10 | 218,075 | `live-server` 71.9, `bots` 44.0, `live-hle` 33.1, `live-proto` 29.3, `live-telemetry` 17.1, `live-worker` 6.7, `film` 6.4, `coop` 3.9, `cloud-saves` 3.4, `host-data` 2.3 |
| Halo 1 conversion | 11 | 136,619 | `h1objects` 45.0, `h1convert` 31.8, `h1campaign` 14.8, `h1scripts` 12.9, `h1models` 9.2, `h1sounds` 6.0, `h1ai` 5.6, `h1map` 4.0, `h1text` 2.6, `h1bitmaps` 2.5, `h1sky` 2.1 |
| Editor | 6 | 45,456 | `studio` 25.0, `scenario-edit` 11.1, `hud-edit` 3.0, `hopper-edit` 3.0, `editor-core` 2.6, `studio-proto` 0.7 |
| Binaries and hosts | 3 | 82,335 | `cli` 78.9, `apple-host` 2.3, `uwp-host` 1.1 |

### Shape

The bottom is clean and the top is one knot.

- **Recompiler core is a chain.** `xbe` → `analyze` → `lift` → `ir`
  (with `x87`, `simd`), then `opt`, `emit`, `clif`. `runtime` depends only
  on `timeline`, `x87` and `xbe`. Nothing here needs to move.
- **The hook registry sits above the renderer.** `feature` depends on
  `engine` and, by default, on `native-render`, which depends on `d3d`.
  Of `feature`'s 29,212 source lines, 2,980 are the hook engine (`lib.rs`,
  `plan.rs`, `install.rs`, `entry.rs`, `arena.rs`, `bridge.rs`,
  `snapshot.rs`, `switches.rs`, `locale.rs`, `leaves.rs`); 24,325 are the
  native renderer's guest half, 1,349 the sim hooks and 558 Halo 1 hooks.
- **`engine` is four things.** Of 27,964 source lines: named points 1,261;
  typed views of guest memory 2,238 (`memory`, `handle`, `datum`, `view`,
  `tags`, `testing`); render ports 12,266 (`render_loop/` is 5,933 of
  them); register-level sim ports 8,981 (`sim_native/`, `sim_physics/`);
  leaf ports and their adapters 3,218.
- **`boot` is the knot.** 158,162 lines, 318 files, 153 `mod` lines, 52
  workspace dependencies. `run_configured`
  (`crates/boot/src/lib.rs:1086`) is one function of 1,524 lines. By
  content, from `scripts/crate-graph.py modules crates/boot/src`:

  | Group | Lines | Modules | What |
  |---|---:|---:|---|
  | game | 51,918 | 78 | menus and screens (mods, map select, skulls, variants, settings pages, campaigns), gameplay hooks (aim assist, cheats, Halo 1 health, HUD swap, touch), HLE glue (`dsound_glue`, `xapi_glue`, `agent_glue`, ...) |
  | render glue | 22,997 | 33 | `native_*_glue`, `d3d_glue`, `d3d_diag`, `render_thread`, `opaque_streams` |
  | Halo 1 runtime | 20,507 | 14 | `h1_ai_control`, `h1_conversation`, `h1_hud_text`, `h1_command_atoms`, `h1_script_builtins`, `h1_cinematics`, ... |
  | UI toolkit | 17,171 | 5 | `ui_kit/`, `list_class`, `ui`, `icons`, `keyboard_glue` |
  | Live, co-op, hosts | 16,655 | 13 | `coop_*`, `host_lobby`, `live_glue`, `live_notice`, sign-in and account screens, `community_variants` |
  | theater | 10,863 | 2 | `theater/`, `theater_ui/` |
  | bots and sim mode | 8,776 | 8 | `bot_glue`, `clankers*`, `clankermatch`, `bots_nav_auto`, `bot_mark`, `sim_bots`, `sim_glue` |
  | composition root | 3,948 | 2 | `lib.rs`, `registration.rs` |
  | build and load | 2,613 | 7 | `fallback`, `shim`, `sse`, `x87`, `tier`, `host_coverage`, plus about 600 lines of `lib.rs` |

- **`cli` is nine programs in one binary crate.** 78,905 lines: the Live
  art pipeline 27,679, test drivers and verdicts for bots and co-op
  10,985, host commands (serve, sim, host, theater, agent) 7,488, play,
  settings and movies 3,710, tag and asset inspection 8,841, Live client
  and admin commands 7,518, dev tooling (`lab`, `follow`, `web_build`)
  5,465, Halo 1 and map-pack import 4,508, `main.rs` and the recompiler
  commands 1,909.
- **The game links its offline converters.** `boot` depends on
  `h1convert`, `h1scripts`, `h1text` and `h1map` for about twenty names
  (`h1convert::shad::COUNTDOWN_DIGITS`, `h1scripts::cinematics::ACTIVE_NAME`,
  `h1text::hud::SCENARIO_HELP`, ...).

### What an edit recompiles

kache is the rustc wrapper and turns incremental compilation off, so a
changed crate is compiled whole, and so is every crate above it
(build-speed.md). The rebuild cone of a crate is itself plus its
dependents, as a share of the workspace's 952,617 source lines:

| Edit in | Cone | Why |
|---|---:|---|
| `map` | 83% | 30 crates depend on it directly |
| `xbe`, `timeline` | 73%, 71% | leaf utilities under `runtime` |
| `runtime`, `tags`, `content` | 65%, 64%, 62% | |
| `live-proto` | 48% | 12 direct dependents, among them `boot`, `bots` and `film` |
| `engine` | 42% | a new point in `points.rs` recompiles `feature`, `bots`, `agent`, `boot`, `cli` |
| `d3d`, `native-render`, `feature` | 41%, 37%, 34% | |
| anything `boot` depends on | at least 27% | the floor: `boot` 155,448, `cli` 77,636, `studio`, the hosts |

Of 803 non-merge commits that touched `crates/boot/src`, 307 touched the
render glue (197 of them nothing else in `boot`), 281 the game group, 277
the composition root (239 of those alongside another group, which is what
a central registry looks like), 96 the bot glue, 91 the UI toolkit and 85
the Halo 1 runtime. Each of them recompiled all of `boot` and all of
`cli`: 233,000 lines.

### The host-only build

`scripts/crate-graph.py closure cli --features host`: 62 of 87 crates and
813,415 of 952,617 source lines (85%). The desktop build is 71 crates and
861,157 lines, so the host feature saves 5%. Three edges account for most
of what should not be there:

- `crates/cli/src/live_accounts.rs:20` uses one function,
  `live_server::identity::write_private`. It is the only use of
  `live-server` outside `#[cfg(feature = "desktop")]`, and it puts
  51,962 lines plus axum, sqlx, reqwest and object_store in the host
  build.
- `crates/h1bitmaps/Cargo.toml` takes `gfx` with default features, so
  `boot` → `h1convert` → `h1bitmaps` turns on `gfx/gpu` and compiles wgpu
  for a dedicated host.
- `boot` → `h1convert` brings `h1bitmaps`, `h1models`, `h1sky`,
  `h1sounds`, `h1text`, `scenario-edit`, `shaders` and `mopp` (about
  60,000 lines) for the twenty names above.

### Merge hot spots

3,958 commits, 1,213 of them merges. Non-merge commits per file:

| File | Commits | Kind |
|---|---:|---|
| `Cargo.lock` | 274 | generated |
| `crates/boot/src/lib.rs` | 249 | composition: every install line |
| `crates/cli/src/h1_import.rs` | 147 | real work |
| `crates/engine/src/points.rs` | 143 | list: 374 points in one `points!` table |
| `crates/d3d/src/device.rs` | 122 | real work (6,688 lines) |
| `crates/cli/src/main.rs` | 112 | list: 85 `mod` lines, the usage text, the dispatch chain |
| `crates/boot/src/registration.rs` | 74 | list: every `features.add` |
| `crates/boot/src/layout_glue.rs` | 56 | names 23 other modules |
| `crates/boot/src/setting_defs.rs` | 34 | list: every setting `Def`; 36 modules read it |
| `crates/boot/src/ui_kit/screens.rs` | 24 | list: every screen's description |

`setting_defs.rs`, `layout_glue.rs`, `ui_kit/screens.rs` and
`string_ids.rs` are central registries the review did not name. They are
also the reason the UI toolkit cannot leave `boot` yet: it names 17
screens through them.

### Against the review

Section 2 of the review was written at `bc48d2053`. What differs here:

| Review | This tree |
|---|---|
| 88 crates, 1.1 M lines | 87 packages, 1,151,360 lines |
| `boot` 144,722 lines, 278 files, 149 `mod` lines, fan-out 51 | 158,162, 318, 153, 52 |
| `run_configured` about 1,460 lines at `lib.rs:1067` | 1,524 lines at `lib.rs:1086` |
| `cli` 78,215 lines, 117 files | 78,905, 120 |
| `feature` 35,843 lines, hook machinery about 2,500 | 36,138; 2,980 |
| `boot/src/lib.rs` 242 commits, `points.rs` 123, `main.rs` 111 | 249, 143, 112 |
| 466 `"H2_*"` literals, 40 `impl.*` names | 475; 40 (35 `impl.render.*`) |
| 9,797 `#[test]`, 504 `#[ignore]` | 9,741; 530 |
| ARCHITECTURE.md's table lacks 24 crates | generated now; 30 manifests have no `description` |
| `boot` by content: UI 40,000, renderer 17,000, net 10,000, bots 6,000 | the table above; the Halo 1 runtime (20,507) was not counted |
| the art pipeline in `cli` about 20,000 | 27,679 |

`integ-1120` (flag cleanup, native cost) is 89 commits ahead and removes
5,136 lines from `boot`, 3,355 from `feature` and 817 from `d3d`. The
moves below wait for it.

## The target map

### Rules

1. **Flat `crates/<name>`, layer in the manifest.** Each crate declares
   `[package.metadata.h2] layer`. `scripts/crate-graph.py layers --check`
   refuses a dependency from a lower layer to a higher one. Nesting
   directories by layer would rename every path in every worktree and
   break `include!("../../../scripts/it_guard.rs")`, `boot`'s
   `include_str!("../../emit/c/bridge_x87.c")` and its `build.rs`, which
   hashes `../analyze` and friends.
2. **A crate is a rebuild unit.** Split where edits are frequent and the
   cone is large. Do not merge crates into a unit over about 30,000 lines.
3. **Guest offsets live in two places:** `guest-layout`, and the bridge
   crate of the subsystem that owns the structure. Nothing else reads a
   raw offset.
4. **Subsystem crates are guest-free.** `sim-*`, `frame-scene` and
   `native-render` take typed values. A bridge crate (`sim-bridge`,
   `render-bridge`) does the reading and the hook installation.
5. **A list lives in the crate that owns its entries** (see "Local
   registration").
6. **The host-only build is a budget.** The gate records the crate count
   of `closure cli --features host` and refuses a rise.

### Diagram

Higher depends on lower. `[x]` belongs to the Live, bots, theater and
film owner. `+` is a new crate; `~` is renamed or reshaped.

```
 BINARIES    cli (h2: main, command registry)        studio (h2-editor)     uwp-host (h2uwp)
             +cmd-host +cmd-play +cmd-tags +cmd-recomp +cmd-h1 +cmd-dev     ~h2host (tvOS, iOS, UWP)
             [+cmd-live] [+cmd-drive]                                       [live-server] [live-worker]
 ───────────────────────────────────────────────────────────────────────────────────────────────────
 PRODUCT     ~boot  composition root: stages, the crate list           app  window, surface, pacing
             +h1-runtime    [+bot-glue]    [+net-glue]    [+theater]   agent  the agent API server
             +game  menus, settings pages, skulls, variants, gameplay hooks, HLE glue
             +ui-kit  the guest UI toolkit: list screens, dialogs, pickers, icons
 ───────────────────────────────────────────────────────────────────────────────────────────────────
 RENDER      +render-bridge  guest -> FrameScene; the hooks and D3D admission until then
             native-render   FrameScene -> wgpu            +frame-scene  the typed scene, plain data
             d3d  guest Direct3D 8; UI, HUD, Bink layer    gfx   assets   hot-render
 ───────────────────────────────────────────────────────────────────────────────────────────────────
 SUBSYSTEMS  +sim-bridge  sim adapters, installs, tick driver
             +sim-anim  +sim-collision  +sim-state  +sim-core      engine  leaf ports, exact maths
             dsound audio   xapi input   bink-hle   voice   hot-tags   lockstep   debug-menu
             [bots] [coop] [live-hle] [live-telemetry] [cloud-saves]
 ───────────────────────────────────────────────────────────────────────────────────────────────────
 HOOKS       ~hooks (feature)  points -> plan -> install; entries; guest arena; shadow execution
             +guest-layout     records, globals, shared points: one definition per guest structure
 ───────────────────────────────────────────────────────────────────────────────────────────────────
 RUNTIME     +recomp  XBE -> module: stamps, analysis cache, build, load, fallback, fast tier
             +lifted  dev: the lifted original as a test oracle
             kernel   vfs   snapshot
             runtime  guest memory, CPU, dispatch, runner, h2abi.h      runtime-test-probe
 ───────────────────────────────────────────────────────────────────────────────────────────────────
 RECOMPILER  emit (IR -> C)   clif (IR -> Cranelift)   +sim-tier (tick closure, relaxed contract)
 CORE        opt   lift   ir   analyze   symbols   x87   simd   oracle   sse-oracle
 ───────────────────────────────────────────────────────────────────────────────────────────────────
 FORMATS     xiso xbe map tags hs mopp pathfind font shaders content variants asf wma movies
             settings timeline +switches +cmd ~sim-bench +h1-contract agent-proto studio-proto
             app-icon [live-proto] [film]

 TOOLS (side stack: uses up to RENDER; only BINARIES use it)
             h1map  +h1-assets  h1convert  h1objects  ~h1campaign      perf
             editor-core  scenario-edit  hopper-edit  hud-edit
 LIVE (side stack, the service)     [live-server] [live-worker] [host-data] [+live-art]
```

### Crates

One line each for what is new or changes. Crates not listed keep their
purpose and place; ARCHITECTURE.md has their rows.

| Crate | Layer | Purpose | Comes from |
|---|---|---|---|
| `switches` | formats | The typed switch table: name, default, owner, removal condition; a name not in it does not parse | `feature/src/switches.rs`; the `guest-layout` lane |
| `cmd` | formats | `Command { name, about, run }`, the registry, the usage text built from it, the disc and output helpers every command shares | `cli/src/main.rs`, `safe_out.rs` |
| `sim-bench` | formats | Sim mode's pure parts: match and batch files, pacing, metrics, the A/B comparison | `sim`, renamed |
| `h1-contract` | formats | The names and ids the Halo 1 converters write and the game's Halo 1 hooks read | about twenty items in `h1convert`, `h1scripts`, `h1text`, `h1objects` |
| `sim-tier` | recompiler | The tick's call closure compiled under the relaxed contract ([sim-review.md](../engine/sim-review.md) 4.2): signature recovery, the tier emitter, the bisect manifest | the `sim-tier` lane |
| `recomp` | runtime | An XBE to a loaded module: build stamps, the analysis cache, lift, emit, compile, `Module`, the fallback interpreter, the Cranelift tier, the bridge C | `boot/src/lib.rs` lines 196 to 822, `fallback`, `shim`, `sse`, `x87`, `tier`, `host_coverage`, `boot/c`, `boot/build.rs` |
| `lifted` | runtime (dev) | The lifted original of a guest function as a test oracle, and the shadow-execution comparison | the `shadow-exec` lane (`crates/lifted`) |
| `guest-layout` | hooks | One definition per guest structure, generic over the memory type; globals; the points several crates share; per-point verification | the `guest-layout` lane; `engine/src/{points, memory, handle, datum, view, tags, testing}.rs` |
| `hooks` | hooks | The hook engine and nothing else: `Feature`, `Registry`, `Plan`, install, entries, the guest arena, snapshot policy, `Host` and `modules!` | `feature` minus its render, sim and Halo 1 modules; `engine/src/entry.rs` |
| `engine` | subsystems | Exact shared arithmetic (`light_constants`, `function_eval`), the leaf ports (locale, datum pools, HaloScript pools, squads) and, until each is superseded, the register-level sim ports as oracles | what is left of `engine`: about 13,000 lines, 4,000 once the oracles go |
| `sim-core` | subsystems | Shared maths, exact SSE-order lanes, the float rules, `Jobs` | the `sim-anim` lane |
| `sim-anim` | subsystems | Decoded clips and the node-record sampler | the `sim-anim` lane |
| `sim-collision` | subsystems | Decoded BSP and MOPP trees, closed query kernels | the `sim-collision` lane |
| `sim-state` | subsystems | The read-only API for theater, film, stats and Live: `SimRead`, events, keyframes, over `guest-layout` | [sim-rewrite.md](../engine/sim-rewrite.md) section 7 |
| `sim-bridge` | subsystems | The guest side of the sim: adapters from entries to `sim-*`, the installs under `impl.sim.native.<subsystem>`, the light-host skips, the tick driver | `feature/src/sim_*.rs`, `engine/src/sim_anim.rs` |
| `frame-scene` | render | `FrameScene`: camera, clusters, instances and palettes, lights, transparents, effect quads, post parameters. Plain data, no wgpu, no guest | the `framescene-design` lane |
| `render-bridge` | render | The only render code that knows guest offsets. Today the stage hooks, admissions and D3D glue; then extraction to `FrameScene` | `feature/src/{native_*, pipeline_*, render_*, occlusion}`, `engine`'s render ports, `boot/src/{native_*_glue, d3d_glue, d3d_diag, render_thread, ...}` |
| `native-render` | render | `FrameScene` to wgpu: owns its targets and pass graph, no `d3d`, no guest reads | as is, reshaped by the FrameScene work |
| `ui-kit` | product | The guest UI toolkit: list-screen class, dialogs, pickers, pages, icons, the host keyboard | `boot/src/{ui_kit/, list_class, ui, icons, keyboard_glue}` |
| `game` | product | Menus and screens, settings pages, skulls, variants, campaigns, gameplay hooks, HLE glue | `boot`, minus everything else on this table |
| `h1-runtime` | product | The Halo 1 campaign's in-game half: AI control, conversations, HUD text, script builtins, cinematics | `boot/src/h1_*` (14 modules), `feature/src/h1_*.rs` |
| `bot-glue` | product | Bots in the game process: phantom input, nav builds, the Clankers pages, sim mode | `boot/src/{bot_glue, bots_nav_auto, bot_mark, clankers*, clankermatch, sim_bots, sim_glue}` |
| `net-glue` | product | Live and co-op in the game process: the co-op session, JIP, look, lobby, sign-in and account screens | `boot/src/{coop_*, host_lobby, live_glue, live_notice, signin_*, account_screen, community_variants}` |
| `theater` | product | Films and theater: recording, replay, the theater UI | `boot/src/{theater/, theater_ui/}` |
| `boot` | product | The composition root: the boot stages, the list of crates to register, the run loop | `boot/src/lib.rs` from `RunOpts` down, `registration.rs` |
| `cmd-host` | binaries | `h2 serve`, `sim`, `host`, `theater`, `agent`: all the host-only build needs | `cli/src/{boot_cmd, sim_cmd, sim_store_cmd, host_cmd, theater_cmd, agent_*}` |
| `cmd-play` | binaries | `h2 play`, `settings`, `movies` | `cli/src/{play_cmd, settings_cmd, movies_cmd}` |
| `cmd-tags` | binaries | The tag, map and asset inspection commands | 19 modules of `cli/src` (`anim`, `bitmap`, `collision`, ..., `ui`) |
| `cmd-recomp` | binaries | `h2 build`, `info`, `ls`, `extract`, `disasm`, `symbols`, `analyze`, `lift`, `emit` | `cli/src/main.rs` lines 599 to 1297, `emit_cache`, `emit_output` |
| `cmd-h1` | binaries | `h2 h1`, `import halo1`, `import-mappack` | `cli/src/{h1_cmd, h1_import, mappack_cmd}` |
| `cmd-dev` | binaries | `h2 lab`, `play --follow`, the web build | `cli/src/{lab, follow, web_build}` |
| `cmd-live`, `cmd-drive`, `live-art` | binaries, live | Live client and admin commands; bot and co-op test drivers with their verdicts; the Live web art pipeline as a library | `cli/src/live_*`, `play_live`, `tunnel`; `bots_*`, `coop_*`, `lockstep_cmd` |
| `h2host` | binaries | The embedded host: the static library a native app shell (UIKit, UWP) calls to boot the game | `apple-host`, renamed to its library's name |
| `h1-assets` | tools | Halo 1 bitmaps, models, sounds, skies and text as Halo 2 tags | `h1bitmaps`, `h1models`, `h1sounds`, `h1sky`, `h1text` (22,494 lines) |
| `h1campaign` | tools | Halo 1 campaign levels as Halo 2 maps, with their AI and scripts | `h1campaign`, `h1ai`, `h1scripts` (33,318 lines) |

Gone: `fastmem-proto`, `viewer`, `sim` and `apple-host` (renamed), nine
`h1*` crates into two, and `feature`'s name if decision 3 says so. About
87 packages become about 100. The count goes up; the size of what an edit
recompiles goes down.

### Where each running lane lands

| Lane | Home | Notes |
|---|---|---|
| `guest-layout` | `crates/guest-layout`, `crates/switches` | Lands as new crates. Step C3 then moves `engine`'s views and the shared points in, behind re-exports. `sim-state` builds on it and holds no second copy of a layout (decision 7). |
| `shadow-exec` | `crates/lifted` | Lands as a new crate. Afterwards `engine`'s `differential` feature and `native-render`'s optional `analyze`, `ir`, `lift`, `xbe` become a dev-dependency on it. The agent API method for a shadow run goes in `agent`; the fork-and-compare hook is `hooks`. |
| `sim-anim` | `crates/sim-core`, `crates/sim-anim`; adapter in `sim-bridge` | Its branch puts the adapter in `engine/src/sim_anim.rs` and the install in `feature/src/sim_native.rs`. Step C2 moves both to `sim-bridge`, one file per subsystem. |
| `sim-collision` | `crates/sim-collision`; adapter in `sim-bridge` | Same. It and `sim-anim` both edit `feature/src/sim_native.rs` today; after C2 each has its own file. |
| `sim-tier` | `crates/sim-tier`; thunks in `runtime`; the tick driver in `sim-bridge` | The tier is a second lowering of `ir`, so it sits beside `emit` and `clif`. |
| `contract-design` | `emit`, `runtime`, `clif`; build flags and stamps in `recomp` | Step B1 moves the build pipeline out of `boot` before its implementers start, so thin transfers and flat RAM never touch `boot/src/lib.rs`. |
| `framescene-design` | `crates/frame-scene`, `crates/render-bridge`, `crates/native-render` | Step C4 gathers the guest half in one crate before implementation, D2 adds `boot`'s render glue. |
| `build-1120` | root `Cargo.toml` profiles, `tests/it.rs`, `.config/nextest.toml`, `docs/dev/build-speed.md` | Moves keep `autotests = false`, `tests/it.rs` and `scripts/it_guard.rs` as they are. A new crate with more than three test files starts with `tests/it.rs`. |
| Live, bots, theater, film | `[x]` crates above | Proposals, in their own section. |

### What it buys

Lines of Rust recompiled by `cargo check -p cli` after an edit (**S**:
computed from the measured sizes and the target graph, not timed):

| Edit in | Today | After D1 to D6 and E1 | After the game group is thinned |
|---|---:|---:|---:|
| the render glue | 233,000 | 41,000 | 41,000 |
| the Halo 1 runtime | 233,000 | 39,000 | 39,000 |
| bot glue, theater, Live glue | 233,000 | 27,000 to 35,000 | same |
| the composition root | 233,000 | 18,000 | 18,000 |
| a menu or a gameplay hook | 233,000 | 150,000 | about 60,000 |
| `engine/src/points.rs` (a new point) | 405,000 | the owning crate's cone | same |
| the art pipeline | 78,000 | 28,000 | 28,000 |

"Thinned" means D7 and D8: `ui-kit` below `game`, and the registries
local, so the crates above `game` depend on `ui-kit` and `guest-layout`
instead of all of `game`.

The host-only build loses `live-server` and its third-party tree (step
A3), wgpu (A3), the converters (F1), and `render-bridge`, `native-render`
and the desktop command crates by construction.

## Local registration

One signature, one list per crate, sorted, one word per line.

```rust
// crates/hooks/src/host.rs
/// What a registering module may ask the host for. Services are looked up
/// by type, so `hooks` names no crate above it.
pub struct Host<'a> {
    pub rt: &'a Arc<Runtime>,
    pub data_root: &'a Path,
    services: &'a Services, // TypeId -> Arc<dyn Any + Send + Sync>
}

impl Host<'_> {
    /// `host.get::<live_hle::Hle>()`: `None` on a run without it (a test,
    /// a headless host), and the module then registers less or nothing.
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> { /* ... */ }
}

/// One module's registration: the only signature a lane has to know.
pub type Register = fn(&mut Registry, &Host<'_>) -> anyhow::Result<()>;

/// A crate's list of modules. Declares them, and a `register` that runs
/// each one's `register` and names the module in any error.
#[macro_export]
macro_rules! modules {
    ($($module:ident),* $(,)?) => {
        $(pub mod $module;)*
        pub const MODULES: &[(&str, $crate::Register)] =
            &[$((stringify!($module), $module::register)),*];
        pub fn register(reg: &mut $crate::Registry, host: &$crate::Host<'_>) -> anyhow::Result<()> {
            $crate::register_all(MODULES, reg, host)
        }
    };
}
```

A module owns everything it used to add to a central file: its points,
its settings, its string ids, its layouts, its hooks.

```rust
// crates/game/src/skulls_mp.rs
hooks::points! { POINTS;
    /// The host's skull set, applied at game start.
    SKULLS_MP_APPLY @ 0x0000_0000: [fn(mask: eax) ret 0]; "docs/game/skulls.md";
}

settings::defs! { DEFS;
    SKULLS_MP: bool = false, key "skulls.multiplayer", screen Skulls;
}

pub fn register(reg: &mut Registry, host: &Host<'_>) -> anyhow::Result<()> {
    reg.points(POINTS);     // duplicate names and addresses fail the plan
    reg.settings(DEFS);     // what setting_defs.rs lists today
    let Some(store) = host.get::<settings::Store>() else { return Ok(()) };
    reg.add(&SkullsMp { store, live: host.get::<live_hle::Hle>() })?;
    Ok(())
}
```

```rust
// crates/game/src/lib.rs: the one line a lane adds, in its own crate.
hooks::modules! {
    aim_assist,
    campaign_menu,
    cheats,
    // ...
    skulls_mp,
    video_screen,
}
```

```rust
// crates/boot/src/stages/register.rs: the only list that names crates.
const CRATES: &[(&str, hooks::Register)] = &[
    ("ui-kit", ui_kit::register),
    ("game", game::register),
    ("sim-bridge", sim_bridge::register),
    #[cfg(feature = "gpu")]
    ("render-bridge", render_bridge::register),
    ("h1-runtime", h1_runtime::register),
    ("bot-glue", bot_glue::register),
    ("net-glue", net_glue::register),
    ("theater", theater::register),
];
```

What each hot spot becomes:

| Today | After |
|---|---|
| `boot/src/registration.rs`: `register_core`, `register_late`, a `use crate::{...}` of 33 modules | `CRATES` above. `CoreInputs` and `LateInputs` become services put into `Host` by the stage that makes them. The plan tests in `registration/tests.rs` call the same `register` |
| `engine/src/points.rs`: one `points! { ALL; ... }` of 374 | `points! { POINTS; ... }` beside the code that hooks it; `reg.points(POINTS)`. Points that two crates hook live in `guest-layout`. The anchor list that [title-updates.md](../runtime/title-updates.md) 3.3 asks for is printed from the registry |
| `boot/src/lib.rs`: `run_configured`, 1,524 lines | `const STAGES: &[(&str, fn(&mut Boot) -> Result<()>)]` and `crates/boot/src/stages/{profile, module, live, content, kernel, register, video, audio, input, agent, plan, movies}.rs`, then `main_loop.rs`. `Boot` holds what the stages pass along; a stage puts its services in `Host` |
| `boot/src/setting_defs.rs`, `string_ids.rs`, `layout_glue.rs`'s rows, `ui_kit/screens.rs` | `reg.settings(DEFS)`, `reg.string_ids(FAMILY)`, `reg.layout_rows(ROWS)`, `reg.screen(DESCRIBE)` from the owning module. `string_ids.rs`'s collision check runs over the registry |
| `cli/src/main.rs`: 85 `mod` lines, `USAGE`, a chain of `if cmd ==` | `cmd::commands! { anim, bitmap, ... }` in each command crate, each module with `pub const COMMAND: cmd::Command`. `main.rs` calls each crate's `register(&mut Commands)` under its `cfg` and `Commands::run`. `USAGE` is printed from the table |
| `feature/src/sim_native.rs`: one install list for every sim port | `sim-bridge/src/{anim, collision, ...}.rs`, one per subsystem, in `hooks::modules!` |

Rules:

- **Sorted lists.** Two lanes adding different lines conflict only when
  the lines are adjacent, and then the resolution is "keep both".
- **Order comes from the plan, not the list.** A module that must run
  around another says so with `HookRef::after_feature`, `first` or
  `last`, which exist (`crates/feature/src/lib.rs`). A test registers
  every crate's list forwards and reversed and compares `Plan::text`.
  Installs that are not features (`layout_glue::install` writes guest
  memory) are the cases that test finds; they get an explicit stage.
- **A guard test per list,** the same shape as `scripts/it_guard.rs`: it
  fails when a file in the crate has a `pub fn register` and is not in
  the list.
- **Not link-time registration.** `inventory` and `linkme` need no list
  at all, but an rlib member that nothing references is dropped by the
  linker, the order is the link order, and the plan must be the same on
  every platform.

## How a freeze window works

A freeze step is one commit that a script made and nothing else touched.

1. The step's script is committed first, as `scripts/moves/<id>-<name>.sh`,
   in an ordinary reviewed commit. It uses `git mv`, edits tracked files
   with fixed `perl -pi` rules, stages everything it creates, and can be
   run twice.
2. The coordinator stops merging lanes, tags the tip `move-<id>-pre`,
   runs the script, commits exactly its output and tags it `move-<id>`.
   Tags are shared by every worktree.
3. The coordinator runs the step's check. Compile slips are fixed in
   following commits, never by amending the move.
4. Merging resumes. The frozen time is the check's run time.

A lane with work on the moved paths replays the move on its own tree:

```sh
git merge move-<id>-pre                  # ordinary merge, up to the commit before the move
scripts/moves/<id>-<name>.sh             # the same transform, on the lane's files too
git commit -qam "Apply move <id>"
git merge -s ours --no-edit move-<id>    # the move is merged; the tree stays the lane's
git merge master                         # then on to the tip as usual
```

`-s ours` is exact because `move-<id>` is the script applied to
`move-<id>-pre`, and the lane has just applied the same script to a tree
that contains `move-<id>-pre`. That is why the move commit holds only the
script's output. The script moves whole directories and rewrites paths
by rule, so a lane's new files and new `use` lines are carried too.

A lane that keeps a linear branch squashes instead:

```sh
git rebase move-<id>-pre
scripts/moves/<id>-<name>.sh && git commit -qam "Apply move <id>"
git reset --soft move-<id> && git commit -m "<lane>: work so far, over move <id>"
git rebase master
```

A lane that did not touch the moved paths needs nothing special. Renames
are followed with:

```sh
git -c merge.renameLimit=30000 -c merge.directoryRenames=true merge master
```

The standard check, run through the shared ledger:

```sh
git diff -M50% --diff-filter=AD --name-only move-<id>-pre move-<id>   # only the files the step says are new or deleted
tested check --workspace --all-targets
tested check -p cli --no-default-features --features host
tested test --workspace --no-run
scripts/crate-table.py --check && scripts/crate-graph.py layers --check
```

plus the tests each step names. `git diff -M` listing a moved file as
added or deleted means the script changed it too much to be a move.

Two kinds of step:

- **Freeze**: a move as above.
- **Prep**: an ordinary lane change, merged like any other, that removes
  references so a later move is mechanical. Its done-check is
  `scripts/crate-graph.py modules <crate>/src --closed <modules>` exiting
  0: no `crate::` path leaves the set that is about to move.

## The moves

In order. Sizes are files and lines moved. "Lanes hit" are those with
uncommitted or unmerged work on the paths, as of this page's commit.
Every freeze step's lane procedure is the one above unless it says
otherwise.

### A. Nothing moves in code

**A1. Manifests** (freeze; 87 manifests, about 250 lines).

- Every crate gets a `description` (30 have none; proposed text for the
  seven with no row anywhere is under "The crate table") and
  `[package.metadata.h2] layer`. `status` and `doc` replace the
  hand-written cells of the old table.
- `git mv crates/runtime/test-probe crates/runtime-test-probe`; drop the
  redundant `"crates/host-data"` member line.
- Existing `path = "../x"` dependencies stay (243 in 51 manifests). New
  crates use `[workspace.dependencies]`. Converting the rest is not worth
  its risk: under edition 2024 a member cannot turn off default features
  that the workspace entry leaves on, and nine crates are taken both ways.
- Lanes hit: any lane that adds a crate or a dependency (`sim-anim`,
  `sim-collision`, `shadow-exec`, `guest-layout`, `build-1120`).
  Different hunks of the same files; ordinary merges.
- Check: the standard one, with `scripts/crate-table.py --strict`, and
  `cargo metadata --offline --format-version 1` giving the same
  `resolve` as before.
- A changed `description` changes the crate's fingerprint, so this is one
  full rebuild. Land it in the same integration as `build-1120`'s dev
  profile commit, which is also one.

**A2. Dead and misnamed crates** (freeze; about 60 files).

- `git rm -r crates/fastmem-proto`.
- `git mv crates/sim crates/sim-bench`, package `sim-bench`. In
  `crates/boot` and `crates/cli` only:
  `perl -pi -e 's/(?<![:\w])sim::/sim_bench::/g'` (22 files; `agent` and
  `agent-proto` have their own `sim` modules and are not touched).
- `git mv crates/apple-host crates/h2host`, package `h2host`. Its library
  is already `h2host`, so no Rust changes: `crates/uwp-host/Cargo.toml`,
  `platform/apple/build.sh`, `platform/xbox/build.sh` and six docs.
- `oracle`'s WMA oracle (`src/wma.rs`, `tests/wma_disc.rs`,
  `examples/wma_oracle.rs`, 390 lines) moves to `crates/wma` behind an
  `oracle` feature; `oracle` loses its `asf` and `wma` dependencies.
- Lanes hit: the Live and bots owner (`boot/src/sim_glue.rs`,
  `host_lobby.rs`, `cli/src/sim_cmd.rs` name `sim::`).
- Check: standard, and `tested test -p sim-bench -p wma -p oracle`.

**A3. Host-build leaks** (ordinary commits, no freeze).

- `crates/h1bitmaps/Cargo.toml`: `gfx` with `default-features = false`.
- `cli`: `live-server` optional under `desktop`, once
  `live_server::identity::write_private` lives somewhere smaller
  (proposal 6).
- The gate records the host closure's crate count (62) as a ceiling and
  lowers it as these land.

### B. Recompiler and runtime

**B1. `recomp` out of `boot`** (freeze; 7 files and 630 lines cut from
`lib.rs`; 3,200 lines).

- Moves `crates/boot/src/{fallback, shim, sse, x87, tier, host_coverage}.rs`
  with their submodules, `crates/boot/c/`, `crates/boot/build.rs`
  and `crates/boot/tests/fast_tier.rs` to `crates/recomp`. Cuts `BRIDGE`
  to `Module` (`lib.rs` lines 196 to 822: stamps, the analysis cache,
  `build*`, `Module`) into `crates/recomp/src/lib.rs`.
- `boot` re-exports: `pub use recomp::{self, fallback, shim, tier, host_coverage, build, build_for, build_slim, build_stamp, read_xbe, dylib_current, dylib_current_for, slim_current_for, current_slim_stamp, analysis_cache, Module, DYLIB};`
  so `crate::tier` and `boot::tier` (28 uses in `cli`) still resolve.
- `boot` loses its build script, so a `Cargo.lock` change no longer
  recompiles it (build-speed.md follow-up 6).
- Prep: `tier` → `host_audit`, two references (`tier.rs:105`, `:213`).
- Lanes hit: `contract-design`'s implementers, `sim-tier`,
  `codegen-mem`, `codegen-speed` if they touch stamps. Do it before the
  contract work starts. The cut region is a new file to git, so a lane's
  pending edit inside those 630 lines is re-applied by hand.
- Check: standard, and `tested test -p recomp -p boot -p emit -p clif`.

**B2. `lifted` adopted** (lane work, no freeze). After `shadow-exec`
lands: `engine`'s `differential` feature and `native-render`'s optional
`analyze`, `ir`, `lift`, `xbe` become `[dev-dependencies] lifted`;
`crates/engine/tests/common` goes.

### C. The hooks layer

**C1. `points.rs` split in place** (freeze; 1 file into 9, 1,261 lines).

- `git mv crates/engine/src/points.rs crates/engine/src/points/mod.rs`
  (types, macros, lookup, tests stay). The script sends each table entry
  to a file by its doc path: `docs/halo1/` → `h1.rs` (126 points),
  `docs/graphics/` → `render.rs` (65), `docs/game/bots.md` → `bots.rs`
  (31), `docs/live/` → `coop.rs` (34), `docs/perf/` → `sim.rs` (30),
  `docs/formats/ui.md` and the rest of `docs/game/` → `ui.rs` (47), the
  rest → `core.rs`. Each file is one `points! { POINTS; ... }`;
  `mod.rs` globs them, so `points::NAME` is unchanged, and `ALL` becomes
  the list of tables.
- Lanes hit: `sim-anim` (13 new points), `sim-collision`,
  `guest-layout`, any lane adding a point. The script classifies a
  lane's new entries too, so the standard procedure works.
- Check: standard, and
  `tested test -p engine -p feature --test plan --test install`.
- This is step one of two. C3, D2 to D6 and the registration work then
  carry each file to the crate that hooks it.

**C2. `sim-bridge`** (freeze; 8 files, about 1,800 lines).

- New crate from `crates/feature/src/{sim_frame, sim_native, sim_physics, sim_observer, sim_static_light, sim_submix}.rs`,
  `crates/feature/tests/sim_physics.rs` and `crates/engine/src/sim_anim.rs`
  (on the `sim-anim` branch). File names stay.
- `perl -pi -e 's/\bfeature::sim_/sim_bridge::sim_/g; s/\bengine::sim_anim\b/sim_bridge::sim_anim/g'`
  over `crates/`.
- Prep: `sim_frame` → `pipeline_render`, one edge.
- Lanes hit: `sim-anim`, `sim-collision`, `sim-pose`, `sim-tier`. They
  are its owners afterwards. Do it right after `sim-anim` merges.
- Check: standard, and `tested test -p sim-bridge -p engine -p feature`.

**C3. `guest-layout` and `hooks` take their parts of `engine`** (freeze;
8 files, about 4,000 lines). Owned by the `guest-layout` lane; this page
fixes the destination.

- `crates/engine/src/{points/, memory, handle, datum, view, tags, testing}.rs`
  → `crates/guest-layout/src/`. `crates/engine/src/entry.rs` →
  `crates/feature/src/guest_entry.rs`.
- `engine` re-exports `pub use guest_layout::{points, memory, handle, datum, view, tags, testing};`
  so 605 `engine::` paths in `boot` and 88 in `bots` are untouched.
  `engine::entry::` (about 30 uses) is rewritten to `feature::guest_entry::`.
- Lanes hit: everyone, with no edits to make.
- Check: standard, and `tested test -p guest-layout -p engine -p feature -p bots`.

**C4. `render-bridge`, the guest half** (freeze; about 73 files, 43,000
lines). After `integ-1120` lands, before FrameScene implementation.

- From `feature`: 30 modules (`native_*`, `pipeline_*`, `render_census`,
  `render_timing`, `render_loop`, `occlusion`) and their 16 test files.
  From `engine`: `render_loop/`, `render.rs`, `glow`, `screen_effect`,
  `screen_effects`, `screen_grid`, `colour_pass`, `light_passes`,
  `shadow_constants`, `gel`, `scene_list_math`, `ui_model`, `ui_sprite`,
  `ui_backdrop` and their differential tests. `light_constants` and
  `function_eval` stay in `engine`: the sim ports use them too.
- Rewrites `feature::<moved>` and `engine::<moved>` to
  `render_bridge::<same name>` by a fixed list of module names.
- `feature` loses its `native-render` cargo feature and dependency;
  `boot`'s `desktop-runtime` turns on `dep:render-bridge` instead.
- Prep (three small edits): `scene_effect_instruction_entries` and
  `native_verbose` out of `feature/src/lib.rs`; the three `native_*`
  helpers out of `switches.rs`; `sim_frame` → `pipeline_render` if C2 has
  not cut it.
- Lanes hit: `flag-cleanup` and `native-cost` (in `integ-1120`; land
  first), the FrameScene implementers (start after).
- Check: standard, and
  `tested test -p render-bridge -p feature -p engine -p native-render -p boot`.

**C5. `feature` becomes `hooks`** (freeze; a rename, about 1,010 path
edits, 793 of them in `boot`). Optional: decision 3.

### D. `boot`

`boot`'s groups reference each other 73 times at module level
(`scripts/crate-graph.py modules`; an upper bound, since it counts paths
in comments). Nothing here extracts a crate by cutting all of its edges.
The composition root goes up first, groups with few inbound references
follow it up, and the UI toolkit goes down last.

**D1. The composition root goes up** (freeze; 316 files renamed, 3
edited).

- `git mv crates/boot crates/game`, package `game`. Then
  `crates/game/src/lib.rs` and `crates/game/src/registration*` move back
  to `crates/boot/src/`, and a new `crates/game/src/lib.rs` takes the
  `mod` lines and the items other modules name (`RunOpts`, `Host`,
  `LiveConfig`, the `log` macro). So the two hot files keep their paths
  and their history, and every other file is a pure rename.
- `crates/boot/src/lib.rs` starts with `pub use game::*;`. Every
  `crate::x` in it, every `boot::x` in `cli`, `studio`, `h2host` and
  `boot/tests` still resolves.
- `boot`'s cargo features forward: `gpu = ["game/gpu"]`.
- The 283 `pub(crate)` items in the moved files become `pub`.
- Prep: `registration::compiled_render_hooks` moves to `d3d_glue` (7
  uses from the mesh); two test-only references
  (`render_thread.rs:276`, `theater/tests.rs:160`).
- Lanes hit: every lane with work under `crates/boot/src`, including the
  Live and bots owner. Pure renames, so a lane without the script needs
  only the `merge.directoryRenames=true` merge.
- Check: standard, and `tested test -p game -p boot -p cli`.
- By itself this saves nothing. It makes each later step a move out of
  `game` that does not touch the root.

**D2. Render glue up, into `render-bridge`** (freeze; 33 modules, 22,997
lines).

- Prep, nine inbound module edges: `agent_glue` → `d3d_diag`,
  `native_shadows_probe`, `render_probe`; `snapshot_glue` → `d3d_glue`;
  `xapi_glue`, `host_perf`, `level_boot` → `d3d_diag` (all for
  `d3d_diag::presents`, a frame counter that belongs in `d3d`);
  `campaign_cards_tests` → `native_map`, `native_world_glue`.
- `game` re-exports nothing here: it cannot name a crate above it. The
  script rewrites `crate::<moved module>` in `boot` to
  `render_bridge::<module>`.
- Lanes hit: the FrameScene implementers; the group with the most
  commits (307).
- Check: standard, and `tested test -p render-bridge -p game -p boot`.

**D3. `h1-runtime` up** (freeze; 14 modules and two files from
`feature`, 21,065 lines).

- Prep, six inbound module edges: `cheats`, `touch_glue`,
  `skulls_custom` → `h1_hud_text`; `native_first_person_glue`,
  `native_objects_glue`, `native_post_glue` → `h1_cinematics`.
- Lanes hit: the Halo 1 lanes (`h1-bg-prompts`, `h1-parity-suite` are
  unmerged).
- Check: standard, and `tested test -p h1-runtime -p boot`.

**D4, D5, D6. `bot-glue`, `net-glue`, `theater` up** (freeze; 8,776,
16,655 and 10,863 lines). The other owner's; see proposals 1 to 3.

**D7. `ui-kit` down** (prep, then freeze; 5 modules, 17,171 lines).

- Prep: 29 module edges, 80 references
  (`scripts/crate-graph.py modules crates/boot/src --closed ui_kit,list_class,ui,icons,keyboard_glue`
  lists each). `ui_kit/screens.rs` (the screen catalogue) moves up to
  `game` first; `list_class` → `speaker_row` is 8 of the rest.
- `studio` then depends on `ui-kit` instead of `boot`, for the one type
  it wanted (`ui_kit::input::Input`).
- Check: standard, and `tested test -p ui-kit -p game -p boot -p studio`.

**D8. Stages and local registries** (prep; lane work on `boot` and
`game`). `run_configured` into `stages/`; `setting_defs`, `string_ids`,
the layout rows and the screen catalogue registered by their modules.
After it, `game` can split into `game-ui` and `gameplay` (22 module
edges between them today) and the crates above stop depending on all of
`game`.

### E. `cli`

**E1. Command crates** (prep, then freeze; 59 files, 41,000 lines
without the Live and bots parts).

- Prep: `cmd` crate with `Command`, `Commands`, `commands!`, and
  `safe_out`, `default_disc`, `open_iso`, `load_xbe`, `repo_root` (22
  references from other modules). Three edges to cut: `boot_cmd` →
  `h1_import`, `boot_cmd` → `mappack_cmd`, `sim_cmd` → `live_accounts`.
- Freeze: the modules move to `cmd-host`, `cmd-play`, `cmd-tags`,
  `cmd-recomp`, `cmd-h1`, `cmd-dev` as listed in the crate table.
  `cli` keeps `main.rs`.
- `app::BuildInfo` moves to a crate that only `cli`'s `main.rs` reads, so
  a commit stops recompiling `app` (build-speed.md follow-up 5).
- Lanes hit: `lab-agent` (unmerged, `cli/src/lab`), the Halo 1 lanes
  (`h1_import.rs`, 147 commits).
- Check: standard, and `tested test -p cli -p cmd-host -p cmd-tags -p cmd-recomp -p cmd-h1 -p cmd-dev -p cmd-play`.

**E2. `live-art`, `cmd-live`, `cmd-drive`** (the other owner's;
proposal 4).

### F. Halo 1

**F1. `h1-contract`** (prep). The names `boot` takes from `h1convert`,
`h1scripts`, `h1text`, `h1objects` and `h1sounds` move to a crate with no
dependencies; `boot` (then `h1-runtime` and `game`) drops the converter
crates. This is what takes them out of the host-only build.

**F2. `h1-assets`** (freeze; 53 files, 22,494 lines). `h1bitmaps`,
`h1models`, `h1sounds`, `h1sky` and `h1text` become modules `bitmaps`,
`models`, `sounds`, `sky`, `text` of one crate; `h1models::` becomes
`h1_assets::models::` by rule.

**F3. `h1campaign` absorbs `h1ai` and `h1scripts`** (freeze; 46 files,
18,541 lines), after F1.

Both wait for a quiet moment in the Halo 1 lanes.

## Delete, merge, rename

| What | Action | When | Note |
|---|---|---|---|
| `fastmem-proto` (2,563 lines) | Delete | A2 | No dependents. `docs/perf/fastmem-proto.md` and `fastmem-4k.md` are the docs lane's to file |
| `viewer` (7,258 lines) | Delete | after decision 4 | Fan-in zero, but three of its four binaries are cited as commands: `bsp --all` (`docs/formats/structure-bsp.md`), `bsp_cracks` (`docs/graphics/high-resolution.md`), `shadowlab` (`docs/graphics/e3-lighting.md`). `bsp` and `bsp_cracks` (1,249 lines) become `cmd-tags` commands; `shadowlab` (3,730) has made its decision and goes; the model window is `studio`'s |
| eleven `h1*` crates | Merge to five, plus `h1-contract` | F1 to F3 | `h1map`, `h1-assets`, `h1convert`, `h1objects`, `h1campaign`. Not three: `h1convert` + `h1objects` would be one unit of 77,000 lines, against rule 2 |
| `sim` | Rename `sim-bench` | A2 | Frees the name before `sim-*` multiply |
| `apple-host` | Rename `h2host` | A2 | The package takes its library's and its header's name (`platform/apple/h2host.h`); `uwp-host` depends on it |
| `oracle`'s WMA oracle | Move to `wma` | A2 | |
| `boot/native_*_glue` with `feature/native_*` | Merge in `render-bridge` | C4, D2 | |
| `feature` | Rename `hooks` | C5 | decision 3 |
| `engine/src/render_loop/` (5,933 lines), `d3d/src/interp` | Delete | their own lanes | The review's stage 7a and interpolation items. `integ-1120` already removes `render_loop/scene.rs`. If `render_loop/` goes before C4, C4 is that much smaller |
| `engine/src/{sim_native, sim_physics}/` (8,981 lines) | Delete per subsystem | as each `impl.sim.native.<subsystem>` lands | They stay in `engine` as oracles until then ([sim-rewrite.md](../engine/sim-rewrite.md) decision 4) |
| `crates/runtime/test-probe` | Move to `crates/runtime-test-probe` | A1 | The only nested crate |

## Gates

### Local: `scripts/gate.sh`

A proposal; the script is the coordinator's to write and run, since it
runs cargo. It replaces `scripts/check.sh`, whose `cargo fmt --all
--check` cannot pass on crates that are not rustfmt-clean.

```sh
scripts/gate.sh lint        # seconds, no compile
scripts/gate.sh check       # desktop and host-only cargo check
scripts/gate.sh test        # everything that needs no disc
scripts/gate.sh disc        # everything that needs one
scripts/gate.sh move <id>   # a freeze step's check
scripts/gate.sh all         # lint, check, test, disc
```

| Tier | Runs |
|---|---|
| `lint` | `scripts/crate-table.py --check`; `scripts/crate-graph.py layers --check`; the host budget (`closure cli --features host` crate count at most the number in the script); the docs check from the docs lane; `rustfmt --check` on files changed against `master` that were clean there |
| `check` | `cargo check --workspace --all-targets`; `cargo check -p cli --no-default-features --features host`; `cargo clippy --workspace --all-targets -- -D warnings` |
| `test` | `cargo nextest run --workspace` with `.config/nextest.toml`; `cargo test --workspace --doc` |
| `disc` | With `H2_DISC`, `H1_ISO`, `H2_XBE` and `H2_ORACLE_FIXTURES` set from `disc/` when unset: `cargo nextest run --workspace --run-ignored ignored-only -E 'not test(/bench|slow/)'`; `cargo test -p lift --features oracle-fixtures --test oracle`; `cargo test -p engine --features differential`. Prints the count of tests that ran and refuses zero |
| `move <id>` | the rename-only diff between the two tags, then `lint`, `check`, `cargo test --workspace --no-run` |

- Every cargo call goes through the shared test ledger, which moves into
  the repository as `scripts/tested` with the cargo-slot path read from
  `H2_CARGO_SLOT` instead of a session directory. A pass for a tree is
  then a fact any agent can reuse.
- The disc tier needs one convention: a test that reads a disc has `_disc`
  in its module or test name, as `h1objects/tests/*_disc.rs` and
  `.config/nextest.toml` already assume, and a benchmark has `bench`.
  Today's 530 `#[ignore]`s carry 25 different reasons and 59 carry none.
- The coordinator runs `all` before a push and `move <id>` in a freeze.

### CI: `.github/workflows/ci.yml`

CI has no disc and never will. It runs what it can and says what it did
not.

| Job | Runner | Runs | Catches |
|---|---|---|---|
| `lint` | ubuntu | `scripts/gate.sh lint` | a crate with no description or layer, an upward dependency, a stale table, the host budget |
| `host` | ubuntu | `cargo check -p cli --no-default-features --features host` | desktop code leaking into the dedicated host; a Linux break |
| `test` | macos | `scripts/gate.sh check`, then `scripts/gate.sh test` | what CI catches now, through nextest |

The differential suites that prove ports (`crates/lift/tests/oracle.rs`,
the 14 `*_differential.rs` in `crates/engine/tests`, the disc tests of
the `h1*` crates) run only in the local `disc` tier. The `test` job ends
by printing how many tests were ignored, so nobody reads a green run as
proof of a port.

## The crate table

`scripts/crate-table.py` writes the table in
[ARCHITECTURE.md](../ARCHITECTURE.md) between two marker comments: one row
per package, from `cargo metadata --offline --no-deps`.

| Column | Source |
|---|---|
| crate | the package name and what it links besides an rlib (`bin h2`, `staticlib h2host`) |
| layer | `[package.metadata.h2] layer`; the column appears once any crate has one, and the rows then sort by layer |
| purpose | `description` |
| docs | `[package.metadata.h2] doc`, else the `docs/...md` paths in the description |
| status | `[package.metadata.h2] status` |

Until step A1, a cell with no source keeps the text the old table had; a
purpose kept that way carries a dagger. `--check` fails when the file is
stale; `--strict` also fails on a missing description or layer. The
table holds no line counts, because they would make it stale on every
commit; `scripts/crate-graph.py sizes` prints them.

Seven crates have no description and had no row. Proposed for A1:

| Crate | `description` |
|---|---|
| `clif` | The Cranelift fast tier: planned IR straight to host objects, linked into the same kind of module clang's build makes (docs/lifter/cranelift-tier.md) |
| `h1campaign` | Halo 1 campaign levels converted onto pinned retail Halo 2 campaign maps (docs/halo1/campaign.md) |
| `h1objects` | Halo 1 sandbox objects (weapons, projectiles, equipment, bipeds, vehicles) as Halo 2 tags (docs/halo1/objects.md) |
| `h1scripts` | Halo 1 campaign scripts decoded from the scenario tag and carried over to Halo 2 (docs/halo1/scripts.md) |
| `h1sky` | Halo 1 skies read from Xbox cache files and converted to Halo 2 sky tags (docs/halo1/sky.md) |
| `h1text` | Halo 1 string lists and HUD message text as Halo 2 localized text (docs/halo1/text.md) |
| `runtime-test-probe` | Native declarations of the fixture generated module, for tests that run hooks through the real runner |

The other 23 take the purpose their old row had, which the table still
shows.

## Proposals for the Live, bots, theater and film owner

Nothing here is scheduled. Each is a move in that owner's files.

1. **`net-glue`** (D5): `boot/src/{coop_jip, coop_latency, coop_live, coop_look, coop_players, coop_probe, host_lobby, live_glue, live_notice, account_screen, signin_screen, signin_keyboard, community_variants}`,
   16,655 lines. Fifteen modules of `game` name it, mostly through
   `coop_probe::read_session` and its constants (which are guest layout
   and belong in `guest-layout`) and through the three registries.
2. **`theater`** (D6): `boot/src/{theater/, theater_ui/}`, 10,863 lines.
   `game` names it three times (`layout_glue`, `snapshot_glue`,
   `variant_ext`); `theater` names `bot_glue`, `sim_glue`, `coop_live`
   and `coop_look`, which fixes the order: `theater` below `net-glue`
   and `bot-glue`, or those four references cut.
3. **`bot-glue`** (D4): `boot/src/{bot_glue, bots_nav_auto, bot_mark, clankers, clanker_card, clankermatch, sim_bots, sim_glue}`,
   8,776 lines. Six inbound module edges from `game`.
4. **`cli`** (E2): `live-art` as a library (27,679 lines, no dependency on
   `boot`: today every `boot` edit recompiles it), `cmd-live` (7,518),
   `cmd-drive` (10,985: the bot and co-op drivers and verdicts).
   `live-art` and `cmd-live` name each other 17 times
   (`live_frames` → `live_cmd`, `live_cmd` → `live_art`, ...).
5. **`live-server/tests/it.rs`**: 40 test binaries of 63 MiB each
   (build-speed.md follow-up 1).
6. **`live_server::identity::write_private`** into `live-proto` or
   `host-data`, so `cli` can make `live-server` optional and the
   dedicated host stops compiling the service.
7. **`live-proto`'s cone is 48%.** `bots` uses two of its modules
   (`bots`, `game`), `film` four. A small `live-types` under them would
   keep an API change in `session` or `matchmaking` from recompiling
   `bots`, `boot` and `cli`.
8. **Points**: C1 puts the 31 bot points and 34 co-op and Live points in
   `points/bots.rs` and `points/coop.rs`; they then belong in `bots` and
   `net-glue`.
9. **`sim` → `sim-bench`** (A2) and **D1** rename paths in these files
   without changing them. Both need a time that suits.

## Decisions needed

1. **Flat `crates/` with the layer in the manifest** (recommended), or
   directories per layer.
2. **`boot` splits into `boot` (root) and `game` (the rest)** at D1, and
   `game` is the name that stays for menus and gameplay.
3. **Rename `feature` to `hooks`?** About 1,010 path edits for a name
   that stops colliding with cargo features. Recommended, last in C.
4. **`viewer`:** delete `shadowlab` outright and keep `bsp` and
   `bsp_cracks` as `h2` commands (recommended), or keep the crate.
5. **Halo 1: eleven crates to five plus `h1-contract`,** not the review's
   three or four.
6. **`apple-host` becomes `h2host`.**
7. **`guest-layout` holds the only copy of each layout;** `sim-state` is
   the read API over it. The sim review's "one schema crate" and the
   architecture review's "typed guest layout crate" are then one crate
   and one API crate.
8. **Who cuts the 73 references in `boot`.** One lane, working down the
   prep lists of D2, D3, D7 and D8, with the coordinator landing a
   freeze after each.
9. **The first freeze.** A1 and A2 together, in the integration that
   lands `integ-1120` and `build-1120`'s profile commit: one full
   rebuild instead of three.

## Not verified

- Nothing was compiled. Each move's script has to be written and run by
  the coordinator; the `perl` rules here are the intent, not tested text.
- Module edges come from `crate::` paths found by a regular expression.
  Paths in comments are counted (an overcount); `super::` paths between
  top-level files and paths built by macros are not (an undercount).
  `--closed` exiting 0 is necessary for a move, and `cargo check` decides.
- The "after" column of "What it buys" is arithmetic on line counts. It
  ignores third-party crates, link time and kache hits.
- Whether a sorted registration order gives the same plan as today's
  order. The forwards-and-reversed test is how to find out.
- Whether `h1bitmaps` builds without `gfx/gpu`. Its uses read as CPU
  texture decoding.
- The other lanes' crates (`guest-layout`, `lifted`, `sim-collision`,
  `sim-tier`) were read as uncommitted or unmerged work on their
  branches and may land under other names.
