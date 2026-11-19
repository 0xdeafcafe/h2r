//! The gate: the chord, gestures and swallowing, over the same fake game.

use std::time::Duration;

use input::GatedPad;
use input::host::{HostButton, HostPad, HostPadId, HostPadState, Touchpad};

use super::*;
use crate::gate::CHORD;

const MS: Duration = Duration::from_millis(1);

fn ds(buttons: &[HostButton], touch: Touchpad) -> GatedPad {
    let mut state = HostPadState {
        prefiltered: true,
        left_stick: (0.0, 0.8),
        right_trigger: 1.0,
        touchpad: Some(touch),
        ..Default::default()
    };
    for b in buttons {
        state.buttons.set(*b, true);
    }
    GatedPad {
        key: (1, HostPadId(7)),
        pad: HostPad {
            id: HostPadId(7),
            name: "DualSense Wireless Controller".into(),
            state,
            physical: true,
            supports_rumble: true,
        },
    }
}

fn finger(x: f32, y: f32) -> Touchpad {
    Touchpad {
        primary: Some((x, y)),
        ..Default::default()
    }
}

struct Rig {
    s: Shared,
    fake: Fake,
    gate: Gate,
    t: Duration,
}

impl Rig {
    fn new() -> Self {
        let (s, fake) = menu();
        let gate = s.gate();
        Rig {
            s,
            fake,
            gate,
            t: Duration::ZERO,
        }
    }
    /// One sync: what the game gets.
    fn sync(&mut self, buttons: &[HostButton], touch: Touchpad) -> HostPadState {
        self.t += T;
        let mut pads = [ds(buttons, touch)];
        self.gate.filter_at(&mut pads, self.t);
        pads[0].pad.state
    }
    fn idle(&mut self) -> HostPadState {
        self.sync(&[], Touchpad::default())
    }
}

fn click() -> Touchpad {
    Touchpad {
        click: true,
        ..finger(0.0, 0.1)
    }
}

#[test]
fn touchpad_click_and_options_open_it_and_the_game_sees_nothing() {
    let mut r = Rig::new();
    let st = r.sync(&[HostButton::South], Touchpad::default());
    assert!(
        st.buttons.contains(HostButton::South),
        "closed: the game gets A"
    );
    assert_eq!(st.right_trigger, 1.0);
    // Click first, then Options: opens; the game sees a neutral pad.
    r.sync(&[HostButton::South], click());
    let st = r.sync(&[HostButton::South, HostButton::Start], click());
    assert!(r.s.is_open());
    assert_eq!(st.buttons, Default::default());
    assert_eq!((st.left_stick, st.right_trigger), ((0.0, 0.0), 0.0));
    assert!(st.prefiltered && st.touchpad.is_some());
    // Held through: still nothing, and the click of the chord does not select.
    r.sync(&[HostButton::South, HostButton::Start], click());
    assert_eq!(r.s.lock().menu.cursor(), 0);
    assert_eq!(r.s.lock().menu.current().map(|x| x.id.clone()), None);
}

#[test]
fn options_first_waits_for_the_click_then_reaches_the_game_alone() {
    let mut r = Rig::new();
    // Options, then the click a sync later: opens, and the game never saw Start.
    let st = r.sync(&[HostButton::Start], finger(0.0, 0.0));
    assert!(!st.buttons.contains(HostButton::Start), "held back");
    let st = r.sync(&[HostButton::Start], click());
    assert!(r.s.is_open());
    assert!(!st.buttons.contains(HostButton::Start));
    // Close with the chord again; Options stays hidden until let go.
    r.sync(&[], Touchpad::default());
    r.sync(&[HostButton::Start], click());
    assert!(!r.s.is_open());
    let st = r.sync(&[HostButton::Start], Touchpad::default());
    assert!(
        !st.buttons.contains(HostButton::Start),
        "latched after closing"
    );
    r.idle();
    // Options alone, held: the game gets it once the chord window passes.
    let mut seen = Vec::new();
    for _ in 0..10 {
        seen.push(
            r.sync(&[HostButton::Start], Touchpad::default())
                .buttons
                .contains(HostButton::Start),
        );
    }
    let first = seen
        .iter()
        .position(|s| *s)
        .expect("Start reached the game");
    assert!(
        first as u32 * 16 >= CHORD.as_millis() as u32 - 16,
        "{seen:?}"
    );
    assert!(seen[first..].iter().all(|s| *s));
    r.idle();
    // A quick tap inside the window still reaches it, one sync late.
    assert!(
        !r.sync(&[HostButton::Start], Touchpad::default())
            .buttons
            .contains(HostButton::Start)
    );
    assert!(r.idle().buttons.contains(HostButton::Start), "replayed");
    assert!(!r.idle().buttons.contains(HostButton::Start));
    let _ = MS;
}

