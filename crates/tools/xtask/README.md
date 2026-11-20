# xtask

`cargo xtask`, the workspace's test and check runner. It works out
which packages a branch can have changed, tests all of them in one
`cargo nextest run` so the tests of every crate share the cores, runs
their doctests, and prints one line per package. The alias is in
`.cargo/config.toml`; the crate depends on `anyhow` and `serde_json`
only, so it builds in seconds.

```sh
cargo xtask test                    # check cli, then test the affected packages
cargo xtask test --dry-run          # the packages and the exact commands, nothing run
cargo xtask test -p tags -p map     # these packages
cargo xtask test --all --web        # every package, and the web app
cargo xtask test --disc             # and the disc tier
cargo xtask check                   # cli checks, crate rules and table, changed docs
cargo xtask affected --why          # the affected packages, one per line
```

| Option | Meaning |
|---|---|
| `--base <rev>` | compare against the merge base with `<rev>` (default `origin/master`) |
| `-p`, `--package <p>` | these packages instead of the affected ones; repeatable, or `a,b` |
| `--all` | every package |
| `--disc` | `test`: the ignored tests with the disc files from `disc/`, the lift oracle and the differentials |
| `--web` | `test`: `bun test` and `bun run typecheck` in `web/` beside the Rust tests; automatic when `web/` changed |
| `--no-ledger` | `test`: run every package, record no pass |
| `--why` | `affected`: which file put each package in the set, on stderr |
| `--dry-run` | print the plan, run nothing |

The modules:

- `workspace`: the packages from `cargo metadata --no-deps`, changed
  paths to packages, reverse dependencies, the files outside a crate
  that its sources name.
- `report`: nextest's JUnit report, `cargo nextest list` and
  `cargo test` output read into one summary per package.
- `ledger`: passes keyed on a package's inputs, shared by every
  checkout on the machine.
- `xml`: just enough XML for the JUnit report.
- `git`, `proc`: the git questions it asks, and running commands.

How the affected set, the ledger and the test run work is in
[docs/dev/build-speed.md](../../../docs/dev/build-speed.md), sections 3
to 5; the tiers are in
[docs/dev/crate-structure.md](../../../docs/dev/crate-structure.md),
"Gates". The nextest profile it runs, `xtask`, is in
`.config/nextest.toml`.
