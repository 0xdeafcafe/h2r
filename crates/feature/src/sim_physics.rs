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
                    if port.point.base == engine::points::OBJECT_SPATIAL_PORTAL.base
                        && (cpu.x87.control & 0x3f != 0x3f || cpu.mxcsr & 0x1f80 != 0x1f80)
                    {
                        let original = ctx.runtime().resolve_original(port.point.base)
                            .ok_or(runtime::GuestFault {
                                addr: pc, len: 1, access: runtime::guest::Access::Execute,
                                reason: runtime::guest::FaultReason::Unmapped,
                            })?;
                        return Ok(original.invoke(cpu, ctx, pc));
                    }
                    (port.run)(cpu, &mut ctx.memory(), pc)
                }).resumes(port.resumes).context_only();
            }
        }
        Ok(())
    }
}
