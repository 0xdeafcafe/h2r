//! The Maps list's cards over invented files: a main menu map built here
//! (its level lists and pictures made up, `map::build`), the test fixture
//! map under invented level names, a package with a picture of its own.
//! No game data.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use editor_core::map_groups::MapGroup;
use map::build::{Block, RawRef, TagPayload, TagSet};
use map::{CacheType, FourCC, TagId};
use scenario_edit::fixture;
use tags::rgba_bitmap::{self, FORMAT_ARGB8888, HEADER_LEN, IMAGE_LEN};
use tags::ui_levels::{
    CAMPAIGN_DESC, CAMPAIGN_LEVEL_SIZE, CAMPAIGN_NAME, CAMPAIGN_PICTURE, LEVEL_DATA_CAMPAIGN,
    LEVEL_DATA_MP, LEVEL_DATA_SIZE, MATG_LEVELS, MATG_START_LEVELS, MP_DESC, MP_LEVEL_SIZE,
    MP_NAME, MP_PATH, MP_PICTURE, MP_SORT, START_LEVEL_SIZE,
};
use tags::ui_levels::{CampaignLevel, MpLevel};

use super::map_cards::{self, Kind, cards_from, halo2_note, read_packages};
use super::map_pack::Picture;
use super::{Action, App};

