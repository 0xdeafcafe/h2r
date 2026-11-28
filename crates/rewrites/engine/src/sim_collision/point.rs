//! Entries A and B. A terminal node supplies the volatile register results
//! as well as the leaf. Both frames are small enough to replay in full.
use super::{CollisionMemory, Read, consts, finish, layout, preflight::Outputs};
use crate::sim_native::{logic_flags, sub_flags};
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_collision::bsp::{NONE, point::PointResult};
use sim_collision::{Bail, CollisionWorld, Entry, GuestRead};
use sim_core::det::{self, Fp};

fn walk<const N: usize>(
    g: &mut impl CollisionMemory,
    c: &CpuAbi,
    world: &CollisionWorld,
    outputs: &Outputs<N>,
    bsp_addr: u32,
    node: u32,
) -> Result<(PointResult, (u64, u64), u32), Bail> {
    if !det::guest_env(c.mxcsr) || !det::host_fast_env() {
        return Err(Bail::Env);
    }
    let revision = g.bsp_revision(bsp_addr)?;
    let m = Read(&*g, revision);
    let bsp = world.bsp(&m, bsp_addr, || consts(&m))?;
    outputs.check_bsp(g, &bsp)?;
    outputs.separate(g, c.edx, 12)?;
    // The walk's SSE arithmetic under the guest's MXCSR: PE comes out set
    // when an operation was inexact.
    let mut fp = Fp::tracked(c.mxcsr);
    let p = bsp.point_result_with(&mut fp, node, m.vec3(c.edx)?)?;
    Ok((p, revision, fp.mxcsr().unwrap_or(c.mxcsr)))
}

fn point_registers(c: &mut CpuAbi, p: PointResult) {
    c.eax = p.leaf;
    c.ecx = p.node_cursor;
    c.xmm[0] = [0; 16];
    c.xmm[1] = u128::from(p.distance).to_le_bytes();
    c.xmm[2] = u128::from(p.x_product).to_le_bytes();
    logic_flags(c, p.leaf);
}

/// Entry A, `0x14a280`: point in EDX, starting node in EAX, BSP in ECX.
pub fn point_to_leaf(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let r = (|| {
        let outputs = Outputs::new(g, c.esp, 8, [])?;
        let (point, _, mxcsr) = walk(g, c, world, &outputs, c.ecx, c.eax)?;
        Ok((point, outputs.target, mxcsr))
    })();
    world.record(Entry::PointToLeaf, &r);
    let Ok((p, target, mxcsr)) = r else { return Ok(None) };
    c.mxcsr = mxcsr;
    g.set_u32(c.esp - 4, c.esi)?;
    g.set_u32(c.esp - 8, c.edi)?;
    point_registers(c, p);
    Ok(Some(finish(c, target, 0)))
}

struct Cluster {
    structure: u32,
    leaves: u32,
    index: i16,
}

struct Location {
    target: u32,
    index: u16,
    point: Option<PointResult>,
    cluster: Option<Cluster>,
    /// The MXCSR after the point walk.
    mxcsr: u32,
}

fn plan_location(
    g: &mut impl CollisionMemory,
    c: &CpuAbi,
    world: &CollisionWorld,
) -> Result<Location, Bail> {
    let m = Read(&*g, (0, 0));
    let index = m.u16(layout::BSP_INDEX)?;
    let active = index != u16::MAX;
    let outputs = Outputs::new(g, c.esp, if active { 16 } else { 4 }, [(c.esi, 8)])?;
    outputs.separate(g, layout::BSP_INDEX, 2)?;
    let mut loc = Location {
        target: outputs.target,
        index,
        point: None,
        cluster: None,
        mxcsr: c.mxcsr,
    };
    if active {
        outputs.separate(g, layout::STRUCTURE_BSP, 4)?;
        let bsp_addr = m.u32(layout::STRUCTURE_BSP)?;
        let (p, revision, mxcsr) = walk(g, c, world, &outputs, bsp_addr, 0)?;
        loc.point = Some(p);
        loc.mxcsr = mxcsr;
        if p.leaf != NONE {
            // The leaf is already in [esi] when the guest reads this chain.
            outputs.separate(g, layout::ACTIVE_STRUCTURE, 4)?;
            let m = Read(&*g, revision);
            let structure = m.u32(layout::ACTIVE_STRUCTURE)?;
            let table_at = structure.wrapping_add(layout::STRUCTURE_LEAVES);
            outputs.separate(g, table_at, 4)?;
            let leaves = m.u32(table_at)?;
            let at = leaves.wrapping_add(p.leaf * 8);
            outputs.separate(g, at, 2)?;
            let table = g.leaf_table(
                structure.wrapping_add(layout::STRUCTURE_LEAF_COUNT),
                leaves,
                p.leaf,
            );
            let m = Read(&*g, revision);
            let index = match table.and_then(|t| world.leaf_cluster(&m, t, p.leaf)) {
                Some(cluster) => cluster,
                None => m.u16(at)? as i16,
            };
            loc.cluster = Some(Cluster {
                structure,
                leaves,
                index,
            });
        }
    }
    Ok(loc)
}

