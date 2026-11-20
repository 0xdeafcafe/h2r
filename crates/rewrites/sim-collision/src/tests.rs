//! Hand-built BSPs for every leaf-state pattern, the matcher's paths and
//! the decoder's refusals. The guest oracle is the engine's differential
//! test; these pin the semantics on data small enough to reason about.
use crate::bsp::{Bsp, NONE, block};
use crate::synth::{self, Builder, CONSTS, Image, Rng, SOLID, leaf, node, surface};
use crate::{Bail, CollisionWorld, Entry, Hit, Segment};
use sim_core::Epoch;

const AT: u32 = 0x1000;
const MASK: u32 = 0x200;

fn image(b: &Builder) -> Image {
    let mut img = Image::new(0, AT as usize + b.size() as usize + 0x100);
    b.write(AT, |a, d| img.put(a, d));
    img
}

fn decode(img: &Image) -> Bsp {
    let header = Bsp::read_header(img, AT).unwrap();
    Bsp::decode(img, AT, header, &CONSTS).unwrap()
}

fn bits(v: [f32; 3]) -> [u32; 3] {
    v.map(f32::to_bits)
}

fn query(start: [f32; 3], delta: [f32; 3], flags: u32) -> Segment {
    Segment {
        flags,
        breakable_count: 0x100,
        mask: 0,
        start: bits(start),
        delta: bits(delta),
        fraction: 1.0f32.to_bits(),
        listed: 0,
        prev_leaf: NONE,
        prev_kind: 0,
        plane: NONE,
    }
}

fn run(img: &Image, bsp: &Bsp, q: &Segment) -> Result<crate::Outcome, Bail> {
    bsp.vector_test(img, q, 0, 0, 1.0f32.to_bits())
}

const SQUARE: [[f32; 3]; 4] = [
    [0.0, -4.0, -4.0],
    [0.0, 4.0, -4.0],
    [0.0, 4.0, 4.0],
    [0.0, -4.0, 4.0],
];

/// The plane `x = 0`: leaf 0 (empty, one reference straight to surface 0)
/// behind it, solid in front.
fn wall(surface_flags: u8) -> Builder {
    let mut b = Builder::default();
    b.planes.push([1.0, 0.0, 0.0, 0.0]);
    b.nodes.push((0, leaf(0), SOLID));
    b.leaves.push((0, 1, 0));
    b.refs.push((0, surface(0)));
    let edge = b.ring(0, &SQUARE, false);
    b.surfaces.push((0, edge, surface_flags, 0, 7));
    b
}

/// The plane `x = 0` with two-sided leaves on both sides, each with a
/// flipped reference onto it (so both project the same way), and surface
/// 0's ring the square around the origin.
fn two_sided(surface_flags: u8, breakable: u8, reversed: bool) -> Builder {
    let mut b = Builder::default();
    b.planes.push([1.0, 0.0, 0.0, 0.0]);
    b.nodes.push((0, leaf(0), leaf(1)));
    b.leaves.push((1, 1, 0));
    b.leaves.push((1, 1, 1));
    b.refs.push((0x8000, surface(0)));
    b.refs.push((0x8000, surface(0)));
    let edge = b.ring(0, &SQUARE, reversed);
    b.surfaces.push((0x8000, edge, surface_flags, breakable, 3));
    b
}

#[test]
fn front_facing_hit_looks_up_the_previous_leaf() {
    let img = image(&wall(0));
    let bsp = decode(&img);
    let out = run(&img, &bsp, &query([-2.0, 0.0, 0.0], [4.0, 0.0, 0.0], 1)).unwrap();
    assert_eq!(
        out.hit,
        Some(Hit {
            fraction: 0.5f32.to_bits(),
            plane_addr: bsp.block_addr(block::PLANES),
            leaf: 0,
            surface: 0,
            plane: 0,
            flags: 0,
            breakable: 0,
            material: 7,
        })
    );
    assert_eq!(out.leaves, [0]);
    assert_eq!((out.prev_leaf, out.prev_kind, out.plane), (0, 1, 0));
    assert!(!out.inexact);
}

