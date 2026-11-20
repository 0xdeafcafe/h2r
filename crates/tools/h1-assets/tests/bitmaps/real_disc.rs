//! Checks on the owner's Halo 1 disc (`$H1_ISO`), all `#[ignore]`d. No map
//! bytes or pixels are kept: everything is read and compared in memory.
//!
//! ```text
//! H1_ISO=disc/halo1.iso cargo test -p h1bitmaps --release -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::io::Cursor;

use h1_assets::bitmaps::convert::{LOD_ALIGN, convert_bitmap};
use h1_assets::bitmaps::decode::{decode_image, layout_len};
use h1_assets::bitmaps::{BitmapExt, lightmap_pages};
use h1map::H1Map;
use h1map::bitmap::{BITM, Bitmap};
use map::build::TagSet;
use map::{CacheType, Map, ResourceFile, TagId};

fn disc() -> xiso::Xiso<std::io::BufReader<std::fs::File>> {
    let path = std::env::var("H1_ISO").expect("set H1_ISO to the Halo 1 disc");
    let f = std::fs::File::open(&path).expect("H1_ISO opens");
    xiso::Xiso::open(std::io::BufReader::new(f)).expect("an XDVDFS image")
}

fn maps() -> Vec<(String, H1Map)> {
    let mut iso = disc();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    d.maps
        .clone()
        .into_iter()
        .map(|e| {
            let bytes = iso.read(&e).unwrap();
            let name = h1map::disc::map_stem(&e).to_string();
            (name, H1Map::from_reader(Cursor::new(bytes)).unwrap())
        })
        .collect()
}

fn one_map(name: &str) -> H1Map {
    let mut iso = disc();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).expect("map on the disc").clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

/// Every image of every bitmap on the disc decodes; its pixel size is
/// exactly the Halo 2 layout's size rounded up to 128 bytes (which pins the
/// mip, DXT chain, pitch and cube face rules); every level has its size's
/// bytes; and no image decodes to a single colour unless its stored bytes
/// are a single repeated value too. Prints the format census.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn every_bitmap_on_the_disc_decodes() {
    let mut census: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let (mut images, mut flat, mut failures, mut size_mismatch) =
        (0usize, 0usize, Vec::new(), Vec::new());
    let mut flat_from_varied = Vec::new();
    for (name, m) in maps() {
        let mut n = 0;
        for t in m.tags().iter().filter(|t| t.group == BITM) {
            let bm = Bitmap::read(&m, t).unwrap();
            for (i, img) in bm.images.iter().enumerate() {
                images += 1;
                n += 1;
                let key = format!(
                    "{:<9} {:<4} {}",
                    format!("{:?}", img.format),
                    format!("{:?}", img.kind),
                    if img.linear() {
                        "linear"
                    } else if img.swizzled() {
                        "swizzled"
                    } else {
                        ""
                    }
                );
                let c = census.entry(key).or_default();
                c.0 += 1;
                c.1 += img.pixel_size as u64;
                let levels = match bm.decode_levels(&m, i) {
                    Ok(l) => l,
                    Err(e) => {
                        failures.push(format!("{name} {} #{i}: {e}", t.name));
                        continue;
                    }
                };
                let need = layout_len(img).unwrap();
                if need.next_multiple_of(LOD_ALIGN) != img.pixel_size as usize {
                    size_mismatch.push(format!(
                        "{name} {} #{i} {:?} {:?} {}x{}x{} mips {}: layout {need:#x} pixels {:#x}",
                        t.name,
                        img.format,
                        img.kind,
                        img.width,
                        img.height,
                        img.depth,
                        img.mip_count,
                        img.pixel_size
                    ));
                }
                let faces = if img.kind == h1map::bitmap::ImageKind::Cube {
                    6
                } else {
                    1
                };
                assert_eq!(
                    levels.len(),
                    faces * levels.iter().filter(|l| l.face == 0).count()
                );
                for l in &levels {
                    assert_eq!(l.rgba.len(), (l.width * l.height * l.depth * 4) as usize);
                }
                let base = &levels[0];
                assert_eq!(
                    (base.width, base.height),
                    (u32::from(img.width), u32::from(img.height))
                );
                let one_colour = base
                    .rgba
                    .as_chunks::<4>()
                    .0
                    .windows(2)
                    .all(|w| w[0] == w[1]);
                if one_colour {
                    let px = bm.pixels(&m, i).unwrap();
                    let stored_flat =
                        px[..need].windows(2).all(|w| w[0] == w[1]) || img.format.is_dxt();
                    flat += 1;
                    // Tiny solid textures (the rasterizer's 4x4 defaults) are
                    // one colour by design.
                    if !stored_flat && u32::from(img.width) * u32::from(img.height) > 16 {
                        flat_from_varied.push(format!(
                            "{name} {} #{i} {:?} {:?} {}x{}x{} mips {}",
                            t.name,
                            img.format,
                            img.kind,
                            img.width,
                            img.height,
                            img.depth,
                            img.mip_count
                        ));
                    }
                }
                // The stacked form has every face.
                let s = bm.decode(&m, i, 0).unwrap();
                assert_eq!(s.height, base.height * base.depth * faces as u32);
            }
        }
        eprintln!("{name:<16} {n} images");
    }
    eprintln!("{images} images, {flat} of one colour");
    eprintln!("{:>6} {:>12}  format, type, storage", "images", "bytes");
    for (k, (n, bytes)) in &census {
        eprintln!("{n:>6} {bytes:>12}  {k}");
    }
    for f in size_mismatch.iter().take(20) {
        eprintln!("size: {f}");
    }
    for f in flat_from_varied.iter().take(20) {
        eprintln!("one colour from varied bytes: {f}");
    }
    eprintln!(
        "{} larger images of one colour from varied bytes",
        flat_from_varied.len()
    );
    for f in failures.iter().take(20) {
        eprintln!("FAIL: {f}");
    }
    assert!(failures.is_empty(), "{} images failed", failures.len());
    assert!(
        flat_from_varied.is_empty(),
        "{} images decode to one colour",
        flat_from_varied.len()
    );
    assert!(
        size_mismatch.is_empty(),
        "{} sizes differ",
        size_mismatch.len()
    );
}

