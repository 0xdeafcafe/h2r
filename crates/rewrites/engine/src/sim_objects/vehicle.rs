//! The vehicle update (slice 10 of docs/engine/sim-rewrite-objects.md;
//! contracts in docs/engine/object-update.md, "Slice 10 body"): the
//! `vehi` descriptor's slot `+40` (`efde0`), and two leaves it calls,
//! the gate `f1320` and the contact reset `f06c0`.
//!
//! `efde0` is a segment body. Its callees stay guest (the physics by
//! vehicle type through a jump table, the contact solve, the component
//! updates, the spatial and damage calls), except the leaves here and the
//! wake for post `bba20`, which [`super::drive`] fuses.
//!
//! **`efde0 (object: stack 0) -> al`; `ret 4`.** A `1984`-byte frame made
//! through the stack probe `320560` (`eax` = size), then `ebx`, `ebp`,
//! `esi`, `edi` pushed. With `S` that `esp`: `[S+12]` the changed byte,
//! `[S+13]` a scratch byte, `[S+14]` a float (the component's rate, then
//! the drag factor), `[S+18]` the definition, `[S+1c]` a float, `[S+20]`
//! a vector (`ba1d0`'s out, then the velocity change, then two words for
//! `e6830`'s caller), `[S+38]`, `[S+3c]`, `[S+44]` floats, `[S+40]` the
//! header offset `(object & ffff) * c`, `[S+48]` a matrix (`ba160`),
//! `[S+7c]` and the word `[S+f8]` `d6660`'s record, `[S+104]` the contact
//! buffer (`f06c0`, the type physics, `f6bd0`; its records' floats read
//! at `[S+238]`, stride `a8`), the object at `[S+1998]`. Across the calls
//! `ebp` holds the object (the matg record from `f037c` to the reload at
//! `f0568` or `f05c0`), `edi` the body, `bl` the gate's answer and later
//! the wake decision, `esi` a component record, the definition, the
//! child handle or none.
//!
//! In order: the gate `f1320` (fused); with body `+c0` bit 12 and a
//! component, `1d24a0(component, rate)` and `1d2460`; the tick into body
//! `+bc` unless `+c1` bit 0; `efc30` when the gate answered. Unless the
//! type is 6 or the gate answered or body `+c0` bit 6 is clear: a parented
//! vehicle `b9b90`; otherwise, unless definition `+2ac` bit 0, `ef4f0`,
//! `ef870`, and either the ground path (`f06c0`, `ba1d0`, `205be0`, the
//! type's physics through the table at `f069c`, `f6010`, `f6670`,
//! `f6bd0`, the mean of the contact records' floats into body `+3fc`, and
//! for types 3 and 5 the drag and pull toward the structure's point,
//! `cc380`, `372d48`, `ba1d0`, `b77d0`) or the early-mover path (`efb60`,
//! `205510`, `ba160`, `1cfb90`, `f6670`, `ba3d0`). Then outside network
//! mode 4 with definition `+1ec` bit 6, `ba1d0` and, below a height, each
//! biped child's `d6660` and `d7b80`; body `+134` bit 23 from `+148`
//! bit 0 and the timer `+2c4`; the x87 test of `|+354|` against
//! definition `+214`, which on a change toggles body `+348` bits 0 and 1
//! and calls `ba7f0`; `113e40` and the wake for post; `e6830`,
//! `111650`; four nones into body `+3bc..+3cc`; `b9ef0`. `al` is the OR
//! of `e6830`'s, `111650`'s and the changed byte.
//!
//! Instruction immediates the public source must not carry (two words
//! compared or stored at `f0619` and `f0640`) are read from the loaded
//! image, as are the `.rdata` constants.

use super::alu;
use super::frame::{set_slot, slot};
use super::functions::{fp_error, fpu, fpu_done};
use super::sse::{self, Ss};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use crate::light_constants::{Error, X87};
use guest_layout::points;
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push, status};
use runtime::{CpuAbi, Guest};
use x87::abi::unary_op;

const NONE: u32 = u32::MAX;
/// `efde0`'s locals.
const FRAME: u32 = 0x1984;
/// The object argument, from `S`.
const ARG: u32 = 0x1998;
/// The stack probe the update makes its frame with.
const STACK_PROBE: u32 = 0x0032_0560;
/// Holds the active structure (`+1c`, `+20` heights, `+1c0` a radius,
/// `+1c4` a point).
const ACTIVE_STRUCTURE: u32 = 0x004e_0348;
/// The jump table over definition `+1f0` (seven entries).
const JUMP_TABLE: u32 = 0x000f_069c;
/// The `imm32` of `cmp dword [eax+7c], imm` at `f0619`.
const ANIMATION_IMM: u32 = 0x000f_061c;
/// The `imm32` of `mov eax, imm` at `f0640`.
const PAIR_IMM: u32 = 0x000f_0641;
/// `.rdata` constants: f32 at `45dbc0`, `45dbd8`, `45dbfc`; f64 at `45e610`.
const K_45DBC0: u32 = 0x0045_dbc0;
const K_45DBD8: u32 = 0x0045_dbd8;
const K_45DBFC: u32 = 0x0045_dbfc;
const K_45E610: u32 = 0x0045_e610;

/// Callees (every one a guest callout, except the leaves and `bba20`).
const COMPONENT_RATE: u32 = 0x001d_24a0;
const COMPONENT_COMMIT: u32 = 0x001d_2460;
const GATED: u32 = 0x000e_fc30;
const REATTACH: u32 = 0x000b_9b90;
const EF4F0: u32 = 0x000e_f4f0;
const EF870: u32 = 0x000e_f870;
const EFB60: u32 = 0x000e_fb60;
const X205510: u32 = 0x0020_5510;
const NODE_MATRIX: u32 = 0x000b_a160;
const X1CFB90: u32 = 0x001c_fb90;
const F6670: u32 = 0x000f_6670;
const BA3D0: u32 = 0x000b_a3d0;
const BA1D0: u32 = 0x000b_a1d0;
const CONTACTS: u32 = 0x0020_5be0;
const F2360: u32 = 0x000f_2360;
const F3010: u32 = 0x000f_3010;
const F4800: u32 = 0x000f_4800;
const F6010: u32 = 0x000f_6010;
const F6BD0: u32 = 0x000f_6bd0;
const CC380: u32 = 0x000c_c380;
const POW: u32 = 0x0037_2d48;
const B77D0: u32 = 0x000b_77d0;
const D6660: u32 = 0x000d_6660;
const D7B80: u32 = 0x000d_7b80;
const BA7F0: u32 = 0x000b_a7f0;
const X113E40: u32 = 0x0011_3e40;
const E6830: u32 = points::UNIT_ACTIONS_UPDATE.base;
const X111650: u32 = 0x0011_1650;
const B9EF0: u32 = 0x000b_9ef0;

