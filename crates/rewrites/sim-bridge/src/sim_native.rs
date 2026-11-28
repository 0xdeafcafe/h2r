//! Independently selectable simulation replacements for the base Xbox image.
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use engine::sim_anim::manager;
use engine::sim_collision::{Bail, Entry};
use engine::{points, sim_native as sim};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};
use std::sync::Arc;

mod manager_arm;
mod manager_fusion;
#[cfg(test)]
mod hs_differential;

pub struct SimNative {
    exported_anim: Vec<AnimBody>,
}

impl SimNative {
    pub fn new(runtime: &runtime::Runtime) -> Self {
        runtime.read_dispatch(|d| Self {
            exported_anim: ANIM_BODIES.iter().copied().filter(|body| {
                d.has_generated(body.point.base) && body.sites.iter().all(|&pc| d.has_resume(pc))
            }).collect(),
        })
    }

    /// An image with all animation resume selectors.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self { exported_anim: ANIM_BODIES.to_vec() }
    }
}

pub(crate) type Body = fn(&mut CpuAbi, &mut Guest) -> sim::Result;
pub(crate) const ENTRIES: &[(&points::Point, &str, Body)] = &[
    (
        &points::OBJECT_TRANSFORM_COMPOSE,
        "sim.native_transform_compose",
        sim::compose,
    ),
    // The root transform is two or three transform compositions.
    (
        &points::OBJECT_ROOT_TRANSFORM,
        "sim.native_transform_compose",
        sim::root::transform,
    ),
    (
        &points::OBJECT_POSE_MATRIX,
        "sim.native_pose_matrix",
        sim::pose_matrix,
    ),
    (
        &points::SIM_UNIT_POSE,
        "sim.native_unit_pose",
        sim::unit::pose,
    ),
    (
        &points::SIM_MARKER_TRANSFORMS,
        "sim.native_marker_transforms",
        sim::marker::transforms,
    ),
];

/// `impl.sim.native_simd`: host vector lanes inside the native bodies'
/// packed SSE fast path. On by default, so it follows the native ports;
/// off keeps the scalar per-lane loop for A/B timing. Both are exact.
fn simd_on(switches: &hooks::Switches) -> bool {
    switches.implementation("sim.native_simd", true)
}

