//! Keyboard and mouse as a virtual Xbox pad.
//!
//! This backend has no window of its own: the platform layer forwards key and
//! mouse events (`key_down("KeyW")`, `mouse_motion(dx, dy)`, ...) and the
//! backend turns the held set into a [`HostPadState`] using [`Bindings`].
//!
//! * Keys are digital: buttons report 255, stick keys full deflection
//!   (diagonals are normalised to the unit circle), trigger keys 1.0.
//! * Mouse motion drives the right stick when `mouse.look` is on. Motion is
//!   held until the guest has read a state carrying it
//!   ([`HostBackend::state_read`]), not until the next `sync`: the manager
//!   syncs far more often than the game polls, and a per-sync reset left the
//!   game seeing a centred stick on almost every read. The deflection is the
//!   mouse *speed* over the guest's read interval, `delta * sensitivity *
//!   (1/60 s) / interval` (the interval counts as at least 1/60 s), so at a
//!   60 Hz read rate it is `delta * sensitivity` as before, and since Halo
//!   treats the stick as a turn rate, turning per mouse count does not grow
//!   or shrink with the game's frame rate. Motion left unread for over
//!   [`STALE_MOTION`] is dropped instead of arriving later as a flick.
//! * The virtual pad is not "physical": no deadzone is applied, so tiny mouse
//!   deltas still register.
//! * Rumble is off by default (`XInputSetState` returns 50, like a pad without
//!   motors). [`KeyboardMouse::set_rumble_supported`] makes the pad advertise both
//!   motors and accept feedback like a standard controller; requests then
//!   complete and the newest motor speeds are readable from
//!   [`KeyboardMouse::rumble`]. Set it before the manager's first `sync`: a
//!   pad's capabilities are fixed when it is inserted.
//! * On-screen touch controls ([`crate::touch`], the iOS app) are merged in
//!   as one more pad: [`KeyboardMouse::touch_update`]. Their buttons add to
//!   the keys, the triggers take the larger pull, the move stick the larger
//!   deflection, and their look adds to the mouse's. Using them marks the
//!   last-used device as a pad, so the game shows pad prompts.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use crate::backend::HostBackend;
use crate::bindings::{Bindings, Control, Input, MouseButton, canonical_key};
use crate::gamepad::Rumble;
use crate::host::{HostButton, HostButtons, HostPad, HostPadId, HostPadState};

use crate::look::LookMotion;
pub use crate::look::{LOOK_FRAME, STALE_MOTION};
use crate::touch::{TouchFrame, TouchPad, TouchTuning};

/// Where the pad gets the time (wall clock by default; tests inject one).
pub type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Cloneable handle to one virtual keyboard+mouse pad. Clone it before
/// boxing the backend into [`InputManager::add_backend`](crate::InputManager::add_backend):
/// the platform layer keeps one clone to feed events, the manager owns the
/// other, and both see the same state.
#[derive(Clone)]
pub struct KeyboardMouse {
    inner: Arc<Mutex<Inner>>,
    clock: Clock,
    /// Last-used device of the pad this keyboard is part of.
    active: crate::active::ActiveDevice,
}

/// Committed host edits; game-specific validation stays on the guest thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextAction {
    Insert(String),
    Backspace,
    Accept,
    Cancel,
}
const MAX_TEXT_EVENTS: usize = 64;
const MAX_COMMIT: usize = 150;
#[derive(Default)]
struct TextState {
    target: Option<u64>,
    events: VecDeque<TextAction>,
    held: HashSet<String>,
    blocked: HashSet<String>,
}
impl TextState {
    fn insert(&mut self, action: TextAction) {
        if self.events.len() >= MAX_TEXT_EVENTS {
            return;
        }
        let action = match action {
            TextAction::Insert(s) => {
                // Xbox UI text is BMP, not surrogate pairs. Controls are not edits.
                let s: String = s
                    .chars()
                    .filter(|c| !c.is_control() && u32::from(*c) <= 0xffff)
                    .take(MAX_COMMIT)
                    .collect();
                if s.is_empty() {
                    return;
                }
                TextAction::Insert(s)
            }
            other => other,
        };
        self.events.push_back(action);
    }
}
struct Inner {
    bindings: Bindings,
    text: TextState,
    activity: u64,
    keys: HashSet<String>,
    mouse: HashSet<MouseButton>,
    /// Mouse motion the guest has not read yet (`crate::look`).
    look: LookMotion,
    enabled: bool,
    rumble_supported: bool,
    rumble: Rumble,
    /// On-screen touch controls (`crate::touch`).
    touch: TouchPad,
}

