use super::*;
use crate::look::census::Entry;
use crate::look::{MARKER, VERSION};

fn look(model: &str, primary: u8) -> MonitorLook {
    MonitorLook {
        version: VERSION,
        model: model.into(),
        variant: None,
        primary,
        secondary: 2,
        // The marker has no tint.
        tint: u8::from(model != MARKER),
        light: 0,
        light_colour: 0,
    }
}

fn source() -> Source {
    Source {
        map: "invented_map".into(),
        sha256: "abc".into(),
    }
}

fn pose(x: f32) -> Pose {
    Pose {
        position: [x, 2., 3.],
        yaw: 0.25,
        pitch: -0.5,
    }
}

fn entry(key: &str) -> Entry {
    Entry {
        key: key.into(),
        name: key.into(),
        source: "invented_map".into(),
        sha256: "abc".into(),
        variants: vec![],
        change_colours: true,
        light: false,
        nodes: 1,
        triangles: 1,
    }
}

fn can(renderer: bool, maps: &[&str]) -> CanDraw {
    CanDraw::ours(renderer, maps.iter().map(|m| m.to_string()).collect())
}

#[test]
fn the_subtypes_leave_the_welcome_and_the_item_alone() {
    // 0 and 1 are `forge::net::film`'s welcome and item.
    assert_eq!((LOOK, POSE), (2, 3));
}

#[test]
fn a_look_and_a_pose_round_trip() {
    let l = LookRecord {
        seat: 3,
        look: look("sentinel", 4),
        source: Some(source()),
    };
    assert_eq!(Record::from_payload(&l.to_payload()), Ok(Record::Look(l)));
    let p = PoseRecord {
        seat: 7,
        tick: 123_456,
        pose: pose(1.5),
        monitor: true,
    };
    let bytes = p.to_payload();
    assert_eq!(bytes.len(), 27);
    assert_eq!(bytes[..3], [POSE, 7, 1]);
    assert_eq!(Record::from_payload(&bytes), Ok(Record::Pose(p)));
}

#[test]
fn unreadable_records_are_named_errors() {
    assert_eq!(Record::from_payload(&[]), Err(ReadError::Empty));
    assert_eq!(Record::from_payload(&[9, 1]), Err(ReadError::Kind(9)));
    assert_eq!(Record::from_payload(&[POSE, 1, 2]), Err(ReadError::Size(3)));
    assert!(matches!(
        Record::from_payload(&[LOOK, b'{']),
        Err(ReadError::Look(_))
    ));
    // A look out of bounds is refused as a look is anywhere.
    let mut bad = look("sentinel", 4);
    bad.primary = 200;
    let payload = LookRecord {
        seat: 0,
        look: bad,
        source: None,
    }
    .to_payload();
    assert!(matches!(
        Record::from_payload(&payload),
        Err(ReadError::Look(_))
    ));
    // A pose that is not finite is refused.
    let mut payload = PoseRecord {
        seat: 0,
        tick: 0,
        pose: pose(0.),
        monitor: true,
    }
    .to_payload();
    payload[7..11].copy_from_slice(&f32::NAN.to_le_bytes());
    assert!(matches!(
        Record::from_payload(&payload),
        Err(ReadError::Look(_))
    ));
    // Too long a look record is refused unparsed.
    let mut long = vec![LOOK];
    long.resize(MAX_LOOK_BYTES + 2, b' ');
    assert!(matches!(
        Record::from_payload(&long),
        Err(ReadError::Size(_))
    ));
}

#[test]
fn the_recorder_writes_a_look_when_the_table_changes_and_poses_at_15_a_second() {
    let mut rec = Recorder::default();
    assert!(rec.look(1, look("sentinel", 4), Some(source())).is_some());
    assert!(
        rec.look(1, look("sentinel", 4), Some(source())).is_none(),
        "unchanged"
    );
    assert!(rec.look(1, look("sentinel", 5), Some(source())).is_some());
    let written: Vec<u32> = (0..10)
        .filter(|t| rec.pose(*t, 1, pose(*t as f32), true).is_some())
        .collect();
    assert_eq!(written, [0, 2, 4, 6, 8], "every second tick of 30");
    // Another seat has its own rate, and leaving the monitor is at once.
    assert!(rec.pose(9, 2, pose(0.), true).is_some());
    assert!(rec.pose(9, 1, pose(0.), false).is_some(), "hidden at once");
}

