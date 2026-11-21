//! Cross-check the executable sites and the independent references on the owner discs.
use h1map::{Group, H1Map};
use h1objects::parity::{self, Rules};
use std::{
    fs::File,
    io::{BufReader, Cursor},
};

#[test]
#[ignore = "reads H1_ISO and H2_DISC"]
fn parity_references_cover_bloodgulch_and_a30() {
    let open = |key| {
        xiso::Xiso::open(BufReader::new(
            File::open(std::env::var(key).unwrap()).unwrap(),
        ))
        .unwrap()
    };
    let mut halo2 = open("H2_DISC");
    let entry = halo2.find("default.xbe").unwrap();
    let bytes = halo2.read(&entry).unwrap();
    let retail = xbe::Xbe::parse(&bytes).unwrap();
    // The barrel's call to the launch step and both real player control submissions.
    for (site, target) in [
        (0x1052c5, 0xfa100),
        (0x15075b, 0xc6de0),
        (0x150775, 0xc6de0),
    ] {
        let b = retail.bytes_at(site, 5).unwrap();
        assert_eq!(b[0], 0xe8);
        assert_eq!(
            (site + 5).wrapping_add(u32::from_le_bytes(b[1..].try_into().unwrap())),
            target
        );
    }
    let mut iso = open("H1_ISO");
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let exe = iso.find("default.xbe").unwrap();
    let rules = Rules::read(&iso.read(&exe).unwrap()).unwrap();
    assert_eq!(rules.hz, 30.);
    for level in ["bloodgulch", "a30"] {
        let map =
            H1Map::from_reader(Cursor::new(iso.read(disc.map(level).unwrap()).unwrap())).unwrap();
        let mut counts = [0; 2];
        for tag in map.tags() {
            if tag.group == Group::new(b"weap") {
                let gun = parity::gun(&map, tag).unwrap();
                assert_eq!(gun.charge_seconds.len(), gun.weapon.triggers.len());
                if tag.name == r"weapons\shotgun\shotgun" {
                    assert_eq!(
                        gun.weapon.triggers[0].flags & u32::from(rules.press_mask),
                        0
                    );
                    assert_eq!(gun.weapon.triggers[0].projectiles_per_shot, 15);
                    assert_eq!(gun.weapon.triggers[0].rounds_per_second, [1., 1.]);
                }
                counts[0] += 1;
            } else if tag.group == Group::new(b"bipd") {
                let model = parity::target(&map, tag, &rules).unwrap();
                assert!(!model.rays.is_empty(), "{level} {}", tag.name);
                assert!(model.poses.iter().all(|p| p.frames > 0));
                counts[1] += 1;
            }
        }
        assert!(counts.iter().all(|&n| n > 0), "{level} {counts:?}");
    }
}