impl Inner {
    fn release_all(&mut self) {
        self.keys.clear();
        self.text.events.clear();
        self.text.held.clear();
        self.text.blocked.clear();
        self.mouse.clear();
        self.drop_motion();
        self.touch.release();
    }

    fn drop_motion(&mut self) {
        self.look.clear();
    }

    fn held(&self, c: Control) -> bool {
        self.bindings.inputs(c).iter().any(|i| match i {
            Input::Key(k) => self.keys.contains(k),
            Input::Mouse(m) => self.mouse.contains(m),
        })
    }
}

impl KeyboardMouse {
    pub fn new(bindings: Bindings) -> Self {
        Self::with_clock(bindings, Arc::new(Instant::now))
    }

    /// Like [`new`](Self::new), timing mouse look with `clock`.
    pub fn with_clock(bindings: Bindings, clock: Clock) -> Self {
        KeyboardMouse {
            inner: Arc::new(Mutex::new(Inner {
                bindings,
                text: TextState::default(),
                activity: 0,
                keys: HashSet::new(),
                mouse: HashSet::new(),
                look: LookMotion::default(),
                enabled: true,
                rumble_supported: false,
                rumble: Rumble::default(),
                touch: TouchPad::default(),
            })),
            clock,
            active: Default::default(),
        }
    }

    /// The last-used device of the pad this keyboard belongs to. The combined
    /// player-one pad (`KeyboardFallback`) tracks the physical pad; the
    /// keyboard itself records fresh bound key and mouse events.
    pub fn active_device(&self) -> crate::active::ActiveDevice {
        self.active.clone()
    }

