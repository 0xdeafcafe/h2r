use super::*;
use tags::anim_keys;

fn node(name: &str, parent: i16) -> Node {
    Node {
        name: Some(name.into()),
        parent,
    }
}

fn rest(parent: i16, rotation: [f32; 4], x: f32) -> NodeDefault {
    NodeDefault {
        parent,
        rotation,
        translation: [x, 0.0, 0.0],
        scale: 1.0,
    }
}

const IDENTITY: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

fn about_z(degrees: f32) -> [f32; 4] {
    let half = degrees.to_radians() / 2.0;
    [0.0, 0.0, half.sin(), half.cos()]
}

fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let p = mul(mul(q, [v[0], v[1], v[2], 0.0]), conjugate(q));
    [p[0], p[1], p[2]]
}

/// The world position of the last node of a three-node chain at `frame`.
fn end_position(doc: &AnimDoc, rest: &[NodeDefault], frame: f32) -> [f32; 3] {
    let pose = doc.sample(frame);
    let mut rotation = IDENTITY;
    let mut at = [0.0; 3];
    for (n, d) in pose.nodes.iter().zip(rest) {
        let t = if n.has_translation {
            n.translation
        } else {
            d.translation
        };
        let r = if n.has_rotation {
            n.rotation
        } else {
            d.rotation
        };
        let local = rotate(rotation, t);
        at = [at[0] + local[0], at[1] + local[1], at[2] + local[2]];
        rotation = mul(rotation, r);
    }
    at
}

fn source() -> (AnimDoc, Vec<NodeDefault>) {
    let nodes = vec![
        node("root", -1),
        node("upper", 0),
        node("hand", 1),
        node("tail", 0),
    ];
    let rests = vec![
        rest(-1, IDENTITY, 0.0),
        rest(0, IDENTITY, 1.0),
        rest(1, IDENTITY, 1.0),
        rest(0, IDENTITY, 1.0),
    ];
    let mut doc = AnimDoc::new("wave".into(), 11, nodes).unwrap();
    doc.set_key(0, Kind::Rotation, 5, about_z(90.0), None)
        .unwrap();
    doc.set_key(3, Kind::Rotation, 5, about_z(10.0), None)
        .unwrap();
    doc.set_key(3, Kind::Scale, 0, [2.0, 0.0, 0.0, 0.0], None)
        .unwrap();
    (doc, rests)
}

fn target() -> (Vec<Node>, Vec<NodeDefault>) {
    (
        vec![node("root", -1), node("upper_arm", 0), node("hand", 1)],
        vec![
            rest(-1, IDENTITY, 0.0),
            rest(0, IDENTITY, 1.0),
            rest(1, IDENTITY, 2.0),
        ],
    )
}

#[test]
fn a_renamed_and_a_longer_bone_keep_the_end_where_the_rotation_puts_it() {
    let (doc, source_rest) = source();
    let (nodes, target_rest) = target();
    let (out, report) = retarget(
        &doc,
        &source_rest,
        &nodes,
        &target_rest,
        "wave".into(),
        &[(1, 1)],
    )
    .unwrap();
    assert_eq!(report.matched, vec![(0, 0), (1, 1), (2, 2)]);
    let at = end_position(&out, &target_rest, 5.0);
    // The arm of length 1 + 2 turned a quarter about z.
    assert!(
        (at[0]).abs() < 1e-5 && (at[1] - 3.0).abs() < 1e-5 && at[2].abs() < 1e-5,
        "{at:?}"
    );
}

#[test]
fn unmatched_nodes_are_listed_and_their_channels_dropped() {
    let (doc, source_rest) = source();
    let (nodes, target_rest) = target();
    let (out, report) =
        retarget(&doc, &source_rest, &nodes, &target_rest, "wave".into(), &[]).unwrap();
    // Without the table "upper" has no "upper_arm".
    assert_eq!(report.matched, vec![(0, 0), (2, 2)]);
    assert_eq!(report.unmatched_targets, vec![1]);
    assert_eq!(report.dropped_nodes, vec![3]);
    assert_eq!(report.dropped_channels, 2);
    assert_eq!(out.channels().count(), 1);
    assert_eq!(out.revision(), 0);
    assert!(!out.can_undo());
}

#[test]
fn a_rotation_carries_as_its_delta_from_each_rest() {
    let (mut doc, mut source_rest) = source();
    source_rest[1].rotation = about_z(30.0);
    doc.set_key(1, Kind::Rotation, 2, about_z(30.0), None)
        .unwrap();
    doc.set_key(1, Kind::Rotation, 8, about_z(50.0), None)
        .unwrap();
    let (nodes, mut target_rest) = target();
    target_rest[1].rotation = about_z(90.0);
    let (out, _) = retarget(
        &doc,
        &source_rest,
        &nodes,
        &target_rest,
        "wave".into(),
        &[(1, 1)],
    )
    .unwrap();
    let keys = out.keys(1, Kind::Rotation);
    // The key at the source's rest stays at the target's rest, and the
    // 20 degree turn past it carries over.
    assert!(anim_keys::distance(Kind::Rotation, keys[0].value, about_z(90.0)) < 1e-5);
    assert!(anim_keys::distance(Kind::Rotation, keys[1].value, about_z(110.0)) < 1e-5);
}

#[test]
fn a_translation_scales_by_the_bone_length_and_a_scale_carries() {
    let (mut doc, source_rest) = source();
    doc.set_key(2, Kind::Translation, 0, [1.5, 0.0, 0.0, 0.0], None)
        .unwrap();
    let (nodes, target_rest) = target();
    let (out, report) = retarget(
        &doc,
        &source_rest,
        &nodes,
        &target_rest,
        "wave".into(),
        &[(1, 1)],
    )
    .unwrap();
    // 0.5 past the source's rest of 1, twice as long a bone, from rest 2.
    assert_eq!(
        out.keys(2, Kind::Translation)[0].value,
        [3.0, 0.0, 0.0, 0.0]
    );
    assert_eq!(report.scaled, vec![(2, 2.0)]);
}

#[test]
fn a_table_that_pairs_a_node_twice_or_off_the_skeleton_is_refused() {
    let (doc, source_rest) = source();
    let (nodes, target_rest) = target();
    for table in [vec![(1, 1), (2, 1)], vec![(9, 1)], vec![(1, 9)]] {
        let r = retarget(&doc, &source_rest, &nodes, &target_rest, "x".into(), &table);
        assert!(r.is_err(), "{table:?}");
    }
    let r = retarget(
        &doc,
        &source_rest[..2],
        &nodes,
        &target_rest,
        "x".into(),
        &[],
    );
    assert!(r.is_err());
}
