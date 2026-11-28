//! The decoded box query against `mopp::boxvm::query_box` (the bit model
//! of `0x00300150` and `0x002ff8b0`): hand-written programs for every
//! opcode class, random programs with rescales, diagonals, key offsets,
//! jumps and traps, and programs from `mopp::build` over invented
//! surfaces. No game data.
use super::*;
use crate::synth::{self, Image, Rng};
use crate::{CollisionWorld, Entry};
use ::mopp::{Aabb, Code, geom::Surface};
use sim_core::Epoch;

/// x87 control words: double and single precision, masked, nearest.
const DOUBLE: u16 = 0x027f;
const SINGLE: u16 = 0x007f;

fn source(code: &Code) -> Source {
    Source {
        origin: code.offset.map(f32::to_bits),
        scale: code.scale.to_bits(),
        bytecode: code.bytecode.clone(),
    }
}

fn tree(code: &Code) -> Tree {
    Tree::decode(0x1000, source(code))
}

/// One world unit per full-precision unit: the root frame cell of `p` is
/// `p >> 16`.
fn prog(bytecode: &[u8]) -> Code {
    Code {
        offset: [0.0; 3],
        scale: 1.0,
        info: ::mopp::code::RETAIL_INFO,
        bytecode: bytecode.to_vec(),
    }
}

fn corners(code: &Code, b: &Aabb, control: u16) -> Result<Corners, Bail> {
    Corners::convert(
        b.min.map(f32::to_bits),
        b.max.map(f32::to_bits),
        code.offset.map(f32::to_bits),
        code.scale.to_bits(),
        control,
    )
}

/// The decoded query and the bit model agree on `b`: the same keys in the
/// same order, or both refuse. Returns whether the kernel answered.
fn same(code: &Code, t: &Tree, b: &Aabb, label: &str) -> bool {
    let Ok(q) = corners(code, b, DOUBLE) else {
        return false;
    };
    if q.check().is_err() {
        return false;
    }
    let model = ::mopp::query_box(code, b);
    match t.query_box(&q, usize::MAX) {
        Ok(found) => {
            assert_eq!(Ok(found.keys), model, "{label}: {b:?}");
            true
        }
        Err(why) => {
            assert_eq!(why, Bail::Data, "{label}: {b:?}");
            assert!(model.is_err(), "{label}: the kernel bailed on {b:?}, the model gave {model:?}");
            false
        }
    }
}

fn cell(c: f32) -> f32 {
    c * 65536.0
}

/// Root cells `lo..=hi` on x, cell 0 on y and z.
fn on_x(lo: f32, hi: f32) -> Aabb {
    Aabb {
        min: [cell(lo) + 1000.0, 1000.0, 1000.0],
        max: [cell(hi) + 60000.0, 60000.0, 60000.0],
    }
}

fn keys(t: &Tree, code: &Code, b: Aabb) -> Vec<u32> {
    t.query_box(&corners(code, &b, DOUBLE).unwrap(), usize::MAX)
        .unwrap()
        .keys
}

/// Boxes around a program's interesting values: whole root cells, cells
/// of finer frames, exact cell edges and points.
fn random_box(rng: &mut Rng) -> Aabb {
    fn edge(rng: &mut Rng) -> f32 {
        let cell = rng.below(256) as f32 * 65536.0;
        match rng.below(4) {
            0 => cell,
            1 => cell + (rng.below(16) * 4096) as f32,
            2 => cell + rng.below(65536) as f32,
            _ => cell + 65535.0,
        }
    }
    let mut min = [0.0; 3];
    let mut max = [0.0; 3];
    for a in 0..3 {
        let lo = edge(rng);
        let hi = if rng.chance(4) {
            lo
        } else {
            (lo + (rng.below(1 << 22)) as f32).min(16_777_215.0)
        };
        min[a] = lo;
        max[a] = hi;
    }
    Aabb { min, max }
}

