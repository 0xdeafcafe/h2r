use super::*;
use agent_proto::live::Layers;
use std::time::Duration;

fn world(x: f32) -> World {
    World {
        objects: vec![Object {
            id: 0x1_0001,
            position: [x, 0.0, 0.0],
            velocity: [10.0, 0.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            scale: 1.0,
            ..Object::default()
        }],
        players: vec![],
    }
}

fn frame(seq: u64, tick: u64, epoch: u64, prev: Option<&World>, next: &World) -> Frame {
    Frame::between(
        Header {
            kind: if prev.is_some() {
                Kind::Delta
            } else {
                Kind::Key
            },
            seq,
            tick,
            epoch,
            paused: false,
            cut: false,
        },
        Layers {
            objects: true,
            players: true,
        },
        prev,
        next,
    )
}

#[test]
fn display_blends_two_intervals_behind_and_late_motion_is_bounded() {
    let mut store = Store::new(10.0, 1).unwrap();
    let start = Instant::now();
    let millis = |ms| start + Duration::from_millis(ms);
    let (a, b, c) = (world(0.0), world(1.0), world(2.0));
    store.apply(&frame(1, 4, 1, None, &a), start).unwrap();
    store
        .apply(&frame(2, 5, 1, Some(&a), &b), millis(100))
        .unwrap();
    store
        .apply(&frame(3, 6, 1, Some(&b), &c), millis(200))
        .unwrap();
    assert_eq!(store.at_tick(5).unwrap().objects[0].position[0], 1.0);
    assert_eq!(
        store.sample(millis(250), false).unwrap().objects[0].position[0],
        0.5
    );
    assert_eq!(
        store.sample(millis(250), true).unwrap().objects[0].position[0],
        2.0
    );
    assert_eq!(
        store.sample(millis(450), false).unwrap().objects[0].position[0],
        2.5
    );
    let late = store.sample(millis(10_000), false).unwrap();
    assert_eq!(late.objects[0].position[0], 3.0);
    assert!(late.stale);
}

#[test]
fn teleport_and_slot_reuse_have_no_intermediate_position() {
    let mut store = Store::new(10.0, 1).unwrap();
    let t = Instant::now();
    let later = |ms| t + Duration::from_millis(ms);
    let (a, b) = (world(0.0), world(20.0));
    let mut c = world(20.5);
    c.objects[0].id += 1 << 16;
    store.apply(&frame(1, 1, 1, None, &a), t).unwrap();
    store
        .apply(&frame(2, 2, 1, Some(&a), &b), later(100))
        .unwrap();
    store
        .apply(&frame(3, 3, 1, Some(&b), &c), later(200))
        .unwrap();
    let before_teleport = store.sample(later(250), false).unwrap();
    assert_eq!(before_teleport.objects[0].position[0], 0.0);
    let before_reuse = store.sample(later(350), false).unwrap();
    assert_eq!(before_reuse.objects[0].position[0], 20.0);
    assert_eq!(before_reuse.objects[0].id, a.objects[0].id);
    assert_eq!(
        store.sample(later(400), false).unwrap().objects[0].id,
        c.objects[0].id
    );
}

#[test]
fn gaps_require_keys_and_cuts_discard_old_epoch_and_interpolation() {
    let mut store = Store::new(30.0, 1).unwrap();
    let t = Instant::now();
    let (a, b) = (world(1.0), world(2.0));
    store.apply(&frame(10, 81, 2, None, &a), t).unwrap();
    assert!(store.apply(&frame(12, 83, 2, Some(&a), &b), t).is_err());
    assert_eq!(store.at_tick(81).unwrap().objects, a.objects);
    let mut cut = frame(13, 12, 3, None, &b);
    cut.header.cut = true;
    cut.header.paused = true;
    store.apply(&cut, t).unwrap();
    assert!(store.at_tick(81).is_none());
    let paused = store.sample(t + Duration::from_secs(60), false).unwrap();
    assert_eq!(paused.tick, 12);
    assert_eq!(paused.objects, b.objects);
    assert!(!paused.stale);
}

#[test]
fn extrapolation_advances_the_parent_and_preserves_the_attachment_offset() {
    let start = Instant::now();
    let mut world = world(4.0);
    world.objects.push(Object {
        id: 0x2_0003,
        parent: Some(world.objects[0].id),
        position: [0.5, 0.0, 0.0],
        velocity: [10.0, 0.0, 0.0],
        ..Object::default()
    });
    let mut store = Store::new(10.0, 1).unwrap();
    store.apply(&frame(1, 4, 1, None, &world), start).unwrap();
    let late = store.sample(start + Duration::from_secs(1), false).unwrap();
    assert_eq!(late.objects[0].position[0], 5.0);
    assert_eq!(late.objects[1].position[0], 0.5);
}

#[test]
fn half_turn_blends_through_an_orthonormal_quarter_turn() {
    let mut a = world(0.0).objects[0];
    let b = Object {
        forward: [-1.0, 0.0, 0.0],
        ..a
    };
    blend(&mut a, &b, 0.5, 0.1);
    assert!(a.forward[0].abs() < 1e-5);
    assert!((a.forward[1].abs() - 1.0).abs() < 1e-5);
    assert!(dot(a.forward, a.up).abs() < 1e-5);
    assert!((dot(a.up, a.up) - 1.0).abs() < 1e-5);
}