pub(crate) struct Temp(pub PathBuf);
impl Temp {
    pub(crate) fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("h2-editor-cards-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An invented `w` x `h` picture, every pixel its own colour.
pub(crate) fn pixels(w: u16, h: u16, seed: u8) -> Vec<u8> {
    (0..u32::from(w) * u32::from(h))
        .flat_map(|i| [seed, i as u8, 0xff - i as u8, 0xff])
        .collect()
}

/// A bitmap tag shaped like the menu's level pictures: one linear
/// `ARGB8888` image (`rgba_bitmap`'s layout), its pixels in this map.
fn add_picture(set: &mut TagSet, name: &str, w: u16, h: u16, rgba: &[u8]) -> TagId {
    let lod = rgba_bitmap::argb8888_linear(w, h, rgba).unwrap();
    let size = lod.len() as u32;
    let raw = set.add_raw(lod, 0x200);
    let mut image = Block::zeroed(IMAGE_LEN);
    image
        .put_u32(0, FourCC(*b"bitm").as_u32())
        .put_u16(4, w)
        .put_u16(6, h)
        .put_u16(0xc, FORMAT_ARGB8888)
        .put_u16(0xe, 0x290)
        .put_u32(0x34, size)
        .raw(0x1c, RawRef::Local(raw));
    image.bytes[8] = 1;
    // LOD pointers 1 and 2 and the texture cache handles: none.
    for at in [0x20, 0x24, 0x28, 0x2c, 0x30] {
        image.put_u32(at, u32::MAX);
    }
    let mut header = Block::zeroed(HEADER_LEN);
    header
        .put_u16(0, 4)
        .put_u16(2, 4)
        .put_u16(4, 1)
        .put_u16(0x18, w)
        .put_u16(0x1a, h)
        .block(0x3c, 0, Block::zeroed(0))
        .block(0x44, 1, image);
    set.add_tag(FourCC(*b"bitm"), name, TagPayload::Built(header))
        .unwrap()
}

fn wide(block: &mut Block, at: usize, s: &str) {
    for (i, u) in s.encode_utf16().enumerate() {
        block.bytes[at + i * 2..at + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
    }
}

fn ascii(block: &mut Block, at: usize, s: &str) {
    block.bytes[at..at + s.len()].copy_from_slice(s.as_bytes());
}

/// The pictures of [`menu`]: the campaign level's (4x2), the multiplayer
/// map's (3x3).
pub(crate) fn pier_picture() -> Vec<u8> {
    pixels(4, 2, 0x40)
}
pub(crate) fn yard_picture() -> Vec<u8> {
    pixels(3, 3, 0xc0)
}

/// An invented main menu map: a campaign level `02_pier` ("The Pier") and
/// a multiplayer map `yard` ("Yard") with pictures of their own, and a
/// free multiplayer slot.
pub(crate) fn menu() -> Vec<u8> {
    let mut set = TagSet::new(CacheType::MainMenu, "mainmenu", "invented\\menu");
    for g in [b"matg", b"scnr", b"bitm"] {
        set.add_group(FourCC(*g), None, None);
    }
    let pier = add_picture(&mut set, "invented\\pier", 4, 2, &pier_picture());
    let yard = add_picture(&mut set, "invented\\yard", 3, 3, &yard_picture());

    let mut mp = Block::zeroed(2 * MP_LEVEL_SIZE);
    mp.put_u32(0, 0x21);
    wide(&mut mp, MP_NAME, "Yard");
    wide(&mut mp, MP_DESC, "A small yard, invented for a test.");
    ascii(&mut mp, MP_PATH, "scenarios\\multi\\yard\\yard");
    mp.put_u32(MP_SORT, 5);
    mp.tag_ref((MP_PICTURE - 4) as u32, yard);
    mp.put_u32(MP_LEVEL_SIZE, u32::MAX);
    let mut select = Block::zeroed(CAMPAIGN_LEVEL_SIZE);
    select.put_u32(0, 1).put_u32(4, 2);
    wide(&mut select, CAMPAIGN_NAME, "The Pier");
    wide(
        &mut select,
        CAMPAIGN_DESC,
        "Wet boots, invented for a test.",
    );
    select.tag_ref((CAMPAIGN_PICTURE - 4) as u32, pier);
    let mut data = Block::zeroed(LEVEL_DATA_SIZE);
    data.block(LEVEL_DATA_CAMPAIGN as u32, 1, select)
        .block(LEVEL_DATA_MP as u32, 2, mp);
    let mut start = Block::zeroed(START_LEVEL_SIZE);
    start.put_u32(0, 1).put_u32(4, 2);
    ascii(&mut start, 8, "scenarios\\solo\\02_pier\\02_pier");
    let mut start_data = Block::zeroed(8);
    start_data.block(0, 1, start);
    let mut matg = Block::zeroed(0x284);
    matg.block(MATG_START_LEVELS as u32, 1, start_data)
        .block(MATG_LEVELS as u32, 1, data);
    set.globals = set
        .add_tag(FourCC(*b"matg"), "globals", TagPayload::Built(matg))
        .unwrap();
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            "invented\\menu",
            TagPayload::Built(Block::zeroed(0x3e0)),
        )
        .unwrap();
    set.build().unwrap().0
}

/// A folder of maps like a disc's `maps`: [`menu`] as `mainmenu.map`, the
/// fixture map as `02_pier.map`, `yard.map` and `shared.map`.
pub(crate) fn maps_folder(dir: &Path) -> PathBuf {
    let maps = dir.join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    std::fs::write(maps.join("mainmenu.map"), menu()).unwrap();
    for m in ["02_pier", "yard", "shared"] {
        let mut bytes = fixture::fixture();
        if m == "02_pier" {
            bytes[0x140..0x144].copy_from_slice(&0u32.to_le_bytes());
        }
        std::fs::write(maps.join(format!("{m}.map")), bytes).unwrap();
    }
    maps
}

/// A package under `data`'s `maps` with an image of its own (2x2, PNG)
/// and a description.
pub(crate) fn package_with_picture(data: &Path) -> Vec<u8> {
    let dir = content::store::maps_dir(data).join("yard-edit");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("manifest.toml"),
        "[package]\nformat = 1\nname = \"Yard Edit\"\ndescription = \"Invented.\"\n\
         image = \"preview.png\"\n\n[base]\nmap = \"yard\"\n",
    )
    .unwrap();
    let rgba = pixels(2, 2, 0x99);
    let mut png = Vec::new();
    assets::png::write_rgba(&mut png, 2, 2, &rgba).unwrap();
    std::fs::write(dir.join("preview.png"), png).unwrap();
    rgba
}

/// Opening a folder of maps with a main menu map names and pictures its
/// levels and maps from the menu's lists, read on the side: the campaign
/// level and the multiplayer map get their names, descriptions and
/// pictures (the pixels the menu holds), any case of the file name finds
/// them, and the maps the lists do not name (the shared caches, the menu
/// itself) have none.
#[test]
fn the_main_menu_s_lists_name_and_picture_the_halo_2_maps() {
    let t = Temp::new("halo2");
    let maps = maps_folder(&t.0);
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(maps));
    app.wait_cards();
    assert!(!app.halo2_cards_loading());

