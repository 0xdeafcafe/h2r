//! Lowering: a checked script ([`Checked`]) to [`Ir`].
//!
//! The checker has resolved every name and typed every expression;
//! lowering reads those types and makes no type decision. An error here is
//! a limit the IR sets (registers, ops, bytes) or an internal one.
//!
//! One pass emits register code. Functions are inlined at each call, so a
//! handler's code is straight-line except for forward jumps and `ForEach`
//! bodies. Strings and tags are interned in first-use order, and labels
//! take their declaration order, which makes the blob a function of the
//! source alone. Handlers for one event (`on enter(goal, o)`,
//! `on enter(capture z, o)`) become one IR handler, each body behind a
//! test that the argument is a member of its label.

use std::collections::BTreeMap;

use super::ast::*;
use super::check::{Checked, SECONDS_SUFFIX};
use super::types::{Handle, Type};
use super::{CompileError, Pos};
use crate::caps::{self, Kind, TICKS_PER_SECOND, VARIANT_FIELDS};
use crate::ir::*;
use crate::ty::{NONE, Ty, fixed};

type R<T> = Result<T, CompileError>;

fn err<T>(pos: Pos, message: impl Into<String>) -> R<T> {
    Err(CompileError::at(pos, message))
}

/// A state the checker rules out.
fn internal<T>(pos: Pos, what: &str) -> R<T> {
    err(pos, format!("internal: {what}"))
}

fn int32(n: i64, pos: Pos) -> R<i32> {
    i32::try_from(n).or_else(|_| err(pos, "number out of the int range"))
}

/// Fields every player, team, object or squad has, read through an entry.
/// A zone's fields are an object's.
pub(crate) const FIELDS: &[(Ty, &str, u16)] = &[
    (Ty::Player, "team", caps::id::TEAM_OF),
    (Ty::Player, "alive", caps::id::ALIVE),
    (Ty::Player, "score", caps::id::PLAYER_SCORE),
    (Ty::Player, "kills", caps::id::KILLS),
    (Ty::Player, "deaths", caps::id::DEATHS),
    (Ty::Player, "lives", caps::id::LIVES),
    (Ty::Player, "airborne", caps::id::AIRBORNE),
    (Ty::Player, "crouching", caps::id::CROUCHING),
    (Ty::Player, "primed", caps::id::PRIMED),
    (Ty::Team, "score", caps::id::TEAM_SCORE),
    (Ty::Object, "number", caps::id::LABEL_NUMBER),
    (Ty::Object, "id", caps::id::ZONE_ID),
    (Ty::Object, "carrier", caps::id::CARRIER_OF),
    (Ty::Object, "team", caps::id::OBJECT_TEAM),
    (Ty::Squad, "alive", caps::id::SQUAD_ALIVE),
    (Ty::Object, "health", caps::id::OBJECT_HEALTH),
    (Ty::Object, "shield", caps::id::OBJECT_SHIELD),
];

struct VarInfo {
    name: String,
    scope: Scope,
    ty: Ty,
    slot: u16,
}

struct TableInfo {
    name: String,
    columns: Vec<(String, Ty)>,
    rows: usize,
}

#[derive(Clone, Copy, Debug)]
enum Local {
    Reg(Reg, Ty),
    /// A table row: the table and the register holding the row index.
    Row(u8, Reg),
    /// A function's set parameter: the label it stands for.
    Set(u16),
    /// A `let` of a `vec`: its x, y and z registers.
    Vec([Reg; 3]),
}

#[derive(Default)]
struct Fun {
    regs: Vec<Ty>,
    code: Vec<Op>,
    free: BTreeMap<Ty, Vec<Reg>>,
    live: Vec<Reg>,
    scopes: Vec<Vec<(String, Local)>>,
    inlining: Vec<String>,
    /// The member a `where` reads fields of without naming it.
    implicit: Vec<(Reg, Ty)>,
}

impl Fun {
    fn alloc(&mut self, ty: Ty, pos: Pos) -> R<Reg> {
        let r = match self.free.get_mut(&ty).and_then(|list| list.pop()) {
            Some(r) => r,
            None => {
                if self.regs.len() >= MAX_REGS {
                    return err(pos, "the handler needs more than 256 registers");
                }
                self.regs.push(ty);
                (self.regs.len() - 1) as Reg
            }
        };
        self.live.push(r);
        Ok(r)
    }

    /// Frees every register allocated since `mark`.
    fn release(&mut self, mark: usize) {
        while self.live.len() > mark {
            if let Some(r) = self.live.pop() {
                let ty = self.regs[r as usize];
                self.free.entry(ty).or_default().push(r);
            }
        }
    }

    /// Frees what was allocated since `mark` except `keep`, which stays
    /// live for the enclosing block.
    fn keep(&mut self, mark: usize, keep: Reg) {
        let kept = self.live[mark.min(self.live.len())..].contains(&keep);
        if kept && let Some(i) = self.live[mark..].iter().position(|&r| r == keep) {
            self.live.remove(mark + i);
        }
        self.release(mark);
        if kept {
            self.live.push(keep);
        }
    }

    /// Like [`Fun::keep`] for several registers: frees what was allocated
    /// since `mark` except `keep`, which stay live for the enclosing block.
    fn keep_all(&mut self, mark: usize, keep: &[Reg]) {
        let mut kept = Vec::new();
        for &k in keep {
            let from = mark.min(self.live.len());
            if let Some(i) = self.live[from..].iter().position(|&r| r == k) {
                self.live.remove(from + i);
                kept.push(k);
            }
        }
        self.release(mark);
        self.live.extend(kept);
    }

    fn emit(&mut self, op: Op) -> usize {
        self.code.push(op);
        self.code.len() - 1
    }

    fn konst(&mut self, ty: Ty, value: i32, pos: Pos) -> R<Reg> {
        let r = self.alloc(ty, pos)?;
        self.emit(Op::Const { dst: r, value });
        Ok(r)
    }

    fn here(&self, pos: Pos) -> R<u16> {
        u16::try_from(self.code.len())
            .ok()
            .filter(|&n| (n as usize) <= MAX_OPS)
            .ok_or_else(|| CompileError::at(pos, "the handler has more than 8192 ops"))
    }

    fn patch(&mut self, at: usize, pos: Pos) -> R<()> {
        let target = self.here(pos)?;
        match &mut self.code[at] {
            Op::Jump { to } | Op::JumpIf { to, .. } | Op::JumpIfNot { to, .. } => *to = target,
            _ => {}
        }
        Ok(())
    }

    fn lookup(&self, name: &str) -> Option<Local> {
        for scope in self.scopes.iter().rev() {
            if let Some((_, l)) = scope.iter().rev().find(|(n, _)| n == name) {
                return Some(*l);
            }
        }
        None
    }

    fn bind(&mut self, name: &str, local: Local) {
        if self.scopes.is_empty() {
            self.scopes.push(Vec::new());
        }
        if let Some(scope) = self.scopes.last_mut() {
            scope.push((name.to_string(), local));
        }
    }
}

/// What a loop body does each pass.
enum Body<'s> {
    Stmts(&'s [Stmt]),
    /// `count`: add one to `acc`.
    Count { acc: Reg, one: Reg },
    /// `first`, and a single label's member: keep the first member in
    /// `acc`.
    First { acc: Reg, none: Reg },
    /// `capture[n]`, `flag_spawn[t]`: keep in `acc` the first member whose
    /// `entry(member)` (its number or its team) is `key`.
    Find { acc: Reg, none: Reg, entry: u16, key: Reg },
    /// `x is goal`: set `found` to `yes` when the member is `x`.
    Member { found: Reg, yes: Reg, x: Reg },
}

/// The handlers of one event (and, for `on timer`, one timer slot), in
/// the order of the first.
struct Group {
    event: Event,
    timer: Option<u16>,
    /// The event's argument registers.
    args: Vec<Reg>,
    f: Fun,
}

struct Cx<'s> {
    c: &'s Checked<'s>,
    fns: BTreeMap<&'s str, &'s FnDecl>,
    strings: Vec<String>,
    string_ix: BTreeMap<String, u16>,
    tags: Vec<TagDef>,
    tag_ix: BTreeMap<String, u16>,
    /// Each option, and whether it is a `time`, held in whole seconds and
    /// read in ticks.
    options: Vec<(String, OptionDef, bool)>,
    enums: Vec<(String, Vec<String>)>,
    enum_names: Vec<String>,
    vars: Vec<VarInfo>,
    slots: Vec<Slot>,
    tables: Vec<TableInfo>,
    table_defs: Vec<Table>,
    palettes: Vec<(String, Palette)>,
    requires: Vec<String>,
    groups: Vec<Group>,
}