#[test]
fn every_opcode_class_matches_the_model() {
    let cases: [(&str, &[u8]); 12] = [
        ("end", &[0x00]),
        ("terminals", &[0x10, 10, 20, 2, 0x35, 0x00, 0x53, 0x20, 0, 0, 7]),
        ("two-plane", &[0x10, 10, 20, 1, 0x31, 0x32]),
        ("wide two-plane", &[0x23, 10, 20, 0, 0, 0, 1, 0x31, 0x32]),
        ("one-plane", &[0x20, 10, 1, 0x31, 0x32]),
        ("bound", &[0x26, 10, 20, 0x31]),
        ("full bound", &[0x29, 0, 0, 100, 0x0a, 0, 0, 0x31]),
        ("rescale", &[0x04, 1, 0, 0, 0x26, 2, 5, 0x31]),
        ("diagonal", &[0x17, 10, 12, 1, 0x31, 0x32]),
        ("diagonal minus", &[0x18, 128, 128, 1, 0x31, 0x32]),
        ("three-axis diagonal", &[0x01, 3, 5, 7, 0x1c, 140, 150, 1, 0x31, 0x32]),
        (
            "keys",
            &[0x0b, 0, 0, 1, 0, 0x09, 3, 0x0a, 1, 0, 0x60, 9, 0x64, 0, 1, 0x68, 0, 0, 0, 1, 0x51, 1, 2],
        ),
    ];
    let mut rng = Rng(0x5851_f42d_4c95_7f2d);
    for (label, bytecode) in cases {
        let code = prog(bytecode);
        let t = tree(&code);
        assert!(!t.guest_only(), "{label}");
        let mut answered = 0;
        for c in 0..40 {
            answered += u32::from(same(&code, &t, &on_x(c as f32, c as f32), label));
            answered += u32::from(same(&code, &t, &on_x(c as f32 / 2.0, c as f32), label));
        }
        for _ in 0..400 {
            answered += u32::from(same(&code, &t, &random_box(&mut rng), label));
        }
        assert!(answered > 400, "{label}: {answered} answered");
    }
}

#[test]
fn thresholds_are_exclusive_where_the_original_is() {
    let code = prog(&[0x10, 10, 20, 1, 0x31, 0x32]);
    let t = tree(&code);
    assert_eq!(keys(&t, &code, on_x(9.0, 9.0)), [1]);
    assert_eq!(keys(&t, &code, on_x(10.0, 12.0)), Vec::<u32>::new());
    assert_eq!(keys(&t, &code, on_x(15.0, 20.0)), [2]);
    assert_eq!(keys(&t, &code, on_x(5.0, 25.0)), [1, 2]);
    let code = prog(&[0x26, 10, 20, 0x31]);
    let t = tree(&code);
    assert_eq!(keys(&t, &code, on_x(5.0, 8.0)), Vec::<u32>::new());
    assert_eq!(keys(&t, &code, on_x(5.0, 9.0)), [1]);
    assert_eq!(keys(&t, &code, on_x(20.0, 25.0)), Vec::<u32>::new());
}

#[test]
fn a_pc_reached_with_two_states_is_emitted_twice() {
    // Both sides of the split reach the terminal at pc 10, with key
    // offsets 5 and 7.
    let code = prog(&[0x10, 200, 0, 4, 0x09, 5, 0x05, 2, 0x09, 7, 0x31]);
    let t = tree(&code);
    let leaves: Vec<u32> = t
        .nodes()
        .iter()
        .filter_map(|n| match n {
            Node::Leaf { key } => Some(*key),
            _ => None,
        })
        .collect();
    assert_eq!(leaves, [6, 8]);
    assert_eq!(keys(&t, &code, on_x(1.0, 3.0)), [6, 8]);
    // The same pc under two rescales: different thresholds.
    let code = prog(&[0x10, 200, 0, 6, 0x01, 0, 0, 0, 0x05, 4, 0x02, 0, 0, 0, 0x26, 1, 2, 0x31]);
    let t = tree(&code);
    let bounds = t.nodes().iter().filter(|n| matches!(n, Node::Bound { .. })).count();
    assert_eq!(bounds, 2);
    for c in 0..8 {
        same(&code, &t, &on_x(c as f32 / 8.0, c as f32 / 8.0), "two rescales");
    }
}

#[test]
fn low_children_follow_their_parent() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    for _ in 0..200 {
        let code = prog(&synth::random_mopp(&mut rng, 6));
        let t = tree(&code);
        for (i, n) in t.nodes().iter().enumerate() {
            let low = match *n {
                Node::Split { low, .. } | Node::Diagonal { low, .. } => low,
                Node::Bound { next, .. } => next,
                _ => NONE,
            };
            assert!(low == NONE || low as usize == i + 1, "{n:?} at {i}");
        }
    }
}

