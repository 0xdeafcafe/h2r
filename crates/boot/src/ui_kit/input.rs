//! Input on a screen of ours, and the glyphs its prompts name
//! (docs/game/ui-screens.md "Input").
//!
//! **Events.** The screen's event slot (screen slot 10) is handed an event
//! whose kind word (`+0`) is a direction or a button: the controller update
//! (`0x1907d6`, through `0x190a3d`) raises kind 1 up, 2 left, 3 down and 4
//! right from the d-pad or the left stick, first when pushed and again
//! every 250 ms while held. Buttons are kind 5 with the button in `+8` (A
//! 0, B 1, X 2, Y 3, black 4, white 5, the triggers, then Start 12 and
//! Back 13), raised only on the frame the button goes down. A lane once
//! waited for the d-pad as buttons 8 to 11: they never arrive that way.
//! [`Input::from_event`] is the one reading of an event.
//!
//! **The opener's press.** The press that opens a screen can be seen again
//! by it (the skulls page: the difficulty A, then a "continue" on the
//! page). [`OpenerGuard`] drops a confirm that comes sooner than two
//! presses can be made.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::list_class::{BUTTON_EVENT, DOWN_EVENT, LEFT_EVENT, RIGHT_EVENT, UP_EVENT};

/// One input a screen of ours handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
pub enum Input {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    X,
    Y,
    Start,
    Back,
}

/// Button words (`+8` of a kind 5 event).
pub const A_BUTTON: u32 = 0;
pub const B_BUTTON: u32 = 1;
pub const X_BUTTON: u32 = 2;
pub const Y_BUTTON: u32 = 3;
pub const START_BUTTON: u32 = 12;
pub const BACK_BUTTON: u32 = 13;

impl Input {
    /// An event's kind and button words. `None` for anything a screen of
    /// ours does not take (black, white, the triggers, sticks as buttons).
    pub fn from_event(kind: u32, button: u32) -> Option<Input> {
        Some(match (kind, button) {
            (UP_EVENT, _) => Input::Up,
            (DOWN_EVENT, _) => Input::Down,
            (LEFT_EVENT, _) => Input::Left,
            (RIGHT_EVENT, _) => Input::Right,
            (BUTTON_EVENT, A_BUTTON) => Input::A,
            (BUTTON_EVENT, B_BUTTON) => Input::B,
            (BUTTON_EVENT, X_BUTTON) => Input::X,
            (BUTTON_EVENT, Y_BUTTON) => Input::Y,
            (BUTTON_EVENT, START_BUTTON) => Input::Start,
            (BUTTON_EVENT, BACK_BUTTON) => Input::Back,
            _ => return None,
        })
    }
    /// The event `(kind, button)` words that raise it: the inverse of
    /// [`Input::from_event`], for tests and scripted input.
    pub fn event(self) -> (u32, u32) {
        match self {
            Input::Up => (UP_EVENT, 0),
            Input::Down => (DOWN_EVENT, 0),
            Input::Left => (LEFT_EVENT, 0),
            Input::Right => (RIGHT_EVENT, 0),
            Input::A => (BUTTON_EVENT, A_BUTTON),
            Input::B => (BUTTON_EVENT, B_BUTTON),
            Input::X => (BUTTON_EVENT, X_BUTTON),
            Input::Y => (BUTTON_EVENT, Y_BUTTON),
            Input::Start => (BUTTON_EVENT, START_BUTTON),
            Input::Back => (BUTTON_EVENT, BACK_BUTTON),
        }
    }
    /// The input named `name` (`up`, `a`, `start`, `back`; any case), as
    /// `h2 ui preview --press` takes them.
    pub fn named(name: &str) -> Option<Input> {
        Some(match name.to_ascii_lowercase().as_str() {
            "up" => Input::Up,
            "down" => Input::Down,
            "left" => Input::Left,
            "right" => Input::Right,
            "a" => Input::A,
            "b" => Input::B,
            "x" => Input::X,
            "y" => Input::Y,
            "start" => Input::Start,
            "back" => Input::Back,
            _ => return None,
        })
    }
    /// Raised again every 250 ms while held (directions only).
    pub fn repeats(self) -> bool {
        matches!(self, Input::Up | Input::Down | Input::Left | Input::Right)
    }
}

