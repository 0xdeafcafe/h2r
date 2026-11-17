//! The look: a dark game-tool theme (neutral greys, one blue accent,
//! small rounding, roomy spacing) and the bundled fonts, Inter for text
//! and a Lucide subset for icons (`assets/fonts`, OFL and ISC licensed).
use dear_imgui_rs::{
    Context, FontConfig, FontId, FontSource, StbTrueTypeFontData, Style, StyleColor,
};

/// Text size in logical points. Dear ImGui 1.92 rasterises at the
/// framebuffer scale, so this stays crisp on HiDPI displays.
pub const FONT_SIZE: f32 = 15.0;

static INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
static INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
static LUCIDE: &[u8] = include_bytes!("../assets/fonts/lucide-subset.ttf");

/// The fonts the UI pushes.
#[derive(Clone, Copy, Debug)]
pub struct Fonts {
    /// Inter Regular with the icons merged in (the default font).
    pub regular: FontId,
    /// Inter SemiBold with the icons, for headings.
    pub bold: FontId,
}

fn source(bytes: &[u8], size: f32) -> FontSource<'static> {
    let data = StbTrueTypeFontData::from_slice(bytes).expect("a bundled font is valid");
    FontSource::stb_truetype_with_size(data, size)
}

fn icons(size: f32) -> FontSource<'static> {
    // Icons sit a touch low against Inter's x-height; nudge them up and
    // give them a fixed advance so toolbars line up.
    source(LUCIDE, size).with_config(
        FontConfig::new()
            .merge_mode(true)
            .glyph_offset([0.0, 2.0])
            .glyph_min_advance_x(size * 1.1),
    )
}

/// Add the fonts to the context's atlas. The first is the default.
pub fn load_fonts(ctx: &mut Context) -> Fonts {
    let atlas = ctx.font_atlas();
    let regular = atlas.add_font(&[source(INTER_REGULAR, FONT_SIZE), icons(FONT_SIZE)]);
    let bold = atlas.add_font(&[source(INTER_SEMIBOLD, FONT_SIZE), icons(FONT_SIZE)]);
    Fonts { regular, bold }
}

/// RGBA from 0..255 components.
const fn rgb(r: u8, g: u8, b: u8) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}
const fn rgba(r: u8, g: u8, b: u8, a: f32) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a]
}

/// The accent (selection, active tabs, gizmo-adjacent highlights).
pub const ACCENT: [f32; 4] = rgb(56, 132, 230);
/// Muted text.
pub const DIM: [f32; 4] = rgb(140, 145, 155);
/// Errors in the log and status bar.
pub const ERROR: [f32; 4] = rgb(235, 96, 88);
/// Warnings (unsaved changes).
pub const WARN: [f32; 4] = rgb(230, 180, 80);
/// The status bar height in points.
pub const STATUS_BAR: f32 = 26.0;

/// Apply the theme to a style.
pub fn apply(style: &mut Style) {
    // Shape and spacing.
    style.set_window_rounding(4.0);
    style.set_child_rounding(3.0);
    style.set_frame_rounding(3.0);
    style.set_popup_rounding(4.0);
    style.set_scrollbar_rounding(6.0);
    style.set_grab_rounding(3.0);
    style.set_tab_rounding(3.0);
    style.set_window_border_size(1.0);
    style.set_frame_border_size(0.0);
    style.set_popup_border_size(1.0);
    style.set_tab_border_size(0.0);
    style.set_window_padding([10.0, 8.0]);
    style.set_frame_padding([8.0, 5.0]);
    style.set_item_spacing([8.0, 6.0]);
    style.set_item_inner_spacing([6.0, 4.0]);
    style.set_indent_spacing(16.0);
    style.set_scrollbar_size(12.0);
    style.set_grab_min_size(10.0);
    style.set_window_title_align([0.0, 0.5]);
    style.set_docking_separator_size(3.0);

    // Colours: near-black panels, slightly lighter frames, one accent.
    let bg = rgb(24, 25, 28);
    let panel = rgb(30, 31, 35);
    let frame = rgb(42, 44, 50);
    let frame_hover = rgb(52, 55, 62);
    let frame_active = rgb(62, 66, 75);
    let border = rgb(48, 50, 56);
    let text = rgb(222, 224, 228);
    let accent_dim = rgba(56, 132, 230, 0.55);
    let accent_soft = rgba(56, 132, 230, 0.30);
    use StyleColor as C;
    for (c, v) in [
        (C::Text, text),
        (C::TextDisabled, rgb(105, 110, 120)),
        (C::WindowBg, panel),
        (C::ChildBg, rgba(0, 0, 0, 0.0)),
        (C::PopupBg, rgb(34, 35, 40)),
        (C::Border, border),
        (C::BorderShadow, rgba(0, 0, 0, 0.0)),
        (C::FrameBg, frame),
        (C::FrameBgHovered, frame_hover),
        (C::FrameBgActive, frame_active),
        (C::TitleBg, bg),
        (C::TitleBgActive, bg),
        (C::TitleBgCollapsed, bg),
        (C::MenuBarBg, rgb(20, 21, 24)),
        (C::ScrollbarBg, rgba(0, 0, 0, 0.0)),
        (C::ScrollbarGrab, rgb(58, 61, 68)),
        (C::ScrollbarGrabHovered, rgb(74, 78, 88)),
        (C::ScrollbarGrabActive, rgb(90, 95, 106)),
        (C::CheckMark, ACCENT),
        (C::SliderGrab, ACCENT),
        (C::SliderGrabActive, rgb(90, 156, 240)),
        (C::Button, frame),
        (C::ButtonHovered, frame_hover),
        (C::ButtonActive, accent_dim),
        (C::Header, accent_soft),
        (C::HeaderHovered, rgba(56, 132, 230, 0.40)),
        (C::HeaderActive, accent_dim),
        (C::Separator, border),
        (C::SeparatorHovered, accent_dim),
        (C::SeparatorActive, ACCENT),
        (C::ResizeGrip, rgba(0, 0, 0, 0.0)),
        (C::ResizeGripHovered, accent_dim),
        (C::ResizeGripActive, ACCENT),
        (C::Tab, bg),
        (C::TabHovered, frame_hover),
        (C::TabSelected, panel),
        (C::TabSelectedOverline, ACCENT),
        (C::TabDimmed, bg),
        (C::TabDimmedSelected, panel),
        (C::TabDimmedSelectedOverline, rgba(0, 0, 0, 0.0)),
        (C::DockingPreview, accent_dim),
        (C::DockingEmptyBg, bg),
        (C::TableHeaderBg, frame),
        (C::TableBorderStrong, border),
        (C::TableBorderLight, rgb(38, 40, 45)),
        (C::TableRowBg, rgba(0, 0, 0, 0.0)),
        (C::TableRowBgAlt, rgba(255, 255, 255, 0.02)),
        (C::TextSelectedBg, accent_soft),
        (C::NavCursor, ACCENT),
        (C::ModalWindowDimBg, rgba(0, 0, 0, 0.55)),
    ] {
        style.set_color(c, v);
    }
}

