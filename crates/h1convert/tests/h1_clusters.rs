//! Halo 1's clusters as the structure keeps them, from the owner's Halo 1
//! disc (`$H1_ISO`). Only counts and ranges are printed.

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;

fn levels() -> Vec<(String, H1Map)> {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let names = h1convert::convert::MAPS
        .iter()
        .map(|m| m.0)
        .chain(h1map::disc::CAMPAIGN_LEVELS.iter().copied());
    let mut out = Vec::new();
    for n in names {
        let Some(e) = disc.map(n).cloned() else {
            continue;
        };
        let m = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        out.push((n.to_owned(), m));
    }
    out
}

fn i16_at(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}

/// The fields `h1convert::structure::sound_zones` reads: each cluster's
/// background sound (`+0x04`) and sound environment (`+0x06`) is -1 or
/// indexes the structure's palettes (`+0x1fc`, `+0x208`), and the
/// structure has a leaf (`+0xe0`, cluster at `+0x08`) per collision leaf,
/// each naming a real cluster.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn clusters_name_their_sounds_and_leaves_their_cluster() {
    let mut bsps = 0;
    for (name, h1) in levels() {
        for bsp in h1.structure_bsps().unwrap() {
            let s = h1.structure_bsp(&bsp).unwrap();
            let clusters = s.space.block_at(s.root, 0x134, 0x68).unwrap();
            let leaves = s.space.block_at(s.root, 0xe0, 0x10).unwrap();
            let bg = s.space.block_at(s.root, 0x1fc, 0x74).unwrap().len() as i16;
            let env = s.space.block_at(s.root, 0x208, 0x50).unwrap().len() as i16;
            let coll = s.collision_bsps().unwrap()[0].leaves.len();
            let mut zones = std::collections::BTreeSet::new();
            for (k, c) in clusters.iter().enumerate() {
                let (b, e) = (i16_at(c, 4), i16_at(c, 6));
                assert!(
                    (-1..bg).contains(&b),
                    "{name} {}: cluster {k} sound {b} of {bg}",
                    bsp.name
                );
                assert!(
                    (-1..env).contains(&e),
                    "{name} {}: cluster {k} environment {e} of {env}",
                    bsp.name
                );
                zones.insert((b, e));
            }
            assert_eq!(leaves.len(), coll, "{name} {}", bsp.name);
            for (k, l) in leaves.iter().enumerate() {
                let c = i16_at(l, 8);
                assert!(
                    c >= 0 && (c as usize) < clusters.len(),
                    "{name} {}: leaf {k} cluster {c}",
                    bsp.name
                );
            }
            println!(
                "{name} {}: {} clusters, {} sound zones, {} leaves",
                bsp.name,
                clusters.len(),
                zones.len(),
                leaves.len()
            );
            bsps += 1;
        }
    }
    assert!(bsps > 0);
}
