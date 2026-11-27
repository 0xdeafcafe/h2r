//! Shared by interactive play, headless simulation and dedicated hosts.
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use engine::points;
use engine::sim_physics::PORTS;
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};

pub struct NativePhysics;

/// `impl.sim.native.physics`: the Havok world step, phase 5 of the
/// simulation rewrite (docs/engine/sim-rewrite-havok.md). The step
/// wrapper and the island collide replay the retail frame at their resume
/// sites, the world phases and most leaves between them stay guest code,
/// and the kernels they absorb run as capture arms
/// (`engine::sim_physics::parity`): the broadphase batch update and the
/// island integrate answer calls themselves once proven, with one call in
/// `AUDIT_EVERY` still compared with the guest (the integrate's guest
/// answer is its frame replay); the two unconstrained-motion methods and
/// the integer MOPP box query are compared with the guest at every call.
/// Retires the per-function `native_havok_solver`,
/// `native_havok_support_vertices`, `native_havok_weld` and
/// `native_mopp_box` switches.
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

/// The original at an entry, or its resume at a site.
fn original(cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32, resume: bool) -> Result<TransferAbi, GuestFault> {
    let original = if resume {
        ctx.runtime().resolve_original_resume(pc)
    } else {
        ctx.runtime().resolve_original(pc).map(|h| (h, pc))
    };
    let (handler, selector) = original.ok_or(GuestFault {
        addr: pc,
        len: 1,
        access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::Unmapped,
    })?;
    Ok(handler.invoke(cpu, ctx, selector))
}

/// A world entry's frame replay when its model takes the state, else the
/// original.
fn replay(port: &'static engine::sim_physics::Port, cpu: &mut CpuAbi, ctx: &mut ExecCtx, pc: u32) -> Result<TransferAbi, GuestFault> {
    let resume = pc != port.point.base && port.resumes.contains(&pc);
    if pc == port.point.base || resume {
        let mut mem = ctx.memory();
        if engine::sim_physics::supported(port, cpu, &mem) {
            return port.invoke(cpu, &mut mem, pc);
        }
    }
    original(cpu, ctx, pc, resume)
}

/// The map load each arm's take-over proof restarts at.
const MAP_LOAD: &points::Point = &points::CACHE_TAGS_LOAD;

