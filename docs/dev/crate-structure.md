# Crate structure: domains, and the windows that move to them

Status: Design. No crate has moved. Built: the domain rules
(`scripts/crate-domains.toml`), their check (`scripts/crate-graph.py layers
--check`), the crate table generator (`scripts/crate-table.py`) and the
move scripts under `scripts/moves/`. Each script was run on a scratch copy
of the tree; none of the results was compiled.
Measured at: `crate-map` on master `100f13cef`, 2026-11-20, with
`scripts/crate-graph.py`. Nothing here runs cargo.

This page carries out section 2 and item 8 of
[architecture-review-2026-11.md](../architecture-review-2026-11.md): where
every crate lives, what each may depend on, and the order of moves that
gets there while lanes keep merging. Four rules hold throughout.

1. **A domain is a directory.** `crates/<domain>/<crate>`. A crate's
   domain is where it sits, and nothing else declares it.
2. **Dependencies between domains go one way,** by a table that a script
   checks.
3. **A move is a script.** It is mechanical (`git mv`, path rewrites by
   rule), it can run twice, and a lane replays it on its own branch
   instead of resolving conflicts.
4. **A list that every lane appends to becomes a list each crate keeps**
   (see "Local registration").

Paths on this page are written without `crates/` when they name a file
that a window moves (`boot/src/lib.rs`), and in full when they name where
something ends up.

## The domains

Ten directories. `+` is a crate that is not on master yet (a lane's, or
made by a later window); `~` is renamed or merged; `[x]` belongs to the
session that owns Live, bots, theater and film.

```
crates/
  formats/     asf content font hs map mopp movies pathfind settings shaders tags variants
               xbe xiso  +h1-contract
  recompiler/  analyze symbols lift ir opt emit clif x87 simd oracle sse-oracle fastmem-proto
               +sim-tier
  runtime/     runtime (and its test-probe) kernel vfs snapshot timeline
               +hostfault +lifted +recomp
  platform/    d3d gfx assets hot-render  dsound audio wma  xapi input  bink-hle voice
  hooking/     ~hooks (feature)  +guest-layout  +switches
  rewrites/    engine native-render
               +sim-core +sim-anim +sim-collision +sim-state +sim-bridge
               +frame-scene +scene-extract +render-bridge
  live/        [live-proto live-hle live-telemetry live-server live-worker coop bots film
                cloud-saves host-data sim]
  extensions/  agent agent-proto debug-menu hot-tags lockstep
               +game +ui-kit +h1-runtime  [+net-glue +bot-glue +theater]
  tools/       h1map ~h1-assets h1convert h1objects ~h1campaign
               editor-core scenario-edit hopper-edit hud-edit  perf viewer  [+live-art]
  apps/        boot cli app app-icon studio studio-proto apple-host uwp-host
               +cmd +cmd-host +cmd-play +cmd-tags +cmd-recomp +cmd-h1 +cmd-dev
               [+cmd-live +cmd-drive]
```

| Domain | What belongs in it | Crates today | Lines |
|---|---|---:|---:|
| `formats` | Disc, tag and file formats, ours and the game's: plain data, no guest, no host | 14 | 91,702 |
| `recompiler` | XBE to host code: analysis, lifting, IR, the C and Cranelift back ends, their oracles | 12 | 57,809 |
| `runtime` | The machine the lifted code runs on: memory, CPU state, kernel, filesystem, snapshots, the build-and-load driver | 6 | 59,304 |
| `platform` | Platform HLE: the Xbox's devices and libraries on the host (Direct3D 8, DirectSound, XAPI, pads, Bink, voice) and the host graphics and audio under them | 11 | 144,685 |
| `hooking` | How our code meets guest code: the hook engine, named points, typed guest layouts, switches | 1 | 32,783 |
| `rewrites` | Rust that replaces a guest subsystem: the native simulation, FrameScene, the native renderer, ported engine functions, and the bridge that installs each one | 2 | 67,225 |
| `live` | Live, the co-op transport, bots, films, cloud saves, the dedicated host's data, the service | 11 | 222,959 |
| `extensions` | Features we add to the game: menus and settings screens, skulls, the campaign picker, Halo 1 in game, Live and bots in game, theater, the agent API, the debug menu | 5 | 27,033 |
| `tools` | Offline work on game data: the Halo 1 converters, the editor cores, the profiler, the Live art pipeline | 17 | 169,758 |
| `apps` | What links a binary: the composition root, the desktop shell, the command line and its command crates, the editor, the embedded hosts | 8 | 270,879 |

`hooking` and `extensions` are small today because their code is inside
`feature`, `engine`, `boot` and `cli`. The splits below fill them.

### What each domain may use

`scripts/crate-domains.toml` holds this table as `uses` lists. A crate may
depend on its own domain and on the ones its row names.

| Domain | May use | Used by |
|---|---|---|
| `formats` | nothing | every domain |
| `recompiler` | `formats` | `runtime`, `rewrites`, `tools`, `apps` |
| `runtime` | `formats`, `recompiler` | everything above it |
| `platform` | `formats`, `runtime` | `rewrites`, `live`, `extensions`, `tools`, `apps` |
| `hooking` | `formats`, `runtime` | `rewrites`, `live`, `extensions`, `apps` |
| `rewrites` | `formats`, `recompiler`, `runtime`, `platform`, `hooking` | `live`, `extensions`, `tools`, `apps` |
| `live` | `formats`, `runtime`, `platform`, `hooking`, `rewrites` | `extensions`, `tools`, `apps` |
| `extensions` | `formats`, `runtime`, `platform`, `hooking`, `rewrites`, `live` | `apps` |
| `tools` | `formats`, `recompiler`, `runtime`, `platform`, `rewrites`, `live` | `apps` |
| `apps` | every domain | nothing |

```
 apps ──────────────────────────────────────────────┐
   │                                                │
 extensions        tools (offline; never hooks a    │
   │                │      running guest)           │
 live ◄─────────────┤                               │
   │                │                               │
 rewrites ◄─────────┤                               │
   │        │       │                               │
 hooking  platform ◄┘                               │
   │        │                                       │
 runtime ◄──┘                                       │
   │                                                │
 recompiler                                         │
   │                                                │
 formats ◄──────────────────────────────────────────┘
```

More rules in the same file:

