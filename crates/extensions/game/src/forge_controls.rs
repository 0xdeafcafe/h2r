//! Desktop and pad input to the local Forge tick. The bridge carries host
//! presentation data only. See docs/game/forge-design.md, “The UI”.
use forge::{
    camera::Camera,
    tool::{Action, Input, TURN_RATE},
};
use input::{
    GatedPad, PadGate,
    forge::Key,
    host::{HostButton, HostButtons, HostPadState},
};
use std::{
    collections::VecDeque,
    sync::{LazyLock, Mutex, MutexGuard},
    time::{Duration, Instant},
};

mod render;
pub use render::{camera_hooks, forget_park, parked};

#[derive(Clone, Copy)]
enum Event {
    Action(Action),
    Click(usize),
    Agent(Input),
}

#[derive(Default)]
struct Bridge {
    active: bool,
    monitor: bool,
    dialog: bool,
    camera: Option<Camera>,
    eye: Option<Camera>,
    keys: [bool; 6],
    mouse: [f32; 2],
    right_mouse: bool,
    pad: HostPadState,
    events: VecDeque<Event>,
    sampled: Option<Instant>,
}

static BRIDGE: LazyLock<Mutex<Bridge>> = LazyLock::new(Mutex::default);
fn lock() -> MutexGuard<'static, Bridge> {
    BRIDGE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn publish(active: bool, monitor: bool, dialog: bool, camera: Option<Camera>) {
    let mut b = lock();
    if !active {
        *b = Bridge::default();
        return;
    }
    if !b.active || b.monitor != monitor || b.dialog != dialog {
        b.keys = [false; 6];
        b.mouse = [0.; 2];
        b.right_mouse = false;
        b.sampled = None;
    }
    b.active = active;
    b.monitor = monitor;
    b.dialog = dialog;
    b.camera = camera;
}

pub fn eye() -> Option<Camera> {
    lock().eye
}

impl Bridge {
    fn push(&mut self, event: Event) {
        if self.events.len() < 64 {
            self.events.push_back(event);
        }
    }

    fn key(&mut self, key: Key, down: bool, repeat: bool) -> bool {
        if !self.active || !self.monitor && key != Key::Toggle {
            return false;
        }
        match key {
            Key::Forward
            | Key::Backward
            | Key::StrafeLeft
            | Key::StrafeRight
            | Key::Rise
            | Key::Fall => {
                let i = match key {
                    Key::Forward => 0,
                    Key::Backward => 1,
                    Key::StrafeLeft => 2,
                    Key::StrafeRight => 3,
                    Key::Rise => 4,
                    _ => 5,
                };
                self.keys[i] = down && !self.dialog;
            }
            _ if down && !repeat || down && self.dialog && matches!(key, Key::Up | Key::Down) => {
                let action = match key {
                    Key::Toggle => Action::Toggle,
                    Key::Accept => Action::Accept,
                    Key::Back => Action::Back,
                    Key::Delete => Action::Delete,
                    Key::Palette => Action::Palette,
                    Key::Properties => Action::Properties,
                    Key::Undo => Action::Undo,
                    Key::Redo => Action::Redo,
                    Key::Up => Action::Up,
                    Key::Down => Action::Down,
                    Key::Link => Action::Link,
                    _ => unreachable!(),
                };
                self.push(Event::Action(action));
            }
            _ => {}
        }
        true
    }
}

pub fn key(key: Key, down: bool, repeat: bool) -> bool {
    lock().key(key, down, repeat)
}
pub fn release_keys() {
    let mut b = lock();
    b.keys = [false; 6];
    b.mouse = [0.; 2];
    b.right_mouse = false;
}
pub fn mouse_look(dx: f32, dy: f32) -> bool {
    let mut b = lock();
    if !b.active || !b.monitor || b.dialog {
        return false;
    }
    if dx.is_finite() && dy.is_finite() {
        b.mouse[0] -= dx * 0.003;
        b.mouse[1] -= dy * 0.003;
    }
    true
}
pub fn mouse_button(left: bool, down: bool) -> bool {
    let mut b = lock();
    if !b.active || !b.monitor {
        return false;
    }
    if !b.dialog {
        if left {
            if down {
                b.push(Event::Action(Action::Accept));
            }
        } else {
            b.right_mouse = down;
        }
    }
    true
}
pub fn click(row: usize) {
    let mut b = lock();
    if b.active && b.dialog {
        b.push(Event::Click(row));
    }
}

