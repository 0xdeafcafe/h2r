//! Tests on invented maps and discs built in memory. The `#[ignore]`d tests
//! at the end read the owner's disc from `$H1_ISO` (default
//! `disc/halo1.iso` in the main checkout).

use std::io::{Cursor, Write};

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::disc::{self, Disc};
use crate::*;

const BASE: u32 = XBOX_METADATA_BASE;
const SCNR_ID: u32 = 0xe174_0000;

/// One invented tag: group, parents, name, data (None = external).
struct Spec {
    group: &'static [u8; 4],
    parent: Option<&'static [u8; 4]>,
    name: &'static str,
    data: Option<Vec<u8>>,
}

/// Knobs for breaking an invented map.
#[derive(Default, Clone)]
struct Opts {
    /// Close the index header at 0x24 (PC layout) instead of 0x20.
    pc_index: bool,
    /// Store this ID for tag 1 instead of the right one.
    bad_id: Option<u32>,
}

struct Built {
    /// The decompressed file.
    file: Vec<u8>,
    /// File offsets of each tag's data.
    data_at: Vec<Option<usize>>,
    /// The two BSP regions: (file offset, len).
    bsps: Vec<(usize, usize)>,
}

fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn group(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

/// A scenario with a two-element structure BSP block (filled in by
/// `build`), two BSP tags, a shader, an external bitmap and an `hmt ` tag.
fn specs() -> Vec<Spec> {
    vec![
        Spec {
            group: b"scnr",
            parent: None,
            name: "levels\\test\\tiny\\tiny",
            data: Some(vec![0x5c; 0x600]),
        },
        Spec {
            group: b"sbsp",
            parent: None,
            name: "levels\\test\\tiny\\tiny_a",
            data: Some(vec![1; 0x10]),
        },
        Spec {
            group: b"sbsp",
            parent: None,
            name: "levels\\test\\tiny\\tiny_b",
            data: Some(vec![2; 0x10]),
        },
        Spec {
            group: b"senv",
            parent: Some(b"shdr"),
            name: "levels\\test\\tiny\\shaders\\floor",
            data: Some(vec![3; 0x44]),
        },
        Spec {
            group: b"bitm",
            parent: None,
            name: "levels\\test\\tiny\\bitmaps\\floor",
            data: None,
        },
        Spec {
            group: b"hmt ",
            parent: None,
            name: "ui\\test\\hud",
            data: Some(vec![4; 0x8]),
        },
    ]
}

fn build(opts: &Opts) -> Built {
    let tags = specs();
    let index_len = if opts.pc_index { 0x28 } else { 0x24 };
    let index_at = HEADER_LEN;
    // Metadata: index header, tag array, names, then each tag's data, then
    // the scenario's BSP block elements.
    let mut meta = vec![0u8; index_len];
    let array_at = meta.len();
    meta.resize(array_at + tags.len() * TAG_ENTRY_LEN, 0);
    let addr = |off: usize| BASE + off as u32;
    let mut name_addrs = Vec::new();
    for t in &tags {
        name_addrs.push(addr(meta.len()));
        meta.extend(t.name.as_bytes());
        meta.push(0);
    }
    let mut data_offs = Vec::new();
    for t in &tags {
        while !meta.len().is_multiple_of(4) {
            meta.push(0);
        }
        data_offs.push(t.data.as_ref().map(|d| {
            let at = meta.len();
            meta.extend(d);
            at
        }));
    }
    let elems_at = meta.len();
    meta.resize(elems_at + 2 * SCNR_BSP_LEN, 0);
    let meta_len = meta.len();

    // The BSP regions sit after the tag data, each at a stated offset.
    let bsp_a = (index_at + meta_len + 0x40, 0x30);
    let bsp_b = (bsp_a.0 + bsp_a.1 + 0x10, 0x20);
    let file_len = bsp_b.0 + bsp_b.1 + 0x8;

    // Scenario BSP block and its elements.
    let scnr = data_offs[0].unwrap();
    put(&mut meta, scnr + SCNR_STRUCTURE_BSPS, 2);
    put(&mut meta, scnr + SCNR_STRUCTURE_BSPS + 4, addr(elems_at));
    for (i, (off, len)) in [bsp_a, bsp_b].into_iter().enumerate() {
        let e = elems_at + i * SCNR_BSP_LEN;
        put(&mut meta, e, off as u32);
        put(&mut meta, e + 4, len as u32);
        put(&mut meta, e + 8, 0x8100_0000);
        put(&mut meta, e + 0x10, group(b"sbsp"));
        put(&mut meta, e + 0x14, name_addrs[1 + i]);
        put(&mut meta, e + 0x18, tags[1 + i].name.len() as u32);
        put(&mut meta, e + 0x1c, SCNR_ID + 1 + i as u32);
    }

    // Index header.
    put(&mut meta, 0x00, addr(array_at));
    put(&mut meta, 0x04, SCNR_ID);
    put(&mut meta, 0x08, 0x1234_5678);
    put(&mut meta, 0x0c, tags.len() as u32);
    put(&mut meta, 0x10, 7);
    put(&mut meta, 0x14, 0x8800_0000);
    put(&mut meta, 0x18, 7);
    put(&mut meta, 0x1c, 0x8900_0000);
    put(&mut meta, index_len - 4, TAGS);

    // Tag array.
    for (i, t) in tags.iter().enumerate() {
        let e = array_at + i * TAG_ENTRY_LEN;
        put(&mut meta, e, group(t.group));
        put(&mut meta, e + 4, t.parent.map_or(u32::MAX, group));
        put(&mut meta, e + 8, u32::MAX);
        let id = if i == 1 {
            opts.bad_id.unwrap_or(SCNR_ID + 1)
        } else {
            SCNR_ID + i as u32
        };
        put(&mut meta, e + 0xc, id);
        put(&mut meta, e + 0x10, name_addrs[i]);
        put(&mut meta, e + 0x14, data_offs[i].map_or(9, addr));
        put(&mut meta, e + 0x18, u32::from(t.data.is_none()));
    }

    let mut file = vec![0u8; file_len];
    put(&mut file, 0, HEAD);
    put(&mut file, 4, VERSION);
    put(&mut file, 8, file_len as u32);
    put(&mut file, 0x10, index_at as u32);
    put(&mut file, 0x14, meta_len as u32);
    file[0x20..0x24].copy_from_slice(b"tiny");
    file[0x40..0x4d].copy_from_slice(b"01.10.12.2276");
    file[0x60] = 0; // campaign
    put(&mut file, 0x64, 0xcafe_f00d);
    put(&mut file, FOOT_OFFSET, FOOT);
    file[index_at..index_at + meta_len].copy_from_slice(&meta);
    for (i, &(off, len)) in [bsp_a, bsp_b].iter().enumerate() {
        file[off..off + len].fill(0xb0 + i as u8);
    }
    Built {
        file,
        data_at: data_offs.iter().map(|o| o.map(|o| index_at + o)).collect(),
        bsps: vec![bsp_a, bsp_b],
    }
}

/// The file as a disc stores it: the header, then the zlib stream.
fn compress(file: &[u8]) -> Vec<u8> {
    let mut out = file[..HEADER_LEN].to_vec();
    let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
    z.write_all(&file[HEADER_LEN..]).unwrap();
    out.extend(z.finish().unwrap());
    out
}

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

const SECTOR: usize = 2048;

/// One directory table, entries chained down the right-hand side.
fn table(items: &[(&str, u32, u32, bool)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut offs = Vec::new();
    for &(name, sector, size, dir) in items {
        offs.push(out.len());
        let mut v = vec![0u8; 4];
        v.extend(sector.to_le_bytes());
        v.extend(size.to_le_bytes());
        v.push(if dir { 0x10 } else { 0 });
        v.push(name.len() as u8);
        v.extend(name.as_bytes());
        while !v.len().is_multiple_of(4) {
            v.push(0xff);
        }
        out.extend(v);
    }
    for i in 0..offs.len().saturating_sub(1) {
        let right = (offs[i + 1] / 4) as u16;
        out[offs[i] + 2..offs[i] + 4].copy_from_slice(&right.to_le_bytes());
    }
    out.resize(SECTOR, 0xff);
    out
}

/// A synthetic XBE: just enough header for `xbe::Xbe::parse`.
fn xbe(title: u32) -> Vec<u8> {
    let base = 0x10000u32;
    let mut d = vec![0u8; 0x1000];
    d[0..4].copy_from_slice(b"XBEH");
    put(&mut d, 0x104, base);
    put(&mut d, 0x108, 0x1000);
    put(&mut d, 0x10c, 0x1000);
    put(&mut d, 0x118, base + 0x200); // certificate
    put(&mut d, 0x11c, 1); // one executable section over the headers
    put(&mut d, 0x120, base + 0x300);
    put(&mut d, 0x300, 0x04);
    put(&mut d, 0x304, base);
    put(&mut d, 0x308, 0x1000);
    put(&mut d, 0x314, base + 0x380);
    d[0x380] = b'x';
    put(&mut d, 0x128, 0xA8FC_57AB ^ (base + 0x400)); // entry (retail key)
    put(&mut d, 0x158, 0x5B6D_40B6 ^ (base + 0x400)); // thunk (retail key)
    put(&mut d, 0x200, 0x1d0);
    put(&mut d, 0x208, title);
    put(&mut d, 0x200 + 0xac, 0x0000_0101);
    d
}

/// A disc with default.xbe and `/maps/` holding `maps` (each 16 bytes of
/// its index) and a non-map file.
fn disc_image(title: u32, maps: &[&str]) -> Vec<u8> {
    let mut img = vec![0u8; 200 * SECTOR];
    let vd = 32 * SECTOR;
    img[vd..vd + 20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
    img[vd + 20..vd + 24].copy_from_slice(&33u32.to_le_bytes());
    img[vd + 24..vd + 28].copy_from_slice(&(SECTOR as u32).to_le_bytes());
    let x = xbe(title);
    let root = table(&[
        ("default.xbe", 40, x.len() as u32, false),
        ("maps", 34, SECTOR as u32, true),
    ]);
    img[33 * SECTOR..34 * SECTOR].copy_from_slice(&root);
    img[40 * SECTOR..40 * SECTOR + x.len()].copy_from_slice(&x);
    let mut items: Vec<(String, u32, u32, bool)> = maps
        .iter()
        .enumerate()
        .map(|(i, m)| (m.to_string(), 50 + i as u32, 16, false))
        .collect();
    items.push(("readme.txt".into(), 49, 4, false));
    let refs: Vec<(&str, u32, u32, bool)> = items
        .iter()
        .map(|(n, s, z, d)| (n.as_str(), *s, *z, *d))
        .collect();
    img[34 * SECTOR..35 * SECTOR].copy_from_slice(&table(&refs));
    for (i, _) in maps.iter().enumerate() {
        let at = (50 + i) * SECTOR;
        img[at..at + 16].fill(i as u8 + 1);
    }
    img
}

const ALL_LEVELS: [&str; 10] = [
    "a10.map", "A30.map", "a50.map", "b30.map", "b40.map", "c10.map", "c20.map", "c40.map",
    "d20.map", "d40.map",
];

#[test]
fn recognises_the_disc_and_picks_the_campaign_maps() {
    let mut names = ALL_LEVELS.to_vec();
    names.extend(["ui.map", "bloodgulch.map"]);
    let mut iso = xiso::Xiso::open(Cursor::new(disc_image(disc::HALO1_TITLE_ID, &names))).unwrap();
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
        &ALL_LEVELS[1..],
    )))
    .unwrap();
    let err = Disc::open(&mut iso).unwrap().campaign_maps().unwrap_err();
    assert!(
        err.to_string().contains("missing campaign maps: a10"),
        "{err}"
    );
    let mut iso = xiso::Xiso::open(Cursor::new(disc_image(0x4d53_0064, &ALL_LEVELS))).unwrap();
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
