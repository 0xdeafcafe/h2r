//! Mouse look as the guest sees it: motion arrives at host rate, the manager
//! syncs after every guest slice, and the game reads `XInputGetState` far
//! less often (about 5 fps today). Every guest read that follows mouse motion
//! must see the right stick deflected.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use input::backend::fallback::KeyboardFallback;
use input::backend::keyboard::KeyboardMouse;
use input::bindings::Bindings;
use input::host::ConvertConfig;
use input::{InputManager, PortIndex};

/// The `h2 play` shape: keyboard+mouse merged into player one, port 0.
fn play_input() -> (InputManager, KeyboardMouse) {
    let keyboard = KeyboardMouse::new(Bindings::default());
    let convert = ConvertConfig::default();
    let mut manager = InputManager::new(convert).unwrap();
    manager.add_backend(Box::new(KeyboardFallback::new(
        keyboard.clone(),
        None,
        convert,
    )));
    (manager, keyboard)
}

#[test]
fn slow_guest_reads_see_motion_from_fast_host_syncs() {
    let (mut manager, keyboard) = play_input();
    manager.sync();
    let handle = manager.open(PortIndex::new(0).unwrap()).unwrap();
    let mut reads = Vec::new();
    // One host millisecond per step: a sync after every slice, a 125 Hz
    // mouse moving right and up, and a guest read every 200 ms that never
    // lands on the step of a mouse event (the first after the mouse moves).
    for ms in 1..=1003u32 {
        if ms % 8 == 0 {
            keyboard.mouse_motion(1.0, -1.0);
        }
        manager.sync();
        if ms % 200 == 3 && ms > 200 {
            reads.push(manager.get_state(handle).unwrap().gamepad);
        }
    }
    assert_eq!(reads.len(), 5);
    for (i, g) in reads.iter().enumerate() {
        assert!(
            g.thumb_rx > 0 && g.thumb_ry > 0,
            "guest read {i} saw right stick ({}, {})",
            g.thumb_rx,
            g.thumb_ry
        );
    }
}

/// Host steps of 1 ms for `ms` milliseconds: a sync after every step, a
/// 125 Hz mouse moving `counts` right each event, and a guest read every
/// `read_every` ms. Returns the right-stick x of each guest read.
fn run(ms: u32, counts: f64, read_every: u32) -> Vec<i16> {
    let now = Arc::new(Mutex::new(Instant::now()));
    let hand = Arc::clone(&now);
    let keyboard =
        KeyboardMouse::with_clock(Bindings::default(), Arc::new(move || *hand.lock().unwrap()));
    let convert = ConvertConfig::default();
    let mut manager = InputManager::new(convert).unwrap();
    manager.add_backend(Box::new(KeyboardFallback::new(
        keyboard.clone(),
        None,
        convert,
    )));
    manager.sync();
    let handle = manager.open(PortIndex::new(0).unwrap()).unwrap();
    manager.get_state(handle).unwrap();
    let mut reads = Vec::new();
    for step in 1..=ms {
        *now.lock().unwrap() += Duration::from_millis(1);
        if step % 8 == 0 {
            keyboard.mouse_motion(counts, 0.0);
        }
        manager.sync();
        if step % read_every == 3 {
            reads.push(manager.get_state(handle).unwrap().gamepad.thumb_rx);
        }
    }
    reads
}

/// `sensitivity` times the mouse speed in counts per 1/60 s, in XInput units.
fn expected(counts_per_8ms: f64) -> f64 {
    counts_per_8ms / 8.0 * (1000.0 / 60.0) * 0.04 * 32767.0
}

#[test]
fn deflection_is_proportional_to_mouse_speed_at_any_guest_rate() {
    for read_every in [17, 33, 200] {
        let slow = run(1000, 2.0, read_every);
        let fast = run(1000, 4.0, read_every);
        // Skip the first read: its interval began before the mouse moved.
        for (s, f) in slow.iter().zip(&fast).skip(1) {
            let (s, f) = (f64::from(*s), f64::from(*f));
            assert!(
                (s / expected(2.0) - 1.0).abs() < 0.5,
                "every {read_every} ms: {s} vs {}",
                expected(2.0)
            );
            assert!(
                (f / s - 2.0).abs() < 0.2,
                "every {read_every} ms: {f} vs {s}"
            );
        }
    }
}

#[test]
fn motion_is_spent_once_the_guest_has_read_it() {
    let now = Arc::new(Mutex::new(Instant::now()));
    let hand = Arc::clone(&now);
    let keyboard =
        KeyboardMouse::with_clock(Bindings::default(), Arc::new(move || *hand.lock().unwrap()));
    let mut manager = InputManager::new(ConvertConfig::default()).unwrap();
    manager.add_backend(Box::new(keyboard.clone()));
    manager.sync();
    let handle = manager.open(PortIndex::new(0).unwrap()).unwrap();
    keyboard.mouse_motion(5.0, 0.0);
    manager.sync();
    manager.sync();
    let first = manager.get_state(handle).unwrap().gamepad.thumb_rx;
    assert!(first > 0);
    // Reads between two syncs all see the same state.
    assert_eq!(manager.get_state(handle).unwrap().gamepad.thumb_rx, first);
    *now.lock().unwrap() += Duration::from_millis(5);
    manager.sync();
    assert_eq!(manager.get_state(handle).unwrap().gamepad.thumb_rx, 0);
}