/// The agent queues exactly one sample. The driver checks the loaded map and
/// local role before admission; resetting the bridge discards outstanding input.
pub fn enqueue(input: Input) -> Result<(), String> {
    let mut b = lock();
    if !b.active || (!b.monitor && input.action != Some(Action::Toggle)) {
        return Err("The local Forge monitor is inactive.".into());
    }
    if b.dialog
        && !matches!(
            input.action,
            Some(Action::Accept | Action::Back | Action::Up | Action::Down)
        )
    {
        return Err("The Forge dialog takes accept, back, up or down.".into());
    }
    if b.events.len() >= 64 {
        return Err("The Forge input queue is full.".into());
    }
    b.events.push_back(Event::Agent(input));
    Ok(())
}

pub fn sample() -> Input {
    let mut b = lock();
    let now = Instant::now();
    let seconds = b.sampled.replace(now).map_or(1. / 30., |prev| {
        now.saturating_duration_since(prev).as_secs_f32().min(0.1)
    });
    // A queued agent tick has no simultaneous physical movement. Keep mouse
    // motion for the next real sample and use the dialog actually on screen.
    let (action, click) = match b.events.pop_front() {
        Some(Event::Agent(input)) => {
            return Input {
                dialog: b.dialog,
                ..input
            };
        }
        Some(Event::Click(i)) => (None, Some(i)),
        Some(Event::Action(a)) => (Some(a), None),
        None => (None, None),
    };
    let mouse = std::mem::take(&mut b.mouse);
    let pad = b.pad;
    let axis = |v: f32| {
        if v.is_finite() && v.abs() > 0.12 {
            v.clamp(-1., 1.)
        } else {
            0.
        }
    };
    let key = |i: usize| u8::from(b.keys[i]) as f32;
    let movement = [
        axis(pad.left_stick.0) + key(3) - key(2),
        axis(pad.left_stick.1) + key(0) - key(1),
        axis(pad.right_trigger) - axis(pad.left_trigger) + key(4) - key(5),
    ];
    Input {
        action,
        click,
        movement,
        dialog: b.dialog,
        seconds,
        rotating: b.right_mouse || pad.buttons.contains(HostButton::RightBumper),
        look: [
            mouse[0] - axis(pad.right_stick.0) * TURN_RATE * seconds,
            mouse[1] + axis(pad.right_stick.1) * TURN_RATE * seconds,
        ],
        roll: movement[0].clamp(-1., 1.) * TURN_RATE * seconds,
    }
}

#[derive(Default)]
struct Gate {
    previous: HostButtons,
    delete: Option<Instant>,
    undo: Option<Instant>,
    navigation: Option<(Action, Instant)>,
}

pub fn gate() -> impl PadGate {
    Gate::default()
}

impl Gate {
    fn held(
        timer: &mut Option<Instant>,
        down: bool,
        was: bool,
        now: Instant,
        short: Action,
        long: Action,
    ) -> Option<Action> {
        if down && !was {
            *timer = Some(now);
        }
        if down
            && timer.is_some_and(|t| now.saturating_duration_since(t) >= Duration::from_millis(450))
        {
            *timer = None;
            return Some(long);
        }
        if !down && was && timer.take().is_some() {
            return Some(short);
        }
        None
    }