#[test]
fn random_programs_match_the_model() {
    let mut answered = 0;
    let mut boxes = 0;
    for n in 0..600u64 {
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ n.wrapping_mul(0x2545_f491_4f6c_dd1d) | 1);
        let code = prog(&synth::random_mopp(&mut rng, 1 + (n % 7) as u32));
        let t = tree(&code);
        assert!(!t.guest_only());
        for _ in 0..60 {
            boxes += 1;
            answered += u32::from(same(&code, &t, &random_box(&mut rng), &format!("program {n}")));
        }
    }
    assert!(answered * 2 > boxes, "{answered} of {boxes} answered");
}

#[test]
fn compiled_programs_match_the_model() {
    let mut rng = Rng(0x6a09_e667_f3bc_c908);
    for n in 0..60 {
        let surfaces: Vec<Surface> = (0..1 + rng.below(40))
            .map(|k| {
                let base = [rng.eighths(64), rng.eighths(64), rng.eighths(16)];
                let mut corner = |d: f32| [base[0] + d, base[1] + rng.eighths(4).abs(), base[2] + d / 2.0];
                Surface {
                    key: ::mopp::geom::structure_key(k as usize),
                    vertices: vec![base, corner(1.0), corner(3.0)],
                }
            })
            .collect();
        let code = ::mopp::build(&surfaces);
        let t = tree(&code);
        assert!(!t.guest_only(), "compiled program {n}");
        let mut answered = 0;
        for _ in 0..400 {
            // Near a surface, so the corners stay inside the corner limit
            // however small the program's bounds (and so its scale) are.
            let near = surfaces[rng.below(surfaces.len() as u32) as usize].vertices[0];
            let lo = [near[0] + rng.eighths(4), near[1] + rng.eighths(4), near[2] + rng.eighths(2)];
            let ext = [rng.eighths(8).abs(), rng.eighths(8).abs(), rng.eighths(8).abs()];
            let b = Aabb {
                min: lo,
                max: [lo[0] + ext[0], lo[1] + ext[1], lo[2] + ext[2]],
            };
            answered += u32::from(same(&code, &t, &b, &format!("compiled {n}")));
        }
        assert!(answered > 300, "compiled {n}: {answered} answered");
    }
}

#[test]
fn traps_bail_only_where_the_original_faults() {
    // A bad opcode on the high side only.
    let code = prog(&[0x10, 10, 20, 1, 0x31, 0x08]);
    let t = tree(&code);
    assert_eq!(keys(&t, &code, on_x(1.0, 2.0)), [1]);
    let q = corners(&code, &on_x(25.0, 30.0), DOUBLE).unwrap();
    assert_eq!(t.query_box(&q, usize::MAX), Err(Bail::Data));
    assert!(::mopp::query_box(&code, &on_x(25.0, 30.0)).is_err());
    for (label, bytecode) in [
        ("property slot 1", &[0x61, 1][..]),
        ("operand past the end", &[0x26, 0][..]),
        ("terminal operand past the end", &[0x51, 1][..]),
        ("jump past the end", &[0x05, 9][..]),
        ("rescale past shift 0", &[0x04, 0, 0, 0, 0x04, 0, 0, 0, 0x04, 0, 0, 0, 0x04, 0, 0, 0, 0x01, 0, 0, 0][..]),
    ] {
        let code = prog(bytecode);
        let t = tree(&code);
        let b = on_x(0.0, 0.0);
        let q = corners(&code, &b, DOUBLE).unwrap();
        assert_eq!(t.query_box(&q, usize::MAX), Err(Bail::Data), "{label}");
        assert!(::mopp::query_box(&code, &b).is_err(), "{label}");
    }
}

#[test]
fn a_program_past_the_decode_bound_is_guest_only() {
    // Each level's two sides both reach the next level: 2^14 paths from 85
    // bytes.
    let mut bytecode = Vec::new();
    for _ in 0..14 {
        bytecode.extend([0x10, 0, 255, 2, 0x05, 0x00]);
    }
    bytecode.push(0x31);
    let code = prog(&bytecode);
    let t = tree(&code);
    assert!(t.guest_only());
    assert!(t.nodes().is_empty());
    let q = corners(&code, &on_x(0.0, 0.0), DOUBLE).unwrap();
    assert_eq!(t.query_box(&q, usize::MAX), Err(Bail::Data));
    assert_eq!(t.query_packet(&[q]), [Err(Bail::Data)]);
}