/// Lowers a checked script with no errors.
pub fn lower<'s>(c: &'s Checked<'s>) -> R<Ir> {
    let mut cx = Cx {
        c,
        fns: BTreeMap::new(),
        strings: Vec::new(),
        string_ix: BTreeMap::new(),
        tags: Vec::new(),
        tag_ix: BTreeMap::new(),
        options: Vec::new(),
        enums: Vec::new(),
        enum_names: Vec::new(),
        vars: Vec::new(),
        slots: Vec::new(),
        tables: Vec::new(),
        table_defs: Vec::new(),
        palettes: Vec::new(),
        requires: Vec::new(),
        groups: Vec::new(),
    };
    cx.script(c.script)
}

impl<'s> Cx<'s> {
    fn script(&mut self, script: &'s Script) -> R<Ir> {
        let pos = script.pos;
        let version = u16::try_from(script.version).or_else(|_| internal(pos, "version"))?;
        let Some(game_type) = self.c.game_type else {
            return internal(pos, "no type");
        };
        let teams = script.teams.map_or(0, |(n, _)| n as u8);
        // What a point is: the script's, else the type's.
        let points = script.points.as_ref().map_or(game_type.points(), |(p, _)| p.as_str()).to_string();
        self.requires = script.requires.iter().map(|(n, _)| n.clone()).collect();

        // Declarations first, so their order does not matter to handlers.
        for item in &script.items {
            if let Item::Enum { name, variants, .. } = item {
                self.enums.push((name.clone(), variants.clone()));
                self.enum_names.push(name.clone());
            }
        }
        for item in &script.items {
            match item {
                Item::Option(o) => self.option(o)?,
                Item::Palette { name, tags, pos } => self.palette(name, tags, *pos)?,
                Item::Var {
                    scope,
                    name,
                    ty,
                    init,
                    pos,
                } => self.var(*scope, name, ty, init.as_ref(), *pos)?,
                Item::Table {
                    name,
                    columns,
                    rows,
                    pos,
                } => self.table(name, columns, rows, *pos)?,
                Item::Fn(f) => {
                    self.fns.insert(f.name.as_str(), f);
                }
                Item::Enum { .. } | Item::Label(_) | Item::Const { .. } | Item::On { .. } => {}
            }
        }
        for (index, item) in script.items.iter().enumerate() {
            if let Item::On {
                event, args, body, pos, ..
            } = item
            {
                self.handler(index, event, args, body, *pos)?;
            }
        }

        let enums = std::mem::take(&mut self.enums);
        let mut enum_defs = Vec::with_capacity(enums.len());
        for (name, variants) in &enums {
            let name = self.string(name, pos)?;
            let mut vs = Vec::with_capacity(variants.len());
            for v in variants {
                vs.push(self.string(v, pos)?);
            }
            enum_defs.push(EnumDef { name, variants: vs });
        }
        let handlers = std::mem::take(&mut self.groups)
            .into_iter()
            .map(|g| Handler {
                event: g.event,
                timer: g.timer,
                regs: g.f.regs,
                code: g.f.code,
            })
            .collect();
        let mut widgets = Vec::with_capacity(script.huds.len());
        for h in &script.huds {
            let Some(kind) = crate::ir::WidgetKind::ALL.into_iter().find(|k| k.word() == h.kind) else {
                return err(h.pos, format!("{} is no hud kind", h.kind));
            };
            widgets.push(crate::ir::WidgetDef {
                name: self.string(&h.name, h.pos)?,
                kind,
                slot: self.string(&h.slot, h.pos)?,
            });
        }
        let mut waypoints = Vec::with_capacity(script.waypoints.len());
        for w in &script.waypoints {
            waypoints.push(crate::ir::WaypointDef {
                name: self.string(&w.name, w.pos)?,
                slot: match &w.slot {
                    Some(slot) => Some(self.string(slot, w.pos)?),
                    None => None,
                },
            });
        }
        let ir = Ir {
            header: Header {
                ir_version: IR_VERSION,
                api_version: caps::API_VERSION,
                name: script.name.clone(),
                version,
                game_type,
                uses: self.c.uses,
                teams,
                points,
                about: script.about.as_ref().map(|(a, _)| a.clone()).unwrap_or_default(),
                requires: self.requires.clone(),
            },
            strings: std::mem::take(&mut self.strings),
            options: self.options.drain(..).map(|(_, o, _)| o).collect(),
            settings: self.c.settings.clone(),
            quick: self.c.quick.clone(),
            enums: enum_defs,
            slots: std::mem::take(&mut self.slots),
            labels: self.c.labels.iter().map(|l| l.def.clone()).collect(),
            tags: std::mem::take(&mut self.tags),
            palettes: self.palettes.drain(..).map(|(_, p)| p).collect(),
            tables: std::mem::take(&mut self.table_defs),
            widgets,
            waypoints,
            handlers,
        };
        let bytes = ir.encode().len();
        if bytes > MAX_IR_BYTES {
            return err(pos, format!("the compiled script is {bytes} bytes, over the 256 KiB cap"));
        }
        Ok(ir)
    }

    // ---- tables of names ----

    fn intern(list: &mut Vec<String>, ix: &mut BTreeMap<String, u16>, s: &str, pos: Pos) -> R<u16> {
        if let Some(&i) = ix.get(s) {
            return Ok(i);
        }
        if list.len() >= u16::MAX as usize / 2 {
            return err(pos, "too many names");
        }
        let i = list.len() as u16;
        list.push(s.to_string());
        ix.insert(s.to_string(), i);
        Ok(i)
    }

    fn string(&mut self, s: &str, pos: Pos) -> R<u16> {
        if s.len() > MAX_TEXT_BYTES {
            return err(pos, "text is at most 128 bytes");
        }
        Self::intern(&mut self.strings, &mut self.string_ix, s, pos)
    }

    /// A tag path's index. A path named once as a weapon or a sound is
    /// that group's on every map: its entry's group is [`TagGroup::Weapon`]
    /// or [`TagGroup::Sound`].
    fn tag(&mut self, s: &str, group: TagGroup, pos: Pos) -> R<u16> {
        if s.is_empty() || s.len() > MAX_TAG_BYTES {
            return err(pos, "a tag path is 1 to 256 bytes");
        }
        if let Some(&i) = self.tag_ix.get(s) {
            if group != TagGroup::Any {
                self.tags[i as usize].group = group;
            }
            return Ok(i);
        }
        if self.tags.len() >= u16::MAX as usize / 2 {
            return err(pos, "too many names");
        }
        let i = self.tags.len() as u16;
        self.tags.push(TagDef {
            path: s.to_string(),
            group,
        });
        self.tag_ix.insert(s.to_string(), i);
        Ok(i)
    }

    fn label_ix(&self, name: &str, pos: Pos) -> R<u16> {
        match self.c.label(name) {
            Some((ix, _)) => Ok(ix),
            None => internal(pos, "an undeclared label"),
        }
    }

    /// The type a declaration names.
    fn declared(&self, t: &TypeName) -> R<Type> {
        match Type::named(&t.name, t.optional, &self.enum_names) {
            Some(ty) => Ok(ty),
            None => internal(t.pos, "an unknown type"),
        }
    }

    /// The checker's type of `e`.
    fn ty_of(&self, e: &Expr) -> R<Type> {
        match self.c.ty(e) {
            Some(t) => Ok(t),
            None => internal(e.pos, "an expression the checker did not type"),
        }
    }

    // ---- declarations ----

    fn option(&mut self, o: &OptionDecl) -> R<()> {
        let pos = o.pos;
        let ty = self.declared(&o.ty)?;
        let time = ty == Type::Time;
        // A time option is held in whole seconds, as an int option of the
        // same range is; the checker has made each value whole.
        let unit = if time { TICKS_PER_SECOND } else { 1 };
        let (lo, hi) = match ty {
            Type::Int | Type::Time => {
                let Some((lo, hi)) = &o.range else {
                    return internal(pos, "an int option with no range");
                };
                (self.const_val(lo, ty)? / unit, self.const_val(hi, ty)? / unit)
            }
            Type::Bool => (0, 1),
            Type::Enum(e) => (0, self.enums[e as usize].1.len() as i32 - 1),
            _ => return internal(pos, "an option of no option type"),
        };
        let default = self.const_val(&o.default, ty)? / unit;
        let name = self.string(&o.name, pos)?;
        let label = match (&o.label, time) {
            (Some(l), false) => self.string(l, pos)?,
            (None, false) => name,
            // The IR stores no unit: the label says it.
            (l, true) => {
                let text = format!("{}{SECONDS_SUFFIX}", l.as_deref().unwrap_or(&o.name));
                self.string(&text, pos)?
            }
        };
        let description = match &o.description {
            Some(d) => Some(self.string(d, pos)?),
            None => None,
        };
        let def = OptionDef {
            name,
            label,
            description,
            ty: ty.ir(),
            lo,
            hi,
            default,
        };
        self.options.push((o.name.clone(), def, time));
        Ok(())
    }

