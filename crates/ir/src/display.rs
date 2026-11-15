//! A readable text form for dumps and test failures. It is not a stable
//! serialisation and nothing parses it.
//!
//! ```text
//! 0x002d0aee +3:
//!   t0:32 = reg eax
//!   t1:32 = add t0, 0x5:32
//!   eax = t1
//!   cf = ult t1, t0
//!   -> next
//! ```

use std::fmt::{self, Display, Formatter};

use crate::*;

impl Display for Width {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.bits())
    }
}

impl Display for Temp {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "t{}", self.id)
    }
}

impl Display for Value {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Value::Temp(t) => write!(f, "{t}"),
            Value::Const { width, bits } => write!(f, "{bits:#x}:{width}"),
        }
    }
}

impl Display for Gpr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(["eax", "ecx", "edx", "ebx", "esp", "ebp", "esi", "edi"][self.index()])
    }
}

fn reg(f: &mut Formatter<'_>, r: Gpr, s: Slice) -> fmt::Result {
    match s {
        Slice::Full => write!(f, "{r}"),
        Slice::Low16 => write!(f, "{r}.lo16"),
        Slice::Low8 => write!(f, "{r}.lo8"),
        Slice::High8 => write!(f, "{r}.hi8"),
    }
}

impl Display for Flag {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let s = match self {
            Flag::Cf => "cf",
            Flag::Pf => "pf",
            Flag::Af => "af",
            Flag::Zf => "zf",
            Flag::Sf => "sf",
            Flag::Df => "df",
            Flag::Of => "of",
        };
        f.write_str(s)
    }
}

impl Display for Addr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.seg {
            Segment::Flat => write!(f, "[{}]", self.offset),
            Segment::Fs => write!(f, "fs:[{}]", self.offset),
        }
    }
}

fn lower(name: impl fmt::Debug) -> String {
    format!("{name:?}").to_ascii_lowercase()
}

impl Display for Expr {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Expr::Unary { op, a } => write!(f, "{} {a}", lower(op)),
            Expr::Binary { op, a, b } => write!(f, "{} {a}, {b}", lower(op)),
            Expr::Compare { op, a, b } => write!(f, "{} {a}, {b}", lower(op)),
            Expr::Cast { op, a } => write!(f, "{} {a}", lower(op)),
            Expr::Select { cond, a, b } => write!(f, "select {cond}, {a}, {b}"),
        }
    }
}