- **Guest-free crates.** `sim-core`, `sim-anim`, `sim-collision`,
  `frame-scene`, `scene-extract` and `native-render` take typed values.
  They may use `formats`, `platform` and each other: no guest memory, no
  hook, no lifted code. The bridge crates (`sim-bridge`, `render-bridge`)
  are the `rewrites` crates that use `hooking`.
- **Dev-dependencies are free.** A test may reach up (`snapshot`'s tests
  use `lockstep`). The check reads normal and build dependencies.
- **Exceptions are listed, with the step that removes each.** The check
  fails on a refused dependency that is not listed, and on a listed one
  that is gone.

| Refused dependency the tree has today | Removed by |
|---|---|
| `feature` → `engine` (`hooking` → `rewrites`) | S2: the points, views and entry glue move to `guest-layout` and `hooks` |
| `feature` → `native-render` (`hooking` → `rewrites`) | S3: the renderer's guest half leaves for `render-bridge` |
| `oracle` → `wma` (`recompiler` → `platform`) | S1: the WMA oracle moves into `wma` |
| `native-render` → `analyze`, `ir`, `lift` (guest-free) | the `lifted` crate lands and the differential tests take it as a dev-dependency |

Every other dependency in the tree already obeys the table.

### Placements that need a reason

| Crate | Domain | Why there |
|---|---|---|
| `settings`, `content`, `variants` | `formats` | Our own file formats (`settings.toml`, map packages, variants). `vfs` and `cloud-saves` read them, so they sit under `runtime` and `live` |
| `timeline` | `runtime` | A trace-event utility, not a format. `runtime` is its lowest user |
| `x87`, `simd` | `recompiler` | `ir` is built on them; `runtime` uses `x87` from above |
| `lifted`, `recomp` | `runtime` | Both run lifted code on `runtime`, which `recompiler` may not use |
| `assets`, `hot-render`, `wma` | `platform` | Host-side support that `d3d` and `dsound` depend on (`assets` uses `gfx`; `wma` keeps snapshot state) |
| `engine` | `rewrites` | Ported engine functions. Its points and typed views are `hooking`'s and leave in S2 |
| `lockstep`, `hot-tags` | `extensions` | A determinism harness and live tag edits: features we add, built on hooks |
| `agent-proto`, `studio-proto` | `extensions`, `apps` | A wire protocol lives with the thing that serves it |
| `sim` | `live` | Sim mode's match and report files; only the bot glue and `h2 sim` read them. `[x]` |
| `net-glue`, `bot-glue`, `theater` | `extensions` | Live and bots inside the game process. They use `game`'s menus and `ui-kit`, which `live` may not, so they are extensions that the Live owner owns (`live_owned` in the rules file) |
| `live-art` | `tools` | The art pipeline works offline on the disc and uses the Halo 1 converters and `scenario-edit`, which `live` may not |
| `cmd-live`, `cmd-drive` | `apps` | Commands depend on `cmd`, and nothing below `apps` may |
| Halo 1 | three domains | `h1-contract` (`formats`): the names both halves share. The converters (`tools`). `h1-runtime` (`extensions`): the in-game half, which then needs no converter |
| `boot` | `apps` | The composition root names every domain. What it holds today leaves by S4 to S6 |
| `runtime-test-probe` | `runtime` | Stays nested in the `runtime` crate's directory, whose `tests/` and `include/` its build script reads |

### The check

```sh
python3 scripts/crate-graph.py layers           # each domain's crates, the excused dependencies
python3 scripts/crate-graph.py layers --check   # exit 1 on any of the below
```

`--check` fails on: a dependency the table refuses and no exception
lists; an exception whose dependency is gone; a crate no domain claims; a
crate in a domain other than the one the rules file lists for its name;
and, once any crate sits in a domain directory, a crate that does not
(a lane's new `crates/<name>`, which no workspace `members` glob
reaches). Before window 1 it checks the planned domains of the flat tree,
and passes today.

`scripts/cratemeta.py` reads the manifests with `tomllib` for all three
scripts, so they run in the middle of a move and never start cargo.
`[workspace] members` lists one glob per domain (`crates/formats/*`, ...)
and not `crates/*/*`: a leftover untracked directory under `crates/`
cannot become a member.

## Where each running lane's crate lands

| Crate | Lane | Directory | Note |
|---|---|---|---|
| `guest-layout` | `guest-layout` | `crates/hooking/guest-layout` | No dependencies. S2 moves `engine`'s points and views into it |
| `switches` | `guest-layout` | `crates/hooking/switches` | Planned by that lane; `feature/src/switches.rs` until then |
| `lifted` | `shadow-exec` | `crates/runtime/lifted` | Uses `analyze`, `ir`, `lift`, `runtime`, `xbe`. Its landing clears three exceptions |
| `sim-core`, `sim-anim` | `sim-anim` | `crates/rewrites/` | Guest-free. The adapter the branch puts in `engine/src/sim_anim.rs` and the install in `feature/src/sim_native.rs` go to `sim-bridge` (S3) |
| `sim-collision` | `sim-collision` | `crates/rewrites/sim-collision` | Guest-free; depends on `sim-core` |
| `frame-scene`, `scene-extract` | `framescene` | `crates/rewrites/` | Guest-free: `scene-extract` reads memory through its own trait and depends only on `frame-scene` |
| `hostfault` | `contract` | `crates/runtime/hostfault` | The process-wide fault handler; `libc` only |
| `sim-tier` | `sim-tier` | `crates/recompiler/sim-tier` | A second lowering of `ir`, beside `emit` and `clif`. Not committed on its branch yet |
| `recomp` | (contract work) | `crates/runtime/recomp` | XBE to a loaded module, out of `boot/src/lib.rs`. Not on a branch yet; it is S4 |

