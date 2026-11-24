; Highlights for the rules language in Zed, where the last pattern that
; captures a node wins: `(identifier) @variable` first, specific patterns
; after. editors/tree-sitter-rules/queries/highlights.scm holds the same
; patterns in the tree-sitter CLI's order.

(identifier) @variable

; Keywords

[
  "rules"
  "type"
  "teams"
  "uses"
  "use"
  "points"
  "about"
  "quick"
  "settings"
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

; Declarations

(field_expression
  field: (identifier) @property)

(table_column
  name: (identifier) @property)

(capability
  (identifier) @constant)

(uses_declaration
  name: (identifier) @constant)

(setting
  value: (identifier) @constant)

(setting
  name: (identifier) @property)

(quick_declaration
  mark: (identifier) @property)

(type_declaration
  name: (identifier) @constant)

(option_declaration
  name: (identifier) @constant)

(const_declaration
  name: (identifier) @constant)

(is_expression
  label: (identifier) @label)

(label_declaration
  name: (identifier) @label)

(enum_declaration
  variant: (identifier) @variant)

(enum_declaration
  name: (identifier) @enum)

(type
  name: (identifier) @type)

(event_parameter
  name: (identifier) @variable.parameter)

(event_parameter
  label: (identifier) @label)

(parameter
  name: (identifier) @variable.parameter)

(call_expression
  function: (identifier) @function.call)

(handler_declaration
  event: (identifier) @function)

(function_declaration
  name: (identifier) @function)

; Literals

(builtin_set) @constant.builtin

(none) @constant.builtin

[
  (true)
  (false)
] @boolean

[
  (integer)
  (fixed)
  (time)
] @number

(string) @string

(interpolation) @embedded

(escape_sequence) @string.escape

(comment) @comment
