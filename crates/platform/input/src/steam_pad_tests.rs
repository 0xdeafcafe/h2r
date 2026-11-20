//! The Steam Controller hub, backend and thread over a fake host. Reports
//! are built from the layouts in `steam.rs`, never captured.
use super::*;
use crate::host::HostButton;
use std::sync::mpsc;

const WIRED: HidDevice = HidDevice {
    key: 7,
    vid: 0x28de,
    pid: 0x1102,
    usage_page: 0xff00,
    usage: 0x01,
};

/// A 2015 state report: buttons (low 24 bits), right pad (x, y).
fn d0g(packet: u32, raw: u32, right: (i16, i16)) -> Vec<u8> {
    let mut r = vec![0u8; 64];
    r[0] = 1;
    r[2] = 1;
    r[3] = 60;
    r[4..8].copy_from_slice(&packet.to_le_bytes());
    r[8..11].copy_from_slice(&raw.to_le_bytes()[..3]);
    r[20..22].copy_from_slice(&right.0.to_le_bytes());
    r[22..24].copy_from_slice(&right.1.to_le_bytes());
    r
}

const RPAD_TOUCH: u32 = 0x10_0000;
const A: u32 = 0x80;

fn shared() -> Shared {
    Shared::default()
}

fn commands(ws: &[(u64, HidWrite)]) -> Vec<u8> {
    ws.iter().map(|(_, w)| w.data[1]).collect()
}

#[test]
fn a_wired_controller_turns_lizard_mode_off_and_becomes_a_pad() {
    let (sh, tu) = (shared(), SteamTuning::default());
    let mut hub = Hub::default();
    let t0 = Instant::now();
    let w = hub.event(HidEvent::Added(WIRED), t0, &sh, &tu);
    assert_eq!(commands(&w), [0x81, 0x8e, 0x87], "lizard mode off");
    assert!(w.iter().all(|(k, w)| *k == 7 && w.feature));
    assert_eq!(lock(&sh.slots).len(), 1);
    // Its keyboard and mouse interfaces, and a non-Valve device, are skipped.
    for (pid, page, usage) in [(0x1102, 1, 6), (0x1102, 1, 2)] {
        let info = HidDevice {
            key: 8,
            pid,
            usage_page: page,
            usage,
            ..WIRED
        };
        assert!(hub.event(HidEvent::Added(info), t0, &sh, &tu).is_empty());
    }
    let other = HidDevice {
        key: 9,
        vid: 0x054c,
        ..WIRED
    };
    assert!(hub.event(HidEvent::Added(other), t0, &sh, &tu).is_empty());
    assert_eq!(lock(&sh.slots).len(), 1);
    // A state report updates the pad; no more lizard reports.
    let w = hub.event(HidEvent::Report(7, d0g(1, A, (0, 0))), t0, &sh, &tu);
    assert!(w.is_empty());
    let mut b = SteamBackend {
        shared: Arc::new(sh),
        _thread: Arc::new(Joiner {
            shared: Arc::new(Shared::default()),
            join: Mutex::new(None),
        }),
    };
    let pads = b.poll();
    assert_eq!(pads.len(), 1);
    assert_eq!(pads[0].name, "Steam Controller");
    assert!(pads[0].state.buttons.contains(HostButton::South));
    assert!(pads[0].supports_rumble && pads[0].physical);
    assert_eq!(
        crate::active::pad_kind(&pads[0].name),
        crate::active::DeviceKind::Steam
    );
}

#[test]
fn a_dongle_slot_waits_for_its_controller() {
    let (sh, tu) = (shared(), SteamTuning::default());
    let mut hub = Hub::default();
    let t0 = Instant::now();
    let slot = HidDevice {
        pid: 0x1142,
        ..WIRED
    };
    assert!(hub.event(HidEvent::Added(slot), t0, &sh, &tu).is_empty());
    assert!(lock(&sh.slots).is_empty());
    let mut ev = vec![0u8; 64];
    ev[0] = 1;
    ev[2] = 3;
    ev[3] = 1;
    ev[4] = 2; // connected
    let w = hub.event(HidEvent::Report(7, ev.clone()), t0, &sh, &tu);
    assert_eq!(commands(&w), [0x81, 0x8e, 0x87]);
    assert_eq!(lock(&sh.slots).len(), 1);
    // Disconnect drops the pad; reconnecting is a new pad id.
    let id = lock(&sh.slots)[0].id;
    ev[4] = 1;
    hub.event(HidEvent::Report(7, ev.clone()), t0, &sh, &tu);
    assert!(lock(&sh.slots).is_empty());
    // A state report with no connect event first also connects it.
    let w = hub.event(HidEvent::Report(7, d0g(5, 0, (0, 0))), t0, &sh, &tu);
    assert_eq!(commands(&w), [0x81, 0x8e, 0x87]);
    assert_ne!(lock(&sh.slots)[0].id, id);
    // Unplugged: gone.
    hub.event(HidEvent::Removed(7), t0, &sh, &tu);
    assert!(lock(&sh.slots).is_empty());
}

