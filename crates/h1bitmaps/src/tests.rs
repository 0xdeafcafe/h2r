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

/// An alpha block keeps its extremes exactly and every texel within half
/// a step of the 8-value mode, read back with Halo 2's decoder.
#[test]
fn an_alpha_block_reads_back_within_half_a_step() {
    use crate::mask::alpha_block;
    use gfx::dxt::{BcKind, decode_block};
    let a: [u8; 16] = std::array::from_fn(|k| ((k * 37) % 200 + 20) as u8);
    let mut block = [0u8; 16];
    block[..8].copy_from_slice(&alpha_block(&a));
    let back = decode_block(BcKind::Bc3, &block);
    let (hi, lo) = (*a.iter().max().unwrap(), *a.iter().min().unwrap());
    let step = u32::from(hi - lo) / 7 + 1;
    for (k, px) in back.iter().enumerate() {
        assert!(u32::from(px[3].abs_diff(a[k])) <= step / 2 + 1, "texel {k}: {} for {}", px[3], a[k]);
    }
    assert!(back.iter().any(|p| p[3] == hi) && back.iter().any(|p| p[3] == lo));
    // One value: both endpoints it, every index 0.
    assert_eq!(alpha_block(&[9; 16]), [9, 9, 0, 0, 0, 0, 0, 0]);
}

/// A DXT1 four-colour block decoded and encoded again is the same block
/// when its endpoints are both among its texels.
#[test]
fn a_colour_block_survives_a_round_trip() {
    use crate::mask::colour_block;
    use gfx::dxt::{BcKind, decode_block};
    let mut block = [0u8; 8];
    block[0..2].copy_from_slice(&0xf81fu16.to_le_bytes());
    block[2..4].copy_from_slice(&0x07e0u16.to_le_bytes());
    block[4..8].copy_from_slice(&0x1b4e_e4e4u32.to_le_bytes());
    let px = decode_block(BcKind::Bc1, &block);
    let rgb: [[u8; 3]; 16] = std::array::from_fn(|k| [px[k][0], px[k][1], px[k][2]]);
    assert_eq!(colour_block(&rgb), block);
    // As DXT5's colour half it reads back the same.
    let mut full = [0u8; 16];
    full[8..].copy_from_slice(&colour_block(&rgb));
    let again = decode_block(BcKind::Bc3, &full);
    for k in 0..16 {
        assert_eq!(again[k][..3], px[k][..3], "texel {k}");
    }
}

/// A DXT1 base with one mip level and an invented A8R8G8B8 mask: the
/// result is DXT5 of the same size and chain, its colours the base's, its
/// alpha the mask's red (level 0 within a DXT5 alpha step; level 1 the
/// mask sampled at every other texel, within a step).
#[test]
fn a_base_takes_its_masks_red_as_alpha() {
    use crate::decode::texture_desc;
    use crate::mask::red_into_alpha;
    let base = image(14, 0, [8, 8, 1], 1, DXT);
    let mut block = [0u8; 8];
    block[0..2].copy_from_slice(&0xf800u16.to_le_bytes());
    block[2..4].copy_from_slice(&0x001fu16.to_le_bytes());
    block[4..8].copy_from_slice(&0xe4e4_e4e4u32.to_le_bytes());
    let base_px: Vec<u8> = block.iter().copied().cycle().take(base.pixel_size as usize).collect();
    let mask = image(11, 0, [8, 8, 1], 0, LIN);
    let pitch = texture_desc(&mask).unwrap().pitch as usize;
    let mut mask_px = vec![0u8; mask.pixel_size as usize];
    for y in 0..8 {
        for x in 0..8 {
            // B, G, R, A: red ramps along x, green and blue are noise.
            mask_px[y * pitch + x * 4..][..4].copy_from_slice(&[0x55, 0xaa, (x * 32) as u8, 0x10]);
        }
    }
    let (img, px) = red_into_alpha(&base, &base_px, &mask, &mask_px).unwrap();
    assert_eq!(img.format, h1map::bitmap::PixelFormat::Dxt5);
    assert_eq!((img.width, img.height, img.mip_count), (8, 8, 1));
    assert_eq!(img.flags & h1map::bitmap::FLAG_COMPRESSED, h1map::bitmap::FLAG_COMPRESSED);
    assert_eq!(img.pixel_size as usize, px.len());
    let want = decode_image(&base, &base_px).unwrap();
    let got = decode_image(&img, &px).unwrap();
    assert_eq!(got.len(), 2);
    for (g, w) in got.iter().zip(&want) {
        assert_eq!((g.width, g.height), (w.width, w.height));
        for t in 0..(g.width * g.height) as usize {
            assert_eq!(g.rgba[t * 4..t * 4 + 3], w.rgba[t * 4..t * 4 + 3], "level {} texel {t}", g.level);
            let x = t as u32 % g.width;
            let red = (x * (8 / g.width) * 32) as u8;
            let a = g.rgba[t * 4 + 3];
            // Within half a step of the block's 8-value mode (its range
            // over 14): 96 over level 0's blocks, 192 over level 1's.
            let tolerance = if g.width == 8 { 8 } else { 15 };
            assert!(a.abs_diff(red) <= tolerance, "level {} texel {t}: alpha {a}, red {red}", g.level);
        }
    }
    // The same inputs give the same bytes.
    assert_eq!(red_into_alpha(&base, &base_px, &mask, &mask_px).unwrap().1, px);
}

