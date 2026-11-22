# tree-sitter-rules

A [tree-sitter](https://tree-sitter.github.io/) grammar for h2recomp's
game type scripts (`.rules`). Zed's extension (`editors/zed`) uses it for
highlighting, brackets, indents and the outline; `h2 rules lsp` does the
checking.

The language is defined by `crates/formats/rules/src/lang/lex.rs` and
`crates/formats/rules/src/lang/parse.rs`. The comment at the top of
`grammar.js` lists where this grammar reads differently: it has no newline
token, so `a` then a line `(b)` reads as a call, and a few places take any
expression where the compiler takes a narrower one.

## Generate and test

`src/` is generated and committed (`parser.c`, `grammar.json`,
`node-types.json`, `tree_sitter/`), so Zed can build the grammar from a
commit. After changing `grammar.js`:

```sh
cd editors/tree-sitter-rules
npx tree-sitter-cli@0.25 generate   # or `tree-sitter generate` with the CLI installed
npx tree-sitter-cli@0.25 test       # test/corpus/*.txt
```

Commit `grammar.js` and `src/` together. Then put the commit's SHA in
`editors/zed/extension.toml` (`[grammars.rules]`, `rev`).

To see a script's tree or its highlights:

```sh
tree-sitter parse ../../crates/formats/rules/src/builtin/vip.rules
tree-sitter highlight ../../crates/formats/rules/src/builtin/vip.rules
```

`queries/highlights.scm` is ordered for the tree-sitter CLI (the first
pattern that captures a node wins). Zed's copy,
`editors/zed/languages/rules/highlights.scm`, has the same patterns in
the reverse order, as Zed lets the last one win. Change both.

## Nodes

Declarations: `header`, `carrier_declaration`, `teams_declaration`,
`list_declaration`, `scoring_declaration`, `about_declaration`,
`quick_declaration`, `settings_declaration` (`setting`),
`requires_declaration` (`capability`),
`option_declaration` (`range`), `palette_declaration`, `enum_declaration`,
`var_declaration`, `table_declaration` (`table_column`, `table_row`),
`const_declaration`, `label_declaration` (`requirement`),
`function_declaration` (`parameters`, `parameter`), `handler_declaration`
(`event_parameters`, `event_parameter`), `type`.

Statements: `block`, `let_statement`, `assignment_statement`,
`expression_statement`, `if_statement` (`let_condition`, `else_clause`),
`for_statement`, `match_statement` (`match_arm`).

Expressions: `binary_expression`, `unary_expression`, `is_expression`,
`field_expression`, `index_expression`, `call_expression` (`arguments`,
`set_expression`, `builtin_set`), `parenthesized_expression`,
`team_expression`, `text_expression`, `budget_expression`,
`palette_expression`.

Leaves: `identifier`, `integer`, `fixed`, `time`, `string`
(`string_content`, `escape_sequence`, `interpolation`), `true`, `false`,
`none`, `comment`. A message hole (`{k}`) is one `interpolation` token,
braces included; `{{` and `}}` are `escape_sequence`s.
