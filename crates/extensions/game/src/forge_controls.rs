//! Desktop and pad input to the local Forge tick. The bridge carries host
//! presentation data only. What each button and key does is the bindings
//! table's ([`bindings::BINDINGS`]; docs/game/forge-editing.md "Controls").
//! While a screen is up, ours or the game's, the pad and the keys go to the
//! game's UI and the monitor takes none of them
//! (docs/game/forge-firefight-ui.md "The screens").
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
    time::Instant,
};

pub mod bindings;
mod render;
pub use bindings::{BINDINGS, Binding, Gesture};
use bindings::{Does, Held, Pad, Side};
pub use render::{camera_hooks, forget_park, parked};

#[derive(Clone, Copy)]
enum Event {
    Action(Action),
    Agent(Input),
}

/// What the session tells the gate and the keys each tick.
#[derive(Clone, Copy, Debug, Default)]
pub struct State {
    /// A local Forge session runs.
    pub active: bool,
    /// The monitor is up, not the player's body.
    pub monitor: bool,
    /// A screen is up, ours or the game's: the game's UI has the pad.
    pub screen: bool,
    pub camera: Option<Camera>,
    /// X opens Properties rather than the object menu: something is aimed
    /// at or held ([`Gesture::ObjectsOrProperties`]).
    pub x_properties: bool,
    /// B has something to cancel: an object held or placing, a link
    /// started. Without it Escape reaches the game (its GAME MENU).
    pub cancel: bool,
}

#[derive(Default)]
struct Bridge {
    state: State,
    eye: Option<Camera>,
    /// The table's keys held down (movement and the held controls).
    keys: Vec<Key>,
    mouse: [f32; 2],
    right_mouse: bool,
    /// Fine movement is on ([`Gesture::Fine`]).
    fine: bool,
    pad: HostPadState,
    events: VecDeque<Event>,
    sampled: Option<Instant>,
}

static BRIDGE: LazyLock<Mutex<Bridge>> = LazyLock::new(Mutex::default);
fn lock() -> MutexGuard<'static, Bridge> {
    BRIDGE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The session's state for the gate and the keys. Inactive clears it all.
pub fn publish(state: State) {
    lock().publish(state)
}

pub fn eye() -> Option<Camera> {
    lock().eye
}

/// Whether the local player is a monitor, for the session's monitor
/// channel and `forge.status`.
pub fn monitor() -> bool {
    lock().state.monitor
}

/// Whether fine movement is on, for the HUD and `forge.status`.
pub fn fine() -> bool {
    lock().fine
}

impl Bridge {
    fn publish(&mut self, state: State) {
        if !state.active {
            *self = Bridge::default();
            return;
        }
        let was = self.state;
        if !was.active || was.monitor != state.monitor || was.screen != state.screen {
            self.keys.clear();
            self.mouse = [0.; 2];
            self.right_mouse = false;
            self.sampled = None;
        }
        if state.screen && !was.screen {
            // A press queued before the screen came up was the monitor's;
            // the screen takes the pad from here.
            self.events.clear();
        }
        self.state = state;
    }

    fn push(&mut self, event: Event) {
        if self.events.len() < 64 {
            self.events.push_back(event);
        }
    }

    /// A gesture of the table: fine movement is the bridge's, the rest the
    /// tool's.
    fn gesture(&mut self, g: Gesture) {
        if g == Gesture::Fine {
            self.fine = !self.fine;
        } else if let Some(a) = bindings::resolve(g, self.state.x_properties) {
            self.push(Event::Action(a));
        }
    }

    /// Whether the key is the session's; a key it does not take reaches
    /// the game.
    fn key(&mut self, key: Key, down: bool, repeat: bool) -> bool {
        let s = self.state;
        // A screen up takes the keys as the game maps them.
        if !s.active || s.screen {
            return false;
        }
        let Some(row) = bindings::of_key(key) else {
            return false;
        };
        match row.does {
            Does::Move(_) | Does::Hold(_) => {
                if !s.monitor {
                    return false;
                }
                self.keys.retain(|k| *k != key);
                if down {
                    self.keys.push(key);
                }
                true
            }
            Does::Act(g) => {
                if !s.monitor && !bindings::in_body(g) {
                    return false;
                }
                // Escape with nothing to cancel is the game's: GAME MENU.
                if g == Gesture::Do(Action::Back) && !s.cancel {
                    return false;
                }
                if down && !repeat {
                    self.gesture(g);
                }
                true
            }
            Does::Game => false,
        }
    }

