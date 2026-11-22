//! The verifier: reads IR, never source, and refuses a blob whole.
//!
//! It checks every op, dead branches included: register and slot indices
//! and types, forward jumps inside their region, set bounds, label, tag,
//! string and table indices, API entry numbers and their capabilities
//! against the header's `requires`. It also reports the worst-tick
//! estimate. See `docs/game/rules-engine.md`, "Compiler and verifier" and
//! "Budget".

use crate::caps::{self, Kind, VARIANT_FIELDS};
use crate::ir::*;
use crate::ty::{MAX_ENUM_VARIANTS, MAX_ENUMS, Sizes, Ty};

/// Engine indices a script may ride (`caps::CARRIERS`).
fn carrier_known(index: u8) -> bool {
    caps::CARRIERS.iter().any(|&(_, i)| i == index)
}

/// Why a blob was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    #[error(transparent)]
    Decode(#[from] DecodeError),
    #[error("IR version {0} is unsupported (this build reads {IR_VERSION})")]
    IrVersion(u16),
    #[error("API version {0} is unsupported (this build has {api})", api = caps::API_VERSION)]
    ApiVersion(u16),
    #[error("engine index {0} cannot carry a script")]
    Carrier(u8),
    #[error("the script needs {0} teams; at most 8 exist")]
    Teams(u8),
    #[error("{what} exceeds its limit of {max}")]
    Limit { what: &'static str, max: usize },
    #[error("unknown capability {0}")]
    UnknownCapability(String),
    #[error("capability {0} is declared twice")]
    DuplicateCapability(String),
    #[error("{table} entry {index} is longer than {max} bytes or empty")]
    Text {
        table: &'static str,
        index: usize,
        max: usize,
    },
    #[error("{what} index {index} is out of range")]
    Index { what: &'static str, index: u32 },
    #[error("option {0} has an invalid type, range or default")]
    Option(usize),
    #[error("the options need {0} bits; a record carries {OPTION_BITS}")]
    OptionBits(u32),
    #[error("slot {0} has an invalid type, scope or initial value")]
    Slot(usize),
    #[error("table {table} cell {cell} is not a value of its column")]
    Cell { table: usize, cell: usize },
    #[error("handler {handler}: {reason}")]
    Handler {
        handler: usize,
        reason: &'static str,
    },
    #[error("two handlers answer {0}")]
    DuplicateHandler(String),
    #[error("handler {handler}, op {pc}: {fault}")]
    Op {
        handler: usize,
        pc: usize,
        fault: Fault,
    },
}

/// What is wrong with one op.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Fault {
    #[error("register {0} does not exist")]
    Register(Reg),
    #[error("register {reg} is {got:?}, not {want}")]
    Type {
        reg: Reg,
        want: &'static str,
        got: Ty,
    },
    #[error("{0} is not a value of the register's type")]
    Const(i32),
    #[error("slot {0} does not exist")]
    Slot(u16),
    #[error("slot {0} is read or written without its owner")]
    Owner(u16),
    #[error("option {0} does not exist")]
    Option(u8),
    #[error("variant field {0} does not exist")]
    VariantField(u8),
    #[error("table {0} does not exist")]
    Table(u8),
    #[error("column {0} does not exist")]
    Column(u8),
    #[error("jump to {0} is not forward")]
    Backward(u16),
    #[error("jump to {0} leaves its region")]
    JumpOut(u16),
    #[error("a body of {0} ops leaves its region")]
    BodyOut(u16),
    #[error("label {0} does not exist")]
    Label(u16),
    #[error("API entry {0} does not exist in this API version")]
    Entry(u16),
    #[error("{entry} takes {want} arguments, given {got}")]
    Arity {
        entry: &'static str,
        want: usize,
        got: usize,
    },
    #[error("{entry} returns {want:?}, written to {got:?}")]
    Result {
        entry: &'static str,
        want: Option<Ty>,
        got: Option<Ty>,
    },
    #[error("{entry} needs capability {cap}, which the header does not declare")]
    Undeclared {
        entry: &'static str,
        cap: &'static str,
    },
    #[error("capability index {0} is not in requires")]
    CapIndex(u8),
    #[error("capability {0} cannot be granted")]
    NotGrantable(String),
    #[error("palette {0} does not exist")]
    Palette(u8),
    #[error("remainder of fixed values")]
    FixedRem,
}

/// A passing blob's budget figures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// Each handler's worst-case op count, in handler order.
    pub handler_ops: Vec<u64>,
    /// Each handler's worst case times the events the estimate assumes a
    /// tick holds (`Event::worst_per_tick`), summed. `on enter` and
    /// `on leave` share their 64 runs.
    pub worst_tick: u64,
    /// The handler that costs the most in the estimate.
    pub costliest: Option<usize>,
}