    pub(crate) fn activity_serial(&self) -> u64 {
        self.lock().activity
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn bindings(&self) -> Bindings {
        self.lock().bindings.clone()
    }

    /// Show or hide the virtual pad (it occupies a port while enabled).
    pub fn set_enabled(&self, on: bool) {
        let mut i = self.lock();
        i.enabled = on;
        if !on {
            i.release_all();
        }
    }

    /// Advertise motors and accept rumble feedback (see the module docs).
    /// Takes effect for the next insertion of the pad.
    pub fn set_rumble_supported(&self, supported: bool) {
        self.lock().rumble_supported = supported;
    }

    /// Newest motor speeds the guest asked for (zero until then).
    pub fn rumble(&self) -> Rumble {
        self.lock().rumble
    }

    /// Publish the currently focused guest text field. A changed target drops
    /// stale edits and blocks held keys until release (including the opener).
    pub fn set_text_target(&self, target: Option<u64>) {
        let mut i = self.lock();
        if i.text.target == target {
            return;
        }
        let mut keys: Vec<_> = i.keys.drain().collect();
        keys.extend(i.text.held.drain());
        i.text.blocked.extend(keys);
        i.text.events.clear();
        i.text.target = target;
    }
    pub fn text_active(&self) -> bool {
        self.lock().text.target.is_some()
    }
    pub fn take_text(&self, target: u64) -> Vec<TextAction> {
        let mut i = self.lock();
        if i.text.target != Some(target) {
            return Vec::new();
        }
        i.text.events.drain(..).collect()
    }
    /// Route one host key before pad mapping. Arrows and other non-text keys
    /// pass through. Releases and repeats of captured keys never leak into pad.
    pub fn text_key(
        &self,
        name: Option<&str>,
        down: bool,
        candidate: bool,
        action: Option<TextAction>,
    ) -> bool {
        let key = name.and_then(canonical_key);
        let mut i = self.lock();
        if !down {
            return key.is_some_and(|key| {
                let captured = i.text.held.remove(&key) | i.text.blocked.remove(&key);
                i.keys.remove(&key);
                captured
            });
        }
        if key.as_ref().is_some_and(|k| i.text.blocked.contains(k)) {
            return true;
        }
        if i.text.target.is_none() || (!candidate && action.is_none()) {
            return false;
        }
        if let Some(key) = key {
            i.keys.remove(&key);
            i.text.held.insert(key);
        }
        if let Some(action) = action {
            i.text.insert(action);
        }
        i.activity = i.activity.wrapping_add(1);
        self.active.set(crate::active::DeviceKind::Keyboard);
        true
    }
    /// OS/IME committed text, never preedit/composition text.
    pub fn commit_text(&self, text: &str) -> bool {
        let mut i = self.lock();
        if i.text.target.is_none() {
            return false;
        }
        i.text.insert(TextAction::Insert(text.into()));
        i.activity = i.activity.wrapping_add(1);
        self.active.set(crate::active::DeviceKind::Keyboard);
        true
    }

    /// Key pressed. Returns false if the name is not a known key.
    pub fn key_down(&self, name: &str) -> bool {
        let Some(key) = canonical_key(name) else {
            return false;
        };
        let mut i = self.lock();
        let bound = i
            .bindings
            .bind
            .values()
            .any(|inputs| inputs.contains(&Input::Key(key.clone())));
        if i.keys.insert(key) && i.enabled && bound {
            i.activity = i.activity.wrapping_add(1);
            self.active.set(crate::active::DeviceKind::Keyboard);
        }
        true
    }

    pub fn key_up(&self, name: &str) -> bool {
        canonical_key(name)
            .map(|k| {
                let mut i = self.lock();
                i.text.held.remove(&k);
                i.text.blocked.remove(&k);
                i.keys.remove(&k)
            })
            .is_some()
    }

    /// Holds or releases `control` as if its first bound key were pressed,
    /// for a host's own on-screen buttons (iOS's Start and Back, which the
    /// system's touch controls cannot provide). False when no key is bound
    /// to it.
    pub fn press_control(&self, control: Control, down: bool) -> bool {
        let key = self.lock().bindings.bind.get(&control).and_then(|inputs| {
            inputs.iter().find_map(|i| match i {
                Input::Key(k) => Some(k.clone()),
                Input::Mouse(_) => None,
            })
        });
        match key {
            Some(k) if down => self.key_down(&k),
            Some(k) => self.key_up(&k),
            None => false,
        }
    }

    pub fn mouse_button(&self, b: MouseButton, down: bool) {
        let mut i = self.lock();
        if down {
            let bound = i
                .bindings
                .bind
                .values()
                .any(|inputs| inputs.contains(&Input::Mouse(b)));
            if i.mouse.insert(b) && i.enabled && bound {
                i.activity = i.activity.wrapping_add(1);
                self.active.set(crate::active::DeviceKind::Keyboard);
            }
        } else {
            i.mouse.remove(&b);
        }
    }

    /// Relative mouse motion in counts (screen space: +y is down). Non-finite
    /// values are ignored. It stays on the right stick until the guest reads it.
    pub fn mouse_motion(&self, dx: f64, dy: f64) {
        if dx.is_finite() && dy.is_finite() {
            let now = (self.clock)();
            let mut i = self.lock();
            if i.enabled
                && i.bindings.mouse.look
                && i.bindings.mouse.sensitivity > 0.0
                && (dx != 0.0 || dy != 0.0)
            {
                i.activity = i.activity.wrapping_add(1);
                self.active.set(crate::active::DeviceKind::Keyboard);
            }
            i.look.add(now, dx, dy);
        }
    }

    /// One report from the on-screen touch controls ([`crate::touch`]).
    /// Returns the touch pad's own state as it stands, for the host's log
    /// (reading it spends nothing the guest has not seen). Ignored while the
    /// pad is disabled.
    pub fn touch_update(&self, frame: &TouchFrame) -> HostPadState {
        let now = (self.clock)();
        let mut i = self.lock();
        if !i.enabled {
            return HostPadState::default();
        }
        i.touch.update(frame, now);
        if i.touch.active() {
            // Touch controls are a pad: the game shows pad prompts.
            self.active.set(crate::active::DeviceKind::Xbox);
        }
        i.touch.clone().state(now)
    }

    /// Player one's state for the touch controls' auto-fire
    /// (`crate::touch_layout`).
    pub fn set_touch_context(&self, context: crate::touch_layout::Context) {
        self.lock().touch.set_context(context);
    }

    /// The touch controls' tuning (from the settings).
    pub fn set_touch_tuning(&self, tuning: TouchTuning) {
        self.lock().touch.set_tuning(tuning);
    }

    pub fn touch_tuning(&self) -> TouchTuning {
        self.lock().touch.tuning()
    }

    /// Whether the touch controls hold crouch down (a toggled crouch
    /// included), for the host to light its crouch button.
    pub fn touch_crouched(&self) -> bool {
        self.lock().touch.crouch_down()
    }

    /// The touch controls went away (a pad connected, the keyboard came
    /// up, the app left the screen): let go of everything they held.
    pub fn touch_release(&self) {
        self.lock().touch.release();
    }

    /// Focus lost: release everything.
    pub fn release_all(&self) {
        self.lock().release_all();
    }

    /// Build the pad state for the current held set and the mouse motion the
    /// guest has not read yet. Nothing is consumed: [`HostBackend::state_read`]
    /// spends the motion this state carried.
    pub fn state(&self) -> HostPadState {
        let now = (self.clock)();
        let mut i = self.lock();
        use Control::*;
        let mut s = HostPadState::default();
        let buttons = [
            (A, HostButton::South),
            (B, HostButton::East),
            (X, HostButton::West),
            (Y, HostButton::North),
            (Black, HostButton::RightBumper),
            (White, HostButton::LeftBumper),
            (DpadUp, HostButton::DpadUp),
            (DpadDown, HostButton::DpadDown),
            (DpadLeft, HostButton::DpadLeft),
            (DpadRight, HostButton::DpadRight),
            (Start, HostButton::Start),
            (Back, HostButton::Back),
            (LeftThumb, HostButton::LeftStick),
            (RightThumb, HostButton::RightStick),
        ];
        for (c, hb) in buttons {
            s.buttons.set(hb, i.held(c));
        }
        let axis = |neg: bool, pos: bool| f32::from(u8::from(pos)) - f32::from(u8::from(neg));
        s.left_trigger = f32::from(u8::from(i.held(LeftTrigger)));
        s.right_trigger = f32::from(u8::from(i.held(RightTrigger)));
        s.left_stick = (
            axis(i.held(LeftStickLeft), i.held(LeftStickRight)),
            axis(i.held(LeftStickDown), i.held(LeftStickUp)),
        );
        s.right_stick = (
            axis(i.held(RightStickLeft), i.held(RightStickRight)),
            axis(i.held(RightStickDown), i.held(RightStickUp)),
        );
        let m = i.bindings.mouse;
        // Mouse up (dy < 0) looks up. Unclamped here: the vector is scaled
        // back to the unit circle below, which keeps a fast flick's direction.
        let (lx, ly) = i.look.stick(now, f64::from(m.sensitivity), m.invert_y);
        if m.look {
            s.right_stick.0 = (f64::from(s.right_stick.0) + lx) as f32;
            s.right_stick.1 = (f64::from(s.right_stick.1) + ly) as f32;
        }
        let t = i.touch.state(now);
        s.buttons = HostButtons::from_bits(s.buttons.bits() | t.buttons.bits());
        s.left_trigger = s.left_trigger.max(t.left_trigger);
        s.right_trigger = s.right_trigger.max(t.right_trigger);
        let strength = |(x, y): (f32, f32)| x.hypot(y);
        if strength(t.left_stick) > strength(s.left_stick) {
            s.left_stick = t.left_stick;
        }
        s.right_stick.0 += t.right_stick.0;
        s.right_stick.1 += t.right_stick.1;
        // Synthesised directions have no physical gate. Normalise here so
        // calibrated hardware and scripted reports can retain independent axes.
        for stick in [&mut s.left_stick, &mut s.right_stick] {
            let magnitude = stick.0.hypot(stick.1);
            if magnitude > 1.0 {
                stick.0 /= magnitude;
                stick.1 /= magnitude;
            }
        }
        s
    }
}

impl HostBackend for KeyboardMouse {
    fn name(&self) -> &str {
        "keyboard+mouse"
    }

