//! Tests on invented maps and discs built in memory. The `#[ignore]`d tests
//! at the end read the owner's disc from `$H1_ISO` (default
//! `disc/halo1.iso` in the main checkout).

use std::io::Cursor;

use crate::disc::{self, Disc};
use crate::testing::*;
use crate::*;

fn open(b: &Built) -> H1Map {
    H1Map::from_reader(Cursor::new(compress(&b.file))).unwrap()
}

#[test]
fn reads_the_header_and_inflates_the_body() {
    let b = build(&Opts::default());
    let m = open(&b);
    let h = m.header();
    assert_eq!(h.version, 5);
    assert_eq!(h.file_length as usize, b.file.len());
    assert_eq!(h.index_offset, 0x800);
    assert_eq!(h.name, "tiny");
    assert_eq!(h.build, "01.10.12.2276");
    assert_eq!(h.cache_type, CacheType::Campaign);
    assert_eq!(h.checksum, 0xcafe_f00d);
    assert_eq!(m.bytes(), &b.file[..]);
}

#[test]
fn reads_the_index_and_derives_the_base() {
    let m = open(&build(&Opts::default()));
    let i = m.index();
    assert_eq!(i.tag_count, 6);
    assert_eq!(i.scenario, TagId(SCNR_ID));
    assert_eq!(i.checksum, 0x1234_5678);
    assert_eq!(i.len, 0x24);
    assert_eq!(i.model_part_count(), 7);
    assert_eq!(i.vertex_data(), 0x8800_0000);
    assert_eq!(i.index_data(), 0x8900_0000);
    assert_eq!(m.metadata_base(), XBOX_METADATA_BASE);
}

#[test]
fn a_pc_style_index_header_closes_at_0x24() {
    let m = open(&build(&Opts {
        pc_index: true,
        ..Default::default()
    }));
    assert_eq!(m.index().len, 0x28);
    assert_eq!(m.index().model_fields.len(), 5);
    assert_eq!(m.metadata_base(), XBOX_METADATA_BASE);
    assert_eq!(m.tags().len(), 6);
}

#[test]
fn lists_tags_with_groups_names_ids_and_data() {
    let b = build(&Opts::default());
    let m = open(&b);
    let names: Vec<(String, &str)> = m
        .tags()
        .iter()
        .map(|t| (t.group.to_string(), t.name.as_str()))
        .collect();
    assert_eq!(names[0], ("scnr".into(), "levels\\test\\tiny\\tiny"));
    assert_eq!(names[5], ("hmt ".into(), "ui\\test\\hud"));
    let shader = &m.tags()[3];
    assert_eq!(shader.parent, Group::new(b"shdr"));
    assert!(shader.grandparent.is_none());
    assert_eq!(shader.id.index(), 3);
    assert_eq!(shader.id.salt(), 0xe174);
    for (t, at) in m.tags().iter().zip(&b.data_at) {
        assert_eq!(t.data.as_ref().map(|r| r.start), *at, "{}", t.name);
    }
    // A tag's data runs to the next tag's data.
    assert_eq!(m.tag_data(shader).unwrap().len(), 0x44);
    assert!(m.tag_data(shader).unwrap().iter().all(|&c| c == 3));
    // External: no data here.
    let bitmap = &m.tags()[4];
    assert!(bitmap.external);
    assert!(matches!(m.tag_data(bitmap), Err(Error::NoTagData)));
    assert_eq!(m.scenario().name, "levels\\test\\tiny\\tiny");
}

#[test]
fn finds_tags_by_id_and_name() {
    let m = open(&build(&Opts::default()));
    assert_eq!(
        m.tag(TagId(SCNR_ID + 2)).unwrap().name,
        "levels\\test\\tiny\\tiny_b"
    );
    // Same index, wrong salt.
    assert!(matches!(m.tag(TagId(0x1111_0002)), Err(Error::TagNotFound)));
    assert!(m.tag(TagId::NONE).is_err());
    let hud = m
        .find(Group::parse("hmt").unwrap(), "UI\\test\\HUD")
        .unwrap();
    assert_eq!(hud.id.index(), 5);
    assert!(m.find(Group::SCENARIO, "ui\\test\\hud").is_none());
}

