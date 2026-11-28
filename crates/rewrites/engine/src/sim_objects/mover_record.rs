//! The early-mover record `109660` (docs/engine/object-update-bodies-core.md,
//! "What `109660` writes").
//!
//! A segment body: the rigid-body transform (`1d08e0`), the inverse and
//! product (`141590`, `142a60`) and the normalise (`30bf0`) stay guest
//! calls. Between them the body runs retail's integer and scalar SSE
//! instructions in retail's order, so every register, the frame slots,
//! `xmm0..3`, MXCSR and the flags the next transfer sees are retail's.
//!
//! Frame, with `esp` after the four pushes (`ebx`, `ebp`, `esi`, `edi`):
//! the found flag byte at `[esp+13]`, the rigid-body counter at
//! `[esp+14]`, its byte offset at `[esp+18]`, the picked rigid body at
//! `[esp+1c]`, the shape tag's definition at `[esp+20]` and the
//! `34`-byte inverse transform at `[esp+24]`. Across the calls `ebx` is
//! the record (`[5107f0] + index × e8`), `ebp` the physics component until
//! `1098fe`, then the record's `+c0` vector; `esi` and `edi` the record's
//! `+7c` and, from `1098f1`, its `+cc` and `+b4` vectors.

use super::sse::{Ss, lane0, load, ss, ss_reg};
use super::{Cx, Next, Result, unowned};
use guest_layout::points;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use runtime::{CpuAbi, Guest};

const NONE: u32 = u32::MAX;
const DF: u32 = 0x400;

/// The early-mover block: 32 records of `e8` bytes, then the list.
const RECORDS: u32 = 0x0051_07f0;
/// The physics components' data array header.
const COMPONENTS: u32 = 0x0051_e9b8;
/// The tag instances (16-byte entries, the definition at `+8`).
const TAGS: u32 = 0x004e_3b44;
/// The zero vector.
const ZERO: u32 = 0x0046_87a4;

/// The rigid body's current transform into `esi`.
const BODY_TRANSFORM: u32 = 0x001d_08e0;
/// The transform inverse: `(in: ecx, out: eax)`.
const INVERSE: u32 = 0x0014_1590;
/// The transform product: `(a: edx, b: ecx, out: stack 0)`, `ret 4`.
const PRODUCT: u32 = 0x0014_2a60;
/// Normalise the vector at `eax`.
const NORMALIZE: u32 = 0x0003_0bf0;

/// After `1d08e0`, `141590`, `142a60`, the three normalises of the basis,
/// the normalise after the Gram–Schmidt step and the last one.
pub(super) const RECORD_SITES: [u32; 8] =
    [0x109806, 0x1098df, 0x1098f1, 0x1098fe, 0x10990b, 0x109918, 0x109985, 0x1099df];

fn at(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

fn ld(g: &Guest, a: u32, off: u32) -> Result<u32> {
    g.u32(at(a, off))
}

/// `109660 (index: ecx, mover: eax)`; `ret`.
pub(super) fn record(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        _ if pc == points::OBJECT_EARLY_MOVER_RECORD.base => entry(c, g),
        0x109806 => velocities(c, g),
        0x1098df => {
            c.ecx = at(c.ebx, 0xb0);
            push(c, g, c.ecx)?;
            c.edx = at(c.esp, 0x28);
            c.ecx = c.esi;
            Ok(Next::Call { target: PRODUCT, site: RECORD_SITES[2] })
        }
        0x1098f1 => {
            c.edi = at(c.ebx, 0xb4);
            c.eax = c.edi;
            Ok(Next::Call { target: NORMALIZE, site: RECORD_SITES[3] })
        }
        0x1098fe => {
            c.ebp = at(c.ebx, 0xc0);
            c.eax = c.ebp;
            Ok(Next::Call { target: NORMALIZE, site: RECORD_SITES[4] })
        }
        0x10990b => {
            c.esi = at(c.ebx, 0xcc);
            c.eax = c.esi;
            Ok(Next::Call { target: NORMALIZE, site: RECORD_SITES[5] })
        }
        0x109918 => {
            orthogonalise(c, g)?;
            c.eax = c.esi;
            Ok(Next::Call { target: NORMALIZE, site: RECORD_SITES[6] })
        }
        0x109985 => {
            cross(c, g)?;
            Ok(Next::Call { target: NORMALIZE, site: RECORD_SITES[7] })
        }
        0x1099df => done(c, g),
        _ => Err(unowned(pc)),
    }
}

