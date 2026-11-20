//! Report parsing and encoding. Every report here is built from the layouts
//! in `steam.rs` (SDL's `controller_structs.h`, Linux `hid-steam.c`), never
//! captured from a controller.
use super::*;

/// A 2015 wired/dongle state report (64 bytes, no report ID).
fn d0g(packet: u32, raw: u32, lt: u8, rt: u8, left: (i16, i16), right: (i16, i16)) -> Vec<u8> {
    let mut r = vec![0u8; 64];
    r[0..2].copy_from_slice(&1u16.to_le_bytes());
    r[2] = 1; // ID_CONTROLLER_STATE
    r[3] = 60;
    r[4..8].copy_from_slice(&packet.to_le_bytes());
    r[8..11].copy_from_slice(&raw.to_le_bytes()[..3]);
    r[11] = lt;
    r[12] = rt;
    r[16..18].copy_from_slice(&left.0.to_le_bytes());
    r[18..20].copy_from_slice(&left.1.to_le_bytes());
    r[20..22].copy_from_slice(&right.0.to_le_bytes());
    r[22..24].copy_from_slice(&right.1.to_le_bytes());
    r
}

/// A 2026 state report (`0x42`/`0x45`, or `0x47` with the trackpad
/// timestamp), 46 bytes.
#[allow(clippy::too_many_arguments)]
fn triton(
    id: u8,
    raw: u32,
    lt: i16,
    rt: i16,
    ls: (i16, i16),
    rs: (i16, i16),
    lp: (i16, i16),
    rp: (i16, i16),
) -> Vec<u8> {
    let mut r = vec![0u8; 46];
    r[0] = id;
    r[1] = 7;
    r[2..6].copy_from_slice(&raw.to_le_bytes());
    let mut put = |at: usize, v: i16| r[at..at + 2].copy_from_slice(&v.to_le_bytes());
    put(6, lt);
    put(8, rt);
    put(10, ls.0);
    put(12, ls.1);
    put(14, rs.0);
    put(16, rs.1);
    let pads = if id == 0x47 { 20 } else { 18 };
    put(pads, lp.0);
    put(pads + 2, lp.1);
    put(pads + 4, 1234); // pressure
    put(pads + 6, rp.0);
    put(pads + 8, rp.1);
    r
}

fn state(e: Option<Event>) -> PadState {
    match e {
        Some(Event::State(s)) => s,
        other => panic!("expected a state, got {other:?}"),
    }
}

#[test]
fn product_ids_and_interfaces() {
    assert_eq!(
        identify(0x28de, 0x1102),
        Some((Model::Original, Link::Wired))
    );
    assert_eq!(
        identify(0x28de, 0x1142),
        Some((Model::Original, Link::Dongle))
    );
    assert_eq!(
        identify(0x28de, 0x1106),
        Some((Model::Original, Link::Bluetooth))
    );
    assert_eq!(identify(0x28de, 0x1302), Some((Model::Triton, Link::Wired)));
    assert_eq!(
        identify(0x28de, 0x1303),
        Some((Model::Triton, Link::Bluetooth))
    );
    assert_eq!(
        identify(0x28de, 0x1304),
        Some((Model::Triton, Link::Dongle))
    );
    assert_eq!(
        identify(0x28de, 0x1205),
        None,
        "the Steam Deck is not handled"
    );
    assert_eq!(identify(0x054c, 0x1102), None, "another vendor");
    // Lizard mode's keyboard and mouse, and the puck's pogo pins, are skipped.
    assert!(!is_controller_interface(0x01, 0x02));
    assert!(!is_controller_interface(0x01, 0x06));
    assert!(!is_controller_interface(0xff00, 0x02));
    assert!(is_controller_interface(0xff00, 0x01));
    assert!(is_controller_interface(0x01, 0x05), "a gamepad collection");
}

#[test]
fn original_buttons_triggers_and_stick() {
    let mut p = Parser::new(Model::Original, Link::Wired);
    // A, right bumper, Escape (Start), Menu (Back), back-right grip.
    let raw = 0x80 | 0x04 | 0x4000 | 0x1000 | 0x1_0000;
    let s = state(p.feed(&d0g(1, raw, 255, 100, (12_000, -5_000), (0, 0))));
    let b = s.buttons;
    assert!(b.has(Buttons::A) && b.has(Buttons::RB) && b.has(Buttons::START));
    assert!(b.has(Buttons::BACK) && b.has(Buttons::RGRIP));
    assert!(!b.has(Buttons::B) && !b.has(Buttons::LB) && !b.has(Buttons::LGRIP));
    // Finger off the left pad: the shared fields are the stick.
    assert_eq!(s.left_stick, (12_000, -5_000));
    assert!(!s.left_pad.touched);
    // (255 << 7 | 255) is past 26000: full. 100 -> 12900 / 26000.
    assert_eq!(s.left_trigger, 1.0);
    assert!((s.right_trigger - 12_900.0 / 26_000.0).abs() < 1e-4);
    // The same packet number again is not new.
    assert_eq!(p.feed(&d0g(1, 0, 0, 0, (0, 0), (0, 0))), None);
    // Every face button maps to its own bit.
    for (bit, want) in [
        (0x80, Buttons::A),
        (0x20, Buttons::B),
        (0x40, Buttons::X),
        (0x10, Buttons::Y),
    ] {
        let s = state(p.feed(&d0g(10 + bit, bit, 0, 0, (0, 0), (0, 0))));
        assert_eq!(s.buttons.0, want, "{bit:#x}");
    }
}