    let pier = app
        .halo2_card("02_pier")
        .expect("the campaign level's card");
    assert_eq!(pier.name, "The Pier");
    assert_eq!(pier.description, "Wet boots, invented for a test.");
    assert_eq!(pier.kind, Kind::Campaign);
    let p = pier.picture.as_ref().expect("its picture");
    assert_eq!((p.width, p.height), (4, 2));
    assert_eq!(p.rgba, pier_picture());

    let yard = app.halo2_card("YARD").expect("the multiplayer map's card");
    assert_eq!(yard.name, "Yard");
    assert_eq!(yard.kind, Kind::Multiplayer);
    let p = yard.picture.as_ref().expect("its picture");
    assert_eq!((p.width, p.height, &p.rgba), (3, 3, &yard_picture()));

    for plain in ["shared", "mainmenu"] {
        assert!(app.halo2_card(plain).is_none(), "{plain}");
    }
    assert!(halo2_note("shared").starts_with("Shared by the multiplayer maps"));
    assert!(halo2_note("single_player_shared").starts_with("Shared by the campaign levels"));
    assert!(halo2_note("mainmenu").starts_with("The main menu"));
}

#[test]
fn source_rows_sort_by_group_and_display_name_before_shared_maps() {
    let t = Temp::new("grouped");
    let maps = maps_folder(&t.0);
    // "02_pier" comes before this stem, while "The Pier" comes after it.
    let mut campaign = fixture::fixture();
    campaign[0x140..0x144].copy_from_slice(&0u32.to_le_bytes());
    std::fs::write(maps.join("coast.map"), campaign).unwrap();
    std::fs::write(maps.join("za_arena.map"), fixture::fixture()).unwrap();
    let renamed_menu = menu();
    std::fs::write(maps.join("q_frontend.map"), renamed_menu).unwrap();
    std::fs::write(maps.join("Single_Player_Shared.MAP"), fixture::fixture()).unwrap();
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(maps));
    app.wait_cards();

    let rows: Vec<_> = app
        .halo2_map_entries()
        .into_iter()
        .map(|entry| (entry.stem, entry.label, entry.group))
        .collect();
    use MapGroup::*;
    assert_eq!(
        rows,
        [
            ("yard", "Yard", Multiplayer),
            ("za_arena", "za_arena", Multiplayer),
            ("coast", "coast", Campaign),
            ("02_pier", "The Pier", Campaign),
            ("mainmenu", "mainmenu", SharedAndMenu),
            ("q_frontend", "q_frontend", SharedAndMenu),
            ("shared", "shared", SharedAndMenu),
            ("Single_Player_Shared", "Single_Player_Shared", SharedAndMenu),
        ]
    );
}

/// A folder with no main menu map reads nothing and stays plain.
#[test]
fn a_source_without_a_main_menu_has_no_cards() {
    let t = Temp::new("nomenu");
    let maps = t.0.join("maps");
    std::fs::create_dir_all(&maps).unwrap();
    std::fs::write(maps.join("yard.map"), fixture::fixture()).unwrap();
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(maps));
    assert!(!app.halo2_cards_loading(), "no reader started");
    app.wait_cards();
    assert!(app.halo2_card("yard").is_none());
}