impl Report {
    /// Whether the estimate fits the VM's per-tick budget.
    pub fn fits(&self) -> bool {
        self.worst_tick <= crate::vm::TICK_BUDGET as u64
    }
}

/// Decodes and verifies a blob.
pub fn load(bytes: &[u8]) -> Result<(Ir, Report), Refusal> {
    let ir = Ir::decode(bytes)?;
    let report = verify(&ir)?;
    Ok((ir, report))
}

/// Verifies a decoded blob.
pub fn verify(ir: &Ir) -> Result<Report, Refusal> {
    let h = &ir.header;
    if h.ir_version != IR_VERSION {
        return Err(Refusal::IrVersion(h.ir_version));
    }
    if h.api_version != caps::API_VERSION {
        return Err(Refusal::ApiVersion(h.api_version));
    }
    if !carrier_known(h.carrier) {
        return Err(Refusal::Carrier(h.carrier));
    }
    if h.teams as usize > crate::ty::MAX_TEAMS {
        return Err(Refusal::Teams(h.teams));
    }
    if h.name.is_empty() || h.name.len() > MAX_TEXT_BYTES {
        return Err(Refusal::Text {
            table: "the name",
            index: 0,
            max: MAX_TEXT_BYTES,
        });
    }
    if h.requires.len() > MAX_REQUIRES {
        return Err(Refusal::Limit {
            what: "requires",
            max: MAX_REQUIRES,
        });
    }
    for (i, name) in h.requires.iter().enumerate() {
        if caps::capability(name, h.api_version).is_none() {
            return Err(Refusal::UnknownCapability(name.clone()));
        }
        if h.requires[..i].contains(name) {
            return Err(Refusal::DuplicateCapability(name.clone()));
        }
    }
    text_table(&ir.strings, "the string table", MAX_TEXT_BYTES)?;
    text_table(&ir.labels, "the label table", MAX_LABEL_BYTES)?;
    text_table(&ir.tags, "the tag table", MAX_TAG_BYTES)?;
    let sizes = ir.sizes();
    let string = |index: u16| -> Result<(), Refusal> {
        if (index as usize) < ir.strings.len() {
            Ok(())
        } else {
            Err(Refusal::Index {
                what: "string",
                index: index as u32,
            })
        }
    };

    if ir.enums.len() > MAX_ENUMS {
        return Err(Refusal::Limit {
            what: "enums",
            max: MAX_ENUMS,
        });
    }
    for e in &ir.enums {
        string(e.name)?;
        if e.variants.is_empty() || e.variants.len() > MAX_ENUM_VARIANTS {
            return Err(Refusal::Limit {
                what: "enum variants",
                max: MAX_ENUM_VARIANTS,
            });
        }
        for &v in &e.variants {
            string(v)?;
        }
    }

    let mut bits = 0u32;
    for (i, o) in ir.options.iter().enumerate() {
        string(o.name)?;
        string(o.label)?;
        if let Some(d) = o.description {
            string(d)?;
        }
        let typed = match o.ty {
            Ty::Int => true,
            Ty::Bool => o.lo == 0 && o.hi == 1,
            Ty::Enum(e) => {
                o.lo == 0 && sizes.enum_variants.get(e as usize).is_some_and(|&n| o.hi as i64 == n as i64 - 1)
            }
            _ => false,
        };
        if !typed || o.lo > o.hi || o.default < o.lo || o.default > o.hi {
            return Err(Refusal::Option(i));
        }
        bits = bits.saturating_add(o.bits());
    }
    if bits > OPTION_BITS {
        return Err(Refusal::OptionBits(bits));
    }

    let mut per_scope = [0usize; 4];
    for (i, s) in ir.slots.iter().enumerate() {
        string(s.name)?;
        per_scope[s.scope as usize] += 1;
        let timer_ok = s.ty != Ty::Timer || s.scope == Scope::Global;
        if !timer_ok || !s.ty.valid(s.init, &sizes) {
            return Err(Refusal::Slot(i));
        }
    }
    let budgets = [
        ("global slots", MAX_GLOBAL_SLOTS),
        ("player slots", MAX_PLAYER_SLOTS),
        ("team slots", MAX_TEAM_SLOTS),
        ("object slots", MAX_OBJECT_SLOTS),
    ];
    for (n, (what, max)) in per_scope.iter().zip(budgets) {
        if *n > max {
            return Err(Refusal::Limit { what, max });
        }
    }

    if ir.palettes.len() > 255 {
        return Err(Refusal::Limit {
            what: "palettes",
            max: 255,
        });
    }
    for p in &ir.palettes {
        string(p.name)?;
        if p.tags.is_empty() || p.tags.len() > MAX_PALETTE_TAGS {
            return Err(Refusal::Limit {
                what: "palette tags",
                max: MAX_PALETTE_TAGS,
            });
        }
        for &t in &p.tags {
            if t as usize >= ir.tags.len() {
                return Err(Refusal::Index {
                    what: "tag",
                    index: t as u32,
                });
            }
        }
    }

    if ir.tables.len() > 255 {
        return Err(Refusal::Limit {
            what: "tables",
            max: 255,
        });
    }
    for (ti, t) in ir.tables.iter().enumerate() {
        string(t.name)?;
        if t.columns.is_empty() || t.columns.len() > MAX_TABLE_COLS {
            return Err(Refusal::Limit {
                what: "table columns",
                max: MAX_TABLE_COLS,
            });
        }
        if t.cells.len() % t.columns.len() != 0 || t.rows() > MAX_TABLE_ROWS {
            return Err(Refusal::Limit {
                what: "table rows",
                max: MAX_TABLE_ROWS,
            });
        }
        for c in &t.columns {
            string(c.name)?;
            if c.ty == Ty::Timer {
                return Err(Refusal::Cell { table: ti, cell: 0 });
            }
        }
        for (ci, &v) in t.cells.iter().enumerate() {
            if !t.columns[ci % t.columns.len()].ty.valid(v, &sizes) {
                return Err(Refusal::Cell {
                    table: ti,
                    cell: ci,
                });
            }
        }
    }

    let mut handler_ops = Vec::with_capacity(ir.handlers.len());
    for (hi, hd) in ir.handlers.iter().enumerate() {
        for other in &ir.handlers[..hi] {
            if other.event == hd.event && other.timer == hd.timer {
                return Err(Refusal::DuplicateHandler(hd.event.name().to_string()));
            }
        }
        let bad = |reason| Refusal::Handler {
            handler: hi,
            reason,
        };
        match (hd.event, hd.timer) {
            (Event::Timer, Some(slot)) => {
                let ok = ir
                    .slots
                    .get(slot as usize)
                    .is_some_and(|s| s.ty == Ty::Timer && s.scope == Scope::Global);
                if !ok {
                    return Err(bad("on timer names no global timer slot"));
                }
            }
            (Event::Timer, None) => return Err(bad("on timer names no slot")),
            (_, Some(_)) => return Err(bad("only on timer names a slot")),
            (_, None) => {}
        }
        if hd.regs.len() > MAX_REGS {
            return Err(Refusal::Limit {
                what: "registers",
                max: MAX_REGS,
            });
        }
        if hd.code.len() > MAX_OPS {
            return Err(Refusal::Limit {
                what: "ops",
                max: MAX_OPS,
            });
        }
        let args = hd.event.args();
        if hd.regs.len() < args.len() || &hd.regs[..args.len()] != args {
            return Err(bad("argument registers do not match the event"));
        }
        if hd.regs.iter().any(|&t| t == Ty::Timer || !enum_known(t, &sizes)) {
            return Err(bad("a register has an invalid type"));
        }
        let cx = Cx { ir, hd, sizes: &sizes };
        let mut ends: Vec<usize> = Vec::new();
        for (pc, op) in hd.code.iter().enumerate() {
            while ends.last() == Some(&pc) {
                ends.pop();
            }
            let region = ends.last().copied().unwrap_or(hd.code.len());
            cx.op(pc, op, region, &mut ends).map_err(|fault| Refusal::Op {
                handler: hi,
                pc,
                fault,
            })?;
        }
        handler_ops.push(cost(&hd.code, 0, hd.code.len()));
    }

    let mut worst_tick = 0u64;
    let mut zone = 0u64;
    let mut costliest: Option<(usize, u64)> = None;
    for (i, (hd, &ops)) in ir.handlers.iter().zip(&handler_ops).enumerate() {
        let total = match hd.event {
            // Enter and leave share 64 runs a tick; the dearer one bounds it.
            Event::Enter | Event::Leave => {
                zone = zone.max(ops);
                ops.saturating_mul(hd.event.worst_per_tick())
            }
            e => {
                let t = ops.saturating_mul(e.worst_per_tick());
                worst_tick = worst_tick.saturating_add(t);
                t
            }
        };
        if costliest.is_none_or(|(_, c)| total > c) {
            costliest = Some((i, total));
        }
    }
    worst_tick = worst_tick.saturating_add(zone.saturating_mul(64));
    Ok(Report {
        handler_ops,
        worst_tick,
        costliest: costliest.map(|(i, _)| i),
    })
}

