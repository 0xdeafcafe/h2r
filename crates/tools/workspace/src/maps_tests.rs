//! PK9 (docs/editor/studio-redesign.md): `map` and `map-patch` items on an
//! invented base map. No disc bytes.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::PathBuf;

use content::manifest::Layer;
use content::package::{Lock, Manifest, Package};
use content::{Library, Sha256};
use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, Map};
use vfs::Rewrite as _;

use super::*;
use crate::tests::{A, ADD_C, ADD_C_FILE, C, base, cc, pkg, target};

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("workspace-pk9-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn base_map() -> Vec<u8> {
    base().build().expect("the invented map builds").0
}

fn tags_of(bytes: &[u8]) -> Vec<(String, String)> {
    let map = Map::from_reader(Cursor::new(bytes)).unwrap();
    map.tags()
        .iter()
        .map(|t| {
            (
                tags::defs::fourcc_text(t.group),
                t.name.clone().unwrap_or_default(),
            )
        })
        .collect()
}

fn has(bytes: &[u8], group: &str, name: &str) -> bool {
    tags_of(bytes).iter().any(|(g, n)| g == group && n == name)
}

/// The disc's map `tiny`, and no other.
struct Disc(Vec<u8>);

impl BaseMaps for Disc {
    fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        if name == "tiny" {
            Ok(self.0.clone())
        } else {
            Err(format!("no map {name}"))
        }
    }
}

/// No disc at all.
struct NoDisc;

impl BaseMaps for NoDisc {
    fn read(&self, _: &str) -> Result<Vec<u8>, String> {
        Err("no disc".into())
    }
}

/// An invented kit: a map of only its globals, remembering what it was asked.
#[derive(Default)]
struct Kit {
    asked: RefCell<Vec<(String, String, Vec<String>)>>,
    version: String,
    /// What the ground generator was asked: the map's name, its base, how
    /// many labels, and the map it was given.
    grounded: RefCell<Vec<(String, String, usize, Vec<u8>)>>,
    ground_version: String,
}

impl Fresh for Kit {
    fn build(&self, job: &FreshJob<'_>) -> Result<Vec<u8>, String> {
        self.asked.borrow_mut().push((
            job.name.to_owned(),
            job.scenario.to_owned(),
            job.kits.to_vec(),
        ));
        let mut set = TagSet::new(CacheType::Multiplayer, job.name, job.scenario);
        set.add_group(cc(b"matg"), None, None);
        set.add_tag(
            cc(b"matg"),
            "globals\\globals",
            TagPayload::Built(Block::zeroed(0x40)),
        )
        .map_err(|e| e.to_string())?;
        Ok(set.build().map_err(|e| e.to_string())?.0)
    }

    fn version(&self) -> String {
        self.version.clone()
    }

    fn ground(&self, job: &GroundJob<'_>) -> Result<Vec<u8>, String> {
        self.grounded.borrow_mut().push((
            job.name.to_owned(),
            job.base.to_owned(),
            job.ground.labels.len(),
            job.map.to_vec(),
        ));
        let mut bytes = job.map.to_vec();
        bytes.extend_from_slice(b"ground");
        Ok(bytes)
    }

    fn ground_version(&self) -> String {
        self.ground_version.clone()
    }
}

const FRESH_TOML: &str = "mode = \"fresh\"\nbase = \"tiny\"\nname = \"fresh\"\n\
    scenario = 'invented\\scenarios\\fresh'\nkits = [\"stage\"]\ntitle = \"Fresh\"\n";
const ONTO_TOML: &str = "mode = \"onto\"\nbase = \"tiny\"\nname = \"scratch\"\n";

/// A package with a `map` item (`map.toml`) and a `tag` item that adds `C`
/// for the map named `takes`.
fn map_package(id: &str, toml: &str, takes: &str) -> Loaded {
    pkg(
        id,
        &[],
        &[
            ("map", "map.toml", None, "all"),
            ("tag", ADD_C_FILE, Some(C), &format!("[\"{takes}\"]")),
        ],
        &[
            ("map.toml", toml.as_bytes()),
            (ADD_C_FILE, ADD_C.as_bytes()),
        ],
    )
}

