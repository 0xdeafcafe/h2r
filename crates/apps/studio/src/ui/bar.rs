//! The fixed bar under the main menu: tool tabs on the left, the open
//! document in the centre and the game link on the right.

use dear_imgui_rs::{Condition, StyleColor, StyleVar, Ui, WindowFlags};

use super::play::{PlayUi, link_icon};
use crate::app::{Action, App, ToolKind};
use crate::play::LinkView;
use crate::play::pair::Request;
use crate::theme::{self, Fonts, TransportIcon};

const TABS: [(ToolKind, &str); 5] = [
    (ToolKind::Scenario, "Map"),
    (ToolKind::Variants, "Variants"),
    (ToolKind::Playlists, "Playlists"),
    (ToolKind::UiScreens, "UI"),
    (ToolKind::Hud, "HUD"),
];

/// The window's padding at its left and right edges.
const EDGE: f32 = 10.0;

pub(super) fn is_map(tool: ToolKind) -> bool {
    matches!(tool, ToolKind::Scenario | ToolKind::Tags)
}

pub(super) fn draw(
    ui: &Ui,
    app: &mut App,
    play: &mut PlayUi,
    fonts: &Fonts,
    actions: &mut Vec<Action>,
) {
    let vp = ui.main_viewport();
    let pos = vp.work_pos();
    let width = vp.work_size()[0];
    ui.set_next_window_viewport(vp.id());
    let flags = WindowFlags::NO_DECORATION
        | WindowFlags::NO_DOCKING
        | WindowFlags::NO_MOVE
        | WindowFlags::NO_SAVED_SETTINGS
        | WindowFlags::NO_SCROLLBAR
        | WindowFlags::NO_SCROLL_WITH_MOUSE
        | WindowFlags::NO_FOCUS_ON_APPEARING
        | WindowFlags::NO_NAV_FOCUS;
    let _rounding = ui.push_style_var(StyleVar::WindowRounding(0.0));
    let _border = ui.push_style_var(StyleVar::WindowBorderSize(0.0));
    let _padding = ui.push_style_var(StyleVar::WindowPadding([EDGE, 0.0]));
    let _bg = ui.push_style_color(StyleColor::WindowBg, theme::BAR_BG);
    ui.window("##studio-bar")
        .flags(flags)
        .position(pos, Condition::Always)
        .size([width, theme::BAR], Condition::Always)
        .build(|| {
            let _font = ui.push_font(fonts.regular);
            // Every control sits on one row, centred in the bar's height.
            let row = (theme::BAR - ui.frame_height()) * 0.5;
            ui.set_cursor_pos([EDGE, row]);
            for (index, (tool, name)) in TABS.into_iter().enumerate() {
                if index > 0 {
                    ui.same_line();
                }
                let active = app.tool == tool || (tool == ToolKind::Scenario && is_map(app.tool));
                let dirty = app.tool_dirty(tool);
                let label = format!("{name}{}###tool-{name}", if dirty { " *" } else { "" });
                let _tone = active.then(|| ui.push_style_color(StyleColor::Button, theme::ACCENT));
                if ui.button(label) && app.tool != tool {
                    actions.push(Action::SetTool(tool));
                }
            }
            let tabs_end = ui.item_rect_max()[0] - pos[0];

            // The link keeps the right edge. The document name takes what
            // is left between the tabs and the link.
            let view = play.play.displayed_link(app);
            let issue_label = view.issue_summary().map(|summary| {
                let headline = view.text
                    .strip_suffix(&format!(" · {summary}"))
                    .unwrap_or(&view.text);
                format!("{headline}\n{summary}")
            });
            let pill_width = {
                let _font = issue_label.as_ref().map(|_| ui.push_font(fonts.caption));
                let label = issue_label.as_deref().unwrap_or(&view.text);
                theme::pill_width(ui, link_icon(view.tone), label)
            };
            let link_start = (width - EDGE - link_width(ui, &view, pill_width))
                .max(tabs_end + theme::BAR_SPACING);
            let document_end = link_start - 16.0;
            if let Some(name) = app.document_caption()
                && document_end > tabs_end + 24.0
            {
                let _font = ui.push_font(fonts.title);
                let text_width = ui.calc_text_size(&name)[0];
                let start = ((width - text_width) / 2.0)
                    .min(document_end - text_width)
                    .max(tabs_end + 16.0);
                ui.set_cursor_pos([start, row]);
                ui.align_text_to_frame_padding();
                theme::clipped_text(ui, &name, theme::TEXT, document_end - start);
            }

            let clicked = {
                let _font = issue_label.as_ref().map(|_| ui.push_font(fonts.caption));
                let label = issue_label.as_deref().unwrap_or(&view.text);
                let height = ui.calc_text_size(label)[1] + 10.0;
                ui.set_cursor_pos([link_start, (theme::BAR - height).max(0.0) * 0.5]);
                theme::pill(ui, theme::link_colour(view.tone), link_icon(view.tone), label)
            };
            if issue_label.is_some() {
                ui.set_item_tooltip("Show pending edits");
                if clicked {
                    app.pairing.open_pending = true;
                }
            } else if let Some(why) = &view.why {
                ui.set_item_tooltip(why);
            }
            play.bar_pill(ui, app, &view, clicked && issue_label.is_none());
            link(ui, app, play, &view, row);
        });
}

