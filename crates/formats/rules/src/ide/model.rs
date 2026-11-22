//! The analysis every feature reads: the scan, the recovered syntax tree,
//! each declaration with its exact span and type, and each name use
//! resolved to its declaration or to an API entry, a variant field, an
//! event, a capability or a carrier. Scoping follows `lang::lower`: a
//! handler sees its arguments, `let`s and set members in scope, and the
//! script's globals, options, tables, enums and functions; a `fn` body
//! sees its parameters and the script's names.

use std::collections::BTreeMap;

use super::scan::{self, K, Tk};
use super::{Position, Range};
use crate::caps::{self, Entry, VARIANT_FIELDS};
use crate::ir::{Event, Scope};
use crate::lang::ast::*;
use crate::lang::lower::FIELDS;
use crate::lang::{CompileError, Pos, lex, parse};
use crate::ty::Ty;

/// Byte offsets to LSP positions and back.
pub(crate) struct LineIndex<'s> {
    src: &'s str,
    starts: Vec<usize>,
}

impl<'s> LineIndex<'s> {
    pub fn new(src: &'s str) -> LineIndex<'s> {
        let mut starts = vec![0];
        for (i, b) in src.bytes().enumerate() {
            if b == b'\n' {
                starts.push(i + 1);
            }
        }
        LineIndex { src, starts }
    }

    pub fn line_of(&self, off: usize) -> usize {
        self.starts.partition_point(|&s| s <= off).saturating_sub(1)
    }

    pub fn position(&self, off: usize) -> Position {
        let mut off = off.min(self.src.len());
        while !self.src.is_char_boundary(off) {
            off -= 1;
        }
        let line = self.line_of(off);
        let character = self.src[self.starts[line]..off].encode_utf16().count() as u32;
        Position {
            line: line as u32,
            character,
        }
    }

    pub fn range(&self, s: usize, e: usize) -> Range {
        Range {
            start: self.position(s),
            end: self.position(e.max(s)),
        }
    }

    /// The byte offset of a position; past the end of a line is its end.
    pub fn offset(&self, p: Position) -> usize {
        let Some(&start) = self.starts.get(p.line as usize) else {
            return self.src.len();
        };
        let end = self.line_end(p.line as usize);
        let mut units = 0u32;
        for (i, ch) in self.src[start..end].char_indices() {
            if units >= p.character {
                return start + i;
            }
            units += ch.len_utf16() as u32;
        }
        end
    }

    /// The byte offset of a lexer position (1-based line, byte column).
    pub fn pos_offset(&self, p: Pos) -> usize {
        let line = p.line.max(1) as usize - 1;
        let Some(&start) = self.starts.get(line) else {
            return self.src.len();
        };
        (start + p.col.max(1) as usize - 1).min(self.src.len())
    }

    /// Where a line's text ends, before its line break.
    pub fn line_end(&self, line: usize) -> usize {
        match self.starts.get(line + 1) {
            Some(&next) => next - 1,
            None => self.src.len(),
        }
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.starts.get(line).copied().unwrap_or(self.src.len())
    }

    pub fn line_text(&self, line: usize) -> &'s str {
        let s = self.line_start(line);
        let e = self.line_end(line).max(s);
        let src: &'s str = self.src;
        src[s..e].trim_end_matches('\r')
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DefKind {
    Option,
    Palette,
    Enum,
    Member,
    Var(Scope),
    Table,
    Column,
    Fn,
    /// A `fn` parameter.
    Param,
    /// A handler argument.
    Arg,
    /// A `let`.
    Local,
    /// A `for each` or set member.
    Binder,
    /// A Forge label, declared by its first use.
    Label,
}

/// A type as far as the tooling knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum T {
    Ty(Ty),
    /// An enum, by its declaration.
    Enum(usize),
    /// A row of a table, by the table's declaration.
    Row(usize),
    Unknown,
}

#[derive(Clone, Debug)]
pub(crate) struct Def {
    pub name: String,
    pub kind: DefKind,
    pub span: (usize, usize),
    pub ty: T,
    /// Where a local is in scope.
    pub visible: Option<(usize, usize)>,
    /// A member's enum, a column's table, a parameter's fn.
    pub parent: Option<usize>,
    /// A handler argument's event.
    pub event: Option<Event>,
}

/// A use of a name that is not a declaration of the script.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Use {
    Api(&'static Entry),
    /// A language built-in, by index in `info::LANG`.
    Lang(usize),
    /// By index in `caps::VARIANT_FIELDS`.
    VariantField(usize),
    /// A built-in field (`p.score`), by index in `lower::FIELDS`.
    Field(usize),
    Capability,
    Event(Event),
    Carrier,
    Type,
    /// The word `variant`.
    Variant,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Ref {
    pub s: usize,
    pub e: usize,
    pub def: usize,
    pub decl: bool,
}

pub(crate) struct Model<'s> {
    pub src: &'s str,
    pub li: LineIndex<'s>,
    /// Every token, comments included.
    pub all: Vec<Tk>,
    /// The tokens other than comments.
    pub code: Vec<Tk>,
    /// Each bracket's partner in `code`, or `usize::MAX`.
    pairs: Vec<usize>,
    pub script: Script,
    /// Lexer and parser errors, in order.
    pub errors: Vec<CompileError>,
    pub defs: Vec<Def>,
    pub refs: Vec<Ref>,
    pub uses: Vec<(usize, usize, Use)>,
    /// Each item's byte span, to its last token.
    pub items: Vec<(usize, usize)>,
    /// Each item's own declaration (a handler has none).
    pub item_defs: Vec<Option<usize>>,
    labels: BTreeMap<String, usize>,
}

#[derive(Default)]
struct Env {
    locals: Vec<usize>,
    /// The member a `where` reads fields of without naming it.
    implicit: Vec<T>,
}

/// The type a type name spells, given the script's enums.
pub(crate) fn builtin_type(name: &str) -> Option<Ty> {
    Some(match name {
        "int" => Ty::Int,
        "fixed" => Ty::Fixed,
        "bool" => Ty::Bool,
        "player" => Ty::Player,
        "team" => Ty::Team,
        "object" | "zone" => Ty::Object,
        "squad" => Ty::Squad,
        "timer" => Ty::Timer,
        "Class" | "class" => Ty::Class,
        "tag" | "weapon" => Ty::Tag,
        "label" | "objects" | "zones" | "spawns" => Ty::Label,
        "text" => Ty::Text,
        _ => return None,
    })
}

fn empty_script() -> Script {
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

fn pairs_of(src: &str, code: &[Tk]) -> Vec<usize> {
    let mut out = vec![usize::MAX; code.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (i, t) in code.iter().enumerate() {
        if t.k != K::Punct {
            continue;
        }
        let want = match &src[t.s..t.e] {
            "{" | "(" | "[" => {
                stack.push(i);
                continue;
            }
            "}" => "{",
            ")" => "(",
            "]" => "[",
            _ => continue,
        };
        if let Some(&top) = stack.last()
            && &src[code[top].s..code[top].e] == want
        {
            stack.pop();
            out[top] = i;
            out[i] = top;
        }
    }
    out
}

pub(crate) fn analyze(src: &str) -> Model<'_> {
    let all = scan::scan(src);
    let code: Vec<Tk> = all.iter().copied().filter(|t| t.k != K::Comment).collect();
    let pairs = pairs_of(src, &code);
    let (tokens, mut errors) = lex::lex_tolerant(src);
    let (script, perr) = parse::parse_recovering(&tokens);
    errors.extend(perr);
    let mut m = Model {
        src,
        li: LineIndex::new(src),
        all,
        code,
        pairs,
        script: empty_script(),
        errors,
        defs: Vec::new(),
        refs: Vec::new(),
        uses: Vec::new(),
        items: Vec::new(),
        item_defs: vec![None; script.items.len()],
        labels: BTreeMap::new(),
    };
    m.items = m.item_spans(&script);
    m.declare(&script);
    m.bodies(&script);
    m.refs.sort_by_key(|r| (r.s, r.e));
    m.uses.sort_by_key(|u| (u.0, u.1));
    m.script = script;
    m
}

impl<'s> Model<'s> {
    pub fn text(&self, t: &Tk) -> &'s str {
        let src: &'s str = self.src;
        &src[t.s..t.e]
    }

    pub fn is_punct(&self, i: usize, p: &str) -> bool {
        self.code
            .get(i)
            .is_some_and(|t| t.k == K::Punct && self.text(t) == p)
    }

    pub fn is_word(&self, i: usize, w: &str) -> bool {
        self.code
            .get(i)
            .is_some_and(|t| t.k == K::Ident && self.text(t) == w)
    }

    /// The index of the first code token starting at or after `off`.
    pub fn ci(&self, off: usize) -> usize {
        self.code.partition_point(|t| t.s < off)
    }

    /// The index of the bracket matching `code[i]`.
    pub fn partner(&self, i: usize) -> Option<usize> {
        self.pairs.get(i).copied().filter(|&p| p != usize::MAX)
    }

    fn byte(&self, p: Pos) -> usize {
        self.li.pos_offset(p)
    }

    /// The span of the token at a lexer position.
    fn tok_at(&self, p: Pos) -> (usize, usize) {
        let off = self.byte(p);
        match self.code.get(self.ci(off)) {
            Some(t) if t.s == off => (t.s, t.e),
            _ => (off, (off + 1).min(self.src.len())),
        }
    }

    /// The first identifier `name` at or after `off`.
    fn ident_from(&self, off: usize, name: &str) -> Option<(usize, usize)> {
        self.code[self.ci(off)..]
            .iter()
            .take(256)
            .find(|t| t.k == K::Ident && self.text(t) == name)
            .map(|t| (t.s, t.e))
    }

    /// The identifier `name` after the token at `p` (a declaration's
    /// keyword), or that token.
    fn name_after(&self, p: Pos, name: &str) -> (usize, usize) {
        let start = self.tok_at(p);
        self.ident_from(start.1, name).unwrap_or(start)
    }

    /// The first string at or after `off`.
    fn str_from(&self, off: usize) -> (usize, usize) {
        self.code[self.ci(off)..]
            .iter()
            .take(8)
            .find(|t| t.k == K::Str)
            .map_or((off, (off + 1).min(self.src.len())), |t| (t.s, t.e))
    }

    /// The end of a dotted name (`forge.place`) starting at `off`.
    pub fn dotted_end(&self, off: usize) -> usize {
        let mut i = self.ci(off);
        let Some(first) = self.code.get(i) else {
            return off;
        };
        let mut end = first.e;
        while self.is_punct(i + 1, ".") && self.code.get(i + 2).is_some_and(|t| t.k == K::Ident) {
            i += 2;
            end = self.code[i].e;
        }
        end
    }

    /// The first `{` at or after `off` and the end of its block (the end
    /// of the file for one left open).
    fn block_after(&self, off: usize) -> (usize, usize) {
        let from = self.ci(off);
        for j in from..self.code.len() {
            if self.is_punct(j, "{") {
                let end = self.partner(j).map_or(self.src.len(), |c| self.code[c].e);
                return (self.code[j].s, end);
            }
        }
        (off, self.src.len())
    }

    fn item_spans(&self, script: &Script) -> Vec<(usize, usize)> {
        let mut bounds: Vec<usize> = script.items.iter().map(|i| self.byte(item_pos(i))).collect();
        let mut header = vec![script.pos];
        header.extend(script.carrier.iter().map(|c| c.1));
        header.extend(script.teams.iter().map(|t| t.1));
        header.extend(script.requires.iter().map(|r| r.1));
        for p in header {
            bounds.push(self.li.line_start(self.li.line_of(self.byte(p))));
        }
        bounds.push(self.src.len());
        bounds.sort_unstable();
        script
            .items
            .iter()
            .map(|item| {
                let s = self.byte(item_pos(item));
                let next = bounds
                    .iter()
                    .copied()
                    .find(|&b| b > s)
                    .unwrap_or(self.src.len());
                let k = self.code.partition_point(|t| t.s < next);
                let e = match k.checked_sub(1).map(|j| self.code[j]) {
                    Some(t) if t.s >= s => t.e,
                    _ => (s + 1).min(self.src.len()),
                };
                (s, e)
            })
            .collect()
    }

    fn def(&mut self, name: &str, kind: DefKind, span: (usize, usize), ty: T) -> usize {
        let d = self.defs.len();
        self.defs.push(Def {
            name: name.to_string(),
            kind,
            span,
            ty,
            visible: None,
            parent: None,
            event: None,
        });
        self.refs.push(Ref {
            s: span.0,
            e: span.1,
            def: d,
            decl: true,
        });
        d
    }

    fn add_ref(&mut self, span: (usize, usize), def: usize) {
        self.refs.push(Ref {
            s: span.0,
            e: span.1,
            def,
            decl: false,
        });
    }

    fn add_use(&mut self, span: (usize, usize), u: Use) {
        self.uses.push((span.0, span.1, u));
    }

    /// A script-level name: a global, an option, a table, an enum or a fn.
    pub fn top(&self, name: &str) -> Option<usize> {
        self.defs.iter().position(|d| {
            d.name == name
                && matches!(
                    d.kind,
                    DefKind::Var(Scope::Global) | DefKind::Option | DefKind::Table | DefKind::Enum | DefKind::Fn
                )
        })
    }

    pub fn top_kind(&self, name: &str, kind: DefKind) -> Option<usize> {
        self.defs.iter().position(|d| d.name == name && d.kind == kind)
    }

    pub fn scoped(&self, scope: Scope, name: &str) -> Option<usize> {
        self.top_kind(name, DefKind::Var(scope))
    }

    /// A declaration's children: an enum's members, a table's columns.
    pub fn children(&self, parent: usize, kind: DefKind) -> impl Iterator<Item = usize> + '_ {
        self.defs
            .iter()
            .enumerate()
            .filter(move |(_, d)| d.kind == kind && d.parent == Some(parent))
            .map(|(i, _)| i)
    }