/// A DXT1 image (red and blue endpoints, each row the four palette
/// entries): the result keeps its colours exactly and takes `gain` times
/// each texel's brightest channel as alpha (255, 255, 170, 170 along a
/// row; at gain 0.5 half that; at gain 2 saturated), within a DXT5 alpha
/// step.
#[test]
fn an_image_takes_its_brightest_channel_as_alpha() {
    use crate::mask::{brightest, brightest_into_alpha};
    assert_eq!(brightest(10, 200, 30, 1.0), 200);
    assert_eq!(brightest(10, 200, 30, 0.5), 100);
    assert_eq!(brightest(10, 200, 30, 2.0), 255);
    let base = image(14, 0, [8, 8, 1], 1, DXT);
    let mut block = [0u8; 8];
    block[0..2].copy_from_slice(&0xf800u16.to_le_bytes());
    block[2..4].copy_from_slice(&0x001fu16.to_le_bytes());
    block[4..8].copy_from_slice(&0xe4e4_e4e4u32.to_le_bytes());
    let px: Vec<u8> = block.iter().copied().cycle().take(base.pixel_size as usize).collect();
    let want = decode_image(&base, &px).unwrap();
    for gain in [1.0f32, 0.5, 2.0] {
        let (img, out) = brightest_into_alpha(&base, &px, gain).unwrap();
        assert_eq!(img.format, h1map::bitmap::PixelFormat::Dxt5);
        assert_eq!((img.width, img.height, img.mip_count), (8, 8, 1));
        assert_eq!(img.pixel_size as usize, out.len());
        let got = decode_image(&img, &out).unwrap();
        assert_eq!(got.len(), 2);
        for (g, w) in got.iter().zip(&want) {
            for t in 0..(g.width * g.height) as usize {
                let c = &w.rgba[t * 4..t * 4 + 3];
                assert_eq!(&g.rgba[t * 4..t * 4 + 3], c, "gain {gain} level {} texel {t}", g.level);
                let a = g.rgba[t * 4 + 3];
                let expect = brightest(c[0], c[1], c[2], gain);
                // Half a step of the block's 8-value mode over its range
                // (at most 255 / 7 / 2).
                assert!(a.abs_diff(expect) <= 19, "gain {gain} level {} texel {t}: {a} vs {expect}", g.level);
            }
        }
        assert_eq!(brightest_into_alpha(&base, &px, gain).unwrap().1, out);
    }
}

