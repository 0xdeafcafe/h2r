//! The installed G/H dispatch: compare every byte and register on the
//! authoritative exits, and check the prediction against the lifted guest
//! elsewhere. The graph and references are invented.
use super::*;
use adapter::spatial_parity::{dispatch, end};
use sim_collision::Entry as Spatial;

fn guest(m: &Ram<'_>) -> GuestRam {
    let mut g = Guest::new();
    let len = (m.image.len() as u32 + 0xfff) & !0xfff;
    g.map_zeroed(0, len, Permissions::RW).unwrap();
    g.write_bytes(0, m.image, 1).unwrap();
    for (&addr, &byte) in &m.overlay {
        g.set_u8(addr, byte).unwrap();
    }
    GuestRam(g)
}

fn fixture<'a>(
    image: &'a [u8],
    entry: Spatial,
    linked: bool,
    marked: bool,
    cluster: u16,
) -> (CpuAbi, Ram<'a>) {
    let mut m = fresh(image);
    let mut p = synth::Portals {
        clusters: vec![vec![]; 2],
        ..Default::default()
    };
    if linked {
        p.planes.push([1., 0., 0., 1.]);
        p.link(
            0,
            1,
            synth::PortalSpec {
                sphere: [1., 0., 0., 5.],
                ..Default::default()
            },
        );
    }
    let (_, bsp) = p.write(TOPOLOGY, |a, d| m.store(a, d));
    m.word(layout::ACTIVE_STRUCTURE, TOPOLOGY);
    m.word(layout::STRUCTURE_BSP, bsp);
    m.word(layout::FLOOD_EPOCH, 9);
    m.word(layout::CLUSTER_MARKS, 0);
    m.word(layout::CLUSTER_MARKS + 4, if marked { 9 } else { 0 });
    m.word(layout::VISIT_GENERATION, u32::MAX);
    put_vector(&mut m, CENTRE, [1., 0., 0.]);
    m.store(OUT, &[0xce; 48]);
    m.store(STACK - 0x480, &[0xab; 0x480]);
    let radius = 2.0f32.to_bits();
    if entry == Spatial::ClusterFlood {
        for (i, arg) in [0, CENTRE, radius, 4, OUT].into_iter().enumerate() {
            m.word(STACK + 4 + i as u32 * 4, arg);
        }
    } else {
        m.word(LOCATION + 4, u32::from(cluster));
        let first = synth::References {
            clusters: vec![
                vec![synth::Reference {
                    handle: 0xabcd0001,
                    kind: 4,
                    bounds: [1., 0., 0., 1.],
                }],
                vec![],
            ],
        };
        let empty = synth::References {
            clusters: vec![vec![], vec![]],
        };
        for ((head, array), ((heads, table, data), list)) in [
            (layout::COLLIDEABLE_HEADS, layout::COLLIDEABLE_ARRAY),
            (layout::OTHER_HEADS, layout::OTHER_ARRAY),
        ]
        .into_iter()
        .zip(REFERENCES.into_iter().zip([first, empty]))
        {
            list.write(heads, table, data, |a, d| m.store(a, d));
            m.word(head, heads);
            m.word(array, table);
        }
        m.word(layout::OBJECT_MARKS + 4, 0x2222);
        for (i, arg) in [0, 0, LOCATION, CENTRE, radius, OUT, 2]
            .into_iter()
            .enumerate()
        {
            m.word(STACK + 4 + 4 * i as u32, arg);
        }
    }
    let mut c = entry_cpu(&mut Rng(1), entry.addr());
    c.mxcsr = 0x1fa0;
    c.x87.control = 0x027f;
    c.x87.tag = 0xffff;
    c.x87.status = 0x20;
    (c, m)
}

#[test]
fn the_authoritative_spatial_exits_replay_the_full_frame() {
    with_oracle("authoritative spatial exits", |oracle, image| {
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        for (entry, linked) in [
            (Spatial::ClusterFlood, false),
            (Spatial::ClusterFlood, true),
            (Spatial::ObjectVisit, false),
        ] {
            let (mut c, m) = fixture(image, entry, linked, true, u16::MAX);
            let (mut original, mut native) = (guest(&m), guest(&m));
            let mut expected = ir_cpu(&c);
            oracle.run(&mut expected, &mut original, entry.addr());
            let t = dispatch(entry, 42, &mut c, &mut native.0, &world)
                .unwrap()
                .expect("exact exit");
            c.eip = t.target_pc;
            assert_eq!(ir_cpu(&c), expected, "{entry:?} linked={linked}");
            let start = native.0.mappings()[0].physical_start as usize;
            let other = original.0.mappings()[0].physical_start as usize;
            let first = &native.0.physical_ram()[start..start + image.len()];
            let second = &original.0.physical_ram()[other..other + image.len()];
            assert_eq!(
                first.iter().zip(second).position(|(a, b)| a != b),
                None,
                "{entry:?} linked={linked}"
            );
        }
    });
}

#[test]
fn spatial_predictions_match_the_lifted_guest_and_catch_extra_marks() {
    with_oracle("spatial parity outputs", |oracle, image| {
        static EPOCH: Epoch = Epoch::new();
        for entry in [Spatial::ClusterFlood, Spatial::ObjectVisit] {
            let world = CollisionWorld::new(&EPOCH);
            let thread = 7;
            for corrupt in [false, true] {
                let (mut c, m) = fixture(image, entry, true, false, 0);
                let mut gm = guest(&m);
                let before = c;
                assert!(
                    dispatch(entry, thread, &mut c, &mut gm.0, &world)
                        .unwrap()
                        .is_none()
                );
                assert_eq!(before, c);
                if entry == Spatial::ClusterFlood {
                    // The observer pair for G's recursive self-call must leave
                    // the outer prediction untouched.
                    let mut child = c;
                    child.esp -= 0x2c;
                    gm.0.set_u32(child.esp, 0x0014_a690).unwrap();
                    assert!(
                        dispatch(entry, thread, &mut child, &mut gm.0, &world)
                            .unwrap()
                            .is_none()
                    );
                    child.esp += 4 + layout::FLOOD_ARGS;
                    end(entry, thread, &child, &gm.0, &world);
                }
                let mut returned = ir_cpu(&c);
                oracle.run(&mut returned, &mut gm, entry.addr());
                if corrupt {
                    let base = if entry == Spatial::ClusterFlood {
                        layout::CLUSTER_MARKS
                    } else {
                        layout::OBJECT_MARKS
                    };
                    let at = base + 20;
                    gm.0.set_u32(at, gm.0.u32(at).unwrap() ^ 1).unwrap();
                }
                end(
                    entry,
                    thread,
                    &lifted::cpu::from_interp(&returned),
                    &gm.0,
                    &world,
                );
                let stats = world.stats();
                let p = if entry == Spatial::ClusterFlood {
                    stats.flood
                } else {
                    stats.visit
                };
                assert_eq!(
                    (p.matched, p.mismatched),
                    (1, u64::from(corrupt)),
                    "{entry:?}: {p:?}"
                );
            }
        }
    });
}