fn install_physics(b: &mut Builder) {
    use engine::sim_physics::parity::{self, Step};
    for port in engine::sim_physics::PHYSICS_PORTS {
        b.raw()
            .implement(port.point, PHYSICS, move |cpu, ctx, pc| replay(port, cpu, ctx, pc))
            .resumes(port.resumes)
            .context_only()
            .after_feature("sim-observer-resources")
            .after_feature("sim-submix");
    }
    // The island integrate: `parity::integrate_enter` decides who answers,
    // as for the broadphase batch below. When the guest answers, the
    // entry's frame replay runs it, its resume sites with it. Ordered as
    // the other world entries.
    let integrate = &engine::sim_physics::ISLAND_INTEGRATE;
    b.raw()
        .implement(integrate.point, PHYSICS, move |cpu, ctx, pc| {
            if pc == integrate.point.base {
                let step = {
                    let mut mem = ctx.memory();
                    parity::integrate_enter(cpu, &mut mem)
                };
                match step {
                    Step::Answered { target } => return Ok(TransferAbi::ret(target)),
                    Step::Audit(prediction) => {
                        let observed = ctx.observe_guest_return(cpu, engine::sim_physics::island_integrate::ARGS, move |c, ctx| {
                            parity::integrate_compare(&prediction, c, &ctx.runtime().memory());
                        });
                        if !observed {
                            parity::integrate_uncompared();
                        }
                    }
                    Step::Guest => {}
                }
            }
            replay(integrate, cpu, ctx, pc)
        })
        .resumes(integrate.resumes)
        .context_only()
        .after_feature("sim-observer-resources")
        .after_feature("sim-submix");
    // The broadphase batch update: `parity::broadphase_enter` decides who
    // answers. Until the arm is proven since the map load, and at every
    // audit slot after, the guest answers and the prediction is compared
    // at its return; a proven call is answered from the prediction.
    let batch = &points::HAVOK_BROADPHASE_BATCH;
    b.raw()
        .implement(batch, PHYSICS, move |cpu, ctx, pc| {
            if pc == batch.base {
                let step = {
                    let mut mem = ctx.memory();
                    parity::broadphase_enter(cpu, &mut mem)
                };
                match step {
                    Step::Answered { target } => return Ok(TransferAbi::ret(target)),
                    Step::Audit(prediction) => {
                        let observed = ctx.observe_guest_return(cpu, parity::BROADPHASE_ARGS, move |c, ctx| {
                            parity::broadphase_compare(&prediction, c, &ctx.runtime().memory());
                        });
                        if !observed {
                            parity::uncompared(&parity::BROADPHASE);
                        }
                    }
                    Step::Guest => {}
                }
            }
            original(cpu, ctx, pc, false)
        })
        .context_only()
        .optional();
    // The compare-only arms: the guest answers each call, and the
    // prediction is compared with what it left. The MOPP box arm observes
    // phase 2's entry, which runs guest while the collision switch is off
    // and natively when it is on; both compare.
    let pose = &points::HAVOK_MOTION_POSE;
    b.raw()
        .observe_before(pose, "physics-motion-pose-begin", move |o| {
            if o.addr == pose.base {
                parity::motion_pose_begin(o.thread_id, o.cpu, &o.guest());
            }
        })
        .optional();
    b.raw()
        .observe_after(pose, "physics-motion-pose-end", move |o| {
            if o.addr == pose.base {
                parity::motion_pose_end(o.thread_id, o.cpu, &o.guest());
            }
        })
        .optional();
    let forces = &points::HAVOK_MOTION_FORCES;
    b.raw()
        .observe_before(forces, "physics-motion-forces-begin", move |o| {
            if o.addr == forces.base {
                parity::motion_forces_begin(o.thread_id, o.cpu, &o.guest());
            }
        })
        .optional();
    b.raw()
        .observe_after(forces, "physics-motion-forces-end", move |o| {
            if o.addr == forces.base {
                parity::motion_forces_end(o.thread_id, o.cpu, &o.guest());
            }
        })
        .optional();
    let mopp = &points::COLLISION_MOPP_BOX;
    b.raw()
        .observe_before(mopp, "physics-mopp-box-begin", move |o| {
            if o.addr == mopp.base {
                parity::mopp_box_begin(o.thread_id, o.cpu, &mut o.guest());
            }
        })
        .optional();
    b.raw()
        .observe_after(mopp, "physics-mopp-box-end", move |o| {
            if o.addr == mopp.base {
                parity::mopp_box_end(o.thread_id, o.cpu, &o.guest());
            }
        })
        .optional();
    // Every arm proves itself again at each map load, so lockstep peers
    // and a film replay take over on the same calls.
    b.raw().observe_before(MAP_LOAD, "physics-takeover-restart", |_| parity::map_loaded());
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
        for port in PHYSICS_PORTS.iter().chain([&engine::sim_physics::ISLAND_INTEGRATE]) {
            let chain = &plan.chains[&port.point.base];
            let hook = chain.implement.as_ref().unwrap();
            assert_eq!(hook.label, PHYSICS);
            assert_eq!(hook.continuation_sites().0, port.resumes);
        }
        // The broadphase batch is one implementation; both motion methods
        // and the MOPP box query keep their observer pairs.
        let chain = &plan.chains[&points::HAVOK_BROADPHASE_BATCH.base];
        assert_eq!(chain.implement.as_ref().unwrap().label, PHYSICS);
        assert!(chain.observe_before.is_empty() && chain.observe_after.is_empty());
        for point in [
            &points::HAVOK_MOTION_POSE,
            &points::HAVOK_MOTION_FORCES,
            &points::COLLISION_MOPP_BOX,
        ] {
            let chain = &plan.chains[&point.base];
            assert!(chain.implement.is_none(), "{:?}", point.name);
            assert_eq!((chain.observe_before.len(), chain.observe_after.len()), (1, 1), "{:?}", point.name);
            assert!(chain.observe_before[0].optional && chain.observe_after[0].optional);
        }
        // The proof restarts at the map load.
        let chain = &plan.chains[&MAP_LOAD.base];
        assert!(chain.implement.is_none());
        assert_eq!(chain.observe_before.len(), 1);
        assert_eq!(
            plan.chains.len(),
            PHYSICS_PORTS.len() + 6,
            "unexpected extra chains"
        );
    }
}
