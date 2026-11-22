//! The syntax tree the parser builds, the checker types and lowering reads.

use super::Pos;
use crate::ir::Scope;

#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    pub name: String,
    pub version: i64,
    pub pos: Pos,
    pub carrier: Option<(String, Pos)>,
    pub teams: Option<(i64, Pos)>,
    /// `list tweakers`: the list's name.
    pub list: Option<(String, Pos)>,
    pub requires: Vec<(String, Pos)>,
    pub items: Vec<Item>,
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

/// What a label asks of the map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    AtLeast(i64),
    AtMost(i64),
    Exactly(i64),
    /// `n..m` members.
    Between(i64, i64),
    /// `numbered`, or `numbered a..b`.
    Numbered(Option<(i64, i64)>),
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