#[test]
fn the_right_trackpad_turns_the_right_stick_until_the_game_reads_it() {
    let (sh, tu) = (Arc::new(shared()), SteamTuning::default());
    let mut hub = Hub::default();
    let t0 = Instant::now();
    hub.event(HidEvent::Added(WIRED), t0, &sh, &tu);
    let ms = |n| t0 + Duration::from_millis(n);
    // Touch, then drag right (rotated 15 degrees by the parser, so mostly
    // right and a little up).
    hub.event(
        HidEvent::Report(7, d0g(1, RPAD_TOUCH, (0, 0))),
        ms(0),
        &sh,
        &tu,
    );
    hub.event(
        HidEvent::Report(7, d0g(2, RPAD_TOUCH, (2000, 0))),
        ms(8),
        &sh,
        &tu,
    );
    let mut b = SteamBackend {
        shared: Arc::clone(&sh),
        _thread: Arc::new(Joiner {
            shared: Arc::new(Shared::default()),
            join: Mutex::new(None),
        }),
    };
    let p = b.poll();
    let (x, y) = p[0].state.right_stick;
    assert!(x > 0.3, "turns right: {x}");
    assert!(y > 0.0 && y < x, "rotated up a little: {y}");
    // Unread motion stays on the stick; once read it is spent.
    assert_eq!(b.poll()[0].state.right_stick.0, x);
    b.state_read(p[0].id);
    assert_eq!(b.poll()[0].state.right_stick, (0.0, 0.0));
    // Lifting the thumb gives nothing more: no stick from position.
    hub.event(HidEvent::Report(7, d0g(3, 0, (2000, 0))), ms(16), &sh, &tu);
    hub.event(
        HidEvent::Report(7, d0g(4, RPAD_TOUCH, (20_000, 20_000))),
        ms(24),
        &sh,
        &tu,
    );
    assert_eq!(b.poll()[0].state.right_stick, (0.0, 0.0));
    // Higher sensitivity, the same drag: more turn.
    let fast = SteamTuning::new(LeftPadMode::Dpad, 10, true);
    hub.event(
        HidEvent::Report(7, d0g(5, RPAD_TOUCH, (20_000, 20_000))),
        ms(32),
        &sh,
        &fast,
    );
    hub.event(
        HidEvent::Report(7, d0g(6, RPAD_TOUCH, (20_400, 20_000))),
        ms(40),
        &sh,
        &fast,
    );
    let x2 = b.poll()[0].state.right_stick.0;
    b.state_read(p[0].id);
    hub.event(
        HidEvent::Report(7, d0g(7, RPAD_TOUCH, (20_800, 20_000))),
        ms(48),
        &sh,
        &tu,
    );
    let x1 = b.poll()[0].state.right_stick.0;
    assert!(x2 > x1 * 1.8, "{x2} vs {x1}");
}

#[test]
fn rumble_and_the_speaker_reach_the_actuators_each_frame() {
    let (sh, tu) = (shared(), SteamTuning::default());
    let mut hub = Hub::default();
    let t0 = Instant::now();
    hub.event(HidEvent::Added(WIRED), t0, &sh, &tu);
    assert!(hub.frame(t0, None, &sh, &tu).is_empty(), "nothing to play");
    let tone = Some(Tone {
        freq: 220.0,
        amp: 0.8,
    });
    let w = hub.frame(t0, tone, &sh, &tu);
    assert_eq!(commands(&w), [0x8f]);
    // Speaker off: the tone is not played.
    let off = SteamTuning::new(LeftPadMode::Dpad, 5, false);
    assert!(hub.frame(t0, tone, &sh, &off).is_empty());
    // Rumble from the game goes to the pad it was set on.
    let id = lock(&sh.slots)[0].id;
    let mut b = SteamBackend {
        shared: Arc::new(Shared::default()),
        _thread: Arc::new(Joiner {
            shared: Arc::new(Shared::default()),
            join: Mutex::new(None),
        }),
    };
    b.shared = Arc::new(sh);
    b.set_rumble(
        id,
        Rumble {
            left_motor: 65_535,
            right_motor: 65_535,
        },
    );
    let w = hub.frame(t0, None, &b.shared, &off);
    assert_eq!(
        commands(&w),
        [0x8f, 0x8f],
        "left heavy, right light: two trains"
    );
}

