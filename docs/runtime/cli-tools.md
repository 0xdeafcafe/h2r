# The tools binary (`h2-tools`)

Status: Built. The Halo 1 importer, the map pack installer and the other offline commands listed under "Rules and limits" still run inside `h2`.
Code: `crates/apps/h2-tools/src/main.rs` (the command table), `crates/apps/cli/src/tools.rs` (`h2`'s forwarding), `crates/apps/cli-common/src/lib.rs` (what both share). Tests: `crates/apps/cli/src/tools.rs` (`tests`), `crates/apps/cli-common/src/lib.rs`, `crates/apps/h2-tools/tests/shader_all.rs` and each command's own `tests` module.
Switch: none.

The offline map, tag and script tools read and write map, tag, script and
disc files and never boot the game. They live in a second binary,
`h2-tools`, which links none of the runtime, renderer, audio or agent
server, so a change to a tool relinks a small binary instead of the game.
`h2 <tool> ...` runs `h2-tools <tool> ...` with the same words, so every
command keeps its `h2` spelling. The rule: a command moves to `h2-tools`
only when nothing it needs is under `boot`, the game or a Live command
that stays in `h2`.

## How it works

`h2`'s `main` collects the words after the command name and asks
`tools::forwards` whether the command is a tool. If it is, `tools::run`
builds the process and replaces `h2` with it (`exec` on Unix; on Windows
it waits and exits with the tool's status), before `h2` does anything
else. The tool's standard streams, exit status and signals are therefore
its own, which `h2 rules lsp` needs: the editors start `h2 rules lsp` and
talk to it over stdio.

Where `h2-tools` comes from (`tools::command`):

1. Under `cargo run` of the `cli` package (`cargo h2`): cargo sets
   `CARGO` and `CARGO_MANIFEST_DIR` for the program it runs, and the
   latter equals the directory `cli` was compiled from. `h2` then runs
   `cargo run [--release] -q --manifest-path crates/apps/h2-tools/Cargo.toml
   --bin h2-tools -- <words>`, with `--release` when `h2` itself was built
   without debug assertions. The tools are rebuilt when their code changed,
   so an older `h2-tools` in the target directory is never run.
2. Otherwise: `h2-tools` (`h2-tools.exe` on Windows) in the directory of
   the running `h2`, or of the file it links to. When neither has one, the
   command fails with `h2-tools not found next to <path to h2>`, followed
   by how to install it.

`h2-tools` itself (`crates/apps/h2-tools/src/main.rs`) is a plain table
from the command name to its module, with its own usage text
(`h2-tools --help`); `h2`'s usage names the tools and points there.

## Interfaces

The commands `h2-tools` runs (`tools::TOOLS`):

| Command | Module in `crates/apps/h2-tools/src` |
|---|---|
| `rules check\|build\|run\|sim\|caps\|builtin\|settings\|lsp\|editors` | `rules_cmd.rs`, `rules_lsp.rs`, `rules_editors.rs` |
| `variants port\|from-script` | `variants_cmd.rs` |
| `firefight kit` | `firefight_cmd.rs` |
| `forge build\|check` | `forge_cmd.rs` |
| `maps`, `tags`, `tag` | `maps.rs`, `tag_repo_cmd.rs` (`tags extract\|build`) |
| `map testbuild` | `map_testbuild.rs` |
| `testmap` | `testmap_cmd.rs` |
| `collision`, `superbounce scan`, `hud preview\|sheet` | `collision.rs`, `superbounce_cmd.rs`, `hud_cmd.rs` |
| `object`, `strings`, `effect`, `lights`, `shader`, `hs` | `object.rs`, `strings.rs`, `effects.rs`, `lights.rs`, `shader.rs`, `scripts.rs` |
| `extract-shaders` | `extract_shaders.rs` |

Two subcommands of those stay in `h2` (`tools::forwards`): `h2 maps
install`, which runs the game's own installer (`boot::new_maps`,
`crates/apps/cli/src/maps.rs`), and `h2 forge looks`, which reads the disc
the way Live's art extractor does (`crates/apps/cli/src/forge_looks.rs`).

`cli-common` holds what both binaries use: the default disc (`$H2_DISC`,
else the Halo 2 `.iso` in the checkout's `disc/`), opening a disc image or
its maps (`open_iso`, `load_xbe`, `load_map`, `disc_map`) and the
no-clobber output directory every exporter writes through (`safe_out`).
Opening a disc registers it with `safe_out` in either binary.

Building and installing:

- `cargo h2-tools <tool> ...` (`.cargo/config.toml`) builds and runs only
  `h2-tools`. `cargo h2 <tool> ...` builds `h2` first, then the tools
  ([build-speed.md](../dev/build-speed.md), "The offline tools in their
  own binary").
- `cargo install --locked --path crates/apps/cli` and `cargo install
  --locked --path crates/apps/h2-tools` put both in `~/.cargo/bin`
  ([building.md](building.md)).
- Buck builds `//crates/apps/cli:h2` and `//crates/apps/h2-tools:h2-tools`
  ([buck.md](../dev/buck.md)).

## Rules and limits

- A tool depends on format and tool crates only. A command that needs
  `boot`, the game crate, the kernel, audio or a module that a runtime
  command also uses stays in `h2`. These offline commands stay for that
  reason: `bitmap` (its reader is shared with `lighting`), `lighting`
  (Live's sky and the input crate), `sound` and `anim` (the audio
  crate's ADPCM decoder), `font` and `ui` (`boot`'s icons and screens),
  `movies` (the Theater export), `import halo1`, `h1` and `parity` (the
  importer `h2 play` runs), `import-mappack` (the kernel's content
  signature, and `h2 play`), `bots nav`, `server-map`, `info`, `ls`,
  `extract`, `extract-xbe`, `disasm`, `analyze`, `fpstats` and `symbols`
  (parsed with `lift` and `emit` in `main.rs`).
- `h2` with only the `host` feature (the dedicated host) forwards the same
  commands; the host container ships no `h2-tools`, and none of its
  commands are tools.
- `cargo h2 <tool>` picks the profile from debug assertions, so a custom
  profile is not passed on, and cargo's own `--target-dir` flag is not
  either (`CARGO_TARGET_DIR` is, through the environment).

## Not verified

- That `exec` keeps `h2 rules lsp` working from VS Code and Zed: the
  forwarding test checks the words and exit status with a script, not an
  editor session.
- The Windows path (wait, then exit with the tool's status) has no test.

## Related

- [building.md](building.md): `cargo h2` and installing.
- [build-speed.md](../dev/build-speed.md): why the tools are a binary of
  their own.
- [rules-editors.md](../editor/rules-editors.md): the editors that start
  `h2 rules lsp`.
