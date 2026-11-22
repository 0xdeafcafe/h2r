//! Paint game feedback over the viewport without placing interactive widgets.

use std::time::Instant;

use dear_imgui_rs::Ui;

use crate::play::Play;
use crate::theme;

pub(super) fn draw(ui: &Ui, play: &Play, [x, y, w, h]: [f32; 4]) {
    let margin = 8.0;
    let pad = 6.0;
    let width = (w - 2.0 * (margin + pad)).min(390.0);
    let mut notices = play.toasts(Instant::now()).peekable();
    if width < 32.0 || notices.peek().is_none() {
        return;
    }
    let list = ui.get_window_draw_list();
    let _clip = list.push_clip_rect([x, y], [x + w, y + h], true);
    let right = x + w - margin;
    let mut top = y + margin;
    let space = ui.calc_text_size(" ")[0];
    for (text, problem) in notices {
        if top >= y + h - margin {
            break;
        }
        let mut lines = String::new();
        let mut line_width = 0.0;
        for word in text.split_whitespace() {
            let word_width = ui.calc_text_size(word)[0];
            if line_width > 0.0 {
                if line_width + space + word_width > width {
                    lines.push('\n');
                    line_width = 0.0;
                } else {
                    lines.push(' ');
                    line_width += space;
                }
            }
            lines.push_str(word);
            line_width += word_width;
        }
        let [tw, th] = ui.calc_text_size(&lines);
        let left = right - tw.min(width) - 2.0 * pad;
        let bottom = top + th + 2.0 * pad;
        list.add_rect([left, top], [right, bottom], theme::BAR_BG)
            .filled(true)
            .rounding(3.0)
            .build();
        let stripe = if problem { theme::ERROR } else { theme::OK };
        list.add_rect([left, top], [left + 2.0, bottom], stripe).filled(true).build();
        let _text_clip = list.push_clip_rect([left, top], [right, bottom], true);
        list.add_text([left + pad, top + pad], theme::TEXT, &lines);
        top = bottom + 4.0;
    }
}
