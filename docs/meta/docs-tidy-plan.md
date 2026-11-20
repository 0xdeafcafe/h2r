# Docs tidy: review and plan

Written 2026-11-20 against master `21ca66709`. Nothing was built or run.
The evidence is the docs, the code, `git log`, and
`scripts/docs-check.py`, which this branch adds.

This page is a one-off. The standing rules are in
[docs-style.md](docs-style.md): the kinds of page, where each goes, the
house style, the evidence labels, the template and the checker. The last
lane deletes this page.

## Findings

179 pages under `docs/` on master, about 152,000 lines, written between 2026-11-14 and 2026-11-20.

| Verdict | Pages | Of which for the Live owner |
|---|---:|---:|
| Keep | 80 | 8 |
| Fix stale facts | 55 | 9 |
| Rewrite as current design | 14 | 1 |
| Split (design page and notes) | 16 | 6 |
| Move (whole page is notes or a review) | 9 | 1 |
| Delete | 5 | 1 |
| Total | 179 | 26 |

What the review found, in order of cost to a reader:

1. **Design pages that are logbooks.** `graphics/native-renderer.md` is
   3,792 lines and has changed 169 times. Its third line says every
   stage is on by default; line 3605 says every native switch stays off.
   `game/bots.md` is 5,701 lines and keeps a retired design and a
   "development history". 83 headings across the tree carry a date, "as
   built", "what was built" or "superseded".
2. **Docs that name things the code no longer has.** The flag cleanup
   (branch `flag-cleanup`, 28cc2b37c, not yet on master) removes 94 `H2_*`
   names and 33 `impl.*` switches (31 of them `impl.render.*`). 86 of the
   removed names appear in 37 docs on master. The cleanup branch edits 44
   docs itself; its docs still name 40 variables that no code reads, most
   of them in `native-renderer.md`, `render-switches.md` (which lists what
   was removed) and `BOOT.md`.
3. **Code comments cite docs, and the citations rot.** 1,010 source files
   cite 157 doc pages about 2,500 times, 680 times with a section. 70 of
   those sections no longer exist. Two comments cited pages that never
   existed (fixed here). This is why pages must not be renamed.
4. **Entry points that are stale.** `ARCHITECTURE.md` lacks 25 of the 86
   crates and describes a test harness that does not exist.
   `lifter/LIFTER.md`, `engine/engine-map.md` and
   `game/settings-menu.md` are "start here" pages that open with a design
   from before the code was written. `docs/README.md` called the agent API
   "design only; none of it is on master" (fixed here).
5. **Lane paperwork in docs.** `live/dedicated-host-build.md` is a contract
   for twenty parallel lanes. `game/mods-ui-review.md` is a review of one
   branch. Three graphics pages carry sections such as "Shared files edited
   by this lane".
6. **Broken links: three.** One fixed here (`runtime/sim.md`). One is in
   `live/dedicated-host.md` (for the Live owner). One is a screenshot that
   was never committed (`editor/editor-guide.md` line 960).

## Before the lanes start

1. **Merge `flag-cleanup` first.** It edits 44 of the docs below and adds
   `docs/runtime/switches.md`. Every lane branches from master after that
   merge. A lane that starts earlier will conflict in almost every file.
2. **Halo 1 lanes (L18 to L20) wait** until the `h1-*` code lanes have
   merged. Those pages changed up to 96 times this week and still do.
3. **Live, bots, theater and film pages are not in these lanes.** Their
   verdicts are in the tables, marked "Live owner", with a suggested split
   at the end.
4. `perf/sim-budget.md`, `perf/sim-attribution.md` and the four
   `engine/sim-rewrite*.md` pages are inputs to design lanes in flight.
   No tidy lane touches them.

## Path changes

Agreed here so that a lane can link to a page another lane is creating.
No other page is renamed or moved.

| Old path | New path | Lane |
|---|---|---|
| `lifter/build-perf.md` | `lifter/notes/build-perf.md` | L01 |
| `lifter/golden-trace.md` | deleted | L01 |
| `runtime/level-preflight.md` | deleted | L02 |
| `perf/gameplay-budget.md` | `perf/notes/gameplay-budget.md` | L07 |
| `perf/menu-budget.md` | `perf/notes/menu-budget.md` | L07 |
| `perf/fastmem-4k.md` | deleted; a row in `perf/notes/not-adopted.md` | L07 |
| `perf/fastmem-proto.md` | `perf/notes/fastmem-proto.md` | L07 |
| `graphics/render-switches.md` | `graphics/notes/not-adopted.md` | L11 |
| `game/mods-ui-review.md` | deleted | L16 |
| `perf/architecture-review.md` | `reviews/2026-11-16-frame-time.md` | LZ |
| `architecture-review-2026-11.md` | `reviews/2026-11-20-architecture.md` | LZ |
| `engine/sim-review.md` | `reviews/2026-11-20-sim.md` | LZ |
| `engine/live-telemetry.md` | `live/notes/telemetry-precision.md` | Live owner |
| `live/dedicated-host-build.md` | deleted | Live owner |

New pages: `lifter/notes/{precedents,fp-census,x87-native,x87-transcendentals,not-adopted}.md`
(L01); `runtime/notes/boot-log.md` (L02); `runtime/notes/{snapshots,sim}.md`
(L04); `perf/notes/{guest-memory,not-adopted}.md` (L07);
`engine/notes/feature-engine.md` (L08);
`graphics/native-{world,objects,transparents,lights,water-fog,post-hud}.md`
and `graphics/notes/native-ab.md` (L09, L10);
`graphics/notes/render-thread.md` (L11);
`graphics/notes/{pipelined-render,scene-pass-timing}.md` (L12);
`graphics/notes/e3-lighting.md` (L13); `game/notes/high-framerate.md`
(L15); `halo1/rebuild.md` (L17).

## Review

Verdicts: **keep** (at most a status line), **fix** (stale facts or a
section to drop; the structure stays), **rewrite** (as the current design),
**split** (the design stays at the path, dated material goes to `notes/`),
**move** (the whole page is notes or a review), **delete**. "Commits" is
`git log --follow` on master. "flag-cleanup" in the evidence means the
branch of that name.

### Top level

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [README.md (root)](../../README.md) | 323 | 58 | cd5e3ccd6 2026-11-20 | **fix** | LZ | Says bots join 'on free controller ports' (line 73); `docs/game/bots.md` line 4 says ports are reserved for people and bots are phantom peers. Lists the Xbox UWP app as Design (line 63); `runtime/xbox.md` says it runs on a Series X. Lists 60 and 120 fps as Design; `game/high-framerate.md` has built slices. Links `native-renderer.md#the-all-native-push-2026-11-1920`, a dated heading. |
| [README.md](../README.md) | 228 | 52 | 64fedb648 2026-11-20 | **rewrite** | LZ | The index. 24 pages were missing from it and two lines were wrong (agent API 'design only; none of it is on master', theater 'not built'); both fixed in this branch. LZ rebuilds it on the layout in docs-style.md. |
| [ARCHITECTURE.md](../ARCHITECTURE.md) | 210 | 27 | 42a1c068e 2026-11-18 | **rewrite** | LZ | Crate table lacks 25 of 86 crates (`agent`, `agent-proto`, `bots`, `clif`, `film`, `native-render`, `sim`, `snapshot`, `voice`, seven `h1*` and others). Says the lifter emits 'LLVM IR or C' (it emits C11), gives `fn(&mut Cpu, &mut Guest)` as the handler signature, and describes a record-and-replay harness that does not exist (review 2.7). `coop` row says 'phase 0' while `live/live-coop.md` says co-op is built and confirmed. Milestone 7 points at the 2026-11-15 menu budget. |
| [architecture-review-2026-11.md](../architecture-review-2026-11.md) | 807 | 1 | 8d35753e4 2026-11-20 | **move** | LZ | A dated review (2026-11-20). Move to `docs/reviews/2026-11-20-architecture.md`. In-flight design branches cite it, so it moves last. |
| [meta/docs-style.md](docs-style.md) | 208 | 1 | 57a0d422d 2026-11-20 | **keep** | - | The standing rules, added in this branch. |
| [meta/docs-tidy-plan.md](docs-tidy-plan.md) | 762 | 1 | de156623c 2026-11-20 | **delete** | LZ | This plan. Deleted when the last lane lands. |

