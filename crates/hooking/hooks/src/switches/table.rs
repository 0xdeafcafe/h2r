//! Every `impl.*` switch, in one table: name, default, owner, the date it
//! appeared and the condition for deleting it (docs/architecture-review-2026-11.md
//! section 2.6).
//!
//! A switch not in the table does not parse ([`super::Switches::parse`]
//! refuses it), and a test scans the sources so every `impl.*` name the
//! code reads is listed. [`BUDGET`] bounds the count: adding a switch means
//! raising it in the same commit, where a reviewer sees it. Each row's
//! `remove` says when it goes; a switch off for more than one integration
//! cycle is deleted with its code or turned on.
//!
//! Besides the rows, two kinds of name parse:
//!
//! - a parent of a row (`impl.sim`, `impl.render`): `implementation()`
//!   falls back through it;
//! - an engine leaf, `impl.engine.<group>.<function>`, one per adapter in
//!   `engine::abi::OVERRIDES` (`crate::leaves`), so the table carries the
//!   groups only.

/// What a switch is when nothing sets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initial {
    On,
    Off,
    /// The value of the named switch (the render stages follow
    /// `impl.render.native`).
    Follows(&'static str),
    /// A parent with no value of its own: setting it sets every child
    /// that `implementation()` reads and that has no setting of its own.
    Group,
}

/// One switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Switch {
    pub name: &'static str,
    pub initial: Initial,
    /// The area that owns it and answers for removing it.
    pub owner: &'static str,
    /// The day it appeared in the code (`git log -S`).
    pub added: &'static str,
    /// When it is deleted.
    pub remove: &'static str,
}

/// The most rows the table may hold. Raise it only with a reason in the
/// commit; lower it when a switch goes.
pub const BUDGET: usize = 87;

use Initial::{Follows, Group, Off, On};

const fn sw(
    name: &'static str,
    initial: Initial,
    owner: &'static str,
    added: &'static str,
    remove: &'static str,
) -> Switch {
    Switch {
        name,
        initial,
        owner,
        added,
        remove,
    }
}

const NATIVE: Initial = Follows("impl.render.native");
const STAGE: &str = "the native renderer owns the frame (review 4.3): delete with the D3D path";
const WALK: &str = "the walk's A/B shows no regression: make it unconditional";
const RENDER_7A: &str = "stage 7a port, a dead end (review 5): delete with the port";
const SIM_PORT: &str =
    "hand port superseded by its subsystem's rewrite (docs/engine/sim-rewrite.md): delete with it";
const DIAGNOSTIC: &str = "a diagnostic: becomes an agent API toggle, then goes";
const EXACT: &str = "both paths are exact: make the default unconditional";

