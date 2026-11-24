//! PK3 (docs/editor/studio-redesign.md): the game's set and
//! [`PackageRewrite`] on an invented map. No disc bytes.
use std::path::{Path, PathBuf};

use content::package::{Package, SetError, SetFile};
use map::Map;
use std::io::Cursor;
use vfs::Rewrite as _;

use super::*;
use crate::tests::{A, ADD_C, ADD_C_FILE, C, adds_c, base, cc, edit_text, edits, pkg, target};

const D: &str = "invented\\dmg\\d";

/// A package that adds a damage tag at `path`, for every map.
fn adds(id: &str, path: &str) -> Loaded {
    pkg(
        id,
        &[],
        &[("tag", ADD_C_FILE, Some(path), "all")],
        &[(ADD_C_FILE, ADD_C.as_bytes())],
    )
}

/// A fresh folder under the system's temp dir.
fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("workspace-pk3-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The invented base map's bytes.
fn base_map() -> Vec<u8> {
    base().build().expect("the invented map builds").0
}

fn has_tag(bytes: &[u8], group: &[u8; 4], name: &str) -> bool {
    let map = Map::from_reader(Cursor::new(bytes)).unwrap();
    map.tags()
        .iter()
        .any(|t| t.group == cc(group) && t.name.as_deref() == Some(name))
}

fn with_features(l: Loaded, features: &[&str]) -> Loaded {
    let mut manifest = l.package.manifest.clone();
    manifest.package.features = features.iter().map(|f| (*f).to_owned()).collect();
    Loaded {
        package: Package::new(manifest, l.package.lock.clone()).unwrap(),
        files: l.files,
    }
}

fn all_on(_: &str) -> bool {
    true
}

const PATH: &str = "maps\\tiny.map";

#[test]
fn the_rewrites_bytes_equal_the_builds() {
    let original = base_map();
    let packages = vec![adds("c", C), adds("d", D)];
    let mut rw = PackageRewrite::new(packages.clone());
    assert!(rw.wants(PATH));
    assert!(rw.wants("MAPS\\Tiny.map"));
    assert!(!rw.wants("maps\\sub\\tiny.map"));
    assert!(!rw.wants("sound\\tiny.map"));
    let served = rw.rewrite(PATH, &original).expect("the set adds to tiny");

    // workspace::build on the map's TagSet, by hand.
    let mut m = Map::from_reader(Cursor::new(original.as_slice())).unwrap();
    let unpin = MapUnpin::new(&mut m).ok();
    let mut set = map::build::TagSet::from_map(&mut m).unwrap();
    set.check_tag_memory = false;
    if let Some(p) = set.pinned.as_mut() {
        p.clear_index_tail();
    }
    let unpin = unpin.as_ref().map(|u| u as &dyn Unpin);
    build(&mut set, &packages, &target(), unpin).unwrap();
    let by_hand = set.build().unwrap().0;
    assert!(
        served == by_hand,
        "the rewrite's map differs from the build's"
    );
    let (shared, _) = build_map(&original, &packages, &target()).unwrap();
    assert!(served == shared);
    assert!(has_tag(&served, b"jpt!", C) && has_tag(&served, b"jpt!", D));
    assert!(has_tag(&served, b"jpt!", A));

    let rec = rw.served().lock().unwrap()["tiny"].clone();
    assert_eq!(rec.scope, "multiplayer");
    assert_eq!(rec.packages, ["c", "d"]);
    assert_eq!((rec.hits, rec.builds), (0, 1));
    assert_eq!(rec.added, 2);
}

#[test]
fn a_second_read_is_a_cache_hit() {
    let data = temp("cache");
    let original = base_map();
    let packages = vec![adds_c("c")];
    let mut first = PackageRewrite::new(packages.clone()).with_cache(&data);
    let built = first.rebuild("tiny", &original).unwrap().unwrap();
    assert!(!built.from_cache);
    let file = built.file.clone().expect("cached");
    assert!(file.starts_with(data.join("packages").join("cache")));
    let base_hex = content::Sha256::of(&original).to_string();
    let name = file.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with(&base_hex[..16]), "{name}");

    // The same rewrite again, and a new one (the next boot).
    let again = first.rebuild("tiny", &original).unwrap().unwrap();
    assert!(again.from_cache);
    assert!(again.bytes == built.bytes);
    let mut next = PackageRewrite::new(packages.clone()).with_cache(&data);
    let hit = next.rewrite(PATH, &original).unwrap();
    assert!(hit == built.bytes);
    let rec = next.served().lock().unwrap()["tiny"].clone();
    assert_eq!((rec.hits, rec.builds), (1, 0));
    let rec = first.served().lock().unwrap()["tiny"].clone();
    assert_eq!((rec.hits, rec.builds), (1, 1));

    // A changed package is a new key; a damaged cache file builds anew.
    let changed = vec![adds_c("c2")];
    let other = PackageRewrite::new(changed)
        .with_cache(&data)
        .rebuild("tiny", &original)
        .unwrap()
        .unwrap();
    assert!(!other.from_cache);
    std::fs::write(&file, b"not the map").unwrap();
    let rebuilt = PackageRewrite::new(packages)
        .with_cache(&data)
        .rebuild("tiny", &original)
        .unwrap()
        .unwrap();
    assert!(!rebuilt.from_cache);
    assert!(rebuilt.bytes == built.bytes);
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn a_package_off_or_with_its_feature_off_adds_nothing() {
    let original = base_map();
    let pogo = with_features(adds_c("pogo"), &["pogo"]);
    let off = |file: SetFile, feature: &dyn Fn(&str) -> bool| {
        let set = GameSet::assemble(vec![pogo.clone()], Vec::new(), Ok(file), feature);
        let mut rw = PackageRewrite::new(set.loaded());
        let served = rw.rebuild("tiny", &original).unwrap();
        (set, rw.wants(PATH), served.is_none())
    };

    let (set, wants, none) = off(
        SetFile {
            off: vec!["pogo".into()],
            ..SetFile::default()
        },
        &all_on,
    );
    assert!(!wants && none);
    assert_eq!(
        set.listed[0].why.as_deref(),
        Some("switched off in set.toml")
    );
    assert!(!set.listed[0].on);

    let (set, wants, none) = off(SetFile::default(), &|f| f != "pogo");
    assert!(!wants && none);
    assert_eq!(
        set.listed[0].why.as_deref(),
        Some("feature \"pogo\" is off")
    );

    let (set, wants, none) = off(SetFile::default(), &all_on);
    assert!(wants && !none);
    assert!(set.listed[0].on);
}

#[test]
fn a_map_that_holds_the_content_is_left_alone() {
    let original = base_map();
    let packages = vec![adds_c("c")];
    let mut rw = PackageRewrite::new(packages);
    let once = rw.rebuild("tiny", &original).unwrap().unwrap().bytes;
    // The built map, read again (say a map package made with it).
    let mut again = PackageRewrite::new(vec![adds_c("c")]);
    assert!(again.rewrite(PATH, &once).is_none());
    let rec = again.served().lock().unwrap()["tiny"].clone();
    assert_eq!(rec.held, ["c"]);
    assert!(rec.packages.is_empty());
}

#[test]
fn items_for_other_maps_leave_a_map_alone() {
    let original = base_map();
    let rw_named = PackageRewrite::new(vec![edits("e", &[], &edit_text(A, 1.0))]);
    // `edits` here is for multiplayer maps: wanted, by class.
    assert!(rw_named.wants("maps\\anything.map"));
    let mut campaign = PackageRewrite::new(vec![pkg(
        "only",
        &[],
        &[("edits", "e.yaml", None, "campaign")],
        &[("e.yaml", edit_text(A, 1.0).as_bytes())],
    )]);
    assert!(campaign.rebuild("tiny", &original).unwrap().is_none());
    let named = PackageRewrite::new(vec![pkg(
        "named",
        &[],
        &[("edits", "e.yaml", None, "[\"lockout\"]")],
        &[("e.yaml", edit_text(A, 1.0).as_bytes())],
    )]);
    assert!(named.wants("maps\\Lockout.map"));
    assert!(!named.wants(PATH));
}

#[test]
fn a_failed_build_is_served_unchanged_and_recorded() {
    let original = base_map();
    let bad = edits("e", &[], &edit_text("invented\\dmg\\missing", 1.0));
    let mut rw = PackageRewrite::new(vec![bad]);
    assert!(rw.rewrite(PATH, &original).is_none());
    let rec = rw.served().lock().unwrap()["tiny"].clone();
    assert!(rec.error.is_some());
}

fn write_package(root: &Path, l: &Loaded) -> PathBuf {
    let dir = root.join(l.package.name());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("package.toml"), l.package.manifest.to_toml()).unwrap();
    for (n, b) in &l.files {
        std::fs::write(dir.join(n), b).unwrap();
    }
    dir
}

