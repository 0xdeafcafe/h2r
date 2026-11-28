use super::*;
use guest_layout::entry::{self, EntryFuture, Regs, Step};
use std::sync::Mutex;

const COMMAND: u32 = 0x4eb000;
const SC: u32 = 0x4ec000;
const ACTOR: u32 = 0x4ed000;
const UNIT: u32 = 0x12340000;
const BODY: u32 = 0x4ee000;
const TIME: u32 = 0x4ef000;
const STATE: u32 = 0x4ef100;
const SCRATCH: u32 = 0x4ef200;

fn fixture(kind: u16, operand: u16, amount: f32) -> (guest_layout::FlatMemory, Atom) {
    let mut m = guest_layout::FlatMemory::new(0x4e0000, 0x40000);
    for (global, header, data) in [
        (COMMANDS, 0x4ea000, COMMAND),
        (ACTORS, 0x4ea100, ACTOR),
        (points::OBJECTS.base, 0x4ea200, 0x4ea300),
    ] {
        m.set_u32(global, header).unwrap();
        m.set_u32(header + 68, data).unwrap();
    }
    m.set_u32(0x4ea200 + 56, 1).unwrap();
    m.set_u32(0x4ea200 + 36, 12).unwrap();
    m.set_u16(0x4ea300, (UNIT >> 16) as u16).unwrap();
    m.set_u32(0x4ea308, BODY).unwrap();
    m.set_u32(BODY + 0x14, NONE).unwrap();
    m.set_u32(points::SCENARIO.base, SC).unwrap();
    m.set_u32(points::GAME_TIME.base, TIME).unwrap();
    m.set_u16(TIME + 2, 30).unwrap();
    m.set_u32(SC + 0x1b8, 1).unwrap();
    m.set_u32(SC + 0x1bc, SC + 0x300).unwrap();
    m.write(SC + 0x300, b"h1_cmd_0\0").unwrap();
    m.set_u16(SC + 0x320, 5).unwrap();
    m.set_u16(COMMAND + 4, 4).unwrap();
    m.set_f32(COMMAND + 8, amount).unwrap();
    m.set_f32(
        COMMAND + 12,
        -(((u32::from(kind) + 1) * 65536 + u32::from(operand)) as f32),
    )
    .unwrap();
    m.set_u32(ACTOR + 24, UNIT).unwrap();
    m.set_u32(ACTOR + 0x58, NONE).unwrap();
    let a = decode(&m, 0).unwrap().unwrap();
    (m, a)
}

#[test]
fn shot_requests_fire_before_acceptance_and_restarts_the_recovery_on_each_miss() {
    let (mut m, a) = fixture(7, 0, 0.125);
    m.set_u32(SC + 0x1d8, 1).unwrap();
    m.set_u32(SC + 0x1dc, SC + 0x400).unwrap();
    m.set_u32(SC + 0x400, 1).unwrap();
    m.set_u32(SC + 0x404, SC + 0x500).unwrap();
    m.set_u32(SC + 0x520, 1).unwrap();
    m.set_u32(SC + 0x524, SC + 0x600).unwrap();
    let update = |m: &mut guest_layout::FlatMemory, t, kind, x, begin| {
        m.set_u32(TIME + 8, t).unwrap();
        m.set_u16(ACTOR + 0x722, kind).unwrap();
        m.set_f32(ACTOR + 0x724, x).unwrap();
        shoot(m, a, 0, UNIT, STATE, begin).unwrap()
    };
    assert!(!update(&mut m, 100, 2, 0.0, true));
    assert!(update(&mut m, 101, 2, 0.0, false));
    assert!(!update(&mut m, 200, 1, 0.0, true));
    assert!(!update(&mut m, 201, 1, 0.0, false));
    assert!(!update(&mut m, 260, 2, 0.49, false));
    assert!(!update(&mut m, 261, 2, 0.5, false));
    assert!(!update(&mut m, 320, 2, 0.0, false));
    assert!(update(&mut m, 321, 2, 0.0, false));
    assert_eq!(ticks(0.125, 60), 6);
    m.write(SCRATCH, &[0xad; 0x40]).unwrap();
    let copy = burst_pattern(&mut m, 0, SCRATCH, SCRATCH + 0x50)
        .unwrap()
        .unwrap();
    assert_eq!(m.u32(copy + 0x1c).unwrap(), 0xadadadad);
    assert_eq!(m.f32(copy + 0x20).unwrap(), 0.1);
    assert_eq!(m.f32(copy + 0x24).unwrap(), 0.1);
    assert_eq!(m.u32(SCRATCH + 0x20).unwrap(), 0xadadadad);
    m.set_f32(COMMAND + 8, 0.0).unwrap();
    assert_eq!(
        burst_pattern(&mut m, 0, SCRATCH, SCRATCH + 0x50).unwrap(),
        None
    );
}