/// A button picture in the fonts' icon range (docs/input/button-icons.md:
/// every disc font maps the same pictures at the same code points; the
/// runner swaps the pictures for the device in use).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize)]
pub enum Glyph {
    A,
    B,
    X,
    Y,
    Black,
    White,
    LeftTrigger,
    RightTrigger,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    Start,
    Back,
    LeftStick,
    RightStick,
}

impl Glyph {
    pub const ALL: [Glyph; 16] = [
        Glyph::A,
        Glyph::B,
        Glyph::X,
        Glyph::Y,
        Glyph::Black,
        Glyph::White,
        Glyph::LeftTrigger,
        Glyph::RightTrigger,
        Glyph::DpadUp,
        Glyph::DpadDown,
        Glyph::DpadLeft,
        Glyph::DpadRight,
        Glyph::Start,
        Glyph::Back,
        Glyph::LeftStick,
        Glyph::RightStick,
    ];
    /// Its code point (U+E100 up, in the order above).
    pub const fn char(self) -> char {
        match char::from_u32(0xe100 + self as u32) {
            Some(c) => c,
            None => unreachable!(),
        }
    }
    /// The picture for the input that raises `input`.
    pub fn of(input: Input) -> Glyph {
        match input {
            Input::Up => Glyph::DpadUp,
            Input::Down => Glyph::DpadDown,
            Input::Left => Glyph::DpadLeft,
            Input::Right => Glyph::DpadRight,
            Input::A => Glyph::A,
            Input::B => Glyph::B,
            Input::X => Glyph::X,
            Input::Y => Glyph::Y,
            Input::Start => Glyph::Start,
            Input::Back => Glyph::Back,
        }
    }
    /// The glyph's name for logs and previews.
    pub fn name(self) -> &'static str {
        match self {
            Glyph::A => "A",
            Glyph::B => "B",
            Glyph::X => "X",
            Glyph::Y => "Y",
            Glyph::Black => "Black",
            Glyph::White => "White",
            Glyph::LeftTrigger => "LT",
            Glyph::RightTrigger => "RT",
            Glyph::DpadUp => "Up",
            Glyph::DpadDown => "Down",
            Glyph::DpadLeft => "Left",
            Glyph::DpadRight => "Right",
            Glyph::Start => "Start",
            Glyph::Back => "Back",
            Glyph::LeftStick => "LS",
            Glyph::RightStick => "RS",
        }
    }
}

/// One prompt of a legend: its pictures, then what it does.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Prompt {
    pub glyphs: Vec<Glyph>,
    pub text: String,
}

impl Prompt {
    pub fn new(glyphs: &[Glyph], text: &str) -> Self {
        Prompt {
            glyphs: glyphs.to_vec(),
            text: text.into(),
        }
    }
}