    fn palette(&mut self, name: &str, tags: &[String], pos: Pos) -> R<()> {
        let n = self.string(name, pos)?;
        let mut list = Vec::with_capacity(tags.len());
        for t in tags {
            list.push(self.tag(t, TagGroup::Any, pos)?);
        }
        self.palettes.push((name.to_string(), Palette { name: n, tags: list }));
        Ok(())
    }

    fn var(&mut self, scope: Scope, name: &str, ty: &TypeName, init: Option<&Expr>, pos: Pos) -> R<()> {
        let (t, front) = if ty.name == "timer" && !ty.optional {
            (Ty::Timer, Type::Time)
        } else {
            let front = self.declared(ty)?;
            (front.ir(), front)
        };
        let init = match init {
            Some(e) => self.const_val(e, front)?,
            None => t.zero(),
        };
        let slot = self.slots.len() as u16;
        let n = self.string(name, pos)?;
        self.slots.push(Slot {
            name: n,
            scope,
            ty: t,
            init,
        });
        self.vars.push(VarInfo {
            name: name.to_string(),
            scope,
            ty: t,
            slot,
        });
        Ok(())
    }

    fn table(&mut self, name: &str, columns: &[(String, TypeName)], rows: &[Vec<Expr>], pos: Pos) -> R<()> {
        let mut cols = Vec::with_capacity(columns.len());
        let mut fronts = Vec::with_capacity(columns.len());
        let mut col_defs = Vec::with_capacity(columns.len());
        for (cname, cty) in columns {
            let front = self.declared(cty)?;
            col_defs.push(Column {
                name: self.string(cname, pos)?,
                ty: front.ir(),
            });
            cols.push((cname.clone(), front.ir()));
            fronts.push(front);
        }
        let mut cells = Vec::with_capacity(rows.len() * cols.len());
        for row in rows {
            for (cell, &t) in row.iter().zip(&fronts) {
                cells.push(self.const_val(cell, t)?);
            }
        }
        let n = self.string(name, pos)?;
        self.table_defs.push(Table {
            name: n,
            columns: col_defs,
            cells,
        });
        self.tables.push(TableInfo {
            name: name.to_string(),
            columns: cols,
            rows: rows.len(),
        });
        Ok(())
    }

    fn const_int(&self, e: &Expr) -> R<i32> {
        match &e.kind {
            ExprKind::Int(n) => int32(*n, e.pos),
            ExprKind::Unary(UnOp::Neg, inner) => match inner.kind {
                ExprKind::Int(n) => int32(-n, e.pos),
                _ => internal(e.pos, "not a constant int"),
            },
            _ => internal(e.pos, "not a constant int"),
        }
    }

    /// A constant of type `ty`: an initial value, a default or a cell. An
    /// `int` or a `time` is the value the checker folded.
    fn const_val(&mut self, e: &Expr, ty: Type) -> R<i32> {
        let pos = e.pos;
        let v = match (&e.kind, ty) {
            (_, Type::Int | Type::Time) => match self.c.value(e) {
                Some(v) => v,
                None => return internal(pos, "a constant the checker did not fold"),
            },
            (ExprKind::Int(_), Type::Fixed) => fixed::from_int(self.const_int(e)?),
            (ExprKind::Fixed(v), Type::Fixed) => *v,
            (ExprKind::Unary(UnOp::Neg, inner), Type::Fixed) => match inner.kind {
                ExprKind::Fixed(v) => -v,
                ExprKind::Int(n) => fixed::from_int(int32(-n, pos)?),
                _ => return internal(pos, "not a constant fixed"),
            },
            (ExprKind::Bool(b), Type::Bool) => *b as i32,
            (ExprKind::None, Type::H(..)) => NONE,
            (ExprKind::Str(s), Type::H(Handle::Tag, _)) => self.tag(s, TagGroup::Any, pos)? as i32,
            (ExprKind::Str(s), Type::H(Handle::Weapon, _)) => self.tag(s, TagGroup::Weapon, pos)? as i32,
            (ExprKind::Str(s), Type::H(Handle::Sound, _)) => self.tag(s, TagGroup::Sound, pos)? as i32,
            (ExprKind::Str(s), Type::Class | Type::Text) => self.string(s, pos)? as i32,
            (ExprKind::Team(inner), Type::H(Handle::Team, _)) => self.const_int(inner)?,
            (ExprKind::Field(_, v), Type::Enum(i)) => match self.enums[i as usize].1.iter().position(|x| x == v) {
                Some(k) => k as i32,
                None => return internal(pos, "an unknown variant"),
            },
            (ExprKind::Call(name, args), Type::Squad) if name == "squad" && args.len() == 1 => {
                self.const_int(&args[0])?
            }
            _ => return internal(pos, "not a constant of its type"),
        };
        Ok(v)
    }

    fn var_named(&self, name: &str, scope: Scope) -> Option<(u16, Ty)> {
        self.vars
            .iter()
            .find(|v| v.scope == scope && v.name == name)
            .map(|v| (v.slot, v.ty))
    }

    // ---- handlers ----

    fn handler(&mut self, index: usize, event: &str, args: &[String], body: &'s [Stmt], pos: Pos) -> R<()> {
        let Some(ev) = Event::named(event) else {
            return internal(pos, "an unknown event");
        };
        let timer = if ev == Event::Timer {
            match args.first().and_then(|n| self.var_named(n, Scope::Global)) {
                Some((slot, Ty::Timer)) => Some(slot),
                _ => return internal(pos, "on timer names no timer"),
            }
        } else {
            None
        };
        let gi = match self.groups.iter().position(|g| g.event == ev && g.timer == timer) {
            Some(i) => i,
            None => {
                let mut f = Fun::default();
                f.scopes.push(Vec::new());
                let mut regs = Vec::new();
                for &ty in ev.args() {
                    regs.push(f.alloc(ty, pos)?);
                }
                self.groups.push(Group {
                    event: ev,
                    timer,
                    args: regs,
                    f,
                });
                self.groups.len() - 1
            }
        };
        let mut f = std::mem::take(&mut self.groups[gi].f);
        let regs = self.groups[gi].args.clone();
        f.scopes.push(Vec::new());
        if timer.is_none() {
            for (k, name) in args.iter().enumerate() {
                let (Some(&r), Some(&ty)) = (regs.get(k), ev.args().get(k)) else {
                    return internal(pos, "more handler arguments than the event gives");
                };
                f.bind(name, Local::Reg(r, ty));
            }
        }
        let result = self.handler_body(&mut f, index, &regs, body, pos);
        f.scopes.pop();
        self.groups[gi].f = f;
        result
    }

    fn handler_body(&mut self, f: &mut Fun, index: usize, regs: &[Reg], body: &'s [Stmt], pos: Pos) -> R<()> {
        let guard = match self.c.handler_label(index) {
            Some((ix, k)) => {
                let Some(&x) = regs.get(k) else {
                    return internal(pos, "a label in no argument's place");
                };
                let mark = f.live.len();
                let found = self.member_test(f, x, ix, pos)?;
                let j = f.emit(Op::JumpIfNot { cond: found, to: 0 });
                f.release(mark);
                Some(j)
            }
            None => None,
        };
        self.block(f, body)?;
        if let Some(j) = guard {
            f.patch(j, pos)?;
        }
        f.here(pos)?;
        Ok(())
    }

    fn block(&mut self, f: &mut Fun, body: &'s [Stmt]) -> R<()> {
        f.scopes.push(Vec::new());
        let mark = f.live.len();
        for s in body {
            self.stmt(f, s)?;
        }
        f.release(mark);
        f.scopes.pop();
        Ok(())
    }

