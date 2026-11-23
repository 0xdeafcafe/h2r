# h2-tools

The `h2-tools` binary: the offline map, tag and script tools (`rules`, `variants`, `firefight kit`, `forge build|check`, `maps`, `tags`, `tag`, `map testbuild`, `testmap`, `collision`, `superbounce`, `hud`, `object`, `strings`, `effect`, `lights`, `shader`, `hs`, `extract-shaders`). It links none of the game, so a change here relinks in seconds. `h2 <tool>` runs it with the same words ([cli-tools.md](../../../docs/runtime/cli-tools.md)).

## Where it sits

`apps`. It depends on format and tool crates and on `cli-common`; nothing depends on it.

## Layout

- `src/main.rs`: the command table and usage.
- One module per command, named as in `h2`'s table (`firefight_cmd.rs`, `rules_cmd.rs`, ...).

## Running

`cargo h2-tools <tool> ...` builds and runs only this binary. Install it beside `h2` with `cargo install --locked --path crates/apps/h2-tools`.

## Tests

`cargo xtask test -p h2-tools`. Tests that need a disc are `#[ignore]`d and read `H2_DISC`.

## Docs

- [cli-tools.md](../../../docs/runtime/cli-tools.md)
- [building.md](../../../docs/runtime/building.md)
