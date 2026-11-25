use super::super::disc::programs::{self, Kind};
use super::*;

#[test]
#[ignore = "reads the owner's Halo 2 ISO (H2_DISC) and XBE (H2_XBE)"]
fn every_disc_cluster_ray_matches_the_guest() {
    assert!(
        common::xbe_path().is_some(),
        "set H2_XBE to run the disc sweep"
    );
    with_oracle("collision cluster ray sweep", |oracle, image| {
        let (mut programs, mut matched) = (0, 0u64);
        programs::for_each(&[Kind::Cluster], |tag, _, view, at, code, _| {
            programs += 1;
            let mut rng = Rng(u64::from(at) << 1 | 1);
            static EPOCH: Epoch = Epoch::new();
            let world = CollisionWorld::new(&EPOCH);
            let version = TableVersion::fresh();
            let mut case = Case {
                code,
                from: [0.0; 4],
                to: [0.0; 4],
                instanced: true,
                filter: false,
                fraction: 1.0,
                control: 0x027f,
            };
            let (mut compared, mut declined, mut traps) = (0, 0, 0);
            for i in 0..400 {
                case.control = if i % 2 == 0 { 0x007f } else { 0x027f };
                let a = [(); 3].map(|_| frame_coordinate(&mut rng));
                let b = [(); 3].map(|_| frame_coordinate(&mut rng));
                case.from = world_point(image, &case.code, a, 0.0);
                case.to = world_point(image, &case.code, b, 0.0);
                let label = format!("{tag} cluster {at:#x} ray {i}");
                let expected = simulate(image, &case);
                if expected == Err(Bail::Data) {
                    traps += 1;
                    continue;
                }
                let before = outcome(&world);
                let (cpu, mut ram) = setup_at(image, &mut rng, &case, at);
                ram.tag = Some((view.address(), view.bytes(), version));
                let got = run(oracle, &cpu, &mut ram, &world);
                let after = outcome(&world);
                assert_eq!(arm::open_queries(), 0, "{label}: query stayed open");
                assert!(got.enters > 0, "{label}: guest did not enter interpreter");
                assert_eq!(after.mismatched, 0, "{label}: {:?}", after.first_mismatch);
                if let Ok(keys) = expected {
                    assert_eq!(keys, got.keys, "{label}: terminal key order");
                    assert_eq!(after.matched, before.matched + 1, "{label}: unverified");
                    compared += 1;
                } else {
                    assert_eq!(
                        after.unverified.iter().sum::<u64>(),
                        before.unverified.iter().sum::<u64>() + 1,
                        "{label}: missing bail"
                    );
                    declined += 1;
                }
            }
            eprintln!(
                "{tag} cluster {at:#x}: {compared} compared, {declined} declined, {traps} traps"
            );
            matched += compared;
        });
        assert!(
            programs > 0 && matched > 0,
            "no cluster ray compared to the guest"
        );
    });
}