#[test]
fn original_left_pad_touch_versus_click_and_the_stick_interleave() {
    let mut p = Parser::new(Model::Original, Link::Wired);
    // Stick pushed: remembered.
    p.feed(&d0g(1, 0, 0, 0, (20_000, 0), (0, 0)));
    // Finger on the left pad, stick still held: the controller alternates;
    // this report carries the pad, the stick keeps its last value.
    let s = state(p.feed(&d0g(2, 0x08_0000 | 0x80_0000, 0, 0, (0, 10_000), (0, 0))));
    assert!(s.left_pad.touched && !s.left_pad.clicked);
    assert_eq!(s.left_stick, (20_000, 0));
    // Rotated -15 degrees (SDL's RotatePad): (0, 10000) -> (2588, 9659).
    assert_eq!((s.left_pad.x, s.left_pad.y), (2588, 9659));
    // The next one carries the stick, and the pad keeps its value and touch.
    let s = state(p.feed(&d0g(3, 0x80_0000, 0, 0, (21_000, 0), (0, 0))));
    assert_eq!(s.left_stick, (21_000, 0));
    assert!(s.left_pad.touched);
    assert_eq!((s.left_pad.x, s.left_pad.y), (2588, 9659));
    // Clicked: touch and click both set.
    let s = state(p.feed(&d0g(4, 0x08_0000 | 0x02_0000, 0, 0, (0, -30_000), (0, 0))));
    assert!(s.left_pad.touched && s.left_pad.clicked);
    assert_eq!(s.left_stick, (0, 0), "the stick is not in use any more");
    // A pad click with the pad idle is the stick click on older firmware.
    let s = state(p.feed(&d0g(5, 0x02_0000, 0, 0, (0, 0), (0, 0))));
    assert!(s.buttons.has(Buttons::L3));
    assert!(!s.left_pad.clicked);
}

#[test]
fn original_right_pad_touch_and_click() {
    let mut p = Parser::new(Model::Original, Link::Wired);
    let s = state(p.feed(&d0g(1, 0x10_0000, 0, 0, (0, 0), (10_000, 0))));
    assert!(s.right_pad.touched && !s.right_pad.clicked);
    // +15 degrees: (10000, 0) -> (9659, 2588).
    assert_eq!((s.right_pad.x, s.right_pad.y), (9659, 2588));
    let s = state(p.feed(&d0g(2, 0x10_0000 | 0x04_0000, 0, 0, (0, 0), (0, 0))));
    assert!(s.right_pad.touched && s.right_pad.clicked);
    assert!(!s.left_pad.touched);
}

#[test]
fn dongle_wireless_events() {
    let mut p = Parser::new(Model::Original, Link::Dongle);
    let mut ev = vec![0u8; 64];
    ev[0] = 1;
    ev[2] = 3; // ID_CONTROLLER_WIRELESS
    ev[3] = 1;
    ev[4] = 2; // established
    assert_eq!(p.feed(&ev), Some(Event::Connected));
    ev[4] = 1;
    assert_eq!(p.feed(&ev), Some(Event::Disconnected));
    ev[3] = 0; // no payload: not an event
    assert_eq!(p.feed(&ev), None);
    // Too short or a foreign version.
    assert_eq!(p.feed(&ev[..3]), None);
    assert_eq!(p.feed(&[2, 0, 1, 60]), None);
}

/// Cuts a BLE message into report 0x03 segments of 18 bytes.
fn segments(msg: &[u8]) -> Vec<Vec<u8>> {
    let chunks: Vec<&[u8]> = msg.chunks(18).collect();
    chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut seg = vec![0u8; 20];
            seg[0] = 0x03;
            seg[1] = 0x80 | i as u8 | if i + 1 == chunks.len() { 0x40 } else { 0 };
            seg[2..2 + c.len()].copy_from_slice(c);
            seg
        })
        .collect()
}