    fn stmt(&mut self, f: &mut Fun, s: &'s Stmt) -> R<()> {
        let mark = f.live.len();
        match s {
            Stmt::Let { name, value, pos, .. } => {
                if let ExprKind::Index(base, index) = &value.kind
                    && let ExprKind::Name(t) = &base.kind
                    && f.lookup(t).is_none()
                    && self.tables.iter().any(|i| i.name == *t)
                {
                    let (table, row) = self.row(f, base, index, *pos)?;
                    f.keep(mark, row);
                    f.bind(name, Local::Row(table, row));
                } else if self.ty_of(value)? == Type::Vec {
                    let v = self.vec_expr(f, value)?;
                    f.keep_all(mark, &v);
                    f.bind(name, Local::Vec(v));
                } else {
                    let (r, t) = self.expr(f, value)?;
                    f.keep(mark, r);
                    f.bind(name, Local::Reg(r, t));
                }
                return Ok(());
            }
            Stmt::Assign { target, value, pos } => self.assign(f, target, value, *pos)?,
            Stmt::Expr(e) => match &e.kind {
                ExprKind::Call(name, args) => {
                    self.call(f, name, args, e.pos, false)?;
                }
                _ => return internal(e.pos, "an expression statement"),
            },
            Stmt::If { cond, then, els } => {
                let (c, _) = self.expr(f, cond)?;
                let skip = f.emit(Op::JumpIfNot { cond: c, to: 0 });
                self.block(f, then)?;
                if els.is_empty() {
                    f.patch(skip, cond.pos)?;
                } else {
                    let over = f.emit(Op::Jump { to: 0 });
                    f.patch(skip, cond.pos)?;
                    self.block(f, els)?;
                    f.patch(over, cond.pos)?;
                }
            }
            Stmt::IfLet {
                name,
                value,
                cond,
                then,
                els,
                pos,
            } => {
                let (v, t) = self.expr(f, value)?;
                let none = f.konst(t, NONE, *pos)?;
                let c = f.alloc(Ty::Bool, *pos)?;
                f.emit(Op::Cmp {
                    op: CmpOp::Ne,
                    dst: c,
                    a: v,
                    b: none,
                });
                let mut skips = vec![f.emit(Op::JumpIfNot { cond: c, to: 0 })];
                f.scopes.push(Vec::new());
                f.bind(name, Local::Reg(v, t));
                if let Some(cond) = cond {
                    let inner = f.live.len();
                    let (r, _) = self.expr(f, cond)?;
                    skips.push(f.emit(Op::JumpIfNot { cond: r, to: 0 }));
                    f.release(inner);
                }
                self.block(f, then)?;
                f.scopes.pop();
                if els.is_empty() {
                    for j in skips {
                        f.patch(j, *pos)?;
                    }
                } else {
                    let over = f.emit(Op::Jump { to: 0 });
                    for j in skips {
                        f.patch(j, *pos)?;
                    }
                    self.block(f, els)?;
                    f.patch(over, *pos)?;
                }
            }
            Stmt::ForEach {
                set,
                binder,
                body,
                pos,
            } => {
                let source = self.set_source(f, set)?;
                self.foreach(f, source, set.filter.as_ref(), Some(binder.as_str()), Body::Stmts(body), *pos)?;
            }
            Stmt::Match { value, arms, els, pos } => {
                // Compares and forward jumps: each arm tests its variants
                // and jumps to its block, or on to the next arm.
                let (v, _) = self.expr(f, value)?;
                let mut ends = Vec::with_capacity(arms.len());
                for arm in arms {
                    let inner = f.live.len();
                    let mut to_body = Vec::with_capacity(arm.patterns.len());
                    for p in &arm.patterns {
                        let (pr, _) = self.expr(f, p)?;
                        let c = f.alloc(Ty::Bool, p.pos)?;
                        f.emit(Op::Cmp {
                            op: CmpOp::Eq,
                            dst: c,
                            a: v,
                            b: pr,
                        });
                        to_body.push(f.emit(Op::JumpIf { cond: c, to: 0 }));
                    }
                    f.release(inner);
                    let next = f.emit(Op::Jump { to: 0 });
                    for j in to_body {
                        f.patch(j, arm.pos)?;
                    }
                    self.block(f, &arm.body)?;
                    ends.push(f.emit(Op::Jump { to: 0 }));
                    f.patch(next, arm.pos)?;
                }
                if let Some(els) = els {
                    self.block(f, els)?;
                }
                for j in ends {
                    f.patch(j, *pos)?;
                }
            }
        }
        f.release(mark);
        Ok(())
    }

    fn assign(&mut self, f: &mut Fun, target: &'s Expr, value: &'s Expr, pos: Pos) -> R<()> {
        match &target.kind {
            ExprKind::Name(n) => {
                let Some((slot, _)) = self.var_named(n, Scope::Global) else {
                    return internal(pos, "an assignment to no variable");
                };
                let (r, _) = self.expr(f, value)?;
                f.emit(Op::Store {
                    slot,
                    owner: None,
                    src: r,
                });
                Ok(())
            }
            ExprKind::Field(base, field) => {
                let (owner, oty) = self.expr(f, base)?;
                let scope = match oty {
                    Ty::Player => Scope::Player,
                    Ty::Team => Scope::Team,
                    Ty::Object => Scope::Object,
                    _ => return internal(pos, "an assignment to a field of no owner"),
                };
                let Some((slot, _)) = self.var_named(field, scope) else {
                    return internal(pos, "an assignment to no variable");
                };
                let (r, _) = self.expr(f, value)?;
                f.emit(Op::Store {
                    slot,
                    owner: Some(owner),
                    src: r,
                });
                Ok(())
            }
            _ => internal(pos, "an assignment to no variable"),
        }
    }

    // ---- loops ----

    /// The label a set name stands for: a set parameter's, or a declared
    /// label's.
    fn set_label(&self, f: &Fun, name: &str, pos: Pos) -> R<u16> {
        match f.lookup(name) {
            Some(Local::Set(ix)) => Ok(ix),
            Some(_) => internal(pos, "a set name bound to a value"),
            None => self.label_ix(name, pos),
        }
    }

    fn set_base(&self, f: &Fun, base: &SetBaseExpr, pos: Pos) -> R<SetBase> {
        Ok(match base {
            SetBaseExpr::Players => SetBase::Players,
            SetBaseExpr::Teams => SetBase::Teams,
            SetBaseExpr::Objectives => SetBase::Objectives,
            SetBaseExpr::Label(l) => SetBase::Labelled(self.set_label(f, l, pos)?),
            SetBaseExpr::Pick(..) => return internal(pos, "pick of a pick"),
        })
    }

    fn set_source(&mut self, f: &mut Fun, set: &'s SetExpr) -> R<Set> {
        Ok(match &set.base {
            SetBaseExpr::Pick(inner, n) => {
                let base = self.set_base(f, &inner.base, inner.pos)?;
                let (n, _) = self.expr(f, n)?;
                Set { base, pick: Some(n) }
            }
            base => Set {
                base: self.set_base(f, base, set.pos)?,
                pick: None,
            },
        })
    }

    fn foreach(
        &mut self,
        f: &mut Fun,
        source: Set,
        filter: Option<&'s Expr>,
        binder: Option<&str>,
        body: Body<'s>,
        pos: Pos,
    ) -> R<()> {
        let elem = source.base.elem();
        let dst = f.alloc(elem, pos)?;
        let at = f.emit(Op::ForEach {
            set: source,
            dst,
            body: 0,
        });
        f.scopes.push(Vec::new());
        if let Some(b) = binder {
            f.bind(b, Local::Reg(dst, elem));
        }
        let mark = f.live.len();
        let mut skips = Vec::new();
        if let Some(filter) = filter {
            f.implicit.push((dst, elem));
            let c = self.expr(f, filter);
            f.implicit.pop();
            let (c, _) = c?;
            skips.push(f.emit(Op::JumpIfNot { cond: c, to: 0 }));
        }
        match body {
            Body::Stmts(stmts) => {
                for s in stmts {
                    self.stmt(f, s)?;
                }
            }
            Body::Count { acc, one } => {
                f.emit(Op::Arith {
                    op: ArithOp::Add,
                    ty: NumTy::Int,
                    dst: acc,
                    a: acc,
                    b: one,
                });
            }
            Body::First { acc, none } => {
                Self::keep_first(f, acc, none, dst, &mut skips, pos)?;
            }
            Body::Find { acc, none, entry, key } => {
                let Some(ty) = caps::ENTRIES[entry as usize].result else {
                    return internal(pos, "a find by an entry with no value");
                };
                let k = f.alloc(ty, pos)?;
                f.emit(Op::Call {
                    entry,
                    args: vec![dst],
                    dst: Some(k),
                });
                let c = f.alloc(Ty::Bool, pos)?;
                f.emit(Op::Cmp {
                    op: CmpOp::Eq,
                    dst: c,
                    a: k,
                    b: key,
                });
                skips.push(f.emit(Op::JumpIfNot { cond: c, to: 0 }));
                Self::keep_first(f, acc, none, dst, &mut skips, pos)?;
            }
            Body::Member { found, yes, x } => {
                let c = f.alloc(Ty::Bool, pos)?;
                f.emit(Op::Cmp {
                    op: CmpOp::Eq,
                    dst: c,
                    a: dst,
                    b: x,
                });
                skips.push(f.emit(Op::JumpIfNot { cond: c, to: 0 }));
                f.emit(Op::Move { dst: found, src: yes });
            }
        }
        f.release(mark);
        f.scopes.pop();
        for j in skips {
            f.patch(j, pos)?;
        }
        let end = f.here(pos)? as usize;
        let len = u16::try_from(end - at - 1).or_else(|_| err(pos, "the loop body is too long"))?;
        if let Op::ForEach { body, .. } = &mut f.code[at] {
            *body = len;
        }
        Ok(())
    }

