//! The projectile update `f8de0` (slice 11 of docs/engine/sim-rewrite-objects.md;
//! contract in docs/engine/object-update.md, "Slice 11 bodies"): the `proj`
//! descriptor's slot `+40`. The projectile step it calls, `f8eb0`, is a
//! body of its own (`projectile_step.rs`), so `drive` fuses the call.
//!
//! **`f8de0 (object: stack 0) -> al`; `ret 4`.** Always returns 1.
//! Frame: `sub esp, 18`, then `ebx`, `esi`, `edi` pushed; with `S` that
//! `esp`, `[S+0c..+18]` a copy of the velocity (body `+88`), `[S+18..+24]`
//! the early mover's displacement `109a00` writes, the object at `[S+28]`.
//! `edi` holds the object and `esi` body `+88` across both calls.
//!
//! Unless body `+c1` bit 0 is set and body `+12c` bit 14 is clear, it asks
//! the early mover that woke the projectile for its displacement
//! (`109a00(object, &S[18], flag)` with `ebx` = `&S[0c]`, `ret 0c`; the
//! flag is body `+12c` bit 15, stored first over the low byte of the
//! object argument slot and pushed as that whole dword). It then runs the
//! step (`f8eb0(object, &S[0c])`, `ret 8`) and, when `109a00` answered
//! non-zero, adds the displacement to the velocity at body `+88` with
//! three `movss`/`addss`/`movss` triples through `xmm0`.
//!
//! Exit: `al` 1 over `f8eb0`'s `eax`; `ecx`, `edx` as `f8eb0` left them;
//! `xmm0` the last sum when the displacement was added; flags from `add
//! esp, 18`.

use super::sse::{self, Ss};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};

const FRAME: u32 = 0x18;
/// The early-mover reader (docs/engine/object-update.md, "Who reads it,
/// and when"): the displacement of the mover that woke an object.
const MOVER_DISPLACEMENT: u32 = 0x0010_9a00;
/// The projectile step (`projectile_step.rs`).
const STEP: u32 = points::PROJECTILE_STEP.base;
const VELOCITY: u32 = 0x88;

/// After the displacement and after the step.
pub(super) const PROJECTILE_SITES: [u32; 2] = [0xf8e57, 0xf8e6c];

/// `f8e61`: the step, with `bl` saying whether to add the displacement.
fn step(c: &mut runtime::CpuAbi, g: &mut runtime::Guest) -> Result<Next> {
    c.eax = c.esp.wrapping_add(0xc);
    push(c, g, c.eax)?;
    push(c, g, c.edi)?;
    Ok(Next::Call { target: STEP, site: PROJECTILE_SITES[1] })
}

/// `f8de0 (object: stack 0) -> al`; `ret 4`.
pub(super) fn projectile_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        p if p == points::PROJECTILE_UPDATE.base => {
            c.esp = arithmetic(c, c.esp, FRAME, true);
            c.ecx = g.u32(points::OBJECTS.base)?;
            c.edx = g.u32(c.ecx.wrapping_add(DATA))?;
            push(c, g, c.ebx)?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.edi = g.u32(c.esp.wrapping_add(0x28))?;
            c.eax = (c.edi & 0xffff) * 3;
            c.eax = g.u32(c.edx.wrapping_add(c.eax * 4 + 8))?;
            c.esi = c.eax.wrapping_add(VELOCITY);
            c.ecx = c.esi;
            c.edx = g.u32(c.ecx)?;
            g.set_u32(c.esp.wrapping_add(0xc), c.edx)?;
            c.edx = g.u32(c.ecx.wrapping_add(4))?;
            c.ecx = g.u32(c.ecx.wrapping_add(8))?;
            g.set_u32(c.esp.wrapping_add(0x14), c.ecx)?;
            let latched = g.u8(c.eax.wrapping_add(0xc1))? & 1;
            logic(c, latched.into(), 0x80);
            g.set_u32(c.esp.wrapping_add(0x10), c.edx)?;
            if latched != 0 {
                c.edx = super::alu::shr(c, g.u32(c.eax.wrapping_add(0x12c))?, 14, 32);
                logic(c, c.edx & 1, 0x80);
                if c.edx & 1 == 0 {
                    // `f8e5f`.
                    c.ebx = byte(c.ebx, 0);
                    logic(c, 0, 0x80);
                    return step(c, g);
                }
            }
            // `f8e34`.
            c.eax = g.u32(c.eax.wrapping_add(0x12c))? >> 15;
            c.eax = byte(c.eax, c.eax as u8 & 1);
            logic(c, c.eax & 1, 0x80);
            g.set_u8(c.esp.wrapping_add(0x28), c.eax as u8)?;
            c.ecx = g.u32(c.esp.wrapping_add(0x28))?;
            push(c, g, c.ecx)?;
            c.edx = c.esp.wrapping_add(0x1c);
            push(c, g, c.edx)?;
            push(c, g, c.edi)?;
            c.ebx = c.esp.wrapping_add(0x18);
            Ok(Next::Call { target: MOVER_DISPLACEMENT, site: PROJECTILE_SITES[0] })
        }
        p if p == PROJECTILE_SITES[0] => {
            logic(c, c.eax & 0xff, 0x80);
            c.ebx = byte(c.ebx, u8::from(c.eax as u8 != 0));
            if c.eax as u8 == 0 {
                // `xor bl, bl`.
                logic(c, 0, 0x80);
            }
            step(c, g)
        }
        p if p == PROJECTILE_SITES[1] => {
            logic(c, c.ebx & 0xff, 0x80);
            c.eax = byte(c.eax, 1);
            if c.ebx as u8 != 0 {
                for i in 0..3u32 {
                    let at = c.esi.wrapping_add(4 * i);
                    sse::load(c, 0, g.u32(at)?);
                    let d = g.u32(c.esp.wrapping_add(0x18 + 4 * i))?;
                    let sum = sse::ss(c, Ss::Add, 0, d);
                    g.set_u32(at, sum)?;
                }
            }
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            c.ebx = pop(c, g)?;
            c.esp = arithmetic(c, c.esp, FRAME, false);
            Ok(Next::Ret { pop: 4 })
        }
        _ => Err(unowned(pc)),
    }
}
