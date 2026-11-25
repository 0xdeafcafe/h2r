//! The key reduction on invented clips: no game data.
use crate::anim_keys::*;
use crate::anim_sample::{NodeTransform, Pose};

/// A turn of `degrees` about z.
fn turn(degrees: f32) -> [f32; 4] {
    let h = degrees.to_radians() * 0.5;
    [0.0, 0.0, h.sin(), h.cos()]
}

/// A two-bone clip: the body holds still (rotation, translation and
/// scale all supplied), the arm turns by `angles` degrees about z while
/// it slides along x by `slide` and keeps its scale.
fn two_bone(angles: &[f32], slide: &[f32]) -> Vec<Pose> {
    angles
        .iter()
        .zip(slide)
        .map(|(a, s)| {
            let still = NodeTransform {
                has_rotation: true,
                has_translation: true,
                has_scale: true,
                ..NodeTransform::default()
            };
            let arm = NodeTransform {
                rotation: turn(*a),
                translation: [*s, 0.0, 0.0],
                scale: 1.0,
                has_rotation: true,
                has_translation: true,
                has_scale: true,
            };
            Pose {
                nodes: vec![still, arm],
            }
        })
        .collect()
}

fn channel(keys: &Keys, node: usize, kind: Kind) -> &Channel {
    keys.channels
        .iter()
        .find(|c| c.node == node && c.kind == kind)
        .expect("the channel")
}

fn frames_of(c: &Channel) -> Vec<usize> {
    c.keys.iter().map(|k| k.frame).collect()
}

#[test]
fn a_two_bone_clip_lists_its_channels_and_drops_the_frames_on_a_line() {
    // 0, 45, 90, 90, 45, 0 degrees: frames 1 and 4 lie on the line
    // between their neighbours, so four keys carry six frames.
    let poses = two_bone(
        &[0.0, 45.0, 90.0, 90.0, 45.0, 0.0],
        &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0],
    );
    let keys = reduce(&poses, Tolerance::DEFAULT);
    assert_eq!((keys.frames, keys.nodes), (6, 2));
    // Six channels: every node's three.
    assert_eq!(keys.channels.len(), 6);
    assert_eq!(frames_of(channel(&keys, 1, Kind::Rotation)), [0, 2, 3, 5]);
    // The slide is one straight line: its two ends.
    assert_eq!(frames_of(channel(&keys, 1, Kind::Translation)), [0, 5]);
    // What does not move is one key, the body's all three and the arm's
    // scale.
    for (node, kind) in [
        (0, Kind::Rotation),
        (0, Kind::Translation),
        (0, Kind::Scale),
        (1, Kind::Scale),
    ] {
        let c = channel(&keys, node, kind);
        assert_eq!(frames_of(c), [0], "{node} {kind:?}");
        assert!(!c.moves());
    }
    assert!(channel(&keys, 1, Kind::Rotation).moves());
    assert_eq!(keys.key_count(), 4 + 2 + 4);
}

#[test]
fn reduce_then_bake_is_within_the_tolerance_at_every_frame() {
    // A wavy turn and a wavy slide that need many keys, at two tolerances.
    let angles: Vec<f32> = (0..60)
        .map(|f| 70.0 * (f as f32 * 0.31).sin() + 5.0 * (f as f32 * 1.7).sin())
        .collect();
    let slide: Vec<f32> = (0..60)
        .map(|f| 2.0 * (f as f32 * 0.2).cos() + 0.05 * (f as f32 * 2.9).sin())
        .collect();
    let poses = two_bone(&angles, &slide);
    let mut counts = Vec::new();
    for scale in [1.0f32, 10.0] {
        let tolerance = Tolerance {
            rotation: Tolerance::DEFAULT.rotation * scale,
            translation: Tolerance::DEFAULT.translation * scale,
            scale: Tolerance::DEFAULT.scale * scale,
        };
        let keys = reduce(&poses, tolerance);
        let baked = keys.bake();
        assert_eq!(baked.len(), poses.len());
        for (f, (a, b)) in poses.iter().zip(&baked).enumerate() {
            for (n, (x, y)) in a.nodes.iter().zip(&b.nodes).enumerate() {
                let q: f32 = x.rotation.iter().zip(&y.rotation).map(|(p, q)| p * q).sum();
                let angle = 2.0 * q.abs().min(1.0).acos();
                // acos of a dot is only good to ~1e-3 near 1: bound it
                // through the exact distance too.
                assert!(angle <= tolerance.rotation + 2e-3, "frame {f} node {n}");
                let d: f32 = (0..3)
                    .map(|k| (x.translation[k] - y.translation[k]).powi(2))
                    .sum::<f32>()
                    .sqrt();
                assert!(
                    d <= tolerance.translation * 1.0001 + 1e-6,
                    "frame {f} node {n}"
                );
                assert!((x.scale - y.scale).abs() <= tolerance.scale);
                assert!(y.has_rotation && y.has_translation && y.has_scale);
            }
        }
        let worst = keys.max_error(&poses);
        assert!(worst[0] <= tolerance.rotation * 1.001, "{worst:?}");
        assert!(worst[1] <= tolerance.translation * 1.001, "{worst:?}");
        assert!(worst[2] <= tolerance.scale, "{worst:?}");
        counts.push(keys.key_count());
    }
    // A looser tolerance needs no more keys, and the tight one keeps
    // many.
    assert!(counts[1] <= counts[0], "{counts:?}");
    assert!(counts[0] > 20, "{counts:?}");
}

