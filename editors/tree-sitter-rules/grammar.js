/**
 * @file Tree-sitter grammar for the h2recomp rules language (`.rules`).
 * @license WTFPL
 *
 * The language is defined by crates/formats/rules/src/lang/lex.rs (tokens)
 * and crates/formats/rules/src/lang/parse.rs (grammar); this grammar
 * follows them. It is for editors (highlighting, outline, indents), not
 * for checking: the compiler and `h2 rules lsp` report errors.
 *
 * Newlines. parse.rs ends a statement at a line break, `;` or `}`, and
 * reads a line break in a few other places. This grammar has no newline
 * token and no external scanner: whitespace, newlines included, is an
 * extra. The language needs no newline to be unambiguous, because no
 * statement or declaration can continue with a token that starts the
 * next one, apart from three cases that precedence settles:
 *
 *   - `a` then a line `(b)`: parse.rs reads two statements (a call needs
 *     its `(` on the same line); this grammar reads the call `a(b)`.
 *   - `a` then a line `[b]`: likewise, read as the index `a[b]`.
 *   - `a` then a line `-b`: both read `a - b` (parse.rs does not stop a
 *     binary operator at a line break).
 *
 * Two statements on one line without `;` parse here; parse.rs refuses
 * them ("expected the end of the line"). Enum variants may be separated
 * by spaces as well as commas and newlines.
 *
 * `settings`, `quick`, `scoring` and `about` are keywords only at the
 * start of a top-level line, where the grammar can take them; in a
 * handler they are names, as in parse.rs.
 *
 * Other places this grammar is looser than parse.rs:
 *
 *   - Label requirement counts, option range bounds and match patterns
 *     take any expression; parse.rs takes a unary expression (a count or
 *     bound) or a postfix expression (a pattern).
 *   - Comparisons chain (`a < b < c`); parse.rs takes one.
 *   - `if let x = v and c`: the `and c` is the `guard` field, as in
 *     parse.rs, because `let_condition` binds tighter than `and`.
 *   - A set argument (`players p where ...`) is accepted in any call's
 *     arguments, not only in `count`, `first` and `pick`; a bare label
 *     name (`count(goal)`) parses as an identifier.
 *   - Field and enum-variant names that are keywords (`p.team`,
 *     `Role.none`, `enum Role { none }`) parse as identifiers, because a
 *     keyword is only a keyword where the grammar can take it.
 *   - `5st` lexes as the time `5s` and the name `t`; lex.rs reads the
 *     int `5` and the name `st`. Both are errors in the language.
 */

/* eslint-disable arrow-parens, camelcase */

// Lowest first, as in parse.rs.
const PREC = {
  or: 1,
  and: 2,
  not: 3,
  compare: 4,
  add: 5,
  mul: 6,
  unary: 7,
  postfix: 8,
  call: 9,
};

/**
 * One or more `rule`, separated by commas.
 *
 * @param {Rule} rule
 * @returns {SeqRule}
 */
function commaSep1(rule) {
  return seq(rule, repeat(seq(',', rule)));
}

/**
 * Zero or more `rule`, separated by commas, with a trailing comma allowed
 * (parse.rs's lists all take one).
 *
 * @param {Rule} rule
 * @returns {ChoiceRule}
 */
function commaList(rule) {
  return optional(seq(commaSep1(rule), optional(',')));
}

