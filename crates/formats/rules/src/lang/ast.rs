//! The syntax tree the parser builds, the checker types and lowering reads.

use super::Pos;
use crate::ir::Scope;

#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    pub name: String,
    pub version: i64,
    pub pos: Pos,
    /// `type slayer`: the game type's word.
    pub game_type: Option<(String, Pos)>,
    pub teams: Option<(i64, Pos)>,
    /// `points "Captures"`: what a point is.
    pub points: Option<(String, Pos)>,
    /// `uses slayer`: the built-in game whose rules the script plays.
    pub uses: Option<(String, Pos)>,
    /// Header lines the language no longer has (`carrier slayer`, `list
    /// party`, `scoring carrier`): the word, the word after it and the
    /// line's position. The checker refuses each, naming the line to write
    /// instead.
    pub retired: Vec<(String, String, Pos)>,
    /// `about "..."`: the description.
    pub about: Option<(String, Pos)>,
    /// `quick a, b, c`: the names, and the line's position.
    pub quick: Option<(Vec<(String, Pos)>, Pos)>,
    /// `settings { name = value ... }`.
    pub settings: Option<SettingsBlock>,
    pub requires: Vec<(String, Pos)>,
    /// `hud name: kind at slot`, in order: a widget's number is its index.
    /// Each also leaves a `const` of the same name in `items`, so a call
    /// names a widget by it.
    pub huds: Vec<HudDecl>,
    /// `waypoint name [at slot]`, in order, as `huds` are.
    pub waypoints: Vec<WaypointDecl>,
    /// `section "Waves"`: where the script is split, for the outline and
    /// folding. It compiles to nothing.
    pub sections: Vec<(String, Pos)>,
    pub items: Vec<Item>,
}

/// `hud <name>: <kind> at <slot>`.
#[derive(Clone, Debug, PartialEq)]
pub struct HudDecl {
    pub name: String,
    /// `text`, `banner` or `meter`; the checker refuses another.
    pub kind: String,
    /// The kit element the widget draws as (`koth.hill`).
    pub slot: String,
    pub pos: Pos,
}

/// `waypoint <name> [at <slot>]`.
#[derive(Clone, Debug, PartialEq)]
pub struct WaypointDecl {
    pub name: String,
    pub slot: Option<String>,
    pub pos: Pos,
}

/// The `settings` block: one `name = value` a line.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingsBlock {
    pub lines: Vec<SettingLine>,
    pub pos: Pos,
}

/// One line of the `settings` block.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingLine {
    pub name: String,
    pub value: Expr,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeName {
    pub name: String,
    /// `player?`: the value may be `none`.
    pub optional: bool,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Option(OptionDecl),
    Palette {
        name: String,
        tags: Vec<String>,
        pos: Pos,
    },
    Enum {
        name: String,
        variants: Vec<String>,
        pos: Pos,
    },
    Var {
        scope: Scope,
        name: String,
        ty: TypeName,
        init: Option<Expr>,
        pos: Pos,
    },
    Table {
        name: String,
        columns: Vec<(String, TypeName)>,
        rows: Vec<Vec<Expr>>,
        pos: Pos,
    },
    Fn(FnDecl),
    Label(LabelDecl),
    /// `const last_phase = 2`: a value the checker folds, usable wherever
    /// a literal is.
    Const {
        name: String,
        value: Expr,
        pos: Pos,
    },
    On {
        event: String,
        /// The argument names, in the event's order; trailing ones may be
        /// left out.
        args: Vec<String>,
        /// `on enter(capture z, o)`: the label and the argument that names
        /// its member (`z`, by index in `args`). In `on enter(goal, o)` the
        /// label is the argument's own name, which the checker tells apart.
        label: Option<(String, usize)>,
        body: Vec<Stmt>,
        pos: Pos,
    },
}

