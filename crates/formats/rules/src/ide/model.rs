//! The analysis every feature reads: the scan, the recovered syntax tree,
//! the checker's reading of it (`lang::check::Checked`), each declaration
//! with its exact span, and each name use resolved to its declaration or
//! to an API entry, a variant field, an event, a capability or a game type.
//!
//! Types come from the checker and from what a declaration states: an
//! expression's type is `Checked::ty`, a handler argument's is the event's
//! front-end signature (`lang::sig`), a declared label's is its kind.
//! Nothing here works out a type of its own, so the editor and the
//! compiler cannot disagree (docs/game/rules-language-types.md, "The
//! front end"). Scoping follows the checker: a handler sees its arguments,
//! `let`s and set members in scope, and the script's globals, options,
//! tables, enums, functions and labels; a `fn` body sees its parameters
//! and the script's names.

use std::collections::BTreeMap;

use super::scan::{self, K, Tk};
use super::{Position, Range};
use crate::caps::{self, Entry, VARIANT_FIELDS};
use crate::ir::{Event, Scope};
use crate::lang::ast::*;
use crate::lang::check::{self, Checked};
use crate::lang::lower::FIELDS;
use crate::lang::sig;
use crate::lang::types::{Handle, Type};
use crate::lang::{CompileError, Pos, lex, parse};
use crate::settings;

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
    /// A `let` or an `if let`.
    Local,
    /// A `for each` or set member.
    Binder,
    /// A declared Forge label (`label goal: zone`).
    Label,
    /// A `const`, of the type and value the checker folds.
    Const,
    /// A setting named in the `settings` block, by its row in
    /// `settings::TABLE`. The names are the game's: no rename.
    Setting,
}

/// A declaration's type, as the checker or the declaration gives it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum T {
    /// A front-end type: a value's, or a set's (`zones`).
    V(Type),
    /// A global `timer`.
    Timer,
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
    /// A label's item, by index in the script.
    pub item: Option<usize>,
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
    /// The word a `type` or `uses` line names; `None` for a word that is
    /// no game type.
    GameType(Option<crate::types::Type>),
    Type,
    /// The word `variant`.
    Variant,
    /// A setting named on the `quick` line that the `settings` block does
    /// not set, by its index in `settings::TABLE`.
    Setting(u16),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Ref {
    pub s: usize,
    pub e: usize,
    pub def: usize,
    pub decl: bool,
    /// The checker's type of the expression at this use, where it is one.
    pub ty: Option<Type>,
    /// The line (from 1) of the test that narrows the name here: the
    /// `x != none` or `x is goal` in force when the checker's type at the
    /// use is narrower than the declaration's.
    pub narrowed: Option<u32>,
}

/// The source read once: the recovered tree and the lexer's and parser's
/// errors. [`analyze`] borrows it, as the checker's reading does.
pub(crate) struct Parsed {
    pub script: Script,
    pub errors: Vec<CompileError>,
}

pub(crate) fn read(src: &str) -> Parsed {
    let (tokens, mut errors) = lex::lex_tolerant(src);
    let (script, perr) = parse::parse_recovering(&tokens);
    errors.extend(perr);
    Parsed { script, errors }
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
    pub script: &'s Script,
    /// Lexer and parser errors, in order.
    pub errors: &'s [CompileError],
    /// The checker's reading of the recovered tree: a type per expression,
    /// the declared labels and every diagnostic.
    pub checked: Checked<'s>,
    /// The checker's enum table, by `Type::Enum` index.
    pub enum_names: Vec<String>,
    pub defs: Vec<Def>,
    pub refs: Vec<Ref>,
    pub uses: Vec<(usize, usize, Use)>,
    /// Each item's byte span, to its last token.
    pub items: Vec<(usize, usize)>,
    /// Each item's own declaration (a handler has none).
    pub item_defs: Vec<Option<usize>>,
    /// Each declared label's declaration, by name.
    pub labels: BTreeMap<String, usize>,
}

#[derive(Default)]
struct Env {
    locals: Vec<usize>,
    /// The member a `where` reads fields of without naming it.
    implicit: Vec<T>,
    /// The names a test in force narrows, each with the test's line.
    checks: Vec<(String, u32)>,
}

/// The type words a declaration may spell: the front-end types and
/// `timer`.
pub(crate) fn is_type_word(w: &str) -> bool {
    w == "timer" || Type::named(w, false, &[]).is_some()
}

/// The checker's enum table: each enum's name in declaration order, a
/// repeated name once, at most `ty::MAX_ENUMS`.
fn enum_names(script: &Script) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in &script.items {
        if let Item::Enum { name, .. } = item
            && !out.contains(name)
            && out.len() < crate::ty::MAX_ENUMS
        {
            out.push(name.clone());
        }
    }
    out
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