#[test]
fn original_over_bluetooth_reassembles_and_keeps_chunks() {
    let mut p = Parser::new(Model::Original, Link::Bluetooth);
    // Buttons 1 (A, right pad touched), triggers, stick and right pad.
    let mask: u32 = 0x10 | 0x20 | 0x80 | 0x100 | 0x200;
    let mut msg = vec![(mask as u8 & 0xf0) | 4, (mask >> 8) as u8];
    msg.extend_from_slice(&(0x80u32 | 0x10_0000).to_le_bytes()[..3]);
    msg.extend_from_slice(&[255, 0]);
    msg.extend_from_slice(&1000i16.to_le_bytes());
    msg.extend_from_slice(&(-2000i16).to_le_bytes());
    msg.extend_from_slice(&[0; 4]); // the left pad, idle
    msg.extend_from_slice(&0i16.to_le_bytes());
    msg.extend_from_slice(&(-8000i16).to_le_bytes());
    let segs = segments(&msg);
    assert_eq!(segs.len(), 2, "{} bytes in two segments", msg.len());
    assert_eq!(p.feed(&segs[0]), None, "not complete yet");
    let s = state(p.feed(&segs[1]));
    assert!(s.buttons.has(Buttons::A));
    assert!(s.right_pad.touched);
    assert_eq!(s.left_stick, (1000, -2000));
    assert_eq!(s.left_trigger, 1.0);
    // Rotated +15 degrees: (0, -8000) -> (2071, -7727).
    assert_eq!((s.right_pad.x, s.right_pad.y), (2071, -7727));
    // A later message with buttons only keeps the stick and pad.
    let msg = vec![0x10 | 4, 0, 0x20, 0, 0];
    let s = state(p.feed(&segments(&msg)[0]));
    assert!(s.buttons.has(Buttons::B) && !s.buttons.has(Buttons::A));
    assert_eq!(s.left_stick, (1000, -2000));
    assert!(!s.right_pad.touched, "the touch bit is a button bit");
    // A segment out of order starts over; an empty one is skipped.
    let mut late = segs[1].clone();
    late[1] = 0x80 | 0x40 | 1;
    assert_eq!(p.feed(&late), None);
    let mut empty = segs[0].clone();
    empty[1] = 0;
    assert_eq!(p.feed(&empty), None);
    // Keyboard and mouse reports on the same device are not segments.
    assert_eq!(p.feed(&[0x01, 0, 0, 0]), None);
}

#[test]
fn triton_buttons_sticks_triggers_and_pads() {
    let mut p = Parser::new(Model::Triton, Link::Wired);
    // A, R, View (Start), Menu (Back), d-pad up, L4, right pad touched.
    let raw = 0x1 | 0x200 | 0x40 | 0x4000 | 0x2000 | 0x2_0000 | 0x20_0000;
    let r = triton(
        0x42,
        raw,
        0,
        32767,
        (100, -200),
        (-30_000, 4_000),
        (0, 0),
        (5_000, -6_000),
    );
    let s = state(p.feed(&r));
    let b = s.buttons;
    assert!(b.has(Buttons::A) && b.has(Buttons::RB) && b.has(Buttons::START));
    assert!(b.has(Buttons::BACK) && b.has(Buttons::DPAD_UP) && b.has(Buttons::LGRIP));
    assert!(!b.has(Buttons::LB) && !b.has(Buttons::RGRIP));
    assert_eq!(s.left_stick, (100, -200));
    assert_eq!(s.right_stick, Some((-30_000, 4_000)));
    assert_eq!((s.left_trigger, s.right_trigger), (0.0, 1.0));
    assert!(s.right_pad.touched && !s.right_pad.clicked);
    assert_eq!(
        (s.right_pad.x, s.right_pad.y),
        (5_000, -6_000),
        "not rotated"
    );
    assert!(!s.left_pad.touched);
    // The BLE report has the same layout.
    let s = state(p.feed(&triton(
        0x45,
        0x0600_0000,
        16384,
        0,
        (0, 0),
        (0, 0),
        (-7, 9),
        (0, 0),
    )));
    assert!(s.left_pad.touched && s.left_pad.clicked);
    assert_eq!((s.left_pad.x, s.left_pad.y), (-7, 9));
    assert!((s.left_trigger - 0.5).abs() < 1e-3);
    // 0x47 moves the pads two bytes on for the trackpad timestamp.
    let s = state(p.feed(&triton(
        0x47,
        0x0020_0000,
        0,
        0,
        (0, 0),
        (0, 0),
        (1, 2),
        (300, 400),
    )));
    assert_eq!((s.right_pad.x, s.right_pad.y), (300, 400));
    assert_eq!((s.left_pad.x, s.left_pad.y), (1, 2));
    // Short, battery and unknown reports say nothing.
    assert_eq!(p.feed(&r[..45]), None);
    assert_eq!(p.feed(&[0x43; 16]), None);
    assert_eq!(p.feed(&[]), None);
    // The puck's wireless status.
    assert_eq!(p.feed(&[0x79, 2]), Some(Event::Connected));
    assert_eq!(p.feed(&[0x46, 1]), Some(Event::Disconnected));
}

