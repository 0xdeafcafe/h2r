//! Type checking and lowering: a [`Script`] to [`Ir`].
//!
//! One pass checks types and emits register code. Functions are inlined
//! at each call, so a handler's code is straight-line except for forward
//! jumps and `ForEach` bodies. Strings, labels and tags are interned in
//! first-use order, which makes the blob a function of the source alone.

use std::collections::BTreeMap;

use super::ast::*;
use super::{CompileError, Pos};
use crate::caps::{self, CARRIERS, Kind, TICKS_PER_SECOND, VARIANT_FIELDS};
use crate::ir::*;
use crate::ty::{MAX_ENUM_VARIANTS, MAX_ENUMS, MAX_SQUADS, MAX_TEAMS, NONE, Ty, fixed};

type R<T> = Result<T, CompileError>;

fn err<T>(pos: Pos, message: impl Into<String>) -> R<T> {
    Err(CompileError::at(pos, message))
}

fn int32(n: i64, pos: Pos) -> R<i32> {
    i32::try_from(n).or_else(|_| err(pos, "number out of the int range"))
}

/// Fields every player, team, object or squad has, read through an entry.
const FIELDS: &[(Ty, &str, u16)] = &[
    (Ty::Player, "team", caps::id::TEAM_OF),
    (Ty::Player, "alive", caps::id::ALIVE),
    (Ty::Player, "score", caps::id::PLAYER_SCORE),
    (Ty::Player, "kills", caps::id::KILLS),
    (Ty::Player, "deaths", caps::id::DEATHS),
    (Ty::Player, "lives", caps::id::LIVES),
    (Ty::Team, "score", caps::id::TEAM_SCORE),
    (Ty::Object, "number", caps::id::LABEL_NUMBER),
    (Ty::Object, "label", caps::id::LABEL_OF),
    (Ty::Object, "id", caps::id::ZONE_ID),
    (Ty::Object, "carrier", caps::id::CARRIER_OF),
    (Ty::Object, "team", caps::id::OBJECT_TEAM),
    (Ty::Squad, "alive", caps::id::SQUAD_ALIVE),
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
}

#[derive(Default)]
struct Fun {
    regs: Vec<Ty>,
    code: Vec<Op>,
    free: BTreeMap<Ty, Vec<Reg>>,
    live: Vec<Reg>,
    scopes: Vec<Vec<(String, Local)>>,
    inlining: Vec<String>,
    /// The element a `where` reads fields of without naming it.
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
    /// `first`: keep the first member in `acc`.
    First { acc: Reg, none: Reg },
}

struct Cx<'s> {
    fns: BTreeMap<&'s str, &'s FnDecl>,
    strings: Vec<String>,
    string_ix: BTreeMap<String, u16>,
    labels: Vec<String>,
    label_ix: BTreeMap<String, u16>,
    tags: Vec<String>,
    tag_ix: BTreeMap<String, u16>,
    options: Vec<(String, OptionDef)>,
    enums: Vec<(String, Vec<String>)>,
    vars: Vec<VarInfo>,
    slots: Vec<Slot>,
    tables: Vec<TableInfo>,
    table_defs: Vec<Table>,
    palettes: Vec<(String, Palette)>,
    requires: Vec<String>,
    handlers: Vec<Handler>,
}

pub fn lower(script: &Script) -> R<Ir> {
    let mut cx = Cx {
        fns: BTreeMap::new(),
        strings: Vec::new(),
        string_ix: BTreeMap::new(),
        labels: Vec::new(),
        label_ix: BTreeMap::new(),
        tags: Vec::new(),
        tag_ix: BTreeMap::new(),
        options: Vec::new(),
        enums: Vec::new(),
        vars: Vec::new(),
        slots: Vec::new(),
        tables: Vec::new(),
        table_defs: Vec::new(),
        palettes: Vec::new(),
        requires: Vec::new(),
        handlers: Vec::new(),
    };
    cx.script(script)
}