#[test]
fn counts_tags_by_group() {
    let m = open(&build(&Opts::default()));
    let counts: Vec<(String, usize)> = m
        .group_counts()
        .into_iter()
        .map(|(g, n)| (g.to_string(), n))
        .collect();
    assert_eq!(
        counts,
        [
            ("bitm", 1),
            ("hmt ", 1),
            ("sbsp", 2),
            ("scnr", 1),
            ("senv", 1)
        ]
        .map(|(g, n)| (g.to_string(), n))
    );
}

#[test]
fn resolves_tag_blocks() {
    let m = open(&build(&Opts::default()));
    let scnr = m.tag_data(m.scenario()).unwrap();
    let block = Block::read(scnr, SCNR_STRUCTURE_BSPS).unwrap();
    assert_eq!(block.count, 2);
    assert_eq!(
        m.block(block, SCNR_BSP_LEN).unwrap().len(),
        2 * SCNR_BSP_LEN
    );
    // Empty: no read, whatever the address.
    assert!(
        m.block(
            Block {
                count: 0,
                address: 0
            },
            0x40
        )
        .unwrap()
        .is_empty()
    );
    // Past the tag data, below the base, and a count that overflows.
    assert!(
        m.block(
            Block {
                count: 0x1000,
                address: block.address
            },
            0x40
        )
        .is_err()
    );
    assert!(
        m.block(
            Block {
                count: 1,
                address: 0x1000
            },
            4
        )
        .is_err()
    );
    assert!(
        m.block(
            Block {
                count: u32::MAX,
                address: block.address
            },
            usize::MAX / 2
        )
        .is_err()
    );
}

#[test]
fn lists_the_structure_bsps() {
    let b = build(&Opts::default());
    let m = open(&b);
    let bsps = m.structure_bsps().unwrap();
    assert_eq!(bsps.len(), 2);
    for (i, bsp) in bsps.iter().enumerate() {
        assert_eq!((bsp.file_offset as usize, bsp.size as usize), b.bsps[i]);
        assert_eq!(bsp.address, 0x8100_0000);
        assert_eq!(bsp.tag.group, Group::STRUCTURE_BSP);
        assert_eq!(bsp.tag.id.index() as usize, 1 + i);
        assert_eq!(m.read_cstr(bsp.tag.name_address).unwrap(), bsp.name);
        let bytes = m.bsp_bytes(bsp).unwrap();
        assert!(bytes.iter().all(|&c| c == 0xb0 + i as u8));
    }
    assert_eq!(bsps[1].name, "levels\\test\\tiny\\tiny_b");
}

#[test]
fn from_decompressed_reads_the_same() {
    let b = build(&Opts::default());
    let a = open(&b);
    let d = H1Map::from_decompressed(b.file.clone()).unwrap();
    assert_eq!(a.tags(), d.tags());
    assert_eq!(a.header(), d.header());
    let mut short = b.file.clone();
    short.pop();
    assert!(H1Map::from_decompressed(short).is_err());
}

#[test]
fn a_stream_shorter_or_longer_than_the_header_says_is_refused() {
    let b = build(&Opts::default());
    let good = compress(&b.file);
    for delta in [-1i64, 1] {
        // The header claims one byte less or more than the stream holds.
        let mut z = good.clone();
        put(&mut z, 8, (b.file.len() as i64 + delta) as u32);
        let err = H1Map::from_reader(Cursor::new(z)).unwrap_err();
        assert!(err.to_string().contains("inflates to"), "{err}");
    }
    // A corrupt stream is an I/O error, not a panic.
    let mut z = compress(&b.file);
    let mid = HEADER_LEN + (z.len() - HEADER_LEN) / 2;
    z[mid..].fill(0xff);
    assert!(H1Map::from_reader(Cursor::new(z)).is_err());
}

