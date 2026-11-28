//! The guest render producers and how far each is replaced
//! (docs/graphics/framescene.md, 8.2): the table [`crate::cut`] and the
//! frame step key their cuts and slots by.

/// A producer's status in the registry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Replaced: its guest code no longer runs.
    Native,
    /// Partly replaced: a native stage draws some of its items from the
    /// scene and their guest draws are skipped; the rest, and the
    /// producer's own guest code, still run through `d3d`
    /// ([`layer_status`]).
    Partial,
    /// The opaque block's layer draws (slice R1, [`KEPT_LAYERS`]): a call
    /// whose items are all native returns without the guest body
    /// (`frame_step::layer`); a call with an item a stage cannot draw runs
    /// the body for it, and each such item is a
    /// `scene_kept_<site><layer>_<kind>_<cause>` count
    /// (`native_layers::layer_return`). Its work here is only those items'.
    /// The present is kept the same way: a published frame returns without
    /// its body, a kept one (`publish_kept_<cause>`) runs it. So are the
    /// frame's own rows (`views`, `outside_view`, `frame_loop`): their own
    /// guest code runs, its only D3D the view setup and view end, which are
    /// Rust with no guest body (`render_bridge::view_setup`).
    Kept,
    /// Still drawn by the guest front-end through `d3d`.
    D3d,
    /// No registered producer was open.
    Unknown,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Status::Native => "native",
            Status::Partial => "partial",
            Status::Kept => "kept",
            Status::D3d => "d3d",
            Status::Unknown => "unknown",
        }
    }
}

/// A registered producer: a guest render function, by name, and the
/// frame slot its work belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub name: &'static str,
    pub entry: u32,
    pub status: Status,
    pub slot: &'static str,
}

/// The fixed producers. The layer draw is a producer per site and layer
/// ([`layer_status`]). A slice changes a row's status in the same commit
/// as its test.
pub const REGISTRY: &[Row] = &[
    Row {
        name: "game_view",
        entry: 0x0002_b790,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "alternate_view",
        entry: 0x0002_c790,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "scene_passes",
        entry: 0x0002_bcd0,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "sky",
        entry: engine::points::RENDER_SKY.base,
        status: Status::Partial,
        slot: "after_lights",
    },
    Row {
        name: "scene_clear",
        entry: 0x0001_4b60,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "target_select",
        entry: 0x0001_cd30,
        status: Status::Partial,
        slot: "opaque",
    },
    // The Rust object setup answers the calls it can build; the rest run
    // the guest's body (docs/graphics/native-objects.md, "Object setup").
    Row {
        name: "object_setup",
        entry: 0x0013_3790,
        status: Status::Partial,
        slot: "opaque",
    },
    // A cut from the first frame: the scene draws the first person and a
    // part it cannot draw is a named drop (docs/graphics/native-objects.md,
    // "First person").
    Row {
        name: "first_person",
        entry: 0x0003_fdf0,
        status: Status::Native,
        slot: "transparents",
    },
    Row {
        name: "object_shadows",
        entry: 0x0003_35e0,
        status: Status::Native,
        slot: "opaque",
    },
    Row {
        name: "object_shadow",
        entry: 0x0003_2350,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "water_teardown",
        entry: 0x0003_c3c0,
        status: Status::Partial,
        slot: "after_lights",
    },
    Row {
        name: "decals",
        entry: 0x0004_4040,
        status: Status::Partial,
        slot: "opaque",
    },
    Row {
        name: "dynamic_lights",
        entry: 0x0003_1c80,
        status: Status::Native,
        slot: "after_opaque",
    },
    Row {
        name: "light_passes",
        entry: 0x0003_1170,
        status: Status::Native,
        slot: "after_opaque",
    },
    // Includes sorted layer-15 pool surfaces. Layer 6 and the teardown
    // composite run outside this dispatcher.
    Row {
        name: "transparents",
        entry: 0x0001_e370,
        status: Status::Partial,
        slot: "transparents",
    },
    Row {
        name: "screen_effects",
        entry: 0x0001_3870,
        status: Status::Partial,
        slot: "transparents",
    },
    Row {
        name: "colour_pass",
        entry: 0x0002_14f0,
        status: Status::Partial,
        slot: "transparents",
    },
    Row {
        name: "screen_effect",
        entry: 0x0002_d140,
        status: Status::Partial,
        slot: "transparents",
    },
    Row {
        name: "hud",
        entry: 0x0019_17eb,
        status: Status::Partial,
        slot: "overlay",
    },
    Row {
        name: "ui",
        entry: 0x0014_7af1,
        status: Status::Partial,
        slot: "overlay",
    },
    Row {
        name: "fade",
        entry: 0x0004_0890,
        status: Status::Partial,
        slot: "overlay",
    },
    Row {
        name: "frame_tail",
        entry: 0x0001_3b90,
        status: Status::Partial,
        slot: "overlay",
    },
    Row {
        name: "loading",
        entry: 0x0002_89e0,
        status: Status::Partial,
        slot: "outside",
    },
    // The movie's tick (docs/graphics/retire-d3d-display.md, "Movies"):
    // its decode and draw are native with a native display; the stop path
    // its service runs releases the guest's texture.
    Row {
        name: "movie",
        entry: 0x0015_6040,
        status: Status::Partial,
        slot: "outside",
    },
    // The frame's own calls around the producers above
    // (docs/graphics/retire-d3d-display.md, "Frames outside a game view"): without
    // these rows their entries were `unknown` inside the frame step and
    // `outside_render` in the frame loops outside it.
    //
    // `render_views`' begin-frame setup and its loop over the views. Its
    // only D3D was the view setup and view end (`render_bridge::view_setup`),
    // Rust with no guest body, so it has no D3D work here.
    Row {
        name: "views",
        entry: 0x0002_b5d0,
        status: Status::Kept,
        slot: "opaque",
    },
    // The frame outside a game view's own setup (the target, the clear,
    // the viewport through `0x0002c8a0`), from the frame step and the map
    // load's two frame loops: the view setup's, kept as `views` is.
    Row {
        name: "outside_view",
        entry: 0x0022_32a0,
        status: Status::Kept,
        slot: "outside",
    },
    // A frame loop outside the frame step: its view `0x0002ca00`'s setup
    // and end, kept as `views` is.
    Row {
        name: "frame_loop",
        entry: 0x0012_b6f0,
        status: Status::Kept,
        slot: "outside",
    },
    // The present: made in Rust (`render_frame::publish`), so its guest
    // body, and the `Swap` in it, never runs.
    Row {
        name: "present",
        entry: 0x0001_3f10,
        status: Status::Kept,
        slot: "outside",
    },
];