    /// Moves `member` into `acc` while `acc` is still `none`.
    fn keep_first(f: &mut Fun, acc: Reg, none: Reg, member: Reg, skips: &mut Vec<usize>, pos: Pos) -> R<()> {
        let c = f.alloc(Ty::Bool, pos)?;
        f.emit(Op::Cmp {
            op: CmpOp::Eq,
            dst: c,
            a: acc,
            b: none,
        });
        skips.push(f.emit(Op::JumpIfNot { cond: c, to: 0 }));
        f.emit(Op::Move { dst: acc, src: member });
        Ok(())
    }

    /// A label's member: the first (a single label's one), or with `find`,
    /// the first whose number or team is the key. `none` when the map has
    /// none.
    fn label_pick(&mut self, f: &mut Fun, ix: u16, find: Option<(u16, Reg)>, pos: Pos) -> R<(Reg, Ty)> {
        let acc = f.konst(Ty::Object, NONE, pos)?;
        let none = f.konst(Ty::Object, NONE, pos)?;
        let body = match find {
            Some((entry, key)) => Body::Find { acc, none, entry, key },
            None => Body::First { acc, none },
        };
        let source = Set {
            base: SetBase::Labelled(ix),
            pick: None,
        };
        self.foreach(f, source, None, None, body, pos)?;
        Ok((acc, Ty::Object))
    }

    /// Whether `x` is a member of label `ix`. It walks the label's members
    /// rather than comparing `label_of(x)`, which reads label 0 for a thing
    /// with no declared label.
    fn member_test(&mut self, f: &mut Fun, x: Reg, ix: u16, pos: Pos) -> R<Reg> {
        let found = f.konst(Ty::Bool, 0, pos)?;
        let yes = f.konst(Ty::Bool, 1, pos)?;
        let source = Set {
            base: SetBase::Labelled(ix),
            pick: None,
        };
        self.foreach(f, source, None, None, Body::Member { found, yes, x }, pos)?;
        Ok(found)
    }

    // ---- expressions ----

    fn expr(&mut self, f: &mut Fun, e: &'s Expr) -> R<(Reg, Ty)> {
        let pos = e.pos;
        match &e.kind {
            ExprKind::Int(n) => {
                let v = int32(*n, pos)?;
                if self.ty_of(e)? == Type::Fixed {
                    Ok((f.konst(Ty::Fixed, fixed::from_int(v), pos)?, Ty::Fixed))
                } else {
                    Ok((f.konst(Ty::Int, v, pos)?, Ty::Int))
                }
            }
            ExprKind::Fixed(v) => Ok((f.konst(Ty::Fixed, *v, pos)?, Ty::Fixed)),
            ExprKind::Time(ticks, _) => match ticks {
                Some(t) => Ok((f.konst(Ty::Int, int32(*t, pos)?, pos)?, Ty::Int)),
                None => internal(pos, "a time of no whole tick"),
            },
            ExprKind::Bool(b) => Ok((f.konst(Ty::Bool, *b as i32, pos)?, Ty::Bool)),
            ExprKind::None => {
                let t = self.ty_of(e)?.ir();
                Ok((f.konst(t, NONE, pos)?, t))
            }
            ExprKind::Str(s) => {
                let (ty, index) = match self.ty_of(e)? {
                    Type::H(Handle::Tag, _) => (Ty::Tag, self.tag(s, TagGroup::Any, pos)?),
                    Type::H(Handle::Weapon, _) => (Ty::Tag, self.tag(s, TagGroup::Weapon, pos)?),
                    Type::H(Handle::Sound, _) => (Ty::Tag, self.tag(s, TagGroup::Sound, pos)?),
                    Type::Class => (Ty::Class, self.string(s, pos)?),
                    Type::Text => (Ty::Text, self.string(s, pos)?),
                    _ => return internal(pos, "a string of no string type"),
                };
                Ok((f.konst(ty, index as i32, pos)?, ty))
            }
            ExprKind::Team(inner) => {
                let n = self.const_int(inner)?;
                Ok((f.konst(Ty::Team, n, pos)?, Ty::Team))
            }
            ExprKind::Text(inner) => {
                let index = match &inner.kind {
                    ExprKind::Str(s) => self.string(s, pos)? as i32,
                    ExprKind::Int(n) => int32(*n, pos)?,
                    _ => return internal(pos, "text of no string"),
                };
                Ok((f.konst(Ty::Text, index, pos)?, Ty::Text))
            }
            ExprKind::Name(n) => self.name(f, n, pos),
            ExprKind::Field(base, field) => self.field(f, base, field, pos),
            ExprKind::Index(base, index) => self.label_index(f, base, index, pos),
            ExprKind::Call(name, args) => match self.call(f, name, args, pos, true)? {
                Some(v) => Ok(v),
                None => internal(pos, "a call with no value"),
            },
            ExprKind::Unary(UnOp::Neg, inner) => {
                match inner.kind {
                    ExprKind::Int(n) => {
                        let v = int32(-n, pos)?;
                        return if self.ty_of(e)? == Type::Fixed {
                            Ok((f.konst(Ty::Fixed, fixed::from_int(v), pos)?, Ty::Fixed))
                        } else {
                            Ok((f.konst(Ty::Int, v, pos)?, Ty::Int))
                        };
                    }
                    ExprKind::Fixed(v) => return Ok((f.konst(Ty::Fixed, -v, pos)?, Ty::Fixed)),
                    _ => {}
                }
                let (r, t) = self.expr(f, inner)?;
                let Some(ty) = numeric(t) else {
                    return internal(pos, "- of no number");
                };
                let dst = f.alloc(t, pos)?;
                f.emit(Op::Neg { ty, dst, a: r });
                Ok((dst, t))
            }
            ExprKind::Unary(UnOp::Not, inner) => {
                let (r, _) = self.expr(f, inner)?;
                let dst = f.alloc(Ty::Bool, pos)?;
                f.emit(Op::Not { dst, a: r });
                Ok((dst, Ty::Bool))
            }
            ExprKind::Binary(op, a, b) => self.binary(f, *op, a, b, pos),
            ExprKind::Is(x, l, lpos) => {
                let (r, _) = self.expr(f, x)?;
                let ix = self.label_ix(l, *lpos)?;
                Ok((self.member_test(f, r, ix, pos)?, Ty::Bool))
            }
            ExprKind::Budget(_) | ExprKind::Palette(_) | ExprKind::Set(_) => {
                internal(pos, "a grant or set argument out of place")
            }
        }
    }