#[test]
fn bad_headers_are_refused() {
    let b = build(&Opts::default());
    let mut v = b.file.clone();
    put(&mut v, 4, 7); // a PC map's version
    assert!(matches!(
        H1Map::from_decompressed(v),
        Err(Error::Unsupported(_))
    ));
    let mut f = b.file.clone();
    put(&mut f, FOOT_OFFSET, 0);
    assert!(matches!(
        H1Map::from_decompressed(f),
        Err(Error::Invalid(_))
    ));
    let mut t = b.file.clone();
    put(&mut t, 0x14, b.file.len() as u32); // tag data past the end
    assert!(matches!(
        H1Map::from_decompressed(t),
        Err(Error::Bounds { .. })
    ));
    assert!(parse_header(&b.file[..0x7ff]).is_err());
}

#[test]
fn a_tag_whose_id_names_another_index_is_refused() {
    let b = build(&Opts {
        bad_id: Some(SCNR_ID + 4),
        ..Default::default()
    });
    let err = H1Map::from_decompressed(b.file).unwrap_err();
    assert!(err.to_string().contains("tag 1 has ID"), "{err}");
}

#[test]
fn a_missing_tags_fourcc_is_refused() {
    let mut b = build(&Opts::default());
    put(&mut b.file, HEADER_LEN + 0x20, 0);
    assert!(
        H1Map::from_decompressed(b.file)
            .unwrap_err()
            .to_string()
            .contains("'tags'")
    );
}

#[test]
fn groups_parse_and_print() {
    assert_eq!(Group::parse("hmt").unwrap().to_string(), "hmt ");
    assert_eq!(Group::parse("snd!").unwrap(), Group::new(b"snd!"));
    assert!(Group::parse("toolong").is_none());
    assert!(Group::parse("").is_none());
    assert_eq!(Group::NONE.to_string(), "none");
    assert_eq!(Group(0x0102_0304).to_string(), "0x01020304");
    // Stored little-endian: `scnr` is the bytes "rncs".
    assert_eq!(Group::SCENARIO.0.to_le_bytes(), *b"rncs");
    assert_eq!(CacheType::from_raw(2), CacheType::UserInterface);
    assert_eq!(CacheType::from_raw(9).raw(), 9);
}

// ---------------------------------------------------------------------------
// The disc.

const ALL_LEVELS: [&str; 10] = [
    "a10.map", "A30.map", "a50.map", "b30.map", "b40.map", "c10.map", "c20.map", "c40.map",
    "d20.map", "d40.map",
];

/// Each name with 16 bytes of its own.
fn files<'a>(names: &[&'a str]) -> Vec<(&'a str, Vec<u8>)> {
    names
        .iter()
        .enumerate()
        .map(|(i, n)| (*n, vec![i as u8 + 1; 16]))
        .collect()
}

#[test]
fn recognises_the_disc_and_picks_the_campaign_maps() {
    let mut names = ALL_LEVELS.to_vec();
    names.extend(["ui.map", "bloodgulch.map"]);
    let mut iso = xiso::Xiso::open(Cursor::new(disc_image(
        disc::HALO1_TITLE_ID,
        &files(&names),
    )))
    .unwrap();
    let d = Disc::open(&mut iso).unwrap();
    assert_eq!(d.title_id, 0x4d53_0004);
    assert_eq!(d.version, 0x101);
    assert_eq!(d.maps.len(), 12, "readme.txt is not a map");
    assert_eq!(d.map("a30").unwrap().path, "/maps/A30.map");
    assert_eq!(d.map("ui.map").map(disc::map_stem), Some("ui"));
    let picked: Vec<&str> = d
        .campaign_maps()
        .unwrap()
        .into_iter()
        .map(disc::file_name)
        .collect();
    let mut want: Vec<&str> = ALL_LEVELS.to_vec();
    want.push("ui.map");
    assert_eq!(
        picked, want,
        "levels in play order, then ui; no multiplayer maps"
    );
}

