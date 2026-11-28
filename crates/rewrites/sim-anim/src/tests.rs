//! Synthetic clips only: every blob here is built by the test.
use crate::clip::{Channel, Clip, Shape, Sizes, Which};
use crate::codec::{Ctx, Frame, RotOp, VecOp, Vector, normalize4};
use crate::kernel::{self, Mode, Ops, Record, Refusal, Request, Secondary, sample};
use crate::store::ClipStore;
use sim_core::Epoch;
use sim_core::det::{self, Fp};

fn b(x: f32) -> u32 {
    x.to_bits()
}

const ONE: u32 = 0x3f80_0000;
const SIGN: u32 = 0x8000_0000;

/// A stream under construction: header words, then data at chosen offsets.
struct S(Vec<u8>);

impl S {
    fn new(codec: u8) -> S {
        let mut v = vec![0; 0x40];
        v[0] = codec;
        S(v)
    }
    fn put(&mut self, at: usize, bytes: &[u8]) -> &mut Self {
        if self.0.len() < at + bytes.len() {
            self.0.resize(at + bytes.len(), 0);
        }
        self.0[at..at + bytes.len()].copy_from_slice(bytes);
        self
    }
    fn u32(&mut self, at: usize, v: u32) -> &mut Self {
        self.put(at, &v.to_le_bytes())
    }
    fn f32s(&mut self, at: usize, v: &[f32]) -> &mut Self {
        let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.put(at, &bytes)
    }
    fn i16s(&mut self, at: usize, v: &[i16]) -> &mut Self {
        let bytes: Vec<u8> = v.iter().flat_map(|x| x.to_le_bytes()).collect();
        self.put(at, &bytes)
    }
}

/// Flagged nodes per channel: rotation, translation, scale.
fn lists<'a>(r: &'a [u16], t: &'a [u16], s: &'a [u16]) -> [&'a [u16]; 3] {
    [r, t, s]
}

/// A clip from streams and their flagged nodes per channel.
fn clip(nodes: u8, streams: [Option<(&S, [&[u16]; 3])>; 2]) -> Clip {
    let flag_bytes = ((usize::from(nodes) + 31) >> 3) & !3;
    let s_len = streams[0].map_or(0, |(s, _)| s.0.len());
    let a_len = streams[1].map_or(0, |(s, _)| s.0.len());
    let mut sizes = [0u8; 16];
    sizes[0] = (3 * flag_bytes) as u8;
    sizes[6..8].copy_from_slice(&(s_len as i16).to_le_bytes());
    sizes[12..16].copy_from_slice(&(a_len as u32).to_le_bytes());
    let mut blob = Vec::new();
    for (s, _) in streams.iter().flatten() {
        blob.extend_from_slice(&s.0);
    }
    for stream in streams {
        let mut flags = vec![0u8; 3 * flag_bytes];
        if let Some((_, lists)) = stream {
            for (c, list) in lists.iter().enumerate() {
                for &n in *list {
                    flags[c * flag_bytes + usize::from(n >> 3)] |= 1 << (n & 7);
                }
            }
        }
        blob.extend_from_slice(&flags);
    }
    let shape = Shape {
        sizes: Sizes(sizes),
        nodes,
    };
    Clip::decode(shape, blob.into_boxed_slice()).expect("synthetic clip decodes")
}

fn request<'a>(mode: Mode, streams: [Option<Ops>; 2]) -> Request<'a> {
    Request {
        mode,
        weight: ONE,
        count: 64,
        mask: None,
        frame: Frame { a: 0, b: 0, w: 0 },
        secondary: None,
        one: ONE,
        sign: SIGN,
        data_addr: 0x1000_0000,
        streams,
        mxcsr: 0x1fa0,
    }
}

const RAW: Ops = Ops {
    rotation: Some(RotOp::FrameRaw),
    translation: Some(VecOp::FrameRaw),
    scale: Some(VecOp::FrameRaw),
};

fn ctx() -> Ctx {
    Ctx {
        fp: Fp::new(),
        mmx: false,
        data_addr: 0x1000_0000,
        one: ONE,
        misaligned: false,
    }
}

