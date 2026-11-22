//! The syntax tree the parser builds and lowering reads.

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
    On {
        event: String,
        args: Vec<String>,
        body: Vec<Stmt>,
        pos: Pos,
    },
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
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    /// `team n`.
    Team(Box<Expr>),
    /// `text n` or `text "..."`.
    Text(Box<Expr>),
    /// `labelled "x"`, a label value.
    Labelled(String),
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
    Labelled(String),
    /// The objective registry's records, by object handle.
    Objectives,
    Pick(Box<SetExpr>, Box<Expr>),
}