/// `109660..1097fc`: the record, the component, the picked rigid body,
/// the previous block, then the call to `1d08e0`.
fn entry(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.esp = c.esp.wrapping_sub(0x48);
    push(c, g, c.ebx)?;
    c.ebx = c.ecx;
    c.ecx = g.u32(points::OBJECTS.base)?;
    c.ebx = c.ebx.wrapping_mul(0xe8);
    c.edx = ld(g, c.ecx, guest_layout::datum::layout::DATA)?;
    c.eax &= 0xffff;
    push(c, g, c.ebp)?;
    c.eax *= 3;
    c.ecx = g.u32(c.edx.wrapping_add(c.eax * 4).wrapping_add(8))?;
    c.eax = ld(g, c.ecx, 0xb4)?;
    push(c, g, c.esi)?;
    c.ebx = c.ebx.wrapping_add(g.u32(RECORDS)?);
    push(c, g, c.edi)?;
    if c.eax == NONE {
        return none(c, g);
    }
    // The component and the physics model's shape definition.
    c.eax &= 0xffff;
    c.ebp = c.eax * 5;
    c.eax = g.u32(COMPONENTS)?;
    c.edx = ld(g, c.eax, 0x44)?;
    c.ebp <<= 5;
    c.ebp = c.ebp.wrapping_add(c.edx);
    c.edx = g.u32(c.ecx)?;
    c.ecx = g.u32(TAGS)?;
    c.edx = (c.edx & 0xffff) << 4;
    c.eax = g.u32(c.edx.wrapping_add(c.ecx).wrapping_add(8))?;
    c.edx = ld(g, c.eax, 0x38)?;
    c.edx = (c.edx & 0xffff) << 4;
    c.eax = g.u32(c.edx.wrapping_add(c.ecx).wrapping_add(8))?;
    c.eax = ld(g, c.eax, 0x24)?;
    if c.eax == NONE {
        return none(c, g);
    }
    c.eax = (c.eax & 0xffff) << 4;
    c.eax = g.u32(c.eax.wrapping_add(c.ecx).wrapping_add(8))?;
    g.set_u32(at(c.esp, 0x20), c.eax)?;
    // The component's default rigid body, when it is in range.
    let first = g.u8(at(c.ebp, 0x18))?;
    c.eax = byte(c.eax, first);
    let mut valid = false;
    if (first as i8) >= 0 {
        c.edx = ld(g, c.ebp, 0x74)?;
        c.ecx = first as i8 as i32 as u32;
        if (c.ecx as i32) < c.edx as i32 {
            c.eax = (c.eax & 0xffff_0000) | u32::from(first as i8 as i16 as u16);
            valid = true;
        }
    }
    if !valid {
        c.eax = NONE;
    }
    c.ecx = ld(g, c.ebp, 0x74)?;
    c.edx = c.eax as u16 as i16 as i32 as u32;
    c.eax = 0;
    g.set_u32(at(c.esp, 0x1c), c.edx)?;
    g.set_u8(at(c.esp, 0x13), 0)?;
    g.set_u32(at(c.esp, 0x14), c.eax)?;
    if c.ecx as i32 > 0 {
        g.set_u32(at(c.esp, 0x18), c.eax)?;
        pick(c, g)?;
    }
    c.eax = g.u32(at(c.esp, 0x1c))?;
    if c.eax == NONE {
        return none(c, g);
    }
    // `109797`: copy the current block to the previous one.
    let cl = g.u8(at(c.ebx, 0xe5))?;
    c.ecx = byte(c.ecx, cl);
    logic(c, cl.into(), 0x80);
    if cl != 0 {
        c.esi = at(c.ebx, 0x7c);
        c.edi = at(c.ebx, 0x48);
        c.ecx = 0xd;
        movsd(c, g)?;
        c.edx = at(c.ebx, 0x24);
        c.esi = g.u32(c.edx)?;
        c.ecx = at(c.ebx, 0x18);
        g.set_u32(c.ecx, c.esi)?;
        c.esi = ld(g, c.edx, 4)?;
        c.edx = ld(g, c.edx, 8)?;
        g.set_u32(at(c.ecx, 4), c.esi)?;
        g.set_u32(at(c.ecx, 8), c.edx)?;
        c.ecx = at(c.ebx, 0x3c);
        c.esi = g.u32(c.ecx)?;
        c.edx = at(c.ebx, 0x30);
        g.set_u32(c.edx, c.esi)?;
        c.esi = ld(g, c.ecx, 4)?;
        c.ecx = ld(g, c.ecx, 8)?;
        g.set_u32(at(c.edx, 4), c.esi)?;
        g.set_u32(at(c.edx, 8), c.ecx)?;
        c.edx = at(c.ebx, 0xc);
        c.esi = g.u32(c.edx)?;
        c.ecx = c.ebx;
        g.set_u32(c.ecx, c.esi)?;
        c.esi = ld(g, c.edx, 4)?;
        c.edx = ld(g, c.edx, 8)?;
        g.set_u32(at(c.ecx, 4), c.esi)?;
        g.set_u32(at(c.ecx, 8), c.edx)?;
        g.set_u8(at(c.ebx, 0xe4), 1)?;
    }
    c.esi = at(c.ebx, 0x7c);
    c.ecx = c.ebp;
    Ok(Next::Call { target: BODY_TRANSFORM, site: RECORD_SITES[0] })
}

