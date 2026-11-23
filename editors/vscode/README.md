# H2 Rules and HaloScript

Language support for h2recomp scripts: `.rules` game type scripts, the small
rules language that `h2 rules` compiles into game variants, and `.hsc`
HaloScript working copies that Studio writes beside a map.

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
- HaloScript (`.hsc`): highlighting of `(global ...)` and `(script ...)`
  headers, calls, strings, numbers, `;` line and `;* ... *;` block
  comments, and semantic highlighting, diagnostics and navigation from the
  language server `h2 hs lsp`. Studio writes an `h2hs.json` context file
  beside each `.hsc`; the server re-reads it when it changes.
- Studio links, when Studio is running: reload a rules script in the game,
  run a rules scenario test, and, for HaloScript, list where a script or
  global is used in the map and show it in Studio's Scripts tab.

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
| `h2rules.serverPath` | `""` | Path to the `h2` binary; it serves both `h2 rules lsp` and `h2 hs lsp`. Empty: `h2` on `PATH`, then `~/.cargo/bin/h2`. A leading `~` is the home directory; a bare name is looked up on `PATH`. Changing it restarts both servers. |
| `h2rules.trace.server` | `off` | `messages` or `verbose` logs the traffic with the server in the "H2 Rules Trace" output channel. |
| `h2hs.trace.server` | `off` | The same for the HaloScript server, in the "H2 HaloScript Trace" output channel. |

## Commands

| Command | What it does |
|---|---|
| H2 Rules: Restart Language Server | Stops and starts `h2 rules lsp`. Use it after rebuilding or reinstalling `h2`. |
| H2 Rules: Show Output | Opens the "H2 Rules" output channel. |
| H2: Reload Script in Game | Rules files. Saves the file and asks Studio to reload it into the running game. |
| H2: Run Scenario Test... | Rules files. Asks for a scenario `.json` (the last one used for the script is offered first), runs the script's current text against it in Studio, and writes the printout to the "H2 Rules" output channel. |
| H2: Show Uses in Map | HaloScript files. Lists where the script or global under the cursor is used in the map; picking one jumps to it. |
| H2: Show in Studio | HaloScript files. Shows the definition under the cursor in Studio's Scripts tab. |
| H2 HaloScript: Restart Language Server | Stops and starts `h2 hs lsp`. |

The Studio commands need Studio running; when it is not, the error says so.

## Troubleshooting

- **"Could not find the h2 binary"**: `h2` is not on the `PATH` VS Code
  sees (a VS Code started from the Dock may not see your shell's `PATH`)
  and not in `~/.cargo/bin`. Set `h2rules.serverPath` to the binary's full
  path. The error's "Open Settings" button goes straight to the setting.
- **No diagnostics or completion**: run "H2 Rules: Show Output" (for
  HaloScript, open the "H2 HaloScript" channel from the Output panel). The
  channel logs which binary started and any error the server printed.
  Check that `h2 rules lsp` runs in a terminal; an older `h2` may not have
  the `lsp` subcommand.
- **Odd server behaviour**: set `h2rules.trace.server` to `verbose` and
  read the "H2 Rules Trace" channel to see every request and reply.
- **After updating `h2`**: run "H2 Rules: Restart Language Server" so the
  editor picks up the new binary.

## License

WTFPL. See the LICENSE file.
