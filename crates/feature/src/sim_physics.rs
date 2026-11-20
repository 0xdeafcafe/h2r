//! Shared by interactive play, headless simulation and dedicated hosts.
use crate::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use engine::sim_physics::PORTS;

pub struct NativePhysics;

impl Feature for NativePhysics {
    fn manifest(&self) -> Manifest {
        Manifest::new("native-simulation-physics", "Native simulation updates")
            .doc("docs/perf/sim-budget.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        for port in PORTS {
            if b.switches().implementation(port.switch, false) {
                b.raw().implement(port.point, port.switch, move |cpu, ctx, pc| {
                    if pc != port.point.base && !port.resumes.contains(&pc) {
                        // The layouts and return sites describe only the base image.
                        let original = ctx.runtime().resolve_original(pc)
                            .ok_or(runtime::GuestFault {
                                addr: pc, len: 1, access: runtime::guest::Access::Execute,
                                reason: runtime::guest::FaultReason::Unmapped,
                            })?;
                        return Ok(original.invoke(cpu, ctx, pc));
                    }
                    let mut mem = ctx.memory();
                    if !engine::sim_physics::supported(port, cpu, &mem) {
                        drop(mem);
                        let original = ctx.runtime().resolve_original(port.point.base)
                            .ok_or(runtime::GuestFault {
                                addr: pc, len: 1, access: runtime::guest::Access::Execute,
                                reason: runtime::guest::FaultReason::Unmapped,
                            })?;
                        return Ok(original.invoke(cpu, ctx, pc));
                    }
                    (port.run)(cpu, &mut mem, pc)
                }).resumes(port.resumes).generated_resumes(port.generated_sites).context_only();
            }
        }
        Ok(())
    }
}
