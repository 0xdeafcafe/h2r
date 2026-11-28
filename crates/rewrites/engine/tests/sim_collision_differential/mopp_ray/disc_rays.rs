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
            let (mut compared, mut declined) = (0, 0);
            for i in 0..400 {
                case.control = if i % 2 == 0 { 0x007f } else { 0x027f };
                let a = [(); 3].map(|_| frame_coordinate(&mut rng));
                let b = [(); 3].map(|_| frame_coordinate(&mut rng));
                case.from = world_point(image, &case.code, a, 0.0);
                case.to = world_point(image, &case.code, b, 0.0);
                let label = format!("{tag} cluster {at:#x} ray {i}");
                let (cpu, mut ram) = setup_at(image, &mut rng, &case, at);
                ram.tag = Some((view.address(), view.bytes(), version));
                match check_cast(oracle, image, &cpu, ram, &case, false, &label) {
                    Some(_) => compared += 1,
                    None => declined += 1,
                }
            }
            eprintln!("{tag} cluster {at:#x}: {compared} compared, {declined} declined");
            matched += compared;
        });
        assert!(
            programs > 0 && matched > 0,
            "no cluster ray compared to the guest"
        );
    });
}