    fn name(&mut self, f: &mut Fun, n: &str, pos: Pos) -> R<(Reg, Ty)> {
        match f.lookup(n) {
            Some(Local::Reg(r, t)) => return Ok((r, t)),
            Some(Local::Set(ix)) => return Ok((f.konst(Ty::Label, ix as i32, pos)?, Ty::Label)),
            Some(Local::Row(..)) => return internal(pos, "a row read as a value"),
            Some(Local::Vec(..)) => return internal(pos, "a vec read as one value"),
            None => {}
        }
        if let Some(&(r, t)) = f.implicit.last()
            && let Some(v) = self.field_of(f, r, t, n, pos)?
        {
            return Ok(v);
        }
        if let Some((slot, ty)) = self.var_named(n, Scope::Global) {
            let dst = f.alloc(ty.register(), pos)?;
            f.emit(Op::Load {
                dst,
                slot,
                owner: None,
            });
            return Ok((dst, ty.register()));
        }
        if let Some(i) = self.options.iter().position(|(o, _, _)| o == n) {
            let (ty, time) = (self.options[i].1.ty, self.options[i].2);
            let dst = f.alloc(ty, pos)?;
            f.emit(Op::Opt {
                dst,
                index: i as u8,
            });
            if time {
                // Whole seconds in the record, ticks in the script.
                let rate = f.konst(Ty::Int, TICKS_PER_SECOND, pos)?;
                f.emit(Op::Arith {
                    op: ArithOp::Mul,
                    ty: NumTy::Int,
                    dst,
                    a: dst,
                    b: rate,
                });
            }
            return Ok((dst, ty));
        }
        if let Some((_, v)) = self.c.constant(n) {
            return Ok((f.konst(Ty::Int, v, pos)?, Ty::Int));
        }
        if let Some((ix, label)) = self.c.label(n) {
            // One value names its member; a set is a set argument.
            if label.single {
                return self.label_pick(f, ix, None, pos);
            }
            return Ok((f.konst(Ty::Label, ix as i32, pos)?, Ty::Label));
        }
        internal(pos, "an unknown name")
    }

    /// A field of an owner register: a scoped variable or a built-in read.
    fn field_of(&mut self, f: &mut Fun, owner: Reg, oty: Ty, field: &str, pos: Pos) -> R<Option<(Reg, Ty)>> {
        let scope = match oty {
            Ty::Player => Some(Scope::Player),
            Ty::Team => Some(Scope::Team),
            Ty::Object => Some(Scope::Object),
            _ => None,
        };
        if let Some(scope) = scope
            && let Some((slot, ty)) = self.var_named(field, scope)
        {
            let dst = f.alloc(ty.register(), pos)?;
            f.emit(Op::Load {
                dst,
                slot,
                owner: Some(owner),
            });
            return Ok(Some((dst, ty.register())));
        }
        if oty == Ty::Object && field == "speed" {
            // The length of the velocity.
            let v = self.object_vec(f, owner, caps::id::OBJECT_VEL, pos)?;
            let dst = f.alloc(Ty::Fixed, pos)?;
            f.emit(Op::Call {
                entry: caps::id::VEC_LENGTH,
                args: v.to_vec(),
                dst: Some(dst),
            });
            return Ok(Some((dst, Ty::Fixed)));
        }
        if let Some(&(_, _, id)) = FIELDS.iter().find(|&&(o, n, _)| o == oty && n == field) {
            let entry = &caps::ENTRIES[id as usize];
            let ty = entry.result.unwrap_or(Ty::Int);
            let dst = f.alloc(ty, pos)?;
            f.emit(Op::Call {
                entry: id,
                args: vec![owner],
                dst: Some(dst),
            });
            return Ok(Some((dst, ty)));
        }
        Ok(None)
    }

    fn field(&mut self, f: &mut Fun, base: &'s Expr, field: &str, pos: Pos) -> R<(Reg, Ty)> {
        if self.c.ty(base) == Some(Type::Vec) {
            let v = self.vec_expr(f, base)?;
            return match field {
                "x" => Ok((v[0], Ty::Fixed)),
                "y" => Ok((v[1], Ty::Fixed)),
                "z" => Ok((v[2], Ty::Fixed)),
                _ => internal(pos, "an unknown vec field"),
            };
        }
        match &base.kind {
            ExprKind::Name(b) => match f.lookup(b) {
                Some(Local::Row(table, row)) => return self.column(f, table, row, field, pos),
                Some(_) => {}
                None => {
                    if b == "variant" {
                        let Some(i) = VARIANT_FIELDS.iter().position(|(n, _)| *n == field) else {
                            return internal(pos, "an unknown variant field");
                        };
                        let ty = VARIANT_FIELDS[i].1;
                        let dst = f.alloc(ty, pos)?;
                        f.emit(Op::Variant {
                            dst,
                            field: i as u8,
                        });
                        return Ok((dst, ty));
                    }
                    if let Some(i) = self.enums.iter().position(|(n, _)| n == b) {
                        let Some(k) = self.enums[i].1.iter().position(|v| v == field) else {
                            return internal(pos, "an unknown variant");
                        };
                        let ty = Ty::Enum(i as u8);
                        return Ok((f.konst(ty, k as i32, pos)?, ty));
                    }
                }
            },
            ExprKind::Index(tbase, index) => {
                if let ExprKind::Name(t) = &tbase.kind
                    && f.lookup(t).is_none()
                    && self.tables.iter().any(|i| i.name == *t)
                {
                    let (table, row) = self.row(f, tbase, index, pos)?;
                    return self.column(f, table, row, field, pos);
                }
            }
            _ => {}
        }
        let (r, t) = self.expr(f, base)?;
        match self.field_of(f, r, t, field, pos)? {
            Some(v) => Ok(v),
            None => internal(pos, "an unknown field"),
        }
    }

    /// `table[index]`: the table and a register with the row index. An int
    /// indexes rows; a value of the first column's type finds its row.
    fn row(&mut self, f: &mut Fun, base: &'s Expr, index: &'s Expr, pos: Pos) -> R<(u8, Reg)> {
        let ExprKind::Name(tname) = &base.kind else {
            return internal(pos, "an index of no table");
        };
        let Some(t) = self.tables.iter().position(|t| t.name == *tname) else {
            return internal(pos, "an unknown table");
        };
        let (r, ty) = self.expr(f, index)?;
        if ty == Ty::Int {
            return Ok((t as u8, r));
        }
        let dst = f.alloc(Ty::Int, pos)?;
        f.emit(Op::TableFind {
            dst,
            table: t as u8,
            key: r,
        });
        Ok((t as u8, dst))
    }

    /// `capture[n]` or `flag_spawn[t]`: the member with that number or of
    /// that team, or `none`.
    fn label_index(&mut self, f: &mut Fun, base: &'s Expr, index: &'s Expr, pos: Pos) -> R<(Reg, Ty)> {
        let ExprKind::Name(l) = &base.kind else {
            return internal(pos, "an index of no label");
        };
        let ix = self.label_ix(l, base.pos)?;
        let (key, kty) = self.expr(f, index)?;
        let entry = match kty {
            Ty::Int => caps::id::LABEL_NUMBER,
            Ty::Team => caps::id::OBJECT_TEAM,
            _ => return internal(pos, "a label index of no number or team"),
        };
        self.label_pick(f, ix, Some((entry, key)), pos)
    }

    fn column(&mut self, f: &mut Fun, table: u8, row: Reg, field: &str, pos: Pos) -> R<(Reg, Ty)> {
        let info = &self.tables[table as usize];
        let Some(col) = info.columns.iter().position(|(n, _)| n == field) else {
            return internal(pos, "an unknown column");
        };
        let ty = info.columns[col].1;
        let dst = f.alloc(ty, pos)?;
        f.emit(Op::TableRead {
            dst,
            table,
            row,
            col: col as u8,
        });
        Ok((dst, ty))
    }

    /// The three axis reads of an object's position or velocity (`entry`).
    fn object_vec(&mut self, f: &mut Fun, object: Reg, entry: u16, pos: Pos) -> R<[Reg; 3]> {
        let mut out = [0; 3];
        for (axis, slot) in out.iter_mut().enumerate() {
            let a = f.konst(Ty::Int, axis as i32, pos)?;
            let dst = f.alloc(Ty::Fixed, pos)?;
            f.emit(Op::Call {
                entry,
                args: vec![object, a],
                dst: Some(dst),
            });
            *slot = dst;
        }
        Ok(out)
    }

    /// Component-wise `a op b` of two vecs, or of a vec and a fixed
    /// register (`b` the same in all three).
    fn vec_arith(&mut self, f: &mut Fun, op: ArithOp, a: [Reg; 3], b: [Reg; 3], pos: Pos) -> R<[Reg; 3]> {
        let mut out = [0; 3];
        for i in 0..3 {
            let dst = f.alloc(Ty::Fixed, pos)?;
            f.emit(Op::Arith {
                op,
                ty: NumTy::Fixed,
                dst,
                a: a[i],
                b: b[i],
            });
            out[i] = dst;
        }
        Ok(out)
    }