#[test]
fn inverted_and_far_boxes_bail() {
    let code = prog(&[0x20, 10, 1, 0x31, 0x32]);
    let t = tree(&code);
    let inverted = Corners {
        lo: [cell(12.0) as i32, 0, 0],
        hi: [cell(9.0) as i32, 0, 0],
    };
    assert_eq!(t.query_box(&inverted, usize::MAX), Err(Bail::Data));
    let far = Corners {
        lo: [-CORNER_LIMIT - 1, 0, 0],
        hi: [0, 0, 0],
    };
    assert_eq!(t.query_box(&far, usize::MAX), Err(Bail::Data));
    let edge = Corners {
        lo: [-CORNER_LIMIT, 0, 0],
        hi: [CORNER_LIMIT - 1, 0, 0],
    };
    assert!(t.query_box(&edge, usize::MAX).is_ok());
}

#[test]
fn room_for_exactly_the_keys_answers_and_one_less_grows() {
    let code = prog(&[0x20, 10, 1, 0x31, 0x32]);
    let t = tree(&code);
    let q = corners(&code, &on_x(5.0, 12.0), DOUBLE).unwrap();
    assert_eq!(t.query_box(&q, 2).unwrap().keys, [1, 2]);
    assert_eq!(t.query_box(&q, 1), Err(Bail::Grow));
    assert_eq!(t.query_box(&q, 0), Err(Bail::Grow));
    // No keys needs no room.
    let q = corners(&code, &on_x(0.0, 0.0), DOUBLE).unwrap();
    let code0 = prog(&[0x26, 100, 120, 0x31]);
    let q0 = corners(&code0, &on_x(0.0, 0.0), DOUBLE).unwrap();
    assert_eq!(tree(&code0).query_box(&q0, 0).unwrap().keys, Vec::<u32>::new());
    assert_eq!(t.query_box(&q, 1).unwrap().keys, [1]);
}

/// Splits nested `depth` deep on their low sides, each visiting both
/// sides for any box in root cell 0: wide splits low first, high a key.
fn nested(depth: u32) -> Vec<u8> {
    let mut inner = vec![0x30];
    for _ in 0..depth {
        let n = inner.len() as u16;
        let mut level = vec![0x23, 255, 0, 0, 0];
        level.extend(n.to_be_bytes());
        level.extend(inner);
        level.push(0x31);
        inner = level;
    }
    inner
}

#[test]
fn frames_count_the_original_self_calls() {
    let b = on_x(0.0, 0.0);
    for depth in [0, 1, 2, 7] {
        let code = prog(&nested(depth));
        let t = tree(&code);
        let found = t.query_box(&corners(&code, &b, DOUBLE).unwrap(), usize::MAX).unwrap();
        assert_eq!(found.frames, depth);
        assert_eq!(found.keys.len() as u32, depth + 1);
        assert_eq!(Ok(found.keys), ::mopp::query_box(&code, &b));
    }
    // An empty low side is still a call.
    let code = prog(&[0x10, 255, 0, 1, 0x00, 0x31]);
    let found = tree(&code)
        .query_box(&corners(&code, &b, DOUBLE).unwrap(), usize::MAX)
        .unwrap();
    assert_eq!((found.keys, found.frames), (vec![1], 1));
    // One side only is not.
    let code = prog(&[0x10, 255, 255, 1, 0x30, 0x31]);
    let found = tree(&code)
        .query_box(&corners(&code, &b, DOUBLE).unwrap(), usize::MAX)
        .unwrap();
    assert_eq!((found.keys, found.frames), (vec![0], 0));
    let code = prog(&nested(MAX_FRAMES));
    let q = corners(&code, &b, DOUBLE).unwrap();
    assert_eq!(tree(&code).query_box(&q, usize::MAX).unwrap().frames, MAX_FRAMES);
    let code = prog(&nested(MAX_FRAMES + 1));
    let q = corners(&code, &b, DOUBLE).unwrap();
    let tree = tree(&code);
    assert_eq!(tree.query_box(&q, usize::MAX), Err(Bail::Depth));
    let high = corners(&code, &on_x(300.0, 301.0), DOUBLE).unwrap();
    assert_eq!(tree.query_packet(&[q, high]), [Err(Bail::Depth), Ok(vec![1])]);
}

