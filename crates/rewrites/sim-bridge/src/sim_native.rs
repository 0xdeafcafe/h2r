//! Independently selectable simulation replacements for the base Xbox image.
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use engine::sim_collision::{Bail, Entry};
use engine::{points, sim_native as sim};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};

pub struct SimNative {
    ray_dispatch: bool,
    pose_resumes: bool,
}

impl SimNative {
    pub fn new(runtime: &runtime::Runtime) -> Self {
        Self {
            pose_resumes: runtime
                .read_dispatch(|d| sim::unit::SITES.iter().all(|&pc| d.has_resume(pc))),
            ray_dispatch: runtime.read_dispatch(|d| d.has_resume(sim::mopp_query::RAY_DISPATCH)),
        }
    }

    /// An image with the ray dispatch and unit pose resume selectors.
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self { ray_dispatch: true, pose_resumes: true }
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
        &points::COLLISION_POINT_TO_LEAF,
        "sim.native_point_to_leaf",
        sim::point_to_leaf,
    ),
    (
        &points::COLLISION_GEOMETRY_TEST,
        "sim.native_geometry_test",
        sim::geometry_test,
    ),
    (
        &points::SIM_UNIT_POSE,
        "sim.native_unit_pose",
        sim::unit::pose,
    ),
    (
        &points::SIM_GRAPH_RESOLVE,
        "sim.native_graph_resolve",
        sim::graph::resolve,
    ),
    (
        &points::SIM_MARKER_TRANSFORMS,
        "sim.native_marker_transforms",
        sim::marker::transforms,
    ),
    (
        &points::COLLISION_BSP_SEGMENT,
        "sim.native_bsp_segment",
        sim::bsp::segment,
    ),
    (
        &points::SIM_COLLISION_QUERY,
        "sim.native_collision_query",
        sim::collision::query,
    ),
    (
        &points::SIM_MOPP_BOX,
        "sim.native_mopp_box",
        sim::mopp_query::box_query,
    ),
    (
        &points::SIM_MOPP_RAY,
        "sim.native_mopp_ray",
        sim::mopp_query::ray_query,
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
        engine::sim_anim::skeleton::set_enabled(skeleton_on(b.switches()));
        let anim = anim_on(b.switches());
        engine::sim_anim::markers::set_enabled(anim);
        engine::sim_anim::palette::set_enabled(anim);
        engine::sim_anim::filter::set_enabled(anim);
        let query_on = b
            .switches()
            .implementation("sim.native_collision_query", false);
        let collision = collision_on(b.switches());
        for &(point, name, body) in ENTRIES {
            // The subsystem switch owns its entries; their per-function
            // ports stay as test oracles.
            if collision
                && (COLLISION_ENTRIES.iter().any(|(p, _, _)| p.base == point.base)
                    || point.base == points::COLLISION_BSP_SEGMENT.base)
            {
                continue;
            }
            if anim
                && (point.base == points::SIM_MARKER_TRANSFORMS.base || point.base == points::SIM_UNIT_POSE.base)
            {
                continue;
            }
            if crate::sim_objects::owns(b.switches(), point.base) {
                continue;
            }
            let completes_query = query_on && point.base == points::COLLISION_BSP_SEGMENT.base;
            // Long programs yield at the float VM dispatch loop. Some
            // generated images omit loop selectors.
            let installed = b.switches().implementation(name, false)
                && (point.base != points::SIM_MOPP_RAY.base || self.ray_dispatch);
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
                                &mut |cpu: &mut CpuAbi, mem: &mut Guest| {
                                    let transfer = body(cpu, mem)?;
                                    if completes_query {
                                        return sim::collision::complete_segment(
                                            cpu, mem, transfer,
                                        );
                                    }
                                    Ok(transfer)
                                },
                            );
                        }
                        original(cpu, ctx, addr)
                    })
                    .context_only();
            }
        }
        if anim {
            install_anim(b, self.pose_resumes);
        }
        if collision {
            install_collision(b);
        }
        if crate::sim_objects::on(b.switches()) {
            crate::sim_objects::install(b);
        }
        // The clip store and the collision world are derived from guest
        // memory and hold no state of their own; a load only has to
        // invalidate them.
        b.snapshot(if anim || collision {
            SnapshotPolicy::section(Vec::new, move |_, _| {
                if anim {
                    engine::sim_anim::invalidate();
                }
                if collision {
                    engine::sim_collision::invalidate();
                }
                Ok(())
            })
        } else {
            SnapshotPolicy::Stateless
        });
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
/// owns the stream, marker and unit pose entries and enables the palette
/// and filter kernels inside the first-person ports.
pub const ANIM: &str = "sim.native.anim";

fn anim_on(switches: &hooks::Switches) -> bool {
    switches.implementation(ANIM, false)
}