/// `109722..109790`: the first rigid body with a shape whose flags byte
/// `+18` has bit 4 set.
fn pick(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    loop {
        let found = g.u8(at(c.esp, 0x13))?;
        c.eax = byte(c.eax, found);
        if found != 0 {
            return Ok(());
        }
        c.edx = g.u32(at(c.esp, 0x18))?;
        c.eax = ld(g, c.ebp, 0x70)?;
        c.eax = c.eax.wrapping_add(c.edx);
        c.edx = ld(g, c.eax, 0x4c)?;
        c.ecx = 0;
        if c.edx as i32 > 0 {
            c.esi = g.u32(at(c.esp, 0x20))?;
            c.esi = ld(g, c.esi, 0x3c)?;
            c.edi = ld(g, c.eax, 0x48)?;
            loop {
                let shape = g.u8(at(c.edi, c.ecx))? as i8 as i32 as u32;
                c.eax = shape.wrapping_mul(9) << 4;
                if g.u8(c.eax.wrapping_add(c.esi).wrapping_add(0x18))? & 0x10 != 0 {
                    c.ecx = g.u32(at(c.esp, 0x14))?;
                    g.set_u32(at(c.esp, 0x1c), c.ecx)?;
                    g.set_u8(at(c.esp, 0x13), 1)?;
                    break;
                }
                c.ecx = c.ecx.wrapping_add(1);
                if (c.ecx as i32) >= c.edx as i32 {
                    break;
                }
            }
        }
        c.eax = g.u32(at(c.esp, 0x14))?;
        c.edx = g.u32(at(c.esp, 0x18))?;
        c.ecx = ld(g, c.ebp, 0x74)?;
        c.eax = c.eax.wrapping_add(1);
        c.edx = c.edx.wrapping_add(0x60);
        g.set_u32(at(c.esp, 0x14), c.eax)?;
        g.set_u32(at(c.esp, 0x18), c.edx)?;
        if (c.eax as i32) >= c.ecx as i32 {
            return Ok(());
        }
    }
}

/// `rep movsd`, honouring DF.
fn movsd(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let step = if c.eflags & DF != 0 { 4u32.wrapping_neg() } else { 4 };
    while c.ecx != 0 {
        let v = g.u32(c.esi)?;
        g.set_u32(c.edi, v)?;
        c.esi = c.esi.wrapping_add(step);
        c.edi = c.edi.wrapping_add(step);
        c.ecx -= 1;
    }
    Ok(())
}