A lane whose crate lands **before** window 1 adds it as `crates/<name>`
as it does now; the window moves it, because every name above is already
in `scripts/crate-domains.toml`. A lane that lands **after** window 1
either replays the window (the script moves the lane's new crate too) or
creates the crate in its domain directory. A crate under a new name needs
one line in a `crates` list first; the window script stops and says so.

## The splits, by domain

Sizes are from the scratch run (see "Scratch run"), in lines of Rust.

**`feature`** (32,783 lines) becomes `hooks` and gives away what is not
the hook engine:

| Part | Lines | Goes to | Window |
|---|---:|---|---|
| The hook engine: `lib`, `plan`, `install`, `entry`, `arena`, `bridge`, `snapshot`, `locale`, `leaves` | 3,973 | `crates/hooking/hooks` | W2 (rename) |
| `switches` | in the above | `crates/hooking/switches` | S2, the `guest-layout` lane |
| The native renderer's guest half: 24 modules (`native_*`, `pipeline_*`, `render_*`, `occlusion`) and 16 test files | 22,272 | `crates/rewrites/render-bridge` | S3 |
| The sim hooks: `sim_frame`, `sim_native`, `sim_observer`, `sim_physics`, `sim_static_light`, `sim_submix` | 1,390 | `crates/rewrites/sim-bridge` | S3 |
| Halo 1 hooks: `h1_projectiles`, `h1_scope_zoom` | 560 | `crates/extensions/h1-runtime` | S3 |

**`engine`** (36,378 lines) keeps the ports and gives away the hooking
parts: `points`, `memory`, `handle`, `datum`, `view`, `tags`, `testing`
to `crates/hooking/guest-layout`, and `entry` to `hooks` (S2, the
`guest-layout` lane). Its render ports follow the render glue into
`render-bridge` when FrameScene replaces them; the register-level sim
ports stay as oracles until each `impl.sim.native.<subsystem>` lands.

**`boot`** (153,027 lines, 312 files) keeps the composition root:

| Part | Lines | Goes to | Window |
|---|---:|---|---|
| Composition root: `lib.rs`, `registration`, the tests | 6,325 | `crates/apps/boot` | stays |
| Build and load: `fallback`, `shim`, `sse`, `x87`, `tier`, `host_coverage`, `build.rs`, `c/`, and the pipeline in `lib.rs` | about 3,200 | `crates/runtime/recomp` | S4 |
| Everything else, as one crate first | 146,827 | `crates/extensions/game` | S5 |
| Render glue: 29 modules (`native_*_glue`, `d3d_glue`, `d3d_diag`, `render_thread`, `opaque_streams`, ...) | 18,694 | `crates/rewrites/render-bridge`, below `game` | S6 |
| Halo 1 runtime: 14 `h1_*` modules | 20,572 | `crates/extensions/h1-runtime` | S6 |
| UI toolkit: `ui_kit`, `list_class`, `ui`, `icons`, `keyboard_glue` | 17,136 | `crates/extensions/ui-kit`, below `game` | S6 |
| Live and co-op glue: 13 modules `[x]` | 16,607 | `crates/extensions/net-glue` | S6 |
| Theater: `theater`, `theater_ui` `[x]` | 10,894 | `crates/extensions/theater` | S6 |
| Bots and sim mode: 8 modules `[x]` | 8,792 | `crates/extensions/bot-glue` | S6 |
| Menus, settings pages, skulls, variants, campaigns, gameplay hooks, HLE glue | 54,115 | stays in `crates/extensions/game` | |

The render glue goes **down**: `rewrites` may not use `extensions`, so
`render-bridge` cannot name a module of `game`. The 33 references it has
today (to `h1_cinematics`, the settings table and others) are cut first.

**`cli`** (79,681 lines) keeps `main.rs` and depends on its command
crates:

| Part | Lines | Goes to |
|---|---:|---|
| `main.rs` and the dispatch | 2,913 | `crates/apps/cli` |
| `safe_out`, then the `Command` registry and the disc helpers | 651 | `crates/apps/cmd` |
| Host commands: `boot_cmd`, `sim_cmd`, `sim_store_cmd`, `host_cmd`, `theater_cmd`, `agent_*` | 7,509 | `crates/apps/cmd-host` |
| `play_cmd`, `settings_cmd`, `movies_cmd` | 2,592 | `crates/apps/cmd-play` |
| 19 tag, map and asset commands | 8,860 | `crates/apps/cmd-tags` |
| `emit_cache`, `emit_output`, then the recompiler commands in `main.rs` | 467 | `crates/apps/cmd-recomp` |
| `h1_cmd`, `h1_import`, `mappack_cmd` | 4,512 | `crates/apps/cmd-h1` |
| `lab`, `follow`, `web_build` | 6,052 | `crates/apps/cmd-dev` |
| The Live art pipeline: 29 `live_*` modules `[x]` | 27,708 | `crates/tools/live-art` |
| Live client and admin commands `[x]` | 7,527 | `crates/apps/cmd-live` |
| Bot and co-op test drivers `[x]` | 11,005 | `crates/apps/cmd-drive` |

After every window: 99 packages; `boot` 6,325 lines, `cli` 2,913, `hooks`
3,973, `game` 54,115.

## How a window runs

A window is one commit that a script made and nothing else touched.

```sh
scripts/moves/window.sh list <id>     # the git mv lines; changes nothing
scripts/moves/window.sh run <id>      # the coordinator, on a clean master
```

`run` tags the tip `move-<id>-pre`, runs the window's script, commits
exactly its output and tags that `move-<id>`. Tags are shared by every
worktree. Then the coordinator checks the window:

```sh
git diff -M50% --diff-filter=AD --name-only move-<id>-pre move-<id>   # only files the window says are new or gone
python3 scripts/crate-graph.py layers --check
python3 scripts/crate-table.py && python3 scripts/docs-check.py docs
cargo check --workspace --all-targets
cargo check -p cli --no-default-features --features host
cargo test --workspace --no-run
```

`Cargo.lock` holds no paths, so window 1 leaves it alone. A window that
renames or adds a package (every other one) leaves the lock file naming
the old packages; the first cargo command rewrites it, offline, and that
goes in the commit after the move. Compile slips are fixed in following
commits too, never by amending the move: lanes replay the script, and the
script's output must be what `move-<id>` holds.

Every script:

- refuses a dirty tree, and stops if a path it rewrote does not resolve;
- can run twice (a path that already resolves is left alone);
- prints what it left for a person: the `crates/...` strings in Rust code
  that are data and not paths, and anything it could not resolve;
- ends by checking that every manifest parses, every path dependency
  finds a `Cargo.toml`, and no tracked file is left behind.

What a script rewrites, besides the `git mv`:

| What | Rule |
|---|---|
| `path = "..."` in every `Cargo.toml`, `[workspace.dependencies]` | resolved against where the manifest used to be, then written for where both ends are now |
| `[workspace] members` | one glob per domain |
| Relative paths in the moved files: `include_str!("../../emit/c/x.c")`, `#[path]`, `CARGO_MANIFEST_DIR` joins (`"../../disc/halo2.iso"`), markdown links, the worktree's climb to the main checkout's `disc/` | the same: old base, new base. A string that is not a path into the repository (`"../evil"`) is left alone |
| `crates/<old>` written from the repository root, in docs, scripts, `platform/`, `deploy/`, `web/`, and Rust and manifest comments | the longest moved prefix |
| A fixed list per window for what no rule sees: `"$here/../.."` in two shell scripts, the `crates/*/README.md` glob in `docs-check.py`, two real paths inside Rust strings | exact text, counted |

Rust strings that name a crate path are not rewritten by rule. The
generated-C banners (`"Generated by crates/emit ..."`) would change every
emitted file and its cache key, and `cli/src/follow.rs`'s tests build a
stand-in tree. The script lists them: 13 after window 1.

## Rebasing a branch over a window

**Replay** is exact and is the default for a branch that touches moved
paths:

```sh
scripts/moves/window.sh replay <id>   # on the lane's branch, clean tree
git merge master
```

It merges `move-<id>-pre` (an ordinary merge; any conflict here is one the
lane had with master anyway), runs the same script on the lane's tree,
commits, and records `move-<id>` as merged with `git merge -s ours`. That
is exact because `move-<id>` is the script applied to `move-<id>-pre`,
and the lane has just applied the same script to a tree that contains
`move-<id>-pre`. The script walks directories and rewrites by rule, so
the lane's new files, new crates and new `use` lines are carried too. A
branch that is several windows behind replays them in order.

A lane that keeps a linear branch squashes instead:

```sh
git rebase move-<id>-pre
python3 scripts/moves/<script> && git commit -qm "Apply move <id>"
git reset --soft move-<id> && git commit -m "<lane>: work so far, over move <id>"
git rebase master
```

**A plain merge** is enough where rename detection carries the lane's
edits:

```sh
git -c merge.renameLimit=30000 -c merge.directoryRenames=true merge -X rename-threshold=30 master
```

| Window | Plain merge | Why |
|---|---|---|
| W1 | works | Pure renames: git matches all 2,668 files at the default 50% (the lowest are `lockstep/Cargo.toml` at 56% and `boot/Cargo.toml` at 60%, hence the lower threshold). `directoryRenames` puts a lane's new files in the new directories. A lane's new `path = "../x"` or new crate still needs the script: run `python3 scripts/moves/w1-domains.py` after the merge |
| W2 | works unless the lane edited a line that says `feature::` | 1,047 one-word edits in 193 files; conflicts are those lines. New lane code that says `feature::` needs the script |
| W3 | replay | `crate::` becomes `crate::<module>::` on 366 lines of the merged sources, so a lane's edits beside them conflict (3 files of `h1-bg-prompts`' 7) |
| S3 to S7 | replay | Files change crate; `crate::` and `pub(crate)` lines change with them |

