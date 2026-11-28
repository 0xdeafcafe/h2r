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
fn nan_and_subnormal_normals_get_the_guest_row_and_its_flags() {
    let mut b = Builder::default();
    let tiny = f32::from_bits(1);
    b.planes.push([tiny, 0.0, 1.0, 0.0]);
    b.planes.push([f32::NAN, 0.0, 0.0, 0.0]);
    for p in 0..2u16 {
        b.refs.push((p, 0));
        b.refs.push((p | 0x8000, 0));
    }
    let bsp = decode(&image(&b));
    let got: Vec<(u8, u8)> = bsp.refs.iter().map(|r| (r.row, r.flags)).collect();
    // z (the `comiss |z|, |x|` raises DE, the `fld` of x DE on the x87);
    // then x: every compare with the NaN is unordered, its `comiss` IE.
    assert_eq!(got, [(5, 0x22), (4, 0x22), (0, 0x01), (1, 0x01)]);
}

#[test]
fn the_matcher_raises_a_subnormal_normal_under_the_guest_state() {
    let mut b = wall(0);
    b.planes[0][1] = f32::from_bits(1);
    let img = image(&b);
    let bsp = decode(&img);
    let q = query([-1.0, 0.0, 0.0], [4.0, 0.0, 0.0], 1);
    assert_eq!(run(&img, &bsp, &q), Err(Bail::Float));
    let out = bsp
        .vector_test_under(0x1f80, 0x027f, &img, &q, 0, 0, 1.0f32.to_bits())
        .unwrap();
    assert_eq!(out.hit.unwrap().fraction, 0.25f32.to_bits());
    // Only the matcher's `fld` of the normal loads it on the x87.
    assert_ne!(out.x87_flags & 0x02, 0);
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
fn a_same_side_cycle_declines_without_recursing() {
    let mut b = wall(0);
    b.nodes[0].2 = node(0);
    let img = image(&b);
    let bsp = decode(&img);
    // Both endpoints are in front. The tail walk would revisit node 0
    // forever without ever incrementing the split recursion depth.
    assert_eq!(run(&img, &bsp, &query([2.0, 0.0, 0.0], [0.0; 3], 0)), Err(Bail::Data));
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
    world.observe_environment(Entry::VectorTest, 0x1fa0, 0x027f);
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

/// Interleaved arrivals compete to claim the fast bins and also reach the
/// sparse tail. Full words (including sticky flags) remain distinguishable.
#[test]
fn entry_environment_counts_every_word_once() {
    static EPOCH: Epoch = Epoch::new();
    let world = CollisionWorld::new(&EPOCH);
    std::thread::scope(|scope| {
        for offset in 0..4u32 {
            let world = &world;
            scope.spawn(move || {
                for i in 0..32 {
                    let k = (i + offset) % 32;
                    let mxcsr = 0x1f80 | k;
                    world.observe_environment(Entry::PointToLeaf, mxcsr, 0x037f);
                    world.observe_environment(Entry::InstanceTrace, mxcsr, 0x0200 | k as u16);
                }
            });
        }
    });
    let stats = world.stats();
    let point = &stats.entries[Entry::PointToLeaf as usize];
    let instance = &stats.entries[Entry::InstanceTrace as usize];
    for k in 0..32 {
        assert_eq!(point.mxcsr[&(0x1f80 | k)], 4);
        assert_eq!(instance.mxcsr[&(0x1f80 | k)], 4);
        assert_eq!(instance.x87_control[&(0x0200 | k as u16)], 4);
    }
    assert_eq!(point.x87_control[&0x037f], 128);
    assert_eq!(stats.precision, [0; 4]);
    assert_eq!(instance.native, 0);
    assert!(stats.entries[Entry::Location as usize].mxcsr.is_empty());
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

#[test]
fn an_old_decode_cannot_replace_the_new_generation_at_the_same_address() {
    static EPOCH: Epoch = Epoch::new();
    let world = CollisionWorld::new(&EPOCH);
    let old = image(&wall(0));
    let new = image(&wall(8));
    let late = world.bsp(&old, AT, || {
        EPOCH.bump();
        world.bsp(&new, AT, || Ok(CONSTS)).unwrap();
        Ok(CONSTS)
    });
    assert_eq!(late.unwrap_err(), Bail::Data);
    let retained = world.bsp(&new, AT, || panic!("already decoded")).unwrap();
    assert!(retained.verify(&new));
    assert!(!retained.verify(&old));
}

#[test]
fn an_alias_decline_invalidates_geometry_before_the_lifted_write() {
    static EPOCH: Epoch = Epoch::new();
    let world = CollisionWorld::new(&EPOCH);
    let mut mem = image(&wall(0));
    let before = world.bsp(&mem, AT, || Ok(CONSTS)).unwrap();
    world.record::<()>(Entry::Location, &Err(Bail::Alias));
    mem.word(before.block_addr(block::PLANES) + 12, 5.0f32.to_bits());
    let after = world.bsp(&mem, AT, || Ok(CONSTS)).unwrap();
    assert_eq!(after.point_to_leaf(0, bits([2.0, 0.0, 0.0])), Ok(0));
    assert_ne!(before.source_hash(), after.source_hash());
}

#[test]
fn a_replaced_guest_address_space_cannot_hit_another_bsps_header() {
    struct View(Image, (u64, u64));
    impl crate::GuestRead for View {
        fn memory_revision(&self) -> (u64, u64) {
            self.1
        }
        fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), crate::Fault> {
            crate::GuestRead::read(&self.0, addr, dst)
        }
    }
    static EPOCH: Epoch = Epoch::new();
    let world = CollisionWorld::new(&EPOCH);
    let first = View(image(&wall(0)), (1, 0));
    let retained = world.bsp(&first, AT, || Ok(CONSTS)).unwrap();
    for revision in [(2, 0), (2, 1)] {
        let replacement = View(image(&wall(8)), revision);
        let current = world.bsp(&replacement, AT, || Ok(CONSTS)).unwrap();
        assert_eq!(retained.header(), current.header());
        assert!(current.verify(&replacement));
        assert!(!current.verify(&first));
    }
    assert_eq!(world.stats().decodes, 3);
}

#[test]
fn header_at_the_end_of_guest_memory_must_not_wrap() {
    use crate::bsp::{HEADER_BYTES, HEADER_WORDS};

    // An extra backing byte lets the synthetic bus accept the wrapping
    // address if the header reader ever skips the guest range check.
    let last = u32::MAX - HEADER_BYTES + 1;
    let mut mem = Image::new(last, HEADER_BYTES as usize + 1);
    let words: [u32; HEADER_WORDS] = std::array::from_fn(|i| 0x0102_0300 + i as u32);
    for (i, word) in words.iter().enumerate() {
        mem.word(last + 4 * i as u32, *word);
    }
    assert_eq!(Bsp::read_header(&mem, last), Ok(words));
    assert_eq!(Bsp::read_header(&mem, last + 1), Err(Bail::Fault));
}

#[test]
fn budget_drops_borrowed_rings_and_preserves_scalar_results() {
    static EPOCH: Epoch = Epoch::new();
    let b = two_sided(0, 0, true);
    let second = AT + b.size() + 0x100;
    let mut mem = Image::new(0, (second + b.size() + 0x100) as usize);
    for at in [AT, second] {
        b.write(at, |a, data| mem.put(a, data));
    }
    let reference = decode(&mem);
    let base = Bsp::essential_bytes(reference.header()).unwrap();
    // Charge both BSP owners and both address records as well as arrays.
    let metadata = 2 * (crate::world::BSP_OWNER_BYTES + crate::world::INDEX_RECORD_BYTES);
    let limit = metadata + (reference.footprint() as u64).max(2 * base);
    let world = CollisionWorld::with_budget(&EPOCH, limit);
    let first = world.bsp(&mem, AT, || Ok(CONSTS)).unwrap();
    assert!(!first.scalar_rings());
    let next = world.bsp(&mem, second, || Ok(CONSTS)).unwrap();
    assert!(first.scalar_rings() && next.scalar_rings());
    assert_eq!(
        first.read_surface_ring(&mem, 0).unwrap(),
        reference.surface_ring(&reference.surfaces[0]).unwrap()
    );
    let stats = world.stats();
    assert_eq!(stats.ring_fallbacks, 1);
    assert_eq!(stats.bsps, 2);
    assert!(stats.bytes <= stats.budget_bytes);
    assert_eq!(
        stats.array_bytes,
        (first.footprint() + next.footprint()) as u64
    );
    assert_eq!(stats.bytes, stats.array_bytes + metadata);
    let warm = world
        .bsp(&mem, AT, || panic!("essential data retained"))
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&first, &warm));
    let mut hit = false;
    for y in [0.0, 4.0, 6.0] {
        for direction in [1.0, -1.0] {
            let q = query([-2.0 * direction, y, 0.0], [4.0 * direction, 0.0, 0.0], 0);
            let want = run(&mem, &reference, &q).unwrap();
            hit |= want.hit.is_some();
            assert_eq!(run(&mem, &first, &q), Ok(want));
        }
    }
    assert!(hit, "the scalar matcher must accept an inside point");
    world.clear();
    let reloaded = world.bsp(&mem, AT, || Ok(CONSTS)).unwrap();
    assert!(
        !reloaded.scalar_rings(),
        "a new generation can retain rings"
    );
}

