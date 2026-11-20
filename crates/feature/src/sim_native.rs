//! Independently selectable simulation replacements for the base Xbox image.
use crate::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use engine::{points, sim_native as sim};
use runtime::{CpuAbi, ExecCtx, Guest, GuestFault, TransferAbi};

pub struct SimNative {
    ray_dispatch: bool,
}

impl SimNative {
    pub fn new(runtime: &runtime::Runtime) -> Self {
        Self {
            ray_dispatch: runtime.read_dispatch(|d| d.has_resume(sim::mopp_query::RAY_DISPATCH)),
        }
    }
}

type Body = fn(&mut CpuAbi, &mut Guest) -> sim::Result;
const ENTRIES: &[(&points::Point, &'static str, Body)] = &[
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
        &points::BASE_OBJECT_IMPORT,
        "sim.native_base_object_import",
        sim::object_base::base_import,
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
    (
        &points::OBJECT_FUNCTION_IMPORT,
        "sim.native_object_import",
        sim::object::import,
    ),
    (
        &points::OBJECT_FUNCTION_VALUE,
        "sim.native_object_value",
        sim::object::value,
    ),
];

/// `impl.sim.native_simd`: host vector lanes inside the native bodies'
/// packed SSE fast path. On by default, so it follows the native ports;
/// off keeps the scalar per-lane loop for A/B timing. Both are exact.
fn simd_on(switches: &crate::Switches) -> bool {
    switches.implementation("sim.native_simd", true)
}

impl Feature for SimNative {
    fn manifest(&self) -> Manifest {
        Manifest::new("sim-native", "Native simulation functions").doc("docs/perf/sim-budget.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        sim::set_simd(simd_on(b.switches()));
        let descriptor_on = b
            .switches()
            .implementation("sim.native_object_import", false);
        let value_on = b
            .switches()
            .implementation("sim.native_object_value", false);
        let query_on = b
            .switches()
            .implementation("sim.native_collision_query", false);
        for &(point, name, body) in ENTRIES {
            let completes_query = query_on && point.base == points::COLLISION_BSP_SEGMENT.base;
            let completes_descriptor =
                descriptor_on && point.base == points::BASE_OBJECT_IMPORT.base;
            let completes_value = value_on
                && (point.base == points::OBJECT_FUNCTION_IMPORT.base || completes_descriptor);
            // Long programs yield at the float VM dispatch loop. Some
            // generated images omit loop selectors.
            if point.base == points::SIM_MOPP_RAY.base && !self.ray_dispatch {
                continue;
            }
            if b.switches().implementation(name, false) {
                b.raw()
                    .implement(point, name, move |cpu, ctx, addr| {
                        if addr == point.base && sim::supported(cpu) {
                            let mut mem = ctx.memory();
                            let transfer = body(cpu, &mut mem)?;
                            if completes_query {
                                return sim::collision::complete_segment(cpu, &mut *mem, transfer);
                            }
                            if completes_descriptor || completes_value {
                                return sim::object::complete_return(
                                    cpu,
                                    &mut *mem,
                                    transfer,
                                    completes_descriptor,
                                    completes_value,
                                );
                            }
                            return Ok(transfer);
                        }
                        original(cpu, ctx, addr)
                    })
                    .context_only();
            }
        }
        let anim = anim_on(b.switches());
        if anim {
            install_anim(b);
        }
        // The clip store is derived from guest memory and holds no state of
        // its own; a load only has to invalidate it.
        b.snapshot(if anim {
            SnapshotPolicy::section(Vec::new, |_, _| {
                engine::sim_anim::invalidate();
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

/// `impl.sim.native.anim`: animation sampling, phase 1a of the simulation
/// rewrite (docs/engine/anim-sampling.md). It replaces the stream call
/// `0x2798a0` and everything under it, so the per-function codec,
/// evaluator and stream ports it covers are test oracles only.
pub const ANIM: &str = "sim.native.anim";

fn anim_on(switches: &crate::Switches) -> bool {
    switches.implementation(ANIM, false)
}

/// Points after which guest memory no longer matches the clip store: map
/// load, checkpoint revert and full restore (snapshot load is the
/// section's `load`).
const ANIM_INVALIDATE: [(&points::Point, &str); 3] = [
    (&points::CACHE_TAGS_LOAD, "anim-invalidate-map"),
    (&points::GAME_STATE_REVERT, "anim-invalidate-revert"),
    (&points::GAME_STATE_RESTORE, "anim-invalidate-restore"),
];

fn install_anim(b: &mut Builder) {
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
    for (point, label) in ANIM_INVALIDATE {
        b.raw()
            .observe_before(point, label, |_| engine::sim_anim::invalidate());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Base, Registry, Switches};

    #[test]
    fn switches_install_only_the_requested_body() {
        for enabled in std::iter::once(None).chain(ENTRIES.iter().map(Some)) {
            let mut switches = Switches::defaults();
            if let Some((_, key, _)) = enabled {
                switches = switches.with(&format!("impl.{key}"), true);
            }
            let mut registry = Registry::new(switches);
            registry.add(&SimNative { ray_dispatch: true }).unwrap();
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
        registry.add(&SimNative { ray_dispatch: true }).unwrap();
        let plan = registry.build(&Base).unwrap();
        let stream = &plan.chains[&points::SIM_ANIMATION_STREAM.base];
        assert_eq!(stream.implement.as_ref().unwrap().label, ANIM);
        for (point, label) in ANIM_INVALIDATE {
            let chain = &plan.chains[&point.base];
            assert!(chain.implement.is_none());
            assert_eq!(chain.observe_before.len(), 1, "{label}");
        }
        assert_eq!(plan.chains.len(), 1 + ANIM_INVALIDATE.len());
        assert_eq!(plan.save_sections(), vec![("sim-native", Vec::new())]);

        // Off by default; the retired per-function switches install nothing.
        for key in ["sim.native_rotation_codec", "sim.native_animation_stream"] {
            let mut registry =
                Registry::new(Switches::defaults().with(&format!("impl.{key}"), true));
            registry.add(&SimNative { ray_dispatch: true }).unwrap();
            let plan = registry.build(&Base).unwrap();
            assert!(plan.chains.is_empty(), "{key}");
            assert!(plan.save_sections().is_empty());
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
