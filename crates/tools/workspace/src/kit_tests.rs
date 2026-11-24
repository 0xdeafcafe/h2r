//! PK12: the kits and Firefight's wave sets through the package model.
//! Each kit's output through the set equals what its own caller made, on
//! invented data.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use content::package::{Item, Kind, Maps};
use ::game::bot_mark::GearRewrite;
use ::game::campaign_cards::{Card, CardRewrite, MAINMENU, TEMPLATE, Table, tag_name};
use map::build::{Block, RawRef, TagPayload, TagSet};
use map::{CacheType, FourCC, TagId};
use tags::rgba_bitmap;
use vfs::Rewrite as _;

use super::*;
use crate::kits::{Kit, kit_of};

fn package(id: &str) -> Loaded {
    Loaded::from_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../content").join(id))
        .unwrap()
}

fn item(kind: Kind, source: Option<&str>) -> Item {
    Item {
        kind,
        source: source.map(str::to_owned),
        inputs: vec![],
        tag: None,
        maps: Maps::default(),
    }
}

fn check_text(item: &Item, text: &[u8]) -> Result<(), Failure> {
    let files = BTreeMap::new();
    let job = Job {
        item,
        source: text,
        files: &files,
    };
    (builder(item.kind).unwrap().check)(&job)
}

// ---- the kit items ----

#[test]
fn our_kit_packages_check_clean_and_carry_their_kit() {
    for (id, kit) in [("core", Kit::Glyphs), ("halo1", Kit::CampaignCards)] {
        let loaded = package(id);
        assert_eq!(check_package(&loaded), vec![], "{id}");
        // A kit is no sim item (core also carries the retail scripts).
        assert!(
            loaded
                .package
                .manifest
                .items
                .iter()
                .filter(|i| i.kind == Kind::Kit)
                .all(|i| i.kind.scope() != content::package::ItemScope::Sim),
            "{id}"
        );
        assert!(PackageRewrite::new(vec![loaded]).has_kit(kit), "{id}");
    }
}

#[test]
fn a_kit_is_named_by_its_settings_file() {
    let of = |s: &str| kit_of(&item(Kind::Kit, Some(s)));
    assert_eq!(of("kits/glyphs.kit.yaml"), Ok(Kit::Glyphs));
    assert_eq!(of("campaign-cards.kit.yaml"), Ok(Kit::CampaignCards));
    assert!(of("kits/glyphs.yaml").unwrap_err().contains(".kit.yaml"));
    assert!(of("kits/fonts.kit.yaml").unwrap_err().contains("not a kit"));
    assert!(kit_of(&item(Kind::Kit, None)).unwrap_err().contains("settings file"));
    assert!(kit_of(&item(Kind::Tag, Some("kits/glyphs.kit.yaml"))).is_err());
}

#[test]
fn a_kit_takes_no_setting_it_does_not_have_and_adds_no_tag() {
    let glyphs = item(Kind::Kit, Some("kits/glyphs.kit.yaml"));
    assert_eq!(check_text(&glyphs, b"# none\n{}\n"), Ok(()));
    assert_eq!(check_text(&glyphs, b"# only a comment\n"), Ok(()));
    let err = check_text(&glyphs, b"gear: true\n").unwrap_err();
    assert!(err.why.contains("glyphs settings"), "{}", err.why);
    let mut tagged = glyphs.clone();
    tagged.tag = Some("ui\\x".into());
    assert!(check_text(&tagged, b"{}").unwrap_err().why.contains("adds no tag"));
    assert!(!changes_map(&glyphs));
}

// ---- glyphs ----

/// A font of one blank record every unit maps to.
fn blank_font() -> Vec<u8> {
    let mut data = vec![0; font::RECORDS_OFFSET + font::RECORD_LEN];
    let put = |data: &mut Vec<u8>, at: usize, n: u32| {
        data[at..at + 4].copy_from_slice(&n.to_le_bytes())
    };
    put(&mut data, font::HEADER_OFFSET, font::MAGIC);
    put(&mut data, font::HEADER_OFFSET + 4, 18);
    put(&mut data, font::HEADER_OFFSET + 0xc, 1);
    data
}

/// Records what the icon sets were handed.
struct Icons(Arc<Mutex<Vec<(String, Vec<u8>)>>>);

impl vfs::Rewrite for Icons {
    fn wants(&self, path: &str) -> bool {
        path.to_ascii_lowercase().starts_with("maps\\fonts\\")
    }
    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        self.0.lock().unwrap().push((path.into(), original.to_vec()));
        None
    }
}

