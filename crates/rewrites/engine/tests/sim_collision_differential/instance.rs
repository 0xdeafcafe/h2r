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
                    check(oracle, &e, &c, &m, instance::empty_cluster, None, "count exit"),
                    count <= 0
                );
            }
        }
    });
}
