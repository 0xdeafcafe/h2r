//! Synthetic animation tests: every byte below is invented, no game data.
use crate::anim::*;
use crate::anim_sample::*;
use crate::wire::StringId;

const BASE: u32 = 0x8000_0000;
const SQRT_HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}
fn close_all(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| close(*x, *y))
}

// ---- graph tag ------------------------------------------------------------

struct Tag {
    bytes: Vec<u8>,
}
impl Tag {
    fn new() -> Self {
        Self {
            bytes: vec![0; HEADER_SIZE],
        }
    }
    fn put32(&mut self, off: usize, v: u32) {
        self.bytes[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    /// Append element bytes and point the (count, address) pair at `owner_off` at them.
    fn block(&mut self, owner_off: usize, count: u32, body: &[u8]) -> usize {
        let at = self.bytes.len();
        self.bytes.extend_from_slice(body);
        self.put32(owner_off, count);
        self.put32(owner_off + 4, BASE + at as u32);
        at
    }
}
fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}
fn le16(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}

fn node(name: u32, next: i16, child: i16, parent: i16) -> Vec<u8> {
    let mut n = Vec::new();
    n.extend(le32(name));
    n.extend(next.to_le_bytes());
    n.extend(child.to_le_bytes());
    n.extend(parent.to_le_bytes());
    n.extend(le16(1)); // model flags
    n.extend(le32(0)); // joint flags
    for f in [0.0f32, 1.0, 2.0, 0.5] {
        n.extend(f.to_le_bytes());
    }
    assert_eq!(n.len(), NODE_SIZE);
    n
}

fn animation_bytes(tag: &mut Tag, sizes: (u8, u8, u16, u32), events: bool) -> Vec<u8> {
    let mut a = vec![0u8; ANIMATION_SIZE];
    a[0..4].copy_from_slice(&le32(0x0400_0042)); // name
    a[0x10] = 0; // base
    a[0x13] = 3; // nodes
    a[0x14..0x16].copy_from_slice(&4i16.to_le_bytes()); // frames
    a[0x16..0x18].copy_from_slice(&le16(0x10));
    a[0x24..0x28].copy_from_slice(&le32(0)); // resource 0
    a[0x28..0x2c].copy_from_slice(&le32(0x40)); // offset
    a[0x2c..0x2e].copy_from_slice(&(-1i16).to_le_bytes());
    a[0x30..0x34].copy_from_slice(&[0xff; 4]);
    a[0x3c] = sizes.0;
    a[0x3d] = sizes.1;
    a[0x42..0x44].copy_from_slice(&le16(sizes.2));
    a[0x48..0x4c].copy_from_slice(&le32(sizes.3));
    if events {
        let ev = [le16(1), le16(2)].concat(); // type 1 at frame 2
        let at = tag.bytes.len();
        tag.bytes.extend(&ev);
        a[0x4c..0x50].copy_from_slice(&le32(1));
        a[0x50..0x54].copy_from_slice(&le32(BASE + at as u32));
    }
    a
}

/// One node block, one animation (with a frame event), one mode -> class -> type
/// -> action chain, one resource and one static pose.
fn sample_graph() -> Vec<u8> {
    let mut t = Tag::new();
    t.put32(0, u32::from_le_bytes(*b"damj"));
    t.put32(4, 0xe123_0001);
    t.put32(8, 0x0006_3500);
    let nodes = [
        node(0x0500_0001, -1, 1, -1),
        node(0x0500_0002, 2, -1, 0),
        node(0x0500_0003, -1, -1, 0),
    ]
    .concat();
    t.block(0x0c, 3, &nodes);
    let anim = animation_bytes(&mut t, (0x18, 0x18, 0x40, 0x100), true);
    t.block(0x2c, 1, &anim);
    // action -> type -> class -> mode, deepest first so addresses exist
    let mut action = Vec::new();
    action.extend(le32(0x0900_0015));
    action.extend((-1i16).to_le_bytes());
    action.extend(0i16.to_le_bytes());
    let at = t.bytes.len();
    t.bytes.extend(&action);
    let mut ty = vec![0u8; TYPE_SIZE];
    ty[0..4].copy_from_slice(&le32(0x0700_0001));
    ty[4..8].copy_from_slice(&le32(1));
    ty[8..12].copy_from_slice(&le32(BASE + at as u32));
    let overlay = t.bytes.len();
    t.bytes.extend(le32(0x0800_0080));
    t.bytes.extend((-1i16).to_le_bytes());
    t.bytes.extend(0i16.to_le_bytes());
    ty[0xc..0x10].copy_from_slice(&le32(1));
    ty[0x10..0x14].copy_from_slice(&le32(BASE + overlay as u32));
    // preload list a: two ints
    let pre = t.bytes.len();
    t.bytes.extend([le32(0), le32(7)].concat());
    ty[0x24..0x28].copy_from_slice(&le32(2));
    ty[0x28..0x2c].copy_from_slice(&le32(BASE + pre as u32));
    let at = t.bytes.len();
    t.bytes.extend(&ty);
    let mut class = vec![0u8; CLASS_SIZE];
    class[0..4].copy_from_slice(&le32(0x0600_0001));
    class[4..8].copy_from_slice(&le32(1));
    class[8..12].copy_from_slice(&le32(BASE + at as u32));
    let at = t.bytes.len();
    t.bytes.extend(&class);
    let mut mode = vec![0u8; MODE_SIZE];
    mode[0..4].copy_from_slice(&le32(0x0200_0001));
    mode[4..8].copy_from_slice(&le32(1));
    mode[8..12].copy_from_slice(&le32(BASE + at as u32));
    t.block(0x34, 1, &mode);
    let mut res = Vec::new();
    res.extend(le32(0xe123_0001));
    res.extend(le32(0x200));
    res.extend(le32(0x4000_1000)); // mainmenu.map selector
    res.extend(le32(u32::MAX));
    res.extend(le32(1));
    t.block(0xac, 1, &res);
    let mut pose = Vec::new();
    for v in [0i16, 0, 0, 100] {
        pose.extend(v.to_le_bytes());
    }
    for f in [1.0f32, 2.0, 3.0, 1.5] {
        pose.extend(f.to_le_bytes());
    }
    t.block(0xb4, 1, &pose);
    t.bytes
}

#[test]
fn graph_decodes_every_block() {
    let g = AnimationGraph::decode(&sample_graph(), BASE).unwrap();
    assert_eq!(g.parent.group.0, *b"jmad");
    assert_eq!(g.version_word, 0x0006_3500);
    assert_eq!(g.nodes.len(), 3);
    assert_eq!(
        (
            g.nodes[0].first_child,
            g.nodes[1].next_sibling,
            g.nodes[2].parent
        ),
        (1, 2, 0)
    );
    assert_eq!(g.nodes[0].vector_range, 0.5);
    assert_eq!(g.nodes[0].base_vector, [0.0, 1.0, 2.0]);
    assert_eq!(g.animations.len(), 1);
    let a = &g.animations[0];
    assert_eq!(
        (a.node_count, a.frame_count, a.anim_type),
        (3, 4, AnimationType::Base)
    );
    assert!(a.in_resource());
    assert_eq!(
        (
            a.sizes.static_flags,
            a.sizes.static_size,
            a.sizes.animated_size
        ),
        (0x18, 0x40, 0x100)
    );
    assert_eq!(
        a.frame_events,
        [FrameEvent {
            event_type: 1,
            frame: 2
        }]
    );
    assert_eq!(g.modes.len(), 1);
    let ty = &g.modes[0].classes[0].types[0];
    assert_eq!(
        ty.actions,
        [Action {
            name: StringId(0x0900_0015),
            graph_index: -1,
            animation_index: 0
        }]
    );
    assert_eq!(
        ty.overlays,
        [Action {
            name: StringId(0x0800_0080),
            graph_index: -1,
            animation_index: 0
        }]
    );
    assert_eq!(ty.preload_a, [0, 7]);
    assert!(ty.preload_b.is_empty());
    assert_eq!(g.resources.len(), 1);
    assert_eq!(g.resources[0].size, 0x200);
    let p = g.resources[0].pointer.unwrap();
    assert_eq!((p.file, p.offset), (map::ResourceFile::MainMenu, 0x1000));
    assert_eq!(g.static_poses[0].rotation, [0, 0, 0, 100]);
    assert_eq!(g.static_poses[0].scale, 1.5);
}

#[test]
fn string_names_resolve_through_the_table() {
    let g = AnimationGraph::decode(&sample_graph(), BASE).unwrap();
    // index 0x42, length 4 => "walk"
    let mut strings = vec![None; 0x43];
    strings[0x42] = Some("walk".to_owned());
    assert_eq!(g.find_animation(&strings, "walk"), Some(0));
    assert_eq!(g.find_animation(&strings, "run"), None);
}

#[test]
fn graph_rejects_damage() {
    let good = sample_graph();
    // too short for the header
    assert!(AnimationGraph::decode(&good[..HEADER_SIZE - 1], BASE).is_err());
    // block pointer below the tag base
    assert!(AnimationGraph::decode(&good, BASE + 0x10_0000).is_err());
    // node block running past the payload
    let mut bad = good.clone();
    bad[0x0c..0x10].copy_from_slice(&le32(0x10_0000));
    assert!(AnimationGraph::decode(&bad, BASE).is_err());
    // two modelled blocks sharing storage
    let mut overlap = good.clone();
    let nodes_ptr = u32::from_le_bytes(good[0x10..0x14].try_into().unwrap());
    overlap[0xb8..0xbc].copy_from_slice(&le32(nodes_ptr)); // static poses onto the nodes
    assert!(AnimationGraph::decode(&overlap, BASE).is_err());
}

#[test]
fn empty_graph_is_valid() {
    let g = AnimationGraph::decode(&[0u8; HEADER_SIZE], BASE).unwrap();
    assert!(g.animations.is_empty() && g.nodes.is_empty() && g.modes.is_empty());
}

// ---- stream builders --------------------------------------------------------

fn pad_to(v: &mut Vec<u8>, n: usize) {
    assert!(v.len() <= n);
    v.resize(n, 0);
}
fn quant(q: [i16; 4]) -> Vec<u8> {
    q.iter().flat_map(|v| v.to_le_bytes()).collect()
}
fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

/// Codec 1 stream: `rot` quantized quaternions, `trans` vectors, `scale` floats.
fn static_stream(rot: &[[i16; 4]], trans: &[[f32; 3]], scale: &[f32]) -> Vec<u8> {
    let mut s = vec![0u8; 0x20];
    s[0] = 1;
    s[1] = rot.len() as u8;
    s[2] = trans.len() as u8;
    s[3] = scale.len() as u8;
    s.extend(rot.iter().flat_map(|q| quant(*q)));
    let trans_at = s.len();
    s.extend(trans.iter().flat_map(|t| floats(t)));
    let scale_at = s.len();
    s.extend(floats(scale));
    s[0xc..0x10].copy_from_slice(&le32(trans_at as u32));
    s[0x10..0x14].copy_from_slice(&le32(scale_at as u32));
    s
}

/// Flag arrays for 3 nodes: one 4-byte array per channel.
fn flags(rot: u8, trans: u8, scale: u8) -> Vec<u8> {
    vec![rot, 0, 0, 0, trans, 0, 0, 0, scale, 0, 0, 0]
}

fn animation(frames: i16, static_size: usize, animated_size: usize, static_flags: u8) -> Animation {
    Animation {
        name: StringId(0),
        node_list_checksum: 0,
        checksums: [0; 2],
        anim_type: AnimationType::Base,
        frame_info_type: 0,
        unknown_12: 0,
        node_count: 3,
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

fn clip(
    frames: i16,
    static_s: &[u8],
    animated: &[u8],
    static_flags: [u8; 3],
    anim_flags: [u8; 3],
) -> Result<AnimationClip, crate::Error> {
    let mut blob = static_s.to_vec();
    blob.extend(animated);
    let sf = if static_s.is_empty() { 0 } else { 12 };
    if !static_s.is_empty() {
        blob.extend(flags(static_flags[0], static_flags[1], static_flags[2]));
    }
    if !animated.is_empty() {
        blob.extend(flags(anim_flags[0], anim_flags[1], anim_flags[2]));
    }
    AnimationClip::new(&animation(frames, static_s.len(), animated.len(), sf), blob)
}

#[test]
fn static_stream_assigns_values_by_flag_rank() {
    // nodes 0 and 2 have a rotation, node 1 a translation, node 2 a scale
    let st = static_stream(
        &[[0, 0, 0, 100], [0, 0, 16384, 16384]],
        &[[1.0, 2.0, 3.0]],
        &[2.5],
    );
    let c = clip(2, &st, &[], [0b101, 0b010, 0b100], [0; 3]).unwrap();
    assert_eq!(c.codecs(), [Some(Codec::UncompressedStatic), None]);
    assert_eq!(c.channel_counts(0), Some([2, 1, 1]));
    assert_eq!(c.declared_counts(0), Some([2, 1, 1]));
    for f in 0..2 {
        let p = c.sample_frame(f).unwrap();
        assert!(close_all(&p.nodes[0].rotation, &[0.0, 0.0, 0.0, 1.0]));
        assert!(p.nodes[0].has_rotation && !p.nodes[0].has_translation);
        assert!(!p.nodes[1].has_rotation && p.nodes[1].has_translation);
        assert_eq!(p.nodes[1].translation, [1.0, 2.0, 3.0]);
        assert!(close_all(
            &p.nodes[2].rotation,
            &[0.0, 0.0, SQRT_HALF, SQRT_HALF]
        ));
        assert_eq!(p.nodes[2].scale, 2.5);
        assert!(p.nodes[2].has_scale && !p.nodes[0].has_scale);
        assert_eq!(p.nodes[0].scale, 1.0, "absent channels stay identity");
    }
}

/// Codec 3: rotations as per-frame quantized words, translations and scales floats.
fn codec3(
    rot_nodes: usize,
    rot: &[[i16; 4]],
    trans: &[[f32; 3]],
    scale: &[f32],
    frames: usize,
) -> Vec<u8> {
    let mut s = vec![0u8; 0x20];
    s[0] = 3;
    s[1] = rot_nodes as u8;
    let stride_r = frames * 8;
    let stride_t = frames * 12;
    let stride_s = frames * 4;
    s[0x14..0x18].copy_from_slice(&le32(stride_r as u32));
    s[0x18..0x1c].copy_from_slice(&le32(stride_t as u32));
    s[0x1c..0x20].copy_from_slice(&le32(stride_s as u32));
    s.extend(rot.iter().flat_map(|q| quant(*q)));
    pad_to(&mut s, 0x20 + rot_nodes * stride_r);
    let t_at = s.len();
    s.extend(trans.iter().flat_map(|t| floats(t)));
    let s_at = s.len();
    s.extend(floats(scale));
    s[2] = (trans.len() / frames.max(1)) as u8;
    s[3] = (scale.len() / frames.max(1)) as u8;
    s[0xc..0x10].copy_from_slice(&le32(t_at as u32));
    s[0x10..0x14].copy_from_slice(&le32(s_at as u32));
    s
}

#[test]
fn quantized_rotation_codec_samples_each_frame_and_blends_between() {
    let rots = [[0, 0, 0, 100], [0, 0, 100, 100], [0, 0, 100, 0]];
    let trans = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]];
    let scale = [1.0, 2.0, 3.0];
    let a = codec3(1, &rots, &trans, &scale, 3);
    let c = clip(3, &[], &a, [0; 3], [0b001, 0b001, 0b001]).unwrap();
    assert_eq!(c.codecs(), [None, Some(Codec::QuantizedRotation8)]);
    let p1 = c.sample_frame(1).unwrap();
    assert!(close_all(
        &p1.nodes[0].rotation,
        &[0.0, 0.0, SQRT_HALF, SQRT_HALF]
    ));
    assert_eq!(p1.nodes[0].translation, [1.0, 0.0, 0.0]);
    assert_eq!(p1.nodes[0].scale, 2.0);
    // half way between frames 1 and 2
    let h = c.sample(1.5).unwrap();
    assert!(close(h.nodes[0].translation[0], 1.5));
    assert!(close(h.nodes[0].scale, 2.5));
    let r = h.nodes[0].rotation;
    assert!(
        close((r.iter().map(|v| v * v).sum::<f32>()).sqrt(), 1.0),
        "blend stays unit length"
    );
    // sampling past the end clamps to the last frame
    let end = c.sample(99.0).unwrap();
    assert_eq!(end, c.sample_frame(2).unwrap());
    assert!(c.sample_frame(3).is_err());
    // untouched nodes keep identity and no channel flags
    assert!(!h.nodes[1].has_rotation && !h.nodes[2].has_translation);
}

#[test]
fn quaternion_blend_takes_the_short_arc() {
    // frame 0 and frame 1 are q and -q: the blend must not collapse to zero
    let rots = [[0, 0, 0, 100], [0, 0, 0, -100]];
    let a = codec3(1, &rots, &[], &[], 2);
    let c = clip(2, &[], &a, [0; 3], [0b001, 0, 0]).unwrap();
    let h = c.sample(0.5).unwrap();
    assert!(close(h.nodes[0].rotation[3].abs(), 1.0));
}

/// Keyframe codec stream with one rotation channel, one translation and one scale.
/// `wide` selects word-sized key times.
#[allow(clippy::too_many_arguments)]
fn keyframes(
    wide: bool,
    codec: u8,
    rot_t: &[u16],
    rot_q: &[[i16; 4]],
    tr_t: &[u16],
    tr_v: &[[f32; 3]],
    sc_t: &[u16],
    sc_v: &[f32],
) -> Vec<u8> {
    let w = |t: &[u16]| -> Vec<u8> {
        t.iter()
            .flat_map(|&v| {
                if wide {
                    v.to_le_bytes().to_vec()
                } else {
                    vec![v as u8]
                }
            })
            .collect()
    };
    let mut s = vec![0u8; 0x30];
    s[0] = codec;
    s[1] = u8::from(!rot_t.is_empty());
    s[2] = u8::from(!tr_t.is_empty());
    s[3] = u8::from(!sc_t.is_empty());
    let word = |n: usize| le32(n as u32); // first key 0, count n
    if !rot_t.is_empty() {
        s.extend(word(rot_t.len()));
    }
    let trans_words = s.len();
    if !tr_t.is_empty() {
        s.extend(word(tr_t.len()));
    }
    let scale_words = s.len();
    if !sc_t.is_empty() {
        s.extend(word(sc_t.len()));
    }
    let rot_times = s.len();
    s.extend(w(rot_t));
    let tr_times = s.len();
    s.extend(w(tr_t));
    let sc_times = s.len();
    s.extend(w(sc_t));
    let aligned = s.len().div_ceil(8) * 8;
    pad_to(&mut s, aligned);
    let rot_vals = s.len();
    s.extend(rot_q.iter().flat_map(|q| quant(*q)));
    let tr_vals = s.len();
    s.extend(tr_v.iter().flat_map(|t| floats(t)));
    let sc_vals = s.len();
    s.extend(floats(sc_v));
    for (off, v) in [
        (0xc, trans_words),
        (0x10, scale_words),
        (0x14, rot_times),
        (0x18, tr_times),
        (0x1c, sc_times),
        (0x20, rot_vals),
        (0x24, tr_vals),
        (0x28, sc_vals),
    ] {
        s[off..off + 4].copy_from_slice(&le32(v as u32));
    }
    s
}

fn keyframe_clip(wide: bool, codec: u8) -> AnimationClip {
    // 11 frames; keys at 0, 4, 10
    let a = keyframes(
        wide,
        codec,
        &[0, 4, 10],
        &[[0, 0, 0, 100], [0, 0, 100, 100], [0, 0, 100, 0]],
        &[0, 10],
        &[[0.0, 0.0, 0.0], [10.0, 20.0, 30.0]],
        &[0, 10],
        &[1.0, 3.0],
    );
    clip(11, &[], &a, [0; 3], [0b010, 0b010, 0b010]).unwrap()
}

#[test]
fn keyframe_codecs_interpolate_between_keys() {
    for (wide, codec, expect) in [
        (false, 4u8, Codec::ByteKeyframe),
        (true, 5, Codec::WordKeyframe),
        (false, 6, Codec::ReverseByteKeyframe),
        (true, 7, Codec::ReverseWordKeyframe),
    ] {
        let c = keyframe_clip(wide, codec);
        assert_eq!(c.codecs()[1], Some(expect));
        assert_eq!(c.key_audit().unwrap(), [3, 3, 3, 3]);
        // exact keys
        let k4 = c.sample_frame(4).unwrap().nodes[1];
        assert!(close_all(&k4.rotation, &[0.0, 0.0, SQRT_HALF, SQRT_HALF]));
        assert!(close_all(&k4.translation, &[4.0, 8.0, 12.0]));
        assert!(close(k4.scale, 1.8));
        // mid-segment rotation: weights 2:2 between keys 0 and 4 at frame 2
        let k2 = c.sample_frame(2).unwrap().nodes[1];
        let (a, b) = ([0.0f32, 0.0, 0.0, 100.0], [0.0f32, 0.0, 100.0, 100.0]);
        let (w0, w1) = (2.0, 2.0);
        let m: Vec<f32> = (0..4).map(|i| a[i] * w0 + b[i] * w1).collect();
        let l = m.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(close_all(
            &k2.rotation,
            &m.iter().map(|v| v / l).collect::<Vec<_>>()
        ));
        // the last key holds
        let last = c.sample_frame(10).unwrap().nodes[1];
        assert!(close_all(&last.translation, &[10.0, 20.0, 30.0]));
        assert!(close_all(&last.rotation, &[0.0, 0.0, 1.0, 0.0]));
        // fractional sampling in a translation segment
        assert!(close(c.sample(2.5).unwrap().nodes[1].translation[0], 2.5));
        // other nodes untouched
        assert!(!c.sample_frame(3).unwrap().nodes[0].has_rotation);
    }
}

#[test]
fn keyframe_bisection_handles_a_single_key() {
    let a = keyframes(
        false,
        4,
        &[0],
        &[[0, 0, 0, 100]],
        &[0],
        &[[5.0, 6.0, 7.0]],
        &[],
        &[],
    );
    let c = clip(5, &[], &a, [0; 3], [0b001, 0b001, 0]).unwrap();
    for f in 0..5 {
        assert_eq!(
            c.sample_frame(f).unwrap().nodes[0].translation,
            [5.0, 6.0, 7.0]
        );
    }
}

#[test]
fn static_and_animated_streams_combine_with_animated_winning() {
    let st = static_stream(&[[0, 0, 0, 100]], &[[9.0, 9.0, 9.0]], &[]);
    let a = codec3(
        1,
        &[[0, 100, 0, 100], [0, 100, 0, 100]],
        &[[1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
        &[],
        2,
    );
    // node 0: static rotation + static translation; node 0: animated rotation + translation too
    let c = clip(2, &st, &a, [0b001, 0b001, 0], [0b001, 0b001, 0]).unwrap();
    let p = c.sample_frame(1).unwrap();
    assert!(close_all(
        &p.nodes[0].rotation,
        &[0.0, SQRT_HALF, 0.0, SQRT_HALF]
    ));
    assert_eq!(p.nodes[0].translation, [2.0, 0.0, 0.0]);
}

#[test]
fn overlay_animations_ignore_the_static_stream() {
    let st = static_stream(&[[0, 0, 0, 100]], &[], &[]);
    let mut anim = animation(2, st.len(), 0, 12);
    anim.anim_type = AnimationType::Overlay;
    let mut blob = st;
    blob.extend(flags(1, 0, 0));
    let c = AnimationClip::new(&anim, blob).unwrap();
    assert_eq!(c.codecs(), [None, None]);
    assert!(!c.sample_frame(0).unwrap().nodes[0].has_rotation);
}

#[test]
fn uncompressed_and_blend_screen_streams_use_float_rotations() {
    for codec in [2u8, 8] {
        let frames = 2usize;
        let mut s = vec![0u8; 0x20];
        s[0] = codec;
        s[1] = 1;
        let stride_r = frames * 16;
        s[0x14..0x18].copy_from_slice(&le32(stride_r as u32));
        s.extend(floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0]));
        let t_at = s.len();
        s[0xc..0x10].copy_from_slice(&le32(t_at as u32));
        s[0x10..0x14].copy_from_slice(&le32(t_at as u32));
        let c = clip(2, &[], &s, [0; 3], [0b001, 0, 0]).unwrap();
        assert_eq!(c.codecs()[1].map(|c| c as u8), Some(codec));
        assert_eq!(
            c.sample_frame(1).unwrap().nodes[0].rotation,
            [0.0, 0.0, 1.0, 0.0]
        );
    }
}

// ---- structural damage ---------------------------------------------------------

#[test]
fn damaged_clips_are_rejected_not_panicked() {
    let st = static_stream(&[[0, 0, 0, 100]], &[], &[]);
    // blob shorter than the declared sections
    let mut anim = animation(2, st.len(), 0, 12);
    assert!(AnimationClip::new(&anim, st.clone()).is_err());
    // zero frames
    anim.frame_count = 0;
    assert!(AnimationClip::new(&anim, vec![0; 256]).is_err());
    // unknown codec byte
    let mut bad = st.clone();
    bad[0] = 0x7f;
    assert!(clip(2, &bad, &[], [1, 0, 0], [0; 3]).is_err());
    // a stream shorter than its header
    assert!(clip(2, &st[..0x10], &[], [1, 0, 0], [0; 3]).is_err());
    // flag bits disagreeing with the stream header's channel counts
    let one = static_stream(&[[0, 0, 0, 100]], &[], &[]);
    assert!(clip(2, &one, &[], [0b111, 0, 0], [0; 3]).is_err());
    assert!(clip(2, &one, &[], [0, 0, 0], [0; 3]).is_err());
    // keyframe words pointing outside the blob
    let mut k = keyframes(
        false,
        4,
        &[0, 4],
        &[[0, 0, 0, 100], [0, 0, 0, 100]],
        &[],
        &[],
        &[],
        &[],
    );
    k[0x30..0x34].copy_from_slice(&le32((0xfffff << 12) | 2));
    assert!(clip(5, &[], &k, [0; 3], [1, 0, 0]).is_err());
    // keyframe channel declaring no keys
    let mut k = keyframes(
        false,
        4,
        &[0, 4],
        &[[0, 0, 0, 100], [0, 0, 0, 100]],
        &[],
        &[],
        &[],
        &[],
    );
    k[0x30..0x34].copy_from_slice(&le32(0));
    assert!(clip(5, &[], &k, [0; 3], [1, 0, 0]).is_err());
    // first key after the sampled frame: held, not an underflow
    let k = keyframes(
        false,
        4,
        &[3, 6],
        &[[0, 0, 0, 100], [0, 0, 0, 100]],
        &[3, 6],
        &[[1.0; 3], [2.0; 3]],
        &[3, 6],
        &[1.0, 2.0],
    );
    let c = clip(8, &[], &k, [0; 3], [1, 1, 1]).unwrap();
    assert_eq!(c.sample_frame(0).unwrap().nodes[0].translation, [1.0; 3]);
}

#[test]
fn flag_array_size_matches_the_engine_formula() {
    for (nodes, bytes) in [(0, 0), (1, 4), (8, 4), (32, 4), (33, 8), (64, 8), (72, 12)] {
        assert_eq!(flag_array_bytes(nodes), bytes, "{nodes} nodes");
    }
}

#[test]
fn node_matrix_applies_rotation_translation_scale() {
    let n = NodeTransform {
        rotation: [0.0, 0.0, SQRT_HALF, SQRT_HALF],
        translation: [1.0, 2.0, 3.0],
        scale: 2.0,
        ..Default::default()
    };
    let m = n.matrix();
    // +90 degrees about z maps x to y; scaled by 2
    assert!(close_all(&m[0], &[0.0, 2.0, 0.0, 0.0]));
    assert!(close_all(&m[1], &[-2.0, 0.0, 0.0, 0.0]));
    assert!(close_all(&m[3], &[1.0, 2.0, 3.0, 1.0]));
    let id = NodeTransform::default().matrix();
    assert_eq!(id[0], [1.0, 0.0, 0.0, 0.0]);
}

#[test]
fn codec_table_names_follow_the_engine() {
    for c in 1..=8u8 {
        let codec = Codec::from_u8(c).unwrap();
        assert_eq!(codec as u8, c);
        assert!(!codec.name().is_empty());
    }
    assert_eq!(Codec::from_u8(0), None);
    assert_eq!(Codec::from_u8(9), None);
    assert_eq!(
        Codec::ReverseByteKeyframe.name(),
        "reverse_byte_keyframe_lightly_quantized"
    );
}

// ---- review fixes ----------------------------------------------------------------

#[test]
fn key_time_after_the_sampled_frame_holds_the_first_key() {
    // keys at 3 and 6: frames 0..=2 lie before the first key
    for (wide, codec) in [(false, 4u8), (true, 5), (false, 6), (true, 7)] {
        let a = keyframes(
            wide,
            codec,
            &[3, 6],
            &[[0, 0, 0, 100], [0, 0, 100, 100]],
            &[3, 6],
            &[[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]],
            &[3, 6],
            &[2.0, 4.0],
        );
        let c = clip(8, &[], &a, [0; 3], [1, 1, 1]).unwrap();
        for f in 0..3 {
            let n = c.sample_frame(f).unwrap().nodes[0];
            assert_eq!(n.rotation, [0.0, 0.0, 0.0, 1.0]);
            assert_eq!(n.translation, [1.0, 2.0, 3.0]);
            assert_eq!(n.scale, 2.0);
        }
        // at the first key and past the last key
        assert_eq!(c.sample_frame(3).unwrap().nodes[0].scale, 2.0);
        assert_eq!(c.sample_frame(7).unwrap().nodes[0].scale, 4.0);
        assert!(close(
            c.sample_frame(4).unwrap().nodes[0].scale,
            2.0 + 2.0 / 3.0
        ));
    }
}

#[test]
fn descending_key_times_do_not_underflow() {
    // keys 6 then 3 (damaged order): must not panic whatever frame is asked
    let a = keyframes(
        false,
        4,
        &[6, 3],
        &[[0, 0, 0, 100], [0, 0, 100, 100]],
        &[6, 3],
        &[[1.0; 3], [2.0; 3]],
        &[6, 3],
        &[1.0, 2.0],
    );
    if let Ok(c) = clip(8, &[], &a, [0; 3], [1, 1, 1]) {
        for f in 0..8 {
            let _ = c.sample_frame(f);
        }
    }
}

#[test]
fn degenerate_rotations_are_errors_not_identity() {
    let st = static_stream(&[[0, 0, 0, 0]], &[], &[]);
    assert!(clip(2, &st, &[], [1, 0, 0], [0; 3]).is_err());
}

#[test]
fn static_size_is_signed_sixteen_bit() {
    let st = static_stream(&[[0, 0, 0, 100]], &[], &[]);
    let mut anim = animation(2, st.len(), 0, 12);
    anim.sizes.static_size = 0x8000;
    assert!(AnimationClip::new(&anim, vec![0; 0x10000]).is_err());
    assert!(AnimationClip::declared_len(&anim).is_err());
}

fn message(e: crate::Error) -> String {
    e.to_string()
}

#[test]
fn reads_stay_inside_their_stream() {
    // One rotation key. Only the value offset differs between the two clips: the
    // second points 4 bytes before the stream's end, so the 8-byte value spans the
    // stream's end and the first flag bytes (inside the blob, outside the stream)
    // and would decode to a perfectly valid quaternion if not for the bound.
    let good = keyframes(false, 4, &[0], &[[0, 0, 0, 100]], &[], &[], &[], &[]);
    assert!(clip(1, &[], &good, [0; 3], [1, 0, 0]).is_ok());
    let mut k = good.clone();
    let len = k.len();
    k[0x20..0x24].copy_from_slice(&le32(len as u32 - 4));
    let e = clip(1, &[], &k, [0; 3], [1, 0, 0]).unwrap_err();
    assert!(message(e).contains("outside its stream"));
}

#[test]
fn float_rotations_are_validated_like_quantised_ones() {
    for codec in [2u8, 8] {
        let mut s = vec![0u8; 0x20];
        s[0] = codec;
        s[1] = 1;
        s[0x14..0x18].copy_from_slice(&le32(16));
        s.extend(floats(&[0.0, 0.0, 0.0, 1.0]));
        assert!(
            clip(1, &[], &s, [0; 3], [1, 0, 0]).is_ok(),
            "control {codec}"
        );
        let n = s.len();
        s[n - 16..].copy_from_slice(&floats(&[0.0; 4]));
        let e = clip(1, &[], &s, [0; 3], [1, 0, 0]).unwrap_err();
        assert!(message(e).contains("degenerate"), "codec {codec}");
        s[n - 16..].copy_from_slice(&floats(&[f32::NAN, 0.0, 0.0, 1.0]));
        assert!(clip(1, &[], &s, [0; 3], [1, 0, 0]).is_err(), "nan {codec}");
    }
}

#[test]
fn blend_screens_are_flagged_as_aim_grids() {
    let mut s = vec![0u8; 0x20];
    s[0] = 8;
    s[1] = 1;
    s[0x14..0x18].copy_from_slice(&le32(16));
    s.extend(floats(&[0.0, 0.0, 0.0, 1.0]));
    let c = clip(1, &[], &s, [0; 3], [1, 0, 0]).unwrap();
    assert!(c.is_aim_grid());
    assert!(!keyframe_clip(false, 4).is_aim_grid());
}

#[test]
fn sample_into_reuses_storage_and_matches_sample() {
    let c = keyframe_clip(false, 4);
    let mut pose = Pose { nodes: Vec::new() };
    for f in [0.0, 2.5, 7.25, 99.0, f32::NAN] {
        c.sample_into(f, &mut pose).unwrap();
        assert_eq!(pose, c.sample(f).unwrap());
    }
}

#[test]
fn world_matrices_compose_up_the_tree_in_any_order() {
    let t = |x: f32| NodeTransform {
        translation: [x, 0.0, 0.0],
        ..NodeTransform::default()
    };
    // children listed before their parents: 0 <- 1 <- 2 (2 is the root)
    let pose = Pose {
        nodes: vec![t(1.0), t(10.0), t(100.0)],
    };
    let w = pose.world_matrices(&[1, 2, -1]).unwrap();
    assert_eq!(w[2][3][0], 100.0);
    assert_eq!(w[1][3][0], 110.0);
    assert_eq!(w[0][3][0], 111.0);
    // a rotated parent turns a child's offset
    let mut p = t(0.0);
    p.rotation = [0.0, 0.0, SQRT_HALF, SQRT_HALF]; // 90 degrees about z
    let pose = Pose {
        nodes: vec![p, t(2.0)],
    };
    let w = pose.world_matrices(&[-1, 0]).unwrap();
    assert!(close(w[1][3][0], 0.0) && close(w[1][3][1], 2.0));
    // bad trees
    assert!(pose.world_matrices(&[1, 0]).is_err());
    assert!(pose.world_matrices(&[5, -1]).is_err());
    assert!(pose.world_matrices(&[-1]).is_err());
}

/// A reader whose bounds check always passes, so any refusal comes from the
/// code under test; it errors if asked for more than it holds.
struct Mock {
    data: Vec<u8>,
    reads: usize,
}
impl crate::sound::ResourceReader for Mock {
    fn check(&self, _: map::RawPointer, _: u32) -> crate::Result<()> {
        Ok(())
    }
    fn read_raw(&mut self, _: map::RawPointer, size: u32) -> crate::Result<Vec<u8>> {
        self.reads += 1;
        self.data
            .get(..size as usize)
            .map(<[u8]>::to_vec)
            .ok_or(crate::Error::Invalid("mock: read beyond the file"))
    }
}

#[test]
fn many_animations_share_one_resource_without_copying_it() {
    let st = static_stream(&[[0, 0, 0, 100]], &[], &[]);
    let anim = animation(2, st.len(), 0, 12);
    let mut data = st;
    data.extend(flags(1, 0, 0));
    data.resize(1 << 16, 0);
    let shared: std::sync::Arc<[u8]> = std::sync::Arc::from(data);
    let clips: Vec<_> = (0..200)
        .map(|_| AnimationClip::from_shared(&anim, shared.clone(), 0).unwrap())
        .collect();
    assert_eq!(std::sync::Arc::strong_count(&shared), 201);
    assert!(clips.iter().all(|c| c.sample_frame(1).is_ok()));
    // the private-copy constructor keeps only the declared span
    let mut big = shared.to_vec();
    big.resize(1 << 20, 7);
    let c = AnimationClip::new(&anim, big).unwrap();
    assert_eq!(c.frame_count(), 2);
}

#[test]
fn oversized_resources_are_refused_before_any_read() {
    let mut g = AnimationGraph::decode(&sample_graph(), BASE).unwrap();
    let mut mock = Mock {
        data: vec![0; 0x400],
        reads: 0,
    };
    // the declared size (0x200) loads, and so does a size right at the limit's edge
    assert_eq!(g.read_resource(0, &mut mock).unwrap().len(), 0x200);
    assert_eq!(mock.reads, 1);
    g.resources[0].size = (MAX_RESOURCE_BYTES + 1) as u32;
    mock.reads = 0;
    let e = g.read_resource(0, &mut mock).unwrap_err();
    assert!(message(e).contains("larger than the limit"));
    assert_eq!(mock.reads, 0, "the reader was never asked");
}

#[test]
fn animation_node_count_must_match_the_graph() {
    // A valid one-rotation clip over three nodes, in a valid resource.
    let st = static_stream(&[[0, 0, 0, 100]], &[], &[]);
    let mut g = AnimationGraph::decode(&sample_graph(), BASE).unwrap();
    g.animations[0] = animation(2, st.len(), 0, 12);
    assert_eq!(g.nodes.len(), 3);
    let mut data = st;
    data.extend(flags(1, 0, 0));
    data.resize(0x200, 0);
    let shared: std::sync::Arc<[u8]> = std::sync::Arc::from(data.clone());
    // control: the same animation with matching counts builds
    assert_eq!(g.clip_from_resource(0, &data).unwrap().node_count(), 3);
    assert_eq!(g.clip_from_shared(0, &shared).unwrap().node_count(), 3);
    // the clip would decode just as well with a fourth node (same flag-array
    // size), so only the graph comparison can refuse it
    g.animations[0].node_count = 4;
    let direct = AnimationClip::new(&g.animations[0], data.clone());
    assert!(direct.is_ok(), "fixture is valid apart from the count");
    for e in [
        g.clip_from_resource(0, &data).unwrap_err(),
        g.clip_from_shared(0, &shared).unwrap_err(),
    ] {
        assert!(message(e).contains("node count differs"));
    }
}

/// A one-rotation, two-frame codec 8 (blend screen) stream and its flags,
/// as an animation's whole blob.
fn blend_screen_blob() -> Vec<u8> {
    let mut s = vec![0u8; 0x20];
    s[0] = 8;
    s[1] = 1;
    s[0x14..0x18].copy_from_slice(&le32(2 * 16));
    s.extend(floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0]));
    let t_at = s.len() as u32;
    s[0xc..0x10].copy_from_slice(&le32(t_at));
    s[0x10..0x14].copy_from_slice(&le32(t_at));
    s
}