/// The codecs' normalization written out in f32.
fn reference_normalize(q: [f32; 4]) -> [u32; 4] {
    let sq = q.map(|x| x * x);
    let p = [sq[0] + sq[2], sq[1] + sq[3], sq[2] + sq[0], sq[3] + sq[1]];
    let t = [p[0] + p[1], p[1] + p[0], p[2] + p[1], p[3] + p[0]];
    std::array::from_fn(|i| (q[i] * f32::from_bits(det::rsqrt(t[i].to_bits()))).to_bits())
}

fn reference_nlerp(a: [f32; 4], b: [f32; 4], w: f32) -> [u32; 4] {
    let m: [f32; 4] = std::array::from_fn(|i| a[i] * b[i]);
    let dot = (m[0] + m[2]) + (m[1] + m[3]);
    let b = if dot.is_sign_negative() {
        b.map(|x| -x)
    } else {
        b
    };
    let r: [f32; 4] = std::array::from_fn(|i| (b[i] - a[i]) * w + a[i]);
    let n = (r[0] * r[0] + r[2] * r[2]) + (r[1] * r[1] + r[3] * r[3]);
    let inv = f32::from_bits(det::rsqrt(n.to_bits()));
    r.map(|x| (x * inv).to_bits())
}

#[test]
fn static_int16_rotation_normalizes_the_track() {
    let mut s = S::new(1);
    s.i16s(0x20 + 8, &[1000, -2000, 3000, 4000]);
    let c = clip(4, [Some((&s, lists(&[0, 2], &[], &[]))), None]);
    let st = c.stream(Which::Static).unwrap();
    let mut cx = ctx();
    let q = RotOp::Static16
        .sample(&mut cx, &c, st, 1, Frame::default())
        .unwrap();
    assert!(cx.mmx);
    assert_eq!(q, reference_normalize([1000.0, -2000.0, 3000.0, 4000.0]));
    assert!(cx.fp.exact());
}

#[test]
fn int16_blend_weights_the_two_frames() {
    let mut s = S::new(3);
    s.u32(0x14, 32); // four frames of eight bytes per track
    s.i16s(0x20 + 32 + 8, &[100, 0, 0, 100])
        .i16s(0x20 + 32 + 16, &[0, 100, 0, 100]);
    let c = clip(2, [None, Some((&s, lists(&[0, 1], &[], &[])))]);
    let st = c.stream(Which::Animated).unwrap();
    let w = 0.25f32;
    let f = Frame {
        a: 1,
        b: 2,
        w: b(w),
    };
    let q = RotOp::Blend16.sample(&mut ctx(), &c, st, 1, f).unwrap();
    let rest = 1.0 - w;
    let mixed: [f32; 4] = std::array::from_fn(|i| {
        let a = [100.0, 0.0, 0.0, 100.0][i];
        let bb = [0.0, 100.0, 0.0, 100.0][i];
        a * rest + bb * w
    });
    assert_eq!(q, reference_normalize(mixed));
}

#[test]
fn float_blend_needs_aligned_keys() {
    let mut s = S::new(8);
    s.u32(0x14, 48);
    s.f32s(
        0x20,
        &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0],
    );
    let c = clip(1, [Some((&s, lists(&[0], &[], &[]))), None]);
    let st = c.stream(Which::Static).unwrap();
    let f = Frame {
        a: 0,
        b: 1,
        w: b(0.5),
    };
    let q = RotOp::BlendF32.sample(&mut ctx(), &c, st, 0, f).unwrap();
    assert_eq!(q, reference_normalize([0.0, 0.0, 0.5, 0.5]));
    let mut cx = ctx();
    cx.data_addr = 0x1000_0004;
    assert_eq!(RotOp::BlendF32.sample(&mut cx, &c, st, 0, f), None);
}

#[test]
fn translation_lerp_and_raw_frames() {
    let mut s = S::new(3);
    s.u32(0x0c, 0x40).u32(0x18, 36);
    s.f32s(0x40 + 36, &[1.0, 2.0, 3.0, 5.0, 7.0, 11.0, 0.0, 0.0, 0.0]);
    let c = clip(3, [None, Some((&s, lists(&[], &[0, 2], &[])))]);
    let st = c.stream(Which::Animated).unwrap();
    let f = Frame {
        a: 0,
        b: 1,
        w: b(0.375),
    };
    let mut cx = ctx();
    let v = VecOp::FrameLerp
        .sample(&mut cx, &c, st, Vector::Translation, 1, f)
        .unwrap();
    let expect = [
        (5.0f32 - 1.0) * 0.375 + 1.0,
        (7.0f32 - 2.0) * 0.375 + 2.0,
        (11.0f32 - 3.0) * 0.375 + 3.0,
    ];
    assert_eq!(v, expect.map(b));
    let v = VecOp::FrameRaw
        .sample(&mut cx, &c, st, Vector::Translation, 1, Frame { a: 1, ..f })
        .unwrap();
    assert_eq!(v, [5.0f32, 7.0, 11.0].map(b));
    assert!(!cx.mmx);
}

