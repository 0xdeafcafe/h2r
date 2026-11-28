//! The crate update `11bce0` (slice 11 of docs/engine/sim-rewrite-objects.md;
//! contract in docs/engine/object-update.md, "Slice 11 bodies"): the `bloc`
//! descriptor's slot `+40`.
//!
//! **`11bce0 (object: stack 0) -> al`; `ret 4`.** No frame beyond `push
//! esi`, which holds the body across the one call. When body `+c0` bit 12
//! is set and the crate has a physics component (body `+b4`), it calls
//! `1d24a0(component record, 1.0)` (`ret 8`) and the result is 1. When
//! body `+c1` bit 0 is clear, it stores the tick count (game time `+8`) at
//! body `+bc` and the result is 1. Otherwise the result is 0.
//!
//! Exit: `al` the result, the rest of `eax` the header offset (`(object &
//! ffff) * 3`) or the component array, as the path left it; `ecx` and
//! `edx` as the last load left them; flags from `test byte [esi+c1], 1`.

use super::alu::{shl, shr, test8};
use super::{Cx, Next, Result, unowned};
use guest_layout::datum::layout::DATA;
use guest_layout::points;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};

const NONE: u32 = u32::MAX;
/// `1.0f`, the second argument of the component call.
const ONE: u32 = 0x3f80_0000;
/// The component call (O: what it does to the component).
const COMPONENT_CALL: u32 = 0x001d_24a0;

pub(super) const CRATE_SITES: [u32; 1] = [0x11bd39];

/// `11bce0 (object: stack 0) -> al`; `ret 4`.
pub(super) fn crate_update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc == points::CRATE_UPDATE.base {
        c.eax = g.u32(c.esp.wrapping_add(4))?;
        c.ecx = g.u32(points::OBJECTS.base)?;
        c.edx = g.u32(c.ecx.wrapping_add(DATA))?;
        c.eax &= 0xffff;
        c.eax *= 3;
        push(c, g, c.esi)?;
        c.esi = g.u32(c.edx.wrapping_add(c.eax * 4 + 8))?;
        // `xor ecx, ecx; mov cx, [esi+c0]`.
        c.ecx = u32::from(g.u16(c.esi.wrapping_add(0xc0))?);
        c.eax = byte(c.eax, 0);
        c.ecx = shr(c, c.ecx, 12, 32);
        logic(c, c.ecx & 1, 0x80);
        if c.ecx & 1 != 0 {
            c.ecx = g.u32(c.esi.wrapping_add(0xb4))?;
            arithmetic(c, c.ecx, NONE, true);
            if c.ecx != NONE {
                c.eax = g.u32(points::PHYSICS_COMPONENTS.base)?;
                c.ecx &= 0xffff;
                c.edx = c.ecx * 5;
                c.ecx = g.u32(c.eax.wrapping_add(DATA))?;
                c.edx = shl(c, c.edx, 5, 32);
                push(c, g, ONE)?;
                c.edx = arithmetic(c, c.edx, c.ecx, false);
                push(c, g, c.edx)?;
                return Ok(Next::Call { target: COMPONENT_CALL, site: CRATE_SITES[0] });
            }
        }
    } else if pc == CRATE_SITES[0] {
        c.eax = byte(c.eax, 1);
    } else {
        return Err(unowned(pc));
    }
    // `11bd3b`.
    if !test8(c, g, c.esi.wrapping_add(0xc1), 1)? {
        c.ecx = g.u32(points::GAME_TIME.base)?;
        c.edx = g.u32(c.ecx.wrapping_add(8))?;
        g.set_u32(c.esi.wrapping_add(0xbc), c.edx)?;
        c.eax = byte(c.eax, 1);
    }
    c.esi = pop(c, g)?;
    Ok(Next::Ret { pop: 4 })
}
