//! Invented level lists: every name, id and path below is made up here.
use std::io::Cursor;

use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC};

use super::*;

/// Invented tag memory: `(address, bytes)` regions.
struct Mem(Vec<(u32, Vec<u8>)>);

impl Mem {
    fn read(&self, addr: u32, size: u32) -> Option<Vec<u8>> {
        self.0.iter().find_map(|(base, b)| {
            let off = addr.checked_sub(*base)? as usize;
            b.get(off..off + size as usize).map(<[u8]>::to_vec)
        })
    }
}

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_wide(b: &mut [u8], at: usize, s: &str) {
    for (i, u) in s.encode_utf16().enumerate() {
        b[at + i * 2..at + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
    }
}

fn slot(id: u32, name: &str, desc: &str, path: &str, sort: i32, picture: u32) -> Vec<u8> {
    let mut e = vec![0u8; MP_LEVEL_SIZE];
    put(&mut e, 0, id);
    put(&mut e, MP_PICTURE - 4, u32::from_le_bytes(*b"mtib"));
    put(&mut e, MP_PICTURE, picture);
    put_wide(&mut e, MP_NAME, name);
    // Another language's name and description must not leak into English.
    put_wide(&mut e, MP_NAME + NAME_UNITS * 2, "Autre");
    put_wide(&mut e, MP_DESC, desc);
    put_wide(&mut e, MP_DESC + DESC_UNITS * 2, "Autre chose.");
    e[MP_PATH..MP_PATH + path.len()].copy_from_slice(path.as_bytes());
    put(&mut e, MP_SORT, sort as u32);
    e
}

fn start_level(campaign: u32, id: u32, path: &str) -> Vec<u8> {
    let mut e = vec![0u8; START_LEVEL_SIZE];
    put(&mut e, 0, campaign);
    put(&mut e, 4, id);
    e[8..8 + path.len()].copy_from_slice(path.as_bytes());
    e
}

fn select_level(campaign: u32, id: u32, picture: u32, name: &str, desc: &str) -> Vec<u8> {
    let mut e = vec![0u8; CAMPAIGN_LEVEL_SIZE];
    put(&mut e, 0, campaign);
    put(&mut e, 4, id);
    put(&mut e, 8, u32::from_le_bytes(*b"mtib"));
    put(&mut e, CAMPAIGN_PICTURE, picture);
    put_wide(&mut e, CAMPAIGN_NAME, name);
    put_wide(&mut e, CAMPAIGN_NAME + NAME_UNITS * 2, "Autre");
    put_wide(&mut e, CAMPAIGN_DESC, desc);
    e
}

/// Globals with both lists: `(multiplayer slots, start levels, level
/// select)`.
fn globals_with(mp: &[Vec<u8>], start: &[Vec<u8>], select: &[Vec<u8>]) -> (Vec<u8>, Mem) {
    let mut matg = vec![0u8; 0x200];
    put(&mut matg, MATG_START_LEVELS, 1);
    put(&mut matg, MATG_START_LEVELS + 4, 0x8000_4000);
    put(&mut matg, MATG_LEVELS, 1);
    put(&mut matg, MATG_LEVELS + 4, 0x8000_1000);
    let mut start_el = vec![0u8; 8];
    put(&mut start_el, 0, start.len() as u32);
    put(&mut start_el, 4, 0x8000_5000);
    let mut data = vec![0u8; LEVEL_DATA_SIZE];
    put(&mut data, LEVEL_DATA_CAMPAIGN, select.len() as u32);
    put(&mut data, LEVEL_DATA_CAMPAIGN + 4, 0x8001_0000);
    put(&mut data, LEVEL_DATA_MP, mp.len() as u32);
    put(&mut data, LEVEL_DATA_MP + 4, 0x8002_0000);
    (
        matg,
        Mem(vec![
            (0x8000_1000, data),
            (0x8000_4000, start_el),
            (0x8000_5000, start.concat()),
            (0x8001_0000, select.concat()),
            (0x8002_0000, mp.concat()),
        ]),
    )
}

#[test]
fn multiplayer_slots_read_their_english_text_path_sort_and_picture() {
    let (matg, mem) = globals_with(
        &[
            slot(
                0x10,
                "Alpha Yard",
                "A yard.",
                "scenarios\\multi\\alpha\\alpha_yard",
                30,
                0xe001_0002,
            ),
            // A free slot: left out, its picture and all.
            slot(u32::MAX, "", "", "", 0, 0xe001_0003),
            slot(0x2a, "Beta", "", "", -4, u32::MAX),
        ],
        &[],
        &[],
    );
    let got = mp_levels(&matg, &mut |a, s| mem.read(a, s)).unwrap();
    assert_eq!(
        got,
        vec![
            MpLevel {
                id: 0x10,
                name: "Alpha Yard".into(),
                description: "A yard.".into(),
                path: "scenarios\\multi\\alpha\\alpha_yard".into(),
                sort: 30,
                picture: Some(0xe001_0002),
            },
            MpLevel {
                id: 0x2a,
                name: "Beta".into(),
                description: String::new(),
                path: String::new(),
                sort: -4,
                picture: None,
            },
        ]
    );
    assert_eq!(got[0].stem(), Some("alpha_yard"));
    assert_eq!(got[1].stem(), None);
    // One slot alone: short or free is none.
    let one = slot(7, "Gamma", "", "x\\gamma", 1, 1);
    assert_eq!(mp_level(&one).map(|l| l.id), Some(7));
    assert_eq!(mp_level(&one[..MP_LEVEL_SIZE - 1]), None);
}

#[test]
fn campaign_levels_join_the_start_list_and_the_level_select() {
    let (matg, mem) = globals_with(
        &[],
        &[
            start_level(1, 7, "scenarios\\solo\\07_moon\\07_moon"),
            start_level(1, 3, "scenarios\\solo\\03_dock\\03_dock"),
            // Not on the level select: left out.
            start_level(1, 9, "scenarios\\solo\\09_test\\09_test"),
            // Same map id in another campaign: not the same level.
            start_level(2, 3, "scenarios\\solo\\other\\other"),
            // On the level select, but with no path.
            start_level(1, 5, ""),
        ],
        &[
            select_level(1, 3, 0xe00a_0001, "The Dock", "Boats."),
            select_level(1, 7, u32::MAX, "Moonrise", ""),
            select_level(1, 5, 0xe00a_0005, "Nowhere", ""),
        ],
    );
    let got = campaign_levels(&matg, &mut |a, s| mem.read(a, s)).unwrap();
    assert_eq!(
        got,
        vec![
            CampaignLevel {
                campaign: 1,
                map_id: 7,
                name: "Moonrise".into(),
                description: String::new(),
                path: "scenarios\\solo\\07_moon\\07_moon".into(),
                picture: None,
            },
            CampaignLevel {
                campaign: 1,
                map_id: 3,
                name: "The Dock".into(),
                description: "Boats.".into(),
                path: "scenarios\\solo\\03_dock\\03_dock".into(),
                picture: Some(0xe00a_0001),
            },
        ]
    );
    assert_eq!(got[1].stem(), Some("03_dock"));
    // The multiplayer list in the same globals is untouched by them.
    assert!(
        mp_levels(&matg, &mut |a, s| mem.read(a, s))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn level_lists_fail_cleanly() {
    let slots = [slot(1, "A", "", "", 0, 1)];
    // Address outside the map.
    let (mut matg, mem) = globals_with(&slots, &[], &[]);
    put(&mut matg, MATG_LEVELS + 4, 0x9000_0000);
    assert!(matches!(
        mp_levels(&matg, &mut |a, s| mem.read(a, s)),
        Err(Error::Bounds { .. })
    ));
    // Implausible count.
    let (mut matg, mem) = globals_with(&slots, &[], &[]);
    put(&mut matg, MATG_LEVELS, 1_000_000);
    assert!(mp_levels(&matg, &mut |a, s| mem.read(a, s)).is_err());
    // No level data at all; a short tag.
    assert!(mp_levels(&[0u8; 0x200], &mut |a, s| mem.read(a, s)).is_err());
    assert!(mp_levels(&[0u8; 4], &mut |a, s| mem.read(a, s)).is_err());
    // No start list, or no level data: an error, not an empty list.
    let (matg, mem) = globals_with(&[], &[start_level(1, 3, "a\\b")], &[]);
    let mut none = matg.clone();
    put(&mut none, MATG_START_LEVELS, 0);
    assert!(campaign_levels(&none, &mut |a, s| mem.read(a, s)).is_err());
    let mut none = matg;
    put(&mut none, MATG_LEVELS, 0);
    assert!(campaign_levels(&none, &mut |a, s| mem.read(a, s)).is_err());
}

/// The text of a little-endian UTF-16 field, for the built map's entries.
fn wide(at: usize, s: &str, block: &mut Block) {
    put_wide(&mut block.bytes, at, s);
}

/// The lists read from a whole (invented) main menu map, through its
/// globals tag: the pictures are the datums of the bitmaps the entries
/// reference.
#[test]
fn a_main_menu_map_s_lists_name_their_pictures() {
    let mut set = TagSet::new(CacheType::MainMenu, "menu", "invented\\menu");
    for g in [b"matg", b"scnr", b"bitm"] {
        set.add_group(FourCC(*g), None, None);
    }
    let mut pictures = Vec::new();
    for name in ["invented\\level picture", "invented\\map picture"] {
        let id = set
            .add_tag(
                FourCC(*b"bitm"),
                name,
                TagPayload::Built(Block::zeroed(0x4c)),
            )
            .unwrap();
        pictures.push(id);
    }
    let mut mp = Block::zeroed(2 * MP_LEVEL_SIZE);
    mp.put_u32(0, 0x21);
    wide(MP_NAME, "Yard", &mut mp);
    wide(MP_DESC, "A small yard.", &mut mp);
    let path = b"scenarios\\multi\\yard\\yard";
    mp.bytes[MP_PATH..MP_PATH + path.len()].copy_from_slice(path);
    mp.put_u32(MP_SORT, 5);
    mp.tag_ref((MP_PICTURE - 4) as u32, pictures[1]);
    mp.put_u32(MP_LEVEL_SIZE, u32::MAX);
    let mut select = Block::zeroed(CAMPAIGN_LEVEL_SIZE);
    select.put_u32(0, 1).put_u32(4, 2);
    wide(CAMPAIGN_NAME, "The Pier", &mut select);
    wide(CAMPAIGN_DESC, "Wet boots.", &mut select);
    select.tag_ref((CAMPAIGN_PICTURE - 4) as u32, pictures[0]);
    let mut data = Block::zeroed(LEVEL_DATA_SIZE);
    data.block(LEVEL_DATA_CAMPAIGN as u32, 1, select)
        .block(LEVEL_DATA_MP as u32, 2, mp);
    let mut start = Block::zeroed(START_LEVEL_SIZE);
    start.put_u32(0, 1).put_u32(4, 2);
    let path = b"scenarios\\solo\\02_pier\\02_pier";
    start.bytes[8..8 + path.len()].copy_from_slice(path);
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
    let bytes = set.build().unwrap().0;
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();

    let mp = read_mp(&mut map).unwrap();
    assert_eq!(mp.len(), 1, "the free slot is left out");
    assert_eq!(
        (mp[0].id, mp[0].name.as_str(), mp[0].description.as_str()),
        (0x21, "Yard", "A small yard.")
    );
    assert_eq!((mp[0].stem(), mp[0].sort), (Some("yard"), 5));
    assert_eq!(mp[0].picture, Some(pictures[1].0));

    let campaign = read_campaign(&mut map).unwrap();
    assert_eq!(campaign.len(), 1);
    let c = &campaign[0];
    assert_eq!((c.campaign, c.map_id, c.stem()), (1, 2, Some("02_pier")));
    assert_eq!(
        (c.name.as_str(), c.description.as_str()),
        ("The Pier", "Wet boots.")
    );
    assert_eq!(c.picture, Some(pictures[0].0));
}
