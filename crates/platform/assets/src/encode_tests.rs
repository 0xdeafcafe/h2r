use super::*;
use crate::bitmap::decode_bytes;
use crate::resource::ResourceFiles;
use crate::write::{Class, Edit, Reshape, plan_image, reshaped};
use map::{RawPointer, ResourceFile};
use std::sync::Arc;
use tags::records::BitmapImage;
use tags::wire::{BinaryRecord, Flags16};
use tags::{PixelFormat, ResourceRange, TextureKind as TagKind};

const SWZ: u16 = 0x8 | 1;
const LIN: u16 = 0x10;
const DXT: u16 = 0x2 | 1;

/// An image whose LOD 0 (and LOD 1 and 2 when `lods` says so) lie in the
/// local file at 0x800, 0x80000 and 0x90000.
fn image(
    format: PixelFormat,
    kind: TagKind,
    dims: [u16; 3],
    mips: u16,
    flags: u16,
    lods: usize,
) -> Image {
    let mut wire = BitmapImage::decode(&[0u8; BitmapImage::SIZE]).unwrap();
    wire.flags = Flags16(flags);
    let mut img = Image {
        format,
        kind,
        dimensions: dims,
        mip_count: mips,
        lods: [None; 3],
        low_detail: None,
        wire,
    };
    for lod in 0..lods {
        let size = texture_desc(&img, lod as u32)
            .unwrap()
            .layout()
            .unwrap()
            .total_size
            .next_multiple_of(LOD_ALIGN) as u32;
        img.lods[lod] = Some(ResourceRange {
            pointer: RawPointer {
                file: ResourceFile::Local,
                offset: 0x800 + 0x8_0000 * lod as u32,
            },
            size,
        });
    }
    img
}

/// A smooth RGBA picture.
fn gradient(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::new();
    for y in 0..h {
        for x in 0..w {
            v.extend_from_slice(&[
                (x * 255 / w.max(1)) as u8,
                (y * 255 / h.max(1)) as u8,
                ((x + y) * 255 / (w + h)) as u8,
                255 - (x * 255 / w.max(1)) as u8,
            ]);
        }
    }
    v
}