module.exports = grammar({
  name: 'rules',

  word: $ => $.identifier,

  extras: $ => [/\s/, $.comment],

  rules: {
    source_file: $ => seq(
      optional($.header),
      repeat(choice($._item, ';')),
    ),

    // `rules "Name" 1`.
    header: $ => seq('rules', field('name', $.string), field('version', $.integer)),

    _item: $ => choice(
      $.carrier_declaration,
      $.teams_declaration,
      $.list_declaration,
      $.scoring_declaration,
      $.about_declaration,
      $.quick_declaration,
      $.settings_declaration,
      $.requires_declaration,
      $.option_declaration,
      $.palette_declaration,
      $.enum_declaration,
      $.var_declaration,
      $.table_declaration,
      $.const_declaration,
      $.label_declaration,
      $.function_declaration,
      $.handler_declaration,
    ),

    carrier_declaration: $ => seq('carrier', field('name', $.identifier)),

    teams_declaration: $ => seq('teams', field('count', $.integer)),

    // `list research`.
    list_declaration: $ => seq('list', field('name', $.identifier)),

    // `scoring carrier`.
    scoring_declaration: $ => seq('scoring', field('name', $.identifier)),

    // `about "No shields. 50 points to win."`.
    about_declaration: $ => seq('about', field('text', $.string)),

    // `quick score_limit, starting_weapon`: the lobby's Quick Options rows.
    quick_declaration: $ => seq('quick', commaSep1(field('mark', $.identifier))),

    // `settings { score_limit = 50; shields = none }`: one setting a line,
    // or `;` between them.
    settings_declaration: $ => seq(
      'settings',
      '{',
      repeat(choice($.setting, ';')),
      '}',
    ),

    setting: $ => seq(field('name', $.identifier), '=', field('value', $._expression)),

    // `requires player.weapons, hud.message`.
    requires_declaration: $ => seq('requires', commaSep1(field('capability', $.capability))),

    capability: $ => seq($.identifier, repeat(seq('.', $.identifier))),

    // `option name: int 1..5 = 3 "Label" "Description"`.
    option_declaration: $ => seq(
      'option',
      field('name', $.identifier),
      ':',
      field('type', $.type),
      optional(field('range', $.range)),
      '=',
      field('default', $._expression),
      optional(seq(
        field('label', $.string),
        optional(field('description', $.string)),
      )),
    ),

    // `lo..hi`: an option's range or a label's member count or numbers.
    range: $ => seq(field('start', $._expression), '..', field('end', $._expression)),

    // `palette "name" = ["tag/path", ...]`.
    palette_declaration: $ => seq(
      'palette',
      field('name', $.string),
      '=',
      '[',
      commaList(field('tag', $.string)),
      ']',
    ),

    // `enum Name { a, b }`, commas or newlines.
    enum_declaration: $ => seq(
      'enum',
      field('name', $.identifier),
      '{',
      repeat(seq(field('variant', $.identifier), optional(','))),
      '}',
    ),

    // `global var x: int = 0`, `player var`, `team var`, `object var`.
    var_declaration: $ => seq(
      field('scope', choice('global', 'player', 'team', 'object')),
      'var',
      field('name', $.identifier),
      ':',
      field('type', $.type),
      optional(seq('=', field('value', $._expression))),
    ),

    // `table name: (c1: type, c2: type) = [(v, v), ...]`.
    table_declaration: $ => seq(
      'table',
      field('name', $.identifier),
      ':',
      '(',
      commaList(field('column', $.table_column)),
      ')',
      '=',
      '[',
      commaList(field('row', $.table_row)),
      ']',
    ),

    table_column: $ => seq(field('name', $.identifier), ':', field('type', $.type)),

    table_row: $ => seq('(', commaList($._expression), ')'),

    const_declaration: $ => seq(
      'const',
      field('name', $.identifier),
      '=',
      field('value', $._expression),
    ),

    // `label name: kind [requirements] [= "map text"]`.
    label_declaration: $ => seq(
      'label',
      field('name', $.identifier),
      ':',
      field('kind', $.type),
      repeat(field('requirement', $.requirement)),
      optional(seq('=', field('text', $.string))),
    ),

    // `at least n`, `at most n`, `exactly n`, `numbered`, `numbered a..b`,
    // `per team`, `n..m`. Right associative so `numbered 0..1` takes its
    // range, as parse.rs does.
    // A count is a number or a constant: an expression could run on into
    // the next declaration (`palette ...`).
    _count: $ => choice($.integer, $.identifier),

    requirement: $ => prec.right(choice(
      seq('at', 'least', field('count', $._count)),
      seq('at', 'most', field('count', $._count)),
      seq('exactly', field('count', $._count)),
      seq('numbered', optional(field('range', $.count_range))),
      seq('per', 'team'),
      field('range', $.count_range),
    )),

    // `0..2`, `1..max_hills`: a label's numbers or counts.
    count_range: $ => seq(field('start', $._count), '..', field('end', $._count)),

    // `fn name(a: type) { ... }` or `fn name(a: type) -> type = expr`.
    function_declaration: $ => seq(
      'fn',
      field('name', $.identifier),
      field('parameters', $.parameters),
      optional(seq('->', field('result', $.type))),
      choice(
        seq('=', field('value', $._expression)),
        field('body', $.block),
      ),
    ),

    parameters: $ => seq('(', commaList($.parameter), ')'),

    parameter: $ => seq(field('name', $.identifier), ':', field('type', $.type)),

    // `on event(args) { ... }`; `on enter(capture z, o)` picks by label.
    handler_declaration: $ => seq(
      'on',
      field('event', $.identifier),
      optional(field('parameters', $.event_parameters)),
      field('body', $.block),
    ),

    event_parameters: $ => seq('(', commaList($.event_parameter), ')'),

    event_parameter: $ => choice(
      seq(field('label', $.identifier), field('name', $.identifier)),
      field('name', $.identifier),
    ),

    // `int`, `player?`.
    type: $ => seq(field('name', $.identifier), optional('?')),

    block: $ => seq('{', repeat(choice($._statement, ';')), '}'),

    _statement: $ => choice(
      $.let_statement,
      $.assignment_statement,
      $.if_statement,
      $.for_statement,
      $.match_statement,
      $.expression_statement,
    ),

    let_statement: $ => seq(
      'let',
      field('name', $.identifier),
      optional(seq(':', field('type', $.type))),
      '=',
      field('value', $._expression),
    ),

    assignment_statement: $ => seq(
      field('target', $._expression),
      '=',
      field('value', $._expression),
    ),

    expression_statement: $ => $._expression,

    // `if cond { } else if cond { } else { }`, or
    // `if let k = killer and cond { }`.
    if_statement: $ => prec.right(seq(
      'if',
      choice(
        field('condition', $._expression),
        seq(
          field('condition', $.let_condition),
          optional(seq('and', field('guard', $._expression))),
        ),
      ),
      field('consequence', $.block),
      optional(field('alternative', $.else_clause)),
    )),

    // Binds tighter than `and`, so the value stops before it (parse.rs
    // reads the value at the level of `not`).
    let_condition: $ => prec(PREC.not, seq(
      'let',
      field('name', $.identifier),
      '=',
      field('value', $._expression),
    )),

    else_clause: $ => seq('else', choice($.if_statement, $.block)),

    // `for each players p where cond { }`, `for each pick(players, n) p { }`.
    for_statement: $ => seq(
      'for',
      'each',
      field('set', choice($.builtin_set, $.identifier, $.call_expression)),
      field('binder', $.identifier),
      optional(seq('where', field('filter', $._expression))),
      field('body', $.block),
    ),

    builtin_set: _ => choice('players', 'teams', 'objectives'),

    // `match value { Role.a, Role.b { } else { } }`.
    match_statement: $ => seq(
      'match',
      field('value', $._expression),
      '{',
      repeat(choice($.match_arm, ';')),
      '}',
    ),

    match_arm: $ => seq(
      choice(
        commaSep1(field('pattern', $._expression)),
        'else',
      ),
      field('body', $.block),
    ),

    _expression: $ => choice(
      $.binary_expression,
      $.unary_expression,
      $.is_expression,
      $.field_expression,
      $.index_expression,
      $._primary,
    ),

    // What `team` and `text` take (parse.rs's `primary`).
    _primary: $ => choice(
      $.call_expression,
      $.parenthesized_expression,
      $.team_expression,
      $.text_expression,
      $.budget_expression,
      $.palette_expression,
      $.identifier,
      $.integer,
      $.fixed,
      $.time,
      $.string,
      $.true,
      $.false,
      $.none,
    ),

    binary_expression: $ => {
      const table = [
        [PREC.or, 'or'],
        [PREC.and, 'and'],
        [PREC.compare, choice('==', '!=', '<', '<=', '>', '>=')],
        [PREC.add, choice('+', '-')],
        [PREC.mul, choice('*', '/', '%')],
      ];
      return choice(...table.map(([p, op]) => prec.left(p, seq(
        field('left', $._expression),
        // @ts-ignore
        field('operator', op),
        field('right', $._expression),
      ))));
    },

    unary_expression: $ => choice(
      prec(PREC.not, seq(field('operator', 'not'), field('operand', $._expression))),
      prec(PREC.unary, seq(field('operator', '-'), field('operand', $._expression))),
    ),

    // `x is goal`: membership of a label.
    is_expression: $ => prec.left(PREC.compare, seq(
      field('left', $._expression),
      'is',
      field('label', $.identifier),
    )),

    field_expression: $ => prec(PREC.postfix, seq(
      field('object', $._expression),
      '.',
      field('field', $.identifier),
    )),

    index_expression: $ => prec(PREC.postfix, seq(
      field('object', $._expression),
      '[',
      field('index', $._expression),
      ']',
    )),

    // Only a name is called (parse.rs's `primary`).
    call_expression: $ => prec(PREC.call, seq(
      field('function', $.identifier),
      field('arguments', $.arguments),
    )),

    arguments: $ => seq('(', commaList(choice($._expression, $.set_expression)), ')'),

    // A set argument of `count`, `first` or `pick`: `players`,
    // `players p`, `players p where cond`, `goal g where cond`,
    // `pick(players, 2) p`. A lone label name is an identifier.
    set_expression: $ => choice(
      seq(
        field('base', $.builtin_set),
        optional(field('binder', $.identifier)),
        optional(seq('where', field('filter', $._expression))),
      ),
      seq(
        field('base', choice($.identifier, $.call_expression)),
        field('binder', $.identifier),
        optional(seq('where', field('filter', $._expression))),
      ),
      seq(
        field('base', choice($.identifier, $.call_expression)),
        'where',
        field('filter', $._expression),
      ),
    ),

    parenthesized_expression: $ => seq('(', $._expression, ')'),

    // `team 0`.
    team_expression: $ => seq('team', field('value', $._primary)),

    // `text 3`, `text "..."`.
    text_expression: $ => seq('text', field('value', $._primary)),

    // `budget 20`, a `grant` argument.
    budget_expression: $ => seq('budget', field('value', $.integer)),

    // `palette "vip"`, a `grant` argument.
    palette_expression: $ => seq('palette', field('name', $.string)),

    identifier: _ => /[A-Za-z_][A-Za-z0-9_]*/,

    integer: _ => /\d+/,

    // 16.16 fixed: `1.5`.
    fixed: _ => /\d+\.\d+/,

    // `5s`, `300t`, `1.5s`.
    time: _ => /\d+(\.\d+)?[st]/,

    true: _ => 'true',

    false: _ => 'false',

    none: _ => 'none',

    // A string ends at its line. A message text holds holes (`{k}`), with
    // `{{` and `}}` for a brace; an unpaired brace is plain content here
    // (the checker reports it).
    string: $ => seq(
      '"',
      repeat(choice(
        $.string_content,
        $.escape_sequence,
        $.interpolation,
        alias($._lone_brace, $.string_content),
      )),
      '"',
    ),

    string_content: _ => token.immediate(prec(1, /[^"\\{}\n]+/)),

    escape_sequence: _ => token.immediate(choice(/\\["\\n]/, '{{', '}}')),

    // `{name}`: one hole, as a single token.
    interpolation: _ => token.immediate(/\{[^{}"\\\n]*\}/),

    _lone_brace: _ => token.immediate(prec(-1, /[{}]/)),

    comment: _ => token(seq('//', /[^\n]*/)),
  },
});
