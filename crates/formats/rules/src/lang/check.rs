//! The checker: every name resolved, every expression given a front-end
//! type, the labels declared, handles that may be `none` narrowed, and
//! every error reported (at most [`super::MAX_ERRORS`], in source order).
//!
//! [`check`] returns a [`Checked`] script: the tree, a type per expression,
//! the declared labels and the diagnostics. Lowering (`lower.rs`) reads a
//! `Checked` script with no errors and makes no type decision. See
//! `docs/game/rules-language-types.md`, "The front end", "Labels" and
//! "Stronger typing".

use std::collections::{BTreeMap, BTreeSet, HashMap};

use super::ast::*;
use super::lower::FIELDS;
use super::sig;
use super::types::{Handle, SetKind, Type};
use super::{Diagnostic, MAX_ERRORS, Pos, Severity};
use crate::caps::{self, CARRIERS, ENTRIES, VARIANT_FIELDS, id};
use crate::ir::{self, Event, LabelDef, LabelKind, List, Scope, label_flags};
use crate::ty::{MAX_ENUM_VARIANTS, MAX_ENUMS, MAX_LABELLED, MAX_SQUADS, MAX_TEAMS};

/// A declared label: its declaration as the IR's label table holds it,
/// and what the script sees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Label {
    pub name: String,
    pub def: LabelDef,
    /// One value (`label goal: zone`), not a set.
    pub single: bool,
    /// `object?` or `zone?`.
    pub optional: bool,
    pub pos: Pos,
    /// Uses in the script.
    pub uses: u32,
}

impl Label {
    /// The type of a member; none for spawn areas.
    pub fn member(&self) -> Option<Handle> {
        match self.def.kind {
            LabelKind::Object => Some(Handle::Object),
            LabelKind::Zone => Some(Handle::Zone),
            LabelKind::Spawn => None,
        }
    }

    /// The set a label of many names.
    pub fn set(&self) -> SetKind {
        match self.def.kind {
            LabelKind::Object => SetKind::Objects,
            LabelKind::Zone => SetKind::Zones,
            LabelKind::Spawn => SetKind::Spawns,
        }
    }
}

/// A script the checker has read.
pub struct Checked<'s> {
    pub script: &'s Script,
    /// Errors and warnings, in source order.
    pub diagnostics: Vec<Diagnostic>,
    /// The declared labels, in declaration order: the IR's label table.
    pub labels: Vec<Label>,
    types: HashMap<*const Expr, Type>,
    handler_labels: HashMap<usize, (u16, usize)>,
}

impl<'s> Checked<'s> {
    /// The front-end type of an expression the checker read.
    pub fn ty(&self, e: &Expr) -> Option<Type> {
        self.types.get(&(e as *const Expr)).copied()
    }

    /// The label named `name` and its index in the label table.
    pub fn label(&self, name: &str) -> Option<(u16, &Label)> {
        self.labels
            .iter()
            .position(|l| l.name == name)
            .map(|i| (i as u16, &self.labels[i]))
    }

    /// A handler by label (`on enter(goal, o)`), by item index: the label
    /// and the argument it names.
    pub fn handler_label(&self, item: usize) -> Option<(u16, usize)> {
        self.handler_labels.get(&item).copied()
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }
}

/// Checks a parsed script.
pub fn check(script: &Script) -> Checked<'_> {
    let mut cx = Cx::new(script);
    cx.run();
    cx.finish()
}

/// A name a body binds, and what it is.
#[derive(Clone, Copy, Debug)]
enum BindTy {
    Val(Type),
    /// A table row: the table, and whether it was found by key.
    Row { table: usize, keyed: bool },
    /// A function's set parameter.
    Set(SetKind),
    /// A binding whose type an error left unknown: its uses report nothing.
    Err,
}

struct Bind {
    name: String,
    ty: BindTy,
}

struct Var {
    name: String,
    scope: Scope,
    ty: Type,
    timer: bool,
}

struct Opt {
    name: String,
    ty: Type,
}

struct Table {
    name: String,
    columns: Vec<(String, Type)>,
    /// The first column is an enum and every variant has a row, so a keyed
    /// read always finds one.
    covered: bool,
}

#[derive(Clone, Copy)]
enum Param {
    Val(Type),
    Set(SetKind),
}

/// Bindings known not to be `none` (or narrowed to a label's member), by
/// binding id.
type Facts = Vec<(usize, Type)>;

/// Most diagnostics kept while checking, before sorting.
const MAX_KEPT: usize = 4096;

struct Cx<'s> {
    script: &'s Script,
    out: Vec<Diagnostic>,
    types: HashMap<*const Expr, Type>,
    labels: Vec<Label>,
    enums: Vec<(String, Vec<String>)>,
    enum_names: Vec<String>,
    options: Vec<Opt>,
    option_bits: Vec<u32>,
    vars: Vec<Var>,
    tables: Vec<Table>,
    palettes: Vec<String>,
    fns: BTreeMap<&'s str, &'s FnDecl>,
    fn_params: BTreeMap<&'s str, Vec<Option<Param>>>,
    requires: Vec<(String, Pos)>,
    used_caps: BTreeSet<String>,
    handler_labels: HashMap<usize, (u16, usize)>,
    handler_keys: Vec<(Event, Option<String>, Option<u16>)>,
    calls: BTreeMap<&'s str, Vec<&'s str>>,
    current_fn: Option<&'s str>,
    // The body being checked.
    binds: Vec<Bind>,
    scopes: Vec<Vec<usize>>,
    facts: Vec<Facts>,
    /// The member a `where` reads fields of without naming it; `None` after
    /// an error.
    implicit: Vec<Option<Type>>,
    /// The function whose argument is being checked, for messages.
    arg_of: Option<&'s str>,
}

fn is_none_lit(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::None)
}

/// A literal whose type comes from context.
fn is_literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_) | ExprKind::None | ExprKind::Str(_) => true,
        ExprKind::Unary(UnOp::Neg, inner) => matches!(inner.kind, ExprKind::Int(_)),
        _ => false,
    }
}

/// A constant int: a literal, or a negated one.
fn const_int(e: &Expr) -> Option<i64> {
    match &e.kind {
        ExprKind::Int(n) => Some(*n),
        ExprKind::Unary(UnOp::Neg, inner) => match inner.kind {
            ExprKind::Int(n) => Some(-n),
            _ => None,
        },
        _ => None,
    }
}

/// The source of an expression, short, for messages.
fn describe(e: &Expr) -> String {
    match &e.kind {
        ExprKind::Name(n) => n.clone(),
        ExprKind::Field(b, f) => format!("{}.{f}", describe(b)),
        ExprKind::Call(n, _) => format!("{n}(...)"),
        ExprKind::Index(b, _) => format!("{}[...]", describe(b)),
        ExprKind::Int(n) => n.to_string(),
        ExprKind::Str(s) => format!("{s:?}"),
        ExprKind::None => "none".into(),
        _ => "this value".into(),
    }
}

/// Edit distance, for "did you mean".
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

fn meet(a: &Facts, b: &Facts) -> Facts {
    a.iter().filter(|x| b.contains(x)).copied().collect()
}

fn union(mut a: Facts, b: Facts) -> Facts {
    a.extend(b);
    a
}

impl<'s> Cx<'s> {
    fn new(script: &'s Script) -> Cx<'s> {
        Cx {
            script,
            out: Vec::new(),
            types: HashMap::new(),
            labels: Vec::new(),
            enums: Vec::new(),
            enum_names: Vec::new(),
            options: Vec::new(),
            option_bits: Vec::new(),
            vars: Vec::new(),
            tables: Vec::new(),
            palettes: Vec::new(),
            fns: BTreeMap::new(),
            fn_params: BTreeMap::new(),
            requires: Vec::new(),
            used_caps: BTreeSet::new(),
            handler_labels: HashMap::new(),
            handler_keys: Vec::new(),
            calls: BTreeMap::new(),
            current_fn: None,
            binds: Vec::new(),
            scopes: Vec::new(),
            facts: Vec::new(),
            implicit: Vec::new(),
            arg_of: None,
        }
    }

    // ---- diagnostics ----

    fn diag(&mut self, severity: Severity, code: &'static str, pos: Pos, message: String) {
        if self.out.len() < MAX_KEPT {
            self.out.push(Diagnostic {
                code,
                severity,
                pos,
                message,
            });
        }
    }

    fn err(&mut self, code: &'static str, pos: Pos, message: impl Into<String>) {
        self.diag(Severity::Error, code, pos, message.into());
    }

    fn warn(&mut self, code: &'static str, pos: Pos, message: impl Into<String>) {
        self.diag(Severity::Warning, code, pos, message.into());
    }

    fn finish(mut self) -> Checked<'s> {
        self.out.sort_by_key(|d| d.pos);
        let mut errors = 0;
        self.out.retain(|d| {
            if d.severity == Severity::Error {
                errors += 1;
                errors <= MAX_ERRORS
            } else {
                true
            }
        });
        Checked {
            script: self.script,
            diagnostics: self.out,
            labels: self.labels,
            types: self.types,
            handler_labels: self.handler_labels,
        }
    }

    fn tn(&self, t: Type) -> String {
        t.name(&self.enum_names)
    }

    // ---- the whole script ----