/// A keyed translation track: times `[0, 4, 10]`, values one per key.
fn keyed(wide: bool) -> S {
    let mut s = S::new(if wide { 5 } else { 4 });
    s.u32(0x0c, 0x40).u32(0x18, 0x50).u32(0x24, 0x60);
    // Track 1's word: three keys starting at key 2 of the shared arrays.
    s.u32(0x44, 3 | 2 << 12);
    if wide {
        s.put(0x50, &[9, 0, 9, 0, 0, 0, 4, 0, 10, 0]);
    } else {
        s.put(0x50, &[9, 9, 0, 4, 10]);
    }
    s.f32s(0x60 + 24, &[1.0, 1.0, 1.0, 2.0, 4.0, 8.0, -2.0, 0.5, 8.0]);
    s
}

#[test]
fn keyed_translation_finds_and_interpolates_keys() {
    for wide in [false, true] {
        let s = keyed(wide);
        let c = clip(2, [None, Some((&s, lists(&[], &[0, 1], &[])))]);
        let st = c.stream(Which::Animated).unwrap();
        let mut cx = ctx();
        let int = VecOp::KeyInt { wide };
        let on_key = int.sample(
            &mut cx,
            &c,
            st,
            Vector::Translation,
            1,
            Frame { a: 4, b: 5, w: 0 },
        );
        assert_eq!(on_key, Some([2.0f32, 4.0, 8.0].map(b)));
        let between = int
            .sample(
                &mut cx,
                &c,
                st,
                Vector::Translation,
                1,
                Frame { a: 7, b: 8, w: 0 },
            )
            .unwrap();
        let t = f32::from_bits(det::rcp(6.0f32.to_bits())) * 3.0;
        let expect = [
            (-2.0f32 - 2.0) * t + 2.0,
            (0.5f32 - 4.0) * t + 4.0,
            (8.0f32 - 8.0) * t + 8.0,
        ];
        assert_eq!(between, expect.map(b));
        let past = int.sample(
            &mut cx,
            &c,
            st,
            Vector::Translation,
            1,
            Frame { a: 20, b: 20, w: 0 },
        );
        assert_eq!(past, Some([-2.0f32, 0.5, 8.0].map(b)));
        let interp = VecOp::KeyInterp { wide };
        let f = Frame {
            a: 4,
            b: 5,
            w: b(0.5),
        };
        let v = interp
            .sample(&mut cx, &c, st, Vector::Translation, 1, f)
            .unwrap();
        let t = f32::from_bits(det::rcp(6.0f32.to_bits())) * 0.5;
        let expect = [
            (-2.0f32 - 2.0) * t + 2.0,
            (0.5f32 - 4.0) * t + 4.0,
            (8.0f32 - 8.0) * t + 8.0,
        ];
        assert_eq!(v, expect.map(b));
        assert!(cx.fp.exact());
    }
}

#[test]
fn product_with_identity_is_the_record() {
    let a = [0.25f32, -0.5, 0.125, 0.75].map(b);
    let mut fp = Fp::new();
    assert_eq!(kernel::product(&mut fp, a, [0, 0, 0, ONE], SIGN), a);
    assert!(fp.exact());
}

#[test]
fn nlerp_takes_the_short_way() {
    let a = [0.0f32, 0.0, 0.0, 1.0];
    let bq = [0.0f32, 0.6, 0.0, -0.8];
    let mut fp = Fp::new();
    let r = kernel::nlerp(&mut fp, a.map(b), bq.map(b), b(0.5), SIGN);
    assert_eq!(r, reference_nlerp(a, bq, 0.5));
}

