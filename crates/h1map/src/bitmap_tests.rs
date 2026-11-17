//! `bitm` tags: invented tags parsed from memory, and an `#[ignore]`d check
//! of every bitmap on the owner's disc (`$H1_ISO`).

use std::collections::BTreeMap;
use std::io::Cursor;

use crate::bitmap::*;
use crate::testing::put;
use crate::*;

const BASE: u32 = 0x8000_0000;

fn put16(b: &mut [u8], at: usize, v: u16) {
    b[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

/// An invented `bitm`: header at `BASE`, one sequence with one sprite, two
/// images.
fn invented() -> Vec<u8> {
    let mut b = vec![0u8; 0x200];
    put16(&mut b, 0, 3); // sprites
    put16(&mut b, 4, 1);
    b[8..12].copy_from_slice(&0.5f32.to_le_bytes());
    put16(&mut b, 0x4c, 4);
    let (seq, spr, img) = (0x80usize, 0xc0usize, 0x100usize);
    put(&mut b, SEQUENCES, 1);
    put(&mut b, SEQUENCES + 4, BASE + seq as u32);
    put(&mut b, IMAGES, 2);
    put(&mut b, IMAGES + 4, BASE + img as u32);
    b[seq..seq + 4].copy_from_slice(b"fire");
    put16(&mut b, seq + 0x20, 0);
    put16(&mut b, seq + 0x22, 2);
    put(&mut b, seq + SPRITES, 1);
    put(&mut b, seq + SPRITES + 4, BASE + spr as u32);
    put16(&mut b, spr, 1);
    for (i, v) in [0.0f32, 0.5, 0.25, 1.0, 8.0, 9.0].iter().enumerate() {
        b[spr + 8 + 4 * i..spr + 12 + 4 * i].copy_from_slice(&v.to_le_bytes());
    }
    for (i, (fmt, kind, flags, w)) in [(14u16, 0u16, 3u16, 64u16), (11, 2, 9, 16)]
        .into_iter()
        .enumerate()
    {
        let e = img + i * IMAGE_LEN;
        put(&mut b, e, BITM.0);
        put16(&mut b, e + 4, w);
        put16(&mut b, e + 6, w);
        put16(&mut b, e + 8, 1);
        put16(&mut b, e + 0xa, kind);
        put16(&mut b, e + 0xc, fmt);
        put16(&mut b, e + 0xe, flags);
        put16(&mut b, e + 0x10, 0xfffe);
        put16(&mut b, e + 0x14, 2);
        put(&mut b, e + 0x18, 0x1000 * (i as u32 + 1));
        put(&mut b, e + 0x1c, 0x800);
    }
    b
}

#[test]
fn an_invented_bitmap_parses() {
    let b = invented();
    let bm = Bitmap::parse(&Space::new(&b, BASE), BASE).unwrap();
    assert_eq!(
        (bm.kind, bm.usage, bm.mipmap_count, bm.detail_fade),
        (3, 1, 4, 0.5)
    );
    assert_eq!(bm.sequences.len(), 1);
    let s = &bm.sequences[0];
    assert_eq!(
        (s.name.as_str(), s.first_image, s.image_count),
        ("fire", 0, 2)
    );
    assert_eq!(s.sprites[0].image, 1);
    assert_eq!(s.sprites[0].bounds, [0.0, 0.5, 0.25, 1.0]);
    assert_eq!(s.sprites[0].registration, [8.0, 9.0]);
    let [a, c] = &bm.images[..] else { panic!() };
    assert_eq!(
        (a.format, a.kind, a.width, a.mip_count),
        (PixelFormat::Dxt1, ImageKind::D2, 64, 2)
    );
    assert!(!a.swizzled() && a.flags & FLAG_COMPRESSED != 0);
    assert_eq!((c.format, c.kind), (PixelFormat::A8R8G8B8, ImageKind::Cube));
    assert!(c.swizzled() && !c.linear());
    assert_eq!(c.registration, [-2, 0]);
    assert_eq!(c.pixel_range(), 0x2000..0x2800);
    // pixels outside the file are refused, not sliced short
    assert!(pixels(&[0u8; 0x2000], c).is_err());
    assert_eq!(pixels(&[0u8; 0x2800], c).unwrap().len(), 0x800);
}

#[test]
fn a_bad_signature_or_block_is_refused() {
    let mut b = invented();
    b[0x100] = b'x';
    assert!(Bitmap::parse(&Space::new(&b, BASE), BASE).is_err());
    let mut b = invented();
    put(&mut b, IMAGES, 1000);
    assert!(Bitmap::parse(&Space::new(&b, BASE), BASE).is_err());
}

#[test]
fn formats_round_trip_their_stored_values() {
    for v in 0..32 {
        assert_eq!(PixelFormat::from_raw(v).raw(), v);
        assert_eq!(ImageKind::from_raw(v).raw(), v);
    }
    assert_eq!(PixelFormat::from_raw(14), PixelFormat::Dxt1);
    assert!(PixelFormat::Dxt5.is_dxt() && !PixelFormat::P8Bump.is_dxt());
}

fn real_disc() -> Option<xiso::Xiso<std::io::BufReader<std::fs::File>>> {
    let path = std::env::var("H1_ISO").ok()?;
    let f = std::fs::File::open(&path).ok()?;
    Some(xiso::Xiso::open(std::io::BufReader::new(f)).expect("an XDVDFS image"))
}

/// Every `bitm` on every map of the disc reads, each image's pixels are in
/// the file, outside the tag data and every BSP region, and no two images
/// overlap. Prints a census of formats.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn real_disc_every_bitmap_reads() {
    let mut iso = real_disc().expect("set H1_ISO to the Halo 1 disc");
    let d = disc::Disc::open(&mut iso).unwrap();
    let mut census: BTreeMap<String, usize> = BTreeMap::new();
    let (mut tags, mut images) = (0, 0);
    for e in d.maps.clone() {
        let bytes = iso.read(&e).unwrap();
        let m = H1Map::from_reader(Cursor::new(&bytes)).unwrap();
        let h = m.header();
        let meta = h.index_offset as usize..(h.index_offset + h.metadata_size) as usize;
        let bsps: Vec<_> = m
            .structure_bsps()
            .unwrap_or_default()
            .iter()
            .map(|b| b.file_offset as usize..(b.file_offset + b.size) as usize)
            .collect();
        let mut ranges = Vec::new();
        let (mut lo, mut hi) = (usize::MAX, 0);
        for t in m.tags().iter().filter(|t| t.group == BITM) {
            let bm =
                Bitmap::read(&m, t).unwrap_or_else(|err| panic!("{} {}: {err}", e.path, t.name));
            tags += 1;
            for img in &bm.images {
                images += 1;
                let r = img.pixel_range();
                assert!(
                    r.end <= meta.start || r.start >= meta.end,
                    "{}: pixels in tag data",
                    t.name
                );
                for b in &bsps {
                    assert!(
                        r.end <= b.start || r.start >= b.end,
                        "{}: pixels in a BSP",
                        t.name
                    );
                }
                lo = lo.min(r.start);
                hi = hi.max(r.end);
                ranges.push(r);
                *census
                    .entry(format!(
                        "{:?} {:?} flags {:#x}",
                        img.format,
                        img.kind,
                        img.flags & 0x1f
                    ))
                    .or_default() += 1;
            }
        }
        ranges.sort_by_key(|r| r.start);
        let overlaps = ranges.windows(2).filter(|w| w[0].end > w[1].start).count();
        eprintln!(
            "{:<16} bitmaps' pixels {lo:#x}..{hi:#x}, tag data {meta:x?}, bsps {bsps:x?}, file {:#x}, overlaps {overlaps}",
            disc::file_name(&e),
            m.bytes().len()
        );
    }
    eprintln!("{tags} bitm tags, {images} images");
    for (k, n) in census {
        eprintln!("{n:>6}  {k}");
    }
}
