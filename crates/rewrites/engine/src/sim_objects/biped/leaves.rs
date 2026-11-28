//! The biped update's leaves: the carrier `baf40`, the animation kind
//! `dd300`, the landing test `e4050` and the seconds-to-ticks conversion
//! `1469f0`. None calls guest code; each reproduces every register, flag,
//! stack store and x87 effect its retail code leaves.

use super::{
    NONE, and32, arithmetic, at, cmp8, flags8, fld_fistp, greater, less, pop, push, set_slot, slot, store_ss, sx8,
    sx16,
};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_objects::alu;
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{byte, logic};

/// `.rdata` constant: the landing test's time in seconds.
const K_45DF20: u32 = 0x0045_df20;

/// The `imm32`s `dd300` compares the name with (the operand of each
/// `cmp ecx, imm32` and `sub ecx, imm32`), and the `imm8` of `sub ecx, 3`.
const KIND_A: u32 = 0x000d_d302;
const KIND_B: u32 = 0x000d_d30e;
const KIND_C: u32 = 0x000d_d318;
const KIND_D: u32 = 0x000d_d320;
const KIND_E: u32 = 0x000d_d329;
const KIND_F: u32 = 0x000d_d331;
const KIND_G: u32 = 0x000d_d33d;
const KIND_H8: u32 = 0x000d_d345;
const KIND_I: u32 = 0x000d_d34a;

/// `baf40 (object: eax) -> eax`; `ret`. The first object up the parent
/// chain from `object` (itself included) whose body `+4` bit 26 is clear,
/// or none when the chain ends first. `esi` is pushed and restored; exit
/// `ecx` the last body read, `edx` its `+4 >> 1a`, the flags of the last
/// `test` or `cmp`.
pub(in crate::sim_objects) fn carrier(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_CARRIER.base {
        return Err(unowned(pc));
    }
    arithmetic(c, c.eax, NONE, true);
    if c.eax == NONE {
        return Ok(Next::Ret { pop: 0 });
    }
    c.ecx = g.u32(points::OBJECTS.base)?;
    push(c, g, c.esi)?;
    c.esi = g.u32(at(c.ecx, DATA))?;
    loop {
        c.ecx = and32(c, c.eax & 0xffff);
        c.edx = c.ecx.wrapping_mul(3);
        c.ecx = g.u32(at(c.esi.wrapping_add(c.edx.wrapping_mul(4)), 8))?;
        c.edx = g.u32(at(c.ecx, 4))?;
        c.edx = alu::shr(c, c.edx, 0x1a, 32);
        if flags8(c, c.edx as u8 & 1) == 0 {
            break;
        }
        c.eax = g.u32(at(c.ecx, 0x14))?;
        arithmetic(c, c.eax, NONE, true);
        if c.eax == NONE {
            break;
        }
    }
    c.esi = pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}

/// `dd300 (name: ecx) -> al`; `ret`. The animation kind of the name
/// (string ID) in `ecx`: 0, 1, or 2 when no compare matches. Signed
/// compares against nine immediates read from the image; on the `sub`
/// path `ecx` returns reduced, and the flags are those of the last
/// `cmp` or `sub`, or of `xor al, al`.
pub(in crate::sim_objects) fn animation_kind(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::BIPED_ANIMATION_KIND.base {
        return Err(unowned(pc));
    }
    const ZERO: u8 = 0;
    const ONE: u8 = 1;
    const OTHER: u8 = 2;
    let done = |c: &mut runtime::CpuAbi, al: Option<u8>| -> Result<Next> {
        if let Some(al) = al {
            c.eax = byte(c.eax, al);
        }
        Ok(Next::Ret { pop: 0 })
    };
    arithmetic(c, c.ecx, g.u32(KIND_A)?, true);
    c.eax = byte(c.eax, OTHER);
    let pc = if greater(c) {
        0xdd33b
    } else if c.ecx == g.u32(KIND_A)? {
        0xdd338
    } else {
        arithmetic(c, c.ecx, g.u32(KIND_B)?, true);
        if greater(c) {
            0xdd327
        } else if c.ecx == g.u32(KIND_B)? {
            0xdd338
        } else {
            arithmetic(c, c.ecx, g.u32(KIND_C)?, true);
            if c.ecx == g.u32(KIND_C)? {
                0xdd338
            } else {
                arithmetic(c, c.ecx, g.u32(KIND_D)?, true);
                if c.ecx == g.u32(KIND_D)? { 0xdd350 } else { 0xdd352 }
            }
        }
    };
    match pc {
        0xdd327 => {
            arithmetic(c, c.ecx, g.u32(KIND_E)?, true);
            if less(c) {
                return done(c, None);
            }
            arithmetic(c, c.ecx, g.u32(KIND_F)?, true);
            if alu::less_or_equal(c) {
                return done(c, Some(ONE));
            }
            done(c, None)
        }
        0xdd338 => {
            logic(c, 0, 0x80);
            done(c, Some(ZERO))
        }
        0xdd33b => {
            c.ecx = arithmetic(c, c.ecx, g.u32(KIND_G)?, true);
            if c.ecx == 0 {
                return done(c, Some(ONE));
            }
            c.ecx = arithmetic(c, c.ecx, sx8(g.u8(KIND_H8)?), true);
            if c.ecx == 0 {
                return done(c, Some(ONE));
            }
            c.ecx = arithmetic(c, c.ecx, g.u32(KIND_I)?, true);
            if c.ecx == 0 {
                return done(c, Some(ONE));
            }
            done(c, None)
        }
        0xdd350 => done(c, Some(ONE)),
        _ => done(c, None),
    }
}