const TABLE: &str = "maps\\fonts\\font_table.txt";

#[test]
fn the_glyphs_kit_gives_every_font_what_the_gear_rewrite_gave_it() {
    let seen: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Arc::default();
    let served = Arc::clone(&seen);
    let icons_seen: Arc<Mutex<Vec<(String, Vec<u8>)>>> = Arc::default();
    let mut r = PackageRewrite::new(vec![package("core")])
        .with_kits(KitInputs {
            cards: None,
            icons: Some(Box::new(Icons(Arc::clone(&icons_seen)))),
        })
        .observing_fonts(move |p, b| served.lock().unwrap().push((p.into(), b.to_vec())));
    assert!(!r.is_empty());
    assert!(r.wants("maps\\fonts\\conduit-16"));
    assert!(r.wants("MAPS\\FONTS\\Arial-13"));
    assert!(r.wants(TABLE));
    assert!(!r.wants("maps\\fonts\\notes.txt"));
    assert!(!r.wants("maps\\mainmenu.map"));

    let blank = blank_font();
    let out = r.rewrite("maps\\fonts\\blank", &blank).unwrap();
    // The one caller made this.
    assert_eq!(Some(out.clone()), GearRewrite.rewrite("maps\\fonts\\blank", &blank));
    // The table is served as it is and seen; a font that has the gear is left.
    assert_eq!(r.rewrite(TABLE, b"blank\r\n"), None);
    assert_eq!(r.rewrite("maps\\fonts\\again", &out), None);
    // The icon sets saw each font as the disc has it, before the gear, and
    // not the table.
    assert_eq!(
        *icons_seen.lock().unwrap(),
        vec![
            ("maps\\fonts\\blank".to_owned(), blank.clone()),
            ("maps\\fonts\\again".to_owned(), out.clone()),
        ]
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            ("maps\\fonts\\blank".to_owned(), out.clone()),
            (TABLE.to_owned(), b"blank\r\n".to_vec()),
            ("maps\\fonts\\again".to_owned(), out),
        ]
    );
}

#[test]
fn without_the_kit_no_font_is_touched() {
    let r = PackageRewrite::new(vec![]);
    assert!(r.is_empty());
    assert!(!r.wants("maps\\fonts\\conduit-16"));
    assert!(!r.wants(TABLE));
    // A set whose packages are other kits' leaves fonts alone.
    let halo1 = PackageRewrite::new(vec![package("halo1")]);
    assert!(!halo1.wants("maps\\fonts\\conduit-16"));
}

// ---- campaign-cards ----

/// An invented 8x6 picture in a frame shaped like retail's.
fn framed_template() -> Vec<u8> {
    let (w, h) = (8i32, 6i32);
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let past = x + y - (w - 1 + h - 1 - 2);
            let p = if past > 0 {
                [0, 0, 0, 0]
            } else if past == 0 || x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                [154, 171, 189, 255]
            } else {
                [x as u8, y as u8, 0x5a, 191]
            };
            px.extend_from_slice(&p);
        }
    }
    px
}

fn add_bitmap(set: &mut TagSet, name: &str, raw: RawRef, size: u32) -> TagId {
    let mut image = Block::zeroed(rgba_bitmap::IMAGE_LEN);
    image
        .put_u32(0, FourCC(*b"bitm").as_u32())
        .put_u16(4, 8)
        .put_u16(6, 6)
        .put_u16(0xc, rgba_bitmap::FORMAT_ARGB8888)
        .put_u16(0xe, 0x290)
        .put_u32(0x34, size)
        .raw(0x1c, raw);
    image.bytes[8] = 1;
    let mut header = Block::zeroed(rgba_bitmap::HEADER_LEN);
    header
        .put_u16(0, 4)
        .block(0x3c, 0, Block::zeroed(0))
        .block(0x44, 1, image);
    set.add_tag(FourCC(*b"bitm"), name, TagPayload::Built(header))
        .unwrap()
}