/// Every return address of `efde0`'s calls.
pub(super) const VEHICLE_SITES: [u32; 39] = [
    0xefdea, 0xefe32, 0xefebe, 0xefec5, 0xeff07, 0xeff45, 0xeff76, 0xeff7d, 0xeffa0, 0xeffe9, 0xefffd,
    0xf000a, 0xf0012, 0xf0019, 0xf002a, 0xf003c, 0xf004a, 0xf008e, 0xf009e, 0xf00ae, 0xf00c3, 0xf00d3,
    0xf00e9, 0xf00f0, 0xf00f8, 0xf0107, 0xf0188, 0xf01f2, 0xf0203, 0xf0354, 0xf0399, 0xf0400, 0xf040f,
    0xf057a, 0xf05ed, 0xf0632, 0xf0653, 0xf0660, 0xf0687,
];

fn at(base: u32, off: u32) -> u32 {
    base.wrapping_add(off)
}

/// `call target` returning to `site`, the flags as the last instruction
/// before it left them.
fn call(target: u32, site: u32) -> Result<Next> {
    Ok(Next::Call { target, site })
}

/// `and r32, k` / `test r32, r32`.
fn and32(c: &mut CpuAbi, v: u32) -> u32 {
    logic(c, v, 1 << 31);
    v
}

/// `and r8, k` / `test r8, r8` / `or r8, r8`: the flags of a byte.
fn flags8(c: &mut CpuAbi, v: u8) -> u8 {
    logic(c, v.into(), 0x80);
    v
}

/// `cmp r8, r8` / `cmp byte ptr [m], imm`.
fn cmp8(c: &mut CpuAbi, a: u8, b: u8) {
    let v = a.wrapping_sub(b);
    let (a, b, v) = (u32::from(a), u32::from(b), u32::from(v));
    let cf = u32::from(a < b);
    let of = u32::from((a ^ b) & (a ^ v) & 0x80 != 0) << 11;
    c.eflags = (c.eflags & !ARITH) | status(v, 0x80) | cf | ((a ^ b ^ v) & 0x10) | of;
    c.eflags_defined |= ARITH;
}

/// `comiss a, b; jbe` taken (CF or ZF; unordered too).
fn below_or_equal(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    sse::comiss(c, a, b) & 0x41 != 0
}

/// `ucomiss a, b; lahf; test ah, 44h; jnp` taken: ordered and equal.
/// `lahf` writes `ah`.
fn equal_jnp(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    sse::ucomiss(c, a, b);
    sse::lahf(c);
    logic(c, (c.eax >> 8) & 0x44, 0x80);
    !sse::parity(c)
}

/// One x87 instruction at `pc` (memory operand `mem`, 0 for none) on the
/// guest's x87 state, written back as that instruction leaves it.
fn fpu_op<T>(
    c: &mut CpuAbi,
    g: &Guest,
    pc: u32,
    mem: u32,
    f: impl FnOnce(&mut X87) -> std::result::Result<T, Error>,
) -> Result<T> {
    let mut x = fpu(c);
    let r = f(&mut x).map_err(fp_error)?;
    fpu_done(c, g, x, pc, mem)?;
    Ok(r)
}

/// `fld dword [at]`.
fn fld(c: &mut CpuAbi, g: &Guest, pc: u32, at: u32) -> Result<()> {
    let v = g.u32(at)?;
    fpu_op(c, g, pc, at, |x| x.fld(v))
}

/// `fld qword [at]`.
fn fld64(c: &mut CpuAbi, g: &Guest, pc: u32, at: u32) -> Result<()> {
    let v = u64::from(g.u32(at)?) | (u64::from(g.u32(at.wrapping_add(4))?) << 32);
    fpu_op(c, g, pc, at, |x| X87::ok(x87::abi::ld_f64(&mut x.0, v)))
}

/// `fstp dword [at]`.
fn fstp(c: &mut CpuAbi, g: &mut Guest, pc: u32, at: u32) -> Result<()> {
    let v = fpu_op(c, g, pc, at, |x| x.fstp_m32())?;
    g.set_u32(at, v)
}

/// `fabs`, `fsqrt`.
fn unary(c: &mut CpuAbi, g: &Guest, pc: u32, op: u32) -> Result<()> {
    fpu_op(c, g, pc, 0, |x| x.unary(op))
}

/// `fxch st(1)`.
fn fxch(c: &mut CpuAbi, g: &Guest, pc: u32) -> Result<()> {
    fpu_op(c, g, pc, 0, |x| x.fxch(1))
}

/// `fcomip st, st(1)`: ZF, PF and CF from the compare, OF, SF and AF
/// clear. Returns those bits.
fn fcomip(c: &mut CpuAbi, g: &Guest, pc: u32) -> Result<u32> {
    let f = fpu_op(c, g, pc, 0, |x| x.fcomip(1))?;
    c.eflags = (c.eflags & !ARITH) | f;
    c.eflags_defined |= ARITH;
    Ok(f)
}

/// `fstp st(0)`.
fn fpop(c: &mut CpuAbi, g: &Guest, pc: u32) -> Result<()> {
    fpu_op(c, g, pc, 0, |x| x.fstp_st(0))
}

