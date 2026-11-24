//! Instanced-geometry traces for one structure cluster (entry E).
use super::{CollisionMemory, Outputs, Read, finish, layout};
use crate::sim_native::{Memory, sub_flags};
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_collision::{Bail, CollisionWorld, Entry, GuestRead};

const CLUSTERS: u32 = 0xa0;
const CLUSTER_SIZE: u32 = 0xb0;
const CODE_COUNT: u32 = 0xa8;

fn cluster_at(m: &impl GuestRead, structure: u32, index: u32) -> Result<u32, Bail> {
    let base = m.u32(structure.wrapping_add(CLUSTERS))?;
    Ok(base.wrapping_add(index.wrapping_mul(CLUSTER_SIZE)))
}

/// The count test precedes all floating point work and all reads of the
/// three arguments. Replay its complete frame and volatile registers.
/// A populated cluster continues into the guest with its original CPU.
pub fn empty_cluster(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let plan = (|| -> Result<_, Bail> {
        let saved_bp = c.esp.checked_sub(4).ok_or(Bail::Fault)?;
        let frame = (saved_bp & !15).checked_sub(0x110).ok_or(Bail::Fault)?;
        let output = Outputs::new(g, c.esp, c.esp - frame, [])?;
        let m = Read(&*g, (0, 0));
        output.separate(g, layout::ACTIVE_STRUCTURE, 4)?;
        let structure = m.u32(layout::ACTIVE_STRUCTURE)?;
        output.separate(g, structure.wrapping_add(CLUSTERS), 4)?;
        let cluster = cluster_at(&m, structure, c.eax)?;
        output.separate(g, cluster.wrapping_add(CODE_COUNT), 4)?;
        let count = m.u32(cluster.wrapping_add(CODE_COUNT))?;
        Ok((saved_bp, frame, output.target, structure, cluster, count))
    })();
    if matches!(&plan, Ok((_, _, _, _, _, count)) if (*count as i32) > 0) {
        // The positive path is compared separately at its owning query.
        return Ok(None);
    }
    world.record(Entry::InstanceTrace, &plan);
    let Ok((saved_bp, frame, target, structure, cluster, count)) = plan else {
        return Ok(None);
    };
    g.set_u32(saved_bp, c.ebp)?;
    g.set_u32(frame, c.ebx)?;
    g.write_bytes(frame + 0x13, &[0], 1)?;
    c.ecx = structure;
    c.edx &= !0xff;
    c.eax = cluster & !0xff;
    sub_flags(c, count, 0, 32);
    Ok(Some(finish(c, target, 12)))
}