### `lifter/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [lifter/LIFTER.md](../lifter/LIFTER.md) | 173 | 3 | 8a8e88155 2026-11-16 | **rewrite** | L01 | The area's start-here page is the proposal written before the build: line 3 says 'design proposal, written before the build ... where this text and the code differ, the code wins'. |
| [lifter/FLOAT.md](../lifter/FLOAT.md) | 580 | 9 | 8a8e88155 2026-11-16 | **split** | L01 | 'Status: measurement plus design', 'revised after the design review'; section 3 'What to build first' is built. Four code files cite the path, so it stays as a short current page; the 2026-11-15 `h2 fpstats` census goes to notes. |
| [lifter/FLOAT-ABI.md](../lifter/FLOAT-ABI.md) | 694 | 9 | d0ef387e7 2026-11-17 | **fix** | L01 | Status paragraph narrates a review ('Revised after the design review (APPROVE WITH CHANGES)', commit e5c4251). Sections 7 'Where the measurement tool fits now' and 8 'Resolved questions' are history. |
| [lifter/x87-native.md](../lifter/x87-native.md) | 303 | 9 | ef6ce7284 2026-11-16 | **split** | L01 | Design plus three measurement sections: 'Benchmark and profile', 'Real menu baseline and remaining cost', 'Final menu comparison'. |
| [lifter/x87-transcendentals.md](../lifter/x87-transcendentals.md) | 390 | 7 | d0ef387e7 2026-11-17 | **fix** | L01 | flag-cleanup removes the f64 experiment (2658859b9, `crates/x87/src/trans_f64.rs`) and `H2_X87_FAST_TRANSCENDENTALS` (361a99744). 'Focused measurements' is a log. |
| [lifter/sse-native.md](../lifter/sse-native.md) | 141 | 6 | f09268c8e 2026-11-18 | **keep** | L01 | Short and current. flag-cleanup edits 15 lines of it. |
| [lifter/simd.md](../lifter/simd.md) | 295 | 2 | ef6ce7284 2026-11-16 | **keep** | L01 | Reference for `crates/simd`. Last touched 2026-11-16. |
| [lifter/oracle.md](../lifter/oracle.md) | 254 | 2 | ef6ce7284 2026-11-16 | **fix** | L01 | Current. Takes over ARCHITECTURE.md's 'Lifter checks' section, whose fixture counts say themselves that they predate x87 lowering. |
| [lifter/sse-oracle.md](../lifter/sse-oracle.md) | 418 | 3 | ef6ce7284 2026-11-16 | **keep** | L01 | Reference for `crates/sse-oracle`. |
| [lifter/golden-trace.md](../lifter/golden-trace.md) | 224 | 3 | ef6ce7284 2026-11-16 | **delete** | L01 | Line 3: 'The `oracle::golden` module, the `h2 golden-trace` command and the CLI's `unicorn` feature described here are not on master'. `rg golden crates/oracle crates/cli` finds nothing. Linked from BOOT.md and title-updates.md. |
| [lifter/dispatch-coverage.md](../lifter/dispatch-coverage.md) | 207 | 4 | ef6ce7284 2026-11-16 | **keep** | L01 | A measured result with its reproduction. 'Review regression coverage' is review residue; fold it into 'Reproduce'. |
| [lifter/xdk-libraries.md](../lifter/xdk-libraries.md) | 196 | 2 | ef6ce7284 2026-11-16 | **keep** | L01 | Reference for `h2 symbols`. |
| [lifter/BUILD-CACHE.md](../lifter/BUILD-CACHE.md) | 309 | 14 | 65057505a 2026-11-18 | **keep** | L01 | AGENTS.md cites it. Check its `H2_*` names against the code after flag-cleanup. |
| [lifter/build-perf.md](../lifter/build-perf.md) | 272 | 9 | d0ef387e7 2026-11-17 | **move** | L01 | A dated log from its first line ('Measured on 2026-11-15 ... Base: `a41dae4`'; 'Where clang's time goes (2026-11-16)'; 'Later correctness changes'). To `lifter/notes/build-perf.md`. One code comment cites it. |
| [lifter/cranelift-tier.md](../lifter/cranelift-tier.md) | 322 | 2 | 29ef4a9c8 2026-11-18 | **fix** | L01 | 'Status: design, 2026-11-18, with the first stages built'. Say which stages `crates/clif` has. Was missing from the index. |

### `runtime/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [runtime/BOOT.md](../runtime/BOOT.md) | 442 | 73 | bc48d2053 2026-11-20 | **split** | L02 | 442 lines changed 73 times. 16 `H2_*` names the code stops reading when flag-cleanup lands (`H2_BOOT_DUMP`, `H2_BOOT_FASTMEM`, `H2_BOOT_OLD_GLUE`, `H2_BOOT_TRAMPOLINE`, `H2_REG_HOMES`, ...). Lines 198 to 442, 'Current state', are a logbook: 'Milestones, from the entry point', 'Menu idle benchmark', 'Accessor A/B with process CPU', 'Retired'. |
| `runtime/switches.md` | - | - | flag-cleanup | **keep** | L02 | Arrives with flag-cleanup (68e2ff872). The one list of switches and variables. Its link to render-switches.md needs repointing. |
| [runtime/agent-api.md](../runtime/agent-api.md) | 1953 | 76 | 100f13cef 2026-11-20 | **rewrite** | L03 | 1,953 lines, 76 commits. Title ends '(design)'; three 'what was built' sections and 'Phase 5 ... (plan)'; labels pinned to commit `51a3810c`. AGENTS.md sends every agent to this page. 50 code comments cite it. |
| [runtime/app.md](../runtime/app.md) | 669 | 36 | 42a1c068e 2026-11-18 | **fix** | L02 | Reference for `crates/app`. One removed variable; a 'Demo' and a 'Health meter' section to check against `crates/app` (halo1/health.md says the window's overlay meter is gone). |
| [runtime/building.md](../runtime/building.md) | 140 | 2 | d0ef387e7 2026-11-17 | **keep** | L05 | Guide to the `justfile`. |
| [runtime/cache-slots.md](../runtime/cache-slots.md) | 231 | 9 | f5acfe435 2026-11-20 | **keep** | L02 | Describes one integration test and the converted campaign slots. A heading carries '(**Built**)'. |
| [runtime/checkpoints.md](../runtime/checkpoints.md) | 95 | 5 | f09268c8e 2026-11-18 | **keep** | L02 | Describes one acceptance test. Its only heading, 'Reached kernel/VFS fix', is a log entry. |
| [runtime/debug-menu.md](../runtime/debug-menu.md) | 255 | 5 | 72e30be5c 2026-11-19 | **keep** | L04 | Status says built, not yet seen in the game. |
| [runtime/ios.md](../runtime/ios.md) | 253 | 6 | 858db5c80 2026-11-18 | **keep** | L05 | Guide. Current. |
| [runtime/tvos.md](../runtime/tvos.md) | 170 | 6 | 7ad82ee0a 2026-11-17 | **keep** | L05 | Guide. Current. |
| [runtime/linux.md](../runtime/linux.md) | 167 | 4 | b4526a3bc 2026-11-19 | **keep** | L05 | Reference for the Linux module. Was missing from the index. |
| [runtime/windows.md](../runtime/windows.md) | 130 | 5 | a14da332d 2026-11-17 | **fix** | L05 | Says 'Nothing here has run on Windows' and ends 'Next: the UWP app on Xbox'; xbox.md runs the same module on a Series X. |
| [runtime/xbox.md](../runtime/xbox.md) | 499 | 6 | 279db40be 2026-11-17 | **keep** | L05 | Current. The root README lists it as Design; the README is wrong. |
| [runtime/kernel-imports.md](../runtime/kernel-imports.md) | 341 | 2 | ef6ce7284 2026-11-16 | **fix** | L02 | Calls itself a 'work list for the Rust kernel replacement' (last touched 2026-11-16); the kernel is built. Keep the import table; check 'Hard work and why' against `crates/kernel`. |
| [runtime/lab.md](../runtime/lab.md) | 213 | 2 | 42a1c068e 2026-11-18 | **keep** | L04 | Guide to `h2 lab`. |
| [runtime/level-boot.md](../runtime/level-boot.md) | 162 | 3 | ef6ce7284 2026-11-16 | **keep** | L02 | Short and current. |
| [runtime/level-preflight.md](../runtime/level-preflight.md) | 350 | 7 | 5baaa7dd6 2026-11-18 | **delete** | L02 | A static audit from 2026-11-15 on branch `level-preflight` (baseline `4620ba0`) of what the first campaign levels would need. The levels run. Cited by perf/gameplay-budget.md and `crates/engine/src/points.rs` (3 comments). |
| [runtime/second-executor.md](../runtime/second-executor.md) | 289 | 5 | 42a1c068e 2026-11-18 | **fix** | L02 | Status line names `H2_PIPELINE_RENDER=1`, removed in 9df79b8e3. |
| [runtime/sim.md](../runtime/sim.md) | 1069 | 29 | b0a8f00f1 2026-11-20 | **rewrite** | L04 | Line 3: 'Nothing on this page has been run against the game yet', with 'What needs a run' and 'First runs' sections. perf/sim-budget.md reports measured sims from 2026-11-20 and the root README lists sim as Built. Section 10 is a predicted speed-up. 34 code comments cite it. |
| [runtime/snapshots.md](../runtime/snapshots.md) | 912 | 13 | ec21b685e 2026-11-19 | **rewrite** | L04 | Title ends '(design)'; five 'Phase N: what was built' sections; labels pinned to commit `c84bdd1d`. 68 code comments cite it. |
| [runtime/storage.md](../runtime/storage.md) | 720 | 26 | 1b38334f7 2026-11-19 | **fix** | L02 | Reference for `crates/vfs`. Names `H2_VFS_WRITE_BEHIND`, removed in 8de6ab969. |
| [runtime/title-updates.md](../runtime/title-updates.md) | 765 | 6 | 4b2399cb2 2026-11-19 | **fix** | L02 | Design only, and says so. Cites `crates/boot/src/game_glue.rs`, which flag-cleanup deletes, and links golden-trace.md. |
| [runtime/xapi.md](../runtime/xapi.md) | 885 | 14 | 6dc64161c 2026-11-19 | **fix** | L02 | Cites `crates/kernel/src/files/save_trace.rs` and `H2_SAVE_TRACE`, both removed (50c19e651). 'Kernel API requests' is a request to another lane. 'Campaign input acceptance and scripted runners' was appended after 'References'. |

### `input/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [input/input.md](../input/input.md) | 478 | 18 | 4c679f8b3 2026-11-18 | **fix** | L05 | Reference for `crates/input`. Its last section, 'Host text in the game's virtual keyboard', repeats host-keyboard.md. |
| [input/host-keyboard.md](../input/host-keyboard.md) | 77 | 3 | b3bc676e6 2026-11-18 | **keep** | L05 | 77 lines, current. Three code files cite the path. |
| [input/controller-layouts.md](../input/controller-layouts.md) | 321 | 8 | 4c679f8b3 2026-11-18 | **fix** | L05 | 'Why the rows did not show (fixed 2026-11-15)' and 'Guards after the in-game crash (2026-11-16)' are bug diaries. Names `H2_NO_LAYOUTS` and `H2_NO_SPEAKER_ROW` (removed) and `crates/boot/src/wrappers.rs` (deleted by flag-cleanup). |
| [input/button-icons.md](../input/button-icons.md) | 314 | 6 | d0ef387e7 2026-11-17 | **keep** | L05 | Current. Last touched 2026-11-17. |
| [input/keyboard-charset.md](../input/keyboard-charset.md) | 499 | 3 | ef6ce7284 2026-11-16 | **keep** | L05 | Design, not built, and says so. |
| [input/steam-controller.md](../input/steam-controller.md) | 246 | 4 | 87d3e4b78 2026-11-17 | **fix** | L05 | Names `H2_NO_STEAM_ROWS`, removed by flag-cleanup. |

### `audio/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [audio/audio.md](../audio/audio.md) | 338 | 15 | d01d69139 2026-11-19 | **keep** | L06 | Reference for `crates/audio`. 'Next steps' to check. |
| [audio/dsound.md](../audio/dsound.md) | 1316 | 22 | 986901a67 2026-11-19 | **fix** | L06 | Reference for `crates/dsound`. 'Wanted in `crates/audio`' is a request to another lane; 'Controller-speaker routing' repeats playstation-audio.md. |
| [audio/wma.md](../audio/wma.md) | 313 | 5 | ef6ce7284 2026-11-16 | **fix** | L06 | Ends with a 'Work record' section. |
| [audio/bink-hle.md](../audio/bink-hle.md) | 406 | 7 | 4911ba9be 2026-11-18 | **fix** | L06 | 'Two execution options' keeps the alternative that was not built. One removed variable. |
| [audio/movies.md](../audio/movies.md) | 155 | 7 | 4911ba9be 2026-11-18 | **keep** | L06 | Guide to `h2 movies`. |
| [audio/playstation-audio.md](../audio/playstation-audio.md) | 562 | 15 | 4b2399cb2 2026-11-19 | **fix** | L06 | Names `H2_NO_SPEAKER_ROW` and `H2_NO_LAYOUTS`, both removed by flag-cleanup. Status paragraph carries dates. |

### `editor/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [editor/editor-guide.md](../editor/editor-guide.md) | 1238 | 27 | 89f41807e 2026-11-19 | **fix** | L06 | Line 960 links `../images/editor/hud-masterchief.jpg`, which is not in the repo. `scripts/editor-screenshots.sh` line 296 makes it; the shot was never committed. The coordinator runs the script; a lane cannot. |
| [editor/editor.md](../editor/editor.md) | 1128 | 37 | 89f41807e 2026-11-19 | **keep** | L06 | Developer reference for `crates/studio`. |
| [editor/scenario-editor.md](../editor/scenario-editor.md) | 413 | 12 | 89f41807e 2026-11-19 | **keep** | L06 | Reference for `crates/scenario-edit`. |
| [editor/variants.md](../editor/variants.md) | 260 | 10 | 4b2399cb2 2026-11-19 | **keep** | L06 | Reference for `crates/variants`. |
| [editor/hud-editor.md](../editor/hud-editor.md) | 175 | 3 | 40b48dae5 2026-11-19 | **keep** | L06 | Current. Was missing from the index. |
| [editor/agent-api.md](../editor/agent-api.md) | 363 | 5 | 4fa19475f 2026-11-19 | **keep** | L06 | Current; AGENTS.md cites it. Was missing from the index. |

### `perf/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [perf/README.md](../perf/README.md) | 73 | 4 | 4c67f2a84 2026-11-20 | **fix** | L07 | Says 'The docs here are measurement logs' and sends the reader to the 2026-11-16 review 'for what is planned'. Becomes the area page: the target, the current baseline, which page answers what. |
| [perf/profiling.md](../perf/profiling.md) | 455 | 19 | ce288e758 2026-11-18 | **fix** | L07 | Reference for `h2 profile` and `h2 bench`. 'Native x87 comparison' and 'Device CPU cost' are measurements. |
| [perf/guest-memory.md](../perf/guest-memory.md) | 820 | 17 | ee570b89f 2026-11-18 | **split** | L07 | Nine section headings carry dates (2026-11-15 to 2026-11-18) and two more are experiments ('Synthetic accessor experiment', 'Mapping lookup experiment'). The review lists the page-cache, entry-gate and 4 KiB variants as dead ends. |
| [perf/gameplay-budget.md](../perf/gameplay-budget.md) | 1035 | 26 | 3f36e5a45 2026-11-20 | **move** | L07 | A measurement log: 'Measured 2026-11-15 on master 9a39ed7', 'Re-measured on master 1a5aa8d', 37 commit hashes, 7 removed variables. To `perf/notes/gameplay-budget.md`. 7 code comments and the root README cite it. |
| [perf/menu-budget.md](../perf/menu-budget.md) | 310 | 10 | 42a1c068e 2026-11-18 | **move** | L07 | 'Measured 2026-11-15 on master 4620ba0'; a plan 'from under 2 fps to 30 fps'; a section titled '(landed)'. To `perf/notes/menu-budget.md`. 4 code comments cite it. |
| [perf/architecture-review.md](../perf/architecture-review.md) | 542 | 7 | 42a1c068e 2026-11-18 | **move** | LZ | A dated review (2026-11-16) whose 'Ranked changes' table is still being updated with statuses. To `docs/reviews/2026-11-16-frame-time.md`, frozen. |
| [perf/fastmem-4k.md](../perf/fastmem-4k.md) | 198 | 5 | 42a1c068e 2026-11-18 | **delete** | L07 | 'Status: not on master. The experiment is on the `fastmem-4k` branch (`894f2a3`)'. Not adopted; the review lists it as a dead end. One row in `perf/notes/not-adopted.md`. |
| [perf/fastmem-proto.md](../perf/fastmem-proto.md) | 261 | 3 | ef6ce7284 2026-11-16 | **move** | L07 | Documents `crates/fastmem-proto`, which the review (2.8) says to delete. Cites `docs/fastmem.md` 'on the `perf` branch', which is not on master. To `perf/notes/fastmem-proto.md`; deleted with the crate. |
| [perf/sim-budget.md](../perf/sim-budget.md) | 209 | 5 | d5ab0741c 2026-11-20 | **keep** | - | The current sim baseline (2026-11-20). In-flight design lanes cite it. Not touched. |
| [perf/sim-attribution.md](../perf/sim-attribution.md) | 516 | 1 | 4c67f2a84 2026-11-20 | **keep** | - | Written 2026-11-20. In-flight design lanes cite it. Not touched. |

### `engine/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [engine/README.md](../engine/README.md) | 106 | 10 | 9047d064d 2026-11-20 | **fix** | L08 | The contracts index. One paragraph is a run log ('Recorded on 2026-11-15, seeds 1, 7, 99 ...'). Names the stage 7a proofs that flag-cleanup removes. |
| [engine/engine-map.md](../engine/engine-map.md) | 723 | 15 | 858db5c80 2026-11-18 | **fix** | L08 | The area's start-here page opens with advice from 2026-11-14: 'Keep the main loop, object creation, HaloScript, and physics generated until the menu actually renders.' Four headings carry '(2026-11-15)', '(added ...)' or '(appended ...)'. 34 code comments cite it. |
| [engine/feature-engine.md](../engine/feature-engine.md) | 1735 | 23 | 9ceec03ba 2026-11-20 | **rewrite** | L08 | 1,735 lines. Title: '(design; Phases 0 to 3 built, Phase 4 in part)'; the index calls it 'design and build log'. Labels pinned to commit `2c34d20`. sim-rewrite.md overrules its ranking rows 15 to 18. Names six switches that flag-cleanup removes. Four code comments cite headings it no longer has. |
| [engine/datum_new.md](../engine/datum_new.md) | 184 | 1 | 5ce2974ef 2026-11-15 | **keep** | L08 | A function contract. Last touched 2026-11-15. |
| [engine/tag_get_checked.md](../engine/tag_get_checked.md) | 96 | 3 | 096531115 2026-11-18 | **keep** | L08 | A function contract. |
| [engine/locale_string_id_find.md](../engine/locale_string_id_find.md) | 65 | 2 | ef6ce7284 2026-11-16 | **keep** | L08 | A function contract. |
| [engine/memory_pool.md](../engine/memory_pool.md) | 44 | 1 | 5ce2974ef 2026-11-15 | **keep** | L08 | Contract notes for code that is not in Rust, and says so. |
| [engine/hs_vm.md](../engine/hs_vm.md) | 273 | 2 | ef6ce7284 2026-11-16 | **keep** | L08 | Reference for `hs::vm`. 'Next steps' to check. |
| [engine/havok-runtime.md](../engine/havok-runtime.md) | 375 | 3 | ad5d1a841 2026-11-19 | **keep** | L08 | Retail research. Was missing from the index. |
| [engine/live-telemetry.md](../engine/live-telemetry.md) | 213 | 8 | 1b38334f7 2026-11-19 | **move** | Live owner | Measurements of the Live v8 telemetry stream, filed under engine. To `live/notes/telemetry-precision.md`. Linked from live/live-worker.md and live/live-telemetry.md. |
| [engine/sim-review.md](../engine/sim-review.md) | 697 | 1 | 8a3b731ea 2026-11-20 | **move** | LZ | A dated review (2026-11-20). To `docs/reviews/2026-11-20-sim.md`. |
| [engine/sim-rewrite.md](../engine/sim-rewrite.md) | 655 | 4 | 21ca66709 2026-11-20 | **keep** | - | Plan, 2026-11-20, not built, and says so. In-flight design lanes build on it. Not touched. |
| [engine/sim-rewrite-collision.md](../engine/sim-rewrite-collision.md) | 852 | 1 | 597af450c 2026-11-20 | **keep** | - | Design, 2026-11-20. Not touched. |
| [engine/sim-rewrite-havok.md](../engine/sim-rewrite-havok.md) | 836 | 1 | 2297fa280 2026-11-20 | **keep** | - | Design, 2026-11-20. Not touched. |
| [engine/sim-rewrite-objects.md](../engine/sim-rewrite-objects.md) | 928 | 1 | c0358cc87 2026-11-20 | **keep** | - | Design, 2026-11-20. Not touched. |

### `formats/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [formats/map-format.md](../formats/map-format.md) | 847 | 14 | 3cfb2ee01 2026-11-19 | **fix** | L17 | The area's start-here page. 'Rust tag definitions for an editable engine' and 'Reviewed tag definitions (2026-11-14)' are plans that tag-definitions.md and tag-schema.md overtook. |
| [formats/map-builder.md](../formats/map-builder.md) | 406 | 11 | 86e44844e 2026-11-19 | **keep** | L17 | 'Status: **Built** (2026-11-16)'. Normalise the status line. |
| [formats/tag-schema.md](../formats/tag-schema.md) | 502 | 14 | 51b38ca73 2026-11-20 | **fix** | L17 | Says 'Not yet loaded by the game'; map-builder.md says maps it writes 'load and play in the game'. Check which holds for schema-relocated maps. 'Fresh maps (`h1convert::fresh`)' is Halo 1 converter content. |
| [formats/tag-definitions.md](../formats/tag-definitions.md) | 1658 | 59 | aef1bc526 2026-11-20 | **split** | L17 | 1,658 lines, 59 commits. Ten of its sixteen sections are Halo 1 converter behaviour ('Scenery from definitions (`h1objects::rebuild::scenery`)' through 'Which converted objects use the definitions'), revised with each converter version. They go to a new `halo1/rebuild.md`. |
| [formats/tag-repo.md](../formats/tag-repo.md) | 310 | 1 | dc035981c 2026-11-17 | **keep** | L17 | 'Built (2026-11-17) ... Not yet loaded by the game': check that against the code. |
| [formats/animation.md](../formats/animation.md) | 348 | 7 | 38c06b833 2026-11-20 | **keep** | L17 | Reference. |
| [formats/bitmaps.md](../formats/bitmaps.md) | 38 | 5 | 42a1c068e 2026-11-18 | **keep** | L17 | 38 lines, reference. |
| [formats/collision.md](../formats/collision.md) | 453 | 10 | 372bb6e35 2026-11-19 | **keep** | L17 | Reference. |
| [formats/effects.md](../formats/effects.md) | 564 | 14 | 91c133bae 2026-11-19 | **keep** | L17 | Reference. |
| [formats/fonts.md](../formats/fonts.md) | 204 | 6 | a76b53a11 2026-11-16 | **keep** | L17 | Reference. Last touched 2026-11-16. |
| [formats/haloscript.md](../formats/haloscript.md) | 311 | 7 | a7e93a169 2026-11-20 | **keep** | L17 | Reference. |
| [formats/lighting.md](../formats/lighting.md) | 299 | 6 | 6418f1d95 2026-11-19 | **keep** | L17 | Reference. |
| [formats/localization.md](../formats/localization.md) | 250 | 5 | 858db5c80 2026-11-18 | **keep** | L17 | Reference. |
| [formats/mopp.md](../formats/mopp.md) | 287 | 7 | 8afb6fb15 2026-11-19 | **keep** | L17 | Reference. |
| [formats/objects.md](../formats/objects.md) | 330 | 7 | 7f17bd12f 2026-11-20 | **keep** | L17 | Reference. |
| [formats/pathfinding.md](../formats/pathfinding.md) | 238 | 6 | faaeb74c7 2026-11-19 | **keep** | L17 | Reference. |
| [formats/shader-instances.md](../formats/shader-instances.md) | 372 | 4 | 093d4d662 2026-11-20 | **keep** | L17 | Reference. |
| [formats/sound.md](../formats/sound.md) | 114 | 4 | 858db5c80 2026-11-18 | **keep** | L17 | Reference. |
| [formats/structure-bsp.md](../formats/structure-bsp.md) | 386 | 13 | 5849ea636 2026-11-18 | **keep** | L17 | Reference. Two code comments cite headings it no longer has. |
| [formats/ui.md](../formats/ui.md) | 638 | 9 | 858db5c80 2026-11-18 | **keep** | L17 | Reference. |

### `graphics/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [graphics/graphics.md](../graphics/graphics.md) | 659 | 31 | 4b2399cb2 2026-11-19 | **fix** | L11 | The area's start-here page, a reference for `crates/gfx`. Names `H2_GFX_MERGE_PASSES` and `H2_GFX_SWAP_FINISH`, removed in 41044ef42. |
| [graphics/d3d.md](../graphics/d3d.md) | 1961 | 54 | 7e4529f01 2026-11-18 | **fix** | L11 | 1,961 lines of reference for `crates/d3d`, ordered by topic. Seven removed `H2_D3D_*` variables (33b6dde1e). 'gfx changes this wants' is a request to another lane. |
| [graphics/shaders.md](../graphics/shaders.md) | 973 | 26 | 90a5be1fd 2026-11-19 | **keep** | L11 | Reference for `crates/shaders`. Two code comments cite headings it no longer has. |
| [graphics/render-thread.md](../graphics/render-thread.md) | 408 | 15 | 42a1c068e 2026-11-18 | **split** | L11 | Opens in the past tense ('every part of rendering ran on the guest's runner thread'). 'Prototype', 'Measurements', 'Risks and next steps' and 'The draw path on the guest's thread (2026-11-17)' are a log. Five removed variables. |
| [graphics/render-switches.md](../graphics/render-switches.md) | 183 | 19 | b8b163968 2026-11-20 | **move** | L11 | After flag-cleanup the page is a table of removed switches keyed by commit. That is the not-adopted list: to `graphics/notes/not-adopted.md`. Two code comments and the root README cite it. |
| [graphics/hot-iteration.md](../graphics/hot-iteration.md) | 187 | 3 | 167180dce 2026-11-19 | **keep** | L11 | Current. Normalise the status line. |
| [graphics/occlusion-culling.md](../graphics/occlusion-culling.md) | 367 | 4 | f1f58256d 2026-11-18 | **keep** | L11 | Design with a 'Status' section; the switch waits for a first in-game check. |
| [graphics/native-renderer.md](../graphics/native-renderer.md) | 3792 | 169 | ca047588a 2026-11-20 | **split** | L09 | The worst case: 3,792 lines, 169 commits, 131 code citations. Line 3 says every stage has defaulted on; line 3605 says 'All native switches stay off by default'; nine more lines say a stage is 'off by default'. 29 removed variables and more than 20 per-stage switches that flag-cleanup folds into `impl.render.native`. Stages 1 and 2 each have a '(design)' section and an 'as built' section. |
| [graphics/native-shadows.md](../graphics/native-shadows.md) | 140 | 4 | 923236245 2026-11-20 | **fix** | L10 | '`impl.render.native_shadows` defaults off'; the switch is gone and the stage is on. 'Interface for the light graph' and 'Shared integration files' are lane handoff notes. |
| [graphics/native-gels.md](../graphics/native-gels.md) | 191 | 5 | 8fa87084a 2026-11-19 | **fix** | L10 | '`impl.render.native_gels` is off by default'; same. 'Interface for the lights lane', 'End-of-integration checks' and 'Shared files edited by this lane' are lane handoff notes. |
| [graphics/native-particles.md](../graphics/native-particles.md) | 358 | 5 | b5bd4b9f0 2026-11-20 | **fix** | L10 | Status names two handoff commits and says 'No cargo, build or game run was' made. 'Lane integration record' and 'Diagnostics and coordinator checks' are lane handoff notes. |
| [graphics/native-decals-research.md](../graphics/native-decals-research.md) | 189 | 5 | b5bd4b9f0 2026-11-20 | **fix** | L10 | Named 'research' but has an 'As built' section. The path stays (three code files cite it); the title and content become the native decals page. |
| [graphics/pipelined-render.md](../graphics/pipelined-render.md) | 1538 | 38 | b8b163968 2026-11-20 | **split** | L12 | 1,538 lines. Opens with the current switch, then 'Earlier whole-frame design and measurements', 'Census results (2026-11-18)', 'Census v2 results (2026-11-18)' and 'The prototype'. Names `H2_PIPELINE_RENDER`, removed. |
| [graphics/render-loop.md](../graphics/render-loop.md) | 468 | 8 | ff6f7d6b8 2026-11-18 | **fix** | L12 | Documents the stage 7a ports, which flag-cleanup deletes (ebaa4a0b2) along with the three tests it names. What stays true is the read of the retail call tree. 14 code citations. |
| [graphics/layer-draw.md](../graphics/layer-draw.md) | 300 | 8 | 712df7968 2026-11-19 | **fix** | L12 | Same: 'The port (Code, Test)' describes removed code; 'The read' stays true. 17 code citations. |
| [graphics/high-resolution.md](../graphics/high-resolution.md) | 934 | 19 | 23f8464a7 2026-11-20 | **rewrite** | L13 | Line 2: 'Disabling `graphics.single_copy` restores the original exact-reference design described below.' The body describes the mode that is not the default. 'Test plan', 'Phases' and 'Open questions' are planning. |
| [graphics/e3-lighting.md](../graphics/e3-lighting.md) | 1125 | 15 | af931e2fe 2026-11-19 | **split** | L13 | 1,125 lines. 'What the E3 2003 demo did', 'How it was meant to work', 'Why it was cut', 'What replaced it' repeat pstencil-history.md sections 4 to 6. 'Phase 0 findings', 'Phase 1 as built' and 'Phase 3: shadow prototype' are a phase diary. |
| [graphics/pstencil.md](../graphics/pstencil.md) | 237 | 2 | ba72f6b55 2026-11-16 | **keep** | L13 | Research on a published tag set. Says what it corrects. |
| [graphics/pstencil-history.md](../graphics/pstencil-history.md) | 520 | 1 | ba72f6b55 2026-11-16 | **keep** | L13 | Public-source history of the cut feature. No claims about our code. |

### `game/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [game/settings-menu.md](../game/settings-menu.md) | 1010 | 36 | 4b2399cb2 2026-11-19 | **rewrite** | L14 | The area's start-here page. Its status says the page 'first recommended' an overlay that was never built, and that 'Opening the menu', 'Testing' and 'Phased plan' are 'the original design, kept for the reasoning'. 'How the existing H2_* flags map onto settings' lists six `H2_NO_*` variables that flag-cleanup removes. |
| [game/skulls.md](../game/skulls.md) | 1039 | 27 | 054be60e2 2026-11-19 | **fix** | L14 | Eight removed variables: `H2_NO_SKULLS`, `H2_NO_SKULLS_MENU` (ed3674729) and the `H2_SKULLS_*` layout knobs (f158c8b84). Three code comments cite headings it no longer has. |
| [game/community-gametypes.md](../game/community-gametypes.md) | 263 | 7 | bbf6121b4 2026-11-18 | **fix** | L14 | Says thirteen variants; the root README says twelve. Names `H2_NO_COMMUNITY_VARIANTS`, removed in 3caad61c2. |
| [game/aim-assist.md](../game/aim-assist.md) | 252 | 5 | 3d66f7a27 2026-11-20 | **keep** | L14 | Current. Status carries a date. |
| [game/game-options.md](../game/game-options.md) | 1029 | 11 | b66fdffa3 2026-11-18 | **rewrite** | L14 | Four 'Built: slice N' sections (2, 3, 4, '5 (so far)'), 'Decisions (answered 2026-11-18)' and 'Open questions' after them. Names `H2_NO_VARIANT_OPTIONS`, removed. |
| [game/ui-builder.md](../game/ui-builder.md) | 916 | 19 | 864c391ea 2026-11-19 | **fix** | L15 | 13 dates. 'The modal dialog (slice 6)' and 'The modal dialog in game' are one topic told twice. |
| [game/ui-screens.md](../game/ui-screens.md) | 323 | 18 | 1b38334f7 2026-11-19 | **fix** | L15 | Two removed `H2_NO_*` variables. Five code comments cite headings it no longer has. Links mods-ui-review.md. |
| [game/high-framerate.md](../game/high-framerate.md) | 1482 | 16 | 42a1c068e 2026-11-18 | **split** | L15 | 1,482 lines. Opens with a 'State.' paragraph that walks the phases; sections 6 to 8 are 'What is built (phase 1 ...)', 'Phase 2: what was built (first slice)', 'E1 ... (built, next to D)'. Names `impl.render.scene_passes`, removed. The root README calls the feature Design. |
| [game/superbounce.md](../game/superbounce.md) | 232 | 5 | a9418c935 2026-11-19 | **keep** | L15 | Retail research with traces. Dated 2026-11-18 in its opening. |
| [game/forge.md](../game/forge.md) | 378 | 3 | 858db5c80 2026-11-18 | **keep** | L15 | Design only, and says so. |
| [game/maps.md](../game/maps.md) | 1120 | 36 | 89f41807e 2026-11-19 | **rewrite** | L16 | 1,120 lines, 36 commits. Sections 6 'Plan' and 7 'What is built' sit after the design they revise. 82 code comments cite it by section number. |
| [game/map-pack.md](../game/map-pack.md) | 651 | 10 | 4b2399cb2 2026-11-19 | **fix** | L16 | Section 6 'What's left before the maps can be played' stands beside section 8 'Playing the map pack on 1.0'. Section 7, 'Why the game said the content was damaged', is a bug diary. |
| [game/map-select.md](../game/map-select.md) | 166 | 13 | 772919347 2026-11-19 | **fix** | L16 | 'An earlier Map Pack report' is a bug diary. |
| [game/mods-ui-review.md](../game/mods-ui-review.md) | 65 | 4 | 1aff4f443 2026-11-19 | **delete** | L16 | A review of one branch ('Reviewed branch base `d1fd1f2f`', 'I inspected all seven PNGs'). What still holds belongs under 'Game verification' in map-select.md. Linked from maps.md and ui-screens.md. |
| [game/bots.md](../game/bots.md) | 5701 | 88 | 7705d10ca 2026-11-20 | **split** | Live owner | The largest page: 5,701 lines, 88 commits, 220 code citations. Line 5: 'The retired local-slot design is recorded in §4 and the development history in §10.' Names `H2_BOTS` and `H2_BOTS_SKILL` 20 times; no code reads either. Eight code comments cite § 8.2.2 and other headings it no longer has. |
| [game/theater.md](../game/theater.md) | 890 | 13 | f09cc380c 2026-11-20 | **split** | Live owner | Opens by sending the reader to film-format.md for what is current, then keeps 'the tier-B v1 design and its baseline experiments'. Section 0 is 'The plan (2026-11-19)'. |
| [game/film-format.md](../game/film-format.md) | 731 | 2 | 7ac932200 2026-11-20 | **fix** | Live owner | Current. 'Private machine data and earlier formats' and 'Legacy version 2 input blocks' describe formats it replaced: keep only what the reader still decodes. Was missing from the index. |

### `halo1/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [halo1/README.md](../halo1/README.md) | 48 | 3 | 64fedb648 2026-11-20 | **keep** | L18 | The area page. Two rows added in this branch. |
| [halo1/campaign.md](../halo1/campaign.md) | 1134 | 88 | 8d773f699 2026-11-20 | **rewrite** | L18 | 1,134 lines, 88 commits. 'Status: design, with parts **Built**'. Sections run 1 to 7, then 9, 10, 8. Section 3 is 'Order of work'. |
| [halo1/campaign-picker.md](../halo1/campaign-picker.md) | 497 | 11 | 17bceb8d3 2026-11-20 | **fix** | L18 | '**Built (2026-11-20).**' Ends with 'Coordinator verification', a lane handoff note. |
| [halo1/health.md](../halo1/health.md) | 306 | 9 | c5cd86c6c 2026-11-20 | **fix** | L18 | The status paragraph is a diary: 'Since 2026-11-18 ...', 'Since 2026-11-17 ...'. One code comment cites a 'Layouts' heading it does not have. |
| [halo1/hud.md](../halo1/hud.md) | 790 | 26 | 237749036 2026-11-20 | **fix** | L18 | '**Built.**' and, six lines on, 'Runtime verification of this rebuild is pending'. 'What a game run should show' is the not-verified list. |
| [halo1/multiplayer.md](../halo1/multiplayer.md) | 1778 | 96 | c1262647e 2026-11-20 | **rewrite** | L19 | 1,778 lines, 96 commits. 'Status: in progress, started 2026-11-16'. Eleven headings carry '(**Built** ...)'. 'Still to do, in order'. 'Importing' repeats campaign.md section 10. Six code comments cite headings it no longer has. |
| [halo1/map-format.md](../halo1/map-format.md) | 381 | 10 | 4b2399cb2 2026-11-19 | **keep** | L19 | Reference for `crates/h1map`. |
| [halo1/bitmaps.md](../halo1/bitmaps.md) | 248 | 6 | 858db5c80 2026-11-18 | **keep** | L19 | Reference. |
| [halo1/sounds.md](../halo1/sounds.md) | 550 | 17 | 51b38ca73 2026-11-20 | **keep** | L19 | Reference. |
| [halo1/sky.md](../halo1/sky.md) | 474 | 10 | aba0961ae 2026-11-20 | **keep** | L19 | Reference. A code comment cited it at a path that never existed; fixed in this branch. |
| [halo1/text.md](../halo1/text.md) | 346 | 7 | 1e4bae6d5 2026-11-20 | **keep** | L19 | Reference. |
| [halo1/objects.md](../halo1/objects.md) | 1480 | 64 | 0cc737264 2026-11-20 | **fix** | L20 | 1,480 lines, 64 commits. The status paragraph lists dated milestones. The 880-line section 'Building Halo 2 tags (Design)' sits under a status that says the building is Built: check which. |
| [halo1/models.md](../halo1/models.md) | 930 | 35 | c1262647e 2026-11-20 | **fix** | L20 | Its own label scheme (V1 to V5) carries dates. 'Needs a game run' to check. |
| [halo1/ai.md](../halo1/ai.md) | 940 | 42 | 8d773f699 2026-11-20 | **keep** | L20 | Reference for `crates/h1ai`, 42 commits. |
| [halo1/ai-control.md](../halo1/ai-control.md) | 249 | 30 | 8d773f699 2026-11-20 | **fix** | L20 | First heading is 'Built'. 'Roster dependency and remaining parity work' is a work list. Was missing from the index. |
| [halo1/scripts.md](../halo1/scripts.md) | 630 | 38 | 5736bbf69 2026-11-20 | **keep** | L20 | Reference for `crates/h1scripts`, 38 commits. |
| [halo1/script-builtins.md](../halo1/script-builtins.md) | 191 | 8 | dcd27c26e 2026-11-20 | **keep** | L20 | Current. Was missing from the index. |
| [halo1/role-audit.md](../halo1/role-audit.md) | 454 | 37 | c1262647e 2026-11-20 | **fix** | L20 | Says changes are 'Built in the full-rebuild lane' and that 'the disc tests are written for the coordinator'. |

### `live/`

| Page | Lines | Commits | Last touched | Verdict | Lane | Evidence |
|---|---:|---:|---|---|---|---|
| [live/live.md](../live/live.md) | 1980 | 57 | 844c6381c 2026-11-19 | **split** | Live owner | 1,980 lines, 57 commits. An overview followed by 14 'Client HLE: ...' sections. 'Order of work' and 'Open questions' sit in the middle. |
| [live/live-api.md](../live/live-api.md) | 2050 | 96 | f09cc380c 2026-11-20 | **fix** | Live owner | 2,050 lines, 96 commits. Two sections are both called 'Dedicated hosts' (lines 1238 and 1622), so one anchor cannot be linked. 'Clarifications (2026-11-15)' and 'As built (2026-11-15)' are dated addenda. |
| [live/live-art.md](../live/live-art.md) | 1534 | 37 | 6d840a29b 2026-11-19 | **fix** | Live owner | The section set is there twice: 'Files', 'Pipeline', 'Checking against your disc', 'Limits' at lines 24 to 1075 and again at 1076 to 1534. 17 code comments cite headings that do not resolve. |
| [live/live-coop.md](../live/live-coop.md) | 2576 | 39 | f5acfe435 2026-11-20 | **split** | Live owner | 2,576 lines. 15 numbered sections that go options, recommendation, decisions (2026-11-16), then what was built, slice by slice. 100 code citations by section number. |
| [live/live-motd.md](../live/live-motd.md) | 470 | 7 | 0053d7886 2026-11-16 | **fix** | Live owner | Keeps '2 (superseded). Fetch: game → XONLINE call chain, first reading' beside the corrected section 2. |
| [live/live-worker.md](../live/live-worker.md) | 973 | 24 | 7ac932200 2026-11-20 | **split** | Live owner | 'What changed to make it build (slice 1)', 'Slice 2 results' to 'Slice 5 results' and 'Slices (serial, about 2 hours each)' are a build log. The runbooks stay. |
| [live/live-matchmaking.md](../live/live-matchmaking.md) | 1296 | 27 | 8bb3c1083 2026-11-19 | **fix** | Live owner | Opens with 'Ping-aware host selection (in progress)'. Section 4 is 'Phased plan'. |
| [live/dedicated-host.md](../live/dedicated-host.md) | 923 | 35 | 7ac932200 2026-11-20 | **rewrite** | Live owner | Opens '**Partially feasible.**', as a feasibility study; the root README lists dedicated hosts as Built. 'Order of experiments', 'Mac System Link spike'. Line 292 links `../runtime/sim.md#sim-stores-the-map-without-the-map`; the anchor is `#61-sim-stores-the-map-without-the-map`. Names `H2_FULL_IMAGE`, `H2_SLIM_IMAGE` and `H2_SMALL_IMAGE`, which no code reads. |
| [live/dedicated-host-build.md](../live/dedicated-host-build.md) | 438 | 5 | 93394271d 2026-11-20 | **delete** | Live owner | 'The shared contract for the parallel dedicated-host build (2026-11-19). Twenty lanes write code at once.' A lane contract. Five names no code reads. Two code comments and five pages link it. |
| [live/live-hosting.md](../live/live-hosting.md) | 726 | 18 | 3355ff48f 2026-11-20 | **fix** | Live owner | Names `H2_LIVE_S3_PUBLIC_ENDPOINT`, `_REGION` and `_SECRET_ACCESS_KEY`, which no code reads. |
| [live/replay-fidelity.md](../live/replay-fidelity.md) | 707 | 10 | 1b38334f7 2026-11-19 | **split** | Live owner | Current playback beside 'Original 1 Hz recording (measured on 2026-11-18)', 'Proposal: the spectator track', 'Plan, after review' and 'Decisions (2026-11-18)'. 10 commit hashes. |
| [live/replay-migration.md](../live/replay-migration.md) | 67 | 4 | 1b38334f7 2026-11-19 | **keep** | Live owner | A runbook. Names `H2_LIVE_ADMIN`, which no code reads: check. |
| [live/live-telemetry.md](../live/live-telemetry.md) | 1256 | 40 | 39bc31746 2026-11-20 | **fix** | Live owner | 1,256 lines, 40 commits. Ends with 'Runtime validation needed first', written before the first run. |
| [live/live-voice.md](../live/live-voice.md) | 726 | 12 | 29446b1fc 2026-11-18 | **fix** | Live owner | Design, not built, and says so. Names `H2_NO_VOICE_SCREEN`, removed in f04e09d05. |
| [live/live-custom-games.md](../live/live-custom-games.md) | 440 | 10 | c7419c7cb 2026-11-19 | **fix** | Live owner | Names `H2_NO_CUSTOM_GAMES_ROW`, `H2_NO_MODS_ROW` and `H2_LIVE_MENU_PITCH`, removed in b6c6994b6. |
| [live/live-boundary.md](../live/live-boundary.md) | 422 | 5 | 858db5c80 2026-11-18 | **keep** | Live owner | Static research from 2026-11-15. |
| [live/live-gamestate.md](../live/live-gamestate.md) | 561 | 14 | 8e5061445 2026-11-19 | **keep** | Live owner | Static research. |
| [live/live-security.md](../live/live-security.md) | 311 | 15 | 949103adf 2026-11-19 | **keep** | Live owner | Current. |
| [live/live-cloud-saves.md](../live/live-cloud-saves.md) | 229 | 5 | 6dc64161c 2026-11-19 | **keep** | Live owner | Current. |
| [live/live-admin.md](../live/live-admin.md) | 753 | 21 | 4fa19475f 2026-11-19 | **keep** | Live owner | Current. |
| [live/web-boundary.md](../live/web-boundary.md) | 348 | 6 | 7b5dfc29f 2026-11-18 | **keep** | Live owner | Current. One heading carries a date. |
| [live/dash-concepts.md](../live/dash-concepts.md) | 29 | 5 | 711a7141c 2026-11-18 | **keep** | Live owner | 29 lines, current. |


## Rules for every lane

Each lane is one Sonnet agent in its own worktree, about one to two hours.
Give it this section and its own lane section.

**Read first:** `$RUSH_KEEP/lane-rules-common.md`,
[docs-style.md](docs-style.md), and your lane below.

**You may run:** `rg`, `git` (`log`, `blame`, `grep`, `show`, `mv`, `rm`,
`diff`) and `python3 scripts/docs-check.py`. Nothing else: no cargo, no
tests, no build, no `h2`, no game.

**You may change:** the files your lane lists and the new files it names.
Not `crates/`, not another lane's page, not `docs/README.md`, not
`AGENTS.md`. If a fix belongs in a file you do not own, put it in your
report.

**For each page:**

1. Run `python3 scripts/docs-check.py <page>` and
   `git grep -n 'docs/<area>/<page>.md' -- crates platform scripts web`.
   The second lists the code comments that cite the page and which
   sections they cite. Keep those headings, with their numbers.
2. Do the moves first, each as its own commit with `git mv` or `git rm`
   and nothing else, so history follows the file.
3. Check each claim about our code against the code. Open the file the
   sentence cites; if it cites none, find it with `rg`. If the code
   disagrees, the code wins and the sentence changes. If you cannot find
   the code, do not guess: put the sentence under "Not verified" with
   **S**. Use `git log -S'<name>' -- crates` to learn whether a name was
   removed or never existed.
4. Rewrite to the house style and the template. Present tense. Cite the
   path. Delete history; move dated evidence to the notes page named in
   your lane.
5. A fact about the retail game (an address, an offset, a layout, a call
   tree) is never history. It stays, or it moves to the page where it
   belongs. Do not delete one.
6. A measured number stays only with its date, commit and method, and
   those live in notes. Do not carry a number forward without them. Do not
   produce new numbers.
7. A decision the owner made stays as a plain rule ("Physics stays
   bit-exact with retail"), without the date or the story.
8. Experiments that lost: one row in the area's `notes/not-adopted.md`
   (what, result, commit). If another lane owns that file, put the rows in
   your report.
9. Fix links in your own pages to the paths in "Path changes".

**Done when:**

- `python3 scripts/docs-check.py --strict <your files>` prints no `style`,
  `names` or `paths` finding that your report does not explain, and no
  `links` finding except to a path that "Path changes" says another lane
  creates.
- `git diff --name-only master...` lists only your files.
- No page of yours has a section named for a phase, a slice, a date, "as
  built", "what was built", "next steps", "order of work", a lane or the
  coordinator. `rg -n -i '^#+ .*(phase \d|slice \d|as built|was built|next steps|order of work|lane|coordinator)' <your files>` prints nothing.
- `rg -n -i '\b(previously|no longer|used to|has since|at the time|first version|earlier (run|pass|version|reading))\b' <your design pages>` prints nothing outside quotations.
- Every design page starts with the three template lines (Status, Code,
  Switch).

**Report:** the commits; per page, what you did; every heading you removed
or renamed that a code comment cites, with the comment's `file:line`; every
claim you could not verify; rows for a `not-adopted.md` you do not own;
anything that needs the owner to decide.

## Lanes

Sizes are lines read on master. "Status line" as an action means: make
the top of the page match the template and run the checker; nothing more.

### Wave 1 (parallel, after `flag-cleanup` is on master)

#### L01 `docs-lifter` (15 pages, 4,800 lines, 2 h)

| Page | Do |
|---|---|
| `lifter/LIFTER.md` | Rewrite as the pipeline that exists: `crates/analyze`, `lift`, `ir`, `opt`, `emit`, `clif`, and the handler contract in `crates/emit/RUNTIME-CONTRACT.md` and `crates/runtime/README.md`. Sections 1.1 to 1.3 of the 2026-11-20 review describe it; check each sentence against the code before using it. Move the precedent research (N64Recomp, XenonRecomp and the rest) to `lifter/notes/precedents.md`. Do not describe contract changes that are only planned. |
| `lifter/FLOAT.md` | Move the 2026-11-15 `h2 fpstats` census to `lifter/notes/fp-census.md`. Keep the path as a page of at most 150 lines: which floating-point paths exist (exact x87 helpers, guarded native x87, the transcendental first pass, the SSE reference, native SSE), one paragraph each, linking the page for each. Drop "What to build first". Check each item of "Unknowns" against the code. |
| `lifter/FLOAT-ABI.md` | Replace the status paragraph with the template lines. Delete sections 7 and 8. Keep the numbers of sections 1 to 6. |
| `lifter/x87-native.md` | Move "Benchmark and profile", "Real menu baseline and remaining cost" and "Final menu comparison" to `lifter/notes/x87-native.md`. |
| `lifter/x87-transcendentals.md` | Remove the f64 experiment and `H2_X87_FAST_TRANSCENDENTALS` (check with `rg trans_f64 crates/x87`). Move "Focused measurements" to `lifter/notes/x87-transcendentals.md`. Row in `lifter/notes/not-adopted.md` for the f64 experiment (2658859b9). |
| `lifter/build-perf.md` | `git mv` to `lifter/notes/build-perf.md`. Add the notes header. Row in `not-adopted.md` for anything it measured and rejected (IR-keyed object caching). |
| `lifter/golden-trace.md` | `git rm`. |
| `lifter/cranelift-tier.md` | Template lines. In "Stages", say for each stage whether `crates/clif` has it, from the code. |
| `lifter/oracle.md` | Copy the "Lifter checks" section of `ARCHITECTURE.md` in, as current facts. Its fixture counts go to `lifter/notes/fp-census.md` with their date, or are dropped if the page says they are out of date. |
| `lifter/dispatch-coverage.md` | Fold "Review regression coverage" into "Reproduce". |
| `sse-native.md`, `simd.md`, `sse-oracle.md`, `xdk-libraries.md`, `BUILD-CACHE.md` | Status line. |

Check with: `rg -n 'pub fn|pub struct' crates/emit/src/lib.rs crates/lift/src/lib.rs crates/opt/src/lib.rs`; `rg -n 'H2_[A-Z0-9_]+' crates/emit/src crates/x87/src -o --no-filename | sort -u` against the variables the pages name.

#### L02 `docs-runtime-boot` (12 pages, 5,000 lines, 2 h)

| Page | Do |
|---|---|
| `runtime/BOOT.md` | Split. The page keeps: what `h2 build` and `h2 run` do, how boot fits together (`crates/boot/src/lib.rs`, `registration.rs`), hosted runs, and the stopgaps in force, each with its code path. Flags and variables: name the few a reader needs and link `switches.md` for the rest; delete the table rows for names `scripts/docs-check.py` reports. "Current state" from "Milestones, from the entry point" to "Retired" moves to `runtime/notes/boot-log.md`. |
| `runtime/switches.md` | Remove the opening sentence about the cleanup's date. Repoint "Removed switches, and why" to `../graphics/notes/not-adopted.md`. Check every row with `rg -n '"<NAME>"' crates`. |
| `runtime/level-preflight.md` | `git rm`. Report the three comments in `crates/engine/src/points.rs` that cite it. |
| `runtime/xapi.md` | Remove `H2_SAVE_TRACE` and `save_trace.rs`. Delete "Kernel API requests" (put any request that is still open in your report). Move "Campaign input acceptance and scripted runners" above "References". |
| `runtime/kernel-imports.md` | Retitle as the import inventory. Check "Hard work and why" and "Unverified details" against `crates/kernel/src`; keep what is still open as "Not verified". |
| `runtime/app.md` | Check "Demo" and "Health meter" against `crates/app/src`; delete what is gone. |
| `runtime/title-updates.md` | Replace the citation of `crates/boot/src/game_glue.rs` with where that code went (`git log -S` on a symbol it names). Remove the link to golden-trace.md. |
| `runtime/second-executor.md`, `storage.md` | Remove the variables the checker reports. Status line. |
| `runtime/checkpoints.md`, `cache-slots.md`, `level-boot.md` | Status line. Fold "Reached kernel/VFS fix" into the test's description. |

#### L03 `docs-agent-api` (1 page, 1,953 lines, 2 h)

`runtime/agent-api.md`. Rewrite as a reference. The method list is in the
code: the registry behind `session.methods` (`rg -n 'session.methods'
crates/agent crates/agent-proto`). One section per domain (`session`,
`ui`, `world`, `havok`, `profile`, `render`, `state`, `input`, `sim`,
`match`, `host`, `theater`, `edit`, `trace`, `flight` and the rest the
registry has), each method with params, result
and one example. Keep the section titles that code cites
(`git grep -n 'docs/runtime/agent-api.md' -- crates` lists 50 comments).
Delete "Why", "Principles", "Phases", the three "what was built" sections,
"Pitfalls from the Live work" and "Open decisions"; a decision that is
still open goes under "Not verified". Do not invent a method: every
method in the page must be found by `rg -n '"<domain>.<method>"' crates`.
List in your report the methods the registry has that the page lacked.
The `sim` and "Theater transport" sections belong to the session that owns
bots and theater: rewrite their form, do not change what they claim, and
say so in your report.

#### L04 `docs-snapshots-sim` (4 pages, 2,450 lines, 2 h)

| Page | Do |
|---|---|
| `runtime/snapshots.md` | Rewrite. Drop "(design)" from the title. Fold the five "Phase N: what was built" sections into the sections they describe ("When a snapshot can be taken", "Inventory", "How a load works", "File format", "Where snapshots live"). Replace labels pinned to `c84bdd1d` with paths in `crates/snapshot` and `crates/runtime`. 68 code comments cite the page: keep the cited headings. "Risks and unknowns" becomes "Not verified". Dated results to `runtime/notes/snapshots.md`. |
| `runtime/sim.md` | Rewrite. Remove "Nothing on this page has been run", "What needs a run" and "First runs"; `perf/sim-budget.md` has the measured runs. Section 10's predicted speed-up moves to `runtime/notes/sim.md` beside a link to the measured one. Keep the section numbers that the 34 citing comments use. Do not change what the bot sections claim. |
| `runtime/debug-menu.md`, `lab.md` | Status line. "To check in the game" and "Not verified without a second Mac" become "Not verified". |

#### L05 `docs-platforms-input` (12 pages, 3,300 lines, 1.5 h)

| Page | Do |
|---|---|
| `runtime/windows.md` | Say what runs today: the module is built for Windows and runs inside the UWP app on a Series X (xbox.md). Delete "Next: the UWP app on Xbox". |
| `input/input.md` | Replace "Host text in the game's virtual keyboard" with one sentence linking host-keyboard.md. |
| `input/controller-layouts.md` | Fold "Why the rows did not show" and "Guards after the in-game crash" into the description of what `layout_glue.rs` does and guards against; drop the story. Remove `H2_NO_LAYOUTS` and `H2_NO_SPEAKER_ROW`. Replace `crates/boot/src/wrappers.rs` with where the code is. |
| `input/steam-controller.md` | Remove `H2_NO_STEAM_ROWS`. "To try on hardware" and "Unverified" become "Not verified". |
| `runtime/building.md`, `ios.md`, `tvos.md`, `linux.md`, `xbox.md`, `input/host-keyboard.md`, `button-icons.md`, `keyboard-charset.md` | Status line. |

#### L06 `docs-audio-editor` (12 pages, 6,700 lines, mostly read, 1.5 h)

| Page | Do |
|---|---|
| `audio/dsound.md` | Delete "Wanted in `crates/audio`" (open requests go in your report). Replace "Controller-speaker routing" with a sentence linking playstation-audio.md. |
| `audio/wma.md` | Delete "Work record". |
| `audio/bink-hle.md` | "Two execution options": describe the one that is built. "Not yet proven in a running game" becomes "Not verified". |
| `audio/playstation-audio.md` | Remove the removed variables. Template lines in place of the dated status. |
| `editor/editor-guide.md` | Do not touch the image link at line 960. Report it. |
| `audio/audio.md`, `movies.md`, `editor/editor.md`, `scenario-editor.md`, `variants.md`, `hud-editor.md`, `agent-api.md` | Status line. For `editor/agent-api.md`, check each method against `rg -n '"<name>"' crates/studio-proto crates/studio`. |

#### L07 `docs-perf` (7 pages, 3,150 lines, 1.5 h)

| Page | Do |
|---|---|
| `perf/gameplay-budget.md`, `menu-budget.md`, `fastmem-proto.md` | `git mv` to `perf/notes/`. Add the notes header. No other edit. |
| `perf/fastmem-4k.md` | `git rm`. Row in `perf/notes/not-adopted.md`: 4 KiB translation, slower than the inline path, branch `fastmem-4k` at `894f2a3`. |
| `perf/guest-memory.md` | Split. The page keeps how generated code reaches guest memory today: the inline RAM path, the gate, direct calls, the exit call, register homes, runner transfers, each with its path in `crates/emit` and `crates/runtime` and its switch linked to `switches.md`. Every dated section and both experiments move, whole, to `perf/notes/guest-memory.md`. Rows in `not-adopted.md` for what lost. |
| `perf/profiling.md` | Move "Native x87 comparison" and "Device CPU cost" to `perf/notes/guest-memory.md` or a note of their own. |
| `perf/README.md` | The area page: the target, the current baseline with links (`sim-budget.md`, `sim-attribution.md`, the notes), "which page answers what", and the startup path section as it is. Remove "The docs here are measurement logs". Point "what is planned" at `../reviews/2026-11-20-architecture.md`. |

#### L08 `docs-engine` (9 pages, 3,600 lines, 2 h)

| Page | Do |
|---|---|
| `engine/feature-engine.md` | Rewrite as what `crates/feature` is: named points (`crates/engine/src/points.rs`), entries and hook chains (`entry.rs`, `install.rs`, `plan.rs`), switches (`switches.rs`), the bridges (`bridge.rs`), leaves (`leaves.rs`), settings (`crates/settings`), and registration in `crates/boot/src/registration.rs`. Take the module list from `ls crates/feature/src`, not from the old text. Delete "Phases and effort", "Migration", "Subsystem map and ranking" (sim-rewrite.md replaces it) and "The top candidates". Remove the stage 7a ports. Keep the headings code cites; four cited headings are already gone ("UI step b", "UI step c", "Hook ordering", and one more the checker lists): restore the content under those titles if it is still true. Dated material to `engine/notes/feature-engine.md`. |
| `engine/engine-map.md` | Replace the opening advice with one paragraph saying what the page is: a map of the retail engine by subsystem, with addresses. Remove the dates and "(added)", "(appended)" from the four headings; keep the titles otherwise, because formats pages and code cite them. Check "Size, coupling, and rewrite priority" against sim-rewrite.md and delete what it overrules. |
| `engine/README.md` | Move the "Recorded on 2026-11-15" run log to `engine/notes/feature-engine.md` or drop it. Check the names of the differential tests against `crates/engine/tests`. |
| `engine/hs_vm.md` | "Next steps" becomes "Not verified" or goes. Check "not yet wired into the game" with `rg -n 'hs::vm' crates/boot crates/feature`. |
| `datum_new.md`, `tag_get_checked.md`, `locale_string_id_find.md`, `memory_pool.md`, `havok-runtime.md` | Status line. Keep every address. |

#### L09 `docs-native-renderer` (1 page, 3,600 lines, 2 h)

`graphics/native-renderer.md`.

1. First commit, alone, and push it so L10 can start: cut the page by its
   top-level sections, text unchanged, into `native-world.md` ("Stage 1"
   and "Stage 1 as built"), `native-objects.md` ("Stage 2" and "Stage 2 as
   built"), `native-transparents.md` ("Stage 3"), `native-lights.md`
   ("Stage 4"), `native-water-fog.md` ("Stage 4b") and
   `native-post-hud.md` ("Stage 5"). The rest stays.
2. Move "The all-native push" to `graphics/notes/native-ab.md`, unchanged.
3. Rewrite `native-renderer.md` to at most 400 lines: what the native
   renderer is, the frame as it flows now (scene lists, walks, admission
   through `d3d::DrawAdmission`, what falls back to D3D and when), a table
   of stages (stage, page, modules in `crates/native-render/src` and
   `crates/feature/src/native_*`, what it replaces), the one switch
   `impl.render.native`, how a frame is checked. Keep a "Stages" list that
   uses the old section names, so a reader arriving from one of the 131
   code citations finds the page.
4. Rewrite `native-world.md` and `native-objects.md`: merge each "(design)"
   section with its "as built" section into one description of what the
   code does. "The live A/B" sections go to the note. Research points keep
   their addresses.
5. Do not describe the planned frame-scene design.

Check with: `rg -n 'impl\.render\.' crates -o --no-filename | sort -u` (the
switches that exist); `ls crates/native-render/src crates/feature/src |
rg native`.

#### L10 `docs-native-stages` (8 pages, 2,400 lines, 2 h; starts from L09's first commit)

| Page | Do |
|---|---|
| `native-transparents.md`, `native-lights.md`, `native-water-fog.md`, `native-post-hud.md` | Rewrite each from the cut text: one description of what the stage does now, its modules, what it refuses. "Order of work" and "Stage 5 status" go. "(off by default)" goes: the stages are on under `impl.render.native`. |
| `native-shadows.md`, `native-gels.md`, `native-particles.md` | Same. Delete "Interface for the lights lane", "Shared files edited by this lane", "Shared integration files", "End-of-integration checks", "Lane integration record", "Diagnostics and coordinator checks"; an interface another stage really uses becomes "Interfaces". |
| `native-decals-research.md` | Keep the path. Retitle "Native decals". Merge "As built" into the description. "Comparison and remaining runtime evidence" becomes "Not verified". |

#### L11 `docs-graphics-core` (7 pages, 4,700 lines, 2 h)

| Page | Do |
|---|---|
| `graphics/render-switches.md` | `git mv` to `graphics/notes/not-adopted.md`. Keep the "Removed" table; drop the rest. Add the rows other lanes report later (LZ does that). |
| `graphics/render-thread.md` | Split. The page keeps: what runs on the render thread, what the guest can observe, how exactness is kept, with paths in `crates/d3d` and `crates/gfx`. "Prototype", "Measurements", "Risks and next steps" and the dated section move to `graphics/notes/render-thread.md`. Present tense throughout. |
| `graphics/graphics.md`, `d3d.md` | Remove the variables the checker reports, and the sentences that only explain them. In `d3d.md` delete "gfx changes this wants" (open requests in your report). "Not supported yet (future work)" becomes "Rules and limits". |
| `graphics/shaders.md` | Two code comments cite headings that are gone (the checker lists them): restore the titles or report. |
| `hot-iteration.md`, `occlusion-culling.md` | Status line. |

#### L12 `docs-graphics-frontend` (3 pages, 2,300 lines, 1.5 h)

| Page | Do |
|---|---|
| `graphics/pipelined-render.md` | Split. The page keeps the design that the switch `impl.render.pipelined_frame` turns on: tick ordering and hash parity, snapshot coverage, the worker, with paths. "Earlier whole-frame design and measurements", both census results and "The prototype" move to `graphics/notes/pipelined-render.md`. |
| `graphics/render-loop.md` | Keep "The call tree" as the read of the retail front-end. Delete "What is built", "Evidence", "Checking it in game", "Next port targets" and "Later stages" where the code is gone (`rg -n 'render_views|scene_passes' crates/engine/src crates/feature/src`). "Where the scene passes' time goes" moves to `graphics/notes/scene-pass-timing.md`. Report a not-adopted row for the stage 7a ports (ebaa4a0b2). |
| `graphics/layer-draw.md` | Keep "The read" and "Shader draw begin". "The port" stays only for code that `rg -n 'layer_draw|layer_entries' crates` still finds. The `render-timing` breakdown moves to the same note. |

#### L13 `docs-lighting-resolution` (4 pages, 2,800 lines, 1.5 h)

| Page | Do |
|---|---|
| `graphics/high-resolution.md` | Rewrite around the default: one copy at the render scale. The exact-reference mode becomes one section saying what `graphics.single_copy` off does. "Test plan" becomes the tests that exist, by name. Delete "Phases". "Open questions" becomes "Not verified". |
| `graphics/e3-lighting.md` | Split. The page keeps the Lighting setting as built and the design not yet built, with a Status line that says which. Move into `pstencil-history.md` anything in "What the E3 2003 demo did" to "What survives" that it lacks, then delete those sections here. "Phase 0 findings" is retail research: keep it under a plain title. "Phase 1 as built" merges into the description. "Phase 3: shadow prototype" moves to `graphics/notes/e3-lighting.md`. |
| `graphics/pstencil-history.md` | Receives the above. No other change. |
| `graphics/pstencil.md` | Status line. |

#### L14 `docs-game-settings` (5 pages, 3,600 lines, 2 h)

| Page | Do |
|---|---|
| `game/settings-menu.md` | Rewrite as what is built: the Video, Controls, Voice and pause screens, FOV, the settings schema (`crates/settings`, `crates/boot/src/setting_defs.rs`), `h2 settings`. Delete "Opening the menu", "Testing" and "Phased plan" (the overlay that was not built). Replace "How the existing H2_* flags map onto settings" with a link to `switches.md` and the rule that each setting has its variable in `setting_defs.rs`. Retail research ("How Halo 2's own settings screens work", "Where settings persist") stays. 24 code citations: keep cited headings. |
| `game/game-options.md` | Rewrite. Fold the four "Built: slice N" sections into "The options" and "The extension". "Decisions (answered 2026-11-18)" becomes plain rules where they apply. "To check in the game" and "Open questions" become "Not verified". 38 code citations. |
| `game/skulls.md` | Remove the eight removed variables and the layout-knob text. Three cited headings are gone ("Calls into the game", "Layout", one more): restore the titles if the content is there. |
| `game/community-gametypes.md` | Count the variants in the code (`rg -n 'community' crates/boot/src -l`) and state the number. Remove `H2_NO_COMMUNITY_VARIANTS`. Report the number for the root README. |
| `game/aim-assist.md` | Status line. |

#### L15 `docs-game-ui-framerate` (5 pages, 3,300 lines, 2 h)

| Page | Do |
|---|---|
| `game/high-framerate.md` | Split. The page keeps: how the tick, the frame and the vblank are coupled (retail research), and the design as it is in the code: `H2_FPS`, reprojection, the interpolated replay, the determinism harness (`crates/lockstep`), each with its switch linked to `switches.md`. Merge sections 6 to 8 into that. "Prior art" and measurements move to `game/notes/high-framerate.md`. Keep the section numbers the 28 citing comments use. |
| `game/ui-builder.md` | Merge "The modal dialog (slice 6)" and "The modal dialog in game" into one section. Remove dates. "What stays unverifiable offline" becomes "Not verified". 38 code citations. |
| `game/ui-screens.md` | Remove the removed variables and the link to mods-ui-review.md. "Pitfalls we hit, and why" becomes "Rules and limits", as rules. Five cited headings are gone: restore or report. |
| `game/superbounce.md`, `forge.md` | Status line. |

#### L16 `docs-game-maps` (4 pages, 2,000 lines, 1.5 h)

| Page | Do |
|---|---|
| `game/maps.md` | Rewrite. Fold section 6 "Plan" and section 7 "What is built" into sections 1 to 5 and 8, which keep their numbers and titles (82 code comments cite them). Report every comment that cites §6 or §7. |
| `game/map-pack.md` | Sections 6 and 8 become one description of how the map pack plays on 1.0. Section 7 becomes one paragraph on what the game checks. Keep cited numbers; report the rest. |
| `game/map-select.md` | Delete "An earlier Map Pack report". Move what still holds from mods-ui-review.md under "Game verification", as open items. |
| `game/mods-ui-review.md` | `git rm`. |

#### L17 `docs-formats` (20 pages; 3,700 lines of work, 2 h)

| Page | Do |
|---|---|
| `formats/tag-definitions.md` | Split. `git mv` is not possible for part of a file: create `halo1/rebuild.md` with the ten sections from "Scenery from definitions" to "Which converted objects use the definitions", text unchanged, in the first commit. The page keeps "Rules", "The files", "The API", "Proof", "Where the names come from" and "Open", with one paragraph linking `../halo1/rebuild.md`. Report the code comments that cite a moved section. |
| `formats/map-format.md` | Delete "Rust tag definitions for an editable engine" and "Reviewed tag definitions (2026-11-14)" where tag-definitions.md and tag-schema.md cover them; link those pages. |
| `formats/tag-schema.md`, `tag-repo.md`, `map-builder.md` | Settle "Not yet loaded by the game" against the code and the Halo 1 pages (`rg -n 'schema::|tags::repo' crates/h1convert crates/h1campaign crates/cli/src`). Template lines without dates. |
| The other 15 | Status line. Each keeps its evidence labels and every layout. |

### Wave 2 (after the `h1-*` code lanes merge)

#### L18 `docs-halo1-game` (5 pages, 2,800 lines, 2 h)

| Page | Do |
|---|---|
| `halo1/campaign.md` | Rewrite. Number the sections in order without changing the numbers code cites (33 comments; check first). "Order of work" goes. "Decisions" become rules. The status paragraph becomes the template lines plus a table: level, converts, what is not verified. |
| `halo1/campaign-picker.md` | Delete "Coordinator verification"; open items to "Not verified". |
| `halo1/health.md`, `hud.md` | Template lines in place of the dated status. `health.md`: a comment cites a "Layouts" heading; restore or report. |
| `halo1/README.md` | Add `rebuild.md`. |

#### L19 `docs-halo1-multiplayer` (6 pages, 3,800 lines, 2 h)

| Page | Do |
|---|---|
| `halo1/multiplayer.md` | Rewrite. Remove "(**Built** ...)" from the eleven headings, keeping the rest of each title (42 code comments cite them; six already miss). "Still to do, in order" becomes "Not verified" and "Rules and limits". "Importing" becomes one sentence linking campaign.md's section. |
| `map-format.md`, `bitmaps.md`, `sounds.md`, `sky.md`, `text.md` | Status line. |

#### L20 `docs-halo1-convert` (7 pages, 4,900 lines, 2 h)

| Page | Do |
|---|---|
| `halo1/objects.md` | Template lines in place of the dated status. Settle "Building Halo 2 tags (Design)": if `crates/h1objects` does it, the heading loses "(Design)" and the text is checked against the code; link `rebuild.md` for the rebuild paths. |
| `halo1/ai-control.md` | Rename the "Built" heading to what it describes. "Roster dependency and remaining parity work" becomes "Not verified". |
| `halo1/role-audit.md` | Remove the lane and coordinator sentences; say which test checks each rule. |
| `halo1/models.md` | Keep the V1 to V5 scheme; move the dates out of the label definitions into `Evidence`. |
| `ai.md`, `scripts.md`, `script-builtins.md` | Status line. |

### Last

#### LZ `docs-index-links` (alone, after every other lane has merged, 2 h)

1. `git mv` the three reviews into `docs/reviews/` under the names in
   "Path changes". Add one line under each title: the date, "frozen".
2. `docs/ARCHITECTURE.md`: rewrite. Goal, targets, the pipeline in one
   paragraph with links, and the crate table: every directory in `crates/`,
   one row, purpose from the `description` in its `Cargo.toml` where there
   is one, and the page that documents it. No status column: status is on
   the pages. Delete "Lifter checks" (L01 moved it) and "Milestones". If
   the `crate-map` lane has landed a table or a generator, use it and do
   not write a second one.
3. Root `README.md`: bots are phantom peers, not "on free controller
   ports"; the Xbox app's state from `runtime/xbox.md`; the frame-rate
   feature's state from `game/high-framerate.md`; the variant count L14
   reports. Repoint links to moved pages.
4. `docs/README.md`: rebuild. Per area: the start-here page, then design,
   reference and guide pages, then one "Notes:" line. A "Reviews" section.
   One line per page saying what it answers.
5. Append the `not-adopted.md` rows the lanes reported.
6. Run `python3 scripts/docs-check.py`. Fix every `links`, `index` and
   `code` finding. A `code` finding is a comment citing a moved page: fix
   the path in the comment, one line, no other change, in any crate except
   `coop`, `live-hle`, `live-server`, `bots` and `boot/src/coop_live*`;
   list those for the Live owner.
7. `code-sections`: fix the comment where the lanes' reports give the new
   heading; list the rest.
8. Delete this page and its index line. In `docs-style.md`, remove the
   two links to it and the two sentences that say something is pending
   (the `AGENTS.md` section, the arrival of `switches.md`).

Done when `python3 scripts/docs-check.py` exits 0 and its `code-sections`
count is below the 70 it started at.

## For the Live owner

26 pages: everything under `live/`, plus `game/bots.md`, `game/theater.md`,
`game/film-format.md` and `engine/live-telemetry.md`. Verdicts are in the
tables. A split that keeps files disjoint:

| Lane | Pages | Main work |
|---|---|---|
| `docs-bots` | `game/bots.md` | Split: phantom path as the design; §4 (local slots) and §10 (stages) deleted; traces to `game/notes/bots-traces.md`. 220 code citations by section: keep the numbers. |
| `docs-live-core` | `live.md`, `live-api.md`, `live-security.md`, `live-admin.md`, `live-cloud-saves.md` | `live.md`: overview stays, the "Client HLE" sections become `live-client-hle.md`. `live-api.md`: one "Dedicated hosts" section; fold the two dated addenda. |
| `docs-live-coop` | `live-coop.md` | Split: sections 10 to 15 are the design; options and the probe go to notes. 100 code citations. |
| `docs-live-hosting` | `dedicated-host.md`, `dedicated-host-build.md`, `live-worker.md`, `live-hosting.md` | Rewrite `dedicated-host.md` as what runs; delete the build contract; `live-worker.md` keeps settings and runbooks. Fix the anchor at `dedicated-host.md` line 292. |
| `docs-live-replay` | `replay-fidelity.md`, `replay-migration.md`, `live-telemetry.md`, `engine/live-telemetry.md`, `live-art.md`, `game/theater.md`, `game/film-format.md` | Remove the duplicated half of `live-art.md`; move the telemetry measurements under `live/notes/`; `theater.md` keeps the design that film-format.md does not cover. |
| `docs-live-game` | `live-matchmaking.md`, `live-custom-games.md`, `live-voice.md`, `live-motd.md`, `live-boundary.md`, `live-gamestate.md`, `web-boundary.md`, `dash-concepts.md` | Remove the removed variables; delete the superseded section of `live-motd.md`. |

## Proposed text for AGENTS.md

Not applied. Add as a section after "Use the agent API".

```markdown
## Keep the docs current

Docs state the current design. Git has the history. When your change
alters behaviour, an interface, a switch, an environment variable or a
path that a doc names, update that doc in the same branch.

- Find the docs: `git grep -n '<symbol, switch or path>' -- docs`, and the
  `docs/...` paths in the comments of the code you changed.
- Edit the page so it reads as if it had always been this way: present
  tense, the code path cited, no "now", "previously" or dates, no
  changelog, no "as built" section. Delete what is no longer true.
- Measurements, experiment results and research records go in
  `docs/<area>/notes/`, dated, with the commit and how to reproduce them.
  A design page links to the note for its numbers. An experiment that
  lost gets a row in the area's `notes/not-adopted.md`.
- Don't rename or move a doc, or a heading that code comments cite. For a
  new page, copy the template in `docs/meta/docs-style.md` and add one
  line to `docs/README.md`.
- Switches and environment variables are listed once, in
  `docs/runtime/switches.md`.
- A lane's contract, handoff or checklist for the coordinator goes in
  your report, not in `docs/`.
- Run `python3 scripts/docs-check.py <the docs you touched>`. Clear what
  it prints, or say in your report why a finding stands.
```

## To decide

1. Merge `flag-cleanup` before any lane starts (assumed above).
2. No archive folder: superseded pages are deleted and git keeps them
   (assumed above). The alternative is `docs/archive/`, which keeps stale
   text where `rg` finds it.
3. May LZ change code comments, one line each, to repoint doc citations?
   Without it, about 20 comments will cite moved pages, and more will
   cite sections that moved to a stage page.
4. Who owns the crate table in `ARCHITECTURE.md`: LZ or the `crate-map`
   lane.
5. Delete `crates/fastmem-proto` (review 2.8)? Then its page is deleted
   rather than moved to notes.
6. `editor-guide.md` needs `hud-masterchief.jpg`: run
   `scripts/editor-screenshots.sh` and commit the shot to LFS.
7. Add `scripts/docs-check.py` to `scripts/check.sh` and CI once it
   exits 0.
