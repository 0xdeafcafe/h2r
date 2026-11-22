# H2 Rules

Language support for h2recomp game type scripts: `.rules` files, the small
rules language that `h2 rules` compiles into game variants.

## Features

- Syntax highlighting: keywords, types, labels, handlers, capabilities,
  time literals (`5s`, `300t`, `1.5s`), and message holes in strings
  (`"{k} scored"`).
- Diagnostics, hover, completion, signature help, go to definition, find
  references, rename, document symbols, formatting, quick fixes and
  semantic highlighting, all from the language server `h2 rules lsp`.
- Snippets for the header, options, labels, variables, functions, every
  `on` handler, `for each`, `if`, `match`, `message` and `grant`.
- Comment toggling, bracket matching, auto-indent, and `// region` /
  `// endregion` folding.

## Install

The extension needs the `h2` binary from h2recomp, which serves the
language server.

With `h2` installed:

```sh
h2 rules editors install
```

Or install a built package by hand:

```sh
code --install-extension h2-rules-0.1.0.vsix
```

To build the package from a checkout:

```sh
cd editors/vscode
npm ci
npm run package
```

## Settings

| Setting | Default | What it does |
|---|---|---|
| `h2rules.serverPath` | `""` | Path to the `h2` binary. Empty: `h2` on `PATH`, then `~/.cargo/bin/h2`. A leading `~` is the home directory; a bare name is looked up on `PATH`. Changing it restarts the server. |
| `h2rules.trace.server` | `off` | `messages` or `verbose` logs the traffic with the server in the "H2 Rules Trace" output channel. |

## Commands

| Command | What it does |
|---|---|
| H2 Rules: Restart Language Server | Stops and starts `h2 rules lsp`. Use it after rebuilding or reinstalling `h2`. |
| H2 Rules: Show Output | Opens the "H2 Rules" output channel. |

## Troubleshooting

- **"Could not find the h2 binary"**: `h2` is not on the `PATH` VS Code
  sees (a VS Code started from the Dock may not see your shell's `PATH`)
  and not in `~/.cargo/bin`. Set `h2rules.serverPath` to the binary's full
  path. The error's "Open Settings" button goes straight to the setting.
- **No diagnostics or completion**: run "H2 Rules: Show Output". The
  channel logs which binary started and any error the server printed.
  Check that `h2 rules lsp` runs in a terminal; an older `h2` may not have
  the `lsp` subcommand.
- **Odd server behaviour**: set `h2rules.trace.server` to `verbose` and
  read the "H2 Rules Trace" channel to see every request and reply.
- **After updating `h2`**: run "H2 Rules: Restart Language Server" so the
  editor picks up the new binary.

## License

WTFPL. See the LICENSE file.