#[test]
fn a_disc_missing_a_level_or_another_title_is_refused() {
    let mut iso = xiso::Xiso::open(Cursor::new(disc_image(
        disc::HALO1_TITLE_ID,
        &files(&ALL_LEVELS[1..]),
    )))
    .unwrap();
    let err = Disc::open(&mut iso).unwrap().campaign_maps().unwrap_err();
    assert!(
        err.to_string().contains("missing campaign maps: a10"),
        "{err}"
    );
    let mut iso =
        xiso::Xiso::open(Cursor::new(disc_image(0x4d53_0064, &files(&ALL_LEVELS)))).unwrap();
    let err = Disc::open(&mut iso).unwrap_err();
    assert!(err.to_string().contains("4D530064"), "{err}");
}

#[test]
fn copies_with_a_hash_and_writes_the_manifest() {
    let mut out = Vec::new();
    let (n, sha) = disc::copy_hashed(&mut &b"abc"[..], &mut out).unwrap();
    assert_eq!((n, out.as_slice()), (3, &b"abc"[..]));
    assert_eq!(
        disc::hex(&sha),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    let d = Disc {
        title_id: disc::HALO1_TITLE_ID,
        title_name: String::new(),
        version: 0x101,
        maps: Vec::new(),
    };
    let text = disc::manifest(
        &d,
        &[disc::Copied {
            name: "a10.map".into(),
            size: 3,
            sha256: sha,
        }],
    );
    assert_eq!(
        text,
        "# h2 import halo1: title 4D530004 version 0x101\n# sha256  size  name\n\
         ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  3  a10.map\n"
    );
}

// ---------------------------------------------------------------------------
// The owner's disc. Run with `cargo test -p h1map -- --ignored`.

fn real_disc() -> Option<xiso::Xiso<std::io::BufReader<std::fs::File>>> {
    let path = std::env::var("H1_ISO").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../disc/halo1.iso").to_string()
    });
    let f = std::fs::File::open(&path)
        .map_err(|e| eprintln!("{path}: {e}"))
        .ok()?;
    Some(xiso::Xiso::open(std::io::BufReader::new(f)).expect("an XDVDFS image"))
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn real_disc_every_map_reads() {
    let mut iso = real_disc().expect("set H1_ISO to the Halo 1 disc");
    let d = Disc::open(&mut iso).unwrap();
    assert_eq!(d.title_id, disc::HALO1_TITLE_ID);
    eprintln!(
        "title {:08X} {:?} version {:#x}",
        d.title_id, d.title_name, d.version
    );
    let campaign = d.campaign_maps().unwrap();
    assert!(campaign.len() > CAMPAIGN_LEVELS_LEN, "levels and ui.map");
    for e in d.maps.clone() {
        let bytes = iso.read(&e).unwrap();
        let m = H1Map::from_reader(Cursor::new(&bytes))
            .unwrap_or_else(|err| panic!("{}: {err}", e.path));
        let h = m.header();
        eprintln!(
            "{:<20} v{} len {:#x} stored {:#x} f0c {:#x} index {:#x} meta {:#x} {:?} {} {} crc {:#x} | base {:#x} tags {} idx-len {:#x} model {:x?}",
            disc::file_name(&e),
            h.version,
            h.file_length,
            bytes.len(),
            h.field_0c,
            h.index_offset,
            h.metadata_size,
            h.build,
            h.name,
            h.cache_type,
            h.checksum,
            m.metadata_base(),
            m.tags().len(),
            m.index().len,
            m.index().model_fields
        );
        assert_eq!(m.metadata_base(), XBOX_METADATA_BASE, "{}", e.path);
        assert_eq!(
            h.name.to_ascii_lowercase(),
            disc::map_stem(&e).to_ascii_lowercase()
        );
        assert_eq!(m.scenario().group, Group::SCENARIO);
        for t in m.tags() {
            assert!(
                !t.name.is_empty() && t.name.is_ascii(),
                "{}: tag {} name",
                e.path,
                t.id
            );
        }
        let bsps = m
            .structure_bsps()
            .unwrap_or_else(|err| panic!("{}: {err}", e.path));
        assert!(
            !bsps.is_empty() || h.cache_type == CacheType::UserInterface,
            "{}",
            e.path
        );
        for b in &bsps {
            eprintln!(
                "    bsp {:#x}+{:#x} @ {:#x} {}",
                b.file_offset, b.size, b.address, b.name
            );
            // The BSP regions are outside the tag data.
            let meta = h.index_offset..h.index_offset + h.metadata_size;
            assert!(!meta.contains(&b.file_offset), "{}: {}", e.path, b.name);
        }
    }
}

