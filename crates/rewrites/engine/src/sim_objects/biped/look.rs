//! The biped's look update `de190`, called by `dd360` for every biped.

use super::{NONE, and32, arithmetic, at, below_or_equal, call, flags8, pop, push, set_slot, slot, store_ss, sx16};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_objects::alu;
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::byte;
use runtime::{CpuAbi, Guest};

/// `de190`'s locals.
const FRAME: u32 = 0x9c;
/// `.rdata` constants: the step's lower and upper clamps.
const K_45E024: u32 = 0x0045_e024;
const K_45DC7C: u32 = 0x0045_dc7c;
/// The `imm32`s of `push imm32` at `de1dc` and `de1f6` (marker names).
const MARKER_A: u32 = 0x000d_e1dd;
const MARKER_B: u32 = 0x000d_e1f7;

/// Callees that stay guest.
const X10F820: u32 = 0x0010_f820;
const DE080: u32 = 0x000d_e080;

/// Every return address of `de190`'s calls.
pub(in crate::sim_objects) const LOOK_SITES: [u32; 5] = [0xde1c8, 0xde1ef, 0xde203, 0xde267, 0xde27b];

/// `de190 (object: eax)`; `ret`. Frame: `sub esp, 9c`, then `ebp`, `esi`,
/// `edi` pushed (`T` = that `esp`), and `ebx` from `de1d0` to `de2d7`
/// (`U` = `T - 4`, the `esp` at every site after the first). Across the
/// calls `ebp` the body, `edi` the object (then, after `cba50`, `U+34`),
/// `esi` the node matrices (body `+ [body+116]`), `ebx` the marker buffer
/// `U+3c` that `b8d30` fills.
///
/// With a target (body `+13c`) and `10f820` answering 0: the first of two
/// markers (names read from the image) through `b8d30`, the vector from
/// the root node's position (`+28..+30`) to the marker's (`[U+9c..a4]`)
/// at `U+1c`, the aim (body `+168`) copied to `U+28` and clamped by
/// `cba50` (fused, the look limits), `de080` (`esi` body `+7c`, `edi`
/// `U+34`, `ecx` `U+10`, `edx` `U+28`), and the two dot products into
/// `U+10..18`. The first time (body `+3c8` clear) they are stored at body
/// `+3cc..3d4` and `+3c8` set; afterwards each component of `+3cc..3d4`
/// moves toward them by at most the two clamps.
pub(in crate::sim_objects) fn look_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    let entry = points::BIPED_LOOK_UPDATE.base;
    match pc {
        p if p == entry => {
            c.ecx = g.u32(points::OBJECTS.base)?;
            c.edx = g.u32(at(c.ecx, DATA))?;
            c.esp = arithmetic(c, c.esp, FRAME, true);
            push(c, g, c.ebp)?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.edi = c.eax;
            c.eax = and32(c, c.eax & 0xffff).wrapping_mul(3);
            let record = at(c.edx.wrapping_add(c.eax.wrapping_mul(4)), 8);
            c.ebp = g.u32(record)?;
            c.esi = record;
            let target = g.u32(at(c.ebp, 0x13c))?;
            arithmetic(c, target, NONE, true);
            if target == NONE {
                return look_ret(c, g);
            }
            c.eax = c.edi;
            call(X10F820, 0xde1c8)
        }
        0xde1c8 => {
            if flags8(c, c.eax as u8) != 0 {
                return look_ret(c, g);
            }
            push(c, g, c.ebx)?;
            c.eax = c.ebp;
            c.esi = sx16(g.u16(at(c.eax, 0x116))?);
            push(c, g, 1)?;
            push(c, g, g.u32(MARKER_A)?)?;
            push(c, g, c.edi)?;
            c.ecx = byte(c.ecx, flags8(c, 0));
            c.ebx = at(c.esp, 0x48);
            c.esi = arithmetic(c, c.esi, c.eax, false);
            call(points::SIM_MARKER_TRANSFORMS.base, 0xde1ef)
        }
        0xde1ef => {
            alu::test16(c, c.eax);
            if c.eax & 0xffff != 0 {
                return directions(c, g);
            }
            push(c, g, 1)?;
            push(c, g, g.u32(MARKER_B)?)?;
            push(c, g, c.edi)?;
            c.ecx = byte(c.ecx, flags8(c, 0));
            call(points::SIM_MARKER_TRANSFORMS.base, 0xde203)
        }
        0xde203 => directions(c, g),
        0xde267 => {
            c.esi = at(c.ebp, 0x7c);
            c.edi = at(c.esp, 0x34);
            c.ecx = at(c.esp, 0x10);
            c.edx = at(c.esp, 0x28);
            call(DE080, 0xde27b)
        }
        0xde27b => smooth(c, g),
        _ => Err(unowned(pc)),
    }
}

