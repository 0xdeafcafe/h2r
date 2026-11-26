use super::*;
use forge::look::body::Pose;
use forge::look::census::Entry;
use forge::look::film::{LookRecord, PoseRecord, Source};
use forge::look::{MARKER, MonitorLook, VERSION};
use std::sync::MutexGuard;

/// The state is a static: tests that use it take turns (the film player's
/// tests too, `film_play::tests`).
static TURN: Mutex<()> = Mutex::new(());

pub(crate) fn turn() -> MutexGuard<'static, ()> {
    let guard = TURN.lock().unwrap_or_else(|e| e.into_inner());
    clear();
    guard
}

fn look(model: &str) -> MonitorLook {
    MonitorLook {
        version: VERSION,
        model: model.into(),
        variant: None,
        primary: 1,
        secondary: 2,
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

pub(crate) fn look_record(seat: u8, model: &str) -> Vec<u8> {
    LookRecord {
        seat,
        look: look(model),
        source: (model != MARKER).then(source),
    }
    .to_payload()
}

pub(crate) fn pose_record(seat: u8, tick: u32, x: f32, monitor: bool) -> Vec<u8> {
    PoseRecord {
        seat,
        tick,
        pose: Pose {
            position: [x, 2., 3.],
            yaw: 0.25,
            pitch: -0.5,
        },
        monitor,
    }
    .to_payload()
}

fn entry() -> Entry {
    Entry {
        key: "sentinel".into(),
        name: "sentinel".into(),
        source: "invented_map".into(),
        sha256: "abc".into(),
        variants: vec![],
        change_colours: true,
        light: false,
        nodes: 1,
        triangles: 1,
    }
}

fn rgb(_: u8) -> [u8; 3] {
    [1, 2, 3]
}

#[test]
fn the_welcome_and_the_items_are_left_to_the_simulation() {
    let _t = turn();
    deliver(1, &[vec![0, 9, 9], vec![1, 9, 9]]).unwrap();
    assert!(!active());
}

#[test]
fn a_look_and_a_pose_make_a_seat_the_camera_can_follow() {
    let _t = turn();
    deliver(
        30,
        &[look_record(13, MARKER), pose_record(13, 30, 5., true)],
    )
    .unwrap();
    assert!(active());
    assert!(is_monitor(13));
    assert!(!is_monitor(14));
    let sub = subject(13, View::Follow).expect("a monitor to follow");
    assert_eq!(sub.seat, 13);
    assert_eq!(sub.pose.position, [5., 2., 3.]);
    assert!(sub.body);
    // A seat the film has not spoken of has no subject.
    assert!(subject(14, View::Follow).is_none());
}

#[test]
fn first_person_looks_from_the_seat_and_hides_its_body() {
    let _t = turn();
    deliver(
        30,
        &[
            look_record(12, "sentinel"),
            look_record(13, "sentinel"),
            pose_record(12, 30, 1., true),
            pose_record(13, 30, 2., true),
        ],
    )
    .unwrap();
    let bodies = Bodies::new(&[entry()]);
    let seats = |d: Vec<Draw>| d.iter().map(|d| d.seat).collect::<Vec<_>>();
    assert_eq!(seats(draws(&bodies, rgb)), [12, 13]);
    let sub = subject(13, View::FirstPerson).unwrap();
    assert!(!sub.body);
    assert_eq!(seats(draws(&bodies, rgb)), [12]);
    // Another camera mode draws it again.
    subject(13, View::Orbit).unwrap();
    assert_eq!(seats(draws(&bodies, rgb)), [12, 13]);
    subject(13, View::FirstPerson).unwrap();
    release();
    assert_eq!(seats(draws(&bodies, rgb)), [12, 13]);
}

#[test]
fn a_seat_back_in_its_biped_is_no_subject() {
    let _t = turn();
    deliver(
        30,
        &[look_record(13, MARKER), pose_record(13, 30, 5., true)],
    )
    .unwrap();
    deliver(31, &[pose_record(13, 31, 5., false)]).unwrap();
    assert!(subject(13, View::Follow).is_none());
}

#[test]
fn the_film_tick_is_the_render_clock() {
    let _t = turn();
    deliver(
        0,
        &[look_record(13, "sentinel"), pose_record(13, 0, 0., true)],
    )
    .unwrap();
    deliver(30, &[pose_record(13, 30, 3., true)]).unwrap();
    let bodies = Bodies::new(&[entry()]);
    let at = |tick| {
        deliver(tick, &[]).unwrap();
        subject(13, View::Follow).unwrap().pose.position[0]
    };
    // Halfway between the samples, a second in, the body is halfway.
    let half = at(15);
    assert!(half > 0. && half < 3., "{half}");
    assert!((at(30) - 3.).abs() < 1e-3);
    assert_eq!(draws(&bodies, rgb).len(), 1);
}

#[test]
fn an_unreadable_record_is_a_named_film_error() {
    let _t = turn();
    let error = deliver(1, &[vec![looks::POSE, 1, 2, 3]]).unwrap_err();
    assert!(
        error.to_string().starts_with("The film's monitor record is not readable"),
        "{error}"
    );
    let error = deliver(1, &[vec![looks::LOOK, b'{']]).unwrap_err();
    assert!(error.to_string().contains("not readable"), "{error}");
}

#[test]
fn a_film_that_starts_over_reads_its_monitors_again() {
    let _t = turn();
    deliver(
        30,
        &[look_record(13, MARKER), pose_record(13, 30, 5., true)],
    )
    .unwrap();
    assert!(active() && is_monitor(13));
    clear();
    assert!(!active());
    assert!(!is_monitor(13));
}

#[test]
fn a_look_the_machine_cannot_draw_is_named_with_the_map_it_needs() {
    let records = vec![
        look_record(12, "sentinel"),
        look_record(13, MARKER),
        look_record(14, "sentinel"),
    ];
    // No renderer: every model look is unseen, the marker never.
    let none = CanDraw::ours(false, vec![]);
    let unseen = unseen_of(&records, &none);
    assert_eq!(
        unseen.iter().map(|u| (u.seat, u.map.as_str(), u.reason.as_str())).collect::<Vec<_>>(),
        [
            (12, "invented_map", looks::REASON),
            (14, "invented_map", looks::REASON)
        ]
    );
    // A machine with the renderer and the source map draws them all.
    let held = CanDraw::ours(true, vec!["abc".into()]);
    assert!(unseen_of(&records, &held).is_empty());
    // The same machine without the map names it.
    let without = CanDraw::ours(true, vec!["other".into()]);
    assert_eq!(unseen_of(&records, &without).len(), 2);
}

#[test]
fn a_file_that_is_not_a_film_names_no_looks() {
    let path = std::env::temp_dir().join("h2-forge-film-test-not-a-film");
    std::fs::write(&path, b"not a film").unwrap();
    assert!(unseen_in(&path, &CanDraw::ours(false, vec![])).is_empty());
    assert!(unseen_in(&path.with_extension("missing"), &CanDraw::ours(false, vec![])).is_empty());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_scan_keeps_each_distinct_look_once_and_ignores_the_rest() {
    let tick = |n: u64, forge: Vec<Vec<u8>>| film::blam::Tick {
        tick: n,
        forge,
        ..Default::default()
    };
    let welcome = vec![0, 1, 2];
    let mut ticks = vec![
        tick(1, vec![welcome, look_record(12, "sentinel"), pose_record(12, 1, 0., true)]),
        // A checkpoint repeats the table.
        tick(2, vec![look_record(12, "sentinel"), look_record(13, MARKER)]),
        tick(3, vec![look_record(12, "other")]),
    ]
    .into_iter();
    let found = distinct_looks(|| Ok(ticks.next())).unwrap();
    assert_eq!(
        found,
        [
            look_record(12, "sentinel"),
            look_record(13, MARKER),
            look_record(12, "other")
        ]
    );
    // Each seat's last word is its look.
    let none = CanDraw::ours(false, vec![]);
    let unseen = unseen_of(&found, &none);
    assert_eq!(unseen.len(), 1);
    assert_eq!(unseen[0].seat, 12);
}
