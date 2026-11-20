//! Guest offsets, one file per guest record, moved unchanged from
//! `native-render`. When the typed guest layout crate (change 5 of the
//! architecture review) lands, moving these onto it is a rename: nothing
//! outside `scene_extract` should name a module here once the stages that
//! still read guest memory have moved (slices 3 to 6).

pub mod aux_depth;
pub mod camera;
pub mod light_passes;
pub mod object_lighting;
pub mod object_nodes;
pub mod object_walk;
pub mod objects;
pub mod skip;
pub mod world_layers;
pub mod world_ranges;