#[test]
fn budget_declines_an_oversized_header_before_reading_the_blocks() {
    static EPOCH: Epoch = Epoch::new();
    let mem = image(&wall(0));
    let header = Bsp::read_header(&mem, AT).unwrap();
    let limit = Bsp::essential_bytes(&header).unwrap()
        + crate::world::BSP_OWNER_BYTES
        + crate::world::INDEX_RECORD_BYTES
        - 1;
    let world = CollisionWorld::with_budget(&EPOCH, limit);
    for _ in 0..2 {
        assert_eq!(
            world.bsp(&mem, AT, || panic!("no decode")).unwrap_err(),
            Bail::Budget
        );
    }
    let stats = world.stats();
    assert_eq!((stats.bsps, stats.array_bytes, stats.decodes), (0, 0, 1));
    assert_eq!(
        stats.bytes,
        crate::world::INDEX_RECORD_BYTES,
        "refused header is charged"
    );
}

#[test]
fn budget_caps_empty_and_failed_address_slots() {
    static EPOCH: Epoch = Epoch::new();
    let mut mem = Image::new(0, 0x2000);
    mem.word(AT, u32::MAX);
    let world = CollisionWorld::with_limits(&EPOCH, 1024, 2);
    assert_eq!(
        world
            .bsp(&mem, AT, || panic!("invalid header"))
            .unwrap_err(),
        Bail::Data
    );
    world.bsp(&mem, AT + 64, || Ok(CONSTS)).unwrap();
    assert_eq!(
        world.bsp(&mem, AT + 128, || panic!("full")).unwrap_err(),
        Bail::Budget
    );
    assert_eq!(world.stats().array_bytes, 0);
    assert_eq!(
        world.stats().bytes,
        2 * crate::world::INDEX_RECORD_BYTES + crate::world::BSP_OWNER_BYTES
    );
    world.clear();
    assert!(world.bsp(&mem, AT + 128, || Ok(CONSTS)).is_ok());
}

