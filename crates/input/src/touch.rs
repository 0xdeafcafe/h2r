//! On-screen touch controls (the iOS app's overlay, docs/runtime/ios.md):
//! what a touch host reports, turned into the pad player one sees.
//!
//! The host (Swift) only tracks fingers and draws. Each update it reports a
//! [`TouchFrame`]: which on-screen buttons are held, where the move thumb is
//! relative to where it landed, and how far the look thumb has dragged since
//! the last update. [`TouchPad`] does the rest, here where it can be tested:
//!
//! * **Move stick**: the thumb's offset over the stick's radius, a radial
//!   deadzone with the live range rescaled ([`move_stick`]), screen +y down
//!   turned into stick +y up.
//! * **Look, direct** (with the boot's hook installed, [`TouchTuning::direct_look`]):
//!   drag travel becomes a turn in radians ([`crate::touch_look`]) that the
//!   hook adds to player one's facing; the gyro's rate times the interval
//!   too. The stick is not used for touch look.
//! * **Look, stick** (no hook, the fallback): drag travel is relative motion,
//!   like a mouse or the Steam Controller's trackpad: it goes through
//!   [`LookMotion`], the mouse-look
//!   path, so it is held until the guest reads it and becomes a turn rate
//!   proportional to the drag's speed, independent of the game's frame rate.
//!   On top: a small jitter deadzone, acceleration with drag speed
//!   ([`look_gain`]), the sensitivity, and a response curve on the resulting
//!   deflection ([`curve`]) so slow drags still move Halo's stick, whose own
//!   response is flat near the centre.
//! * **Gyro** (optional): the device's rotation rate is a turn rate too, added
//!   to the look stick ([`gyro_stick`]).
//! * **Buttons**: one bit per pad control ([`bits`]); the triggers report full
//!   pull. Crouch (the left-stick click) can be a toggle ([`CrouchMode`]).
//! * **Auto-fire** (Touch Auto Fire): the right trigger is pulled while the
//!   reticle is red, as [`crate::touch_layout::fire_mode`] decides from the
//!   [`Context`] the host passes in ([`TouchPad::set_context`]).
//!
//! The result is merged into player one by the keyboard pad
//! (`backend::keyboard::KeyboardMouse::touch_update`), which shares port 0
//! with every other device, as the system's own touch controls did.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::host::{HostButton, HostButtons, HostPadState};
use crate::look::LookMotion;
use crate::touch_layout::{AutoFire, Context, FireMode, fire_mode};
use crate::touch_look;

static SHOWN: AtomicBool = AtomicBool::new(false);

/// Whether the host's on-screen touch controls are showing (the iOS app sets
/// it as they appear and go). Read by features that change with touch, such
/// as the aim assist's automatic level.
pub fn controls_shown() -> bool {
    SHOWN.load(Ordering::Relaxed)
}

/// Set by the host as its touch controls show or hide.
pub fn set_controls_shown(shown: bool) {
    SHOWN.store(shown, Ordering::Relaxed);
}

/// The held-button bits of [`TouchFrame::buttons`] (`H2_TOUCH_*` in h2host.h).
/// Named for the pad control; the labels the overlay draws are Halo 2's
/// Default layout (A jumps, B melees, X reloads, Y switches weapons, White is
/// the flashlight, Black switches grenades, the left-stick click crouches,
/// the right-stick click zooms).
pub mod bits {
    pub const A: u32 = 1 << 0;
    pub const B: u32 = 1 << 1;
    pub const X: u32 = 1 << 2;
    pub const Y: u32 = 1 << 3;
    /// Right shoulder.
    pub const BLACK: u32 = 1 << 4;
    /// Left shoulder.
    pub const WHITE: u32 = 1 << 5;
    pub const LEFT_TRIGGER: u32 = 1 << 6;
    pub const RIGHT_TRIGGER: u32 = 1 << 7;
    pub const START: u32 = 1 << 8;
    pub const BACK: u32 = 1 << 9;
    /// Left-stick click: crouch (a toggle or held, [`super::CrouchMode`]).
    pub const LEFT_THUMB: u32 = 1 << 10;
    /// Right-stick click: zoom.
    pub const RIGHT_THUMB: u32 = 1 << 11;
    pub const DPAD_UP: u32 = 1 << 12;
    pub const DPAD_DOWN: u32 = 1 << 13;
    pub const DPAD_LEFT: u32 = 1 << 14;
    pub const DPAD_RIGHT: u32 = 1 << 15;
    pub const ALL: u32 = (1 << 16) - 1;
}

