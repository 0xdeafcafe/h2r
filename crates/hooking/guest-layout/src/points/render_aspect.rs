//! Dispatch sites for fitting each on-screen HUD group to its own anchor.
use super::{Point, Sig, RENDER_GAME_VIEW, UI_HUD_DRAW, UI_RENDER_LAYERS, ui_render};

pub const DOC: &str = "docs/graphics/high-resolution.md";
pub const ANCHOR: Point = Point::new("HUD_LAYOUT_ANCHOR", 0x0013_96c7, Sig::Site, DOC);
pub const ANCHOR_READY: Point = Point::new("HUD_LAYOUT_ANCHOR_READY", 0x0013_9929, Sig::Site, DOC);
// Fade over the whole per-player UI viewport inside the menu callback.
pub const MENU_FILL: Point = Point::new("UI_MENU_VIEW_FILL", 0x0014_7cdb, Sig::Site, DOC);
pub const MENU_FILL_END: Point = Point::new("UI_MENU_VIEW_FILL_END", 0x0014_7d10, Sig::Site, DOC);
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
        UI_HUD_DRAW.base,
        UI_RENDER_LAYERS.base,
        ui_render::HUD_RETURN.base,
        ui_render::HUD_HIDDEN_RETURN.base,
        ui_render::LAYERS_RETURN.base,
        ANCHOR.base,
        ANCHOR_READY.base,
        MENU_FILL.base,
        MENU_FILL_END.base,
    ]
    .into_iter()
    .chain(GROUPS.iter().map(|(_, end)| end.base))
}