/// `efde0 (object: stack 0) -> al`; `ret 4`.
pub(super) fn vehicle_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::VEHICLE_UPDATE.base;
    if pc != entry && !VEHICLE_SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let mut pc = pc;
    loop {
        pc = match pc {
            p if p == entry => {
                c.eax = FRAME;
                return call(STACK_PROBE, 0xefdea);
            }
            0xefdea => {
                c.ecx = g.u32(points::OBJECTS.base)?;
                c.edx = g.u32(at(c.ecx, DATA))?;
                c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                push(c, g, c.ebx)?;
                push(c, g, c.ebp)?;
                c.ebp = slot(c, g, 0x1990)?;
                c.eax = and32(c, c.ebp & 0xffff);
                c.eax = c.eax.wrapping_mul(3);
                c.eax = alu::shl(c, c.eax, 2, 32);
                push(c, g, c.esi)?;
                set_slot(c, g, 0x3c, c.eax)?;
                push(c, g, c.edi)?;
                c.edi = g.u32(at(at(c.edx, c.eax), 8))?;
                c.eax = and32(c, g.u32(c.edi)? & 0xffff);
                c.eax = alu::shl(c, c.eax, 4, 32);
                c.edx = g.u32(at(at(c.eax, c.ecx), 8))?;
                c.eax = c.ebp;
                set_slot(c, g, 0x18, c.edx)?;
                return call(points::VEHICLE_UPDATE_GATE.base, 0xefe32);
            }
            0xefe32 => {
                c.ebx = byte(c.ebx, c.eax as u8);
                c.eax = and32(c, 0);
                c.eax = u32::from(g.u16(at(c.edi, 0xc0))?);
                g.set_u8(at(c.esp, 0x12), 0)?;
                c.ecx = alu::shr(c, c.eax, 12, 32);
                let cl = flags8(c, c.ecx as u8 & 1);
                c.ecx = byte(c.ecx, cl);
                g.set_u8(at(c.esp, 0x13), cl)?;
                if cl == 0 {
                    0xefed9
                } else {
                    c.ecx = g.u32(at(c.edi, 0xb4))?;
                    arithmetic(c, c.ecx, NONE, true);
                    if c.ecx == NONE { 0xefed9 } else { 0xefe5f }
                }
            }
            0xefe5f => {
                c.edx = g.u32(points::PHYSICS_COMPONENTS.base)?;
                sse::load(c, 1, g.u32(K_45DBC0)?);
                c.ecx = and32(c, c.ecx & 0xffff);
                c.esi = c.ecx.wrapping_mul(5);
                c.ecx = g.u32(at(c.edx, 0x44))?;
                c.esi = alu::shl(c, c.esi, 5, 32);
                c.esi = arithmetic(c, c.esi, c.ecx, false);
                c.ecx = g.u32(at(c.edi, 0x148))?;
                if flags8(c, (c.ecx >> 8) as u8 & 0x20) == 0 {
                    sse::zero(c, 1);
                }
                sse::load(c, 0, g.u32(K_45DBC0)?);
                c.eax = alu::shr(c, c.eax, 10, 32);
                let al = flags8(c, c.eax as u8 & 1);
                c.eax = byte(c.eax, al);
                g.set_u8(at(c.esp, 0x13), al)?;
                if al == 0 {
                    sse::zero(c, 0);
                }
                let (x1, x0) = (sse::lane0(c, 1), sse::lane0(c, 0));
                let not_above = below_or_equal(c, x1, x0);
                g.set_u32(at(c.esp, 0x14), x1)?;
                if not_above {
                    g.set_u32(at(c.esp, 0x14), x0)?;
                }
                c.eax = slot(c, g, 0x14)?;
                push(c, g, c.eax)?;
                push(c, g, c.esi)?;
                return call(COMPONENT_RATE, 0xefebe);
            }
            0xefebe => {
                c.ecx = c.esi;
                return call(COMPONENT_COMMIT, 0xefec5);
            }
            0xefec5 => {
                sse::load(c, 0, slot(c, g, 0x14)?);
                let x0 = sse::lane0(c, 0);
                if !below_or_equal(c, x0, g.u32(K_45DBD8)?) {
                    g.set_u8(at(c.esp, 0x12), 1)?;
                }
                0xefed9
            }
            0xefed9 => {
                let cl = flags8(c, g.u8(at(c.edi, 0xc1))? & 1);
                c.ecx = byte(c.ecx, cl);
                g.set_u8(at(c.esp, 0x13), cl)?;
                if cl == 0 {
                    c.edx = g.u32(points::GAME_TIME.base)?;
                    c.eax = g.u32(at(c.edx, 8))?;
                    g.set_u32(at(c.edi, 0xbc), c.eax)?;
                    g.set_u8(at(c.esp, 0x12), 1)?;
                }
                if flags8(c, c.ebx as u8) != 0 {
                    c.eax = c.ebp;
                    return call(GATED, 0xeff07);
                }
                0xeff07
            }
            0xeff07 => {
                c.eax = slot(c, g, 0x18)?;
                let kind = g.u16(at(c.eax, 0x1f0))?;
                alu::cmp16(c, kind.into(), 6);
                if kind == 6 || flags8(c, c.ebx as u8) != 0 {
                    0xf05c7
                } else {
                    let v = alu::shr(c, g.u8(at(c.edi, 0xc0))?.into(), 6, 8) as u8;
                    let cl = flags8(c, v & 1);
                    c.ecx = byte(c.ecx, cl);
                    g.set_u8(at(c.esp, 0x13), cl)?;
                    if cl == 0 {
                        0xf05c7
                    } else {
                        let parent = g.u32(at(c.edi, 0x14))?;
                        arithmetic(c, parent, NONE, true);
                        if parent == NONE {
                            0xeff4a
                        } else {
                            push(c, g, c.ebp)?;
                            c.edx = byte(c.edx, 1);
                            return call(REATTACH, 0xeff45);
                        }
                    }
                }
            }
            0xeff45 => 0xf05c7,
            0xeff4a => {
                c.eax = slot(c, g, 0x18)?;
                let cl = g.u8(at(c.eax, 0x2ac))?;
                c.ecx = byte(c.ecx, cl);
                c.eax = arithmetic(c, c.eax, 0x2ac, false);
                if flags8(c, cl & 1) != 0 {
                    0xf05c7
                } else {
                    sse::zero(c, 0);
                    c.edx = at(c.esp, 0x14);
                    push(c, g, c.edx)?;
                    push(c, g, c.ebp)?;
                    g.set_u32(at(c.esp, 0x1c), sse::lane0(c, 0))?;
                    return call(EF4F0, 0xeff76);
                }
            }
            0xeff76 => {
                c.eax = c.ebp;
                return call(EF870, 0xeff7d);
            }
            0xeff7d => {
                let dl = flags8(c, g.u8(at(c.esp, 0x12))? | c.eax as u8);
                c.edx = byte(c.edx, dl);
                let al = flags8(c, g.u8(at(c.edi, 0xc1))? & 1);
                c.eax = byte(c.eax, al);
                g.set_u8(at(c.esp, 0x12), dl)?;
                g.set_u8(at(c.esp, 0x13), al)?;
                if al == 0 {
                    0xf001e
                } else {
                    c.eax = c.ebp;
                    return call(EFB60, 0xeffa0);
                }
            }
            0xeffa0 => {
                if flags8(c, c.eax as u8) != 0 {
                    0xf001e
                } else if flags8(c, g.u8(at(c.edi, 0x349))? & 1) == 0 {
                    0xf0354
                } else {
                    c.eax = g.u32(at(c.edi, 0xb4))?;
                    arithmetic(c, c.eax, NONE, true);
                    if c.eax == NONE {
                        0xf0354
                    } else {
                        c.ecx = g.u32(points::PHYSICS_COMPONENTS.base)?;
                        c.edx = g.u32(at(c.ecx, 0x44))?;
                        c.eax = and32(c, c.eax & 0xffff);
                        c.esi = c.eax.wrapping_mul(5);
                        c.esi = alu::shl(c, c.esi, 5, 32);
                        c.esi = arithmetic(c, c.esi, c.edx, false);
                        c.edx = slot(c, g, 0x18)?;
                        c.ecx = at(c.esp, 0x20);
                        c.edx = arithmetic(c, c.edx, 0x2ac, false);
                        return call(X205510, 0xeffe9);
                    }
                }
            }
            0xeffe9 => {
                if flags8(c, c.eax as u8) == 0 {
                    0xf0354
                } else {
                    c.edx = at(c.esp, 0x48);
                    push(c, g, c.edx)?;
                    c.eax = c.ebp;
                    return call(NODE_MATRIX, 0xefffd);
                }
            }
            0xefffd => {
                c.eax = at(c.esp, 0x20);
                c.ecx = at(c.esp, 0x48);
                return call(X1CFB90, 0xf000a);
            }
            0xf000a => {
                push(c, g, 0)?;
                push(c, g, c.ebp)?;
                return call(F6670, 0xf0012);
            }
            0xf0012 => {
                c.eax = c.ebp;
                return call(BA3D0, 0xf0019);
            }
            0xf0019 => 0xf0354,
            0xf001e => {
                c.eax = at(c.esp, 0x104);
                return call(points::VEHICLE_CONTACTS_RESET.base, 0xf002a);
            }
            0xf002a => {
                push(c, g, 0)?;
                c.ecx = at(c.esp, 0x24);
                c.eax = c.ebp;
                g.set_u8(at(c.esp, 0x16), 1)?;
                return call(BA1D0, 0xf003c);
            }
            0xf003c => {
                push(c, g, c.ebp)?;
                c.eax = at(c.esp, 0x108);
                push(c, g, c.eax)?;
                return call(CONTACTS, 0xf004a);
            }
            0xf004a => {
                if flags8(c, c.eax as u8) == 0 {
                    0xf0354
                } else {
                    let v = alu::shr(c, g.u8(at(c.edi, 0x348))?.into(), 2, 8) as u8;
                    let cl = flags8(c, v & 1);
                    c.ecx = byte(c.ecx, cl);
                    g.set_u8(at(c.esp, 0x13), cl)?;
                    if cl != 0 {
                        0xf00e9
                    } else {
                        c.edx = slot(c, g, 0x18)?;
                        c.eax = g.u16(at(c.edx, 0x1f0))? as i16 as i32 as u32;
                        arithmetic(c, c.eax, 6, true);
                        if c.eax > 6 { 0xf00e9 } else { g.u32(JUMP_TABLE.wrapping_add(c.eax * 4))? }
                    }
                }
            }
            // The jump table's cases: the type's physics on the contact
            // buffer.
            0xf007f => {
                c.eax = at(c.esp, 0x104);
                push(c, g, c.eax)?;
                c.ebx = c.ebp;
                return call(points::VEHICLE_TREAD_UPDATE.base, 0xf008e);
            }
            0xf0090 => {
                c.ecx = at(c.esp, 0x104);
                push(c, g, c.ecx)?;
                push(c, g, c.ebp)?;
                return call(points::VEHICLE_WHEEL_UPDATE.base, 0xf009e);
            }
            0xf00a0 => {
                c.edx = at(c.esp, 0x104);
                push(c, g, c.edx)?;
                push(c, g, c.ebp)?;
                return call(F2360, 0xf00ae);
            }
            0xf00b0 => {
                c.eax = slot(c, g, 0x14)?;
                push(c, g, c.eax)?;
                c.ecx = at(c.esp, 0x108);
                push(c, g, c.ecx)?;
                push(c, g, c.ebp)?;
                return call(F3010, 0xf00c3);
            }
            0xf00c5 => {
                c.edx = at(c.esp, 0x104);
                push(c, g, c.edx)?;
                push(c, g, c.ebp)?;
                return call(F4800, 0xf00d3);
            }
            0xf00d5 => {
                push(c, g, 0)?;
                push(c, g, 0)?;
                push(c, g, 0)?;
                c.eax = at(c.esp, 0x110);
                push(c, g, c.eax)?;
                push(c, g, c.ebp)?;
                return call(points::VEHICLE_CONTACT_SOLVE.base, 0xf00e9);
            }
            0xf008e | 0xf009e | 0xf00ae | 0xf00c3 | 0xf00d3 => 0xf00e9,
            0xf00e9 => {
                c.ecx = c.ebp;
                return call(F6010, 0xf00f0);
            }
            0xf00f0 => {
                push(c, g, 1)?;
                push(c, g, c.ebp)?;
                return call(F6670, 0xf00f8);
            }
            0xf00f8 => {
                c.ecx = at(c.esp, 0x104);
                push(c, g, c.ecx)?;
                c.eax = c.ebp;
                return call(F6BD0, 0xf0107);
            }
            0xf0107 => {
                c.esi = slot(c, g, 0x18)?;
                c.ecx = and32(c, g.u32(at(c.esi, 0x2e8))?);
                sse::zero(c, 2);
                sse::movaps(c, 0, 2);
                if (c.ecx as i32) > 0 {
                    // The mean of the contact records' floats.
                    c.eax = at(c.esp, 0x238);
                    c.edx = c.ecx;
                    loop {
                        let v = g.u32(c.eax)?;
                        sse::ss(c, Ss::Add, 0, v);
                        c.eax = arithmetic(c, c.eax, 0xa8, false);
                        c.edx = alu::dec32(c, c.edx);
                        if c.edx == 0 {
                            break;
                        }
                    }
                    and32(c, c.ecx);
                }
                if c.ecx == 0 {
                    sse::movaps(c, 0, 2);
                } else {
                    sse::cvtsi2ss(c, 1, c.ecx);
                    sse::ss_reg(c, Ss::Div, 0, 1);
                }
                let dl = alu::shr(c, g.u8(at(c.edi, 0xc0))?.into(), 6, 8) as u8;
                c.edx = byte(c.edx, dl);
                flags8(c, dl & 1);
                g.set_u32(at(c.edi, 0x3fc), sse::lane0(c, 0))?;
                if dl & 1 == 0 {
                    0xf0357
                } else {
                    let cl = g.u8(at(c.esi, 0x1f0))?;
                    c.ecx = byte(c.ecx, cl);
                    // `mov eax, 1; shl eax, cl`: its flags are overwritten
                    // by the `test`.
                    c.eax = 1u32 << (cl & 31);
                    if flags8(c, c.eax as u8 & 0x28) == 0 {
                        0xf0357
                    } else {
                        sse::load(c, 0, g.u32(at(c.edi, 0x25c))?);
                        let (x0, x2) = (sse::lane0(c, 0), sse::lane0(c, 2));
                        if below_or_equal(c, x0, x2) {
                            0xf0357
                        } else {
                            c.eax = c.ebp;
                            return call(CC380, 0xf0188);
                        }
                    }
                }
            }
            0xf0188 => {
                if flags8(c, c.eax as u8) != 0 {
                    0xf0357
                } else {
                    let v = alu::shr(c, g.u8(at(c.edi, 0x10a))?.into(), 2, 8) as u8;
                    c.ecx = byte(c.ecx, v);
                    if flags8(c, v & 1) != 0 {
                        0xf0357
                    } else {
                        c.esi = g.u32(ACTIVE_STRUCTURE)?;
                        fld64(c, g, 0xf01a8, K_45E610)?;
                        sse::load(c, 0, g.u32(at(c.esi, 0x1c))?);
                        c.eax = g.u32(points::GAME_TIME.base)?;
                        fld(c, g, 0xf01b8, at(c.eax, 4))?;
                        set_slot(c, g, 0x44, sse::lane0(c, 0))?;
                        sse::load(c, 0, g.u32(at(c.esi, 0x20))?);
                        set_slot(c, g, 0x3c, sse::lane0(c, 0))?;
                        sse::load(c, 0, g.u32(at(c.esi, 0x1c0))?);
                        set_slot(c, g, 0x1c, sse::lane0(c, 0))?;
                        sse::load(c, 0, g.u32(at(c.eax, 4))?);
                        sse::ss(c, Ss::Mul, 0, g.u32(K_45DBFC)?);
                        set_slot(c, g, 0x38, sse::lane0(c, 0))?;
                        return call(POW, 0xf01f2);
                    }
                }
            }
            0xf01f2 => {
                fstp(c, g, 0xf01f2, at(c.esp, 0x14))?;
                push(c, g, 0)?;
                c.ecx = at(c.esp, 0x24);
                c.eax = c.ebp;
                return call(BA1D0, 0xf0203);
            }
            0xf0203 => {
                drag(c, g)?;
                c.edx = at(c.esp, 0x20);
                push(c, g, c.edx)?;
                c.eax = and32(c, 0);
                c.esi = c.ebp;
                return call(B77D0, 0xf0354);
            }
            0xf0354 => {
                sse::zero(c, 2);
                0xf0357
            }
            0xf0357 => {
                c.eax = g.u32(points::GAME_GLOBALS.base)?;
                let mode = g.u8(at(c.eax, 0xc))?;
                cmp8(c, mode, 4);
                if mode == 4 {
                    0xf041a
                } else {
                    c.ecx = slot(c, g, 0x18)?;
                    c.edx = g.u32(at(c.ecx, 0x1ec))?;
                    c.edx = alu::shr(c, c.edx, 6, 32);
                    if flags8(c, c.edx as u8 & 1) == 0 {
                        0xf041a
                    } else {
                        c.eax = g.u32(points::GLOBALS_TAG.base)?;
                        c.ebp = g.u32(at(c.eax, 0x144))?;
                        c.eax = slot(c, g, ARG)?;
                        push(c, g, 0)?;
                        c.ecx = at(c.esp, 0x24);
                        return call(BA1D0, 0xf0399);
                    }
                }
            }
            0xf0399 => {
                sse::movaps(c, 0, 2);
                sse::ss(c, Ss::Sub, 0, g.u32(at(c.ebp, 0x5c))?);
                let x0 = sse::lane0(c, 0);
                if below_or_equal(c, x0, slot(c, g, 0x28)?) {
                    0xf041a
                } else {
                    c.esi = g.u32(at(c.edi, 0x10))?;
                    arithmetic(c, c.esi, NONE, true);
                    if c.esi == NONE { 0xf041a } else { 0xf03b0 }
                }
            }
            // Each child: a biped (type 0) not flagged at `+10a` bit 7
            // gets `d6660` and `d7b80`.
            0xf03b0 => {
                c.edx = g.u32(points::OBJECTS.base)?;
                c.eax = and32(c, c.esi & 0xffff);
                c.ecx = c.eax.wrapping_mul(3);
                c.eax = g.u32(at(c.edx, DATA))?;
                c.ebx = g.u32(at(c.eax.wrapping_add(c.ecx.wrapping_mul(4)), 8))?;
                let cl = g.u8(at(c.ebx, 0xaa))?;
                c.ecx = byte(c.ecx, cl);
                c.edx = 1u32 << (cl & 31);
                if flags8(c, c.edx as u8 & 1) == 0 {
                    0xf0412
                } else {
                    let al = alu::shr(c, g.u8(at(c.ebx, 0x10a))?.into(), 7, 8) as u8;
                    c.eax = byte(c.eax, al);
                    if flags8(c, al & 1) != 0 {
                        0xf0412
                    } else {
                        c.ecx = g.u32(at(c.ebp, 0x28))?;
                        push(c, g, c.ecx)?;
                        c.edx = at(c.esp, 0x80);
                        g.set_u16(at(c.esp, 0xfc), 0xffff)?;
                        return call(D6660, 0xf0400);
                    }
                }
            }
            0xf0400 => {
                push(c, g, 0)?;
                push(c, g, NONE)?;
                push(c, g, NONE)?;
                push(c, g, NONE)?;
                push(c, g, c.esi)?;
                push(c, g, c.edx)?;
                return call(D7B80, 0xf040f);
            }
            0xf040f => {
                sse::zero(c, 2);
                0xf0412
            }
            0xf0412 => {
                c.esi = g.u32(at(c.ebx, 0xc))?;
                arithmetic(c, c.esi, NONE, true);
                if c.esi != NONE { 0xf03b0 } else { 0xf041a }
            }
            0xf041a => {
                let on = flags8(c, g.u8(at(c.edi, 0x148))? & 1) != 0;
                c.eax = g.u32(at(c.edi, 0x134))?;
                c.eax = and32(c, if on { c.eax | 0x80_0000 } else { c.eax & 0xff7f_ffff });
                g.set_u32(at(c.edi, 0x134), c.eax)?;
                c.eax = alu::shr(c, c.eax, 0x17, 32);
                if flags8(c, c.eax as u8 & 1) == 0 {
                    0xf0495
                } else {
                    sse::load(c, 1, g.u32(K_45DBC0)?);
                    let x1 = sse::lane0(c, 1);
                    if below_or_equal(c, x1, g.u32(at(c.edi, 0x2c4))?) {
                        0xf04dc
                    } else {
                        c.esi = slot(c, g, 0x18)?;
                        sse::load(c, 0, g.u32(at(c.esi, 0x25c))?);
                        c.ecx = g.u32(points::GAME_TIME.base)?;
                        sse::ss(c, Ss::Mul, 0, g.u32(at(c.ecx, 4))?);
                        sse::ss(c, Ss::Add, 0, g.u32(at(c.edi, 0x2c4))?);
                        let (x0, x1) = (sse::lane0(c, 0), sse::lane0(c, 1));
                        let not_above = below_or_equal(c, x0, x1);
                        g.set_u32(at(c.edi, 0x2c4), x0)?;
                        if !not_above {
                            sse::movaps(c, 0, 1);
                        }
                        g.set_u32(at(c.edi, 0x2c4), sse::lane0(c, 0))?;
                        g.set_u8(at(c.esp, 0x12), 1)?;
                        0xf04e0
                    }
                }
            }
            0xf0495 => {
                sse::load(c, 0, g.u32(at(c.edi, 0x2c4))?);
                let (x0, x2) = (sse::lane0(c, 0), sse::lane0(c, 2));
                if below_or_equal(c, x0, x2) {
                    0xf04dc
                } else {
                    c.esi = slot(c, g, 0x18)?;
                    sse::load(c, 1, g.u32(at(c.esi, 0x25c))?);
                    c.edx = g.u32(points::GAME_TIME.base)?;
                    sse::ss(c, Ss::Mul, 1, g.u32(at(c.edx, 4))?);
                    sse::ss_reg(c, Ss::Sub, 0, 1);
                    let (x0, x2) = (sse::lane0(c, 0), sse::lane0(c, 2));
                    let not_above = below_or_equal(c, x0, x2);
                    g.set_u32(at(c.edi, 0x2c4), x0)?;
                    if not_above {
                        sse::movaps(c, 0, 2);
                    }
                    g.set_u32(at(c.edi, 0x2c4), sse::lane0(c, 0))?;
                    g.set_u8(at(c.esp, 0x12), 1)?;
                    0xf04e0
                }
            }
            0xf04dc => {
                c.esi = slot(c, g, 0x18)?;
                0xf04e0
            }
            0xf04e0 => {
                fld(c, g, 0xf04e0, at(c.edi, 0x354))?;
                unary(c, g, 0xf04e6, unary_op::ABS)?;
                fld(c, g, 0xf04e8, at(c.esi, 0x214))?;
                fxch(c, g, 0xf04ee)?;
                let below = fcomip(c, g, 0xf04f0)? & 1 != 0;
                fpop(c, g, 0xf04f2)?;
                let bl = if below {
                    flags8(c, 0);
                    0
                } else {
                    1
                };
                c.ebx = byte(c.ebx, bl);
                c.eax = (c.eax & 0xffff_0000) | u32::from(g.u16(at(c.edi, 0x348))?);
                let v = alu::shr(c, c.eax & 0xff, 5, 8) as u8;
                let cl = flags8(c, v & 1);
                c.ecx = byte(c.ecx, cl);
                if cl != 0 {
                    let a = alu::shr(c, c.eax & 0xff, 1, 8) as u8;
                    c.eax = byte(c.eax, a);
                } else {
                    c.eax = byte(c.eax, g.u8(at(c.edi, 0x348))?);
                }
                let al = flags8(c, c.eax as u8 & 1);
                c.eax = byte(c.eax, al);
                cmp8(c, bl, al);
                let changed = flags8(c, u8::from(bl != al));
                c.eax = byte(c.eax, changed);
                if changed == 0 { 0xf05c0 } else { 0xf0526 }
            }
            0xf0526 => {
                // `mov dl, bl; neg dl; sbb edx, edx`: their flags are
                // overwritten by the `test`.
                let bl = c.ebx as u8;
                c.edx = if bl != 0 { u32::MAX } else { 0 };
                let cl = c.ecx as u8;
                if flags8(c, cl) == 0 {
                    0xf0568
                } else {
                    c.ecx = and32(c, g.u32(at(c.esi, 0x2f0))?);
                    if (c.ecx as i32) <= 0 {
                        0xf0568
                    } else {
                        c.eax = g.u32(at(c.esi, 0x2f4))?;
                        c.eax = arithmetic(c, c.eax, 0x48, false);
                        c.esi = c.ecx;
                        loop {
                            c.ecx = g.u32(c.eax.wrapping_sub(0x44))?;
                            c.ecx = alu::shr(c, c.ecx, 4, 32);
                            if flags8(c, c.ecx as u8 & 1) != 0 {
                                c.ecx = g.u32(c.eax)?;
                                arithmetic(c, c.ecx, NONE, true);
                                if c.ecx != NONE {
                                    c.ebp = !(1u32 << (c.ecx & 31));
                                    c.edx = and32(c, c.edx & c.ebp);
                                }
                            }
                            c.eax = arithmetic(c, c.eax, 0x4c, false);
                            c.esi = alu::dec32(c, c.esi);
                            if c.esi == 0 {
                                break;
                            }
                        }
                        0xf0568
                    }
                }
            }
            0xf0568 => {
                c.ebp = slot(c, g, ARG)?;
                push(c, g, c.edx)?;
                push(c, g, 0)?;
                push(c, g, NONE)?;
                push(c, g, c.ebp)?;
                return call(BA7F0, 0xf057a);
            }
            0xf057a => {
                let dl = alu::shr(c, g.u8(at(c.edi, 0x348))?.into(), 5, 8) as u8;
                c.edx = byte(c.edx, dl);
                let a = at(c.edi, 0x348);
                let bl = c.ebx as u8;
                if flags8(c, dl & 1) != 0 {
                    if flags8(c, bl) != 0 {
                        alu::rmw8(c, g, a, |v| v | 2)?;
                    } else {
                        alu::rmw8(c, g, a, |v| v & 0xfd)?;
                    }
                    alu::rmw8(c, g, a, |v| v & 0xfe)?;
                } else {
                    alu::rmw8(c, g, a, |v| v & 0xfd)?;
                    if flags8(c, bl) == 0 {
                        alu::rmw8(c, g, a, |v| v & 0xfe)?;
                    } else {
                        alu::rmw8(c, g, a, |v| v | 1)?;
                    }
                }
                0xf05c7
            }
            0xf05c0 => {
                c.ebp = slot(c, g, ARG)?;
                0xf05c7
            }
            0xf05c7 => {
                c.eax = g.u32(at(c.edi, 0x14))?;
                c.esi = and32(c, NONE);
                c.ebx = byte(c.ebx, flags8(c, 0));
                arithmetic(c, c.eax, c.esi, true);
                if c.eax == c.esi {
                    let component = g.u32(at(c.edi, 0xb4))?;
                    arithmetic(c, component, c.esi, true);
                    let root = component == c.esi || flags8(c, g.u8(at(c.edi, 0xc1))? & 1) == 0;
                    if root {
                        c.ebx = byte(c.ebx, 1);
                    }
                }
                c.eax = c.ebp;
                return call(X113E40, 0xf05ed);
            }
            0xf05ed => {
                let wake = if flags8(c, c.eax as u8) == 0 {
                    flags8(c, c.ebx as u8) != 0
                } else {
                    c.eax = g.u32(points::OBJECTS.base)?;
                    c.ecx = g.u32(at(c.eax, DATA))?;
                    c.edx = slot(c, g, 0x40)?;
                    c.ecx = g.u32(at(at(c.ecx, c.edx), 8))?;
                    c.eax = g.u16(at(c.ecx, 0x12a))? as i16 as i32 as u32;
                    c.eax = arithmetic(c, c.eax, c.ecx, false);
                    let state = g.u32(at(c.eax, 0x68))?;
                    arithmetic(c, state, c.esi, true);
                    if state == c.esi {
                        true
                    } else {
                        let tag = g.u32(c.eax)?;
                        arithmetic(c, tag, c.esi, true);
                        if tag == c.esi {
                            true
                        } else {
                            let index = g.u16(at(c.eax, 6))?;
                            alu::cmp16(c, index.into(), c.esi);
                            if u32::from(index) == c.esi & 0xffff {
                                true
                            } else {
                                let name = g.u32(at(c.eax, 0x7c))?;
                                let imm = g.u32(ANIMATION_IMM)?;
                                arithmetic(c, name, imm, true);
                                name != imm || flags8(c, c.ebx as u8) != 0
                            }
                        }
                    }
                };
                if wake {
                    c.eax = c.ebp;
                    g.set_u8(at(c.esp, 0x12), 1)?;
                    return call(points::OBJECT_WAKE_FOR_POST.base, 0xf0632);
                }
                0xf0632
            }
            0xf0632 => {
                c.eax = and32(c, 0);
                set_slot(c, g, 0x20, 0)?;
                set_slot(c, g, 0x24, 0)?;
                set_slot(c, g, 0x28, 0)?;
                c.eax = g.u32(PAIR_IMM)?;
                push(c, g, c.ebp)?;
                set_slot(c, g, 0x24, c.eax)?;
                set_slot(c, g, 0x28, c.eax)?;
                return call(E6830, 0xf0653);
            }
            0xf0653 => {
                c.ecx = at(c.esp, 0x20);
                push(c, g, c.ecx)?;
                push(c, g, c.ebp)?;
                c.ebx = byte(c.ebx, c.eax as u8);
                return call(X111650, 0xf0660);
            }
            0xf0660 => {
                let bl = flags8(c, c.ebx as u8 | c.eax as u8);
                c.ebx = byte(c.ebx, bl);
                c.ecx = at(c.edi, 0x3d0);
                c.eax = c.ebp;
                for off in [0x3bc, 0x3c4, 0x3c8, 0x3cc] {
                    g.set_u32(at(c.edi, off), c.esi)?;
                }
                return call(B9EF0, 0xf0687);
            }
            0xf0687 => {
                let cl = g.u8(at(c.esp, 0x12))?;
                c.ecx = byte(c.ecx, cl);
                c.edi = pop(c, g)?;
                c.esi = pop(c, g)?;
                c.eax = byte(c.eax, c.ebx as u8);
                c.ebp = pop(c, g)?;
                let al = flags8(c, c.eax as u8 | cl);
                c.eax = byte(c.eax, al);
                c.ebx = pop(c, g)?;
                c.esp = arithmetic(c, c.esp, FRAME, false);
                return Ok(Next::Ret { pop: 4 });
            }
            _ => return Err(unowned(pc)),
        };
    }
}