fn mp(id: u32, stem: &str, name: &str, picture: Option<u32>) -> MpLevel {
    MpLevel {
        id,
        name: name.into(),
        description: format!("{name}."),
        path: format!("scenarios\\multi\\{stem}\\{stem}"),
        sort: 0,
        picture,
    }
}

fn level(map_id: u32, stem: &str, name: &str, picture: Option<u32>) -> CampaignLevel {
    CampaignLevel {
        campaign: 1,
        map_id,
        name: name.into(),
        description: String::new(),
        path: format!("scenarios\\solo\\{stem}\\{stem}"),
        picture,
    }
}

/// Cards from the lists: each picture decoded once however many entries
/// name it; a stem in both lists keeps the campaign level's card; an
/// entry with no name or no path is left out; a picture that fails to
/// decode, or decodes to the wrong size, leaves the card without one.
#[test]
fn cards_take_each_picture_once_and_the_first_card_of_a_map() {
    let mut calls = Vec::new();
    let mut decode = |id: u32| {
        calls.push(id);
        match id {
            1 => Some(Picture {
                width: 1,
                height: 1,
                rgba: vec![1, 2, 3, 4],
            }),
            2 => Some(Picture {
                width: 2,
                height: 2,
                rgba: vec![0; 4],
            }),
            _ => None,
        }
    };
    let cards = cards_from(
        &[
            level(10, "dock", "Dock", Some(1)),
            level(11, "", "No Path", Some(1)),
        ],
        &[
            mp(20, "dock", "Dock Again", Some(2)),
            mp(21, "ring", "Ring", Some(1)),
            mp(22, "pit", "", Some(1)),
            mp(23, "hole", "Hole", Some(3)),
            mp(24, "bent", "Bent", Some(2)),
        ],
        &mut decode,
    );
    let mut asked = calls.clone();
    asked.sort();
    assert_eq!(asked, [1, 2, 3], "each picture decoded once");
    assert_eq!(cards.len(), 4);
    let dock = cards.get("dock").unwrap();
    assert_eq!((dock.name.as_str(), dock.kind), ("Dock", Kind::Campaign));
    let ring = cards.get("Ring").unwrap();
    assert_eq!(ring.description, "Ring.");
    assert!(Arc::ptr_eq(
        dock.picture.as_ref().unwrap(),
        ring.picture.as_ref().unwrap()
    ));
    assert!(cards.get("hole").unwrap().picture.is_none(), "no picture");
    assert!(cards.get("bent").unwrap().picture.is_none(), "wrong size");
    assert!(cards.get("pit").is_none(), "no name");
    assert!(map_cards::picture(0, 1, vec![]).is_none());
}

/// A package's `[package] image` is its picture: read from inside the
/// package once the packages are listed, keyed by its directory. A path
/// out of the package is not followed.
#[test]
fn a_package_s_image_is_its_picture() {
    let t = Temp::new("package");
    let data = t.0.join("data");
    let rgba = package_with_picture(&data);
    let mut app = App::new();
    app.set_data_root(data.clone());
    app.wait_cards();
    let p = app
        .package_picture("yard-edit")
        .expect("the package's picture");
    assert_eq!((p.width, p.height, &p.rgba), (2, 2, &rgba));
    let e = app
        .packages()
        .iter()
        .find(|e| e.dir == "yard-edit")
        .unwrap();
    assert_eq!(e.description.as_deref(), Some("Invented."));
    assert_eq!(e.image.as_deref(), Some("preview.png"));

    let maps = content::store::maps_dir(&data);
    let out = read_packages(
        &maps,
        &[
            ("yard-edit".into(), "../yard-edit/preview.png".into()),
            ("yard-edit".into(), "/etc/hosts".into()),
            ("yard-edit".into(), "missing.png".into()),
        ],
    );
    assert!(out.is_empty());
}