/// Builds a map holding `tags`, reads each back with Halo 2's tag decoder
/// and decodes every image with Halo 2's bitmap decoder (`assets`), and
/// compares every level with the Halo 1 decode. Returns images compared.
fn round_trip(m: &H1Map, tags: &[&h1map::Tag]) -> usize {
    let mut set = TagSet::new(CacheType::Multiplayer, "h1bitmaps", "invented\\h1bitmaps");
    let mut ids: Vec<(TagId, Bitmap)> = Vec::new();
    for t in tags {
        let (bm, out, report) = convert_bitmap(m, t).unwrap_or_else(|e| panic!("{}: {e}", t.name));
        for r in &report {
            // Halo 1 pads every chain exactly as Halo 2 does: nothing cut.
            assert_eq!(r.trimmed, 0, "{}", t.name);
        }
        ids.push((out.install(&mut set, &t.name).unwrap(), bm));
    }
    let (bytes, _) = set.build().expect("map builds");
    let mut h2 = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
    let mut compared = 0;
    for (id, h1) in &ids {
        let decoded = tags::DecodedTag::read(&mut h2, *id).unwrap();
        let tags::TagData::Bitmap(b) = decoded.data else {
            panic!("not a bitmap")
        };
        assert_eq!(b.images.len(), h1.images.len());
        assert_eq!(b.sequences.len(), h1.sequences.len());
        for (s2, s1) in b.sequences.iter().zip(&h1.sequences) {
            assert_eq!(s2.sprites.len(), s1.sprites.len());
            for (p2, p1) in s2.sprites.iter().zip(&s1.sprites) {
                assert_eq!(p2.image_index, p1.image);
                assert_eq!(p2.bounds, p1.bounds);
            }
        }
        for (i, img) in b.images.iter().enumerate() {
            assert_eq!(img.wire.runtime_id, id.0, "the image names its tag");
            let lod = img.lods[0].expect("LOD 0");
            assert_eq!(lod.pointer.file, ResourceFile::Local);
            assert!(img.lods[1].is_none() && img.lods[2].is_none());
            let desc = assets::texture_desc(img, 0).unwrap();
            let len = desc.layout().unwrap().total_size;
            assert_eq!(len.next_multiple_of(LOD_ALIGN), lod.size as usize);
            let at = lod.pointer.offset as usize;
            let h2_levels = assets::bitmap::decode_bytes(&desc, &bytes[at..at + len]).unwrap();
            let h1_img = &h1.images[i];
            let h1_levels = decode_image(h1_img, h1.pixels(m, i).unwrap()).unwrap();
            assert_eq!(h2_levels.len(), h1_levels.len());
            for (a, c) in h2_levels.iter().zip(&h1_levels) {
                assert_eq!(
                    (a.face, a.level, a.width, a.height, a.depth),
                    (c.face, c.level, c.width, c.height, c.depth)
                );
                assert!(
                    a.rgba == c.rgba,
                    "pixels differ: image {i} face {} level {}",
                    a.face,
                    a.level
                );
            }
            compared += 1;
        }
    }
    compared
}