    /// A `vec` expression's three registers: `vec(x, y, z)`, a `let` of
    /// one, an object's `position` or `velocity`, `a + b`, `a - b` and
    /// `v * k`.
    fn vec_expr(&mut self, f: &mut Fun, e: &'s Expr) -> R<[Reg; 3]> {
        let pos = e.pos;
        match &e.kind {
            ExprKind::Call(name, args) if name == "vec" => {
                let [x, y, z] = &args[..] else {
                    return internal(pos, "a vec of no three components");
                };
                Ok([self.expr(f, x)?.0, self.expr(f, y)?.0, self.expr(f, z)?.0])
            }
            ExprKind::Name(n) => match f.lookup(n) {
                Some(Local::Vec(v)) => Ok(v),
                _ => internal(pos, "a vec name of no vec"),
            },
            ExprKind::Field(base, field) if field == "position" || field == "velocity" => {
                let (o, _) = self.expr(f, base)?;
                let entry = if field == "position" {
                    caps::id::OBJECT_POS
                } else {
                    caps::id::OBJECT_VEL
                };
                self.object_vec(f, o, entry, pos)
            }
            ExprKind::Binary(op @ (BinOp::Add | BinOp::Sub), a, b) => {
                let va = self.vec_expr(f, a)?;
                let vb = self.vec_expr(f, b)?;
                let aop = if *op == BinOp::Add { ArithOp::Add } else { ArithOp::Sub };
                self.vec_arith(f, aop, va, vb, pos)
            }
            ExprKind::Binary(BinOp::Mul, a, b) => {
                let (v, k) = if self.ty_of(a)? == Type::Vec { (a, b) } else { (b, a) };
                let rv = self.vec_expr(f, v)?;
                let (rk, _) = self.expr(f, k)?;
                self.vec_arith(f, ArithOp::Mul, rv, [rk; 3], pos)
            }
            _ => internal(pos, "a vec expression"),
        }
    }

    /// The place `teleport` and `move_to` go to: a `vec`, or an object's
    /// position, plus an offset.
    fn place(&mut self, f: &mut Fun, at: &'s Expr, offset: Option<&'s Expr>, pos: Pos) -> R<[Reg; 3]> {
        if self.ty_of(at)? == Type::Vec {
            return self.vec_expr(f, at);
        }
        let (o, _) = self.expr(f, at)?;
        let p = self.object_vec(f, o, caps::id::OBJECT_POS, pos)?;
        match offset {
            Some(off) => {
                let vo = self.vec_expr(f, off)?;
                self.vec_arith(f, ArithOp::Add, p, vo, pos)
            }
            None => Ok(p),
        }
    }

    fn binary(&mut self, f: &mut Fun, op: BinOp, a: &'s Expr, b: &'s Expr, pos: Pos) -> R<(Reg, Ty)> {
        if matches!(op, BinOp::And | BinOp::Or) {
            let dst = f.alloc(Ty::Bool, pos)?;
            let (ra, _) = self.expr(f, a)?;
            f.emit(Op::Move { dst, src: ra });
            let j = f.emit(if op == BinOp::And {
                Op::JumpIfNot { cond: dst, to: 0 }
            } else {
                Op::JumpIf { cond: dst, to: 0 }
            });
            let (rb, _) = self.expr(f, b)?;
            f.emit(Op::Move { dst, src: rb });
            f.patch(j, pos)?;
            return Ok((dst, Ty::Bool));
        }
        // A literal takes its type from the other side, which the checker
        // read first.
        let (ra, rb, ty) = if is_literal(a) && !is_literal(b) {
            let (rb, tb) = self.expr(f, b)?;
            let (ra, _) = self.expr(f, a)?;
            (ra, rb, tb)
        } else {
            let (ra, ta) = self.expr(f, a)?;
            let (rb, _) = self.expr(f, b)?;
            (ra, rb, ta)
        };
        let cmp = match op {
            BinOp::Eq => Some(CmpOp::Eq),
            BinOp::Ne => Some(CmpOp::Ne),
            BinOp::Lt => Some(CmpOp::Lt),
            BinOp::Le => Some(CmpOp::Le),
            BinOp::Gt => Some(CmpOp::Gt),
            BinOp::Ge => Some(CmpOp::Ge),
            _ => None,
        };
        if let Some(c) = cmp {
            let dst = f.alloc(Ty::Bool, pos)?;
            f.emit(Op::Cmp {
                op: c,
                dst,
                a: ra,
                b: rb,
            });
            return Ok((dst, Ty::Bool));
        }
        let Some(nt) = numeric(ty) else {
            return internal(pos, "arithmetic on no number");
        };
        let aop = match op {
            BinOp::Add => ArithOp::Add,
            BinOp::Sub => ArithOp::Sub,
            BinOp::Mul => ArithOp::Mul,
            BinOp::Div => ArithOp::Div,
            _ => ArithOp::Rem,
        };
        let dst = f.alloc(ty, pos)?;
        f.emit(Op::Arith {
            op: aop,
            ty: nt,
            dst,
            a: ra,
            b: rb,
        });
        Ok((dst, ty))
    }

    // ---- calls ----

    fn call(&mut self, f: &mut Fun, name: &str, args: &'s [Expr], pos: Pos, need_value: bool) -> R<Option<(Reg, Ty)>> {
        match name {
            "count" | "first" => {
                let Some(ExprKind::Set(set)) = args.first().map(|a| &a.kind) else {
                    return internal(pos, "count or first of no set");
                };
                let source = self.set_source(f, set)?;
                let elem = source.base.elem();
                let binder = set.binder.as_deref();
                if name == "count" {
                    let acc = f.konst(Ty::Int, 0, pos)?;
                    let one = f.konst(Ty::Int, 1, pos)?;
                    self.foreach(f, source, set.filter.as_ref(), binder, Body::Count { acc, one }, pos)?;
                    return Ok(Some((acc, Ty::Int)));
                }
                let acc = f.konst(elem, NONE, pos)?;
                let none = f.konst(elem, NONE, pos)?;
                self.foreach(f, source, set.filter.as_ref(), binder, Body::First { acc, none }, pos)?;
                return Ok(Some((acc, elem)));
            }
            "len" => {
                let Some(ExprKind::Name(t)) = args.first().map(|a| &a.kind) else {
                    return internal(pos, "len of no table");
                };
                let Some(info) = self.tables.iter().find(|i| i.name == *t) else {
                    return internal(pos, "len of an unknown table");
                };
                let rows = info.rows as i32;
                return Ok(Some((f.konst(Ty::Int, rows, pos)?, Ty::Int)));
            }
            "seconds" => {
                let (n, _) = self.expr(f, &args[0])?;
                let rate = f.konst(Ty::Int, TICKS_PER_SECOND, pos)?;
                let dst = f.alloc(Ty::Int, pos)?;
                f.emit(Op::Arith {
                    op: ArithOp::Mul,
                    ty: NumTy::Int,
                    dst,
                    a: n,
                    b: rate,
                });
                return Ok(Some((dst, Ty::Int)));
            }
            // A time is ticks: `ticks(t)` is the same number as an int,
            // `whole_seconds(t)` the whole seconds in it.
            "ticks" => {
                let (t, _) = self.expr(f, &args[0])?;
                return Ok(Some((t, Ty::Int)));
            }
            "whole_seconds" => {
                let (t, _) = self.expr(f, &args[0])?;
                let rate = f.konst(Ty::Int, TICKS_PER_SECOND, pos)?;
                let dst = f.alloc(Ty::Int, pos)?;
                f.emit(Op::Arith {
                    op: ArithOp::Div,
                    ty: NumTy::Int,
                    dst,
                    a: t,
                    b: rate,
                });
                return Ok(Some((dst, Ty::Int)));
            }
            // A `vec` is read through `vec_expr`; one named as a statement
            // does nothing.
            "vec" if !need_value => return Ok(None),
            "length" => {
                let v = self.vec_expr(f, &args[0])?;
                let dst = f.alloc(Ty::Fixed, pos)?;
                f.emit(Op::Call {
                    entry: caps::id::VEC_LENGTH,
                    args: v.to_vec(),
                    dst: Some(dst),
                });
                return Ok(Some((dst, Ty::Fixed)));
            }
            "distance" => {
                let va = self.vec_expr(f, &args[0])?;
                let vb = self.vec_expr(f, &args[1])?;
                let d = self.vec_arith(f, ArithOp::Sub, va, vb, pos)?;
                let dst = f.alloc(Ty::Fixed, pos)?;
                f.emit(Op::Call {
                    entry: caps::id::VEC_LENGTH,
                    args: d.to_vec(),
                    dst: Some(dst),
                });
                return Ok(Some((dst, Ty::Fixed)));
            }
            // `teleport(o, at)`, `teleport(o, at, offset)` and
            // `move_to(o, at, t)`: the entry takes the place as three
            // fixed registers, read from an object or given as a `vec`.
            "teleport" | "move_to" if !self.fns.contains_key(name) => {
                let (o, _) = self.expr(f, &args[0])?;
                let at = self.place(f, &args[1], args.get(2).filter(|_| name == "teleport"), pos)?;
                let mut regs = vec![o, at[0], at[1], at[2]];
                if name == "move_to" {
                    regs.push(self.expr(f, &args[2])?.0);
                }
                let Some(entry) = caps::entry_named(name) else {
                    return internal(pos, "an unknown function");
                };
                f.emit(Op::Call {
                    entry: entry.id,
                    args: regs,
                    dst: None,
                });
                return Ok(None);
            }
            "grant" => {
                let (player, _) = self.expr(f, &args[0])?;
                let cap = self.cap_arg(&args[1])?;
                let mut uses = None;
                let mut palette = None;
                for a in &args[2..] {
                    match &a.kind {
                        ExprKind::Budget(n) => uses = Some(*n as u8),
                        ExprKind::Palette(p) => palette = self.palettes.iter().position(|(n, _)| n == p).map(|i| i as u8),
                        _ => return internal(a.pos, "a grant argument"),
                    }
                }
                let Some(uses) = uses else {
                    return internal(pos, "a grant with no budget");
                };
                f.emit(Op::Grant {
                    player,
                    cap,
                    uses,
                    palette,
                });
                return Ok(None);
            }
            "revoke" | "granted" => {
                let (player, _) = self.expr(f, &args[0])?;
                let cap = self.cap_arg(&args[1])?;
                if name == "revoke" {
                    f.emit(Op::Revoke { player, cap });
                    return Ok(None);
                }
                let dst = f.alloc(Ty::Bool, pos)?;
                f.emit(Op::Granted { dst, player, cap });
                return Ok(Some((dst, Ty::Bool)));
            }
            _ => {}
        }
        if let Some(&decl) = self.fns.get(name) {
            return self.inline(f, decl, args, pos);
        }
        if name == "message" {
            return self.message(f, args, pos);
        }
        if name == "mark" {
            return self.mark(f, args, pos);
        }
        let Some(entry) = caps::entry_named(name) else {
            return internal(pos, "an unknown function");
        };
        let mut regs = Vec::with_capacity(args.len());
        for a in args {
            regs.push(self.expr(f, a)?.0);
        }
        match entry.result {
            Some(ty) => {
                let dst = f.alloc(ty, pos)?;
                f.emit(Op::Call {
                    entry: entry.id,
                    args: regs,
                    dst: Some(dst),
                });
                Ok(Some((dst, ty)))
            }
            None => {
                if need_value {
                    return internal(pos, "a value of an action");
                }
                debug_assert_eq!(entry.kind, Kind::Action);
                f.emit(Op::Call {
                    entry: entry.id,
                    args: regs,
                    dst: None,
                });
                Ok(None)
            }
        }
    }