/// Icon glyphs from the bundled Lucide subset (private use area).
pub mod icon {
    pub const FOLDER_OPEN: &str = "\u{e247}";
    pub const FOLDER: &str = "\u{e0d7}";
    pub const SAVE: &str = "\u{e14d}";
    pub const UNDO: &str = "\u{e2a1}";
    pub const REDO: &str = "\u{e2a0}";
    pub const COPY: &str = "\u{e09e}";
    pub const TRASH: &str = "\u{e18e}";
    pub const PLUS: &str = "\u{e13d}";
    pub const MAP: &str = "\u{e110}";
    pub const BOX: &str = "\u{e061}";
    pub const CROSSHAIR: &str = "\u{e0ac}";
    pub const MOVE: &str = "\u{e2e5}";
    pub const ROTATE: &str = "\u{e2ea}";
    pub const SEARCH: &str = "\u{e151}";
    pub const FILE: &str = "\u{e0c0}";
    pub const DISC: &str = "\u{e494}";
    pub const LAYERS: &str = "\u{e529}";
    pub const EYE: &str = "\u{e0ba}";
    pub const PACKAGE: &str = "\u{e129}";
    pub const PACKAGE_OPEN: &str = "\u{e2cc}";
    pub const ALERT: &str = "\u{e077}";
    pub const INFO: &str = "\u{e0f9}";
    pub const FOCUS: &str = "\u{e29e}";
    pub const TREE: &str = "\u{e408}";
    pub const SLIDERS: &str = "\u{e29a}";
    pub const LOG: &str = "\u{e45f}";
    pub const CLOSE: &str = "\u{e1b2}";
    pub const CHECK: &str = "\u{e226}";
    pub const DRIVE: &str = "\u{e0ed}";
    pub const MOUNTAIN: &str = "\u{e231}";
    pub const FLAG: &str = "\u{e0d1}";
    pub const CAR: &str = "\u{e1d5}";
    pub const SWORDS: &str = "\u{e2b4}";
    pub const SHIELD: &str = "\u{e158}";
    pub const USERS: &str = "\u{e1a4}";
    pub const SOUND: &str = "\u{e1ab}";
    pub const COG: &str = "\u{e30b}";
    pub const KNIFE: &str = "\u{e4a0}";
    pub const ZAP: &str = "\u{e1b4}";
    pub const MAXIMIZE: &str = "\u{e112}";
}

/// The outliner icon of a placement block.
pub fn block_icon(kind: scenario_edit::BlockKind) -> &'static str {
    use scenario_edit::BlockKind::*;
    match kind {
        Scenery => icon::MOUNTAIN,
        Vehicles => icon::CAR,
        Equipment => icon::SHIELD,
        Weapons => icon::SWORDS,
        Machines => icon::COG,
        SoundScenery => icon::SOUND,
        Crates => icon::BOX,
        StartingLocations => icon::USERS,
        NetgameFlags => icon::FLAG,
        NetgameEquipment => icon::ZAP,
        StartingEquipment => icon::KNIFE,
    }
}