/// Every bitmap on beavercreek, converted into one built map, decodes
/// through Halo 2's decoder to exactly the Halo 1 decode.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn beavercreek_bitmaps_convert_and_decode_identically() {
    let m = one_map("beavercreek");
    let tags: Vec<&h1map::Tag> = m.tags().iter().filter(|t| t.group == BITM).collect();
    let n = round_trip(&m, &tags);
    eprintln!("beavercreek: {} bitmaps, {n} images identical", tags.len());
}

/// a30's lightmap bitmaps: the pages each lightmap names exist, and the
/// bitmap converts and decodes identically.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn a30_lightmaps_convert_and_decode_identically() {
    let m = one_map("a30");
    let mut tags = Vec::new();
    for b in m.structure_bsps().unwrap() {
        let bsp = m.structure_bsp(&b).unwrap();
        let pages = lightmap_pages(&m, &bsp).unwrap();
        let tag = pages.tag.clone().expect("a lightmap bitmap");
        let bm = Bitmap::read(&m, &tag).unwrap();
        let used = pages.pages.iter().flatten().count();
        let formats: BTreeMap<String, usize> =
            bm.images.iter().fold(BTreeMap::new(), |mut c, i| {
                *c.entry(format!(
                    "{:?} {}x{} mips {}",
                    i.format, i.width, i.height, i.mip_count
                ))
                .or_default() += 1;
                c
            });
        eprintln!(
            "{}: {} lightmaps ({used} with a page), bitmap {} with {} pages: {formats:?}",
            b.name,
            pages.pages.len(),
            tag.name,
            bm.images.len()
        );
        tags.push(tag);
    }
    let refs: Vec<&h1map::Tag> = tags.iter().collect();
    let n = round_trip(&m, &refs);
    eprintln!("a30 lightmaps: {n} pages identical");
}