    fn type_named(&self, name: &str) -> T {
        match builtin_type(name) {
            Some(t) => T::Ty(t),
            None => self.top_kind(name, DefKind::Enum).map_or(T::Unknown, T::Enum),
        }
    }

    fn type_use(&mut self, t: &TypeName) {
        let span = self.tok_at(t.pos);
        let d = self.top_kind(&t.name, DefKind::Enum);
        match d {
            Some(d) => self.add_ref(span, d),
            None => self.add_use(span, Use::Type),
        }
    }

    // ---- declarations ----

    fn declare(&mut self, script: &Script) {
        if let Some((name, p)) = &script.carrier {
            let span = self.name_after(*p, name);
            self.add_use(span, Use::Carrier);
        }
        for (_, p) in &script.requires {
            let s = self.byte(*p);
            let e = self.dotted_end(s);
            self.add_use((s, e), Use::Capability);
        }
        // Enums first, as the compiler reads them, so a type may name one
        // declared further down.
        for (i, item) in script.items.iter().enumerate() {
            if let Item::Enum { name, variants, pos } = item {
                let span = self.name_after(*pos, name);
                let d = self.def(name, DefKind::Enum, span, T::Unknown);
                self.defs[d].ty = T::Enum(d);
                self.item_defs[i] = Some(d);
                let mut at = span.1;
                for v in variants {
                    let Some(vs) = self.ident_from(at, v) else { break };
                    at = vs.1;
                    let m = self.def(v, DefKind::Member, vs, T::Enum(d));
                    self.defs[m].parent = Some(d);
                }
            }
        }
        for (i, item) in script.items.iter().enumerate() {
            match item {
                Item::Option(o) => {
                    let span = self.name_after(o.pos, &o.name);
                    let ty = self.type_named(&o.ty.name);
                    self.type_use(&o.ty);
                    let d = self.def(&o.name, DefKind::Option, span, ty);
                    self.item_defs[i] = Some(d);
                }
                Item::Palette { name, pos, .. } => {
                    let span = self.str_from(self.tok_at(*pos).1);
                    let d = self.def(name, DefKind::Palette, span, T::Unknown);
                    self.item_defs[i] = Some(d);
                }
                Item::Var {
                    scope, name, ty, pos, ..
                } => {
                    let span = self.name_after(*pos, name);
                    let t = self.type_named(&ty.name);
                    self.type_use(ty);
                    let d = self.def(name, DefKind::Var(*scope), span, t);
                    self.item_defs[i] = Some(d);
                }
                Item::Table {
                    name, columns, pos, ..
                } => {
                    let span = self.name_after(*pos, name);
                    let d = self.def(name, DefKind::Table, span, T::Unknown);
                    self.defs[d].ty = T::Row(d);
                    self.item_defs[i] = Some(d);
                    let mut at = span.1;
                    for (cname, cty) in columns {
                        let Some(cs) = self.ident_from(at, cname) else { break };
                        at = cs.1;
                        let t = self.type_named(&cty.name);
                        let c = self.def(cname, DefKind::Column, cs, t);
                        self.defs[c].parent = Some(d);
                        self.type_use(cty);
                    }
                }
                Item::Fn(f) => {
                    let span = self.name_after(f.pos, &f.name);
                    let d = self.def(&f.name, DefKind::Fn, span, T::Unknown);
                    self.item_defs[i] = Some(d);
                }
                Item::On { event, pos, .. } => {
                    let span = self.name_after(*pos, event);
                    if let Some(ev) = Event::named(event) {
                        self.add_use(span, Use::Event(ev));
                    }
                }
                Item::Label(l) => {
                    let span = self.name_after(l.pos, &l.name);
                    let d = self.def(&l.name, DefKind::Label, span, T::Ty(Ty::Object));
                    self.labels.insert(l.name.clone(), d);
                    self.item_defs[i] = Some(d);
                }
                Item::Enum { .. } => {}
            }
        }
    }

