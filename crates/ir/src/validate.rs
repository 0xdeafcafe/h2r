//! Structural checks: temps defined once before use, operand widths agree,
//! block shape. Lowering bugs show up here long before a backend
//! miscompiles them, so the lifter's tests run this over every instruction
//! they produce.

use std::collections::HashMap;
use std::fmt;

use crate::*;

/// A broken invariant, with the guest PC it was found at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidateError {
    pub pc: u32,
    pub message: String,
}

impl fmt::Display for ValidateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010x}: {}", self.pc, self.message)
    }
}

impl std::error::Error for ValidateError {}

struct Checker {
    pc: u32,
    /// Temps in scope, with their width. A `Repeat` body gets a copy of the
    /// enclosing scope; its own temps don't leak out.
    defined: HashMap<u32, Width>,
    /// Every id ever defined in this instruction, at any depth.
    ever: std::collections::HashSet<u32>,
}

impl Checker {
    fn err<T>(&self, message: impl Into<String>) -> Result<T, ValidateError> {
        Err(ValidateError { pc: self.pc, message: message.into() })
    }

    fn use_value(&self, v: &Value) -> Result<Width, ValidateError> {
        match v {
            Value::Const { width, bits } => {
                if bits & !width.mask() != 0 {
                    return self.err(format!("constant {bits:#x} does not fit {width:?}"));
                }
                Ok(*width)
            }
            Value::Temp(t) => match self.defined.get(&t.id) {
                Some(w) if *w == t.width => Ok(t.width),
                Some(w) => self.err(format!("t{} used as {:?} but defined as {w:?}", t.id, t.width)),
                None => self.err(format!("t{} used before definition", t.id)),
            },
        }
    }

    fn expect(&self, v: &Value, w: Width, what: &str) -> Result<(), ValidateError> {
        let got = self.use_value(v)?;
        if got != w {
            return self.err(format!("{what}: expected {w:?}, got {got:?}"));
        }
        Ok(())
    }

    fn define(&mut self, t: &Temp) -> Result<(), ValidateError> {
        if !self.ever.insert(t.id) {
            return self.err(format!("t{} defined twice", t.id));
        }
        self.defined.insert(t.id, t.width);
        Ok(())
    }

    fn addr(&self, a: &Addr) -> Result<(), ValidateError> {
        self.expect(&a.offset, Width::W32, "address")
    }

    fn expr(&self, dst: &Temp, e: &Expr) -> Result<(), ValidateError> {
        match e {
            Expr::Unary { op, a } => {
                let w = self.use_value(a)?;
                let want = if *op == UnOp::Parity { Width::W1 } else { w };
                if dst.width != want {
                    return self.err(format!("{op:?} result is {want:?}, dst is {:?}", dst.width));
                }
                if *op == UnOp::Bswap && w.bits() % 8 != 0 {
                    return self.err("bswap of a non-byte width");
                }
            }
            Expr::Binary { op, a, b } => {
                let w = self.use_value(a)?;
                self.expect(b, w, &format!("{op:?} rhs"))?;
                if dst.width != w {
                    return self.err(format!("{op:?} result is {w:?}, dst is {:?}", dst.width));
                }
            }
            Expr::Compare { op, a, b } => {
                let w = self.use_value(a)?;
                self.expect(b, w, &format!("{op:?} rhs"))?;
                if dst.width != Width::W1 {
                    return self.err("comparison into a non-W1 temp");
                }
            }
            Expr::Cast { op, a } => {
                let w = self.use_value(a)?;
                let ok = match op {
                    CastOp::Zext | CastOp::Sext => dst.width >= w,
                    CastOp::Trunc => dst.width <= w,
                };
                if !ok {
                    return self.err(format!("{op:?} from {w:?} to {:?}", dst.width));
                }
            }
            Expr::Select { cond, a, b } => {
                self.expect(cond, Width::W1, "select condition")?;
                self.expect(a, dst.width, "select lhs")?;
                self.expect(b, dst.width, "select rhs")?;
            }
        }
        Ok(())
    }