/// `de203`: the vector to the marker and the aim, then `cba50`.
fn directions(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    sse::load(c, 0, slot(c, g, 0x9c)?);
    sse::ss(c, Ss::Sub, 0, g.u32(at(c.esi, 0x28))?);
    c.eax = at(c.ebp, 0x168);
    c.ecx = g.u32(c.eax)?;
    c.edx = g.u32(at(c.eax, 4))?;
    c.eax = g.u32(at(c.eax, 8))?;
    store_ss(c, g, at(c.esp, 0x1c), 0)?;
    sse::load(c, 0, slot(c, g, 0xa0)?);
    sse::ss(c, Ss::Sub, 0, g.u32(at(c.esi, 0x2c))?);
    store_ss(c, g, at(c.esp, 0x20), 0)?;
    sse::load(c, 0, slot(c, g, 0xa4)?);
    sse::ss(c, Ss::Sub, 0, g.u32(at(c.esi, 0x30))?);
    set_slot(c, g, 0x28, c.ecx)?;
    push(c, g, 0)?;
    c.ecx = at(c.esp, 0x2c);
    set_slot(c, g, 0x34, c.eax)?;
    push(c, g, c.ecx)?;
    c.eax = c.edi;
    store_ss(c, g, at(c.esp, 0x2c), 0)?;
    set_slot(c, g, 0x34, c.edx)?;
    call(points::UNIT_CLAMP_DIRECTION.base, 0xde267)
}

/// `cmp xmm[lo], xmm[v]` against the two clamps: `xmm[v]` becomes the
/// lower clamp (`xmm2`) when that is above it, else the upper (`xmm4`)
/// when it is above that (`comiss`, `jbe`, `movaps`).
fn clamp_step(c: &mut CpuAbi, v: usize) {
    let (lo, x) = (sse::lane0(c, 2), sse::lane0(c, v));
    if !below_or_equal(c, lo, x) {
        sse::movaps(c, v, 2);
        return;
    }
    let hi = sse::lane0(c, 4);
    if !below_or_equal(c, x, hi) {
        sse::movaps(c, v, 4);
    }
}

/// `de27b`: the two dot products, then the first store or the clamped
/// step toward them; the return.
fn smooth(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    sse::load(c, 3, slot(c, g, 0x20)?);
    sse::load(c, 0, slot(c, g, 0x14)?);
    sse::load(c, 1, slot(c, g, 0x10)?);
    sse::load(c, 2, slot(c, g, 0x1c)?);
    let al = g.u8(at(c.ebp, 0x3c8))?;
    c.eax = byte(c.eax, al);
    flags8(c, al);
    sse::ss_reg(c, Ss::Mul, 0, 3);
    sse::ss_reg(c, Ss::Mul, 1, 2);
    sse::ss_reg(c, Ss::Add, 0, 1);
    sse::load(c, 1, slot(c, g, 0x38)?);
    sse::ss_reg(c, Ss::Mul, 1, 3);
    sse::load(c, 3, slot(c, g, 0x34)?);
    sse::ss_reg(c, Ss::Mul, 3, 2);
    sse::ss_reg(c, Ss::Add, 1, 3);
    sse::load(c, 3, slot(c, g, 0x24)?);
    store_ss(c, g, at(c.esp, 0x10), 0)?;
    store_ss(c, g, at(c.esp, 0x14), 1)?;
    store_ss(c, g, at(c.esp, 0x18), 3)?;
    c.ebx = pop(c, g)?;
    if al == 0 {
        // `de381`: the first time, stored as they are.
        c.eax = slot(c, g, 0xc)?;
        c.ecx = slot(c, g, 0x10)?;
        c.edx = at(c.ebp, 0x3cc);
        g.set_u32(c.edx, c.eax)?;
        c.eax = slot(c, g, 0x14)?;
        g.set_u32(at(c.edx, 4), c.ecx)?;
        g.set_u32(at(c.edx, 8), c.eax)?;
        g.set_u8(at(c.ebp, 0x3c8), 1)?;
        return look_ret(c, g);
    }
    sse::ss(c, Ss::Sub, 0, g.u32(at(c.ebp, 0x3cc))?);
    sse::load(c, 2, g.u32(K_45E024)?);
    let (lo, x0) = (sse::lane0(c, 2), sse::lane0(c, 0));
    let not_above = below_or_equal(c, lo, x0);
    sse::load(c, 4, g.u32(K_45DC7C)?);
    if !not_above {
        sse::movaps(c, 0, 2);
    } else {
        let (x0, hi) = (sse::lane0(c, 0), sse::lane0(c, 4));
        if !below_or_equal(c, x0, hi) {
            sse::movaps(c, 0, 4);
        }
    }
    sse::load(c, 5, g.u32(at(c.ebp, 0x3cc))?);
    sse::ss_reg(c, Ss::Add, 5, 0);
    store_ss(c, g, at(c.ebp, 0x3cc), 5)?;
    sse::ss(c, Ss::Sub, 1, g.u32(at(c.ebp, 0x3d0))?);
    clamp_step(c, 1);
    sse::load(c, 0, g.u32(at(c.ebp, 0x3d0))?);
    sse::ss_reg(c, Ss::Add, 0, 1);
    store_ss(c, g, at(c.ebp, 0x3d0), 0)?;
    sse::ss(c, Ss::Sub, 3, g.u32(at(c.ebp, 0x3d4))?);
    sse::movaps(c, 0, 3);
    clamp_step(c, 0);
    sse::ss(c, Ss::Add, 0, g.u32(at(c.ebp, 0x3d4))?);
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    store_ss(c, g, at(c.ebp, 0x3d4), 0)?;
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    Ok(Next::Ret { pop: 0 })
}

/// `de3a2`: `pop edi, esi, ebp`, drop the frame, `ret`.
fn look_ret(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    Ok(Next::Ret { pop: 0 })
}