#[test]
fn the_2026_controller_is_kept_out_of_lizard_mode_and_released() {
    let (sh, tu) = (shared(), SteamTuning::default());
    let mut hub = Hub::default();
    let t0 = Instant::now();
    let tri = HidDevice {
        pid: 0x1302,
        usage_page: 0x01,
        usage: 0x05,
        ..WIRED
    };
    let w = hub.event(HidEvent::Added(tri), t0, &sh, &tu);
    assert_eq!(w.len(), 1);
    assert_eq!(&w[0].1.data[..4], &[1, 0x87, 3, 9]);
    assert!(hub.frame(t0, None, &sh, &tu).is_empty());
    let w = hub.frame(t0 + steam::KEEPALIVE, None, &sh, &tu);
    assert_eq!(w.len(), 1, "lizard mode off again");
    assert_eq!(&w[0].1.data[..4], &[1, 0x87, 3, 9]);
    assert!(hub.frame(t0 + steam::KEEPALIVE, None, &sh, &tu).is_empty());
    let w = hub.release(&sh);
    assert_eq!(w[0].1.data[0], 0x80, "motors stopped");
    assert_eq!(&w[1].1.data[..2], &[1, 0x85]);
    assert_eq!(&w[2].1.data[..2], &[1, 0x8e]);
    assert!(lock(&sh.slots).is_empty());
}

#[test]
fn bluetooth_feature_reports_go_out_in_segments() {
    let (sh, tu) = (shared(), SteamTuning::default());
    let mut hub = Hub::default();
    let ble = HidDevice {
        pid: 0x1106,
        ..WIRED
    };
    let w = hub.event(HidEvent::Added(ble), Instant::now(), &sh, &tu);
    assert_eq!(w.len(), 3);
    assert!(
        w.iter()
            .all(|(_, w)| w.data.len() == 20 && w.data[0] == 0x03)
    );
}

#[test]
fn the_left_pad_mode_setting_applies_to_the_next_report() {
    let (sh, tu) = (shared(), SteamTuning::default());
    let mut hub = Hub::default();
    let t0 = Instant::now();
    hub.event(HidEvent::Added(WIRED), t0, &sh, &tu);
    // Left pad touched and clicked near the top.
    let mut r = d0g(1, 0x08_0000 | 0x02_0000, (0, 0));
    r[16..18].copy_from_slice(&0i16.to_le_bytes());
    r[18..20].copy_from_slice(&30_000i16.to_le_bytes());
    hub.event(HidEvent::Report(7, r.clone()), t0, &sh, &tu);
    assert!(lock(&sh.slots)[0].host.buttons.contains(HostButton::DpadUp));
    tu.set_left_pad(LeftPadMode::Stick);
    r[4] = 2;
    hub.event(HidEvent::Report(7, r), t0, &sh, &tu);
    let h = lock(&sh.slots)[0].host;
    assert!(!h.buttons.contains(HostButton::DpadUp));
    assert!(h.buttons.contains(HostButton::LeftStick));
    assert!(h.left_stick.1 > 0.5);
}

#[test]
fn tones_queue_at_most_a_few_frames() {
    let sh = shared();
    for i in 0..10 {
        sh.push_tone(Some(Tone {
            freq: 100.0 + i as f32,
            amp: 1.0,
        }));
    }
    assert_eq!(lock(&sh.tones).len(), MAX_TONES);
    assert_eq!(sh.next_tone().unwrap().freq, 106.0, "the oldest dropped");
}

/// A fake host: events from a channel, writes recorded.
struct Fake {
    events: mpsc::Receiver<HidEvent>,
    writes: Arc<Mutex<Vec<(u64, HidWrite)>>>,
}

impl SteamHid for Fake {
    fn wait(&mut self, timeout: Duration) -> io::Result<Vec<HidEvent>> {
        Ok(self.events.recv_timeout(timeout).into_iter().collect())
    }

    fn write(&mut self, key: u64, report: &HidWrite) -> io::Result<()> {
        lock(&self.writes).push((key, report.clone()));
        Ok(())
    }
}

#[test]
fn the_thread_gives_lizard_mode_back_when_dropped() {
    let (tx, rx) = mpsc::channel();
    let writes = Arc::new(Mutex::new(Vec::new()));
    let fake = Fake {
        events: rx,
        writes: Arc::clone(&writes),
    };
    let mut slot = Some(fake);
    let pads = spawn(
        Box::new(move || slot.take().map(|f| Box::new(f) as Box<dyn SteamHid>)),
        SteamTuning::default(),
        Timing {
            retry: Duration::from_millis(1),
            poll: Duration::from_millis(1),
        },
    );
    tx.send(HidEvent::Added(WIRED)).unwrap();
    tx.send(HidEvent::Report(7, d0g(1, A, (0, 0)))).unwrap();
    let start = Instant::now();
    while pads.connected() == 0 && start.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(pads.connected(), 1);
    let mut backend = pads.backend();
    let start = Instant::now();
    while !backend
        .poll()
        .first()
        .is_some_and(|p| p.state.buttons.contains(HostButton::South))
        && start.elapsed() < Duration::from_secs(5)
    {
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(pads.speaker_live());
    drop(backend);
    drop(pads);
    let w = lock(&writes).clone();
    let cmds: Vec<u8> = w.iter().map(|(_, w)| w.data[1]).collect();
    assert_eq!(&cmds[..3], &[0x81, 0x8e, 0x87], "off on connect");
    assert_eq!(
        &cmds[cmds.len() - 3..],
        &[0x85, 0x8e, 0x87],
        "on at the end"
    );
}
