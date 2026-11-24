//! Encoder tests: invented clips are encoded, decoded by `anim_sample` and
//! compared within the i16 quantisation. No game data.
use crate::anim::*;
use crate::anim_sample::*;
use crate::anim_write::*;
use crate::wire::StringId;

const NODES: usize = 3;
/// One i16 step of a unit quaternion component, with slack for the decoder's
/// renormalisation.
const Q_TOL: f32 = 2.0 / 32767.0;

fn animation(frames: i16, static_size: usize, animated_size: usize, static_flags: u8) -> Animation {
    Animation {
        name: StringId(0),
        node_list_checksum: 0,
        checksums: [0; 2],
        anim_type: AnimationType::Base,
        frame_info_type: 0,
        unknown_12: 0,
        node_count: NODES as u8,
        frame_count: frames,
        flags: 0x10,
        unknown_18: [0; 8],
        unknown_20: 0,
        resource_index: 0,
        data_offset: 0,
        static_pose: -1,
        unknown_2e: 0,
        link_a: -1,
        link_b: -1,
        inline_size: 0,
        inline_address: 0,
        sizes: DataSizes {
            static_flags,
            animated_flags: 12,
            unknown: [0; 2],
            unknown_4: 0,
            static_size: static_size as u16,
            unknown_8: 0,
            animated_size: animated_size as u32,
        },
        frame_events: vec![],
        sound_events: vec![],
        effect_events: vec![],
        extra_block_count: 0,
    }
}

fn unit(q: [f32; 4]) -> [f32; 4] {
    let l = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    q.map(|x| x / l)
}

fn pose(seed: f32) -> Vec<Transform> {
    (0..NODES)
        .map(|n| {
            let s = seed + n as f32;
            Transform {
                rotation: unit([0.1 * s, 0.3 - 0.05 * s, 0.2, 0.9]),
                translation: [s, -2.0 * s, 0.5],
                scale: 1.0 + 0.25 * s,
            }
        })
        .collect()
}

fn clip(frames: &[Vec<Transform>], animated: Channels, with_static: bool) -> AnimationClip {
    let all = (1u64 << NODES) - 1;
    let defaults: Channels = animated.map(|m| !m & all);
    let stat = if with_static {
        static_stream(&frames[0], defaults)
    } else {
        Vec::new()
    };
    let anim = animated_stream(frames, animated, false);
    let mut blob = stat.clone();
    blob.extend(&anim);
    if with_static {
        blob.extend(flag_set(defaults, NODES));
    }
    blob.extend(flag_set(animated, NODES));
    let sf = if with_static { 12 } else { 0 };
    AnimationClip::new(
        &animation(frames.len() as i16, stat.len(), anim.len(), sf),
        blob,
    )
    .unwrap()
}

fn near(a: &[f32], b: &[f32], tol: f32) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() <= tol)
}

#[test]
fn codec_3_round_trips_every_channel_within_quantisation() {
    let frames = vec![pose(0.0), pose(0.5), pose(1.0)];
    let c = clip(&frames, [0b111; 3], false);
    for (k, want) in frames.iter().enumerate() {
        let got = c.sample_frame(k).unwrap();
        for (n, w) in want.iter().enumerate() {
            let g = &got.nodes[n];
            assert!(g.has_rotation && g.has_translation && g.has_scale);
            assert!(near(&g.rotation, &w.rotation, Q_TOL), "node {n} frame {k}");
            assert_eq!(g.translation, w.translation);
            assert_eq!(g.scale, w.scale);
        }
    }
}

#[test]
fn codec_1_carries_the_channels_codec_3_does_not_animate() {
    // Node 1 rotates and node 2 translates; every other channel is static.
    let frames = vec![pose(0.0), pose(1.0)];
    let animated: Channels = [0b010, 0b100, 0];
    let c = clip(&frames, animated, true);
    let got = c.sample_frame(0).unwrap();
    for n in 0..NODES {
        let w = &frames[0][n];
        let g = &got.nodes[n];
        assert!(near(&g.rotation, &w.rotation, Q_TOL), "node {n}");
        assert_eq!(g.translation, w.translation, "node {n}");
        assert_eq!(g.scale, w.scale, "node {n}");
    }
    let last = c.sample_frame(1).unwrap();
    assert!(near(&last.nodes[1].rotation, &frames[1][1].rotation, Q_TOL));
    assert_eq!(last.nodes[2].translation, frames[1][2].translation);
    // A static channel keeps frame 0's value.
    assert_eq!(last.nodes[0].translation, frames[0][0].translation);
}

#[test]
fn the_stream_headers_count_their_channels() {
    let s = static_stream(&pose(0.0), [0b101, 0b010, 0b100]);
    assert_eq!(s[0], CODEC_STATIC);
    assert_eq!(&s[1..4], &[2, 1, 1]);
    let a = animated_stream(&[pose(0.0), pose(1.0)], [0b111, 0, 0b001], false);
    assert_eq!(a[0], CODEC_QUANTIZED_ROTATION);
    assert_eq!(&a[1..4], &[3, 0, 1]);
    let screen = animated_stream(&[pose(0.0)], [0b001, 0, 0], true);
    assert_eq!(screen[0], CODEC_BLEND_SCREEN);
}

#[test]
fn flag_sets_hold_one_array_per_channel() {
    let f = flag_set([0b101, 0b010, 0b100], NODES);
    assert_eq!(f, vec![0b101, 0, 0, 0, 0b010, 0, 0, 0, 0b100, 0, 0, 0]);
}