    fn run(&mut self) {
        let script = self.script;
        self.header();
        for item in &script.items {
            if let Item::Enum { name, variants, pos } = item {
                self.enum_decl(name, variants, *pos);
            }
        }
        for item in &script.items {
            if let Item::Label(l) = item {
                self.label_decl(l);
            }
        }
        for item in &script.items {
            match item {
                Item::Option(o) => self.option(o),
                Item::Palette { name, tags, pos } => self.palette(name, tags, *pos),
                Item::Var {
                    scope,
                    name,
                    ty,
                    init,
                    pos,
                } => self.var_decl(*scope, name, ty, init.as_ref(), *pos),
                Item::Table {
                    name,
                    columns,
                    rows,
                    pos,
                } => self.table(name, columns, rows, *pos),
                Item::Fn(f) => {
                    if self.fns.insert(f.name.as_str(), f).is_some() {
                        self.err("decl", f.pos, format!("fn {} is declared twice", f.name));
                    }
                }
                Item::Enum { .. } | Item::Label(_) | Item::On { .. } => {}
            }
        }
        self.label_clashes();
        for item in &script.items {
            if let Item::Fn(f) = item {
                self.fn_params(f);
            }
        }
        for item in &script.items {
            if let Item::Fn(f) = item {
                self.fn_body(f);
            }
        }
        for (index, item) in script.items.iter().enumerate() {
            if let Item::On {
                event,
                args,
                label,
                body,
                pos,
            } = item
            {
                self.handler(index, event, args, label.as_ref(), body, *pos);
            }
        }
        self.recursion();
        self.unused();
    }

    fn header(&mut self) {
        let s = self.script;
        let pos = s.pos;
        if s.name.is_empty() || s.name.len() > ir::MAX_TEXT_BYTES {
            self.err("decl", pos, "a script name is 1 to 128 bytes");
        }
        if u16::try_from(s.version).is_err() {
            self.err("decl", pos, "the version is 0 to 65535");
        }
        match &s.carrier {
            None => self.err("decl", pos, "a script names its carrier: carrier slayer"),
            Some((c, cpos)) => {
                if !CARRIERS.iter().any(|(n, _)| n == c) {
                    self.err("name.unknown", *cpos, format!("unknown carrier {c}"));
                }
            }
        }
        if let Some((n, tpos)) = s.teams
            && !(1..=MAX_TEAMS as i64).contains(&n)
        {
            self.err("decl", tpos, "teams is 1 to 8");
        }
        if let Some((name, lpos)) = &s.list
            && List::named(name).is_none()
        {
            let names: Vec<&str> = List::NAMED.iter().filter_map(|l| l.name()).collect();
            let names = names.join(", ");
            self.err("name.unknown", *lpos, format!("unknown list {name}: a list is {names}"));
        }
        for (name, rpos) in &s.requires {
            if caps::capability(name, caps::API_VERSION).is_none() {
                self.err("name.unknown", *rpos, format!("unknown capability {name}"));
            } else if self.requires.iter().any(|(r, _)| r == name) {
                self.err("decl", *rpos, format!("capability {name} is required twice"));
            } else if self.requires.len() >= ir::MAX_REQUIRES {
                self.err("limit", *rpos, "more than 32 capabilities");
            } else {
                self.requires.push((name.clone(), *rpos));
            }
        }
    }

    fn required(&self, cap: &str) -> bool {
        self.requires.iter().any(|(r, _)| r == cap)
    }

    // ---- declarations ----

    fn enum_decl(&mut self, name: &str, variants: &[String], pos: Pos) {
        if self.enums.iter().any(|(n, _)| n == name) {
            self.err("decl", pos, format!("enum {name} is declared twice"));
            return;
        }
        if self.enums.len() >= MAX_ENUMS {
            self.err("limit", pos, "more than 32 enums");
            return;
        }
        if variants.is_empty() || variants.len() > MAX_ENUM_VARIANTS {
            self.err("limit", pos, "an enum has 1 to 64 variants");
        }
        for (i, v) in variants.iter().enumerate() {
            if variants[..i].contains(v) {
                self.err("decl", pos, format!("{name}.{v} is declared twice"));
            }
        }
        self.enums.push((name.to_string(), variants.to_vec()));
        self.enum_names.push(name.to_string());
    }

    fn label_decl(&mut self, l: &LabelDecl) {
        let pos = l.pos;
        if self.labels.iter().any(|x| x.name == l.name) {
            self.err("decl", pos, format!("label {} is declared twice", l.name));
            return;
        }
        if self.labels.len() >= ir::MAX_LABELS {
            self.err("limit", pos, "a script declares at most 64 labels");
            return;
        }
        let word = l.kind.name.as_str();
        let (kind, single) = match (word, l.kind.optional) {
            ("object", _) => (LabelKind::Object, true),
            ("zone", _) => (LabelKind::Zone, true),
            ("objects", false) => (LabelKind::Object, false),
            ("zones", false) => (LabelKind::Zone, false),
            ("spawns", false) => (LabelKind::Spawn, false),
            _ => {
                self.err(
                    "decl",
                    l.kind.pos,
                    "a label is object, zone, object?, zone?, objects, zones or spawns",
                );
                return;
            }
        };
        let optional = l.kind.optional;
        let text = l.text.clone().unwrap_or_else(|| l.name.clone());
        if text.is_empty() || text.len() > ir::MAX_LABEL_BYTES || !text.chars().all(ir::label_char) {
            if l.text.is_some() {
                self.err(
                    "decl",
                    pos,
                    "a map label's text is 1 to 32 bytes of lowercase letters, digits, _, - and .",
                );
            } else {
                self.err(
                    "decl",
                    pos,
                    format!(
                        "{0} is not a map label's text (1 to 32 bytes of lowercase letters, digits, _, - and .); give the map's text: label {0}: {word} = \"...\"",
                        l.name
                    ),
                );
            }
        }
        if let Some(other) = self.labels.iter().find(|x| x.def.text == text) {
            let message = format!("{} and {} both name the map label \"{text}\". Keep one.", other.name, l.name);
            self.err("label.same-text", pos, message);
        }
        let mut def = LabelDef {
            text,
            kind,
            min: 0,
            max: MAX_LABELLED as u16,
            flags: 0,
            lo: 0,
            hi: 0,
        };
        if single {
            def.flags = label_flags::SINGLE;
            def.max = 1;
            def.min = if optional { 0 } else { 1 };
            if let Some(&(_, rpos)) = l.requirements.first() {
                self.err(
                    "decl",
                    rpos,
                    format!(
                        "{} is one {word}; a requirement is for a set. Declare it as {word}s.",
                        l.name
                    ),
                );
            }
        } else {
            let mut min: Option<i64> = None;
            let mut max: Option<i64> = None;
            let mut numbered = false;
            let mut per_team = false;
            for &(req, rpos) in &l.requirements {
                let fits = |n: i64| (0..=MAX_LABELLED as i64).contains(&n);
                let counts_fit = match req {
                    Requirement::AtLeast(n) | Requirement::AtMost(n) | Requirement::Exactly(n) => fits(n),
                    Requirement::Between(a, b) => fits(a) && fits(b),
                    _ => true,
                };
                if !counts_fit {
                    self.err("decl", rpos, "a label's count is 0 to 256");
                    continue;
                }
                let (lo, hi) = match req {
                    Requirement::AtLeast(n) => (Some(n), None),
                    Requirement::AtMost(n) => (None, Some(n)),
                    Requirement::Exactly(n) => (Some(n), Some(n)),
                    Requirement::Between(a, b) => (Some(a), Some(b)),
                    Requirement::Numbered(range) => {
                        if numbered {
                            self.err("decl", rpos, "numbered is given twice");
                            continue;
                        }
                        numbered = true;
                        def.flags |= label_flags::NUMBERED;
                        if let Some((a, b)) = range {
                            let span = b - a + 1;
                            if a > b {
                                self.err("decl", rpos, format!("numbered {a}..{b} is empty"));
                            } else if span > MAX_LABELLED as i64
                                || i32::try_from(a).is_err()
                                || i32::try_from(b).is_err()
                            {
                                self.err("decl", rpos, format!("numbered {a}..{b} holds more than 256 numbers"));
                            } else {
                                def.flags |= label_flags::RANGE;
                                def.lo = a as i32;
                                def.hi = b as i32;
                            }
                        }
                        continue;
                    }
                    Requirement::PerTeam => {
                        if kind == LabelKind::Spawn {
                            self.err(
                                "decl",
                                rpos,
                                "per team is for objects and zones; starting locations go to spawn_areas by team",
                            );
                        } else if per_team {
                            self.err("decl", rpos, "per team is given twice");
                        } else {
                            def.flags |= label_flags::PER_TEAM;
                        }
                        per_team = true;
                        continue;
                    }
                };
                if (lo.is_some() && min.is_some()) || (hi.is_some() && max.is_some()) {
                    self.err("decl", rpos, "a label states each end of its count once");
                    continue;
                }
                min = min.or(lo);
                max = max.or(hi);
            }
            let (lo, hi) = (min.unwrap_or(0), max.unwrap_or(MAX_LABELLED as i64));
            if lo > hi {
                self.err("decl", pos, format!("at least {lo} is more than at most {hi}"));
            } else {
                def.min = lo as u16;
                def.max = hi as u16;
            }
            if let Some(range) = def.range() {
                let span = (*range.end() as i64) - (*range.start() as i64) + 1;
                if span > def.max as i64 {
                    self.err(
                        "decl",
                        pos,
                        format!(
                            "numbered {}..{} needs {span} members, more than at most {}",
                            range.start(),
                            range.end(),
                            def.max
                        ),
                    );
                }
            }
        }
        self.labels.push(Label {
            name: l.name.clone(),
            def,
            single,
            optional,
            pos,
            uses: 0,
        });
    }

    /// A label's name is its own: no global, option, table or enum has it.
    fn label_clashes(&mut self) {
        let mut found = Vec::new();
        for l in &self.labels {
            let what = if self.vars.iter().any(|v| v.scope == Scope::Global && v.name == l.name) {
                Some("a global variable")
            } else if self.options.iter().any(|o| o.name == l.name) {
                Some("an option")
            } else if self.tables.iter().any(|t| t.name == l.name) {
                Some("a table")
            } else if self.enums.iter().any(|(n, _)| *n == l.name) {
                Some("an enum")
            } else if self.fns.contains_key(l.name.as_str()) {
                Some("a fn")
            } else {
                None
            };
            if let Some(what) = what {
                found.push((l.pos, format!("{} is already {what}; a label needs a name of its own", l.name)));
            }
        }
        for (pos, m) in found {
            self.err("decl", pos, m);
        }
    }

