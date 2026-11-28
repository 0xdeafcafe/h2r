//! Native GPU renderer (docs/graphics/native-renderer.md).
//!
//! It draws from [`frame_scene::FrameScene`] (docs/graphics/framescene.md).
//! The guest offsets the world and object stages read moved to
//! `scene_extract::layout` in slice 1; they are re-exported below at
//! their old paths for the hooks in `feature` and `boot`, and for the
//! stages here that still read guest memory at their hooks (slices 3 to
//! 6 move them).
//!
//! Stage 1, the static world: the structure BSP's opaque geometry is read
//! from the map's tags once ([`load`], [`world`]), uploaded once
//! ([`gpu`]), and drawn each frame from the game's render camera
//! ([`camera`]) with frustum-culled indirect draws ([`cull`]), while the
//! game skips its own layer-3 entries for it ([`skip`]). Nothing here
//! writes guest memory except [`skip`]'s mask bits, which it restores.
//!
//! Stage 2, objects and skinning: render models read once per tag
//! ([`model_load`], [`model`]), uploaded once ([`model_gpu`]), and drawn
//! per view from the game's own node matrices ([`objects`],
//! [`instances`]), skinned on the GPU, while the game skips their layer-3
//! entries ([`objects::clear_objects`]).
//!
//! Stage 3, transparents: sorted geometry entries ([`transparent`])
//! use native vertex bindings at their original indexed draws.
//!
//! Stage 4b, planar fog: the world's materials drawn again with their own
//! layer-16 passes, the fog unit resolved per view from the device's
//! state ([`fog`], [`fog_gpu`]), and the water edge blend, layer 6, with
//! its scene-copy and reflection inputs supplied per view
//! ([`water_gpu`]).

// A run of ranges is the data's shape even when it holds one, and the
// guest's min/max order decides NaN, which `clamp` would not keep.
#![allow(clippy::single_range_in_vec_init, clippy::manual_clamp)]
#[cfg(feature = "gpu")]
pub mod base_look;
pub mod bodies;
pub mod camera;
pub mod cone_shadows;
pub mod contrail_material;
#[cfg(feature = "gpu")]
pub mod cone_shadows_gpu;
pub mod cull;
#[cfg(feature = "gpu")]
pub mod cull_gpu;
pub mod decal;
pub mod decal_material;
pub mod decal_scene;
pub mod weather_place;
pub mod weather_quad;
#[cfg(feature = "gpu")]
pub mod cinematic_gpu;
#[cfg(feature = "gpu")]
pub mod decal_gpu;
pub mod decorators;
#[cfg(feature = "gpu")]
pub mod decorators_gpu;
#[cfg(feature = "gpu")]
pub mod decorators_load;
pub mod effect_geometry;
pub mod effect_immediate;
pub mod effect_material;
pub mod flare_draw_gpu;
pub mod flare_gpu;
pub mod flares;
pub mod fog_quad;
pub mod fog_fans;
#[cfg(feature = "gpu")]
pub mod engine_states;
pub mod fog;
#[cfg(feature = "gpu")]
pub mod fog_gpu;
#[cfg(feature = "gpu")]
pub mod frame_graph;
pub mod gels;
#[cfg(feature = "gpu")]
pub mod gels_gpu;
#[cfg(feature = "gpu")]
pub mod glow;
#[cfg(feature = "gpu")]
pub mod gpu;
pub mod hud;
#[cfg(feature = "gpu")]
pub mod hud_material;
pub mod instances;
pub mod layer_items;
#[cfg(feature = "gpu")]
pub mod layer_pass;
#[cfg(feature = "gpu")]
pub mod layers_load;
pub mod light_choice;
#[cfg(feature = "gpu")]
pub mod light_gpu;
pub mod light_graph;
pub mod light_list;
#[cfg(feature = "gpu")]
pub mod light_look;
#[cfg(feature = "gpu")]
pub mod light_models;
pub mod light_scene;
pub mod light_tiles;
#[cfg(feature = "gpu")]
pub mod light_tiles_gpu;
pub mod lights;
#[cfg(feature = "gpu")]
pub mod load;
#[cfg(feature = "gpu")]
pub mod look;
pub mod markers;
pub mod model;
#[cfg(feature = "gpu")]
pub mod model_gpu;
#[cfg(feature = "gpu")]
pub mod model_load;
#[cfg(feature = "gpu")]
pub mod object_program;
#[cfg(feature = "gpu")]
pub mod object_program_gpu;
#[cfg(feature = "gpu")]
pub mod object_state;
pub mod object_streams;
pub mod model_view;
pub mod overlay;
#[cfg(feature = "gpu")]
pub mod particle_gpu;
#[cfg(feature = "gpu")]
pub mod particle_compute;
#[cfg(feature = "gpu")]
pub mod particle_definition;
pub mod particle_material;
pub mod particle_mirror;
#[cfg(feature = "gpu")]
pub mod particle_set;
pub mod particle_system;
pub mod particles;
#[cfg(feature = "gpu")]
pub mod pass_state;
#[cfg(feature = "gpu")]
pub mod pipelines;
#[cfg(feature = "gpu")]
pub mod program;
#[cfg(feature = "gpu")]
pub mod lighting;
#[cfg(feature = "gpu")]
pub mod render_scale;
#[cfg(feature = "gpu")]
pub mod renderer;
#[cfg(feature = "gpu")]
pub mod stage;
#[cfg(feature = "gpu")]
pub mod screen_gpu;
#[cfg(feature = "gpu")]
pub mod screen_look;
#[cfg(feature = "gpu")]
pub mod shadow_chain;
#[cfg(feature = "gpu")]
pub mod shadow_filter_gpu;
#[cfg(feature = "gpu")]
pub mod shadow_gpu;
#[cfg(feature = "gpu")]
pub mod shadow_look;
pub mod shadow_scene;
pub mod shadows;
pub mod showcase;
pub mod sorted_entries;
#[cfg(feature = "gpu")]
pub mod sky_material;
#[cfg(feature = "gpu")]
pub mod showcase_gpu;
#[cfg(feature = "gpu")]
pub mod stage_images;
pub mod sun_casters;
pub mod sun_tiles;
#[cfg(feature = "gpu")]
pub mod sun_tiles_gpu;
pub mod text;
#[cfg(feature = "gpu")]
pub mod texture_groups;
pub mod transparent;
#[cfg(feature = "gpu")]
pub mod transparent_sort;
#[cfg(feature = "gpu")]
pub mod transparent_streams;
#[cfg(feature = "gpu")]
pub mod ui_gpu;
#[cfg(feature = "gpu")]
pub mod water_gpu;
#[cfg(feature = "gpu")]
pub mod snapshot_gpu;
pub mod subpart_map;
pub mod weather_scene;
pub mod world;
#[cfg(feature = "gpu")]
pub mod world_layers_gpu;

pub use scene_extract::layout::{
    aux_depth, light_passes, object_lighting, object_nodes, object_walk, objects, post, skip, sky_rows,
    structure_parameters, world_layers, world_ranges,
};

#[cfg(feature = "gpu")]
pub use lighting::Lighting;
#[cfg(feature = "gpu")]
pub use renderer::Renderer;
#[cfg(feature = "gpu")]
pub use stage::{Frame, FrameBuilder, FrameOp, Stage, StageWork, frame_builder};