fn film() -> (Vec<(u32, Vec<u8>)>, Recorder, Vec<Vec<u8>>) {
    // Two seats over 100 ticks: seat 1 changes look at 30 and steps back
    // into its biped at 90; seat 2 never has a look but the marker.
    let mut rec = Recorder::default();
    let mut records = Vec::new();
    let mut checkpoint = Vec::new();
    for tick in 0..100u32 {
        if tick == 0 {
            records.push((0, rec.look(1, look("sentinel", 4), Some(source())).unwrap()));
            records.push((0, rec.look(2, look(MARKER, 9), None).unwrap()));
        }
        if tick == 30 {
            records.push((
                tick,
                rec.look(1, look("sentinel", 7), Some(source())).unwrap(),
            ));
        }
        if tick == 40 {
            checkpoint = rec.checkpoint();
        }
        let monitor = tick < 90;
        for seat in [1u8, 2] {
            let p = pose(tick as f32 + f32::from(seat) * 100.);
            if let Some(bytes) = rec.pose(tick, seat, p, monitor || seat == 2) {
                records.push((tick, bytes));
            }
        }
    }
    (records, rec, checkpoint)
}

#[test]
fn a_seek_replays_to_the_same_table_and_poses_at_every_tick_after_it() {
    let (records, _, checkpoint) = film();
    let bodies = Bodies::new(&[entry("sentinel")]);
    let rgb = |i: u8| [i, i, i];
    // Played from the start.
    let mut whole = Playback::default();
    // Seeked to tick 40: the checkpoint, then what follows it.
    let mut seeked = Playback::default();
    for bytes in &checkpoint {
        seeked.feed(bytes).unwrap();
    }
    let mut seen_a = Vec::new();
    let mut seen_b = Vec::new();
    for tick in 0..100u32 {
        for (at, bytes) in &records {
            if *at == tick {
                whole.feed(bytes).unwrap();
                if tick >= 40 {
                    seeked.feed(bytes).unwrap();
                }
            }
        }
        if tick >= 40 {
            seen_a.push((
                whole
                    .table()
                    .into_iter()
                    .map(|(s, l)| (s, l.clone()))
                    .collect::<Vec<_>>(),
                whole.draws(&bodies, tick, None, rgb),
                whole.body(1, tick),
                whole.is_monitor(1),
            ));
            seen_b.push((
                seeked
                    .table()
                    .into_iter()
                    .map(|(s, l)| (s, l.clone()))
                    .collect::<Vec<_>>(),
                seeked.draws(&bodies, tick, None, rgb),
                seeked.body(1, tick),
                seeked.is_monitor(1),
            ));
        }
    }
    assert_eq!(seen_a, seen_b);
    // The look change at 30 is in the checkpoint's table; the pose at 90
    // hid seat 1's body in both.
    assert_eq!(seen_b[0].0[0].1.primary, 7);
    assert!(seen_b[20].2.is_some());
    assert!(seen_b[55].2.is_none());
    // Seat 2 wears the marker: no body, but a pose a camera can follow.
    assert!(seeked.body(2, 60).is_none());
    assert!(seeked.subject(2, 60, View::Follow).is_some());
}

#[test]
fn a_look_that_changed_before_a_checkpoint_is_the_checkpoints() {
    let (_, rec, checkpoint) = film();
    // The table and one pose a monitor seat: looks first, then poses.
    let kinds: Vec<u8> = checkpoint.iter().map(|p| p[0]).collect();
    assert_eq!(kinds, [LOOK, LOOK, POSE, POSE]);
    // The final checkpoint holds the last pose, not a history.
    assert_eq!(rec.checkpoint().len(), 4);
}