fn enum_known(t: Ty, sizes: &Sizes) -> bool {
    match t {
        Ty::Enum(e) => (e as usize) < sizes.enum_variants.len(),
        _ => true,
    }
}

fn text_table(list: &[String], table: &'static str, max: usize) -> Result<(), Refusal> {
    if list.len() > u16::MAX as usize / 2 {
        return Err(Refusal::Limit {
            what: table,
            max: u16::MAX as usize / 2,
        });
    }
    for (index, s) in list.iter().enumerate() {
        if s.is_empty() && table != "the string table" || s.len() > max {
            return Err(Refusal::Text { table, index, max });
        }
    }
    Ok(())
}

/// The worst-case op count of `code[start..end]`: every op once, and a
/// `ForEach` body once per member of its set's bound.
pub fn cost(code: &[Op], start: usize, end: usize) -> u64 {
    let mut total = 0u64;
    let mut pc = start;
    while pc < end && pc < code.len() {
        total = total.saturating_add(1);
        if let Op::ForEach { set, body, .. } = code[pc] {
            let body_end = (pc + 1 + body as usize).min(end);
            let inner = cost(code, pc + 1, body_end);
            total = total.saturating_add(set.base.bound().saturating_mul(inner));
            pc = body_end;
        } else {
            pc += 1;
        }
    }
    total
}

