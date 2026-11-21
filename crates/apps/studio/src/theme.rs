//! The look: a dark game-tool theme (neutral greys, one blue accent,
//! small rounding, roomy spacing) and the bundled fonts, Inter for text
//! and a Lucide subset for icons (`assets/fonts`, OFL and ISC licensed).
use dear_imgui_rs::{
    Context, FontConfig, FontId, FontSource, StbTrueTypeFontData, Style, StyleColor, StyleVar, Ui,
};

use crate::play::LinkTone;

/// Text size in logical points. Dear ImGui 1.92 rasterises at the
/// framebuffer scale, so this stays crisp on HiDPI displays.
pub const FONT_SIZE: f32 = 15.0;

pub(crate) static INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
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
/// Passing checks.
pub const OK: [f32; 4] = rgb(110, 200, 125);
/// Warnings (unsaved changes).
pub const WARN: [f32; 4] = rgb(230, 180, 80);
/// The bar between the menu and the dockspace.
pub const BAR: f32 = 36.0;
pub const BAR_BG: [f32; 4] = rgb(30, 31, 35);
pub const BAR_SPACING: f32 = 8.0;
pub const BAR_BUTTON: f32 = 30.0;
pub const LIVE_FRAME: f32 = 2.0;
/// The status bar height in points.
pub const STATUS_BAR: f32 = 26.0;

pub fn link_colour(tone: LinkTone) -> [f32; 4] {
    match tone {
        LinkTone::Idle => DIM,
        LinkTone::Notice => rgb(224, 190, 116),
        LinkTone::View => rgb(120, 175, 243),
        LinkTone::Live => OK,
        LinkTone::Warn => WARN,
        LinkTone::Error => ERROR,
    }
}

pub fn section(ui: &Ui, text: &str) {
    ui.separator_with_text(text);
}

const PILL_PADDING: [f32; 2] = [17.0, 5.0];

pub fn pill_width(ui: &Ui, icon: &str, text: &str) -> f32 {
    ui.calc_text_size(format!("{text}  {icon}"))[0] + 2.0 * PILL_PADDING[0]
}

fn pill_fill(colour: [f32; 4], tint: f32) -> [f32; 4] {
    std::array::from_fn(|i| BAR_BG[i] + (colour[i] - BAR_BG[i]) * tint)
}

/// A rounded status button with a coloured dot, a label and an icon.
pub fn pill(ui: &Ui, colour: [f32; 4], icon: &str, text: &str) -> bool {
    let _rounding = ui.push_style_var(StyleVar::FrameRounding(16.0));
    let _padding = ui.push_style_var(StyleVar::FramePadding(PILL_PADDING));
    let _bg = ui.push_style_color(StyleColor::Button, pill_fill(colour, 0.16));
    let _hover = ui.push_style_color(StyleColor::ButtonHovered, pill_fill(colour, 0.26));
    let _active = ui.push_style_color(StyleColor::ButtonActive, pill_fill(colour, 0.32));
    let clicked = ui.button(format!("{text}  {icon}###link-pill"));
    let [x, y] = ui.item_rect_min();
    let height = ui.item_rect_max()[1] - y;
    ui.get_window_draw_list()
        .add_circle([x + 9.0, y + height * 0.5], 2.5, colour)
        .filled(true)
        .build();
    clicked
}

pub fn link_badge(ui: &Ui, tone: LinkTone, text: &str) {
    let colour = link_colour(tone);
    let [w, h] = ui.calc_text_size(text);
    let [x, y] = ui.cursor_screen_pos();
    let size = [w + 18.0, h + 4.0];
    let dl = ui.get_window_draw_list();
    dl.add_rect([x, y], [x + size[0], y + size[1]], pill_fill(colour, 0.16))
        .rounding(size[1] * 0.5)
        .filled(true)
        .build();
    dl.add_text([x + 9.0, y + 2.0], colour, text);
    ui.dummy(size);
}

pub fn edit_switch_width(ui: &Ui) -> f32 {
    40.0 + ui.calc_text_size("Edit")[0]
}

pub fn edit_switch(ui: &Ui, on: bool) -> bool {
    let _align = ui.push_style_var(StyleVar::ButtonTextAlign([1.0, 0.5]));
    let clicked = ui.button_with_size(
        "Edit###edit-switch",
        [edit_switch_width(ui), ui.frame_height()],
    );
    let [x, y0] = ui.item_rect_min();
    let y = (y0 + ui.item_rect_max()[1]) * 0.5;
    let dl = ui.get_window_draw_list();
    dl.add_rect(
        [x + 8.0, y - 6.0],
        [x + 28.0, y + 6.0],
        if on { OK } else { DIM },
    )
    .rounding(6.0)
    .filled(true)
    .build();
    dl.add_circle([x + if on { 22.0 } else { 14.0 }, y], 4.0, BAR_BG)
        .filled(true)
        .build();
    clicked
}

pub enum TransportIcon {
    Play,
    Stop,
    Pause,
    Step,
}

pub fn transport_button(ui: &Ui, icon: TransportIcon, tooltip: &str) -> bool {
    let clicked = ui.button_with_size(format!("##{tooltip}"), [BAR_BUTTON, ui.frame_height()]);
    let min = ui.item_rect_min();
    let max = ui.item_rect_max();
    let [x, y] = [(min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5];
    let colour = ui.style_color(StyleColor::Text);
    let dl = ui.get_window_draw_list();
    let rect = |x0, x1| {
        dl.add_rect([x + x0, y - 5.0], [x + x1, y + 5.0], colour)
            .filled(true)
            .build();
    };
    let triangle = |dx| {
        dl.add_triangle(
            [x - 4.0 + dx, y - 5.0],
            [x + 5.0 + dx, y],
            [x - 4.0 + dx, y + 5.0],
            colour,
        )
        .filled(true)
        .build();
    };
    match icon {
        TransportIcon::Play => triangle(0.0),
        TransportIcon::Stop => rect(-5.0, 5.0),
        TransportIcon::Pause => {
            rect(-5.0, -2.0);
            rect(2.0, 5.0);
        }
        TransportIcon::Step => {
            triangle(-1.5);
            rect(4.0, 6.0);
        }
    }
    ui.set_item_tooltip(tooltip);
    clicked
}

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