#[test]
fn gaze_snapshots_an_unseen_units_head_once_and_releases_on_the_source_tick() {
    let (mut m, a) = fixture(4, 3, 0.125);
    m.set_u8(COMMAND + 0x46, 1).unwrap();
    m.set_u16(COMMAND + 0x48, 1).unwrap();
    m.set_u32(COMMAND + 0x4c, UNIT).unwrap();
    m.set_u16(TIME + 2, 60).unwrap();
    m.set_u32(TIME + 8, u32::MAX - 2).unwrap();
    let cx = Cx::new(Mutex::new(m), Regs::default());
    let mut fut: EntryFuture = Box::pin(begin_gaze(cx.clone(), a, 0, UNIT, STATE, SCRATCH));
    let Step::Call(marker) = entry::poll(&mut fut, &cx) else {
        panic!("expected head lookup")
    };
    assert_eq!(
        (marker.target, marker.ebx, marker.args),
        (0xb8d30, Some(SCRATCH), vec![UNIT, 0x04000095, 1])
    );
    let head = [1.0, 2.0, 3.0];
    cx.write(|m| put_vector(m, SCRATCH + 0x60, head)).unwrap();
    entry::resume(&cx, Regs::default());
    assert_eq!(entry::poll(&mut fut, &cx), Step::Done(Ok(Ret::new(0, 16))));
    cx.write(|m| {
        m.set_u16(0x4ea300, 0).unwrap(); // original target has disappeared
        gaze_targets(m, 0).unwrap();
        assert_eq!(m.u16(ACTOR + 0x434).unwrap(), 3);
        assert_eq!(m.vec3(ACTOR + 0x438).unwrap(), head);
        assert_eq!(m.vec3(ACTOR + 0x424).unwrap(), head);
        assert_eq!(m.u8(COMMAND + 0x50).unwrap(), 1);
        assert_eq!(m.u8(ACTOR + 0x44d).unwrap(), 1);
        m.set_u32(TIME + 8, 2).unwrap();
        assert!(!update_gaze(m, a, STATE).unwrap());
        m.set_u32(TIME + 8, 3).unwrap();
        assert!(update_gaze(m, a, STATE).unwrap());
        for offset in [0x46, 0x50, 0x51, 0x52] {
            assert_eq!(m.u8(COMMAND + offset).unwrap(), 0);
        }
    });
}

#[test]
fn perceived_unit_remains_live_and_conditional_facing_tracks_its_cone() {
    let (mut m, a) = fixture(4, 4, 1.0);
    m.set_u8(COMMAND + 0x46, 1).unwrap();
    m.set_u16(COMMAND + 0x48, 1).unwrap();
    m.set_u32(COMMAND + 0x4c, UNIT).unwrap();
    m.set_u32(ACTOR + 0x58, 0).unwrap();
    m.set_u32(0x502418, 0x4ea400).unwrap();
    m.set_u32(0x4ea444, 0x4ea500).unwrap();
    m.set_u32(0x4ea520, UNIT).unwrap();
    m.set_u16(0x4ea524, 1).unwrap();
    m.set_u32(0x4ea52c, NONE).unwrap();
    let cx = Cx::new(Mutex::new(m), Regs::default());
    let mut f: EntryFuture = Box::pin(begin_gaze(cx.clone(), a, 0, UNIT, STATE, SCRATCH));
    assert_eq!(entry::poll(&mut f, &cx), Step::Done(Ok(Ret::new(0, 16))));
    cx.write(|m| {
        assert_eq!(m.u8(COMMAND + 0x1c).unwrap(), 0);
        m.set_u16(ACTOR + 0x434, 6).unwrap();
        m.set_u32(ACTOR + 0x438, UNIT).unwrap();
        put_vector(m, ACTOR + 0x290, [1.0, 0.0, 0.0]).unwrap();
        for (x, body_turn) in [(1.0, 0), (-1.0, 1)] {
            put_vector(m, BODY + 0x64, [x, 0.0, 1.0]).unwrap();
            gaze_targets(m, 0).unwrap();
            assert_eq!(m.u8(ACTOR + 0x44d).unwrap(), body_turn);
            assert_eq!(m.u32(ACTOR + 0x438).unwrap(), UNIT);
        }
    });
}

#[test]
fn gaze_priority_and_facing_use_the_source_thresholds() {
    assert_eq!(
        (0..5).map(|n| strength(n, 3)).collect::<Vec<_>>(),
        [7, 5, 2, 8, 7]
    );
    assert_eq!(strength(0, 4), 2);
    let eye = [0.0; 3];
    let horizontal = [1.0, 0.0, 0.0];
    assert!(face_complete(eye, horizontal, [1.0, 0.0, 2.0], false));
    assert!(!face_complete(eye, horizontal, [1.0, 0.0, 2.0], true));
    assert!(face_complete(eye, [0.984, 0.0, 0.0], horizontal, false));
    assert!(!face_complete(eye, [0.983, 0.0, 0.0], horizontal, false));
    assert!(face_complete(eye, horizontal, eye, true));
}

#[test]
fn animation_finishes_when_the_custom_channel_is_interrupted() {
    let (mut m, _) = fixture(13, 2, 0.0);
    m.set_u16(BODY + 0x12a, 0x500).unwrap();
    let ch = BODY + 0x500;
    m.set_u32(ch + 0x7c, 0x0e0000c2).unwrap();
    assert!(playing(&m, UNIT).unwrap());
    m.set_u32(ch + 0x7c, 0).unwrap();
    assert!(!playing(&m, UNIT).unwrap());
    m.set_u32(ch + 0x7c, 0x0e0000c2).unwrap();
    m.set_u16(ch + 6, u16::MAX).unwrap();
    assert!(!playing(&m, UNIT).unwrap());
}
