use super::*;
use engine::testing::FakeGuest;

/// A real pad that is always there.
struct Human(u64);
impl HostBackend for Human {
    fn name(&self) -> &str {
        "human"
    }
    fn poll(&mut self) -> Vec<HostPad> {
        vec![HostPad {
            id: HostPadId(self.0),
            name: "pad".into(),
            state: HostPadState::default(),
            physical: true,
            supports_rumble: true,
        }]
    }
}

fn manager(local: &Local, humans: usize) -> input::InputManager {
    let mut m = input::InputManager::new(Default::default()).unwrap();
    for i in 0..humans {
        m.add_backend(Box::new(Human(i as u64)));
    }
    m.add_backend(Box::new(local.backend()));
    m.sync();
    m
}

fn idle() -> Box<dyn PadSource> {
    Box::new(|_: &Frame<'_>| HostPadState::default())
}

fn forward() -> HostPadState {
    HostPadState {
        left_stick: (0.0, 1.0),
        ..HostPadState::default()
    }
}

#[test]
fn a_bot_takes_the_port_it_asks_for() {
    let l = Local::default();
    let h = l.add(2, "b", idle()).unwrap();
    let m = manager(&l, 1);
    assert_eq!(m.connected_mask(), 0b101);
    assert_eq!(
        l.list(),
        vec![BotInfo {
            handle: h,
            name: "b".into(),
            wanted: 2,
            port: Some(2)
        }]
    );
    assert_eq!(l.port_mask(), 0b100);
    assert_eq!(l.name_on(2).as_deref(), Some("b"));
}

#[test]
fn a_port_a_human_holds_sends_the_bot_to_the_lowest_free_one() {
    let l = Local::default();
    l.add(1, "b", idle()).unwrap();
    manager(&l, 2);
    assert_eq!(l.list()[0].port, Some(2));
}

#[test]
fn adding_checks_the_port_and_the_count() {
    let l = Local::default();
    assert_eq!(l.add(4, "x", idle()), Err(Error::NoSuchPort(4)));
    l.add(1, "a", idle()).unwrap();
    assert_eq!(l.add(1, "b", idle()), Err(Error::PortTaken(1)));
    l.add(2, "b", idle()).unwrap();
    l.add(3, "c", idle()).unwrap();
    assert_eq!(l.add(0, "d", idle()), Err(Error::Full));
}

#[test]
fn a_removed_bot_stays_plugged_and_idle_until_unplugged() {
    let l = Local::default();
    let h = l.add(1, "a", Box::new(|_: &Frame<'_>| forward())).unwrap();
    let mut m = manager(&l, 1);
    assert_eq!(m.connected_mask(), 0b11);
    let g = FakeGuest::new();
    l.frame(0, &g, |_| false);
    assert_eq!(l.pad(h), Some(forward()));
    l.remove(h).unwrap();
    // Off the books at once, but the game still sees its pad: pulling it
    // in a game pauses on "controller disconnected".
    assert!(l.is_empty() && l.list().is_empty());
    assert_eq!(l.leaving(), vec![(h, Some(1))]);
    assert_eq!(l.port_mask(), 0b10, "its view stays hidden");
    assert_eq!(l.joining_mask(), 0, "it does not sign in again");
    assert_eq!(l.name_on(1), None);
    m.sync();
    assert_eq!(m.connected_mask(), 0b11, "still plugged");
    l.frame(1, &g, |_| true);
    assert_eq!(
        l.pad(h),
        Some(HostPadState::default()),
        "idle, its source not asked"
    );
    assert_eq!(l.remove(h), Err(Error::NoSuchBot(h)));
    assert!(l.unplug(h));
    m.sync();
    assert_eq!(m.connected_mask(), 0b1);
    assert!(l.leaving().is_empty());
    assert!(!l.unplug(h));
}

#[test]
fn only_a_leaving_bot_is_unplugged() {
    let l = Local::default();
    let h = l.add(1, "a", idle()).unwrap();
    assert!(!l.unplug(h), "on the books");
    assert_eq!(l.list().len(), 1);
}

#[test]
fn a_bot_added_on_a_leaving_port_takes_its_pad_back() {
    let l = Local::default();
    let h = l.add(1, "a", idle()).unwrap();
    let mut m = manager(&l, 1);
    l.remove(h).unwrap();
    let again = l.add(1, "b", Box::new(|_: &Frame<'_>| forward())).unwrap();
    assert_eq!(again, h, "the same pad: the game never sees it go");
    assert!(l.leaving().is_empty());
    m.sync();
    assert_eq!(m.connected_mask(), 0b11);
    l.frame(0, &FakeGuest::new(), |_| false);
    assert_eq!(l.pad(h), Some(forward()));
    assert_eq!(l.name_on(1).as_deref(), Some("b"));
}

#[test]
fn leaving_bots_wait_out_the_game_then_sign_out_then_unplug() {
    let (a, b, c) = (BotHandle(0), BotHandle(1), BotHandle(2));
    let leaving = [(a, Some(1)), (b, Some(2)), (c, None)];
    // In a game: nothing at all, whatever the slots say.
    assert_eq!(leave_step(true, &leaving, |_| true), LeaveStep::default());
    // Back in the menus: port 1 still signed in, port 2 already empty, the
    // never-placed bot has no slot.
    let step = leave_step(false, &leaving, |p| p == 1);
    assert_eq!(
        step,
        LeaveStep {
            sign_out: vec![1],
            unplug: vec![b, c],
        }
    );
    // The frame after the sign-out: the slot is empty, so the pad goes.
    let step = leave_step(false, &[(a, Some(1))], |_| false);
    assert_eq!(
        step,
        LeaveStep {
            sign_out: vec![],
            unplug: vec![a],
        }
    );
}

#[test]
fn a_bot_holding_a_saved_profile_gives_it_back_out_of_a_game() {
    // Bots on ports 1 and 3; port 3's slot holds a saved profile file (a
    // chooser picked the user's profile for its controller), port 0 is
    // the human's own.
    let saved = |p: u8| p == 3 || p == 0;
    assert_eq!(release_step(false, 0b1010, saved), vec![3]);
    assert!(
        release_step(true, 0b1010, saved).is_empty(),
        "never while a game runs"
    );
    assert!(
        release_step(false, 0b1010, |p| p == 0).is_empty(),
        "the human's profile is the human's"
    );
    assert!(
        release_step(false, 0b0010, |p| p == 3).is_empty(),
        "a port with no bot is not touched"
    );
}

#[test]
fn each_frame_the_source_sets_the_pad_the_game_reads() {
    let l = Local::default();
    let h = l
        .add(
            1,
            "a",
            Box::new(|f: &Frame<'_>| {
                let mut p = forward();
                p.right_trigger = if f.joining { 0.0 } else { 1.0 };
                p
            }),
        )
        .unwrap();
    let mut m = manager(&l, 1);
    let g = FakeGuest::new();
    l.frame(0, &g, |_| false);
    assert_eq!(l.pad(h).unwrap().right_trigger, 1.0);
    m.sync();
    let handle = m.open(input::PortIndex::new(1).unwrap()).unwrap();
    let pad = m.get_state(handle).unwrap().gamepad;
    assert_eq!(pad.thumb_ly, i16::MAX);
    assert_eq!(pad.analog[input::AnalogButton::RightTrigger.index()], 255);
    l.frame(1, &g, |p| p == 1);
    assert_eq!(l.pad(h).unwrap().right_trigger, 0.0, "told it is joining");
}

#[test]
fn an_unplaced_bot_is_not_asked() {
    let l = Local::default();
    let h = l.add(1, "a", Box::new(|_: &Frame<'_>| forward())).unwrap();
    l.frame(0, &FakeGuest::new(), |_| false);
    assert_eq!(l.pad(h), Some(HostPadState::default()));
}

fn frame_at(n: u64, g: &FakeGuest) -> Frame<'_> {
    Frame {
        frame: n,
        tick: None,
        port: 1,
        machine: None,
        guest: g,
        joining: false,
    }
}

#[test]
fn scripted_steps_play_in_turn_and_loop() {
    let g = FakeGuest::new();
    let mut s = Scripted::new(vec![(2, forward()), (1, HostPadState::default())], false);
    let seq: Vec<bool> = (10..14)
        .map(|n| s.pad(&frame_at(n, &g)) == forward())
        .collect();
    assert_eq!(seq, vec![true, true, false, false]);
    let mut s = Scripted::new(vec![(2, forward()), (1, HostPadState::default())], true);
    let seq: Vec<bool> = (0..6)
        .map(|n| s.pad(&frame_at(n, &g)) == forward())
        .collect();
    assert_eq!(seq, vec![true, true, false, true, true, false]);
}

#[test]
fn a_recording_plays_once_from_its_first_frame() {
    let g = FakeGuest::new();
    let mut r = Recorded::new(vec![forward(), HostPadState::default(), forward()]);
    let seq: Vec<bool> = (5..9)
        .map(|n| r.pad(&frame_at(n, &g)) == forward())
        .collect();
    assert_eq!(seq, vec![true, false, true, false]);
}

#[test]
fn a_source_is_told_the_game_tick_in_a_game_only() {
    let l = Local::default();
    let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
    let s2 = std::sync::Arc::clone(&seen);
    let h = l
        .add(
            1,
            "a",
            Box::new(move |f: &Frame<'_>| {
                s2.lock().unwrap().push((f.frame, f.tick));
                HostPadState::default()
            }),
        )
        .unwrap();
    let mut m = manager(&l, 1);
    m.sync();
    assert!(l.list().iter().any(|b| b.handle == h && b.port.is_some()));
    let mut g = FakeGuest::new();
    l.frame(0, &g, |_| false);
    let time = engine::points::GAME_TIME.base;
    g.map(time & !0xfff, 0x1000)
        .map(0x0068_0000, 0x10)
        .put_u32(time, 0x0068_0000)
        .put_u32(0x0068_0000 + crate::guest::GAME_TIME_TICK, 42);
    l.frame(1, &g, |_| false);
    assert_eq!(*seen.lock().unwrap(), vec![(0, None), (1, Some(42))]);
}

/// The human's real pad, which keeps the rumble it is sent.
struct Felt(Arc<Mutex<Vec<(HostPadId, input::Rumble)>>>);
impl HostBackend for Felt {
    fn name(&self) -> &str {
        "felt"
    }
    fn poll(&mut self) -> Vec<HostPad> {
        vec![HostPad {
            id: HostPadId(7),
            name: "pad".into(),
            state: HostPadState::default(),
            physical: true,
            supports_rumble: true,
        }]
    }
    fn set_rumble(&mut self, id: HostPadId, r: input::Rumble) {
        self.0.lock().unwrap().push((id, r));
    }
}

#[test]
fn rumble_for_a_bots_port_never_reaches_the_humans_pad() {
    use input::device::{FeedbackRequest, InputError};
    let felt = Arc::new(Mutex::new(Vec::new()));
    let l = Local::default();
    l.add(1, "b", idle()).unwrap();
    let mut m = input::InputManager::new(Default::default()).unwrap();
    m.add_backend(Box::new(Felt(Arc::clone(&felt))));
    m.add_backend(Box::new(l.backend()));
    m.sync();
    assert_eq!(m.connected_mask(), 0b11);
    let port = |p| input::PortIndex::new(p).unwrap();
    let (human, bot) = (m.open(port(0)).unwrap(), m.open(port(1)).unwrap());
    let shake = |left_motor| input::Rumble {
        left_motor,
        right_motor: 0,
    };
    // The bot's pad has no motors: the game's XInputSetState on its port is
    // refused (as on an Xbox pad with none) and nothing is queued.
    assert_eq!(
        m.capabilities(bot).unwrap().output,
        input::Rumble::default()
    );
    assert!(matches!(
        m.set_state(
            bot,
            FeedbackRequest {
                addr: 0x100,
                h_event: 0,
                rumble: shake(0x4000),
            },
        ),
        Err(InputError::NotSupported)
    ));
    m.set_state(
        human,
        FeedbackRequest {
            addr: 0x200,
            h_event: 0,
            rumble: shake(0x8000),
        },
    )
    .unwrap();
    m.sync();
    m.sync();
    assert_eq!(
        *felt.lock().unwrap(),
        vec![(HostPadId(7), shake(0x8000))],
        "only the human's own rumble, once"
    );
}
