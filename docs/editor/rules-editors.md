# Game type scripts in VS Code and Zed

Status: Built. The extensions have not been through a person's install
on a clean machine; see "Not verified".
Code: `editors/vscode/` (the VS Code extension), `editors/zed/` (the Zed
extension), `editors/tree-sitter-rules/` (the grammar Zed highlights with),
`crates/apps/cli/src/rules_lsp.rs` (`h2 rules lsp`),
`crates/apps/cli/src/rules_editors.rs` (`h2 rules editors`). Tests: each
module's `tests`, `crates/formats/rules/src/ide/tests.rs`,
`editors/vscode/test/`, `editors/tree-sitter-rules/test/corpus/`.
Switch: none.

A game type script (a `.rules` file in the
[rules language](../game/rules-engine.md#the-language)) is written in VS
Code or Zed. Both start our language server, `h2 rules lsp`, which answers
every request from `rules::ide`, the same module Studio's script window and
Live's `POST /v1/rules/ide` use. Each extension also highlights on its own,
so a file reads well before the server starts. Studio's **Open in editor**
opens the script in whichever of the two has the extension, and each save
there comes back into Studio and the paired game
([editor-guide.md, "Game type scripts"](editor-guide.md#game-type-scripts)).

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
   directory), and whether `h2 rules lsp` answers `initialize` within ten
   seconds.
4. Open a `.rules` file.

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
| Completion, ranked, with snippets | `completion` (triggered by `.`, `:`, `"`, `(`, `,`, and a space after `is`, `on`, `carrier`, `requires`, `each`, `palette`, `list`, `scoring`, `quick`, `:` or `,`) |
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

## CI

`.github/workflows/editors.yml` runs on pushes to master and on pull
requests that touch `editors/**`, `crates/formats/rules/src/ide/**`,
`rules_lsp.rs`, `rules_editors.rs` or the workflow:

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
- **lsp**: `cargo test -p rules --lib ide` and the `rules_` tests of the
  `h2` binary (`rules_lsp`, `rules_editors`, `rules_cmd`).
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

## Related

- [rules-engine.md](../game/rules-engine.md): the language, the tooling
  and `rules::ide`.
- [rules-language-types.md, "Diagnostics and the editor"](../game/rules-language-types.md#diagnostics-and-the-editor):
  the diagnostic codes and their fixes.
- [editor-guide.md, "Game type scripts"](editor-guide.md#game-type-scripts):
  Studio's script window and **Open in editor**.
