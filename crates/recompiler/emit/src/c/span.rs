//! Prove a common base and constant byte offsets for straight-line RAM
//! sites. Only the address calculation is moved: guest loads, stores and
//! access charges stay at their original statements.
use ir::{Addr, BinOp, Control, Expr, Gpr, IrInsn, Segment, Slice, Stmt, Value, Width};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone, Copy)]
struct Address {
    reg: Option<Gpr>,
    offset: u32,
}

fn address(v: Value, temps: &BTreeMap<u32, Address>) -> Option<Address> {
    match v {
        Value::Const {
            width: Width::W32,
            bits,
        } => Some(Address {
            reg: None,
            offset: bits as u32,
        }),
        Value::Temp(t) => temps.get(&t.id).copied(),
        _ => None,
    }
}

struct Site {
    addr: Address,
    bytes: u32,
    rights: u32,
}

fn sites(insn: &IrInsn) -> Option<(Vec<Site>, u32)> {
    if insn.atomic.is_some() || insn.control != Control::Next {
        return None;
    }
    let mut temps = BTreeMap::new();
    let mut changed = 0u32;
    let mut sites = Vec::new();
    for stmt in &insn.stmts {
        let (addr, bytes, rights) = match stmt {
            Stmt::ReadReg {
                dst,
                reg,
                slice: Slice::Full,
            } => {
                temps.insert(
                    dst.id,
                    Address {
                        reg: Some(*reg),
                        offset: 0,
                    },
                );
                continue;
            }
            Stmt::Assign {
                dst,
                expr: Expr::Binary { op, a, b },
            } if dst.width == Width::W32 => {
                if let (Some(a), Some(b)) = (address(*a, &temps), address(*b, &temps)) {
                    let result = match op {
                        BinOp::Add if a.reg.is_none() || b.reg.is_none() => Some(Address {
                            reg: a.reg.or(b.reg),
                            offset: a.offset.wrapping_add(b.offset),
                        }),
                        BinOp::Sub if b.reg.is_none() => Some(Address {
                            reg: a.reg,
                            offset: a.offset.wrapping_sub(b.offset),
                        }),
                        _ => None,
                    };
                    if let Some(r) = result {
                        temps.insert(dst.id, r);
                    }
                }
                continue;
            }
            Stmt::WriteReg { reg, .. } => {
                changed |= 1 << reg.index();
                continue;
            }
            Stmt::Load { addr, dst } => (addr, dst.width.bytes(), 1),
            Stmt::Store { addr, value } => (addr, value.width().bytes(), 2),
            Stmt::Assign { .. }
            | Stmt::ReadReg { .. }
            | Stmt::ReadFlag { .. }
            | Stmt::WriteFlag { .. }
            | Stmt::ReadEflags { .. }
            | Stmt::WriteEflags { .. }
            | Stmt::LazyFlags(_)
            | Stmt::ReadXmm { .. }
            | Stmt::WriteXmm { .. } => continue,
            _ => return None, // Includes every helper, conditional, REP and alignment fault.
        };
        let Addr {
            seg: Segment::Flat,
            offset,
        } = addr
        else {
            return None;
        };
        if !matches!(bytes, 1 | 2 | 4 | 8 | 10 | 16) {
            return None;
        }
        sites.push(Site {
            addr: address(*offset, &temps)?,
            bytes,
            rights,
        });
    }
    // Includes writes after the access: a following instruction may still
    // use the same certificate.
    if let Some(first) = sites.first() {
        let reg = first.addr.reg;
        if reg.is_some_and(|r| changed & (1 << r.index()) != 0)
            || sites.iter().any(|s| s.addr.reg != reg)
        {
            return None;
        }
    }
    Some((sites, changed))
}

pub(super) struct Span {
    pub reg: Option<Gpr>,
    pub start: u32,
    pub len: u32,
    pub rights: u32,
    pub offsets: VecDeque<u32>,
}

pub(super) fn group(insns: &[IrInsn], selectors: &BTreeSet<u32>) -> Option<Span> {
    let (mut run, _) = sites(insns.first()?)?;
    let reg = run.first()?.addr.reg;
    let bounds = |s: &Site| {
        let lo = i64::from(s.addr.offset as i32);
        (lo, lo + i64::from(s.bytes))
    };
    let mut lo = run.iter().map(|s| bounds(s).0).min()?;
    let mut hi = run.iter().map(|s| bounds(s).1).max()?;
    for insn in &insns[1..] {
        if selectors.contains(&insn.pc) {
            break;
        }
        let Some((next, changed)) = sites(insn) else {
            break;
        };
        if reg.is_some_and(|r| changed & (1 << r.index()) != 0) {
            break;
        }
        let Some(first) = next.first() else { continue };
        if first.addr.reg != reg {
            break;
        }
        let l = next.iter().map(|s| bounds(s).0).min()?.min(lo);
        let h = next.iter().map(|s| bounds(s).1).max()?.max(hi);
        if h - l > 4096 {
            break;
        }
        lo = l;
        hi = h;
        run.extend(next);
    }
    if run.len() < 2 || hi - lo > 4096 {
        return None;
    }
    Some(Span {
        reg,
        start: lo as u32,
        len: (hi - lo) as u32,
        rights: run.iter().fold(0, |r, s| r | s.rights),
        offsets: run.iter().map(|s| (bounds(s).0 - lo) as u32).collect(),
    })
}