    fn stmts(&mut self, stmts: &[Stmt]) -> Result<(), ValidateError> {
        for s in stmts {
            match s {
                Stmt::Assign { dst, expr } => {
                    self.expr(dst, expr)?;
                    self.define(dst)?;
                }
                Stmt::ReadReg { dst, reg, slice } => {
                    if dst.width != slice.width() {
                        return self.err(format!("read of {reg:?}.{slice:?} into {:?}", dst.width));
                    }
                    if *slice == Slice::High8 && reg.index() >= 4 {
                        return self.err(format!("{reg:?} has no high byte"));
                    }
                    self.define(dst)?;
                }
                Stmt::WriteReg { reg, slice, value } => {
                    self.expect(value, slice.width(), &format!("write to {reg:?}.{slice:?}"))?;
                    if *slice == Slice::High8 && reg.index() >= 4 {
                        return self.err(format!("{reg:?} has no high byte"));
                    }
                }
                Stmt::ReadFlag { dst, .. } => {
                    if dst.width != Width::W1 {
                        return self.err("flag read into a non-W1 temp");
                    }
                    self.define(dst)?;
                }
                Stmt::WriteFlag { value, .. } => {
                    if let FlagValue::Defined(v) = value {
                        self.expect(v, Width::W1, "flag value")?;
                    }
                }
                Stmt::ReadEflags { dst } => {
                    if dst.width != Width::W32 {
                        return self.err("eflags read into a non-W32 temp");
                    }
                    self.define(dst)?;
                }
                Stmt::WriteEflags { value, .. } => self.expect(value, Width::W32, "eflags value")?,
                Stmt::ReadXmm { dst, reg } => {
                    if dst.width != Width::W128 || *reg >= 8 {
                        return self.err(format!("bad read of xmm{reg} into {:?}", dst.width));
                    }
                    self.define(dst)?;
                }
                Stmt::WriteXmm { reg, value } => {
                    if *reg >= 8 {
                        return self.err(format!("xmm{reg} does not exist"));
                    }
                    self.expect(value, Width::W128, "xmm write")?;
                }
                Stmt::ReadMmx { dst, reg } => {
                    if dst.width != Width::W64 || *reg >= 8 {
                        return self.err(format!("bad read of mm{reg} into {:?}", dst.width));
                    }
                    self.define(dst)?;
                }
                Stmt::WriteMmx { reg, value } => {
                    if *reg >= 8 {
                        return self.err(format!("mm{reg} does not exist"));
                    }
                    self.expect(value, Width::W64, "mmx write")?;
                }
                Stmt::Load { dst, addr } => {
                    self.addr(addr)?;
                    if dst.width == Width::W1 {
                        return self.err("W1 load");
                    }
                    self.define(dst)?;
                }
                Stmt::Store { addr, value } => {
                    self.addr(addr)?;
                    if self.use_value(value)? == Width::W1 {
                        return self.err("W1 store");
                    }
                }
                Stmt::FaultIf { cond, .. } => self.expect(cond, Width::W1, "fault condition")?,
                Stmt::StopIf { cond, .. } => self.expect(cond, Width::W1, "stop condition")?,
                Stmt::Intrinsic { op, args, dsts } => {
                    let (aw, dw) = op.signature();
                    if args.len() != aw.len() || dsts.len() != dw.len() {
                        return self.err(format!("{op:?} takes {} args and {} results", aw.len(), dw.len()));
                    }
                    for (a, w) in args.iter().zip(aw) {
                        self.expect(a, *w, &format!("{op:?} argument"))?;
                    }
                    for (d, w) in dsts.iter().zip(dw) {
                        if d.width != *w {
                            return self.err(format!("{op:?} result is {w:?}, dst is {:?}", d.width));
                        }
                        self.define(d)?;
                    }
                }
                Stmt::If { cond, then, otherwise } => {
                    self.expect(cond, Width::W1, "if condition")?;
                    for body in [then, otherwise] {
                        let saved = self.defined.clone();
                        self.stmts(body)?;
                        self.defined = saved;
                    }
                }
                Stmt::Repeat { body, .. } => {
                    let saved = self.defined.clone();
                    self.stmts(body)?;
                    self.defined = saved;
                }
                Stmt::LazyFlags(l) => {
                    let fm = &l.formula;
                    if l.args.len() != fm.params.len() {
                        return self.err("lazy flags with the wrong number of arguments");
                    }
                    for (a, w) in l.args.iter().zip(&fm.params) {
                        self.expect(a, *w, "lazy flag argument")?;
                    }
                    formula(fm).map_err(|m| ValidateError { pc: self.pc, message: m })?;
                }
                Stmt::Unsupported { .. } => {}
            }
        }
        Ok(())
    }
}

/// A [`FlagFormula`] is self-contained: parameters, then body temps
/// numbered in order, each defined before use, `W1` flags each once.
fn formula(fm: &FlagFormula) -> Result<(), String> {
    let mut c = Checker { pc: 0, defined: HashMap::new(), ever: Default::default() };
    for (i, w) in fm.params.iter().enumerate() {
        c.define(&Temp { id: i as u32, width: *w }).map_err(|e| e.message)?;
    }
    for (k, (t, e)) in fm.body.iter().enumerate() {
        if t.id as usize != fm.params.len() + k {
            return Err("lazy flag formula temps out of order".into());
        }
        c.expr(t, e).map_err(|e| e.message)?;
        c.define(t).map_err(|e| e.message)?;
    }
    let mut seen = FlagSet::default();
    for (f, v) in &fm.flags {
        if seen.contains(*f) {
            return Err(format!("lazy flag formula writes {f:?} twice"));
        }
        seen.insert(*f);
        c.expect(v, Width::W1, "lazy flag value").map_err(|e| e.message)?;
    }
    Ok(())
}