const CAMPAIGN_LEVELS_LEN: usize = disc::CAMPAIGN_LEVELS.len();

#[test]
fn the_invented_campaign_disc_reads_end_to_end() {
    let mut iso = xiso::Xiso::open(Cursor::new(campaign_disc())).unwrap();
    let d = Disc::open(&mut iso).unwrap();
    let picked: Vec<_> = d.campaign_maps().unwrap().into_iter().cloned().collect();
    assert_eq!(picked.len(), 11);
    for e in &picked {
        let m = H1Map::from_reader(Cursor::new(iso.read(e).unwrap())).unwrap();
        assert_eq!(m.structure_bsps().unwrap().len(), 2);
    }
}

// ---------------------------------------------------------------------------
// Collision BSP and scenario layouts, on invented bytes.

fn room() -> bsp::CollisionBsp {
    let base = 0x8190_0000;
    let (bytes, at) = collision_room(base);
    let space = Space::new(&bytes, base);
    bsp::CollisionBsp::read(&space, &bytes[at..at + bsp::COLLISION_BSP_LEN]).unwrap()
}

#[test]
fn the_invented_collision_bsp_reads_and_validates() {
    let c = room();
    assert_eq!(c.nodes.len(), 6);
    assert_eq!(c.nodes[5].front, bsp::Child::Leaf(0));
    assert_eq!(c.nodes[0].back, bsp::Child::Solid);
    assert_eq!(c.refs[0].node, 0x8000_0000);
    assert_eq!(c.surfaces[0].breakable_surface, -1);
    assert_eq!(c.edges[0].right_surface, u32::MAX);
    assert_eq!(c.validate(), Vec::<String>::new());
    assert_eq!(c.surface_ring(0), Some(vec![0, 1, 2, 3]));
}

#[test]
fn points_classify_as_open_inside_the_room_and_solid_outside() {
    let c = room();
    assert_eq!(c.leaf_at([0.5, 0.5, 0.5]), Some(0));
    assert_eq!(c.leaf_at([0.0, 0.0, 0.0]), Some(0), "on a plane is front");
    for p in [
        [-0.1, 0.5, 0.5],
        [0.5, 1.1, 0.5],
        [0.5, 0.5, -2.0],
        [9.0, 9.0, 9.0],
    ] {
        assert_eq!(c.leaf_at(p), None, "{p:?}");
    }
}

#[test]
fn validation_catches_a_bad_index_and_a_vertex_off_its_plane() {
    let mut c = room();
    c.vertices[2].point[2] = 0.5;
    assert!(c.validate()[0].contains("off its plane"));
    let mut c = room();
    c.edges[1].forward_edge = 99;
    assert!(c.validate()[0].contains("edge 1"));
}

#[test]
fn a_short_block_is_a_bounds_error() {
    let base = 0x8190_0000;
    let (mut bytes, _) = collision_room(base);
    // Ask for more vertices than the bytes hold.
    put(&mut bytes, 7 * 12, 1000);
    let space = Space::new(&bytes, base);
    assert!(matches!(
        bsp::CollisionBsp::read(&space, &bytes[..0x60]),
        Err(Error::Bounds { .. })
    ));
}