/// `impl.sim.native.anim.skeleton`: the skeleton walk of `bd090` as one
/// Rust call, phase 1b (docs/engine/skeleton-walk.md). On by default. It
/// runs wherever `bd090` runs natively: the `impl.sim.native.objects` body
/// or the `impl.sim.native_pose_matrix` port. Unset, it follows
/// `impl.sim.native.anim`, so `-impl.sim.native.anim` and
/// `-impl.sim.native` turn it off too.
fn skeleton_on(switches: &hooks::Switches) -> bool {
    switches.implementation("sim.native.anim.skeleton", true)
}

/// Points after which guest memory no longer matches the clip store: map
/// load, checkpoint revert and full restore (snapshot load is the
/// section's `load`).
const ANIM_INVALIDATE: [(&points::Point, &str); 3] = [
    (&points::CACHE_TAGS_LOAD, "anim-invalidate-map"),
    (&points::GAME_STATE_REVERT, "anim-invalidate-revert"),
    (&points::GAME_STATE_RESTORE, "anim-invalidate-restore"),
];

fn install_anim(b: &mut Builder, pose_resumes: bool) {
    // A generated image needs a selector at every return site so that an
    // unsupported segment can enter the original there. Older images that
    // lack a selector keep their generated pose body.
    if pose_resumes {
        let pose = &points::SIM_UNIT_POSE;
        b.raw()
            .implement(pose, ANIM, move |cpu, ctx, pc| {
                if let Some(transfer) = sim::unit::run(cpu, &mut *ctx.memory(), pc)? {
                    return Ok(transfer);
                }
                crate::sim_objects::original(cpu, ctx, pc, sim::unit::SITES.binary_search(&pc).is_ok())
            })
            .resumes(sim::unit::SITES)
            .context_only();
    }
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
    for (point, label) in ANIM_INVALIDATE {
        b.raw()
            .observe_before(point, label, |_| engine::sim_anim::invalidate());
    }
}

/// `impl.sim.native.collision`: the collision world, phase 2 of the
/// simulation rewrite (docs/engine/collision-queries.md). It takes over
/// point to leaf and the structure location. Vector and instance traces
/// execute their original bodies while we measure their float environments.
pub const COLLISION: &str = "sim.native.collision";

fn collision_on(switches: &hooks::Switches) -> bool {
    switches.implementation(COLLISION, false)
}

type CollisionBody = fn(
    &mut CpuAbi,
    &mut Guest,
    &engine::sim_collision::CollisionWorld,
) -> Result<Option<TransferAbi>, GuestFault>;

const COLLISION_ENTRIES: [(&points::Point, Entry, CollisionBody); 2] = [
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
];

