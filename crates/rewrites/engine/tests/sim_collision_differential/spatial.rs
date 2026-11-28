//! The installed G/H dispatch against the lifted guest: every byte and
//! register on the frame-exact exits, and on the contested flood and the
//! populated visit the return value, the output, the epochs, the active
//! bytes, the mark tables and both PE flags, entered with PE clear. The
//! graph and references are invented.
use super::*;
use adapter::spatial::dispatch;
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
            let t = dispatch(entry, &mut c, &mut native.0, &world).unwrap();
            c.eip = t.expect("exact exit").target_pc;
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
fn contested_spatial_calls_match_the_lifted_guest_from_pe_clear() {
    with_oracle("contested spatial calls", |oracle, image| {
        static EPOCH: Epoch = Epoch::new();
        for entry in [Spatial::ClusterFlood, Spatial::ObjectVisit] {
            let world = CollisionWorld::new(&EPOCH);
            let (mut c, m) = fixture(image, entry, true, false, 0);
            // The live game's state: neither PE flag sticky.
            (c.mxcsr, c.x87.status) = (0x1f80, 0);
            let (mut original, mut native) = (guest(&m), guest(&m));
            let mut expected = ir_cpu(&c);
            oracle.run(&mut expected, &mut original, entry.addr());
            let expected = lifted::cpu::from_interp(&expected);
            let t = dispatch(entry, &mut c, &mut native.0, &world)
                .unwrap()
                .expect("a contested call is answered");
            assert_eq!(t.target_pc, expected.eip, "{entry:?}");
            assert_eq!(c.eax, expected.eax, "{entry:?} EAX");
            assert_eq!(
                (c.ebx, c.ebp, c.esi, c.edi, c.esp),
                (expected.ebx, expected.ebp, expected.esi, expected.edi, expected.esp),
                "{entry:?} callee-saved"
            );
            assert_eq!(c.mxcsr, expected.mxcsr, "{entry:?} MXCSR");
            assert_eq!(c.x87.status & 0x20, expected.x87.status & 0x20, "{entry:?} x87 PE");
            // Everything but the dead stack: the frame below the entry and
            // the argument slots the callee pops (G and H reuse them).
            let start = native.0.mappings()[0].physical_start as usize;
            let other = original.0.mappings()[0].physical_start as usize;
            let first = &native.0.physical_ram()[start..start + image.len()];
            let second = &original.0.physical_ram()[other..other + image.len()];
            let dead = (STACK - 0x480) as usize..(STACK + 4 + 0x1c) as usize;
            let differs = first
                .iter()
                .zip(second)
                .enumerate()
                .position(|(i, (a, b))| a != b && !dead.contains(&i));
            assert_eq!(differs, None, "{entry:?}");
        }
    });
}
