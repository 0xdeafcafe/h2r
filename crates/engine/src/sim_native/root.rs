//! Object root transform (`bdc40`): a basis from two axes and their cross
//! product, an optional uniform scale, an optional parent (whose copy may
//! be unscaled or mirrored first), composed through the transform leaf.
//! Arguments: EAX and ECX point at the two axes, BL marks a mirrored
//! parent, the stack holds position, scale, mirror flag, parent and output.
use super::*;
use crate::function_eval::constant::ONE;

const AFTER_SCALE: u32 = 0x000b_de18;
const AFTER_PARENT: u32 = 0x000b_dece;
const AFTER_BASIS: u32 = 0x000b_dee0;

/// `ucomiss; lahf; test ah, 0x44; jnp`: equal and ordered.
fn equal(c: &mut CpuAbi, a: u32, b: u32) -> bool {
    let f = simd::ucomiss(&mut c.mxcsr, a, b).expect("masked SSE exceptions");
    f & 0x44 == 0x40
}

/// `push out; call 142a60` with the parent in ECX and the child in EDX.
fn compose_into(
    c: &mut CpuAbi,
    g: &mut impl Memory,
    out: u32,
    resume: u32,
) -> std::result::Result<(), GuestFault> {
    push(c, g, out)?;
    push(c, g, resume)?;
    super::pose::compose(c, g)?;
    Ok(())
}

pub fn transform(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let entry = c.esp;
    let (a, b) = (c.eax, c.ecx);
    c.esp = entry.wrapping_sub(0x9c);
    c.edx = g.u32(at(c.esp, 0xa0))?;
    let av = [g.u32(a)?, g.u32(at(a, 4))?, g.u32(at(a, 8))?];
    let bv = [g.u32(b)?, g.u32(at(b, 4))?, g.u32(at(b, 8))?];
    let m1 = op(c, Mul, bv[1], av[2]);
    let m2 = op(c, Mul, av[1], bv[2]);
    let m3 = op(c, Mul, av[0], bv[2]);
    let m4 = op(c, Mul, bv[0], av[1]);
    let one = g.u32(ONE)?;
    let saved = c.ebp;
    push(c, g, saved)?;
    c.ebp = g.u32(at(c.esp, 0xb4))?;
    let saved = c.esi;
    push(c, g, saved)?;
    c.esi = g.u32(at(c.esp, 0xb4))?;
    let saved = c.edi;
    push(c, g, saved)?;
    let t = c.esp;
    let (out, parent) = (c.ebp, c.esi);
    let position = c.edx;
    c.edi = g.u32(position)?;
    g.set_u32(at(t, 0x68), c.edi)?;
    c.edi = g.u32(at(position, 4))?;
    c.edx = g.u32(at(position, 8))?;
    g.set_u32(at(t, 0x70), c.edx)?;
    c.edx = b;
    g.set_u32(at(t, 0x6c), c.edi)?;
    c.edi = bv[0];
    let s0 = op(c, Sub, m2, m1);
    let m5 = op(c, Mul, bv[0], av[2]);
    g.set_u32(at(t, 0x78), c.edi)?;
    c.edi = bv[1];
    c.edx = bv[2];
    let s1 = op(c, Sub, m5, m3);
    let m6 = op(c, Mul, bv[1], av[0]);
    c.ecx = av[0];
    g.set_u32(at(t, 0x80), c.edx)?;
    c.edx = av[1];
    c.eax = av[2];
    g.set_u32(at(t, 0x98), c.eax)?;
    let mirror = g.u8(at(t, 0xb4))?;
    c.eax = low(c.eax, mirror);
    let s2 = op(c, Sub, m6, m4);
    for (off, v) in [
        (0x40, one),
        (0x44, one),
        (0x48, 0),
        (0x4c, 0),
        (0x50, 0),
        (0x54, one),
        (0x58, 0),
        (0x5c, 0),
        (0x60, 0),
        (0x64, one),
        (0x74, one),
        (0x7c, c.edi),
        (0x84, s0),
        (0x88, s1),
        (0x8c, s2),
        (0x90, c.ecx),
        (0x94, c.edx),
        (0x9c, 0),
        (0xa0, 0),
        (0xa4, 0),
    ] {
        g.set_u32(at(t, off), v)?;
    }
    if mirror != 0 {
        let n0 = op(c, Sub, 0, s0);
        let n1 = op(c, Sub, 0, s1);
        g.set_u32(at(t, 0x88), n1)?;
        let n2 = op(c, Sub, 0, s2);
        g.set_u32(at(t, 0x84), n0)?;
        g.set_u32(at(t, 0x8c), n2)?;
    }
    let scale = g.u32(at(t, 0xb0))?;
    if !equal(c, scale, one) {
        c.ecx = at(t, 0x74);
        push(c, g, c.ecx)?;
        c.edx = at(c.esp, 0x10);
        for (i, v) in [scale, one, 0, 0, 0, one, 0, 0, 0, one, 0, 0, 0]
            .into_iter()
            .enumerate()
        {
            g.set_u32(at(c.esp, 0x10 + 4 * i as u32), v)?;
        }
        push(c, g, AFTER_SCALE)?;
        super::pose::compose(c, g)?;
    }
    // XORPS and the reload of 1.0 after the scale product.
    let mut basis = at(t, 0x40);
    if parent != 0 {
        let parent_scale = g.u32(parent)?;
        let unit = equal(c, parent_scale, one);
        c.esi = parent;
        if !unit || c.ebx as u8 != 0 {
            // REP MOVSD of the parent into the frame.
            c.ecx = 13;
            c.edi = at(t, 0xc);
            let step = if c.eflags & 0x400 == 0 {
                4
            } else {
                4u32.wrapping_neg()
            };
            while c.ecx != 0 {
                let v = g.u32(c.esi)?;
                g.set_u32(c.edi, v)?;
                c.esi = c.esi.wrapping_add(step);
                c.edi = c.edi.wrapping_add(step);
                c.ecx -= 1;
            }
            let copied = g.u32(at(t, 0xc))?;
            if !equal(c, copied, one) {
                for off in [0x68, 0x6c, 0x70] {
                    let v = op(c, Mul, g.u32(at(t, off))?, copied);
                    g.set_u32(at(t, off), v)?;
                }
                g.set_u32(at(t, 0xc), one)?;
            }
            if c.ebx as u8 != 0 {
                let x = op(c, Sub, 0, g.u32(at(t, 0x1c))?);
                g.set_u32(at(t, 0x1c), x)?;
                let y = op(c, Sub, 0, g.u32(at(t, 0x20))?);
                let z = op(c, Sub, 0, g.u32(at(t, 0x24))?);
                g.set_u32(at(t, 0x20), y)?;
                g.set_u32(at(t, 0x24), z)?;
            }
            c.esi = at(t, 0xc);
        }
        c.edx = at(t, 0x40);
        c.ecx = c.esi;
        compose_into(c, g, out, AFTER_PARENT)?;
        basis = out;
    }
    c.ecx = basis;
    c.edx = at(t, 0x74);
    compose_into(c, g, out, AFTER_BASIS)?;
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    add_flags(c, c.esp, 0x9c);
    c.esp = at(c.esp, 0x9c);
    ret(g, c, 0x14)
}