/// Entry B, `0x11bed0`: point in EDX, eight-byte location in ESI.
pub fn location(
    c: &mut CpuAbi,
    g: &mut impl CollisionMemory,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let r = plan_location(g, c, world);
    world.record(Entry::Location, &r);
    let Ok(loc) = r else { return Ok(None) };
    let out = c.esi;
    g.set_u32(c.esp - 4, c.edi)?;
    if let Some(p) = loc.point {
        // B handles the call at 0x11bef6 without re-entering A's hook.
        world.observe_environment(Entry::PointToLeaf, c.mxcsr, c.x87.control);
        g.set_u32(c.esp - 8, layout::LOCATION_AFTER_POINT)?;
        g.set_u32(c.esp - 12, c.esi)?;
        g.set_u32(c.esp - 16, (c.edi & 0xffff_0000) | u32::from(loc.index))?;
        point_registers(c, p);
        g.set_u32(out, p.leaf)?;
        if let Some(cluster) = loc.cluster {
            sub_flags(c, p.leaf, NONE, 32);
            c.ecx = cluster.structure;
            c.edx = cluster.leaves;
            c.eax = i32::from(cluster.index) as u32;
        }
        g.set_u16(out + 6, loc.index)?;
        g.set_u16(out + 4, c.eax as u16)?;
    } else {
        sub_flags(c, u32::from(loc.index), u32::from(u16::MAX), 16);
        g.set_u16(out + 4, loc.index)?;
        g.set_u16(out + 6, loc.index)?;
        g.set_u32(out, NONE)?;
    }
    // After A's environment was observed with the entry's MXCSR.
    c.mxcsr = loc.mxcsr;
    Ok(Some(finish(c, loc.target, 0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::{Guest, guest::Permissions};
    use sim_core::Epoch;

    #[test]
    fn location_checks_writability_and_physical_aliases_before_the_push() {
        const STACK: u32 = 0x0001_1000;
        const STACK_ALIAS: u32 = 0x0002_1000;
        const READ_ONLY: u32 = 0x0003_1000;
        static EPOCH: Epoch = Epoch::new();
        let mut g = Guest::new();
        g.map(STACK, 0x1000, 0, Permissions::RW).unwrap();
        g.map(STACK_ALIAS, 0x1000, 0, Permissions::RW).unwrap();
        g.map(READ_ONLY, 0x1000, 0x1000, Permissions::READ).unwrap();
        g.map(layout::BSP_INDEX, 2, 0x2000, Permissions::RW)
            .unwrap();
        g.set_u16(layout::BSP_INDEX, u16::MAX).unwrap();
        let world = CollisionWorld::new(&EPOCH);
        for (out, bail) in [(STACK_ALIAS, Bail::Alias), (READ_ONLY, Bail::Fault)] {
            let mut c = CpuAbi {
                esp: STACK + 4,
                edi: 42,
                esi: out,
                ..CpuAbi::default()
            };
            let before = c;
            assert!(location(&mut c, &mut g, &world).unwrap().is_none());
            assert_eq!(c, before);
            assert_eq!(g.u32(STACK).unwrap(), 0, "the saved EDI slot is untouched");
            assert_eq!(g.u32(out).unwrap(), 0);
            assert_eq!(
                world.stats().entries[Entry::Location as usize].bails[bail as usize],
                1
            );
        }
    }
}