/// A Halo 1 multipurpose map's blue (invented A8R8G8B8) as a Halo 2
/// change-colour map: DXT5 of the same size, the blue in the channel asked
/// for (red: change colour 0), every other channel, alpha included, 0 (a
/// non-zero alpha would tint by change colour 3 as well). Each 4x4 block
/// is uniform here, so the values come back exactly.
#[test]
fn a_multipurpose_blue_becomes_one_change_colour_channel() {
    use crate::decode::texture_desc;
    use crate::mask::channel_into;
    let mask = image(11, 0, [8, 8, 1], 0, LIN);
    let pitch = texture_desc(&mask).unwrap().pitch as usize;
    let mut px = vec![0u8; mask.pixel_size as usize];
    for y in 0..8 {
        for x in 0..8 {
            // B, G, R, A: blue is the mask (left half full, right empty),
            // red and green are the reflection and glow masks.
            let b = if x < 4 { 0xff } else { 0 };
            px[y * pitch + x * 4..][..4].copy_from_slice(&[b, 0x40, 0x80, 0xff]);
        }
    }
    for to in 0..4 {
        let (img, out) = channel_into(&mask, &px, 2, to).unwrap();
        assert_eq!(img.format, h1map::bitmap::PixelFormat::Dxt5);
        assert_eq!((img.width, img.height), (8, 8));
        assert_eq!(img.pixel_size as usize, out.len());
        let got = decode_image(&img, &out).unwrap();
        assert_eq!(got.len(), 1);
        for t in 0..64 {
            let want = if t % 8 < 4 { 0xff } else { 0 };
            for c in 0..4 {
                let w = if c == to { want } else { 0 };
                assert_eq!(got[0].rgba[t * 4 + c], w, "to {to} texel {t} channel {c}");
            }
        }
        assert_eq!(channel_into(&mask, &px, 2, to).unwrap().1, out);
    }
    assert!(channel_into(&mask, &px, 2, 4).is_err());
}

/// A DXT3 leaf base (8x8, one mip) whose four level-0 blocks are clear,
/// alpha 119, alpha 136 and opaque: its cut-out bump is DXT5 of the same
/// size and chain, every texel the flat normal (within a step of
/// `default_vector`'s 128, 128, 255), its alpha 0 below 128 and 255 from
/// it, exactly; level 1 (the clear block again) is all 0.
#[test]
fn a_cut_out_base_gives_a_flat_bump_with_its_binary_alpha() {
    use crate::mask::{CUTOUT_THRESHOLD, cutout_bump};
    const DXT3: u16 = 15;
    let base = image(DXT3, 0, [8, 8, 1], 1, DXT);
    let block = |nibble: u8| {
        let mut b = [0u8; 16];
        b[..8].fill(nibble << 4 | nibble);
        // A green DXT1 colour half, as Halo 1's leaves are.
        b[8..10].copy_from_slice(&0x07e0u16.to_le_bytes());
        b[10..12].copy_from_slice(&0x0200u16.to_le_bytes());
        b
    };
    let px: Vec<u8> = [0x0, 0x7, 0x8, 0xf]
        .into_iter()
        .flat_map(block)
        .cycle()
        .take(base.pixel_size as usize)
        .collect();
    let (img, out) = cutout_bump(&base, &px).unwrap();
    assert_eq!(img.format, h1map::bitmap::PixelFormat::Dxt5);
    assert_eq!((img.width, img.height, img.mip_count), (8, 8, 1));
    assert_eq!(img.pixel_size as usize, out.len());
    let want = decode_image(&base, &px).unwrap();
    let got = decode_image(&img, &out).unwrap();
    assert_eq!(got.len(), 2);
    for (g, w) in got.iter().zip(&want) {
        assert_eq!((g.width, g.height), (w.width, w.height));
        for t in 0..(g.width * g.height) as usize {
            let n = &g.rgba[t * 4..t * 4 + 3];
            assert!(
                n[0].abs_diff(128) <= 1 && n[1].abs_diff(128) <= 1 && n[2] == 255,
                "level {} texel {t}: normal {n:?}",
                g.level
            );
            let a = w.rgba[t * 4 + 3];
            let cut = if a >= CUTOUT_THRESHOLD { 255 } else { 0 };
            assert_eq!(
                g.rgba[t * 4 + 3],
                cut,
                "level {} texel {t}: base alpha {a}",
                g.level
            );
        }
    }
    // The threshold sits between the second and third blocks.
    let top = &got[0];
    let alpha_at = |x: u32, y: u32| top.rgba[((y * 8 + x) * 4 + 3) as usize];
    assert_eq!(
        [
            alpha_at(0, 0),
            alpha_at(4, 0),
            alpha_at(0, 4),
            alpha_at(4, 4)
        ],
        [0, 0, 255, 255]
    );
    assert!(got[1].rgba.chunks_exact(4).all(|t| t[3] == 0));
    assert_eq!(cutout_bump(&base, &px).unwrap().1, out);
}