/// The width of the link pill and the controls that follow it.
fn link_width(ui: &Ui, view: &LinkView, pill_width: f32) -> f32 {
    let buttons = [view.can_play, view.can_stop, view.can_pause, view.paused];
    let mut width = pill_width
        + (theme::BAR_BUTTON + theme::BAR_SPACING)
            * buttons.into_iter().filter(|&on| on).count() as f32;
    if view.can_pause {
        width += theme::edit_switch_width(ui) + theme::BAR_BUTTON + 2.0 * theme::BAR_SPACING;
    }
    width
}

/// The game link: the pill, then the transport buttons and the Edit
/// switch when the link allows them.
fn link(ui: &Ui, app: &mut App, play: &mut PlayUi, view: &LinkView, row: f32) {
    let same_line = || {
        ui.same_line_with_spacing(0.0, theme::BAR_SPACING);
        ui.set_cursor_pos([ui.cursor_pos()[0], row]);
    };
    if view.can_play {
        same_line();
        if theme::transport_button(ui, TransportIcon::Play, "Play (F5)") {
            play.play.request(Request::Play);
        }
    }
    if view.can_stop {
        same_line();
        if theme::transport_button(ui, TransportIcon::Stop, "Stop (Shift+F5)") {
            play.play.request(Request::Stop);
        }
    }
    if view.can_pause {
        same_line();
        let (icon, text, request) = if view.paused {
            (TransportIcon::Play, "Resume", Request::Resume)
        } else {
            (TransportIcon::Pause, "Pause", Request::Pause)
        };
        if theme::transport_button(ui, icon, text) {
            play.play.request(request);
        }
    }
    if view.paused {
        same_line();
        if theme::transport_button(ui, TransportIcon::Step, "Step") {
            play.step();
        }
    }
    if view.can_pause {
        same_line();
        let enabled = app.pairing.selection.enabled;
        let available = crate::play::Play::can_aim_select(app);
        let disabled = ui.begin_disabled_with_cond(!available);
        let tone = enabled.then(|| ui.push_style_color(StyleColor::Button, theme::ACCENT));
        if ui.button_with_size(theme::icon::CROSSHAIR, [theme::BAR_BUTTON, ui.frame_height()])
            && let Err(why) = crate::play::Play::set_aim_select(app, !enabled)
        {
            app.error(why);
        }
        ui.set_item_tooltip("Aim select");
        drop(tone);
        drop(disabled);
        same_line();
        play.edit_switch(ui, view);
    }
}