#[test]
fn follow_orbit_and_first_person_pick_the_monitors_pose_and_its_body() {
    let mut play = Playback::default();
    play.feed(
        &LookRecord {
            seat: 1,
            look: look("sentinel", 4),
            source: Some(source()),
        }
        .to_payload(),
    )
    .unwrap();
    play.feed(
        &PoseRecord {
            seat: 1,
            tick: 10,
            pose: pose(5.),
            monitor: true,
        }
        .to_payload(),
    )
    .unwrap();
    let follow = play.subject(1, 12, View::Follow).unwrap();
    assert_eq!((follow.pose, follow.body), (pose(5.), true));
    assert!(play.subject(1, 12, View::Orbit).unwrap().body);
    let first = play.subject(1, 12, View::FirstPerson).unwrap();
    assert_eq!((first.pose, first.body), (pose(5.), false));
    // A seat that is not a monitor, or never spoke, has no subject.
    assert!(play.subject(2, 12, View::Follow).is_none());
    play.feed(
        &PoseRecord {
            seat: 1,
            tick: 14,
            pose: pose(5.),
            monitor: false,
        }
        .to_payload(),
    )
    .unwrap();
    assert!(play.subject(1, 16, View::Follow).is_none());
    // First person draws every body but the viewing seat's.
    let bodies = Bodies::new(&[entry("sentinel")]);
    play.feed(
        &PoseRecord {
            seat: 1,
            tick: 18,
            pose: pose(6.),
            monitor: true,
        }
        .to_payload(),
    )
    .unwrap();
    assert_eq!(play.draws(&bodies, 18, None, |_| [0; 3]).len(), 1);
    assert!(play.draws(&bodies, 18, Some(1), |_| [0; 3]).is_empty());
}

#[test]
fn a_machine_without_the_source_reports_the_reason_and_draws_the_marker() {
    let mut play = Playback::default();
    play.feed(
        &LookRecord {
            seat: 1,
            look: look("sentinel", 4),
            source: Some(source()),
        }
        .to_payload(),
    )
    .unwrap();
    play.feed(
        &LookRecord {
            seat: 2,
            look: look(MARKER, 9),
            source: None,
        }
        .to_payload(),
    )
    .unwrap();
    play.feed(
        &PoseRecord {
            seat: 1,
            tick: 0,
            pose: pose(1.),
            monitor: true,
        }
        .to_payload(),
    )
    .unwrap();
    // A machine holding the map and the renderer reports nothing.
    assert!(play.unseen(&can(true, &["abc"])).is_empty());
    // Without the map: the reason, the seat and the map's name.
    let none = can(true, &[]);
    let unseen = play.unseen(&none);
    assert_eq!(
        unseen,
        [Unseen {
            seat: 1,
            map: "invented_map".into(),
            reason: REASON.into()
        }]
    );
    assert!(unseen[0].sentence().contains("invented_map"));
    // Without the renderer, the same reason.
    assert_eq!(play.unseen(&can(false, &["abc"])).len(), 1);
    // The bar shows it once: when the seat first appears, not again.
    assert_eq!(play.announce(&none).len(), 1);
    assert!(play.announce(&none).is_empty());
    // The film still plays: that machine's list lacks the model, so the
    // seat draws as the marker in the look's colours.
    let bodies = Bodies::new(&[]);
    let draws = play.draws(&bodies, 0, None, |_| [0; 3]);
    assert_eq!((draws[0].model, draws[0].primary), (None, 4));
}

#[test]
fn a_leaving_seat_drops_out_of_the_checkpoint() {
    let mut rec = Recorder::default();
    rec.look(1, look("sentinel", 4), None);
    rec.pose(0, 1, pose(0.), true);
    rec.left(1);
    assert!(rec.checkpoint().is_empty());
}

#[test]
fn a_replay_routes_looks_and_poses_beside_the_session_records() {
    use crate::net::film::{FilmError, Replay};
    use forge_doc::Limits;
    let mut replay = Replay::new(Limits::default());
    // They need no welcome and never reach the peer.
    replay
        .feed(
            &LookRecord {
                seat: 1,
                look: look("sentinel", 4),
                source: None,
            }
            .to_payload(),
        )
        .unwrap();
    replay
        .feed(
            &PoseRecord {
                seat: 1,
                tick: 3,
                pose: pose(1.),
                monitor: true,
            }
            .to_payload(),
        )
        .unwrap();
    assert!(replay.peer().is_none());
    assert_eq!(replay.presence().table().len(), 1);
    assert!(replay.presence().is_monitor(1));
    // An unreadable one is a named film error, and a session record is
    // still refused before its welcome.
    assert!(matches!(replay.feed(&[POSE, 1]), Err(FilmError::Record(_))));
    assert!(matches!(replay.feed(&[1, 0]), Err(FilmError::Order(_))));
}