/// A picture made opaque keeps its colour and samples alpha 255 in every
/// format with an alpha it can set, through the swizzled, linear and DXT
/// paths; the formats whose alpha it can't set are left as they are.
#[test]
fn an_opaque_copy_keeps_the_colour_and_drops_the_alpha() {
    use crate::convert::force_opaque;
    use h1map::bitmap::PixelFormat;
    let decode = |format: u16, flags: u16, px: &[u8]| {
        let img = image(format, 0, [1, 1, 1], 0, flags);
        let mut b = vec![0u8; img.pixel_size as usize];
        b[..px.len()].copy_from_slice(px);
        let before: [u8; 4] = decode_image(&img, &b).unwrap()[0].rgba[..4]
            .try_into()
            .unwrap();
        let done = force_opaque(PixelFormat::from_raw(format), &mut b);
        let after: [u8; 4] = decode_image(&img, &b).unwrap()[0].rgba[..4]
            .try_into()
            .unwrap();
        (before, after, done)
    };
    for (format, texel) in [
        (0u16, &[0x10u8][..]),   // A8
        (8, &[0xe0, 0x03][..]),  // A1R5G5B5, alpha bit clear
        (9, &[0x0f, 0x30][..]),  // A4R4G4B4, alpha 3
        (10, &[1, 2, 3, 0][..]), // X8R8G8B8
        (11, &[1, 2, 3, 4][..]), // A8R8G8B8, Blood Gulch's clear blue sky
    ] {
        let (before, after, done) = decode(format, SWZ, texel);
        assert!(done, "format {format}");
        assert_eq!(after[3], 255, "format {format}");
        assert_eq!(after[..3], before[..3], "format {format}: colour kept");
    }
    // Linear rows: the padding is touched too, harmlessly.
    let img = image(11, 0, [3, 2, 1], 0, LIN);
    let mut px = vec![0u8; img.pixel_size as usize];
    px[64..68].copy_from_slice(&[9, 8, 7, 0]);
    assert!(force_opaque(PixelFormat::A8R8G8B8, &mut px));
    let l = decode_image(&img, &px).unwrap();
    assert_eq!(&l[0].rgba[12..16], &[7, 8, 9, 255]);
    // DXT3 and DXT5: every texel's alpha 255, the colour block untouched.
    for format in [15u16, 16] {
        // Alpha bytes that aren't zero: DXT5 codes 6 (0 with equal
        // endpoints) everywhere.
        let mut b = vec![0x5au8, 0x5a, 0xb6, 0x6d, 0xdb, 0xb6, 0x6d, 0xdb];
        b.extend_from_slice(&dxt1_block());
        let img = image(format, 0, [4, 4, 1], 0, DXT);
        let before = decode_image(&img, &b).unwrap()[0].rgba.clone();
        assert!(force_opaque(PixelFormat::from_raw(format), &mut b));
        let after = decode_image(&img, &b).unwrap()[0].rgba.clone();
        for (p, q) in before
            .as_chunks::<4>()
            .0
            .iter()
            .zip(after.as_chunks::<4>().0)
        {
            assert_eq!((q[3], &q[..3]), (255, &p[..3]), "format {format}");
        }
    }
    // Not settable without re-encoding: left alone.
    for format in [2u16, 3, 14] {
        let mut b = vec![0x40u8; 8];
        let keep = b.clone();
        assert!(
            !force_opaque(PixelFormat::from_raw(format), &mut b),
            "format {format}"
        );
        assert_eq!(b, keep);
    }
    // No alpha to set.
    assert!(force_opaque(PixelFormat::Y8, &mut [0x80]));
}