/// `109806..1098da`: the two velocity vectors (`[4687a4]` when the rigid
/// body's `+40` byte is set) and the vector at `+0c`; then, with a
/// previous transform, the call to `141590`.
fn velocities(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = g.u32(at(c.esp, 0x1c))?;
    c.ecx = ld(g, c.ebp, 0x70)?;
    c.eax = c.eax.wrapping_mul(3) << 5;
    c.ecx = g.u32(c.eax.wrapping_add(c.ecx).wrapping_add(0x40))?;
    let dl = g.u8(at(c.ecx, 0x40))?;
    c.edx = byte(c.edx, dl);
    c.edx = g.u32(ZERO)?;
    if dl == 0 {
        c.ecx = ld(g, c.ecx, 0x3c)?;
        load(c, 0, ld(g, c.ecx, 0x48)?);
        load(c, 1, ld(g, c.ecx, 0x44)?);
        c.ecx = at(c.ecx, 0x40);
        c.ecx = g.u32(c.ecx)?;
        g.set_u32(at(c.ebx, 0x24), c.ecx)?;
        g.set_u32(at(c.ebx, 0x28), lane0(c, 1))?;
        g.set_u32(at(c.ebx, 0x2c), lane0(c, 0))?;
    } else {
        c.ecx = c.edx;
        c.edi = g.u32(c.ecx)?;
        g.set_u32(at(c.ebx, 0x24), c.edi)?;
        c.edi = ld(g, c.ecx, 4)?;
        g.set_u32(at(c.ebx, 0x28), c.edi)?;
        c.ecx = ld(g, c.ecx, 8)?;
        g.set_u32(at(c.ebx, 0x2c), c.ecx)?;
    }
    c.ecx = ld(g, c.ebp, 0x70)?;
    c.ecx = g.u32(c.eax.wrapping_add(c.ecx).wrapping_add(0x40))?;
    if g.u8(at(c.ecx, 0x40))? == 0 {
        c.ecx = ld(g, c.ecx, 0x3c)?;
        c.edx = ld(g, c.ecx, 0x50)?;
        load(c, 0, ld(g, c.ecx, 0x58)?);
        load(c, 1, ld(g, c.ecx, 0x54)?);
        c.ecx = at(c.ecx, 0x50);
        g.set_u32(at(c.ebx, 0x3c), c.edx)?;
        g.set_u32(at(c.ebx, 0x40), lane0(c, 1))?;
        g.set_u32(at(c.ebx, 0x44), lane0(c, 0))?;
    } else {
        c.ecx = g.u32(c.edx)?;
        g.set_u32(at(c.ebx, 0x3c), c.ecx)?;
        c.ecx = ld(g, c.edx, 4)?;
        g.set_u32(at(c.ebx, 0x40), c.ecx)?;
        c.edx = ld(g, c.edx, 8)?;
        g.set_u32(at(c.ebx, 0x44), c.edx)?;
    }
    c.ecx = ld(g, c.ebp, 0x70)?;
    c.edx = g.u32(c.eax.wrapping_add(c.ecx).wrapping_add(0x40))?;
    c.eax = ld(g, c.edx, 0x3c)?;
    load(c, 0, ld(g, c.eax, 0x78)?);
    load(c, 1, ld(g, c.eax, 0x74)?);
    c.eax = at(c.eax, 0x70);
    c.eax = g.u32(c.eax)?;
    g.set_u32(at(c.ebx, 0xc), c.eax)?;
    g.set_u32(at(c.ebx, 0x10), lane0(c, 1))?;
    g.set_u32(at(c.ebx, 0x14), lane0(c, 0))?;
    let al = g.u8(at(c.ebx, 0xe4))?;
    c.eax = byte(c.eax, al);
    logic(c, al.into(), 0x80);
    g.set_u8(at(c.ebx, 0xe5), 1)?;
    if al == 0 {
        return done(c, g);
    }
    c.ecx = at(c.ebx, 0x48);
    c.eax = at(c.esp, 0x24);
    Ok(Next::Call { target: INVERSE, site: RECORD_SITES[1] })
}