/// The index of `name` in [`REGISTRY`].
pub fn row(name: &str) -> Option<usize> {
    REGISTRY.iter().position(|r| r.name == name)
}

/// Where a layer draw is called from, by name.
pub const LAYER_SITES: [&str; 7] = [
    "scene",
    "alternate",
    "first_person",
    "light",
    "object_shadow",
    "other",
    "sorted",
];

/// The layer draws a native stage draws items of from the scene, skipping
/// those items' guest draws, while the guest body runs for every call
/// (docs/graphics/retire-d3d.md, "By caller"), by [`LAYER_SITES`] index:
/// the first person's layer 16, the light passes' 12 and 13, and the
/// object shadow's caster and receiver layers 10 and 11. Each is
/// [`Status::Partial`]: what its stages refuse stays the guest's.
pub const PARTIAL_LAYERS: [(u8, &[u8]); 3] = [
    (2, &[16]),
    (3, &[12, 13]),
    (4, &[10, 11]),
];

/// The layer draws that return when every item is native, by
/// [`LAYER_SITES`] index: the opaque block's (the scene's layers 1, 3, 5,
/// 7, 8 and 20, the alternate view's 1, 3 and 5, the first person's 1, 3,
/// 7, 8 and 20: `frame_step::layer::LAYERS`) and R5's (the scene's 2 and
/// 17, the alternate view's and the first person's 2, the sorted
/// handlers' 18, 19 and 21: `frame_step::layer::LATE_LAYERS`; the scene's
/// water 6 and fog 16: `FOG_WATER_LAYERS`), at the sites
/// `native_layers::return_site` names. Each is [`Status::Kept`].
pub const KEPT_LAYERS: [(u8, &[u8]); 4] = [
    (0, &[1, 2, 3, 5, 6, 7, 8, 16, 17, 20]),
    (1, &[1, 2, 3, 5]),
    (2, &[1, 2, 3, 7, 8, 20]),
    (6, &[18, 19, 21]),
];

/// The status of the layer draw at site `at` ([`LAYER_SITES`]) of `layer`.
pub fn layer_status(at: u8, layer: u8) -> Status {
    let listed = |table: &[(u8, &[u8])]| table.iter().any(|&(site, layers)| site == at && layers.contains(&layer));
    if listed(&KEPT_LAYERS) {
        Status::Kept
    } else if listed(&PARTIAL_LAYERS) {
        Status::Partial
    } else {
        Status::D3d
    }
}

