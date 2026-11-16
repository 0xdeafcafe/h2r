//! Invented maps and discs, built in memory, for this crate's tests and for
//! tests downstream (`h2 import halo1`, the converter). Nothing here comes
//! from a real disc.

use std::io::Write;

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::*;

pub const BASE: u32 = XBOX_METADATA_BASE;
pub const SCNR_ID: u32 = 0xe174_0000;

/// One invented tag: group, parents, name, data (None = external).
pub struct Spec {
    pub group: &'static [u8; 4],
    pub parent: Option<&'static [u8; 4]>,
    pub name: &'static str,
    pub data: Option<Vec<u8>>,
}

/// Knobs for breaking an invented map.
#[derive(Default, Clone)]
pub struct Opts {
    /// Close the index header at 0x24 (PC layout) instead of 0x20.
    pub pc_index: bool,
    /// Store this ID for tag 1 instead of the right one.
    pub bad_id: Option<u32>,
}

pub struct Built {
    /// The decompressed file.
    pub file: Vec<u8>,
    /// File offsets of each tag's data.
    pub data_at: Vec<Option<usize>>,
    /// The two BSP regions: (file offset, len).
    pub bsps: Vec<(usize, usize)>,
}

pub fn put(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

pub fn group(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

/// A scenario with a two-element structure BSP block (filled in by
/// `build`), two BSP tags, a shader, an external bitmap and an `hmt ` tag.
pub fn specs() -> Vec<Spec> {
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

pub fn build(opts: &Opts) -> Built {
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
pub fn compress(file: &[u8]) -> Vec<u8> {
    let mut out = file[..HEADER_LEN].to_vec();
    let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
    z.write_all(&file[HEADER_LEN..]).unwrap();
    out.extend(z.finish().unwrap());
    out
}
pub const SECTOR: usize = 2048;

/// One directory table, entries chained down the right-hand side.
pub fn table(items: &[(&str, u32, u32, bool)]) -> Vec<u8> {
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
pub fn xbe(title: u32) -> Vec<u8> {
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

/// A disc with default.xbe for `title` and `/maps/` holding `maps` (name,
/// bytes) and a file that is not a map.
pub fn disc_image(title: u32, maps: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let sectors = |len: usize| len.div_ceil(SECTOR).max(1);
    let x = xbe(title);
    let xbe_at = 40;
    let mut next = xbe_at + sectors(x.len()) + 1;
    let mut items: Vec<(String, u32, u32, bool)> = Vec::new();
    for (name, bytes) in maps {
        items.push((name.to_string(), next as u32, bytes.len() as u32, false));
        next += sectors(bytes.len());
    }
    items.push(("readme.txt".into(), next as u32, 4, false));
    let mut img = vec![0u8; (next + 1) * SECTOR];
    let vd = 32 * SECTOR;
    img[vd..vd + 20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
    img[vd + 20..vd + 24].copy_from_slice(&33u32.to_le_bytes());
    img[vd + 24..vd + 28].copy_from_slice(&(SECTOR as u32).to_le_bytes());
    let root = table(&[
        ("default.xbe", xbe_at as u32, x.len() as u32, false),
        ("maps", 34, SECTOR as u32, true),
    ]);
    img[33 * SECTOR..34 * SECTOR].copy_from_slice(&root);
    img[xbe_at * SECTOR..xbe_at * SECTOR + x.len()].copy_from_slice(&x);
    let refs: Vec<(&str, u32, u32, bool)> = items
        .iter()
        .map(|(n, s, z, d)| (n.as_str(), *s, *z, *d))
        .collect();
    img[34 * SECTOR..35 * SECTOR].copy_from_slice(&table(&refs));
    for ((_, bytes), (_, sector, _, _)) in maps.iter().zip(&items) {
        let at = *sector as usize * SECTOR;
        img[at..at + bytes.len()].copy_from_slice(bytes);
    }
    img
}

/// A small valid map as a disc stores it, compressed.
pub fn tiny_map() -> Vec<u8> {
    compress(&build(&Opts::default()).file)
}

/// A Halo 1 disc with every campaign level, `ui.map` and one multiplayer
/// map, each a [`tiny_map`].
pub fn campaign_disc() -> Vec<u8> {
    let map = tiny_map();
    let names = crate::disc::CAMPAIGN_LEVELS
        .iter()
        .chain(&["ui", "bloodgulch"])
        .map(|n| format!("{n}.map"))
        .collect::<Vec<_>>();
    let maps: Vec<(&str, Vec<u8>)> = names.iter().map(|n| (n.as_str(), map.clone())).collect();
    disc_image(crate::disc::HALO1_TITLE_ID, &maps)
}