#[test]
fn the_scenario_multiplayer_blocks_read_at_their_offsets() {
    use scenario::layout::*;
    let base = 0x8070_0000u32;
    let mut b = vec![0u8; 0x5b0];
    let add = |b: &mut Vec<u8>, field: (usize, usize), elems: Vec<Vec<u8>>| {
        let addr = base + b.len() as u32;
        put(b, field.0, elems.len() as u32);
        put(b, field.0 + 4, addr);
        for e in elems {
            assert_eq!(e.len(), field.1);
            b.extend(e);
        }
    };
    let f = |v: &[f32]| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
    let mut start = f(&[1.0, 2.0, 3.0, 0.5]);
    start.extend([1, 0, 0, 0, 12, 0, 0, 0, 0, 0, 0, 0]);
    start.resize(0x34, 0);
    add(&mut b, STARTING_LOCATIONS, vec![start]);
    let mut flag = f(&[4.0, 5.0, 6.0, 1.0]);
    flag.extend([6, 0, 3, 0]);
    flag.extend(group(b"itmc").to_le_bytes());
    flag.extend([0; 8]);
    flag.extend(u32::MAX.to_le_bytes());
    flag.resize(0x94, 0);
    add(&mut b, NETGAME_FLAGS, vec![flag]);
    let mut eq = vec![0u8; 0x40];
    eq[4] = 13;
    eq[0xe] = 30;
    eq.extend(f(&[7.0, 8.0, 9.0, 0.25]));
    eq.extend(group(b"itmc").to_le_bytes());
    eq.extend([0; 8]);
    eq.extend(0xe1cb_0057u32.to_le_bytes());
    eq.resize(0x90, 0);
    add(&mut b, NETGAME_EQUIPMENT, vec![eq]);
    let mut sky = group(b"sky ").to_le_bytes().to_vec();
    sky.extend([0; 8]);
    sky.extend(0xe175_0001u32.to_le_bytes());
    add(&mut b, SKIES, vec![sky]);
    let space = Space::new(&b, base);
    let mp = scenario::Multiplayer::read(&space, &b[..0x5b0]).unwrap();
    let s = &mp.starting_locations[0];
    assert_eq!(
        (s.position, s.facing, s.team, s.bsp),
        ([1.0, 2.0, 3.0], 0.5, 1, 0)
    );
    assert_eq!(s.game_types[0], scenario::GameType::ALL);
    let fl = &mp.flags[0];
    assert_eq!((fl.kind, fl.team), (scenario::FlagKind::TeleportFrom, 3));
    assert!(fl.weapon_group.id.is_none());
    let e = &mp.equipment[0];
    assert_eq!(
        (e.position, e.facing, e.spawn_time),
        ([7.0, 8.0, 9.0], 0.25, 30)
    );
    assert_eq!(e.game_types[0].name(), "all except ctf");
    assert_eq!(e.item.id, TagId(0xe1cb_0057));
    assert_eq!(mp.skies[0].group, Group::new(b"sky "));
    assert!(mp.vehicles.is_empty());
}

// ---------------------------------------------------------------------------
// Real maps as stored on the disc, from `$H1_MAP_DIR` (every `*.map` in it).
// Run with `H1_MAP_DIR=... cargo test -p h1map -- --ignored real_maps`.

fn real_maps() -> Vec<(String, H1Map)> {
    let dir = std::env::var("H1_MAP_DIR").expect("set H1_MAP_DIR to a folder of Halo 1 maps");
    let mut out = Vec::new();
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "map"))
        .collect();
    paths.sort();
    for p in paths {
        let bytes = std::fs::read(&p).unwrap();
        let m = H1Map::from_reader(Cursor::new(&bytes))
            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
        out.push((p.file_stem().unwrap().to_string_lossy().into_owned(), m));
    }
    assert!(!out.is_empty(), "no maps in {dir}");
    out
}

