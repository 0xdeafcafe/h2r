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
mod hs_dispatch;
mod hs_eval;
mod hs_globals;
pub mod hs_parity;
mod hs_thread;

const HS: &str = "sim.native.hs";
const HS_POINTS: &[&points::Point] = &[
    &points::HS_THREAD_EXECUTE,
    &points::HS_GLOBAL_READ,
    &points::HS_GLOBAL_STORED,
    &points::HS_EVALUATE,
    &points::HS_EVALUATE_ARGUMENTS,
    &points::HS_NEW_MAP,
];

pub struct SimNative {
    missing_hs_entries: Vec<&'static str>,
    exported_anim: Vec<AnimBody>,
}

impl SimNative {
    pub fn new(runtime: &runtime::Runtime) -> Self {
        runtime.read_dispatch(|d| Self {
            missing_hs_entries: HS_POINTS.iter().filter(|p| !d.has_generated(p.base))
                .map(|p| p.name).collect(),
            exported_anim: ANIM_BODIES.iter().copied().filter(|body| {
                d.has_generated(body.point.base) && body.sites.iter().all(|&pc| d.has_resume(pc))
            }).collect(),
        })
    }

    /// An image with all animation resume selectors.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self { missing_hs_entries: Vec::new(), exported_anim: ANIM_BODIES.to_vec() }
    }
}

pub(crate) type Body = fn(&mut CpuAbi, &mut Guest) -> sim::Result;
pub(crate) const ENTRIES: &[(&points::Point, &str, Body)] = &[
    (
        &points::OBJECT_TRANSFORM_COMPOSE,
        "sim.port.transform_compose",
        sim::compose,
    ),
    // The root transform is two or three transform compositions.
    (
        &points::OBJECT_ROOT_TRANSFORM,
        "sim.port.transform_compose",
        sim::root::transform,
    ),
    (
        &points::OBJECT_POSE_MATRIX,
        "sim.port.pose_matrix",
        sim::pose_matrix,
    ),
    (
        &points::SIM_UNIT_POSE,
        "sim.port.unit_pose",
        sim::unit::pose,
    ),
    (
        &points::SIM_MARKER_TRANSFORMS,
        "sim.port.marker_transforms",
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
        // The hand ports at points no subsystem body owns: the transform
        // composition, the root transform and the pose matrix, reached
        // from guest callers outside the object bodies.
        for &(point, name, body) in ENTRIES {
            let owned = owns_anim(point.base)
                || crate::sim_objects::owns(point.base)
                || crate::sim_seats::owns(point.base);
            crate::shadow::register(name, crate::shadow::SIM_NATIVE, point, !owned);
            if owned {
                continue;
            }
            b.raw()
                .implement(point, name, move |cpu, ctx, addr| {
                    if addr != point.base {
                        return original(cpu, ctx, addr);
                    }
                    if !sim::supported(cpu) {
                        outside(name);
                    }
                    let mut mem = ctx.memory();
                    crate::shadow::run(name, cpu, &mut mem, addr, &mut |cpu, mem| body(cpu, mem))
                })
                .context_only();
        }
        if b.switches().implementation(HS, false) {
            if self.missing_hs_entries.is_empty() {
                hs_thread::install(b);
                hs_eval::install(b);
                hs_globals::install(b);
                hs_dispatch::install(b);
            } else {
                eprintln!("HaloScript parity requires these generated entries: {}",
                    self.missing_hs_entries.join(", "));
            }
        }
        install_anim(b, &self.exported_anim);
        // The native collision world answers every collision query; there
        // is no switch and no guest arm (docs/engine/collision-queries.md).
        install_collision(b);
        crate::sim_objects::install(b);
        // Every effect port, always (crate::sim_effects).
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
            engine::sim_collision::invalidate();
            Ok(())
        }));
        Ok(())
    }
}

/// Calls each hand port ran outside the float environment its arithmetic
/// assumes, by label. The port still answered.
static OUTSIDE: std::sync::Mutex<std::collections::BTreeMap<&'static str, u64>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

fn outside(name: &'static str) {
    *OUTSIDE.lock().unwrap_or_else(|e| e.into_inner()).entry(name).or_default() += 1;
}

/// Every hand port's count of calls run outside its float environment,
/// by label (`sim.port.*`).
pub fn outside_counts() -> Vec<(&'static str, u64)> {
    OUTSIDE.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|(&k, &v)| (k, v)).collect()
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

