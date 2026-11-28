//! G and H as installed. Every admitted call is answered in Rust: the
//! uncontested flood and the no-cluster visit replay their scratch frame
//! too, every other call commits the kernel's plan.
use super::*;
use runtime::Guest;

fn commit_uncontested_flood(
    c: &mut CpuAbi,
    g: &mut Guest,
    p: FloodPlan,
) -> Result<TransferAbi, GuestFault> {
    let sp = c.esp;
    for (i, value) in [p.structure, c.ebx, c.ebp, c.esi, c.edi]
        .into_iter()
        .enumerate()
    {
        g.set_u32(sp - 4 * (1 + i as u32), value)?;
    }
    let portals = p.flood.portal_count as i32;
    g.set_u32(sp.wrapping_add(16), portals.max(0) as u32)?;
    g.set_u32(sp.wrapping_add(20), 1)?;
    c.edx = if portals > 0 { p.epoch } else { p.structure };
    commit_flood(c, g, p)
}

fn commit_empty_visit(
    c: &mut CpuAbi,
    g: &mut Guest,
    p: VisitPlan,
) -> Result<TransferAbi, GuestFault> {
    let frame = c.esp - 0x430;
    for (i, value) in [c.edi, c.esi, c.ebp, c.ebx, 0, 0].into_iter().enumerate() {
        g.set_u32(frame + 4 * i as u32, value)?;
    }
    g.set_u32(frame + 36, 0)?;
    if p.lists == 0 {
        g.set_u32(c.esp.wrapping_add(4), u32::MAX)?;
    }
    if p.types == 0 {
        g.set_u32(c.esp.wrapping_add(8), u32::MAX)?;
    }
    c.edx = p.generation.wrapping_add(1);
    c.xmm[4] = u128::from(p.radius).to_le_bytes();
    commit_visit(c, g, p)
}

/// Answer G or H. A declined plan returns `None` with its bail recorded
/// (`sim_collision::take_bail`); nothing was written.
pub fn dispatch(
    entry: Entry,
    c: &mut CpuAbi,
    g: &mut Guest,
    world: &CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault> {
    let declined = if !crate::sim_native::supported(c) {
        Bail::Env
    } else if g.has_trap() {
        Bail::Trap
    } else {
        match entry {
            Entry::ClusterFlood => match plan_flood(c, g, world) {
                Ok(p) => {
                    world.record(entry, &Ok(()));
                    let t = if p.flood.deepest.is_none() {
                        commit_uncontested_flood(c, g, p)?
                    } else {
                        commit_flood(c, g, p)?
                    };
                    return Ok(Some(t));
                }
                Err(why) => why,
            },
            Entry::ObjectVisit => match plan_visit(c, g, world) {
                Ok(p) => {
                    world.record(entry, &Ok(()));
                    let t = if p.empty_location {
                        commit_empty_visit(c, g, p)?
                    } else {
                        commit_visit(c, g, p)?
                    };
                    return Ok(Some(t));
                }
                Err(why) => why,
            },
            _ => unreachable!("spatial entry"),
        }
    };
    world.record::<()>(entry, &Err(declined));
    Ok(None)
}