    fn poll(&mut self) -> Vec<HostPad> {
        if !self.lock().enabled {
            return Vec::new();
        }
        vec![HostPad {
            id: HostPadId(0),
            name: "Keyboard + Mouse".into(),
            state: self.state(),
            physical: false,
            supports_rumble: self.lock().rumble_supported,
        }]
    }

    /// The guest saw the newest state: spend the motion it carried and start
    /// the next read interval.
    fn state_read(&mut self, _id: HostPadId) {
        let now = (self.clock)();
        let mut i = self.lock();
        i.look.read(now);
        i.touch.read(now);
    }

    fn set_rumble(&mut self, _id: HostPadId, rumble: Rumble) {
        self.lock().rumble = rumble;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gamepad::{AnalogButton, Buttons};
    use crate::host::ConvertConfig;

    fn gp(k: &KeyboardMouse) -> crate::gamepad::Gamepad {
        k.state().to_gamepad(&ConvertConfig::default(), false)
    }

    #[test]
    fn keys_drive_buttons_and_sticks() {
        let k = KeyboardMouse::new(Bindings::default());
        k.key_down("Space");
        k.key_down("KeyW");
        k.key_down("D");
        k.mouse_button(MouseButton::Left, true);
        let g = gp(&k);
        assert_eq!(g.analog(AnalogButton::A), 255);
        assert_eq!(g.analog(AnalogButton::RightTrigger), 255);
        // W+D diagonal is normalised, not sqrt(2) long.
        let m = f64::from(g.thumb_lx).hypot(f64::from(g.thumb_ly));
        assert!((m - 32767.0).abs() < 2.0, "{m}");
        assert!(g.thumb_ly > 0 && g.thumb_lx > 0);
        k.key_up("space");
        assert_eq!(gp(&k).analog(AnalogButton::A), 0);
    }

    #[test]
    fn press_control_holds_the_bound_key() {
        use crate::gamepad::Buttons;
        let k = KeyboardMouse::new(Bindings::default());
        assert!(k.press_control(Control::Start, true));
        assert!(gp(&k).buttons.0 & Buttons::START.0 != 0);
        assert!(k.press_control(Control::Back, true));
        assert!(gp(&k).buttons.0 & Buttons::BACK.0 != 0);
        assert!(k.press_control(Control::Start, false));
        assert!(k.press_control(Control::Back, false));
        assert_eq!(gp(&k).buttons.0 & (Buttons::START.0 | Buttons::BACK.0), 0);
        let mut none = Bindings::default();
        none.bind.remove(&Control::Start);
        assert!(!KeyboardMouse::new(none).press_control(Control::Start, true));
    }

    #[test]
    fn opposite_keys_cancel() {
        let k = KeyboardMouse::new(Bindings::default());
        k.key_down("a");
        k.key_down("d");
        assert_eq!(gp(&k).thumb_lx, 0);
    }

    /// A pad on a hand-wound clock, and the hand.
    fn clocked(bindings: Bindings) -> (KeyboardMouse, Arc<Mutex<Instant>>) {
        let now = Arc::new(Mutex::new(Instant::now()));
        let hand = Arc::clone(&now);
        let k = KeyboardMouse::with_clock(bindings, Arc::new(move || *hand.lock().unwrap()));
        (k, now)
    }

    #[test]
    fn mouse_motion_is_held_until_read_and_signed() {
        let (mut k, _) = clocked(Bindings::default());
        k.mouse_motion(5.0, -10.0); // right, up
        let g = gp(&k);
        assert_eq!(g.thumb_rx, (5.0f32 * 0.04 * 32767.0).round() as i16);
        assert_eq!(g.thumb_ry, (10.0f32 * 0.04 * 32767.0).round() as i16);
        assert_eq!(gp(&k), g, "unread motion stays on the stick");
        k.state_read(HostPadId(0));
        let g = gp(&k);
        assert_eq!((g.thumb_rx, g.thumb_ry), (0, 0), "read motion is spent");
        k.mouse_motion(1000.0, 0.0);
        assert_eq!(gp(&k).thumb_rx, 32767);
    }

    #[test]
    fn motion_after_the_sample_survives_the_read() {
        let (mut k, _) = clocked(Bindings::default());
        k.mouse_motion(5.0, 0.0);
        let _ = k.state(); // what the guest is about to read
        k.mouse_motion(3.0, 0.0); // arrives before the read is reported
        k.state_read(HostPadId(0));
        assert_eq!(gp(&k).thumb_rx, (3.0f32 * 0.04 * 32767.0).round() as i16);
    }

    #[test]
    fn deflection_is_mouse_speed_over_the_read_interval() {
        let (mut k, now) = clocked(Bindings::default());
        k.state_read(HostPadId(0)); // a guest read starts the interval
        // 30 counts over 10 look frames: 3 counts per frame.
        k.mouse_motion(30.0, 0.0);
        *now.lock().unwrap() += LOOK_FRAME * 10;
        let rx = gp(&k).thumb_rx;
        assert_eq!(rx, (3.0f32 * 0.04 * 32767.0).round() as i16);
    }

    #[test]
    fn a_fast_flick_keeps_its_direction() {
        let (k, _) = clocked(Bindings::default());
        k.mouse_motion(300.0, -100.0);
        let g = gp(&k);
        let ratio = f64::from(g.thumb_rx) / f64::from(g.thumb_ry);
        assert!((ratio - 3.0).abs() < 0.01, "{ratio}");
    }

    #[test]
    fn motion_the_guest_never_reads_goes_stale() {
        let (k, now) = clocked(Bindings::default());
        k.mouse_motion(10.0, 0.0);
        *now.lock().unwrap() += STALE_MOTION + LOOK_FRAME;
        k.mouse_motion(1.0, 0.0);
        assert_eq!(gp(&k).thumb_rx, (0.04f32 * 32767.0).round() as i16);
    }

    #[test]
    fn invert_y_and_disabled() {
        let b = Bindings::from_toml("[mouse]\ninvert_y = true").unwrap();
        let mut k = KeyboardMouse::new(b);
        k.mouse_motion(0.0, -10.0);
        assert!(gp(&k).thumb_ry < 0);
        k.key_down("Escape");
        assert!(gp(&k).buttons.contains(Buttons::START));
        k.set_enabled(false);
        assert!(k.poll().is_empty());
        k.set_enabled(true);
        assert!(
            !gp(&k).buttons.contains(Buttons::START),
            "released on disable"
        );
    }

    #[test]
    fn unknown_key_names_are_reported() {
        let k = KeyboardMouse::new(Bindings::default());
        assert!(!k.key_down("Nonsense"));
        assert!(k.key_down("KeyQ"));
    }
}

#[cfg(test)]
mod touch_tests {
    use super::*;
    use crate::backend::fallback::KeyboardFallback;
    use crate::gamepad::{AnalogButton, Buttons};
    use crate::host::ConvertConfig;
    use crate::touch::{CrouchMode, TouchFrame, bits};

