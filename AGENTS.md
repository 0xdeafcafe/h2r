# Notes for coding agents

## Use the agent API

When you need to see, drive or check the running game or the editor, use
the agent API. Don't screen-scrape, read process memory by hand, drive the
UI with synthetic input, or add one-off debug hooks.

- The game: `h2 agent` ([docs/runtime/agent-api.md](docs/runtime/agent-api.md)).
- The editor: `h2-editor`'s socket API ([docs/editor/agent-api.md](docs/editor/agent-api.md)).

`session.methods` lists every method each API offers.

If the API can't do what you need, don't route around it. Decide whether
the API should be expanded to cover it:

- **Expand it** when the need will come up again: a new view of game or
  editor state, a new action, or a new check. Add the method with its
  params, result and tests, and document it in the API's doc. Then use it.
- **Don't expand it** only for a genuine one-off that no later agent or
  test will need. Say in your report why the API wasn't used and why it
  wasn't expanded.

In both cases, record the decision in your report or commit message.

Use the agent API whenever it can do the job, before anything else.

## Build fast

- **Game image (`h2 build`):** set `H2_OBJ_CACHE=~/Library/Caches/h2recomp/obj`
  on every `h2 build`/`h2 emit`. Clang objects are then reused across output
  dirs and worktrees. Without it, each new `--out` recompiles all ~1000
  shards (about 16 minutes at `H2_OPT=2`); with it, only the shards whose
  C changed are rebuilt. Reuse the same `--out` for repeated builds too, so
  the emit receipt skips lifting and C emission
  ([docs/lifter/BUILD-CACHE.md](docs/lifter/BUILD-CACHE.md)).
- **`H2_OPT` must match** between `h2 build` and `h2 run`/`h2 sim`, or the
  image silently recompiles.
- **Rust:** kache is the machine-wide `rustc-wrapper` (`~/.cargo/config.toml`),
  so compiles are shared between target dirs. Keep one target dir per
  worktree for a whole session, rather than deleting it between runs. Check
  before testing (`cargo check -p <crate> --tests`), and test only the crates
  a change touches (`cargo test -p a -p b`), not the workspace.
- **Headless host:** `cargo check -p cli --no-default-features --features host`
  covers the Linux host's feature set without a cross build.
- **Parallel work:** hand the code to several subagents at once, then build
  and test their branches together in one pass.