## The windows, in order

| # | Window | Script | Moves | Ready |
|---|---|---|---|---|
| W1 | Every crate into its domain directory | `w1-domains.py` | 86 `git mv`, 2,668 files renamed, 513 edited | now |
| W2 | `feature` becomes `hooks` | `w2-hooks.py` | 91 files renamed, 193 edited | after W1 |
| W3 | Eleven Halo 1 crates become five | `w3-h1-merge.py` | 99 files renamed, 7 manifests removed, 121 edited | after W1; `h1-contract` first (below) |
| S1 | Small renames and deletions | `rename-crate.py` | 2 crates renamed, 1 deleted | needs decisions 2 and 3 |
| S2 | `guest-layout` and `hooks` take `engine`'s hooking parts | the `guest-layout` lane | 8 files, about 4,000 lines | when that lane lands |
| S3 | `hooks` gives away the sim, render and Halo 1 hooks | `split.py s3-sim-bridge`, `s3-render-bridge`, `s3-h1-hooks` | 6 + 24 + 2 modules, 24,222 lines | sim and Halo 1: after W2. Render: 7 references to cut |
| S4 | `recomp` out of `boot` | the contract lane | 7 files and 630 lines of `lib.rs` | before the contract work starts |
| S5 | `boot` keeps the root, `game` takes the rest | `s5-boot-root.py` | 151 modules, 291 files, 146,827 lines | 96 references to cut, about 30 of them S4's |
| S6 | `game` gives away the render glue, the Halo 1 runtime, the UI toolkit and the Live glue | `split.py s6-*` (six specs) | 71 modules, 92,695 lines | 3 to 82 references each |
| S7 | `cli` becomes `main.rs` and ten command crates | `split.py s7-*` (ten specs) | 91 modules, 76,883 lines | four specs now, the rest 1 to 29 references each |

"Ready" counts are from `--check` on the scratch tree with every earlier
window applied. They are what stands between today and a mechanical move.

### W1. Domain directories

`scripts/moves/w1-domains.py`. `crates/<crate>` becomes
`crates/<domain>/<crate>` for all 86 crate directories, by the `crates`
lists in `scripts/crate-domains.toml`; `--list` prints the 86 lines. No
crate is renamed, split or merged, and no Rust code changes except path
strings: 189 `path` values in 52 manifests, 199 relative paths in the
moved files, 1,805 `crates/<crate>` mentions in 370 docs, scripts and
comment lines, 8 fixed edits.

- `crates/runtime` moves into itself (`crates/runtime/runtime`); the
  script parks it under another name first, before `kernel`, `vfs`,
  `snapshot` and `timeline` arrive beside it.
- `docs-check.py` reports the same 46 link findings before and after.
- Lanes: every branch with work under `crates/` (52 of the 61 branches
  ahead of master). Plain merge or replay; see the table above.
- Do it first and alone. Every later window's paths are written against
  this layout.

### W2. `feature` to `hooks`

`scripts/moves/w2-hooks.py`. `crates/hooking/feature` becomes
`crates/hooking/hooks`; the package name; the dependency key in `boot`,
`agent`, `lockstep` and `live-telemetry`; `"feature/native-render"` in
`boot`'s features; 1,047 `feature::` paths in Rust (`boot` has about 750);
7 `-p feature` in docs; the symbol prefix in
`scripts/sim-attribution/attribute.py`; the two exceptions in the rules
file.

