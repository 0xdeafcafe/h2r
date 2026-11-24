# Game type scripts in VS Code and Zed

Status: Built. The extensions have not been through a person's install
on a clean machine; see "Not verified".
Code: `editors/vscode/` (the VS Code extension), `editors/zed/` (the Zed
extension), `editors/tree-sitter-rules/` (the grammar Zed highlights with),
`crates/apps/h2-tools/src/rules_lsp.rs` (`h2 rules lsp`),
`crates/apps/h2-tools/src/hs_lsp.rs` (`h2 hs lsp`, over
`crates/formats/hs/src/ide/`), `crates/apps/h2-tools/src/studio_link.rs`
(the commands' calls to Studio),
`crates/apps/h2-tools/src/rules_editors.rs` (`h2 rules editors`). Tests: each
module's `tests`, `crates/formats/rules/src/ide/tests.rs`,
`crates/formats/hs/src/ide/tests.rs`, `editors/vscode/test/`,
`editors/tree-sitter-rules/test/corpus/`.
Switch: `H2_STUDIO` (the Studio socket the commands call).

The same extensions edit HaloScript, the scenario scripts Studio writes
out as a working copy (see "HaloScript"), and their commands talk to a
running Studio (see "Commands that talk to Studio").

A game type script (a `.rules` file in the
[rules language](../game/rules-engine.md#the-language)) is written in VS
Code or Zed. Both start our language server, `h2 rules lsp`, which answers
every request from `rules::ide`, the same module Studio's script window and
Live's `POST /v1/rules/ide` use. Each extension also highlights on its own,
so a file reads well before the server starts. Studio's **Open in VS
Code** and **Open in Zed** open the script there, each save comes back
into Studio and the paired game, and Studio's edits go to the file
([editor-guide.md, "Scripts in VS Code and Zed"](editor-guide.md#scripts-in-vs-code-and-zed)).

## Install

1. Install `h2` (it serves the language server). The extensions look for
   it on `PATH`, then at `~/.cargo/bin/h2`.
2. Run `h2 rules editors install`.
   - **VS Code**: it finds `code` (on `PATH`, in Homebrew's or
     `/usr/local`'s `bin`, or inside `Visual Studio Code.app`), takes the
     newest `h2-rules-<version>.vsix` in `editors/vscode/` of the tree `h2`
     was built from, else downloads `h2-rules.vsix` from the
     `editors-latest` release (`EDITORS_RELEASE`), else builds one there
     with npm, and runs `code --install-extension <vsix> --force`.
     `--build` builds with npm first (`npm ci`, then `vsce package`).
   - **Zed**: Zed installs a dev extension from its command palette only,
     so the command prints the steps: install Rust with rustup (Zed builds
     the extension itself, for WebAssembly), run **zed: install dev
     extension**, and pick `editors/zed`.
   - `h2 rules editors install vscode` or `install zed` does one.
3. Run `h2 rules editors status`: whether VS Code has
   `h2recomp.h2-rules` (from `code --list-extensions --show-versions`),
   whether Zed has `h2-rules` (in `extensions/installed/` under Zed's data
   directory), and whether `h2 rules lsp` and `h2 hs lsp` each answer
   `initialize` within ten seconds.
4. Open a `.rules` file, or a map's scripts from Studio's Scripts tab
   (**Open in VS Code**).

By hand: `code --install-extension h2-rules-0.1.0.vsix` for VS Code. For
Zed, `editors/zed/extension.toml` names the grammar by repository and
commit; for a grammar not yet pushed, point its `repository` at
`file:///<checkout>` (Zed fetches the grammar with git, so the commit must
be committed locally) and keep `path = "editors/tree-sitter-rules"`. The
extension's README (`editors/zed/README.md`) has both forms.

## What the editors do

From the language server (`rules_lsp.rs`, every answer from `rules::ide`;
[rules-engine.md, "Security and usability"](../game/rules-engine.md#security-and-usability)
describes each feature):

| Feature | LSP method |
|---|---|
| Problems as you type, each with its code and a link to [the codes](../game/rules-language-types.md#diagnostics-and-the-editor) | `publishDiagnostics` (pushed on open and on each change) |
| Completion, ranked, with snippets | `completion` (triggered by `.`, `:`, `"`, `(`, `,`, and a space after `is`, `on`, `type`, `requires`, `each`, `palette`, `uses`, `quick`, `:` or `,`) |
| Hover | `hover` |
| The call's parameters | `signatureHelp` (`(` and `,`) |
| Go to definition, find references, the uses of the name under the cursor | `definition`, `references`, `documentHighlight` |
| Outline and breadcrumbs | `documentSymbol` |
| Rename, refused with the reason (a keyword, a name in use, a palette) | `prepareRename`, `rename` |
| Quick fixes | `codeAction` (`quickfix`) |
| Format the file or a selection | `formatting`, `rangeFormatting` |
| Colour by meaning | `semanticTokens/full`, legend `ide::LEGEND` |
| Folding of blocks, tables, comment runs and `// region` pairs | `foldingRange` |

The server syncs incrementally (`TextDocumentSyncKind` 2) in UTF-16
positions, as `initialize` declares (`positionEncoding: "utf-16"`).

On their own:

- **VS Code** (`editors/vscode/`): the `rules` language for `*.rules`; a
  TextMate grammar (`syntaxes/rules.tmLanguage.json`) for every keyword,
  the types and set words, strings with `{hole}` templates and escapes,
  numbers, time literals (`5s`, `300t`, `1.5s`), comments, operators,
  labels after `label` and `is`, handlers after `on`, and capabilities;
  `semanticTokenScopes` mapping each legend name, `api`, `label` and
  `capability` included, to a TextMate scope so any theme colours them;
  `language-configuration.json` (`//` comments, brackets, auto-closing
  pairs outside strings and comments, indentation after `{`, comment
  continuation on Enter, the word pattern, `// region` folding markers);
  snippets (`snippets/rules.json`). `src/extension.ts` starts the server
  with `vscode-languageclient`, finding `h2` from the `h2rules.serverPath`
  setting, else on `PATH`, else at `~/.cargo/bin/h2`
  (`src/serverPath.ts`). **H2 Rules: Restart Language Server** restarts
  it, and a change of `h2rules.serverPath` does too; the **H2 Rules**
  output channel logs which binary started, and `h2rules.trace.server`
  logs the traffic in **H2 Rules Trace**.
- **Zed** (`editors/zed/`): the `Rules` language for `*.rules`, its
  brackets, indents, outline and highlights from the
  `tree-sitter-rules` grammar (`languages/rules/*.scm`), and a WebAssembly
  extension (`src/lib.rs`, over `zed_extension_api`) whose
  `language_server_command` runs the `lsp.h2-rules-lsp.binary.path`
  setting if set (with its `arguments` and `env`), else `h2` from the
  worktree's `PATH`, else `~/.cargo/bin/h2`, with `rules lsp`. The
  extension API has no hook for semantic tokens: the server sends them,
  and whether Zed paints them over the tree-sitter highlights is Zed's
  setting.
- **tree-sitter-rules** (`editors/tree-sitter-rules/`): a grammar of the
  whole language as `crates/formats/rules/src/lang/lex.rs` and
  `parse.rs` define it, with corpus tests. `src/parser.c` is generated
  (`tree-sitter generate`) and committed; CI checks it matches
  `grammar.js`. Whitespace, line breaks included, is an extra: no
  statement goes on with a token that starts the next one, except a line
  that starts with `(`, `[` or `-`, which precedence reads as a call, an
  index or a subtraction (`parse.rs` reads the first two as a new
  statement). It is looser than `parse.rs` in a few places (two
  statements on a line without `;`, any expression as a label count or
  match pattern); `grammar.js`'s header lists them. Highlighting needs
  no more. `editors/zed/extension.toml` pins the grammar to a commit of
  this repository (`rev`), so a grammar change lands in Zed after that
  commit is pushed and `rev` moves to it.

A change to the language (a keyword, a token, a statement) changes
`lex.rs` or `parse.rs`, and with them `grammar.js`, the TextMate grammar
and the Zed queries.

## HaloScript

Studio's **Open in VS Code** and **Open in Zed** on the Scripts tab write
the open map's globals and scripts to `<data>/scripts/<map>/<map>.hsc`,
with a context file, `h2hs.json`, beside it
([editor-guide.md, "Scripts in VS Code and Zed"](editor-guide.md#scripts-in-vs-code-and-zed)).
Both extensions start `h2 hs lsp` for `*.hsc` files. It answers from
`hs::ide`, which checks a file against the `h2hs.json` in its folder
(`hs::ide::Context`):

- the executable's value types and script kinds (with the kinds that
  take a return type);
- every builtin, with its name (or `#builtin:<index>`), return and
  parameter types, usage text and documentation;
- the game globals the scripts name;
- the scenario's names by type (object names, trigger volumes, and the
  other names the scripts spell).

The context comes from the user's own map and executable and stays in
the data root. A `.hsc` file with no context beside it is still parsed:
its syntax, definitions and references work, and one information line
says the builtins and names are unknown. The server reads the context
again when it changes on disk (`workspace/didChangeWatchedFiles`, or a
new modification time) and checks every open file again.

| Feature | LSP method |
|---|---|
| Problems as you type: unbalanced parentheses and strings, a malformed or duplicate definition, an unknown type or script kind, an unknown builtin or script (with the nearest name), the wrong number of arguments, an object name or trigger volume the scenario does not have (a warning) | `publishDiagnostics` |
| Completion of builtins, scripts, globals, types, script kinds and, in a parameter of a named type, the scenario's names of that type | `completion` (`(` and `#`) |
| Hover: a builtin's call form, usage and documentation; a script's or global's header | `hover` |
| The call's parameters | `signatureHelp` (`(` and a space) |
| Go to definition and references of scripts and globals across the file; the uses under the cursor | `definition`, `references`, `documentHighlight` |
| Outline, folding of each list and comment that spans lines | `documentSymbol`, `foldingRange` |
| Rename a script or global, refused for a builtin or a name in use | `prepareRename`, `rename` |
| Quick fixes (the nearest known name) and the Studio commands as source actions | `codeAction` |
| Format: each line indented two spaces per open parenthesis, comments and line breaks kept | `formatting` |
| Colour by meaning | `semanticTokens/full`, legend `hs::ide::LEGEND` |

On their own, VS Code has the `haloscript` language for `*.hsc`: a
TextMate grammar (`syntaxes/haloscript.tmLanguage.json`) for comments,
strings, numbers, definitions and calls, and
`haloscript-language-configuration.json` (`;` comments, brackets).
**H2 HaloScript: Restart Language Server** restarts its server, and
`h2hs.trace.server` logs the traffic. Zed has the `HaloScript` language
(`languages/haloscript/config.toml`) with no tree-sitter grammar: its
colour comes from the server's semantic tokens alone.

## Commands that talk to Studio

The extensions' commands go to the language server as
`workspace/executeCommand`, and the server calls Studio's agent API on
its socket (`studio_proto::default_socket`: `$H2_STUDIO`, else
`studio.sock` in the cache folder; `studio_link.rs`). Studio has to be
running; when it is not, the command says so.

| Command (VS Code) | Server command | Studio method | What it does |
|---|---|---|---|
| **H2: Reload Script in Game** | `h2.studio.reload` | `rules.reload {path}`, then `rules.get` | Saves the `.rules` file; Studio reads it into its script window (opening it there if another script is open) and reloads it into the paired game. The message says how it went in: in place, restarted (with the reasons) or refused. |
| **H2: Run Scenario Test...** | `h2.studio.runScenario` | `rules.run` | Runs the editor's text over a scenario file (picked once, then offered again) on the fake game, as `h2 rules run` does, and prints the result in the **H2 Rules** output channel. |
| **H2: Show Uses in Map** | `h2.studio.uses` | `script.uses` | Lists the definitions that use the script, global, object name or trigger volume under the cursor, each going to its place in the working copy. |
| **H2: Show in Studio** | `h2.studio.show` | `script.source` | Shows the definition under the cursor in Studio's Scripts tab. |

VS Code puts the first two in a `.rules` file's right-click menu and the
other two in a `.hsc` file's. Zed has no command palette entries for an
extension: each is a code action (**Cmd+.**) of kind `source`. Run
scenario is offered for each `<script name>*.json` beside the script.

## CI

`.github/workflows/editors.yml` runs on pushes to master and on pull
requests that touch `editors/**`, `crates/formats/rules/src/ide/**`,
`crates/formats/hs/src/ide/**`, `rules_lsp.rs`, `hs_lsp.rs`,
`studio_link.rs`, `rules_editors.rs` or the workflow:

- **tree-sitter-rules**: installs the tree-sitter CLI, runs
  `tree-sitter generate`, fails if `src/` differs from what is
  committed, and runs `tree-sitter test`.
- **vscode**: `npm ci`, `npm run typecheck`, `npm test` (the TextMate
  grammar's scopes on a sample through `vscode-textmate` and
  `vscode-oniguruma`, every JSON file, the server path search), then
  `vsce package`; the artifact holds `h2-rules-<version>.vsix` and the same
  file as `h2-rules.vsix`.
- **zed**: builds the extension for `wasm32-wasip2` and uploads the
  folder with `extension.wasm` as `h2-rules-zed.zip`.
- **lsp**: `cargo test -p rules --lib ide`, `cargo test -p hs --lib ide`,
  and the `rules_`, `hs_lsp` and `studio_link` tests of the `h2-tools`
  binary.
- **release** (master only, after the four pass): moves the
  `editors-latest` tag to the commit, creating the prerelease the first
  time, and replaces its assets with the `.vsix` and the Zed zip.

## Troubleshooting

- **"Could not find the h2 binary"** (VS Code) or no server in Zed's log
  (**zed: open log**): an app started from the Dock does not see the
  shell's `PATH`. Set `h2rules.serverPath`, or in Zed's settings
  `"lsp": {"h2-rules-lsp": {"binary": {"path": "/path/to/h2"}}}`.
- **No problems or completion**: `h2 rules editors status` says whether
  the server starts. VS Code's **H2 Rules** output channel shows its
  errors; `h2rules.trace.server: "verbose"` shows each request.
- **After rebuilding `h2`**: **H2 Rules: Restart Language Server** in VS
  Code; in Zed, **editor: restart language server**.
- **The download fails**: `editors-latest` exists after the workflow's
  first run on master. Build locally with `h2 rules editors install
  --build`, which needs Node.js.
- **Zed does not build the extension**: Zed needs Rust from rustup to
  build a dev extension; see its log.
- **Studio opens another editor**: Studio looks for the extension in
  `~/.vscode/extensions/h2recomp.h2-rules-*` and in Zed's
  `extensions/installed/h2-rules` (`extension_installed` in
  `crates/apps/studio/src/app/rules_edit.rs`). A VS Code that keeps its
  extensions elsewhere (Insiders, a portable install) is not seen.

## Not verified

- Installing the `.vsix` and the Zed dev extension on a clean machine.
- Zed's semantic-token colours from `h2 rules lsp`, and whether Zed reads
  a `file://` grammar repository.
- The `editors-latest` release download from a machine without GitHub
  credentials, which needs the repository to be public.
- Whether Zed takes a project's own `.rules` file (its agent rules file)
  for a Rules script.
- HaloScript in Zed: the language has no grammar, so its colour depends
  on Zed painting the server's semantic tokens.
- The Studio commands end to end in VS Code and Zed against a running
  Studio; the tests drive the servers and Studio's API separately.

## Related

- [rules-engine.md](../game/rules-engine.md): the language, the tooling
  and `rules::ide`.
- [rules-language-types.md, "Diagnostics and the editor"](../game/rules-language-types.md#diagnostics-and-the-editor):
  the diagnostic codes and their fixes.
- [editor-guide.md, "Game type scripts"](editor-guide.md#game-type-scripts):
  Studio's script window.
- [editor-guide.md, "Scripts in VS Code and Zed"](editor-guide.md#scripts-in-vs-code-and-zed):
  **Open in VS Code**, the working copy and the sync both ways.
- [haloscript.md](../formats/haloscript.md): the script tables the
  working copy is decompiled from.