#[test]
fn back_facing_hit_needs_query_bit_one() {
    let img = image(&wall(0));
    let bsp = decode(&img);
    let back = query([2.0, 0.0, 0.0], [-4.0, 0.0, 0.0], 2);
    let out = run(&img, &bsp, &back).unwrap();
    assert_eq!(
        out.hit.map(|h| (h.leaf, h.fraction)),
        Some((0, 0.5f32.to_bits()))
    );
    assert!(out.leaves.is_empty());

    let out = run(&img, &bsp, &Segment { flags: 1, ..back }).unwrap();
    assert_eq!(out.hit, None);
    assert_eq!(out.leaves, [0]);
    assert_eq!((out.prev_leaf, out.prev_kind), (0, 1));
}

#[test]
fn a_missed_lookup_falls_through_into_solid() {
    let mut b = wall(0);
    b.refs[0] = (0, 0xffff);
    let img = image(&b);
    let bsp = decode(&img);
    let out = run(&img, &bsp, &query([-2.0, 0.0, 0.0], [4.0, 0.0, 0.0], 1)).unwrap();
    assert_eq!(out.hit, None);
    assert_eq!(out.leaves, [0]);
    assert_eq!((out.prev_leaf, out.prev_kind), (NONE, 3));
}

#[test]
fn surface_filters_skip_flagged_surfaces() {
    for (surface_flags, query_bits, hit) in [
        (2, 8, false),
        (2, 0x10, true),
        (8, 0x10, false),
        (8, 8, true),
    ] {
        let img = image(&wall(surface_flags));
        let bsp = decode(&img);
        let out = run(
            &img,
            &bsp,
            &query([-2.0, 0.0, 0.0], [4.0, 0.0, 0.0], 1 | query_bits),
        )
        .unwrap();
        assert_eq!(out.hit.is_some(), hit, "{surface_flags:#x} {query_bits:#x}");
    }
}

#[test]
fn two_sided_hits_test_the_ring_in_either_edge_direction() {
    for reversed in [false, true] {
        let img = image(&two_sided(0, 0, reversed));
        let bsp = decode(&img);
        let inside = query([-2.0, 0.0, 0.0], [4.0, 0.0, 0.0], 0);
        let out = run(&img, &bsp, &inside).unwrap();
        assert_eq!(out.hit.map(|h| (h.leaf, h.plane)), Some((1, 0xffff_8000)));
        assert_eq!(out.leaves, [0]);

        // Under bit 0 the lookup uses the previous leaf instead.
        let out = run(&img, &bsp, &Segment { flags: 1, ..inside }).unwrap();
        assert_eq!(out.hit.map(|h| h.leaf), Some(0));

        let outside = query([-2.0, 10.0, 0.0], [4.0, 0.0, 0.0], 0);
        let out = run(&img, &bsp, &outside).unwrap();
        assert_eq!(out.hit, None);
        assert_eq!(out.leaves, [0, 1]);

        let off = run(&img, &bsp, &Segment { flags: 4, ..inside }).unwrap();
        assert_eq!(off.hit, None);
        assert_eq!(off.leaves, [0, 1]);
    }
}

#[test]
fn a_cleared_breakable_bit_hides_the_surface() {
    let mut img = image(&two_sided(8, 3, false));
    let bsp = decode(&img);
    let q = Segment {
        mask: MASK,
        ..query([-2.0, 0.0, 0.0], [4.0, 0.0, 0.0], 0)
    };
    img.word(MASK, !(1 << 3));
    let out = run(&img, &bsp, &q).unwrap();
    assert_eq!(out.hit, None);
    assert_eq!(out.mask_reads, [MASK]);
    img.word(MASK, 1 << 3);
    assert!(run(&img, &bsp, &q).unwrap().hit.is_some());
    // An index at or above the count is not tested.
    img.word(MASK, 0);
    let out = run(
        &img,
        &bsp,
        &Segment {
            breakable_count: 3,
            ..q
        },
    )
    .unwrap();
    assert!(out.hit.is_some());
    assert!(out.mask_reads.is_empty());
}

#[test]
fn the_leaf_list_overflows_into_its_last_slot() {
    let img = image(&two_sided(0, 0, false));
    let bsp = decode(&img);
    let out = run(&img, &bsp, &query([-2.0, 10.0, 0.0], [4.0, 0.0, 0.0], 0)).unwrap();
    assert_eq!(out.leaves, [0, 1]);
    let (count, writes) = out.list_writes(0xff);
    assert_eq!(count, 0x100);
    assert_eq!(writes, [(0x1c + 4 * 0xff, 0), (0x418, 1)]);
    let negative = Segment {
        listed: u32::MAX,
        ..query([-2.0, 10.0, 0.0], [4.0, 0.0, 0.0], 0)
    };
    assert_eq!(run(&img, &bsp, &negative), Err(Bail::Data));
}

