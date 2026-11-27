# App: following a branch and watching the checkout

Status: Built. `h2 play --follow` and `h2 play --watch`. Part of [App: the host shell](app.md).

## Following a branch: `h2 play --follow`

```sh
h2 play --follow /abs/path/to/disc/halo2.iso
```

Plays as `h2 play` does, and meanwhile follows `origin/master`: when a new
commit lands it builds it in the background, then a banner offers the
update. F5 restarts into it; F6 hides the banner. Closing the window ends
it. Everything after the follow flags is passed to `h2 play` as is.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--repo PATH` | the checkout containing the current directory | Repository to follow. |
| `--branch B`, `--remote R` | `master`, `origin` | Branch to watch. |
| `--no-fetch` | off | Watch the local `refs/heads/B` instead of fetching. |
| `--poll SECS` | 45 (minimum 5) | How often to fetch or look. |
| `--fresh-data` | off | Each launch gets an empty data dir under the cache (otherwise the user's `--data`). |
| `--cache DIR` | `~/Library/Caches/h2recomp/follow` | Where everything below lives. |

How it works (crates/apps/cli `follow.rs`):

- **Supervisor.** The parent process runs the game as a child (`h2 play`
  with the same arguments, plus `--out`, `--data` and the hidden
  `--follow-status FILE`). The first child is the `h2` you started, and the
  branch's head at startup is taken to be what it runs, so the first
  update is the next commit. Every process the supervisor starts (the game,
  build steps, git) runs in its own process group with SIGINT and SIGTERM
  unblocked (the supervisor blocks them for its signal thread, and a
  child would inherit that). A Ctrl-C in the terminal reaches only the
  supervisor, which asks the game to stop with one SIGTERM, cancels any
  build, and exits with the game's code. A second Ctrl-C sends the game's
  group a second SIGTERM (`h2 play` then exits at once) and SIGKILL after
  15 s. Nothing it started outlives it: the build's group gets SIGKILL 3 s
  after its SIGTERM, and both are reaped before the supervisor exits and
  drops its lock. Whenever a group's leader exits, normally or not, the
  rest of its group is killed before the leader is reaped. The watcher's
  poll sleep wakes on shutdown, and its thread (and the startup read's,
  after an interrupt) is joined, with any fetch stopped and reaped, before
  the supervisor exits.
- **Watching.** Every `--poll` seconds: `git fetch <remote> <branch>`, which
  only moves the remote-tracking ref, then `rev-parse`. With `--no-fetch` it
  reads the local branch. A failed fetch, or one with no answer in 2
  minutes (prompts are off: `GIT_TERMINAL_PROMPT=0`), is logged and the
  last fetched ref is used. The startup read runs on a thread too, so a
  Ctrl-C during a hanging first fetch stops it and exits at once.
- **Building.** One commit at a time. Heads that arrive mid-build queue, and
  the latest one wins. A commit that failed is not retried until the head
  moves again. If the branch rolls back (a force-push) to the running or the
  ready build, the queue is dropped, the build in flight is discarded when
  it finishes, and rolling back to the running build withdraws the offer.
  A build the branch merely moved forward from is still offered while the
  newer one builds. Each build runs in a detached worktree the supervisor
  owns (`<cache>/checkout`, added with `git worktree add --force --detach`,
  which only re-adds that path's own entry if its directory went missing;
  there is no global `worktree prune`). Before reusing it (`checkout
  --force`, then `clean`) it must be a real directory, not a symlink, at
  exactly `<cache>/checkout`, the top of its work tree, registered in the
  repository's `git worktree list` at that path as a linked worktree (not
  the main checkout), with the repository's git dir, and a `.git` file
  selecting that worktree's own private git dir (`<common>/worktrees/<name>`,
  whose `gitdir` names this `.git`), never the main git dir or another
  worktree's. Anything else there is refused and left alone. It runs `nice -n 15 cargo build
  --release -p cli` with `CARGO_TARGET_DIR=<cache>/target`, and
  `CARGO_BUILD_JOBS` set to half the cores unless you have set it.
  A rustc wrapper (`RUSTC_WRAPPER`, such as kache or sccache) and the rest of your environment pass through.
  The new `h2` decides whether the newest generated game is still its own:
  `h2 build <disc> --out <previous> --check` compares the dylib's `.stamp`
  (image hash, generator sources hash including the boot bridge, module
  ABI: `boot::build_stamp`) with its own. If not (or the `h2` is too old to
  know `--check`), the game is generated again with `nice h2 build <disc>
  --out <cache>/gen/<hash>`. The first previous game is the first child's
  `--out`. Objects go to `H2_OBJ_CACHE`, which defaults to
  `<cache>/objcache` and persists, so an emit only compiles C that changed.
  The finished `h2` is copied to `<cache>/builds/<hash>/`. Builds and gen
  dirs no longer in use are pruned before each build.
- **Banner.** The supervisor writes `<cache>/status` (one line,
  `info|ready|error<TAB>text`, replaced atomically). The child reads it
  twice a second into an `app::Banner`, which the shell draws in the
  top-left corner of the window over the game frame. It is the 8x8
  public-domain font at 2x on the debug menu's dark plate, with a strip in
  the notice's colour down its left edge, a second render pass with alpha
  blending (`app::overlay`). While the debug menu is open the notice is a
  line inside the menu instead ([debug-menu.md](debug-menu.md#layout)).
  The messages:
  "Building <hash>...";
  "Update ready: <subject, or N commits> - press F5 to restart" (with
  "(building <hash>...)" or "(<hash> failed to build)" when a newer commit
  is in progress or failed);
  "Update failed to build: <first error line>". After a failure the game
  keeps running the current build.
- **Restart.** With a banner configured, F5 and F6 never reach the game's
  pad. F5 works only while an update is ready, even with the banner hidden.
  It closes the window like Cmd+Q (the run ends as `HostStop` and prints
  its report), and `h2 play` exits with code 75 (`follow::RESTART_EXIT`).
  The supervisor then starts the new `h2` with the new `--out`, which boots
  fresh. Any other exit code ends the supervisor.
- **Safety.** Your checkout's working tree, index and branches are never
  touched. The only repository-side effects are the fetch's remote-tracking
  ref and the cache worktree's entry under `.git/worktrees`. Generated code,
  builds and logs stay in the cache: `logs/follow.log` holds the
  supervisor's events, and `logs/build-<hash>.log` holds each build's cargo
  and emit output. A lock file allows only one follower per cache. Pruning
  only touches `<cache>/builds` and `<cache>/gen` when they are plain
  directories, and unlinks (never follows) symlinks inside them. To
  remove it all: `git worktree remove --force <cache>/checkout`, then
  delete the cache dir.

Tests: `follow.rs` unit-tests the state machine (idle, building, ready,
failed, restart, queueing, rollbacks, close and interrupt). Against scratch
repositories and plain shell processes (no game, no cargo), it checks:

- a checkout symlinked to the user's checkout, the user's main checkout at
  the cache path, a foreign repository there, and a registered checkout
  whose `.git` file selects the main git dir or another worktree's are all
  refused, with the user's edits, staged changes, untracked files and
  branch intact;
- a leader that exits normally leaves nothing behind in its group;
- the first build leaves another worktree's missing entry alone, and
  re-adds its own;
- a build step started from a thread with the signals blocked still dies
  of SIGTERM;
- a cancel stops a step that ignores SIGTERM, and its background children,
  within the grace period, and a cancel before the spawn starts nothing;
- a second interrupt kills and reaps a game child that ignores SIGTERM
  before `drive` returns;
- a hanging fetch times out to the last fetched ref, and an interrupt
  during the startup fetch ends it at once; a transport helper that
  ignores SIGTERM is killed and reaped before the startup read returns,
  and before the watcher's thread ends on shutdown, mid-poll or mid-fetch. It also runs the event
loop with a fake builder and a fake child: build, F5, failure, fix, F5,
close. The status file format, the first-error pick and the argument split
are covered too. An ignored test (`cargo test -p cli --bin h2 -- --ignored
real_builder`) drives the real git, cargo, worktree and child code against
a scratch repository whose `cli` crate is a stand-in `h2`:

- the branch moves;
- the build generates the game because the previous one's stamp is not the
  new `h2`'s;
- a script child waits for "ready" and exits 75;
- the new build runs with the expected arguments;
- the user checkout stays clean;
- a broken commit reports `error[E0308]`;
- an h2-only commit reuses the generated game (`--check` passes);
- a bridge-only commit, whose `h2` sources are unchanged, generates it again;
- a clone follows its origin by fetching.

`app::overlay` tests dismissal, the restart offer and rasterising. A GPU test
draws the banner over a frame and reads back the blend.

## Watching the checkout: `h2 play --watch`

```sh
h2 play --watch /abs/path/to/disc/halo2.iso
```

Plays as `h2 play` does, and meanwhile watches your own checkout: its
working tree and its git state. When a burst of edits, or a `git pull`,
has settled, it does only what the changed files need: a rules script is
reloaded into the running game, the live web app is rebuilt, and a Rust or
generator change rebuilds `h2` in the checkout (and the game code when the
new `h2` says it must), then offers the same "Update ready" banner as
`--follow`. F5 restarts into it; F6 hides the banner. Everything after the
watch flags is passed to `h2 play` as is.

| Flag | Default | Meaning |
| --- | --- | --- |
| `--repo PATH` | the checkout containing the current directory | Checkout to watch. |
| `--quiet MS` | 1500 (minimum 200) | How long the files must be quiet before a change set is taken. |
| `--fresh-data` | off | Each launch gets an empty data dir under the cache (otherwise the user's `--data`). |
| `--cache DIR` | `watch/<hash of the checkout's path>` in the cache directory | Where logs, builds and generated games live. |