- `live-telemetry` has its own `mod hooks`, so the script writes
  `::hooks::` there. `[x]`: 14 lines in that owner's crate.
- Not touched: cargo features, `#[cfg(feature = "...")]`, the word in
  prose (36 backticked mentions in 6 docs, the docs lane's), and
  `docs/engine/feature-engine.md`'s name.
- `rename_crate` in `movelib.py` is the whole of it;
  `scripts/moves/rename-crate.py <old> <new>` renames any other crate the
  same way. It rewrites Rust only in the renamed crate and the crates
  that depend on it, and leaves, and names, a file where the old name is
  a module or an import of something else.

### W3. The Halo 1 merge

`scripts/moves/w3-h1-merge.py`.

| Before | After |
|---|---|
| `h1map` | `h1map` |
| `h1bitmaps`, `h1models`, `h1sky`, `h1sounds`, `h1text` (22,400 lines) | `h1-assets`, modules `bitmaps`, `models`, `sky`, `sounds`, `text` |
| `h1convert` | `h1convert` |
| `h1objects` | `h1objects` |
| `h1campaign`, `h1ai`, `h1scripts` (33,300 lines) | `h1campaign`, with modules `ai`, `scripts` |

For a crate S that becomes module M of crate T: `S/src/lib.rs` moves to
`T/src/M/mod.rs` and the rest of `S/src` to `T/src/M/`; `S/tests` to
`T/tests/M/`, each file with its own `[[test]]` named `M_<file>`, so
every test binary keeps its root and its `mod common;`; `S/examples`
likewise. In Rust, `crate::` becomes `crate::M::` in the moved sources
(366 places); `S::` becomes `crate::M::` inside T and `T::M::` everywhere
else (726 places). T's manifest takes the union of the dependencies; the
other manifests' several `h1bitmaps = ...`, `h1models = ...` lines fold
into one `h1-assets = ...`, in `[features]` too.

- **`h1-contract` goes first** (prep, lane work, not a script). `boot`
  names about 30 items of the converters in 17 files:
  `h1text::hud::{SCENARIO_HELP, SILHOUETTE_BASE, HELP_FLASH, ...}`,
  `h1scripts::cinematics::{ACTIVE_NAME, VIDEO_BITMAPS, Field, MARKER}`,
  `h1scripts::unit_builtins`, `h1scripts::compile::field::STRINGS`,
  `h1convert::shad::{COUNTDOWN_DIGITS, MODEL_REFLECTION_SIGNATURE}`,
  `h1convert::ui::{CAMPAIGN_ORDER, campaign_card}`,
  `h1convert::apply::precision_swaps`, and the `h1map` reader in the
  campaign picker. They move to `crates/formats/h1-contract` and `boot`
  drops the converter crates. Without it the merge still builds, but
  `boot`'s `h1scripts` becomes all of `h1campaign`, which brings
  `h1objects`: the host-only build grows from 806,060 to 837,778 source
  lines.
- A branch on `build-1120`'s one-test-binary layout is handled: a crate
  with `autotests = false` carries its declared `[[test]]` and nothing
  else.
- Not rewritten: the old crate names in prose (241 mentions in 26 docs).

### S1. Small renames and deletions

`scripts/moves/rename-crate.py sim sim-bench` and
`scripts/moves/rename-crate.py apple-host h2host` (the package takes its
library's name; `platform/apple/build.sh` and `platform/xbox/build.sh`
name the package and are edited by hand). On the scratch tree the first
renames 84 `sim::` paths in `boot` and `cli` and leaves
`cli/src/agent_sim.rs`, where `sim` is `agent_proto::sim`; the second
changes no Rust. `git rm -r` of `fastmem-proto`.
`oracle`'s WMA oracle (`src/wma.rs`, a test, an example) moves into `wma`
behind an `oracle` feature, which clears the `oracle` → `wma` exception.
`viewer`: decision 3.

### S2. `guest-layout` and `hooks` take `engine`'s hooking parts

The `guest-layout` lane's. `engine/src/{points, memory, handle, datum,
view, tags, testing}.rs` move to `guest-layout`, and `engine/src/entry.rs`
to `hooks`. `engine` re-exports them, so the 605 `engine::` paths in
`boot` and 88 in `bots` are untouched. `hooks` then depends on
`guest-layout` and not on `engine`: the first exception goes. `points.rs`
is one table of 374 entries and the second hottest file in the tree; it
splits by owner when the registration work lets each crate keep its own
(see "Local registration").

### S3. `hooks` gives away what is not the hook engine

`scripts/moves/split.py <spec>`, three specs in `scripts/moves/splits/`.
Each new crate depends on `hooks`; `boot`, `agent` and the others that
named `hooks::<module>` name the new crate and gain the dependency.

| Spec | To | Modules | Lines | Stops it today |
|---|---|---:|---:|---|
| `s3-sim-bridge` | `crates/rewrites/sim-bridge` | 6 | 1,390 | nothing. `sim_frame` names `pipeline_render`, so `sim-bridge` depends on `render-bridge` until that edge is cut |
| `s3-render-bridge` | `crates/rewrites/render-bridge` | 24, and 16 tests | 22,272 | `switches.rs` lines 120 to 126 name seven `native_*` modules |
| `s3-h1-hooks` | `crates/extensions/h1-runtime` | 2 | 560 | nothing |

After `s3-render-bridge`, `hooks`' `native-render` feature and dependency
are deleted by hand and the second exception goes with them. Do
`s3-sim-bridge` right after the `sim-anim` lane merges: that lane and
`sim-collision` both edit `sim_native.rs`.

### S4. `recomp` out of `boot`

Lane work, before the contract implementers start, so thin transfers and
flat RAM never touch `boot/src/lib.rs`. `fallback`, `shim`, `sse`, `x87`,
`tier`, `host_coverage`, `c/`, `build.rs`, `tests/fast_tier.rs` and the
pipeline in `lib.rs` (`BRIDGE` to `Module`: stamps, the analysis cache,
`build*`) go to `crates/runtime/recomp`; `boot` re-exports the names
`cli` uses (`boot::tier`, `boot::build`, ...). `boot` loses its build
script, so a `Cargo.lock` change stops recompiling it. This also removes
about 30 of S5's 96 references (`stamp_path`, `analysis_cache`,
`build_stamp`, `DYLIB`, `dylib_current`, ...).

### S5. `boot` keeps the root

`scripts/moves/s5-boot-root.py`. Every top-level module of `boot/src`
except `lib.rs` and `registration` moves, unchanged, to
`crates/extensions/game/src`. `boot` depends on `game`, forwards its
features, and starts with `pub use game::*;`, so `crate::skulls` in the
root and `boot::tier` in `cli`, `studio` and the hosts still resolve, and
no other crate changes. `game`'s manifest is a copy of `boot`'s without
the dependencies `extensions` may not have (`analyze`, `lift`, `emit`,
`clif`, `ir`, `x87`, the Halo 1 converters, `perf`): what still needs one
of those is a layering error that `cargo check` then names.

- Stops it today, 96 references from modules to the root:
  `crate::telemetry` 32, `crate::log` 14, `crate::registration` 10
  (`d3d_glue` reads `compiled_render_hooks`), `RunOpts` 5, `Host` 2,
  `LiveConfig` 1, and S4's. Each moves into a module first.
- `pub(crate)` becomes `pub` in the moved files: the root is another
  crate.
- Lanes: every branch with work under `boot/src`. Replay.
- By itself this saves nothing. It makes each S6 window a move out of
  `game` that does not touch the root.

### S6. `game` gives away its groups

`scripts/moves/split.py <spec>`. "Up" means the new crate depends on
`game` and no module left in `game` may name one that leaves; `boot`'s
root gains `pub use <crate>::*;`. "Down" means `game` depends on the new
crate and re-exports its modules, and no module that leaves may name one
that stays.

| Spec | To | Direction | Modules | Lines | References to cut |
|---|---|---|---:|---:|---:|
| `s6-h1-runtime` | `crates/extensions/h1-runtime` | up | 14 | 20,572 | 3 |
| `s6-theater` `[x]` | `crates/extensions/theater` | up | 2 | 10,894 | 14 |
| `s6-bot-glue` `[x]` | `crates/extensions/bot-glue` | up | 8 | 8,792 | 29 |
| `s6-net-glue` `[x]` | `crates/extensions/net-glue` | up | 13 | 16,607 | 56 |
| `s6-render-glue` | `crates/rewrites/render-bridge` | down | 29 | 18,694 | 33 |
| `s6-ui-kit` | `crates/extensions/ui-kit` | down | 5 | 17,136 | 82 |

`scripts/moves/split.py <spec> --check` lists each reference with its
file and line. About half of them go through four central lists
(`setting_defs`, `string_ids`, `layout_glue`, `ui_kit/screens.rs`), which
is the registration work below. `theater` names `bot_glue`, `sim_glue`,
`coop_live` and `coop_look`, so it goes after `bot-glue` and `net-glue`
or those are cut.

### S7. `cli` becomes `main.rs` and command crates

`scripts/moves/split.py <spec>`, all down: `cli` depends on each crate
and its `mod x;` becomes `pub use <crate>::x;`. `s7-cmd` goes first; a
later crate's `crate::safe_out` then becomes `cmd::safe_out`.

| Spec | To | Modules | Lines | References to cut |
|---|---|---:|---:|---:|
| `s7-cmd` | `crates/apps/cmd` | 1 | 651 | 0 |
| `s7-cmd-tags` | `crates/apps/cmd-tags` | 19 | 8,860 | 0 |
| `s7-cmd-recomp` | `crates/apps/cmd-recomp` | 2 | 467 | 0 |
| `s7-cmd-drive` `[x]` | `crates/apps/cmd-drive` | 10 | 11,005 | 0 |
| `s7-cmd-live` `[x]` | `crates/apps/cmd-live` | 9 | 7,527 | 1 |
| `s7-cmd-dev` | `crates/apps/cmd-dev` | 3 | 6,052 | 2 |
| `s7-cmd-host` | `crates/apps/cmd-host` | 11 | 7,509 | 8 |
| `s7-cmd-h1` | `crates/apps/cmd-h1` | 4 | 4,512 | 9 |
| `s7-live-art` `[x]` | `crates/tools/live-art` | 29 | 27,708 | 20 |
| `s7-cmd-play` | `crates/apps/cmd-play` | 3 | 2,592 | 29 |

A zero means no `crate::` path leaves the set. Helpers that `main.rs`
defines and a command reaches through `super::` or a `use` are not
counted; `default_disc`, `open_iso`, `load_xbe` and `repo_root` move to
`cmd` by hand before the rest. `live-art` lands in `tools` without `cli`'s
dependencies on `apps` and `extensions` crates, so the art pipeline stops
recompiling when `boot` changes.

## Halo 1 branches

Of 41 `h1*` branches, 37 have no commit that master lacks (`h1-integ`,
`h1-integ2`, `h1-integ3`, `h1-integ-fix2` and every `h1-full-*`,
`h1-next-*`, `h1-push-*` among them): their worktrees hold nothing to
rebase, and none has uncommitted changes. Four are ahead
(`git log master..<branch>`):

| Branch | Commits | Touches | Over W1 and W2 | Over W3 |
|---|---:|---|---|---|
| `h1-bg-prompts` | 1 | `h1text` 7 files, `h1objects` 2, `h1convert` 2, docs 2 | plain merge is clean; replay is exact | replay: its 13 files and 649 added lines arrive as `h1-assets/src/text/...`, the new `multiplayer.rs` included. A plain merge conflicts in 3 files |
| `h1-fall-ragdoll` | 4 | `h1convert` 1 file | clean either way | clean either way: `h1convert` stays a crate |
| `h1-parity-suite` | 1 | `agent` 7, `h1objects` 4, `cli` 4, `hot-tags` 2, docs 3 | conflicts with master today in `engine/src/points.rs` and `docs/runtime/agent-api.md`, before any move. Resolve that merge at `move-w1-pre`, then replay | no merged crate touched |
| `h1-bg-bots` | 1 | `bots` 16 files, docs 2 | conflicts with master today in three `bots/src` files; they are the Live owner's. Same: resolve at `move-w1-pre`, then replay | not touched |

So the merge is safe to run as soon as `h1-bg-prompts` is merged or
replayed; nothing else in flight edits a crate that W3 dissolves.

## Scratch run

A worktree under `$TMPDIR` at this branch's tip, each script run in
order with a commit between, `layers --check` after each, the worktree
removed afterwards. Splits were forced past their `--check`.

| Step | Script exit | `layers --check` | Result |
|---|---|---|---|
| W1 | 0 | 0 | 2,668 renames and 201 other files, no add or delete at `-M50%`; second run changes nothing |
| W2 | 0 | 0 | 91 renames, 193 edited; no `feature::` path left in a crate that depends on it |
| W3 | 0 | 0 | 99 renames, 2 added, 7 removed; every `mod x;` of the merged crates finds its file; second run changes nothing |
| S1 renames (2) | 0 | 0 | `sim`: 9 renames, 19 edited. `apple-host`: 8 renames, 11 edited |
| `s3-*` (3) | 0 | 0 | |
| S5 | 0 | 0 | `boot/src` is `lib.rs` and `registration` |
| `s6-*` (6), `s7-*` (10) | 0 | 0 | 99 packages at the end |

`h1-bg-prompts` and `h1-fall-ragdoll` were replayed over W1, W2 and W3;
each ends with exactly its own diff against the moved tree (13 files,
+649 −58; 1 file, +588).

Two things the check caught on the way, both fixed in the plan: moving
the render glue up made `render-bridge` depend on `game` (it goes down
instead), and `live-art` in `live` needed the Halo 1 converters (it is a
tool).

## Local registration

One signature, one list per crate, sorted, one word per line.

```rust
// crates/hooking/hooks/src/host.rs
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
// crates/extensions/game/src/skulls_mp.rs
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
// crates/extensions/game/src/lib.rs: the one line a lane adds, in its own crate.
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
// crates/apps/boot/src/stages/register.rs: the only list that names crates.
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
| `boot/src/lib.rs`: `run_configured`, 1,524 lines | `const STAGES: &[(&str, fn(&mut Boot) -> Result<()>)]` and `boot/src/stages/{profile, module, live, content, kernel, register, video, audio, input, agent, plan, movies}.rs`, then `main_loop.rs`. `Boot` holds what the stages pass along; a stage puts its services in `Host` |
| `boot/src/setting_defs.rs`, `string_ids.rs`, `layout_glue.rs`'s rows, `ui_kit/screens.rs` | `reg.settings(DEFS)`, `reg.string_ids(FAMILY)`, `reg.layout_rows(ROWS)`, `reg.screen(DESCRIBE)` from the owning module. `string_ids.rs`'s collision check runs over the registry |
| `cli/src/main.rs`: 85 `mod` lines, `USAGE`, a chain of `if cmd ==` | `cmd::commands! { anim, bitmap, ... }` in each command crate, each module with `pub const COMMAND: cmd::Command`. `main.rs` calls each crate's `register(&mut Commands)` under its `cfg` and `Commands::run`. `USAGE` is printed from the table |
| `feature/src/sim_native.rs`: one install list for every sim port | `sim-bridge/src/{anim, collision, ...}.rs`, one per subsystem, in `hooks::modules!` |

