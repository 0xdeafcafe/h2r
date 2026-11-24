//! AS10 (docs/engine/asset-store.md, "Build slices"): converted levels kept
//! in an invented store. The "converter's output" is an invented map; no
//! disc bytes.
use std::io::Read;
use std::path::Path;

use content::{MapId, Sha256};
use map::CacheType;
use map::build::TagSet;
use tags::repo::{StoreFile, TagRepo};

use crate::BaseMaps;
use crate::converted::*;
use crate::level::{assemble, images_dir};
use crate::level_tests::{base_with_raw, put_store, temp};
use crate::tests::{A, adds_c, cc, zeroed_tag};

const B: &str = "invented\\dmg\\b";
const D: &str = "invented\\dmg\\d";

/// No shared caches: the invented maps point into none.
struct NoDisc;

impl BaseMaps for NoDisc {
    fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        Err(format!("no map {name}"))
    }
}

/// An invented converter output: one damage tag per name.
fn output(names: &[&str]) -> Vec<u8> {
    let mut set = TagSet::new(CacheType::Multiplayer, "tiny", "invented\\scenarios\\tiny");
    set.add_group(cc(b"jpt!"), None, None);
    for n in names {
        zeroed_tag(&mut set, b"jpt!", n);
    }
    set.build().expect("the invented map builds").0
}

fn read_level(store: &Path, stem: &str) -> Vec<u8> {
    let mut out = Vec::new();
    StoreFile::open(store, stem)
        .expect("the level opens")
        .read_to_end(&mut out)
        .unwrap();
    out
}

#[test]
fn a_converted_level_is_in_the_store_byte_for_byte() {
    let dir = temp("conv-one");
    let store = dir.join("tags");
    let bytes = output(&[A, B]);
    let got = store_converted(&store, "h1_one", &bytes, &NoDisc, "halo1").unwrap();
    assert!(!got.unchanged);
    assert_eq!(got.file, Sha256::of(&bytes));
    assert_eq!(got.len, bytes.len() as u64);
    assert!(got.put.refused.is_empty(), "{:?}", got.put.refused);
    assert_eq!((got.put.new, got.put.same), (2, 0));
    assert!(got.skeleton.is_some());
    assert!(read_level(&store, "h1_one") == bytes);
    assert!(is_stored(&store, "h1_one", &got.file));
    assert!(!is_stored(&store, "h1_one", &Sha256::of(b"another file")));
    let want = Marked {
        stem: "h1_one".into(),
        converter: "halo1".into(),
        file: got.file.to_string(),
    };
    assert_eq!(marked(&store, "h1_one"), Some(want.clone()));
    assert_eq!(levels(&store), [want]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_second_level_stores_only_the_tags_the_store_lacks() {
    let dir = temp("conv-two");
    let store = dir.join("tags");
    store_converted(&store, "h1_one", &output(&[A, B]), &NoDisc, "halo1").unwrap();
    let bytes = output(&[A, B, D]);
    let two = store_converted(&store, "h1_two", &bytes, &NoDisc, "halo1").unwrap();
    assert_eq!((two.put.new, two.put.same), (1, 2));
    assert!(two.put.variants.is_empty());
    assert!(read_level(&store, "h1_two") == bytes);
    // The first level is as it was.
    assert!(read_level(&store, "h1_one") == output(&[A, B]));
    let repo = TagRepo::open(&store).unwrap();
    assert_eq!(repo.maps().unwrap(), ["h1_one", "h1_two"]);
    let stems: Vec<String> = levels(&store).into_iter().map(|m| m.stem).collect();
    assert_eq!(stems, ["h1_one", "h1_two"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_same_output_is_left_alone_and_a_new_one_replaces_the_level() {
    let dir = temp("conv-again");
    let store = dir.join("tags");
    let first = output(&[A, B]);
    store_converted(&store, "h1_one", &first, &NoDisc, "halo1").unwrap();
    let again = store_converted(&store, "h1_one", &first, &NoDisc, "halo1").unwrap();
    assert!(again.unchanged);
    assert_eq!((again.put.new, again.put.same), (0, 0));
    let second = output(&[A, D]);
    let swapped = store_converted(&store, "h1_one", &second, &NoDisc, "halo1").unwrap();
    assert!(!swapped.unchanged);
    assert!(read_level(&store, "h1_one") == second);
    assert!(is_stored(&store, "h1_one", &Sha256::of(&second)));
    assert!(!is_stored(&store, "h1_one", &Sha256::of(&first)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_converted_level_assembles_as_the_map_the_converter_wrote() {
    let dir = temp("conv-assemble");
    let store = dir.join("tags");
    let images = images_dir(&dir);
    let bytes = output(&[A, B]);
    store_converted(&store, "h1_one", &bytes, &NoDisc, "halo1").unwrap();
    // Nothing over it: the stored file, no image.
    let plain = assemble(&store, &images, &[], "H1_One").unwrap();
    assert!(plain.image.is_none() && !plain.built());
    assert_eq!(plain.base, Sha256::of(&bytes));
    assert_eq!(
        plain.map_id,
        Some(MapId::retail("h1_one", Sha256::of(&bytes)))
    );
    // A package over it: an image of the level, kept by its id.
    let level = assemble(&store, &images, &[adds_c("c")], "h1_one").unwrap();
    assert!(level.built());
    assert_eq!(level.packages, ["c"]);
    assert!(level.image.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_level_that_cannot_be_kept_is_a_named_error_and_no_marker() {
    let dir = temp("conv-errors");
    let store = dir.join("tags");
    let bytes = output(&[A]);
    for stem in ["", "H1_One", "h1/one", "h1 one", "shared", "mainmenu"] {
        let why = store_converted(&store, stem, &bytes, &NoDisc, "halo1").unwrap_err();
        assert!(why.contains("converted level"), "{stem:?}: {why}");
    }
    let why = store_converted(&store, "h1_one", &bytes, &NoDisc, "").unwrap_err();
    assert!(why.contains("converter"), "{why}");
    let why = store_converted(&store, "h1_one", b"not a map", &NoDisc, "halo1").unwrap_err();
    assert!(why.contains("h1_one") && why.contains("not a map"), "{why}");
    assert!(marked(&store, "h1_one").is_none());
    assert!(!StoreFile::exists(&store, "h1_one"));
    // A level the disc's extraction made is not overwritten.
    let (disc, raw) = base_with_raw();
    put_store(&store, "tiny", &disc, raw);
    let why = store_converted(&store, "tiny", &bytes, &NoDisc, "halo1").unwrap_err();
    assert!(
        why.contains("tiny") && why.contains("no converter made"),
        "{why}"
    );
    assert!(read_level(&store, "tiny") == disc);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_reextraction_of_the_disc_empties_the_converted_levels() {
    let dir = temp("conv-reset");
    let store = dir.join("tags");
    let bytes = output(&[A]);
    let got = store_converted(&store, "h1_one", &bytes, &NoDisc, "halo1").unwrap();
    let mut repo = TagRepo::open(&store).unwrap();
    repo.reset(tags::repo::EXTRACT_VERSION, tags::repo::format::VERSION)
        .unwrap();
    assert!(!is_stored(&store, "h1_one", &got.file));
    assert!(levels(&store).is_empty());
    // The converter's unit runs again and the level is back.
    let back = store_converted(&store, "h1_one", &bytes, &NoDisc, "halo1").unwrap();
    assert!(!back.unchanged);
    assert!(read_level(&store, "h1_one") == bytes);
    let _ = std::fs::remove_dir_all(&dir);
}
