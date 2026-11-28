//! Slice 3: object function values and change colours
//! (docs/engine/object-update.md, "Slice 3 bodies"; design
//! docs/engine/sim-rewrite-objects.md section 12).
//!
//! - [`value`]: `bab40` OBJECT_FUNCTION_VALUE, the export evaluator
//!   `ba8c0`, and the import dispatcher `108d90` (slot `+4c` of every
//!   ancestry level, an indirect callout).
//! - [`base`]: `bcc20` BASE_OBJECT_IMPORT, the `obje` level's handler.
//! - [`colors`]: `be8e0` OBJECT_CHANGE_COLORS.
//! - The type levels' import handlers: [`unit`] (`c6b90`, with `d1210` and
//!   `d1410`), [`biped`] (`de3b0`), [`vehicle`] (`f6d10`), [`weapon`]
//!   (`ff5f0`), [`projectile`] (`fbe70`), [`device`] (`106d70`) and
//!   [`machine`] (`11bd60`, machines and crates).
//!
//! Every body here is a segment: retail-site arms over the retail frame,
//! with all eight integer registers retail's at each call and return.
//! Arithmetic flags are left undefined at every transfer
//! ([`undefined_flags`]): no caller or callee of these functions reads
//! them (docs/engine/object-update.md, "Register liveness": every slice 3
//! entry and site reads no flag). The SSE registers and the x87 state are
//! kept as retail leaves them where a caller reads them (results in
//! `xmm0`), and through every operation the bodies make.
//!
//! Constants the public source must not carry (string IDs, the handle
//! hash's parameters) are read from the instruction bytes that hold them
//! in the loaded image, as the per-function ports do.

pub(super) mod base;
pub(super) mod biped;
pub(super) mod colors;
pub(super) mod device;
pub(super) mod machine;
pub(super) mod projectile;
pub(super) mod unit;
pub(super) mod value;
pub(super) mod vehicle;
pub(super) mod weapon;
/// The curve evaluator's capture arm, reached by `sim_bridge`.
pub mod parity;
/// The function value's shadow arm over the Rust evaluator
/// (`sim_objects::functions::evaluator`), reached by `sim_bridge`.
pub mod arm;
#[cfg(test)]
mod curve_tests;

use super::{Next, Result};
#[cfg(test)]
use guest_layout::datum::layout;
use crate::light_constants::{Error, X87};
use guest_layout::points;
use crate::sim_physics::ARITH;
use runtime::{CpuAbi, Guest, GuestFault};

pub(super) use crate::sim_physics::{byte, logic, pop, push};

/// The object array global (`[4e0300]`); its data at `+44`.
pub(super) const OBJECTS: u32 = points::OBJECTS.base;
/// The tag entries global (`[4e3b44]`); 16-byte entries, data at `+8`.
pub(super) const TAGS: u32 = points::TAG_ENTRIES.base;
/// The type table: 13 descriptor pointers.
pub(super) const TYPES: u32 = 0x0046_8630;
/// 1.0 and 0.0 (f32) in the image's constants.
pub(super) const ONE: u32 = crate::function_eval::constant::ONE;
pub(super) const ZERO: u32 = crate::function_eval::constant::ZERO;
/// 1.0f32 as an immediate.
pub(super) const ONE_BITS: u32 = 0x3f80_0000;
pub(super) const NONE: u32 = u32::MAX;

/// Leave the arithmetic flags undefined at a transfer (rule 3 of the
/// callout contract; no reader, see the module comment).
pub(super) fn undefined_flags(c: &mut CpuAbi) {
    c.eflags_defined &= !ARITH;
}

/// `call target` returning to `site`, flags undefined.
pub(super) fn call(c: &mut CpuAbi, target: u32, site: u32) -> Result<Next> {
    undefined_flags(c);
    Ok(Next::Call { target, site })
}

/// `ret pop`, flags undefined.
pub(super) fn ret(c: &mut CpuAbi, pop: u16) -> Result<Next> {
    undefined_flags(c);
    Ok(Next::Ret { pop })
}

/// The dword at `[esp + off]`.
pub(super) fn slot(c: &CpuAbi, g: &Guest, off: u32) -> Result<u32> {
    g.u32(c.esp.wrapping_add(off))
}

/// The low lane of `xmm[i]`.
pub(super) fn lo(c: &CpuAbi, i: usize) -> u32 {
    u128::from_le_bytes(c.xmm[i]) as u32
}

fn full(c: &CpuAbi, i: usize) -> u128 {
    u128::from_le_bytes(c.xmm[i])
}

fn set_full(c: &mut CpuAbi, i: usize, v: u128) {
    c.xmm[i] = v.to_le_bytes();
}

/// `movss xmm[i], m32`: the low lane, the rest zero.
pub(super) fn load(c: &mut CpuAbi, i: usize, v: u32) {
    set_full(c, i, u128::from(v));
}

/// `xorps xmm[i], xmm[i]`.
pub(super) fn zero(c: &mut CpuAbi, i: usize) {
    set_full(c, i, 0);
}

/// `movaps xmm[d], xmm[s]`.
pub(super) fn movaps(c: &mut CpuAbi, d: usize, s: usize) {
    let v = full(c, s);
    set_full(c, d, v);
}

/// `movss xmm[d], xmm[s]`: the low lane only.
#[cfg(test)]
pub(super) fn movss(c: &mut CpuAbi, d: usize, s: usize) {
    let v = (full(c, d) & !0xffff_ffff) | u128::from(lo(c, s));
    set_full(c, d, v);
}