#[test]
fn the_far_side_needs_a_split_below_the_fraction() {
    let img = image(&wall(0));
    let bsp = decode(&img);
    let q = Segment {
        fraction: 0.5f32.to_bits(),
        ..query([-2.0, 0.0, 0.0], [4.0, 0.0, 0.0], 1)
    };
    let out = run(&img, &bsp, &q).unwrap();
    assert_eq!(out.hit, None);
    assert_eq!(out.leaves, [0]);
    assert_eq!(
        out.plane, NONE,
        "the plane is recorded only on the far side"
    );
}

#[test]
fn deep_nesting_bails() {
    let mut b = Builder::default();
    b.leaves.push((0, 0, 0xffff));
    for k in 0..300u32 {
        b.planes.push([1.0, 0.0, 0.0, (300 - k) as f32]);
        let back = if k == 299 { leaf(0) } else { node(k + 1) };
        b.nodes.push((k as u16, back, leaf(0)));
    }
    let img = image(&b);
    let bsp = decode(&img);
    let q = query([-1.0, 0.0, 0.0], [400.0, 0.0, 0.0], 0);
    assert_eq!(run(&img, &bsp, &q), Err(Bail::Depth));
}

#[test]
fn operands_off_the_fast_path_bail() {
    let img = image(&wall(0));
    let bsp = decode(&img);
    for bad in [f32::NAN, f32::INFINITY, 1e-30] {
        let q = query([bad, 0.0, 0.0], [4.0, 0.0, 0.0], 1);
        assert_eq!(run(&img, &bsp, &q), Err(Bail::Float), "{bad}");
        assert_eq!(
            bsp.point_to_leaf(0, bits([bad, 0.0, 0.0])),
            Err(Bail::Float)
        );
    }
}

#[test]
fn inexact_splits_are_reported() {
    let img = image(&wall(0));
    let bsp = decode(&img);
    let out = run(&img, &bsp, &query([-1.0, 0.0, 0.0], [3.0, 0.0, 0.0], 1)).unwrap();
    assert!(out.inexact);
    assert_eq!(out.hit.unwrap().fraction, (-(-1.0f32 / 3.0)).to_bits());
}

#[test]
fn point_to_leaf_takes_the_front_on_the_plane() {
    let img = image(&wall(0));
    let bsp = decode(&img);
    assert_eq!(bsp.point_to_leaf(0, bits([-1.0, 0.0, 0.0])), Ok(0));
    assert_eq!(bsp.point_to_leaf(0, bits([0.0, 0.0, 0.0])), Ok(NONE));
    assert_eq!(bsp.point_to_leaf(0, bits([-0.0, 3.0, 0.0])), Ok(NONE));
    // The start word is masked to 23 bits even when it is a leaf word.
    assert_eq!(bsp.point_to_leaf(leaf(0), bits([-1.0, 0.0, 0.0])), Ok(0));
}

#[test]
fn projection_rows_follow_axis_sign_and_flip() {
    let mut b = Builder::default();
    let normals = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 0.5, 0.0],
        [0.0, -0.5, 0.25],
        [0.0, 0.0, 1.0],
        [0.6, -0.6, -0.6],
    ];
    for n in normals {
        b.planes.push([n[0], n[1], n[2], 0.0]);
    }
    for p in 0..normals.len() as u16 {
        b.refs.push((p, 0));
        b.refs.push((p | 0x8000, 0));
    }
    let bsp = decode(&image(&b));
    let rows: Vec<u8> = bsp.refs.iter().map(|r| r.row).collect();
    // Ties go to z, then y.
    assert_eq!(rows, [1, 0, 0, 1, 3, 2, 2, 3, 5, 4, 4, 5]);
}

