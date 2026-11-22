//! The parser: tokens to a [`Script`].
//!
//! A statement ends at `;`, at `}` or at a line break. Precedence, lowest
//! first: `or`, `and`, `not`, comparisons, `+ -`, `* / %`, unary `-`, then
//! `.field`, `[index]` and calls.

use super::ast::*;
use super::lex::{Tok, Token};
use super::{CompileError, Pos};
use crate::ir::Scope;

/// Nesting deeper than this is refused, so no source can exhaust the stack.
const MAX_DEPTH: u32 = 64;

pub(crate) const KEYWORDS: &[&str] = &[
    "rules", "carrier", "teams", "list", "requires", "option", "palette", "enum", "global", "var",
    "table", "fn", "on", "if", "else", "for", "each", "where", "let", "and", "or", "not", "true",
    "false", "none", "labelled", "text", "budget",
];

pub fn parse(tokens: &[Token]) -> Result<Script, CompileError> {
    let mut p = Parser::new(tokens, false);
    p.script()
}

/// Parses on past errors, for the editor tooling (`crate::ide`). A bad
/// statement is skipped to the end of its line, a bad declaration to the
/// next line that starts one, and a block left open ends at the next
/// declaration or the end of the file. The script holds everything that
/// parsed; every error is returned, in order. A source with no errors
/// parses as [`parse`] parses it.
pub fn parse_recovering(tokens: &[Token]) -> (Script, Vec<CompileError>) {
    let mut p = Parser::new(tokens, true);
    let script = match p.script() {
        Ok(s) => s,
        // Recovery returns no error; this is the empty script.
        Err(e) => {
            p.errors.push(e);
            Script {
                name: String::new(),
                version: 0,
                pos: Pos { line: 1, col: 1 },
                carrier: None,
                teams: None,
                list: None,
                requires: Vec::new(),
                items: Vec::new(),
            }
        }
    };
    (script, p.errors)
}

struct Parser<'a> {
    t: &'a [Token],
    at: usize,
    depth: u32,
    /// Record errors and go on instead of returning the first.
    recover: bool,
    errors: Vec<CompileError>,
}

type R<T> = Result<T, CompileError>;