#[test]
fn scalar_ring_rejects_a_cycle_and_out_of_range_vertices() {
    for bad_vertex in [false, true] {
        let mut b = two_sided(0, 0, false);
        if bad_vertex {
            b.edges[0][0] = u16::MAX;
        } else {
            b.edges[3][2] = 1;
        }
        let mem = image(&b);
        let reference = decode(&mem);
        let scalar =
            Bsp::decode_with_ring_limit(&mem, AT, *reference.header(), &CONSTS, 0).unwrap();
        let q = query([-2.0, 0.0, 0.0], [4.0, 0.0, 0.0], 0);
        assert_eq!(run(&mem, &reference, &q), Err(Bail::Data));
        assert_eq!(run(&mem, &scalar, &q), Err(Bail::Data));
    }
}

#[test]
fn budget_is_rechecked_after_a_concurrent_first_decode() {
    static EPOCH: Epoch = Epoch::new();
    let b = wall(0);
    let second = AT + b.size() + 0x80;
    let mut mem = Image::new(0, (second + b.size() + 0x80) as usize);
    for at in [AT, second] {
        b.write(at, |a, row| mem.put(a, row));
    }
    let required = Bsp::essential_bytes(&Bsp::read_header(&mem, AT).unwrap()).unwrap();
    let limit = crate::world::INDEX_RECORD_BYTES + crate::world::BSP_OWNER_BYTES + required;
    let world = CollisionWorld::with_budget(&EPOCH, limit);
    let outer = world.bsp(&mem, AT, || {
        // While the outer decode is unlocked, the other BSP takes the budget.
        world.bsp(&mem, second, || Ok(CONSTS)).unwrap();
        Ok(CONSTS)
    });
    assert_eq!(outer.unwrap_err(), Bail::Budget);
    let stats = world.stats();
    assert_eq!(stats.bsps, 1);
    assert!(stats.bytes <= limit);
    assert!(
        world
            .bsp(&mem, second, || panic!("the winning decode remains cached"))
            .is_ok()
    );
}

