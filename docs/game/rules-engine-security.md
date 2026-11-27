# Rules engine: security and usability

Status: part of [the rules engine](rules-engine.md); see there for status, decisions and the roadmap. This page: the VM budgets, threat model and authoring experience.

Code and evidence labels are those of the parent page and [docs-style.md](../meta/docs-style.md).

## Security and usability

- **The VM is the boundary.** A blob runs on whichever machine hosts,
  often a player's, and wherever the film is replayed. The verified IR,
  a VM in memory-safe Rust, the bounds below and an API with no I/O make
  that safe. A guest never executes a script and reaches the VM only
  through its ordinary inputs.
- **Allow-listed where results count.** Ranked hoppers and Live's stats
  and ranks accept a game only when its script hash is on Live's
  allow-list. A modified host can already report any score; a script
  must not be a second way to do it.
- **Verified twice**, at upload and at load, by the same code. Live
  stores only blobs that verify.
- **Capability-limited.** The API table is the whole world: no file,
  network, clock, address, datum or other process, and a script reaches
  only the rows whose capability it declares, which the lobby, the
  hopper and the allow-list read first
  ("Roles, powers and capabilities"). A tag the map lacks or a label it
  does not define is a load error naming it.
- **Bounded.** IR, state, ops per tick, labelled objects, objects made
  per tick, objects alive, grants and edits in a match all have caps; the engine's object and tag
  limits are checked before a create.
- **Text.** Labels are at most 32 bytes and messages 128, sanitised by
  the rules for player names (game-options.md section 4) and formatted
  only with typed arguments.
- **Input rate.** `on press` fires at most 10 times a second per player;
  the rest are dropped the same way on every run.
- **Clean failure.** A script error ends the round; the engine call in
  progress returns first.

Tooling:

