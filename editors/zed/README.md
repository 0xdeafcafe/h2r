# H2 Rules for Zed

A Zed extension for h2recomp's game type scripts (`.rules`): highlighting,
brackets, indents and the outline from the tree-sitter grammar in
`editors/tree-sitter-rules`, and diagnostics, completion, hover,
go-to-definition, references, rename, code actions and formatting from
`h2 rules lsp`.

## Install

1. Install `h2` and the `h2-tools` it runs the language server from, from
   an h2recomp checkout:

   ```sh
   cargo install --locked --path crates/apps/cli
   cargo install --locked --path crates/apps/h2-tools
   ```

2. Install Rust through rustup with the WebAssembly target Zed builds
   extensions for. Current Zed builds for `wasm32-wasip2`:

   ```sh
   rustup target add wasm32-wasip2
   ```

3. In Zed, run **zed: install dev extension** from the command palette and
   pick this directory (`editors/zed`). Zed builds the extension and fetches
   the grammar.

Open a `.rules` file. The status bar's language reads "Rules", and the
server starts on the first file.

## The grammar's revision

`extension.toml` names the grammar by repository, commit and path:

```toml
[grammars.rules]
repository = "https://github.com/0xdeafcafe/h2r"
rev = "<commit SHA>"
path = "editors/tree-sitter-rules"
```

Zed clones the repository at `rev` and builds `path/src/parser.c`, so `rev`
must be a pushed commit that holds the generated `src/`. To try grammar
changes before they are pushed, point `repository` at your checkout and
`rev` at a local commit:

```toml
repository = "file:///absolute/path/to/h2recomp"
rev = "<local commit SHA>"
```

Zed reads the commit, not the working tree: commit `grammar.js` and the
regenerated `src/` first (see `editors/tree-sitter-rules/README.md`), then
reinstall the dev extension.

## Settings

The server is `h2-rules-lsp`. The extension starts, in order: the
`binary.path` setting, `h2` on the project's `PATH`, then
`$HOME/.cargo/bin/h2`, with the arguments `rules lsp`. To name the binary:

```json
{
  "lsp": {
    "h2-rules-lsp": {
      "binary": {
        "path": "/Users/you/.cargo/bin/h2",
        "arguments": ["rules", "lsp"]
      }
    }
  }
}
```

`arguments` replaces `rules lsp` when it is set; `binary.env` adds
environment variables.

Semantic tokens: `h2 rules lsp` answers `textDocument/semanticTokens/full`.
The extension API has no hook for them; whether Zed paints them on top of
the tree-sitter highlights depends on the Zed version and its own settings
(see Zed's documentation for semantic tokens). Highlighting from the
grammar works either way.

## Troubleshooting

- **The server does not start.** Run **zed: open log** and look for
  `h2-rules-lsp`. "No such file" means `h2` is not where the extension
  looked: the extension runs in a sandbox and cannot check that
  `$HOME/.cargo/bin/h2` exists, so put `h2` on `PATH` or set
  `lsp.h2-rules-lsp.binary.path`.
- **`h2` is found but too old.** `h2 rules lsp` came with the rules editor
  work; reinstall `h2` from a current checkout.
- **No highlighting.** The grammar failed to build or fetch: the log names
  the `rules` grammar. Check that `rev` is a commit holding
  `editors/tree-sitter-rules/src/parser.c`.
- **The extension fails to build.** Check `rustup target list --installed`
  for `wasm32-wasip2` (older Zed releases use `wasm32-wasip1`). To build by
  hand: `cargo build --release --target wasm32-wasip2` in this directory.
- **Zed's own `.rules` file.** Zed reads a project-root file named `.rules`
  as agent instructions; this extension's `path_suffixes = ["rules"]` may
  also claim it as a Rules script. Pick another language for it from the
  status bar if so.

## Layout

- `extension.toml`: the extension, its language server and its grammar.
- `languages/rules/`: the language's config and queries (`highlights.scm`,
  `brackets.scm`, `indents.scm`, `outline.scm`).
- `src/lib.rs`: starts `h2 rules lsp`.
- `Cargo.toml`: a standalone crate (its own `[workspace]`), not part of
  the repository's root workspace.