    // ---- bodies ----

    fn bodies(&mut self, script: &Script) {
        for (i, item) in script.items.iter().enumerate() {
            let end = self.items[i].1;
            let mut env = Env::default();
            match item {
                Item::Option(o) => {
                    if let Some((lo, hi)) = &o.range {
                        self.expr(lo, &mut env);
                        self.expr(hi, &mut env);
                    }
                    self.expr(&o.default, &mut env);
                }
                Item::Var { init: Some(e), .. } => {
                    self.expr(e, &mut env);
                }
                Item::Table { rows, .. } => {
                    for cell in rows.iter().flatten() {
                        self.expr(cell, &mut env);
                    }
                }
                Item::Fn(f) => {
                    let Some(fd) = self.item_defs[i] else { continue };
                    let mut at = self.defs[fd].span.1;
                    for (pname, pty) in &f.params {
                        let Some(ps) = self.ident_from(at, pname) else { break };
                        at = ps.1;
                        let t = self.type_named(&pty.name);
                        let d = self.def(pname, DefKind::Param, ps, t);
                        self.defs[d].visible = Some((ps.1, end));
                        self.defs[d].parent = Some(fd);
                        self.type_use(pty);
                        env.locals.push(d);
                    }
                    self.block(&f.body, &mut env, end);
                }
                Item::On {
                    event, args, body, pos, ..
                } => {
                    let ev = Event::named(event);
                    let mut at = self.name_after(*pos, event).1;
                    for (k, a) in args.iter().enumerate() {
                        let Some(span) = self.ident_from(at, a) else { break };
                        at = span.1;
                        if ev == Some(Event::Timer) {
                            // `on timer(t)` names a global timer.
                            if let Some(g) = self.scoped(Scope::Global, a) {
                                self.add_ref(span, g);
                            }
                            continue;
                        }
                        let ty = ev
                            .and_then(|e| e.args().get(k).copied())
                            .map_or(T::Unknown, T::Ty);
                        let d = self.def(a, DefKind::Arg, span, ty);
                        self.defs[d].visible = Some((span.1, end));
                        self.defs[d].event = ev;
                        env.locals.push(d);
                    }
                    self.block(body, &mut env, end);
                }
                _ => {}
            }
        }
    }

