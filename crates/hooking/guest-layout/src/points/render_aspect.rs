//! Dispatch sites for fitting each on-screen HUD group to its own anchor.
use super::{
    Point, RENDER_ALTERNATE_VIEW, RENDER_GAME_VIEW, Sig, UI_HUD_DRAW, UI_RENDER_LAYERS, ui_render,
};

pub const DOC: &str = "docs/graphics/high-resolution.md";
// The active camera builder has returned its projection terms on the stack,
// one instruction after the call's continuation (0x0002fd9e loads only the
// camera's +0x20): a return resumes the caller's own code, never a hook at
// the continuation, so a site on 0x0002fd9e would never run.
pub const VIEW_PROJECTION_TERMS: Point =
    Point::new("VIEW_PROJECTION_TERMS", 0x0002_fda3, Sig::Site, DOC);
pub const ANCHOR: Point = Point::new("HUD_LAYOUT_ANCHOR", 0x0013_96c7, Sig::Site, DOC);
pub const ANCHOR_READY: Point = Point::new("HUD_LAYOUT_ANCHOR_READY", 0x0013_9929, Sig::Site, DOC);
// Fade over the whole per-player UI viewport inside the menu callback.
pub const MENU_FILL: Point = Point::new("UI_MENU_VIEW_FILL", 0x0014_7cdb, Sig::Site, DOC);
pub const MENU_FILL_END: Point = Point::new("UI_MENU_VIEW_FILL_END", 0x0014_7d10, Sig::Site, DOC);
// The flare reports the area of a rectangle projected on the CPU. These
// two boundaries enclose just the vertices and END after that area is known.
pub const FLARE_QUERY: Point = Point::new("FLARE_QUERY_SCREEN_QUAD", 0x0002_e82b, Sig::Site, DOC);
pub const FLARE_QUERY_END: Point = Point::new("FLARE_QUERY_SCREEN_END", 0x0002_e9bb, Sig::Site, DOC);
// A single reflection's CPU-projected billboard, including native admission.
pub const FLARE_REFLECTION: Point = Point::new("FLARE_REFLECTION_SCREEN_QUAD", 0x0002_f247, Sig::Site, DOC);
pub const FLARE_REFLECTION_END: Point = Point::new("FLARE_REFLECTION_SCREEN_END", 0x0002_f24c, Sig::Site, DOC);
// Calls to the two CPU-projected volume/trail emitters and the final flare fans.
pub const VOLUME_SPRITE: Point = Point::new("VOLUME_SPRITE", 0x0004_fdd8, Sig::Site, DOC);
pub const VOLUME_SPRITE_END: Point = Point::new("VOLUME_SPRITE_END", 0x0004_fddd, Sig::Site, DOC);
pub const VOLUME_DEPTH: Point = Point::new("VOLUME_DEPTH", 0x0004_fd98, Sig::Site, DOC);
pub const VOLUME_DEPTH_END: Point = Point::new("VOLUME_DEPTH_END", 0x0004_fd9d, Sig::Site, DOC);
pub const FLARE_COMPOSITE_LOOP: Point = Point::new("FLARE_COMPOSITE_LOOP", 0x0004_7732, Sig::Site, DOC);
pub const FLARE_COMPOSITE_DONE: Point = Point::new("FLARE_COMPOSITE_DONE", 0x0004_7863, Sig::Site, DOC);
// Return PC at the call to the HUD anchor helper and the end of the
// producer using that anchor. The sensor blips have their own call and
// use the same bottom-left origin as the motion sensor bitmap widget.
pub const GROUPS: [(u32, Point); 6] = [
    (
        0x0013_9628,
        Point::new("HUD_SENSOR_LAYOUT_END", 0x0013_968a, Sig::Site, DOC),
    ),
    (
        0x0020_0573,
        Point::new("HUD_INDICATOR_LAYOUT_END", 0x0020_07b2, Sig::Site, DOC),
    ),
    (
        0x0022_ab1c,
        Point::new("HUD_TEXT_LAYOUT_END", 0x0022_acb1, Sig::Site, DOC),
    ),
    (
        0x0022_b055,
        Point::new("HUD_BITMAP_LAYOUT_END", 0x0022_b7da, Sig::Site, DOC),
    ),
    (
        0x0022_b9e7,
        Point::new("HUD_EFFECT_LAYOUT_END", 0x0022_bbc7, Sig::Site, DOC),
    ),
    (
        0x0024_cfdf,
        Point::new("HUD_MESSAGE_LAYOUT_END", 0x0024_d6c4, Sig::Site, DOC),
    ),
];

/// Function and interior instruction entries used by the aspect hooks.
/// A host module can be built on a different machine from its GPU host,
/// so these entry boundaries are independent of the builder's features.
pub fn instruction_entries() -> impl Iterator<Item = u32> {
    [
        RENDER_GAME_VIEW.base,
        RENDER_ALTERNATE_VIEW.base,
        UI_HUD_DRAW.base,
        UI_RENDER_LAYERS.base,
        ui_render::HUD_RETURN.base,
        ui_render::HUD_HIDDEN_RETURN.base,
        ui_render::LAYERS_RETURN.base,
        VIEW_PROJECTION_TERMS.base,
        ANCHOR.base,
        ANCHOR_READY.base,
        MENU_FILL.base,
        MENU_FILL_END.base,
        FLARE_QUERY.base,
        FLARE_QUERY_END.base,
        FLARE_REFLECTION.base,
        FLARE_REFLECTION_END.base,
        VOLUME_SPRITE.base,
        VOLUME_SPRITE_END.base,
        VOLUME_DEPTH.base,
        VOLUME_DEPTH_END.base,
        FLARE_COMPOSITE_LOOP.base,
        FLARE_COMPOSITE_DONE.base,
    ]
    .into_iter()
    .chain(GROUPS.iter().map(|(_, end)| end.base))
}
