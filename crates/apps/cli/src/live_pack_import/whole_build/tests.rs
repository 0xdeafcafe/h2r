//! Invented Xbox cache headers and directory entries. All data stays in
//! the test's private input and scratch directories.
use super::*;
use crate::live_mod_import::{InputKind, apply, layer_for};
use crate::live_pack_import::Scratch;
use std::fs;

fn cache(name: &str, kind: u32) -> Vec<u8> {
    let mut bytes = vec![0; HEADER_BYTES + 64];
    bytes[..4].copy_from_slice(b"daeh");
    bytes[4..8].copy_from_slice(&8_u32.to_le_bytes());
    bytes[0x140..0x144].copy_from_slice(&kind.to_le_bytes());
    bytes[0x198..0x198 + name.len()].copy_from_slice(name.as_bytes());
    let scenario = format!("levels\\invented\\{name}");
    bytes[0x1bc..0x1bc + scenario.len()].copy_from_slice(scenario.as_bytes());
    bytes[0x7fc..0x800].copy_from_slice(b"toof");
    bytes
}

#[test]
fn whole_build_matches_the_header_of_a_renamed_level_and_skips_unchanged_maps() {
    let tmp = Scratch::new().unwrap();
    let disc = tmp.0.join("owner");
    let maps = disc.join("MAPS");
    fs::create_dir_all(&maps).unwrap();
    let base = cache("invented", 1);
    fs::write(maps.join("First.map"), &base).unwrap();
    fs::write(maps.join("Second.map"), cache("another", 1)).unwrap();
    fs::write(maps.join("menu.map"), cache("menu", 2)).unwrap();
    let catalogue = RetailMaps::open(&disc, None).unwrap();
    assert_eq!(catalogue.stems().collect::<Vec<_>>(), ["first", "second"]);

    let file = tmp.0.join("ancient.MAP");
    fs::write(&file, &base).unwrap();
    assert_eq!(catalogue.changed_base(&file, &file).unwrap(), None);
    let mut patched = base.clone();
    patched[HEADER_BYTES + 11] = 45;
    fs::write(&file, &patched).unwrap();
    assert_eq!(
        catalogue.changed_base(&file, &file).unwrap().as_deref(),
        Some("first")
    );
    // Matching names which point at different levels cannot silently
    // create a patch over the wrong disc file.
    assert!(
        catalogue
            .changed_base(Path::new("second.map"), &file)
            .is_err()
    );
    let (name, delta, _) = layer_for(InputKind::Map, &patched, &base, "Test pack").unwrap();
    assert_eq!(name, "mod.ppf");
    assert_eq!(apply(&base, &delta).unwrap(), patched);
    assert!(delta.len() < base.len());

    for kind in [0, 2, 3, 4] {
        fs::write(&file, cache("resource", kind)).unwrap();
        assert_eq!(catalogue.changed_base(&file, &file).unwrap(), None);
    }
    fs::write(&file, cache("missing", 1)).unwrap();
    assert!(
        catalogue
            .changed_base(&file, &file)
            .unwrap_err()
            .to_string()
            .contains("matches no base")
    );
}

fn directory_entry(name: &str, sector: u32, size: u32, dir: bool) -> Vec<u8> {
    let mut record = vec![0; 4]; // child offsets
    record.extend(sector.to_le_bytes());
    record.extend(size.to_le_bytes());
    record.push(if dir { 0x10 } else { 0 });
    record.push(name.len() as u8);
    record.extend(name.bytes());
    record.resize(record.len().next_multiple_of(4), 0xff);
    record
}

#[test]
fn xiso_stages_multiplayer_map_bytes_only() {
    let tmp = Scratch::new().unwrap();
    let iso = tmp.0.join("community.xiso");
    let scratch = tmp.0.join("parts");
    let level = cache("garden", 1);
    let shared = cache("stuff", 3);
    let sector = xiso::SECTOR as usize;
    let mut img = vec![0u8; sector * 40];
    // A game partition with /maps/Garden.map and /maps/shared.map.
    let vd = sector * 32;
    img[vd..vd + 20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
    img[vd + 20..vd + 24].copy_from_slice(&33u32.to_le_bytes());
    img[vd + 24..vd + 28].copy_from_slice(&(sector as u32).to_le_bytes());
    let root = directory_entry("maps", 34, sector as u32, true);
    img[33 * sector..33 * sector + root.len()].copy_from_slice(&root);
    let mut dir = directory_entry("Garden.map", 35, level.len() as u32, false);
    let second = (dir.len() / 4) as u16;
    dir[2..4].copy_from_slice(&second.to_le_bytes());
    dir.extend(directory_entry(
        "shared.map",
        37,
        shared.len() as u32,
        false,
    ));
    img[34 * sector..34 * sector + dir.len()].copy_from_slice(&dir);
    img[35 * sector..35 * sector + level.len()].copy_from_slice(&level);
    img[37 * sector..37 * sector + shared.len()].copy_from_slice(&shared);
    fs::write(&iso, img).unwrap();

    let candidates = discover_iso(&iso, &scratch).unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].logical, Path::new("maps/Garden.map"));
    assert_eq!(fs::read(&candidates[0].file).unwrap(), level);
    assert_eq!(fs::read_dir(&scratch).unwrap().count(), 1);
    let retail = RetailMaps::open(&iso, None).unwrap();
    assert_eq!(
        retail
            .changed_base(&candidates[0].logical, &candidates[0].file)
            .unwrap(),
        None
    );
}