    fn gp(k: &KeyboardMouse) -> crate::gamepad::Gamepad {
        k.state().to_gamepad(&ConvertConfig::default(), false)
    }

    #[test]
    fn touch_fire_reaches_player_one_as_a_full_right_trigger() {
        let k = KeyboardMouse::new(Bindings::default());
        let mut p1 = KeyboardFallback::new(k.clone(), None, ConvertConfig::default());
        k.touch_update(&TouchFrame {
            buttons: bits::RIGHT_TRIGGER,
            ..Default::default()
        });
        let pads = p1.poll();
        assert_eq!(pads.len(), 1);
        let g = pads[0]
            .state
            .to_gamepad(&ConvertConfig::default(), pads[0].physical);
        assert_eq!(g.analog[AnalogButton::RightTrigger.index()], 255);
        assert_eq!(
            k.active_device().get(),
            Some(crate::active::DeviceKind::Xbox),
            "pad prompts"
        );
        k.touch_update(&TouchFrame::default());
        let g = p1.poll()[0]
            .state
            .to_gamepad(&ConvertConfig::default(), false);
        assert_eq!(g.analog[AnalogButton::RightTrigger.index()], 0);
    }

    #[test]
    fn touch_merges_with_keys_and_mouse() {
        let k = KeyboardMouse::new(Bindings::default());
        k.key_down("Space"); // A
        k.touch_update(&TouchFrame {
            buttons: bits::LEFT_THUMB | bits::RIGHT_THUMB | bits::START,
            move_offset: Some((0.0, -500.0)),
            ..Default::default()
        });
        let g = gp(&k);
        assert_eq!(
            g.analog[AnalogButton::A.index()],
            255,
            "the key still counts"
        );
        assert!(g.buttons.contains(Buttons::LEFT_THUMB), "crouch");
        assert!(g.buttons.contains(Buttons::RIGHT_THUMB), "zoom");
        assert!(g.buttons.contains(Buttons::START));
        assert_eq!(g.thumb_ly, 32767, "the touch stick is the stronger one");
        // Crouch is a toggle: it stays down after the tap.
        k.touch_update(&TouchFrame::default());
        assert!(gp(&k).buttons.contains(Buttons::LEFT_THUMB));
        assert!(k.touch_crouched());
        k.touch_release();
        assert!(!k.touch_crouched());
        assert!(
            !gp(&k).buttons.contains(Buttons::LEFT_THUMB),
            "hiding stands up"
        );
    }