/// Reads `src` with the checker and resolves every name. `p` is
/// [`read`]'s result for the same source.
pub(crate) fn analyze<'s>(src: &'s str, p: &'s Parsed) -> Model<'s> {
    let all = scan::scan(src);
    let code: Vec<Tk> = all.iter().copied().filter(|t| t.k != K::Comment).collect();
    let pairs = pairs_of(src, &code);
    let script: &'s Script = &p.script;
    let mut m = Model {
        src,
        li: LineIndex::new(src),
        all,
        code,
        pairs,
        script,
        errors: &p.errors,
        checked: check::check(script),
        enum_names: enum_names(script),
        defs: Vec::new(),
        refs: Vec::new(),
        uses: Vec::new(),
        items: Vec::new(),
        item_defs: vec![None; script.items.len()],
        labels: BTreeMap::new(),
    };
    m.items = m.item_spans();
    m.declare();
    m.bodies();
    m.refs.sort_by_key(|r| (r.s, r.e));
    m.uses.sort_by_key(|u| (u.0, u.1));
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

    pub fn byte(&self, p: Pos) -> usize {
        self.li.pos_offset(p)
    }

    /// The span of the token at a lexer position.
    pub fn tok_at(&self, p: Pos) -> (usize, usize) {
        let off = self.byte(p);
        match self.code.get(self.ci(off)) {
            Some(t) if t.s == off => (t.s, t.e),
            _ => (off, (off + 1).min(self.src.len())),
        }
    }

    /// The first identifier `name` at or after `off`.
    pub fn ident_from(&self, off: usize, name: &str) -> Option<(usize, usize)> {
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

    fn item_spans(&self) -> Vec<(usize, usize)> {
        let script = self.script;
        let mut bounds: Vec<usize> = script.items.iter().map(|i| self.byte(item_pos(i))).collect();
        let mut header = vec![script.pos];
        header.extend(script.game_type.iter().map(|c| c.1));
        header.extend(script.teams.iter().map(|t| t.1));
        header.extend(script.requires.iter().map(|r| r.1));
        header.extend(script.points.iter().map(|p| p.1));
        header.extend(script.uses.iter().map(|u| u.1));
        header.extend(script.retired.iter().map(|r| r.2));
        header.extend(script.about.iter().map(|a| a.1));
        header.extend(script.quick.iter().map(|q| q.1));
        header.extend(script.settings.iter().map(|b| b.pos));
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
            item: None,
        });
        self.refs.push(Ref {
            s: span.0,
            e: span.1,
            def: d,
            decl: true,
            ty: None,
            narrowed: None,
        });
        d
    }

    fn add_ref(&mut self, span: (usize, usize), def: usize, ty: Option<Type>) {
        self.refs.push(Ref {
            s: span.0,
            e: span.1,
            def,
            decl: false,
            ty,
            narrowed: None,
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
                    DefKind::Var(Scope::Global)
                        | DefKind::Option
                        | DefKind::Table
                        | DefKind::Enum
                        | DefKind::Fn
                        | DefKind::Const
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

    /// The type a declaration spells.
    fn type_named(&self, t: &TypeName) -> T {
        if t.name == "timer" && !t.optional {
            return T::Timer;
        }
        Type::named(&t.name, t.optional, &self.enum_names).map_or(T::Unknown, T::V)
    }

    fn type_use(&mut self, t: &TypeName) {
        let span = self.tok_at(t.pos);
        let d = self.top_kind(&t.name, DefKind::Enum);
        match d {
            Some(d) => self.add_ref(span, d, None),
            None => self.add_use(span, Use::Type),
        }
    }

    /// The type of a value of an enum, by its declaration's name.
    fn enum_type(&self, name: &str) -> T {
        self.enum_names
            .iter()
            .position(|n| n == name)
            .map_or(T::Unknown, |i| T::V(Type::Enum(i as u8)))
    }

    // ---- declarations ----

    fn declare(&mut self) {
        let script: &'s Script = self.script;
        for (name, p) in script.game_type.iter().chain(&script.uses) {
            let span = self.name_after(*p, name);
            self.add_use(span, Use::GameType(crate::types::Type::named(name)));
        }
        for (_, p) in &script.requires {
            let s = self.byte(*p);
            let e = self.dotted_end(s);
            self.add_use((s, e), Use::Capability);
        }
        // Each setting the block names, once: a declaration the `quick`
        // line, a hover and go to definition read.
        if let Some(block) = &script.settings {
            for line in &block.lines {
                let known = settings::named(&line.name).is_some();
                if known && self.top_kind(&line.name, DefKind::Setting).is_none() {
                    let span = self.tok_at(line.pos);
                    self.def(&line.name, DefKind::Setting, span, T::Unknown);
                }
            }
        }
        // Enums first, as the checker reads them, so a type may name one
        // declared further down.
        for (i, item) in script.items.iter().enumerate() {
            if let Item::Enum { name, variants, pos } = item {
                let span = self.name_after(*pos, name);
                let ty = self.enum_type(name);
                let d = self.def(name, DefKind::Enum, span, ty);
                self.item_defs[i] = Some(d);
                let mut at = span.1;
                for v in variants {
                    let Some(vs) = self.ident_from(at, v) else { break };
                    at = vs.1;
                    let m = self.def(v, DefKind::Member, vs, ty);
                    self.defs[m].parent = Some(d);
                }
            }
        }
        for (i, item) in script.items.iter().enumerate() {
            match item {
                Item::Option(o) => {
                    let span = self.name_after(o.pos, &o.name);
                    let ty = self.type_named(&o.ty);
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
                    let t = self.type_named(ty);
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
                        let t = self.type_named(cty);
                        let c = self.def(cname, DefKind::Column, cs, t);
                        self.defs[c].parent = Some(d);
                        self.type_use(cty);
                    }
                }
                Item::Fn(f) => {
                    let span = self.name_after(f.pos, &f.name);
                    // A value function's type is its result's.
                    let ty = f.result.as_ref().map_or(T::Unknown, |r| self.type_named(r));
                    let d = self.def(&f.name, DefKind::Fn, span, ty);
                    self.item_defs[i] = Some(d);
                }
                Item::Const { name, pos, .. } => {
                    let span = self.name_after(*pos, name);
                    let ty = self.checked.constant(name).map_or(T::Unknown, |(t, _)| T::V(t));
                    let d = self.def(name, DefKind::Const, span, ty);
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
                    let ty = match self.checked.label(&l.name) {
                        Some((_, x)) if x.single => x.member().map_or(T::Unknown, |h| T::V(Type::H(h, x.optional))),
                        Some((_, x)) => T::V(Type::Set(x.set())),
                        None => T::Unknown,
                    };
                    self.type_use(&l.kind);
                    let d = self.def(&l.name, DefKind::Label, span, ty);
                    self.defs[d].item = Some(i);
                    self.labels.entry(l.name.clone()).or_insert(d);
                    self.item_defs[i] = Some(d);
                }
                Item::Enum { .. } => {}
            }
        }
        // The `quick` line's names: an option or a setting the block sets
        // is a use of its declaration (so a rename of the option renames
        // its mark); another setting of the table is a use of the table's.
        if let Some((names, _)) = &script.quick {
            for (name, pos) in names {
                let span = self.tok_at(*pos);
                let found = self
                    .top_kind(name, DefKind::Option)
                    .or_else(|| self.top_kind(name, DefKind::Setting));
                match (found, settings::named(name)) {
                    (Some(d), _) => self.add_ref(span, d, None),
                    (None, Some((index, _))) => self.add_use(span, Use::Setting(index)),
                    (None, None) => {}
                }
            }
        }
    }

    // ---- bodies ----

    fn bodies(&mut self) {
        let script: &'s Script = self.script;
        // A `settings` value names a `const` and no variable: a bare name
        // is a choice's value (`shields = none`) unless it is a `const`.
        if let Some(block) = &script.settings {
            let mut env = Env::default();
            for line in &block.lines {
                match &line.value.kind {
                    ExprKind::Name(n) => {
                        if let Some(d) = self.top_kind(n, DefKind::Const) {
                            let span = self.tok_at(line.value.pos);
                            let ty = self.checked.ty(&line.value);
                            self.use_of(span, d, ty, &env);
                        }
                    }
                    _ => self.expr(&line.value, &mut env),
                }
            }
        }
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
                        let t = self.type_named(pty);
                        let d = self.def(pname, DefKind::Param, ps, t);
                        self.defs[d].visible = Some((ps.1, end));
                        self.defs[d].parent = Some(fd);
                        self.type_use(pty);
                        env.locals.push(d);
                    }
                    if let Some(r) = &f.result {
                        self.type_use(r);
                    }
                    // `fn f(..) -> T = expr`, or a block.
                    if let Some(v) = &f.value {
                        self.expr(v, &mut env);
                    }
                    self.block(&f.body, &mut env, end);
                }
                Item::Const { value, .. } => {
                    self.expr(value, &mut env);
                }
                Item::Label(l) => {
                    for e in requirement_exprs(&l.requirements) {
                        self.expr(e, &mut env);
                    }
                }
                Item::On {
                    event,
                    args,
                    label,
                    body,
                    pos,
                } => {
                    self.handler(i, event, args, label.as_ref(), *pos, end, &mut env);
                    self.block(body, &mut env, end);
                }
                _ => {}
            }
        }
    }

    /// A handler's arguments. A label in an argument's place is a use of
    /// the label: `capture` in `on enter(capture z, o)`, and `goal` in
    /// `on enter(goal, o)`, whose body reads `goal` as the zone entered.
    fn handler(
        &mut self,
        item: usize,
        event: &str,
        args: &[String],
        label: Option<&(String, usize)>,
        pos: Pos,
        end: usize,
        env: &mut Env,
    ) {
        let ev = Event::named(event);
        let by_label = self.checked.handler_label(item);
        let mut at = self.name_after(pos, event).1;
        if let Some((lname, _)) = label
            && let Some(span) = self.ident_from(at, lname)
        {
            at = span.1;
            if let Some(&ld) = self.labels.get(lname.as_str()) {
                self.add_ref(span, ld, None);
            }
        }
        for (k, a) in args.iter().enumerate() {
            let Some(span) = self.ident_from(at, a) else { break };
            at = span.1;
            if ev == Some(Event::Timer) {
                // `on timer(t)` names a global timer.
                if let Some(g) = self.scoped(Scope::Global, a) {
                    self.add_ref(span, g, None);
                }
                continue;
            }
            let labelled = by_label.filter(|&(_, lk)| lk == k).map(|(ix, _)| ix);
            if label.is_none()
                && labelled.is_some()
                && let Some(&ld) = self.labels.get(a.as_str())
            {
                self.add_ref(span, ld, None);
                env.locals.push(ld);
                continue;
            }
            let ty = match labelled.and_then(|ix| self.checked.labels.get(ix as usize)) {
                Some(l) => l.member().map_or(T::Unknown, |h| T::V(Type::H(h, false))),
                None => ev
                    .and_then(|e| sig::event_args(e).get(k).copied())
                    .map_or(T::Unknown, T::V),
            };
            let d = self.def(a, DefKind::Arg, span, ty);
            self.defs[d].visible = Some((span.1, end));
            self.defs[d].event = ev;
            env.locals.push(d);
        }
    }

    fn block(&mut self, body: &[Stmt], env: &mut Env, end: usize) {
        let mark = env.locals.len();
        for s in body {
            self.stmt(s, env, end);
        }
        env.locals.truncate(mark);
    }

    /// A block read with the names `cond` narrows when it is `when`.
    fn block_after_test(&mut self, cond: Option<&Expr>, when: bool, body: &[Stmt], env: &mut Env, end: usize) {
        let mark = env.checks.len();
        if let Some(c) = cond {
            tested(c, when, &mut env.checks);
        }
        self.block(body, env, end);
        env.checks.truncate(mark);
    }

    /// The `else` of a block that closes at `close`: `else if` and
    /// `else if let` go on as a statement of their own, a plain `else` is
    /// a block. `cond` is the test the `else` follows, false there.
    fn else_branch(&mut self, cond: Option<&Expr>, els: &[Stmt], close: usize, env: &mut Env, end: usize) {
        if els.is_empty() {
            return;
        }
        let after = self.ci(close);
        let else_if = self.is_word(after, "else") && self.is_word(after + 1, "if");
        match els {
            [inner @ (Stmt::If { .. } | Stmt::IfLet { .. })] if else_if => {
                let mark = env.checks.len();
                if let Some(c) = cond {
                    tested(c, false, &mut env.checks);
                }
                self.stmt(inner, env, end);
                env.checks.truncate(mark);
            }
            _ => {
                let (_, eclose) = self.block_after(close);
                self.block_after_test(cond, false, els, env, eclose);
            }
        }
    }

    fn stmt(&mut self, s: &Stmt, env: &mut Env, end: usize) {
        match s {
            Stmt::Let { name, ty, value, pos } => {
                self.expr(value, env);
                let t = match ty {
                    Some(t) => self.type_named(t),
                    None => self.row_of(value, env).unwrap_or_else(|| self.value_type(value)),
                };
                let span = self.name_after(*pos, name);
                let d = self.def(name, DefKind::Local, span, t);
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
                self.expr(value, env);
                let ty = self.checked.ty(value).map_or(T::Unknown, |t| T::V(t.unwrapped()));
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
                self.block_after_test(cond.as_ref(), true, then, env, close);
                env.locals.truncate(mark);
                // The else has no binding, and a false `cond` proves
                // nothing alone.
                self.else_branch(None, els, close, env, end);
            }
            Stmt::Expr(e) => {
                self.expr(e, env);
            }
            Stmt::If { cond, then, els } => {
                self.expr(cond, env);
                let (_, close) = self.block_after(self.byte(cond.pos));
                self.block_after_test(Some(cond), true, then, env, close);
                self.else_branch(Some(cond), els, close, env, end);
            }
            Stmt::ForEach {
                set,
                binder,
                body,
                pos,
            } => {
                let member = self.set(set, env);
                // `for each <set> <binder>`: past `for`, `each` and the set.
                let base = self.skip_set(self.ci(self.byte(*pos)) + 2);
                let span = match self.code.get(base) {
                    Some(t) if self.text(t) == binder => (t.s, t.e),
                    _ => self.ident_from(self.byte(*pos), binder).unwrap_or((0, 0)),
                };
                let (_, close) = self.block_after(span.1);
                let mark = env.locals.len();
                let ty = member.map_or(T::Unknown, T::V);
                let d = self.def(binder, DefKind::Binder, span, ty);
                self.defs[d].visible = Some((span.1, close));
                env.locals.push(d);
                if let Some(f) = &set.filter {
                    env.implicit.push(ty);
                    self.expr(f, env);
                    env.implicit.pop();
                }
                self.block_after_test(set.filter.as_ref(), true, body, env, close);
                env.locals.truncate(mark);
            }
            Stmt::Match { value, arms, els, .. } => {
                self.expr(value, env);
                let (_, whole) = self.block_after(self.byte(value.pos));
                let mut after = self.byte(value.pos);
                for arm in arms {
                    for p in &arm.patterns {
                        self.expr(p, env);
                    }
                    let from = arm.patterns.last().map_or(self.byte(arm.pos), |p| self.byte(p.pos));
                    let (_, close) = self.block_after(from);
                    self.block(&arm.body, env, close.min(whole));
                    after = close;
                }
                if let Some(body) = els {
                    let (_, close) = self.block_after(after);
                    self.block(body, env, close.min(whole));
                }
            }
        }
    }

    /// `table[i]`, a row: the table's declaration.
    fn row_of(&self, value: &Expr, env: &Env) -> Option<T> {
        let ExprKind::Index(base, _) = &value.kind else { return None };
        let ExprKind::Name(n) = &base.kind else { return None };
        if self.local(n, env).is_some() {
            return None;
        }
        self.top_kind(n, DefKind::Table).map(T::Row)
    }

    /// The checker's type of an expression.
    fn value_type(&self, e: &Expr) -> T {
        self.checked.ty(e).map_or(T::Unknown, T::V)
    }

    /// The code index past a set base starting at `i`.
    fn skip_set(&self, i: usize) -> usize {
        let Some(t) = self.code.get(i) else { return i };
        match self.text(t) {
            "pick" if self.is_punct(i + 1, "(") => self.partner(i + 1).map_or(i + 2, |c| c + 1),
            _ => i + 1,
        }
    }

    /// Walks a set's base; the type of its member, as the set's
    /// declaration states it.
    fn set(&mut self, set: &SetExpr, env: &mut Env) -> Option<Type> {
        match &set.base {
            SetBaseExpr::Players => Some(Type::PLAYER),
            SetBaseExpr::Teams => Some(Type::TEAM),
            SetBaseExpr::Objectives => Some(Type::OBJECT),
            SetBaseExpr::Label(l) => {
                let span = self.ident_from(self.byte(set.pos), l).unwrap_or_else(|| self.tok_at(set.pos));
                let d = self.local(l, env).or_else(|| self.labels.get(l.as_str()).copied())?;
                self.add_ref(span, d, None);
                match self.defs[d].ty {
                    T::V(Type::Set(k)) => k.member().map(|h| Type::H(h, false)),
                    _ => None,
                }
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
    fn set_arg(&mut self, set: &SetExpr, env: &mut Env, call: Pos) {
        let member = self.set(set, env);
        let ty = member.map_or(T::Unknown, T::V);
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
            let d = self.def(binder, DefKind::Binder, span, ty);
            self.defs[d].visible = Some((span.1, close));
            env.locals.push(d);
        }
        if let Some(f) = &set.filter {
            env.implicit.push(ty);
            self.expr(f, env);
            env.implicit.pop();
        }
        env.locals.truncate(mark);
    }

    fn local(&self, name: &str, env: &Env) -> Option<usize> {
        env.locals
            .iter()
            .rev()
            .copied()
            .find(|&d| self.defs[d].name == name)
    }

    /// A use of a declaration with the checker's type there, and the test
    /// that narrows it when that type is narrower than the declared one.
    fn use_of(&mut self, span: (usize, usize), d: usize, ty: Option<Type>, env: &Env) {
        let narrower = match (self.defs[d].ty, ty) {
            (T::V(declared), Some(here)) => declared != here && here.fits(declared),
            _ => false,
        };
        let name = &self.defs[d].name;
        let narrowed = if narrower {
            env.checks.iter().rev().find(|(n, _)| n == name).map(|&(_, line)| line)
        } else {
            None
        };
        self.refs.push(Ref {
            s: span.0,
            e: span.1,
            def: d,
            decl: false,
            ty,
            narrowed,
        });
    }

    fn expr(&mut self, e: &Expr, env: &mut Env) {
        match &e.kind {
            ExprKind::Int(_)
            | ExprKind::Fixed(_)
            | ExprKind::Time(..)
            | ExprKind::Bool(_)
            | ExprKind::Str(_)
            | ExprKind::None
            | ExprKind::Budget(_) => {}
            ExprKind::Name(n) => self.name(n, e, env),
            ExprKind::Field(base, f) => self.field(base, f, e, env),
            ExprKind::Index(base, index) => {
                let table = match &base.kind {
                    ExprKind::Name(n) if self.local(n, env).is_none() => self.top_kind(n, DefKind::Table),
                    _ => None,
                };
                match table {
                    Some(d) => {
                        let span = self.tok_at(base.pos);
                        self.add_ref(span, d, None);
                    }
                    None => self.expr(base, env),
                }
                self.expr(index, env);
            }
            ExprKind::Call(name, args) => self.call(name, args, e.pos, env),
            ExprKind::Unary(_, inner) | ExprKind::Team(inner) | ExprKind::Text(inner) => self.expr(inner, env),
            ExprKind::Binary(op @ (BinOp::And | BinOp::Or), a, b) => {
                // The right side is read with what the left proves.
                self.expr(a, env);
                let mark = env.checks.len();
                tested(a, *op == BinOp::And, &mut env.checks);
                self.expr(b, env);
                env.checks.truncate(mark);
            }
            ExprKind::Binary(_, a, b) => {
                self.expr(a, env);
                self.expr(b, env);
            }
            ExprKind::Is(x, l, lpos) => {
                self.expr(x, env);
                let span = self.tok_at(*lpos);
                if let Some(&d) = self.labels.get(l.as_str()) {
                    self.add_ref(span, d, None);
                }
            }
            ExprKind::Palette(p) => {
                let span = self.str_from(self.tok_at(e.pos).1);
                if let Some(d) = self.top_kind(p, DefKind::Palette) {
                    self.add_ref(span, d, None);
                }
            }
            ExprKind::Set(set) => {
                self.set(set, env);
            }
        }
    }

    fn name(&mut self, n: &str, e: &Expr, env: &mut Env) {
        let span = self.tok_at(e.pos);
        let ty = self.checked.ty(e);
        if let Some(d) = self.local(n, env) {
            self.use_of(span, d, ty, env);
            return;
        }
        if let Some(&t) = env.implicit.last()
            && self.field_of(t, n, span, ty)
        {
            return;
        }
        if n == "variant" {
            self.add_use(span, Use::Variant);
            return;
        }
        if let Some(d) = self.top(n).or_else(|| self.labels.get(n).copied()) {
            self.use_of(span, d, ty, env);
        }
    }

    /// The span of the field name after the `.` at `dot`.
    fn field_span(&self, dot: Pos) -> (usize, usize) {
        let i = self.ci(self.byte(dot));
        match self.code.get(i + 1) {
            Some(t) if self.is_punct(i, ".") => (t.s, t.e),
            _ => self.tok_at(dot),
        }
    }

    /// `base.f`, whose position is its dot.
    fn field(&mut self, base: &Expr, f: &str, e: &Expr, env: &mut Env) {
        let span = self.field_span(e.pos);
        let ty = self.checked.ty(e);
        if let ExprKind::Name(b) = &base.kind
            && self.local(b, env).is_none()
        {
            if b == "variant" {
                let bspan = self.tok_at(base.pos);
                self.add_use(bspan, Use::Variant);
                if let Some(i) = VARIANT_FIELDS.iter().position(|(n, _)| *n == f) {
                    self.add_use(span, Use::VariantField(i));
                }
                return;
            }
            if let Some(ed) = self.top_kind(b, DefKind::Enum) {
                let bspan = self.tok_at(base.pos);
                self.add_ref(bspan, ed, None);
                let member = self.children(ed, DefKind::Member).find(|&m| self.defs[m].name == f);
                if let Some(m) = member {
                    self.add_ref(span, m, ty);
                }
                return;
            }
        }
        self.expr(base, env);
        let owner = self.owner(base, env);
        self.field_of(owner, f, span, ty);
    }

    /// The type whose field `base.f` reads: a row's table, or the
    /// checker's type of `base`.
    fn owner(&self, base: &Expr, env: &Env) -> T {
        match &base.kind {
            ExprKind::Name(n) => match self.local(n, env).map(|d| self.defs[d].ty) {
                Some(row @ T::Row(_)) => row,
                _ => self.value_type(base),
            },
            ExprKind::Index(..) => self.row_of(base, env).unwrap_or_else(|| self.value_type(base)),
            _ => self.value_type(base),
        }
    }

    /// A field of a value of type `t`: a scoped variable, a table column
    /// or a built-in read. Records the use; whether there was one.
    fn field_of(&mut self, t: T, f: &str, span: (usize, usize), ty: Option<Type>) -> bool {
        match field_target(self, t, f) {
            Some(FieldTarget::Def(d)) => {
                self.add_ref(span, d, ty);
                true
            }
            Some(FieldTarget::Builtin(i)) => {
                self.add_use(span, Use::Field(i));
                true
            }
            None => false,
        }
    }

    fn call(&mut self, name: &str, args: &[Expr], pos: Pos, env: &mut Env) {
        let span = self.tok_at(pos);
        if let Some(l) = super::info::lang_index(name) {
            self.add_use(span, Use::Lang(l));
            for (k, a) in args.iter().enumerate() {
                match (&a.kind, k) {
                    (ExprKind::Set(set), 0) => self.set_arg(set, env, pos),
                    (_, 1) if matches!(name, "grant" | "revoke" | "granted") => self.cap_arg(a),
                    _ => self.expr(a, env),
                }
            }
            return;
        }
        if let Some(d) = self.top_kind(name, DefKind::Fn) {
            self.add_ref(span, d, None);
        } else if let Some(entry) = caps::entry_named(name) {
            self.add_use(span, Use::Api(entry));
        }
        for a in args {
            self.expr(a, env);
        }
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

    /// The declared type of a name at `off`: a local in scope, or a
    /// script-level variable, option or label.
    pub fn type_of_name(&self, name: &str, off: usize) -> T {
        if let Some(d) = self
            .visible_locals(off)
            .into_iter()
            .find(|&d| self.defs[d].name == name)
        {
            return self.defs[d].ty;
        }
        match self.top(name).or_else(|| self.labels.get(name).copied()) {
            Some(d) if matches!(self.defs[d].kind, DefKind::Var(_) | DefKind::Option | DefKind::Label | DefKind::Const) => {
                self.defs[d].ty
            }
            _ => T::Unknown,
        }
    }

    /// The engine index the script's record is on (`ir::Header::engine`):
    /// the `uses` game's, else the type's, when the header names them.
    pub fn engine(&self) -> Option<u8> {
        use crate::types::Type;
        let named = |w: Option<&str>| w.and_then(Type::named);
        let (ty, uses) = if self.script.game_type.is_some() || self.script.uses.is_some() {
            let w = |l: &Option<(String, Pos)>| l.as_ref().map(|(n, _)| n.clone());
            (w(&self.script.game_type), w(&self.script.uses))
        } else {
            // A header that did not parse (a `settings` block left open)
            // has no tree: read the `type` and `uses` lines from the
            // tokens.
            (self.line_word("type"), self.line_word("uses"))
        };
        match uses {
            Some(u) => named(Some(u.as_str())).filter(|t| t.is_retail()).map(Type::engine),
            None => named(ty.as_deref()).map(Type::engine),
        }
    }

    /// The word after `word` where `word` starts a line.
    fn line_word(&self, word: &str) -> Option<String> {
        let i = (0..self.code.len()).find(|&i| {
            self.is_word(i, word)
                && (i == 0 || self.li.line_of(self.code[i - 1].s) < self.li.line_of(self.code[i].s))
                && self.code.get(i + 1).is_some_and(|t| t.k == K::Ident)
        })?;
        Some(self.text(&self.code[i + 1]).to_string())
    }

    /// The code indices of the header words `settings`, `quick`, `type`,
    /// `uses`, `scoring`, `points` and `about`: the first word of a
    /// top-level line followed by what the parser looks for (a `{`, a name
    /// or a string). Elsewhere they are names.
    pub fn header_words(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut depth = 0i32;
        for (i, t) in self.code.iter().enumerate() {
            match t.k {
                K::Punct => match self.text(t) {
                    "{" | "(" | "[" => depth += 1,
                    "}" | ")" | "]" => depth = (depth - 1).max(0),
                    _ => {}
                },
                K::Ident if depth == 0 => {
                    let first = i == 0 || self.li.line_of(self.code[i - 1].s) < self.li.line_of(t.s);
                    let Some(next) = self.code.get(i + 1) else { continue };
                    let ok = match self.text(t) {
                        "settings" => next.k == K::Punct && self.text(next) == "{",
                        "quick" | "type" | "uses" | "scoring" => next.k == K::Ident,
                        "points" | "about" | "use" => next.k == K::Str,
                        _ => false,
                    };
                    if first && ok {
                        out.push(i);
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// A type's name as the language writes it.
    pub fn type_name(&self, t: T) -> String {
        match t {
            T::V(t) => t.name(&self.enum_names),
            T::Timer => "timer".to_string(),
            T::Row(d) => format!("a row of {}", self.defs[d].name),
            T::Unknown => "?".to_string(),
        }
    }

    /// The text of the source line holding `off`, trimmed.
    pub fn line_at(&self, off: usize) -> &'s str {
        self.li.line_text(self.li.line_of(off)).trim()
    }

    /// The label declaration `d` is, when it is one.
    pub fn label_decl(&self, d: usize) -> Option<&'s LabelDecl> {
        let script: &'s Script = self.script;
        match script.items.get(self.defs[d].item?) {
            Some(Item::Label(l)) => Some(l),
            _ => None,
        }
    }
}

/// The names a test narrows when it is `when`: `x != none` and `x is
/// goal` through `and` when true, `x == none` through `or` when false,
/// each with the test's line.
fn tested(e: &Expr, when: bool, out: &mut Vec<(String, u32)>) {
    match &e.kind {
        ExprKind::Binary(BinOp::And, a, b) if when => {
            tested(a, true, out);
            tested(b, true, out);
        }
        ExprKind::Binary(BinOp::Or, a, b) if !when => {
            tested(a, false, out);
            tested(b, false, out);
        }
        ExprKind::Unary(UnOp::Not, inner) => tested(inner, !when, out),
        ExprKind::Binary(op @ (BinOp::Eq | BinOp::Ne), a, b) => {
            let x = match (&a.kind, &b.kind) {
                (_, ExprKind::None) => a,
                (ExprKind::None, _) => b,
                _ => return,
            };
            if (*op == BinOp::Ne) == when
                && let ExprKind::Name(n) = &x.kind
            {
                out.push((n.clone(), x.pos.line));
            }
        }
        ExprKind::Is(x, _, _) if when => {
            if let ExprKind::Name(n) = &x.kind {
                out.push((n.clone(), x.pos.line));
            }
        }
        _ => {}
    }
}

pub(crate) enum FieldTarget {
    Def(usize),
    Builtin(usize),
}

/// What `.f` of a value of type `t` reads.
pub(crate) fn field_target(m: &Model, t: T, f: &str) -> Option<FieldTarget> {
    match t {
        T::V(Type::H(h, _)) => {
            if let Some(scope) = scope_of(h)
                && let Some(d) = m.scoped(scope, f)
            {
                return Some(FieldTarget::Def(d));
            }
            let owner = h.erase();
            FIELDS
                .iter()
                .position(|&(o, n, _)| o == owner && n == f)
                .map(FieldTarget::Builtin)
        }
        T::Row(td) => m
            .children(td, DefKind::Column)
            .find(|&c| m.defs[c].name == f)
            .map(FieldTarget::Def),
        _ => None,
    }
}

/// A built-in field's front-end type, from its entry's signature.
pub(crate) fn builtin_field_type(i: usize) -> T {
    let id = FIELDS[i].2 as usize;
    caps::ENTRIES
        .get(id)
        .and_then(sig::of)
        .and_then(|s| s.result)
        .map_or(T::Unknown, T::V)
}

/// The variable scope a handle owns.
pub(crate) fn scope_of(h: Handle) -> Option<Scope> {
    match h {
        Handle::Player => Some(Scope::Player),
        Handle::Team => Some(Scope::Team),
        Handle::Object | Handle::Zone => Some(Scope::Object),
        Handle::Tag | Handle::Weapon => None,
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
        Item::Const { pos, .. } => *pos,
    }
}

/// The constant expressions of a label's requirements, in source order.
pub(crate) fn requirement_exprs(reqs: &[(Requirement, Pos)]) -> Vec<&Expr> {
    let mut out = Vec::new();
    for (r, _) in reqs {
        match r {
            Requirement::AtLeast(e) | Requirement::AtMost(e) | Requirement::Exactly(e) => out.push(e),
            Requirement::Between(a, b) | Requirement::Numbered(Some((a, b))) => {
                out.push(a);
                out.push(b);
            }
            Requirement::Numbered(None) | Requirement::PerTeam => {}
        }
    }
    out
}