#[test]
fn the_game_set_reads_baked_then_the_data_folder_and_set_toml() {
    let data = temp("set");
    let root = data.join("packages");
    let core = pkg(
        "core",
        &[],
        &[("edits", "e.yaml", None, "all")],
        &[("e.yaml", b"[]\n")],
    );
    write_package(&root, &adds_c("mine"));
    write_package(&root, &edits("needs-ghost", &["ghost"], "[]\n"));
    // A local package with a baked one's id is refused, by name.
    write_package(&root, &core);
    std::fs::create_dir_all(root.join("cache")).unwrap();
    std::fs::write(root.join("set.toml"), "order = [\"mine\"]\n").unwrap();
    let set = GameSet::read(vec![core.clone()], &data, &all_on);
    let on: Vec<&str> = set.on.iter().map(|e| e.loaded.package.name()).collect();
    assert_eq!(on, ["mine", "core"]);
    assert_eq!(set.on[0].origin, Origin::Local(root.join("mine")));
    assert_eq!(set.on[1].origin, Origin::Baked);
    assert!(
        set.problems
            .iter()
            .any(|p| p.contains("\"core\" is already built in")),
        "{:?}",
        set.problems
    );
    let ghost = set.listed.iter().find(|l| l.id == "needs-ghost").unwrap();
    assert!(!ghost.on);
    assert!(ghost.why.as_deref().unwrap().contains("\"ghost\""));
    let core_row = set.listed.iter().find(|l| l.id == "core").unwrap();
    assert!(core_row.always_on && core_row.signed);
    let mine = set.listed.iter().find(|l| l.id == "mine").unwrap();
    assert!(!mine.signed);
    assert_eq!(mine.items.get("tag"), Some(&1));
    assert_eq!(mine.scopes, ["sim"]);
    // `mine` adds a tag (sim) and is not signed: local games only.
    let why = set.network_refusal().unwrap();
    assert!(why.contains("\"mine\""), "{why}");

    // A set file that turns core off is not used, and says so.
    std::fs::write(root.join("set.toml"), "off = [\"core\", \"mine\"]\n").unwrap();
    let set = GameSet::read(vec![core], &data, &all_on);
    assert!(set.on.iter().any(|e| e.loaded.package.name() == "mine"));
    assert!(
        set.problems
            .iter()
            .any(|p| p.contains(&SetError::AlwaysOn("core".into()).to_string())),
        "{:?}",
        set.problems
    );
    let _ = std::fs::remove_dir_all(&data);
}