#[test]
fn swipes_move_and_change_a_click_selects_two_fingers_go_back() {
    let mut r = Rig::new();
    r.sync(&[], click());
    r.sync(&[HostButton::Start], click());
    r.idle();
    assert!(r.s.is_open());
    // Swipe down three rows to Cheats (one long drag, then a short one),
    // click into it.
    for y in [0.6, 0.0, -0.6] {
        r.sync(&[], finger(0.0, y));
    }
    r.idle();
    for y in [0.5, -0.1] {
        r.sync(&[], finger(0.0, y));
    }
    r.idle();
    assert_eq!(r.s.lock().menu.cursor(), 3);
    r.sync(&[], finger(0.0, 0.0));
    r.sync(&[], click());
    r.idle();
    assert_eq!(
        r.s.lock().menu.current().map(|x| x.id.clone()),
        Some("cheats".into())
    );
    r.s.lines();
    // Swipe right on God mode: cheat.set.
    for x in [-0.5, 0.1] {
        r.sync(&[], finger(x, 0.0));
    }
    r.idle();
    r.s.lines();
    assert!(r.fake.game(|g| g.god));
    // Two fingers: back to the first page; again: closed.
    let two = Touchpad {
        primary: Some((0.0, 0.0)),
        secondary: Some((0.5, 0.5)),
        click: false,
    };
    r.sync(&[], finger(0.0, 0.0));
    r.sync(&[], two);
    r.idle();
    assert_eq!(r.s.lock().menu.current().map(|x| x.id.clone()), None);
    r.sync(&[], finger(0.0, 0.0));
    let st = r.sync(&[], two);
    assert!(!r.s.is_open());
    assert_eq!(
        st.right_trigger, 0.0,
        "the trigger held across the close stays hidden"
    );
    assert_eq!(r.idle().right_trigger, 0.0, "until it is let go");
    let mut pads = [ds(&[], Touchpad::default())];
    pads[0].pad.state.right_trigger = 0.0;
    r.gate.filter_at(&mut pads, r.t + T);
    let st = r.sync(&[], Touchpad::default());
    assert_eq!(st.right_trigger, 1.0, "pulled again: the game has it");
}

#[test]
fn the_d_pad_and_face_buttons_work_and_circle_never_reaches_the_game() {
    let mut r = Rig::new();
    r.s.key(Key::Toggle);
    for b in [HostButton::DpadDown, HostButton::DpadDown] {
        let st = r.sync(&[b], Touchpad::default());
        assert_eq!(st.buttons, Default::default());
        r.idle();
    }
    assert_eq!(r.s.lock().menu.cursor(), 2);
    r.sync(&[HostButton::South], Touchpad::default());
    r.idle();
    assert_eq!(
        r.s.lock().menu.current().map(|x| x.id.clone()),
        Some("tags".into())
    );
    r.sync(&[HostButton::East], Touchpad::default());
    r.idle();
    // Circle at the first page closes; held on, it never melees.
    let st = r.sync(&[HostButton::East], Touchpad::default());
    assert!(!r.s.is_open());
    assert!(!st.buttons.contains(HostButton::East));
    let st = r.sync(&[HostButton::East, HostButton::South], Touchpad::default());
    assert!(
        !st.buttons.contains(HostButton::East),
        "still held: still hidden"
    );
    assert!(
        st.buttons.contains(HostButton::South),
        "pressed after closing: the game's"
    );
    r.idle();
    assert!(
        r.sync(&[HostButton::East], Touchpad::default())
            .buttons
            .contains(HostButton::East)
    );
}

