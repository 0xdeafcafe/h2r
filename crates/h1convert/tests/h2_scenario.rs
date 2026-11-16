//! The Halo 2 side of the multiplayer placements: netgame flag kinds, read
//! off every multiplayer scenario on the owner's disc (`$H2_DISC`).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;

use map::{CacheType, Map};

/// `scnr+0x118`: netgame flags, `0x20` each: position, facing, `u16` type at
/// `0x10`, `u16` team at `0x12` (crates/scenario-edit/src/layout.rs).
const NETGAME_FLAGS: usize = 0x118;
const FLAG_LEN: usize = 0x20;

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn netgame_flag_kinds_on_the_disc() {
    let disc = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc).unwrap())).unwrap();
    // type -> (count, maps, teams seen)
    let mut kinds: BTreeMap<u16, (usize, usize, BTreeMap<u16, usize>)> = BTreeMap::new();
    for e in iso.entries().unwrap() {
        if !e.path.ends_with(".map") {
            continue;
        }
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let Ok(mut m) = Map::from_region(
            BufReader::new(File::open(&disc).unwrap()),
            base,
            e.size.into(),
        ) else {
            continue;
        };
        if m.header().cache_type != CacheType::Multiplayer {
            continue;
        }
        let s = m.scenario().unwrap();
        let (id, address) = (s.id, s.location.unwrap().address);
        let b = m.read_tag_bytes(id).unwrap();
        let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let (count, at) = (
            u(NETGAME_FLAGS) as usize,
            (u(NETGAME_FLAGS + 4).wrapping_sub(address)) as usize,
        );
        let mut seen = std::collections::BTreeSet::new();
        let mut per_map: BTreeMap<u16, usize> = BTreeMap::new();
        let mut line = String::new();
        for i in 0..count {
            let f = &b[at + i * FLAG_LEN..at + (i + 1) * FLAG_LEN];
            let ty = u16::from_le_bytes([f[0x10], f[0x11]]);
            let team = u16::from_le_bytes([f[0x12], f[0x13]]);
            let k = kinds.entry(ty).or_default();
            k.0 += 1;
            *k.2.entry(team).or_default() += 1;
            if seen.insert(ty) {
                k.1 += 1;
            }
            line += &format!(" {ty}/{team}");
            *per_map.entry(ty).or_default() += 1;
            if ty == 4 {
                assert_eq!(team, 8, "{}: oddball spawns are neutral", e.path);
            }
        }
        // Teleporter sources and destinations come in equal numbers.
        assert_eq!(per_map.get(&7), per_map.get(&8), "{}", e.path);
        eprintln!("{}:{line}", e.path);
    }
    for (ty, (n, maps, teams)) in &kinds {
        eprintln!("type {ty}: {n} flags on {maps} maps, teams {teams:?}");
    }
    assert!(!kinds.is_empty());
}
