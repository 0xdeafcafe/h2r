; Highlights for the rules language, for the tree-sitter CLI
; (`tree-sitter highlight`), where the first pattern that captures a node
; wins: specific patterns come first, `(identifier) @variable` last.
; editors/zed/languages/rules/highlights.scm holds the same patterns in
; Zed's order (the last pattern wins).

(comment) @comment

; Literals

(escape_sequence) @string.escape

(interpolation) @embedded

(string) @string

[
  (integer)
  (fixed)
  (time)
] @number

[
  (true)
  (false)
] @boolean

(none) @constant.builtin

(builtin_set) @constant.builtin

; Declarations

(function_declaration
  name: (identifier) @function)

(handler_declaration
  event: (identifier) @function)

(call_expression
  function: (identifier) @function.call)

(parameter
  name: (identifier) @variable.parameter)

(event_parameter
  label: (identifier) @label)

(event_parameter
  name: (identifier) @variable.parameter)

(type
  name: (identifier) @type)

(enum_declaration
  name: (identifier) @type)

(enum_declaration
  variant: (identifier) @constant)

(label_declaration
  name: (identifier) @label)

(is_expression
  label: (identifier) @label)

(const_declaration
  name: (identifier) @constant)

(option_declaration
  name: (identifier) @constant)

(carrier_declaration
  name: (identifier) @constant)

(list_declaration
  name: (identifier) @constant)

(capability
  (identifier) @constant)

(table_column
  name: (identifier) @property)

(field_expression
  field: (identifier) @property)

; Keywords

[
  "rules"
  "carrier"
  "teams"
  "list"
  "requires"
  "option"
  "palette"
  "enum"
  "global"
  "player"
  "team"
  "object"
  "var"
  "table"
  "const"
  "label"
  "fn"
  "on"
  "let"
  "if"
  "else"
  "for"
  "each"
  "where"
  "match"
  "is"
  "text"
  "budget"
  "and"
  "or"
  "not"
  "at"
  "least"
  "most"
  "exactly"
  "numbered"
  "per"
] @keyword

; Operators and punctuation

[
  "=="
  "!="
  "<"
  "<="
  ">"
  ">="
  "+"
  "-"
  "*"
  "/"
  "%"
  "="
  "->"
  ".."
  "?"
] @operator

[
  "("
  ")"
  "["
  "]"
  "{"
  "}"
] @punctuation.bracket

[
  ","
  ":"
  ";"
  "."
] @punctuation.delimiter

(identifier) @variable
