//! Shared by interactive play, headless simulation and dedicated hosts.
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use guest_layout::points;
use engine::sim_physics::PORTS;
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, Inline, TransferAbi};

/// The contact commit's one resume: the return from its world listener
/// walk.
static COMMIT_SITES: [u32; 1] = [engine::sim_physics::contact_commit::WALK_SITE];

pub struct NativePhysics;

/// The Havok world step, phase 5 of the simulation rewrite
/// (docs/engine/sim-rewrite-havok.md), with no switch. The step wrapper
/// and the island collide replay the retail frame at their resume sites,
/// and the kernels they absorb answer every call they cover
/// (`engine::sim_physics::parity`): the broadphase batch update, the
/// island integrate, the integrate phase, the deactivation test, the
/// structure polygon builder, the convex agent's process call, the
/// closest-feature query, the add-point call and the contact commit. A
/// call a kernel cannot take is counted by reason. The two
/// unconstrained-motion methods and the integer MOPP box query are
/// compared with the guest at every call. The label names the hooks.
pub const PHYSICS: &str = "sim.native.physics";

impl Feature for NativePhysics {
    fn manifest(&self) -> Manifest {
        Manifest::new("native-simulation-physics", "Native simulation updates")
            .doc("docs/perf/sim-budget.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        for port in PORTS {
            // A subsystem body owns this entry and its return sites where
            // there is one; the independent port remains a differential
            // oracle there, and is the implementation everywhere else.
            let installed = !crate::sim_native::owns_anim(port.point.base)
                && !crate::sim_objects::owns(port.point.base);
            crate::shadow::register(port.name, crate::shadow::SIM_PHYSICS, port.point, installed);
            if installed {
                b.raw().implement(port.point, port.name, move |cpu, ctx, pc| {
                    let resume = pc != port.point.base && port.resumes.contains(&pc);
                    if pc != port.point.base && !resume {
                        return original(cpu, ctx, pc, resume);
                    }
                    let mut mem = ctx.memory();
                    // Outside the float environment the port assumes: counted,
                    // and the port still answers.
                    if !engine::sim_physics::supported(port, cpu, &mem) {
                        count_outside(port.name);
                    }
                    // Entries and resumes alike: each segment is checked from
                    // the state it starts in.
                    crate::shadow::run(port.name, cpu, &mut mem, pc, &mut |cpu: &mut CpuAbi, mem: &mut Guest| {
                        port.invoke(cpu, mem, pc)
                    })
                }).resumes(port.resumes).generated_resumes(port.generated_sites).context_only();
            }
        }
        install_physics(b);
        Ok(())
    }
}

/// Calls each standalone port ran outside its float environment, by label.
static OUTSIDE: std::sync::Mutex<std::collections::BTreeMap<&'static str, u64>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

fn count_outside(name: &'static str) {
    *OUTSIDE.lock().unwrap_or_else(|e| e.into_inner()).entry(name).or_default() += 1;
}

/// Every standalone port's count of calls run outside its float
/// environment, by label (`sim.port.*`).
pub fn outside_counts() -> Vec<(&'static str, u64)> {
    OUTSIDE.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(&k, &v)| (k, v)).collect()
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

