//! Invented images only: per-format texels, Xbox swizzle, DXT blocks, cube
//! faces and mips through the Halo 1 path, and a conversion read back with
//! Halo 2's decoders.

use std::io::Cursor;

use h1map::bitmap::{BITM, Bitmap, IMAGE_LEN, Image, ImageKind, Sequence, Sprite};
use map::build::TagSet;
use map::{CacheType, Map, ResourceFile};

use crate::convert::{IMAGE_CACHED, IMAGE_NONE_WORDS, LOD_ALIGN, convert_with};
use crate::decode::{decode_image, layout_len, stack};

const SWZ: u16 = 0x9;
const LIN: u16 = 0x11;
const DXT: u16 = 0x3;
const P8: u16 = 0xd;

fn image(format: u16, kind: u16, [w, h, d]: [u16; 3], mips: u16, flags: u16) -> Image {
    let mut e = [0u8; IMAGE_LEN];
    e[0..4].copy_from_slice(&BITM.0.to_le_bytes());
    for (at, v) in [
        (4, w),
        (6, h),
        (8, d),
        (0xa, kind),
        (0xc, format),
        (0xe, flags),
        (0x14, mips),
    ] {
        e[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    let mut img = Image::parse(&e).unwrap();
    img.pixel_size = layout_len(&img).unwrap().next_multiple_of(LOD_ALIGN) as u32;
    img
}

fn one(format: u16, flags: u16, texel: &[u8]) -> [u8; 4] {
    let img = image(format, 0, [1, 1, 1], 0, flags);
    let mut px = vec![0u8; img.pixel_size as usize];
    px[..texel.len()].copy_from_slice(texel);
    let l = decode_image(&img, &px).unwrap();
    l[0].rgba[..4].try_into().unwrap()
}

#[test]
fn every_uncompressed_format_decodes_a_known_texel() {
    let cases: [(u16, &[u8], [u8; 4]); 10] = [
        (0, &[0x80], [255, 255, 255, 0x80]),          // A8
        (1, &[0x80], [0x80, 0x80, 0x80, 255]),        // Y8
        (2, &[0x40], [0x40; 4]),                      // AY8
        (3, &[0x10, 0x20], [0x10, 0x10, 0x10, 0x20]), // A8Y8: Y then A
        (6, &[0x00, 0xf8], [255, 0, 0, 255]),         // R5G6B5
        (8, &[0xe0, 0x83], [0, 255, 0, 255]),         // A1R5G5B5
        (9, &[0x0f, 0xf0], [0, 0, 255, 255]),         // A4R4G4B4
        (10, &[1, 2, 3, 0], [3, 2, 1, 255]),          // X8R8G8B8: B, G, R, X
        (11, &[1, 2, 3, 4], [3, 2, 1, 4]),            // A8R8G8B8: B, G, R, A
        (17, &[0x7f], [0x7f, 0x7f, 0x7f, 255]),       // P8 bump: the index, as luminance
    ];
    for (format, texel, want) in cases {
        let flags = if format == 17 { P8 } else { SWZ };
        assert_eq!(one(format, flags, texel), want, "format {format}");
    }
    // the disc's one linear format, rows padded to 64 bytes
    let img = image(9, 0, [3, 2, 1], 0, LIN);
    let mut px = vec![0u8; img.pixel_size as usize];
    px[64..66].copy_from_slice(&[0x0f, 0xf0]);
    let l = decode_image(&img, &px).unwrap();
    assert_eq!(
        &l[0].rgba[12..16],
        &[0, 0, 255, 255],
        "row 1 starts at byte 64"
    );
}

/// Xbox swizzle: the texel at (x, y) of a power-of-two image is stored at
/// the interleaving of x and y's bits, x in the lowest bit (for a non-square
/// image the longer side's extra bits come last).
fn morton(x: u32, y: u32, w: u32, h: u32) -> u32 {
    let (mut out, mut bit, mut sx, mut sy) = (0, 0, 1, 1);
    let (mut x, mut y) = (x, y);
    while sx < w || sy < h {
        if sx < w {
            out |= (x & 1) << bit;
            x >>= 1;
            sx <<= 1;
            bit += 1;
        }
        if sy < h {
            out |= (y & 1) << bit;
            y >>= 1;
            sy <<= 1;
            bit += 1;
        }
    }
    out
}

#[test]
fn swizzled_texels_land_at_their_interleaved_addresses() {
    for (w, h) in [(4u16, 4u16), (8, 2), (2, 8), (16, 16)] {
        // A8R8G8B8 texels whose blue byte is their stored index.
        let img = image(11, 0, [w, h, 1], 0, SWZ);
        let mut px = vec![0u8; img.pixel_size as usize];
        for i in 0..u32::from(w) * u32::from(h) {
            px[4 * i as usize..4 * i as usize + 4].copy_from_slice(&[i as u8, 0, 0, 255]);
        }
        let l = decode_image(&img, &px).unwrap();
        for y in 0..u32::from(h) {
            for x in 0..u32::from(w) {
                let at = 4 * (y * u32::from(w) + x) as usize;
                let want = morton(x, y, w.into(), h.into()) as u8;
                assert_eq!(l[0].rgba[at + 2], want, "{w}x{h} ({x}, {y})");
            }
        }
    }
}

/// DXT1 colour block: red and blue endpoints, texel i using code i % 4.
fn dxt1_block() -> [u8; 8] {
    let mut b = [0u8; 8];
    b[0..2].copy_from_slice(&0xf800u16.to_le_bytes());
    b[2..4].copy_from_slice(&0x001fu16.to_le_bytes());
    b[4..8].copy_from_slice(&0xe4e4_e4e4u32.to_le_bytes()); // codes 0,1,2,3 per row
    b
}

#[test]
fn dxt_blocks_decode() {
    let close = |a: u8, b: u8| a.abs_diff(b) <= 2;
    // DXT1: c0 > c1, so codes 2 and 3 are the 2/3 and 1/3 blends.
    let img = image(14, 0, [4, 4, 1], 0, DXT);
    let l = decode_image(&img, &dxt1_block()).unwrap();
    let px = l[0].rgba.as_chunks::<4>().0;
    assert_eq!(px[0], [255, 0, 0, 255]);
    assert_eq!(px[1], [0, 0, 255, 255]);
    assert!(close(px[2][0], 170) && close(px[2][2], 85), "{:?}", px[2]);
    assert!(close(px[3][0], 85) && close(px[3][2], 170), "{:?}", px[3]);
    // DXT3: explicit 4-bit alpha, texel i has alpha nibble i.
    let mut b = vec![0u8; 16];
    for (i, a) in b.iter_mut().take(8).enumerate() {
        *a = (((2 * i + 1) << 4) | (2 * i)) as u8;
    }
    b[8..].copy_from_slice(&dxt1_block());
    let l = decode_image(&image(15, 0, [4, 4, 1], 0, DXT), &b).unwrap();
    for (i, p) in l[0].rgba.as_chunks::<4>().0.iter().enumerate() {
        assert_eq!(p[3], (i as u8) * 17, "texel {i}");
    }
    // DXT5: alpha endpoints 255 and 0, every texel code 1 (= a1 = 0) but
    // texel 0, code 0 (= a0 = 255).
    let mut b = vec![255u8, 0];
    let codes: u64 = (1..16).fold(0, |c, i| c | 1 << (3 * i));
    b.extend_from_slice(&codes.to_le_bytes()[..6]);
    b.extend_from_slice(&dxt1_block());
    let l = decode_image(&image(16, 0, [4, 4, 1], 0, DXT), &b).unwrap();
    let px = l[0].rgba.as_chunks::<4>().0;
    assert_eq!((px[0][3], px[1][3], px[15][3]), (255, 0, 0));
}

#[test]
fn dxt_chains_stop_at_one_block_and_mips_follow_the_base() {
    // 16x16 DXT1 with 4 levels below: 16, 8, 4 stored (2x2 and 1x1 are not).
    let img = image(14, 0, [16, 16, 1], 4, DXT);
    assert_eq!(layout_len(&img).unwrap(), 128 + 32 + 8);
    // R5G6B5 2x2 with one mip: the 1x1 level follows the 4 base texels.
    let img = image(6, 0, [2, 2, 1], 1, SWZ);
    let mut px = vec![0u8; img.pixel_size as usize];
    px[8..10].copy_from_slice(&0x07e0u16.to_le_bytes());
    let l = decode_image(&img, &px).unwrap();
    assert_eq!(l.len(), 2);
    assert_eq!((l[1].width, &l[1].rgba[..]), (1, &[0u8, 255, 0, 255][..]));
}

#[test]
fn cube_faces_are_128_byte_aligned_and_stack_in_order() {
    let img = image(10, 2, [2, 2, 1], 0, SWZ);
    assert_eq!(layout_len(&img).unwrap(), 5 * 128 + 16);
    let mut px = vec![0u8; img.pixel_size as usize];
    for f in 0..6 {
        for t in 0..4 {
            px[f * 128 + 4 * t..f * 128 + 4 * t + 4].copy_from_slice(&[0, 0, f as u8 * 40, 0]);
        }
    }
    let l = decode_image(&img, &px).unwrap();
    assert_eq!(l.len(), 6);
    let s = stack(&l, 0).unwrap();
    assert_eq!((s.width, s.height), (2, 12));
    for f in 0..6 {
        assert_eq!(s.rgba[f * 16], f as u8 * 40, "face {f} row");
    }
    // a volume's slices stack the same way
    let img = image(10, 1, [2, 2, 4], 0, SWZ);
    let l = decode_image(&img, &vec![0u8; img.pixel_size as usize]).unwrap();
    assert_eq!(stack(&l, 0).unwrap().height, 8);
    assert!(stack(&l, 1).is_err(), "no such level");
}

#[test]
fn bad_images_are_refused() {
    let short = image(11, 0, [4, 4, 1], 0, SWZ);
    assert!(decode_image(&short, &[0u8; 63]).is_err());
    let white = Image {
        kind: ImageKind::White,
        ..short.clone()
    };
    assert!(decode_image(&white, &[0u8; 64]).is_err());
    // neither swizzled nor linear
    let neither = Image {
        flags: 1,
        ..short.clone()
    };
    assert!(decode_image(&neither, &[0u8; 64]).is_err());
}

#[test]
fn a_conversion_reads_back_through_halo_2s_decoders() {
    let cube = image(14, 2, [8, 8, 1], 1, DXT);
    let flat = image(11, 0, [4, 2, 1], 2, SWZ);
    let mut bm = Bitmap {
        kind: 3,
        encoding: 2,
        usage: 1,
        flags: 4,
        detail_fade: 0.25,
        sharpen: 0.5,
        bump_height: 2.0,
        sprite_budget: [1, 4],
        color_plate: [64, 32],
        blur: 1.5,
        alpha_bias: -0.5,
        mipmap_count: 3,
        sprite_usage: 1,
        sprite_spacing: 2,
        sequences: vec![Sequence {
            name: "smoke".into(),
            first_image: 0,
            image_count: 2,
            sprites: vec![Sprite {
                image: 1,
                bounds: [0.0, 1.0, 0.0, 0.5],
                registration: [2.0, 3.0],
            }],
        }],
        images: vec![cube, flat],
    };
    bm.images[1].registration = [-3, 7];
    let data: Vec<Vec<u8>> = bm
        .images
        .iter()
        .enumerate()
        .map(|(i, img)| {
            (0..img.pixel_size)
                .map(|b| (b as u8).wrapping_mul(31).wrapping_add(i as u8))
                .collect()
        })
        .collect();
    let (out, report) = convert_with(&bm, |i| Ok(data[i].clone())).unwrap();
    assert!(report.iter().all(|r| r.trimmed == 0));
    let mut set = TagSet::new(CacheType::Multiplayer, "h1bitmaps", "invented\\h1bitmaps");
    let id = out.install(&mut set, "invented\\smoke").unwrap();
    let (bytes, _) = set.build().unwrap();
    let mut m = Map::from_reader(Cursor::new(bytes.clone())).unwrap();
    let d = tags::DecodedTag::read(&mut m, id).unwrap();
    let tags::TagData::Bitmap(b) = d.data else {
        panic!("not a bitmap")
    };
    let h = &b.header;
    assert_eq!(
        (h.kind.0, h.encoding_preference.0, h.usage.0, h.flags.0),
        (3, 2, 1, 4)
    );
    assert_eq!(
        (h.detail_fade, h.sharpen, h.bump_scale, h.blur, h.alpha_bias),
        (0.25, 0.5, 2.0, 1.5, -0.5)
    );
    assert_eq!(
        (
            h.sprite_budget_class.0,
            h.sprite_budget,
            h.source_width,
            h.source_height
        ),
        (1, 4, 64, 32)
    );
    assert_eq!(
        (h.requested_mips, h.sprite_usage.0, h.sprite_spacing),
        (3, 1, 2)
    );
    let s = &b.sequences[0];
    assert_eq!(
        (&s.wire.label[..5], s.wire.first_image, s.wire.image_count),
        (&b"smoke"[..], 0, 2)
    );
    assert_eq!(s.sprites[0].image_index, 1);
    assert_eq!(s.sprites[0].bounds, [0.0, 1.0, 0.0, 0.5]);
    assert_eq!(s.sprites[0].registration, [2.0, 3.0]);
    for (i, img) in b.images.iter().enumerate() {
        let h1 = &bm.images[i];
        assert_eq!(img.format, tags::PixelFormat::from(h1.format.raw()));
        assert_eq!(img.dimensions, [h1.width, h1.height, h1.depth]);
        assert_eq!(
            (img.mip_count, img.wire.flags.0),
            (h1.mip_count, h1.flags | IMAGE_CACHED)
        );
        assert_eq!(
            (img.wire.registration_x, img.wire.registration_y),
            (h1.registration[0], h1.registration[1])
        );
        assert_eq!(img.wire.runtime_id, id.0);
        let lod = img.lods[0].unwrap();
        assert_eq!(
            (lod.pointer.file, lod.size),
            (ResourceFile::Local, h1.pixel_size)
        );
        let at = lod.pointer.offset as usize;
        assert_eq!(
            &bytes[at..at + lod.size as usize],
            &data[i][..],
            "bytes copied as they are"
        );
        let desc = assets::texture_desc(img, 0).unwrap();
        let len = desc.layout().unwrap().total_size;
        let h2 = assets::bitmap::decode_bytes(&desc, &bytes[at..at + len]).unwrap();
        let h1 = decode_image(h1, &data[i]).unwrap();
        assert_eq!(h2.len(), h1.len());
        assert!(h2.iter().zip(&h1).all(|(a, b)| a.rgba == b.rgba));
        assert!(img.lods[1].is_none() && img.lods[2].is_none());
    }
    // As on every retail image: LOD pointers 1 and 2 and the three texture
    // cache handles are none, so the texture cache loads the pixels
    // instead of drawing whatever its slot 0 holds.
    let tag = m.tag(id).unwrap().clone();
    let at = tag.location.unwrap().address;
    let p = m.read_tag_bytes(id).unwrap();
    let word = |o: usize| u32::from_le_bytes(p[o..o + 4].try_into().unwrap());
    let images = (word(0x48) - at) as usize;
    for i in 0..2 {
        for w in IMAGE_NONE_WORDS {
            assert_eq!(word(images + 0x74 * i + w), u32::MAX, "image {i} +{w:#x}");
        }
        // Flagged as a cache resource, as every retail image is, or the
        // texture cache never loads it.
        let flags = u16::from_le_bytes([p[images + 0x74 * i + 0xe], p[images + 0x74 * i + 0xf]]);
        assert_eq!(
            flags & IMAGE_CACHED,
            IMAGE_CACHED,
            "image {i} flags {flags:#x}"
        );
    }
}

#[test]
fn short_pixels_are_refused_and_long_ones_cut() {
    let img = image(11, 0, [4, 4, 1], 0, SWZ);
    let bm = |img: Image| Bitmap {
        kind: 0,
        encoding: 0,
        usage: 0,
        flags: 0,
        detail_fade: 0.0,
        sharpen: 0.0,
        bump_height: 0.0,
        sprite_budget: [0; 2],
        color_plate: [0; 2],
        blur: 0.0,
        alpha_bias: 0.0,
        mipmap_count: 0,
        sprite_usage: 0,
        sprite_spacing: 0,
        sequences: vec![],
        images: vec![img],
    };
    assert!(convert_with(&bm(img.clone()), |_| Ok(vec![0; 63])).is_err());
    let (_, r) = convert_with(&bm(img.clone()), |_| Ok(vec![0; 200])).unwrap();
    assert_eq!((r[0].lod_size, r[0].trimmed), (128, 72));
    let (_, r) = convert_with(&bm(img), |_| Ok(vec![0; 64])).unwrap();
    assert_eq!((r[0].lod_size, r[0].trimmed), (128, -64));
}