/// `f0203`..`f0346`: the drag on the velocity change at `[S+20]` and the
/// pull toward the structure's point, in scalar SSE (`xmm1` 0 for the
/// three "is it zero" tests, `xmm7` the scaled tick, `[S+14]` the drag
/// factor `372d48` returned), with `fsqrt` on the x87 for the distance.
fn drag(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let s = |c: &CpuAbi, off: u32| at(c.esp, off);
    sse::load(c, 4, g.u32(s(c, 0x44))?);
    sse::zero(c, 1);
    let mut f0242 = true;
    let (x4, x1) = (sse::lane0(c, 4), sse::lane0(c, 1));
    if !equal_jnp(c, x4, x1) {
        let z = g.u32(at(c.edi, 0x6c))?;
        if !below_or_equal(c, x4, z) {
            sse::ss(c, Ss::Sub, 4, z);
            sse::load(c, 0, g.u32(s(c, 0x28))?);
            sse::load(c, 7, g.u32(s(c, 0x38))?);
            sse::ss(c, Ss::Mul, 0, g.u32(s(c, 0x14))?);
            sse::ss_reg(c, Ss::Mul, 4, 7);
            sse::ss_reg(c, Ss::Add, 4, 0);
            g.set_u32(s(c, 0x28), sse::lane0(c, 4))?;
            f0242 = false;
        }
    }
    if f0242 {
        sse::load(c, 4, g.u32(s(c, 0x28))?);
        sse::load(c, 7, g.u32(s(c, 0x38))?);
    }
    // f024e
    sse::load(c, 0, g.u32(s(c, 0x3c))?);
    let (x0, x1) = (sse::lane0(c, 0), sse::lane0(c, 1));
    if !equal_jnp(c, x0, x1) {
        sse::load(c, 2, g.u32(at(c.edi, 0x6c))?);
        let x2 = sse::lane0(c, 2);
        if !below_or_equal(c, x2, x0) {
            sse::ss(c, Ss::Mul, 4, g.u32(s(c, 0x14))?);
            sse::ss_reg(c, Ss::Sub, 0, 2);
            sse::ss_reg(c, Ss::Mul, 0, 7);
            sse::ss_reg(c, Ss::Add, 0, 4);
            sse::movaps(c, 4, 0);
            g.set_u32(s(c, 0x28), sse::lane0(c, 4))?;
        }
    }
    // f0282
    sse::load(c, 6, g.u32(s(c, 0x1c))?);
    let (x6, x1) = (sse::lane0(c, 6), sse::lane0(c, 1));
    if equal_jnp(c, x6, x1) {
        return Ok(());
    }
    sse::load(c, 2, g.u32(at(c.esi, 0x1c8))?);
    sse::ss(c, Ss::Sub, 2, g.u32(at(c.edi, 0x68))?);
    sse::load(c, 3, g.u32(at(c.esi, 0x1cc))?);
    sse::ss(c, Ss::Sub, 3, g.u32(at(c.edi, 0x6c))?);
    sse::load(c, 1, g.u32(at(c.esi, 0x1c4))?);
    sse::ss(c, Ss::Sub, 1, g.u32(at(c.edi, 0x64))?);
    sse::movaps(c, 0, 3);
    sse::ss_reg(c, Ss::Mul, 0, 3);
    sse::movaps(c, 5, 2);
    sse::ss_reg(c, Ss::Mul, 5, 2);
    sse::ss_reg(c, Ss::Add, 0, 5);
    sse::movaps(c, 5, 1);
    sse::ss_reg(c, Ss::Mul, 5, 1);
    sse::ss_reg(c, Ss::Add, 0, 5);
    sse::movaps(c, 5, 6);
    sse::ss_reg(c, Ss::Mul, 5, 6);
    let (x0, x5) = (sse::lane0(c, 0), sse::lane0(c, 5));
    let not_above = below_or_equal(c, x0, x5);
    g.set_u32(s(c, 0x1c), x0)?;
    if not_above {
        return Ok(());
    }
    fld(c, g, 0xf02eb, s(c, 0x1c))?;
    sse::ss(c, Ss::Mul, 4, g.u32(s(c, 0x14))?);
    unary(c, g, 0xf02f5, unary_op::SQRT)?;
    fstp(c, g, 0xf02f7, s(c, 0x1c))?;
    sse::load(c, 5, g.u32(s(c, 0x1c))?);
    sse::movaps(c, 0, 5);
    sse::ss_reg(c, Ss::Sub, 0, 6);
    sse::ss_reg(c, Ss::Div, 0, 5);
    sse::ss_reg(c, Ss::Mul, 0, 7);
    sse::ss_reg(c, Ss::Mul, 3, 0);
    sse::ss_reg(c, Ss::Mul, 1, 0);
    sse::ss_reg(c, Ss::Mul, 2, 0);
    sse::load(c, 0, g.u32(s(c, 0x20))?);
    sse::ss_reg(c, Ss::Add, 0, 1);
    g.set_u32(s(c, 0x20), sse::lane0(c, 0))?;
    sse::load(c, 0, g.u32(s(c, 0x24))?);
    sse::ss_reg(c, Ss::Add, 0, 2);
    sse::ss_reg(c, Ss::Add, 3, 4);
    g.set_u32(s(c, 0x24), sse::lane0(c, 0))?;
    g.set_u32(s(c, 0x28), sse::lane0(c, 3))
}