#[test]
fn straight_walk_requires_a_single_leaf_and_no_surface_lookup() {
    use crate::bsp::segment::{KIND_SOLID, PlaneDistances, StraightWalk};

    let img = image(&wall(0));
    let bsp = decode(&img);
    let q = query([-2.0, 0.0, 0.0], [0.5, 0.0, 0.0], 0);
    let result = run(&img, &bsp, &q).unwrap();
    assert_eq!(
        result.straight,
        Some(StraightWalk {
            last: Some(PlaneDistances {
                distance: (-2.0f32).to_bits(),
                delta: 0.5f32.to_bits(),
                start: (-2.0f32).to_bits(),
                end: (-1.5f32).to_bits(),
            })
        })
    );
    let direct = bsp.vector_test(&img, &q, leaf(0), 0, 1.0f32.to_bits()).unwrap();
    assert_eq!(direct.straight, Some(StraightWalk { last: None }));
    // A crossing that ultimately misses still used the recursive stack.
    let cross = query([-2.0, 0.0, 0.0], [3.0, 0.0, 0.0], 0);
    assert!(run(&img, &bsp, &cross).unwrap().straight.is_none());
    // Visiting just one leaf can also run the matcher and then miss.
    let from_solid = Segment { flags: 2, prev_kind: KIND_SOLID, plane: NONE, ..q };
    let lookup = run(&img, &bsp, &from_solid).unwrap();
    assert!(lookup.hit.is_none());
    assert!(lookup.straight.is_none());
}

#[test]
fn steps_follow_the_guest_frames_in_order() {
    use crate::bsp::segment::{PlaneDistances, Step};
    // x = 0, then x = 1 in front of it: three empty leaves.
    let mut b = Builder::default();
    b.planes.extend([[1.0, 0.0, 0.0, 0.0], [1.0, 0.0, 0.0, 1.0]]);
    b.nodes.extend([(0, leaf(0), node(1)), (1, leaf(1), leaf(2))]);
    b.leaves.extend([(0, 0, 0), (0, 0, 0), (0, 0, 0)]);
    let img = image(&b);
    let bsp = decode(&img);
    let f = |x: f32| x.to_bits();
    let out = run(&img, &bsp, &query([-0.5, 0.0, 0.0], [2.0, 0.0, 0.0], 0)).unwrap();
    let d = |distance: f32, start: f32, end: f32| PlaneDistances {
        distance: f(distance),
        delta: f(2.0),
        start: f(start),
        end: f(end),
    };
    assert_eq!(
        out.steps,
        [
            Step::Plane { node: 0, plane: 0, d: d(-0.5, -0.5, 1.5), front: [false, true], next: NONE },
            Step::Cross { split: f(0.25), increasing: true, near: leaf(0) | 0xff00_0000 },
            Step::Leaf { child: leaf(0) | 0xff00_0000, kind: 1, lookup: false },
            Step::Far { far: 1 },
            Step::Plane { node: 1, plane: 1, d: d(-1.5, -1.0, 0.5), front: [false, true], next: NONE },
            Step::Cross { split: f(0.75), increasing: true, near: leaf(1) | 0xff00_0000 },
            Step::Leaf { child: leaf(1) | 0xff00_0000, kind: 1, lookup: false },
            Step::Far { far: leaf(2) | 0xff00_0000 },
            Step::Leaf { child: leaf(2) | 0xff00_0000, kind: 1, lookup: false },
        ]
    );
    assert_eq!(out.leaves, [0, 1, 2]);
    // A split not below R's fraction stops the frame before the far side.
    let mut q = query([-0.5, 0.0, 0.0], [2.0, 0.0, 0.0], 0);
    q.fraction = f(0.25);
    let out = run(&img, &bsp, &q).unwrap();
    assert_eq!(out.steps.len(), 4);
    assert_eq!(out.steps[3], Step::Stop);
}

