//! Shared by interactive play, headless simulation and dedicated hosts.
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use engine::points;
use engine::sim_physics::PORTS;
use runtime::{CpuAbi, Guest};

pub struct NativePhysics;

/// `impl.sim.native.physics`: the Havok world step, phase 5 of the
/// simulation rewrite (docs/engine/sim-rewrite-havok.md). The step
/// wrapper, the island collide and the island integrate replay the retail
/// frame at their resume sites, the world phases and every leaf between
/// them stay guest code, and the kernels they absorb — the broadphase
/// batch update, the two unconstrained-motion methods and the integer
/// MOPP box query — run as capture arms compared with the guest at every
/// call (`engine::sim_physics::parity`). Retires the per-function
/// `native_havok_solver`, `native_havok_support_vertices`,
/// `native_havok_weld` and `native_mopp_box` switches.
pub const PHYSICS: &str = "sim.native.physics";

fn physics_on(switches: &hooks::Switches) -> bool {
    switches.implementation(PHYSICS, false)
}

impl Feature for NativePhysics {
    fn manifest(&self) -> Manifest {
        Manifest::new("native-simulation-physics", "Native simulation updates")
            .doc("docs/perf/sim-budget.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        for port in PORTS {
            // A subsystem body owns this entry and its return sites when
            // selected. The independent port remains a differential oracle.
            let installed = b.switches().implementation(port.switch, false)
                && !crate::sim_native::owns_anim(b.switches(), port.point.base)
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
                    .after_feature("sim-observer-resources");
            }
        }
        if physics_on(b.switches()) {
            install_physics(b);
        }
        Ok(())
    }
}

fn install_physics(b: &mut Builder) {
    for port in engine::sim_physics::PHYSICS_PORTS {
        b.raw()
            .implement(port.point, PHYSICS, move |cpu, ctx, pc| {
                let resume = pc != port.point.base && port.resumes.contains(&pc);
                if pc == port.point.base || resume {
                    let mut mem = ctx.memory();
                    if engine::sim_physics::supported(port, cpu, &mem) {
                        return port.invoke(cpu, &mut mem, pc);
                    }
                }
                let original = if resume {
                    ctx.runtime().resolve_original_resume(pc)
                } else {
                    ctx.runtime().resolve_original(pc).map(|h| (h, pc))
                };
                let (handler, selector) = original.ok_or(runtime::GuestFault {
                    addr: pc,
                    len: 1,
                    access: runtime::guest::Access::Execute,
                    reason: runtime::guest::FaultReason::Unmapped,
                })?;
                Ok(handler.invoke(cpu, ctx, selector))
            })
            .resumes(port.resumes)
            .context_only()
            .after_feature("sim-observer-resources")
            .after_feature("sim-submix");
    }
    // The capture arms: the guest answers each call, and the prediction
    // is compared with what it left. The MOPP box arm observes phase 2's
    // entry, which runs guest while the collision switch is off and
    // natively when it is on; both compare.
    let batch = &points::HAVOK_BROADPHASE_BATCH;
    b.raw()
        .observe_before(batch, "physics-broadphase-begin", move |o| {
            if o.addr == batch.base {
                engine::sim_physics::parity::broadphase_begin(o.thread_id, o.cpu, &*o.guest());
            }
        })
        .optional();
    b.raw()
        .observe_after(batch, "physics-broadphase-end", move |o| {
            if o.addr == batch.base {
                engine::sim_physics::parity::broadphase_end(o.thread_id, o.cpu, &*o.guest());
            }
        })
        .optional();
    let pose = &points::HAVOK_MOTION_POSE;
    b.raw()
        .observe_before(pose, "physics-motion-pose-begin", move |o| {
            if o.addr == pose.base {
                engine::sim_physics::parity::motion_pose_begin(o.thread_id, o.cpu, &*o.guest());
            }
        })
        .optional();
    b.raw()
        .observe_after(pose, "physics-motion-pose-end", move |o| {
            if o.addr == pose.base {
                engine::sim_physics::parity::motion_pose_end(o.thread_id, o.cpu, &*o.guest());
            }
        })
        .optional();
    let forces = &points::HAVOK_MOTION_FORCES;
    b.raw()
        .observe_before(forces, "physics-motion-forces-begin", move |o| {
            if o.addr == forces.base {
                engine::sim_physics::parity::motion_forces_begin(o.thread_id, o.cpu, &*o.guest());
            }
        })
        .optional();
    b.raw()
        .observe_after(forces, "physics-motion-forces-end", move |o| {
            if o.addr == forces.base {
                engine::sim_physics::parity::motion_forces_end(o.thread_id, o.cpu, &*o.guest());
            }
        })
        .optional();
    let mopp = &points::COLLISION_MOPP_BOX;
    b.raw()
        .observe_before(mopp, "physics-mopp-box-begin", move |o| {
            if o.addr == mopp.base {
                engine::sim_physics::parity::mopp_box_begin(o.thread_id, o.cpu, &mut *o.guest());
            }
        })
        .optional();
    b.raw()
        .observe_after(mopp, "physics-mopp-box-end", move |o| {
            if o.addr == mopp.base {
                engine::sim_physics::parity::mopp_box_end(o.thread_id, o.cpu, &*o.guest());
            }
        })
        .optional();
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::sim_physics::PHYSICS_PORTS;

    #[test]
    fn every_simulation_port_is_listed() {
        for port in PORTS {
            let key = format!("impl.{}", port.switch);
            assert!(hooks::switches::table::get(&key).is_some(), "{key} is not in the switch table");
        }
    }

    #[test]
    fn the_havok_port_switches_are_retired() {
        for key in [
            "sim.native_havok_solver",
            "sim.native_havok_support_vertices",
            "sim.native_havok_weld",
            "sim.native_mopp_box",
        ] {
            assert!(
                hooks::Switches::parse(&format!("impl.{key}")).is_err(),
                "{key} still parses"
            );
        }
    }

    #[test]
    fn physics_installs_the_world_entries_and_the_capture_arms() {
        let mut registry = hooks::Registry::new(
            hooks::Switches::defaults().with("impl.sim.native.physics", true),
        );
        registry.add(&super::NativePhysics).unwrap();
        let plan = registry.build(&hooks::Base).unwrap();
        // The per-function ports stay off: only the physics switch is on.
        for port in PORTS {
            assert!(!plan.chains.contains_key(&port.point.base), "{} installed", port.switch);
        }
        for port in PHYSICS_PORTS {
            let chain = &plan.chains[&port.point.base];
            let hook = chain.implement.as_ref().unwrap();
            assert_eq!(hook.label, PHYSICS);
            assert_eq!(hook.continuation_sites().0, port.resumes);
        }
        // Four observer pairs: the broadphase batch, both motion methods
        // and the MOPP box query.
        for point in [
            &points::HAVOK_BROADPHASE_BATCH,
            &points::HAVOK_MOTION_POSE,
            &points::HAVOK_MOTION_FORCES,
            &points::COLLISION_MOPP_BOX,
        ] {
            let chain = &plan.chains[&point.base];
            assert!(chain.implement.is_none(), "{:?}", point.name);
            assert_eq!((chain.observe_before.len(), chain.observe_after.len()), (1, 1), "{:?}", point.name);
            assert!(chain.observe_before[0].optional && chain.observe_after[0].optional);
        }
        assert_eq!(
            plan.chains.len(),
            PHYSICS_PORTS.len() + 4,
            "unexpected extra chains"
        );
    }
}
