//! The kit goes into a map, the same way every time, and comes back from
//! the cache; a map without the room is refused by name. The `#[ignore]`
//! test puts it into every multiplayer map of the disc (`$H2_DISC`).

use std::io::Cursor;
use std::path::PathBuf;

use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC, Map, TagId};
use vfs::Rewrite;

use crate::inject::{self, Given, add_pogo, add_pogo_with};
use crate::placed::{self, Placed};
use crate::rewrite::{self, PogoRewrite};
use crate::tests::{check_chain, disc_multiplayer_maps, fixture, fixture_class};
use crate::{KIT_VERSION, Style};

/// The shared tags and five for each look.
const KIT_TAGS: u32 = 2 + 5 * 3;

fn given() -> Given {
    Given {
        class: Some(fixture_class()),
        tag_memory: None,
    }
}

/// A scratch directory of this test's own, empty.
fn scratch(what: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pogo-kit-{what}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_kit_goes_into_a_map_and_reads_back() {
    let original = fixture(0);
    let added = add_pogo_with(&original, "court", &given()).unwrap();
    let map = Map::from_reader(Cursor::new(added.bytes.as_slice())).unwrap();
    assert_eq!(map.header().cache_type, CacheType::Multiplayer);
    let marker = format!("pogo_kit_v{KIT_VERSION}");
    assert_eq!(inject::marker_string(), marker);
    assert!(map.strings().iter().flatten().any(|s| *s == marker));
    // Every tag the map had is still there, by name and datum.
    let before = Map::from_reader(Cursor::new(original.as_slice())).unwrap();
    for t in before.tags() {
        let now = map.tag(t.id).unwrap();
        assert_eq!((now.group, &now.name), (t.group, &t.name));
    }
    assert_eq!(map.tags().len(), before.tags().len() + KIT_TAGS as usize);
    assert_eq!(added.costs.tags, KIT_TAGS);
    assert!(added.costs.tag_growth > 0 && added.costs.tag_growth <= added.costs.available);
    assert_eq!(
        added.costs.file_growth,
        added.bytes.len() as i64 - original.len() as i64
    );
    for style in Style::ALL {
        check_chain(
            &added.bytes,
            TagId(added.placed.weapons[style.index()]),
            style,
        );
    }
}

/// The placed datums are the `weap` tags' in the built map, each its own.
#[test]
fn the_placed_datums_are_the_weapons() {
    let added = add_pogo_with(&fixture(0), "court", &given()).unwrap();
    let map = Map::from_reader(Cursor::new(added.bytes.as_slice())).unwrap();
    for style in Style::ALL {
        let datum = added.placed.weapons[style.index()];
        let tag = map.tag(TagId(datum)).unwrap();
        assert_eq!(tag.id, TagId(datum));
        assert_eq!(TagId(datum).index(), (datum & 0xffff) as usize);
        assert_eq!(tag.group, FourCC(*b"weap"));
        assert_eq!(tag.name.as_deref(), Some(style.tag_path().as_str()));
    }
    let [a, b, c] = added.placed.weapons;
    assert!(a != b && b != c && a != c);
}

#[test]
fn the_same_map_builds_the_same_bytes() {
    let original = fixture(0);
    let a = add_pogo_with(&original, "court", &given()).unwrap();
    let b = add_pogo_with(&original, "court", &given()).unwrap();
    assert!(a.bytes == b.bytes, "two builds differ");
    assert_eq!((a.placed, a.costs), (b.placed, b.costs));
    // Another map, another build, the kit the same size in it.
    let c = add_pogo_with(&fixture(0x400), "court", &given()).unwrap();
    assert!(c.bytes != a.bytes);
    assert_eq!(c.costs.tags, a.costs.tags);
}

#[test]
fn a_map_without_the_room_is_refused_by_name() {
    let original = fixture(0);
    let had = Map::from_reader(Cursor::new(original.as_slice()))
        .unwrap()
        .header()
        .meta_size;
    let tight = Given {
        tag_memory: Some(had + 0x100),
        ..given()
    };
    let e = format!(
        "{:#}",
        add_pogo_with(&original, "court", &tight).unwrap_err()
    );
    assert!(
        e.contains("court") && e.contains("no room for the pogo kit"),
        "{e}"
    );
    assert!(e.contains("256 B free"), "{e}");
    // With exactly what it takes, it fits.
    let took = add_pogo_with(&original, "court", &given())
        .unwrap()
        .costs
        .tag_growth;
    let exact = Given {
        tag_memory: Some(had + took),
        ..given()
    };
    assert!(add_pogo_with(&original, "court", &exact).is_ok());
}

#[test]
fn a_map_that_already_has_the_kit_or_is_not_multiplayer_is_refused() {
    let once = add_pogo_with(&fixture(0), "court", &given()).unwrap();
    let e = format!(
        "{:#}",
        add_pogo_with(&once.bytes, "court", &given()).unwrap_err()
    );
    assert!(e.contains("already has"), "{e}");

    let mut set = TagSet::new(CacheType::Campaign, "level", r"invented\level");
    set.add_group(FourCC(*b"scnr"), None, None);
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            r"invented\level",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    let level = set.build().unwrap().0;
    let e = format!("{:#}", add_pogo(&level, "level").unwrap_err());
    assert!(e.contains("multiplayer"), "{e}");
    // The rewrite serves it as it is, and says nothing of it.
    let mut r = PogoRewrite::new().with_given(given());
    assert!(r.wants(r"maps\level.map"));
    assert_eq!(r.rewrite(r"maps\level.map", &level), None);
    assert_eq!(placed::get("level"), None);
}

#[test]
fn only_level_maps_are_wanted() {
    assert_eq!(
        rewrite::stem_of(r"maps\Lockout.map").as_deref(),
        Some("lockout")
    );
    assert_eq!(
        rewrite::stem_of(r"MAPS\ascension.MAP").as_deref(),
        Some("ascension")
    );
    for path in [
        r"maps\mainmenu.map",
        r"maps\shared.map",
        r"maps\single_player_shared.map",
        r"maps\fonts\lockout.map",
        r"maps\.map",
        r"maps\lockout.bin",
        r"lockout.map",
    ] {
        assert_eq!(rewrite::stem_of(path), None, "{path}");
        assert!(!PogoRewrite::new().wants(path), "{path}");
    }
}

#[test]
fn placed_is_kept_by_map() {
    let p = Placed { weapons: [1, 2, 3] };
    assert_eq!(placed::get("placed_test"), None);
    placed::set("Placed_Test", p);
    assert_eq!(placed::get("placed_test"), Some(p));
    assert_eq!(placed::get("placed_other"), None);
    placed::clear("PLACED_TEST");
    assert_eq!(placed::get("placed_test"), None);
}

#[test]
fn a_second_boot_reads_the_map_back_from_the_cache() {
    let dir = scratch("cache");
    let original = fixture(0);
    let mut first = PogoRewrite::new().with_given(given()).with_cache(&dir);
    let built = first.rebuild("cached", &original).unwrap();
    assert!(!built.from_cache);
    let file = dir
        .join("cache")
        .join(rewrite::CACHE_DIR)
        .join("cached.map");
    assert_eq!(built.file.as_deref(), Some(file.as_path()));
    assert_eq!(std::fs::read(&file).unwrap(), built.added.bytes);
    assert!(file.with_extension("json").is_file());
    assert_eq!(placed::get("cached"), Some(built.added.placed));

    // Another boot: the same bytes, the same weapons, nothing built.
    placed::clear("cached");
    let mut second = PogoRewrite::new().with_given(given()).with_cache(&dir);
    let again = second.rebuild("cached", &original).unwrap();
    assert!(again.from_cache);
    assert!(again.added == built.added);
    assert_eq!(placed::get("cached"), Some(built.added.placed));
    // Through the VFS's own call, too.
    assert_eq!(
        second.rewrite(r"maps\cached.map", &original),
        Some(built.added.bytes.clone())
    );

    // A changed map is another key; so is another name.
    let other = fixture(0x400);
    assert_ne!(
        second.key("cached", &other),
        second.key("cached", &original)
    );
    assert_ne!(
        second.key("other", &original),
        second.key("cached", &original)
    );
    assert!(!second.rebuild("cached", &other).unwrap().from_cache);
    // A cache file changed on disk is not served.
    let mut third = PogoRewrite::new().with_given(given()).with_cache(&dir);
    assert!(third.rebuild("cached", &other).unwrap().from_cache);
    std::fs::write(&file, b"not the map").unwrap();
    assert!(!third.rebuild("cached", &other).unwrap().from_cache);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_refused_map_is_served_unchanged_and_not_placed() {
    let dir = scratch("refused");
    let original = fixture(0);
    placed::set("refused", Placed { weapons: [7, 8, 9] });
    let tight = Given {
        tag_memory: Some(0),
        ..given()
    };
    let mut r = PogoRewrite::new().with_given(tight).with_cache(&dir);
    assert_eq!(r.rewrite(r"maps\refused.map", &original), None);
    assert_eq!(placed::get("refused"), None);
    assert!(
        !dir.join("cache")
            .join(rewrite::CACHE_DIR)
            .join("refused.map")
            .exists()
    );

    // One too large for the VFS to hand over is cleared the same way.
    placed::set("refused", Placed { weapons: [7, 8, 9] });
    r.skipped(r"maps\refused.map");
    assert_eq!(placed::get("refused"), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The kit goes into every multiplayer map of the disc; prints what it
/// takes of each map's tag memory.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn every_multiplayer_map_has_the_room() {
    let mut tightest: Option<(String, u32)> = None;
    for (stem, bytes) in disc_multiplayer_maps() {
        let added = add_pogo(&bytes, &stem).unwrap_or_else(|e| panic!("{stem}: {e:#}"));
        let spare = added.costs.available - added.costs.tag_growth;
        println!("{stem}: {}; {spare} B left", added.costs);
        if tightest.as_ref().is_none_or(|(_, s)| spare < *s) {
            tightest = Some((stem.clone(), spare));
        }
        for style in Style::ALL {
            check_chain(
                &added.bytes,
                TagId(added.placed.weapons[style.index()]),
                style,
            );
        }
        let again = add_pogo(&bytes, &stem).unwrap();
        assert!(again.bytes == added.bytes, "{stem}: two builds differ");
    }
    let (stem, spare) = tightest.expect("a multiplayer map");
    println!("the tightest: {stem}, {spare} B left");
}
