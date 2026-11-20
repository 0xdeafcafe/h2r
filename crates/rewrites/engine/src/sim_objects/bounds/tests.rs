//! `bdef0` on a synthetic object: the spheres it stores equal the pure
//! kernel's (`sim_objects::bounds`), and the partition paths return and
//! call as retail. Synthetic values only; the image constants the body
//! reads are written here with values of the test's choosing.

use super::*;
use ::sim_objects::bounds::{NodeMatrix, relink, spheres};
use ::sim_objects::defs::Def;
use guest_layout::Datum;
use runtime::CpuAbi;
use runtime::guest::Permissions;

const MAP: u32 = 0x0045_0000;
const ARRAY: u32 = 0x0050_0000;
const HEADERS: u32 = 0x0050_1000;
const TAGS: u32 = 0x0050_2000;
const DEFINITION: u32 = 0x0050_3000;
const BODY: u32 = 0x0050_4000;
const MATRIX: u32 = 0x1c0;
const STACK: u32 = 0x0051_8000;
const CALLER: u32 = 0x00de_ad00;
const OBJECT: u32 = 0x8100_0001;
const THRESHOLD: f32 = 0.0625;

fn f(g: &mut Guest, a: u32, v: f32) {
    g.set_u32(a, v.to_bits()).unwrap();
}

fn def(flags: u16, dynamic_scale: f32) -> Def {
    Def {
        tag: Datum(0),
        flags,
        bounding_radius: 0.75,
        bounding_offset: [0.5, -0.25, 1.0],
        dynamic_scale,
        dynamic_offset: [0.125, 0.25, -0.5],
    }
}

/// One object (slot 1) with `d`, the root matrix `m`, scale `scale`, and
/// body `+04` bit 8 when `in_partition`.
fn world(d: &Def, m: &NodeMatrix, scale: f32, in_partition: bool) -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(MAP, 0xd_0000, Permissions::RW).unwrap();
    f(&mut g, xform::ONE, 1.0);
    f(&mut g, ZERO, 0.0);
    f(&mut g, MOVED, THRESHOLD);
    g.set_u32(points::OBJECTS.base, ARRAY).unwrap();
    g.set_u32(ARRAY + DATA, HEADERS).unwrap();
    g.set_u32(HEADERS + 12 + 8, BODY).unwrap();
    g.set_u32(points::TAG_ENTRIES.base, TAGS).unwrap();
    g.set_u32(TAGS + 8, DEFINITION).unwrap();
    g.set_u16(DEFINITION + 2, d.flags).unwrap();
    f(&mut g, DEFINITION + 4, d.bounding_radius);
    for i in 0..3 {
        f(&mut g, DEFINITION + 8 + 4 * i, d.bounding_offset[i as usize]);
        f(&mut g, DEFINITION + 0x24 + 4 * i, d.dynamic_offset[i as usize]);
    }
    f(&mut g, DEFINITION + 0x20, d.dynamic_scale);
    g.set_u32(BODY + 4, u32::from(in_partition) << 8).unwrap();
    g.set_u32(BODY + 0xb4, u32::MAX).unwrap();
    f(&mut g, BODY + 0xa0, scale);
    g.set_u16(BODY + 0x116, MATRIX as u16).unwrap();
    let words = [
        [m.scale],
        [m.forward[0]],
        [m.forward[1]],
        [m.forward[2]],
        [m.left[0]],
        [m.left[1]],
        [m.left[2]],
        [m.up[0]],
        [m.up[1]],
        [m.up[2]],
        [m.translation[0]],
        [m.translation[1]],
        [m.translation[2]],
    ];
    for (i, [w]) in words.into_iter().enumerate() {
        g.set_u32(BODY + MATRIX + 4 * i as u32, w).unwrap();
    }
    g.set_u32(STACK, CALLER).unwrap();
    g.set_u32(STACK + 4, OBJECT).unwrap();
    let cpu = CpuAbi { esp: STACK, mxcsr: 0x1fa0, ebx: 0x0b, ebp: 0x0b9, esi: 0x05, edi: 0x0d, ..Default::default() };
    (cpu, g)
}

