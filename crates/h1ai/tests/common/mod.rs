//! Shared by the tests: build a converted plan into a small v8 map, read
//! the scenario back through the crate's Halo 2 decoders, and open the
//! user's discs.
#![allow(dead_code)]

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1ai::convert::Plan;
use h1ai::h2::{ScenarioAi, layout};
use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC, Map, TagId};
use tags::geometry::TagView;

/// Builds `plan` into a map holding a scenario and one empty `char` tag per
/// palette entry (named as the retail one), then reads the scenario's AI
/// data back. Returns it with the palette's datums.
pub fn build_and_read(plan: &Plan) -> (ScenarioAi, Vec<TagId>) {
    let mut set = TagSet::new(CacheType::Campaign, "h1ai", "invented\\h1ai");
    set.add_group(FourCC(*b"char"), None, None);
    set.add_group(FourCC(*b"scnr"), None, None);
    let ids: Vec<(String, TagId)> = plan
        .characters
        .iter()
        .map(|c| {
            let id = set
                .add_tag(
                    FourCC(*b"char"),
                    &c.h2_character,
                    TagPayload::Built(Block::zeroed(layout::CHARACTER_SIZE)),
                )
                .unwrap();
            (c.h2_character.to_owned(), id)
        })
        .collect();
    let blocks = plan
        .blocks(|p| ids.iter().find(|(n, _)| n == p).map(|(_, id)| *id))
        .unwrap();
    let mut root = Block::zeroed(layout::SCENARIO_MIN);
    blocks.apply(&mut root).unwrap();
    let scnr = set
        .add_tag(FourCC(*b"scnr"), "invented\\h1ai", TagPayload::Built(root))
        .unwrap();
    set.scenario = scnr;
    let (bytes, _) = set.build().expect("map builds");
    let len = bytes.len() as u64;
    let mut m = Map::from_region(Cursor::new(bytes), 0, len).expect("map reads");
    let t = m.scenario().unwrap().clone();
    let payload = m.read_tag_bytes(t.id).unwrap();
    let view = TagView::new(&payload, t.location.unwrap().address);
    let ai = ScenarioAi::read(&view).expect("scenario AI decodes");
    (ai, ids.into_iter().map(|(_, id)| id).collect())
}

pub fn repo_disc(name: &str, var: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

/// A Halo 1 map off the Halo 1 disc (`$H1_ISO`, else `disc/halo1.iso`).
pub fn h1_map(name: &str) -> h1map::H1Map {
    let path = repo_disc("halo1.iso", "H1_ISO");
    let f = File::open(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let mut iso = xiso::Xiso::open(BufReader::new(f)).unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).unwrap_or_else(|| panic!("no {name}")).clone();
    let bytes = iso.read(&e).unwrap();
    h1map::H1Map::from_reader(Cursor::new(bytes)).unwrap()
}

pub type H2Map = Map<BufReader<File>>;

/// A Halo 2 map off the Halo 2 disc (`$H2_DISC`, else `disc/halo2.iso`).
pub fn h2_map(name: &str) -> H2Map {
    let path = repo_disc("halo2.iso", "H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let p = iso.partition_offset();
    let want = format!("/maps/{name}.map");
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.eq_ignore_ascii_case(&want))
        .unwrap_or_else(|| panic!("no {want}"));
    Map::from_region(
        BufReader::new(File::open(&path).unwrap()),
        p + u64::from(e.sector) * xiso::SECTOR,
        u64::from(e.size),
    )
    .unwrap()
}

/// The retail campaign levels on the Halo 2 disc.
pub const H2_CAMPAIGN: [&str; 15] = [
    "00a_introduction",
    "01a_tutorial",
    "01b_spacestation",
    "03a_oldmombasa",
    "03b_newmombasa",
    "04a_gasgiant",
    "04b_floodlab",
    "05a_deltaapproach",
    "05b_deltatowers",
    "06a_sentinelwalls",
    "06b_floodzone",
    "07a_highcharity",
    "07b_forerunnership",
    "08a_deltacliffs",
    "08b_deltacontrol",
];
