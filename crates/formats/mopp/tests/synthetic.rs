//! Opcode semantics on hand-assembled programs, and the compiler on
//! invented geometry. No game data.
mod common;
use common::metrics::{Rng, measure, sample_boxes};
use mopp::check::validate;
use mopp::geom::Surface;
use mopp::vm::Fault;
use mopp::{Aabb, Code, query_aabb, query_sphere};

/// One world unit is one root-frame unit (scale 2^16, origin 0).
fn prog(bytecode: &[u8]) -> Code {
    Code {
        offset: [0.0; 3],
        scale: 65536.0,
        info: 0xff,
        bytecode: bytecode.to_vec(),
    }
}
fn cube(lo: f32, hi: f32) -> Aabb {
    Aabb {
        min: [lo; 3],
        max: [hi; 3],
    }
}
fn bx(min: [f32; 3], max: [f32; 3]) -> Aabb {
    Aabb { min, max }
}
fn q(code: &Code, b: Aabb) -> Vec<u32> {
    let mut k = query_aabb(code, &b).unwrap();
    k.sort_unstable();
    k
}

#[test]
fn terminals_and_key_offsets() {
    let all = cube(0.5, 200.5);
    assert_eq!(q(&prog(&[0x00]), all), Vec::<u32>::new());
    assert_eq!(q(&prog(&[0x35]), all), vec![5]);
    assert_eq!(q(&prog(&[0x50, 0xab]), all), vec![0xab]);
    assert_eq!(q(&prog(&[0x51, 0x12, 0x34]), all), vec![0x1234]);
    assert_eq!(q(&prog(&[0x52, 0x12, 0x34, 0x56]), all), vec![0x12_3456]);
    assert_eq!(q(&prog(&[0x53, 0x20, 0, 0, 7]), all), vec![0x2000_0007]);
    // 0x0b sets, 0x09 and 0x0a add; all big-endian.
    assert_eq!(
        q(
            &prog(&[0x0b, 0x20, 0, 0x01, 0, 0x09, 3, 0x0a, 0x01, 0x00, 0x31]),
            all
        ),
        vec![0x2000_0204]
    );
    // Properties 0x60/0x64/0x68 are skipped over.
    assert_eq!(
        q(&prog(&[0x60, 1, 0x64, 0, 1, 0x68, 0, 0, 0, 1, 0x32]), all),
        vec![2]
    );
}

#[test]
fn jumps() {
    let all = cube(0.5, 200.5);
    assert_eq!(q(&prog(&[0x05, 1, 0x31, 0x32]), all), vec![2]);
    assert_eq!(q(&prog(&[0x06, 0, 1, 0x31, 0x32]), all), vec![2]);
    assert_eq!(q(&prog(&[0x07, 0, 0, 1, 0x31, 0x32]), all), vec![2]);
}

#[test]
fn bound_checks() {
    // x in 10..=20 (root frame).
    let p = prog(&[0x26, 10, 20, 0x31]);
    assert_eq!(q(&p, cube(15.2, 15.8)), vec![1]);
    assert_eq!(q(&p, cube(20.2, 30.0)), vec![1]);
    assert_eq!(q(&p, cube(21.5, 30.0)), Vec::<u32>::new());
    assert_eq!(q(&p, cube(0.5, 9.5)), Vec::<u32>::new());
    // z 24-bit bound in full units (world * 65536).
    let p = prog(&[0x2b, 0x0a, 0, 0, 0x14, 0, 0, 0x31]);
    assert_eq!(q(&p, bx([0.0; 3], [1.0, 1.0, 15.0])), vec![1]);
    assert_eq!(q(&p, bx([0.0; 3], [1.0, 1.0, 9.9])), Vec::<u32>::new());
    assert_eq!(
        q(&p, bx([0.0, 0.0, 20.1], [1.0, 1.0, 30.0])),
        Vec::<u32>::new()
    );
}

