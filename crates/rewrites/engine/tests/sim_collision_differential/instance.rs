//! The cluster trace's empty exit compares its entire CPU and saved stack.
use super::*;
use engine::sim_collision::instance;

#[test]
fn empty_instance_cluster_matches_full_machine_state() {
    let e = Entry {
        point: &points::COLLISION_INSTANCE_TRACE,
        eax: u32::MAX,
        dead: (0, 0),
    };
    with_oracle("empty instance cluster", |oracle, image| {
        let mut rng = Rng(37);
        for misalignment in 0..16 {
            for count in [0, -1i32, i32::MIN, 1] {
                let mut c = entry_cpu(&mut rng, e.point.base);
                let mut m = fresh(image);
                c.esp -= misalignment;
                c.eax = 2;
                m.word(c.esp, HARNESS_EXIT);
                m.word(layout::ACTIVE_STRUCTURE, STRUCTURE);
                m.word(STRUCTURE + 0xa0, CLUSTERS);
                m.word(CLUSTERS + 2 * 0xb0 + 0xa8, count as u32);
                assert_eq!(
                    check(
                        oracle,
                        &e,
                        &c,
                        &m,
                        |c, g, w| instance::empty_cluster(c, g, w),
                        None,
                        "count exit"
                    ),
                    count <= 0
                );
            }
        }
    });
}

const DEF: u32 = 0x0069_0000;
const PLACE: u32 = 0x006a_0000;
const CODE: u32 = 0x006a_1000;
const STATE: u32 = 0x006a_2000;
const MATERIAL: u32 = 0x006a_3000;
const EPOCH: u32 = 71;

#[test]
fn populated_instance_cluster_against_the_lifted_callbacks() {
    with_oracle("populated instance cluster", |oracle, image| {
        let mut rng = Rng(164);
        for scale in [0.5, 1.0, 4.0] {
            for case in [
                "hit",
                "first hit",
                "already marked",
                "filtered",
                "table",
                "outside",
                "broken",
            ] {
                // A rotated, translated wall, local x=0. A world +y ray
                // crosses it through the centre of its square.
                let mut bsp = Builder::default();
                bsp.planes.push([1., 0., 0., 0.]);
                bsp.nodes.push((0, synth::leaf(0), synth::SOLID));
                bsp.leaves.push((0, 1, 0));
                bsp.refs.push((0, synth::surface(0)));
                let ring = bsp.ring(
                    0,
                    &[[0., -4., -4.], [0., 4., -4.], [0., 4., 4.], [0., -4., 4.]],
                    false,
                );
                let surface_flags = if case == "filtered" { 0x10 } else { 8 };
                bsp.surfaces.push((0, ring, surface_flags, 0, 0));

                let mut m = fresh(image);
                bsp.write(DEF + 0x70, |at, bytes| m.store(at, bytes));
                m.word(layout::ACTIVE_STRUCTURE, STRUCTURE);
                m.store(layout::BSP_INDEX, &0u16.to_le_bytes());
                m.word(STRUCTURE + 0xc, 1);
                m.word(STRUCTURE + 0x10, MATERIAL);
                m.store(MATERIAL + 8, &1309u16.to_le_bytes());
                m.word(STRUCTURE + 0x9c, 1);
                m.word(STRUCTURE + 0xa0, CLUSTERS);
                m.word(CLUSTERS + 0xa8, 1);
                m.word(CLUSTERS + 0xac, CODE);
                m.word(STRUCTURE + 0x138, 1);
                m.word(STRUCTURE + 0x13c, DEF);
                m.word(STRUCTURE + 0x140, 1);
                m.word(STRUCTURE + 0x144, PLACE);
                m.word(instance::MARK_BASE - 4, EPOCH);
                m.word(
                    instance::MARK_BASE,
                    if case == "already marked" { EPOCH } else { 19 },
                );
                if case == "table" {
                    m.word(STRUCTURE + 0xc4, 1);
                    m.word(STRUCTURE + 0xc8, STATE + 0x400);
                    m.word(STATE + 0x440, 1);
                    m.word(STATE + 0x444, STATE + 0x450);
                    m.store(STATE + 0x450, &u16::MAX.to_le_bytes());
                }
                m.word(0x004e_d280, STATE);
                m.word(STATE + 0x201, u32::from(case != "broken"));
                m.float(PLACE, scale);
                put_vector(&mut m, PLACE + 4, [0., 1., 0.]);
                put_vector(&mut m, PLACE + 0x10, [-1., 0., 0.]);
                put_vector(&mut m, PLACE + 0x1c, [0., 0., 1.]);
                put_vector(&mut m, PLACE + 0x28, [3., 5., 2.]);
                m.store(PLACE + 0x34, &0u16.to_le_bytes());
                put_vector(&mut m, PLACE + 0x3c, [3., 5., 2.]);
                m.float(PLACE + 0x48, if case == "outside" { 0.25 } else { 64. });
                let start = [if case == "outside" { 33. } else { 3. }, 3., 2.];
                let delta = [0., 4., 0.];
                put_vector(&mut m, START, start);
                put_vector(&mut m, DELTA, delta);
                let code = mopp::Code {
                    offset: [0.; 3],
                    scale: 1.,
                    info: 37,
                    bytecode: vec![0x30],
                };
                m.store(CODE, &code.to_block());
                m.word(CODE + 0x24, 4);
                let flags = match case {
                    "first hit" => 1 << 28,
                    "table" => 0x40000,
                    _ => 0,
                };
                m.store(RECORD, &[0xa7; 0x5c]);
                m.float(RECORD + 4, 1.);
                m.word(STACK + 4, flags);
                m.word(STACK + 8, 1);
                m.word(STACK + 12, RECORD);
                let mut c = entry_cpu(&mut rng, points::COLLISION_INSTANCE_TRACE.base);
                c.eax = 0;
                c.esi = START;
                c.edi = DELTA;
                c.mxcsr = 0x1fa0;
                (c.x87.control, c.x87.tag, c.x87.status) = (0x027f, 0xffff, 0);
                let entry = m.overlay.clone();
                static GENERATION: Epoch = Epoch::new();
                let world = CollisionWorld::new(&GENERATION);
                let q = instance::Query {
                    cluster: 0,
                    start: start.map(f32::to_bits),
                    delta: delta.map(f32::to_bits),
                    flags,
                    filter: 1,
                    limit: 1f32.to_bits(),
                };
                let plan = instance::predict_with_marks(&c, &mut m, &world, q, EPOCH, &[]).unwrap();
                assert_eq!(m.overlay, entry, "predictor wrote memory");
                let mut guest = m.clone();
                let mut cpu = ir_cpu(&c);
                oracle.run(&mut cpu, &mut guest, c.eip);
                let hit = case == "hit" || case == "first hit";
                assert_eq!(cpu.gpr[0] as u8, u8::from(hit), "{case} {scale} lifted");
                assert_eq!(plan.hit.is_some(), hit, "{case} {scale} native");
                if let Some(hit) = &plan.hit {
                    hit.writes(|offset, bytes| m.store(RECORD + offset, bytes));
                }
                assert_eq!(plan.marks.len(), usize::from(case != "already marked"));
                for (at, value) in plan.marks {
                    m.word(at, value);
                }
                for k in 0..0x5c {
                    assert_eq!(
                        m.get(RECORD + k),
                        guest.get(RECORD + k),
                        "{case} {scale} result+{k:#x}"
                    );
                }
                for at in [instance::MARK_BASE, CODE + 0x24, c.fs_base] {
                    for k in 0..4 {
                        assert_eq!(m.get(at + k), guest.get(at + k), "{case} word {at:#x}");
                    }
                }
            }
        }
    });
}