/// `f1320 (object: eax) -> al`; `ret`. A leaf: `al` 1 when damage flags
/// `+10a` bit 2 is set and the definition's type (`+1f0`) is 5, or 4
/// with definition `+218` equal to 1. Exit: `ecx` the body with its low
/// byte replaced (by `+10a >> 2`, then the type word in `cx` when the bit
/// is set), `edx` the definition, `eax` the tag offset with `al` the
/// answer; the flags of the last `test` or `cmp`.
pub(super) fn vehicle_update_gate(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::VEHICLE_UPDATE_GATE.base {
        return Err(unowned(pc));
    }
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.edx = g.u32(at(c.ecx, DATA))?;
    c.eax = and32(c, c.eax & 0xffff).wrapping_mul(3);
    c.ecx = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
    c.eax = g.u32(c.ecx)?;
    c.edx = g.u32(points::TAG_ENTRIES.base)?;
    c.ecx = byte(c.ecx, g.u8(at(c.ecx, 0x10a))?);
    c.eax = and32(c, c.eax & 0xffff);
    c.eax = alu::shl(c, c.eax, 4, 32);
    c.edx = g.u32(at(at(c.eax, c.edx), 8))?;
    let cl = alu::shr(c, c.ecx & 0xff, 2, 8) as u8;
    c.ecx = byte(c.ecx, cl);
    c.eax = byte(c.eax, flags8(c, 0));
    if flags8(c, cl & 1) == 0 {
        return Ok(Next::Ret { pop: 0 });
    }
    let kind = u32::from(g.u16(at(c.edx, 0x1f0))?);
    c.ecx = (c.ecx & 0xffff_0000) | kind;
    alu::cmp16(c, kind, 4);
    let yes = if kind == 4 {
        let w = u32::from(g.u16(at(c.edx, 0x218))?);
        alu::cmp16(c, w, 1);
        w == 1 || {
            alu::cmp16(c, kind, 5);
            false
        }
    } else {
        alu::cmp16(c, kind, 5);
        kind == 5
    };
    if yes {
        c.eax = byte(c.eax, 1);
    }
    Ok(Next::Ret { pop: 0 })
}

