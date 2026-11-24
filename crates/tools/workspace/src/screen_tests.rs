//! PK5: compare the package build against applying the retail edits first
//! and then injecting the existing UI plan into the result.

use std::io::Cursor;
use std::path::Path;

use content::Sha256;
use content::package::{Kind, Lock, Package, SetError, SetFile};
use game::ui_kit::{inject, retail_edits};
use map::{CacheType, Map};
use tags::ui::check::check_map;
use vfs::Rewrite as _;

use super::*;

fn ui_package() -> Loaded {
    Loaded::from_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content/ui")).unwrap()
}

fn replace_source(package: &mut Loaded, path: &str, text: String) {
    package.files.insert(path.into(), text.into_bytes());
    let lock = Lock::new(
        package
            .files
            .iter()
            .map(|(p, b)| (p.clone(), Sha256::of(b)))
            .collect(),
    );
    package.package = Package::new(package.package.manifest.clone(), lock).unwrap();
}

#[test]
fn ui_is_an_always_on_client_package() {
    for id in ["core", "ui"] {
        let settings = format!("off = [{id:?}]");
        assert_eq!(
            SetFile::from_toml(&settings),
            Err(SetError::AlwaysOn(id.into()))
        );
    }
    let ui = ui_package();
    assert_eq!(ui.package.name(), "ui");
    assert!(!ui.package.manifest.has_sim_item());
    assert_eq!(check_package(&ui), vec![]);
    let set = GameSet::assemble(vec![ui], vec![], Ok(Default::default()), &|_| false);
    assert!(set.listed[0].on && set.listed[0].always_on);
    let rewrite = PackageRewrite::new(set.loaded());
    for map in inject::MAPS {
        assert!(rewrite.wants(&format!("maps\\{map}.map")));
    }
    assert!(!rewrite.wants(r"maps\lockout.map"));
}

#[test]
fn the_package_builds_the_same_tags_on_every_ui_map() {
    for (stem, class, fixture) in [
        ("mainmenu", CacheType::MainMenu, inject::fixture::menu()),
        (
            "shared",
            CacheType::Shared,
            inject::fixture::interface(CacheType::Shared, "shared", &[200, 222, 223, 240]),
        ),
        (
            "single_player_shared",
            CacheType::SinglePlayerShared,
            inject::fixture::interface(
                CacheType::SinglePlayerShared,
                "single_player_shared",
                &[200, 222, 223, 240],
            ),
        ),
    ] {
        let mut ui = ui_package();
        // The retail edit in the fixture grows a block of the list template.
        // Its address must be resolved after the edit, including in every
        // injected screen that shares the template's text block.
        let edit = r"screen: 'ui\screens\retail_39'
edits:
  - op: duplicate
    block: text.0
    rect: [20, -100, 4, 100]
    as: text.1
  - op: set_string
    block: text.1
    name: example_copy
";
        let path = ui
            .package
            .manifest
            .items
            .iter()
            .find(|item| item.kind == Kind::ScreenEdits)
            .unwrap()
            .source
            .clone()
            .unwrap();
        replace_source(&mut ui, &path, edit.into());
        let files = [(
            "invented".into(),
            retail_edits::parse("invented", edit).unwrap(),
        )];
        let edited =
            retail_edits::apply_all(&fixture, if stem == "mainmenu" { &files } else { &[] })
                .unwrap();
        assert!(edited.failed.is_empty(), "{:?}", edited.failed);
        let expected = inject::tag::add_screens(&edited.map, &inject::plan().unwrap()).unwrap();

        let target = Target {
            name: stem,
            scope: scope_of(class),
        };
        let (actual, report) = build_map(&fixture, std::slice::from_ref(&ui), &target).unwrap();
        assert_eq!(report.added.len(), expected.injected.len(), "{stem}");
        assert_eq!(report.changed.len(), 2, "{stem}");
        assert!(
            actual == expected.map,
            "{stem}: the package map differs from the direct build"
        );
        let mut map = Map::from_reader(Cursor::new(&actual)).unwrap();
        map.validate_checksum().unwrap();
        assert_eq!(check_map(&mut map).failures, Vec::<String>::new());

        let mut rewrite = PackageRewrite::new(vec![ui]);
        let served = rewrite
            .rewrite(&format!("maps\\{stem}.map"), &fixture)
            .unwrap();
        assert!(served == actual, "{stem}: the VFS served different bytes");
    }
}

#[test]
fn the_cache_key_covers_each_screen_source() {
    let ui = ui_package();
    let target = Target {
        name: "mainmenu",
        scope: content::package::MapScope::Mainmenu,
    };
    let hash = Sha256::of(&inject::fixture::menu());
    let key = |package: &Loaded| cache_key(&hash, std::slice::from_ref(package), &target);
    let original = key(&ui);
    let item = ui
        .package
        .manifest
        .items
        .iter()
        .find(|item| item.kind == Kind::Screens)
        .unwrap();
    for path in item.inputs.iter().chain(item.source.iter()) {
        let mut changed = ui.clone();
        let text = std::str::from_utf8(&ui.files[path]).unwrap();
        replace_source(&mut changed, path, format!("{text}\n# edited\n"));
        assert_ne!(key(&changed), original, "{path}");
    }
}