#[test]
fn the_decoder_marks_what_the_guest_would_read_out_of_range() {
    // A node plane past the planes.
    let mut b = wall(0);
    b.nodes[0].0 = 5;
    let img = image(&b);
    let bsp = decode(&img);
    let q = query([-2.0, 0.0, 0.0], [4.0, 0.0, 0.0], 1);
    assert_eq!(run(&img, &bsp, &q), Err(Bail::Data));
    assert_eq!(bsp.point_to_leaf(0, bits([0.0; 3])), Err(Bail::Data));

    // A ring that never comes back to its first edge.
    let mut b = two_sided(0, 0, false);
    b.edges[3][2] = 1;
    let img = image(&b);
    let bsp = decode(&img);
    assert_eq!(bsp.surfaces[0].ring, NONE);
    assert_eq!(
        run(&img, &bsp, &query([-2.0, 0.0, 0.0], [4.0, 0.0, 0.0], 0)),
        Err(Bail::Data)
    );

    // Counts past the cap are refused at decode.
    let mut img = image(&wall(0));
    img.word(AT, 1 << 30);
    let header = Bsp::read_header(&img, AT).unwrap();
    assert_eq!(
        Bsp::decode(&img, AT, header, &CONSTS).unwrap_err(),
        Bail::Data
    );
}

#[test]
fn the_source_hash_sees_a_changed_byte() {
    let mut img = image(&wall(0));
    let bsp = decode(&img);
    assert!(bsp.verify(&img));
    let surfaces = bsp.block_addr(block::SURFACES);
    img.put(surfaces + 6, &[9]);
    assert!(!bsp.verify(&img));
}

#[test]
fn the_world_caches_by_pointer_and_header_and_drops_on_the_epoch() {
    static EPOCH: Epoch = Epoch::new();
    let world = CollisionWorld::new(&EPOCH);
    let mut img = image(&wall(0));
    let consts = || Ok(CONSTS);
    let a = world.bsp(&img, AT, consts).unwrap();
    let b = world.bsp(&img, AT, || panic!("no second decode")).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b));

    // The same pointer with another header rebuilds.
    img.word(AT, 0);
    let c = world.bsp(&img, AT, consts).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&a, &c));
    assert!(c.nodes.is_empty());

    EPOCH.bump();
    let d = world.bsp(&img, AT, consts).unwrap();
    assert!(!std::sync::Arc::ptr_eq(&c, &d));

    // A refused decode is remembered until the header changes.
    img.word(AT, 1 << 30);
    assert_eq!(world.bsp(&img, AT, consts).unwrap_err(), Bail::Data);
    assert_eq!(
        world.bsp(&img, AT, || panic!("remembered")).unwrap_err(),
        Bail::Data
    );

    world.record(Entry::VectorTest, &Ok(()));
    world.record::<()>(Entry::VectorTest, &Err(Bail::Float));
    world.record_precision(0x027f);
    let s = world.stats();
    assert_eq!(s.decodes, 4);
    assert_eq!(s.decode_failures, 1);
    assert_eq!(s.flushes, 1);
    assert_eq!(s.entries[Entry::VectorTest as usize].native, 1);
    assert_eq!(
        s.entries[Entry::VectorTest as usize].bails[Bail::Float as usize],
        1
    );
    assert_eq!(s.precision, [0, 0, 1, 0]);
}

/// Random BSPs and segments: every query answers or bails, answers are
/// repeatable, and a hit names records that exist.
#[test]
fn random_bsps_answer_consistently() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut answered = 0;
    for case in 0..400 {
        let b = synth::random(&mut rng, 1 + case % 6);
        let img = image(&b);
        let bsp = decode(&img);
        for _ in 0..16 {
            let start = [rng.eighths(12), rng.eighths(12), rng.eighths(12)];
            let delta = [rng.eighths(24), rng.eighths(24), rng.eighths(24)];
            let q = query(start, delta, rng.below(32));
            let first = run(&img, &bsp, &q);
            assert_eq!(first, run(&img, &bsp, &q));
            if let Ok(out) = first {
                answered += 1;
                if let Some(h) = out.hit {
                    assert!((h.surface as usize) < bsp.surfaces.len());
                    assert!((h.leaf as usize) < bsp.leaves.len());
                }
                assert!(out.leaves.iter().all(|&l| (l as usize) < bsp.leaves.len()));
            }
            let p = bsp.point_to_leaf(0, bits(start));
            assert!(matches!(p, Ok(l) if l == NONE || (l as usize) < bsp.leaves.len()));
        }
    }
    assert!(answered > 3200, "{answered} of 6400");
}
