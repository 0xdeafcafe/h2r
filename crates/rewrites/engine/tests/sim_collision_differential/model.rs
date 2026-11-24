//! The complete object query, including its real model iterator, inverse
//! node transforms, recursive BSP walk, child recursion and plane scatter.
use super::*;
use sim_collision::{
    Bail, Fault, GuestRead,
    model::{ObjectQuery, trace_objects},
};

const ENTRY: u32 = 0x0016_9510;
const TABLE: u32 = 0x006a_0000;
const HEADERS: u32 = TABLE + 0x100;
const TAGS: u32 = TABLE + 0x200;
const OBJECT_TAG: u32 = TABLE + 0x300;
const HLMT: u32 = TABLE + 0x400;
const COLL: u32 = TABLE + 0x600;
const BODIES: u32 = TABLE + 0x1000;
const PERM: u32 = TABLE + 0x5000;
const PART: u32 = TABLE + 0x6000;
const ROOT: u32 = 0x1717_0001;
const CHILD: u32 = 0x2929_0002;
const SIBLING: u32 = 0x3434_0003;

struct Inputs<'a, 'b>(&'a Ram<'b>);

impl GuestRead for Inputs<'_, '_> {
    fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), Fault> {
        self.0.read(addr, dst).map_err(|_| Fault { addr })
    }
}

fn fixture(m: &mut Ram<'_>) {
    let regions = PERM;
    let states = PERM + 0x100;
    let coll_regions = PERM + 0x200;
    let coll_perms = PERM + 0x300;
    for (at, value) in [
        (0x004e_0300, TABLE),
        (TABLE + 0x44, HEADERS),
        (0x004e_3b44, TAGS),
        (TAGS + 0x18, OBJECT_TAG),
        (TAGS + 0x28, HLMT),
        (TAGS + 0x38, COLL),
        (OBJECT_TAG + 0x38, 2),
        (HLMT + 12, 3),
        (HLMT + 0x70, 1),
        (HLMT + 0x74, regions),
        (regions + 8, 2),
        (regions + 12, states),
        (COLL + 0x1c, 2),
        (COLL + 0x20, coll_regions),
        (coll_regions + 12 + 4, 2),
        (coll_regions + 12 + 8, coll_perms),
        (coll_perms + 20 + 4, 2),
        (coll_perms + 20 + 8, PART),
        (HLMT + 0x5c, PERM + 0x400),
    ] {
        m.word(at, value);
    }
    m.store(regions + 4, &[1]);
    m.store(states + 5, &[u8::MAX]);
    m.store(states + 8 + 5, &[1]);
    m.store(PERM + 0x400 + 3 * 20 + 16, &179u16.to_le_bytes());
    let mut wall = Builder::default();
    wall.planes.push([1.0, 0.0, 0.0, 0.0]);
    wall.nodes.push((0, synth::leaf(0), synth::SOLID));
    wall.leaves.push((0, 1, 0));
    wall.refs.push((0, synth::surface(0)));
    let ring = wall.ring(
        0,
        &[[0., -4., -4.], [0., 4., -4.], [0., 4., 4.], [0., -4., 4.]],
        false,
    );
    wall.surfaces.push((0, ring, 0, 0, 3));
    wall.write(BSP, |a, b| m.store(a, b));
    let mut header = [0; 64];
    m.read(BSP, &mut header).unwrap();
    for i in 0..2 {
        m.store(PART + 0x44 * i, &[i as u8]);
        m.store(PART + 0x44 * i + 4, &header);
    }
    for (i, id) in [ROOT, CHILD, SIBLING].into_iter().enumerate() {
        let body = BODIES + i as u32 * 0x1000;
        let h = HEADERS + (id & 0xffff) * 12;
        m.word(h + 8, body);
        m.store(h + 2, &[0, 0]);
        m.word(body, 1);
        m.word(body + 4, 0);
        m.word(body + 12, if i == 0 { SIBLING } else { u32::MAX });
        m.word(body + 16, if i == 0 { CHILD } else { u32::MAX });
        m.store(body + 0x116, &0x200u16.to_le_bytes());
        m.store(body + 0x11a, &0x380u16.to_le_bytes());
        m.store(body + 0x380, &[1]);
        put_vector(m, body + 0x40, [0.; 3]);
        m.float(body + 0x4c, 40.);
        for node in 0..2 {
            let matrix = body + 0x200 + node * 0x34;
            m.float(matrix, 2.);
            // Local +X is world +Y. Object and node both shift the plane.
            put_vector(m, matrix + 4, [0., 1., 0.]);
            put_vector(m, matrix + 16, [-1., 0., 0.]);
            put_vector(m, matrix + 28, [0., 0., 1.]);
            put_vector(
                m,
                matrix + 40,
                [0.5, 4. - 2. * i as f32 - node as f32, -0.25],
            );
        }
    }
    for (r, row) in synth::CONSTS.projection.iter().enumerate() {
        for (c, x) in row.iter().enumerate() {
            m.store(
                layout::PROJECTION + 2 * (r * 3 + c) as u32,
                &x.to_le_bytes(),
            );
        }
    }
    m.float(0x0044_aed4, 1.);
    m.float(layout::ONE, 1.);
    m.word(layout::ZERO, 0);
}

