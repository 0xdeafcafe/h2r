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
fn removing_a_bot_unplugs_its_pad() {
    let l = Local::default();
    let h = l.add(1, "a", idle()).unwrap();
    let mut m = manager(&l, 1);
    assert_eq!(m.connected_mask(), 0b11);
    l.remove(h).unwrap();
    m.sync();
    assert_eq!(m.connected_mask(), 0b1);
    assert_eq!(l.remove(h), Err(Error::NoSuchBot(h)));
    assert!(l.is_empty());
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
fn the_join_pad_taps_a() {
    let a = |f| join_pad(f).buttons.contains(HostButton::South);
    assert!(a(0) && a(JOIN_HOLD - 1) && !a(JOIN_HOLD) && !a(JOIN_EVERY - 1) && a(JOIN_EVERY));
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