#[test]
fn a_pad_without_a_touchpad_gets_start_at_once_and_is_swallowed_too() {
    let mut r = Rig::new();
    let mut pads = [ds(&[HostButton::Start], Touchpad::default())];
    pads[0].pad.state.touchpad = None;
    r.gate.filter_at(&mut pads, T);
    assert!(
        pads[0].pad.state.buttons.contains(HostButton::Start),
        "no chord to wait for"
    );
    r.s.key(Key::Toggle);
    let mut pads = [ds(&[HostButton::West], Touchpad::default())];
    pads[0].pad.state.touchpad = None;
    pads[0].pad.state.analog_buttons = Some([0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
    r.gate.filter_at(&mut pads, 2 * T);
    assert_eq!(pads[0].pad.state.buttons, Default::default());
    assert_eq!(pads[0].pad.state.analog_buttons, None);
}

#[test]
fn keys_drive_it_and_closed_keys_pass() {
    let (s, _) = menu();
    assert!(!s.key(Key::Nav(Nav::Down)), "closed: the game's key");
    assert!(s.key(Key::Toggle));
    assert!(s.key(Key::Nav(Nav::Down)));
    assert_eq!(s.lock().menu.cursor(), 1);
    assert!(s.needs_redraw());
    s.lines();
    assert!(!s.needs_redraw());
}

#[test]
fn writes_are_only_what_the_user_asked() {
    let (s, fake) = menu();
    enter(&s, "render");
    enter(&s, "perf");
    enter(&s, "features");
    assert!(fake.writes().is_empty(), "{:?}", fake.writes());
}

#[test]
fn a_touchpad_click_alone_opens_it_without_selecting() {
    let mut r = Rig::new();
    let st = r.sync(&[], click());
    assert!(r.s.is_open(), "the click alone opens the menu");
    assert_eq!(st.buttons, Default::default());
    // Held: no select, no close.
    r.sync(&[], click());
    r.sync(&[HostButton::Start], click());
    assert!(r.s.is_open(), "Options with the opening click does not close it");
    assert_eq!(r.s.lock().menu.current().map(|x| x.id.clone()), None);
    // Let go, then a new click selects the row under the cursor.
    r.idle();
    r.sync(&[], click());
    r.idle();
    assert!(r.s.is_open());
    assert!(r.s.lock().menu.current().is_some(), "a later click selects");
}

#[test]
fn the_gate_reports_each_pad_and_the_chord_wait() {
    let mut r = Rig::new();
    assert_eq!(r.s.gate_report(), (false, Default::default()));
    r.sync(&[HostButton::Start], finger(0.0, 0.1));
    let (open, g) = r.s.gate_report();
    assert!(!open);
    assert_eq!(g.syncs, 1);
    assert_eq!(g.pads.len(), 1);
    let p = g.pads[0];
    assert_eq!(p.key, (1, 7));
    assert!(p.touchpad && !p.click && !p.chord);
    assert!(p.chord_pending, "Start waits for the click");
    r.sync(&[HostButton::Start], click());
    let (open, g) = r.s.gate_report();
    assert!(open);
    assert!(g.pads[0].click && g.pads[0].chord && g.pads[0].opened_by_press);
    assert!(!g.pads[0].chord_pending);
    // A pad without a touchpad says so.
    let mut pads = [ds(&[], Touchpad::default())];
    pads[0].pad.state.touchpad = None;
    r.gate.filter_at(&mut pads, r.t + T);
    assert!(!r.s.gate_report().1.pads[0].touchpad);
}

/// The whole path `h2 play` builds: a DualShock 4 behind the keyboard
/// fallback (it is player one, merged with the keyboard), the input
/// manager, and the menu's gate. The merge used to drop the touchpad, so
/// the click never reached the gate.
#[test]
fn a_touchpad_click_on_player_ones_pad_opens_it_through_the_input_manager() {
    use input::backend::fallback::KeyboardFallback;
    use input::backend::keyboard::KeyboardMouse;
    use input::bindings::Bindings;
    use input::{HostBackend, InputManager};
    use std::sync::{Arc, Mutex};

    struct Ds4(Arc<Mutex<Touchpad>>);
    impl HostBackend for Ds4 {
        fn name(&self) -> &str {
            "game-controller"
        }
        fn poll(&mut self) -> Vec<HostPad> {
            vec![HostPad {
                id: HostPadId(1),
                name: "DUALSHOCK 4 Wireless Controller".into(),
                state: HostPadState {
                    prefiltered: true,
                    touchpad: Some(*self.0.lock().unwrap()),
                    ..Default::default()
                },
                physical: true,
                supports_rumble: true,
            }]
        }
    }
    let touch = Arc::new(Mutex::new(Touchpad::default()));
    let (s, _fake) = menu();
    let mut m = InputManager::new(Default::default()).unwrap();
    m.add_backend(Box::new(KeyboardFallback::new(
        KeyboardMouse::new(Bindings::default()),
        Some(Box::new(Ds4(Arc::clone(&touch)))),
        Default::default(),
    )));
    m.set_gate(Box::new(s.gate()));
    m.sync();
    assert!(!s.is_open());
    *touch.lock().unwrap() = click();
    m.sync();
    assert!(s.is_open(), "the click on player one's pad opened the menu");
    let r = m.host_report().get();
    assert_eq!(r.pads[0].backend, "keyboard-fallback");
    assert_eq!(r.pads[0].port, Some(0));
    assert!(r.pads[0].state.touchpad.is_some_and(|t| t.click));
    let (_, g) = s.gate_report();
    assert!(g.pads[0].touchpad && g.pads[0].click);
}