fn planes<'a>(img: &Image, data: &'a [&'a [u8]]) -> Planes<'a> {
    let depth = if img.kind == TagKind::D3 {
        u32::from(img.dimensions[2])
    } else {
        1
    };
    Planes {
        width: u32::from(img.dimensions[0]),
        height: u32::from(img.dimensions[1]),
        depth,
        planes: data,
    }
}

fn decode(img: &Image, lod: u32, bytes: &[u8]) -> Vec<crate::bitmap::Level> {
    decode_bytes(&texture_desc(img, lod).unwrap(), bytes).unwrap()
}

/// The part of the file a LOD occupies, so the apply of spans is checkable.
fn apply(bytes: &mut [u8], spans: &[Span], lod: u32) {
    for s in spans.iter().filter(|s| s.lod == lod) {
        bytes[s.offset..s.offset + s.bytes.len()].copy_from_slice(&s.bytes);
    }
}

#[test]
fn a_swizzled_level_is_the_morton_order_of_its_texels_exactly() {
    let img = image(PixelFormat::Argb8888, TagKind::D2, [8, 8, 1], 0, SWZ, 1);
    let px = gradient(8, 8);
    let bytes = encode_lod(&img, 0, &planes(&img, &[&px])).unwrap();
    let bgra: Vec<u8> = px
        .chunks(4)
        .flat_map(|p| [p[2], p[1], p[0], p[3]])
        .collect();
    let want = gfx::swizzle::swizzle(&bgra, 8, 8, 1, 4).unwrap();
    assert_eq!(&bytes[..want.len()], &want[..]);
    // The LOD is its layout rounded up to 128.
    assert_eq!(bytes.len(), 256);
}

#[test]
fn a_linear_level_pads_each_row_to_the_pitch() {
    let img = image(PixelFormat::Argb8888, TagKind::D2, [21, 5, 1], 0, LIN, 1);
    let px = gradient(21, 5);
    let bytes = encode_lod(&img, 0, &planes(&img, &[&px])).unwrap();
    // 21 * 4 = 84 -> pitch 128.
    assert_eq!(bytes.len(), (128 * 5usize).next_multiple_of(LOD_ALIGN));
    for y in 0..5usize {
        for x in 0..21usize {
            let p = &px[(y * 21 + x) * 4..][..4];
            assert_eq!(
                &bytes[y * 128 + x * 4..][..4],
                &[p[2], p[1], p[0], p[3]],
                "({x},{y})"
            );
        }
        assert!(
            bytes[y * 128 + 84..(y + 1) * 128].iter().all(|b| *b == 0),
            "row {y} padding"
        );
    }
}

#[test]
fn uncompressed_formats_round_trip_through_the_decoder_exactly() {
    for (fmt, kind_flags) in [
        (PixelFormat::A8, SWZ),
        (PixelFormat::Y8, SWZ),
        (PixelFormat::Ay8, SWZ),
        (PixelFormat::A8Y8, SWZ),
        (PixelFormat::Rgb565, SWZ),
        (PixelFormat::Argb1555, SWZ),
        (PixelFormat::Argb4444, SWZ),
        (PixelFormat::Xrgb8888, SWZ),
        (PixelFormat::Argb8888, SWZ),
        (PixelFormat::Rgb565, LIN),
        (PixelFormat::Argb8888, LIN),
    ] {
        let mips = if kind_flags == LIN { 0 } else { 3 };
        let img = image(fmt, TagKind::D2, [16, 8, 1], mips, kind_flags, 1);
        let px = gradient(16, 8);
        let first = encode_lod(&img, 0, &planes(&img, &[&px])).unwrap();
        // decode -> encode -> decode is the identity on what the format can hold
        let a = decode(&img, 0, &first);
        let pl: Vec<&[u8]> = vec![a[0].rgba.as_slice()];
        let second = encode_lod(&img, 0, &planes(&img, &pl)).unwrap();
        let b = decode(&img, 0, &second);
        // (the mips below are filtered from different bases)
        assert_eq!(a[0].rgba, b[0].rgba, "{fmt:?} {kind_flags:#x}");
        assert_eq!(a.len() as u16, mips + 1, "{fmt:?}");
    }
}

#[test]
fn a_grey_value_survives_luma_formats() {
    for fmt in [PixelFormat::Y8, PixelFormat::Ay8] {
        let img = image(fmt, TagKind::D2, [4, 4, 1], 0, SWZ, 1);
        let px: Vec<u8> = (0..16u8)
            .flat_map(|i| [i * 16, i * 16, i * 16, 255])
            .collect();
        let l = decode(
            &img,
            0,
            &encode_lod(&img, 0, &planes(&img, &[&px])).unwrap(),
        );
        for i in 0..16usize {
            assert_eq!(l[0].rgba[i * 4], (i * 16) as u8, "{fmt:?}");
        }
    }
}

#[test]
fn dxt_blocks_round_trip_within_tolerance() {
    for (fmt, color_tol, alpha_tol) in [
        (PixelFormat::Dxt1, 40, 0),
        (PixelFormat::Dxt3, 40, 9),
        (PixelFormat::Dxt5, 40, 12),
    ] {
        let img = image(fmt, TagKind::D2, [32, 16, 1], 5, DXT, 1);
        let mut px = gradient(32, 16);
        if fmt == PixelFormat::Dxt1 {
            px.chunks_exact_mut(4).for_each(|p| p[3] = 255);
        }
        let bytes = encode_lod(&img, 0, &planes(&img, &[&px])).unwrap();
        let l = decode(&img, 0, &bytes);
        let base = &l[0];
        assert_eq!((base.width, base.height), (32, 16));
        for (got, want) in base.rgba.chunks(4).zip(px.chunks(4)) {
            for c in 0..3 {
                assert!(
                    got[c].abs_diff(want[c]) <= color_tol,
                    "{fmt:?} colour {got:?} vs {want:?}"
                );
            }
            assert!(
                got[3].abs_diff(want[3]) <= alpha_tol,
                "{fmt:?} alpha {got:?} vs {want:?}"
            );
        }
        // every stored level decodes (32x16 down to one block: 5 levels)
        assert_eq!(l.len(), 4);
    }
}

#[test]
fn a_solid_dxt_block_is_one_colour_and_dxt1_keeps_cut_outs() {
    let img = image(PixelFormat::Dxt1, TagKind::D2, [4, 4, 1], 0, DXT, 1);
    // Half the block transparent, half red.
    let px: Vec<u8> = (0..16)
        .flat_map(|i| {
            if i < 8 {
                [255, 0, 0, 255]
            } else {
                [0, 0, 0, 0]
            }
        })
        .collect();
    let l = decode(
        &img,
        0,
        &encode_lod(&img, 0, &planes(&img, &[&px])).unwrap(),
    );
    for (i, p) in l[0].rgba.chunks(4).enumerate() {
        if i < 8 {
            assert_eq!(p[3], 255);
            assert!(p[0] >= 247 && p[1] <= 4 && p[2] <= 8, "{p:?}");
        } else {
            assert_eq!(p[3], 0, "texel {i}");
        }
    }
}

#[test]
fn cube_faces_sit_a_face_stride_apart() {
    let img = image(PixelFormat::Argb8888, TagKind::Cube, [4, 4, 1], 2, SWZ, 1);
    let faces: Vec<Vec<u8>> = (0..6u8)
        .map(|f| {
            (0..16)
                .flat_map(|_| [f * 10, f * 10 + 1, f * 10 + 2, 255])
                .collect()
        })
        .collect();
    let refs: Vec<&[u8]> = faces.iter().map(Vec::as_slice).collect();
    let bytes = encode_lod(&img, 0, &planes(&img, &refs)).unwrap();
    let stride = texture_desc(&img, 0).unwrap().layout().unwrap().face_stride;
    assert_eq!(stride, 128);
    for (f, face) in faces.iter().enumerate() {
        // texel (0, 0) of mip 0 is the first four bytes of the face
        assert_eq!(
            &bytes[f * stride..f * stride + 4],
            &[face[2], face[1], face[0], 255]
        );
    }
    let l = decode(&img, 0, &bytes);
    assert_eq!(l.len(), 18);
    assert_eq!(l[3 * 3].face, 3);
    assert_eq!(&l[3 * 3].rgba[..4], &[30, 31, 32, 255]);
}

#[test]
fn a_volume_chain_averages_its_slices() {
    let img = image(PixelFormat::Xrgb8888, TagKind::D3, [4, 4, 4], 2, SWZ, 1);
    let slices: Vec<Vec<u8>> = (0..4u8)
        .map(|z| (0..16).flat_map(|_| [z * 60, 0, 0, 255]).collect())
        .collect();
    let refs: Vec<&[u8]> = slices.iter().map(Vec::as_slice).collect();
    let l = decode(&img, 0, &encode_lod(&img, 0, &planes(&img, &refs)).unwrap());
    // 4x4x4, 2x2x2, 1x1x1; level 1's first slice is the mean of slices 0 and 1.
    assert_eq!(l.len(), 3);
    assert_eq!(l[1].rgba[0], 30);
    assert_eq!(l[1].rgba[2 * 2 * 4], 150);
    assert_eq!(l[2].rgba[0], 90);
}

#[test]
fn a_painted_corner_changes_only_its_blocks_and_their_mips() {
    for fmt in [PixelFormat::Dxt1, PixelFormat::Dxt5] {
        let img = image(fmt, TagKind::D2, [64, 64, 1], 6, DXT, 3);
        let mut px = gradient(64, 64);
        let before: Vec<Vec<u8>> = (0..3)
            .map(|l| encode_lod(&img, l, &planes(&img, &[&px])).unwrap())
            .collect();
        // Paint the top left 3x3 pixels.
        for y in 0..3usize {
            for x in 0..3usize {
                px[(y * 64 + x) * 4..][..4].copy_from_slice(&[255, 0, 255, 255]);
            }
        }
        let dirty = [Dirty {
            face: 0,
            slice: 0,
            rect: PixelRect::new(0, 0, 3, 3),
        }];
        let spans = encode_tiles(&img, &planes(&img, &[&px]), &dirty).unwrap();
        let bb = if fmt == PixelFormat::Dxt1 { 8 } else { 16 };
        // LOD 0 stores 64, 32, 16, 8, 4: five levels, one block each.
        // LOD 1 stores 32..4 (4 levels), LOD 2 stores 16..4 (3 levels).
        for (lod, levels) in [(0u32, 5usize), (1, 4), (2, 3)] {
            let mine: Vec<&Span> = spans.iter().filter(|s| s.lod == lod).collect();
            let blocks: usize = mine.iter().map(|s| s.bytes.len() / bb).sum();
            assert_eq!(blocks, levels, "{fmt:?} LOD {lod}");
        }
        // Writing them over the old bytes is the whole chain encoded afresh,
        // and every byte outside a span is the old byte.
        for lod in 0..3u32 {
            let mut bytes = before[lod as usize].clone();
            apply(&mut bytes, &spans, lod);
            assert_eq!(
                bytes,
                encode_lod(&img, lod, &planes(&img, &[&px])).unwrap(),
                "{fmt:?} LOD {lod}"
            );
            let changed = bytes
                .iter()
                .zip(&before[lod as usize])
                .filter(|(a, b)| a != b)
                .count();
            let covered: usize = spans
                .iter()
                .filter(|s| s.lod == lod)
                .map(|s| s.bytes.len())
                .sum();
            assert!(changed <= covered);
        }
    }
}

#[test]
fn a_painted_texel_of_a_swizzled_image_changes_only_its_texels() {
    let img = image(PixelFormat::Argb8888, TagKind::D2, [16, 16, 1], 4, SWZ, 1);
    let mut px = gradient(16, 16);
    let before = encode_lod(&img, 0, &planes(&img, &[&px])).unwrap();
    px[(5 * 16 + 9) * 4..][..4].copy_from_slice(&[1, 2, 3, 4]);
    let dirty = [Dirty {
        face: 0,
        slice: 0,
        rect: PixelRect::new(9, 5, 1, 1),
    }];
    let spans = encode_tiles(&img, &planes(&img, &[&px]), &dirty).unwrap();
    // One texel at each of the five levels, 4 bytes each.
    assert_eq!(spans.iter().map(|s| s.bytes.len()).sum::<usize>(), 5 * 4);
    let mut bytes = before.clone();
    apply(&mut bytes, &spans, 0);
    assert_eq!(bytes, encode_lod(&img, 0, &planes(&img, &[&px])).unwrap());
    // Nothing in the unpainted bytes moved.
    let touched: Vec<usize> = spans
        .iter()
        .flat_map(|s| s.offset..s.offset + s.bytes.len())
        .collect();
    for (i, (a, b)) in bytes.iter().zip(&before).enumerate() {
        assert!(a == b || touched.contains(&i), "byte {i}");
    }
}

#[test]
fn tiles_outside_the_image_or_a_bad_plane_count_are_errors() {
    let img = image(PixelFormat::Argb8888, TagKind::Cube, [4, 4, 1], 0, SWZ, 1);
    let face = gradient(4, 4);
    let one: [&[u8]; 1] = [face.as_slice()];
    let pl = planes(&img, &one);
    assert!(
        encode_tiles(&img, &pl, &[]).is_err(),
        "a cube needs six planes"
    );
    let refs: Vec<&[u8]> = (0..6).map(|_| face.as_slice()).collect();
    let pl = planes(&img, &refs);
    assert!(
        encode_tiles(
            &img,
            &pl,
            &[Dirty {
                face: 6,
                slice: 0,
                rect: PixelRect::new(0, 0, 1, 1)
            }]
        )
        .is_err()
    );
    // a rectangle off the image writes nothing
    let spans = encode_tiles(
        &img,
        &pl,
        &[Dirty {
            face: 0,
            slice: 0,
            rect: PixelRect::new(50, 50, 4, 4),
        }],
    )
    .unwrap();
    assert!(spans.is_empty());
    // data formats are not pictures
    let p8 = image(PixelFormat::P8, TagKind::D2, [4, 4, 1], 0, SWZ, 1);
    assert!(encode_lod(&p8, 0, &planes(&p8, &[&face])).is_err());
}

/// A file with `plan`'s appended blocks after 0x1000, the image's LOD
/// pointers and sizes patched as the plan says, and the image read back.
fn read_back(plan: &crate::write::Plan, image_at: u32, new: &Image) -> crate::bitmap::DecodedImage {
    let mut file = vec![0u8; 0x1000];
    let mut img = new.clone();
    let mut sizes = [None::<u32>; 3];
    for (addr, bytes) in &plan.tag {
        for n in 0..3u32 {
            if *addr == image_at + 0x34 + 4 * n {
                sizes[n as usize] = Some(u32::from_le_bytes(bytes[..4].try_into().unwrap()));
            }
        }
    }
    img.lods = [None; 3];
    for a in &plan.appended {
        let n = ((a.pointer_address - image_at - 0x1c) / 4) as usize;
        let offset = file.len().next_multiple_of(a.align as usize);
        file.resize(offset, 0);
        file.extend_from_slice(&a.bytes);
        img.lods[n] = Some(ResourceRange {
            pointer: RawPointer {
                file: ResourceFile::Local,
                offset: offset as u32,
            },
            size: sizes[n].unwrap(),
        });
    }
    let mut files = ResourceFiles::new();
    files.set_bytes(ResourceFile::Local, Arc::from(file));
    crate::bitmap::decode_image(&img, &mut files).unwrap()
}

#[test]
fn a_resized_image_reads_back_with_every_level() {
    let img = image(PixelFormat::Argb8888, TagKind::D2, [16, 16, 1], 4, SWZ, 3);
    let (w, h) = (64u32, 32u32);
    let px: Vec<u8> = (0..w * h).flat_map(|_| [200, 100, 50, 255]).collect();
    let pl = Planes {
        width: w,
        height: h,
        depth: 1,
        planes: &[&px],
    };
    let mut files = ResourceFiles::new();
    for format in [
        PixelFormat::Argb8888,
        PixelFormat::Dxt1,
        PixelFormat::Rgb565,
    ] {
        let edit = Edit {
            image: &img,
            image_at: 0x4000,
            planes: pl,
            dirty: &[],
            reshape: Some(Reshape {
                width: w,
                height: h,
                format,
            }),
        };
        let plan = plan_image(&edit, &mut files).unwrap();
        assert_eq!(plan.class, Class::Appended);
        let (new, fields) = reshaped(
            &img,
            Reshape {
                width: w,
                height: h,
                format,
            },
        )
        .unwrap();
        assert_eq!(
            (new.dimensions[0], new.dimensions[1], new.format),
            (64, 32, format)
        );
        // size, format, flags and mips are patched in the record
        assert_eq!(fields.len(), 4);
        // all three LODs the image stored are written
        assert_eq!(plan.appended.len(), 3, "{format:?}");
        let d = read_back(&plan, 0x4000, &new);
        assert!(d.size_matches(), "{format:?}");
        // 64x32 has 6 levels (to 2x1... 7 with 1x1); LOD 0 reads them all
        let levels = u32::from(new.mip_count) + 1;
        let stored = if format == PixelFormat::Dxt1 {
            crate::bitmap::stored_dxt_levels(w, h)
        } else {
            levels
        };
        assert_eq!(d.levels.len() as u32, stored, "{format:?}");
        assert_eq!((d.levels[0].width, d.levels[0].height), (64, 32));
        for l in &d.levels {
            for p in l.rgba.chunks(4) {
                assert!(
                    p[0].abs_diff(200) <= 8
                        && p[1].abs_diff(100) <= 4
                        && p[2].abs_diff(50) <= 8
                        && p[3] == 255,
                    "{format:?} {p:?}"
                );
            }
        }
    }
}

#[test]
fn a_size_that_is_not_a_power_of_two_is_linear_and_drops_mips() {
    let img = image(PixelFormat::Argb8888, TagKind::D2, [16, 16, 1], 4, SWZ, 1);
    let (new, _) = reshaped(
        &img,
        Reshape {
            width: 21,
            height: 9,
            format: PixelFormat::Argb8888,
        },
    )
    .unwrap();
    assert!(new.is_linear() && !new.is_swizzled());
    assert_eq!(new.mip_count, 0);
    assert!(
        reshaped(
            &img,
            Reshape {
                width: 21,
                height: 9,
                format: PixelFormat::Dxt1
            }
        )
        .is_err()
    );
    assert!(
        reshaped(
            &img,
            Reshape {
                width: 8,
                height: 8,
                format: PixelFormat::P8
            }
        )
        .is_err()
    );
    let cube = image(PixelFormat::Argb8888, TagKind::Cube, [8, 8, 1], 0, SWZ, 1);
    assert!(
        reshaped(
            &cube,
            Reshape {
                width: 8,
                height: 4,
                format: PixelFormat::Argb8888
            }
        )
        .is_err()
    );
}

#[test]
fn a_shared_cache_lod_is_copied_with_only_the_painted_blocks_changed() {
    let mut img = image(PixelFormat::Dxt1, TagKind::D2, [16, 16, 1], 2, DXT, 1);
    let mut px = gradient(16, 16);
    px.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    let stored = encode_lod(&img, 0, &planes(&img, &[&px])).unwrap();
    // The pixels live in shared.map at 0x300.
    let mut shared = vec![0u8; 0x300];
    shared.extend_from_slice(&stored);
    let mut files = ResourceFiles::new();
    files.set_bytes(ResourceFile::Shared, Arc::from(shared));
    img.lods[0] = Some(ResourceRange {
        pointer: RawPointer {
            file: ResourceFile::Shared,
            offset: 0x300,
        },
        size: stored.len() as u32,
    });
    px[0..4].copy_from_slice(&[0, 255, 0, 255]);
    let dirty = [Dirty {
        face: 0,
        slice: 0,
        rect: PixelRect::new(0, 0, 1, 1),
    }];
    let plan = plan_image(
        &Edit {
            image: &img,
            image_at: 0x4000,
            planes: planes(&img, &[&px]),
            dirty: &dirty,
            reshape: None,
        },
        &mut files,
    )
    .unwrap();
    assert_eq!(plan.class, Class::Appended);
    assert!(plan.raw.is_empty() && plan.tag.is_empty());
    let [copy] = &plan.appended[..] else {
        panic!("one copy")
    };
    assert_eq!(copy.pointer_address, 0x4000 + 0x1c);
    assert_eq!(copy.bytes.len(), stored.len());
    let differs = copy
        .bytes
        .chunks(8)
        .zip(stored.chunks(8))
        .filter(|(a, b)| a != b)
        .count();
    assert!((1..=3).contains(&differs), "{differs} blocks changed");
    assert_eq!(
        copy.bytes,
        encode_lod(&img, 0, &planes(&img, &[&px])).unwrap()
    );
}

#[test]
fn a_local_lod_is_patched_in_place() {
    let img = image(PixelFormat::Dxt1, TagKind::D2, [16, 16, 1], 2, DXT, 1);
    let px = gradient(16, 16);
    let dirty = [Dirty {
        face: 0,
        slice: 0,
        rect: PixelRect::new(4, 4, 4, 4),
    }];
    let plan = plan_image(
        &Edit {
            image: &img,
            image_at: 0x4000,
            planes: planes(&img, &[&px]),
            dirty: &dirty,
            reshape: None,
        },
        &mut ResourceFiles::new(),
    )
    .unwrap();
    assert_eq!(plan.class, Class::InPlace);
    assert!(plan.appended.is_empty() && plan.tag.is_empty());
    assert!(!plan.raw.is_empty());
    let r = img.lods[0].unwrap();
    for w in &plan.raw {
        assert_eq!((w.block, w.block_size), (r.pointer, r.size));
        assert!(w.offset as usize + w.bytes.len() <= r.size as usize);
    }
}

/// The solid picture and the per channel tolerance each written format
/// reads back within.
fn solid_cases() -> Vec<(PixelFormat, [u8; 4], [u8; 4])> {
    vec![
        (PixelFormat::Argb8888, [200, 100, 50, 180], [0, 0, 0, 0]),
        (PixelFormat::Xrgb8888, [200, 100, 50, 255], [0, 0, 0, 0]),
        (PixelFormat::Rgb565, [200, 100, 50, 255], [8, 4, 8, 0]),
        (PixelFormat::Argb1555, [200, 100, 50, 255], [8, 8, 8, 0]),
        (PixelFormat::Argb4444, [200, 100, 50, 170], [9, 9, 9, 9]),
        (PixelFormat::Dxt1, [200, 100, 50, 255], [8, 4, 8, 0]),
        (PixelFormat::Dxt3, [200, 100, 50, 170], [8, 4, 8, 9]),
        (PixelFormat::Dxt5, [200, 100, 50, 170], [8, 4, 8, 2]),
    ]
}

#[test]
fn every_colour_format_reshapes_and_reads_back_at_every_level() {
    let img = image(PixelFormat::Argb8888, TagKind::D2, [16, 16, 1], 4, SWZ, 3);
    let (w, h) = (64u32, 32u32);
    let mut files = ResourceFiles::new();
    for (format, rgba, tol) in solid_cases() {
        let px: Vec<u8> = (0..w * h).flat_map(|_| rgba).collect();
        let pl = Planes {
            width: w,
            height: h,
            depth: 1,
            planes: &[&px],
        };
        let r = Reshape {
            width: w,
            height: h,
            format,
        };
        let plan = plan_image(
            &Edit {
                image: &img,
                image_at: 0x4000,
                planes: pl,
                dirty: &[],
                reshape: Some(r),
            },
            &mut files,
        )
        .unwrap();
        let (new, _) = reshaped(&img, r).unwrap();
        assert_eq!(plan.appended.len(), 3, "{format:?}");
        let d = read_back(&plan, 0x4000, &new);
        assert!(d.size_matches(), "{format:?}");
        let full = u32::from(new.mip_count) + 1;
        let want = if matches!(
            format,
            PixelFormat::Dxt1 | PixelFormat::Dxt3 | PixelFormat::Dxt5
        ) {
            crate::bitmap::stored_dxt_levels(w, h)
        } else {
            full
        };
        assert_eq!(d.levels.len() as u32, want, "{format:?}");
        // each level halves, down to one texel or one block
        for (n, l) in d.levels.iter().enumerate() {
            assert_eq!(l.width, (w >> n).max(1), "{format:?} level {n}");
            assert_eq!(l.height, (h >> n).max(1), "{format:?} level {n}");
            for p in l.rgba.chunks(4) {
                for c in 0..4 {
                    assert!(
                        p[c].abs_diff(rgba[c]) <= tol[c],
                        "{format:?} level {n} channel {c}: {p:?} vs {rgba:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_data_free_formats_reshape_to_the_value_they_hold() {
    let img = image(PixelFormat::Argb8888, TagKind::D2, [8, 8, 1], 3, SWZ, 1);
    let mut files = ResourceFiles::new();
    // A8 keeps alpha, Y8 and AY8 the luma, A8Y8 both.
    for (format, rgba, want) in [
        (
            PixelFormat::A8,
            [10, 20, 30, 77],
            [None, None, None, Some(77)],
        ),
        (
            PixelFormat::Y8,
            [90, 90, 90, 255],
            [Some(90), Some(90), Some(90), None],
        ),
        (
            PixelFormat::Ay8,
            [90, 90, 90, 255],
            [Some(90), Some(90), Some(90), None],
        ),
        (
            PixelFormat::A8Y8,
            [90, 90, 90, 33],
            [Some(90), Some(90), Some(90), Some(33)],
        ),
    ] {
        let px: Vec<u8> = (0..64).flat_map(|_| rgba).collect();
        let r = Reshape {
            width: 8,
            height: 8,
            format,
        };
        let plan = plan_image(
            &Edit {
                image: &img,
                image_at: 0x4000,
                planes: planes(&img, &[&px]),
                dirty: &[],
                reshape: Some(r),
            },
            &mut files,
        )
        .unwrap();
        let (new, _) = reshaped(&img, r).unwrap();
        let d = read_back(&plan, 0x4000, &new);
        assert_eq!(d.levels.len(), 4, "{format:?}");
        for l in &d.levels {
            for p in l.rgba.chunks(4) {
                for c in 0..4 {
                    if let Some(v) = want[c] {
                        assert_eq!(p[c], v, "{format:?} channel {c}");
                    }
                }
            }
        }
    }
}

#[test]
fn the_last_mip_is_the_mean_of_the_picture() {
    let img = image(PixelFormat::Argb8888, TagKind::D2, [32, 32, 1], 5, SWZ, 1);
    let px = gradient(32, 32);
    let a = decode(
        &img,
        0,
        &encode_lod(&img, 0, &planes(&img, &[&px])).unwrap(),
    );
    assert_eq!(a.len(), 6);
    let last = a.last().unwrap();
    assert_eq!((last.width, last.height), (1, 1));
    for c in 0..4 {
        let mean = px.chunks(4).map(|p| u32::from(p[c])).sum::<u32>() as f32 / 1024.0;
        assert!(
            (f32::from(last.rgba[c]) - mean).abs() <= 1.5,
            "channel {c}: {} vs {mean}",
            last.rgba[c]
        );
    }
}

#[test]
fn an_image_without_mips_stays_without_and_a_tiny_one_drops_the_lods_it_cannot_hold() {
    let flat = image(PixelFormat::Argb8888, TagKind::D2, [16, 16, 1], 0, SWZ, 1);
    let (new, _) = reshaped(
        &flat,
        Reshape {
            width: 64,
            height: 64,
            format: PixelFormat::Dxt5,
        },
    )
    .unwrap();
    assert_eq!(new.mip_count, 0);
    // 16x16 with mips and three LODs shrunk to 2x2: two levels, so LOD 2
    // has no block and its pointer and size are cleared.
    let img = image(PixelFormat::Argb8888, TagKind::D2, [16, 16, 1], 4, SWZ, 3);
    let px: Vec<u8> = (0..4).flat_map(|_| [1, 2, 3, 255]).collect();
    let plan = plan_image(
        &Edit {
            image: &img,
            image_at: 0x4000,
            planes: Planes {
                width: 2,
                height: 2,
                depth: 1,
                planes: &[&px],
            },
            dirty: &[],
            reshape: Some(Reshape {
                width: 2,
                height: 2,
                format: PixelFormat::Argb8888,
            }),
        },
        &mut ResourceFiles::new(),
    )
    .unwrap();
    assert_eq!(plan.appended.len(), 2);
    assert!(
        plan.tag
            .contains(&(0x4000 + 0x1c + 8, u32::MAX.to_le_bytes().to_vec()))
    );
    assert!(
        plan.tag
            .contains(&(0x4000 + 0x34 + 8, 0u32.to_le_bytes().to_vec()))
    );
}

#[test]
fn sizes_a_format_refuses_are_named_and_change_nothing() {
    let img = image(PixelFormat::Argb8888, TagKind::D2, [16, 16, 1], 4, SWZ, 1);
    let ask = |img: &Image, width: u32, height: u32, format: PixelFormat| {
        reshaped(
            img,
            Reshape {
                width,
                height,
                format,
            },
        )
        .err()
        .map(|e| e.to_string())
    };
    for format in [PixelFormat::Dxt1, PixelFormat::Dxt3, PixelFormat::Dxt5] {
        let e = ask(&img, 20, 16, format).expect("DXT needs power of two sides");
        assert!(e.contains("power of two"), "{format:?}: {e}");
        assert!(ask(&img, 16, 12, format).is_some());
    }
    assert!(
        ask(&img, 0, 16, PixelFormat::Argb8888)
            .unwrap()
            .contains("zero")
    );
    assert!(
        ask(&img, 16, 0, PixelFormat::Argb8888)
            .unwrap()
            .contains("zero")
    );
    assert!(
        ask(&img, 70_000, 16, PixelFormat::Argb8888)
            .unwrap()
            .contains("65535")
    );
    // data formats and floats are not written
    for format in [
        PixelFormat::P8,
        PixelFormat::V8U8,
        PixelFormat::G8B8,
        PixelFormat::P8Bump,
    ] {
        assert!(ask(&img, 16, 16, format).is_some(), "{format:?}");
    }
    // a cube stays square and a power of two
    let cube = image(PixelFormat::Argb8888, TagKind::Cube, [8, 8, 1], 0, SWZ, 1);
    assert!(
        ask(&cube, 8, 4, PixelFormat::Argb8888)
            .unwrap()
            .contains("square")
    );
    assert!(ask(&cube, 6, 6, PixelFormat::Argb8888).is_some());
    assert!(ask(&cube, 16, 16, PixelFormat::Dxt1).is_none());
    // a refused plan writes nothing: the plan is an error, not a half plan
    let px = vec![0u8; 20 * 16 * 4];
    let plan = plan_image(
        &Edit {
            image: &img,
            image_at: 0x4000,
            planes: Planes {
                width: 20,
                height: 16,
                depth: 1,
                planes: &[&px],
            },
            dirty: &[],
            reshape: Some(Reshape {
                width: 20,
                height: 16,
                format: PixelFormat::Dxt1,
            }),
        },
        &mut ResourceFiles::new(),
    );
    assert!(plan.is_err());
}

#[test]
fn a_cube_reshape_writes_every_face_at_the_new_size() {
    let img = image(PixelFormat::Argb8888, TagKind::Cube, [8, 8, 1], 3, SWZ, 1);
    let faces: Vec<Vec<u8>> = (0..6u8)
        .map(|f| (0..16 * 16).flat_map(|_| [f * 40, 10, 20, 255]).collect())
        .collect();
    let refs: Vec<&[u8]> = faces.iter().map(Vec::as_slice).collect();
    let r = Reshape {
        width: 16,
        height: 16,
        format: PixelFormat::Argb8888,
    };
    let plan = plan_image(
        &Edit {
            image: &img,
            image_at: 0x4000,
            planes: Planes {
                width: 16,
                height: 16,
                depth: 1,
                planes: &refs,
            },
            dirty: &[],
            reshape: Some(r),
        },
        &mut ResourceFiles::new(),
    )
    .unwrap();
    let (new, _) = reshaped(&img, r).unwrap();
    let d = read_back(&plan, 0x4000, &new);
    // 5 levels a face (16 down to 1), six faces
    assert_eq!(d.levels.len(), 6 * 5);
    for (n, l) in d.levels.iter().enumerate() {
        let face = (n / 5) as u8;
        assert_eq!(l.rgba[0], face * 40, "level {n}");
    }
}

fn opaque_planes(w: u32, h: u32, alpha: &[u8]) -> Vec<u8> {
    (0..(w * h) as usize)
        .flat_map(|i| [10, 20, 30, alpha[i % alpha.len()]])
        .collect()
}

#[test]
fn the_suggested_format_follows_the_picture_and_the_size() {
    use crate::write::{AlphaUse, alpha_use, suggest_format};
    let pl = |px: &Vec<u8>, w: u32, h: u32| {
        let px = px.clone();
        move |cur: PixelFormat| {
            suggest_format(
                &Planes {
                    width: w,
                    height: h,
                    depth: 1,
                    planes: &[px.as_slice()],
                },
                cur,
            )
        }
    };
    let opaque = opaque_planes(8, 8, &[255]);
    let cutout = opaque_planes(8, 8, &[255, 0]);
    let blended = opaque_planes(8, 8, &[255, 128]);
    let alpha = |px: &Vec<u8>| {
        alpha_use(&Planes {
            width: 8,
            height: 8,
            depth: 1,
            planes: &[px.as_slice()],
        })
    };
    assert_eq!(alpha(&opaque), AlphaUse::Opaque);
    assert_eq!(alpha(&cutout), AlphaUse::Cutout);
    assert_eq!(alpha(&blended), AlphaUse::Blended);
    // what holds the picture is kept
    assert_eq!(pl(&opaque, 8, 8)(PixelFormat::Rgb565), PixelFormat::Rgb565);
    assert_eq!(pl(&cutout, 8, 8)(PixelFormat::Dxt1), PixelFormat::Dxt1);
    assert_eq!(pl(&blended, 8, 8)(PixelFormat::Dxt3), PixelFormat::Dxt3);
    assert_eq!(
        pl(&blended, 8, 8)(PixelFormat::Argb8888),
        PixelFormat::Argb8888
    );
    // what loses alpha is replaced
    assert_eq!(pl(&cutout, 8, 8)(PixelFormat::Rgb565), PixelFormat::Dxt1);
    assert_eq!(pl(&blended, 8, 8)(PixelFormat::Dxt1), PixelFormat::Dxt5);
    assert_eq!(pl(&blended, 8, 8)(PixelFormat::Xrgb8888), PixelFormat::Dxt5);
    // masks and greys are kept
    for f in [
        PixelFormat::A8,
        PixelFormat::Y8,
        PixelFormat::Ay8,
        PixelFormat::A8Y8,
    ] {
        assert_eq!(pl(&blended, 8, 8)(f), f);
    }
    // a side that is not a power of two leaves DXT
    let odd = opaque_planes(7, 5, &[255]);
    assert_eq!(pl(&odd, 7, 5)(PixelFormat::Dxt1), PixelFormat::Xrgb8888);
    let odd_blend = opaque_planes(7, 5, &[255, 9]);
    assert_eq!(
        pl(&odd_blend, 7, 5)(PixelFormat::Dxt5),
        PixelFormat::Argb8888
    );
    // data formats are never answered
    assert_eq!(pl(&opaque, 8, 8)(PixelFormat::P8), PixelFormat::Dxt1);
    // and what it answers is always one the encoder writes and a reshape takes
    let img = image(PixelFormat::Argb8888, TagKind::D2, [16, 16, 1], 4, SWZ, 1);
    for (px, w, h) in [
        (&opaque, 8, 8),
        (&blended, 8, 8),
        (&odd, 7, 5),
        (&odd_blend, 7, 5),
    ] {
        for cur in [PixelFormat::P8, PixelFormat::Dxt1, PixelFormat::Rgb565] {
            let f = pl(px, w, h)(cur);
            assert!(writable(f));
            assert!(
                reshaped(
                    &img,
                    Reshape {
                        width: w,
                        height: h,
                        format: f
                    }
                )
                .is_ok(),
                "{f:?} at {w}x{h}"
            );
        }
    }
}