    /// The type a type name names; reports an unknown one.
    fn type_of(&mut self, t: &TypeName) -> Option<Type> {
        if let Some(ty) = Type::named(&t.name, t.optional, &self.enum_names) {
            return Some(ty);
        }
        if t.optional && Type::named(&t.name, false, &self.enum_names).is_some() {
            self.err("decl", t.pos, format!("{} is never none, so it takes no ?", t.name));
        } else {
            self.err("name.unknown", t.pos, format!("unknown type {}", t.name));
        }
        None
    }

    fn option(&mut self, o: &'s OptionDecl) {
        let pos = o.pos;
        if self.options.iter().any(|x| x.name == o.name) {
            self.err("decl", pos, format!("option {} is declared twice", o.name));
            return;
        }
        if self.options.len() >= 32 {
            self.err("limit", pos, "more than 32 options");
            return;
        }
        let Some(ty) = self.type_of(&o.ty) else { return };
        self.options.push(Opt {
            name: o.name.clone(),
            ty,
        });
        let range = match ty {
            Type::Int => {
                let Some((lo, hi)) = &o.range else {
                    self.err("decl", pos, "an int option needs a range: int 1..5");
                    return;
                };
                let lo = self.const_val(lo, Type::Int);
                let hi = self.const_val(hi, Type::Int);
                let (Some(lo), Some(hi)) = (lo, hi) else { return };
                if lo > hi {
                    self.err("decl", pos, "the range is empty");
                    return;
                }
                (lo, hi)
            }
            Type::Bool => (0, 1),
            Type::Enum(e) => (0, self.enums[e as usize].1.len() as i32 - 1),
            _ => {
                self.err("decl", o.ty.pos, "an option is an int, a bool or an enum");
                return;
            }
        };
        if ty != Type::Int && o.range.is_some() {
            self.err("decl", pos, "only an int option takes a range");
        }
        if let Some(d) = self.const_val(&o.default, ty)
            && (d < range.0 || d > range.1)
        {
            self.err("decl", o.default.pos, "the default is outside the range");
        }
        for text in [&o.label, &o.description].into_iter().flatten() {
            if text.len() > ir::MAX_TEXT_BYTES {
                self.err("limit", pos, "text is at most 128 bytes");
            }
        }
        let bits_of = |lo: i32, hi: i32| {
            let span = (hi as i64 - lo as i64).max(0) as u64;
            64 - span.leading_zeros()
        };
        self.option_bits.push(bits_of(range.0, range.1));
        let bits: u32 = self.option_bits.iter().sum();
        if bits > ir::OPTION_BITS {
            self.err("limit", pos, format!("the options take {bits} bits; a variant carries 32"));
        }
    }

    fn palette(&mut self, name: &str, tags: &[String], pos: Pos) {
        if self.palettes.iter().any(|n| n == name) {
            self.err("decl", pos, format!("palette {name} is declared twice"));
            return;
        }
        if self.palettes.len() >= 255 {
            self.err("limit", pos, "more than 255 palettes");
            return;
        }
        if tags.is_empty() || tags.len() > ir::MAX_PALETTE_TAGS {
            self.err("limit", pos, "a palette has 1 to 64 tags");
        }
        if tags.iter().any(|t| t.is_empty() || t.len() > ir::MAX_TAG_BYTES) {
            self.err("limit", pos, "a tag path is 1 to 256 bytes");
        }
        self.palettes.push(name.to_string());
    }

    fn var_decl(&mut self, scope: Scope, name: &str, ty: &TypeName, init: Option<&'s Expr>, pos: Pos) {
        let timer = ty.name == "timer" && !ty.optional;
        if timer && scope != Scope::Global {
            self.err("decl", pos, "a timer is a global variable");
        }
        let t = if timer {
            Type::Int
        } else {
            match self.type_of(ty) {
                Some(Type::Set(_)) => {
                    self.err("decl", ty.pos, "a variable holds a value, not a set");
                    return;
                }
                Some(t) => t,
                None => return,
            }
        };
        if self.vars.iter().any(|v| v.scope == scope && v.name == name) {
            self.err("decl", pos, format!("variable {name} is declared twice"));
            return;
        }
        if scope == Scope::Global && self.options.iter().any(|o| o.name == name) {
            self.err("decl", pos, format!("{name} is already an option"));
        }
        if let Some(o) = scope.owner()
            && FIELDS.iter().any(|&(fo, f, _)| fo == o && f == name)
        {
            self.err("decl", pos, format!("{name} is a built-in field of every {}", o.name()));
        }
        let max = match scope {
            Scope::Global => ir::MAX_GLOBAL_SLOTS,
            Scope::Player => ir::MAX_PLAYER_SLOTS,
            Scope::Team => ir::MAX_TEAM_SLOTS,
            Scope::Object => ir::MAX_OBJECT_SLOTS,
        };
        if self.vars.iter().filter(|v| v.scope == scope).count() >= max {
            self.err("limit", pos, format!("more than {max} variables of this scope"));
        }
        let starts_none = init.is_none_or(is_none_lit);
        if let Type::H(h, false) = t
            && starts_none
        {
            let n = h.name();
            self.err(
                "null.var",
                ty.pos,
                format!("a {n} variable starts as none, so its type is {n}?."),
            );
        } else if let Some(e) = init {
            self.const_val(e, t);
        }
        self.vars.push(Var {
            name: name.to_string(),
            scope,
            ty: t,
            timer,
        });
    }

    fn table(&mut self, name: &str, columns: &[(String, TypeName)], rows: &'s [Vec<Expr>], pos: Pos) {
        if self.tables.iter().any(|t| t.name == name) {
            self.err("decl", pos, format!("table {name} is declared twice"));
            return;
        }
        if self.tables.len() >= 255 {
            self.err("limit", pos, "more than 255 tables");
            return;
        }
        if columns.is_empty() || columns.len() > ir::MAX_TABLE_COLS {
            self.err("limit", pos, "a table has 1 to 16 columns");
            return;
        }
        if rows.len() > ir::MAX_TABLE_ROWS {
            self.err("limit", pos, "a table has at most 256 rows");
        }
        let mut cols = Vec::with_capacity(columns.len());
        for (i, (cname, cty)) in columns.iter().enumerate() {
            let t = if cty.name == "timer" {
                self.err("decl", cty.pos, "a table holds no timers");
                None
            } else {
                self.type_of(cty)
            };
            let t = match t {
                Some(Type::Set(_)) => {
                    self.err("decl", cty.pos, "a table holds values, not sets");
                    None
                }
                t => t,
            };
            if columns[..i].iter().any(|(n, _)| n == cname) {
                self.err("decl", cty.pos, format!("column {cname} is declared twice"));
            }
            cols.push((cname.clone(), t));
        }
        let mut keys = Vec::new();
        for row in rows {
            if row.len() != cols.len() {
                let p = row.first().map_or(pos, |e| e.pos);
                self.err(
                    "decl",
                    p,
                    format!("a row of {name} has {} values, not {}", row.len(), cols.len()),
                );
                continue;
            }
            for (k, (cell, (_, t))) in row.iter().zip(&cols).enumerate() {
                if let Some(t) = *t
                    && let Some(v) = self.const_val(cell, t)
                    && k == 0
                {
                    keys.push(v);
                }
            }
        }
        let columns: Vec<(String, Type)> = cols
            .into_iter()
            .map(|(n, t)| (n, t.unwrap_or(Type::Int)))
            .collect();
        let covered = match columns[0].1 {
            Type::Enum(e) => {
                let n = self.enums[e as usize].1.len() as i32;
                (0..n).all(|v| keys.contains(&v))
            }
            _ => false,
        };
        self.tables.push(Table {
            name: name.to_string(),
            columns,
            covered,
        });
    }

    /// Checks a constant of type `ty` (an initial value, a default, a cell
    /// or a range end) and gives its value where it has one.
    fn const_val(&mut self, e: &'s Expr, ty: Type) -> Option<i32> {
        let pos = e.pos;
        let bad = |cx: &mut Self| {
            let t = cx.tn(ty);
            cx.err("type.mismatch", pos, format!("expected a constant {t}"));
            None
        };
        let v = match (&e.kind, ty) {
            (ExprKind::Int(_) | ExprKind::Unary(UnOp::Neg, _), Type::Int) => match const_int(e) {
                Some(n) => match i32::try_from(n) {
                    Ok(v) => v,
                    Err(_) => {
                        self.err("type.mismatch", pos, "number out of the int range");
                        return None;
                    }
                },
                None => return bad(self),
            },
            (ExprKind::Int(_) | ExprKind::Unary(UnOp::Neg, _), Type::Fixed) => match &e.kind {
                ExprKind::Unary(UnOp::Neg, inner) if matches!(inner.kind, ExprKind::Fixed(_)) => 0,
                _ => match const_int(e).and_then(|n| i32::try_from(n).ok()) {
                    Some(v) => v,
                    None => return bad(self),
                },
            },
            (ExprKind::Fixed(_), Type::Fixed) => 0,
            (ExprKind::Bool(b), Type::Bool) => *b as i32,
            (ExprKind::None, Type::H(_, true)) => crate::ty::NONE,
            (ExprKind::None, Type::H(h, false)) => {
                let n = h.name();
                self.err("null.var", pos, format!("none is not a {n}; a {n} that may be none is {n}?"));
                return None;
            }
            (ExprKind::Str(s), Type::H(Handle::Tag | Handle::Weapon, _)) => {
                if s.is_empty() || s.len() > ir::MAX_TAG_BYTES {
                    self.err("limit", pos, "a tag path is 1 to 256 bytes");
                    return None;
                }
                0
            }
            (ExprKind::Str(s), Type::Class | Type::Text) => {
                if s.len() > ir::MAX_TEXT_BYTES {
                    self.err("limit", pos, "text is at most 128 bytes");
                    return None;
                }
                0
            }
            (ExprKind::Team(inner), Type::H(Handle::Team, _)) => self.const_team(inner)?,
            (ExprKind::Field(base, v), Type::Enum(i)) => {
                let (ename, variants) = &self.enums[i as usize];
                match &base.kind {
                    ExprKind::Name(n) if n == ename => match variants.iter().position(|x| x == v) {
                        Some(k) => k as i32,
                        None => {
                            let m = format!("{ename} has no variant {v}");
                            self.err("name.unknown", pos, m);
                            return None;
                        }
                    },
                    _ => return bad(self),
                }
            }
            (ExprKind::Call(name, args), Type::Squad) if name == "squad" && args.len() == 1 => {
                match const_int(&args[0]) {
                    Some(n) if (0..MAX_SQUADS as i64).contains(&n) => n as i32,
                    _ => {
                        self.err("type.mismatch", pos, "a squad slot is 0 to 15");
                        return None;
                    }
                }
            }
            _ => return bad(self),
        };
        self.types.insert(e as *const Expr, ty);
        Some(v)
    }