    fn block(&mut self, body: &[Stmt], env: &mut Env, end: usize) {
        let mark = env.locals.len();
        for s in body {
            self.stmt(s, env, end);
        }
        env.locals.truncate(mark);
    }

    fn stmt(&mut self, s: &Stmt, env: &mut Env, end: usize) {
        match s {
            Stmt::Let { name, value, pos, .. } => {
                let ty = self.expr(value, env);
                let span = self.name_after(*pos, name);
                let d = self.def(name, DefKind::Local, span, ty);
                self.defs[d].visible = Some((span.1, end));
                env.locals.push(d);
            }
            Stmt::Assign { target, value, .. } => {
                self.expr(target, env);
                self.expr(value, env);
            }
            Stmt::IfLet {
                name,
                value,
                cond,
                then,
                els,
                pos,
            } => {
                let ty = self.expr(value, env);
                let span = self.ident_from(self.byte(*pos), name).unwrap_or((0, 0));
                let start = cond.as_ref().map_or(value.pos, |c| c.pos);
                let (_, close) = self.block_after(self.byte(start));
                let mark = env.locals.len();
                let d = self.def(name, DefKind::Local, span, ty);
                self.defs[d].visible = Some((span.1, close));
                env.locals.push(d);
                if let Some(c) = cond {
                    self.expr(c, env);
                }
                self.block(then, env, close);
                env.locals.truncate(mark);
                if !els.is_empty() {
                    let (_, eclose) = self.block_after(close);
                    self.block(els, env, eclose);
                }
            }
            Stmt::Expr(e) => {
                self.expr(e, env);
            }
            Stmt::If { cond, then, els } => {
                self.expr(cond, env);
                let (_, close) = self.block_after(self.byte(cond.pos));
                self.block(then, env, close);
                if els.is_empty() {
                    return;
                }
                let after = self.ci(close);
                let else_if = self.is_word(after, "else") && self.is_word(after + 1, "if");
                match els.as_slice() {
                    [inner @ Stmt::If { .. }] if else_if => self.stmt(inner, env, end),
                    _ => {
                        let (_, eclose) = self.block_after(close);
                        self.block(els, env, eclose);
                    }
                }
            }
            Stmt::ForEach {
                set,
                binder,
                body,
                pos,
            } => {
                let elem = self.set(set, env);
                // `for each <set> <binder>`: past `for`, `each` and the set.
                let base = self.skip_set(self.ci(self.byte(*pos)) + 2);
                let span = match self.code.get(base) {
                    Some(t) if self.text(t) == binder => (t.s, t.e),
                    _ => self.ident_from(self.byte(*pos), binder).unwrap_or((0, 0)),
                };
                let (_, close) = self.block_after(span.1);
                let mark = env.locals.len();
                let d = self.def(binder, DefKind::Binder, span, T::Ty(elem));
                self.defs[d].visible = Some((span.1, close));
                env.locals.push(d);
                if let Some(f) = &set.filter {
                    env.implicit.push(T::Ty(elem));
                    self.expr(f, env);
                    env.implicit.pop();
                }
                self.block(body, env, close);
                env.locals.truncate(mark);
            }
        }
    }

