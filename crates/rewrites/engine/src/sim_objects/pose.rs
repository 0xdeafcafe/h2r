//! The node matrices `bd090` (slice 4 of docs/engine/sim-rewrite-objects.md;
//! contract in docs/engine/object-update.md, "Slice 4 bodies"), shared
//! with phase 1b of docs/engine/sim-rewrite.md.
//!
//! Phase 1 owns the arithmetic, this phase the orchestration (design
//! section 6): the body is the register-level port
//! `sim_physics::node_matrices` taken into the body table unchanged. It
//! selects the orientation block, makes the animation callouts in retail
//! order (`e1670`, `1d3920`, `b8bd0`, `16d940`, `bf5a0`, `bfc90`, `bff10`,
//! `bfe80`, `bfed0`, `bdb60`, `bfe20`, `b6780`, `1c6440`, `a98b0`,
//! `a9940`), builds the root matrix, calls the after-root dispatcher
//! `1090d0` (a slice 1 body, so [`super::drive`] fuses it), and walks the
//! skeleton. The root transform `bdc40` and the root's pose and
//! composition run inline through `sim_native`'s exact bodies. The
//! skeleton walk is phase 1b's: `sim_anim::build_matrices` through
//! `crate::sim_anim::skeleton`, with the
//! port's per-node loop as its fallback (docs/engine/skeleton-walk.md).
//!
//! The port makes its transfers through the runtime's stack helpers
//! (`Guest::call` pushes the return address, `Guest::ret` pops it and the
//! argument); [`node_matrices`] hands them to [`super::drive`] instead,
//! which makes the same push and pop at the same address.

use super::{Cx, Next, Result, unowned};
use guest_layout::points;
use crate::sim_physics::node_matrices::{SITES, update};
use runtime::abi;

/// The sites of `bd090`'s guest calls, as the port lists them.
pub(super) const POSE_SITES: [u32; 16] = SITES;

/// `bd090 (object: stack 0)`; `ret 4`.
pub(super) fn node_matrices(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    if pc != points::OBJECT_NODE_MATRICES.base && !SITES.contains(&pc) {
        return Err(unowned(pc));
    }
    let t = update(c, g, pc)?;
    match t.kind {
        abi::CALL => {
            // `Guest::call` pushed the return address; `drive` pushes it.
            c.esp = c.esp.wrapping_add(4);
            Ok(Next::Call { target: t.target_pc, site: t.resume_pc })
        }
        abi::RETURN => {
            // `Guest::ret` popped the return address and the argument;
            // `drive` pops both.
            c.esp = c.esp.wrapping_sub(8);
            Ok(Next::Ret { pop: 4 })
        }
        _ => Err(unowned(pc)),
    }
}