fn rotated() -> NodeMatrix {
    let b = |v: [f32; 3]| v.map(f32::to_bits);
    NodeMatrix {
        scale: 2.0f32.to_bits(),
        forward: b([0.0, 1.0, 0.0]),
        left: b([-1.0, 0.0, 0.0]),
        up: b([0.0, 0.0, 1.0]),
        translation: b([4.0, 5.0, 6.0]),
    }
}

fn vec3(g: &Guest, a: u32) -> [u32; 3] {
    [0, 4, 8].map(|o| g.u32(a + o).unwrap())
}

fn run(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Next {
    bounds(&mut Cx { cpu: c, g }, pc).unwrap()
}

#[test]
fn the_body_stores_the_kernels_spheres() {
    for (flags, dynamic) in [(0, 0.0), (0x1000, 3.0), (0, 3.0), (0x1000, -1.0)] {
        let d = def(flags, dynamic);
        let m = rotated();
        let (mut c, mut g) = world(&d, &m, 0.5, false);
        let expected = spheres(&d, &m, 0.5f32.to_bits()).expect("on the fast path");
        assert_eq!(run(&mut c, &mut g, points::OBJECT_BOUNDS.base), Next::Ret { pop: 4 });
        assert_eq!(c.eax & 0xff, 1);
        assert_eq!(c.esp, STACK, "the frame is popped down to the return address");
        assert_eq!((c.ebx, c.ebp, c.esi, c.edi), (0x0b, 0x0b9, 0x05, 0x0d));
        assert_eq!(vec3(&g, BODY + 0x30), expected.centre);
        assert_eq!(g.u32(BODY + 0x3c).unwrap(), expected.radius);
        assert_eq!(vec3(&g, BODY + 0x50), expected.second_centre);
        assert_eq!(g.u32(BODY + 0x5c).unwrap(), expected.second_radius);
        let (centre, radius) = expected.partition();
        assert_eq!((vec3(&g, BODY + 0x40), g.u32(BODY + 0x4c).unwrap()), (centre, radius));
    }
}

#[test]
fn in_the_partition_an_unmoved_sphere_returns_0_and_a_moved_one_relinks() {
    let d = def(0, 0.0);
    let m = rotated();
    let expected = spheres(&d, &m, 1.0f32.to_bits()).unwrap();
    let (centre, radius) = expected.partition();

    // Registered where the bounds land: nothing to do.
    let (mut c, mut g) = world(&d, &m, 1.0, true);
    for (i, w) in centre.into_iter().enumerate() {
        g.set_u32(BODY + 0x40 + 4 * i as u32, w).unwrap();
    }
    g.set_u32(BODY + 0x4c, radius).unwrap();
    assert_eq!(relink((centre, radius), (centre, radius), THRESHOLD.to_bits()), Some(false));
    assert_eq!(run(&mut c, &mut g, points::OBJECT_BOUNDS.base), Next::Ret { pop: 4 });
    assert_eq!(c.eax & 0xff, 0);
    assert_eq!(c.esp, STACK);

    // Registered elsewhere: disconnect, then connect, then 1.
    let (mut c, mut g) = world(&d, &m, 1.0, true);
    g.set_u32(BODY + 0x4c, 9.0f32.to_bits()).unwrap();
    let next = run(&mut c, &mut g, points::OBJECT_BOUNDS.base);
    assert_eq!(next, Next::Call { target: DISCONNECT, site: BOUNDS_SITES[1] });
    assert_eq!(g.u32(c.esp).unwrap(), OBJECT, "b87b0's argument");
    // The callee returns (`ret 4`): `drive` would pop the site and the argument.
    c.esp += 4;
    let next = run(&mut c, &mut g, BOUNDS_SITES[1]);
    assert_eq!(next, Next::Call { target: CONNECT, site: BOUNDS_SITES[2] });
    assert_eq!((g.u32(c.esp).unwrap(), g.u32(c.esp + 4).unwrap()), (OBJECT, 0));
    assert_eq!((vec3(&g, BODY + 0x40), g.u32(BODY + 0x4c).unwrap()), (centre, radius));
    c.esp += 8;
    assert_eq!(run(&mut c, &mut g, BOUNDS_SITES[2]), Next::Ret { pop: 4 });
    assert_eq!(c.eax & 0xff, 1);
    assert_eq!(c.esp, STACK);
}
