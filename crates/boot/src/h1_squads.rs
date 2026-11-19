//! Set the two AI pool capacities for this scenario before squad initialization.
pub(crate) struct SquadPools;

impl feature::Feature for SquadPools {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(
            "campaign-squad-pools",
            "Size the squad and group arrays for the scenario",
        )
        .doc("docs/halo1/ai.md")
    }
    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        b.raw().before(
            &engine::points::AI_SQUADS_INITIALIZE,
            "size the campaign squad pools",
            |c| {
                let mut m = c.ctx.runtime().memory();
                engine::squad_pool::prepare(&mut *m).map_err(|f| runtime::GuestFault {
                    addr: f.addr,
                    len: f.len as usize,
                    access: if f.write {
                        runtime::guest::Access::Write
                    } else {
                        runtime::guest::Access::Read
                    },
                    reason: runtime::guest::FaultReason::Unmapped,
                })?;
                Ok(feature::Flow::Continue)
            },
        );
        b.snapshot(feature::SnapshotPolicy::Stateless);
        Ok(())
    }
}
