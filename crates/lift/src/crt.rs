//! Lifter-side replacements for CRT floating-point helpers: a whole
//! function whose IR is written here instead of lowered from its code.
//!
//! Only `_ftol2` for now (docs/lifter/FLOAT-ABI.md §4.5): 214 call sites, each of
//! which would otherwise run `fistp m64` at round-to-nearest, reload, and
//! correct toward zero through a dozen x87 and integer instructions. The
//! replacement is one [`X87Op::Ftol2`] call.
//!
//! Contract of the replacement, against the retail routine:
//!
//! - **Same:** EDX:EAX = `ST(0)` truncated toward zero whatever RC is, as
//!   an int64, with `0x8000_0000_0000_0000` for NaN, infinities and
//!   |x| >= 2^63; one value popped; ESP + 4 and a plain `ret`; EBX, ESI,
//!   EDI, EBP and DF preserved.
//! - **Different, all dead by the calling convention:** ECX is preserved
//!   (the routine clobbers it on some paths); the status flags are
//!   undefined (the routine leaves whatever its last `test`/`adc`/`sbb`
//!   produced); nothing is written below ESP (the routine spills to an
//!   aligned frame there); x87 sticky flags may differ (PE from the
//!   correction arithmetic); FIP/FOP/FDP are not updated. An unmasked
//!   invalid-operation exception would be delivered at the caller's next
//!   waiting x87 instruction rather than inside the routine. The game masks
//!   every exception (FLOAT.md §1.2).
//!
//! The image check is the title ID and entry point, so another build of
//! the game never gets a replacement at an address that means something
//! else.

use ir::x87::X87Op;
use ir::{Addr, Control, Flag, Gpr, Intrinsic, IrBlock, IrFunction, IrInsn, Segment, Slice, Stmt, Value, Width};
use xbe::Xbe;

use crate::builder::Builder;

/// `_ftol2` in the retail XBE.
pub const FTOL2: u32 = 0x0032_2b0c;
/// Retail Halo 2: title ID and entry point.
const RETAIL_TITLE: u32 = 0x4d53_0064;
const RETAIL_ENTRY: u32 = 0x002d_0aee;

/// Which replacements apply to an image.
#[derive(Debug, Clone, Default)]
pub struct Replacements {
    /// `_ftol2`'s entry, when the image is the retail game.
    pub ftol2: Option<u32>,
}

impl Replacements {
    /// The replacements for `xbe`: none unless it is the retail image.
    pub fn of(xbe: &Xbe) -> Self {
        let retail = xbe.certificate.title_id == RETAIL_TITLE && xbe.entry == RETAIL_ENTRY;
        Replacements { ftol2: retail.then_some(FTOL2) }
    }

    /// The replacement function at `entry`, if any.
    pub fn function(&self, entry: u32) -> Option<IrFunction> {
        (self.ftol2 == Some(entry)).then(|| ftol2(entry))
    }
}

/// The `_ftol2` replacement as a one-instruction function at `entry`.
pub fn ftol2(entry: u32) -> IrFunction {
    let mut b = Builder::new();
    // The return address first: a faulting read changes nothing.
    let esp = b.read_reg(Gpr::Esp, Slice::Full);
    let ret = b.load(Addr { seg: Segment::Flat, offset: esp }, Width::W32);
    let out = b.temp(Width::W64);
    b.push(Stmt::Intrinsic { op: Intrinsic::X87(X87Op::Ftol2), args: vec![], dsts: vec![out] });
    let out = Value::from(out);
    let lo = b.trunc(out, Width::W32);
    let hi = b.bin(ir::BinOp::Shr, out, Value::constant(Width::W64, 32));
    let hi = b.trunc(hi, Width::W32);
    b.write_reg(Gpr::Eax, Slice::Full, lo);
    b.write_reg(Gpr::Edx, Slice::Full, hi);
    let new = b.add(esp, Value::u32(4));
    b.write_reg(Gpr::Esp, Slice::Full, new);
    for f in Flag::STATUS {
        b.undef_flag(f);
    }
    let insn = IrInsn {
        pc: entry,
        len: 1,
        atomic: None,
        stmts: b.finish(),
        control: Control::Return { target: ret, cleanup: 0 },
    };
    IrFunction {
        entry,
        blocks: [(entry, IrBlock { start: entry, insns: vec![insn] })].into(),
        interior_entries: vec![],
        errors: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ftol2_is_a_valid_one_instruction_return() {
        let f = ftol2(FTOL2);
        f.validate().unwrap_or_else(|e| panic!("{e}\n{f}"));
        let insn = f.insns().next().unwrap();
        assert!(matches!(insn.control, Control::Return { cleanup: 0, .. }));
        // The load comes before the helper, the helper before any write.
        let pos = |p: &dyn Fn(&Stmt) -> bool| insn.stmts.iter().position(p).unwrap();
        let load = pos(&|s| matches!(s, Stmt::Load { .. }));
        let call = pos(&|s| matches!(s, Stmt::Intrinsic { op: Intrinsic::X87(X87Op::Ftol2), .. }));
        let write = pos(&|s| matches!(s, Stmt::WriteReg { .. }));
        assert!(load < call && call < write);
    }
}