/// `109918..10997b`: `up -= forward × (up · forward)` (`esi` up, `edi`
/// forward), stored back into `esi`.
fn orthogonalise(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let (s, d) = (c.esi, c.edi);
    load(c, 0, ld(g, s, 8)?);
    ss(c, Ss::Mul, 0, ld(g, d, 8)?);
    load(c, 1, ld(g, s, 4)?);
    ss(c, Ss::Mul, 1, ld(g, d, 4)?);
    load(c, 2, ld(g, d, 4)?);
    load(c, 3, ld(g, d, 8)?);
    ss_reg(c, Ss::Add, 0, 1);
    load(c, 1, g.u32(d)?);
    ss(c, Ss::Mul, 1, g.u32(s)?);
    ss_reg(c, Ss::Add, 0, 1);
    load(c, 1, g.u32(d)?);
    ss_reg(c, Ss::Mul, 1, 0);
    ss_reg(c, Ss::Mul, 2, 0);
    ss_reg(c, Ss::Mul, 3, 0);
    load(c, 0, g.u32(s)?);
    ss_reg(c, Ss::Sub, 0, 1);
    g.set_u32(s, lane0(c, 0))?;
    load(c, 0, ld(g, s, 4)?);
    ss_reg(c, Ss::Sub, 0, 2);
    g.set_u32(at(s, 4), lane0(c, 0))?;
    load(c, 0, ld(g, s, 8)?);
    ss_reg(c, Ss::Sub, 0, 3);
    g.set_u32(at(s, 8), lane0(c, 0))?;
    Ok(())
}

/// `109985..1099d5`: the record's `+c0` vector (`ebp`) from the cross
/// product of `+cc` (`esi`) and `+b4` (`edi`); `eax` = `ebp`.
fn cross(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let (s, d) = (c.esi, c.edi);
    load(c, 0, ld(g, d, 4)?);
    load(c, 1, g.u32(d)?);
    ss(c, Ss::Mul, 1, ld(g, s, 4)?);
    ss(c, Ss::Mul, 0, g.u32(s)?);
    load(c, 2, ld(g, d, 8)?);
    ss(c, Ss::Mul, 2, g.u32(s)?);
    load(c, 3, ld(g, s, 8)?);
    ss(c, Ss::Mul, 3, ld(g, d, 4)?);
    ss_reg(c, Ss::Sub, 0, 1);
    load(c, 1, g.u32(d)?);
    ss(c, Ss::Mul, 1, ld(g, s, 8)?);
    ss_reg(c, Ss::Sub, 1, 2);
    load(c, 2, ld(g, s, 4)?);
    ss(c, Ss::Mul, 2, ld(g, d, 8)?);
    ss_reg(c, Ss::Sub, 2, 3);
    c.eax = c.ebp;
    g.set_u32(c.ebp, lane0(c, 2))?;
    g.set_u32(at(c.ebp, 4), lane0(c, 1))?;
    g.set_u32(at(c.ebp, 8), lane0(c, 0))?;
    Ok(())
}

/// `1099df`: restore, drop the frame, `ret`.
fn done(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x48, false);
    Ok(Next::Ret { pop: 0 })
}