Rules:

- **Sorted lists.** Two lanes adding different lines conflict only when
  the lines are adjacent, and then the resolution is "keep both".
- **Order comes from the plan, not the list.** A module that must run
  around another says so with `HookRef::after_feature`, `first` or
  `last`, which exist (`feature/src/lib.rs`). A test registers
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
scripts/gate.sh move <id>   # a window's check
scripts/gate.sh all         # lint, check, test, disc
```

| Tier | Runs |
|---|---|
| `lint` | `scripts/crate-table.py --check`; `scripts/crate-graph.py layers --check` (the domain rules); the host budget (`closure cli --features host` crate count at most the number in the script); the docs check from the docs lane; `rustfmt --check` on files changed against `master` that were clean there |
| `check` | `cargo check --workspace --all-targets`; `cargo check -p cli --no-default-features --features host`; `cargo clippy --workspace --all-targets -- -D warnings` |
| `test` | `cargo nextest run --workspace` with `.config/nextest.toml`; `cargo test --workspace --doc` |
| `disc` | With `H2_DISC`, `H1_ISO`, `H2_XBE` and `H2_ORACLE_FIXTURES` set from `disc/` when unset: `cargo nextest run --workspace --run-ignored ignored-only -E 'not test(/bench|slow/)'`; `cargo test -p lift --features oracle-fixtures --test oracle`; `cargo test -p engine --features differential`. Prints the count of tests that ran and refuses zero |
| `move <id>` | the rename-only diff between `move-<id>-pre` and `move-<id>`, then `lint`, `check`, `cargo test --workspace --no-run` |

- Every cargo call goes through the shared test ledger, which moves into
  the repository as `scripts/tested` with the cargo-slot path read from
  `H2_CARGO_SLOT` instead of a session directory. A pass for a tree is
  then a fact any agent can reuse.
- The disc tier needs one convention: a test that reads a disc has `_disc`
  in its module or test name, as `h1objects/tests/*_disc.rs` and
  `.config/nextest.toml` already assume, and a benchmark has `bench`.
  Today's 530 `#[ignore]`s carry 25 different reasons and 59 carry none.
- The coordinator runs `all` before a push and `move <id>` in a window.

### CI: `.github/workflows/ci.yml`

CI has no disc and never will. It runs what it can and says what it did
not.

| Job | Runner | Runs | Catches |
|---|---|---|---|
| `lint` | ubuntu | `scripts/gate.sh lint` | a crate with no description or outside its domain, a refused dependency, a stale table, the host budget |
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
per package, read from the manifests by `scripts/cratemeta.py`.

| Column | Source |
|---|---|
| crate | the package name and what it links besides an rlib (`bin h2`, `staticlib h2host`) |
| domain | the crate's directory, `crates/<domain>/<crate>`; for a crate still flat in `crates/`, where `scripts/crate-domains.toml` places it. Rows sort by domain, lowest first |
| purpose | `description` |
| docs | `[package.metadata.h2] doc`, else the `docs/...md` paths in the description |
| status | `[package.metadata.h2] status` |

A cell with no source keeps the text the old table had; a purpose kept
that way carries a dagger. `--check` fails when the file is stale;
`--strict` also fails on a missing description or domain. The table holds
no line counts, because they would make it stale on every commit;
`scripts/crate-graph.py sizes` prints them. Run it after every window.

Thirty manifests have no `description`. Seven of them had no row either;
proposed text:

| Crate | `description` |
|---|---|
| `clif` | The Cranelift fast tier: planned IR straight to host objects, linked into the same kind of module clang's build makes (docs/lifter/cranelift-tier.md) |
| `h1campaign` | Halo 1 campaign levels converted onto pinned retail Halo 2 campaign maps, with their AI and scripts (docs/halo1/campaign.md) |
| `h1objects` | Halo 1 sandbox objects (weapons, projectiles, equipment, bipeds, vehicles) as Halo 2 tags (docs/halo1/objects.md) |
| `runtime-test-probe` | Native declarations of the fixture generated module, for tests that run hooks through the real runner |

(`h1scripts`, `h1sky` and `h1text` stop being packages in W3; `h1-assets`
gets its description from the script.)

## For the Live, bots, theater and film owner

Everything marked `[x]`. The scripts run on these files; nothing here was
edited by hand, and each item is that owner's to confirm or change.

1. **W1 moves eleven crates into `crates/live/`**: `live-proto`,
   `live-hle`, `live-telemetry`, `live-server`, `live-worker`, `coop`,
   `bots`, `film`, `cloud-saves`, `host-data`, `sim`. No file changes
   except paths: `live-worker`'s `deploy.sh`, `smoke/run.sh`,
   `smoke/smoke.ts` and `wrangler.toml` each climb one directory more to
   reach `web/` and `deploy/`, and `web/test/*.ts` name
   `crates/live/live-proto/...`. A deploy from a tree after W1 uses the
   new `wrangler.toml` paths; check one.
2. **`live` may use `hooking` and `rewrites` and not `extensions`.** Every
   crate above already complies.
3. **W2** writes `::hooks::` on 14 lines of `live-telemetry`, which has
   its own `mod hooks`.
4. **The in-game glue is three `extensions` crates** (S6): `net-glue` (13
   modules, 16,607 lines), `bot-glue` (8, 8,792) and `theater` (2,
   10,894), each above `game`. The module lists are in
   `scripts/moves/splits/s6-{net-glue, bot-glue, theater}.toml`. `game`
   names them 56, 29 and 14 times today, mostly through
   `coop_probe::read_session` and its constants (guest layout, which
   belongs in `guest-layout`) and the central lists.
5. **`cli`** (S7): `live-art` in `tools` (29 modules, 27,708 lines),
   `cmd-live` (9, 7,527) and `cmd-drive` (10, 11,005) in `apps`.
   `live_query` and `live_recovery` are placed in `cmd-live` by their
   names; say if they are the art pipeline's.
6. **`sim` becomes `sim-bench`** (S1), which frees the name before
   `sim-*` multiply. It renames 84 `sim::` paths in `boot` (`sim_glue`,
   `host_lobby`, `host_mem`, `theater`) and `cli` (`sim_cmd`).
7. **`live_server::identity::write_private`** is `cli`'s only use of
   `live-server` outside the desktop feature, and puts 51,962 lines and
   axum, sqlx, reqwest and object_store in the dedicated host's build. In
   `live-proto` or `host-data` it would not.
8. **`live-proto`'s rebuild cone is 48%.** `bots` uses two of its modules
   and `film` four; a small types crate under them would keep an API
   change in `session` or `matchmaking` from recompiling `bots`, `boot`
   and `cli`.
9. **`h1-bg-bots`** is one commit of 16 `bots` files that conflicts with
   master today.

## Decisions needed

Decided: domain directories; `feature` becomes `hooks`; the Halo 1 merge
to five crates and `h1-contract`.

1. **The Live glue in `extensions`** (and `live-art` in `tools`,
   `cmd-live` and `cmd-drive` in `apps`), marked as the Live owner's, so
   that the rules stay one table by directory. The other choice is to
   keep all of that owner's crates under `crates/live/` and give `live`
   two tiers in the rules: the service and libraries below `extensions`,
   three glue crates above it.
2. **`apple-host` becomes `h2host`, `sim` becomes `sim-bench`** (S1).
3. **`viewer`:** delete `shadowlab` (3,730 lines) and keep `bsp` and
   `bsp_cracks` as `h2` commands in `cmd-tags`, or keep the crate in
   `tools`.
4. **`h1_options`, `h1_options_row` and `h1_variants` stay in `game`.**
   The Clankers and theater pages use them as menu rows; the other 14
   `h1_*` modules are `h1-runtime`.
5. **Who cuts the references.** One lane working down the `--check` lists
   of S5 and S6, with the coordinator landing a window after each.
6. **When W1 runs.** It touches every branch. The cheapest moment is
   right after an integration that leaves few lanes open; `integ-1120`
   and `build-1120` landing first means two fewer large replays.

## Not verified

- Nothing was compiled. Each script's output passed the structural
  checks above; `cargo check` has not seen any of it.
- W2: a module or import named `hooks` in a crate that depends on the
  renamed crate would shadow it. Only `live-telemetry` has one at its
  root, and the script handles that; nested ones were searched for and
  not found.
- W3: an item that was `pub(crate)` in a small crate is now visible
  across the merged one, and two merged modules may glob-import the same
  name. Both are compile errors if they bite, not silent changes.
- The splits copy a manifest and trim it by the domain rules only, do not
  make an item `pub` when the other crate needs it (S5 widens
  `pub(crate)` in bulk; the others do not), and cannot see an `impl` the
  orphan rule refuses, a `super::` path that crosses the cut, or a path a
  macro builds. `--check` passing is necessary for a split, and `cargo
  check` decides.
- A `CARGO_MANIFEST_DIR` join to a file inside the crate (no `../`) is
  not rewritten when a split moves the code to another crate.
- The reference counts for S5 to S7 were taken with the earlier splits
  forced, so a count can include an edge an earlier prep would have cut.
- Whether a sorted registration order gives the same plan as today's
  order. The forwards-and-reversed test is how to find out.
- `live-worker`'s deploy and smoke scripts after W1: the paths were
  rewritten by rule and not run.
- The scratch tree was this branch, which is master plus the docs
  merges; on master alone the counts of rewritten doc mentions differ.
