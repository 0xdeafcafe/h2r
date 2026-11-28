//! Entry I, `0x00301800`: the general MOPP ray (docs/engine/collision-queries.md,
//! "Entry I"), answered in Rust by [`CollectorCast`].
//!
//! The setup and the float interpreter `0x003009e0` are the kernel walk
//! (`sim_collision::mopp::ray`). Each terminal's callbacks (the optional
//! filter, the collection's child method and the child shape's ray
//! method) are guest calls the installer makes between steps of the walk
//! (`sim_bridge::sim_native`), at the stack depth the interpreter would
//! have called them from.
mod collector;
pub use collector::CollectorCast;
use collector::Setup;

use super::Read;
use super::layout::{self, mopp_ray as l};
use super::preflight::CollisionMemory;
use runtime::CpuAbi;
use sim_collision::mopp::Tree;
use sim_collision::mopp::ray::{Constants, Precision, Ray, RayWalk, Vec4};
use sim_collision::{Bail, CollisionWorld, GuestRead};
use sim_core::det;
use std::sync::Arc;

/// x87 registers the interpreter's sequences need free.
const X87_FREE: u32 = 3;

/// A decoded program's bytecode, shared with the world's cache.
#[derive(Clone)]
struct Program(Arc<Tree>);

impl AsRef<[u8]> for Program {
    fn as_ref(&self) -> &[u8] {
        &self.0.source().bytecode
    }
}

fn vec4(m: &impl GuestRead, at: u32) -> Result<Vec4, Bail> {
    let b = m.bytes(at, 16)?;
    Ok(std::array::from_fn(|k| {
        u32::from_le_bytes(b[4 * k..4 * k + 4].try_into().unwrap())
    }))
}

/// The setup's inputs, read at its entry: `ecx` the VM; stack
/// `collection, code, ray_input, parent_body, collector`.
fn prepare(
    c: &CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<(Setup, RayWalk<Program>), Bail> {
    if g.trapped() {
        return Err(Bail::Trap);
    }
    if !det::guest_env(c.mxcsr) || !det::x87_env(c.x87.control, c.x87.status, c.x87.tag, X87_FREE) {
        return Err(Bail::Env);
    }
    let precision = Precision::from_control(c.x87.control)?;
    let (sp, vm) = (c.esp, c.ecx);
    let (collection, code, input, parent, collector) = {
        let m = Read(&*g, (0, 0));
        let arg = |k: u32| m.u32(sp.wrapping_add(4 * k));
        (arg(1)?, arg(2)?, arg(3)?, arg(4)?, arg(5)?)
    };
    // The setup copies the code origin and both input vectors with MOVAPS.
    if (code | input | vm) & 15 != 0 {
        return Err(Bail::Fault);
    }
    let revision = g.mopp_revision(code)?;
    let m = Read(&*g, (0, 0));
    let tree = world.mopp(&m, code, revision)?;
    let source = tree.source();
    let origin = [
        source.origin[0],
        source.origin[1],
        source.origin[2],
        source.scale,
    ];
    let constants = Constants {
        zero: m.u32(layout::ZERO)?,
        one: m.u32(layout::ONE)?,
        unit: vec4(&m, l::UNIT)?,
        root_scale: m.u32(l::ROOT_SCALE)?,
    };
    let ray = Ray {
        origin,
        from: vec4(&m, input)?,
        to: vec4(&m, input.wrapping_add(l::INPUT_TO))?,
        precision,
        mxcsr: c.mxcsr,
        constants,
    };
    let root = ray.root()?;
    // The setup reads arguments and ray data between VM and stack writes.
    // Decline if forwarding through an overlap would change their values.
    let aligned = sp.checked_sub(4).ok_or(Bail::Fault)? & !15;
    let call_sp = aligned.checked_sub(0x60).ok_or(Bail::Fault)?;
    let outputs = super::preflight::Outputs::new(g, sp, sp - call_sp, [(vm, 0x68)])?;
    for (at, len) in [
        tree.span(),
        (input, 0x28),
        (collector.wrapping_add(l::COLLECTOR_FRACTION), 4),
        (sp.wrapping_add(4), 20),
        (layout::ONE, 4),
        (l::ROOT_SCALE, 4),
    ] {
        outputs.separate(g, at, len)?;
    }
    let setup = Setup::read(
        &m,
        vm,
        code,
        input,
        parent,
        collection,
        collector,
        root.inverse_scale,
        revision,
        constants,
    )?;
    Ok((setup, RayWalk::new(Program(tree), ray, &root)))
}