/// A map shaped like mainmenu where it matters: [`TEMPLATE`] and one more
/// picture.
fn mainmenu() -> Vec<u8> {
    let template = framed_template();
    let mut set = TagSet::new(CacheType::MainMenu, "mainmenu", "ui\\mainmenu");
    for g in [b"matg", b"scnr", b"bitm"] {
        set.add_group(FourCC(*g), None, None);
    }
    set.globals = set
        .add_tag(
            FourCC(*b"matg"),
            "globals",
            TagPayload::Built(Block::zeroed(0x284)),
        )
        .unwrap();
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            "ui\\mainmenu",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    for (name, fill) in [
        (TEMPLATE, 0x11),
        ("ui\\code_global_bitmaps\\campaign1", 0x22),
    ] {
        let pixels = if name == TEMPLATE {
            rgba_bitmap::argb8888_linear(8, 6, &template).unwrap()
        } else {
            vec![fill; 384]
        };
        let size = pixels.len() as u32;
        let raw = set.add_raw(pixels, 0x200);
        add_bitmap(&mut set, name, RawRef::Local(raw), size);
    }
    set.build().unwrap().0
}

fn card(map: &str, w: u32, h: u32) -> Card {
    Card {
        map: map.into(),
        name: tag_name("halo1", map),
        width: w,
        height: h,
        rgba: (0..w * h)
            .flat_map(|i| [i as u8, 0x40, 0x80, 0xff])
            .collect(),
    }
}

#[test]
fn the_campaign_cards_kit_serves_mainmenu_as_the_card_rewrite_did() {
    let original = mainmenu();
    let cards = vec![card("a10", 3, 2), card("b30", 5, 5)];

    let table = Table::default();
    let mut r = PackageRewrite::new(vec![package("halo1")]).with_kits(KitInputs {
        cards: CardRewrite::new(cards.clone(), Arc::clone(&table)),
        icons: None,
    });
    assert!(!r.is_empty());
    assert!(r.wants(MAINMENU));
    assert!(r.wants("MAPS\\MAINMENU.MAP"));
    assert!(!r.wants("maps\\lockout.map"));
    let through = r.rewrite(MAINMENU, &original).expect("the cards are added");

    let own_table = Table::default();
    let own = CardRewrite::new(cards, Arc::clone(&own_table))
        .unwrap()
        .rewrite(MAINMENU, &original)
        .unwrap();
    assert_eq!(through, own);
    assert_eq!(*table.lock().unwrap(), *own_table.lock().unwrap());
    assert_eq!(table.lock().unwrap().len(), 2);
}

#[test]
fn the_cards_need_the_kit_in_the_set_and_cards_to_add() {
    let cards = vec![card("a10", 3, 2)];
    // No package with the kit: the cards are not added.
    let off = PackageRewrite::new(vec![]).with_kits(KitInputs {
        cards: CardRewrite::new(cards, Table::default()),
        icons: None,
    });
    assert!(!off.wants(MAINMENU));
    // The kit with no cards (no campaign): mainmenu is left as it is.
    let mut none = PackageRewrite::new(vec![package("halo1")]);
    assert!(!none.wants(MAINMENU));
    assert_eq!(none.rewrite(MAINMENU, &mainmenu()), None);
}

// ---- Firefight's wave sets ----

#[test]
fn the_modes_package_carries_the_four_wave_sets_the_game_builds_in() {
    let modes = package("modes");
    assert_eq!(check_package(&modes), vec![]);
    let sets = wave_sets(std::slice::from_ref(&modes));
    assert!(sets.iter().all(|(id, _)| id == "modes"));
    let defs: Vec<_> = sets.into_iter().map(|(_, d)| d).collect();
    assert_eq!(defs, firefight::builtin::definitions().to_vec());
    // The director's script builds from each, as a lobby builds it.
    for def in &defs {
        firefight::script::compile(def).unwrap();
    }
    // The items add nothing to a map.
    assert!(modes
        .package
        .manifest
        .items
        .iter()
        .filter(|i| i.kind == Kind::Rules)
        .all(|i| !changes_map(i)));
}

#[test]
fn a_wave_set_that_does_not_read_is_refused() {
    let set = item(Kind::Rules, Some("firefight/mine.firefight.json"));
    let err = check_text(&set, b"{}").unwrap_err();
    assert!(err.why.contains("not a Firefight set"), "{}", err.why);
    let mut def = firefight::builtin::definitions()[0].clone();
    assert_eq!(
        check_text(&set, serde_json::to_string(&def).unwrap().as_bytes()),
        Ok(())
    );
    def.rounds.clear();
    let err = check_text(&set, serde_json::to_string(&def).unwrap().as_bytes()).unwrap_err();
    assert!(err.why.contains("round"), "{}", err.why);
    let script = item(Kind::Rules, Some("rules/slayer.rules"));
    // A script is the modes builders', checked as a script and not as a set.
    let err = check_text(&script, b"").unwrap_err();
    assert!(!err.why.contains("Firefight"), "{}", err.why);
}
