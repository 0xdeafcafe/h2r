//! The look: a dark game-tool theme (neutral greys, one blue accent,
//! small rounding, roomy spacing) and the bundled fonts, Inter for text
//! and a Lucide subset for icons (`assets/fonts`, OFL and ISC licensed).
use dear_imgui_rs::{
    ChildFlags, Context, FontConfig, FontId, FontSource, StbTrueTypeFontData, Style, StyleColor,
    StyleVar, Ui,
};

use crate::play::LinkTone;

/// Text size in logical points. Dear ImGui 1.92 rasterises at the
/// framebuffer scale, so this stays crisp on HiDPI displays.
pub const CAPTION_SIZE: f32 = 12.0;
pub const FONT_SIZE: f32 = 15.0;
pub const TITLE_SIZE: f32 = 18.0;

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
    pub caption: FontId,
    pub title: FontId,
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
    let caption = atlas.add_font(&[source(INTER_REGULAR, CAPTION_SIZE), icons(CAPTION_SIZE)]);
    let title = atlas.add_font(&[source(INTER_SEMIBOLD, TITLE_SIZE), icons(TITLE_SIZE)]);
    Fonts { regular, bold, caption, title }
}

/// RGBA from 0..255 components.
const fn rgb(r: u8, g: u8, b: u8) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}
const fn rgba(r: u8, g: u8, b: u8, a: f32) -> [f32; 4] {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a]
}

pub const BG: [f32; 4] = rgb(24, 25, 28);
pub const PANEL: [f32; 4] = rgb(30, 31, 35);
pub const RAISED: [f32; 4] = rgb(42, 44, 50);
pub const TEXT: [f32; 4] = rgb(222, 224, 228);
pub const STATUS_BG: [f32; 4] = rgb(19, 20, 23);
pub const RULE: [f32; 4] = rgb(48, 50, 56);

pub const HUD_WIDGET: [f32; 4] = [0.35, 0.8, 1.0, 0.45];
pub const SCREEN_WIDGET: [f32; 4] = [0.35, 0.8, 1.0, 0.55];
pub const WIDGET_FOCUS: [f32; 4] = [1.0, 1.0, 1.0, 0.95];
pub const WIDGET_ERROR: [f32; 4] = [1.0, 0.3, 0.25, 1.0];
pub const SAFE_FRAME: [f32; 4] = [0.4, 1.0, 0.45, 0.8];
pub const HIDDEN_WIDGET: [f32; 4] = [0.6, 0.6, 0.6, 0.5];
pub const OTHER_HUD: [f32; 4] = [1.0, 0.75, 0.2, 0.5];
pub const RESERVED_AREA: [f32; 4] = [1.0, 0.75, 0.2, 0.8];

/// Authoring glyphs and live players share the same team swatches.
const TEAM_COLORS: [[u8; 3]; 8] = [
    [255, 110, 95],
    [105, 175, 255],
    [84, 215, 144],
    [255, 193, 72],
    [174, 144, 255],
    [255, 156, 206],
    [112, 217, 225],
    [191, 155, 127],
];
pub(crate) const NEUTRAL_COLOR: [u8; 3] = [225, 230, 235];

pub(crate) fn team_color(team: i64) -> [u8; 3] {
    usize::try_from(team)
        .ok()
        .and_then(|i| TEAM_COLORS.get(i).copied())
        .unwrap_or(NEUTRAL_COLOR)
}

pub(crate) fn placement_color(kind: scenario_edit::BlockKind, team: Option<i64>) -> [f32; 3] {
    use scenario_edit::BlockKind::*;
    if let Some(team) = team {
        return team_color(team).map(|v| f32::from(v) / 255.0);
    }
    match kind {
        Scenery => [0.55, 0.55, 0.45],
        Vehicles => [0.2, 0.5, 1.0],
        Equipment => [0.2, 0.9, 0.9],
        Weapons => [1.0, 0.6, 0.1],
        Machines => [0.6, 0.4, 0.9],
        SoundScenery => [0.9, 0.9, 0.2],
        Crates => [0.6, 0.35, 0.2],
        StartingLocations => [0.2, 1.0, 0.3],
        NetgameFlags => [1.0, 0.2, 0.6],
        NetgameEquipment => [1.0, 0.25, 0.2],
        StartingEquipment => [0.7, 0.7, 0.7],
    }
}

/// At longer distances the gameplay glyph remains visible without its label.
pub(crate) const LABEL_DISTANCE: f32 = 40.0;
pub(crate) const LABEL_OFFSET: [f32; 2] = [7.0, -18.0];

