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