/// The Halo 2 side: what retail images hold in the fields the converter
/// leaves zero (runtime flags, LOD bias, cache usage, pixel offset, runtime
/// ID, low-detail), and when LODs 1 and 2 are present. Reads `$H2_DISC`'s
/// multiplayer maps; prints a survey.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn halo2_image_fields_survey() {
    use std::io::BufReader;
    let disc = std::env::var("H2_DISC").expect("set H2_DISC to the Halo 2 disc");
    let open = || BufReader::new(std::fs::File::open(&disc).unwrap());
    let mut iso = xiso::Xiso::open(open()).unwrap();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    let mut count = |k: String| *seen.entry(k).or_default() += 1;
    let mut min_lod1 = u32::MAX;
    let mut max_no_lod1 = 0u32;
    for e in iso.entries().unwrap() {
        let Some(stem) = e
            .path
            .rsplit(['/', '\\'])
            .next()
            .and_then(|f| f.strip_suffix(".map"))
        else {
            continue;
        };
        if !["beavercreek", "ascension", "zanzibar"].contains(&stem) {
            continue;
        }
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let mut m = Map::from_region(open(), base, e.size.into()).unwrap();
        let ids: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == map::FourCC(*b"bitm") && t.location.is_some())
            .map(|t| t.id)
            .collect();
        for id in ids {
            let Ok(d) = tags::DecodedTag::read(&mut m, id) else {
                continue;
            };
            let tags::TagData::Bitmap(b) = d.data else {
                continue;
            };
            for img in &b.images {
                let w = &img.wire;
                count(format!("runtime_flags {:#x}", w.runtime_flags));
                count(format!(
                    "bytes 0x16/0x17 {:>2} {:>2} for {:?} {}x{} mips {} lod0 {:#x}",
                    w.lod_bias,
                    w.cache_usage,
                    img.format,
                    img.dimensions[0],
                    img.dimensions[1],
                    img.mip_count,
                    img.lods[0].map_or(0, |l| l.size)
                ));
                count(format!("pixel_offset zero {}", w.pixel_offset == 0));
                count(format!(
                    "runtime_id {}",
                    if w.runtime_id == 0 {
                        "zero"
                    } else if w.runtime_id == id.0 {
                        "own datum"
                    } else {
                        "other"
                    }
                ));
                count(format!(
                    "low_detail {}",
                    if w.low_detail_size == 0 {
                        "none"
                    } else {
                        "present"
                    }
                ));
                let texels = u32::from(img.dimensions[0]) * u32::from(img.dimensions[1]);
                match img.lods[1] {
                    Some(_) => min_lod1 = min_lod1.min(texels),
                    None if img.mip_count > 0 => max_no_lod1 = max_no_lod1.max(texels),
                    None => {}
                }
                count(format!(
                    "lods {}{}{}",
                    img.lods[0].is_some() as u8,
                    img.lods[1].is_some() as u8,
                    img.lods[2].is_some() as u8
                ));
            }
        }
    }
    for (k, n) in &seen {
        eprintln!("{n:>6}  {k}");
    }
    eprintln!(
        "smallest image with LOD 1: {min_lod1} texels; largest mipped image without: {max_no_lod1}"
    );
}

/// Every structure BSP on the disc: its lightmap bitmap resolves, every
/// lightmap's page is one of its images, and the pages are what the
/// structure writer gets. Prints the lightmap formats.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn every_bsp_lightmap_page_resolves() {
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let (mut bsps, mut lightmaps, mut unlit, mut shared) = (0, 0, 0, 0);
    let (mut shared_pages, mut unused_pages) = (0, 0);
    for (name, m) in maps() {
        let mut seen = std::collections::HashSet::new();
        for b in m.structure_bsps().unwrap_or_default() {
            let bsp = m.structure_bsp(&b).unwrap();
            let pages =
                lightmap_pages(&m, &bsp).unwrap_or_else(|e| panic!("{name} {}: {e}", b.name));
            bsps += 1;
            lightmaps += pages.pages.len();
            unlit += pages.pages.iter().filter(|p| p.is_none()).count();
            let Some(tag) = pages.tag else {
                eprintln!("{name} {}: no lightmap bitmap", b.name);
                continue;
            };
            if !seen.insert(tag.id) {
                shared += 1;
            }
            let images = Bitmap::read(&m, &tag).unwrap().images;
            let mut used: Vec<u16> = pages.pages.iter().flatten().copied().collect();
            used.sort_unstable();
            used.dedup();
            shared_pages += pages.pages.iter().flatten().count() - used.len();
            unused_pages += images.len() - used.len();
            for img in images {
                *formats
                    .entry(format!(
                        "{:?} {:?} mips {} flags {:#x}",
                        img.format, img.kind, img.mip_count, img.flags
                    ))
                    .or_default() += 1;
            }
        }
    }
    eprintln!(
        "{bsps} BSPs, {lightmaps} lightmaps ({unlit} without a page), {shared} bitmaps shared by BSPs, \
         {shared_pages} lightmaps on a page another lightmap uses, {unused_pages} pages no lightmap uses"
    );
    for (k, n) in &formats {
        eprintln!("{n:>6}  {k}");
    }
}
