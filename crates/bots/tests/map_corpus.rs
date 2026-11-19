//! Asset-backed oracle for the same generator the host uses. No game or cache writes.
use bots::{autonav, nav::Kind};
use map::{CacheType, Map};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::PathBuf;

fn measure<R: Read + Seek>(map: &mut Map<R>) -> (usize, usize) {
    let name = map.header().name.clone();
    let data = autonav::build_map(map).unwrap_or_else(|e| panic!("{name}: {e}"));
    let edges = || data.iter().flat_map(|d| &d.nav.edges);
    let tele = edges().filter(|e| e.kind == Kind::Teleporter).count();
    let lift = edges()
        .filter(|e| matches!(e.kind, Kind::Lift | Kind::Elevator))
        .count();
    let jumps = edges().filter(|e| e.kind == Kind::CrouchJump).count();
    let places = &data[0].places;
    assert!(
        !places.vantages.is_empty() && !places.lanes.is_empty(),
        "{name}"
    );
    if name == "h1_bloodgulch" || name == "h1_sidewinder" {
        for d in 0..2 {
            let (min, max) = places
                .vantages
                .iter()
                .map(|p| p.at[d])
                .fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
            assert!(max - min > 25., "{name} clusters on one flank");
        }
        assert!(
            jumps > 0 && !places.pads.is_empty(),
            "{name} needs crouch jumps and power pads"
        );
    }
    if matches!(name.as_str(), "h1_bloodgulch" | "h1_sidewinder") {
        let portals = edges().filter(|e| e.kind == Kind::Teleporter);
        let near =
            |a: [f32; 3], b: [f32; 3]| (0..3).map(|i| (a[i] - b[i]).powi(2)).sum::<f32>() < 1.0;
        let reciprocal = portals
            .clone()
            .filter(|p| portals.clone().any(|q| near(p.a, q.b) && near(p.b, q.a)))
            .count();
        assert_eq!(
            reciprocal,
            if name == "h1_sidewinder" { 2 } else { 0 },
            "{name}"
        );
        let (sniper, rocket) = if name == "h1_bloodgulch" {
            (Some(120 * 30), Some(90 * 30))
        } else {
            (None, Some(120 * 30))
        };
        for (model, expected) in [("sniper_rifle", sniper), ("rocket_launcher", rocket)] {
            assert!(
                places.pads.iter().any(|pad| {
                    pad.period == expected
                        && pad
                            .model
                            .and_then(|id| map.tag(map::TagId(id)).ok())
                            .and_then(|tag| tag.name.as_deref())
                            .and_then(|path| path.rsplit(['/', '\\']).next())
                            .is_some_and(|name| name.replace(' ', "_") == model)
                }),
                "{name} {model}: {expected:?}"
            );
        }
    }
    println!(
        "{name}: {tele} teleporter, {lift} lift, {jumps} crouch jump, {} vantages",
        places.vantages.len()
    );
    (tele, lift)
}

#[test]
#[ignore = "reads H2_DISC and the installed Map Pack and H1 conversions in H2_DATA"]
fn map_corpus_traversal_and_places() {
    let disc = PathBuf::from(std::env::var_os("H2_DISC").expect("set H2_DISC"));
    let data =
        PathBuf::from(std::env::var_os("H2_DATA").expect("set H2_DATA to the installed data root"));
    let expected = [
        ("ascension", 1, 1),
        ("beavercreek", 2, 0),
        ("burial_mounds", 0, 0),
        ("coagulation", 2, 0),
        ("colossus", 0, 5),
        ("cyclotron", 0, 4),
        ("foundation", 0, 4),
        ("headlong", 1, 2),
        ("lockout", 0, 1),
        ("midship", 0, 5),
        ("waterworks", 2, 0),
        ("zanzibar", 0, 0),
        ("containment", 2, 0),
        ("warlock", 4, 4),
        ("deltatap", 0, 0),
        ("turf", 0, 0),
        ("backwash", 0, 4),
        ("dune", 2, 0),
        ("elongation", 0, 0),
        ("gemini", 4, 0),
        ("triplicate", 0, 0),
        ("h1_beavercreek", 2, 0),
        ("h1_bloodgulch", 2, 0),
        ("h1_boardingaction", 8, 0),
        ("h1_carousel", 4, 0),
        ("h1_chillout", 3, 0),
        ("h1_damnation", 2, 0),
        ("h1_hangemhigh", 0, 0),
        ("h1_longest", 0, 0),
        ("h1_prisoner", 0, 0),
        ("h1_putput", 30, 0),
        ("h1_ratrace", 4, 0),
        ("h1_sidewinder", 4, 0),
        ("h1_wizard", 4, 0),
    ];
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc).unwrap())).unwrap();
    let mut found = BTreeMap::new();
    for e in iso.entries().unwrap().into_iter().filter(|e| {
        expected
            .iter()
            .any(|(n, _, _)| e.path.ends_with(&format!("/{n}.map")))
    }) {
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let mut map = Map::from_region(
            BufReader::new(File::open(&disc).unwrap()),
            base,
            u64::from(e.size),
        )
        .unwrap();
        if map.header().cache_type == CacheType::Multiplayer {
            found.insert(map.header().name.clone(), measure(&mut map));
        }
    }
    let packages = data.join("disk/Partition1/TDATA/4D530064/$c");
    for offer in std::fs::read_dir(packages)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_dir())
    {
        for f in std::fs::read_dir(offer.path()).unwrap().flatten() {
            let p = f.path();
            if !expected
                .iter()
                .any(|&(n, _, _)| p.file_stem() == Some(n.as_ref()))
                || p.extension() != Some("map".as_ref())
            {
                continue;
            }
            let mut map = Map::open(p).unwrap();
            found.insert(map.header().name.clone(), measure(&mut map));
        }
    }
    for (name, tp, lf) in expected {
        assert_eq!(found.get(name), Some(&(tp, lf)), "{name}");
    }
}