    /// Whether a held control is down on the pad or the keys.
    fn held(&self, h: Held) -> bool {
        let row = bindings::binding(Does::Hold(h));
        pad_down(&self.pad, row.pad)
            || row.keys.iter().any(|k| self.keys.contains(k))
            || (h == Held::Manipulate && self.right_mouse)
    }
}

/// Whether a [`Pad::Down`] or [`Pad::Trigger`] row is down.
fn pad_down(pad: &HostPadState, p: Pad) -> bool {
    match p {
        Pad::Down(b) => pad.buttons.contains(b),
        Pad::Trigger(Side::Left) => pad.left_trigger >= bindings::TRIGGER,
        Pad::Trigger(Side::Right) => pad.right_trigger >= bindings::TRIGGER,
        Pad::Tap(_) | Pad::Hold(_) | Pad::Stick(_) => false,
    }
}

pub fn key(key: Key, down: bool, repeat: bool) -> bool {
    lock().key(key, down, repeat)
}
pub fn release_keys() {
    let mut b = lock();
    b.keys.clear();
    b.mouse = [0.; 2];
    b.right_mouse = false;
}
pub fn mouse_look(dx: f32, dy: f32) -> bool {
    let mut b = lock();
    if !b.state.active || !b.state.monitor || b.state.screen {
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
    if !b.state.active || !b.state.monitor || b.state.screen {
        return false;
    }
    if left {
        if down {
            b.push(Event::Action(Action::Accept));
        }
    } else {
        b.right_mouse = down;
    }
    true
}

/// The agent queues exactly one sample. The driver checks the loaded map and
/// local role before admission; resetting the bridge discards outstanding input.
pub fn enqueue(input: Input) -> Result<(), String> {
    lock().enqueue(input)
}

impl Bridge {
    fn enqueue(&mut self, input: Input) -> Result<(), String> {
        let s = self.state;
        if !s.active || (!s.monitor && input.action != Some(Action::Toggle)) {
            return Err("The local Forge monitor is inactive.".into());
        }
        if s.screen {
            return Err("A screen is up; drive it with ui.press.".into());
        }
        if input.action == Some(Action::Menu) {
            return Err("The Forge menu is the game's GAME MENU; open it with Start.".into());
        }
        if self.events.len() >= 64 {
            return Err("The Forge input queue is full.".into());
        }
        self.events.push_back(Event::Agent(input));
        Ok(())
    }

    fn sample(&mut self, now: Instant) -> Input {
        let seconds = self.sampled.replace(now).map_or(1. / 30., |prev| {
            now.saturating_duration_since(prev).as_secs_f32().min(0.1)
        });
        // A queued agent tick has no simultaneous physical movement. Keep mouse
        // motion for the next real sample and use the screen actually up.
        let action = match self.events.pop_front() {
            Some(Event::Agent(input)) => {
                return Input {
                    dialog: self.state.screen,
                    ..input
                };
            }
            Some(Event::Action(a)) => Some(a),
            None => None,
        };
        let mouse = std::mem::take(&mut self.mouse);
        let pad = self.pad;
        let axis = |v: f32| {
            if v.is_finite() && v.abs() > 0.12 {
                v.clamp(-1., 1.)
            } else {
                0.
            }
        };
        let key = |k: Key| u8::from(self.keys.contains(&k)) as f32;
        let held = |h: Held| u8::from(self.held(h)) as f32;
        let scale = if self.fine { bindings::FINE } else { 1. };
        let thrust = if self.held(Held::Thrust) {
            bindings::THRUST
        } else {
            1.
        };
        let rotating = self.held(Held::Manipulate);
        let stick = [
            (axis(pad.left_stick.0) + key(Key::StrafeRight) - key(Key::StrafeLeft)).clamp(-1., 1.),
            (axis(pad.left_stick.1) + key(Key::Forward) - key(Key::Backward)).clamp(-1., 1.),
        ];
        // Manipulating, the left stick pushes, pulls and rolls: no thrust.
        let fly = if rotating { scale } else { scale * thrust };
        let movement = [
            stick[0] * fly,
            stick[1] * fly,
            (held(Held::Rise) - held(Held::Fall)) * fly,
        ];
        Input {
            action,
            click: None,
            movement,
            dialog: self.state.screen,
            seconds,
            rotating,
            look: [
                (mouse[0] - axis(pad.right_stick.0) * TURN_RATE * seconds) * scale,
                (mouse[1] + axis(pad.right_stick.1) * TURN_RATE * seconds) * scale,
            ],
            roll: stick[0] * TURN_RATE * seconds * scale,
        }
    }
}

pub fn sample() -> Input {
    lock().sample(Instant::now())
}

/// A button of the table's taps and holds: its tap row and its hold row.
struct Button {
    button: HostButton,
    tap: Option<Does>,
    hold: Option<Does>,
}

/// The table's tapped and held buttons, each once.
fn buttons() -> Vec<Button> {
    let mut v: Vec<Button> = Vec::new();
    for row in BINDINGS {
        let (b, is_hold) = match row.pad {
            Pad::Tap(b) => (b, false),
            Pad::Hold(b) => (b, true),
            _ => continue,
        };
        let i = match v.iter().position(|x| x.button == b) {
            Some(i) => i,
            None => {
                v.push(Button {
                    button: b,
                    tap: None,
                    hold: None,
                });
                v.len() - 1
            }
        };
        if is_hold {
            v[i].hold = Some(row.does);
        } else {
            v[i].tap = Some(row.does);
        }
    }
    v
}

#[derive(Default)]
struct Gate {
    previous: HostButtons,
    /// When each of [`buttons`]'s went down, while a hold may follow.
    held: [Option<Instant>; BINDINGS.len()],
}

pub fn gate() -> impl PadGate {
    Gate::default()
}

impl Gate {
    /// What `button` fires now: a tap as it goes down, or, for a button
    /// that can be held, the tap on a release before [`bindings::HOLD`] and
    /// the hold once it is reached.
    fn fire(
        timer: &mut Option<Instant>,
        button: &Button,
        down: bool,
        was: bool,
        now: Instant,
    ) -> Option<Does> {
        let Some(hold) = button.hold else {
            return if down && !was { button.tap } else { None };
        };
        if down && !was {
            *timer = Some(now);
        }
        if down && timer.is_some_and(|t| now.saturating_duration_since(t) >= bindings::HOLD) {
            *timer = None;
            return Some(hold);
        }
        if !down && was && timer.take().is_some() {
            return button.tap;
        }
        None
    }

    fn reset(&mut self) {
        self.held = Default::default();
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
        let pressed = pad.buttons;
        if b.state.screen {
            // The screen has the pad; a press that began under it is its.
            self.reset();
        } else {
            for (i, button) in buttons().iter().enumerate() {
                let down = pressed.contains(button.button);
                let was = self.previous.contains(button.button);
                let Some(Does::Act(g)) = Self::fire(&mut self.held[i], button, down, was, now)
                else {
                    continue;
                };
                if b.state.monitor || bindings::in_body(g) {
                    b.gesture(g);
                }
            }
        }
        self.previous = pressed;
        b.pad = pad;
    }
}
impl PadGate for Gate {
    fn filter(&mut self, pads: &mut [GatedPad]) {
        let mut b = lock();
        if !b.state.active {
            self.previous = pads
                .first()
                .map(|p| p.pad.state.buttons)
                .unwrap_or_default();
            self.reset();
            return;
        }
        // Player one is the first host pad. The desktop fallback merges
        // physical input with the keyboard and reports it as a virtual pad.
        let controller = pads.first_mut();
        let state = controller.as_ref().map(|p| p.pad.state).unwrap_or_default();
        self.controls(&mut b, state, Instant::now());
        if let Some(p) = controller {
            if b.state.screen {
                // The screen up (ours or the game's) takes the whole pad.
            } else if b.state.monitor {
                // Start still reaches the game: it opens the GAME MENU.
                let start = p.pad.state.buttons.contains(HostButton::Start);
                p.pad.state = HostPadState::default();
                p.pad.state.buttons.set(HostButton::Start, start);
            } else {
                let swap = bindings::binding(Does::Act(Gesture::Do(Action::Toggle)));
                if let Some(button) = swap.pad.button() {
                    p.pad.state.buttons.set(button, false);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn next(b: &mut Bridge) -> Option<Action> {
        match b.events.pop_front() {
            Some(Event::Action(a)) => Some(a),
            _ => None,
        }
    }

    fn monitor() -> Bridge {
        Bridge {
            state: State {
                active: true,
                monitor: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn key_routes_are_inactive_in_the_player_body_except_toggle() {
        let mut b = Bridge::default();
        assert!(!b.key(Key::Toggle, true, false));
        b.state.active = true;
        assert!(!b.key(Key::Accept, true, false));
        assert!(b.key(Key::Toggle, true, false));
        assert_eq!(next(&mut b), Some(Action::Toggle));
        b.state.monitor = true;
        b.key(Key::Objects, true, false);
        b.key(Key::Objects, true, true);
        b.key(Key::Objects, false, false);
        assert_eq!(next(&mut b), Some(Action::Palette));
        assert_eq!(next(&mut b), None);
        b.key(Key::Rise, true, false);
        assert!(b.held(Held::Rise));
        b.key(Key::Rise, false, false);
        assert!(!b.held(Held::Rise));
    }

    /// Escape is B's key: it cancels what is held, and with nothing to
    /// cancel it reaches the game, which opens its GAME MENU.
    #[test]
    fn escape_cancels_or_reaches_the_game() {
        let mut b = monitor();
        assert!(!b.key(Key::Back, true, false), "nothing held: the game's");
        assert_eq!(next(&mut b), None);
        b.state.cancel = true;
        assert!(b.key(Key::Back, true, false));
        assert_eq!(next(&mut b), Some(Action::Back));
        b.state.screen = true;
        assert!(!b.key(Key::Back, true, false), "a screen up: the game's");
    }

    #[test]
    fn the_merged_player_one_pad_preserves_face_pressure() {
        let mut bridge = monitor();
        let mut gate = Gate::default();
        let pad = HostPadState {
            analog_buttons: Some([1., 0., 0., 0., 1., 0.]),
            ..Default::default()
        };
        gate.controls(&mut bridge, pad, Instant::now());
        assert_eq!(next(&mut bridge), Some(Action::Accept));
        assert!(bridge.pad.buttons.contains(HostButton::RightBumper));
        assert!(bridge.held(Held::Rise), "a pressed Black is RB: up");
        gate.controls(&mut bridge, pad, Instant::now());
        assert_eq!(next(&mut bridge), None);
    }

    /// The gate's table: RB and LB rise and fall, RT held manipulates, LT
    /// held thrusts, hold Y deletes once, a tap of Y does nothing, B
    /// cancels at once, D-pad Left undoes and held redoes, D-pad Right and
    /// Down open the Tools and their settings, the stick click is fine
    /// movement.
    #[test]
    fn the_gate_follows_halo_3s_buttons() {
        let mut gate = Gate::default();
        let mut bridge = monitor();
        let now = Instant::now();
        let at = |ms: u64| now + Duration::from_millis(ms);
        let mut pad = HostPadState::default();
        let step = |gate: &mut Gate, b: &mut Bridge, pad: HostPadState, t| {
            gate.controls(b, pad, t);
            let a = next(b);
            assert_eq!(next(b), None, "one action a step");
            a
        };

        pad.buttons.set(HostButton::RightBumper, true);
        assert_eq!(step(&mut gate, &mut bridge, pad, at(0)), None);
        bridge.sampled = Some(at(0));
        let input = bridge.sample(at(33));
        assert!(input.movement[2] > 0.99, "RB rises");
        pad.buttons.set(HostButton::RightBumper, false);
        pad.buttons.set(HostButton::LeftBumper, true);
        step(&mut gate, &mut bridge, pad, at(40));
        assert!(bridge.sample(at(70)).movement[2] < -0.99, "LB falls");
        pad.buttons.set(HostButton::LeftBumper, false);

        pad.right_trigger = 1.;
        pad.left_stick = (0., 1.);
        step(&mut gate, &mut bridge, pad, at(80));
        let input = bridge.sample(at(110));
        assert!(input.rotating, "RT held manipulates");
        assert!(
            (input.movement[1] - 1.).abs() < 1e-6,
            "no thrust while manipulating"
        );
        pad.right_trigger = 0.;
        pad.left_trigger = 1.;
        step(&mut gate, &mut bridge, pad, at(120));
        let input = bridge.sample(at(150));
        assert!(!input.rotating);
        assert!(
            (input.movement[1] - bindings::THRUST).abs() < 1e-6,
            "LT thrusts"
        );
        pad.left_trigger = 0.;
        pad.left_stick = (0., 0.);

        pad.buttons.set(HostButton::North, true);
        assert_eq!(step(&mut gate, &mut bridge, pad, at(200)), None);
        assert_eq!(
            step(&mut gate, &mut bridge, pad, at(700)),
            Some(Action::Delete)
        );
        assert_eq!(step(&mut gate, &mut bridge, pad, at(1500)), None, "once");
        pad.buttons.set(HostButton::North, false);
        assert_eq!(step(&mut gate, &mut bridge, pad, at(1600)), None);
        pad.buttons.set(HostButton::North, true);
        step(&mut gate, &mut bridge, pad, at(1700));
        pad.buttons.set(HostButton::North, false);
        assert_eq!(
            step(&mut gate, &mut bridge, pad, at(1800)),
            None,
            "a tap of Y"
        );

        pad.buttons.set(HostButton::East, true);
        assert_eq!(
            step(&mut gate, &mut bridge, pad, at(1900)),
            Some(Action::Back)
        );
        pad.buttons.set(HostButton::East, false);
        step(&mut gate, &mut bridge, pad, at(1950));

        pad.buttons.set(HostButton::DpadLeft, true);
        step(&mut gate, &mut bridge, pad, at(2000));
        pad.buttons.set(HostButton::DpadLeft, false);
        assert_eq!(
            step(&mut gate, &mut bridge, pad, at(2100)),
            Some(Action::Undo)
        );
        pad.buttons.set(HostButton::DpadLeft, true);
        step(&mut gate, &mut bridge, pad, at(2200));
        assert_eq!(
            step(&mut gate, &mut bridge, pad, at(2700)),
            Some(Action::Redo)
        );
        pad.buttons.set(HostButton::DpadLeft, false);
        assert_eq!(
            step(&mut gate, &mut bridge, pad, at(2800)),
            None,
            "no undo after"
        );

        for (button, action) in [
            (HostButton::DpadRight, Action::Tools),
            (HostButton::DpadDown, Action::Settings),
            (HostButton::Back, Action::Overlays),
        ] {
            pad.buttons.set(button, true);
            assert_eq!(step(&mut gate, &mut bridge, pad, at(3000)), Some(action));
            pad.buttons.set(button, false);
            step(&mut gate, &mut bridge, pad, at(3050));
        }

        pad.buttons.set(HostButton::LeftStick, true);
        step(&mut gate, &mut bridge, pad, at(3100));
        assert!(bridge.fine, "the stick click turns fine movement on");
        pad.buttons.set(HostButton::LeftStick, false);
        pad.left_stick = (0., 1.);
        step(&mut gate, &mut bridge, pad, at(3150));
        let input = bridge.sample(at(3180));
        assert!(
            (input.movement[1] - bindings::FINE).abs() < 1e-6,
            "fine scales flying"
        );

        // Start is the game's: it fires nothing of ours.
        pad.buttons.set(HostButton::Start, true);
        assert_eq!(step(&mut gate, &mut bridge, pad, at(3200)), None);
    }

    #[test]
    fn a_screen_takes_the_pad() {
        let mut gate = Gate::default();
        let mut bridge = monitor();
        bridge.state.screen = true;
        let mut pad = HostPadState::default();
        pad.buttons.set(HostButton::DpadUp, true);
        pad.buttons.set(HostButton::South, true);
        gate.controls(&mut bridge, pad, Instant::now());
        assert_eq!(next(&mut bridge), None);
        assert!(!bridge.key(Key::Accept, true, false), "and the keys");
        assert!(bridge.sample(Instant::now()).dialog);
    }

    #[test]
    fn x_follows_what_is_aimed_at() {
        let mut gate = Gate::default();
        let mut bridge = monitor();
        let mut pad = HostPadState::default();
        let now = Instant::now();
        pad.buttons.set(HostButton::West, true);
        gate.controls(&mut bridge, pad, now);
        assert_eq!(next(&mut bridge), Some(Action::Palette));
        pad.buttons.set(HostButton::West, false);
        gate.controls(&mut bridge, pad, now);
        bridge.state.x_properties = true;
        pad.buttons.set(HostButton::West, true);
        gate.controls(&mut bridge, pad, now);
        assert_eq!(next(&mut bridge), Some(Action::Properties));
    }

    /// In the body only the swap works; the rest of the pad is the
    /// player's.
    #[test]
    fn the_body_takes_only_the_swap() {
        let mut gate = Gate::default();
        let mut bridge = Bridge {
            state: State {
                active: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pad = HostPadState::default();
        pad.buttons.set(HostButton::West, true);
        pad.buttons.set(HostButton::DpadUp, true);
        gate.controls(&mut bridge, pad, Instant::now());
        assert_eq!(next(&mut bridge), Some(Action::Toggle));
        assert_eq!(next(&mut bridge), None);
    }

    #[test]
    fn a_screen_up_refuses_the_agent_by_name() {
        let mut b = monitor();
        b.state.screen = true;
        let input = Input {
            action: Some(Action::Accept),
            ..Default::default()
        };
        assert_eq!(
            b.enqueue(input),
            Err("A screen is up; drive it with ui.press.".into())
        );
        b.state.screen = false;
        let menu = Input {
            action: Some(Action::Menu),
            ..Default::default()
        };
        assert!(b.enqueue(menu).unwrap_err().contains("GAME MENU"));
        assert_eq!(b.enqueue(input), Ok(()));
    }
}