/// What the touch host reports, each update.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TouchFrame {
    /// [`bits`] held.
    pub buttons: u32,
    /// The move thumb's offset from where it landed, in points (+y down),
    /// or `None` with no thumb on the move side.
    pub move_offset: Option<(f32, f32)>,
    /// Look drag since the previous update, in points (+y down).
    pub look: (f32, f32),
    /// Device rotation rate in radians per second, already in screen terms:
    /// +x turns right, +y looks up. Zero with gyro aiming off.
    pub gyro: (f32, f32),
}

/// How the crouch button behaves.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CrouchMode {
    /// A tap crouches, the next tap stands (a held stick click for the game).
    #[default]
    Toggle,
    /// Crouched while held, as the stick click is on a pad.
    Hold,
}

/// Touch tuning. Distances are in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchTuning {
    /// Thumb travel for full deflection of the move stick.
    pub stick_radius: f32,
    /// Fraction of the radius that is dead centre.
    pub stick_deadzone: f32,
    /// Look sensitivity multiplier (1.0 is the default speed).
    pub look_sensitivity: f64,
    /// Extra look gain at speed ([`look_gain`]).
    pub look_acceleration: f64,
    /// Exponent of the look response curve ([`curve`]); below 1 lifts small
    /// deflections, 1 is linear.
    pub look_curve: f64,
    /// Drags shorter than this per update are jitter and dropped.
    pub look_jitter: f64,
    /// Look down when dragging up.
    pub invert_y: bool,
    pub crouch: CrouchMode,
    /// Gyro sensitivity multiplier ([`gyro_stick`]); 0 turns gyro aiming off.
    pub gyro_sensitivity: f64,
    /// Look turns the camera directly ([`crate::touch_look`]): set while a
    /// hook takes the turn. Off, look is a stick deflection.
    pub direct_look: bool,
    /// Direct look's acceleration ([`touch_look::accel_gain`]; 1.0 default,
    /// 0 off): Look Acceleration / 5.
    pub direct_acceleration: f64,
    /// Fire by itself while the reticle is red (Touch Auto Fire).
    pub auto_fire: bool,
}

impl Default for TouchTuning {
    fn default() -> Self {
        Self {
            stick_radius: 56.0,
            stick_deadzone: 0.12,
            look_sensitivity: 1.0,
            look_acceleration: 0.6,
            look_curve: 0.75,
            look_jitter: 0.25,
            invert_y: false,
            crouch: CrouchMode::Toggle,
            gyro_sensitivity: 0.0,
            direct_look: false,
            direct_acceleration: 1.0,
            auto_fire: false,
        }
    }
}

/// Look deflection per point dragged per 1/60 s at sensitivity 1, before
/// acceleration and the curve: a 7-point drag in one frame (about 420 points
/// a second, a brisk swipe) is close to a full-speed turn.
pub const LOOK_K: f64 = 0.1;
/// The drag speed (points per second) at which acceleration has added
/// `look_acceleration` to the gain.
pub const ACCEL_SPEED: f64 = 800.0;
/// Acceleration stops growing at this many times [`ACCEL_SPEED`].
pub const ACCEL_CAP: f64 = 2.0;
/// Rotation rate (radians per second) that is a full look deflection at gyro
/// sensitivity 1: about 170 degrees a second.
pub const GYRO_FULL_RATE: f64 = 3.0;
/// Speeds use at least this long between look updates.
pub const MIN_INTERVAL: Duration = Duration::from_millis(4);
/// And at most this long (a pause between drags).
pub const MAX_INTERVAL: Duration = Duration::from_millis(50);