/// Animation sampling and pose (docs/engine/sim-rewrite.md), always
/// installed, and the label of its implementations. The group owns the
/// channel manager, graph resolve, stream, marker, unit pose, unit aim and
/// first-person filter entries and the graph walk; the palette, marker and
/// filter kernels run inside them. Each answers from Rust; a call a
/// kernel declines is the guest's, counted by reason.
pub const ANIM: &str = "sim.native.anim";

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
pub(crate) fn owns_anim(addr: u32) -> bool {
    addr == points::SIM_MARKER_TRANSFORMS.base || ANIM_BODIES.iter().any(|body| body.point.base == addr)
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
                // A kernel decline, counted by reason in its own stats
                // (`engine::sim_anim`, `manager`): the guest runs it.
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
            // The kernel checks the float environment itself and counts a
            // decline by reason (`engine::sim_anim::stats`).
            if addr == point.base {
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
    // kernel inside it (phase 1c). A float environment off the port's is
    // counted and still answered.
    let marker = &points::SIM_MARKER_TRANSFORMS;
    b.raw()
        .implement(marker, ANIM, move |cpu, ctx, addr| {
            if addr != marker.base {
                return original(cpu, ctx, addr);
            }
            if !sim::supported(cpu) {
                outside(MARKER);
            }
            let mut mem = ctx.memory();
            sim::marker::transforms(cpu, &mut *mem)
        })
        .context_only();
    // The graph walk `1dd1c0`: the body answers every call it can plan
    // (docs/engine/skeleton-walk.md, "The graph walk `1dd1c0`"); a
    // declined plan is the guest's, counted by reason.
    let walk = &points::ANIM_GRAPH_MATRICES;
    b.raw()
        .implement(walk, ANIM, move |cpu, ctx, addr| {
            if addr == walk.base {
                let mut mem = ctx.memory();
                if let Some(transfer) = engine::sim_anim::graph_walk::run(cpu, &mut *mem)? {
                    return Ok(transfer);
                }
            }
            original(cpu, ctx, addr)
        })
        .context_only()
        .optional();
}

/// The marker port's label in [`outside_counts`].
const MARKER: &str = "sim.anim.marker_transforms";

/// Drop the derived animation state.
fn observe_anim_invalidate(b: &mut Builder) {
    for (point, label) in ANIM_INVALIDATE {
        b.raw().observe_before(point, label, |_| engine::sim_anim::invalidate());
    }
}

/// The collision queries' hook label. The native collision world is the
/// only path, with no switch (docs/engine/collision-queries.md).
pub const COLLISION: &str = "sim.native.collision";

/// The chains [`install_collision`] adds to a plan that already holds the
/// animation invalidation: the map load observer is shared with it.
#[cfg(test)]
pub(crate) const COLLISION_CHAINS: usize = COLLISION_ENTRIES.len()
    + COLLISION_INVALIDATE.len()
    - 1
    + COLLISION_PARTIAL.len()
    + COLLISION_OBSERVED.len()
    + RAY_PARITY.len();

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

/// `plan` with only the animation group's implementations and the
/// observers at its invalidation points.
#[cfg(test)]
pub(crate) fn only_anim(plan: &mut hooks::Plan) {
    plan.chains.retain(|&addr, c| {
        if c.implement.as_ref().is_some_and(|h| h.label != ANIM) {
            c.implement = None;
        }
        c.implement.is_some() || ANIM_INVALIDATE.iter().any(|(p, _)| p.base == addr)
    });
    for chain in plan.chains.values_mut() {
        chain.before.clear();
        chain.after.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hooks::{Base, Registry, Switches};

    #[test]
    fn hand_ports_install_where_no_subsystem_body_owns_the_point() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        for &(point, name, _) in ENTRIES {
            let hook = plan.chains[&point.base].implement.as_ref().unwrap();
            if owns_anim(point.base) {
                assert_eq!(hook.label, ANIM, "{name}");
            } else {
                assert!(!crate::sim_objects::owns(point.base), "{name}");
                assert_eq!(hook.label, name);
            }
        }
        // The transform composition, root transform and pose matrix have
        // no subsystem body; the unit pose and marker lookup are anim's.
        assert!(!owns_anim(points::OBJECT_TRANSFORM_COMPOSE.base));
        assert!(owns_anim(points::SIM_UNIT_POSE.base) && owns_anim(points::SIM_MARKER_TRANSFORMS.base));
        // The per-function switches are gone.
        for key in [
            "impl.sim.native_transform_compose",
            "impl.sim.native_pose_matrix",
            "impl.sim.native_unit_pose",
            "impl.sim.native_marker_transforms",
            "impl.sim.native_unit_aim",
            "impl.sim.native_first_person_pose",
            "impl.sim.native_first_person_filters",
        ] {
            assert!(Switches::parse(key).is_err(), "{key}");
        }
    }

    #[test]
    fn hs_switch_installs_guest_observers_for_the_complete_image() {
        let plan_for = |spec, feature: SimNative| {
            let mut registry = Registry::new(Switches::parse(spec).unwrap());
            registry.add(&feature).unwrap();
            registry.build(&Base).unwrap()
        };
        for (spec, installed) in [
            ("", false),
            ("impl.sim.native.hs", true),
            ("-impl.sim.native", false),
            ("impl.sim.native,-impl.sim.native.hs", false),
            ("-impl.sim.native,impl.sim.native.hs", true),
        ] {
            let plan = plan_for(spec, SimNative::for_tests());
            for point in HS_POINTS {
                let observed = plan.chains.get(&point.base).is_some_and(|chain| {
                    chain.implement.is_none() && !chain.observe_before.is_empty()
                });
                assert_eq!(observed, installed, "{spec}: {}", point.name);
            }
        }
        let mut incomplete = SimNative::for_tests();
        incomplete.missing_hs_entries.push(points::HS_THREAD_EXECUTE.name);
        let plan = plan_for("impl.sim.native.hs", incomplete);
        for point in HS_POINTS {
            assert!(!plan.chains.contains_key(&point.base), "{}", point.name);
        }
    }

    #[test]
    fn anim_replaces_the_stream_call_and_invalidates_on_reload() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        let stream = &plan.chains[&points::SIM_ANIMATION_STREAM.base];
        assert_eq!(stream.implement.as_ref().unwrap().label, ANIM);
        let marker = &plan.chains[&points::SIM_MARKER_TRANSFORMS.base];
        assert_eq!(marker.implement.as_ref().unwrap().label, ANIM);
        for (point, label) in ANIM_INVALIDATE {
            let chain = &plan.chains[&point.base];
            assert!(chain.implement.is_none());
            assert!(chain.observe_before.iter().any(|h| h.label == label), "{label}");
        }
        let pose = plan.chains[&points::SIM_UNIT_POSE.base].implement.as_ref().unwrap();
        assert_eq!(pose.label, ANIM);
        assert_eq!(pose.continuation_sites().0, sim::unit::SITES);
        // The graph walk is the body's, with no compare hooks.
        let walk = &plan.chains[&points::ANIM_GRAPH_MATRICES.base];
        assert_eq!(walk.implement.as_ref().unwrap().label, ANIM);
        assert_eq!((walk.before.len(), walk.after.len()), (0, 0));
        assert_eq!(plan.save_sections(), vec![("sim-native", Vec::new())]);
        // The subsystem and the retired per-function switches are gone.
        for key in ["impl.sim.native.anim", "impl.sim.native_rotation_codec", "impl.sim.native_animation_stream", "impl.sim.native_graph_resolve"] {
            assert!(Switches::parse(key).is_err(), "{key}");
        }
    }

    #[test]
    fn animation_owns_aim_and_filters_and_the_first_person_pose_port_installs() {
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        registry.add(&crate::sim_physics::NativePhysics).unwrap();
        let plan = registry.build(&Base).unwrap();
        assert!(!crate::sim_objects::owns(points::UNIT_AIM_UPDATE.base));
        for (point, sites) in [
            (&points::UNIT_AIM_UPDATE, engine::sim_anim::UNIT_AIM_SITES),
            (&points::FIRST_PERSON_FILTER_UPDATE, engine::sim_anim::FIRST_PERSON_FILTER_SITES),
        ] {
            let hook = plan.chains[&point.base].implement.as_ref().unwrap();
            assert_eq!(hook.label, ANIM, "{}", point.name);
            assert_eq!(hook.continuation_sites().0, sites);
        }
        // The movement that calls the aim update is an object body.
        let movement = plan.chains[&points::UNIT_MOVEMENT.base].implement.as_ref().unwrap();
        assert_eq!(movement.label, crate::sim_objects::OBJECTS);
        // The first-person pose has no subsystem body: its port installs.
        let pose = plan.chains[&points::FIRST_PERSON_POSE_UPDATE.base].implement.as_ref().unwrap();
        assert_eq!(pose.label, "sim.port.first_person_pose");
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
        let mut registry = Registry::new(Switches::defaults());
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
            let mut registry = Registry::new(Switches::defaults());
            registry.add(&SimNative::new(&rt)).unwrap();
            let mut plan = registry.build(&Base).unwrap();
            // Only the animation manager is under test: the object bodies,
            // the hand ports and the effect ports always install but need
            // their own originals.
            only_anim(&mut plan);
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
            let mut registry = Registry::new(Switches::defaults());
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
        let mut registry = Registry::new(Switches::defaults());
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
            // The map load is also observed by the animation invalidation
            // and the object arms' resets.
            let shared = if point.base == points::CACHE_TAGS_LOAD.base { 4 } else { 1 };
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
        // No switch turns the collision world off: `-impl.sim.native`
        // leaves every entry in place.
        let mut registry = Registry::new(Switches::parse("-impl.sim.native").unwrap());
        registry.add(&SimNative::for_tests()).unwrap();
        let off = registry.build(&Base).unwrap();
        for (point, entry, _) in COLLISION_ENTRIES {
            assert!(off.chains[&point.base].implement.is_some(), "{:#x}", entry.addr());
        }
        assert_eq!(plan.save_sections(), vec![("sim-native", Vec::new())]);
        assert!(hooks::Switches::parse(&format!("impl.{COLLISION}")).is_err(), "the switch is deleted");
    }

    #[test]
    fn collision_entries_count_the_environment_before_declining() {
        let mut registry = Registry::new(Switches::defaults());
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
        let mut registry = Registry::new(Switches::defaults());
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