impl<'a> Parser<'a> {
    fn new(t: &'a [Token], recover: bool) -> Parser<'a> {
        Parser {
            t,
            at: 0,
            depth: 0,
            recover,
            errors: Vec::new(),
        }
    }

    fn at_eof(&self) -> bool {
        self.peek().tok == Tok::Eof
    }

    /// Whether the next token starts a declaration or a header line.
    fn is_item_start(&self) -> bool {
        if self.scope_word().is_some() {
            return true;
        }
        matches!(&self.peek().tok, Tok::Ident(s) if matches!(s.as_str(),
            "rules" | "carrier" | "teams" | "list" | "requires" | "option" | "palette" | "enum" | "table" | "fn" | "on"))
    }

    /// Recovery from a bad declaration: on to the next line that starts
    /// one outside braces, or the end.
    fn sync_item(&mut self) {
        let mut depth = 0u32;
        while !self.at_eof() {
            if depth == 0 && self.peek().nl && self.is_item_start() {
                return;
            }
            if self.is_punct("{") {
                depth += 1;
            } else if self.is_punct("}") {
                depth = depth.saturating_sub(1);
            }
            self.bump();
        }
    }

    /// Recovery from a bad statement: on to the next line or the `}` that
    /// ends the block, outside nested braces.
    fn sync_stmt(&mut self) {
        let mut depth = 0u32;
        while !self.at_eof() {
            if depth == 0 && (self.peek().nl || self.is_punct("}")) {
                return;
            }
            if self.is_punct("{") {
                depth += 1;
            } else if self.is_punct("}") {
                depth -= 1;
            }
            self.bump();
        }
    }

    fn peek(&self) -> &Token {
        &self.t[self.at.min(self.t.len() - 1)]
    }

    fn peek2(&self) -> &Token {
        &self.t[(self.at + 1).min(self.t.len() - 1)]
    }

    fn pos(&self) -> Pos {
        self.peek().pos
    }

    fn bump(&mut self) -> Token {
        let t = self.peek().clone();
        if self.at < self.t.len() - 1 {
            self.at += 1;
        }
        t
    }

    fn is_punct(&self, p: &str) -> bool {
        matches!(&self.peek().tok, Tok::Punct(q) if *q == p)
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(&self.peek().tok, Tok::Ident(s) if s == w)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if self.is_punct(p) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, w: &str) -> bool {
        if self.is_word(w) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, p: &str) -> R<()> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(self.unexpected(&format!("`{p}`")))
        }
    }

    fn expect_word(&mut self, w: &str) -> R<()> {
        if self.eat_word(w) {
            Ok(())
        } else {
            Err(self.unexpected(&format!("`{w}`")))
        }
    }

    fn unexpected(&self, want: &str) -> CompileError {
        let found = match &self.peek().tok {
            Tok::Ident(s) => format!("`{s}`"),
            Tok::Int(n) => format!("{n}"),
            Tok::Fixed(_) => "a fixed number".to_string(),
            Tok::Str(s) => format!("{s:?}"),
            Tok::Punct(p) => format!("`{p}`"),
            Tok::Eof => "the end of the file".to_string(),
        };
        CompileError::at(self.pos(), format!("expected {want}, found {found}"))
    }

    /// A name that is not a keyword.
    fn name(&mut self) -> R<String> {
        match &self.peek().tok {
            Tok::Ident(s) if !KEYWORDS.contains(&s.as_str()) => {
                let s = s.clone();
                self.bump();
                Ok(s)
            }
            _ => Err(self.unexpected("a name")),
        }
    }

    /// Any word, keywords included (field and enum variant names).
    fn word(&mut self) -> R<String> {
        match &self.peek().tok {
            Tok::Ident(s) => {
                let s = s.clone();
                self.bump();
                Ok(s)
            }
            _ => Err(self.unexpected("a name")),
        }
    }

    fn string(&mut self) -> R<String> {
        match &self.peek().tok {
            Tok::Str(s) => {
                let s = s.clone();
                self.bump();
                Ok(s)
            }
            _ => Err(self.unexpected("a string")),
        }
    }

    fn int(&mut self) -> R<i64> {
        match self.peek().tok {
            Tok::Int(n) => {
                self.bump();
                Ok(n)
            }
            _ => Err(self.unexpected("a number")),
        }
    }

    fn ty(&mut self) -> R<TypeName> {
        let pos = self.pos();
        Ok(TypeName {
            name: self.word()?,
            pos,
        })
    }

    fn enter(&mut self) -> R<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(CompileError::at(self.pos(), "nested too deeply"));
        }
        Ok(())
    }

    fn leave(&mut self) {
        self.depth -= 1;
    }

    fn end_of_line(&mut self) -> R<()> {
        if self.eat_punct(";") || self.peek().nl || self.is_punct("}") || self.peek().tok == Tok::Eof {
            Ok(())
        } else {
            Err(self.unexpected("the end of the line"))
        }
    }

    fn script(&mut self) -> R<Script> {
        let pos = self.pos();
        let mut script = Script {
            name: String::new(),
            version: 0,
            pos,
            carrier: None,
            teams: None,
            list: None,
            requires: Vec::new(),
            items: Vec::new(),
        };
        match self.header() {
            Ok((name, version)) => {
                script.name = name;
                script.version = version;
            }
            Err(e) if self.recover => {
                self.errors.push(e);
                self.sync_item();
            }
            Err(e) => return Err(e),
        }
        loop {
            while self.eat_punct(";") {}
            if self.at_eof() {
                break;
            }
            let (at, depth) = (self.at, self.depth);
            match self.item(&mut script) {
                Ok(()) => {}
                Err(e) if self.recover => {
                    self.errors.push(e);
                    self.depth = depth;
                    if self.at == at {
                        self.bump();
                    }
                    self.sync_item();
                }
                Err(e) => return Err(e),
            }
        }
        Ok(script)
    }

    /// `rules "name" version`.
    fn header(&mut self) -> R<(String, i64)> {
        self.expect_word("rules")?;
        let name = self.string()?;
        let version = self.int()?;
        self.end_of_line()?;
        Ok((name, version))
    }

    /// One header line, declaration or handler, to the end of its line.
    fn item(&mut self, script: &mut Script) -> R<()> {
        let pos = self.pos();
        if self.eat_word("carrier") {
            if script.carrier.is_some() {
                return Err(CompileError::at(pos, "the carrier is named twice"));
            }
            script.carrier = Some((self.word()?, pos));
        } else if self.eat_word("teams") {
            if script.teams.is_some() {
                return Err(CompileError::at(pos, "teams is given twice"));
            }
            script.teams = Some((self.int()?, pos));
        } else if self.eat_word("list") {
            if script.list.is_some() {
                return Err(CompileError::at(pos, "the list is named twice"));
            }
            script.list = Some((self.word()?, pos));
        } else if self.eat_word("requires") {
            loop {
                let at = self.pos();
                let mut cap = self.word()?;
                while self.eat_punct(".") {
                    cap.push('.');
                    cap.push_str(&self.word()?);
                }
                script.requires.push((cap, at));
                if !self.eat_punct(",") {
                    break;
                }
            }
        } else if self.eat_word("option") {
            script.items.push(Item::Option(self.option(pos)?));
        } else if self.eat_word("palette") {
            let name = self.string()?;
            self.expect_punct("=")?;
            self.expect_punct("[")?;
            let mut tags = Vec::new();
            while !self.is_punct("]") {
                tags.push(self.string()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct("]")?;
            script.items.push(Item::Palette { name, tags, pos });
        } else if self.eat_word("enum") {
            let name = self.name()?;
            self.expect_punct("{")?;
            let mut variants = Vec::new();
            while !self.is_punct("}") {
                variants.push(self.word()?);
                if !self.eat_punct(",") && !self.peek().nl && !self.is_punct("}") {
                    return Err(self.unexpected("`,` or `}`"));
                }
            }
            self.expect_punct("}")?;
            script.items.push(Item::Enum {
                name,
                variants,
                pos,
            });
        } else if let Some(scope) = self.scope_word() {
            self.bump();
            self.expect_word("var")?;
            let name = self.name()?;
            self.expect_punct(":")?;
            let ty = self.ty()?;
            let init = if self.eat_punct("=") {
                Some(self.expr()?)
            } else {
                None
            };
            script.items.push(Item::Var {
                scope,
                name,
                ty,
                init,
                pos,
            });
        } else if self.eat_word("table") {
            script.items.push(self.table(pos)?);
        } else if self.eat_word("fn") {
            let name = self.name()?;
            self.expect_punct("(")?;
            let mut params = Vec::new();
            while !self.is_punct(")") {
                let n = self.name()?;
                self.expect_punct(":")?;
                params.push((n, self.ty()?));
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct(")")?;
            let body = self.block()?;
            script.items.push(Item::Fn(FnDecl {
                name,
                params,
                body,
                pos,
            }));
        } else if self.eat_word("on") {
            let event = self.word()?;
            let mut args = Vec::new();
            if self.eat_punct("(") {
                while !self.is_punct(")") {
                    args.push(self.name()?);
                    if !self.eat_punct(",") {
                        break;
                    }
                }
                self.expect_punct(")")?;
            }
            let body = self.block()?;
            script.items.push(Item::On {
                event,
                args,
                body,
                pos,
            });
        } else {
            return Err(self.unexpected("a declaration or handler"));
        }
        self.end_of_line()
    }

    /// `global`, `player`, `team` or `object` followed by `var`.
    fn scope_word(&self) -> Option<Scope> {
        let next_is_var = matches!(&self.peek2().tok, Tok::Ident(s) if s == "var");
        if !next_is_var {
            return None;
        }
        match &self.peek().tok {
            Tok::Ident(s) => match s.as_str() {
                "global" => Some(Scope::Global),
                "player" => Some(Scope::Player),
                "team" => Some(Scope::Team),
                "object" => Some(Scope::Object),
                _ => None,
            },
            _ => None,
        }
    }

    fn option(&mut self, pos: Pos) -> R<OptionDecl> {
        let name = self.name()?;
        self.expect_punct(":")?;
        let ty = self.ty()?;
        let range = if self.is_punct("=") {
            None
        } else {
            let lo = self.unary()?;
            self.expect_punct("..")?;
            let hi = self.unary()?;
            Some((lo, hi))
        };
        self.expect_punct("=")?;
        let default = self.expr()?;
        let mut label = None;
        let mut description = None;
        if matches!(self.peek().tok, Tok::Str(_)) && !self.peek().nl {
            label = Some(self.string()?);
            if matches!(self.peek().tok, Tok::Str(_)) && !self.peek().nl {
                description = Some(self.string()?);
            }
        }
        Ok(OptionDecl {
            name,
            ty,
            range,
            default,
            label,
            description,
            pos,
        })
    }

    fn table(&mut self, pos: Pos) -> R<Item> {
        let name = self.name()?;
        self.expect_punct(":")?;
        self.expect_punct("(")?;
        let mut columns = Vec::new();
        while !self.is_punct(")") {
            let n = self.word()?;
            self.expect_punct(":")?;
            columns.push((n, self.ty()?));
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct(")")?;
        self.expect_punct("=")?;
        self.expect_punct("[")?;
        let mut rows = Vec::new();
        while !self.is_punct("]") {
            self.expect_punct("(")?;
            let mut row = Vec::new();
            while !self.is_punct(")") {
                row.push(self.expr()?);
                if !self.eat_punct(",") {
                    break;
                }
            }
            self.expect_punct(")")?;
            rows.push(row);
            if !self.eat_punct(",") {
                break;
            }
        }
        self.expect_punct("]")?;
        Ok(Item::Table {
            name,
            columns,
            rows,
            pos,
        })
    }

    fn block(&mut self) -> R<Vec<Stmt>> {
        self.enter()?;
        self.expect_punct("{")?;
        let mut out = Vec::new();
        loop {
            while self.eat_punct(";") {}
            if self.eat_punct("}") {
                break;
            }
            if self.peek().tok == Tok::Eof {
                let e = self.unexpected("`}`");
                if !self.recover {
                    return Err(e);
                }
                self.errors.push(e);
                break;
            }
            // A block left open ends where the next declaration starts.
            if self.recover && self.peek().nl && self.is_item_start() {
                let e = self.unexpected("`}`");
                self.errors.push(e);
                break;
            }
            let (at, depth) = (self.at, self.depth);
            let result = match self.stmt() {
                Ok(s) => {
                    out.push(s);
                    self.end_of_line()
                }
                Err(e) => Err(e),
            };
            if let Err(e) = result {
                if !self.recover {
                    return Err(e);
                }
                self.errors.push(e);
                self.depth = depth;
                if self.at == at {
                    self.bump();
                }
                self.sync_stmt();
            }
        }
        self.leave();
        Ok(out)
    }

    fn stmt(&mut self) -> R<Stmt> {
        let pos = self.pos();
        if self.eat_word("let") {
            let name = self.name()?;
            self.expect_punct("=")?;
            let value = self.expr()?;
            return Ok(Stmt::Let { name, value, pos });
        }
        if self.is_word("if") {
            return self.if_stmt();
        }
        if self.eat_word("for") {
            self.expect_word("each")?;
            let base = self.set_base()?;
            let binder = self.name()?;
            let filter = if self.eat_word("where") {
                Some(self.expr()?)
            } else {
                None
            };
            let body = self.block()?;
            return Ok(Stmt::ForEach {
                set: SetExpr {
                    base,
                    binder: None,
                    filter,
                    pos,
                },
                binder,
                body,
                pos,
            });
        }
        let target = self.expr()?;
        if self.eat_punct("=") {
            let value = self.expr()?;
            return Ok(Stmt::Assign { target, value, pos });
        }
        Ok(Stmt::Expr(target))
    }

    fn if_stmt(&mut self) -> R<Stmt> {
        self.enter()?;
        self.expect_word("if")?;
        let cond = self.expr()?;
        let then = self.block()?;
        let els = if self.eat_word("else") {
            if self.is_word("if") {
                vec![self.if_stmt()?]
            } else {
                self.block()?
            }
        } else {
            Vec::new()
        };
        self.leave();
        Ok(Stmt::If { cond, then, els })
    }

    fn set_base(&mut self) -> R<SetBaseExpr> {
        let pos = self.pos();
        if self.eat_word("players") {
            return Ok(SetBaseExpr::Players);
        }
        if self.eat_word("teams") {
            return Ok(SetBaseExpr::Teams);
        }
        if self.eat_word("objects") {
            self.expect_word("labelled")?;
            return Ok(SetBaseExpr::Labelled(self.string()?));
        }
        if self.eat_word("objectives") {
            return Ok(SetBaseExpr::Objectives);
        }
        if self.is_word("pick") && matches!(self.peek2().tok, Tok::Punct("(")) {
            self.bump();
            self.bump();
            self.enter()?;
            let inner = self.set_arg()?;
            self.expect_punct(",")?;
            let n = self.expr()?;
            self.expect_punct(")")?;
            self.leave();
            return Ok(SetBaseExpr::Pick(Box::new(inner), Box::new(n)));
        }
        Err(CompileError::at(
            pos,
            "expected a set: players, teams, objectives, objects labelled \"x\" or pick(set, n)",
        ))
    }

    /// A set as a call argument: a base, an optional member name and an
    /// optional `where` (`count(players p where p.team == t)`).
    fn set_arg(&mut self) -> R<SetExpr> {
        let pos = self.pos();
        let base = self.set_base()?;
        let named = matches!(&self.peek().tok, Tok::Ident(s) if !KEYWORDS.contains(&s.as_str()));
        let binder = if named { Some(self.name()?) } else { None };
        let filter = if self.eat_word("where") {
            Some(self.expr()?)
        } else {
            None
        };
        Ok(SetExpr {
            base,
            binder,
            filter,
            pos,
        })
    }

    pub fn expr(&mut self) -> R<Expr> {
        self.enter()?;
        let e = self.or();
        self.leave();
        e
    }

    fn binary(&mut self, lhs: Expr, op: BinOp, rhs: Expr) -> Expr {
        let pos = lhs.pos;
        Expr {
            kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)),
            pos,
        }
    }

    fn or(&mut self) -> R<Expr> {
        let mut e = self.and()?;
        while self.eat_word("or") {
            let rhs = self.and()?;
            e = self.binary(e, BinOp::Or, rhs);
        }
        Ok(e)
    }

    fn and(&mut self) -> R<Expr> {
        let mut e = self.not()?;
        while self.eat_word("and") {
            let rhs = self.not()?;
            e = self.binary(e, BinOp::And, rhs);
        }
        Ok(e)
    }

    fn not(&mut self) -> R<Expr> {
        let pos = self.pos();
        if self.eat_word("not") {
            self.enter()?;
            let inner = self.not()?;
            self.leave();
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Not, Box::new(inner)),
                pos,
            });
        }
        self.cmp()
    }

    fn cmp(&mut self) -> R<Expr> {
        let e = self.add()?;
        let op = match &self.peek().tok {
            Tok::Punct("==") => BinOp::Eq,
            Tok::Punct("!=") => BinOp::Ne,
            Tok::Punct("<") => BinOp::Lt,
            Tok::Punct("<=") => BinOp::Le,
            Tok::Punct(">") => BinOp::Gt,
            Tok::Punct(">=") => BinOp::Ge,
            _ => return Ok(e),
        };
        self.bump();
        let rhs = self.add()?;
        Ok(self.binary(e, op, rhs))
    }

    fn add(&mut self) -> R<Expr> {
        let mut e = self.mul()?;
        loop {
            let op = match &self.peek().tok {
                Tok::Punct("+") => BinOp::Add,
                Tok::Punct("-") => BinOp::Sub,
                _ => return Ok(e),
            };
            self.bump();
            let rhs = self.mul()?;
            e = self.binary(e, op, rhs);
        }
    }

    fn mul(&mut self) -> R<Expr> {
        let mut e = self.unary()?;
        loop {
            let op = match &self.peek().tok {
                Tok::Punct("*") => BinOp::Mul,
                Tok::Punct("/") => BinOp::Div,
                Tok::Punct("%") => BinOp::Rem,
                _ => return Ok(e),
            };
            self.bump();
            let rhs = self.unary()?;
            e = self.binary(e, op, rhs);
        }
    }

    fn unary(&mut self) -> R<Expr> {
        let pos = self.pos();
        if self.eat_punct("-") {
            self.enter()?;
            let inner = self.unary()?;
            self.leave();
            return Ok(Expr {
                kind: ExprKind::Unary(UnOp::Neg, Box::new(inner)),
                pos,
            });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        loop {
            let pos = self.pos();
            if self.is_punct(".") && !matches!(self.peek2().tok, Tok::Punct(".")) {
                self.bump();
                let field = self.word()?;
                e = Expr {
                    kind: ExprKind::Field(Box::new(e), field),
                    pos,
                };
            } else if self.is_punct("[") && !self.peek().nl {
                self.bump();
                let index = self.expr()?;
                self.expect_punct("]")?;
                e = Expr {
                    kind: ExprKind::Index(Box::new(e), Box::new(index)),
                    pos,
                };
            } else {
                return Ok(e);
            }
        }
    }

    fn primary(&mut self) -> R<Expr> {
        let pos = self.pos();
        let at = |kind| Expr { kind, pos };
        let tok = self.peek().tok.clone();
        match tok {
            Tok::Int(n) => {
                self.bump();
                Ok(at(ExprKind::Int(n)))
            }
            Tok::Fixed(v) => {
                self.bump();
                Ok(at(ExprKind::Fixed(v)))
            }
            Tok::Str(s) => {
                self.bump();
                Ok(at(ExprKind::Str(s)))
            }
            Tok::Punct("(") => {
                self.bump();
                let e = self.expr()?;
                self.expect_punct(")")?;
                Ok(e)
            }
            Tok::Ident(w) => {
                match w.as_str() {
                    "true" | "false" => {
                        self.bump();
                        return Ok(at(ExprKind::Bool(w == "true")));
                    }
                    "none" => {
                        self.bump();
                        return Ok(at(ExprKind::None));
                    }
                    "team" | "text" => {
                        self.bump();
                        self.enter()?;
                        let inner = self.primary()?;
                        self.leave();
                        let b = Box::new(inner);
                        return Ok(at(if w == "team" {
                            ExprKind::Team(b)
                        } else {
                            ExprKind::Text(b)
                        }));
                    }
                    "labelled" => {
                        self.bump();
                        return Ok(at(ExprKind::Labelled(self.string()?)));
                    }
                    "budget" => {
                        self.bump();
                        return Ok(at(ExprKind::Budget(self.int()?)));
                    }
                    "palette" => {
                        self.bump();
                        return Ok(at(ExprKind::Palette(self.string()?)));
                    }
                    "players" | "teams" | "objects" | "objectives" => {
                        return Err(CompileError::at(
                            pos,
                            "a set is only allowed in for each, count, first and pick",
                        ));
                    }
                    _ => {}
                }
                if KEYWORDS.contains(&w.as_str()) {
                    return Err(self.unexpected("an expression"));
                }
                self.bump();
                if self.is_punct("(") && !self.peek().nl {
                    self.bump();
                    let mut args = Vec::new();
                    let takes_set = matches!(w.as_str(), "count" | "first" | "pick");
                    while !self.is_punct(")") {
                        if takes_set && args.is_empty() {
                            let set = self.set_arg()?;
                            let p = set.pos;
                            args.push(Expr {
                                kind: ExprKind::Set(Box::new(set)),
                                pos: p,
                            });
                        } else {
                            args.push(self.expr()?);
                        }
                        if !self.eat_punct(",") {
                            break;
                        }
                    }
                    self.expect_punct(")")?;
                    return Ok(at(ExprKind::Call(w, args)));
                }
                Ok(at(ExprKind::Name(w)))
            }
            _ => Err(self.unexpected("an expression")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::lex::lex;
    use super::*;

    fn expr(src: &str) -> Expr {
        let toks = lex(src).unwrap();
        let mut p = Parser::new(&toks, false);
        p.expr().unwrap()
    }

    fn shape(e: &Expr) -> String {
        match &e.kind {
            ExprKind::Int(n) => n.to_string(),
            ExprKind::Name(n) => n.clone(),
            ExprKind::Binary(op, a, b) => format!("({op:?} {} {})", shape(a), shape(b)),
            ExprKind::Unary(op, a) => format!("({op:?} {})", shape(a)),
            ExprKind::Field(a, f) => format!("{}.{f}", shape(a)),
            other => format!("{other:?}"),
        }
    }

    #[test]
    fn precedence_and_binds_tighter_than_or() {
        assert_eq!(shape(&expr("a or b and c")), "(Or a (And b c))");
        assert_eq!(shape(&expr("a = b")), "a");
        assert_eq!(shape(&expr("b + c * 2")), "(Add b (Mul c 2))");
        assert_eq!(shape(&expr("(b + c) * 2")), "(Mul (Add b c) 2)");
        assert_eq!(shape(&expr("not a == b and c")), "(And (Not (Eq a b)) c)");
        assert_eq!(shape(&expr("-a - 1")), "(Sub (Neg a) 1)");
        assert_eq!(shape(&expr("k.team == v.team")), "(Eq k.team v.team)");
    }

    #[test]
    fn deep_nesting_is_refused_not_overflowed() {
        let src = format!("{}1{}", "(".repeat(500), ")".repeat(500));
        let toks = lex(&src).unwrap();
        let mut p = Parser::new(&toks, false);
        assert!(p.expr().is_err());
    }

    #[test]
    fn statements_end_at_line_breaks() {
        let toks = lex("rules \"T\" 1\ncarrier slayer\non tick { a = 1\n b = 2; c = 3 }\n").unwrap();
        let s = parse(&toks).unwrap();
        match &s.items[0] {
            Item::On { body, .. } => assert_eq!(body.len(), 3),
            other => panic!("{other:?}"),
        }
        let toks = lex("rules \"T\" 1\non tick { a = 1 b = 2 }\n").unwrap();
        let e = parse(&toks).unwrap_err();
        assert_eq!(e.line, 2);
    }

    fn recovering(src: &str) -> (Script, Vec<CompileError>) {
        parse_recovering(&lex(src).unwrap())
    }

    #[test]
    fn recovery_skips_a_bad_statement_and_keeps_the_rest() {
        let src = "rules \"T\" 1\ncarrier slayer\non tick {\n    a = 1\n    b = \n    score(, )\n    c = 3\n}\non init { d = 4 }\n";
        let (s, errors) = recovering(src);
        // `b =` reads `score` from the next line as its value, then stops
        // at `(`; the recovery skips the rest of line 6.
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].line, 6);
        assert_eq!(s.items.len(), 2);
        match &s.items[0] {
            Item::On { body, .. } => assert_eq!(body.len(), 2),
            other => panic!("{other:?}"),
        }
        let src = "rules \"T\" 1\ncarrier slayer\non tick {\n    score(, )\n    c = 3\n    d = )\n}\n";
        let (s, errors) = recovering(src);
        let lines: Vec<u32> = errors.iter().map(|e| e.line).collect();
        assert_eq!(lines, vec![4, 6], "{errors:?}");
        match &s.items[0] {
            Item::On { body, .. } => assert_eq!(body.len(), 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn recovery_ends_an_open_block_at_the_next_declaration() {
        let src = "rules \"T\" 1\ncarrier slayer\non tick {\n    a = 1\non spawn(p) { b = 2 }\nfn f( {\n}\noption o: int 1..2 = 1\n";
        let (s, errors) = recovering(src);
        let lines: Vec<u32> = errors.iter().map(|e| e.line).collect();
        assert_eq!(lines, vec![5, 6], "{errors:?}");
        let names: Vec<&str> = s
            .items
            .iter()
            .map(|i| match i {
                Item::On { event, .. } => event.as_str(),
                Item::Option(o) => o.name.as_str(),
                _ => "?",
            })
            .collect();
        assert_eq!(names, vec!["tick", "spawn", "o"]);
    }

    #[test]
    fn recovery_survives_a_bad_header_and_agrees_with_parse_on_clean_source() {
        let (s, errors) = recovering("rules oops\ncarrier slayer\non init { }\n");
        assert_eq!(errors.len(), 1);
        assert_eq!(s.carrier.as_ref().map(|c| c.0.as_str()), Some("slayer"));
        assert_eq!(s.items.len(), 1);
        for b in crate::builtin::ALL {
            let toks = lex(b.source).unwrap();
            let (s, errors) = parse_recovering(&toks);
            assert!(errors.is_empty(), "{}: {errors:?}", b.name);
            assert_eq!(s, parse(&toks).unwrap());
        }
        // Garbage ends; it does not loop.
        let (_, errors) = recovering("} } { ( on on fn fn ) [ ; ; option");
        assert!(!errors.is_empty());
    }
}