impl<'s> Cx<'s> {
    fn script(&mut self, script: &'s Script) -> R<Ir> {
        let pos = script.pos;
        if script.name.is_empty() || script.name.len() > MAX_TEXT_BYTES {
            return err(pos, "a script name is 1 to 128 bytes");
        }
        let version = u16::try_from(script.version).or_else(|_| err(pos, "the version is 0 to 65535"))?;
        let Some((carrier_name, cpos)) = &script.carrier else {
            return err(pos, "a script names its carrier: carrier slayer");
        };
        let carrier = CARRIERS
            .iter()
            .find(|(n, _)| n == carrier_name)
            .map(|&(_, i)| i)
            .ok_or_else(|| CompileError::at(*cpos, format!("unknown carrier {carrier_name}")))?;
        let teams = match script.teams {
            None => 0,
            Some((n, tpos)) => {
                if !(1..=MAX_TEAMS as i64).contains(&n) {
                    return err(tpos, "teams is 1 to 8");
                }
                n as u8
            }
        };
        for (name, rpos) in &script.requires {
            if caps::capability(name, caps::API_VERSION).is_none() {
                return err(*rpos, format!("unknown capability {name}"));
            }
            if self.requires.contains(name) {
                return err(*rpos, format!("capability {name} is required twice"));
            }
            if self.requires.len() >= MAX_REQUIRES {
                return err(*rpos, "more than 32 capabilities");
            }
            self.requires.push(name.clone());
        }

        // Declarations first, so their order does not matter to handlers.
        for item in &script.items {
            if let Item::Enum {
                name,
                variants,
                pos,
            } = item
            {
                self.enum_decl(name, variants, *pos)?;
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
                    if self.fns.insert(f.name.as_str(), f).is_some() {
                        return err(f.pos, format!("fn {} is declared twice", f.name));
                    }
                }
                Item::Enum { .. } | Item::On { .. } => {}
            }
        }
        for item in &script.items {
            if let Item::On {
                event,
                args,
                body,
                pos,
            } = item
            {
                self.handler(event, args, body, *pos)?;
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
        let ir = Ir {
            header: Header {
                ir_version: IR_VERSION,
                api_version: caps::API_VERSION,
                name: script.name.clone(),
                version,
                carrier,
                teams,
                requires: self.requires.clone(),
            },
            strings: std::mem::take(&mut self.strings),
            options: self.options.drain(..).map(|(_, o)| o).collect(),
            enums: enum_defs,
            slots: std::mem::take(&mut self.slots),
            labels: std::mem::take(&mut self.labels),
            tags: std::mem::take(&mut self.tags),
            palettes: self.palettes.drain(..).map(|(_, p)| p).collect(),
            tables: std::mem::take(&mut self.table_defs),
            handlers: std::mem::take(&mut self.handlers),
        };
        let bytes = ir.encode().len();
        if bytes > MAX_IR_BYTES {
            return err(pos, format!("the compiled script is {bytes} bytes, over the 64 KiB cap"));
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

    fn label(&mut self, s: &str, pos: Pos) -> R<u16> {
        if s.is_empty() || s.len() > MAX_LABEL_BYTES {
            return err(pos, "a label is 1 to 32 bytes");
        }
        Self::intern(&mut self.labels, &mut self.label_ix, s, pos)
    }

    fn tag(&mut self, s: &str, pos: Pos) -> R<u16> {
        if s.is_empty() || s.len() > MAX_TAG_BYTES {
            return err(pos, "a tag path is 1 to 256 bytes");
        }
        Self::intern(&mut self.tags, &mut self.tag_ix, s, pos)
    }

    fn ty(&self, t: &TypeName) -> R<Ty> {
        Ok(match t.name.as_str() {
            "int" => Ty::Int,
            "fixed" => Ty::Fixed,
            "bool" => Ty::Bool,
            "player" => Ty::Player,
            "team" => Ty::Team,
            "object" => Ty::Object,
            "squad" => Ty::Squad,
            "timer" => Ty::Timer,
            "Class" | "class" => Ty::Class,
            "tag" => Ty::Tag,
            "label" => Ty::Label,
            "text" => Ty::Text,
            other => match self.enums.iter().position(|(n, _)| n == other) {
                Some(i) => Ty::Enum(i as u8),
                None => return err(t.pos, format!("unknown type {other}")),
            },
        })
    }

    fn ty_name(&self, t: Ty) -> String {
        match t {
            Ty::Enum(i) => self
                .enums
                .get(i as usize)
                .map_or_else(|| "enum".to_string(), |(n, _)| n.clone()),
            t => t.name().to_string(),
        }
    }

    // ---- declarations ----

    fn enum_decl(&mut self, name: &str, variants: &[String], pos: Pos) -> R<()> {
        if self.enums.iter().any(|(n, _)| n == name) {
            return err(pos, format!("enum {name} is declared twice"));
        }
        if self.enums.len() >= MAX_ENUMS {
            return err(pos, "more than 32 enums");
        }
        if variants.is_empty() || variants.len() > MAX_ENUM_VARIANTS {
            return err(pos, "an enum has 1 to 64 variants");
        }
        for (i, v) in variants.iter().enumerate() {
            if variants[..i].contains(v) {
                return err(pos, format!("{name}.{v} is declared twice"));
            }
        }
        self.enums.push((name.to_string(), variants.to_vec()));
        Ok(())
    }

    fn option(&mut self, o: &OptionDecl) -> R<()> {
        let pos = o.pos;
        if self.options.iter().any(|(n, _)| *n == o.name) {
            return err(pos, format!("option {} is declared twice", o.name));
        }
        if self.options.len() >= 32 {
            return err(pos, "more than 32 options");
        }
        let ty = self.ty(&o.ty)?;
        let (lo, hi) = match ty {
            Ty::Int => {
                let Some((lo, hi)) = &o.range else {
                    return err(pos, "an int option needs a range: int 1..5");
                };
                let lo = self.const_int(lo)?;
                let hi = self.const_int(hi)?;
                if lo > hi {
                    return err(pos, "the range is empty");
                }
                (lo, hi)
            }
            Ty::Bool => (0, 1),
            Ty::Enum(e) => (0, self.enums[e as usize].1.len() as i32 - 1),
            _ => return err(o.ty.pos, "an option is an int, a bool or an enum"),
        };
        if ty != Ty::Int && o.range.is_some() {
            return err(pos, "only an int option takes a range");
        }
        let default = self.const_val(&o.default, ty)?;
        if default < lo || default > hi {
            return err(o.default.pos, "the default is outside the range");
        }
        let name = self.string(&o.name, pos)?;
        let label = match &o.label {
            Some(l) => self.string(l, pos)?,
            None => name,
        };
        let description = match &o.description {
            Some(d) => Some(self.string(d, pos)?),
            None => None,
        };
        let def = OptionDef {
            name,
            label,
            description,
            ty,
            lo,
            hi,
            default,
        };
        let bits: u32 = self.options.iter().map(|(_, o)| o.bits()).sum::<u32>() + def.bits();
        if bits > OPTION_BITS {
            return err(pos, format!("the options take {bits} bits; a variant carries 32"));
        }
        self.options.push((o.name.clone(), def));
        Ok(())
    }

    fn palette(&mut self, name: &str, tags: &[String], pos: Pos) -> R<()> {
        if self.palettes.iter().any(|(n, _)| n == name) {
            return err(pos, format!("palette {name} is declared twice"));
        }
        if self.palettes.len() >= 255 {
            return err(pos, "more than 255 palettes");
        }
        if tags.is_empty() || tags.len() > MAX_PALETTE_TAGS {
            return err(pos, "a palette has 1 to 64 tags");
        }
        let n = self.string(name, pos)?;
        let mut list = Vec::with_capacity(tags.len());
        for t in tags {
            list.push(self.tag(t, pos)?);
        }
        self.palettes.push((name.to_string(), Palette { name: n, tags: list }));
        Ok(())
    }

    fn var(&mut self, scope: Scope, name: &str, ty: &TypeName, init: Option<&Expr>, pos: Pos) -> R<()> {
        let t = self.ty(ty)?;
        if t == Ty::Timer && scope != Scope::Global {
            return err(pos, "a timer is a global variable");
        }
        if self.vars.iter().any(|v| v.scope == scope && v.name == name) {
            return err(pos, format!("variable {name} is declared twice"));
        }
        if scope == Scope::Global && self.options.iter().any(|(n, _)| n == name) {
            return err(pos, format!("{name} is already an option"));
        }
        let owner = scope.owner();
        if let Some(o) = owner
            && FIELDS.iter().any(|&(fo, f, _)| fo == o && f == name)
        {
            return err(pos, format!("{name} is a built-in field of every {}", o.name()));
        }
        let (count, max) = match scope {
            Scope::Global => (0, MAX_GLOBAL_SLOTS),
            Scope::Player => (1, MAX_PLAYER_SLOTS),
            Scope::Team => (2, MAX_TEAM_SLOTS),
            Scope::Object => (3, MAX_OBJECT_SLOTS),
        };
        let used = self.vars.iter().filter(|v| v.scope as usize == count).count();
        if used >= max {
            return err(pos, format!("more than {max} variables of this scope"));
        }
        let init = match init {
            Some(e) => self.const_val(e, t)?,
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
        if self.tables.iter().any(|t| t.name == name) {
            return err(pos, format!("table {name} is declared twice"));
        }
        if self.tables.len() >= 255 {
            return err(pos, "more than 255 tables");
        }
        if columns.is_empty() || columns.len() > MAX_TABLE_COLS {
            return err(pos, "a table has 1 to 16 columns");
        }
        if rows.len() > MAX_TABLE_ROWS {
            return err(pos, "a table has at most 256 rows");
        }
        let mut cols = Vec::with_capacity(columns.len());
        let mut col_defs = Vec::with_capacity(columns.len());
        for (i, (cname, cty)) in columns.iter().enumerate() {
            let t = self.ty(cty)?;
            if t == Ty::Timer {
                return err(cty.pos, "a table holds no timers");
            }
            if columns[..i].iter().any(|(n, _)| n == cname) {
                return err(cty.pos, format!("column {cname} is declared twice"));
            }
            col_defs.push(Column {
                name: self.string(cname, pos)?,
                ty: t,
            });
            cols.push((cname.clone(), t));
        }
        let mut cells = Vec::with_capacity(rows.len() * cols.len());
        for row in rows {
            if row.len() != cols.len() {
                let p = row.first().map_or(pos, |e| e.pos);
                return err(p, format!("a row of {name} has {} values, not {}", row.len(), cols.len()));
            }
            for (cell, &(_, t)) in row.iter().zip(&cols) {
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
                _ => err(e.pos, "expected a constant int"),
            },
            _ => err(e.pos, "expected a constant int"),
        }
    }

    /// A constant of type `ty`: an initial value, a default or a cell.
    fn const_val(&mut self, e: &Expr, ty: Ty) -> R<i32> {
        let pos = e.pos;
        let v = match (&e.kind, ty) {
            (ExprKind::Int(_) | ExprKind::Unary(UnOp::Neg, _), Ty::Int | Ty::Timer) => self.const_int(e)?,
            (ExprKind::Int(_), Ty::Fixed) => fixed::from_int(self.const_int(e)?),
            (ExprKind::Fixed(v), Ty::Fixed) => *v,
            (ExprKind::Unary(UnOp::Neg, inner), Ty::Fixed) => match inner.kind {
                ExprKind::Fixed(v) => -v,
                ExprKind::Int(n) => fixed::from_int(int32(-n, pos)?),
                _ => return err(pos, "expected a constant fixed"),
            },
            (ExprKind::Bool(b), Ty::Bool) => *b as i32,
            (ExprKind::None, t) if t.nullable() => NONE,
            (ExprKind::Str(s), Ty::Tag) => self.tag(s, pos)? as i32,
            (ExprKind::Str(s), Ty::Label) => self.label(s, pos)? as i32,
            (ExprKind::Str(s), Ty::Class | Ty::Text) => self.string(s, pos)? as i32,
            (ExprKind::Team(inner), Ty::Team) => self.const_team(inner)?,
            (ExprKind::Field(base, v), Ty::Enum(i)) => {
                let (ename, variants) = &self.enums[i as usize];
                match &base.kind {
                    ExprKind::Name(n) if n == ename => match variants.iter().position(|x| x == v) {
                        Some(k) => k as i32,
                        None => return err(pos, format!("{ename} has no variant {v}")),
                    },
                    _ => return err(pos, format!("expected a constant {ename}")),
                }
            }
            (ExprKind::Call(name, args), Ty::Squad) if name == "squad" && args.len() == 1 => {
                let n = self.const_int(&args[0])?;
                if !(0..MAX_SQUADS as i32).contains(&n) {
                    return err(pos, "a squad slot is 0 to 15");
                }
                n
            }
            _ => return err(pos, format!("expected a constant {}", self.ty_name(ty))),
        };
        Ok(v)
    }

    fn const_team(&self, inner: &Expr) -> R<i32> {
        let n = self.const_int(inner)?;
        if !(0..MAX_TEAMS as i32).contains(&n) {
            return err(inner.pos, "a team is 0 to 7");
        }
        Ok(n)
    }

    fn var_named(&self, name: &str, scope: Scope) -> Option<(u16, Ty)> {
        self.vars
            .iter()
            .find(|v| v.scope == scope && v.name == name)
            .map(|v| (v.slot, v.ty))
    }

    // ---- handlers ----

    fn handler(&mut self, event: &str, args: &[String], body: &'s [Stmt], pos: Pos) -> R<()> {
        let Some(ev) = Event::named(event) else {
            return err(pos, format!("unknown event {event}"));
        };
        let mut f = Fun::default();
        f.scopes.push(Vec::new());
        let timer = if ev == Event::Timer {
            let [name] = args else {
                return err(pos, "on timer names one timer variable: on timer(t)");
            };
            match self.var_named(name, Scope::Global) {
                Some((slot, Ty::Timer)) => Some(slot),
                _ => return err(pos, format!("{name} is not a global timer")),
            }
        } else {
            let tys = ev.args();
            if args.len() != tys.len() {
                return err(pos, format!("on {event} takes {} arguments", tys.len()));
            }
            for (name, &ty) in args.iter().zip(tys) {
                let r = f.alloc(ty, pos)?;
                f.bind(name, Local::Reg(r, ty));
            }
            None
        };
        if self.handlers.iter().any(|h| h.event == ev && h.timer == timer) {
            return err(pos, format!("a second handler for on {event}"));
        }
        self.block(&mut f, body)?;
        f.here(pos)?;
        self.handlers.push(Handler {
            event: ev,
            timer,
            regs: f.regs,
            code: f.code,
        });
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
            Stmt::Let { name, value, pos } => {
                if let ExprKind::Index(base, index) = &value.kind {
                    let (table, row) = self.row(f, base, index, *pos)?;
                    f.keep(mark, row);
                    f.bind(name, Local::Row(table, row));
                } else {
                    let (r, t) = self.expr(f, value, None)?;
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
                _ => return err(e.pos, "this expression does nothing; call something or assign it"),
            },
            Stmt::If { cond, then, els } => {
                let c = self.expect(f, cond, Ty::Bool)?;
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
            Stmt::ForEach {
                set,
                binder,
                body,
                pos,
            } => self.foreach(f, set, Some(binder.as_str()), Body::Stmts(body), *pos)?,
        }
        f.release(mark);
        Ok(())
    }

    fn assign(&mut self, f: &mut Fun, target: &'s Expr, value: &'s Expr, pos: Pos) -> R<()> {
        match &target.kind {
            ExprKind::Name(n) => {
                if f.lookup(n).is_some() {
                    return err(pos, format!("{n} is a let or a parameter and cannot be assigned"));
                }
                if let Some((slot, ty)) = self.var_named(n, Scope::Global) {
                    let r = self.expect(f, value, ty.register())?;
                    f.emit(Op::Store {
                        slot,
                        owner: None,
                        src: r,
                    });
                    return Ok(());
                }
                if self.options.iter().any(|(o, _)| o == n) {
                    return err(pos, format!("option {n} is read-only"));
                }
                err(pos, format!("unknown variable {n}"))
            }
            ExprKind::Field(base, field) => {
                let (owner, oty) = self.expr(f, base, None)?;
                let scope = match oty {
                    Ty::Player => Scope::Player,
                    Ty::Team => Scope::Team,
                    Ty::Object => Scope::Object,
                    _ => return err(pos, format!("a {} has no variables", self.ty_name(oty))),
                };
                let Some((slot, ty)) = self.var_named(field, scope) else {
                    return err(pos, format!("{field} is not a {} variable", oty.name()));
                };
                let r = self.expect(f, value, ty.register())?;
                f.emit(Op::Store {
                    slot,
                    owner: Some(owner),
                    src: r,
                });
                Ok(())
            }
            _ => err(pos, "only a variable can be assigned"),
        }
    }

    // ---- loops ----

    fn set_source(&mut self, f: &mut Fun, set: &'s SetExpr) -> R<Set> {
        Ok(match &set.base {
            SetBaseExpr::Players => Set {
                base: SetBase::Players,
                pick: None,
            },
            SetBaseExpr::Teams => Set {
                base: SetBase::Teams,
                pick: None,
            },
            SetBaseExpr::Labelled(l) => Set {
                base: SetBase::Labelled(self.label(l, set.pos)?),
                pick: None,
            },
            SetBaseExpr::Objectives => Set {
                base: SetBase::Objectives,
                pick: None,
            },
            SetBaseExpr::Pick(inner, n) => {
                if inner.filter.is_some() || inner.binder.is_some() {
                    return err(inner.pos, "pick takes a set with no name and no where");
                }
                let base = match &inner.base {
                    SetBaseExpr::Players => SetBase::Players,
                    SetBaseExpr::Teams => SetBase::Teams,
                    SetBaseExpr::Labelled(l) => SetBase::Labelled(self.label(l, inner.pos)?),
                    SetBaseExpr::Objectives => SetBase::Objectives,
                    SetBaseExpr::Pick(..) => return err(inner.pos, "pick of a pick"),
                };
                let n = self.expect(f, n, Ty::Int)?;
                Set {
                    base,
                    pick: Some(n),
                }
            }
        })
    }

    fn set_elem(set: &SetExpr) -> Ty {
        match &set.base {
            SetBaseExpr::Players => Ty::Player,
            SetBaseExpr::Teams => Ty::Team,
            SetBaseExpr::Labelled(_) | SetBaseExpr::Objectives => Ty::Object,
            SetBaseExpr::Pick(inner, _) => Self::set_elem(inner),
        }
    }

    fn foreach(&mut self, f: &mut Fun, set: &'s SetExpr, binder: Option<&str>, body: Body<'s>, pos: Pos) -> R<()> {
        let source = self.set_source(f, set)?;
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
        if let Some(filter) = &set.filter {
            f.implicit.push((dst, elem));
            let c = self.expect(f, filter, Ty::Bool);
            f.implicit.pop();
            let c = c?;
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
                let c = f.alloc(Ty::Bool, pos)?;
                f.emit(Op::Cmp {
                    op: CmpOp::Eq,
                    dst: c,
                    a: acc,
                    b: none,
                });
                skips.push(f.emit(Op::JumpIfNot { cond: c, to: 0 }));
                f.emit(Op::Move { dst: acc, src: dst });
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

    // ---- expressions ----

    /// Lowers `e` and checks it has type `ty`.
    fn expect(&mut self, f: &mut Fun, e: &'s Expr, ty: Ty) -> R<Reg> {
        let (r, got) = self.expr(f, e, Some(ty))?;
        if got != ty {
            return err(
                e.pos,
                format!("expected {}, found {}", self.ty_name(ty), self.ty_name(got)),
            );
        }
        Ok(r)
    }

    fn expr(&mut self, f: &mut Fun, e: &'s Expr, want: Option<Ty>) -> R<(Reg, Ty)> {
        let pos = e.pos;
        match &e.kind {
            ExprKind::Int(n) => {
                let v = int32(*n, pos)?;
                if want == Some(Ty::Fixed) {
                    Ok((f.konst(Ty::Fixed, fixed::from_int(v), pos)?, Ty::Fixed))
                } else {
                    Ok((f.konst(Ty::Int, v, pos)?, Ty::Int))
                }
            }
            ExprKind::Fixed(v) => Ok((f.konst(Ty::Fixed, *v, pos)?, Ty::Fixed)),
            ExprKind::Bool(b) => Ok((f.konst(Ty::Bool, *b as i32, pos)?, Ty::Bool)),
            ExprKind::None => match want {
                Some(t) if t.nullable() => Ok((f.konst(t, NONE, pos)?, t)),
                _ => err(pos, "none needs a player, team, object or tag here"),
            },
            ExprKind::Str(s) => {
                let (ty, index) = match want {
                    Some(Ty::Tag) => (Ty::Tag, self.tag(s, pos)?),
                    Some(Ty::Label) => (Ty::Label, self.label(s, pos)?),
                    Some(Ty::Class) => (Ty::Class, self.string(s, pos)?),
                    Some(Ty::Text) => (Ty::Text, self.string(s, pos)?),
                    _ => return err(pos, "a string is a tag path, a label, a class or a text here"),
                };
                Ok((f.konst(ty, index as i32, pos)?, ty))
            }
            ExprKind::Labelled(l) => {
                let index = self.label(l, pos)?;
                Ok((f.konst(Ty::Label, index as i32, pos)?, Ty::Label))
            }
            ExprKind::Team(inner) => {
                let n = self.const_team(inner)?;
                Ok((f.konst(Ty::Team, n, pos)?, Ty::Team))
            }
            ExprKind::Text(inner) => {
                let index = match &inner.kind {
                    ExprKind::Str(s) => self.string(s, pos)? as i32,
                    ExprKind::Int(n) => int32(*n, pos)?,
                    _ => return err(pos, "text takes a string or a string-table index"),
                };
                Ok((f.konst(Ty::Text, index, pos)?, Ty::Text))
            }
            ExprKind::Name(n) => self.name(f, n, pos),
            ExprKind::Field(base, field) => self.field(f, base, field, pos),
            ExprKind::Index(..) => err(pos, "a row is read by column: table[i].column"),
            ExprKind::Call(name, args) => match self.call(f, name, args, pos, true)? {
                Some(v) => Ok(v),
                None => err(pos, format!("{name} has no value")),
            },
            ExprKind::Unary(UnOp::Neg, inner) => {
                match inner.kind {
                    ExprKind::Int(n) if want != Some(Ty::Fixed) => {
                        return Ok((f.konst(Ty::Int, int32(-n, pos)?, pos)?, Ty::Int));
                    }
                    ExprKind::Fixed(v) => return Ok((f.konst(Ty::Fixed, -v, pos)?, Ty::Fixed)),
                    _ => {}
                }
                let (r, t) = self.expr(f, inner, want)?;
                let ty = numeric(t).ok_or_else(|| CompileError::at(pos, "- takes an int or a fixed"))?;
                let dst = f.alloc(t, pos)?;
                f.emit(Op::Neg { ty, dst, a: r });
                Ok((dst, t))
            }
            ExprKind::Unary(UnOp::Not, inner) => {
                let r = self.expect(f, inner, Ty::Bool)?;
                let dst = f.alloc(Ty::Bool, pos)?;
                f.emit(Op::Not { dst, a: r });
                Ok((dst, Ty::Bool))
            }
            ExprKind::Binary(op, a, b) => self.binary(f, *op, a, b, want, pos),
            ExprKind::Budget(_) | ExprKind::Palette(_) => {
                err(pos, "budget and palette are arguments of grant")
            }
            ExprKind::Set(_) => err(pos, "a set is only allowed in for each, count, first and pick"),
        }
    }

    fn name(&mut self, f: &mut Fun, n: &str, pos: Pos) -> R<(Reg, Ty)> {
        match f.lookup(n) {
            Some(Local::Reg(r, t)) => return Ok((r, t)),
            Some(Local::Row(..)) => return err(pos, format!("{n} is a row: read a column, as {n}.x")),
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
        if let Some(i) = self.options.iter().position(|(o, _)| o == n) {
            let ty = self.options[i].1.ty;
            let dst = f.alloc(ty, pos)?;
            f.emit(Op::Opt {
                dst,
                index: i as u8,
            });
            return Ok((dst, ty));
        }
        if let Some(v) = self.vars.iter().find(|v| v.name == n) {
            let owner = v.scope.owner().map_or("its owner", |t| t.name());
            return err(pos, format!("{n} is a variable of each {owner}: read it as x.{n}"));
        }
        err(pos, format!("unknown name {n}"))
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
        match &base.kind {
            ExprKind::Name(b) => match f.lookup(b) {
                Some(Local::Row(table, row)) => return self.column(f, table, row, field, pos),
                Some(Local::Reg(..)) => {}
                None => {
                    if b == "variant" {
                        let Some(i) = VARIANT_FIELDS.iter().position(|(n, _)| *n == field) else {
                            return err(pos, format!("variant has no field {field}"));
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
                            return err(pos, format!("{b} has no variant {field}"));
                        };
                        let ty = Ty::Enum(i as u8);
                        return Ok((f.konst(ty, k as i32, pos)?, ty));
                    }
                }
            },
            ExprKind::Index(tbase, index) => {
                let (table, row) = self.row(f, tbase, index, pos)?;
                return self.column(f, table, row, field, pos);
            }
            _ => {}
        }
        let (r, t) = self.expr(f, base, None)?;
        match self.field_of(f, r, t, field, pos)? {
            Some(v) => Ok(v),
            None => err(pos, format!("a {} has no field {field}", self.ty_name(t))),
        }
    }

    /// `table[index]`: the table and a register with the row index. An int
    /// indexes rows; a value of the first column's type finds its row.
    fn row(&mut self, f: &mut Fun, base: &'s Expr, index: &'s Expr, pos: Pos) -> R<(u8, Reg)> {
        let ExprKind::Name(tname) = &base.kind else {
            return err(pos, "only a table can be indexed");
        };
        let Some(t) = self.tables.iter().position(|t| t.name == *tname) else {
            return err(pos, format!("unknown table {tname}"));
        };
        let key_ty = self.tables[t].columns[0].1;
        let hint = if key_ty == Ty::Int { Ty::Int } else { key_ty };
        let (r, ty) = self.expr(f, index, Some(hint))?;
        if ty == Ty::Int {
            return Ok((t as u8, r));
        }
        if ty != key_ty {
            return err(
                index.pos,
                format!("{tname} is indexed by int or {}, not {}", self.ty_name(key_ty), self.ty_name(ty)),
            );
        }
        let dst = f.alloc(Ty::Int, pos)?;
        f.emit(Op::TableFind {
            dst,
            table: t as u8,
            key: r,
        });
        Ok((t as u8, dst))
    }

    fn column(&mut self, f: &mut Fun, table: u8, row: Reg, field: &str, pos: Pos) -> R<(Reg, Ty)> {
        let info = &self.tables[table as usize];
        let Some(col) = info.columns.iter().position(|(n, _)| n == field) else {
            return err(pos, format!("table {} has no column {field}", info.name));
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

    fn binary(&mut self, f: &mut Fun, op: BinOp, a: &'s Expr, b: &'s Expr, want: Option<Ty>, pos: Pos) -> R<(Reg, Ty)> {
        if matches!(op, BinOp::And | BinOp::Or) {
            let dst = f.alloc(Ty::Bool, pos)?;
            let ra = self.expect(f, a, Ty::Bool)?;
            f.emit(Op::Move { dst, src: ra });
            let j = f.emit(if op == BinOp::And {
                Op::JumpIfNot { cond: dst, to: 0 }
            } else {
                Op::JumpIf { cond: dst, to: 0 }
            });
            let rb = self.expect(f, b, Ty::Bool)?;
            f.emit(Op::Move { dst, src: rb });
            f.patch(j, pos)?;
            return Ok((dst, Ty::Bool));
        }
        let arith = matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem);
        let hint = if arith { want.filter(|t| numeric(*t).is_some()) } else { None };
        // A literal takes its type from the other side.
        let (ra, rb, ty) = if is_literal(a) && !is_literal(b) {
            let (rb, tb) = self.expr(f, b, hint)?;
            let ra = self.expect(f, a, tb)?;
            (ra, rb, tb)
        } else {
            let (ra, ta) = self.expr(f, a, hint)?;
            let rb = self.expect(f, b, ta)?;
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
            if !matches!(c, CmpOp::Eq | CmpOp::Ne) && numeric(ty).is_none() {
                return err(pos, format!("{} values have no order", self.ty_name(ty)));
            }
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
            return err(pos, format!("arithmetic on {}", self.ty_name(ty)));
        };
        let aop = match op {
            BinOp::Add => ArithOp::Add,
            BinOp::Sub => ArithOp::Sub,
            BinOp::Mul => ArithOp::Mul,
            BinOp::Div => ArithOp::Div,
            _ => ArithOp::Rem,
        };
        if aop == ArithOp::Rem && nt == NumTy::Fixed {
            return err(pos, "% takes ints");
        }
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
        let arity = |n: usize| -> R<()> {
            if args.len() == n {
                Ok(())
            } else {
                err(pos, format!("{name} takes {n} arguments"))
            }
        };
        match name {
            "count" | "first" => {
                arity(1)?;
                let ExprKind::Set(set) = &args[0].kind else {
                    return err(pos, format!("{name} takes a set"));
                };
                let elem = Self::set_elem(set);
                if name == "count" {
                    let acc = f.konst(Ty::Int, 0, pos)?;
                    let one = f.konst(Ty::Int, 1, pos)?;
                    self.foreach(f, set, set.binder.as_deref(), Body::Count { acc, one }, pos)?;
                    return Ok(Some((acc, Ty::Int)));
                }
                let acc = f.konst(elem, NONE, pos)?;
                let none = f.konst(elem, NONE, pos)?;
                self.foreach(f, set, set.binder.as_deref(), Body::First { acc, none }, pos)?;
                return Ok(Some((acc, elem)));
            }
            "pick" => return err(pos, "pick makes a set: use it in for each or count"),
            "len" => {
                arity(1)?;
                let ExprKind::Name(t) = &args[0].kind else {
                    return err(pos, "len takes a table");
                };
                let Some(info) = self.tables.iter().find(|i| i.name == *t) else {
                    return err(pos, format!("unknown table {t}"));
                };
                let rows = info.rows as i32;
                return Ok(Some((f.konst(Ty::Int, rows, pos)?, Ty::Int)));
            }
            "seconds" => {
                arity(1)?;
                let n = self.expect(f, &args[0], Ty::Int)?;
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
            "grant" => {
                if need_value {
                    return err(pos, "grant has no value");
                }
                if args.len() < 3 || args.len() > 4 {
                    return err(pos, "grant takes a player, a capability, a budget and a palette");
                }
                let player = self.expect(f, &args[0], Ty::Player)?;
                let cap = self.cap_arg(&args[1])?;
                let mut uses = None;
                let mut palette = None;
                for a in &args[2..] {
                    match &a.kind {
                        ExprKind::Budget(n) if uses.is_none() => {
                            if !(0..=255).contains(n) {
                                return err(a.pos, "a grant holds at most 255 uses");
                            }
                            uses = Some(*n as u8);
                        }
                        ExprKind::Palette(p) if palette.is_none() => {
                            let Some(i) = self.palettes.iter().position(|(n, _)| n == p) else {
                                return err(a.pos, format!("unknown palette {p}"));
                            };
                            palette = Some(i as u8);
                        }
                        _ => return err(a.pos, "expected budget n or palette \"x\""),
                    }
                }
                let Some(uses) = uses else {
                    return err(pos, "a grant needs a budget: budget n");
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
                arity(2)?;
                if name == "revoke" && need_value {
                    return err(pos, "revoke has no value");
                }
                let player = self.expect(f, &args[0], Ty::Player)?;
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
            if need_value {
                return err(pos, format!("fn {name} has no value"));
            }
            self.inline(f, decl, args, pos)?;
            return Ok(None);
        }
        let Some(entry) = caps::entry_named(name) else {
            return err(pos, format!("unknown function {name}"));
        };
        arity(entry.args.len())?;
        if let Some(cap) = entry.cap
            && !self.requires.iter().any(|r| r == cap)
        {
            return err(pos, format!("{name} needs capability {cap}: add it to requires"));
        }
        let mut regs = Vec::with_capacity(args.len());
        for (a, &ty) in args.iter().zip(entry.args) {
            regs.push(self.expect(f, a, ty)?);
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
                    return err(pos, format!("{name} has no value"));
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

    /// A capability argument (`forge.place`): its index in `requires`.
    fn cap_arg(&self, e: &Expr) -> R<u8> {
        fn path(e: &Expr) -> Option<String> {
            match &e.kind {
                ExprKind::Name(n) => Some(n.clone()),
                ExprKind::Field(b, f) => path(b).map(|p| format!("{p}.{f}")),
                _ => None,
            }
        }
        let Some(name) = path(e) else {
            return err(e.pos, "expected a capability, as forge.place");
        };
        let Some(i) = self.requires.iter().position(|r| *r == name) else {
            return err(e.pos, format!("capability {name} is not in requires"));
        };
        if !caps::capability(&name, caps::API_VERSION).is_some_and(|c| c.grantable) {
            return err(e.pos, format!("capability {name} cannot be granted"));
        }
        Ok(i as u8)
    }

    fn inline(&mut self, f: &mut Fun, decl: &'s FnDecl, args: &'s [Expr], pos: Pos) -> R<()> {
        if f.inlining.iter().any(|n| *n == decl.name) {
            return err(pos, format!("fn {} calls itself; recursion is not allowed", decl.name));
        }
        if f.inlining.len() >= 16 {
            return err(pos, "functions nest more than 16 deep");
        }
        if args.len() != decl.params.len() {
            return err(pos, format!("{} takes {} arguments", decl.name, decl.params.len()));
        }
        let mut bound = Vec::with_capacity(args.len());
        for (a, (pname, pty)) in args.iter().zip(&decl.params) {
            let ty = self.ty(pty)?;
            if ty == Ty::Timer {
                return err(pty.pos, "a parameter is not a timer; pass an int");
            }
            let r = self.expect(f, a, ty)?;
            bound.push((pname.clone(), Local::Reg(r, ty)));
        }
        // The body sees its parameters, globals and options only.
        let scopes = std::mem::replace(&mut f.scopes, vec![bound]);
        let implicit = std::mem::take(&mut f.implicit);
        f.inlining.push(decl.name.clone());
        let result = self.block(f, &decl.body);
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