/// `label <name>: <kind> [requirements] [= "<map text>"]`.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelDecl {
    pub name: String,
    /// `object` or `zone` (either may take `?`), `objects`, `zones` or
    /// `spawns`.
    pub kind: TypeName,
    pub requirements: Vec<(Requirement, Pos)>,
    /// The map's label text, when it is not the name.
    pub text: Option<String>,
    pub pos: Pos,
}

/// What a label asks of the map. Each count is a constant expression: a
/// literal or a `const`.
#[derive(Clone, Debug, PartialEq)]
pub enum Requirement {
    AtLeast(Expr),
    AtMost(Expr),
    Exactly(Expr),
    /// `n..m` members.
    Between(Expr, Expr),
    /// `numbered`, or `numbered a..b`.
    Numbered(Option<(Expr, Expr)>),
    PerTeam,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OptionDecl {
    pub name: String,
    pub ty: TypeName,
    pub range: Option<(Expr, Expr)>,
    pub default: Expr,
    pub label: Option<String>,
    pub description: Option<String>,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<(String, TypeName)>,
    /// `-> bool`: the type of a value function.
    pub result: Option<TypeName>,
    /// `= <expression>`: a value function's one expression, inlined at
    /// each call. A block function has none and a `body`.
    pub value: Option<Expr>,
    pub body: Vec<Stmt>,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Let {
        name: String,
        /// `let heir: player? = none`.
        ty: Option<TypeName>,
        value: Expr,
        pos: Pos,
    },
    Assign {
        target: Expr,
        value: Expr,
        pos: Pos,
    },
    Expr(Expr),
    If {
        cond: Expr,
        then: Vec<Stmt>,
        els: Vec<Stmt>,
    },
    /// `if let x = value [and cond] { } else { }`: in `cond` and `then`,
    /// `x` is the value, which is not `none`.
    IfLet {
        name: String,
        value: Expr,
        cond: Option<Expr>,
        then: Vec<Stmt>,
        els: Vec<Stmt>,
        pos: Pos,
    },
    ForEach {
        set: SetExpr,
        binder: String,
        body: Vec<Stmt>,
        pos: Pos,
    },
    /// `match <enum value> { Role.vip { } else { } }`: one arm per
    /// variant, or an `else` arm.
    Match {
        value: Expr,
        arms: Vec<MatchArm>,
        els: Option<Vec<Stmt>>,
        pos: Pos,
    },
}

/// An arm of a `match`: the variants it takes (`Role.vip`, `Role.guard`),
/// each an enum variant expression, and its block.
#[derive(Clone, Debug, PartialEq)]
pub struct MatchArm {
    pub patterns: Vec<Expr>,
    pub body: Vec<Stmt>,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Expr {
    pub kind: ExprKind,
    pub pos: Pos,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ExprKind {
    Int(i64),
    Fixed(i32),
    /// `5s`, `300t`, `1.5s`: the ticks, or `None` when the literal is not a
    /// whole number of ticks; and the literal as written.
    Time(Option<i64>, String),
    Str(String),
    Bool(bool),
    None,
    Name(String),
    Field(Box<Expr>, String),
    /// `table[i]`, `capture[n]`, `flag_spawn[t]`.
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `team n`.
    Team(Box<Expr>),
    /// `text n` or `text "..."`.
    Text(Box<Expr>),
    /// `x is goal`: whether `x` is a member of the label; the label's
    /// name and its position.
    Is(Box<Expr>, String, Pos),
    /// `budget n`, a `grant` argument.
    Budget(i64),
    /// `palette "x"`, a `grant` argument.
    Palette(String),
    /// A set argument of `count`, `first` or `pick`.
    Set(Box<SetExpr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct SetExpr {
    pub base: SetBaseExpr,
    /// The member's name in a set argument (`count(players p where ...)`);
    /// a `for each` names it in its own binder.
    pub binder: Option<String>,
    pub filter: Option<Expr>,
    pub pos: Pos,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SetBaseExpr {
    Players,
    Teams,
    /// A declared label, or a function's set parameter, by name.
    Label(String),
    /// The objective registry's records, by object handle.
    Objectives,
    Pick(Box<SetExpr>, Box<Expr>),
}