fn stmts(f: &mut Formatter<'_>, list: &[Stmt], depth: usize) -> fmt::Result {
    let pad = "  ".repeat(depth);
    for s in list {
        write!(f, "{pad}")?;
        match s {
            Stmt::Assign { dst, expr } => writeln!(f, "{dst}:{} = {expr}", dst.width)?,
            Stmt::ReadReg { dst, reg: r, slice } => {
                write!(f, "{dst}:{} = reg ", dst.width)?;
                reg(f, *r, *slice)?;
                writeln!(f)?;
            }
            Stmt::WriteReg { reg: r, slice, value } => {
                reg(f, *r, *slice)?;
                writeln!(f, " = {value}")?;
            }
            Stmt::ReadFlag { dst, flag } => writeln!(f, "{dst}:1 = flag {flag}")?,
            Stmt::WriteFlag { flag, value: FlagValue::Defined(v) } => writeln!(f, "{flag} = {v}")?,
            Stmt::WriteFlag { flag, value: FlagValue::Undefined } => writeln!(f, "{flag} = undefined")?,
            Stmt::ReadEflags { dst } => writeln!(f, "{dst}:32 = eflags")?,
            Stmt::WriteEflags { value, mask } => writeln!(f, "eflags[{mask:#x}] = {value}")?,
            Stmt::ReadXmm { dst, reg } => writeln!(f, "{dst}:128 = xmm{reg}")?,
            Stmt::WriteXmm { reg, value } => writeln!(f, "xmm{reg} = {value}")?,
            Stmt::ReadMmx { dst, reg } => writeln!(f, "{dst}:64 = mm{reg}")?,
            Stmt::WriteMmx { reg, value } => writeln!(f, "mm{reg} = {value}")?,
            Stmt::Load { dst, addr } => writeln!(f, "{dst}:{} = load{} {addr}", dst.width, dst.width)?,
            Stmt::Store { addr, value } => writeln!(f, "store{} {addr}, {value}", value.width())?,
            Stmt::FaultIf { cond, fault } => writeln!(f, "fault {fault:?} if {cond}")?,
            Stmt::StopIf { cond, reason } => writeln!(f, "stop {reason:?} if {cond}")?,
            Stmt::If { cond, then, otherwise } => {
                writeln!(f, "if {cond} {{")?;
                stmts(f, then, depth + 1)?;
                if otherwise.is_empty() {
                    writeln!(f, "{pad}}}")?;
                } else {
                    writeln!(f, "{pad}}} else {{")?;
                    stmts(f, otherwise, depth + 1)?;
                    writeln!(f, "{pad}}}")?;
                }
            }
            Stmt::Intrinsic { op, args, dsts } => {
                let d: Vec<String> = dsts.iter().map(|t| format!("{t}:{}", t.width)).collect();
                let a: Vec<String> = args.iter().map(|v| v.to_string()).collect();
                let name = match op {
                    crate::Intrinsic::X87(x) => format!("x87 {x:?}"),
                    op => op.name().to_owned(),
                };
                writeln!(f, "({}) = intrinsic {name}({})", d.join(", "), a.join(", "))?
            }
            Stmt::Repeat { kind, body } => {
                writeln!(f, "{} {{", lower(kind))?;
                stmts(f, body, depth + 1)?;
                writeln!(f, "{pad}}}")?;
            }
            Stmt::Unsupported { mnemonic, detail } => writeln!(f, "unsupported {mnemonic}: {detail}")?,
        }
    }
    Ok(())
}

impl Display for Dest {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}{:#x}", if self.tail { "tail " } else { "" }, self.pc)
    }
}

impl Display for Target {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Target::Direct(a) => write!(f, "{a:#x}"),
            Target::Dynamic(v) => write!(f, "{v}"),
        }
    }
}

impl Display for Control {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Control::Next => write!(f, "next"),
            Control::Jump { target, tail, kind } => {
                write!(f, "{} {target}", if *tail { "tail" } else { "jump" })?;
                if let CallKind::Kernel { slot, ordinal } = kind {
                    write!(f, " kernel slot {slot:#x} ordinal {ordinal}")?;
                }
                Ok(())
            }
            Control::Branch { cond, taken, fallthrough } => write!(f, "branch {cond} ? {taken} : {fallthrough}"),
            Control::Switch { target, table, shape, immutable, cases } => {
                let ro = if *immutable { " read-only" } else { "" };
                write!(f, "switch {target} table {table:#x}{ro} {shape:?} ({} cases)", cases.len())
            }
            Control::Call { target, continuation, returns, kind } => {
                write!(f, "call {target} -> {continuation:#x} returns={returns} {kind:?}")
            }
            Control::Return { target, cleanup } => write!(f, "return {target} cleanup {cleanup}"),
            Control::Fault(x) => write!(f, "fault {x:?}"),
            Control::Stop(r) => write!(f, "stop {r:?}"),
        }
    }
}

impl Display for IrInsn {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match &self.atomic {
            Some(a) => writeln!(f, "{:#010x} +{} atomic {:?}:", self.pc, self.len, a.op)?,
            None => writeln!(f, "{:#010x} +{}:", self.pc, self.len)?,
        }
        stmts(f, &self.stmts, 1)?;
        writeln!(f, "  -> {}", self.control)
    }
}

impl Display for IrFunction {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "function {:#010x}", self.entry)?;
        for b in self.blocks.values() {
            writeln!(f, "block {:#010x}", b.start)?;
            for i in &b.insns {
                write!(f, "{i}")?;
            }
        }
        Ok(())
    }
}