/// A package of one `map-patch` item over `patch`, for `tiny` with `sha`.
fn patch_package(patch: &[u8], sha: &Sha256) -> Loaded {
    let text = "[package]\nformat = 1\nid = \"mp\"\nname = \"mp\"\n\
        [[item]]\nkind = \"map-patch\"\nsource = \"tiny.ppf\"\ninputs = [\"tiny.base.toml\"]\n";
    let sidecar = format!("map = \"tiny\"\nsha256 = \"{sha}\"\ntitle = \"Snipers\"\n");
    let files: BTreeMap<String, Vec<u8>> = [
        ("tiny.ppf".to_owned(), patch.to_vec()),
        ("tiny.base.toml".to_owned(), sidecar.into_bytes()),
    ]
    .into();
    let lock = Lock::new(
        files
            .iter()
            .map(|(n, b)| (n.clone(), Sha256::of(b)))
            .collect(),
    );
    Loaded {
        package: Package::new(Manifest::from_toml(text).unwrap(), lock).unwrap(),
        files,
    }
}

#[test]
fn a_fresh_map_builds_to_a_derived_package_with_what_it_takes() {
    let kit = Kit::default();
    let p = map_package("stage", FRESH_TOML, "fresh");
    let built = build_maps(&[p], &Disc(base_map()), &kit);
    assert!(
        built.failed.is_empty(),
        "{built:?}"
    );
    let [m] = built.maps.as_slice() else {
        panic!("one map: {built:?}")
    };
    assert_eq!(m.dir, "fresh");
    assert_eq!((m.package.as_str(), m.item), ("stage", 0));
    assert_eq!(
        kit.asked.borrow().as_slice(),
        [(
            "fresh".to_owned(),
            "invented\\scenarios\\fresh".to_owned(),
            vec!["stage".to_owned()]
        )]
    );
    assert!(m.manifest.base.derived);
    assert_eq!(m.manifest.package.name, "Fresh");
    assert_eq!(
        m.manifest.list.as_ref().and_then(|l| l.base.as_deref()),
        Some("tiny")
    );
    let bytes = &m.files[0].1;
    assert_eq!(m.manifest.patches[0].sha256, Sha256::of(bytes));
    // The kit's own tags, and the item the map takes; not the base's.
    assert!(has(bytes, "matg", "globals\\globals"));
    assert!(has(bytes, "jpt!", C));
    assert!(!has(bytes, "jpt!", A));
}

#[test]
fn an_onto_map_is_its_base_plus_what_it_takes() {
    let original = base_map();
    let p = map_package("scratch", ONTO_TOML, "scratch");
    let built = build_maps(&[p.clone()], &Disc(original.clone()), &NoFresh);
    let [m] = built.maps.as_slice() else {
        panic!("one map: {built:?}")
    };
    let bytes = &m.files[0].1;
    let mut want: Vec<_> = tags_of(&original);
    want.push(("jpt!".into(), C.into()));
    let mut have = tags_of(bytes);
    want.sort();
    have.sort();
    assert_eq!(have, want, "only the taken tag is new");
    // The bytes are the build's, the path the rewrite takes.
    let taken = restrict(&[p], &|i| i.kind == content::package::Kind::Tag);
    let (by_build, _) = build_map(
        &original,
        &taken,
        &Target {
            name: "scratch",
            ..target()
        },
    )
    .unwrap();
    assert!(*bytes == by_build);
}

#[test]
fn an_onto_map_that_takes_nothing_is_refused() {
    let p = pkg(
        "s",
        &[],
        &[("map", "map.toml", None, "all")],
        &[("map.toml", ONTO_TOML.as_bytes())],
    );
    let built = build_maps(&[p], &Disc(base_map()), &NoFresh);
    assert!(built.maps.is_empty());
    assert!(
        built.failed[0].why.contains("no item names the map"),
        "{built:?}"
    );
}

#[test]
fn a_fresh_map_with_no_builder_and_a_missing_disc_fail_by_name() {
    let p = map_package("stage", FRESH_TOML, "fresh");
    let built = build_maps(&[p.clone()], &Disc(base_map()), &NoFresh);
    assert!(built.failed[0].why.contains("no builder for fresh maps"));
    let built = build_maps(&[p], &NoDisc, &NoFresh);
    assert!(built.failed[0].why.contains("the base map tiny"));
}

const GROUND_YAML: &str = "labels:\n  - {label: ff_drop, position: [1.0, 2.0, 0.5], facing: 1.5}\n";

fn ground_package(yaml: &str) -> Loaded {
    let toml = format!("{ONTO_TOML}ground = \"ground.yaml\"\n");
    pkg(
        "g",
        &[],
        &[("map", "map.toml", None, "all")],
        &[("map.toml", toml.as_bytes()), ("ground.yaml", yaml.as_bytes())],
    )
}