#[test]
fn two_plane_split() {
    // y: low child (max <= 100) emits 1, high child (min >= 120) emits 2.
    let p = prog(&[0x11, 100, 120, 1, 0x31, 0x32]);
    assert_eq!(q(&p, bx([0.0, 50.2, 0.0], [1.0, 60.8, 1.0])), vec![1]);
    assert_eq!(q(&p, bx([0.0, 130.2, 0.0], [1.0, 140.8, 1.0])), vec![2]);
    assert_eq!(q(&p, bx([0.0, 90.2, 0.0], [1.0, 125.8, 1.0])), vec![1, 2]);
    // In the gap: the engine tests `hi < high_min` first and enters low.
    assert_eq!(q(&p, bx([0.0, 105.2, 0.0], [1.0, 110.8, 1.0])), vec![1]);
    // 16-bit form: low at J1, high at J2.
    let p = prog(&[0x24, 100, 120, 0, 1, 0, 0, 0x32, 0x31]);
    assert_eq!(q(&p, bx([0.0, 50.2, 0.0], [1.0, 60.8, 1.0])), vec![1]);
    assert_eq!(q(&p, bx([0.0, 130.2, 0.0], [1.0, 140.8, 1.0])), vec![2]);
    assert_eq!(q(&p, bx([0.0, 90.2, 0.0], [1.0, 125.8, 1.0])), vec![1, 2]);
}

#[test]
fn one_plane_split() {
    // x at 50: low may reach 51, high starts at 50.
    let p = prog(&[0x20, 50, 1, 0x31, 0x32]);
    assert_eq!(q(&p, cube(10.2, 48.8)), vec![1]);
    assert_eq!(q(&p, cube(52.2, 60.8)), vec![2]);
    assert_eq!(q(&p, cube(51.2, 51.8)), vec![1, 2]);
    assert_eq!(q(&p, cube(50.2, 50.8)), vec![1, 2]);
}

#[test]
fn rescale() {
    // Zoom by 2^4 into the cell at 10 on every axis, then bound x to the
    // upper half of that cell (16 sub-units, 8..=15).
    let p = prog(&[0x04, 10, 10, 10, 0x26, 8, 15, 0x31]);
    assert_eq!(q(&p, cube(10.6, 10.7)), vec![1]);
    assert_eq!(q(&p, cube(10.1, 10.4)), Vec::<u32>::new());
    assert_eq!(validate(&p.bytecode).unwrap().keys, vec![1]);
}

#[test]
fn diagonal_split() {
    // x + y, operands doubled: low has x + y <= 100, high x + y >= 120.
    let p = prog(&[0x17, 50, 60, 1, 0x31, 0x32]);
    assert_eq!(q(&p, bx([10.2, 10.2, 0.0], [20.8, 20.8, 1.0])), vec![1]);
    assert_eq!(q(&p, bx([70.2, 70.2, 0.0], [80.8, 80.8, 1.0])), vec![2]);
    // x - y + 255: low <= 200, high >= 300 (operands x2: 100, 150).
    let p = prog(&[0x18, 100, 150, 1, 0x31, 0x32]);
    assert_eq!(q(&p, bx([10.2, 100.2, 0.0], [20.8, 110.8, 1.0])), vec![1]);
    assert_eq!(q(&p, bx([150.2, 10.2, 0.0], [160.8, 20.8, 1.0])), vec![2]);
}

#[test]
fn sphere_follows_the_engine() {
    // Centre is round(p * scale) - 1 and the frame radius (r >> s) + 1.
    let p = prog(&[0x26, 10, 20, 0x31]);
    assert_eq!(query_sphere(&p, [15.0, 0.0, 0.0], 0.1).unwrap(), vec![1]);
    // 8.5 with radius 0.4: frame centre 8, radius 1, so 7..=9 misses 10.
    assert_eq!(
        query_sphere(&p, [8.5, 0.0, 0.0], 0.4).unwrap(),
        Vec::<u32>::new()
    );
    // 9.5: frame centre 9, radius 1, reaches 10.
    assert_eq!(query_sphere(&p, [9.5, 0.0, 0.0], 0.4).unwrap(), vec![1]);
}