impl Feature for SimNative {
    fn manifest(&self) -> Manifest {
        Manifest::new("sim-native", "Native simulation functions").doc("docs/perf/sim-budget.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        sim::set_simd(simd_on(b.switches()));
        let anim = anim_on(b.switches());
        engine::sim_anim::markers::set_enabled(anim);
        engine::sim_anim::palette::set_enabled(anim);
        engine::sim_anim::filter::set_enabled(anim);
        let collision = collision_on(b.switches());
        for &(point, name, body) in ENTRIES {
            if owns_anim(b.switches(), point.base) {
                continue;
            }
            if crate::sim_objects::owns(b.switches(), point.base) {
                continue;
            }
            if crate::sim_seats::owns(point.base) {
                continue;
            }
            let installed = b.switches().implementation(name, false);
            crate::shadow::register(name, crate::shadow::SIM_NATIVE, point, installed);
            if installed {
                b.raw()
                    .implement(point, name, move |cpu, ctx, addr| {
                        if addr == point.base && sim::supported(cpu) {
                            let mut mem = ctx.memory();
                            return crate::shadow::run(
                                name,
                                cpu,
                                &mut mem,
                                addr,
                                &mut |cpu, mem| body(cpu, mem),
                            );
                        }
                        original(cpu, ctx, addr)
                    })
                    .context_only();
            }
        }
        // Phase 1d's manager and graph entries always install: each answers
        // from Rust once proven (docs/engine/animation-manager.md).
        if anim {
            install_anim(b, &self.exported_anim);
        } else {
            install_anim_bodies(b, &manager_arms(&self.exported_anim));
            observe_anim_invalidate(b);
        }
        if collision {
            install_collision(b);
        }
        crate::sim_objects::install(b);
        // Registers the effect ports with the shadow registry whatever
        // the switch says; installs them when it is on.
        crate::sim_effects::install(b);
        // Phase 7: the seats (crate::sim_seats), and the aim-assist,
        // respawn-gate and pickup-scan kernels (crate::present), always.
        crate::sim_seats::install(b);
        crate::present::install(b);
        // The clip store and the collision world are derived from guest
        // memory and hold no state of their own; a load only has to
        // invalidate them.
        b.snapshot(SnapshotPolicy::section(Vec::new, move |_, _| {
            engine::sim_anim::invalidate();
            if collision {
                engine::sim_collision::invalidate();
            }
            Ok(())
        }));
        Ok(())
    }
}

/// The original at `addr`, for calls a native body declines.
fn original(cpu: &mut CpuAbi, ctx: &mut ExecCtx, addr: u32) -> Result<TransferAbi, GuestFault> {
    if let Some(original) = ctx.runtime().resolve_original(addr) {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let event = ctx.emit(runtime::Event::UnsupportedTarget {
        pc: cpu.eip,
        target: addr,
        kind: runtime::abi::JUMP,
        route: runtime::abi::ENTRY,
    });
    Ok(TransferAbi::fault(event))
}

/// Animation sampling and pose (docs/engine/sim-rewrite.md). The group
/// owns the channel manager, graph resolve, stream, marker, unit pose, unit
/// aim and first-person filter entries and enables the palette and filter
/// kernels. It compares the graph walk at each call and the six phase 1d
/// manager and graph entries at each generated transfer.
pub const ANIM: &str = "sim.native.anim";

pub(crate) fn anim_on(switches: &hooks::Switches) -> bool {
    switches.implementation(ANIM, false)
}

/// The phase 1d rows of `exports`.
fn manager_arms(exports: &[AnimBody]) -> Vec<AnimBody> {
    exports.iter().copied().filter(|b| matches!(b.run, AnimRun::ManagerArm)).collect()
}

/// Points after which guest memory no longer matches the clip store: map
/// load, checkpoint revert and full restore (snapshot load is the
/// section's `load`).
const ANIM_INVALIDATE: [(&points::Point, &str); 3] = [
    (&points::CACHE_TAGS_LOAD, "anim-invalidate-map"),
    (&points::GAME_STATE_REVERT, "anim-invalidate-revert"),
    (&points::GAME_STATE_RESTORE, "anim-invalidate-restore"),
];

#[derive(Clone, Copy)]
enum AnimRun {
    Pose(fn(&mut CpuAbi, &mut Guest, u32) -> Result<Option<TransferAbi>, GuestFault>),
    Manager,
    // Its segments always dispatch. Fusion only indexes Manager rows.
    ManagerArm,
}

#[derive(Clone, Copy)]
struct AnimBody {
    point: &'static points::Point,
    sites: &'static [u32],
    run: AnimRun,
}

const POSE_BODIES: [AnimBody; 3] = [
    AnimBody {
        point: &points::SIM_UNIT_POSE,
        sites: sim::unit::SITES,
        run: AnimRun::Pose(sim::unit::run),
    },
    AnimBody {
        point: &points::UNIT_AIM_UPDATE,
        sites: engine::sim_anim::UNIT_AIM_SITES,
        run: AnimRun::Pose(engine::sim_anim::unit_aim),
    },
    AnimBody {
        point: &points::FIRST_PERSON_FILTER_UPDATE,
        sites: engine::sim_anim::FIRST_PERSON_FILTER_SITES,
        run: AnimRun::Pose(engine::sim_anim::first_person_filters),
    },
];

// A manager row owns its static point. When the hook catalogue already
// names the entry, use that point's signature and image-map name.
static ANIM_BODIES: std::sync::LazyLock<Vec<AnimBody>> = std::sync::LazyLock::new(|| {
    POSE_BODIES.into_iter().chain(manager::bodies().map(|body| AnimBody {
        point: points::by_base(body.entry).unwrap_or(&body.point),
        sites: body.sites,
        run: if manager::arm::ENTRIES.contains(&body.entry) {
            AnimRun::ManagerArm
        } else {
            AnimRun::Manager
        },
    })).collect()
});

/// The per-function ports stand down even on an older image: without a
/// complete set of selectors its generated implementation owns this point.
pub(crate) fn owns_anim(switches: &hooks::Switches, addr: u32) -> bool {
    anim_on(switches) && (addr == points::SIM_MARKER_TRANSFORMS.base
        || ANIM_BODIES.iter().any(|body| body.point.base == addr))
}

fn install_anim(b: &mut Builder, exports: &[AnimBody]) {
    install_anim_bodies(b, exports);
    install_anim_points(b);
    observe_anim_invalidate(b);
}

/// The pose bodies and manager rows in `exports`.
fn install_anim_bodies(b: &mut Builder, exports: &[AnimBody]) {
    let fusion = Arc::new(manager_fusion::Fusion::new(exports));
    for &body in exports {
        let run_fusion = Arc::clone(&fusion);
        let hook = b.raw()
            .implement(body.point, ANIM, move |cpu, ctx, pc| {
                let transfer = match body.run {
                    AnimRun::Pose(run) => run(cpu, &mut ctx.memory(), pc)?,
                    AnimRun::ManagerArm => {
                        return manager_arm::run(cpu, ctx, pc, pc != body.point.base);
                    }
                    AnimRun::Manager => {
                        let available = run_fusion.available(ctx.runtime());
                        manager::drive(cpu, &mut ctx.memory(), pc, |next| available(next.entry))?
                    }
                };
                if let Some(transfer) = transfer {
                    return Ok(transfer);
                }
                crate::sim_objects::original(cpu, ctx, pc, body.sites.contains(&pc))
            })
            .resumes(body.sites)
            .context_only();
        if matches!(body.run, AnimRun::Manager) {
            let guard_fusion = Arc::clone(&fusion);
            hook.optional().when_shared(move |entry| guard_fusion.guard(entry));
        } else if matches!(body.run, AnimRun::ManagerArm) {
            hook.optional();
        }
    }
}

/// The stream, marker and graph walk points of the animation group.
fn install_anim_points(b: &mut Builder) {
    let point = &points::SIM_ANIMATION_STREAM;
    b.raw()
        .implement(point, ANIM, move |cpu, ctx, addr| {
            if addr == point.base && sim::supported(cpu) {
                let mut mem = ctx.memory();
                if let Some(transfer) =
                    engine::sim_anim::stream(cpu, &mut *mem, &engine::sim_anim::STORE)?
                {
                    return Ok(transfer);
                }
            }
            original(cpu, ctx, addr)
        })
        .context_only();
    // The marker lookup, run by the register-level port with the Rust
    // kernel inside it (phase 1c). Its per-function switch stays for the
    // oracle; this entry wins when both are on.
    let marker = &points::SIM_MARKER_TRANSFORMS;
    b.raw()
        .implement(marker, ANIM, move |cpu, ctx, addr| {
            if addr == marker.base && sim::supported(cpu) {
                let mut mem = ctx.memory();
                return sim::marker::transforms(cpu, &mut *mem);
            }
            original(cpu, ctx, addr)
        })
        .context_only();
    // The graph walk `1dd1c0`: the body answers once the arm has matched
    // enough guest answers with no mismatch; until then, and on every
    // audit slot after, the guest answers and the plan made at the entry
    // is compared at the return (docs/engine/skeleton-walk.md, "The graph
    // walk `1dd1c0`").
    let walk = &points::ANIM_GRAPH_MATRICES;
    b.raw()
        .implement(walk, ANIM, move |cpu, ctx, addr| {
            use engine::sim_anim::graph_walk::{self, Step};
            if addr == walk.base {
                let step = {
                    let mut mem = ctx.memory();
                    graph_walk::enter(cpu, &mut *mem)?
                };
                match step {
                    Step::Answered(transfer) => return Ok(transfer),
                    Step::Audit(audit) => {
                        let seen = ctx.observe_guest_return(cpu, graph_walk::ARGS, move |c, ctx| {
                            graph_walk::audit(*audit, c, &*ctx.runtime().memory());
                        });
                        if !seen {
                            graph_walk::arm_dropped();
                        }
                    }
                    Step::Guest => {}
                }
            }
            original(cpu, ctx, addr)
        })
        .context_only()
        .optional();
}

/// Drop the derived animation state and restart the arms' audit slots.
fn observe_anim_invalidate(b: &mut Builder) {
    for (point, label) in ANIM_INVALIDATE {
        b.raw()
            .observe_before(point, label, |_| {
                engine::sim_anim::invalidate();
                engine::sim_anim::graph_walk::arm_new_run();
                manager::arm::new_run();
            });
    }
}

/// Phase two collision queries and their return comparisons.
/// See docs/engine/collision-queries.md for the admission rules.
pub const COLLISION: &str = "sim.native.collision";

fn collision_on(switches: &hooks::Switches) -> bool {
    switches.implementation(COLLISION, false)
}

type CollisionBody = fn(
    &mut CpuAbi,
    &mut Guest,
    &engine::sim_collision::CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault>;

const COLLISION_ENTRIES: [(&points::Point, Entry, CollisionBody); 5] = [
    (
        &points::COLLISION_POINT_TO_LEAF,
        Entry::PointToLeaf,
        engine::sim_collision::point_to_leaf,
    ),
    (
        &points::COLLISION_LOCATION,
        Entry::Location,
        engine::sim_collision::location,
    ),
    (
        &points::COLLISION_INSTANCE_TRACE,
        Entry::InstanceTrace,
        engine::sim_collision::instance::empty_cluster,
    ),
    (
        &points::COLLISION_MOPP_BOX,
        Entry::MoppBox,
        engine::sim_collision::mopp_box,
    ),
    (
        &points::COLLISION_GEOMETRY_TEST,
        Entry::ObjectSphere,
        engine::sim_collision::object_sphere_pretest,
    ),
];

// Each body answers exact exits, then compares admitted lifted calls at their
// real returns. A return continuation in guest RAM could alias query inputs.
const COLLISION_PARTIAL: [(&points::Point, Entry); 4] = [
    (&points::COLLISION_VECTOR_TEST, Entry::VectorTest),
    (&points::COLLISION_BSP_SEGMENT, Entry::VectorContext),
    (&points::COLLISION_CLUSTER_FLOOD, Entry::ClusterFlood),
    (&points::OBJECT_SPATIAL_VISIT, Entry::ObjectVisit),
];

const COLLISION_OBSERVED: [(&points::Point, Entry, &str); 2] = [
    (&points::COLLISION_MOPP_RAY, Entry::MoppRay, "collision-ray-fp"),
    (&points::SIM_COLLISION_QUERY, Entry::Query, "collision-query"),
];

/// The rest of the general MOPP ray's parity arm: each interpreter call,
/// each key the instanced collection accepts, and the setup's return.
const RAY_PARITY: [(&points::Point, &str); 2] = [
    (&points::SIM_MOPP_RAY, "collision-ray-enter"),
    (&points::COLLISION_INSTANCE_CHILD, "collision-ray-key"),
];

/// Points after which the collision BSPs in guest memory may differ from
/// the decoded ones: map load and structure switch (snapshot load is the
/// section's `load`). Checkpoint revert and restore leave tag data alone.
const COLLISION_INVALIDATE: [(&points::Point, &str); 2] = [
    (&points::CACHE_TAGS_LOAD, "collision-invalidate-map"),
    (&points::BSP_SWITCH, "collision-invalidate-bsp"),
];

fn install_collision(b: &mut Builder) {
    for (point, entry, body) in COLLISION_ENTRIES {
        b.raw()
            .implement(point, COLLISION, move |cpu, ctx, addr| {
                if addr != point.base {
                    return original(cpu, ctx, addr);
                }
                let world: &'static engine::sim_collision::CollisionWorld = &engine::sim_collision::WORLD;
                world.observe_environment(entry, cpu.mxcsr, cpu.x87.control);
                if sim::supported(cpu) {
                    let mut mem = ctx.memory();
                    if mem.has_trap() {
                        world.record::<()>(entry, &Err(Bail::Trap));
                    } else if let Some(transfer) = body(cpu, &mut mem, world)? {
                        return Ok(transfer);
                    } else if entry == Entry::InstanceTrace {
                        let prediction = engine::sim_collision::instance::ReturnPrediction::gather(cpu, &mut *mem, world);
                        drop(mem);
                        match prediction {
                            Ok(p) => {
                                ctx.observe_guest_return(cpu, 12, move |c, ctx| {
                                    world.record_instance(p.compare(c, &*ctx.runtime().memory()));
                                });
                            }
                            Err(why) => world.record_instance(Err(why)),
                        }
                    }
                } else {
                    world.record::<()>(entry, &Err(Bail::Env));
                }
                original(cpu, ctx, addr)
            });
    }
    // This family updates shared decode caches, write watches and counters.
    // The hook registry keeps these calls on the owning executor.
    use engine::sim_collision::{WORLD, mopp_ray, query, spatial_parity, vector_parity};
    for (point, entry) in COLLISION_PARTIAL {
        let points::Sig::Fn { pop, .. } = point.sig else {
            unreachable!()
        };
        let pop = u32::from(pop);
        b.raw()
            .implement(point, COLLISION, move |cpu, ctx, addr| {
                if addr != point.base {
                    return original(cpu, ctx, addr);
                }
                let mut g = ctx.memory();
                // The pending prediction is compared at this call's own
                // guest return, so each arm owns its comparison.
                enum Pending {
                    Vector(vector_parity::Prediction),
                    Spatial(spatial_parity::Prediction),
                }
                let (transfer, pending) = match entry {
                    Entry::VectorTest | Entry::VectorContext => {
                        WORLD.observe_environment(entry, cpu.mxcsr, cpu.x87.control);
                        let (t, p) = if entry == Entry::VectorTest {
                            vector_parity::dispatch_vector(cpu, &mut g, &WORLD)?
                        } else {
                            vector_parity::dispatch_context(cpu, &mut g, &WORLD)?
                        };
                        (t, p.map(Pending::Vector))
                    }
                    _ => {
                        let (t, p) = spatial_parity::dispatch(entry, cpu, &mut g, &WORLD)?;
                        (t, p.map(Pending::Spatial))
                    }
                };
                if let Some(transfer) = transfer {
                    return Ok(transfer);
                }
                drop(g);
                if let Some(pending) = pending {
                    ctx.observe_guest_return(cpu, pop, move |c, ctx| {
                        let g = ctx.runtime().memory();
                        let g = &*g;
                        match pending {
                            Pending::Vector(p) => p.settle(c, g, &WORLD),
                            Pending::Spatial(p) => WORLD.record_spatial(entry, p.compare(c, g)),
                        }
                    });
                }
                original(cpu, ctx, addr)
            })
            .optional();
    }
    for (point, entry, label) in COLLISION_OBSERVED {
        let points::Sig::Fn { pop, .. } = point.sig else {
            unreachable!()
        };
        let pop = u32::from(pop);
        b.raw()
            .before(point, label, move |call| {
                if call.addr == point.base {
                    WORLD.observe_environment(entry, call.cpu.mxcsr, call.cpu.x87.control);
                    let thread = call.ctx.thread_id;
                    if entry == Entry::MoppRay {
                        let observation =
                            mopp_ray::begin_observed(thread, call.cpu, &mut *call.ctx.memory(), &WORLD);
                        call.ctx.observe_guest_return(call.cpu, pop, move |_, ctx| {
                            observation.finish(&*ctx.runtime().memory(), &WORLD);
                        });
                    } else if let Some(pending) = {
                        let mut mem = call.ctx.memory();
                        query::observe(call.cpu, &mut mem, &WORLD)
                    } {
                        call.ctx.observe_guest_return(call.cpu, pop, move |c, ctx| {
                            pending.finish(c, &ctx.runtime().memory(), &WORLD);
                        });
                    }
                }
                Ok(hooks::Flow::Continue)
            })
            .optional();
    }
    for (point, label) in RAY_PARITY {
        b.raw()
            .observe_before(point, label, move |o| {
                if o.addr != point.base {
                    return;
                }
                let g = o.guest();
                if point.base == points::SIM_MOPP_RAY.base {
                    engine::sim_collision::mopp_ray::enter(o.thread_id, o.cpu, &*g);
                } else {
                    engine::sim_collision::mopp_ray::key(o.thread_id, o.cpu, &*g);
                }
            })
            .optional();
    }
    for (point, label) in COLLISION_INVALIDATE {
        b.raw()
            .observe_before(point, label, |_| engine::sim_collision::invalidate());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hooks::{Base, Registry, Switches};

    #[test]
    fn switches_install_only_the_requested_body() {
        // The default-on animation manager installs its own chains.
        let baseline = {
            let mut registry = Registry::new(Switches::defaults());
            registry.add(&SimNative::for_tests()).unwrap();
            registry.build(&Base).unwrap().chains.len()
        };
        for enabled in std::iter::once(None).chain(ENTRIES.iter().map(Some)) {
            let mut switches = Switches::defaults();
            if let Some((_, key, _)) = enabled {
                switches = switches.with(&format!("impl.{key}"), true);
            }
            let mut registry = Registry::new(switches);
            registry.add(&SimNative::for_tests()).unwrap();
            let plan = registry.build(&Base).unwrap();
            // A switch installs every entry that shares it, and only those.
            let sharing: Vec<_> = enabled
                .map(|(_, key, _)| ENTRIES.iter().filter(|e| e.1 == *key).collect())
                .unwrap_or_default();
            assert_eq!(plan.chains.len(), baseline + sharing.len());
            for (point, key, _) in sharing {
                assert_eq!(
                    plan.chains[&point.base].implement.as_ref().unwrap().label,
                    *key
                );
            }
        }
    }

    #[test]
    fn anim_replaces_the_stream_call_and_invalidates_on_reload() {
        let mut registry = Registry::new(Switches::defaults().with(&format!("impl.{ANIM}"), true));
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        let stream = &plan.chains[&points::SIM_ANIMATION_STREAM.base];
        assert_eq!(stream.implement.as_ref().unwrap().label, ANIM);
        // The marker lookup runs under the subsystem switch too.
        let marker = &plan.chains[&points::SIM_MARKER_TRANSFORMS.base];
        assert_eq!(marker.implement.as_ref().unwrap().label, ANIM);
        for (point, label) in ANIM_INVALIDATE {
            let chain = &plan.chains[&point.base];
            assert!(chain.implement.is_none());
            assert_eq!(chain.observe_before.len(), 1, "{label}");
        }
        let pose = plan.chains[&points::SIM_UNIT_POSE.base].implement.as_ref().unwrap();
        assert_eq!(pose.label, ANIM);
        assert_eq!(pose.continuation_sites().0, sim::unit::SITES);
        // The graph walk answers once proven; until then it compares.
        let walk = &plan.chains[&points::ANIM_GRAPH_MATRICES.base];
        assert_eq!(walk.implement.as_ref().unwrap().label, ANIM);
        assert_eq!((walk.before.len(), walk.after.len()), (0, 0));
        // The walk table is on by default, without the aim update the
        // subsystem owns, with its event clear.
        let walk = crate::sim_objects::walk_chains_without_aim();
        assert_eq!(plan.chains.len(), 3 + ANIM_BODIES.len() + ANIM_INVALIDATE.len() + walk);
        let aim = plan.chains[&points::UNIT_AIM_UPDATE.base].implement.as_ref().unwrap();
        assert_eq!(aim.label, ANIM);
        assert_eq!(plan.save_sections(), vec![("sim-native", Vec::new())]);

        // The subsystem owns both shared points when their individual
        // port switches are also enabled.
        let switches = Switches::defaults()
            .with(&format!("impl.{ANIM}"), true)
            .with("impl.sim.native_marker_transforms", true)
            .with("impl.sim.native_unit_pose", true);
        let mut registry = Registry::new(switches);
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        let marker = &plan.chains[&points::SIM_MARKER_TRANSFORMS.base];
        assert_eq!(marker.implement.as_ref().unwrap().label, ANIM);
        let pose = plan.chains[&points::SIM_UNIT_POSE.base].implement.as_ref().unwrap();
        assert_eq!(pose.label, ANIM);
        assert_eq!(pose.continuation_sites().0, sim::unit::SITES);
        // The walk table is on by default, without the aim update the
        // subsystem owns, with its event clear.
        let walk = crate::sim_objects::walk_chains_without_aim();
        assert_eq!(plan.chains.len(), 3 + ANIM_BODIES.len() + ANIM_INVALIDATE.len() + walk);
        let aim = plan.chains[&points::UNIT_AIM_UPDATE.base].implement.as_ref().unwrap();
        assert_eq!(aim.label, ANIM);

        // The retired per-function switches are gone from the table.
        for key in ["sim.native_rotation_codec", "sim.native_animation_stream", "sim.native_graph_resolve"] {
            assert!(Switches::parse(&format!("impl.{key}")).is_err(), "{key}");
        }
    }

    fn legacy_ports_and_objects() -> Switches {
        Switches::defaults()
            .with("impl.sim.native.objects", true)
            .with("impl.sim.native_unit_aim", true)
            .with("impl.sim.native_first_person_filters", true)
    }

    #[test]
    fn animation_owns_aim_and_filters_even_for_fused_object_calls() {
        for anim in [false, true] {
            let switches = legacy_ports_and_objects().with("impl.sim.native.anim", anim);
            assert_eq!(crate::sim_objects::owns(&switches, points::UNIT_AIM_UPDATE.base), !anim);
            let mut registry = Registry::new(switches);
            registry.add(&SimNative::for_tests()).unwrap();
            registry.add(&crate::sim_physics::NativePhysics).unwrap();
            let plan = registry.build(&Base).unwrap();
            for (point, sites, legacy) in [
                (&points::UNIT_AIM_UPDATE, engine::sim_anim::UNIT_AIM_SITES, crate::sim_objects::OBJECTS),
                (&points::FIRST_PERSON_FILTER_UPDATE, engine::sim_anim::FIRST_PERSON_FILTER_SITES, "sim.native_first_person_filters"),
            ] {
                let hook = plan.chains[&point.base].implement.as_ref().unwrap();
                assert_eq!(hook.label, if anim { ANIM } else { legacy });
                if anim {
                    assert_eq!(hook.continuation_sites().0, sites);
                }
            }
        }
    }

    /// The walk table makes the same hand-off: its aim update is the
    /// walk's with the subsystem off, the subsystem's with it on.
    #[test]
    fn animation_owns_aim_over_the_walk_table_too() {
        for anim in [false, true] {
            let switches = Switches::defaults().with("impl.sim.native_unit_aim", true).with("impl.sim.native.anim", anim);
            assert!(crate::sim_objects::walk_on(&switches));
            assert_eq!(crate::sim_objects::owns(&switches, points::UNIT_AIM_UPDATE.base), !anim);
            let mut registry = Registry::new(switches);
            registry.add(&SimNative::for_tests()).unwrap();
            registry.add(&crate::sim_physics::NativePhysics).unwrap();
            let plan = registry.build(&Base).unwrap();
            let hook = plan.chains[&points::UNIT_AIM_UPDATE.base].implement.as_ref().unwrap();
            assert_eq!(hook.label, if anim { ANIM } else { crate::sim_objects::WALK_LABEL });
            // The movement that calls it is the walk's either way.
            let movement = plan.chains[&points::UNIT_MOVEMENT.base].implement.as_ref().unwrap();
            assert_eq!(movement.label, crate::sim_objects::WALK_LABEL);
        }
    }

    #[test]
    fn a_declined_animation_segment_invokes_the_original_selector() {
        let mut dispatch = runtime::Dispatch::default();
        // The return PCs can belong to another generated entry. Give it
        // different selectors so a raw return PC cannot pass this check.
        let owner = 0x14000;
        dispatch.register_generated_rust(owner, |_, _, selector| Ok(TransferAbi::ret(selector))).unwrap();
        for body in ANIM_BODIES.iter() {
            dispatch.register_generated_rust(body.point.base, |_, _, selector| Ok(TransferAbi::jump(selector))).unwrap();
            for &pc in body.sites {
                dispatch.register_resume(pc, owner, pc - body.point.base).unwrap();
            }
        }
        let mut guest = Guest::new();
        guest.install_trap(4);
        let rt = runtime::Runtime::new(guest, dispatch);
        let mut registry = Registry::new(Switches::defaults().with("impl.sim.native.anim", true));
        registry.add(&SimNative::new(&rt)).unwrap();
        let plan = registry.build(&Base).unwrap();
        let mut ctx = ExecCtx::new(rt, 1);
        let mut cpu = CpuAbi { mxcsr: 0, ..CpuAbi::default() };
        let initial = cpu;
        for body in ANIM_BODIES.iter() {
            let entry = body.point.base;
            let hook = plan.chains[&entry].implement.as_ref().unwrap();
            let hooks::Run::Implement(run) = &hook.run else { unreachable!() };
            assert_eq!(run(&mut cpu, &mut ctx, entry).unwrap(), TransferAbi::jump(entry), "{entry:#x}");
            assert_eq!(cpu, initial);
            for &pc in body.sites {
                assert_eq!(run(&mut cpu, &mut ctx, pc).unwrap(), TransferAbi::ret(pc - entry), "{pc:#x}");
                assert_eq!(cpu, initial);
            }
        }
    }

    #[test]
    fn manager_uses_exported_points_and_owns_the_graph_port() {
        let graph = manager::bodies().find(|b| b.entry == points::SIM_GRAPH_RESOLVE.base).unwrap();
        let leaf = manager::bodies()
            .find(|b| b.sites.is_empty() && points::by_base(b.entry).is_none())
            .unwrap();
        let last = *graph.sites.last().unwrap();
        for have_last in [false, true] {
            let mut dispatch = runtime::Dispatch::default();
            for (point, _) in ANIM_INVALIDATE {
                dispatch.register_generated_rust(point.base, |_, _, _| panic!("registration only")).unwrap();
            }
            for entry in [leaf.entry, graph.entry] {
                dispatch.register_generated_rust(entry, |_, _, _| panic!("registration only")).unwrap();
            }
            for &site in graph.sites {
                if have_last || site != last {
                    dispatch.register_resume(site, graph.entry, site).unwrap();
                }
            }
            let rt = runtime::Runtime::new(Guest::new(), dispatch);
            let switches = Switches::defaults().with("impl.sim.native.anim", true);
            let mut registry = Registry::new(switches);
            registry.add(&SimNative::new(&rt)).unwrap();
            let mut plan = registry.build(&Base).unwrap();
            // Only the animation manager is under test: the objects walk and the
            // tick's effects phase always install but need their own originals.
            crate::sim_objects::without_walk(&mut plan);
            plan.chains.remove(&points::GAME_TICK.base);
            let installed = plan.chains.get(&graph.entry).and_then(|c| c.implement.as_ref());
            assert_eq!(installed.is_some(), have_last);
            if let Some(graph_hook) = installed {
                assert_eq!(graph_hook.label, ANIM);
                assert!(std::ptr::eq(graph_hook.point, &points::SIM_GRAPH_RESOLVE));
                assert_eq!(graph_hook.continuation_sites().0, graph.sites);
            }
            let leaf_hook = plan.chains[&leaf.entry].implement.as_ref().unwrap();
            assert_eq!(leaf_hook.label, ANIM);
            assert_eq!(leaf_hook.continuation_sites().0, leaf.sites);
            for body in manager::bodies().filter(|b| b.entry != leaf.entry && b.entry != graph.entry) {
                assert!(!plan.chains.contains_key(&body.entry), "{:#x}", body.entry);
            }
            let installed = plan.install(&rt).unwrap();
            assert_eq!(installed.off_reason("sim-native"), None);
            assert!(installed.placed.contains(&leaf.entry));
            assert_eq!(installed.placed.contains(&graph.entry), have_last);
        }
    }

    #[test]
    fn one_missing_export_only_disables_its_animation_body() {
        for missing in POSE_BODIES {
            let mut dispatch = runtime::Dispatch::default();
            for body in POSE_BODIES {
                let entry = body.point.base;
                dispatch.register_generated_rust(entry, |_, _, _| panic!("registration only")).unwrap();
                for &site in body.sites {
                    if entry != missing.point.base || site != missing.sites[0] {
                        dispatch.register_resume(site, entry, site).unwrap();
                    }
                }
            }
            let rt = runtime::Runtime::new(Guest::new(), dispatch);
            let switches = legacy_ports_and_objects()
                .with("impl.sim.native_unit_pose", true)
                .with("impl.sim.native.anim", true);
            let mut registry = Registry::new(switches);
            registry.add(&SimNative::new(&rt)).unwrap();
            registry.add(&crate::sim_physics::NativePhysics).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert!(!plan.chains.contains_key(&missing.point.base));
            for body in POSE_BODIES.into_iter().filter(|b| b.point.base != missing.point.base) {
                let hook = plan.chains[&body.point.base].implement.as_ref().unwrap();
                assert_eq!(hook.label, ANIM);
                assert_eq!(hook.continuation_sites().0, body.sites);
            }
            assert!(plan.chains.contains_key(&points::SIM_ANIMATION_STREAM.base));
            assert!(plan.chains.contains_key(&points::SIM_MARKER_TRANSFORMS.base));
        }
    }

    #[test]
    fn collision_takes_its_entries_and_invalidates_on_reload() {
        let key = format!("impl.{COLLISION}");
        let mut registry = Registry::new(Switches::defaults().with(&key, true));
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for (point, entry, _) in COLLISION_ENTRIES {
            assert_eq!(point.base, entry.addr());
            let chain = &plan.chains[&point.base];
            assert_eq!(chain.implement.as_ref().unwrap().label, COLLISION);
            assert!(chain.render_thread().is_err(), "shares the collision world");
        }
        for (point, label) in COLLISION_INVALIDATE {
            let chain = &plan.chains[&point.base];
            assert!(chain.implement.is_none());
            // The map load is also observed by the default-on animation manager.
            let shared = if point.base == points::CACHE_TAGS_LOAD.base { 2 } else { 1 };
            assert_eq!(chain.observe_before.len(), shared, "{label}");
        }
        for (point, entry, _) in COLLISION_OBSERVED {
            let chain = &plan.chains[&point.base];
            assert_eq!(chain.addr, entry.addr());
            assert!(chain.implement.is_none());
            assert!(chain.shadow.is_none());
            assert_eq!(chain.before.len(), 1);
            assert!(chain.before[0].optional);
        }
        for (point, entry) in COLLISION_PARTIAL {
            let chain = &plan.chains[&point.base];
            assert_eq!(chain.addr, entry.addr());
            let body = chain.implement.as_ref().unwrap();
            assert_eq!(body.label, COLLISION);
            assert!(body.optional);
            assert!(chain.shadow.is_none());
        }
        // The ray parity arm also observes each interpreter call and
        // each instanced key.
        for (point, label) in RAY_PARITY {
            let chain = &plan.chains[&point.base];
            assert!(chain.implement.is_none(), "{label}");
            assert_eq!(chain.observe_before.len(), 1, "{label}");
            assert!(chain.observe_before[0].optional, "{label}");
        }
        // The default-on animation manager adds its own chains, one of
        // which (the map load) the collision invalidation shares.
        let mut base = Registry::new(Switches::defaults());
        base.add(&SimNative::for_tests()).unwrap();
        let manager_chains = base.build(&Base).unwrap().chains.len();
        assert_eq!(
            plan.chains.len(),
            manager_chains - 1
                + COLLISION_ENTRIES.len()
                + COLLISION_INVALIDATE.len()
                + COLLISION_PARTIAL.len()
                + COLLISION_OBSERVED.len()
                + RAY_PARITY.len()
        );
        assert_eq!(plan.save_sections(), vec![("sim-native", Vec::new())]);

        // Off by default: only the animation manager's chains install.
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        assert_eq!(plan.chains.len(), manager_chains);
        for (point, entry, _) in COLLISION_ENTRIES {
            assert!(!plan.chains.contains_key(&point.base), "{:#x}", entry.addr());
        }
    }

    #[test]
    fn collision_entries_count_the_environment_before_declining() {
        let switches = Switches::defaults().with(&format!("impl.{COLLISION}"), true);
        let mut registry = Registry::new(switches);
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        let rt = runtime::Runtime::new(Guest::new(), runtime::Dispatch::default());
        let mut ctx = ExecCtx::new(rt.clone(), 1);
        let mut cpu = CpuAbi {
            mxcsr: 0x1f3d,
            ..CpuAbi::default()
        };
        cpu.x87.control = 0x017f; // Reserved PC; rejected before any memory access.
        let before = engine::sim_collision::stats();
        for (point, _, _) in COLLISION_ENTRIES {
            let hook = plan.chains[&point.base].implement.as_ref().unwrap();
            let hooks::Run::Implement(run) = &hook.run else {
                unreachable!()
            };
            run(&mut cpu, &mut ctx, point.base).unwrap();
        }
        for (point, _) in COLLISION_PARTIAL {
            let hook = plan.chains[&point.base].implement.as_ref().unwrap();
            let hooks::Run::Implement(run) = &hook.run else { unreachable!() };
            run(&mut cpu, &mut ctx, point.base).unwrap();
        }
        for (point, _, _) in COLLISION_OBSERVED {
            let hook = &plan.chains[&point.base].before[0];
            let hooks::Run::Before(run) = &hook.run else { unreachable!() };
            let flow = run(&mut hooks::Call { cpu: &mut cpu, ctx: &mut ctx, addr: point.base, point }).unwrap();
            assert!(matches!(flow, hooks::Flow::Continue));
        }
        let after = engine::sim_collision::stats();
        for entry in Entry::ALL {
            let old = &before.entries[entry as usize];
            let new = &after.entries[entry as usize];
            assert_eq!(
                new.mxcsr[&cpu.mxcsr],
                old.mxcsr.get(&cpu.mxcsr).copied().unwrap_or(0) + 1
            );
            assert_eq!(
                new.x87_control[&cpu.x87.control],
                old.x87_control.get(&cpu.x87.control).copied().unwrap_or(0) + 1
            );
            let env = Bail::Env as usize;
            assert_eq!(
                new.bails[env] - old.bails[env],
                u64::from(COLLISION_ENTRIES.iter().any(|(_, native, _)| *native == entry)
                    || COLLISION_PARTIAL.iter().any(|(_, native)| *native == entry))
            );
        }
    }

    #[test]
    fn collision_leaves_memory_traps_to_the_lifted_instructions() {
        let switches = Switches::defaults().with(&format!("impl.{COLLISION}"), true);
        let mut registry = Registry::new(switches);
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        let mut guest = Guest::new();
        guest.install_trap(4);
        let rt = runtime::Runtime::new(guest, runtime::Dispatch::default());
        let mut ctx = ExecCtx::new(rt, 1);
        let mut cpu = CpuAbi { mxcsr: 0x1fa0, ..CpuAbi::default() };
        let before = engine::sim_collision::stats();
        for (point, _, _) in COLLISION_ENTRIES {
            let hook = plan.chains[&point.base].implement.as_ref().unwrap();
            let hooks::Run::Implement(run) = &hook.run else { unreachable!() };
            let input = cpu;
            run(&mut cpu, &mut ctx, point.base).unwrap();
            assert_eq!(cpu, input);
        }
        let after = engine::sim_collision::stats();
        for (_, entry, _) in COLLISION_ENTRIES {
            let trap = Bail::Trap as usize;
            assert!(after.entries[entry as usize].bails[trap] > before.entries[entry as usize].bails[trap]);
        }
    }

    #[test]
    fn simd_lanes_default_on_and_switch_off_independently() {
        assert!(simd_on(&Switches::defaults()));
        let native = Switches::defaults().with("impl.sim.native_transform_compose", true);
        assert!(simd_on(&native));
        assert!(!simd_on(
            &Switches::defaults().with("impl.sim.native_simd", false)
        ));
        // The sub-switch follows the `impl.sim` group when unset.
        assert!(!simd_on(&Switches::defaults().with("impl.sim", false)));
        assert!(simd_on(
            &Switches::defaults()
                .with("impl.sim", false)
                .with("impl.sim.native_simd", true)
        ));
    }
}