/// `f06c0 (buffer: eax)`; `ret`. A leaf: the first word of each of the 16
/// contact records (`d8` bytes, from `buffer + bc0`) set to none. Exit:
/// `ecx` past the last record, `edx` 0, the flags of the last `dec` (CF
/// from the last `add`).
pub(super) fn vehicle_contacts_reset(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::VEHICLE_CONTACTS_RESET.base {
        return Err(unowned(pc));
    }
    c.ecx = at(c.eax, 0xbc0);
    c.edx = 0x10;
    loop {
        g.set_u16(c.ecx, 0xffff)?;
        c.ecx = arithmetic(c, c.ecx, 0xd8, false);
        c.edx = alu::dec32(c, c.edx);
        if c.edx == 0 {
            break;
        }
    }
    Ok(Next::Ret { pop: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    fn guest() -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0, 0x0080_0000, Permissions::RW).unwrap();
        g
    }

    #[test]
    fn the_contact_reset_marks_sixteen_records_and_leaves_ecx_past_them() {
        let mut g = guest();
        let mut c = CpuAbi { eax: 0x0010_0000, ..Default::default() };
        let next = vehicle_contacts_reset(&mut Cx { cpu: &mut c, g: &mut g }, points::VEHICLE_CONTACTS_RESET.base).unwrap();
        assert_eq!(next, Next::Ret { pop: 0 });
        for i in 0..16 {
            assert_eq!(g.u16(0x0010_0bc0 + 0xd8 * i).unwrap(), 0xffff);
            assert_eq!(g.u16(0x0010_0bc2 + 0xd8 * i).unwrap(), 0);
        }
        assert_eq!(c.ecx, 0x0010_0bc0 + 0xd8 * 16);
        assert_eq!(c.edx, 0);
        assert_eq!(c.eflags & 0x40, 0x40);
    }

    /// A pool of one vehicle (slot 1) whose definition is tag `12`.
    fn vehicle(g: &mut Guest, damage: u16, kind: u16, flier: u16) -> u32 {
        let (objects, data, body, tags, def) = (0x1000, 0x2000, 0x3000, 0x5000, 0x6000);
        g.set_u32(points::OBJECTS.base, objects).unwrap();
        g.set_u32(objects + DATA, data).unwrap();
        g.set_u32(data + 12 + 8, body).unwrap();
        g.set_u32(body, 0xe000_0012).unwrap();
        g.set_u16(body + 0x10a, damage).unwrap();
        g.set_u32(points::TAG_ENTRIES.base, tags).unwrap();
        g.set_u32(tags + 0x128, def).unwrap();
        g.set_u16(def + 0x1f0, kind).unwrap();
        g.set_u16(def + 0x218, flier).unwrap();
        0x8001_0001
    }

    fn gate(g: &mut Guest, object: u32) -> CpuAbi {
        let mut c = CpuAbi { eax: object, ecx: 0xdead_beef, ..Default::default() };
        let next = vehicle_update_gate(&mut Cx { cpu: &mut c, g }, points::VEHICLE_UPDATE_GATE.base).unwrap();
        assert_eq!(next, Next::Ret { pop: 0 });
        c
    }

    #[test]
    fn the_gate_answers_for_types_five_and_four_with_the_flag() {
        for (damage, kind, flier, yes) in [
            (4, 5, 0, true),
            (4, 4, 1, true),
            (4, 4, 0, false),
            (4, 2, 1, false),
            (0, 5, 1, false),
            (0x0e, 5, 0, true),
            (0x0b, 5, 0, false),
        ] {
            let mut g = guest();
            let object = vehicle(&mut g, damage, kind, flier);
            let c = gate(&mut g, object);
            assert_eq!(c.eax & 0xff, u32::from(yes), "damage {damage:#x} type {kind} flier {flier}");
            assert_eq!(c.eax & !0xff, 0x120 & !0xff);
            assert_eq!(c.edx, 0x6000);
            if damage & 4 == 0 {
                // `test cl, 1` with the bit clear: ZF.
                assert_eq!(c.eflags & 0x40, 0x40);
                assert_eq!(c.ecx, (0x3000 & !0xff) | u32::from(damage as u8 >> 2));
            } else {
                assert_eq!(c.ecx & 0xffff, u32::from(kind));
            }
        }
    }
}