/// Twelve nodes; rotations on 1, 9, 10; translations on 3, 9; scale on 11.
fn twelve() -> Clip {
    let mut s = S::new(8);
    s.u32(0x14, 32)
        .u32(0x0c, 0x100)
        .u32(0x18, 24)
        .u32(0x10, 0x200)
        .u32(0x1c, 8);
    for track in 0..3u32 {
        for frame in 0..2u32 {
            let v = (10 * track + frame + 1) as f32;
            s.f32s(
                0x20 + 32 * track as usize + 16 * frame as usize,
                &[v, 0.0, 0.0, 1.0],
            );
        }
    }
    for track in 0..2usize {
        s.f32s(
            0x100 + 24 * track,
            &[track as f32 + 1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        );
    }
    s.f32s(0x200, &[2.0, 3.0]);
    let lists = lists(&[1, 9, 10], &[3, 9], &[11]);
    clip(12, [None, Some((&s, lists))])
}

#[test]
fn replace_writes_flagged_live_nodes_by_track_rank() {
    let c = twelve();
    let mask = [!(1u32 << 9)];
    let mut req = request(Mode::Replace, [None, Some(RAW)]);
    req.mask = Some(&mask);
    req.count = 12;
    let mut recs = vec![[7u32; 8]; 12];
    let mut written = vec![0u8; 12];
    sample(&req, &c, &mut recs, &mut written).unwrap();
    assert_eq!(written[1], kernel::ROTATION);
    assert_eq!(written[9], 0, "masked out");
    assert_eq!(written[10], kernel::ROTATION);
    assert_eq!(written[3], kernel::TRANSLATION);
    assert_eq!(written[11], kernel::SCALE);
    assert_eq!(
        recs[10][0..4],
        [21.0f32, 0.0, 0.0, 1.0].map(b),
        "track 2, frame 0"
    );
    assert_eq!(recs[3][4..7], [1.0f32, 2.0, 3.0].map(b));
    assert_eq!(recs[11][7], b(2.0));
    assert_eq!(recs[0], [7; 8]);

    req.count = 10;
    let mut written = vec![0u8; 12];
    sample(&req, &c, &mut recs, &mut written).unwrap();
    assert_eq!(written[10], 0, "at or past the count");
    assert_eq!(written[11], 0);
}

#[test]
fn modes_combine_with_the_record() {
    let c = twelve();
    let w = 0.5f32;
    let base: Record = [0, 0, 0, ONE, b(1.0), b(1.0), b(1.0), b(4.0)];
    let run = |mode| {
        let mut req = request(mode, [None, Some(RAW)]);
        req.weight = b(w);
        req.count = 12;
        let mut recs = vec![base; 12];
        let mut written = vec![0u8; 12];
        sample(&req, &c, &mut recs, &mut written).unwrap();
        recs
    };
    let r = run(Mode::Add);
    assert_eq!(r[3][4..7], [2.0f32, 3.0, 4.0].map(b));
    assert_eq!(r[11][7], b(8.0));
    assert_eq!(r[1][0..4], [1.0f32, 0.0, 0.0, 1.0].map(b));
    let r = run(Mode::Blend);
    assert_eq!(r[3][4..7], [1.0f32, 1.5, 2.0].map(b));
    assert_eq!(r[11][7], b(3.0));
    assert_eq!(
        r[1][0..4],
        reference_nlerp([0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0], w)
    );
    let r = run(Mode::AddScaled);
    assert_eq!(r[3][4..7], [1.5f32, 2.0, 2.5].map(b));
    assert_eq!(r[11][7], b(((2.0f32 - 1.0) * w + 1.0) * 4.0));
    assert_eq!(r[1][0..4], [0.5f32, 0.0, 0.0, 1.0].map(b));
}

#[test]
fn second_frame_blends_before_the_mode() {
    let c = twelve();
    let mut req = request(Mode::Add, [None, Some(RAW)]);
    req.count = 12;
    req.secondary = Some(Secondary {
        frame: Frame { a: 1, b: 1, w: 0 },
        blend: b(0.25),
    });
    let mut recs = vec![[0, 0, 0, ONE, 0, 0, 0, ONE]; 12];
    let mut written = vec![0u8; 12];
    sample(&req, &c, &mut recs, &mut written).unwrap();
    let rot = reference_nlerp([1.0, 0.0, 0.0, 1.0], [2.0, 0.0, 0.0, 1.0], 0.25);
    let mut fp = Fp::new();
    assert_eq!(
        recs[1][0..4],
        kernel::product(&mut fp, [0, 0, 0, ONE], rot, SIGN)
    );
    assert_eq!(recs[3][4], b((4.0f32 - 1.0) * 0.25 + 1.0));
    assert_eq!(recs[11][7], b((3.0f32 - 2.0) * 0.25 + 2.0));
}

#[test]
fn broken_replace_with_second_frame_declines_when_it_reaches_a_node() {
    let c = twelve();
    let mut req = request(Mode::Replace, [None, Some(RAW)]);
    req.count = 12;
    req.secondary = Some(Secondary {
        frame: Frame::default(),
        blend: 0,
    });
    let mut recs = vec![[0; 8]; 12];
    let mut written = vec![0u8; 12];
    let broken = Refusal::Broken { which: Which::Animated, channel: Channel::Rotation, node: 1 };
    assert_eq!(sample(&req, &c, &mut recs, &mut written), Err(broken));
    req.count = 1;
    assert_eq!(
        sample(&req, &c, &mut recs, &mut written).map(|e| e.mmx),
        Ok(false)
    );
    assert!(written.iter().all(|&w| w == 0));
}

#[test]
fn off_path_operands_run_under_the_mxcsr_and_unknown_codecs_decline() {
    let c = twelve();
    let mut req = request(Mode::Add, [None, Some(RAW)]);
    req.count = 12;
    let mut recs = vec![[0, 0, 0, ONE, 0, 0, 0, 1]; 12];
    let mut written = vec![0u8; 12];
    // A denormal scale takes the SSE model: `mulss` raises DE.
    let effects = sample(&req, &c, &mut recs, &mut written).unwrap();
    assert_ne!(effects.mxcsr & simd::DE, 0, "denormal scale");
    // Denormal unmasked: that `mulss` is the guest's `#XM`.
    req.mxcsr = 0x1ea0;
    let mut recs = vec![[0, 0, 0, ONE, 0, 0, 0, 1]; 12];
    assert_eq!(sample(&req, &c, &mut recs, &mut written), Err(Refusal::Unmasked));
    let mut req = request(Mode::Replace, [None, Some(Ops { scale: None, ..RAW })]);
    req.count = 12;
    let unknown = Refusal::UnknownOp { which: Which::Animated, channel: Channel::Scale };
    assert_eq!(sample(&req, &c, &mut recs, &mut written), Err(unknown));
    req.count = 11;
    assert!(
        sample(&req, &c, &mut recs, &mut written).is_ok(),
        "unknown codec never reached"
    );
    let short = [0u32; 0];
    req.mask = Some(&short);
    assert!(matches!(sample(&req, &c, &mut recs, &mut written), Err(Refusal::Mask { .. })));
}

#[test]
fn flag_arrays_cross_byte_and_word_boundaries() {
    let mut s = S::new(3);
    s.u32(0x0c, 0x40);
    let nodes: Vec<u16> = vec![0, 7, 8, 31, 32, 39, 40];
    for (i, _) in nodes.iter().enumerate() {
        s.f32s(0x40 + 12 * i, &[i as f32 + 1.0, 0.0, 0.0]);
    }
    let c = clip(41, [Some((&s, lists(&[], &nodes, &[]))), None]);
    let ops = Ops {
        translation: Some(VecOp::Static),
        ..Ops::default()
    };
    let mask = [u32::MAX, !1u32];
    let mut req = request(Mode::Replace, [Some(ops), None]);
    req.mask = Some(&mask);
    req.count = 41;
    let mut recs = vec![[0; 8]; 41];
    let mut written = vec![0u8; 41];
    sample(&req, &c, &mut recs, &mut written).unwrap();
    for (rank, &n) in nodes.iter().enumerate() {
        let n = usize::from(n);
        if n == 32 {
            assert_eq!(written[n], 0);
        } else {
            assert_eq!(recs[n][4], b(rank as f32 + 1.0), "node {n}");
        }
    }
}

#[test]
fn store_builds_once_and_flushes_on_epoch() {
    static EPOCH: Epoch = Epoch::new();
    let store = ClipStore::new(&EPOCH);
    let c = twelve();
    let mut lens = Vec::new();
    let mut get = |store: &ClipStore, shape| {
        store.get(0x1234, shape, |layout| {
            lens.push(layout.len as usize);
            Some(c.bytes().into())
        })
    };
    let first = get(&store, c.shape).unwrap();
    let again = get(&store, c.shape).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &again));
    EPOCH.bump();
    let rebuilt = get(&store, c.shape).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&first, &rebuilt));
    assert_eq!(*rebuilt, c);
    let mut other = c.shape;
    other.nodes = 40;
    assert!(
        get(&store, other).is_none(),
        "blob length no longer matches"
    );
    // Built twice from the clip's own layout, then asked for a longer one.
    let n = c.bytes().len();
    assert_eq!(lens.len(), 3);
    assert_eq!(&lens[..2], &[n, n]);
    assert!(lens[2] > n);
    assert_eq!(store.stats().hits, 1);
}