    fn const_team(&mut self, inner: &Expr) -> Option<i32> {
        match const_int(inner) {
            Some(n) if (0..MAX_TEAMS as i64).contains(&n) => Some(n as i32),
            _ => {
                self.err("type.mismatch", inner.pos, "a team is 0 to 7");
                None
            }
        }
    }

    fn var(&self, name: &str, scope: Scope) -> Option<&Var> {
        self.vars.iter().find(|v| v.scope == scope && v.name == name)
    }

    fn label_ix(&self, name: &str) -> Option<usize> {
        self.labels.iter().position(|l| l.name == name)
    }

    fn table_ix(&self, name: &str) -> Option<usize> {
        self.tables.iter().position(|t| t.name == name)
    }

    fn close_label(&self, name: &str) -> Option<String> {
        self.labels
            .iter()
            .map(|l| (distance(name, &l.name), &l.name))
            .filter(|(d, _)| *d <= 2 && *d < name.len().max(1))
            .min()
            .map(|(_, n)| n.clone())
    }

    fn unknown_label(&mut self, name: &str, pos: Pos) {
        let m = match self.close_label(name) {
            Some(l) => format!("no label named {name}. Did you mean {l}?"),
            None => format!("no label named {name}. Declare it: label {name}: objects"),
        };
        self.err("label.unknown", pos, m);
    }

    fn unknown_name(&mut self, name: &str, pos: Pos) {
        match self.close_label(name) {
            Some(l) => self.err("label.unknown", pos, format!("no label named {name}. Did you mean {l}?")),
            None => self.err("name.unknown", pos, format!("unknown name {name}")),
        }
    }

    // ---- functions and handlers ----

    fn fn_params(&mut self, f: &'s FnDecl) {
        let mut params = Vec::with_capacity(f.params.len());
        for (_, pty) in &f.params {
            if pty.name == "timer" && !pty.optional {
                self.err("decl", pty.pos, "a parameter is not a timer; pass an int");
                params.push(None);
                continue;
            }
            params.push(match self.type_of(pty) {
                Some(Type::Set(k)) => Some(Param::Set(k)),
                Some(t) => Some(Param::Val(t)),
                None => None,
            });
        }
        self.fn_params.insert(f.name.as_str(), params);
    }

    fn reset_body(&mut self) {
        self.binds.clear();
        self.scopes = vec![Vec::new()];
        self.facts.clear();
        self.implicit.clear();
        self.arg_of = None;
    }

    fn fn_body(&mut self, f: &'s FnDecl) {
        if self.fns.get(f.name.as_str()).is_none_or(|d| !std::ptr::eq(*d, f)) {
            return;
        }
        self.reset_body();
        self.current_fn = Some(f.name.as_str());
        let params = self.fn_params.get(f.name.as_str()).cloned().unwrap_or_default();
        for ((pname, _), p) in f.params.iter().zip(params) {
            let ty = match p {
                Some(Param::Val(t)) => BindTy::Val(t),
                Some(Param::Set(k)) => BindTy::Set(k),
                None => BindTy::Err,
            };
            self.bind(pname, ty);
        }
        self.block(&f.body);
        self.current_fn = None;
    }

    fn handler(
        &mut self,
        index: usize,
        event: &'s str,
        args: &'s [String],
        label: Option<&'s (String, usize)>,
        body: &'s [Stmt],
        pos: Pos,
    ) {
        let Some(ev) = Event::named(event) else {
            self.err("name.unknown", pos, format!("unknown event {event}"));
            return;
        };
        self.reset_body();
        let mut timer = None;
        let mut key_label = None;
        if ev == Event::Timer {
            let [name] = args else {
                self.err("event.args", pos, "on timer names one timer variable: on timer(t)");
                return;
            };
            if !self.var(name, Scope::Global).is_some_and(|v| v.timer) {
                self.err("name.unknown", pos, format!("{name} is not a global timer"));
                return;
            }
            timer = Some(name.clone());
        } else {
            let tys = sig::event_args(ev);
            if args.len() > tys.len() {
                let names: Vec<String> = tys.iter().map(|t| self.tn(*t)).collect();
                let s = if tys.len() == 1 { "" } else { "s" };
                self.err(
                    "event.args",
                    pos,
                    format!(
                        "on {event} gives {} argument{s} ({}); this handler names {}.",
                        tys.len(),
                        names.join(", "),
                        args.len()
                    ),
                );
                return;
            }
            let slot = sig::label_arg(ev);
            let mut labelled: Option<(usize, usize)> = None;
            if let Some((l, k)) = label {
                match slot {
                    Some((sk, _)) if sk == *k => match self.label_ix(l) {
                        Some(ix) => labelled = Some((ix, *k)),
                        None => {
                            self.unknown_label(l, pos);
                            return;
                        }
                    },
                    Some((sk, want)) => {
                        let m = format!("on {event} takes {} label in place of argument {}", want.a(), sk + 1);
                        self.err("event.label", pos, m);
                        return;
                    }
                    None => {
                        self.err("event.label", pos, format!("on {event} takes no label"));
                        return;
                    }
                }
            } else if let Some((sk, _)) = slot
                && let Some(a) = args.get(sk)
                && let Some(ix) = self.label_ix(a)
            {
                labelled = Some((ix, sk));
            }
            if let Some((ix, k)) = labelled {
                let want = slot.map_or(Handle::Object, |s| s.1);
                let (name, member) = (self.labels[ix].name.clone(), self.labels[ix].member());
                match member {
                    None => {
                        let m = format!("{name} names spawn areas; on {event} takes {} label.", want.a());
                        self.err("event.label", pos, m);
                        return;
                    }
                    Some(Handle::Object) if want == Handle::Zone => {
                        let m = format!("{name} is an object; on {event} takes a zone label.");
                        self.err("event.label", pos, m);
                        return;
                    }
                    _ => {}
                }
                self.labels[ix].uses += 1;
                self.handler_labels.insert(index, (ix as u16, k));
                key_label = Some(ix as u16);
            }
            for (k, a) in args.iter().enumerate() {
                let t = match labelled {
                    Some((ix, lk)) if lk == k => Type::H(self.labels[ix].member().unwrap_or(Handle::Object), false),
                    _ => tys[k],
                };
                self.bind(a, BindTy::Val(t));
            }
        }
        let key = (ev, timer, key_label);
        if self.handler_keys.contains(&key) {
            let m = match key_label {
                Some(ix) => format!("a second handler for on {event}({})", self.labels[ix as usize].name),
                None => format!("a second handler for on {event}"),
            };
            self.err("decl", pos, m);
        } else {
            self.handler_keys.push(key);
        }
        self.block(body);
    }

