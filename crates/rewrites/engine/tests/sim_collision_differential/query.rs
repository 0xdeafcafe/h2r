//! J from its six stack arguments through the BSP, liquid and live-object
//! loops to its return. The lifted arm executes every callee on synthetic
//! memory. Record padding starts nonzero, and marks can wrap or repeat.
use super::*;
#[path = "query/batch.rs"]
mod batch;

const CLUSTER_RECORDS: u32 = TOPOLOGY;
const BOUNDARIES: u32 = TOPOLOGY + 0x400;
const MATERIALS: u32 = TOPOLOGY + 0x800;
const HANDLE: u32 = 0x1234_0005;

fn fixture(image: &[u8], wall: bool, flags: u32, epoch: u32) -> Ram<'_> {
    let mut m = fresh(image);
    let mut b = Builder::default();
    b.planes.push([1.0, 0.0, 0.0, 0.0]);
    b.nodes.push((
        0,
        synth::leaf(0),
        if wall { synth::SOLID } else { synth::leaf(1) },
    ));
    b.leaves.extend([(0, 1, 0), (0, 0, 0)]);
    b.refs.push((0, synth::surface(0)));
    let edge = b.ring(
        0,
        &[
            [0.0, -2.0, -2.0],
            [0.0, 2.0, -2.0],
            [0.0, 2.0, 2.0],
            [0.0, -2.0, 2.0],
        ],
        false,
    );
    b.surfaces.push((0, edge, 0, 2, 0));
    write_bsp(&mut m, &b);
    for (at, val) in [
        (layout::STRUCTURE_BSP, BSP),
        (0x004e_0340, BSP),
        (layout::ACTIVE_STRUCTURE, STRUCTURE),
        (0x004e_d280, MASK),
        (STRUCTURE + 0x10, MATERIALS),
        (STRUCTURE + 0x30, CLUSTERS),
        (STRUCTURE + 0x68, BOUNDARIES),
        (STRUCTURE + 0x9c, 1),
        (STRUCTURE + 0xa0, CLUSTER_RECORDS),
        (CLUSTER_RECORDS + 0xa8, 0),
        (layout::FLOOD_EPOCH, epoch),
        (0x004e_7c1c, epoch),
        (layout::VISIT_GENERATION, epoch),
        (layout::CLUSTER_MARKS, epoch),
        (layout::OBJECT_MARKS + 4 * 5, epoch),
        (STACK + 4, flags),
        (STACK + 8, START),
        (STACK + 12, DELTA),
        (STACK + 16, HANDLE),
        (STACK + 20, u32::MAX),
        (STACK + 24, OUT),
    ] {
        m.word(at, val);
    }
    m.store(layout::BSP_INDEX, &0i16.to_le_bytes());
    m.store(CLUSTERS, &0u16.to_le_bytes());
    m.store(CLUSTERS + 8, &0u16.to_le_bytes());
    m.store(MATERIALS + 8, &37u16.to_le_bytes());
    m.store(CLUSTER_RECORDS + 0x70, &[0x80]);
    m.store(BOUNDARIES + 2, &55u16.to_le_bytes());
    put_vector(&mut m, BOUNDARIES + 4, [1.0, 0.0, 0.0]);
    m.float(BOUNDARIES + 16, -1.0);
    let (heads, array, refs) = REFERENCES[0];
    m.word(layout::COLLIDEABLE_HEADS, heads);
    m.word(layout::COLLIDEABLE_ARRAY, array);
    m.word(heads, 0);
    m.word(array + 0x24, 0x20);
    m.word(array + 0x44, refs);
    for i in 0..2 {
        let r = refs + 0x20 * i;
        m.word(r + 4, HANDLE);
        m.word(r + 8, if i == 0 { 1 } else { u32::MAX });
        m.word(r + 0xc, 0);
        put_vector(&mut m, r + 0x10, [0.0, 0.0, 0.0]);
        m.float(r + 0x1c, 3.0);
    }
    put_vector(&mut m, START, [-2.0, 0.0, 0.0]);
    put_vector(&mut m, DELTA, [4.0, 0.0, 0.0]);
    m.store(OUT, &[0xa5; 0x5c]);
    m
}