- **Studio**: the Game type script window
  ([editor-guide.md](../editor/editor-guide.md#game-type-scripts);
  `crates/apps/studio/src/app/rules_edit.rs`) colours the text by
  `rules::ide`'s semantic tokens, underlines and lists its diagnostics
  as you type, and offers completion as you type (ranked by
  `sort_text`, snippets with their defaults), hover, signature help,
  quick fixes (`ide::actions`), rename, go to definition, the uses of
  the name at the cursor (`ide::references`) and format; its check report
  is `rules::workbench::check` (worst-tick estimate against the budget,
  the costliest handler, slots, IR bytes, capabilities). It reloads the
  script into the paired game ("Reloading a running script"). The blob
  carries an optional line table for messages; the VM ignores it.
- **Command line**: `h2 rules` is pure over its inputs and needs no
  disc (`crates/apps/h2-tools/src/rules_cmd.rs`).
  - `h2 rules check <file.rules>` compiles and verifies. Every error
    `rules::diagnose` finds prints as `file:line:col: message [code]`,
    and the exit status is 1; warnings print the same way with
    `warning: ` before the message, after the report. The report gives
    the worst-tick estimate against the budget, the costliest handler,
    the IR bytes, the slots by scope, the blob's id, what the script can
    do in the words of `caps::CAPABILITIES`, and the entries a host
    cannot make. A script over the tick budget or the IR cap
    (`rules::fits`) prints the report, then `file: refused: ...`, and
    exits 1.
  - `h2 rules build <file.rules> -o <out.rir>` writes the verified blob
    and prints its SHA-256. It writes nothing, and exits 1, for a script
    `check` refuses.
  - `h2 rules run <file.rules> --scenario <scenario.json> [--trace]`
    steps the script over a scenario on the fake game, printing the calls
    made each tick (with `--trace`, each event and call), the final
    scores and the result; a script error exits 1.
  - `h2 rules sim <file.rules> [--players N] [--teams N] [--seed S]
    [--matches N] [--length SECONDS] [--score-limit N] [--json]
    [-o <scenario.json>]` plays generated matches (`rules::sim`,
    "Workbench" below) and prints how the seed's match ended, who won
    and the standings; with `--matches`, the batch's summary and
    warnings too. `-o` writes the seed's match as a scenario file that
    `h2 rules run` reads. `--json` prints the match, the summary and the rows. A script
    error in any match exits 1.
  - `h2 rules caps` lists the capabilities and the API entries with their
    status. `h2 rules builtin [name]` lists the engine ports
    (`builtin::ALL`, as `engine <index>: <name>`), then the example
    scripts (`builtin::examples`, as `example <key>: <name>`), or prints
    one by name, engine index or key.
  - `h2 rules lsp` is a language server on stdin and stdout, reading a
    script's `use` files from beside the document
    (`crates/apps/h2-tools/src/rules_lsp.rs`, over `lsp-server`). It keeps
    each open document and applies incremental changes in UTF-16
    positions (`ide::offset`), publishes diagnostics on open and change
    (each with its code and a `codeDescription` link to
    [rules-language-types.md, "Diagnostics and the
    editor"](rules-language-types.md#diagnostics-and-the-editor)), and
    answers completion, hover, signature help, definition, references,
    document highlights, `semanticTokens/full`, document symbols,
    folding ranges, formatting and range formatting, quick fix code
    actions, `prepareRename` and rename from `rules::ide`. A typed space
    asks for completions only after `is`, `on`, `type`, `requires`,
    `each`, `palette`, `uses`, `quick`, a `:` or a `,`
    (`SPACE_TRIGGER_WORDS`).
  - `h2 rules editors [status|install [vscode|zed] [--build]]` installs
    the VS Code extension and prints the Zed steps, or says what is
    installed and whether `h2 rules lsp` starts
    (`crates/apps/h2-tools/src/rules_editors.rs`;
    [rules-editors.md](../editor/rules-editors.md)).
- **Language intelligence**: `rules::ide` (`crates/formats/rules/src/ide/`)
  is pure over source text, and the language server, Live's
  `POST /v1/rules/ide`, the web editor and Studio share it. Positions are LSP's:
  a 0-based line and a UTF-16 `character`. The lexer and the parser
  recover (`lex::lex_tolerant`, `parse::parse_recovering`): a bad
  character is skipped, a bad statement is skipped to the end of its
  line, a bad declaration to the next line that starts one, and a block
  left open ends at the next declaration. Every feature works from what
  parsed.
  - `diagnostics`: every syntax error; when the source parses, every
    error and warning of the checker (`lang/check.rs`), each with its
    code and its fixes ([rules-language-types.md, "Diagnostics and the
    editor"](rules-language-types.md#diagnostics-and-the-editor)); with
    no error, a verifier refusal, the tick budget and the IR size (on
    the first line); a warning for a `let`, a set member or a `fn` never
    used; and a note at each call of an entry no host can make yet,
    naming its status.
  - `actions` gives the fixes of the diagnostics at a range, and
    `rename` the edits that rename a name and every use of it
    (`ide/fix.rs`). An option's `quick` mark is a use of the option, so
    renaming the option renames the mark; a setting's name is the
    game's and is not renamed.
  - `complete` reads the tokens before the place: declarations and
    their snippets at the start of a top-level line; event names, with
    their arguments as a snippet, after `on`; type words after `type` and `uses`;
    capabilities on a `requires` line and as the second argument of
    `grant`, `revoke` and `granted`; types after `:`; after a `.`, the
    `variant` fields, an enum's variants, a table row's columns, or a
    player's, team's or object's variables and built-in fields;
    palettes after `palette`, and sets, the declared labels among them,
    after `each` and in `count`, `first` and `pick`. Inside `settings { }`
    it offers the setting names of the script's type that the block
    does not set (each as `name = ` with its values as the snippet's
    choice), and after an `=` that setting's values: a choice's names, a
    bool's two, `none` where zero is none, a number's place. After
    `quick` it offers the script's options, then the settings its block
    sets, then the type's other record settings. The header words `type`,
    `points`, `uses`, `about`, `quick` and
    `settings` are among the top-level snippets. In a body it offers the locals
    in scope, the script's globals, options, tables, enums and
    functions, every API entry and language built-in as a snippet, and
    the statements. Each item's documentation is the hover text.
  - `hover`: a setting's label, kind, the game types it applies to and
    its values (on its line in the block or on its `quick` mark); a name's
    declaration and type; an API entry's signature,
    whether it reads or acts (and whether it is queued), the capability
    it needs in the lobby's words, and its status in plain words; an
    event's arguments; a `variant` field, a built-in field, a carrier, a
    capability, a type or a keyword.
  - `signature_help` for API entries, language built-ins, a script's
    functions and an `on` handler's arguments, with the argument the
    place is in.
  - `definition` and `references` for locals, arguments, parameters,
    variables, options, enums and their variants, tables and columns,
    functions, palettes and labels (a label's first use is its
    definition). `highlights` gives the same uses in source order, the
    declaration and each assignment marked as a write.
  - `prepare_rename` names what a rename at a place would rename, or
    refuses (no name there, or a palette, named by its string).
  - `folding_ranges`: each bracketed span over more than one line (the
    closer stays in sight), each run of two or more whole-line comments,
    and each `// region` ... `// endregion` pair (`ide/fold.rs`).
  - `semantic_tokens` classifies every comment, literal, keyword, name
    and operator by the legend `ide::LEGEND`: keyword, event, function,
    api, variable, parameter, enum, enumMember, type, number, string,
    comment, operator, label, capability. A dotted capability is one
    token.
  - `symbols` is the outline: the header, then each declaration and
    handler, with an enum's variants and a table's columns as children.
  - `format` lays a script that parses out canonically: four-space
    indents by bracket depth, one statement per line (a block's braces
    and `;` break lines), one space around operators and after commas
    and colons, at most one blank line in a row, comments where they
    are. It moves line breaks only where the grammar ignores them and
    refuses its own output if a script that compiled would compile to
    another blob. Every built-in formats to itself on a second pass and
    to the same blob. `format_range` formats the whole script, diffs
    it by line and keeps the changed runs that meet the range.
- **Editors**: VS Code and Zed have extensions in `editors/` that start
  `h2 rules lsp` and highlight without it (a TextMate grammar for VS
  Code, the `tree-sitter-rules` grammar for Zed);
  [rules-editors.md](../editor/rules-editors.md) covers installing them,
  what they do and their CI. Any other editor points a generic LSP client
  at `h2 rules lsp` for files ending `.rules`:
  - Helix, in `languages.toml`:

    ```toml
    [language-server.h2-rules]
    command = "h2"
    args = ["rules", "lsp"]

    [[language]]
    name = "h2-rules"
    scope = "source.h2rules"
    file-types = ["rules"]
    comment-token = "//"
    indent = { tab-width = 4, unit = "    " }
    language-servers = ["h2-rules"]
    ```

  - Neovim (0.11 or later):

    ```lua
    vim.filetype.add({ extension = { rules = "h2rules" } })
    vim.lsp.config("h2_rules", {
      cmd = { "h2", "rules", "lsp" },
      filetypes = { "h2rules" },
      root_markers = { ".git" },
    })
    vim.lsp.enable("h2_rules")
    ```
- **Scenario tests**: a scenario is JSON read by `rules::scenario`:
  `seed`, `options` (bits; the blob's defaults when absent), `players`
  (`slot`, `team`, `alive`, `score`, `lives`, `unit`, the object
  that is the player's unit, and `body`: `airborne`, `crouching` and
  `primed`, what the physics reads answer until a `touchdown` lands the
  player or a `launch` puts it in the air), `sight` (`[from, to]` pairs of objects with nothing solid
  between them, what `sees` answers true for), `teams`, `variant` (record fields by name;
  each one left out reads the stock record of the script's type,
  `fake::stock_variant`), `map` (the things the game holds that a script
  labels: `label`, `object`, `number`, `placement`, `team` and `class`,
  below; what a declared label's set visits and `o.number` reads;
  `labelled` is its older name),
  `round_time_left`, and `ticks`, each with `events` of `kill`
  (`victim`, `killer`), `spawn`, `death`, `join`, `quit`, `enter` and
  `leave` (`zone`, `object`), `press`, `pickup`, `drop` and
  `edit` (`player`, `object`), `bounce` (`player`, `object`, `chain`,
  1 when absent, and `monster`, false when absent), `bounced` (the
  player), `squad_dead`, `actor_kill` (`killer`,
  `class`), `actor_assist` (`killer`, `assist`, `class`), `damage`
  (`object`, `by`, `amount`, a raw `fixed`), `touchdown` (`player`,
  `down`, `up`, raw `fixed`), `launch` (`player`, `spot`, `none` when
  absent, `speed`, a raw `fixed`), `grounded` (the player) and
  `round_end`. Tick 0 reports `init` and `round_start` first; each tick
  ends with the per-tick group; queued calls are made at the start of
  the next tick and after the last. The fake game is the one the
  workbench steps (`scenario::step` runs a tick for both): a kill marks
  the victim dead and counts its death and the killer's kill (each
  taking one of the victim's lives when the variant's `lives` field is
  positive), a spawn marks the player alive unless the engine holds it
  (out of lives under a positive `lives` field, when only the script's
  `respawn` brings the player back), `round_time_left` counts down a
  tick at a time, and a zone a living team alone is inside is the one
  `ticks_held` counts.
  `h2 sim` runs a script in a headless match. Each built-in script has
  scenario tests beside its source.
- **Agent API**: `rules.state` (script, hash, refusal, budget use),
  `rules.vars`, `rules.trace` (handlers run and calls made per tick),
  `rules.step` and `rules.load` (swap a verified blob between rounds).
  They join [agent-api.md](../runtime/agent-api.md) when built.