#[test]
fn an_always_on_package_ignores_its_features() {
    let core = with_features(adds_c("core"), &["pogo"]);
    let set = GameSet::assemble(vec![core], Vec::new(), Ok(SetFile::default()), &|_| false);
    assert_eq!(set.on.len(), 1);
    assert!(set.network_refusal().is_none());
}

#[test]
fn baked_packages_read_from_their_files() {
    let files: &[(&str, &[u8])] = &[
        (
            "package.toml",
            b"[package]\nformat = 1\nid = \"b\"\nname = \"B\"\n[[item]]\nkind = \"tag\"\nsource = \"dmg_c.jpt!.yaml\"\ntag = 'invented\\dmg\\c'\n",
        ),
        ("dmg_c.jpt!.yaml", b"fields: {}\n"),
    ];
    let broken: &[(&str, &[u8])] = &[("package.toml", b"[package]\n")];
    let (loaded, problems) = baked(&[("b", files), ("broken", broken)]);
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].package.name(), "b");
    assert_eq!(loaded[0].files.len(), 1);
    assert_eq!(problems.len(), 1);
    assert!(problems[0].starts_with("content/broken"), "{problems:?}");
}

#[test]
fn the_list_has_each_package_and_each_map_read() {
    let original = base_map();
    let set = GameSet::assemble(
        vec![adds_c("c"), with_features(adds("d", D), &["off-one"])],
        Vec::new(),
        Ok(SetFile::default()),
        &|f| f != "off-one",
    );
    let mut rw = PackageRewrite::new(set.loaded());
    rw.rebuild("tiny", &original).unwrap().unwrap();
    let v = set.list(&rw.served());
    let packages = v["packages"].as_array().unwrap();
    assert_eq!(packages.len(), 2);
    assert_eq!(packages[0]["id"], "c");
    assert_eq!(packages[0]["on"], true);
    assert_eq!(packages[0]["origin"], "baked");
    assert_eq!(packages[0]["items"]["tag"], 1);
    assert_eq!(packages[1]["id"], "d");
    assert_eq!(packages[1]["on"], false);
    assert_eq!(packages[1]["why"], "feature \"off-one\" is off");
    assert!(v["content_hash"].is_string());
    assert!(v["local_only"].is_null());
    assert_eq!(v["maps"]["tiny"]["packages"], serde_json::json!(["c"]));
    assert_eq!(v["maps"]["tiny"]["builds"], 1);
}

#[test]
fn a_package_of_huds_leaves_every_map_alone() {
    let original = base_map();
    let huds = pkg(
        "h",
        &[],
        &[("hud", "i.hud.yaml", None, "all")],
        &[(
            "i.hud.yaml",
            b"name: invented\nelements:\n  - {id: i.line, kind: prompt, longest: Hello}\n",
        )],
    );
    let mut rw = PackageRewrite::new(vec![huds]);
    assert!(rw.is_empty());
    assert!(rw.rebuild("tiny", &original).unwrap().is_none());
}