fn simd_fault(_: simd::Xm) -> GuestFault {
    fp_error(Error::Simd)
}

/// An unmasked floating-point exception: [`super::supported`] rules it
/// out at entry, so this is a fault, never a result.
pub(super) fn fp_error(_: Error) -> GuestFault {
    GuestFault {
        addr: 0,
        len: 0,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::InvalidOperation,
    }
}

/// A scalar SSE operation on `xmm[d]`'s low lane, upper lanes kept.
#[derive(Clone, Copy)]
pub(super) enum Ss {
    Add,
    Sub,
    Mul,
    Div,
}

/// `op xmm[d], b` (`addss`, `subss`, `mulss`, `divss`).
pub(super) fn ss(c: &mut CpuAbi, op: Ss, d: usize, b: u32) -> Result<u32> {
    let a = full(c, d);
    let r = match op {
        Ss::Add => simd::addss(&mut c.mxcsr, a, b),
        Ss::Sub => simd::subss(&mut c.mxcsr, a, b),
        Ss::Mul => simd::mulss(&mut c.mxcsr, a, b),
        Ss::Div => simd::divss(&mut c.mxcsr, a, b),
    }
    .map_err(simd_fault)?;
    set_full(c, d, r);
    Ok(r as u32)
}

/// `cvtsi2ss xmm[d], i`.
pub(super) fn cvtsi2ss(c: &mut CpuAbi, d: usize, i: u32) -> Result<u32> {
    let old = full(c, d);
    let r = simd::cvtsi2ss(&mut c.mxcsr, old, i).map_err(simd_fault)?;
    set_full(c, d, r);
    Ok(r as u32)
}

/// `comiss a, b`: ZF, PF and CF as the compare leaves them, OF, SF and AF
/// clear. Returns those bits.
pub(super) fn comiss(c: &mut CpuAbi, a: u32, b: u32) -> Result<u32> {
    let f = simd::comiss(&mut c.mxcsr, a, b).map_err(simd_fault)?;
    c.eflags = (c.eflags & !ARITH) | f;
    c.eflags_defined |= ARITH;
    Ok(f)
}

/// `ucomiss a, b`, as [`comiss`].
pub(super) fn ucomiss(c: &mut CpuAbi, a: u32, b: u32) -> Result<u32> {
    let f = simd::ucomiss(&mut c.mxcsr, a, b).map_err(simd_fault)?;
    c.eflags = (c.eflags & !ARITH) | f;
    c.eflags_defined |= ARITH;
    Ok(f)
}

/// `comiss a, b; ja` (CF and ZF clear).
pub(super) fn above(c: &mut CpuAbi, a: u32, b: u32) -> Result<bool> {
    Ok(comiss(c, a, b)? & 0x41 == 0)
}

/// `comiss a, b; jbe` taken: the negation of [`above`].
#[cfg(test)]
pub(super) fn below_or_equal(c: &mut CpuAbi, a: u32, b: u32) -> Result<bool> {
    Ok(!above(c, a, b)?)
}

/// `ucomiss a, b; lahf; test ah, 44h; jp` taken: not (ordered and
/// equal). `lahf` writes `ah`.
pub(super) fn ucomiss_lahf_jp(c: &mut CpuAbi, a: u32, b: u32) -> Result<bool> {
    let f = ucomiss(c, a, b)?;
    // lahf: AH = SF ZF 0 AF 0 PF 1 CF.
    let ah = (c.eflags & 0xd5) as u8 | 2;
    c.eax = (c.eax & !0xff00) | (u32::from(ah) << 8);
    let t = ah & 0x44;
    logic(c, u32::from(t), 0x80);
    Ok(f & 0x44 != 0x40)
}

/// The x87 state of `c`, to run instructions on.
pub(super) fn fpu(c: &CpuAbi) -> X87 {
    let x = &c.x87;
    X87(x87::abi::X87Abi {
        st: x.st,
        control: x.control,
        status: x.status,
        tag: x.tag,
        opcode: x.opcode,
        instruction_pointer: x.instruction_pointer,
        data_pointer: x.data_pointer,
        instruction_selector: x.instruction_selector,
        data_selector: x.data_selector,
    })
}

/// Put the x87 state back after a sequence whose last x87 instruction is
/// at `pc` with memory operand `operand` (0 for none): the stack, status
/// and tags, and the last-instruction pointers as that instruction leaves
/// them (FOP from its mapped bytes).
pub(super) fn fpu_done(c: &mut CpuAbi, g: &Guest, x: X87, pc: u32, operand: u32) -> Result<()> {
    let [op, modrm] = g.read::<2>(pc)?;
    c.x87.st = x.0.st;
    c.x87.status = x.0.status;
    c.x87.tag = x.0.tag;
    c.x87.instruction_pointer = pc;
    c.x87.instruction_selector = 8;
    c.x87.opcode = (u16::from(op & 7) << 8) | u16::from(modrm);
    if operand != 0 {
        c.x87.data_pointer = operand;
        c.x87.data_selector = 16;
    }
    Ok(())
}

/// Fault on a pc a body does not own.
pub(super) fn unowned(pc: u32) -> GuestFault {
    super::unowned(pc)
}

/// `test r8, r8`: flags of a byte.
pub(super) fn test8(c: &mut CpuAbi, v: u32) {
    logic(c, v & 0xff, 0x80);
}

/// Read the imm32 an instruction carries at `addr` (a string ID or a
/// hash parameter, from the loaded image).
pub(super) fn imm(g: &Guest, addr: u32) -> Result<u32> {
    g.u32(addr)
}

#[cfg(test)]
mod tests;