fn install_physics(b: &mut Builder) {
    use engine::sim_physics::parity::{self, Step};
    for port in engine::sim_physics::PHYSICS_PORTS {
        b.raw()
            .implement(port.point, PHYSICS, move |cpu, ctx, pc| replay(port, cpu, ctx, pc))
            .resumes(port.resumes)
            .context_only()
            .after_feature("sim-submix");
    }
    // The island integrate: `parity::integrate_enter` answers every call
    // its model takes. A declined call, counted by reason, runs the
    // entry's frame replay with its resume sites. Ordered as the other
    // world entries.
    let integrate = &engine::sim_physics::ISLAND_INTEGRATE;
    b.raw()
        .implement(integrate.point, PHYSICS, move |cpu, ctx, pc| {
            if pc == integrate.point.base {
                let step = {
                    let mut mem = ctx.memory();
                    parity::integrate_enter(cpu, &mut mem)
                };
                if let Step::Answered { target } = step {
                    return Ok(TransferAbi::ret(target));
                }
            }
            replay(integrate, cpu, ctx, pc)
        })
        .resumes(integrate.resumes)
        .context_only()
        .after_feature("sim-submix");
    // The integrate phase and the deactivation test answer as the
    // integrate does. Neither has a frame replay: a declined call runs the
    // lifted original, which reaches the integrate and the test through
    // the dispatcher. Ordered as the other world entries.
    let phase = &points::HAVOK_INTEGRATE_PHASE;
    b.raw()
        .implement(phase, PHYSICS, move |cpu, ctx, pc| {
            if pc == phase.base {
                let step = {
                    let mut mem = ctx.memory();
                    parity::phase_enter(cpu, &mut mem)
                };
                if let Step::Answered { target } = step {
                    return Ok(TransferAbi::ret(target));
                }
            }
            original(cpu, ctx, pc, false)
        })
        .context_only()
        .after_feature("sim-submix");
    // The structure polygon builder, as the phase: a declined call runs
    // the lifted original. Ordered as the other world entries.
    let polygon = &points::HAVOK_STRUCTURE_POLYGON;
    b.raw()
        .implement(polygon, PHYSICS, move |cpu, ctx, pc| {
            if pc == polygon.base {
                let step = {
                    let mut mem = ctx.memory();
                    parity::polygon_enter(cpu, &mut mem)
                };
                if let Step::Answered { target } = step {
                    return Ok(TransferAbi::ret(target));
                }
            }
            original(cpu, ctx, pc, false)
        })
        .context_only()
        .after_feature("sim-submix");
    // The convex agent's process call, as the polygon builder: a declined
    // call runs the lifted original. Ordered as the other world entries.
    let convex = &points::HAVOK_CONVEX_PROCESS;
    b.raw()
        .implement(convex, PHYSICS, move |cpu, ctx, pc| {
            if pc == convex.base {
                let step = {
                    let mut mem = ctx.memory();
                    parity::convex_enter(cpu, &mut mem)
                };
                if let Step::Answered { target } = step {
                    return Ok(TransferAbi::ret(target));
                }
            }
            original(cpu, ctx, pc, false)
        })
        .context_only()
        .after_feature("sim-submix");
    // The closest-feature query, as the process call.
    let query = &points::HAVOK_CLOSEST_QUERY;
    b.raw()
        .implement(query, PHYSICS, move |cpu, ctx, pc| {
            if pc == query.base {
                let step = {
                    let mut mem = ctx.memory();
                    parity::query_enter(cpu, &mut mem)
                };
                if let Step::Answered { target } = step {
                    return Ok(TransferAbi::ret(target));
                }
            }
            original(cpu, ctx, pc, false)
        })
        .context_only()
        .after_feature("sim-submix");
    // The add-point call, as the process call. Cdecl: the return pops
    // nothing but its address.
    let add = &points::HAVOK_ADD_POINT;
    b.raw()
        .implement(add, PHYSICS, move |cpu, ctx, pc| {
            if pc == add.base {
                let step = {
                    let mut mem = ctx.memory();
                    parity::point_enter(cpu, &mut mem)
                };
                if let Step::Answered { target } = step {
                    return Ok(TransferAbi::ret(target));
                }
            }
            original(cpu, ctx, pc, false)
        })
        .context_only()
        .after_feature("sim-submix");
    // The contact commit: the Rust answers it to its world listener walk,
    // which calls Halo's contact callback; the walk is called in place and
    // the lifted tail runs from its site.
    let commit = &points::HAVOK_CONTACT_COMMIT;
    b.raw()
        .implement(commit, PHYSICS, move |cpu, ctx, pc| {
            use engine::sim_physics::contact_commit::{WALK_SITE, WORLD_WALK};
            if pc != commit.base {
                return original(cpu, ctx, pc, pc == WALK_SITE);
            }
            let step = {
                let mut mem = ctx.memory();
                parity::commit_enter(cpu, &mut mem)
            };
            if step != parity::Commit::Answered {
                return original(cpu, ctx, pc, false);
            }
            match ctx.call_inline(cpu, WORLD_WALK, WALK_SITE) {
                Inline::Returned => original(cpu, ctx, WALK_SITE, true),
                Inline::Leave { transfer, .. } => Ok(transfer),
            }
        })
        .resumes(&COMMIT_SITES)
        .context_only()
        .after_feature("sim-submix");
    let test = &points::HAVOK_DEACTIVATION_TEST;
    b.raw()
        .implement(test, PHYSICS, move |cpu, ctx, pc| {
            if pc == test.base {
                let step = {
                    let mut mem = ctx.memory();
                    parity::deactivation_enter(cpu, &mut mem)
                };
                if let Step::Answered { target } = step {
                    return Ok(TransferAbi::ret(target));
                }
            }
            original(cpu, ctx, pc, false)
        })
        .context_only()
        .after_feature("sim-submix");
    // The broadphase batch update: `parity::broadphase_enter` answers every
    // call it captures; a declined call, counted, runs the lifted original.
    let batch = &points::HAVOK_BROADPHASE_BATCH;
    b.raw()
        .implement(batch, PHYSICS, move |cpu, ctx, pc| {
            if pc == batch.base {
                let step = {
                    let mut mem = ctx.memory();
                    parity::broadphase_enter(cpu, &mut mem)
                };
                if let Step::Answered { target } = step {
                    return Ok(TransferAbi::ret(target));
                }
            }
            original(cpu, ctx, pc, false)
        })
        .context_only()
        .optional();
    // The compare-only arms: the call runs, and the prediction is compared
    // with what it left. The MOPP box arm observes phase 2's entry, which
    // the native collision world answers (sim_native::install_collision).
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::sim_physics::PHYSICS_PORTS;

    #[test]
    fn every_standalone_port_is_a_label_not_a_switch() {
        for port in PORTS {
            assert!(port.name.starts_with("sim.port."), "{}", port.name);
            let key = format!("impl.{}", port.name);
            assert!(hooks::switches::table::get(&key).is_none(), "{key} is a switch");
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
        let mut registry = hooks::Registry::new(hooks::Switches::parse("-impl.sim.native").unwrap());
        registry.add(&super::NativePhysics).unwrap();
        let plan = registry.build(&hooks::Base).unwrap();
        // A standalone port installs where no subsystem body owns its
        // point: the first-person pose, alone. `-impl.sim.native` leaves
        // the physics entries in place: they have no switch.
        for port in PORTS {
            let installed = plan.chains.get(&port.point.base).and_then(|c| c.implement.as_ref());
            let free = port.point.base == points::FIRST_PERSON_POSE_UPDATE.base;
            assert_eq!(installed.map(|h| h.label), free.then_some(port.name), "{}", port.name);
        }
        for port in PHYSICS_PORTS.iter().chain([&engine::sim_physics::ISLAND_INTEGRATE]) {
            let chain = &plan.chains[&port.point.base];
            let hook = chain.implement.as_ref().unwrap();
            assert_eq!(hook.label, PHYSICS);
            assert_eq!(hook.continuation_sites().0, port.resumes);
        }
        // The integrate phase and the deactivation test are implementations
        // with no resume sites.
        for point in [&points::HAVOK_INTEGRATE_PHASE, &points::HAVOK_DEACTIVATION_TEST, &points::HAVOK_STRUCTURE_POLYGON, &points::HAVOK_CONVEX_PROCESS, &points::HAVOK_CLOSEST_QUERY, &points::HAVOK_ADD_POINT] {
            let hook = plan.chains[&point.base].implement.as_ref().unwrap();
            assert_eq!(hook.label, PHYSICS);
            assert!(hook.continuation_sites().0.is_empty(), "{:?}", point.name);
        }
        // The contact commit resumes at its walk's return only.
        let hook = plan.chains[&points::HAVOK_CONTACT_COMMIT.base].implement.as_ref().unwrap();
        assert_eq!(hook.label, PHYSICS);
        assert_eq!(hook.continuation_sites().0, &COMMIT_SITES[..]);
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
        // No arm audits the guest: the contact walk and the map load are
        // not hooked.
        assert!(!plan.chains.contains_key(&points::HAVOK_CONTACT_WALK.base));
        assert!(!plan.chains.contains_key(&points::CACHE_TAGS_LOAD.base));
        assert_eq!(
            plan.chains.len(),
            PHYSICS_PORTS.len() + 13,
            "unexpected extra chains"
        );
    }
}