#[test]
fn a_map_with_a_ground_is_built_over_what_the_item_makes() {
    let kit = Kit::default();
    let p = ground_package(GROUND_YAML);
    assert!(check_package(&p).is_empty(), "{:?}", check_package(&p));
    let built = build_maps(&[p.clone()], &Disc(base_map()), &kit);
    assert!(built.failed.is_empty(), "{built:?}");
    let [m] = built.maps.as_slice() else {
        panic!("one map: {built:?}")
    };
    assert_eq!(m.dir, "scratch");
    // The generator was asked once, over the map as the item makes it, with
    // the labels the file holds.
    assert_eq!(
        kit.grounded.borrow().as_slice(),
        [("scratch".to_owned(), "tiny".to_owned(), 1, base_map())]
    );
    // What it returned is the map: a mark the generator makes is in the bytes.
    assert!(m.files[0].1.ends_with(b"ground"), "the generator's bytes are the map");
    assert_eq!(m.manifest.patches[0].sha256, Sha256::of(&m.files[0].1));
    // The labels and the generator's version are in the key.
    let again = build_maps(&[ground_package(GROUND_YAML)], &Disc(base_map()), &kit);
    assert_eq!(again.maps[0].key, m.key);
    let moved = GROUND_YAML.replace("1.0,", "9.0,");
    let other = build_maps(&[ground_package(&moved)], &Disc(base_map()), &kit);
    assert_ne!(other.maps[0].key, m.key);
    let newer = Kit {
        ground_version: "kit 2".into(),
        ..Kit::default()
    };
    let later = build_maps(&[p], &Disc(base_map()), &newer);
    assert_ne!(later.maps[0].key, m.key);
}

#[test]
fn a_map_with_a_ground_and_no_generator_stops_naming_it() {
    let built = build_maps(&[ground_package(GROUND_YAML)], &Disc(base_map()), &NoFresh);
    assert!(built.maps.is_empty());
    assert!(
        built.failed[0].why.contains("no builder for AI ground"),
        "{:?}",
        built.failed
    );
}

#[test]
fn a_ground_file_that_does_not_read_fails_the_check() {
    for (yaml, says) in [
        ("labels:\n  - {label: ff_drop, position: [0, 0, 0], team: red}\n", "team"),
        ("labels:\n  - {label: nonsense, position: [0, 0, 0]}\n", "nonsense"),
    ] {
        let p = ground_package(yaml);
        let found = check_package(&p);
        assert!(
            found.iter().any(|f| f.to_string().contains(says)),
            "{says}: {found:?}"
        );
    }
}

#[test]
fn a_map_patch_has_the_id_the_importer_gives_the_same_patch() {
    let original = base_map();
    let mut edited = original.clone();
    edited[0x100] ^= 1;
    let patch = content::patch::diff(&original, &edited, "invented").bytes;
    let p = patch_package(&patch, &Sha256::of(&original));
    assert!(check_package(&p).is_empty());
    // No disc is read to make it.
    let built = build_maps(&[p], &NoDisc, &NoFresh);
    let [m] = built.maps.as_slice() else {
        panic!("one map: {built:?}")
    };
    // What `h2 live maps import` writes for a patch kept as it is.
    let mut importer = content::Manifest::retail("tiny", Sha256::of(&original));
    importer.patches = vec![Layer {
        file: "whatever.ppf".into(),
        sha256: Sha256::of(&patch),
    }];
    assert_eq!(m.id, importer.map_id());
    assert_eq!(m.manifest.package.name, "Snipers");
    assert!(!m.manifest.base.derived);
    assert_eq!(m.files[0].1, patch);
    assert_eq!(m.dir, "mp-tiny");
}

#[test]
fn prepare_writes_the_packages_once_and_the_library_reads_them() {
    let maps = temp("prepare");
    let fresh = map_package("stage", FRESH_TOML, "fresh");
    let onto = map_package("scratch", ONTO_TOML, "scratch");
    let packages = [fresh, onto];
    let kit = Kit::default();
    let first = prepare(&maps, &packages, &Disc(base_map()), &kit);
    assert!(first.failed.is_empty(), "{first:?}");
    assert_eq!(first.written.len(), 2);
    let lib = Library::open(&maps);
    for m in &first.written {
        let p = lib
            .packages()
            .iter()
            .find(|p| p.dir_name() == m.dir)
            .unwrap_or_else(|| panic!("{} is a valid package", m.dir));
        assert_eq!(p.id, m.id);
    }
    // The same inputs write nothing.
    let second = prepare(&maps, &packages, &Disc(base_map()), &kit);
    assert_eq!((second.written.len(), second.unchanged.len()), (0, 2));
    // Nothing was built for them: the kit was asked once, by the first.
    assert_eq!(kit.asked.borrow().len(), 1);
    // A directory the user made is not replaced.
    let _ = std::fs::remove_file(maps.join("fresh").join(BUILT_FILE));
    let third = prepare(&maps, &packages, &Disc(base_map()), &kit);
    assert!(third.failed[0].why.contains("was not built from a package"));
    let _ = std::fs::remove_dir_all(&maps);
}