    /// `mark(wp, at)`: every viewer; `mark(wp, at, to)` with `to` a
    /// player, a team or `all`, which picks the entry (`caps::spelled`);
    /// `all` is the two-argument form.
    fn mark(&mut self, f: &mut Fun, args: &'s [Expr], pos: Pos) -> R<Option<(Reg, Ty)>> {
        let [wp, at, rest @ ..] = args else {
            return internal(pos, "mark of no two arguments");
        };
        let mut regs = Vec::with_capacity(3);
        regs.push(self.expr(f, wp)?.0);
        regs.push(self.expr(f, at)?.0);
        let entry = match rest {
            [] => caps::id::MARK,
            [to] => match &to.kind {
                ExprKind::Name(n) if n == caps::EVERYONE => caps::id::MARK,
                _ => {
                    let (r, ty) = self.expr(f, to)?;
                    regs.push(r);
                    match ty {
                        Ty::Player => caps::id::MARK_PLAYER,
                        Ty::Team => caps::id::MARK_TEAM,
                        _ => return internal(to.pos, "a mark for no player or team"),
                    }
                }
            },
            _ => return internal(pos, "mark of more than three arguments"),
        };
        f.emit(Op::Call {
            entry,
            args: regs,
            dst: None,
        });
        Ok(None)
    }

    /// `message(to, text)`: `to` is a player, a team or `all` (everyone),
    /// and picks the entry (`caps::spelled`).
    fn message(&mut self, f: &mut Fun, args: &'s [Expr], pos: Pos) -> R<Option<(Reg, Ty)>> {
        let [to, text] = args else {
            return internal(pos, "message of no two arguments");
        };
        let mut regs = Vec::with_capacity(2);
        let entry = match &to.kind {
            ExprKind::Name(n) if n == caps::EVERYONE => caps::id::MESSAGE_ALL,
            _ => {
                let (r, ty) = self.expr(f, to)?;
                regs.push(r);
                match ty {
                    Ty::Player => caps::id::MESSAGE,
                    Ty::Team => caps::id::MESSAGE_TEAM,
                    _ => return internal(to.pos, "a message to no player or team"),
                }
            }
        };
        let c: &'s Checked<'s> = self.c;
        let Some(template) = c.template(text) else {
            regs.push(self.expr(f, text)?.0);
            f.emit(Op::Call {
                entry,
                args: regs,
                dst: None,
            });
            return Ok(None);
        };
        // The holes' values first, then the stored template, the `fmt_*`
        // run and the message, with nothing between them (the verifier's
        // message rule).
        let mut values = Vec::with_capacity(template.holes.len());
        for (name, hole) in &template.holes {
            values.push((self.name(f, name, text.pos)?.0, *hole));
        }
        let index = self.string(&template.text, text.pos)?;
        regs.push(f.konst(Ty::Text, index as i32, text.pos)?);
        for (r, hole) in values {
            f.emit(Op::Call {
                entry: hole.entry(),
                args: vec![r],
                dst: None,
            });
        }
        f.emit(Op::Call {
            entry,
            args: regs,
            dst: None,
        });
        Ok(None)
    }

    /// A capability argument (`forge.place`): its index in `requires`.
    fn cap_arg(&self, e: &Expr) -> R<u8> {
        fn path(e: &Expr) -> Option<String> {
            match &e.kind {
                ExprKind::Name(n) => Some(n.clone()),
                ExprKind::Field(b, f) => path(b).map(|p| format!("{p}.{f}")),
                _ => None,
            }
        }
        match path(e).and_then(|name| self.requires.iter().position(|r| *r == name)) {
            Some(i) => Ok(i as u8),
            None => internal(e.pos, "a capability not in requires"),
        }
    }

    /// A function's body at its call, with its parameters bound to the
    /// arguments: a block function's statements, or a value function's
    /// expression, whose register it gives.
    fn inline(&mut self, f: &mut Fun, decl: &'s FnDecl, args: &'s [Expr], pos: Pos) -> R<Option<(Reg, Ty)>> {
        if f.inlining.contains(&decl.name) {
            return err(pos, format!("fn {} calls itself; recursion is not allowed", decl.name));
        }
        if f.inlining.len() >= 16 {
            return err(pos, "functions nest more than 16 deep");
        }
        let mut bound = Vec::with_capacity(args.len());
        for (a, (pname, pty)) in args.iter().zip(&decl.params) {
            let local = match self.declared(pty)? {
                Type::Set(_) => {
                    let ExprKind::Name(n) = &a.kind else {
                        return internal(a.pos, "a set argument of no name");
                    };
                    Local::Set(self.set_label(f, n, a.pos)?)
                }
                _ => {
                    let (r, ty) = self.expr(f, a)?;
                    Local::Reg(r, ty)
                }
            };
            bound.push((pname.clone(), local));
        }
        // The body sees its parameters, globals, options and labels only.
        let scopes = std::mem::replace(&mut f.scopes, vec![bound]);
        let implicit = std::mem::take(&mut f.implicit);
        f.inlining.push(decl.name.clone());
        let result = match &decl.value {
            Some(v) => self.expr(f, v).map(Some),
            None => self.block(f, &decl.body).map(|()| None),
        };
        f.inlining.pop();
        f.implicit = implicit;
        f.scopes = scopes;
        result
    }
}

fn numeric(t: Ty) -> Option<NumTy> {
    match t {
        Ty::Int => Some(NumTy::Int),
        Ty::Fixed => Some(NumTy::Fixed),
        _ => None,
    }
}

/// A literal whose type comes from context.
fn is_literal(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Int(_) | ExprKind::None | ExprKind::Str(_) => true,
        ExprKind::Unary(UnOp::Neg, inner) => matches!(inner.kind, ExprKind::Int(_)),
        _ => false,
    }
}