/// A radial deadzone with the live range rescaled to 0..1, clamped to the
/// unit circle: `offset` points from where the thumb landed (+y down) to a
/// stick (+y up). A thumb past the radius is full deflection in its
/// direction.
pub fn move_stick(offset: (f32, f32), radius: f32, deadzone: f32) -> (f32, f32) {
    let (x, y) = offset;
    if !(x.is_finite() && y.is_finite()) || !(radius.is_finite() && radius > 0.0) {
        return (0.0, 0.0);
    }
    let (x, y) = (x / radius, -y / radius);
    let mag = x.hypot(y);
    let dz = if deadzone.is_finite() {
        deadzone.clamp(0.0, 0.95)
    } else {
        0.0
    };
    if mag <= dz {
        return (0.0, 0.0);
    }
    let live = ((mag.min(1.0) - dz) / (1.0 - dz)).clamp(0.0, 1.0);
    (x / mag * live, y / mag * live)
}

/// Look gain for a drag at `speed` points per second:
/// `1 + acceleration * min(speed / ACCEL_SPEED, ACCEL_CAP)`.
pub fn look_gain(speed: f64, acceleration: f64) -> f64 {
    let s = if speed.is_finite() {
        speed.max(0.0)
    } else {
        0.0
    };
    1.0 + acceleration.max(0.0) * (s / ACCEL_SPEED).min(ACCEL_CAP)
}

/// The look response curve on a stick vector: the direction is kept, the
/// magnitude (clamped to 1) becomes `magnitude ^ exponent`.
pub fn curve(stick: (f64, f64), exponent: f64) -> (f64, f64) {
    let mag = stick.0.hypot(stick.1);
    if mag == 0.0 || !mag.is_finite() {
        return (0.0, 0.0);
    }
    let e = if exponent.is_finite() {
        exponent.clamp(0.2, 3.0)
    } else {
        1.0
    };
    let out = mag.min(1.0).powf(e);
    (stick.0 / mag * out, stick.1 / mag * out)
}

/// The gyro's rotation rate (radians per second, +x right, +y up) as a look
/// deflection, clamped per axis.
pub fn gyro_stick(rate: (f32, f32), sensitivity: f64) -> (f64, f64) {
    if !(sensitivity.is_finite() && sensitivity > 0.0) {
        return (0.0, 0.0);
    }
    let axis = |v: f32| {
        let v = f64::from(v);
        if v.is_finite() {
            (v * sensitivity / GYRO_FULL_RATE).clamp(-1.0, 1.0)
        } else {
            0.0
        }
    };
    (axis(rate.0), axis(rate.1))
}

/// The pad buttons for held [`bits`] (crouch already resolved; the triggers
/// are analog and not here).
pub fn buttons(held: u32) -> HostButtons {
    use HostButton::*;
    let mut b = HostButtons::default();
    for (bit, hb) in [
        (bits::A, South),
        (bits::B, East),
        (bits::X, West),
        (bits::Y, North),
        (bits::BLACK, RightBumper),
        (bits::WHITE, LeftBumper),
        (bits::START, Start),
        (bits::BACK, Back),
        (bits::LEFT_THUMB, LeftStick),
        (bits::RIGHT_THUMB, RightStick),
        (bits::DPAD_UP, DpadUp),
        (bits::DPAD_DOWN, DpadDown),
        (bits::DPAD_LEFT, DpadLeft),
        (bits::DPAD_RIGHT, DpadRight),
    ] {
        b.set(hb, held & bit != 0);
    }
    b
}

/// The touch controls' state: what [`TouchFrame`]s have said so far.
#[derive(Clone, Debug)]
pub struct TouchPad {
    tuning: TouchTuning,
    held: u32,
    /// Crouch latched on by a tap ([`CrouchMode::Toggle`]).
    crouched: bool,
    move_stick: (f32, f32),
    gyro: (f32, f32),
    look: LookMotion,
    last_look: Option<Instant>,
    /// The previous report (the gyro's interval in direct look).
    last_update: Option<Instant>,
    /// The turn the last report added in direct look (radians, right/up).
    last_turn: (f64, f64),
    /// Player one's state (`touch_layout`), for auto-fire.
    context: Context,
    auto_fire: AutoFire,
}

impl Default for TouchPad {
    fn default() -> Self {
        Self::new(TouchTuning::default())
    }
}

impl TouchPad {
    pub fn new(tuning: TouchTuning) -> Self {
        Self {
            tuning,
            held: 0,
            crouched: false,
            move_stick: (0.0, 0.0),
            gyro: (0.0, 0.0),
            look: LookMotion::default(),
            last_look: None,
            last_update: None,
            last_turn: (0.0, 0.0),
            context: Context::default(),
            auto_fire: AutoFire::default(),
        }
    }

