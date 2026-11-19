//! Configure checkpointed AI and script storage before retail loads the scenario.
pub(crate) struct CampaignPools;

impl feature::Feature for CampaignPools {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(
            "campaign-squad-pools",
            "Size the squad and script arrays for the scenario",
        )
        .doc("docs/halo1/ai.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        type Prepare = fn(&mut dyn engine::GuestMemory) -> Result<(), engine::MemFault>;
        for (point, label, prepare) in [
            (&engine::points::AI_SQUADS_INITIALIZE, "size the campaign squad pools", engine::squad_pool::prepare as Prepare),
            (&engine::points::HS_NEW_MAP, "size the campaign script pools", engine::hs_pool::prepare as Prepare),
        ] {
            b.raw().before(point, label, move |call| {
                let mut memory = call.ctx.runtime().memory();
                prepare(&mut *memory).map_err(|f| runtime::GuestFault {
                    addr: f.addr,
                    len: f.len as usize,
                    access: if f.write { runtime::guest::Access::Write } else { runtime::guest::Access::Read },
                    reason: runtime::guest::FaultReason::Unmapped,
                })?;
                Ok(feature::Flow::Continue)
            });
        }
        b.snapshot(feature::SnapshotPolicy::Stateless);
        Ok(())
    }
}