struct Cx<'a> {
    ir: &'a Ir,
    hd: &'a Handler,
    sizes: &'a Sizes,
}

impl Cx<'_> {
    fn reg(&self, r: Reg) -> Result<Ty, Fault> {
        self.hd.regs.get(r as usize).copied().ok_or(Fault::Register(r))
    }

    fn want(&self, r: Reg, want: Ty) -> Result<(), Fault> {
        let got = self.reg(r)?;
        if got == want {
            Ok(())
        } else {
            Err(Fault::Type {
                reg: r,
                want: want.name(),
                got,
            })
        }
    }

    fn num(&self, r: Reg, ty: NumTy) -> Result<(), Fault> {
        self.want(r, ty.ty())
    }

    fn jump(&self, pc: usize, to: u16, region: usize) -> Result<(), Fault> {
        if (to as usize) <= pc {
            return Err(Fault::Backward(to));
        }
        if to as usize > region {
            return Err(Fault::JumpOut(to));
        }
        Ok(())
    }

    fn cap(&self, cap: u8) -> Result<&str, Fault> {
        self.ir
            .header
            .requires
            .get(cap as usize)
            .map(String::as_str)
            .ok_or(Fault::CapIndex(cap))
    }

    fn slot(&self, slot: u16, owner: Option<Reg>) -> Result<&Slot, Fault> {
        let s = self.ir.slots.get(slot as usize).ok_or(Fault::Slot(slot))?;
        match (s.scope.owner(), owner) {
            (None, None) => {}
            (Some(ty), Some(r)) => self.want(r, ty)?,
            _ => return Err(Fault::Owner(slot)),
        }
        Ok(s)
    }

    fn op(&self, pc: usize, op: &Op, region: usize, ends: &mut Vec<usize>) -> Result<(), Fault> {
        match *op {
            Op::Const { dst, value } => {
                if !self.reg(dst)?.valid(value, self.sizes) {
                    return Err(Fault::Const(value));
                }
            }
            Op::Move { dst, src } => {
                let t = self.reg(src)?;
                self.want(dst, t)?;
            }
            Op::Arith { op, ty, dst, a, b } => {
                if op == ArithOp::Rem && ty == NumTy::Fixed {
                    return Err(Fault::FixedRem);
                }
                self.num(dst, ty)?;
                self.num(a, ty)?;
                self.num(b, ty)?;
            }
            Op::Neg { ty, dst, a } => {
                self.num(dst, ty)?;
                self.num(a, ty)?;
            }
            Op::Cmp { op, dst, a, b } => {
                self.want(dst, Ty::Bool)?;
                let t = self.reg(a)?;
                self.want(b, t)?;
                let ordered = matches!(t, Ty::Int | Ty::Fixed);
                if !matches!(op, CmpOp::Eq | CmpOp::Ne) && !ordered {
                    return Err(Fault::Type {
                        reg: a,
                        want: "int or fixed",
                        got: t,
                    });
                }
            }
            Op::Not { dst, a } => {
                self.want(dst, Ty::Bool)?;
                self.want(a, Ty::Bool)?;
            }
            Op::Load { dst, slot, owner } => {
                let s = self.slot(slot, owner)?;
                self.want(dst, s.ty.register())?;
            }
            Op::Store { slot, owner, src } => {
                let s = self.slot(slot, owner)?;
                self.want(src, s.ty.register())?;
            }
            Op::Opt { dst, index } => {
                let o = self.ir.options.get(index as usize).ok_or(Fault::Option(index))?;
                self.want(dst, o.ty)?;
            }
            Op::Variant { dst, field } => {
                let &(_, ty) = VARIANT_FIELDS
                    .get(field as usize)
                    .ok_or(Fault::VariantField(field))?;
                self.want(dst, ty)?;
            }
            Op::TableRead {
                dst,
                table,
                row,
                col,
            } => {
                let t = self.ir.tables.get(table as usize).ok_or(Fault::Table(table))?;
                let c = t.columns.get(col as usize).ok_or(Fault::Column(col))?;
                self.want(row, Ty::Int)?;
                self.want(dst, c.ty)?;
            }
            Op::TableFind { dst, table, key } => {
                let t = self.ir.tables.get(table as usize).ok_or(Fault::Table(table))?;
                self.want(dst, Ty::Int)?;
                self.want(key, t.columns[0].ty)?;
            }
            Op::Jump { to } => self.jump(pc, to, region)?,
            Op::JumpIf { cond, to } | Op::JumpIfNot { cond, to } => {
                self.want(cond, Ty::Bool)?;
                self.jump(pc, to, region)?;
            }
            Op::ForEach { set, dst, body } => {
                if let SetBase::Labelled(l) = set.base
                    && l as usize >= self.ir.labels.len()
                {
                    return Err(Fault::Label(l));
                }
                if let Some(n) = set.pick {
                    self.want(n, Ty::Int)?;
                }
                self.want(dst, set.base.elem())?;
                let end = pc + 1 + body as usize;
                if end > region {
                    return Err(Fault::BodyOut(body));
                }
                ends.push(end);
            }
            Op::Call {
                entry,
                ref args,
                dst,
            } => {
                let e = caps::entry(entry, self.ir.header.api_version).ok_or(Fault::Entry(entry))?;
                if args.len() != e.args.len() {
                    return Err(Fault::Arity {
                        entry: caps::spelled(e),
                        want: e.args.len(),
                        got: args.len(),
                    });
                }
                for (&r, &ty) in args.iter().zip(e.args) {
                    self.want(r, ty)?;
                }
                let got = match dst {
                    Some(r) => Some(self.reg(r)?),
                    None => None,
                };
                // A read's result may be dropped; an action has none.
                let fits = match (e.result, got) {
                    (Some(w), Some(g)) => w == g,
                    (Some(_), None) => e.kind != Kind::Action,
                    (None, None) => true,
                    (None, Some(_)) => false,
                };
                if !fits {
                    return Err(Fault::Result {
                        entry: caps::spelled(e),
                        want: e.result,
                        got,
                    });
                }
                if let Some(cap) = e.cap
                    && !self.ir.header.requires.iter().any(|r| r == cap)
                {
                    return Err(Fault::Undeclared { entry: caps::spelled(e), cap });
                }
            }
            Op::Grant {
                player,
                cap,
                uses: _,
                palette,
            } => {
                self.want(player, Ty::Player)?;
                self.grantable(cap)?;
                if let Some(p) = palette
                    && p as usize >= self.ir.palettes.len()
                {
                    return Err(Fault::Palette(p));
                }
            }
            Op::Revoke { player, cap } => {
                self.want(player, Ty::Player)?;
                self.grantable(cap)?;
            }
            Op::Granted { dst, player, cap } => {
                self.want(dst, Ty::Bool)?;
                self.want(player, Ty::Player)?;
                self.grantable(cap)?;
            }
        }
        Ok(())
    }

    fn grantable(&self, cap: u8) -> Result<(), Fault> {
        let name = self.cap(cap)?;
        let known = caps::capability(name, self.ir.header.api_version);
        if !known.is_some_and(|c| c.grantable) {
            return Err(Fault::NotGrantable(name.to_string()));
        }
        Ok(())
    }
}