    pub fn tuning(&self) -> TouchTuning {
        self.tuning
    }

    /// New tuning. Switching crouch to Hold stands a toggled crouch up.
    pub fn set_tuning(&mut self, tuning: TouchTuning) {
        if tuning.crouch == CrouchMode::Hold {
            self.crouched = false;
        }
        if tuning.direct_look != self.tuning.direct_look {
            self.look.clear();
        }
        self.tuning = tuning;
    }

    /// Player one's state now (for auto-fire).
    pub fn set_context(&mut self, context: Context) {
        self.context = context;
    }

    /// What auto-fire does now.
    pub fn fire_mode(&self) -> FireMode {
        fire_mode(self.tuning.auto_fire, &self.context)
    }

    /// The turn (radians, +x right, +y up) the last report added in direct
    /// look.
    pub fn last_turn(&self) -> (f64, f64) {
        self.last_turn
    }

    /// One report from the host at `now`.
    pub fn update(&mut self, f: &TouchFrame, now: Instant) {
        let held = f.buttons & bits::ALL;
        let pressed = held & !self.held;
        if pressed & bits::LEFT_THUMB != 0 && self.tuning.crouch == CrouchMode::Toggle {
            self.crouched = !self.crouched;
        }
        self.held = held;
        let t = &self.tuning;
        self.move_stick = f.move_offset.map_or((0.0, 0.0), |o| {
            move_stick(o, t.stick_radius, t.stick_deadzone)
        });
        let interval = |since: Option<Instant>| {
            since
                .map_or(MAX_INTERVAL, |at| now.saturating_duration_since(at))
                .clamp(MIN_INTERVAL, MAX_INTERVAL)
                .as_secs_f64()
        };
        let (dx, dy) = (f64::from(f.look.0), f64::from(f.look.1));
        if t.direct_look {
            // Every point turns: no jitter deadzone on the direct path.
            self.gyro = (0.0, 0.0);
            let drag = if dx.is_finite() && dy.is_finite() && (dx, dy) != (0.0, 0.0) {
                let d = touch_look::drag_turn(
                    (dx, dy),
                    interval(self.last_look),
                    t.look_sensitivity,
                    t.direct_acceleration,
                    t.invert_y,
                );
                self.last_look = Some(now);
                d
            } else {
                (0.0, 0.0)
            };
            let gyro =
                touch_look::gyro_turn(f.gyro, interval(self.last_update), t.gyro_sensitivity);
            self.last_update = Some(now);
            self.last_turn = (drag.0 + gyro.0, drag.1 + gyro.1);
            touch_look::push(now, self.last_turn);
            return;
        }
        self.last_update = Some(now);
        self.gyro = if t.gyro_sensitivity > 0.0 {
            f.gyro
        } else {
            (0.0, 0.0)
        };
        let mag = dx.hypot(dy);
        if mag.is_finite() && mag > t.look_jitter.max(0.0) {
            let dt = interval(self.last_look);
            let gain = look_gain(mag / dt, t.look_acceleration) * t.look_sensitivity.max(0.0);
            self.look.add(now, dx * gain, dy * gain);
            // Updates with no drag in between (the host's per-frame ones)
            // keep this: the speed is over the time since the last drag,
            // capped at MAX_INTERVAL, so a pause still starts slow.
            self.last_look = Some(now);
        }
    }

    /// Lets go of everything: the controls hid, or the app left the screen.
    /// A toggled crouch stands up.
    pub fn release(&mut self) {
        let context = self.context;
        *self = Self::new(self.tuning);
        self.context = context;
        if self.tuning.direct_look {
            touch_look::clear();
        }
    }

    /// Whether the left-stick click is down for the game.
    pub fn crouch_down(&self) -> bool {
        match self.tuning.crouch {
            CrouchMode::Toggle => self.crouched,
            CrouchMode::Hold => self.held & bits::LEFT_THUMB != 0,
        }
    }