#[test]
#[ignore = "reads the owner's maps and executable"]
fn every_placement_inverse_matches_the_guest() {
    use sim_collision::instance::PlacementMatrix;
    use tags::structure::InstancedGeometryInstance;
    const INVERT: u32 = 0x0014_1590;
    with_oracle("placement inverses", |oracle, image| {
        let mut accepted = 0;
        let mut refused = 0;
        let mut rng = Rng(137);
        let one = u32::from_le_bytes(image[layout::ONE as usize..][..4].try_into().unwrap());
        super::disc::for_each_tag(&[map::FourCC(*b"sbsp")], |label, _, view, address| {
            let root = view.range(address, 0x148).unwrap();
            for (index, row) in view
                .block(root, 0x140, InstancedGeometryInstance::SIZE)
                .unwrap()
                .into_iter()
                .enumerate()
            {
                let words = std::array::from_fn(|i| {
                    u32::from_le_bytes(row[4 * i..4 * i + 4].try_into().unwrap())
                });
                let inv = PlacementMatrix::from_words(words).inverse(one);
                let mut c = entry_cpu(&mut rng, INVERT);
                c.ecx = PLACE;
                c.eax = RECORD;
                c.mxcsr = 0x1fa0;
                let mut m = fresh(image);
                m.store(PLACE, &row[..52]);
                m.store(RECORD, &[0xa7; 52]);
                let mut cpu = ir_cpu(&c);
                oracle.run(&mut cpu, &mut m, INVERT);
                match inv {
                    Ok(matrix) => {
                        for (i, value) in matrix.words().into_iter().enumerate() {
                            let mut bytes = [0; 4];
                            m.read(RECORD + i as u32 * 4, &mut bytes).unwrap();
                            assert_eq!(
                                value.to_le_bytes(),
                                bytes,
                                "{label} placement {index} word {i}"
                            );
                        }
                        accepted += 1;
                    }
                    Err(sim_collision::Bail::Float) => refused += 1,
                    Err(why) => panic!("{label} placement {index}: {why:?}"),
                }
            }
        });
        eprintln!("placement inverses: {accepted} exact, {refused} outside FP range");
        assert!(accepted > 0, "no instance transforms compared");
    });
}