    /// The code index past a set base starting at `i`.
    fn skip_set(&self, i: usize) -> usize {
        let Some(t) = self.code.get(i) else { return i };
        match self.text(t) {
            "pick" if self.is_punct(i + 1, "(") => self.partner(i + 1).map_or(i + 2, |c| c + 1),
            _ => i + 1,
        }
    }

    /// Walks a set's base; its member type.
    fn set(&mut self, set: &SetExpr, env: &mut Env) -> Ty {
        match &set.base {
            SetBaseExpr::Players => Ty::Player,
            SetBaseExpr::Teams => Ty::Team,
            SetBaseExpr::Objectives => Ty::Object,
            SetBaseExpr::Label(l) => {
                let span = self.tok_at(set.pos);
                match self.local(l, env) {
                    Some(d) => self.add_ref(span, d),
                    None => self.label(l, span),
                }
                Ty::Object
            }
            SetBaseExpr::Pick(inner, n) => {
                let t = self.set(inner, env);
                self.expr(n, env);
                t
            }
        }
    }

    /// A set argument of `count`, `first` or `pick` at `call`: its member
    /// is in scope to the call's `)`.
    fn set_arg(&mut self, set: &SetExpr, env: &mut Env, call: Pos) -> Ty {
        let elem = self.set(set, env);
        let mark = env.locals.len();
        if let Some(binder) = &set.binder {
            let i = self.skip_set(self.ci(self.byte(set.pos)));
            let span = match self.code.get(i) {
                Some(t) if self.text(t) == binder => (t.s, t.e),
                _ => self.ident_from(self.byte(set.pos), binder).unwrap_or((0, 0)),
            };
            let open = self.ci(self.byte(call)) + 1;
            let close = match self.partner(open) {
                Some(c) if self.is_punct(open, "(") => self.code[c].e,
                _ => self.src.len(),
            };
            let d = self.def(binder, DefKind::Binder, span, T::Ty(elem));
            self.defs[d].visible = Some((span.1, close));
            env.locals.push(d);
        }
        if let Some(f) = &set.filter {
            env.implicit.push(T::Ty(elem));
            self.expr(f, env);
            env.implicit.pop();
        }
        env.locals.truncate(mark);
        elem
    }

