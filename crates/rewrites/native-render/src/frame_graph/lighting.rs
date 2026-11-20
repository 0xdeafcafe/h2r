//! Work issued by the lighting mode in a single graph draw. The guest's
//! own light and shadow commands are already counted in the graph totals.

/// Submitted work for one E3 stage. An empty stage has no GPU interval.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PassWork {
    pub draws: u32,
    pub gpu_ns: Option<u64>,
}

/// Per-draw accounting for E3's own lighting passes. Selection and atlas
/// counters describe decisions of this renderer, independently of the
/// guest's selections retained in the sealed scene.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct E3Report {
    pub tiles: PassWork,
    pub sun: PassWork,
    pub marks: PassWork,
    pub lights: PassWork,
    pub lights_lit: u32,
    pub lights_shadowed: u32,
    pub lights_over_cap: u32,
    pub sun_tiles: u32,
    pub sun_tiles_redrawn: u32,
    pub atlas_halved: u32,
}