/// The first entry a blob calls that a host cannot make
/// ([`caps::Status::hostable`]). A host refuses such a script by that
/// entry's name.
pub fn unhostable(ir: &Ir) -> Option<&'static caps::Entry> {
    for hd in &ir.handlers {
        for op in &hd.code {
            if let Op::Call { entry, .. } = *op
                && let Some(e) = caps::entry(entry, ir.header.api_version)
                && !e.status.hostable()
            {
                return Some(e);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::tests::sample;

    #[test]
    fn the_sample_verifies() {
        let report = verify(&sample()).unwrap();
        // 6 ops, the empty ForEach body costs nothing.
        assert_eq!(report.handler_ops, vec![6]);
        assert_eq!(report.worst_tick, 6 * 16);
        assert!(report.fits());
    }

    #[test]
    fn header_refusals_name_what_failed() {
        let mut ir = sample();
        ir.header.requires.push("net.socket".into());
        assert_eq!(verify(&ir), Err(Refusal::UnknownCapability("net.socket".into())));

        let mut ir = sample();
        ir.header.requires.push("forge.move".into());
        assert_eq!(verify(&ir), Err(Refusal::DuplicateCapability("forge.move".into())));

        let mut ir = sample();
        ir.header.ir_version = 2;
        assert_eq!(verify(&ir), Err(Refusal::IrVersion(2)));

        let mut ir = sample();
        ir.header.api_version = 9;
        assert_eq!(verify(&ir), Err(Refusal::ApiVersion(9)));

        let mut ir = sample();
        ir.header.carrier = 5;
        assert_eq!(verify(&ir), Err(Refusal::Carrier(5)));
    }

    #[test]
    fn op_refusals_name_the_op() {
        fn refuse(f: impl Fn(&mut Ir)) -> Fault {
            let mut ir = sample();
            f(&mut ir);
            match verify(&ir) {
                Err(Refusal::Op { fault, .. }) => fault,
                other => panic!("expected an op refusal, got {other:?}"),
            }
        }
        // A backward jump.
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[2] = Op::JumpIfNot { cond: 3, to: 1 }),
            Fault::Backward(1)
        );
        // A jump past the end.
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[2] = Op::Jump { to: 99 }),
            Fault::JumpOut(99)
        );
        // A register that does not exist.
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[0] = Op::Const { dst: 9, value: 0 }),
            Fault::Register(9)
        );
        // A wrong type.
        assert!(matches!(
            refuse(|ir| ir.handlers[0].code[0] = Op::Const { dst: 0, value: 99 }),
            Fault::Const(99)
        ));
        // A slot that does not exist, and a scoped slot without its owner.
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[0] = Op::Load { dst: 3, slot: 7, owner: None }),
            Fault::Slot(7)
        );
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[0] = Op::Load { dst: 3, slot: 0, owner: None }),
            Fault::Owner(0)
        );
        // An unknown entry, and one whose capability is not declared.
        assert_eq!(
            refuse(|ir| ir.handlers[0].code[3] = Op::Call { entry: 999, args: vec![], dst: None }),
            Fault::Entry(999)
        );
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[3] = Op::Call {
                    entry: caps::id::SET_TEAM,
                    args: vec![0, 0],
                    dst: None,
                }
            }),
            Fault::Type {
                reg: 0,
                want: "team",
                got: Ty::Player
            }
        );
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[3] = Op::Call {
                    entry: caps::id::RESPAWN,
                    args: vec![0, 0],
                    dst: None,
                }
            }),
            Fault::Arity {
                entry: "respawn",
                want: 1,
                got: 2
            }
        );
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[3] = Op::Call {
                    entry: caps::id::DELETE,
                    args: vec![4],
                    dst: None,
                }
            }),
            Fault::Undeclared {
                entry: "delete",
                cap: "objects.delete"
            }
        );
        // An action's result written to a register.
        assert!(matches!(
            refuse(|ir| {
                ir.handlers[0].code[3] = Op::Call {
                    entry: caps::id::SCORE,
                    args: vec![0, 2],
                    dst: Some(2),
                }
            }),
            Fault::Result { .. }
        ));
        // A label, a palette and a ForEach body out of range.
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[4] = Op::ForEach {
                    set: Set {
                        base: SetBase::Labelled(5),
                        pick: None,
                    },
                    dst: 4,
                    body: 0,
                }
            }),
            Fault::Label(5)
        );
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[4] = Op::ForEach {
                    set: Set {
                        base: SetBase::Players,
                        pick: None,
                    },
                    dst: 0,
                    body: 9,
                }
            }),
            Fault::BodyOut(9)
        );
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[5] = Op::Grant {
                    player: 0,
                    cap: 0,
                    uses: 1,
                    palette: Some(3),
                }
            }),
            Fault::Palette(3)
        );
        // A table that does not exist.
        assert_eq!(
            refuse(|ir| {
                ir.handlers[0].code[0] = Op::TableRead {
                    dst: 2,
                    table: 4,
                    row: 2,
                    col: 0,
                }
            }),
            Fault::Table(4)
        );
    }

    #[test]
    fn a_jump_cannot_leave_a_loop_body() {
        let mut ir = sample();
        let hd = &mut ir.handlers[0];
        hd.code = vec![
            Op::ForEach {
                set: Set {
                    base: SetBase::Players,
                    pick: None,
                },
                dst: 0,
                body: 1,
            },
            Op::Jump { to: 3 },
            Op::Const { dst: 2, value: 0 },
        ];
        assert_eq!(
            verify(&ir),
            Err(Refusal::Op {
                handler: 0,
                pc: 1,
                fault: Fault::JumpOut(3)
            })
        );
        ir.handlers[0].code[1] = Op::Jump { to: 2 };
        let report = verify(&ir).unwrap();
        // The loop op, 16 bodies of one op, and the const.
        assert_eq!(report.handler_ops, vec![1 + 16 + 1]);
    }

    #[test]
    fn grants_need_a_declared_grantable_capability() {
        let mut ir = sample();
        ir.header.requires = vec!["hud.message".into()];
        assert_eq!(
            verify(&ir),
            Err(Refusal::Op {
                handler: 0,
                pc: 5,
                fault: Fault::NotGrantable("hud.message".into())
            })
        );
        ir.header.requires.clear();
        assert_eq!(
            verify(&ir),
            Err(Refusal::Op {
                handler: 0,
                pc: 5,
                fault: Fault::CapIndex(0)
            })
        );
    }

    #[test]
    fn table_option_and_slot_refusals() {
        let mut ir = sample();
        ir.tables[0].columns[0].ty = Ty::Bool;
        assert_eq!(verify(&ir), Err(Refusal::Cell { table: 0, cell: 0 }));

        let mut ir = sample();
        ir.options[0].default = 9;
        assert_eq!(verify(&ir), Err(Refusal::Option(0)));

        let mut ir = sample();
        ir.options[0].hi = i32::MAX;
        ir.options[0].lo = i32::MIN;
        ir.options[0].default = 0;
        assert!(verify(&ir).is_ok(), "32 bits fit");
        let mut wide = ir.options[0].clone();
        wide.lo = 0;
        wide.hi = 1;
        wide.default = 0;
        ir.options.push(wide);
        assert_eq!(verify(&ir), Err(Refusal::OptionBits(33)));

        let mut ir = sample();
        ir.slots[0].ty = Ty::Timer;
        assert_eq!(verify(&ir), Err(Refusal::Slot(0)));

        let mut ir = sample();
        ir.labels[0] = "x".repeat(33);
        assert_eq!(
            verify(&ir),
            Err(Refusal::Text {
                table: "the label table",
                index: 0,
                max: MAX_LABEL_BYTES
            })
        );
    }

    #[test]
    fn the_estimate_shares_zone_events_and_names_the_costliest() {
        let mut ir = sample();
        let mut enter = ir.handlers[0].clone();
        enter.event = Event::Enter;
        enter.regs[0] = Ty::Object;
        enter.regs[1] = Ty::Object;
        enter.code = vec![Op::Const { dst: 2, value: 1 }; 3];
        let mut leave = enter.clone();
        leave.event = Event::Leave;
        leave.code.truncate(1);
        ir.handlers.push(enter);
        ir.handlers.push(leave);
        let report = verify(&ir).unwrap();
        assert_eq!(report.handler_ops, vec![6, 3, 1]);
        assert_eq!(report.worst_tick, 6 * 16 + 3 * 64);
        assert_eq!(report.costliest, Some(1));
    }

    #[test]
    fn unhostable_names_the_first_untraced_entry() {
        let mut ir = sample();
        assert_eq!(unhostable(&ir), None);
        ir.header.requires.push("player.team".into());
        ir.handlers[0].regs.push(Ty::Team);
        ir.handlers[0].code[3] = Op::Call {
            entry: caps::id::SET_TEAM,
            args: vec![0, 5],
            dst: None,
        };
        verify(&ir).unwrap();
        assert_eq!(unhostable(&ir).map(|e| e.name), Some("set_team"));
    }
}