/// Under the guest's state the walks run operands the fast path declines:
/// a plane component far below its range and a NaN coordinate.
#[test]
fn the_walks_run_operands_off_the_fast_path_under_the_guest_state() {
    use sim_core::det::Fp;
    let img = image(&wall(0));
    let bsp = decode(&img);
    // `x = 0` with a y component of 1e-30: on `y = 0` each product by it is
    // zero, so every outcome is the plain wall's.
    let mut b = wall(0);
    b.planes[0] = [1.0, 1.0e-30, 0.0, 0.0];
    let tiny_img = image(&b);
    let tiny = decode(&tiny_img);
    let p = bits([-0.5, 0.0, 0.0]);
    assert!(matches!(tiny.point_result(0, p), Err(Bail::Float)), "the fast path declines");
    let mut fp = Fp::guest(0x1f80);
    assert_eq!(tiny.point_result_with(&mut fp, 0, p).unwrap(), bsp.point_result(0, p).unwrap());
    assert_eq!(fp.mxcsr(), Some(0x1f80));

    let q = query([-1.0, 0.0, 0.0], [3.0, 0.0, 0.0], 1);
    assert!(matches!(run(&tiny_img, &tiny, &q), Err(Bail::Float)), "the fast path declines");
    let t1 = 1.0f32.to_bits();
    let want = bsp.vector_test_under(0x1f80, 0x027f, &img, &q, 0, 0, t1).unwrap();
    let got = tiny.vector_test_under(0x1f80, 0x027f, &tiny_img, &q, 0, 0, t1).unwrap();
    assert!(want.hit.is_some());
    assert_eq!(got.hit, want.hit);
    assert_eq!(got.steps, want.steps);
    assert_eq!(got.mxcsr, want.mxcsr);
    assert_eq!(got.x87_flags, want.x87_flags);

    // A NaN coordinate: the distance is unordered, `comiss dist, 0; jb`
    // is taken (the back child, leaf 0) and the compare raises IE.
    let mut fp = Fp::guest(0x1f80);
    let got = bsp.point_result_with(&mut fp, 0, bits([f32::NAN, 0.0, 0.0])).unwrap();
    assert_eq!(got.leaf, 0);
    assert_ne!(fp.mxcsr().unwrap() & 0x01, 0);
    assert!(fp.exact());
}

/// Under the live MXCSR (PE clear) the point walk is the fast path's and
/// gathers PE: an exact walk leaves it clear, an inexact one sets it.
#[test]
fn the_point_walk_raises_pe_under_the_guest_mxcsr() {
    use sim_core::det::Fp;
    let img = image(&wall(0));
    let bsp = decode(&img);
    let walk = |p: [f32; 3]| {
        let mut fp = Fp::tracked(0x1f80);
        let got = bsp.point_result_with(&mut fp, 0, bits(p)).unwrap();
        assert_eq!(got, bsp.point_result(0, bits(p)).unwrap());
        fp.mxcsr()
    };
    assert_eq!(walk([-1.0, 0.0, 0.0]), Some(0x1f80), "1 * -1 is exact");
    assert_eq!(walk([-0.1, 0.0, 0.0]), Some(0x1f80), "a product by 1 is exact");
    // An inexact plane: (0.6, 0.8, 0) . p - 0.3.
    let mut b = wall(0);
    b.planes[0] = [0.6, 0.8, 0.0, 0.3];
    let img = image(&b);
    let bsp = decode(&img);
    let mut fp = Fp::tracked(0x1f80);
    let got = bsp.point_result_with(&mut fp, 0, bits([0.7, 0.1, 0.0])).unwrap();
    assert_eq!(got, bsp.point_result(0, bits([0.7, 0.1, 0.0])).unwrap());
    assert_eq!(fp.mxcsr(), Some(0x1fa0));
}

/// The segment walk under the live MXCSR: the same outcome as the fast
/// path's, with the MXCSR it leaves.
#[test]
fn the_segment_walk_runs_under_the_guest_mxcsr() {
    let img = image(&wall(0));
    let bsp = decode(&img);
    let q = query([-1.0, 0.0, 0.0], [3.0, 0.0, 0.0], 1);
    let fast = run(&img, &bsp, &q).unwrap();
    let live = bsp.vector_test_under(0x1f80, 0x027f, &img, &q, 0, 0, 1.0f32.to_bits()).unwrap();
    assert_eq!(live.hit, fast.hit);
    assert_eq!(live.steps, fast.steps);
    let m = live.mxcsr.expect("the guest's MXCSR");
    assert_eq!(m & !0x20, 0x1f80, "only PE can be raised");
    assert_eq!(fast.mxcsr, None);
}