`--watch` and `--follow` are separate supervisors; giving both is an
error.

What each changed path asks for (`watch::classify`, relative to the
checkout's top; the first row that matches wins):

| Paths | Action |
| --- | --- |
| any `target/`, `node_modules/`, `buck-out/`, `__pycache__/`, `.direnv/`, `.devenv*` component; `.claude/worktrees/`, `web/dist/`, `out/`, `extracted/`, `disc/`, `.tools/`; `.git/objects/`, `logs/`, `worktrees/`, `modules/`, `lfs/`; editor droppings (`*~`, `*.swp`, `.#*`, `4913`, `.DS_Store`) | ignored |
| the rest of `.git` (HEAD, refs, the index, operation markers) | holds the settle timer open; builds nothing |
| `content/core/rules/*.rules`, `content/modes/rules/**/*.rules` | hot: `rules.reload`; also rebuilds `h2`, which bakes it |
| any other `*.rules` | hot: `rules.reload` |
| `docs/`, `*.md` | nothing |
| `web/test/`, `web/bench/`, `web/mock/` | nothing |
| the rest of `web/` | hot: `bun install --frozen-lockfile`, `bun run build` (with `--live-local`) |
| a `tests`, `benches` or `examples` component; `tests.rs`, `*_tests.rs`, `*_test.rs` | nothing |
| `crates/formats/xbe/src/`, `crates/recompiler/`, `crates/runtime/runtime/include/`, `crates/runtime/recomp/c/`, `crates/runtime/recomp/build.rs`, `crates/runtime/recomp/src/render_entries.rs` | game image: rebuild `h2`, then the game code if its stamp moved |
| the rest of `crates/`, `assets/`, `.cargo/`; `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` | host: rebuild `h2` |
| anything else (`scripts/`, `deploy/`, `editors/`, ...) | nothing |

The game-image rows are the directories `crates/runtime/recomp/build.rs`
hashes into the generator's stamp; the two lists are kept in step.

How it works (crates/apps/cli `watch.rs`):

- **Supervisor.** As `--follow`'s: the game runs as a child with
  `--follow-status`, in its own process group, and the same Ctrl-C, close
  and restart rules apply (`follow::ChildLauncher`, `follow::StatusFile`,
  `follow::watch_signals`). The first child is the `h2` you started.
- **Watching.** `notify` watches the checkout recursively: FSEvents on
  macOS, inotify on Linux. A linked worktree's own git dir, which lies
  outside it, is watched too (not recursively). Paths in the git dir count
  as `.git/...`.
- **Settling.** Every event that is not ignored, git's own included,
  restarts a quiet period of `--quiet` milliseconds (`watch::Debounce`).
  While the git dir holds `index.lock`, `HEAD.lock`, `MERGE_HEAD`,
  `rebase-merge`, `rebase-apply`, `CHERRY_PICK_HEAD` or `REVERT_HEAD`,
  nothing is taken, and the quiet period starts again once they are gone,
  so a pull or a rebase settles once, after its last write. A merge left
  with conflicts waits until it is committed or aborted. The changed paths
  are then one change set, classified into a plan (`watch::Plan`). When the
  watcher reports lost events (FSEvents' rescan flag), the plan rebuilds
  `h2` and checks the game code.
- **Hot, no restart.** One thread runs hot work in order, against the
  game's agent socket: `$H2_AGENT` when it names one, none when it is off,
  else the child's `agents/<pid>.sock`
  ([agent-api.md](agent-api.md#finding-a-game)). A rules script is
  compiled first (a compile error is the banner's), then, if the game's
  `rules.state` runs a script of the same name, sent with `rules.reload`
  (`{source, mode: auto}`): in place when its state keeps its shape, else
  the game restarts on the same map and variant
  ([rules-engine.md](../game/rules-engine.md#reloading-a-running-script)).
  `ctf.rules` makes both Capture the Flag and Assault
  (`rules::builtin::sources_of`). The banner says "Reloaded <name> (in
  place)" or "(restarted)", or "Reload failed: <error>". A web change runs
  bun in the checkout's `web/` when the game was started with
  `--live-local` and no `H2_LIVE_WEB`; the embedded server reads
  `web/dist` at each request, so the next page load has it ("Web app
  rebuilt"). That is so only while the running `h2` serves this checkout's
  `web/dist`: one built from this checkout does.
- **Building.** One build at a time. Changes that arrive during a build
  are merged into one queued plan, built when it finishes, and a failed
  build's plan is carried into the next one. A build runs `nice -n 15 cargo
  build --release -p cli` in the checkout, with its own target dir
  (`CARGO_TARGET_DIR`, else `target`), so it is incremental and uses the
  machine's rustc wrapper (kache); `CARGO_BUILD_JOBS` is half the cores
  unless you have set it. The new `h2 build <disc> --out <current game>
  --check` then says whether the current game code is still its own. If
  not, it runs `nice h2 build <disc> --out <cache>/gen/<n>`: a new dir,
  since the running game has the old dylib mapped, seeded with an APFS
  clone of the last game's `src/` so the emit rewrites only changed files,
  and with `H2_OBJ_CACHE` (default `obj` in the cache directory, shared
  with your other builds), so only changed shards compile
  ([BUILD-CACHE.md](../lifter/BUILD-CACHE.md)). `H2_OPT`, `H2_TIER` and the
  rest of your environment pass through. The new `h2` is copied to
  `<cache>/builds/<n>/` and offered: "Update ready: local changes (h2) -
  press F5 to restart", or "(h2 and the game code)". A build that left
  `h2` as it was and the game code current offers nothing. "Rebuilding
  h2..." or "Rebuilding h2 and the game code..." shows meanwhile.
- **Failures.** A failed build shows "Local build failed: <first error
  line>" and the game keeps running; `logs/build-<n>.log` has the whole
  output.
- **Safety.** It never writes to the checkout's sources, index or refs; the
  only writes there are cargo's target dir and, for the web app, `web/dist`
  and `web/node_modules`. A lock file in the cache allows one watcher per
  checkout. Its cache is not `--follow`'s, so the two can run side by side.
  `logs/watch.log` holds the supervisor's events.

Tests: `watch/tests.rs` covers the classifier table above, the plans, the
paths of a linked worktree's git dir, the debounce (a burst, a git
operation in progress, git or ignored events alone, a rescan), the
operation markers, and the supervisor over fakes: docs only (nothing), a
rules script (hot, no restart), a built-in script (hot and a build), a
host change (build, offer, F5), a failure (the game keeps running; the
plan is carried), changes merged into a queued build, an unchanged build,
close and interrupt, and `drive` end to end. The watcher, cargo, `h2
build` and the agent socket are not exercised by tests.

Hot reloading `h2` or the game code in place is not built. The game code
is a dylib whose functions the guest's threads are always inside, and
whose static data (dispatch tables, the guest's code addresses) the host
holds pointers into, so a swap needs every guest thread parked at a known
point and the host's tables rebuilt; the boundary where that holds is the
one machine snapshots use, between ticks with the guest parked
([snapshots.md](snapshots.md)). Restoring a snapshot taken by the old
module into a new one is only sound when the guest memory layout is the
same, which the stamp does not promise. Host Rust has no stable ABI, so
swapping it means a process restart or a narrow C-ABI plugin boundary.