    fn controls(&mut self, b: &mut Bridge, pad: HostPadState, now: Instant) {
        use HostButton as H;
        // The desktop merges the first physical pad with keyboard input into
        // an Xbox report. Its A/B/X/Y and bumpers are pressures, even when
        // the physical controller supplied digital buttons.
        let mut pad = pad;
        if let Some(pressures) = pad.analog_buttons {
            for (button, pressure) in [
                H::South,
                H::East,
                H::West,
                H::North,
                H::RightBumper,
                H::LeftBumper,
            ]
            .into_iter()
            .zip(pressures)
            {
                if pressure >= 0.25 {
                    pad.buttons.set(button, true);
                }
            }
        }
        let buttons = pad.buttons;
        let rising = |button| buttons.contains(button) && !self.previous.contains(button);
        if b.dialog {
            for (button, action) in [(H::South, Action::Accept), (H::East, Action::Back)] {
                if rising(button) {
                    b.push(Event::Action(action));
                }
            }
            let up = buttons.contains(H::DpadUp) || pad.left_stick.1 > 0.65;
            let down = buttons.contains(H::DpadDown) || pad.left_stick.1 < -0.65;
            let direction = match (up, down) {
                (true, false) => Some(Action::Up),
                (false, true) => Some(Action::Down),
                _ => None,
            };
            if let Some(direction) = direction {
                if self.navigation.is_none_or(|(last, at)| {
                    last != direction
                        || now.saturating_duration_since(at) >= Duration::from_millis(250)
                }) {
                    b.push(Event::Action(direction));
                    self.navigation = Some((direction, now));
                }
            } else {
                self.navigation = None;
            }
            self.delete = None;
            self.undo = None;
        } else if rising(H::DpadUp) {
            b.push(Event::Action(Action::Toggle));
            self.delete = None;
            self.undo = None;
        } else if b.monitor {
            for (button, action) in [
                (H::South, Action::Accept),
                (H::West, Action::Palette),
                (H::North, Action::Properties),
                (H::Start, Action::Menu),
                (H::DpadRight, Action::Link),
            ] {
                if rising(button) {
                    b.push(Event::Action(action));
                }
            }
            for action in [
                Self::held(
                    &mut self.delete,
                    buttons.contains(H::East),
                    self.previous.contains(H::East),
                    now,
                    Action::Back,
                    Action::Delete,
                ),
                Self::held(
                    &mut self.undo,
                    buttons.contains(H::DpadLeft),
                    self.previous.contains(H::DpadLeft),
                    now,
                    Action::Undo,
                    Action::Redo,
                ),
            ]
            .into_iter()
            .flatten()
            {
                b.push(Event::Action(action));
            }
        }
        if !b.dialog {
            self.navigation = None;
        }
        self.previous = buttons;
        b.pad = pad;
    }
}
impl PadGate for Gate {
    fn filter(&mut self, pads: &mut [GatedPad]) {
        let mut b = lock();
        if !b.active {
            self.previous = pads
                .first()
                .map(|p| p.pad.state.buttons)
                .unwrap_or_default();
            self.delete = None;
            self.undo = None;
            self.navigation = None;
            return;
        }
        // Player one is the first host pad. The desktop fallback merges
        // physical input with the keyboard and reports it as a virtual pad.
        let controller = pads.first_mut();
        let state = controller.as_ref().map(|p| p.pad.state).unwrap_or_default();
        self.controls(&mut b, state, Instant::now());
        if let Some(p) = controller {
            if b.monitor {
                p.pad.state = HostPadState::default();
            } else {
                p.pad.state.buttons.set(HostButton::DpadUp, false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next(b: &mut Bridge) -> Option<Action> {
        match b.events.pop_front() {
            Some(Event::Action(a)) => Some(a),
            _ => None,
        }
    }

    #[test]
    fn key_routes_are_inactive_in_the_player_body_except_toggle() {
        let mut b = Bridge::default();
        assert!(!b.key(Key::Toggle, true, false));
        b.active = true;
        assert!(!b.key(Key::Accept, true, false));
        assert!(b.key(Key::Toggle, true, false));
        assert_eq!(next(&mut b), Some(Action::Toggle));
        b.monitor = true;
        b.key(Key::Palette, true, false);
        b.key(Key::Palette, true, true);
        b.key(Key::Palette, false, false);
        assert_eq!(next(&mut b), Some(Action::Palette));
        assert_eq!(next(&mut b), None);
        b.key(Key::Rise, true, false);
        assert!(b.keys[4]);
        b.key(Key::Rise, false, false);
        assert!(!b.keys[4]);
    }

    #[test]
    fn the_merged_player_one_pad_preserves_face_pressure_and_bumper_rotation() {
        let mut bridge = Bridge {
            active: true,
            monitor: true,
            ..Default::default()
        };
        let mut gate = Gate::default();
        let pad = HostPadState {
            analog_buttons: Some([1., 0., 0., 0., 1., 0.]),
            ..Default::default()
        };
        gate.controls(&mut bridge, pad, Instant::now());
        assert_eq!(next(&mut bridge), Some(Action::Accept));
        assert!(bridge.pad.buttons.contains(HostButton::RightBumper));
        gate.controls(&mut bridge, pad, Instant::now());
        assert_eq!(next(&mut bridge), None);
    }

    #[test]
    fn a_hold_has_a_single_result_on_the_pad_and_a_modal_takes_the_dpad() {
        let mut gate = Gate::default();
        let mut bridge = Bridge {
            active: true,
            monitor: true,
            ..Default::default()
        };
        let mut pad = HostPadState::default();
        let now = Instant::now();
        pad.buttons.set(HostButton::East, true);
        gate.controls(&mut bridge, pad, now);
        gate.controls(&mut bridge, pad, now + Duration::from_secs(1));
        gate.controls(&mut bridge, pad, now + Duration::from_secs(2));
        pad.buttons.set(HostButton::East, false);
        gate.controls(&mut bridge, pad, now + Duration::from_secs(3));
        assert_eq!(next(&mut bridge), Some(Action::Delete));
        assert_eq!(next(&mut bridge), None);
        pad.buttons.set(HostButton::DpadLeft, true);
        gate.controls(&mut bridge, pad, now);
        pad.buttons.set(HostButton::DpadLeft, false);
        gate.controls(&mut bridge, pad, now + Duration::from_millis(100));
        assert_eq!(next(&mut bridge), Some(Action::Undo));
        bridge.dialog = true;
        pad.buttons.set(HostButton::DpadUp, true);
        gate.controls(&mut bridge, pad, now);
        assert_eq!(next(&mut bridge), Some(Action::Up));
    }
}
