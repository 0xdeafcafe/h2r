//! Shared by interactive play, headless simulation and dedicated hosts.
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use engine::sim_physics::PORTS;
use runtime::{CpuAbi, Guest};

pub struct NativePhysics;

impl Feature for NativePhysics {
    fn manifest(&self) -> Manifest {
        Manifest::new("native-simulation-physics", "Native simulation updates")
            .doc("docs/perf/sim-budget.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        for port in PORTS {
            // A body under `impl.sim.native.objects` owns its point; the
            // port is then a test oracle.
            let installed = b.switches().implementation(port.switch, false)
                && !crate::sim_objects::owns(b.switches(), port.point.base);
            crate::shadow::register(port.switch, crate::shadow::SIM_PHYSICS, port.point, installed);
            if installed {
                b.raw().implement(port.point, port.switch, move |cpu, ctx, pc| {
                    let resume = pc != port.point.base && port.resumes.contains(&pc);
                    if pc == port.point.base || resume {
                        let mut mem = ctx.memory();
                        if engine::sim_physics::supported(port, cpu, &mem) {
                            // Entries and resumes alike: each segment is
                            // checked from the state it starts in.
                            return crate::shadow::run(port.switch, cpu, &mut mem, pc, &mut |cpu: &mut CpuAbi, mem: &mut Guest| {
                                port.invoke(cpu, mem, pc)
                            });
                        }
                    }
                    let original = if resume {
                        ctx.runtime().resolve_original_resume(pc)
                    } else {
                        ctx.runtime().resolve_original(pc).map(|h| (h,pc))
                    };
                    let (handler,selector) = original.ok_or(runtime::GuestFault {
                        addr: pc, len: 1, access: runtime::guest::Access::Execute,
                        reason: runtime::guest::FaultReason::Unmapped,
                    })?;
                    Ok(handler.invoke(cpu, ctx, selector))
                }).resumes(port.resumes).generated_resumes(port.generated_sites).context_only()
                    // Inside the light-host skips that share a point.
                    .after_feature("sim-observer-resources")
                    .after_feature("sim-submix");
            }
        }
        Ok(())
    }
}
