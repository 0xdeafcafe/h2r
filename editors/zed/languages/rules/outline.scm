(header
  "rules" @context
  name: (string) @name) @item

(type_declaration
  "type" @context
  name: (identifier) @name) @item

(teams_declaration
  "teams" @context
  count: (integer) @name) @item

(uses_declaration
  "uses" @context
  name: (identifier) @name) @item

(use_declaration
  "use" @context
  path: (string) @name) @item

(about_declaration
  "about" @name) @item

(quick_declaration
  "quick" @name) @item

(settings_declaration
  "settings" @name) @item

(requires_declaration
  "requires" @name) @item

(option_declaration
  "option" @context
  name: (identifier) @name) @item

(palette_declaration
  "palette" @context
  name: (string) @name) @item

(enum_declaration
  "enum" @context
  name: (identifier) @name) @item

(var_declaration
  scope: _ @context
  "var" @context
  name: (identifier) @name) @item

(table_declaration
  "table" @context
  name: (identifier) @name) @item

(const_declaration
  "const" @context
  name: (identifier) @name) @item

(label_declaration
  "label" @context
  name: (identifier) @name) @item

(function_declaration
  "fn" @context
  name: (identifier) @name) @item

(handler_declaration
  "on" @context
  event: (identifier) @name) @item
