use super::*;
use adapter::query::{Trace, trace_batch};

#[test]
fn trace_batch_matches_consecutive_lifted_queries_through_epoch_wrap() {
    with_oracle("batch of whole collision queries", |oracle, image| {
        static EPOCH: Epoch = Epoch::new();
        let mut ram = fixture(image, false, 14, u32::MAX - 1);
        ram.word(layout::CLUSTER_MARKS, 0);
        ram.word(layout::OBJECT_MARKS + 4 * 5, 0);
        let original = ram.overlay.clone();
        let world = CollisionWorld::new(&EPOCH);
        let queries = [14, 0, 14, 8, 4].map(|flags| Trace {
            start: [-2.0f32, 0.0, 0.0].map(f32::to_bits),
            delta: [4.0f32, 0.0, 0.0].map(f32::to_bits),
            flags,
            ignore: [HANDLE, u32::MAX],
        });
        let result = trace_batch(&mut ram, &world, &queries).unwrap();
        assert_eq!(ram.overlay, original);
        assert_eq!(result[1].cluster_epoch, None);
        assert_eq!(result[2].cluster_marks, [0]);
        assert_eq!(result[2].object_marks, [5]);
        assert_eq!(result[2].cluster_epoch, Some(0));
        for (q, answer) in queries.iter().zip(result) {
            let cpu = CpuAbi {
                esp: STACK,
                mxcsr: 0x1fa0,
                ..CpuAbi::xbox_thread()
            };
            // The six caller arguments are fresh for each invocation.
            for (i, word) in [q.flags, START, DELTA, q.ignore[0], q.ignore[1], OUT]
                .into_iter()
                .enumerate()
            {
                ram.word(STACK + 4 + i as u32 * 4, word);
            }
            oracle.run(
                &mut ir_cpu(&cpu),
                &mut ram,
                points::SIM_COLLISION_QUERY.base,
            );
            let read = |at| sim::Memory::u32(&ram, at).unwrap();
            assert_eq!(answer.hit_type, read(OUT));
            assert_eq!(answer.fraction, read(OUT + 4));
            assert_eq!(answer.position, [8, 12, 16].map(|off| read(OUT + off)));
            for (epoch, slots, epoch_at, marks_at) in [
                (
                    answer.cluster_epoch,
                    &answer.cluster_marks,
                    layout::FLOOD_EPOCH,
                    layout::CLUSTER_MARKS,
                ),
                (
                    answer.instance_epoch,
                    &answer.instance_marks,
                    0x004e_7c1c,
                    0x004e_7c20,
                ),
                (
                    answer.object_epoch,
                    &answer.object_marks,
                    layout::VISIT_GENERATION,
                    layout::OBJECT_MARKS,
                ),
            ] {
                if let Some(epoch) = epoch {
                    assert_eq!(read(epoch_at), epoch);
                    for &slot in slots {
                        assert_eq!(read(marks_at + 4 * slot), epoch);
                    }
                }
            }
        }
    });
}