#[test]
#[ignore = "reads Halo 1 maps from $H1_MAP_DIR"]
fn real_maps_read_and_their_multiplayer_parts_and_collision_check_out() {
    for (name, m) in real_maps() {
        let h = m.header();
        eprintln!(
            "{name}: len {:#x} f0c {:#x} index {:#x} meta {:#x} {} {} tags {} model {:x?}",
            h.file_length,
            h.field_0c,
            h.index_offset,
            h.metadata_size,
            h.build,
            h.cache_type,
            m.tags().len(),
            m.index().model_fields
        );
        assert_eq!(m.metadata_base(), XBOX_METADATA_BASE, "{name}");
        assert_eq!(h.name.to_ascii_lowercase(), name.to_ascii_lowercase());
        assert_eq!(m.scenario().group, Group::SCENARIO);
        assert_eq!(m.index().len, 0x24, "Xbox index header closes at 0x20");
        for t in m.tags() {
            assert!(!t.name.is_empty() && t.name.is_ascii(), "{name}: {}", t.id);
        }
        let bsps = m.structure_bsps().unwrap();
        assert!(!bsps.is_empty());
        let mp = m.multiplayer().unwrap();
        eprintln!(
            "  starts {} flags {} equipment {} vehicles {} skies {}",
            mp.starting_locations.len(),
            mp.flags.len(),
            mp.equipment.len(),
            mp.vehicles.len(),
            mp.skies.len()
        );
        for s in &mp.skies {
            assert_eq!(m.tag(s.id).unwrap().group, Group::new(b"sky "));
        }
        for e in &mp.equipment {
            let ic = m.item_collection(e.item.id).unwrap();
            for p in &ic.permutations {
                let g = m.tag(p.item.id).unwrap().group;
                assert!(g == Group::new(b"weap") || g == Group::new(b"eqip"), "{g}");
                assert!(p.weight > 0.0);
            }
        }
        for f in &mp.flags {
            assert!(
                !matches!(f.kind, scenario::FlagKind::Other(_)),
                "{:?}",
                f.kind
            );
            if f.kind == scenario::FlagKind::TeleportFrom {
                assert!(
                    mp.flags
                        .iter()
                        .any(|t| t.kind == scenario::FlagKind::TeleportTo && t.team == f.team),
                    "{name}: teleporter channel {} has no exit",
                    f.team
                );
            }
        }
        for s in &mp.starting_locations {
            let pi = std::f32::consts::PI + 1e-3;
            assert!(
                (-pi..=pi).contains(&s.facing),
                "{name}: facing {}",
                s.facing
            );
        }
        for v in &mp.vehicles {
            assert!((v.palette as usize) < mp.vehicle_palette.len());
        }
        for p in &mp.vehicle_palette {
            assert_eq!(m.tag(p.id).unwrap().group, Group::new(b"vehi"));
        }
        for b in &bsps {
            let s = m.structure_bsp(b).unwrap();
            let bounds = s.world_bounds();
            eprintln!(
                "  bsp {} @ {:#x}+{:#x}: bounds {bounds:?} clusters {} lightmaps {}",
                b.name,
                b.address,
                b.size,
                s.block_count(bsp::root::CLUSTERS),
                s.block_count(bsp::root::LIGHTMAPS)
            );
            assert_eq!(
                m.tag(s.lightmaps_bitmap().id).unwrap().group,
                Group::new(b"bitm")
            );
            for r in s.collision_materials().unwrap() {
                let g = m.tag(r.id).unwrap();
                assert!(g.parent == Group::new(b"shdr") || g.group == Group::new(b"shdr"));
            }
            let colls = s.collision_bsps().unwrap();
            assert_eq!(colls.len(), 1);
            let c = &colls[0];
            assert_eq!(c.validate(), Vec::<String>::new(), "{name}");
            // Every spawn point stands in open space with solid ground under it.
            let (mut open, mut ground) = (0, 0);
            for sp in &mp.starting_locations {
                let [x, y, z] = sp.position;
                assert!(x >= bounds[0] && x <= bounds[1] && y >= bounds[2] && y <= bounds[3]);
                open += c.leaf_at([x, y, z + 0.3]).is_some() as usize;
                ground +=
                    (0..20).any(|i| c.leaf_at([x, y, z - 0.05 * i as f32]).is_none()) as usize;
            }
            eprintln!(
                "  spawns in open space {open}/{n}, with ground within 1 m {ground}/{n}",
                n = mp.starting_locations.len()
            );
            assert_eq!(open, mp.starting_locations.len());
            assert_eq!(ground, mp.starting_locations.len());
        }
    }
}