#[test]
fn the_ends_are_always_keys_and_baking_reproduces_a_keyed_frame_exactly() {
    let poses = two_bone(&[0.0, 90.0, 0.0], &[0.0, 0.0, 0.0]);
    let keys = reduce(&poses, Tolerance::DEFAULT);
    let rotation = channel(&keys, 1, Kind::Rotation);
    assert_eq!(frames_of(rotation), [0, 1, 2]);
    let baked = keys.bake();
    for (a, b) in poses.iter().zip(&baked) {
        assert_eq!(a.nodes[1].rotation, b.nodes[1].rotation);
    }
    // Before the first key and after the last the value holds.
    assert_eq!(rotation.value_at(-3.0), rotation.keys[0].value);
    assert_eq!(rotation.value_at(9.0), rotation.keys[2].value);
}

#[test]
fn a_flipped_sign_is_the_same_turn() {
    // The same orientation written as q then -q reads as no change.
    let mut poses = two_bone(&[30.0, 30.0, 30.0, 30.0], &[0.0; 4]);
    for p in poses.iter_mut().skip(1).step_by(2) {
        p.nodes[1].rotation = p.nodes[1].rotation.map(|x| -x);
    }
    let keys = reduce(&poses, Tolerance::DEFAULT);
    assert_eq!(frames_of(channel(&keys, 1, Kind::Rotation)), [0]);
    // A turn through the flip is the turn without it: the same keys.
    let plain = two_bone(&[0.0, 20.0, 40.0, 60.0, 80.0], &[0.0; 5]);
    let mut flipped = plain.clone();
    flipped[3].nodes[1].rotation = flipped[3].nodes[1].rotation.map(|x| -x);
    let want = frames_of(channel(
        &reduce(&plain, Tolerance::DEFAULT),
        1,
        Kind::Rotation,
    ))
    .to_vec();
    let keys = reduce(&flipped, Tolerance::DEFAULT);
    assert_eq!(frames_of(channel(&keys, 1, Kind::Rotation)), want);
    assert!(keys.max_error(&flipped)[0] <= Tolerance::DEFAULT.rotation * 1.001);
}

#[test]
fn channels_the_clip_does_not_drive_have_no_keys_and_stay_unsupplied() {
    let mut poses = two_bone(&[0.0, 10.0, 20.0], &[0.0; 3]);
    for p in &mut poses {
        p.nodes[0].has_translation = false;
        p.nodes[0].has_scale = false;
        p.nodes[1].has_scale = false;
    }
    let keys = reduce(&poses, Tolerance::DEFAULT);
    assert!(
        keys.channels
            .iter()
            .all(|c| !(c.node == 0 && c.kind != Kind::Rotation)
                && !(c.node == 1 && c.kind == Kind::Scale))
    );
    let baked = keys.bake();
    assert!(!baked[1].nodes[0].has_translation && !baked[1].nodes[1].has_scale);
    assert!(baked[1].nodes[1].has_rotation);
}

#[test]
fn one_frame_and_no_frames() {
    let one = two_bone(&[12.0], &[3.0]);
    let keys = reduce(&one, Tolerance::DEFAULT);
    assert_eq!(keys.key_count(), 6);
    assert!(keys.channels.iter().all(|c| frames_of(c) == [0]));
    let none = reduce(&[], Tolerance::DEFAULT);
    assert_eq!((none.frames, none.nodes, none.key_count()), (0, 0, 0));
    assert!(none.bake().is_empty());
}

#[test]
fn a_looser_tolerance_keeps_fewer_keys_and_a_zero_one_keeps_every_change() {
    let angles: Vec<f32> = (0..40).map(|f| (f as f32 * f as f32) * 0.05).collect();
    let poses = two_bone(&angles, &[0.0; 40]);
    let loose = reduce(
        &poses,
        Tolerance {
            rotation: 5.0_f32.to_radians(),
            ..Tolerance::DEFAULT
        },
    );
    let tight = reduce(
        &poses,
        Tolerance {
            rotation: 0.0,
            ..Tolerance::DEFAULT
        },
    );
    let n = |k: &Keys| channel(k, 1, Kind::Rotation).keys.len();
    assert!(n(&loose) < n(&tight));
    // Zero tolerance bakes back to the very frames (within float noise).
    assert!(tight.max_error(&poses)[0] < 1e-3);
}