/// Colours shared by the viewport's layers and the Show legend.
pub fn layer_color(id: scenario_edit::layers::LayerId) -> [f32; 3] {
    use scenario_edit::layers::LayerId;
    match id {
        LayerId::Structure => [0.70, 0.72, 0.76],
        LayerId::Sky => [0.4, 0.65, 0.9],
        LayerId::Objects => [0.9, 0.75, 0.5],
        LayerId::Fog => [0.65, 0.68, 0.75],
        LayerId::Markers => [0.85, 0.85, 0.85],
        LayerId::Placement(kind) => placement_color(kind, None),
        LayerId::SpawnPoints => placement_color(scenario_edit::BlockKind::StartingLocations, None),
        LayerId::Flags => placement_color(scenario_edit::BlockKind::NetgameFlags, None),
        LayerId::NetgameItems => placement_color(scenario_edit::BlockKind::NetgameEquipment, None),
        LayerId::Halo1 => [1.0, 0.55, 0.15],
        LayerId::Live => [0.42, 0.8, 0.55],
    }
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
    let frame_hover = rgb(52, 55, 62);
    let frame_active = rgb(62, 66, 75);
    let accent_dim = rgba(56, 132, 230, 0.55);
    let accent_soft = rgba(56, 132, 230, 0.30);
    use StyleColor as C;
    for (c, v) in [
        (C::Text, TEXT),
        (C::TextDisabled, rgb(105, 110, 120)),
        (C::WindowBg, PANEL),
        (C::ChildBg, rgba(0, 0, 0, 0.0)),
        (C::PopupBg, rgb(34, 35, 40)),
        (C::Border, RULE),
        (C::BorderShadow, rgba(0, 0, 0, 0.0)),
        (C::FrameBg, RAISED),
        (C::FrameBgHovered, frame_hover),
        (C::FrameBgActive, frame_active),
        (C::TitleBg, BG),
        (C::TitleBgActive, BG),
        (C::TitleBgCollapsed, BG),
        (C::MenuBarBg, rgb(20, 21, 24)),
        (C::ScrollbarBg, rgba(0, 0, 0, 0.0)),
        (C::ScrollbarGrab, rgb(58, 61, 68)),
        (C::ScrollbarGrabHovered, rgb(74, 78, 88)),
        (C::ScrollbarGrabActive, rgb(90, 95, 106)),
        (C::CheckMark, ACCENT),
        (C::SliderGrab, ACCENT),
        (C::SliderGrabActive, rgb(90, 156, 240)),
        (C::Button, RAISED),
        (C::ButtonHovered, frame_hover),
        (C::ButtonActive, accent_dim),
        (C::Header, accent_soft),
        (C::HeaderHovered, rgba(56, 132, 230, 0.40)),
        (C::HeaderActive, accent_dim),
        (C::Separator, RULE),
        (C::SeparatorHovered, accent_dim),
        (C::SeparatorActive, ACCENT),
        (C::ResizeGrip, rgba(0, 0, 0, 0.0)),
        (C::ResizeGripHovered, accent_dim),
        (C::ResizeGripActive, ACCENT),
        (C::Tab, BG),
        (C::TabHovered, frame_hover),
        (C::TabSelected, PANEL),
        (C::TabSelectedOverline, ACCENT),
        (C::TabDimmed, BG),
        (C::TabDimmedSelected, PANEL),
        (C::TabDimmedSelectedOverline, rgba(0, 0, 0, 0.0)),
        (C::DockingPreview, accent_dim),
        (C::DockingEmptyBg, BG),
        (C::TableHeaderBg, RAISED),
        (C::TableBorderStrong, RULE),
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

/// A labelled rule, shared by inspector and library sections.
pub fn section(ui: &Ui, label: impl AsRef<str>) {
    ui.separator_with_text(label.as_ref());
}

pub fn empty_card(ui: &Ui, fonts: &Fonts, title: &str, contents: impl FnOnce()) {
    let [available_width, available_height] = ui.content_region_avail();
    let width = (available_width - 16.0).clamp(1.0, 336.0);
    let height = available_height.clamp(1.0, 106.0);
    let start = ui.cursor_pos();
    ui.set_cursor_pos([
        start[0] + (available_width - width).max(0.0) * 0.5,
        start[1] + (available_height - height).max(0.0) * 0.5,
    ]);
    let _bg = ui.push_style_color(StyleColor::ChildBg, RAISED);
    let _padding = ui.push_style_var(StyleVar::WindowPadding([14.0, 12.0]));
    ui.child_window("##empty-card")
        .size([width, height])
        .child_flags(ChildFlags::ALWAYS_USE_WINDOW_PADDING)
        .build(ui, || {
            {
                let _font = ui.push_font(fonts.title);
                ui.text_wrapped(title);
            }
            contents();
        });
}

/// Keep long document names and log messages inside their assigned area.
pub fn clipped_text(ui: &Ui, text: &str, color: [f32; 4], width: f32) {
    if width <= 0.0 {
        return;
    }
    let [x, y] = ui.cursor_screen_pos();
    let draw = ui.get_window_draw_list();
    {
        let _clip = draw.push_clip_rect([x, y], [x + width, y + ui.text_line_height() + 8.0], true);
        ui.text_colored(color, text);
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

#[cfg(test)]
mod tests {
    use super::*;
    use scenario_edit::{BlockKind, layers::LayerId};

    #[test]
    fn gameplay_glyphs_take_their_layer_and_team_colours_from_the_theme() {
        let kind = BlockKind::StartingLocations;
        assert_eq!(placement_color(kind, None), layer_color(LayerId::SpawnPoints));
        let red = TEAM_COLORS[0].map(|v| f32::from(v) / 255.0);
        assert_eq!(placement_color(kind, Some(0)), red);
        assert_ne!(placement_color(kind, Some(1)), red);
        for team in [-1, 8, 900] {
            assert_eq!(team_color(team), NEUTRAL_COLOR);
        }
    }
}