#[test]
fn faults() {
    let all = cube(0.5, 200.5);
    for op in [0x08u8, 0x0c, 0x1d, 0x2c, 0x54, 0x6c, 0xff] {
        assert_eq!(
            query_aabb(&prog(&[op]), &all),
            Err(Fault::BadOpcode { at: 0, op })
        );
        assert_eq!(validate(&[op]).unwrap_err(), Fault::BadOpcode { at: 0, op });
    }
    for op in [0x61u8, 0x65, 0x6b] {
        assert!(matches!(
            validate(&[op, 0, 0, 0, 0]),
            Err(Fault::Property { .. })
        ));
    }
    assert_eq!(
        validate(&[0x26, 1]).unwrap_err(),
        Fault::OutOfCode { at: 0 }
    );
    assert_eq!(
        validate(&[0x05, 9, 0x30]).unwrap_err(),
        Fault::OutOfCode { at: 11 }
    );
    // Five zooms of 4 would go below shift 0.
    let deep = [[0x04u8, 0, 0, 0]; 5].concat();
    assert!(matches!(
        validate(&[deep, vec![0x30]].concat()),
        Err(Fault::Shift { .. })
    ));
}

#[test]
fn block_round_trip() {
    let c = Code {
        offset: [-1.5, 2.0, 3.25],
        scale: 1234.5,
        info: 0xff,
        bytecode: vec![0x26, 0, 255, 0x30],
    };
    let b = c.to_block();
    assert_eq!(b.len(), 0x40);
    assert_eq!(u32::from_le_bytes(b[0x20..0x24].try_into().unwrap()), 0x34);
    assert_eq!(u32::from_le_bytes(b[0x24..0x28].try_into().unwrap()), 1);
    assert_eq!(Code::parse(&b).unwrap(), c);
    assert!(Code::parse(&b[..0x20]).is_err());
    let mut bad = b.clone();
    bad[0x20] = 0x80;
    assert!(Code::parse(&bad).is_err());
}

/// Random triangles of 5 cm to 20 m in a `world` m cube, keyed as structure
/// surfaces.
fn triangles(n: usize, seed: u64) -> Vec<Surface> {
    triangles_in(n, seed, 300.0)
}
fn triangles_in(n: usize, seed: u64, world: f32) -> Vec<Surface> {
    let mut rng = Rng(seed);
    (0..n)
        .map(|i| {
            let c = [0, 1, 2].map(|_| rng.unit() * world - 100.0);
            let size = 0.05 * 400f32.powf(rng.unit());
            let vertices = (0..3)
                .map(|_| [0, 1, 2].map(|a| c[a] + (rng.unit() - 0.5) * size))
                .collect();
            Surface {
                key: mopp::geom::structure_key(i),
                vertices,
            }
        })
        .collect()
}

#[test]
fn built_program_is_a_tight_superset() {
    let surfaces = triangles_in(3000, 7, 60.0);
    let code = mopp::build(&surfaces);
    let r = validate(&code.bytecode).unwrap();
    assert_eq!(r.keys, surfaces.iter().map(|s| s.key).collect::<Vec<_>>());
    assert_eq!(r.unreached, 0);
    for op in r
        .histogram
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(op, _)| op)
    {
        assert!(
            matches!(op, 0x01..=0x04 | 0x07 | 0x09..=0x0b | 0x10..=0x12 | 0x23..=0x28 | 0x30..=0x53),
            "unexpected opcode {op:#x}"
        );
    }
    let boxes = sample_boxes(&surfaces, 600, 3);
    let m = measure(&code, &surfaces, &boxes, 1e-4);
    assert_eq!(m.misses, 0);
    assert_eq!(m.sphere_misses, 0);
    assert_eq!(m.box_misses, 0, "{m:?}");
    assert!(m.truth > 1000, "{m:?}");
    // Returned per surface whose box overlaps: 1.0 would be a box test on
    // every surface.
    assert!(m.bounds_ratio() < 1.3, "{m:?}");
}

#[test]
fn large_programs_use_long_jumps() {
    let surfaces = triangles(12_000, 11);
    let code = mopp::build(&surfaces);
    assert!(code.bytecode.len() > 0x1_0000);
    let r = validate(&code.bytecode).unwrap();
    assert!(r.histogram[0x23..=0x25].iter().sum::<u32>() > 0);
    assert!(r.histogram[0x07] > 0);
    assert_eq!(r.keys.len(), surfaces.len());
    let m = measure(&code, &surfaces, &sample_boxes(&surfaces, 100, 5), 1e-4);
    assert_eq!(m.misses, 0);
    assert_eq!(m.box_misses, 0, "{m:?}");
}