/// A prompt's glyphs a space apart: the fonts draw the d-pad's left and
/// right arrows edge to edge, so side by side they read as one diamond
/// (the Game Options footer, 2026-11-18).
fn glyphs(gs: &[Glyph]) -> String {
    gs.iter()
        .map(|g| g.char().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The legend as the font draws it: each prompt's glyphs, a space, its
/// words; prompts three spaces apart (retail's own legends' spacing).
pub fn legend(prompts: &[Prompt]) -> String {
    prompts
        .iter()
        .map(|p| {
            let g = glyphs(&p.glyphs);
            format!("{g} {}", p.text)
        })
        .collect::<Vec<_>>()
        .join("   ")
}

/// The legend as the game's footers write it: glyph, space, the words in
/// capitals, two spaces between prompts (`a_select_b_back`: "(A) SELECT
/// (B) BACK" in mainmenu.map's strings).
pub fn footer_legend(prompts: &[Prompt]) -> String {
    prompts
        .iter()
        .map(|p| {
            let g = glyphs(&p.glyphs);
            format!("{g} {}", p.text.to_uppercase())
        })
        .collect::<Vec<_>>()
        .join("  ")
}

/// The legend with names in place of glyphs, for logs: `[Left Right]
/// Toggle   [A] Start Level`.
pub fn legend_named(prompts: &[Prompt]) -> String {
    prompts
        .iter()
        .map(|p| {
            let g: Vec<&str> = p.glyphs.iter().map(|g| g.name()).collect();
            format!("[{}] {}", g.join(" "), p.text)
        })
        .collect::<Vec<_>>()
        .join("   ")
}

/// A confirm this soon after a screen opened is the press that opened it,
/// seen again: under two frames at 30 fps, and well under the gap between
/// two presses of a thumb.
pub const OPENER_GUARD: Duration = Duration::from_millis(50);

/// Drops the opener's press: [`OpenerGuard::opened`] when the screen
/// opens, then [`OpenerGuard::too_soon`] on each confirm.
#[derive(Debug)]
pub struct OpenerGuard {
    opened: Mutex<Option<Instant>>,
    guard: Duration,
}

impl Default for OpenerGuard {
    fn default() -> Self {
        Self::new(OPENER_GUARD)
    }
}

impl OpenerGuard {
    pub fn new(guard: Duration) -> Self {
        OpenerGuard {
            opened: Mutex::new(None),
            guard,
        }
    }
    pub fn guard(&self) -> Duration {
        self.guard
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Instant>> {
        self.opened.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// The screen opened now.
    pub fn opened(&self) {
        self.opened_at(Instant::now());
    }
    pub fn opened_at(&self, at: Instant) {
        *self.lock() = Some(at);
    }
    /// When the screen last opened.
    pub fn last(&self) -> Option<Instant> {
        *self.lock()
    }
    /// How long after the opening `now` is, when within the guard.
    pub fn too_soon(&self, now: Instant) -> Option<Duration> {
        self.too_soon_by(now, self.guard)
    }
    /// The same against a guard of the caller's (one set after opening).
    pub fn too_soon_by(&self, now: Instant, guard: Duration) -> Option<Duration> {
        let since = now.saturating_duration_since(self.last()?);
        (since < guard).then_some(since)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_read_as_inputs_and_back() {
        for i in [
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
            Input::A,
            Input::B,
            Input::X,
            Input::Y,
            Input::Start,
            Input::Back,
        ] {
            let (k, b) = i.event();
            assert_eq!(Input::from_event(k, b), Some(i), "{i:?}");
        }
        // Directions are kinds 1 to 4 whatever the button word; the d-pad
        // as buttons 8 to 11 never comes, and is not read as a direction.
        assert_eq!(Input::from_event(2, 99), Some(Input::Left));
        for b in 8..=11 {
            assert_eq!(Input::from_event(BUTTON_EVENT, b), None, "button {b}");
        }
        assert_eq!(Input::from_event(BUTTON_EVENT, 4), None, "black");
        assert!(Input::Left.repeats() && !Input::A.repeats());
    }

    #[test]
    fn glyphs_are_the_fonts_code_points() {
        assert_eq!(Glyph::A.char(), '\u{e100}');
        assert_eq!(Glyph::B.char(), '\u{e101}');
        assert_eq!(Glyph::X.char(), '\u{e102}');
        assert_eq!(Glyph::DpadUp.char(), '\u{e108}');
        assert_eq!(Glyph::DpadLeft.char(), '\u{e10a}');
        assert_eq!(Glyph::DpadRight.char(), '\u{e10b}');
        assert_eq!(Glyph::Start.char(), '\u{e10c}');
        assert_eq!(Glyph::Back.char(), '\u{e10d}');
        assert_eq!(Glyph::RightStick.char(), '\u{e10f}');
        assert_eq!(Glyph::of(Input::Left), Glyph::DpadLeft);
    }

    #[test]
    fn a_legend_is_glyphs_then_words_three_spaces_apart() {
        let p = [
            Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Toggle"),
            Prompt::new(&[Glyph::A], "Start Level"),
            Prompt::new(&[Glyph::B], "Back"),
        ];
        assert_eq!(
            legend(&p),
            "\u{e10a} \u{e10b} Toggle   \u{e100} Start Level   \u{e101} Back"
        );
        assert_eq!(
            legend_named(&p),
            "[Left Right] Toggle   [A] Start Level   [B] Back"
        );
    }

    #[test]
    fn the_guard_drops_a_confirm_right_after_opening() {
        let g = OpenerGuard::default();
        let t = Instant::now();
        assert_eq!(g.too_soon(t), None, "never opened");
        g.opened_at(t);
        assert_eq!(
            g.too_soon(t + Duration::from_millis(10)),
            Some(Duration::from_millis(10))
        );
        assert_eq!(g.too_soon(t + OPENER_GUARD), None);
    }
}