    fn label(&mut self, name: &str, span: (usize, usize)) {
        match self.labels.get(name) {
            Some(&d) => self.add_ref(span, d),
            None => {
                let d = self.def(name, DefKind::Label, span, T::Ty(Ty::Label));
                self.labels.insert(name.to_string(), d);
            }
        }
    }

    fn local(&self, name: &str, env: &Env) -> Option<usize> {
        env.locals
            .iter()
            .rev()
            .copied()
            .find(|&d| self.defs[d].name == name)
    }

    fn expr(&mut self, e: &Expr, env: &mut Env) -> T {
        match &e.kind {
            ExprKind::Int(_) => T::Ty(Ty::Int),
            ExprKind::Fixed(_) => T::Ty(Ty::Fixed),
            ExprKind::Bool(_) => T::Ty(Ty::Bool),
            ExprKind::Str(_) | ExprKind::None => T::Unknown,
            ExprKind::Name(n) => self.name(n, e.pos, env),
            ExprKind::Field(base, f) => self.field(base, f, e.pos, env),
            ExprKind::Index(base, index) => {
                let t = match &base.kind {
                    ExprKind::Name(n) if self.local(n, env).is_none() => match self.top_kind(n, DefKind::Table) {
                        Some(d) => {
                            let span = self.tok_at(base.pos);
                            self.add_ref(span, d);
                            T::Row(d)
                        }
                        None => {
                            self.expr(base, env);
                            T::Unknown
                        }
                    },
                    _ => {
                        self.expr(base, env);
                        T::Unknown
                    }
                };
                self.expr(index, env);
                t
            }
            ExprKind::Call(name, args) => self.call(name, args, e.pos, env),
            ExprKind::Unary(UnOp::Neg, inner) => self.expr(inner, env),
            ExprKind::Unary(UnOp::Not, inner) => {
                self.expr(inner, env);
                T::Ty(Ty::Bool)
            }
            ExprKind::Binary(op, a, b) => {
                let ta = self.expr(a, env);
                let tb = self.expr(b, env);
                match op {
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                        if ta == T::Unknown || literal(a) { tb } else { ta }
                    }
                    _ => T::Ty(Ty::Bool),
                }
            }
            ExprKind::Team(inner) => {
                self.expr(inner, env);
                T::Ty(Ty::Team)
            }
            ExprKind::Text(inner) => {
                self.expr(inner, env);
                T::Ty(Ty::Text)
            }
            ExprKind::Is(x, l, lpos) => {
                self.expr(x, env);
                let span = self.tok_at(*lpos);
                self.label(l, span);
                T::Ty(Ty::Bool)
            }
            ExprKind::Budget(_) => T::Ty(Ty::Int),
            ExprKind::Palette(p) => {
                let span = self.str_from(self.tok_at(e.pos).1);
                if let Some(d) = self.top_kind(p, DefKind::Palette) {
                    self.add_ref(span, d);
                }
                T::Unknown
            }
            ExprKind::Set(set) => T::Ty(self.set(set, env)),
        }
    }

    fn name(&mut self, n: &str, pos: Pos, env: &mut Env) -> T {
        let span = self.tok_at(pos);
        if let Some(d) = self.local(n, env) {
            self.add_ref(span, d);
            return self.defs[d].ty;
        }
        if let Some(&t) = env.implicit.last()
            && let Some(r) = self.field_of(t, n, span)
        {
            return r;
        }
        if n == "variant" {
            self.add_use(span, Use::Variant);
            return T::Unknown;
        }
        if let Some(d) = self.top(n) {
            self.add_ref(span, d);
            return match self.defs[d].kind {
                DefKind::Var(_) | DefKind::Option => self.defs[d].ty,
                _ => T::Unknown,
            };
        }
        T::Unknown
    }

    /// The span of the field name after the `.` at `dot`.
    fn field_span(&self, dot: Pos) -> (usize, usize) {
        let i = self.ci(self.byte(dot));
        match self.code.get(i + 1) {
            Some(t) if self.is_punct(i, ".") => (t.s, t.e),
            _ => self.tok_at(dot),
        }
    }

    fn field(&mut self, base: &Expr, f: &str, dot: Pos, env: &mut Env) -> T {
        let span = self.field_span(dot);
        if let ExprKind::Name(b) = &base.kind
            && self.local(b, env).is_none()
        {
            if b == "variant" {
                let bspan = self.tok_at(base.pos);
                self.add_use(bspan, Use::Variant);
                return match VARIANT_FIELDS.iter().position(|(n, _)| *n == f) {
                    Some(i) => {
                        self.add_use(span, Use::VariantField(i));
                        T::Ty(VARIANT_FIELDS[i].1)
                    }
                    None => T::Unknown,
                };
            }
            if let Some(ed) = self.top_kind(b, DefKind::Enum) {
                let bspan = self.tok_at(base.pos);
                self.add_ref(bspan, ed);
                let member = self.children(ed, DefKind::Member).find(|&m| self.defs[m].name == f);
                if let Some(m) = member {
                    self.add_ref(span, m);
                }
                return T::Enum(ed);
            }
        }
        let t = self.expr(base, env);
        self.field_of(t, f, span).unwrap_or(T::Unknown)
    }

    /// A field of a value of type `t`: a scoped variable, a table column
    /// or a built-in read. Records the use.
    fn field_of(&mut self, t: T, f: &str, span: (usize, usize)) -> Option<T> {
        match field_target(self, t, f) {
            Some(FieldTarget::Def(d)) => {
                self.add_ref(span, d);
                Some(self.defs[d].ty)
            }
            Some(FieldTarget::Builtin(i)) => {
                self.add_use(span, Use::Field(i));
                Some(builtin_field_type(i))
            }
            None => None,
        }
    }

    fn call(&mut self, name: &str, args: &[Expr], pos: Pos, env: &mut Env) -> T {
        let span = self.tok_at(pos);
        if let Some(l) = super::info::lang_index(name) {
            self.add_use(span, Use::Lang(l));
            let mut elem = T::Unknown;
            for (k, a) in args.iter().enumerate() {
                match (&a.kind, k) {
                    (ExprKind::Set(set), 0) => elem = T::Ty(self.set_arg(set, env, pos)),
                    (_, 1) if matches!(name, "grant" | "revoke" | "granted") => self.cap_arg(a),
                    _ => {
                        self.expr(a, env);
                    }
                }
            }
            return match name {
                "count" | "len" | "seconds" => T::Ty(Ty::Int),
                "first" => elem,
                "granted" => T::Ty(Ty::Bool),
                _ => T::Unknown,
            };
        }
        if let Some(d) = self.top_kind(name, DefKind::Fn) {
            self.add_ref(span, d);
            for a in args {
                self.expr(a, env);
            }
            return T::Unknown;
        }
        if let Some(entry) = caps::entry_named(name) {
            self.add_use(span, Use::Api(entry));
            for (k, a) in args.iter().enumerate() {
                match (&a.kind, entry.args.get(k)) {
                    (ExprKind::Str(s), Some(Ty::Label)) => {
                        let sp = self.tok_at(a.pos);
                        self.label(s, sp);
                    }
                    _ => {
                        self.expr(a, env);
                    }
                }
            }
            return entry.result.map_or(T::Unknown, T::Ty);
        }
        for a in args {
            self.expr(a, env);
        }
        T::Unknown
    }

    /// A capability argument (`forge.place`).
    fn cap_arg(&mut self, e: &Expr) {
        let mut head = e;
        while let ExprKind::Field(b, _) = &head.kind {
            head = &**b;
        }
        if let ExprKind::Name(_) = head.kind {
            let s = self.byte(head.pos);
            let end = self.dotted_end(s);
            self.add_use((s, end), Use::Capability);
        }
    }

    // ---- queries ----

    /// The code token at `off`: the one holding it, or the identifier or
    /// literal ending at it.
    pub fn token_at(&self, off: usize) -> Option<usize> {
        let i = self.ci(off + 1);
        if let Some(j) = i.checked_sub(1)
            && self.code[j].s <= off
            && off < self.code[j].e
        {
            return Some(j);
        }
        let j = self.ci(off).checked_sub(1)?;
        let t = self.code[j];
        (t.e == off && t.k != K::Punct).then_some(j)
    }

    /// The resolved name holding `off`.
    pub fn ref_at(&self, off: usize) -> Option<Ref> {
        self.refs.iter().copied().find(|r| r.s <= off && off < r.e)
    }

    pub fn use_at(&self, off: usize) -> Option<(usize, usize, Use)> {
        self.uses.iter().copied().find(|u| u.0 <= off && off < u.1)
    }

    /// Locals in scope at `off`, innermost first, one per name.
    pub fn visible_locals(&self, off: usize) -> Vec<usize> {
        let mut out: Vec<usize> = Vec::new();
        let mut found: Vec<usize> = self
            .defs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.visible.is_some_and(|(s, e)| s <= off && off <= e))
            .map(|(i, _)| i)
            .collect();
        found.sort_by_key(|&d| std::cmp::Reverse(self.defs[d].span.0));
        for d in found {
            if !out.iter().any(|&o| self.defs[o].name == self.defs[d].name) {
                out.push(d);
            }
        }
        out
    }

    /// The type of a name at `off`: a local in scope or a script-level
    /// name.
    pub fn type_of_name(&self, name: &str, off: usize) -> T {
        if let Some(d) = self
            .visible_locals(off)
            .into_iter()
            .find(|&d| self.defs[d].name == name)
        {
            return self.defs[d].ty;
        }
        match self.top(name) {
            Some(d) if matches!(self.defs[d].kind, DefKind::Var(_) | DefKind::Option) => self.defs[d].ty,
            _ => T::Unknown,
        }
    }

    /// A type's name as the language writes it.
    pub fn type_name(&self, t: T) -> String {
        match t {
            T::Ty(t) => t.name().to_string(),
            T::Enum(d) => self.defs[d].name.clone(),
            T::Row(d) => format!("a row of {}", self.defs[d].name),
            T::Unknown => "?".to_string(),
        }
    }

    /// The text of the source line holding `off`, trimmed.
    pub fn line_at(&self, off: usize) -> &'s str {
        self.li.line_text(self.li.line_of(off)).trim()
    }
}