/// `1099e7`: no component, no shape definition or no rigid body: clear
/// `+e5` and `+e4`, restore, `ret`.
fn none(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    g.set_u8(at(c.ebx, 0xe5), 0)?;
    g.set_u8(at(c.ebx, 0xe4), 0)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x48, false);
    Ok(Next::Ret { pop: 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const D: u32 = 0x0057_0000;
    const STACK: u32 = 0x0057_f000;
    const HEADERS: u32 = D + 0x100;
    const BODY: u32 = D + 0x200;
    const COMPONENT: u32 = D + 0x400 + 3 * 0xa0;
    const TAG_ENTRIES: u32 = D + 0x700;
    const SHAPES: u32 = D + 0xa00;
    const SHAPE_ARRAY: u32 = D + 0xb00;
    const RIGID: u32 = D + 0x2000;
    const BLOCK: u32 = D + 0x4000;
    const REC: u32 = BLOCK + 0xe8;
    const ENTRY: u32 = points::OBJECT_EARLY_MOVER_RECORD.base;

    /// A mover (handle index 2) whose body names component `component`.
    fn guest(component: u32) -> (CpuAbi, Guest) {
        let mut g = Guest::new();
        g.map_zeroed(0x0046_8000, 0x1000, Permissions::RW).unwrap();
        g.map_zeroed(0x004e_0000, 0x4000, Permissions::RW).unwrap();
        g.map_zeroed(0x0051_0000, 0x10000, Permissions::RW).unwrap();
        g.map_zeroed(D, 0x10000, Permissions::RW).unwrap();
        g.set_u32(points::OBJECTS.base, D).unwrap();
        g.set_u32(D + guest_layout::datum::layout::DATA, HEADERS).unwrap();
        g.set_u32(HEADERS + 2 * 12 + 8, BODY).unwrap();
        g.set_u32(BODY, 1).unwrap();
        g.set_u32(BODY + 0xb4, component).unwrap();
        g.set_u32(RECORDS, BLOCK).unwrap();
        g.set_u32(COMPONENTS, D + 0x300).unwrap();
        g.set_u32(D + 0x300 + 0x44, D + 0x400).unwrap();
        g.set_u32(TAGS, TAG_ENTRIES).unwrap();
        // Definition (tag 1) -> physics model (tag 2) -> shapes (tag 3).
        g.set_u32(TAG_ENTRIES + 0x18, D + 0x800).unwrap();
        g.set_u32(D + 0x800 + 0x38, 2).unwrap();
        g.set_u32(TAG_ENTRIES + 0x28, D + 0x900).unwrap();
        g.set_u32(D + 0x900 + 0x24, 3).unwrap();
        g.set_u32(TAG_ENTRIES + 0x38, SHAPES).unwrap();
        g.set_u32(SHAPES + 0x3c, SHAPE_ARRAY).unwrap();
        g.set_u8(SHAPE_ARRAY + 0x90 + 0x18, 0x10).unwrap();
        // No default rigid body; two rigid bodies, the second with shape 1.
        g.set_u8(COMPONENT + 0x18, 0xff).unwrap();
        g.set_u32(COMPONENT + 0x70, RIGID).unwrap();
        g.set_u32(COMPONENT + 0x74, 2).unwrap();
        g.set_u32(RIGID + 0x48, D + 0x3000).unwrap();
        g.set_u32(RIGID + 0x4c, 1).unwrap();
        g.set_u32(RIGID + 0x60 + 0x48, D + 0x3010).unwrap();
        g.set_u32(RIGID + 0x60 + 0x4c, 2).unwrap();
        g.set_u8(D + 0x3011, 1).unwrap();
        let cpu = CpuAbi {
            esp: STACK,
            eax: 0x1234_0002,
            ecx: 1,
            ebx: 0xb0b0,
            ebp: 0xe0e0,
            esi: 0x5151,
            edi: 0xd1d1,
            ..Default::default()
        };
        (cpu, g)
    }

    #[test]
    fn a_mover_with_no_physics_component_clears_its_record_and_returns() {
        let (mut cpu, mut g) = guest(NONE);
        g.set_u8(REC + 0xe4, 1).unwrap();
        g.set_u8(REC + 0xe5, 1).unwrap();
        let next = record(&mut Cx { cpu: &mut cpu, g: &mut g }, ENTRY).unwrap();
        assert_eq!(next, Next::Ret { pop: 0 });
        assert_eq!((g.u8(REC + 0xe4).unwrap(), g.u8(REC + 0xe5).unwrap()), (0, 0));
        assert_eq!((cpu.eax, cpu.ecx, cpu.edx), (NONE, BODY, HEADERS));
        assert_eq!((cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (0xb0b0, 0xe0e0, 0x5151, 0xd1d1));
        assert_eq!(cpu.esp, STACK);
        // `add esp, 48h`: no carry, not zero.
        assert_eq!(cpu.eflags & 0x41, 0);
    }

    #[test]
    fn a_mover_with_a_flagged_body_keeps_its_last_block_and_reads_the_transform() {
        let (mut cpu, mut g) = guest(3);
        g.set_u8(REC + 0xe5, 1).unwrap();
        for i in 0..13 {
            g.set_u32(REC + 0x7c + 4 * i, 0x100 + i).unwrap();
        }
        g.set_u32(REC + 0x14, 0x77).unwrap();
        let next = record(&mut Cx { cpu: &mut cpu, g: &mut g }, ENTRY).unwrap();
        assert_eq!(next, Next::Call { target: BODY_TRANSFORM, site: 0x109806 });
        let esp = STACK - 0x58;
        assert_eq!(cpu.esp, esp);
        assert_eq!((g.u32(esp + 0x1c).unwrap(), g.u32(esp + 0x14).unwrap()), (1, 2));
        assert_eq!((g.u32(esp + 0x18).unwrap(), g.u32(esp + 0x20).unwrap()), (0xc0, SHAPES));
        assert_eq!(g.u32(esp + 0xc).unwrap(), 0xb0b0);
        for i in 0..13 {
            assert_eq!(g.u32(REC + 0x48 + 4 * i).unwrap(), 0x100 + i);
        }
        assert_eq!(g.u32(REC + 8).unwrap(), 0x77);
        assert_eq!(g.u8(REC + 0xe4).unwrap(), 1);
        assert_eq!((cpu.eax, cpu.ebx, cpu.ecx, cpu.ebp), (1, REC, COMPONENT, COMPONENT));
        assert_eq!((cpu.esi, cpu.edi, cpu.edx), (REC + 0x7c, REC + 0x7c, 0x77));
        // `test cl, cl` on the set `+e5`.
        assert_eq!(cpu.eflags & 0x40, 0);
    }

    #[test]
    fn the_velocities_come_from_the_rigid_body_state_then_the_inverse_is_called() {
        let (mut cpu, mut g) = guest(3);
        let rb = D + 0x5000;
        let state = D + 0x5100;
        g.set_u32(RIGID + 0x60 + 0x40, rb).unwrap();
        g.set_u32(rb + 0x3c, state).unwrap();
        for (off, v) in [(0x40, 1.0f32), (0x44, 2.0), (0x48, 3.0), (0x50, 4.0), (0x54, 5.0), (0x58, 6.0)] {
            g.set_u32(state + off, v.to_bits()).unwrap();
        }
        for (off, v) in [(0x70, 7.0f32), (0x74, 8.0), (0x78, 9.0)] {
            g.set_u32(state + off, v.to_bits()).unwrap();
        }
        g.set_u8(REC + 0xe4, 1).unwrap();
        record(&mut Cx { cpu: &mut cpu, g: &mut g }, ENTRY).unwrap();
        let next = record(&mut Cx { cpu: &mut cpu, g: &mut g }, 0x109806).unwrap();
        assert_eq!(next, Next::Call { target: INVERSE, site: 0x1098df });
        let f = |a: u32| f32::from_bits(g.u32(a).unwrap());
        assert_eq!([f(REC + 0x24), f(REC + 0x28), f(REC + 0x2c)], [1.0, 2.0, 3.0]);
        assert_eq!([f(REC + 0x3c), f(REC + 0x40), f(REC + 0x44)], [4.0, 5.0, 6.0]);
        assert_eq!([f(REC + 0xc), f(REC + 0x10), f(REC + 0x14)], [7.0, 8.0, 9.0]);
        assert_eq!(g.u8(REC + 0xe5).unwrap(), 1);
        assert_eq!((lane0(&cpu, 0), lane0(&cpu, 1)), (9.0f32.to_bits(), 8.0f32.to_bits()));
        assert_eq!((cpu.ecx, cpu.eax, cpu.edx), (REC + 0x48, STACK - 0x58 + 0x24, rb));
        let next = record(&mut Cx { cpu: &mut cpu, g: &mut g }, 0x1098df).unwrap();
        assert_eq!(next, Next::Call { target: PRODUCT, site: 0x1098f1 });
        assert_eq!((cpu.esp, cpu.ecx, cpu.edx), (STACK - 0x5c, REC + 0x7c, STACK - 0x58 + 0x24));
        assert_eq!(g.u32(cpu.esp).unwrap(), REC + 0xb0);
    }
}