#[test]
fn packets_equal_serial_queries() {
    for n in 0..300u64 {
        let mut rng = Rng(0xbb67_ae85_84ca_a73b ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let code = prog(&synth::random_mopp(&mut rng, 1 + (n % 6) as u32));
        let t = tree(&code);
        let size = 1 + rng.below(8) as usize;
        let boxes: Vec<Corners> = (0..size)
            .map(|_| loop {
                if let Ok(q) = corners(&code, &random_box(&mut rng), DOUBLE) {
                    break q;
                }
            })
            .collect();
        let packet = t.query_packet(&boxes);
        for (i, q) in boxes.iter().enumerate() {
            let serial = t.query_box(q, usize::MAX).map(|f| f.keys);
            assert_eq!(packet[i], serial, "program {n} box {i}");
        }
    }
}

#[test]
fn conversion_follows_the_x87_precision() {
    // origin 0.1 and a coordinate whose difference rounds differently in
    // binary32 and binary64 before the multiply.
    let origin = [0.1f32.to_bits(), 0, 0];
    let mut rng = Rng(0x3c6e_f372_fe94_f82b);
    for _ in 0..2000 {
        let p = (rng.below(1 << 20) as f32) * 1.37 + 0.003;
        let scale = 1.0 + rng.below(4096) as f32 / 512.0;
        let single = {
            let v = (p - 0.1f32) * scale;
            v.round_ties_even() as i32 - 1
        };
        let double = {
            let v = ((f64::from(p) - f64::from(0.1f32)) * f64::from(scale)) as f32;
            v.round_ties_even() as i32 - 1
        };
        let min = [p.to_bits(), 0, 0];
        for (control, want) in [(SINGLE, single), (DOUBLE, double)] {
            let c = Corners::convert(min, min, origin, scale.to_bits(), control).unwrap();
            assert_eq!(c.lo[0], want, "{p} * {scale} control {control:#x}");
            assert_eq!(c.hi[0], want);
            assert_eq!((c.lo[1], c.hi[2]), (-1, -1));
        }
    }
}

#[test]
fn conversion_bails_off_the_checked_path() {
    let one = 1.0f32.to_bits();
    let ok = [one; 3];
    let convert = |min: [u32; 3], control| Corners::convert(min, ok, [0; 3], one, control);
    assert!(convert(ok, DOUBLE).is_ok());
    assert_eq!(convert(ok, 0x037f), Err(Bail::Float), "extended precision");
    assert_eq!(convert(ok, 0x067f), Err(Bail::Float), "round down");
    assert_eq!(convert(ok, 0x027e), Err(Bail::Float), "invalid unmasked");
    assert_eq!(convert([f32::NAN.to_bits(), one, one], DOUBLE), Err(Bail::Float));
    assert_eq!(convert([f32::INFINITY.to_bits(), one, one], DOUBLE), Err(Bail::Float));
    assert_eq!(convert([1, one, one], DOUBLE), Err(Bail::Float), "subnormal");
    assert_eq!(convert([3.0e9f32.to_bits(), one, one], DOUBLE), Err(Bail::Float), "fistp range");
    // Zero is on the path: 0 - 0 rounds to 0, minus one.
    let c = Corners::convert([0; 3], [0; 3], [0; 3], one, DOUBLE).unwrap();
    assert_eq!(c.lo, [-1; 3]);
}

/// Under the guest's control word nothing on the checked path's list
/// declines: each runs as the guest's x87 does.
#[test]
fn guest_conversion_runs_what_the_checked_path_declines() {
    let one = 1.0f32.to_bits();
    let ok = [one; 3];
    let convert = |min: [u32; 3], control| Corners::convert_guest(min, ok, [0; 3], one, control).unwrap();
    // On the checked path both agree, PE clear.
    let plain = Corners::convert(ok, ok, [0; 3], one, DOUBLE).unwrap();
    assert_eq!(convert(ok, DOUBLE), (plain, 0));
    // Extended precision and round down run.
    assert_eq!(convert(ok, 0x037f).0, plain);
    assert_eq!(convert(ok, 0x067f).0, plain);
    // A NaN or out-of-range value stores the indefinite, minus one, with IE.
    for bad in [f32::NAN.to_bits(), f32::INFINITY.to_bits(), 3.0e9f32.to_bits()] {
        let (c, flags) = convert([bad, one, one], DOUBLE);
        assert_eq!(c.lo[0], i32::MIN.wrapping_sub(1), "{bad:#x}");
        assert_ne!(flags & 1, 0, "{bad:#x}");
    }
    // A subnormal loads with DE and converts to zero, minus one.
    let (c, flags) = convert([1, one, one], DOUBLE);
    assert_eq!(c.lo[0], -1);
    assert_ne!(flags & 2, 0);
}

const CODE: u32 = 0x2000;

fn image_with(code: &Code) -> Image {
    let block = code.to_block();
    let mut img = Image::new(0, CODE as usize + block.len() + 0x100);
    img.put(CODE, &block);
    img
}

#[test]
fn the_world_decodes_once_and_checks_moved_sources() {
    static EPOCH: Epoch = Epoch::new();
    let world = CollisionWorld::new(&EPOCH);
    let code = prog(&[0x10, 10, 20, 1, 0x31, 0x32]);
    let mut img = image_with(&code);
    let first = world.mopp(&img, CODE, 0).unwrap();
    assert_eq!(first.code(), CODE);
    let again = world.mopp(&img, CODE, 0).unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &again));
    assert_eq!((world.stats().decodes, world.stats().mopps), (1, 1));
    // The reference count is not part of the program.
    img.word(CODE + REFCOUNT, 7);
    assert!(std::sync::Arc::ptr_eq(&first, &world.mopp(&img, CODE, 1).unwrap()));
    assert_eq!(world.stats().decodes, 1, "a moved revision with the same bytes keeps the tree");
    // A bytecode edit under a new revision decodes again.
    img.put(CODE + HEADER_BYTES + 4, &[0x33]);
    let edited = world.mopp(&img, CODE, 2).unwrap();
    assert_eq!(world.stats().decodes, 2);
    let q = corners(&code, &on_x(1.0, 1.0), DOUBLE).unwrap();
    assert_eq!(edited.query_box(&q, 8).unwrap().keys, [3]);
    // A header edit is seen even under the same revision.
    img.float(CODE + 0xc, 2.0);
    world.mopp(&img, CODE, 2).unwrap();
    assert_eq!(world.stats().decodes, 3);
    assert!(world.stats().array_bytes >= edited.footprint() as u64);
    EPOCH.bump();
    world.mopp(&img, CODE, 2).unwrap();
    assert_eq!((world.stats().decodes, world.stats().mopps), (4, 1));
}

