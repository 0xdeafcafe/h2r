//! G and H answer the paths whose full live and scratch state is known.
//! On the others the guest runs, and the outer call compares its result,
//! mark tables, generations and output with the same kernel plan.
use super::*;
use runtime::Guest;

// Return address of G's only self-call. Its parent already predicts the
// complete flood, including this visit and its marks.
const FLOOD_SELF_RETURN: u32 = 0x0014_a690;

struct Region {
    at: u32,
    bytes: Vec<u8>,
}

impl Region {
    fn snapshot(g: &impl CollisionMemory, at: u32, len: u32) -> Result<Self, Bail> {
        Ok(Self {
            at,
            bytes: Read(g, (0, 0)).bytes(at, len as usize)?,
        })
    }

    fn set_word(&mut self, at: u32, value: u32) {
        let offset = (at - self.at) as usize;
        self.bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
}

/// Owned by the passive return observer for this invocation.
pub struct Prediction {
    epoch: u64,
    eax: u32,
    regions: Vec<Region>,
}

impl Prediction {
    fn flood(g: &impl CollisionMemory, p: FloodPlan) -> Result<Self, Bail> {
        let mut marks = Region::snapshot(g, layout::CLUSTER_MARKS, 4 * MAX_CLUSTERS)?;
        for &cluster in &p.flood.clusters {
            marks.set_word(layout::CLUSTER_MARKS + 4 * u32::from(cluster), p.epoch);
        }
        let out = Region {
            at: p.out,
            bytes: p.flood.clusters[..p.flood.written]
                .iter()
                .flat_map(|c| c.to_le_bytes())
                .collect(),
        };
        Ok(Self {
            epoch: super::super::EPOCH.get(),
            eax: u32::from(p.flood.count()),
            regions: vec![marks, out],
        })
    }

    fn visit(g: &impl CollisionMemory, p: VisitPlan) -> Result<Self, Bail> {
        let mut objects = Region::snapshot(
            g,
            layout::VISIT_ACTIVE,
            layout::OBJECT_MARKS + 4 * layout::OBJECT_MARK_SLOTS - layout::VISIT_ACTIVE,
        )?;
        let mut clusters = Region::snapshot(
            g,
            layout::FLOOD_ACTIVE,
            layout::CLUSTER_MARKS + 4 * MAX_CLUSTERS - layout::FLOOD_ACTIVE,
        )?;
        let generation = p.generation.wrapping_add(1);
        objects.bytes[0] = 0;
        objects.set_word(layout::VISIT_GENERATION, generation);
        for &slot in &p.visit.marks {
            objects.set_word(layout::OBJECT_MARKS + 4 * u32::from(slot), generation);
        }
        if let Some(flood) = &p.visit.flood {
            let epoch = p.flood_epoch.wrapping_add(1);
            clusters.bytes[0] = 0;
            clusters.set_word(layout::FLOOD_EPOCH, epoch);
            for &cluster in &flood.clusters {
                clusters.set_word(layout::CLUSTER_MARKS + 4 * u32::from(cluster), epoch);
            }
        }
        let out = Region {
            at: p.out,
            bytes: p
                .visit
                .handles
                .iter()
                .flat_map(|h| h.to_le_bytes())
                .collect(),
        };
        Ok(Self {
            epoch: super::super::EPOCH.get(),
            eax: p.visit.eax(),
            regions: vec![objects, clusters, out],
        })
    }

    pub fn compare(self, c: &CpuAbi, g: &impl CollisionMemory) -> Result<Option<String>, Bail> {
        if g.trapped() {
            return Err(Bail::Trap);
        }
        if self.epoch != super::super::EPOCH.get() {
            return Err(Bail::Data);
        }
        if self.eax != c.eax {
            return Ok(Some(format!(
                "EAX expected {:#x}, guest {:#x}",
                self.eax, c.eax
            )));
        }
        for region in self.regions {
            if region.bytes.is_empty() {
                continue;
            }
            let actual = Read(g, (0, 0)).bytes(region.at, region.bytes.len())?;
            if let Some(i) = actual.iter().zip(&region.bytes).position(|(a, b)| a != b) {
                return Ok(Some(format!(
                    "{:#x} expected {:#04x}, guest {:#04x}",
                    region.at + i as u32,
                    region.bytes[i],
                    actual[i],
                )));
            }
        }
        Ok(None)
    }
}

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

/// Run a scratch-exact exit or give the caller a prediction to attach
/// to this invocation's guest return.
pub fn dispatch(
    entry: Entry,
    c: &mut CpuAbi,
    g: &mut Guest,
    world: &CollisionWorld,
) -> Result<(Option<TransferAbi>, Option<Prediction>), GuestFault> {
    world.observe_environment(entry, c.mxcsr, c.x87.control);
    let prediction = if !crate::sim_native::supported(c) {
        Err(Bail::Env)
    } else if g.has_trap() {
        Err(Bail::Trap)
    } else if entry == Entry::ClusterFlood && g.u32(c.esp) == Ok(FLOOD_SELF_RETURN) {
        return Ok((None, None));
    } else {
        match entry {
            Entry::ClusterFlood => match plan_flood(c, g, world) {
                Ok(p) if p.flood.deepest.is_none() => {
                    let t = commit_uncontested_flood(c, g, p)?;
                    world.record(entry, &Ok(()));
                    return Ok((Some(t), None));
                }
                Ok(p) => Prediction::flood(g, p),
                Err(why) => Err(why),
            },
            Entry::ObjectVisit => match plan_visit(c, g, world) {
                Ok(p) if p.empty_location => {
                    let t = commit_empty_visit(c, g, p)?;
                    world.record(entry, &Ok(()));
                    return Ok((Some(t), None));
                }
                Ok(p) => Prediction::visit(g, p),
                Err(why) => Err(why),
            },
            _ => unreachable!("spatial entry"),
        }
    };
    match prediction {
        Ok(p) => Ok((None, Some(p))),
        Err(why) => {
            world.record::<()>(entry, &Err(why));
            world.record_spatial(entry, Err(why));
            Ok((None, None))
        }
    }
}