#[test]
fn a_map_loaded_twice_holds_the_same_clip_bytes_and_none_between() {
    static EPOCH: Epoch = Epoch::new();
    let store = ClipStore::new(&EPOCH);
    let c = twelve();
    let load = |store: &ClipStore| {
        for key in [0x1234, 0x2234, 0x3234] {
            store.get(key, c.shape, |_| Some(c.bytes().into())).unwrap();
        }
        store.bytes()
    };
    let first = load(&store);
    assert_eq!(first, 3 * c.footprint());
    // Back to the menu: nothing looks a clip up there, and none stays.
    store.invalidate();
    assert_eq!((store.len(), store.bytes()), (0, 0));
    assert_eq!(load(&store), first);
    store.invalidate();
    assert_eq!(load(&store), first);
}

#[test]
fn a_flushed_clip_is_rebuilt_from_the_edited_bytes() {
    static EPOCH: Epoch = Epoch::new();
    let store = ClipStore::new(&EPOCH);
    let c = twelve();
    let mut edited: Vec<u8> = c.bytes().to_vec();
    // A keyframe value of the animated stream (track 0, frame 0, x).
    edited[0x20..0x24].copy_from_slice(&99.0f32.to_le_bytes());
    let blob = std::sync::Mutex::new(c.bytes().to_vec());
    let get = |key| store.get(key, c.shape, |_| Some(blob.lock().unwrap().clone().into())).unwrap();
    let first = get(0x1234);
    let other = get(0x9000);
    *blob.lock().unwrap() = edited.clone();
    // Same shape, same key: the store keeps its copy until told.
    assert!(std::sync::Arc::ptr_eq(&first, &get(0x1234)));
    assert_eq!(store.flush_range(0x1000, 0x2000), 1, "only the edited tag's clip");
    let rebuilt = get(0x1234);
    assert_eq!(rebuilt.bytes(), &edited[..]);
    assert_eq!(*rebuilt, Clip::decode(c.shape, edited.into()).unwrap());
    assert!(std::sync::Arc::ptr_eq(&other, &get(0x9000)), "a clip outside the range stays");
    assert_eq!(store.flush_range(0x5000, 0x6000), 0);
}

#[test]
fn cold_and_warm_samples_agree_on_both_lane_paths() {
    let c = twelve();
    for simd in [false, true] {
        sim_core::lanes::with(simd, || {
            let mut req = request(Mode::Blend, [None, Some(RAW)]);
            req.weight = b(0.3);
            req.count = 12;
            let base: Record = [
                b(0.1),
                b(0.2),
                b(0.3),
                b(0.9),
                b(1.0),
                b(1.0),
                b(1.0),
                b(4.0),
            ];
            let mut warm = vec![base; 12];
            let mut cold = vec![base; 12];
            let (mut w1, mut w2) = (vec![0u8; 12], vec![0u8; 12]);
            sample(&req, &c, &mut warm, &mut w1).unwrap();
            let fresh = Clip::decode(c.shape, c.bytes().into()).unwrap();
            sample(&req, &fresh, &mut cold, &mut w2).unwrap();
            assert_eq!(warm, cold);
            assert_eq!(w1, w2);
        });
    }
}

#[test]
fn normalize_matches_reference() {
    let q = [0.3f32, -0.4, 0.5, 0.6];
    let mut fp = Fp::new();
    assert_eq!(normalize4(&mut fp, q.map(b)), reference_normalize(q));
}
