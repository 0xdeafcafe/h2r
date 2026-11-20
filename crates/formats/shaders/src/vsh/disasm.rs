//! Text form of vertex microcode, close to the D3D assembler's syntax.
//!
//! Constants print with hardware numbering (`c[58]` is D3D's `c-38`). An
//! instruction that writes several places lists them in braces, and a paired
//! ILU op follows after ` : `, e.g.
//! `dp4 {r0.x, oPos.x}, v0, c[96] : rcc r1.x, r12.w`.

use super::*;
use std::fmt::Write;

/// Disassembles a whole program, one instruction per line, prefixed with
/// its slot number.
pub fn disassemble(program: &Program) -> String {
    let mut out = String::new();
    for (i, inst) in program.instructions.iter().enumerate() {
        let _ = writeln!(out, "{i:03}: {}", inst);
    }
    out
}

pub(crate) fn source_text(s: &Source) -> String {
    let mut t = String::new();
    if s.neg {
        t.push('-');
    }
    match s.reg {
        SrcReg::Temp(r) => {
            let _ = write!(t, "r{r}");
        }
        SrcReg::Input(v) => {
            let _ = write!(t, "v{v}");
        }
        SrcReg::Const {
            index,
            relative: false,
        } => {
            let _ = write!(t, "c[{index}]");
        }
        SrcReg::Const {
            index,
            relative: true,
        } => {
            let _ = write!(t, "c[a0.x+{index}]");
        }
        SrcReg::Unused => t.push('?'),
    }
    let names = b"xyzw";
    let sw = s.swizzle;
    if sw != [0, 1, 2, 3] {
        t.push('.');
        if sw.iter().all(|&c| c == sw[0]) {
            t.push(names[sw[0] as usize] as char);
        } else {
            for c in sw {
                t.push(names[c as usize] as char);
            }
        }
    }
    t
}

fn scalar_text(s: &Source) -> String {
    let mut s = *s;
    s.swizzle = [s.swizzle[0]; 4];
    source_text(&s)
}

fn dests(list: &[String]) -> String {
    match list.len() {
        0 => "_".into(),
        1 => list[0].clone(),
        _ => format!("{{{}}}", list.join(", ")),
    }
}

fn mac_name(op: MacOp) -> &'static str {
    match op {
        MacOp::Nop => "nop",
        MacOp::Mov => "mov",
        MacOp::Mul => "mul",
        MacOp::Add => "add",
        MacOp::Mad => "mad",
        MacOp::Dp3 => "dp3",
        MacOp::Dph => "dph",
        MacOp::Dp4 => "dp4",
        MacOp::Dst => "dst",
        MacOp::Min => "min",
        MacOp::Max => "max",
        MacOp::Slt => "slt",
        MacOp::Sge => "sge",
        MacOp::Arl => "mov",
    }
}

fn ilu_name(op: IluOp) -> &'static str {
    match op {
        IluOp::Nop => "nop",
        IluOp::Mov => "mov",
        IluOp::Rcp => "rcp",
        IluOp::Rcc => "rcc",
        IluOp::Rsq => "rsq",
        IluOp::Exp => "expp",
        IluOp::Log => "logp",
        IluOp::Lit => "lit",
    }
}

fn masked(name: String, m: Mask) -> String {
    if m == Mask::XYZW {
        name
    } else {
        format!("{name}.{m}")
    }
}

fn out_text(o: &OutputWrite) -> String {
    match o.dest {
        OutDest::Output(r) => masked(r.name(), o.mask),
        OutDest::Const(c) => masked(format!("c[{c}]"), o.mask),
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if self.mac != MacOp::Nop {
            let mut d = Vec::new();
            if self.mac == MacOp::Arl {
                d.push("a0.x".to_string());
            } else if self.mac_writes_temp() {
                d.push(masked(format!("r{}", self.mac_temp), self.mac_mask));
            }
            if let Some(o) = self.out.filter(|o| o.unit == Unit::Mac) {
                d.push(out_text(&o));
            }
            let (ra, rb, rc) = self.mac_reads();
            let mut ops = Vec::new();
            if ra {
                ops.push(source_text(&self.a));
            }
            if rb {
                ops.push(source_text(&self.b));
            }
            if rc {
                ops.push(source_text(&self.c));
            }
            parts.push(format!(
                "{} {}, {}",
                mac_name(self.mac),
                dests(&d),
                ops.join(", ")
            ));
        }
        if self.ilu != IluOp::Nop {
            let mut d = Vec::new();
            if self.ilu_writes_temp() {
                d.push(masked(format!("r{}", self.ilu_temp), self.ilu_mask));
            }
            if let Some(o) = self.out.filter(|o| o.unit == Unit::Ilu) {
                d.push(out_text(&o));
            }
            let c = if self.ilu.is_scalar() {
                scalar_text(&self.c)
            } else {
                source_text(&self.c)
            };
            parts.push(format!("{} {}, {}", ilu_name(self.ilu), dests(&d), c));
        }
        if parts.is_empty() {
            parts.push("nop".into());
        }
        write!(f, "{}", parts.join(" : "))
    }
}