#[test]
fn object_tree_model_parts_and_result_match_the_guest() {
    with_oracle("object model collision", |oracle, image| {
        const START_V: [f32; 3] = [0., -4., 0.];
        const DELTA_V: [f32; 3] = [0., 12., 0.];
        for scenario in 0..9 {
            let mut m = fresh(image);
            fixture(&mut m);
            let q = ObjectQuery {
                object: ROOT,
                pretested: scenario != 5,
                flags: 16
                    | match scenario {
                        1 => 1 << 28,
                        2 => 1 << 17,
                        _ => 0,
                    },
                bsp_flags: 1,
                start: START_V.map(f32::to_bits),
                delta: DELTA_V.map(f32::to_bits),
                ignore: [
                    if scenario == 5 { ROOT } else { u32::MAX },
                    if scenario == 3 { CHILD } else { u32::MAX },
                ],
                fraction: if scenario == 6 { 0.25f32 } else { 1.0 }.to_bits(),
            };
            if scenario == 4 {
                // Damage removes both direct-list targets. The child is
                // still tested; its own model resolves a live permutation.
                m.store(BODIES + 0x380, &[0]);
                m.store(BODIES + 0x2000 + 0x380, &[0]);
            }
            if scenario == 7 {
                for i in 0..3 {
                    m.store(BODIES + i * 0x1000 + 0x380, &[0xff]);
                }
            }
            if scenario == 8 {
                // The second child part is coplanar with the first.
                m.float(BODIES + 0x1000 + 0x200 + 0x34 + 44, 2.);
            }
            put_vector(&mut m, START, START_V);
            put_vector(&mut m, DELTA, DELTA_V);
            m.store(RECORD, &[0xb7; 0x60]);
            m.word(RECORD + 4, q.fraction);
            for (i, arg) in [
                ROOT,
                u32::from(q.pretested),
                q.flags,
                q.bsp_flags,
                START,
                DELTA,
                q.ignore[0],
                q.ignore[1],
                RECORD,
            ]
            .into_iter()
            .enumerate()
            {
                m.word(STACK + 4 * (i as u32 + 1), arg);
            }
            let mut guest = m.clone();
            let mut cpu = ir_cpu(&CpuAbi {
                esp: STACK,
                mxcsr: 0x1fa0,
                ..Default::default()
            });
            oracle.run(&mut cpu, &mut guest, ENTRY);
            let mut native = m.clone();
            let mut read = Inputs(&m);
            static EPOCH: Epoch = Epoch::new();
            let world = CollisionWorld::new(&EPOCH);
            let mut visited = 0;
            let hit = trace_objects(&mut read, &q, |g, part| {
                visited += 1;
                world.bsp(g, part.bsp, || Ok(synth::CONSTS))?.vector_test(
                    g,
                    &part.query,
                    0,
                    0,
                    part.t1,
                )
            })
            .unwrap();
            assert_eq!(
                cpu.gpr[0] as u8,
                u8::from(hit.is_some()),
                "AL, scenario {scenario}"
            );
            assert_eq!(
                hit.is_some(),
                scenario != 6 && scenario != 7,
                "synthetic oracle coverage {scenario}"
            );
            if let Some(hit) = hit {
                hit.write_result(|offset, bytes| {
                    native.store(RECORD + offset, bytes);
                    Ok::<_, Bail>(())
                })
                .unwrap();
            }
            if scenario == 0 {
                assert_eq!(visited, 6);
            }
            let mut expected = [0; 0x60];
            let mut actual = expected;
            guest.read(RECORD, &mut expected).unwrap();
            native.read(RECORD, &mut actual).unwrap();
            assert_eq!(expected, actual, "record, scenario {scenario}");
        }
    });
}