#[test]
fn mixed_key_types_and_sparse_keys() {
    let mut surfaces = triangles(200, 13);
    for (i, s) in surfaces.iter_mut().enumerate() {
        s.key = match i % 3 {
            0 => mopp::geom::structure_key(i * 300),
            1 => (2 << 29) | (5 << 16) | i as u32,
            _ => (4 << 29) | i as u32,
        };
    }
    let code = mopp::build(&surfaces);
    let r = validate(&code.bytecode).unwrap();
    let mut want: Vec<u32> = surfaces.iter().map(|s| s.key).collect();
    want.sort_unstable();
    assert_eq!(r.keys, want);
    // Every key comes back for a box around its own surface.
    for s in &surfaces {
        assert!(query_aabb(&code, &s.bounds()).unwrap().contains(&s.key));
        assert!(
            mopp::query_box(&code, &s.bounds())
                .unwrap()
                .contains(&s.key)
        );
    }
}

#[test]
fn empty_and_degenerate_inputs() {
    let code = mopp::build(&[]);
    assert_eq!(code.bytecode, vec![0x00]);
    assert_eq!(
        query_aabb(&code, &cube(-1.0, 1.0)).unwrap(),
        Vec::<u32>::new()
    );
    // One point-sized surface, and many identical ones.
    let p = Surface {
        key: 0x2000_0000,
        vertices: vec![[1.0, 2.0, 3.0]],
    };
    let code = mopp::build(std::slice::from_ref(&p));
    assert_eq!(
        query_aabb(&code, &cube(0.0, 4.0)).unwrap(),
        vec![0x2000_0000]
    );
    let same: Vec<Surface> = (0..50)
        .map(|i| Surface {
            key: 0x2000_0000 + i,
            vertices: vec![[1.0, 2.0, 3.0], [1.5, 2.0, 3.0], [1.0, 2.5, 3.0]],
        })
        .collect();
    let code = mopp::build(&same);
    assert_eq!(query_aabb(&code, &cube(0.0, 4.0)).unwrap().len(), 50);
    assert_eq!(
        query_aabb(&code, &cube(5.0, 6.0)).unwrap(),
        Vec::<u32>::new()
    );
}

#[test]
fn key_bounds_hold_each_surface() {
    let surfaces = triangles(500, 17);
    let code = mopp::build(&surfaces);
    let bounds = mopp::bounds::key_bounds(&code).unwrap();
    assert_eq!(bounds.len(), surfaces.len());
    for s in &surfaces {
        let b = s.bounds();
        let k = bounds[&s.key];
        for a in 0..3 {
            assert!(
                k.min[a] <= b.min[a] && b.max[a] <= k.max[a],
                "{:#x} axis {a}",
                s.key
            );
            // Tight to within a frame unit or so of a 300 m world.
            assert!(b.min[a] - k.min[a] < 1.5 && k.max[a] - b.max[a] < 1.5);
        }
    }
}

/// The engine's box query (`0x002ff8b0`) keeps a surface for a box that
/// starts at the surface's highest point and reaches up: a body standing
/// on the top of a ramp. Its splits and bound checks treat the low child's
/// and the bound's top as exclusive, so a program that stores the top cell
/// inclusively cuts such a box off as soon as its low corner enters that
/// cell (Blood Gulch's blue ramp, converter version 71).
#[test]
fn box_query_keeps_a_surface_from_its_top() {
    for (world, seed) in [(60.0, 19), (300.0, 23), (2000.0, 29)] {
        let surfaces = triangles_in(2000, seed, world);
        let code = mopp::build(&surfaces);
        let mut missed = Vec::new();
        for s in &surfaces {
            let top = *s
                .vertices
                .iter()
                .max_by(|a, b| a[2].total_cmp(&b[2]))
                .unwrap();
            let b = Aabb {
                min: top.map(|v| v - 0.01),
                max: [top[0] + 0.2, top[1] + 0.2, top[2] + 0.7],
            };
            if !mopp::query_box(&code, &b).unwrap().contains(&s.key) {
                missed.push(s.key);
            }
        }
        assert!(
            missed.is_empty(),
            "world {world}: {} of {} surfaces missed from their tops: {:#x?}",
            missed.len(),
            surfaces.len(),
            &missed[..missed.len().min(8)]
        );
    }
}