#[test]
fn structure_liquid_and_reference_order_match_the_lifted_query() {
    with_oracle("whole collision query", |oracle, image| {
        static EPOCH: Epoch = Epoch::new();
        for (wall, flags, kind) in [
            (false, 0, 0),
            (true, 1, 1),
            (true, 1 | (1 << 29), 1),
            (false, 2, 2),
            (true, 3, 2),
            (true, 3 | (1 << 28), 1),
            (false, 8, 0),
            (false, 14, 2),
            (false, 14 | (1 << 28), 2),
            (false, 14 | (1 << 29), 2),
        ] {
            for generation in [0x6123_abcd, u32::MAX] {
                let m = fixture(image, wall, flags, generation);
                let before = m.overlay.clone();
                let cpu = CpuAbi {
                    esp: STACK,
                    mxcsr: 0x1fa0,
                    ..CpuAbi::xbox_thread()
                };
                let world = CollisionWorld::new(&EPOCH);
                let mut guest = m.clone();
                let mut c = ir_cpu(&cpu);
                oracle.run(&mut c, &mut guest, points::SIM_COLLISION_QUERY.base);
                assert_eq!(
                    sim::Memory::u32(&guest, OUT).unwrap(),
                    kind,
                    "wall={wall} flags={flags:#x}"
                );
                for _ in 0..2 {
                    let mut native = m.clone();
                    let predicted =
                        adapter::query::Prediction::gather(&cpu, &mut native, &world).unwrap();
                    assert_eq!(native.overlay, before);
                    assert_eq!(
                        predicted.compare(c.gpr[0] as u8, &guest).unwrap(),
                        None,
                        "wall={wall} flags={flags:#x} epoch={generation:#x}"
                    );
                    let mut corrupt = guest.clone();
                    corrupt.store(OUT + 0x26, &[3]);
                    assert!(
                        predicted
                            .compare(c.gpr[0] as u8, &corrupt)
                            .unwrap()
                            .is_some()
                    );
                    if flags == 8 {
                        corrupt = guest.clone();
                        corrupt.word(layout::OBJECT_MARKS + 4 * 5, generation);
                        assert!(
                            predicted
                                .compare(c.gpr[0] as u8, &corrupt)
                                .unwrap()
                                .is_some()
                        );
                    }
                }
                // The installed body: every byte the guest left outside
                // its dead stack (its frames below the entry and the
                // arguments it pops, but not the flags argument it
                // rewrites), AL and the callee-saved registers.
                let (mut nc, mut nm) = (cpu, m.clone());
                let t = adapter::query::query(&mut nc, &mut nm, &world)
                    .unwrap()
                    .unwrap_or_else(|| panic!("flags={flags:#x}: declined {:?}", sim_collision::take_bail()));
                let label = format!("native wall={wall} flags={flags:#x} epoch={generation:#x}");
                assert_eq!(t.target_pc, HARNESS_EXIT, "{label}");
                let returned = lifted::cpu::from_interp(&c);
                assert_eq!(nc.eax & 0xff, returned.eax & 0xff, "{label}: AL");
                assert_eq!(
                    (nc.esp, nc.ebx, nc.ebp, nc.esi, nc.edi),
                    (returned.esp, returned.ebx, returned.ebp, returned.esi, returned.edi),
                    "{label}: callee-saved"
                );
                let dead = (STACK - 0x10000, STACK + 4 + 0x18);
                for at in nm.touched(&guest, STACK, dead).into_iter().chain(STACK + 4..STACK + 8) {
                    assert_eq!(nm.get(at), guest.get(at), "{label}: byte {at:#x}");
                }
            }
        }
    });
}