    /// The pad the game should see at `now`. Nothing is consumed: [`read`]
    /// spends the look motion this state carried.
    ///
    /// [`read`]: Self::read
    pub fn state(&mut self, now: Instant) -> HostPadState {
        let t = self.tuning;
        let mut held = self.held & !bits::LEFT_THUMB;
        if self.crouch_down() {
            held |= bits::LEFT_THUMB;
        }
        let look = self.look.stick(now, LOOK_K, t.invert_y);
        let look = curve(look, t.look_curve);
        let gyro = gyro_stick(self.gyro, t.gyro_sensitivity);
        let (mut rx, mut ry) = (look.0 + gyro.0, look.1 + gyro.1);
        let mag = rx.hypot(ry);
        if mag > 1.0 {
            rx /= mag;
            ry /= mag;
        }
        let pull = |bit: u32| if held & bit != 0 { 1.0 } else { 0.0 };
        let auto = self.auto_fire.pulled(self.fire_mode());
        HostPadState {
            buttons: buttons(held),
            left_stick: self.move_stick,
            right_stick: (rx as f32, ry as f32),
            left_trigger: pull(bits::LEFT_TRIGGER),
            right_trigger: if auto { 1.0 } else { pull(bits::RIGHT_TRIGGER) },
            analog_buttons: None,
            prefiltered: true,
        }
    }

    /// The guest read the newest state: spend its look motion.
    pub fn read(&mut self, now: Instant) {
        self.look.read(now);
        let mode = self.fire_mode();
        self.auto_fire.read(mode);
    }

    /// Whether anything is held or moving (the overlay is in use).
    pub fn active(&self) -> bool {
        self.held != 0
            || self.crouched
            || self.move_stick != (0.0, 0.0)
            || self.gyro != (0.0, 0.0)
            || self.look.pending() != (0.0, 0.0)
            || self.last_turn != (0.0, 0.0)
    }
}

/// A one-line summary of a touch pad state for the device log, coarse
/// enough that it changes only on a real change: held buttons by name and
/// each stick in steps of a quarter.
pub fn summary(s: &HostPadState) -> String {
    use HostButton::*;
    let mut out = Vec::new();
    for (hb, name) in [
        (South, "A"),
        (East, "B"),
        (West, "X"),
        (North, "Y"),
        (LeftBumper, "White"),
        (RightBumper, "Black"),
        (Start, "Start"),
        (Back, "Back"),
        (LeftStick, "LS"),
        (RightStick, "RS"),
        (DpadUp, "Up"),
        (DpadDown, "Down"),
        (DpadLeft, "Left"),
        (DpadRight, "Right"),
    ] {
        if s.buttons.contains(hb) {
            out.push(name.to_owned());
        }
    }
    if s.left_trigger > 0.0 {
        out.push("LT".into());
    }
    if s.right_trigger > 0.0 {
        out.push("RT".into());
    }
    let q = |v: f32| (v * 4.0).round() / 4.0 + 0.0;
    let stick = |name: &str, (x, y): (f32, f32)| {
        let (x, y) = (q(x), q(y));
        (x != 0.0 || y != 0.0).then(|| format!("{name}({x:+.2},{y:+.2})"))
    };
    out.extend(stick("move", s.left_stick));
    out.extend(stick("look", s.right_stick));
    if out.is_empty() {
        "idle".into()
    } else {
        out.join(" ")
    }
}

/// Logs touch pad changes at a low rate: a new [`summary`] is reported at
/// once when the held buttons change, and at most every [`LOG_EVERY`] when
/// only the sticks moved.
#[derive(Clone, Debug, Default)]
pub struct ChangeLog {
    last: String,
    last_buttons: Option<(HostButtons, bool, bool)>,
    at: Option<Instant>,
}

/// The shortest gap between two stick-only log lines.
pub const LOG_EVERY: Duration = Duration::from_millis(250);

impl ChangeLog {
    /// The line to log for `s` at `now`, if any.
    pub fn line(&mut self, s: &HostPadState, now: Instant) -> Option<String> {
        let text = summary(s);
        if text == self.last {
            return None;
        }
        let buttons = (s.buttons, s.left_trigger > 0.0, s.right_trigger > 0.0);
        let buttons_changed = self.last_buttons != Some(buttons);
        let due = self
            .at
            .is_none_or(|t| now.saturating_duration_since(t) >= LOG_EVERY);
        if !buttons_changed && !due {
            return None;
        }
        self.last = text.clone();
        self.last_buttons = Some(buttons);
        self.at = Some(now);
        Some(text)
    }
}

#[cfg(test)]
#[path = "touch_tests.rs"]
mod tests;