#[test]
fn lizard_mode_reports_for_the_2015_controller() {
    let off = lizard_off(Model::Original);
    assert_eq!(off.len(), 3);
    assert!(
        off.iter()
            .all(|w| w.feature && w.data.len() == 65 && w.data[0] == 0)
    );
    assert_eq!(&off[0].data[1..3], &[0x81, 0], "clear digital mappings");
    assert_eq!(&off[1].data[1..3], &[0x8e, 0], "load default settings");
    assert_eq!(
        &off[2].data[1..15],
        &[0x87, 12, 49, 2, 0, 7, 7, 0, 8, 7, 0, 24, 0, 0],
        "packet version 2, both trackpads TRACKPAD_NONE, smoothing off"
    );
    assert!(off[2].data[15..].iter().all(|&b| b == 0));
    let on = lizard_on(Model::Original);
    assert_eq!(on.len(), 3);
    assert_eq!(&on[0].data[1..3], &[0x85, 0], "default digital mappings");
    assert_eq!(&on[1].data[1..3], &[0x8e, 0]);
    assert_eq!(&on[2].data[1..6], &[0x87, 3, 8, 0, 0], "right pad: mouse");
    assert_eq!(keepalive(Model::Original), None);
}

#[test]
fn lizard_mode_reports_for_the_2026_controller() {
    let off = lizard_off(Model::Triton);
    assert_eq!(off.len(), 1);
    let w = &off[0];
    assert!(w.feature);
    assert_eq!(w.data.len(), 64);
    assert_eq!(
        &w.data[..6],
        &[1, 0x87, 3, 9, 0, 0],
        "SETTING_LIZARD_MODE off"
    );
    assert_eq!(keepalive(Model::Triton).as_ref(), Some(w));
    let on = lizard_on(Model::Triton);
    assert_eq!(on.len(), 2);
    assert_eq!(&on[0].data[..3], &[1, 0x85, 0]);
    assert_eq!(&on[1].data[..3], &[1, 0x8e, 0]);
}

#[test]
fn haptic_pulse_and_rumble_encoding() {
    let t = PulseTrain {
        on_us: 0x0190,
        off_us: 0x014d,
        count: 0x002d,
        gain_db: -6,
    };
    let w = haptic_pulse(Model::Original, Side::Right, t);
    assert!(w.feature);
    assert_eq!(
        &w.data[..11],
        &[0, 0x8f, 8, 0, 0x90, 0x01, 0x4d, 0x01, 0x2d, 0x00, 0xfa],
        "right pad is 0 on the wire"
    );
    assert_eq!(haptic_pulse(Model::Original, Side::Left, t).data[3], 1);
    assert_eq!(haptic_pulse(Model::Original, Side::Both, t).data[3], 2);
    let w = haptic_pulse(Model::Triton, Side::Left, t);
    assert!(!w.feature);
    assert_eq!(w.data, [0x81, 1, 0x90, 0x01, 0x4d, 0x01, 0x2d, 0x00]);
    let w = triton_rumble(0x1234, 0xabcd);
    assert!(!w.feature);
    assert_eq!(w.data, [0x80, 0, 0, 0, 0x34, 0x12, 0, 0xcd, 0xab, 0]);
}

#[test]
fn bluetooth_feature_reports_are_segmented() {
    let off = lizard_off(Model::Original);
    // 0x87, 12, 12 bytes of settings: 14 bytes, one segment.
    let segs = on_link(Model::Original, Link::Bluetooth, off[2].clone());
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].data.len(), 20);
    assert_eq!(&segs[0].data[..4], &[0x03, 0xc0, 0x87, 12]);
    // A long message: three segments numbered 0, 1, 2, the last flagged.
    let long = feature(Model::Original, 0x87, &[0xaa; 45]);
    let segs = ble_segments(&long);
    assert_eq!(
        segs.iter().map(|s| s.data[1]).collect::<Vec<_>>(),
        [0x80, 0x81, 0xc2]
    );
    assert_eq!(&segs[0].data[2..5], &[0x87, 45, 0xaa]);
    // Wired, the dongle and the 2026 controller send it whole.
    assert_eq!(
        on_link(Model::Original, Link::Wired, off[2].clone()),
        [off[2].clone()]
    );
    let p = haptic_pulse(
        Model::Triton,
        Side::Left,
        PulseTrain {
            on_us: 1,
            off_us: 1,
            count: 1,
            gain_db: 0,
        },
    );
    assert_eq!(on_link(Model::Triton, Link::Bluetooth, p.clone()), [p]);
}
