//! AS4 and AS5 (docs/engine/asset-store.md, "Build slices"): level images
//! on an invented map kept in an invented store, and the assets a level's
//! `LevelId` names from the store's catalogue. No disc bytes.
use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use content::{LevelId, MapId, Sha256};
use map::CacheType;
use map::build::{BuildLayout, RawBlob};
use tags::repo::skeleton::{Extent, Extents, Skeleton, partition};
use tags::repo::{StoreFile, TagKey, TagRepo};
use vfs::store_map::Levels as _;

use crate::level::*;
use crate::tests::{A, ADD_C, ADD_C_FILE, C, adds_c, base, cc, pkg, target};
use crate::{BuiltMap, Loaded, Report, Served, build_map};

const D: &str = "invented\\dmg\\d";

/// A fresh folder under the system's temp dir.
fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("workspace-as4-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A package that adds a damage tag at `path` to maps of `maps`.
fn adds(id: &str, path: &str, maps: &str) -> Loaded {
    pkg(
        id,
        &[],
        &[("tag", ADD_C_FILE, Some(path), maps)],
        &[(ADD_C_FILE, ADD_C.as_bytes())],
    )
}

/// The invented map with one raw resource: its bytes and the resource's
/// (offset, length).
fn base_with_raw() -> (Vec<u8>, (u64, u64)) {
    let mut set = base();
    set.raw.push(RawBlob {
        bytes: (0..0x1800u32).map(|i| (i * 7 + i / 251) as u8).collect(),
        align: 0x10,
    });
    let (bytes, layout) = set.build().expect("the invented map builds");
    (bytes, (u64::from(layout.raw[0]), 0x1800))
}

/// Keep `bytes` in the store at `root` as the map `name`, as the extractor
/// does: the raw resource at `raw` an extent of its own file, everything
/// else the skeleton.
fn put_store(root: &Path, name: &str, bytes: &[u8], raw: (u64, u64)) -> String {
    let mut repo = TagRepo::create(root).unwrap();
    let len = bytes.len() as u64;
    let (o, l) = raw;
    let h = repo.put_raw(&bytes[o as usize..(o + l) as usize]).unwrap();
    let parts = partition(len, &[(0, o), (o + l, len - o - l)], &[(o, l, h.clone())]);
    let file = Sha256::of(bytes).to_string();
    let mut read = |at: u64, n: u64| -> tags::repo::Result<Vec<u8>> {
        Ok(bytes[at as usize..(at + n) as usize].to_vec())
    };
    repo.put_skeleton(name, &file, len, &parts, &mut read)
        .unwrap();
    h
}

struct Store {
    root: PathBuf,
    tags: PathBuf,
    images: PathBuf,
    bytes: Vec<u8>,
    raw: (u64, u64),
    raw_hash: String,
}

fn store(name: &str) -> Store {
    let root = temp(name);
    let tags = root.join("tags");
    let (bytes, raw) = base_with_raw();
    let raw_hash = put_store(&tags, "tiny", &bytes, raw);
    Store {
        images: images_dir(&root),
        root,
        tags,
        bytes,
        raw,
        raw_hash,
    }
}

impl Drop for Store {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn read_image(s: &Store, id: &LevelId) -> Vec<u8> {
    let (_, mut f) = open_image(&s.images, &s.tags.join("raw"), id).expect("the image opens");
    let mut out = Vec::new();
    f.read_to_end(&mut out).unwrap();
    out
}

#[test]
fn an_image_reads_back_as_build_maps_map() {
    let s = store("image");
    let packages = vec![adds_c("c")];
    let level = assemble(&s.tags, &s.images, &packages, "Tiny").unwrap();
    assert!(level.built());
    assert_eq!(level.packages, ["c"]);
    assert_eq!(level.map_id, None);
    assert_eq!(level.base, Sha256::of(&s.bytes));
    assert_eq!(level.id, level_id("tiny", Sha256::of(&s.bytes), &packages));
    let image = level.image.clone().expect("an image");
    let (want, _) = build_map(&s.bytes, &packages, &target()).unwrap();
    let got = read_image(&s, &level.id);
    assert!(got == want, "the image differs from build_map's map");
    assert_eq!(
        (image.len, image.file),
        (want.len() as u64, Sha256::of(&want))
    );
    // The base's raw resource is still the store's file, at its offset.
    let ext = Extents::parse(&std::fs::read_to_string(&image.extents).unwrap()).unwrap();
    assert!(
        ext.extents
            .iter()
            .any(|e| e.hash == s.raw_hash && e.offset == s.raw.0),
        "{ext:?}"
    );
    // The image holds less than the map: the raw resource is not copied.
    let skel = Skeleton::decode(&std::fs::read(&image.skeleton).unwrap()).unwrap();
    assert!((skel.bytes.len() as u64) < image.len - s.raw.1 + 1);
    assert_eq!(images(&s.images).len(), 1);
}

#[test]
fn a_level_with_nothing_over_it_is_the_disc_bytes() {
    let s = store("retail");
    let base = Sha256::of(&s.bytes);
    // No packages; one for campaign maps only; one the map already holds.
    for packages in [
        vec![],
        vec![adds("camp", D, "campaign")],
        vec![adds("held", A, "all")],
    ] {
        let level = assemble(&s.tags, &s.images, &packages, "tiny").unwrap();
        assert!(level.image.is_none() && !level.built());
        assert_eq!(level.map_id, Some(MapId::retail("tiny", base)));
        assert_eq!(level.id, level_id("tiny", base, &[]));
        assert!(level.packages.is_empty());
    }
    let held = assemble(&s.tags, &s.images, &[adds("held", A, "all")], "tiny").unwrap();
    assert_eq!(held.held, ["held"]);
    assert!(!s.images.exists(), "no image is written");
}

#[test]
fn a_set_change_gives_another_id_and_the_first_image_is_still_a_hit() {
    let s = store("sets");
    let one = vec![adds_c("c")];
    let two = vec![adds_c("c"), adds("d", D, "multiplayer")];
    let first = assemble(&s.tags, &s.images, &one, "tiny").unwrap();
    assert!(first.built());
    let second = assemble(&s.tags, &s.images, &two, "tiny").unwrap();
    assert!(second.built());
    assert_ne!(first.id, second.id);
    assert_eq!(second.packages, ["c", "d"]);
    let again = assemble(&s.tags, &s.images, &one, "tiny").unwrap();
    assert!(!again.built(), "a hit");
    assert_eq!(again.id, first.id);
    assert_eq!(again.image, first.image);
    let ids: Vec<LevelId> = images(&s.images).into_iter().map(|i| i.id).collect();
    let mut want = vec![first.id, second.id];
    want.sort();
    assert_eq!(ids, want);
    // A damaged image is built again, to the same bytes.
    let image = first.image.unwrap();
    std::fs::write(&image.skeleton, b"not an image").unwrap();
    let rebuilt = assemble(&s.tags, &s.images, &one, "tiny").unwrap();
    assert!(rebuilt.built());
    let (want, _) = build_map(&s.bytes, &one, &target()).unwrap();
    assert!(read_image(&s, &first.id) == want);
}

#[test]
fn a_level_the_store_lacks_is_named() {
    let s = store("lacks");
    let why = assemble(&s.tags, &s.images, &[adds_c("c")], "midship").unwrap_err();
    assert!(why.contains("midship"), "{why}");
}

#[test]
fn split_keeps_base_extents_and_stores_placed_raw() {
    let dir = temp("split");
    let raw_root = dir.join("raw");
    let bytes: Vec<u8> = (0..200u32).map(|i| (i * 13) as u8).collect();
    let hash = |b: &[u8]| Sha256::of(b).to_string();
    let base = Extents {
        file: String::new(),
        len: 0,
        extents: vec![
            // Equal at its offset: kept.
            Extent {
                offset: 10,
                len: 10,
                hash: hash(&bytes[10..20]),
            },
            // Other bytes now: not kept.
            Extent {
                offset: 30,
                len: 10,
                hash: hash(b"0123456789"),
            },
            // Past the end: not kept.
            Extent {
                offset: 195,
                len: 10,
                hash: hash(b"0123456789"),
            },
        ],
    };
    let mut puts = Vec::new();
    let mut put = |b: &[u8]| {
        let h = hash(b);
        let p = raw_root.join(&h[..2]).join(&h);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b).unwrap();
        puts.push(h.clone());
        Ok::<String, String>(h)
    };
    // The second placed range overlaps the first and is left in it.
    let (skel, ext) = split(&bytes, &base, &[(50, 10), (55, 10)], &mut put).unwrap();
    assert_eq!(puts, [hash(&bytes[50..60])]);
    let at: Vec<(u64, u64)> = ext.extents.iter().map(|e| (e.offset, e.len)).collect();
    assert_eq!(at, [(10, 10), (50, 10)]);
    assert_eq!(skel.ranges, [(0, 10), (20, 30), (60, 140)]);
    assert_eq!(ext.file, hash(&bytes));
    // The base extent's raw file, as the store holds it.
    let h = hash(&bytes[10..20]);
    std::fs::create_dir_all(raw_root.join(&h[..2])).unwrap();
    std::fs::write(raw_root.join(&h[..2]).join(&h), &bytes[10..20]).unwrap();
    let mut f = StoreFile::from_parts("x", skel, ext, raw_root).unwrap();
    let mut back = Vec::new();
    f.read_to_end(&mut back).unwrap();
    assert_eq!(back, bytes);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_image_over_the_tag_memory_bound_is_refused_naming_the_tags() {
    let bound = map::writer::max_metadata_size(CacheType::Multiplayer).unwrap();
    let mut report = Report::default();
    report.added.push(TagKey {
        group: cc(b"jpt!"),
        path: C.into(),
    });
    report.changed.insert(TagKey {
        group: cc(b"jpt!"),
        path: A.into(),
    });
    let built = |memory_size: u32| BuiltMap {
        bytes: Vec::new(),
        report: report.clone(),
        layout: BuildLayout {
            memory_size,
            ..Default::default()
        },
        raw: Vec::new(),
    };
    let why = check_tag_memory(&built(bound + 0x1000), CacheType::Multiplayer, 0).unwrap_err();
    assert!(
        why.contains(C) && why.contains(A) && why.contains("0x1000"),
        "{why}"
    );
    check_tag_memory(&built(bound), CacheType::Multiplayer, 0).unwrap();
    // A base already larger than the retail bound may stay that large.
    check_tag_memory(
        &built(bound + 0x1000),
        CacheType::Multiplayer,
        bound + 0x1000,
    )
    .unwrap();
}

#[test]
fn trim_drops_the_least_recently_used_and_spares_kept_ids() {
    let dir = temp("trim");
    let skel = Skeleton {
        len: 4,
        ranges: vec![(0, 4)],
        bytes: b"abcd".to_vec(),
    };
    let ext = Extents {
        file: Sha256::of(b"abcd").to_string(),
        len: 4,
        extents: Vec::new(),
    };
    let ids = [LevelId([1; 32]), LevelId([2; 32]), LevelId([3; 32])];
    let now = std::time::SystemTime::now();
    for (n, id) in ids.iter().enumerate() {
        let img = write_image(&dir, id, &skel, &ext).unwrap();
        let when = now - std::time::Duration::from_secs(1000 * (3 - n as u64));
        std::fs::File::options()
            .append(true)
            .open(&img.extents)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }
    let total: u64 = images(&dir).iter().map(|i| i.disk).sum();
    let keep: BTreeSet<LevelId> = [ids[0]].into();
    // Room for two: the oldest is kept, so the next oldest goes.
    let gone = trim(&dir, total - 1, &keep);
    assert_eq!(gone, [ids[1]]);
    let left: Vec<LevelId> = images(&dir).into_iter().map(|i| i.id).collect();
    assert_eq!(left, [ids[0], ids[2]]);
    assert!(trim(&dir, u64::MAX, &BTreeSet::new()).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn store_levels_assemble_once_and_record_what_they_served() {
    let s = store("levels");
    let served = Served::default();
    let mut levels = StoreLevels::new(
        s.tags.clone(),
        s.images.clone(),
        vec![adds_c("c")],
        served.clone(),
    );
    let first = levels.image("TINY").unwrap().expect("an image");
    let again = levels.image("tiny").unwrap().expect("an image");
    assert_eq!(first, again);
    let rec = served.lock().unwrap()["tiny"].clone();
    assert_eq!(rec.packages, ["c"]);
    assert_eq!((rec.builds, rec.hits), (1, 0));
    assert_eq!(rec.key, first.id);
    assert!(levels.image("midship").is_err());
    let rec = served.lock().unwrap()["midship"].clone();
    assert!(rec.error.is_some());
}

/// An invented tag: a root of one word, of `group`.
fn invented(group: &[u8; 4], path: &str, word: u32) -> tags::repo::LooseTag {
    tags::repo::LooseTag {
        key: TagKey::new(cc(group), path),
        body: tags::repo::Body::Tree(tags::repo::Tree {
            nodes: vec![tags::repo::Node {
                bytes: word.to_le_bytes().to_vec(),
                count: 1,
                size: 4,
                align: 4,
                fields: Vec::new(),
            }],
            ..Default::default()
        }),
    }
}

/// Build and write the catalogue of the store at `store`.
fn write_catalogue(store: &Path) {
    let schema = tags::schema::Schema::default();
    let (c, report) = tags::repo::catalogue::build(store, &schema).unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    tags::repo::catalogue::write(store, &c).unwrap();
}

/// [`store()`]'s level with a manifest and a catalogue: two invented
/// tags, of one content.
fn catalogued(name: &str) -> Store {
    let s = store(name);
    let mut repo = TagRepo::create(&s.tags).unwrap();
    let mut m = tags::repo::store::Manifest {
        map: "tiny".into(),
        ..Default::default()
    };
    for path in ["b\\zeta", "a\\alpha"] {
        let t = invented(b"weap", path, 7);
        repo.put(&t).unwrap();
        m.tags.push((t.key.clone(), None));
    }
    repo.put_manifest(&m, &[0; 0x800]).unwrap();
    write_catalogue(&s.tags);
    s
}

/// A level with nothing over it names every asset of its manifest by
/// hash, from the store's catalogue: its `LevelId` moves with an asset.
/// With a package over it no asset is named, and a store with no current
/// catalogue is refused, naming what writes it.
#[test]
fn a_level_names_its_assets_by_hash_from_the_catalogue() {
    let s = catalogued("level-assets");
    let catalogue = tags::repo::catalogue::read(&s.tags).unwrap();
    let assets = level_assets(&s.tags, "tiny", catalogue.as_ref()).unwrap();
    // Sorted by key; two tags of one content share a hash.
    let names: Vec<&str> = assets.iter().map(|a| a.0.as_str()).collect();
    assert_eq!(names, ["weap:a\\alpha", "weap:b\\zeta"]);
    assert_eq!(assets[0].1, assets[1].1);
    let base = Sha256::of(&s.bytes);
    let level = assemble(&s.tags, &s.images, &[], "tiny").unwrap();
    assert!(level.image.is_none() && !level.built());
    assert_eq!(level.assets, assets);
    assert_eq!(level.id, level_id_of("tiny", base, &[], assets.clone()));
    assert_ne!(level.id, level_id("tiny", base, &[]));
    // With a package over it, the packages stand for every asset.
    let layered = assemble(&s.tags, &s.images, &[adds_c("c")], "tiny").unwrap();
    assert!(layered.assets.is_empty());
    assert_eq!(layered.id, level_id("tiny", base, &[adds_c("c")]));

    // A changed asset moves the id, once the catalogue is built again (as
    // `prepare` does after an extraction).
    let key = TagKey::new(cc(b"weap"), "a\\alpha");
    let repo = TagRepo::open(&s.tags).unwrap();
    std::fs::write(
        repo.path_of(&key, None),
        tags::repo::format::encode(&invented(b"weap", "a\\alpha", 9)),
    )
    .unwrap();
    write_catalogue(&s.tags);
    let moved = assemble(&s.tags, &s.images, &[], "tiny").unwrap();
    assert_ne!(moved.id, level.id);
    assert_ne!(moved.assets[0].1, assets[0].1);
    assert_eq!(moved.assets[1].1, assets[1].1);
    let mut tables = tags::repo::normal::Tables::none();
    let changed = tags::repo::normal::form(&invented(b"weap", "a\\alpha", 9), &mut tables).unwrap();
    assert_eq!(moved.assets[0].1, crate::asset_hash(&changed));

    // A manifest that changes leaves the catalogue stale: refused, naming
    // what rewrites it.
    let mut m = repo.manifest("tiny").unwrap();
    m.name = "renamed".into();
    repo.write_manifest(&m).unwrap();
    let why = assemble(&s.tags, &s.images, &[], "tiny").unwrap_err();
    assert!(why.contains("stale") && why.contains("h2 build"), "{why}");
    let why = level_assets(&s.tags, "tiny", catalogue.as_ref()).unwrap_err();
    assert!(why.contains("stale"), "{why}");
    // No catalogue at all: refused the same way.
    std::fs::remove_file(s.tags.join(tags::repo::catalogue::CATALOGUE_FILE)).unwrap();
    let why = assemble(&s.tags, &s.images, &[], "tiny").unwrap_err();
    assert!(
        why.contains("no catalogue") && why.contains("workspace.extract"),
        "{why}"
    );
    // The game's levels: the catalogue is read once and the level served.
    write_catalogue(&s.tags);
    let mut levels = StoreLevels::new(
        s.tags.clone(),
        s.images.clone(),
        Vec::new(),
        Served::default(),
    );
    assert!(levels.image("tiny").unwrap().is_none());
    assert!(levels.image("midship").is_err());
    // A store with no manifest for the level names no asset.
    let plain = store("level-no-manifest");
    assert!(level_assets(&plain.tags, "tiny", None).unwrap().is_empty());
}
