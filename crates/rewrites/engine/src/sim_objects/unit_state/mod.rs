//! The unit update's state children (slice 6b of
//! docs/engine/sim-rewrite-objects.md; contracts in
//! docs/engine/object-update.md, "Slice 6b bodies"): the control update
//! `c49b0` ([`control`]), the state update `c58f0` ([`state`]), the
//! powered seats `c5eb0` and the control refresh `ce0c0` it calls
//! ([`seats`]). `c6b30` (slice 6a) calls the first three; with these
//! bodies its first, second and fourth calls stay in Rust.
//!
//! Every body is a segment over the retail frame, the retail code block by
//! block, keyed by retail address: at each call and return the eight
//! integer registers, the `xmm` registers, MXCSR and the x87 state are
//! retail's, and every store retail makes is made, stack stores included.
//! Arithmetic flags are left undefined at every transfer: no caller and no
//! callee of the four reads them before writing them
//! (`scripts/sim-liveness.py`, docs/engine/object-update.md, "Slice 6b
//! bodies"). The x87 sequences (`c49b0`'s `fld; fistp`, `ce0c0`'s three
//! `fld; fabs; fstp`) run through `x87::abi` on the guest's x87 state,
//! with the last-instruction pointers retail's last x87 instruction
//! leaves.
//!
//! String IDs and float immediates are read from the instruction bytes
//! that carry them in the loaded image ([`imm`]), never written here.

pub(super) mod control;
pub(super) mod seats;
pub(super) mod state;

use super::functions::{fp_error, fpu, fpu_done};
use super::{Result, unowned};
use crate::light_constants::X87;
use runtime::{CpuAbi, Guest};

pub(super) use super::functions::{call, ret};
pub(super) use crate::sim_physics::{byte, pop, push};

pub(super) const NONE: u32 = u32::MAX;
/// 1.0 (f32) in the image's constants.
pub(super) const ONE: u32 = 0x0045_dbc0;
/// The image's small epsilon, also slice 6a's wake threshold.
pub(super) const SMALL: u32 = 0x0045_dbdc;
/// Pointers to constant vectors (L: zero, and a second one).
pub(super) const VECTOR_A4: u32 = 0x0046_87a4;
pub(super) const VECTOR_A8: u32 = 0x0046_87a8;

/// The guest functions the four bodies call. `c85c0` (slice 6a) and
/// `ce0c0` are bodies; the rest are callouts.
pub(super) mod callee {
    // `c49b0`.
    /// `(unit: eax)`: the control input (L).
    pub const D0D20: u32 = 0xd0d20;
    /// `(unit: eax, out: esi)`: the facing a parent seat gives (L).
    pub const D0080: u32 = 0xd0080;
    /// `(object: eax) -> eax`: an animation or seat name (L).
    pub const F10F5F0: u32 = 0x10f5f0;
    /// `(object: ecx) -> al`.
    pub const C7070: u32 = 0xc7070;
    /// `(object: eax, stack 0) -> eax`; `ret 4`.
    pub const F10F720: u32 = 0x10f720;
    /// `(unit: eax) -> al`.
    pub const F1143D0: u32 = 0x1143d0;
    /// `(unit: eax) -> al`.
    pub const F42F0: u32 = 0xf42f0;
    /// `(unit: eax) -> al`.
    pub const CC380: u32 = 0xcc380;
    // `c58f0`.
    /// `(n: ecx, unit: stack 0)`; `ret 4`: set one unit state.
    pub const STATE_SET: u32 = 0xe68c0;
    pub const F113DA0: u32 = 0x113da0;
    /// `(unit: eax, value: stack 0)`; `ret 4`.
    pub const F113D20: u32 = 0x113d20;
    pub const F113DF0: u32 = 0x113df0;
    /// `(unit: eax, value: stack 0)`; `ret 4`.
    pub const F113D60: u32 = 0x113d60;
    /// `(unit: stack 0) -> al`; `ret 4`.
    pub const F110AB0: u32 = 0x110ab0;
    /// `(unit: eax, index: cx, ebx) -> ax`.
    pub const CBEC0: u32 = 0xcbec0;
    /// `(name: eax, unit: esi, record: edi)`.
    pub const F189210: u32 = 0x189210;
    /// `(weapon: stack 0, flags: stack 1, value: stack 2)`; `ret c`.
    pub const F100430: u32 = 0x100430;
    /// `(unit: stack 0)`; `ret 4`.
    pub const D1540: u32 = 0xd1540;
    // `c5eb0`.
    /// `(unit: ebx)`.
    pub const D0C10: u32 = 0xd0c10;
    // `ce0c0`.
    /// `(unit: eax) -> al`.
    pub const E4050: u32 = 0xe4050;
    /// `(unit: eax, out: stack 0, out: stack 1) -> al`; `ret 8`.
    pub const D1080: u32 = 0xd1080;
    /// `() -> al`.
    pub const F138880: u32 = 0x138880;
}

/// Whether `pc` is `entry` or one of `sites`.
pub(super) fn owned(pc: u32, entry: u32, sites: &[u32]) -> Result<()> {
    if pc == entry || sites.contains(&pc) { Ok(()) } else { Err(unowned(pc)) }
}

/// `movsx r32, word`.
pub(super) fn sx16(v: u16) -> u32 {
    v as i16 as i32 as u32
}

/// `movsx r32, byte`.
pub(super) fn sx8(v: u8) -> u32 {
    v as i8 as i32 as u32
}

/// The imm32 an instruction carries at `addr` in the loaded image (a
/// string ID or a float immediate).
pub(super) fn imm(g: &Guest, addr: u32) -> Result<u32> {
    g.u32(addr)
}

/// Run `f` on the guest's x87 state, then put it back as the instruction
/// at `pc` with memory operand `operand` leaves it (the last x87
/// instruction of the sequence).
pub(super) fn x87(
    c: &mut CpuAbi,
    g: &Guest,
    pc: u32,
    operand: u32,
    f: impl FnOnce(&mut X87) -> std::result::Result<(), crate::light_constants::Error>,
) -> Result<()> {
    let mut x = fpu(c);
    f(&mut x).map_err(fp_error)?;
    fpu_done(c, g, x, pc, operand)
}

/// `fistp dword` under the current rounding control: the integer, the
/// stack popped.
pub(super) fn fistp(x: &mut X87) -> std::result::Result<i32, crate::light_constants::Error> {
    let (code, store) = x87::abi::st_i32(&x.0, 0);
    X87::ok(code)?;
    x87::abi::store_commit(&mut x.0, store.status, true);
    Ok(store.value)
}

/// `fabs`.
pub(super) fn fabs(x: &mut X87) -> std::result::Result<(), crate::light_constants::Error> {
    x.unary(x87::abi::unary_op::ABS)
}

#[cfg(test)]
mod tests;
