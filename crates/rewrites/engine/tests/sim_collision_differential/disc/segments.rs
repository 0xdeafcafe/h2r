//! Compare the result record, hit byte and (D) the walk context against the
//! lifted entries over the owner's immutable tag payloads.
use super::*;
use adapter::vector_parity::Prediction;
use layout::context as ctx;

#[test]
#[ignore = "reads the owner's Halo 2 ISO (H2_DISC) and XBE (H2_XBE)"]
fn every_disc_bsp_segment_matches_the_guest() {
    assert!(
        common::xbe_path().is_some(),
        "set H2_XBE to run the disc sweep"
    );
    with_oracle("collision BSP segment sweep", |oracle, image| {
        let mut compared = [[0u64; 2]; 2];
        let mut exact = [[0u64; 2]; 2];
        for_each_tag(&[SBSP, COLL], |label, group, view, root| {
            for (i, header) in bsp_headers(view, group, root).into_iter().enumerate() {
                if word(header, 8 * block::NODES) == 0 {
                    continue;
                }
                let offset = header.as_ptr() as usize - view.bytes().as_ptr() as usize;
                let bsp = view
                    .address()
                    .checked_add(offset.try_into().unwrap())
                    .unwrap();
                let samples = Samples::new(view, header);
                let count = if group == SBSP {
                    STRUCTURE_POINTS
                } else {
                    OBJECT_POINTS
                };
                let world = CollisionWorld::new(&EPOCH);
                let mut ram = fresh(image);
                ram.tag = Some((view.address(), view.bytes(), TableVersion::fresh()));
                // A nonzero byte in the unused tail makes a spurious record clear visible.
                ram.word(RECORD + layout::record::OVERFLOW, 0xdead_beef);
                let mut rng = Rng(u64::from(bsp) << 1 | 1);
                let mut bails = [[0u64; sim_collision::BAILS]; 2];
                let mut straight = [0u64; 2];
                let mut counts = [0u64; 2];
                for n in 0..count {
                    let start = samples.point(&mut rng, n);
                    let end = if n.is_multiple_of(16) {
                        start
                    } else {
                        samples.point(&mut rng, n + 3)
                    };
                    put_vector(&mut ram, START, start);
                    put_vector(&mut ram, DELTA, std::array::from_fn(|k| end[k] - start[k]));
                    let flags = 1 + rng.below(3);
                    let mut c = entry_cpu(&mut rng, VECTOR.point.base);
                    (c.mxcsr, c.x87.control, c.x87.tag) = (0x1fa0, 0x027f, 0xffff);
                    c.eax = RECORD;
                    c.ecx = flags;
                    c.edx = bsp;
                    c.xmm[0][..4].copy_from_slice(&1.0f32.to_le_bytes());
                    ram.word(STACK + 4, 0);
                    ram.word(STACK + 8, 0);
                    ram.word(STACK + 12, START);
                    ram.word(STACK + 16, DELTA);
                    for entry in 0..=usize::from(n.is_multiple_of(10)) {
                        let prediction = if entry == 0 {
                            Prediction::gather(&c, &mut ram, &world)
                        } else {
                            c.eip = CONTEXT_ENTRY.point.base;
                            for (field, value) in [
                                (ctx::FLAGS, flags),
                                (ctx::BSP, bsp),
                                (ctx::BREAKABLE_COUNT, 0),
                                (ctx::MASK, 0),
                                (ctx::START, START),
                                (ctx::DELTA, DELTA),
                                (ctx::RECORD, RECORD),
                                (ctx::PREV_LEAF, u32::MAX),
                                (ctx::PREV_KIND, 0),
                                (ctx::PLANE, u32::MAX),
                            ] {
                                ram.word(CONTEXT + field, value);
                            }
                            ram.float(RECORD + layout::record::FRACTION, 1.0);
                            ram.word(RECORD + layout::record::COUNT, 0);
                            ram.word(STACK + 4, CONTEXT);
                            ram.word(STACK + 8, 0);
                            ram.float(STACK + 12, 0.0);
                            ram.float(STACK + 16, 1.0);
                            Prediction::gather_context(&c, &mut ram, &world)
                        };
                        match prediction {
                            Ok(predicted) => {
                                let mut guest = ram.clone();
                                let mut cpu = ir_cpu(&c);
                                oracle.run(&mut cpu, &mut guest, c.eip);
                                assert_eq!(
                                    predicted.compare(cpu.gpr[0] as u8, &guest).unwrap(),
                                    None,
                                    "{label} BSP {i} sample {n} entry {:#x}",
                                    c.eip
                                );
                                let mut native = ram.clone();
                                let mut candidate = c;
                                let result = if entry == 0 {
                                    adapter::straight::vector_test(
                                        &mut candidate,
                                        &mut native,
                                        &world,
                                    )
                                } else {
                                    adapter::straight::vector_context(
                                        &mut candidate,
                                        &mut native,
                                        &world,
                                    )
                                }
                                .unwrap();
                                if let Some(transfer) = result {
                                    same_cpu(&candidate, transfer, &cpu, label, n);
                                    same_memory(&native, &guest, label, n);
                                    straight[entry] += 1;
                                } else {
                                    assert_eq!(
                                        candidate, c,
                                        "{label} segment {n}: decline moved registers"
                                    );
                                    assert_eq!(
                                        native.overlay, ram.overlay,
                                        "{label} segment {n}: decline wrote"
                                    );
                                }
                                counts[entry] += 1;
                            }
                            Err(reason) => bails[entry][reason as usize] += 1,
                        }
                    }
                }
                eprintln!(
                    "{label} BSP {i}: C/D records {counts:?}, full CPU and memory {straight:?}"
                );
                for why in sim_collision::Bail::ALL {
                    let [c, d] = bails.map(|totals| totals[why as usize]);
                    if c + d != 0 {
                        eprintln!("  {}: C {c}, D {d}", why.name());
                    }
                }
                let family = usize::from(group == SBSP);
                for (sum, n) in compared[family].iter_mut().zip(counts) {
                    *sum += n;
                }
                for (sum, n) in exact[family].iter_mut().zip(straight) {
                    *sum += n;
                }
            }
        });
        assert!(
            exact.into_iter().flatten().all(|n| n > 0),
            "no straight full-state comparison in one family: {exact:?}"
        );
        assert!(
            compared.into_iter().flatten().all(|n| n > 0),
            "both entries must reach the oracle on structure and object BSPs: {compared:?}"
        );
    });
}
