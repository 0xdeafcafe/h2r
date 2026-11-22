//! The UI Screens tool's animation transport (docs/game/ui-builder.md
//! "Editing in the editor"): one of the kit's transitions
//! ([`Transition`]) played, paused, looped or scrubbed by tick, at 0.25 to
//! 2 times the builder's rate, with an optional onion skin of the
//! widgets two ticks either side.

use dear_imgui_rs::Ui;

use boot::ui_kit::animation::{Phase, TICKS_PER_SECOND, Transition};
use boot::ui_kit::view::View;

use crate::app::UiScreens;
use crate::theme;

/// Ticks either side the onion skin shows.
const ONION: u32 = 2;

/// The transport's state.
pub struct Transport {
    pub transition: Transition,
    /// Where on its timeline, in ticks.
    pub tick: f32,
    pub playing: bool,
    pub looping: bool,
    /// 0.25 to 2.
    pub speed: f32,
    pub onion: bool,
    /// Showing the transition; off, the screen is at rest.
    pub active: bool,
}

impl Default for Transport {
    fn default() -> Self {
        Transport {
            transition: Transition::Open,
            tick: 0.0,
            playing: false,
            looping: true,
            speed: 1.0,
            onion: false,
            active: false,
        }
    }
}

impl Transport {
    /// The pose shown.
    pub fn phase(&self) -> Phase {
        if self.active {
            self.transition.phase(self.tick as u32)
        } else {
            Phase::Rest
        }
    }

    /// Moves the timeline on by `dt` seconds of play.
    pub fn advance(&mut self, dt: f32) {
        if !self.playing {
            return;
        }
        let end = self.transition.ticks() as f32;
        self.tick += dt * TICKS_PER_SECOND as f32 * self.speed.clamp(0.25, 2.0);
        if self.tick > end {
            if self.looping {
                self.tick %= end.max(1.0);
            } else {
                self.tick = end;
                self.playing = false;
            }
        }
    }

    /// The onion skin's poses: two ticks before and after.
    fn onion_phases(&self) -> [Phase; 2] {
        let t = self.tick as u32;
        [
            self.transition.phase(t.saturating_sub(ONION)),
            self.transition.phase(t + ONION),
        ]
    }
}

/// The transport's row above the picture; sets the pose shown.
pub fn bar(ui: &Ui, s: &mut UiScreens, t: &mut Transport) {
    let name = |tr: Transition| match tr {
        Transition::Open => "Open",
        Transition::Close => "Close",
    };
    ui.set_next_item_width(90.0);
    if let Some(_combo) = ui.begin_combo("##ui-transition", name(t.transition)) {
        for tr in Transition::ALL {
            if ui
                .selectable_config(name(tr))
                .selected(t.transition == tr)
                .build()
            {
                t.transition = tr;
                t.tick = 0.0;
                t.active = true;
            }
        }
    }
    ui.same_line();
    if ui.button(if t.playing { "Pause##ui-play" } else { "Play##ui-play" }) {
        if !t.playing && t.tick >= t.transition.ticks() as f32 {
            t.tick = 0.0;
        }
        t.playing = !t.playing;
        t.active = true;
    }
    ui.same_line();
    ui.checkbox("Loop##ui-loop", &mut t.looping);
    ui.same_line();
    let end = t.transition.ticks() as f32;
    ui.set_next_item_width(160.0);
    if ui.slider_f32("##ui-scrub", &mut t.tick, 0.0, end) {
        t.playing = false;
        t.active = true;
    }
    ui.same_line();
    ui.text_disabled(format!(
        "{:.2}s",
        Transition::seconds(t.tick as u32)
    ));
    ui.same_line();
    ui.set_next_item_width(90.0);
    ui.slider_f32("Speed##ui-speed", &mut t.speed, 0.25, 2.0);
    ui.same_line();
    ui.checkbox("Onion##ui-onion", &mut t.onion);
    ui.same_line();
    if ui.button("Rest##ui-rest") {
        t.playing = false;
        t.active = false;
        t.tick = 0.0;
    }
    t.advance(ui.io().delta_time());
    s.set_animation(t.phase());
}

/// The onion skin's outlines over the picture (`at` maps UI pixels to
/// points): the widgets two ticks before and after the pose shown.
pub fn onion(ui: &Ui, s: &mut UiScreens, t: &Transport, at: &dyn Fn([f32; 2]) -> [f32; 2]) {
    if !(t.onion && t.active) {
        return;
    }
    let poses: Vec<View> = s.poses(&t.onion_phases());
    let dl = ui.get_window_draw_list();
    for (pose, colour) in poses.iter().zip([theme::OTHER_HUD, theme::SAFE_FRAME]) {
        for w in pose.widgets.iter().filter(|w| w.alpha != 0) {
            let [x, y, wd, h] = w.pixels().map(|v| v as f32);
            let mut c = colour;
            c[3] = 0.25 + 0.5 * f32::from(w.alpha) / 255.0;
            dl.add_rect(at([x, y]), at([x + wd, y + h]), c)
                .thickness(1.0)
                .build();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_loops_or_stops_at_the_end() {
        let mut t = Transport {
            playing: true,
            active: true,
            ..Default::default()
        };
        let end = Transition::Open.ticks() as f32;
        // Half a second at 30 ticks a second, double speed: past the end.
        t.speed = 2.0;
        t.advance(0.5);
        assert!(t.tick < end && t.playing, "looped to {}", t.tick);
        t.looping = false;
        t.tick = end - 1.0;
        t.advance(1.0);
        assert_eq!(t.tick, end);
        assert!(!t.playing);
        assert_eq!(t.phase(), Phase::In(Transition::Open.ticks()));
        t.active = false;
        assert_eq!(t.phase(), Phase::Rest);
        // Paused, nothing moves.
        t.tick = 3.0;
        t.advance(1.0);
        assert_eq!(t.tick, 3.0);
        t.active = true;
        assert_eq!(t.onion_phases(), [Phase::In(1), Phase::In(5)]);
    }
}