    /// A fn that reaches itself, or a chain of more than 16, is refused:
    /// functions are inlined.
    fn recursion(&mut self) {
        fn depth<'a>(
            f: &'a str,
            calls: &BTreeMap<&'a str, Vec<&'a str>>,
            active: &mut Vec<&'a str>,
            memo: &mut BTreeMap<&'a str, usize>,
        ) -> Option<usize> {
            if let Some(&d) = memo.get(f) {
                return Some(d);
            }
            if active.contains(&f) {
                return None;
            }
            if active.len() > 16 {
                return Some(0);
            }
            active.push(f);
            let mut best = 0;
            for &callee in calls.get(f).map_or(&[][..], |v| v.as_slice()) {
                match depth(callee, calls, active, memo) {
                    Some(d) => best = best.max(d + 1),
                    None => {
                        active.pop();
                        return None;
                    }
                }
            }
            active.pop();
            memo.insert(f, best);
            Some(best)
        }
        let mut memo = BTreeMap::new();
        let names: Vec<&'s str> = self.fns.keys().copied().collect();
        for name in names {
            let pos = self.fns[name].pos;
            match depth(name, &self.calls, &mut Vec::new(), &mut memo) {
                None => {
                    self.err("decl", pos, format!("fn {name} calls itself; recursion is not allowed"));
                    return;
                }
                Some(d) if d >= 16 => {
                    self.err("limit", pos, "functions nest more than 16 deep");
                    return;
                }
                Some(_) => {}
            }
        }
    }

    fn unused(&mut self) {
        let mut found = Vec::new();
        for l in &self.labels {
            if l.uses == 0 {
                let m = if l.def.min > 0 {
                    format!("label {} is declared but never used; a map would still need one.", l.name)
                } else {
                    format!("label {} is declared but never used.", l.name)
                };
                found.push(("label.unused", l.pos, m));
            }
        }
        for (cap, pos) in &self.requires {
            if self.used_caps.contains(cap) {
                continue;
            }
            let Some(c) = caps::capability(cap, caps::API_VERSION) else { continue };
            // A capability no entry gates (`hud.superbounce`) is a host
            // feature the declaration itself turns on.
            let gates = c.grantable || ENTRIES.iter().any(|e| e.cap == Some(c.name));
            if gates {
                let m = format!(
                    "{cap} is required but nothing uses it; the lobby would still say \"{}\".",
                    c.words
                );
                found.push(("cap.unused", *pos, m));
            }
        }
        for (code, pos, m) in found {
            self.warn(code, pos, m);
        }
    }

    // ---- scopes ----

    fn bind(&mut self, name: &str, ty: BindTy) -> usize {
        let id = self.binds.len();
        self.binds.push(Bind {
            name: name.to_string(),
            ty,
        });
        if self.scopes.is_empty() {
            self.scopes.push(Vec::new());
        }
        if let Some(s) = self.scopes.last_mut() {
            s.push(id);
        }
        id
    }

    fn lookup(&self, name: &str) -> Option<usize> {
        self.scopes
            .iter()
            .rev()
            .flat_map(|s| s.iter().rev())
            .copied()
            .find(|&id| self.binds[id].name == name)
    }

    /// A binding's type where it is read, narrowed by the facts in force.
    fn current(&self, id: usize, declared: Type) -> Type {
        for frame in self.facts.iter().rev() {
            if let Some(&(_, t)) = frame.iter().rev().find(|(i, _)| *i == id) {
                return t;
            }
        }
        declared
    }

    fn shadow_check(&mut self, name: &str, pos: Pos) {
        let what = if self.label_ix(name).is_some() {
            Some("label")
        } else if self.var(name, Scope::Global).is_some() {
            Some("global")
        } else if self.options.iter().any(|o| o.name == name) {
            Some("option")
        } else {
            None
        };
        if let Some(what) = what {
            self.warn("name.shadow", pos, format!("{name} shadows the {what} {name}."));
        }
    }

    // ---- statements ----

    fn block(&mut self, body: &'s [Stmt]) {
        self.scopes.push(Vec::new());
        for s in body {
            self.stmt(s);
        }
        self.scopes.pop();
    }

    /// A block with `facts` in force.
    fn block_with(&mut self, facts: Facts, body: &'s [Stmt]) {
        self.facts.push(facts);
        self.block(body);
        self.facts.pop();
    }

    fn stmt(&mut self, s: &'s Stmt) {
        match s {
            Stmt::Let { name, ty, value, pos } => self.let_stmt(name, ty.as_ref(), value, *pos),
            Stmt::Assign { target, value, pos } => self.assign(target, value, *pos),
            Stmt::Expr(e) => match &e.kind {
                ExprKind::Call(name, args) => {
                    self.call(name, args, e.pos, false);
                }
                _ => self.err(
                    "type.mismatch",
                    e.pos,
                    "this expression does nothing; call something or assign it",
                ),
            },
            Stmt::If { cond, then, els } => {
                let (t, f) = self.cond(cond);
                self.block_with(t, then);
                self.block_with(f, els);
            }
            Stmt::IfLet {
                name,
                value,
                cond,
                then,
                els,
                pos,
            } => {
                let bound = match self.expr(value, None) {
                    Some(Type::H(h, opt)) => {
                        if !opt {
                            self.warn("null.never", value.pos, format!("{} is never none here.", describe(value)));
                        }
                        BindTy::Val(Type::H(h, false))
                    }
                    Some(Type::Set(_)) | None => BindTy::Err,
                    Some(other) => {
                        let m = format!(
                            "if let binds a value that may be none; {} is {}, which never is",
                            describe(value),
                            self.tn(other)
                        );
                        self.err("type.mismatch", value.pos, m);
                        BindTy::Err
                    }
                };
                self.scopes.push(Vec::new());
                self.shadow_check(name, *pos);
                self.bind(name, bound);
                let facts = match cond {
                    Some(c) => self.cond(c).0,
                    None => Vec::new(),
                };
                self.block_with(facts, then);
                self.scopes.pop();
                self.block(els);
            }
            Stmt::ForEach {
                set,
                binder,
                body,
                pos,
            } => {
                let member = self.set_source(set);
                self.scopes.push(Vec::new());
                self.shadow_check(binder, *pos);
                self.bind(binder, member.map_or(BindTy::Err, BindTy::Val));
                let facts = match &set.filter {
                    Some(f) => {
                        self.implicit.push(member);
                        let (t, _) = self.cond(f);
                        self.implicit.pop();
                        t
                    }
                    None => Vec::new(),
                };
                self.block_with(facts, body);
                self.scopes.pop();
            }
        }
    }

    fn let_stmt(&mut self, name: &'s str, ty: Option<&'s TypeName>, value: &'s Expr, pos: Pos) {
        // A table row: `let kit = loadouts[p.team]`.
        if let ExprKind::Index(base, index) = &value.kind
            && let ExprKind::Name(t) = &base.kind
            && self.lookup(t).is_none()
            && let Some(ti) = self.table_ix(t)
        {
            if let Some(ty) = ty {
                self.err("type.mismatch", ty.pos, "a row has no type to state");
            }
            let bt = match self.row(ti, index) {
                Some(keyed) => BindTy::Row { table: ti, keyed },
                None => BindTy::Err,
            };
            self.shadow_check(name, pos);
            self.bind(name, bt);
            return;
        }
        let want = match ty {
            Some(t) => match self.type_of(t) {
                Some(Type::Set(_)) => {
                    self.err("type.mismatch", t.pos, "a let holds a value, not a set");
                    None
                }
                Some(w) => Some(w),
                None => {
                    self.bind(name, BindTy::Err);
                    return;
                }
            },
            None => None,
        };
        if want.is_none() && ty.is_none() && is_none_lit(value) {
            self.err(
                "type.mismatch",
                value.pos,
                format!("let {name} = none has no type. State it: let {name}: player? = none"),
            );
            self.bind(name, BindTy::Err);
            return;
        }
        let got = match want {
            Some(w) => self.expect(value, w, None),
            None => self.expr(value, None),
        };
        let t = match (want, got) {
            (Some(w), Some(_)) => Some(w),
            (None, g) => g,
            _ => None,
        };
        self.shadow_check(name, pos);
        self.bind(name, t.map_or(BindTy::Err, BindTy::Val));
    }

    fn assign(&mut self, target: &'s Expr, value: &'s Expr, pos: Pos) {
        match &target.kind {
            ExprKind::Name(n) => {
                if self.lookup(n).is_some() {
                    self.err("decl", pos, format!("{n} is a let or a parameter and cannot be assigned"));
                    return;
                }
                if let Some(t) = self.var(n, Scope::Global).map(|v| v.ty) {
                    self.expect(value, t, None);
                    return;
                }
                if self.options.iter().any(|o| o.name == *n) {
                    self.err("decl", pos, format!("option {n} is read-only"));
                } else if self.label_ix(n).is_some() {
                    self.err("decl", pos, format!("{n} is a label and cannot be assigned"));
                } else {
                    self.err("name.unknown", pos, format!("unknown variable {n}"));
                }
            }
            ExprKind::Field(base, field) => {
                let Some(bt) = self.expr(base, None) else { return };
                if bt.is_opt() {
                    self.null_use(base);
                    return;
                }
                let scope = match bt.handle() {
                    Some(Handle::Player) => Scope::Player,
                    Some(Handle::Team) => Scope::Team,
                    Some(Handle::Object | Handle::Zone) => Scope::Object,
                    _ => {
                        let m = format!("a {} has no variables", self.tn(bt));
                        self.err("type.mismatch", pos, m);
                        return;
                    }
                };
                let Some(t) = self.var(field, scope).map(|v| v.ty) else {
                    let owner = scope.owner().map_or("", |o| o.name());
                    self.err("name.unknown", pos, format!("{field} is not a {owner} variable"));
                    return;
                };
                self.expect(value, t, None);
            }
            _ => self.err("type.mismatch", pos, "only a variable can be assigned"),
        }
    }

    // ---- conditions and narrowing ----

    /// Checks a condition; what it proves when true, and when false.
    fn cond(&mut self, e: &'s Expr) -> (Facts, Facts) {
        let r = match &e.kind {
            ExprKind::Binary(BinOp::And, a, b) => {
                let (ta, fa) = self.cond(a);
                self.facts.push(ta.clone());
                let (tb, fb) = self.cond(b);
                self.facts.pop();
                (union(ta, tb), meet(&fa, &fb))
            }
            ExprKind::Binary(BinOp::Or, a, b) => {
                let (ta, fa) = self.cond(a);
                self.facts.push(fa.clone());
                let (tb, fb) = self.cond(b);
                self.facts.pop();
                (meet(&ta, &tb), union(fa, fb))
            }
            ExprKind::Unary(UnOp::Not, inner) => {
                let (t, f) = self.cond(inner);
                (f, t)
            }
            ExprKind::Binary(op @ (BinOp::Eq | BinOp::Ne), a, b) if is_none_lit(a) || is_none_lit(b) => {
                if self.expr(e, None).is_none() {
                    return (Vec::new(), Vec::new());
                }
                let x = if is_none_lit(a) { b } else { a };
                let fact: Facts = self.none_test(x).into_iter().collect();
                if *op == BinOp::Eq { (Vec::new(), fact) } else { (fact, Vec::new()) }
            }
            ExprKind::Is(x, l, _) => {
                if self.expr(e, None).is_none() {
                    return (Vec::new(), Vec::new());
                }
                let mut fact = Vec::new();
                if let ExprKind::Name(n) = &x.kind
                    && let Some(id) = self.lookup(n)
                    && let BindTy::Val(declared) = self.binds[id].ty
                    && let Some(ix) = self.label_ix(l)
                    && let Some(member) = self.labels[ix].member()
                {
                    let now = self.current(id, declared);
                    let t = if member == Handle::Zone { Type::ZONE } else { now.unwrapped() };
                    fact.push((id, t));
                }
                (fact, Vec::new())
            }
            _ => {
                self.expect(e, Type::Bool, None);
                return (Vec::new(), Vec::new());
            }
        };
        self.types.insert(e as *const Expr, Type::Bool);
        r
    }

    /// `x == none` or `x != none`: the binding it narrows, if `x` is a
    /// let, a parameter or a loop member that may be none.
    fn none_test(&mut self, x: &'s Expr) -> Option<(usize, Type)> {
        let t = self.types.get(&(x as *const Expr)).copied()?;
        if t.handle().is_some() && !t.is_opt() {
            self.warn("null.never", x.pos, format!("{} is never none here.", describe(x)));
            return None;
        }
        let ExprKind::Name(n) = &x.kind else { return None };
        let id = self.lookup(n)?;
        match self.binds[id].ty {
            BindTy::Val(_) if t.is_opt() => Some((id, t.unwrapped())),
            _ => None,
        }
    }

    fn null_fix(&self, e: &Expr) -> String {
        let x = describe(e);
        match &e.kind {
            ExprKind::Name(n) if self.lookup(n).is_some() => {
                format!("Check it with \"if {x} != none\", or bind it with \"if let {x} = ...\".")
            }
            _ => format!("Bind it first: \"if let v = {x} {{ ... }}\"."),
        }
    }

    fn null_use(&mut self, e: &'s Expr) {
        let m = format!("{} may be none here. {}", describe(e), self.null_fix(e));
        self.err("null.use", e.pos, m);
    }

    // ---- expressions ----

    /// Checks `e` fits `want`, naming `callee` in what it reports.
    fn expect(&mut self, e: &'s Expr, want: Type, callee: Option<&'s str>) -> Option<Type> {
        let saved = self.arg_of;
        self.arg_of = callee;
        let got = self.expr(e, Some(want));
        self.arg_of = saved;
        let got = got?;
        if got.fits(want) {
            return Some(got);
        }
        self.mismatch(e, got, want, callee);
        None
    }

    fn mismatch(&mut self, e: &'s Expr, got: Type, want: Type, callee: Option<&str>) {
        let x = describe(e);
        let who = callee.unwrap_or("this");
        match (got, want) {
            (Type::H(g, true), Type::H(w, _)) if g.fits(w) => {
                let fix = self.null_fix(e);
                let m = match callee {
                    Some(c) => format!("{c} takes {}, and {x} may be none here. {fix}", w.a()),
                    None => format!("{x} may be none here. {fix}"),
                };
                self.err("null.arg", e.pos, m);
            }
            (Type::H(Handle::Object, _), Type::H(Handle::Zone, _)) => {
                let label = match &e.kind {
                    ExprKind::Name(n) if self.lookup(n).is_none() && self.label_ix(n).is_some() => Some(n),
                    _ => None,
                };
                match label {
                    Some(n) => {
                        let m = format!(
                            "{n} is an object, and {who} takes a zone. If the map's {n} is a volume, declare it \"label {n}: zone\"."
                        );
                        self.err("label.kind", e.pos, m);
                    }
                    None => {
                        let m = format!(
                            "{who} takes a zone, and {x} is an object. Narrow it with \"{x} is <zone label>\"."
                        );
                        self.err("type.mismatch", e.pos, m);
                    }
                }
            }
            (Type::H(Handle::Tag, _), Type::H(Handle::Weapon, _)) => {
                let hint = match &e.kind {
                    ExprKind::Field(..) => "Declare the column as weapon.",
                    _ => "Name a weapon's tag path, or declare it as weapon.",
                };
                self.err("tag.group", e.pos, format!("{who} takes a weapon, and {x} is a tag. {hint}"));
            }
            (Type::Set(g), Type::Set(w)) => {
                self.err("label.kind", e.pos, format!("{x} is {}, and {who} takes {}.", g.words(), w.words()));
            }
            _ => {
                let m = format!("expected {}, found {}", self.tn(want), self.tn(got));
                self.err("type.mismatch", e.pos, m);
            }
        }
    }

    /// Checks `e`, with `want` the type its context wants (it types
    /// literals and set names); records and returns its type, `None` after
    /// an error.
    fn expr(&mut self, e: &'s Expr, want: Option<Type>) -> Option<Type> {
        let callee = self.arg_of.take();
        let t = self.expr_inner(e, want, callee)?;
        self.types.insert(e as *const Expr, t);
        Some(t)
    }

    fn expr_inner(&mut self, e: &'s Expr, want: Option<Type>, callee: Option<&'s str>) -> Option<Type> {
        let pos = e.pos;
        match &e.kind {
            ExprKind::Int(n) => {
                if i32::try_from(*n).is_err() {
                    self.err("type.mismatch", pos, "number out of the int range");
                    return None;
                }
                Some(if want == Some(Type::Fixed) { Type::Fixed } else { Type::Int })
            }
            ExprKind::Fixed(_) => Some(Type::Fixed),
            ExprKind::Bool(_) => Some(Type::Bool),
            ExprKind::None => match want {
                Some(Type::H(h, _)) => Some(Type::H(h, true)),
                _ => {
                    self.err("type.mismatch", pos, "none needs a player, team, object or tag here");
                    None
                }
            },
            ExprKind::Str(s) => match want {
                Some(Type::H(h @ (Handle::Tag | Handle::Weapon), _)) => {
                    if s.is_empty() || s.len() > ir::MAX_TAG_BYTES {
                        self.err("limit", pos, "a tag path is 1 to 256 bytes");
                        return None;
                    }
                    Some(Type::H(h, false))
                }
                Some(t @ (Type::Class | Type::Text)) => {
                    if s.len() > ir::MAX_TEXT_BYTES {
                        self.err("limit", pos, "text is at most 128 bytes");
                        return None;
                    }
                    Some(t)
                }
                Some(Type::Set(k)) => {
                    let m = format!(
                        "a label is named by its declaration, not by a string: declare \"label {s}: {}\" and write {s}",
                        k.name()
                    );
                    self.err("label.unknown", pos, m);
                    None
                }
                _ => {
                    self.err("type.mismatch", pos, "a string is a tag path, a class or a text here");
                    None
                }
            },
            ExprKind::Team(inner) => {
                self.const_team(inner)?;
                Some(Type::TEAM)
            }
            ExprKind::Text(inner) => match &inner.kind {
                ExprKind::Str(s) if s.len() <= ir::MAX_TEXT_BYTES => Some(Type::Text),
                ExprKind::Int(n) if i32::try_from(*n).is_ok() => Some(Type::Text),
                _ => {
                    self.err("type.mismatch", pos, "text takes a string or a string-table index");
                    None
                }
            },
            ExprKind::Name(n) => self.name(n, pos, want, callee),
            ExprKind::Field(base, f) => self.field(base, f, pos),
            ExprKind::Index(base, index) => self.index(base, index, pos),
            ExprKind::Call(name, args) => self.call(name, args, pos, true).flatten(),
            ExprKind::Unary(UnOp::Neg, inner) => {
                let t = self.expr(inner, want)?;
                if !t.numeric() {
                    self.err("type.mismatch", pos, "- takes an int or a fixed");
                    return None;
                }
                Some(t)
            }
            ExprKind::Unary(UnOp::Not, _) | ExprKind::Binary(BinOp::And | BinOp::Or, ..) => {
                self.cond(e);
                Some(Type::Bool)
            }
            ExprKind::Binary(op, a, b) => self.binary(*op, a, b, want, pos),
            ExprKind::Is(x, l, lpos) => self.is_test(x, l, *lpos),
            ExprKind::Budget(_) | ExprKind::Palette(_) => {
                self.err("type.mismatch", pos, "budget and palette are arguments of grant");
                None
            }
            ExprKind::Set(_) => {
                self.err("type.mismatch", pos, "a set is only allowed in for each, count, first and pick");
                None
            }
        }
    }

    fn name(&mut self, n: &'s str, pos: Pos, want: Option<Type>, callee: Option<&str>) -> Option<Type> {
        if let Some(id) = self.lookup(n) {
            return match self.binds[id].ty {
                BindTy::Val(t) => Some(self.current(id, t)),
                BindTy::Err => None,
                BindTy::Row { .. } => {
                    self.err("type.mismatch", pos, format!("{n} is a row: read a column, as {n}.x"));
                    None
                }
                BindTy::Set(k) => self.set_value(n, k, want, pos, callee),
            };
        }
        match self.implicit.last().copied() {
            Some(Some(m)) => {
                if let Some(t) = self.field_type(m, n) {
                    return Some(t);
                }
            }
            Some(None) => {
                // The member's type is unknown: the name may be its field.
                if self.var(n, Scope::Global).is_none()
                    && !self.options.iter().any(|o| o.name == n)
                    && self.label_ix(n).is_none()
                {
                    return None;
                }
            }
            None => {}
        }
        if let Some(t) = self.var(n, Scope::Global).map(|v| v.ty) {
            return Some(t);
        }
        if let Some(t) = self.options.iter().find(|o| o.name == n).map(|o| o.ty) {
            return Some(t);
        }
        if let Some(ix) = self.label_ix(n) {
            return self.label_value(ix, want, pos, callee);
        }
        if let Some(v) = self.vars.iter().find(|v| v.name == n) {
            let owner = v.scope.owner().map_or("its owner", |t| t.name());
            self.err("name.unknown", pos, format!("{n} is a variable of each {owner}: read it as x.{n}"));
            return None;
        }
        self.unknown_name(n, pos);
        None
    }

    /// A set named where a value or a set is read.
    fn set_value(&mut self, n: &str, kind: SetKind, want: Option<Type>, pos: Pos, callee: Option<&str>) -> Option<Type> {
        match want {
            Some(Type::Set(w)) if kind.fits(w) => Some(Type::Set(kind)),
            Some(Type::Set(w)) => {
                let who = callee.unwrap_or("this");
                self.err("label.kind", pos, format!("{n} is {}, and {who} takes {}.", kind.words(), w.words()));
                None
            }
            _ if kind == SetKind::Spawns => {
                let m = format!("{n} names spawn areas, which have no objects to visit. Pass it to spawn_areas.");
                self.err("label.spawns", pos, m);
                None
            }
            _ => {
                let b: String = n.chars().next().map_or("x".into(), |c| c.to_string());
                let words = kind.words();
                let m = match callee {
                    Some(c) => format!(
                        "{n} is {words}, and {c} takes one. Loop with \"for each {n} {b}\", or take one with first({n}) or {n}[n]."
                    ),
                    None => format!(
                        "{n} is {words}, not one value. Loop with \"for each {n} {b}\", or take one with first({n}) or {n}[n]."
                    ),
                };
                self.err("label.set-as-one", pos, m);
                None
            }
        }
    }

    fn label_value(&mut self, ix: usize, want: Option<Type>, pos: Pos, callee: Option<&str>) -> Option<Type> {
        self.labels[ix].uses += 1;
        let l = &self.labels[ix];
        if l.single {
            return l.member().map(|m| Type::H(m, l.optional));
        }
        let (name, kind) = (l.name.clone(), l.set());
        self.set_value(&name, kind, want, pos, callee)
    }

    /// The type of `owner.f`: a scoped variable or a built-in field.
    fn field_type(&self, owner: Type, f: &str) -> Option<Type> {
        let scope = match owner.handle()? {
            Handle::Player => Scope::Player,
            Handle::Team => Scope::Team,
            Handle::Object | Handle::Zone => Scope::Object,
            _ => return None,
        };
        if let Some(v) = self.var(f, scope) {
            return Some(v.ty);
        }
        let ir_owner = owner.erase()?;
        let &(_, _, id) = FIELDS.iter().find(|&&(o, n, _)| o == ir_owner && n == f)?;
        sig::of(&ENTRIES[id as usize])?.result
    }

    fn field(&mut self, base: &'s Expr, f: &'s str, pos: Pos) -> Option<Type> {
        match &base.kind {
            ExprKind::Name(b) => match self.lookup(b).map(|id| self.binds[id].ty) {
                Some(BindTy::Row { table, keyed }) => return self.column(table, keyed, f, pos),
                Some(_) => {}
                None => {
                    if b == "variant" {
                        return match VARIANT_FIELDS.iter().find(|(n, _)| *n == f) {
                            Some(&(_, t)) => Type::from_ir(t),
                            None => {
                                self.err("name.unknown", pos, format!("variant has no field {f}"));
                                None
                            }
                        };
                    }
                    if let Some(i) = self.enums.iter().position(|(n, _)| n == b) {
                        if !self.enums[i].1.iter().any(|v| v == f) {
                            self.err("name.unknown", pos, format!("{b} has no variant {f}"));
                            return None;
                        }
                        return Some(Type::Enum(i as u8));
                    }
                }
            },
            ExprKind::Index(tb, index) => {
                if let ExprKind::Name(t) = &tb.kind
                    && self.lookup(t).is_none()
                    && let Some(ti) = self.table_ix(t)
                {
                    let keyed = self.row(ti, index)?;
                    return self.column(ti, keyed, f, pos);
                }
            }
            _ => {}
        }
        let bt = self.expr(base, None)?;
        if bt.is_opt() {
            self.null_use(base);
            return None;
        }
        if f == "label" && matches!(bt.handle(), Some(Handle::Object | Handle::Zone)) {
            let x = describe(base);
            self.err(
                "name.unknown",
                pos,
                format!("{x} has no label field: test it with \"{x} is <label>\""),
            );
            return None;
        }
        match self.field_type(bt, f) {
            Some(t) => Some(t),
            None => {
                let m = format!("a {} has no field {f}", self.tn(bt));
                self.err("name.unknown", pos, m);
                None
            }
        }
    }

    fn column(&mut self, table: usize, keyed: bool, f: &str, pos: Pos) -> Option<Type> {
        let info = &self.tables[table];
        let Some(c) = info.columns.iter().position(|(n, _)| n == f) else {
            let m = format!("table {} has no column {f}", info.name);
            self.err("name.unknown", pos, m);
            return None;
        };
        let t = info.columns[c].1;
        // A key no row holds reads the zero row, whose handles are none.
        Some(if keyed && t.handle().is_some() && !info.covered { t.optional() } else { t })
    }

    /// `table[index]`: whether the row is found by key rather than by
    /// number.
    fn row(&mut self, ti: usize, index: &'s Expr) -> Option<bool> {
        let key = self.tables[ti].columns[0].1;
        let it = self.expr(index, Some(key))?;
        if it == Type::Int {
            return Some(false);
        }
        if it.unwrapped().fits(key.unwrapped()) {
            return Some(true);
        }
        let m = format!(
            "{} is indexed by int or {}, not {}",
            self.tables[ti].name,
            self.tn(key),
            self.tn(it)
        );
        self.err("type.mismatch", index.pos, m);
        None
    }

    fn index(&mut self, base: &'s Expr, index: &'s Expr, pos: Pos) -> Option<Type> {
        let ExprKind::Name(n) = &base.kind else {
            self.err("type.mismatch", pos, "only a table or a label can be indexed");
            return None;
        };
        if let Some(id) = self.lookup(n) {
            let m = match self.binds[id].ty {
                BindTy::Err => return None,
                BindTy::Row { .. } => format!("{n} is a row: read a column, as {n}.x"),
                BindTy::Set(_) => format!("{n}[...] needs the label itself: index the label, not a parameter"),
                BindTy::Val(_) => "only a table or a label can be indexed".into(),
            };
            self.err("type.mismatch", pos, m);
            return None;
        }
        if self.table_ix(n).is_some() {
            self.err("type.mismatch", pos, format!("a row is read by column: {n}[i].column"));
            return None;
        }
        let Some(ix) = self.label_ix(n) else {
            self.unknown_name(n, base.pos);
            return None;
        };
        let l = self.labels[ix].clone();
        let name = &l.name;
        let Some(member) = l.member() else {
            let m = format!("{name} names spawn areas, which have no objects to visit. Pass it to spawn_areas.");
            self.err("label.spawns", pos, m);
            return None;
        };
        if l.single {
            let w = member.name();
            let m = format!("{name} is one {w}, not a set. Use it directly, or declare it as {w}s.");
            self.err("label.one-as-set", pos, m);
            return None;
        }
        self.labels[ix].uses += 1;
        let it = self.expr(index, None)?;
        match it {
            Type::Int => {
                if !l.def.numbered() {
                    let m = format!(
                        "{name} is not numbered, so {name}[n] has no meaning. Add \"numbered\" to its declaration."
                    );
                    self.err("label.index", pos, m);
                    return None;
                }
                if let (Some(n), Some(range)) = (const_int(index), l.def.range()) {
                    if n >= *range.start() as i64 && n <= *range.end() as i64 {
                        return Some(Type::H(member, false));
                    }
                    let m = format!("{name}[{n}] is outside \"numbered {}..{}\".", range.start(), range.end());
                    self.err("label.index", pos, m);
                    return None;
                }
                Some(Type::H(member, true))
            }
            Type::H(Handle::Team, false) => Some(Type::H(member, true)),
            Type::H(Handle::Team, true) => {
                let fix = self.null_fix(index);
                let m = format!("{name}[t] takes a team, and {} may be none here. {fix}", describe(index));
                self.err("null.arg", index.pos, m);
                None
            }
            other => {
                let m = format!("{name}[...] takes a number or a team, not {}", self.tn(other));
                self.err("type.mismatch", index.pos, m);
                None
            }
        }
    }

    fn is_test(&mut self, x: &'s Expr, l: &str, lpos: Pos) -> Option<Type> {
        let t = self.expr(x, None)?;
        match t {
            Type::H(Handle::Object | Handle::Zone, false) => {}
            Type::H(Handle::Object | Handle::Zone, true) => {
                self.null_use(x);
                return None;
            }
            other => {
                let m = format!("is tests an object or a zone, and {} is {}", describe(x), self.tn(other));
                self.err("type.mismatch", x.pos, m);
                return None;
            }
        }
        let Some(ix) = self.label_ix(l) else {
            self.unknown_label(l, lpos);
            return None;
        };
        if self.labels[ix].def.kind == LabelKind::Spawn {
            let m = format!("{l} names spawn areas, which have no objects to test. Pass it to spawn_areas.");
            self.err("label.spawns", lpos, m);
            return None;
        }
        self.labels[ix].uses += 1;
        Some(Type::Bool)
    }

    fn binary(&mut self, op: BinOp, a: &'s Expr, b: &'s Expr, want: Option<Type>, pos: Pos) -> Option<Type> {
        let arith = matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem);
        let hint = if arith { want.filter(|t| t.numeric()) } else { None };
        let eq = matches!(op, BinOp::Eq | BinOp::Ne);
        // A literal takes its type from the other side.
        let ta = if is_literal(a) && !is_literal(b) {
            let tb = self.expr(b, hint)?;
            self.operand(a, tb, eq)?;
            tb
        } else {
            let ta = self.expr(a, hint)?;
            self.operand(b, ta, eq)?;
            ta
        };
        if eq {
            return Some(Type::Bool);
        }
        if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) {
            if !ta.numeric() {
                let m = format!("{} values have no order", self.tn(ta));
                self.err("type.mismatch", pos, m);
                return None;
            }
            return Some(Type::Bool);
        }
        if !ta.numeric() {
            let m = format!("arithmetic on {}", self.tn(ta));
            self.err("type.mismatch", pos, m);
            return None;
        }
        if op == BinOp::Rem && ta == Type::Fixed {
            self.err("type.mismatch", pos, "% takes ints");
            return None;
        }
        Some(ta)
    }

    /// The second operand, against the first's type: `==` and `!=` compare
    /// a handle with its `?` or `none`; the rest want the same type.
    fn operand(&mut self, e: &'s Expr, other: Type, eq: bool) -> Option<Type> {
        if !eq {
            return self.expect(e, other, None);
        }
        let t = self.expr(e, Some(other.optional()))?;
        if t.comparable(other) {
            return Some(t);
        }
        let m = format!("expected {}, found {}", self.tn(other), self.tn(t));
        self.err("type.mismatch", e.pos, m);
        None
    }

    // ---- sets ----

    /// A loop's set: the type of its members.
    fn set_source(&mut self, set: &'s SetExpr) -> Option<Type> {
        match &set.base {
            SetBaseExpr::Players => Some(Type::PLAYER),
            SetBaseExpr::Teams => Some(Type::TEAM),
            SetBaseExpr::Objectives => Some(Type::OBJECT),
            SetBaseExpr::Label(n) => self.set_named(n, set.pos),
            SetBaseExpr::Pick(inner, k) => {
                if inner.binder.is_some() || inner.filter.is_some() {
                    self.err("type.mismatch", inner.pos, "pick takes a set with no name and no where");
                }
                if matches!(inner.base, SetBaseExpr::Pick(..)) {
                    self.err("type.mismatch", inner.pos, "pick of a pick");
                    return None;
                }
                let m = self.set_source(inner);
                self.expect(k, Type::Int, Some("pick"));
                m
            }
        }
    }

    fn set_named(&mut self, n: &str, pos: Pos) -> Option<Type> {
        let spawns = |cx: &mut Self| {
            let m = format!("{n} names spawn areas, which have no objects to visit. Pass it to spawn_areas.");
            cx.err("label.spawns", pos, m);
            None
        };
        if let Some(id) = self.lookup(n) {
            return match self.binds[id].ty {
                BindTy::Set(SetKind::Spawns) => spawns(self),
                BindTy::Set(k) => k.member().map(|h| Type::H(h, false)),
                BindTy::Err => None,
                _ => {
                    let m = format!("{n} is not a set: a loop visits players, teams, objectives or a label");
                    self.err("type.mismatch", pos, m);
                    None
                }
            };
        }
        let Some(ix) = self.label_ix(n) else {
            self.unknown_label(n, pos);
            return None;
        };
        let l = &self.labels[ix];
        let Some(member) = l.member() else { return spawns(self) };
        if l.single {
            let w = member.name();
            let m = format!("{n} is one {w}, not a set. Use it directly, or declare it as {w}s.");
            self.err("label.one-as-set", pos, m);
            return None;
        }
        self.labels[ix].uses += 1;
        Some(Type::H(member, false))
    }

    /// A set argument of `count` or `first`: its member's type.
    fn set_arg(&mut self, set: &'s SetExpr) -> Option<Type> {
        let member = self.set_source(set);
        self.scopes.push(Vec::new());
        if let Some(b) = &set.binder {
            self.bind(b, member.map_or(BindTy::Err, BindTy::Val));
        }
        if let Some(f) = &set.filter {
            self.implicit.push(member);
            self.cond(f);
            self.implicit.pop();
        }
        self.scopes.pop();
        member
    }

    // ---- calls ----

    /// Checks a call; `Some(None)` for one with no value.
    fn call(&mut self, name: &'s str, args: &'s [Expr], pos: Pos, need_value: bool) -> Option<Option<Type>> {
        let arity = |cx: &mut Self, n: usize| {
            if args.len() == n {
                true
            } else {
                cx.err("type.mismatch", pos, format!("{name} takes {n} arguments"));
                false
            }
        };
        let no_value = |cx: &mut Self| {
            if need_value {
                cx.err("type.mismatch", pos, format!("{name} has no value"));
            }
            !need_value
        };
        match name {
            "count" | "first" => {
                if !arity(self, 1) {
                    return None;
                }
                let ExprKind::Set(set) = &args[0].kind else {
                    self.err("type.mismatch", pos, format!("{name} takes a set"));
                    return None;
                };
                let member = self.set_arg(set);
                return Some(Some(if name == "count" { Type::Int } else { member?.optional() }));
            }
            "pick" => {
                self.err("type.mismatch", pos, "pick makes a set: use it in for each or count");
                return None;
            }
            "len" => {
                if !arity(self, 1) {
                    return None;
                }
                let ExprKind::Name(t) = &args[0].kind else {
                    self.err("type.mismatch", pos, "len takes a table");
                    return None;
                };
                if self.table_ix(t).is_none() {
                    self.err("name.unknown", pos, format!("unknown table {t}"));
                    return None;
                }
                return Some(Some(Type::Int));
            }
            "seconds" => {
                if !arity(self, 1) {
                    return None;
                }
                self.expect(&args[0], Type::Int, Some("seconds"))?;
                return Some(Some(Type::Int));
            }
            "grant" => {
                if !no_value(self) {
                    return None;
                }
                if args.len() < 3 || args.len() > 4 {
                    self.err("type.mismatch", pos, "grant takes a player, a capability, a budget and a palette");
                    return None;
                }
                self.expect(&args[0], Type::PLAYER, Some("grant"));
                self.cap_arg(&args[1]);
                let (mut uses, mut palette) = (false, false);
                for a in &args[2..] {
                    match &a.kind {
                        ExprKind::Budget(n) if !uses => {
                            if !(0..=255).contains(n) {
                                self.err("limit", a.pos, "a grant holds at most 255 uses");
                            }
                            uses = true;
                        }
                        ExprKind::Palette(p) if !palette => {
                            if !self.palettes.contains(p) {
                                self.err("name.unknown", a.pos, format!("unknown palette {p}"));
                            }
                            palette = true;
                        }
                        _ => self.err("type.mismatch", a.pos, "expected budget n or palette \"x\""),
                    }
                }
                if !uses {
                    self.err("type.mismatch", pos, "a grant needs a budget: budget n");
                }
                return Some(None);
            }
            "revoke" | "granted" => {
                if !arity(self, 2) {
                    return None;
                }
                if name == "revoke" && !no_value(self) {
                    return None;
                }
                self.expect(&args[0], Type::PLAYER, Some(name));
                self.cap_arg(&args[1]);
                return Some(if name == "revoke" { None } else { Some(Type::Bool) });
            }
            _ => {}
        }
        if let Some(&decl) = self.fns.get(name) {
            return self.fn_call(decl, args, pos, need_value);
        }
        if name == "message" {
            return self.message(args, pos, need_value);
        }
        let Some(entry) = caps::entry_named(name) else {
            self.err("name.unknown", pos, format!("unknown function {name}"));
            return None;
        };
        let Some(sig) = sig::of(entry) else {
            let m = format!("{name} is not called by name: test a label with \"o is <label>\"");
            self.err("name.unknown", pos, m);
            return None;
        };
        if !arity(self, sig.args.len()) {
            return None;
        }
        if let Some(cap) = entry.cap {
            if !self.required(cap) {
                let m = format!("{name} needs capability {cap}: add it to requires");
                self.err("cap.missing", pos, m);
            }
            self.used_caps.insert(cap.to_string());
        }
        let mut ok = true;
        for (a, &want) in args.iter().zip(&sig.args) {
            match self.expect(a, want, Some(name)) {
                Some(got) => {
                    if entry.id == id::DELETE && got.handle() == Some(Handle::Zone) {
                        let x = describe(a);
                        let m = format!(
                            "{x} is a zone. A flag cannot be deleted, so on a map where {x} is a flag this does nothing."
                        );
                        self.warn("zone.delete", a.pos, m);
                    }
                }
                None => ok = false,
            }
        }
        if sig.result.is_none() && !no_value(self) {
            return None;
        }
        ok.then_some(sig.result)
    }

    fn fn_call(&mut self, decl: &'s FnDecl, args: &'s [Expr], pos: Pos, need_value: bool) -> Option<Option<Type>> {
        let name = decl.name.as_str();
        let params = self.fn_params.get(name).cloned().unwrap_or_default();
        if args.len() != params.len() {
            self.err("type.mismatch", pos, format!("{name} takes {} arguments", params.len()));
            return None;
        }
        if let Some(cur) = self.current_fn {
            self.calls.entry(cur).or_default().push(name);
        }
        for (a, p) in args.iter().zip(params) {
            match p {
                Some(Param::Val(t)) => {
                    self.expect(a, t, Some(name));
                }
                Some(Param::Set(k)) => {
                    self.expect(a, Type::Set(k), Some(name));
                }
                None => {}
            }
        }
        if need_value {
            self.err("type.mismatch", pos, format!("fn {name} has no value"));
            return None;
        }
        Some(None)
    }

    /// `message(to, text)`: `to` is a player, a team or `all`.
    fn message(&mut self, args: &'s [Expr], pos: Pos, need_value: bool) -> Option<Option<Type>> {
        if need_value {
            self.err("type.mismatch", pos, "message has no value");
            return None;
        }
        if args.len() != 2 {
            self.err("type.mismatch", pos, "message takes 2 arguments: message(to, text)");
            return None;
        }
        let cap = "hud.message";
        if !self.required(cap) {
            self.err("cap.missing", pos, format!("message needs capability {cap}: add it to requires"));
        }
        self.used_caps.insert(cap.to_string());
        let to = &args[0];
        match &to.kind {
            ExprKind::Name(n) if n == caps::EVERYONE => {}
            ExprKind::None => self.err("type.mismatch", to.pos, "message is to a player, a team or all, not none"),
            _ => match self.expr(to, None) {
                Some(Type::H(Handle::Player | Handle::Team, false)) | None => {}
                Some(t @ Type::H(h @ (Handle::Player | Handle::Team), true)) => {
                    self.mismatch(to, t, Type::H(h, false), Some("message"));
                }
                Some(other) => {
                    let m = format!("message is to a player, a team or all, not {}", self.tn(other));
                    self.err("type.mismatch", to.pos, m);
                }
            },
        }
        self.expect(&args[1], Type::Text, Some("message"));
        Some(None)
    }

    /// A capability argument (`forge.place`): in `requires` and grantable.
    fn cap_arg(&mut self, e: &Expr) {
        fn path(e: &Expr) -> Option<String> {
            match &e.kind {
                ExprKind::Name(n) => Some(n.clone()),
                ExprKind::Field(b, f) => path(b).map(|p| format!("{p}.{f}")),
                _ => None,
            }
        }
        let Some(name) = path(e) else {
            self.err("type.mismatch", e.pos, "expected a capability, as forge.place");
            return;
        };
        if !self.required(&name) {
            self.err("cap.missing", e.pos, format!("capability {name} is not in requires"));
            return;
        }
        self.used_caps.insert(name.clone());
        if !caps::capability(&name, caps::API_VERSION).is_some_and(|c| c.grantable) {
            self.err("type.mismatch", e.pos, format!("capability {name} cannot be granted"));
        }
    }
}
