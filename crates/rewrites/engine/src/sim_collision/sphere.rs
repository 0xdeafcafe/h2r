//! The sphere pretest at the object-reference call site in the whole query.
use super::{CollisionMemory, Read, finish, preflight::Outputs};
use crate::sim_native::add_flags;
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_collision::{
    Bail, CollisionWorld, Entry, GuestRead,
    sphere::{Sphere, SphereOffset, Sweep},
};
use sim_core::det::{self, Fp};

const AFTER_OBJECT_SPHERE: u32 = 0x0016_9d3f;
const FRAME_BYTES: u32 = 12;

struct Plan {
    radius: u32,
    offset: SphereOffset,
    sweep: Option<([u32; 3], Sweep)>,
    /// The MXCSR after the pretest's arithmetic.
    mxcsr: u32,
}

fn plan(c: &CpuAbi, g: &impl CollisionMemory) -> Result<Plan, Bail> {
    if !det::guest_env(c.mxcsr) || !det::host_fast_env() {
        return Err(Bail::Env);
    }
    let mem = Read(g, (0, 0));
    let sphere = Sphere {
        center: mem.vec3(c.eax)?,
        radius: mem.u32(c.esp.wrapping_add(4))?,
    };
    let mut fp = Fp::guest(c.mxcsr);
    let offset = sphere.offset_with(&mut fp, mem.vec3(c.ecx)?)?;
    let sweep = if offset.contains_start() {
        None
    } else {
        let delta = mem.vec3(c.edx)?;
        let sweep = offset.sweep_with(&mut fp, delta)?;
        // All three direction words are read before the original spills
        // them. The return address remains live after the spills.
        Outputs::new(g, c.esp, FRAME_BYTES, [])?;
        Some((delta, sweep))
    };
    Ok(Plan {
        radius: sphere.radius,
        offset,
        sweep,
        mxcsr: fp.mxcsr().unwrap_or(c.mxcsr),
    })
}

fn xmm(c: &mut CpuAbi, index: usize, word: u32) {
    c.xmm[index] = u128::from(word).to_le_bytes();
}

/// Run the pretest for the lifted whole query's object references.
pub fn object_sphere_pretest(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    if g.u32(c.esp).ok() != Some(AFTER_OBJECT_SPHERE) {
        return Ok(None);
    }
    let planned = plan(c, g);
    world.record(Entry::ObjectSphere, &planned);
    let Ok(plan) = planned else { return Ok(None) };
    c.mxcsr = plan.mxcsr;

    let frame = c.esp.wrapping_sub(FRAME_BYTES);
    let [x, y, z] = plan.offset.relative;
    for (i, word) in [
        plan.radius,
        plan.offset.radius_squared,
        plan.offset.gap,
        x,
        y,
        z,
        0,
    ]
    .into_iter()
    .enumerate()
    {
        xmm(c, i, word);
    }
    if let Some(([dx, dy, dz], sweep)) = plan.sweep {
        g.set_u32(frame + 8, dz)?;
        g.set_u32(frame + 4, dy)?;
        g.set_u32(frame, dx)?;
        (c.eax, c.ecx, c.edx) = (dx, dy, dz);
        for (i, word) in [
            (0, sweep.projection),
            (1, dz),
            (4, dx),
            (5, dy),
            (7, sweep.x_product),
        ] {
            xmm(c, i, word);
        }
        c.eax = (c.eax & !0xff) | u32::from(sweep.hit());
        if let Some(q) = sweep.quadratic {
            xmm(c, 1, q.length_squared);
            xmm(c, 3, q.discriminant);
            xmm(c, 4, q.scaled_gap);
            if let Some(end) = q.end {
                xmm(c, 2, end.gap);
                if let Some(squared) = end.gap_squared {
                    xmm(c, 0, squared);
                    c.eax &= 0xff;
                }
            }
        }
    } else {
        c.eax = (c.eax & !0xff) | 1;
    }
    add_flags(c, frame, FRAME_BYTES);
    Ok(Some(finish(c, AFTER_OBJECT_SPHERE, 4)))
}

#[cfg(test)]
mod tests;
