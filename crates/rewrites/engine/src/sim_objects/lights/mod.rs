//! Lights, slice 5 of docs/engine/sim-rewrite-objects.md (contracts:
//! docs/engine/object-update.md, "Slice 5 bodies"): the tick's light walk
//! `c1400`, the light record update `c0c80` and its phase step `c0b00`,
//! and the light-list operations `c2d00` (link), `c3220` (unlink) and
//! `c3260` (expire).
//!
//! Lights live in the datum array whose header `[4e030c]` points at
//! (`110`-byte elements at header `+44`, the element size at `+24`, the
//! high-water mark at `+38`, the live bitmap at `+48`; an element's salt
//! is its first word). Element fields named by these bodies (L for the
//! names, C for the accesses):
//!
//! | Offset | Field |
//! |---|---|
//! | `+02` | flags (`u16`): 2 active, 8 linked, `10` timed; 1 and 4 choose paths in `c2d00` |
//! | `+04` | definition tag datum |
//! | `+10` | the list record `1cae40` and `1cac60` keep |
//! | `+14` | creation tick |
//! | `+18`, `+24` | position, radius |
//! | `+38`, `+44` | the cluster location `11bed0` and `ba300` fill |
//! | `+48` | a word `ffff` when the light has no BSP location of its own |
//! | `+4c`, `+58` | owner object; marker parent object |
//! | `+54`, `+56` | function name index (`ffff` none); its source |
//! | `+5c` | the parent's node index |
//! | `+60`, `+6c`, `+78` | local offset and two local directions |
//! | `+84`, `+90`, `+ac`, `+b8` | world origin, position, and directions |
//! | `+c4`..`+f0` | the outputs `c0c80` writes |
//!
//! Every body leaves the arithmetic flags undefined at its calls and
//! returns: the liveness scan (object-update.md, "Register liveness")
//! shows no reader at any slice 5 entry or site, nor at any call of these
//! entries from `bef30`.

pub(super) mod list;
pub(super) mod phase;
pub(super) mod record;
pub(super) mod walk;

use super::Result;
use crate::sim_physics::ARITH;
use runtime::{CpuAbi, Guest};
use x87::abi as fp;

/// The light datum array's header.
pub(super) const LIGHTS: u32 = 0x004e_030c;
/// The light-list owner `1cae40` and `1cac60` take (in `edi`, or pushed).
pub(super) const LIGHT_LISTS: u32 = 0x004e_0310;
/// Element bytes.
pub(super) const STRIDE: u32 = 0x110;
/// Datum array header fields.
pub(super) const ELEMENT_SIZE: u32 = 0x24;
pub(super) const HIGH_WATER: u32 = 0x38;
pub(super) const DATA: u32 = 0x44;
pub(super) const BITMAP: u32 = 0x48;

/// `[esp + off]` and `a + b`, wrapping.
pub(super) fn at(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

/// No reader of EFLAGS's arithmetic bits at any slice 5 transfer.
pub(super) fn flags_undefined(c: &mut CpuAbi) {
    c.eflags_defined &= !ARITH;
}

/// The guest's x87 state as the `x87` crate's.
pub(super) fn fpu(c: &CpuAbi) -> fp::X87Abi {
    fp::X87Abi {
        st: c.x87.st,
        control: c.x87.control,
        status: c.x87.status,
        tag: c.x87.tag,
        opcode: c.x87.opcode,
        instruction_pointer: c.x87.instruction_pointer,
        data_pointer: c.x87.data_pointer,
        instruction_selector: c.x87.instruction_selector,
        data_selector: c.x87.data_selector,
    }
}

/// Back into the guest's x87 state, with the last instruction at `pc` and
/// its memory operand at `data`.
pub(super) fn fpu_done(c: &mut CpuAbi, g: &Guest, x: &fp::X87Abi, pc: u32, data: u32) -> Result<()> {
    c.x87.st = x.st;
    c.x87.status = x.status;
    c.x87.tag = x.tag;
    c.x87.instruction_pointer = pc;
    c.x87.opcode = g.u16(pc)?.swap_bytes() & 0x7ff;
    c.x87.data_pointer = data;
    c.x87.instruction_selector = 8;
    c.x87.data_selector = 16;
    Ok(())
}

#[cfg(test)]
mod tests;