/// At most one write per flag on any path. `written` is what the path has
/// written so far; returns the flags `stmts` may write.
fn flags_once(stmts: &[Stmt], written: FlagSet) -> Result<FlagSet, String> {
    let mut written = written;
    let mut mine = FlagSet::default();
    for s in stmts {
        let fx = match s {
            Stmt::WriteFlag { flag, .. } => {
                if written.contains(*flag) {
                    return Err(format!("{flag:?} written twice"));
                }
                let mut f = FlagSet::default();
                f.insert(*flag);
                f
            }
            Stmt::WriteEflags { mask, .. } => {
                let mut f = FlagSet::default();
                for flag in Flag::ALL {
                    if mask & (1 << flag.eflags_bit()) != 0 {
                        if written.contains(flag) {
                            return Err(format!("{flag:?} written twice"));
                        }
                        f.insert(flag);
                    }
                }
                f
            }
            Stmt::LazyFlags(l) => {
                let mut f = FlagSet::default();
                for (flag, _) in &l.formula.flags {
                    if written.contains(*flag) {
                        return Err(format!("{flag:?} written twice"));
                    }
                    f.insert(*flag);
                }
                f
            }
            // Each iteration of a Repeat body is its own path segment, but
            // it starts after whatever came before the loop.
            Stmt::Repeat { body, .. } => flags_once(body, written)?,
            Stmt::If { then, otherwise, .. } => {
                let a = flags_once(then, written)?;
                let b = flags_once(otherwise, written)?;
                FlagSet(a.0 | b.0)
            }
            _ => continue,
        };
        for flag in Flag::ALL {
            if fx.contains(flag) {
                written.insert(flag);
                mine.insert(flag);
            }
        }
    }
    Ok(mine)
}

/// The atomic shape (crate docs): a pure prefix, then one load and one
/// store of the same address at the top level, load first, no state
/// written before the store; the descriptor's operand computed by the
/// prefix at the memory width, its `old` register present exactly when the
/// operation returns the old value; control `Next`.
fn atomic_shape(insn: &IrInsn, a: &Atomic) -> Result<(), String> {
    let stmts = &insn.stmts;
    if insn.control != Control::Next {
        return Err("atomic instruction that transfers control".into());
    }
    let mut load: Option<(usize, Addr, Width)> = None;
    let mut store = None;
    let mut prefix = std::collections::HashSet::new();
    for (i, s) in stmts.iter().enumerate() {
        if load.is_none() {
            match s {
                Stmt::Assign { dst, .. } | Stmt::ReadReg { dst, .. } => {
                    prefix.insert(dst.id);
                }
                Stmt::Load { .. } => {}
                _ => return Err("atomic prefix is not pure (Assign/ReadReg only)".into()),
            }
        }
        match s {
            Stmt::Load { addr, dst } => {
                if load.replace((i, *addr, dst.width)).is_some() {
                    return Err("atomic instruction with two loads".into());
                }
            }
            Stmt::Store { addr, .. } => {
                if store.replace((i, *addr)).is_some() {
                    return Err("atomic instruction with two stores".into());
                }
            }
            Stmt::WriteReg { .. } | Stmt::WriteFlag { .. } | Stmt::WriteEflags { .. } | Stmt::WriteXmm { .. }
            | Stmt::WriteMmx { .. } | Stmt::LazyFlags(_)
                if store.is_none() =>
            {
                return Err("atomic instruction writes state before its store".into());
            }
            Stmt::Repeat { .. } | Stmt::If { .. } | Stmt::Intrinsic { .. } => {
                return Err("atomic instruction with nested or intrinsic statements".into());
            }
            _ => {}
        }
    }
    let width = match (load, store) {
        (Some((l, la, w)), Some((s, sa))) if l < s && la == sa => w,
        (Some(_), Some(_)) => return Err("atomic load and store differ or are out of order".into()),
        _ => return Err("atomic instruction without one load and one store".into()),
    };
    if !matches!(width, Width::W8 | Width::W16 | Width::W32) {
        return Err(format!("atomic access of {width:?}"));
    }
    if matches!(a.op, AtomicOp::Bts | AtomicOp::Btr | AtomicOp::Btc) && width == Width::W8 {
        return Err("atomic bit operation on a byte".into());
    }
    match a.operand {
        None if a.op.has_operand() => return Err(format!("atomic {:?} without an operand", a.op)),
        Some(_) if !a.op.has_operand() => return Err(format!("atomic {:?} with an operand", a.op)),
        Some(v) if v.width() != width => return Err("atomic operand width differs from memory".into()),
        Some(Value::Temp(t)) if !prefix.contains(&t.id) => {
            return Err("atomic operand not computed before the load".into());
        }
        _ => {}
    }
    match a.old {
        Some((_, s)) if !a.op.returns_old() || s.width() != width => Err("atomic old-value register wrong".into()),
        None if a.op.returns_old() => Err(format!("atomic {:?} without an old-value register", a.op)),
        _ => Ok(()),
    }
}