#[test]
fn a_map_is_built_again_only_when_what_it_is_made_from_changed() {
    let maps = temp("held");
    let fresh = map_package("stage", FRESH_TOML, "fresh");
    let onto = map_package("scratch", ONTO_TOML, "scratch");
    let packages = [fresh, onto];
    let v1 = Kit {
        version: "1".into(),
        ..Kit::default()
    };
    assert_eq!(
        prepare(&maps, &packages, &Disc(base_map()), &v1).written.len(),
        2
    );
    // The kit's version moves the fresh map and not the onto one.
    let v2 = Kit {
        version: "2".into(),
        ..Kit::default()
    };
    let again = prepare(&maps, &packages, &Disc(base_map()), &v2);
    assert_eq!(
        (again.written.len(), again.unchanged.len()),
        (1, 1),
        "{again:?}"
    );
    assert_eq!(again.written[0].dir, "fresh");
    assert_eq!(v2.asked.borrow().len(), 1);
    let _ = std::fs::remove_dir_all(&maps);
}

#[test]
fn check_refuses_what_cannot_build() {
    let bad = |toml: &str| {
        let p = pkg(
            "b",
            &[],
            &[("map", "map.toml", None, "all")],
            &[("map.toml", toml.as_bytes())],
        );
        check_package(&p)
            .first()
            .map(|f| f.what.clone())
            .unwrap_or_default()
    };
    assert!(
        bad("mode = \"fresh\"\nbase = \"tiny\"\nname = \"x\"\nkits = [\"k\"]\n")
            .contains("needs a scenario")
    );
    assert!(
        bad("mode = \"fresh\"\nbase = \"tiny\"\nname = \"x\"\nscenario = \"s\"\n")
            .contains("needs kits")
    );
    assert!(bad("mode = \"onto\"\nbase = \"tiny\"\nname = \"TINY\"\n").contains("new map"));
    assert!(
        bad("mode = \"onto\"\nbase = \"tiny\"\nname = \"x\"\nscenario = \"s\"\n")
            .contains("for a fresh map")
    );
    assert!(bad("mode = \"clone\"\nbase = \"tiny\"\nname = \"x\"\n").contains("map.toml"));
    assert!(
        bad("mode = \"onto\"\nbase = \"tiny\"\nname = \"x\"\nground = \"nope\"\n")
            .contains("no ground file")
    );
}

#[test]
fn a_map_patch_without_its_base_hash_is_refused() {
    let original = base_map();
    let mut edited = original.clone();
    edited[0x100] ^= 1;
    let patch = content::patch::diff(&original, &edited, "invented").bytes;
    let mut p = patch_package(&patch, &Sha256::of(&original));
    // A sidecar with no hash.
    p.files
        .insert("tiny.base.toml".into(), b"map = \"tiny\"\n".to_vec());
    assert!(check_package(&p)[0].what.contains("the base file"));
    // No inputs file at all.
    let mut manifest = p.package.manifest.clone();
    manifest.items[0].inputs.clear();
    p.package = Package::new(manifest, p.package.lock.clone()).unwrap();
    assert!(check_package(&p)[0].what.contains("inputs file"));
    // Not a patch.
    let mut q = patch_package(&patch, &Sha256::of(&original));
    q.files.insert("tiny.ppf".into(), b"nope".to_vec());
    assert!(check_package(&q)[0].what.contains("not a PPF"));
}

#[test]
fn map_items_change_no_other_map() {
    let p = map_package("stage", FRESH_TOML, "fresh");
    let mut set = base();
    let before = set.build().unwrap().0;
    // The build for another map runs the `map` item as nothing and the tag
    // item (named for `fresh`) not at all.
    let report = build(&mut set, &[p], &target(), None).unwrap();
    assert!(report.added.is_empty() && report.changed.is_empty());
    assert!(set.build().unwrap().0 == before);
}

#[test]
fn the_rewrite_does_not_apply_what_a_built_map_took() {
    let p = map_package("stage", FRESH_TOML, "fresh");
    assert_eq!(
        built_names(&[p.clone()]).into_iter().collect::<Vec<_>>(),
        ["fresh"]
    );
    let mut rw = PackageRewrite::new(vec![p]);
    // Served the base's bytes under the built map's name, the item that
    // adds `C` is not run again.
    assert!(rw.rewrite("maps\\fresh.map", &base_map()).is_none());
}
