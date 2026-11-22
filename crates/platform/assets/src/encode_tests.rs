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