#[test]
fn engine_problems_require_aligned_blend_screen_streams() {
    let s = blend_screen_blob();
    let mut g = AnimationGraph::decode(&sample_graph(), BASE).unwrap();
    assert!(g.blend_screens.is_empty());
    let mut anim = animation(2, 0, s.len(), 0);
    anim.anim_type = AnimationType::Overlay;
    anim.unknown_12 = 0xff;
    let resource_at = |offset: usize| {
        let mut data = vec![0u8; offset];
        data.extend(&s);
        data.extend(flags(1, 0, 0));
        data.resize(0x200, 0);
        data
    };
    // At offset 0x40 the engine's `movaps` reads aligned floats.
    anim.data_offset = 0x40;
    g.animations[0] = anim.clone();
    assert_eq!(
        g.engine_problems(0, &resource_at(0x40)),
        Vec::<String>::new()
    );
    // At 0x4c (4-byte aligned, as converter version 84 placed them) the
    // same data decodes, but the engine's read faults.
    anim.data_offset = 0x4c;
    g.animations[0] = anim.clone();
    let data = resource_at(0x4c);
    assert!(g.clip_from_resource(0, &data).is_ok());
    let p = g.engine_problems(0, &data);
    assert_eq!(p.len(), 1, "{p:?}");
    assert!(p[0].contains("not 16-byte aligned"), "{p:?}");
    // A screen past the graph's (none here) is a problem too.
    anim.data_offset = 0x40;
    anim.unknown_12 = 0;
    g.animations[0] = anim;
    let p = g.engine_problems(0, &resource_at(0x40));
    assert_eq!(p, ["blend screen 0 of 0"]);
}