pub(crate) enum FieldTarget {
    Def(usize),
    Builtin(usize),
}

/// What `.f` of a value of type `t` reads.
pub(crate) fn field_target(m: &Model, t: T, f: &str) -> Option<FieldTarget> {
    match t {
        T::Ty(ty) => {
            if let Some(scope) = scope_of(ty)
                && let Some(d) = m.scoped(scope, f)
            {
                return Some(FieldTarget::Def(d));
            }
            FIELDS
                .iter()
                .position(|&(o, n, _)| o == ty && n == f)
                .map(FieldTarget::Builtin)
        }
        T::Row(td) => m
            .children(td, DefKind::Column)
            .find(|&c| m.defs[c].name == f)
            .map(FieldTarget::Def),
        _ => None,
    }
}

pub(crate) fn builtin_field_type(i: usize) -> T {
    let id = FIELDS[i].2 as usize;
    caps::ENTRIES
        .get(id)
        .and_then(|e| e.result)
        .map_or(T::Unknown, T::Ty)
}

/// The variable scope a value of this type owns.
pub(crate) fn scope_of(t: Ty) -> Option<Scope> {
    match t {
        Ty::Player => Some(Scope::Player),
        Ty::Team => Some(Scope::Team),
        Ty::Object => Some(Scope::Object),
        _ => None,
    }
}

fn literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_) | ExprKind::None | ExprKind::Str(_) => true,
        ExprKind::Unary(UnOp::Neg, inner) => matches!(inner.kind, ExprKind::Int(_)),
        _ => false,
    }
}

/// Where an item's keyword is.
pub(crate) fn item_pos(item: &Item) -> Pos {
    match item {
        Item::Option(o) => o.pos,
        Item::Palette { pos, .. }
        | Item::Enum { pos, .. }
        | Item::Var { pos, .. }
        | Item::Table { pos, .. }
        | Item::On { pos, .. } => *pos,
        Item::Fn(f) => f.pos,
        Item::Label(l) => l.pos,
    }
}