/// `e4050 (object: eax) -> eax`, 1 or 0; `ret`. 1 when body `+c1` bit 0
/// is clear, body `+348` bit 0 is set, the counter `+399` has reached the
/// tick count of `[45df20]` seconds (`fld`/`fistp` into its frame), the
/// state `+3dc` is 1 or 3 and the biped has no parent. Frame: `sub esp, 8`,
/// `[esp]` the seconds times the tick rate, `[esp+4]` the ticks. Exit
/// `edx` 1, `ecx` the last value read into it, the flags of `add esp, 8`.
pub(in crate::sim_objects) fn airborne_test(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::BIPED_AIRBORNE_TEST.base {
        return Err(unowned(pc));
    }
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.edx = g.u32(at(c.ecx, DATA))?;
    c.eax = and32(c, c.eax & 0xffff).wrapping_mul(3);
    c.eax = g.u32(at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8))?;
    let cl = g.u8(at(c.eax, 0xc1))?;
    c.ecx = byte(c.ecx, cl);
    c.edx = 1;
    c.esp = arithmetic(c, c.esp, 8, true);
    let yes = 'test: {
        if flags8(c, cl & 1) != 0 {
            break 'test false;
        }
        if !alu::test8(c, g, at(c.eax, 0x348), 1)? {
            break 'test false;
        }
        c.ecx = g.u32(points::GAME_TIME.base)?;
        c.ecx = sx16(g.u16(at(c.ecx, 2))?);
        sse::cvtsi2ss(c, 0, c.ecx);
        sse::ss(c, Ss::Mul, 0, g.u32(K_45DF20)?);
        store_ss(c, g, c.esp, 0)?;
        fld_fistp(c, g, c.esp, at(c.esp, 4), 0xe409d)?;
        c.ecx = sx8(g.u8(at(c.eax, 0x399))?);
        arithmetic(c, c.ecx, slot(c, g, 4)?, true);
        if less(c) {
            break 'test false;
        }
        let cl = g.u8(at(c.eax, 0x3dc))?;
        c.ecx = byte(c.ecx, cl);
        cmp8(c, cl, 1);
        if cl != 1 {
            cmp8(c, cl, 3);
            if cl != 3 {
                break 'test false;
            }
        }
        let parent = g.u32(at(c.eax, 0x14))?;
        arithmetic(c, parent, NONE, true);
        parent == NONE
    };
    c.eax = if yes { c.edx } else { and32(c, 0) };
    c.esp = arithmetic(c, c.esp, 8, false);
    Ok(Next::Ret { pop: 0 })
}

/// `1469f0 (seconds: stack 0) -> eax`; `ret 4`. The seconds times the
/// tick rate (`[game time + 2]`), rounded by `fistp` under the guest's
/// control word. The product is stored over the argument slot, and the
/// integer over the saved `ecx`, so `ecx` returns equal to `eax`. No
/// instruction writes the flags.
pub(in crate::sim_objects) fn seconds_to_ticks(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::GAME_SECONDS_TO_TICKS.base {
        return Err(unowned(pc));
    }
    push(c, g, c.ecx)?;
    c.eax = g.u32(points::GAME_TIME.base)?;
    c.ecx = sx16(g.u16(at(c.eax, 2))?);
    sse::cvtsi2ss(c, 0, c.ecx);
    sse::ss(c, Ss::Mul, 0, slot(c, g, 8)?);
    set_slot(c, g, 8, sse::lane0(c, 0))?;
    fld_fistp(c, g, at(c.esp, 8), c.esp, 0x146a0e)?;
    c.eax = slot(c, g, 0)?;
    c.ecx = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}