    #[test]
    fn touch_look_is_held_until_the_guest_reads_it() {
        let k = KeyboardMouse::new(Bindings::default());
        let mut backend = k.clone();
        k.touch_update(&TouchFrame {
            look: (3.0, 0.0),
            ..Default::default()
        });
        let g = gp(&k);
        assert!(g.thumb_rx > 0);
        assert_eq!(gp(&k), g);
        backend.state_read(HostPadId(0));
        assert_eq!(gp(&k).thumb_rx, 0);
    }

    #[test]
    fn touch_tuning_is_kept_and_a_disabled_pad_ignores_touch() {
        let k = KeyboardMouse::new(Bindings::default());
        let t = TouchTuning {
            crouch: CrouchMode::Hold,
            ..TouchTuning::default()
        };
        k.set_touch_tuning(t);
        assert_eq!(k.touch_tuning(), t);
        k.set_enabled(false);
        k.touch_update(&TouchFrame {
            buttons: bits::A,
            ..Default::default()
        });
        k.set_enabled(true);
        assert_eq!(gp(&k).analog[AnalogButton::A.index()], 0);
    }
}

#[cfg(test)]
mod text_tests {
    use super::*;

    #[test]
    fn text_capture_drops_stale_edits_and_holds_keys_out_of_pad_until_release() {
        let kb = KeyboardMouse::new(Bindings::default());
        kb.key_down("Space");
        kb.set_text_target(Some(1));
        assert!(kb.text_key(
            Some("Space"),
            true,
            true,
            Some(TextAction::Insert(" ".into()))
        ));
        assert!(kb.take_text(1).is_empty(), "opening held key cannot type");
        kb.text_key(Some("Space"), false, true, None);
        assert!(kb.text_key(
            Some("KeyW"),
            true,
            true,
            Some(TextAction::Insert("É".into()))
        ));
        assert!(kb.text_key(
            Some("Space"),
            true,
            true,
            Some(TextAction::Insert(" ".into()))
        ));
        assert_eq!(
            kb.take_text(1),
            vec![
                TextAction::Insert("É".into()),
                TextAction::Insert(" ".into())
            ]
        );
        assert!(kb.lock().keys.is_empty());
        kb.commit_text("stale");
        kb.set_text_target(Some(2));
        assert!(kb.take_text(1).is_empty());
        assert!(kb.take_text(2).is_empty());
        kb.set_text_target(None);
        assert!(kb.text_key(
            Some("KeyW"),
            true,
            true,
            Some(TextAction::Insert("w".into()))
        ));
        assert!(
            kb.lock().keys.is_empty(),
            "repeat cannot turn into a pad after close"
        );
        kb.text_key(Some("KeyW"), false, true, None);
        assert!(!kb.text_key(
            Some("KeyW"),
            true,
            true,
            Some(TextAction::Insert("w".into()))
        ));
        kb.key_down("KeyW");
        assert!(kb.lock().keys.contains("w"));
    }
    #[test]
    fn commits_are_bmp_printable_bounded_and_focus_loss_clears_pending_work() {
        let kb = KeyboardMouse::new(Bindings::default());
        assert!(!kb.commit_text("outside"));
        kb.set_text_target(Some(7));
        assert!(kb.commit_text("Å中😀\n\t"));
        assert_eq!(kb.take_text(7), vec![TextAction::Insert("Å中".into())]);
        kb.commit_text(&"x".repeat(300));
        assert_eq!(
            kb.take_text(7),
            vec![TextAction::Insert("x".repeat(MAX_COMMIT))]
        );
        for _ in 0..1000 {
            kb.commit_text("x");
        }
        assert_eq!(kb.take_text(7).len(), MAX_TEXT_EVENTS);
        kb.commit_text("discard");
        kb.release_all();
        assert!(kb.take_text(7).is_empty());
        assert!(kb.text_active());
    }
}