#[test]
fn query_reaches_model_and_instance_walks_and_compares_their_marks() {
    with_oracle(
        "whole collision query with model and instance hits",
        |oracle, image| {
            for object in [false, true] {
                for first in [0, 1 << 28] {
                    let flags = first | if object { 8 } else { 4 };
                    let mut m = fixture(image, false, flags, 23);
                    // Keep the structure's BSP apart from the model's own part.
                    let mut scene = Builder::default();
                    scene.planes.push([0., 0., 1., -1.]);
                    scene.nodes.push((0, synth::leaf(0), synth::leaf(0)));
                    scene.leaves.push((0, 0, 0));
                    scene.write(OTHER_BSP, |at, b| m.store(at, b));
                    m.word(0x004e_0340, OTHER_BSP);
                    m.word(layout::STRUCTURE_BSP, OTHER_BSP);
                    let (expected, mark) = if object {
                        super::model::fixture(&mut m);
                        let object = super::model::ROOT;
                        let data = REFERENCES[0].2;
                        for row in [data, data + 0x20] {
                            m.word(row + 4, object);
                            m.float(row + 0x1c, 100.);
                        }
                        put_vector(&mut m, START, [0., -4., 0.]);
                        put_vector(&mut m, DELTA, [0., 12., 0.]);
                        (4, layout::OBJECT_MARKS + 4 * (object & 0xffff))
                    } else {
                        super::instance::populate_instance_cluster(&mut m, 2.);
                        put_vector(&mut m, START, [3., 3., 2.]);
                        put_vector(&mut m, DELTA, [0., 4., 0.]);
                        (3, engine::sim_collision::instance::MARK_BASE)
                    };
                    let cpu = CpuAbi {
                        esp: STACK,
                        mxcsr: 0x1fa0,
                        ..CpuAbi::xbox_thread()
                    };
                    let mut guest = m.clone();
                    let mut c = ir_cpu(&cpu);
                    oracle.run(&mut c, &mut guest, points::SIM_COLLISION_QUERY.base);
                    assert_eq!(sim::Memory::u32(&guest, OUT).unwrap(), expected);
                    static EPOCH: Epoch = Epoch::new();
                    let world = CollisionWorld::new(&EPOCH);
                    let p = adapter::query::Prediction::gather(&cpu, &mut m, &world).unwrap();
                    assert_eq!(p.compare(c.gpr[0] as u8, &guest).unwrap(), None);
                    let extra = mark + 0x40;
                    let untouched = sim::Memory::u32(&guest, extra).unwrap();
                    guest.word(extra, !untouched);
                    assert!(p.compare(c.gpr[0] as u8, &guest).unwrap().is_some());
                }
            }
        },
    );
}

#[test]
fn query_declines_when_live_sources_alias_the_recursive_frame_or_result() {
    with_oracle("whole query preflight", |_, image| {
        let cpu = CpuAbi {
            esp: STACK,
            mxcsr: 0x1fa0,
            ..CpuAbi::xbox_thread()
        };
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        for (at, alias) in [
            (STACK + 24, layout::ACTIVE_STRUCTURE),
            (STACK + 24, BSP),
            (STACK + 8, STACK - 0x800),
        ] {
            let mut m = fixture(image, false, 8, 23);
            m.word(at, alias);
            let before = m.overlay.clone();
            assert!(matches!(
                adapter::query::Prediction::gather(&cpu, &mut m, &world),
                Err(sim_collision::Bail::Alias)
            ));
            assert_eq!(m.overlay, before);
        }
        let mut m = fixture(image, false, 8, 23);
        let dirty = CpuAbi {
            eflags: cpu.eflags | 1 << 10,
            ..cpu
        };
        assert!(matches!(
            adapter::query::Prediction::gather(&dirty, &mut m, &world),
            Err(sim_collision::Bail::Env)
        ));
    });
}
