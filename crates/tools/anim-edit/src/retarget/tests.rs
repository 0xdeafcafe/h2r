use super::*;

fn node(name: &str, parent: i16) -> Node {
    Node {
        name: Some(name.to_owned()),
        parent,
    }
}

fn rest(parent: i16, translation: [f32; 3]) -> NodeDefault {
    NodeDefault {
        parent,
        rotation: [0.0, 0.0, 0.0, 1.0],
        translation,
        scale: 1.0,
    }
}

fn turn_z(degrees: f32) -> [f32; 4] {
    let half = degrees.to_radians() / 2.0;
    [0.0, 0.0, half.sin(), half.cos()]
}

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let p = hamilton(hamilton(q, [v[0], v[1], v[2], 0.0]), [-q[0], -q[1], -q[2], q[3]]);
    [p[0], p[1], p[2]]
}

/// root -> arm -> hand, one unit apart; the arm turns 90 degrees about z
/// and the hand slides out half a unit along its own bone.
fn source() -> (AnimDoc, Vec<NodeDefault>) {
    let nodes = vec![node("root", -1), node("arm", 0), node("hand", 1)];
    let mut clip = AnimDoc::new("reach".into(), 5, nodes).unwrap();
    clip.set_key(1, Kind::Rotation, 4, turn_z(90.0), None).unwrap();
    clip.set_key(2, Kind::Translation, 4, [1.5, 0.0, 0.0, 0.0], None).unwrap();
    let rest = vec![rest(-1, [0.0; 3]), rest(0, [1.0, 0.0, 0.0]), rest(1, [1.0, 0.0, 0.0])];
    (clip, rest)
}

/// The hand's position in the root frame at `frame`.
fn hand(clip: &AnimDoc, rest: &[NodeDefault], frame: f32) -> [f32; 3] {
    let pose = clip.sample(frame);
    let local = |n: usize| {
        let t = &pose.nodes[n];
        (
            if t.has_rotation { t.rotation } else { rest[n].rotation },
            if t.has_translation { t.translation } else { rest[n].translation },
        )
    };
    let (arm_rotation, arm_offset) = local(1);
    let (_, hand_offset) = local(2);
    let turned = rotate(arm_rotation, hand_offset);
    std::array::from_fn(|i| arm_offset[i] + turned[i])
}

#[test]
fn a_renamed_node_and_a_longer_bone_keep_the_end_position_in_proportion() {
    let (clip, source_rest) = source();
    // The arm is renamed and the hand bone is twice as long.
    let target = vec![node("root", -1), node("upper_arm", 0), node("hand", 1)];
    let target_rest = vec![rest(-1, [0.0; 3]), rest(0, [1.0, 0.0, 0.0]), rest(1, [2.0, 0.0, 0.0])];
    let overrides = [NodeMapping { source: 1, target: Some(1) }];
    let (out, report) = retarget(
        &clip,
        &source_rest,
        &target,
        &target_rest,
        &overrides,
        "reach copy".into(),
        ClipKind::Base,
    )
    .unwrap();
    assert!(report.unmatched_source.is_empty());
    assert!(report.unmatched_target.is_empty());
    assert_eq!(out.nodes(), target.as_slice());
    let want = hand(&clip, &source_rest, 4.0);
    let got = hand(&out, &target_rest, 4.0);
    // The hand reaches the same direction, its bone-relative reach scaled
    // by the length ratio: (1, 1.5) becomes (1, 3).
    assert!((got[0] - want[0]).abs() < 1e-5, "{got:?} {want:?}");
    assert!((got[1] - want[1] * 2.0).abs() < 1e-5, "{got:?} {want:?}");
    // Key times and interpolation are untouched.
    assert_eq!(out.keys(2, Kind::Translation)[0].frame, 4);
    assert_eq!(out.keys(2, Kind::Translation)[0].value[0], 3.0);
}

#[test]
fn an_unmatched_node_is_listed_with_its_dropped_channels() {
    let (clip, source_rest) = source();
    let target = vec![node("root", -1), node("arm", 0)];
    let target_rest = vec![rest(-1, [0.0; 3]), rest(0, [1.0, 0.0, 0.0])];
    let (out, report) = retarget(
        &clip, &source_rest, &target, &target_rest, &[], "partial".into(), ClipKind::Base,
    )
    .unwrap();
    assert_eq!(report.unmatched_source, [2]);
    assert!(report.unmatched_target.is_empty());
    assert_eq!(report.dropped_channels, [DroppedChannel { node: 2, kind: Kind::Translation }]);
    assert_eq!(out.channels().count(), 1);
}

#[test]
fn a_rest_pose_rotation_difference_moves_the_clip_onto_the_new_rest() {
    let nodes = vec![node("a", -1)];
    let mut clip = AnimDoc::new("hold".into(), 2, nodes.clone()).unwrap();
    clip.set_key(0, Kind::Rotation, 1, [0.0, 0.0, 0.0, 1.0], None).unwrap();
    let source_rest = [rest(-1, [0.0; 3])];
    let mut target_rest = [rest(-1, [0.0; 3])];
    target_rest[0].rotation = turn_z(90.0);
    let (out, _) = retarget(
        &clip, &source_rest, &nodes, &target_rest, &[], "held".into(), ClipKind::Base,
    )
    .unwrap();
    // A key equal to the source rest is the target rest.
    let key = out.keys(0, Kind::Rotation)[0].value;
    for (a, b) in key.iter().zip(turn_z(90.0)) {
        assert!((a - b).abs() < 1e-6);
    }
}

#[test]
fn manual_pairs_win_and_misuse_is_refused() {
    let (clip, _) = source();
    let target = vec![node("root", -1), node("upper_arm", 0), node("hand", 1)];
    let report = retarget_report(&clip, &target, &[NodeMapping { source: 1, target: Some(1) }]).unwrap();
    assert_eq!(report.nodes[1].target, Some(1));
    let dropped = retarget_report(&clip, &target, &[NodeMapping { source: 2, target: None }]).unwrap();
    assert_eq!(dropped.unmatched_source, [1, 2]);
    assert_eq!(dropped.unmatched_target, [1, 2]);
    assert!(retarget_report(&clip, &target, &[NodeMapping { source: 9, target: Some(0) }]).is_err());
    let twice = [
        NodeMapping { source: 0, target: Some(0) },
        NodeMapping { source: 1, target: Some(0) },
    ];
    assert!(retarget_report(&clip, &target, &twice).is_err());
}

#[test]
fn an_overlay_scales_only_its_translation() {
    let nodes = vec![node("root", -1), node("arm", 0)];
    let mut clip = AnimDoc::new("add".into(), 2, nodes.clone()).unwrap();
    clip.set_key(1, Kind::Translation, 1, [0.5, 0.0, 0.0, 0.0], None).unwrap();
    clip.set_key(1, Kind::Rotation, 1, turn_z(30.0), None).unwrap();
    let source_rest = [rest(-1, [0.0; 3]), rest(0, [1.0, 0.0, 0.0])];
    let target_rest = [rest(-1, [0.0; 3]), rest(0, [3.0, 0.0, 0.0])];
    let (out, _) = retarget(
        &clip, &source_rest, &nodes, &target_rest, &[], "add copy".into(), ClipKind::Overlay,
    )
    .unwrap();
    assert_eq!(out.keys(1, Kind::Translation)[0].value[0], 1.5);
    assert_eq!(out.keys(1, Kind::Rotation)[0].value, clip.keys(1, Kind::Rotation)[0].value);
}