pub const TABLE: &[Switch] = &[
    // Rendering.
    sw("impl.render", Group, "render", "2026-11-16", "goes with the stage 7a ports it parents"),
    sw("impl.render.native", On, "render", "2026-11-14", STAGE),
    sw("impl.render.native_world", NATIVE, "render", "2026-11-18", STAGE),
    sw("impl.render.native_world_programs", On, "render", "2026-11-18", WALK),
    sw("impl.render.native_world_skip_build", On, "render", "2026-11-20", WALK),
    sw("impl.render.native_world_layers", NATIVE, "render", "2026-11-19", STAGE),
    sw("impl.render.native_lights", NATIVE, "render", "2026-11-18", STAGE),
    sw("impl.render.native_lights_split", Off, "render", "2026-11-19", "off a cycle: delete or turn on"),
    sw("impl.render.native_light_walk", NATIVE, "render", "2026-11-20", WALK),
    sw("impl.render.native_post", NATIVE, "render", "2026-11-19", STAGE),
    sw("impl.render.native_post_walk", On, "render", "2026-11-20", WALK),
    sw("impl.render.native_objects", NATIVE, "render", "2026-11-18", STAGE),
    sw("impl.render.native_object_walk", On, "render", "2026-11-20", WALK),
    sw("impl.render.native_ui", On, "render", "2026-11-20", STAGE),
    sw("impl.render.native_hud", On, "render", "2026-11-19", "the old name of native_ui: delete"),
    sw("impl.render.native_hud_walk", On, "render", "2026-11-20", WALK),
    sw("impl.render.native_transparents", NATIVE, "render", "2026-11-18", STAGE),
    sw("impl.render.native_transparents_walk", On, "render", "2026-11-20", WALK),
    sw("impl.render.native_particles", NATIVE, "render", "2026-11-19", STAGE),
    sw("impl.render.native_particles_walk", On, "render", "2026-11-20", WALK),
    sw("impl.render.native_effects_walk", On, "render", "2026-11-20", WALK),
    sw("impl.render.native_gels", NATIVE, "render", "2026-11-19", STAGE),
    sw("impl.render.native_fog", NATIVE, "render", "2026-11-19", STAGE),
    sw("impl.render.native_shadows", NATIVE, "render", "2026-11-19", STAGE),
    sw("impl.render.native_scene_lists", NATIVE, "render", "2026-11-20", STAGE),
    sw("impl.render.native_first_person", NATIVE, "render", "2026-11-19", STAGE),
    sw("impl.render.native_first_person_walk", On, "render", "2026-11-20", WALK),
    sw("impl.render.native_decals", NATIVE, "render", "2026-11-19", STAGE),
    sw("impl.render.native_decals_walk", On, "render", "2026-11-20", WALK),
    sw("impl.render.native_water", NATIVE, "render", "2026-11-19", STAGE),
    sw("impl.render.native_residual", NATIVE, "render", "2026-11-19", STAGE),
    sw("impl.render.pipelined_frame", Off, "render", "2026-11-18",
        "the pipelined COW worker in its current form is a dead end (review 5): delete"),
    sw("impl.render.occlusion", Off, "render", "2026-11-14", "off a cycle: delete or turn on"),
    sw("impl.render.interpolate", Off, "render", "2026-11-14",
        "the presentation snapshot (sim-review lane 7) replaces it: delete"),
    sw("impl.render.reproject", Off, "render", "2026-11-18",
        "the presentation snapshot (sim-review lane 7) replaces it: delete"),
    sw("impl.render.reproject_ahead", Off, "render", "2026-11-18",
        "the presentation snapshot (sim-review lane 7) replaces it: delete"),
    sw("impl.render.views", Off, "render", "2026-11-16", RENDER_7A),
    sw("impl.render.layer_draw", Off, "render", "2026-11-16", RENDER_7A),
    sw("impl.render.layer_entries", Off, "render", "2026-11-16", RENDER_7A),
    sw("impl.render.game_view", Off, "render", "2026-11-18", RENDER_7A),
    sw("impl.render.scene_passes", Off, "render", "2026-11-18", RENDER_7A),
    sw("impl.render.states", Off, "render", "2026-11-18", RENDER_7A),
    sw("impl.render.states_skip", Off, "render", "2026-11-18", RENDER_7A),
    // Engine leaves (the per-function keys are `crate::leaves`').
    sw("impl.engine", Group, "engine", "2026-11-16", "goes with its last group"),
    sw("impl.engine.locale", On, "engine", "2026-11-16", EXACT),
    sw("impl.engine.tags", Off, "engine", "2026-11-16",
        "hot leaves: on once thin transfers make an override free (review 5 item 1), else delete"),
    sw("impl.engine.datums", Off, "engine", "2026-11-16",
        "hot leaves: on once thin transfers make an override free (review 5 item 1), else delete"),
    // Audio.
    sw("impl.audio.packet-clock", On, "audio", "2026-11-20", EXACT),
    // Simulation: the subsystem switches.
    sw("impl.sim", Group, "sim", "2026-11-20", "goes with its last child"),
    sw("impl.sim.native", Group, "sim", "2026-11-20",
        "the parent of the subsystem rewrites (lane rules): each child goes when its subsystem defaults on"),
    sw("impl.sim.native.anim", Off, "sim", "2026-11-20",
        "animation sampling (sim-anim): on in the lane that lands it, then deleted"),
    sw("impl.sim.native.collision", Off, "sim", "2026-11-20",
        "collision queries (sim-collision): on in the lane that lands it, then deleted"),
    sw("impl.sim.native.objects", Off, "sim", "2026-11-20",
        "the object update (sim-objects): on in the lane that lands it, then deleted"),
    sw("impl.sim.arena-hash", Off, "sim", "2026-11-20", DIAGNOSTIC),
    sw("impl.sim.metrics-projection", On, "sim", "2026-11-20", EXACT),
    sw("impl.sim.observer-resources", On, "sim", "2026-11-20", EXACT),
    sw("impl.sim.static-light", On, "sim", "2026-11-20", EXACT),
    sw("impl.sim.submix-fx", On, "sim", "2026-11-20", EXACT),
    sw("impl.sim.native_simd", On, "sim", "2026-11-20", EXACT),
    // Simulation: hand ports (`sim_bridge::sim_native`).
    sw("impl.sim.native_transform_compose", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_pose_matrix", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_rotation_codec", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_point_to_leaf", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_geometry_test", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_unit_pose", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_animation_stream", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_graph_resolve", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_marker_transforms", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_bsp_segment", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_collision_query", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_base_object_import", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_mopp_box", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_mopp_ray", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_object_import", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_object_value", Off, "sim", "2026-11-20", SIM_PORT),
    // Simulation: hand ports (`engine::sim_physics::PORTS`).
    sw("impl.sim.native_havok_solver", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_havok_support_vertices", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_havok_weld", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_object_change_colors", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_spatial_visit", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_spatial_portal", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_unit_aim", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_unit_update", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_light_record", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_first_person_pose", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_first_person_filters", Off, "sim", "2026-11-20", SIM_PORT),
    sw("impl.sim.native_observer_camera", Off, "sim", "2026-11-20", SIM_PORT),
];

/// The row named `name`.
pub fn get(name: &str) -> Option<&'static Switch> {
    TABLE.iter().find(|s| s.name == name)
}

/// Whether `name` (a full `impl.*` key) parses: a row, a parent of one, or
/// an engine leaf's key.
pub fn known(name: &str) -> bool {
    if get(name).is_some() {
        return true;
    }
    let parent = TABLE.iter().any(|s| {
        s.name
            .strip_prefix(name)
            .is_some_and(|rest| rest.starts_with('.'))
    });
    if parent {
        return true;
    }
    if let Some(rest) = name.strip_prefix("impl.engine.")
        && let Some((group, function)) = rest.split_once('.')
    {
        return engine::abi::OVERRIDES
            .iter()
            .any(|o| o.name == function && crate::leaves::group(o.name) == group);
    }
    false
}

/// The table as text, one switch a line, for `--help`-style listings.
pub fn text() -> String {
    TABLE
        .iter()
        .map(|s| {
            let initial = match s.initial {
                On => "on".to_owned(),
                Off => "off".to_owned(),
                Follows(other) => format!("= {other}"),
                Group => "group".to_owned(),
            };
            format!("{} [{initial}] {} {}: {}", s.name, s.owner, s.added, s.remove)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