/// These entries have a contract and a precision census. The vector kernels
/// still need transitive register liveness or exact scratch replay.
const COLLISION_OBSERVED: [(&points::Point, Entry, &str); 3] = [
    (&points::COLLISION_VECTOR_TEST, Entry::VectorTest, "collision-vector-fp"),
    (&points::COLLISION_BSP_SEGMENT, Entry::VectorContext, "collision-walk-fp"),
    (&points::COLLISION_INSTANCE_TRACE, Entry::InstanceTrace, "collision-instance-fp"),
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
                let world = &engine::sim_collision::WORLD;
                world.observe_environment(entry, cpu.mxcsr, cpu.x87.control);
                if sim::supported(cpu) {
                    let mut mem = ctx.memory();
                    if mem.has_trap() {
                        world.record::<()>(entry, &Err(Bail::Trap));
                    } else if let Some(transfer) = body(cpu, &mut mem, world)? {
                        return Ok(transfer);
                    }
                } else {
                    world.record::<()>(entry, &Err(Bail::Env));
                }
                original(cpu, ctx, addr)
            });
    }
    // This family updates shared decode caches, write watches and counters.
    // The hook registry keeps these calls on the owning executor.
    for (point, entry, label) in COLLISION_OBSERVED {
        b.raw()
            .observe_before(point, label, move |o| {
                if o.addr == point.base {
                    engine::sim_collision::WORLD.observe_environment(entry, o.cpu.mxcsr, o.cpu.x87.control);
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
            assert_eq!(plan.chains.len(), sharing.len());
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
        assert_eq!(plan.chains.len(), 3 + ANIM_INVALIDATE.len());
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
        assert_eq!(plan.chains.len(), 3 + ANIM_INVALIDATE.len());

        // The retired per-function switches are gone from the table.
        for key in ["sim.native_rotation_codec", "sim.native_animation_stream"] {
            assert!(Switches::parse(&format!("impl.{key}")).is_err(), "{key}");
        }
    }

    #[test]
    fn a_declined_pose_segment_invokes_the_original_selector() {
        let entry = points::SIM_UNIT_POSE.base;
        let site = sim::unit::SITES[0];
        let mut dispatch = runtime::Dispatch::default();
        dispatch.register_generated_rust(entry, |_, _, selector| Ok(TransferAbi::jump(selector))).unwrap();
        // An overlapping generated owner may use a different selector.
        let owner = 0x14000;
        dispatch.register_generated_rust(owner, |_, _, selector| Ok(TransferAbi::ret(selector))).unwrap();
        for &pc in sim::unit::SITES {
            dispatch.register_resume(pc, owner, pc - entry).unwrap();
        }
        let rt = runtime::Runtime::new(Guest::new(), dispatch);
        let mut registry = Registry::new(Switches::defaults().with("impl.sim.native.anim", true));
        registry.add(&SimNative::new(&rt)).unwrap();
        let plan = registry.build(&Base).unwrap();
        let hook = plan.chains[&entry].implement.as_ref().unwrap();
        let hooks::Run::Implement(run) = &hook.run else { unreachable!() };
        let mut ctx = ExecCtx::new(rt, 1);
        // No mapped operands, so both segments must decline before a write.
        let mut cpu = CpuAbi::default();
        let initial = cpu;
        assert_eq!(run(&mut cpu, &mut ctx, entry).unwrap(), TransferAbi::jump(entry));
        assert_eq!(run(&mut cpu, &mut ctx, site).unwrap(), TransferAbi::ret(site - entry));
        assert_eq!(cpu, initial);
    }

    #[test]
    fn animation_leaves_pose_to_an_image_without_its_resumes() {
        let runtime = runtime::Runtime::new(Guest::new(), runtime::Dispatch::default());
        let mut registry = Registry::new(Switches::defaults().with("impl.sim.native.anim", true));
        registry.add(&SimNative::new(&runtime)).unwrap();
        let plan = registry.build(&Base).unwrap();
        assert!(!plan.chains.contains_key(&points::SIM_UNIT_POSE.base));
        assert!(plan.chains.contains_key(&points::SIM_ANIMATION_STREAM.base));
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
            assert_eq!(chain.observe_before.len(), 1, "{label}");
        }
        for (point, entry, _) in COLLISION_OBSERVED {
            let chain = &plan.chains[&point.base];
            assert_eq!(chain.addr, entry.addr());
            assert!(chain.implement.is_none());
            assert!(chain.shadow.is_none());
            assert_eq!(chain.observe_before.len(), 1);
            assert!(chain.observe_before[0].optional);
        }
        assert_eq!(
            plan.chains.len(),
            COLLISION_ENTRIES.len() + COLLISION_INVALIDATE.len() + COLLISION_OBSERVED.len()
        );
        assert_eq!(plan.save_sections(), vec![("sim-native", Vec::new())]);

        // With the per-function ports also on, the subsystem owns the
        // shared points and the other ports still install.
        let switches = Switches::defaults()
            .with(&key, true)
            .with("impl.sim.native_point_to_leaf", true)
            .with("impl.sim.native_bsp_segment", true)
            .with("impl.sim.native_geometry_test", true);
        let mut registry = Registry::new(switches);
        registry.add(&SimNative::for_tests()).unwrap();
        let plan = registry.build(&Base).unwrap();
        assert_eq!(plan.chains[&points::COLLISION_POINT_TO_LEAF.base].implement.as_ref().unwrap().label, COLLISION);
        assert!(plan.chains[&points::COLLISION_BSP_SEGMENT.base].implement.is_none());
        let geometry = &plan.chains[&points::COLLISION_GEOMETRY_TEST.base];
        assert_eq!(
            geometry.implement.as_ref().unwrap().label,
            "sim.native_geometry_test"
        );

        // Off by default.
        let mut registry = Registry::new(Switches::defaults());
        registry.add(&SimNative::for_tests()).unwrap();
        assert!(registry.build(&Base).unwrap().chains.is_empty());
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
        for (point, _, _) in COLLISION_OBSERVED {
            let hook = &plan.chains[&point.base].observe_before[0];
            let hooks::Run::ObserveBefore(observe) = &hook.run else {
                unreachable!()
            };
            observe(&hooks::Obs {
                thread_id: 1,
                cpu: &cpu,
                runtime: &rt,
                addr: point.base,
                point,
            });
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
                u64::from(matches!(entry, Entry::PointToLeaf | Entry::Location))
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
    fn the_skeleton_walk_is_on_by_default_and_follows_the_anim_group() {
        let on = |text: &str| skeleton_on(&Switches::parse(text).unwrap());
        assert!(on(""));
        assert!(!on("-impl.sim.native.anim.skeleton"));
        assert!(!on("-impl.sim.native.anim"));
        assert!(!on("-impl.sim.native"));
        assert!(on("impl.sim.native.anim"));
        assert!(on("-impl.sim.native.anim,impl.sim.native.anim.skeleton"));
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
