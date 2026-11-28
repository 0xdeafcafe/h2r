use super::*;
use crate::bsp::NONE;
use crate::synth::{Builder, CONSTS, Image, leaf, surface};
use crate::{Outcome, Segment};

const BSP_ADDR: u32 = 0x1000;
const POLYGON: [[f32; 3]; 9] = [
    [0.0, -4.0, -4.0],
    [0.0, 0.0, -4.0],
    [0.0, 4.0, -4.0],
    [0.0, 4.0, 0.0],
    [0.0, 4.0, 4.0],
    [0.0, 0.0, 4.0],
    [0.0, -4.0, 4.0],
    [0.0, -4.0, 0.0],
    [0.0, -4.0, -2.0],
];

fn fixture(vertices: &[[f32; 3]], reversed: bool, budget: u64) -> (Image, Bsp) {
    let mut builder = Builder::default();
    builder.planes.push([1.0, 0.0, 0.0, 0.0]);
    builder.nodes.push((0, leaf(0), leaf(1)));
    builder.leaves.extend([(1, 1, 0), (1, 1, 0)]);
    builder.refs.push((0x8000, surface(0)));
    let edge = builder.ring(0, vertices, reversed);
    builder.surfaces.push((0x8000, edge, 0, 0, 7));
    let mut image = Image::new(BSP_ADDR, builder.size() as usize);
    builder.write(BSP_ADDR, |addr, bytes| image.put(addr, bytes));
    let header = Bsp::read_header(&image, BSP_ADDR).unwrap();
    let bsp = Bsp::decode_with_ring_limit(&image, BSP_ADDR, header, &CONSTS, budget).unwrap();
    (image, bsp)
}

fn trace(image: &Image, bsp: &Bsp, y: f32, z: f32) -> Outcome {
    let q = Segment {
        flags: 0,
        breakable_count: 0,
        mask: 0,
        start: [-2.0, y, z].map(f32::to_bits),
        delta: [4.0f32, 0.0, 0.0].map(f32::to_bits),
        fraction: 1.0f32.to_bits(),
        listed: 0,
        prev_leaf: NONE,
        prev_kind: 0,
        plane: NONE,
    };
    bsp.vector_test(image, &q, 0, 0, q.fraction).unwrap()
}

#[test]
fn packed_and_scalar_walks_agree_from_three_to_nine_edges() {
    for len in 3..=9 {
        let points: Vec<_> = (0..len).map(|i| POLYGON[i * POLYGON.len() / len]).collect();
        for reversed in [false, true] {
            let (image, bsp) = fixture(&points, reversed, 4096);
            let (_, scalar) = fixture(&points, reversed, 0);
            let packed_ring = bsp.surfaces[0].packed_ring;
            assert_eq!(packed_ring != NONE, len <= MAX_PACKED_EDGES);
            for (y, z, hit) in [
                (0.0, -0.0, true),
                (-4.0, -4.0, true),
                (8.0, 8.0, false),
                (-8.0, 0.0, false),
            ] {
                let expected = trace(&image, &scalar, y, z);
                assert_eq!(expected.hit.is_some(), hit, "{len} edges at ({y}, {z})");
                assert_eq!(trace(&image, &bsp, y, z), expected);
            }
        }
    }
}

#[test]
fn shared_groups_use_the_axes_in_each_reference() {
    let (image, bsp) = fixture(&POLYGON[..8], false, 4096);
    let surface = &bsp.surfaces[0];
    let edges = bsp.read_surface_ring(&image, 0).unwrap();
    let cached = bsp.ring.read().unwrap();
    let cached = cached.as_ref().unwrap();
    let groups = cached.groups(surface);
    assert_eq!(groups.len(), 2);
    for row in 0..6 {
        let axes = bsp.row_axes(row).unwrap();
        for d in [-5.0f32, -4.0, -0.0, 0.0, 1.5, 4.0, 5.0] {
            let point = [d.to_bits(), (-d).to_bits()];
            let expected = contains_scalar(&edges, axes, point, &mut Fp::new()).unwrap();
            assert_eq!(
                contains_packed(groups, axes, point, Fp::new()).map(|(inside, _)| inside),
                Some(expected)
            );
        }
    }
}

#[test]
fn packed_ring_under_the_guest_mxcsr_raises_only_what_the_ordered_test_does() {
    let (image, bsp) = fixture(&POLYGON[..8], false, 4096);
    let surface = &bsp.surfaces[0];
    let edges = bsp.read_surface_ring(&image, 0).unwrap();
    for row in 0..6 {
        let axes = bsp.row_axes(row).unwrap();
        for d in [-5.0f32, -4.0, -0.0, 0.0, 0.1, 1.5, 4.0, 5.0] {
            let point = [d.to_bits(), (-d).to_bits()];
            for mxcsr in [0x1f80, 0x1fa0] {
                let mut want = Fp::guest(mxcsr);
                let inside = contains_scalar(&edges, axes, point, &mut want).unwrap();
                let mut got = Fp::guest(mxcsr);
                assert_eq!(
                    bsp.ring_contains(&image, 0, surface, axes, point, &mut got),
                    Ok(inside)
                );
                assert_eq!(got.mxcsr(), want.mxcsr(), "row {row} d {d} mxcsr {mxcsr:#x}");
            }
        }
    }
}

#[test]
fn packed_range_failure_replays_the_ordered_edge_test() {
    let large = 4_294_967_296.0f32;
    let points = [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, large, 0.0]];
    let (image, bsp) = fixture(&points, false, 4096);
    let surface = &bsp.surfaces[0];
    assert_ne!(surface.packed_ring, NONE);
    let axes = [2, 1];
    for (u, expected) in [(large, Ok(false)), (-large, Err(Bail::Float))] {
        let point = [u.to_bits(), 0];
        {
            let cached = bsp.ring.read().unwrap();
            let groups = cached.as_ref().unwrap().groups(surface);
            assert!(contains_packed(groups, axes, point, Fp::new()).is_none());
        }
        assert_eq!(
            bsp.ring_contains(&image, 0, surface, axes, point, &mut Fp::new()),
            expected
        );
    }
}

#[test]
fn packed_storage_uses_only_the_budget_left_by_raw_edges() {
    let points = &POLYGON[..4];
    let raw_bytes = (points.len() * size_of::<RingEdge>()) as u64;
    let group_bytes = size_of::<EdgeGroup>() as u64;
    let (image, scalar) = fixture(points, false, raw_bytes + group_bytes - 1);
    let (_, packed) = fixture(points, false, raw_bytes + group_bytes);
    assert!(!scalar.scalar_rings());
    assert_eq!(scalar.surfaces[0].packed_ring, NONE);
    assert_ne!(packed.surfaces[0].packed_ring, NONE);
    assert_eq!(
        packed.footprint() - scalar.footprint(),
        group_bytes as usize
    );
    assert_eq!(packed.discard_rings(), (raw_bytes + group_bytes) as usize);
    assert_eq!(
        packed.read_surface_ring(&image, 0),
        scalar.read_surface_ring(&image, 0)
    );
}
