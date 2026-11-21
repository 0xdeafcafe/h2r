//! Fixed bar below the menu. The game link occupies its right side.

use dear_imgui_rs::{Condition, StyleColor, StyleVar, Ui, WindowFlags};

use super::play::{PlayUi, link_icon};
use crate::App;
use crate::play::pair::Request;
use crate::theme::{self, Fonts, TransportIcon};

pub fn draw(ui: &Ui, app: &mut App, play: &mut PlayUi, fonts: &Fonts) {
    let vp = ui.main_viewport();
    let (pos, size) = (vp.work_pos(), vp.work_size());
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
    let _padding = ui.push_style_var(StyleVar::WindowPadding([10.0, 0.0]));
    let _bg = ui.push_style_color(StyleColor::WindowBg, theme::BAR_BG);
    ui.window("##studio-bar")
        .flags(flags)
        .position(pos, Condition::Always)
        .size([size[0], theme::BAR], Condition::Always)
        .build(|| {
            let _font = ui.push_font(fonts.regular);
            let view = play.play.link_view(app);
            let can_edit = view.state.can_edit();
            let buttons = [view.can_stop || view.can_play, view.can_pause, view.paused];
            let extra_width = (theme::BAR_BUTTON + theme::BAR_SPACING)
                * buttons.into_iter().filter(|&on| on).count() as f32
                + if can_edit {
                    theme::edit_switch_width(ui) + theme::BAR_SPACING
                } else {
                    0.0
                };
            let width = theme::pill_width(ui, link_icon(view.tone), &view.text) + extra_width;
            let start = ui.cursor_pos();
            ui.set_cursor_pos([
                start[0] + (ui.content_region_avail()[0] - width).max(0.0),
                (theme::BAR - ui.frame_height()) * 0.5,
            ]);
            play.bar_pill(ui, app, &view);
            let same_line = || ui.same_line_with_spacing(0.0, theme::BAR_SPACING);
            if view.can_stop {
                same_line();
                if theme::transport_button(ui, TransportIcon::Stop, "Stop (Shift+F5)") {
                    play.play.request(Request::Stop);
                }
            } else if view.can_play {
                same_line();
                if theme::transport_button(ui, TransportIcon::Play, "Play (F5)") {
                    play.play.request(Request::Play);
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
            if can_edit {
                same_line();
                play.edit_switch(ui);
            }
        });
}