#[test]
fn the_world_refuses_bad_headers_and_counts_guest_only_programs() {
    static EPOCH: Epoch = Epoch::new();
    let world = CollisionWorld::new(&EPOCH);
    let mut img = image_with(&prog(&[0x31]));
    img.word(CODE + SIZE, 0x10);
    assert_eq!(world.mopp(&img, CODE, 0).err(), Some(Bail::Data));
    img.word(CODE + SIZE, 0x30 + 0x10_0000);
    assert_eq!(world.mopp(&img, CODE, 0).err(), Some(Bail::Fault));
    let mut bytecode = Vec::new();
    for _ in 0..14 {
        bytecode.extend([0x10, 0, 255, 2, 0x05, 0x00]);
    }
    bytecode.push(0x31);
    let img = image_with(&prog(&bytecode));
    assert!(world.mopp(&img, CODE, 0).unwrap().guest_only());
    let stats = world.stats();
    assert_eq!((stats.decodes, stats.decode_failures), (1, 1));
    assert_eq!(Entry::MoppBox.addr(), 0x0030_0150);
    assert_eq!(Entry::MoppBox.name(), "mopp_box");
}

#[test]
fn the_world_holds_programs_inside_its_budget() {
    static EPOCH: Epoch = Epoch::new();
    let world = CollisionWorld::with_budget(&EPOCH, 64);
    let img = image_with(&prog(&[0x10, 10, 20, 1, 0x31, 0x32]));
    assert_eq!(world.mopp(&img, CODE, 0).err(), Some(Bail::Budget));
    assert_eq!(world.stats().mopps, 0);
}
