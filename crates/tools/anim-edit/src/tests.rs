use super::*;
use tags::anim_sample::NodeDefault;

fn doc() -> AnimDoc {
    AnimDoc::new(
        "invented".into(),
        11,
        vec![Node {
            name: Some("left_foot".into()),
            parent: -1,
        }],
    )
    .unwrap()
}

#[test]
fn a_drag_is_one_undo_step_and_different_edits_have_separate_steps() {
    let mut clip = doc();
    let gesture = clip.begin_gesture();
    for x in [1.0, 2.0, 3.0] {
        clip.set_key_during_gesture(
            0,
            Kind::Translation,
            4,
            [x, 0.0, 0.0, 0.0],
            None,
            Some(gesture),
        )
        .unwrap();
    }
    clip.set_key(0, Kind::Scale, 4, [2.0, 0.0, 0.0, 0.0], None)
        .unwrap();
    assert!(clip.undo());
    assert_eq!(clip.sample(4.0).nodes[0].translation[0], 3.0);
    assert!(clip.undo());
    assert_eq!(clip.channels().count(), 0);
    assert!(!clip.undo());
    assert!(clip.redo());
    assert_eq!(clip.sample(4.0).nodes[0].translation[0], 3.0);
    assert!(clip.redo());
    assert_eq!(clip.sample(4.0).nodes[0].scale, 2.0);
}

#[test]
fn an_angle_view_unwraps_without_changing_the_quaternion() {
    let mut clip = doc();
    let kind = Kind::Rotation;
    let a = angles_to_rotation([179.0, 22.0, -9.0]);
    let b = angles_to_rotation([181.0, 24.0, -8.0]);
    clip.set_key(0, kind, 0, a, None).unwrap();
    clip.set_key(0, kind, 10, b, None).unwrap();
    let keys = clip.keys(0, kind).to_vec();
    let angles = angles_near(keys[1].value, rotation_to_angles(keys[0].value));
    assert!((angles[0] - 181.0).abs() < 0.001);
    assert!(anim_keys::distance(kind, angles_to_rotation(angles), keys[1].value) < 0.00001);
    assert_eq!(clip.keys(0, kind), keys);
    assert_eq!(clip.revision(), 2);
}

#[test]
fn angle_view_round_trips_at_gimbal_lock_and_unwraps_through_it() {
    let singular = angles_to_rotation([35.0, 90.0, 62.0]);
    let shown = rotation_to_angles(singular);
    assert!(anim_keys::distance(Kind::Rotation, singular, angles_to_rotation(shown)) < 0.00001);
    let through = angles_to_rotation([35.0, 91.0, 62.0]);
    let shown = angles_near(through, [35.0, 89.0, 62.0]);
    assert!((shown[1] - 91.0).abs() < 0.001);
}

#[test]
fn sparse_channels_hold_and_use_the_outgoing_interpolation() {
    let mut clip = doc();
    let kind = Kind::Scale;
    clip.set_key(0, kind, 2, [1.0, 0.0, 0.0, 0.0], Some(Interp::Step))
        .unwrap();
    clip.set_key(0, kind, 10, [9.0, 0.0, 0.0, 0.0], None)
        .unwrap();
    assert_eq!(clip.sample(0.0).nodes[0].scale, 1.0);
    assert_eq!(clip.sample(7.0).nodes[0].scale, 1.0);
    assert_eq!(clip.sample(10.0).nodes[0].scale, 9.0);
    clip.set_key(0, kind, 2, [1.0, 0.0, 0.0, 0.0], Some(Interp::Linear))
        .unwrap();
    assert_eq!(clip.sample(4.0).nodes[0].scale, 3.0);
    clip.set_key(0, kind, 2, [1.0, 0.0, 0.0, 0.0], Some(Interp::Smooth))
        .unwrap();
    assert_eq!(clip.sample(4.0).nodes[0].scale, 2.25);
    clip.set_tangent(0, kind, 2, [1.0, 0.0, 0.0, 0.0], None)
        .unwrap();
    assert_eq!(clip.sample(4.0).nodes[0].scale, 3.375);
    assert!(!clip.sample(4.0).nodes[0].has_rotation);
}

#[test]
fn a_failed_bulk_edit_is_atomic_and_duplicate_has_its_own_history() {
    let mut clip = doc();
    let kind = Kind::Translation;
    assert!(!clip.remove_key(0, kind, 1).unwrap());
    assert!(clip.set_key(0, Kind::Rotation, 1, [0.0; 4], None).is_err());
    clip.set_key(0, kind, 1, [1.0, 0.0, 0.0, 0.0], None)
        .unwrap();
    let mut other = clip.duplicate("copy".to_owned()).unwrap();
    let key = other.key(0, kind, 1).unwrap();
    assert!(other.replace_keys(0, kind, vec![key, key], None).is_err());
    assert_eq!(other.revision(), 0);
    assert!(!other.undo());
    assert!(other.remove_key(0, kind, 1).unwrap());
    assert_eq!(clip.key(0, kind, 1), Some(key));
    assert!(other.undo());
    assert_eq!(other.key(0, kind, 1), Some(key));
}

#[test]
fn check_measures_codec_rounding_ground_and_seam() {
    let mut clip = doc();
    let v = [0.0, 0.0, -1.0, 0.0];
    let q = angles_to_rotation([31.0, 10.0, 13.0]);
    clip.set_key(0, Kind::Rotation, 0, q, None).unwrap();
    clip.set_key(0, Kind::Translation, 10, v, None).unwrap();
    clip.set_key(0, Kind::Translation, 0, [0.0; 4], Some(Interp::Step))
        .unwrap();
    let rest = [NodeDefault {
        parent: -1,
        rotation: [0.0, 0.0, 0.0, 1.0],
        translation: [0.0, 0.0, 0.5],
        scale: 1.0,
    }];
    let check = clip.check(Some(&rest), true).unwrap();
    assert!(check.feet_checked);
    assert_eq!(
        check.feet_below_ground,
        vec![FootContact {
            node: 0,
            frame: 10,
            z: -0.5
        }]
    );
    assert!(check.nodes[0].rotation > 0.0 && check.nodes[0].rotation < 0.0001);
    assert_eq!(check.nodes[0].translation, 0.0);
    assert_eq!(check.loop_seam[0].translation, 1.0);
    assert!(!clip.check(None, false).unwrap().feet_checked);
}