impl IrInsn {
    /// Check this instruction's internal consistency.
    pub fn validate(&self) -> Result<(), ValidateError> {
        let mut c = Checker { pc: self.pc, defined: HashMap::new(), ever: Default::default() };
        c.stmts(&self.stmts)?;
        let fail = |message: String| Err(ValidateError { pc: self.pc, message });
        if let Err(m) = flags_once(&self.stmts, FlagSet::default()) {
            return fail(m);
        }
        if let Some(a) = &self.atomic
            && let Err(m) = atomic_shape(self, a)
        {
            return fail(m);
        }
        if self.unsupported().is_some() != (self.control == Control::Stop(StopReason::Unsupported)) {
            return fail("unsupported statement and Stop(Unsupported) must come together".into());
        }
        let target = |c: &Checker, t: &Target| match t {
            Target::Direct(_) => Ok(()),
            Target::Dynamic(v) => c.expect(v, Width::W32, "transfer target"),
        };
        match &self.control {
            Control::Next | Control::Fault(_) | Control::Stop(_) => {}
            Control::Jump { target: t, .. } | Control::Call { target: t, .. } => target(&c, t)?,
            Control::Branch { cond, .. } => c.expect(cond, Width::W1, "branch condition")?,
            Control::Switch { target, .. } | Control::Return { target, .. } => {
                c.expect(target, Width::W32, "transfer target")?
            }
        }
        Ok(())
    }
}

impl IrBlock {
    /// Check every instruction, and that only the last one transfers.
    pub fn validate(&self) -> Result<(), ValidateError> {
        let mut expect_pc = self.start;
        for (i, insn) in self.insns.iter().enumerate() {
            insn.validate()?;
            if insn.pc != expect_pc {
                return Err(ValidateError {
                    pc: insn.pc,
                    message: format!("expected an instruction at {expect_pc:#x}"),
                });
            }
            let mid_ok = matches!(insn.control, Control::Next | Control::Stop(StopReason::Unsupported));
            if i + 1 < self.insns.len() && !mid_ok {
                return Err(ValidateError { pc: insn.pc, message: "transfer before the end of a block".into() });
            }
            expect_pc = insn.next_pc();
        }
        Ok(())
    }
}

impl IrFunction {
    /// Check every block, that blocks are keyed by their start, that every
    /// local successor exists, and that the entry and interior entries
    /// start blocks.
    pub fn validate(&self) -> Result<(), ValidateError> {
        let fail = |pc: u32, message: String| Err(ValidateError { pc, message });
        if !self.blocks.contains_key(&self.entry) {
            return fail(self.entry, "no block at the entry".into());
        }
        for &e in &self.interior_entries {
            if !self.blocks.contains_key(&e) {
                return fail(e, "interior entry does not start a block".into());
            }
        }
        for (&start, b) in &self.blocks {
            if start != b.start {
                return fail(start, format!("block keyed {start:#x} starts at {:#x}", b.start));
            }
            if b.insns.is_empty() {
                return fail(start, "empty block".into());
            }
            b.validate()?;
            let last = b.insns.last().unwrap();
            let local = |pc: u32| -> Result<(), ValidateError> {
                if self.blocks.contains_key(&pc) {
                    Ok(())
                } else {
                    fail(last.pc, format!("successor {pc:#x} is not a block"))
                }
            };
            let dest = |d: &Dest| if d.tail { Ok(()) } else { local(d.pc) };
            match &last.control {
                Control::Next => local(last.next_pc())?,
                Control::Jump { target: Target::Direct(t), tail: false, .. } => local(*t)?,
                Control::Jump { target: Target::Dynamic(_), tail: false, .. } => {
                    return fail(last.pc, "a computed jump must be a tail transfer".into());
                }
                Control::Branch { taken, fallthrough, .. } => {
                    dest(taken)?;
                    dest(fallthrough)?;
                }
                Control::Switch { cases, .. } => cases.iter().try_for_each(dest)?,
                _ => {}
            }
        }
        Ok(())
    }
}
