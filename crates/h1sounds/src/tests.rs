//! Synthetic tests: invented tags, invented samples, maps built in memory.

use std::io::Cursor;

use h1map::{Block as H1Block, Group, TagId as H1TagId, TagRef};
use map::build::TagSet;
use map::{CacheType, Map, RawPointer, ResourceFile};

use crate::convert::{CHUNK_LEN, convert_sound, install_sounds};
use crate::decode::{decode_all, decode_bytes};
use crate::gestalt::{ChunkPointer, SoundGestalt, blocks};
use crate::h1::{Compression, DataRef, NO_NEXT, Permutation, PitchRange, Sound};
use crate::{Error, class};

/// `blocks` whole Xbox ADPCM blocks of `channels` channels, varied by `seed`.
fn adpcm(blocks: usize, channels: usize, seed: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut x = u32::from(seed).wrapping_mul(2654435761) | 1;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        x as u8
    };
    for _ in 0..blocks {
        for _ in 0..channels {
            out.extend_from_slice(&[next(), next() & 0x3f, next() % 89, 0]);
        }
        for _ in 0..32 * channels {
            out.push(next());
        }
    }
    out
}

fn perm(name: &str, next: u16, at: usize, data: &[u8]) -> Permutation {
    Permutation {
        name: name.into(),
        skip_fraction: 0.0,
        gain: 1.0,
        compression: Compression::XboxAdpcm,
        next,
        samples: DataRef {
            size: data.len() as u32,
            flags: 0,
            file_offset: at as u32,
            address: 0,
        },
        mouth: DataRef::default(),
        subtitles: DataRef::default(),
    }
}

/// An invented sound and the "file" its samples are in: range 0 has two
/// playable permutations, the first continued by permutation 2 (and long
/// enough to need several Halo 2 chunks); range 1 has one.
fn invented(channels: usize) -> (Sound, Vec<u8>) {
    let mut file = vec![0u8; 0x800];
    let mut add = |d: Vec<u8>| {
        let at = file.len();
        file.extend_from_slice(&d);
        (at, d)
    };
    let a = add(adpcm(1500, channels, 1));
    let b = add(adpcm(7, channels, 2));
    let c = add(adpcm(40, channels, 3));
    let d = add(adpcm(3, channels, 4));
    let sound = Sound {
        flags: 0,
        class: 23,
        sample_rate: 1,
        distance: [1.0, 30.0],
        skip_fraction: 0.0,
        random_pitch_bounds: [1.0, 1.0],
        cone_angles: [std::f32::consts::TAU; 2],
        outer_cone_gain: 1.0,
        gain_modifier: 1.0,
        max_bend_per_second: 0.0,
        zero_modifiers: [1.0; 3],
        one_modifiers: [1.0; 3],
        encoding: (channels - 1) as u16,
        compression: Compression::XboxAdpcm,
        promotion: TagRef {
            group: Group::new(b"snd!"),
            name_address: 0,
            name_len: 0,
            id: H1TagId(u32::MAX),
        },
        promotion_count: 0,
        pitch_ranges: vec![
            PitchRange {
                name: "idle".into(),
                natural_pitch: 1.0,
                bend_bounds: [0.5, 1.0],
                actual_permutation_count: 2,
                permutations: vec![
                    perm("rev1", 2, a.0, &a.1),
                    perm("rev2", NO_NEXT, b.0, &b.1),
                    perm("rev1b", NO_NEXT, c.0, &c.1),
                ],
            },
            PitchRange {
                name: "high".into(),
                natural_pitch: 2.0,
                bend_bounds: [1.0, 3.0],
                actual_permutation_count: 1,
                permutations: vec![perm("hi", NO_NEXT, d.0, &d.1)],
            },
        ],
    };
    (sound, file)
}

#[test]
fn a_header_and_its_blocks_parse_at_the_disc_offsets() {
    // One pitch range at 0x1100 with two permutations at 0x1200.
    let base = 0x1000u32;
    let mut mem = vec![0u8; 0x400];
    let put = |m: &mut Vec<u8>, at: usize, v: &[u8]| m[at..at + v.len()].copy_from_slice(v);
    put(&mut mem, 0x04, &13u16.to_le_bytes());
    put(&mut mem, 0x06, &1u16.to_le_bytes());
    put(&mut mem, 0x08, &2.5f32.to_le_bytes());
    put(&mut mem, 0x1c, &1.5f32.to_le_bytes());
    put(&mut mem, 0x6c, &1u16.to_le_bytes());
    put(&mut mem, 0x6e, &1u16.to_le_bytes());
    put(&mut mem, 0x7c, &u32::MAX.to_le_bytes());
    put(&mut mem, 0x98, &1u32.to_le_bytes());
    put(&mut mem, 0x9c, &0x1100u32.to_le_bytes());
    put(&mut mem, 0x100, b"default\0");
    put(&mut mem, 0x120, &1.0f32.to_le_bytes());
    put(&mut mem, 0x12c, &1u16.to_le_bytes());
    put(&mut mem, 0x13c, &2u32.to_le_bytes());
    put(&mut mem, 0x140, &0x1200u32.to_le_bytes());
    for (i, name) in [&b"a\0"[..], b"a2\0"].iter().enumerate() {
        let p = 0x200 + i * 0x7c;
        put(&mut mem, p, name);
        put(&mut mem, p + 0x24, &0.5f32.to_le_bytes());
        put(&mut mem, p + 0x28, &1u16.to_le_bytes());
        put(
            &mut mem,
            p + 0x2a,
            &(if i == 0 { 1u16 } else { NO_NEXT }).to_le_bytes(),
        );
        put(&mut mem, p + 0x40, &72u32.to_le_bytes());
        put(
            &mut mem,
            p + 0x48,
            &(0x9000u32 + 72 * i as u32).to_le_bytes(),
        );
    }
    let s = Sound::parse(&mem[..0xa4], |b: H1Block, len| {
        let at = (b.address - base) as usize;
        Ok(&mem[at..at + b.count as usize * len])
    })
    .unwrap();
    assert_eq!(
        (s.class, s.sample_rate, s.encoding, s.compression),
        (13, 1, 1, Compression::XboxAdpcm)
    );
    assert_eq!((s.distance[0], s.cone_angles[0]), (2.5, 1.5));
    assert!(s.promotion_id().is_none());
    let r = &s.pitch_ranges[0];
    assert_eq!(
        (
            r.name.as_str(),
            r.actual_permutation_count,
            r.permutations.len()
        ),
        ("default", 1, 2)
    );
    assert_eq!(
        (r.permutations[1].name.as_str(), r.permutations[0].gain),
        ("a2", 0.5)
    );
    assert_eq!(r.permutations[1].samples.file_offset, 0x9048);
    assert_eq!(s.chains(0).unwrap(), vec![vec![0, 1]]);
}

#[test]
fn chains_follow_next_and_refuse_loops() {
    let (mut s, file) = invented(1);
    assert_eq!(s.chains(0).unwrap(), vec![vec![0, 2], vec![1]]);
    assert_eq!(s.chains(1).unwrap(), vec![vec![0]]);
    let all = decode_all(&s, &file).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].2.frames(), (1500 + 40) * 64);
    assert_eq!(all[0].2.rate, 44100);
    s.pitch_ranges[0].permutations[2].next = 0;
    assert!(matches!(s.chains(0), Err(Error::Invalid(_))));
    s.pitch_ranges[0].permutations[2].next = 9;
    assert!(s.chains(0).is_err());
}

#[test]
fn decoding_is_the_audio_crates_and_refuses_partial_blocks() {
    let data = adpcm(3, 2, 9);
    assert_eq!(
        decode_bytes(Compression::XboxAdpcm, 2, &data).unwrap(),
        audio::adpcm::decode(&data, 2).unwrap()
    );
    assert!(decode_bytes(Compression::XboxAdpcm, 2, &data[..100]).is_err());
    assert!(decode_bytes(Compression::Ogg, 1, &data).is_err());
    let pcm = crate::Pcm {
        rate: 22050,
        channels: 2,
        samples: vec![1, -2, 3, -4],
    };
    let wav = pcm.wav();
    assert_eq!(
        (&wav[..4], &wav[8..16], wav.len()),
        (&b"RIFF"[..], &b"WAVEfmt "[..], 44 + 8)
    );
    assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 22050);
    assert_eq!(&wav[44..46], &1i16.to_le_bytes());
}

#[test]
fn every_named_halo1_class_has_a_halo2_class() {
    for (v, n) in class::H1_NAMES.iter().enumerate() {
        match n {
            Some(_) => {
                let h2 = class::to_h2(v as u16).unwrap();
                assert!(class::H2_NAMES[usize::from(h2)].is_some(), "{v}");
            }
            None => assert!(class::to_h2(v as u16).is_err(), "{v}"),
        }
    }
    assert_eq!(class::to_h2(19).unwrap(), 19);
    assert_eq!(class::to_h2(47).unwrap(), 48);
    assert!(class::to_h2(51).is_err());
}

/// An invented gestalt with every block, the nested ones with children.
fn invented_gestalt() -> (Vec<u8>, u32) {
    let base = 0x8000_0000u32;
    let mut b = vec![0u8; 0x58];
    let block = |b: &mut Vec<u8>, i: Option<usize>, count: u32, bytes: Vec<u8>| -> u32 {
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
        let at = base + b.len() as u32;
        if let Some(i) = i {
            b[i * 8..i * 8 + 4].copy_from_slice(&count.to_le_bytes());
            b[i * 8 + 4..i * 8 + 8].copy_from_slice(&at.to_le_bytes());
        }
        b.extend(bytes);
        at
    };
    let strides = crate::gestalt::STRIDES;
    for i in 0..11 {
        let count = (i as u32 % 3) + 1;
        let mut elems: Vec<u8> = (0..count as usize * strides[i])
            .map(|n| (n * 7 + i) as u8 | 1)
            .collect();
        if i == blocks::CHUNKS {
            for (n, c) in elems.chunks_exact_mut(12).enumerate() {
                c[..4].copy_from_slice(&(0x800u32 * (n as u32 + 1)).to_le_bytes());
            }
        }
        let children = crate::gestalt::child_blocks(i);
        if children.is_empty() {
            block(&mut b, Some(i), count, elems);
            continue;
        }
        for e in elems.chunks_exact_mut(strides[i]) {
            for &(off, _) in children {
                e[off..off + 8].fill(0);
            }
        }
        let at = block(&mut b, Some(i), count, elems);
        for n in 0..count as usize {
            for &(off, stride) in children {
                if stride == 0 {
                    continue;
                }
                let child: Vec<u8> = (0..2 * stride).map(|k| (k + n) as u8).collect();
                let caddr = block(&mut b, None, 2, child);
                let e = (at - base) as usize + n * strides[i] + off;
                b[e..e + 4].copy_from_slice(&2u32.to_le_bytes());
                b[e + 4..e + 8].copy_from_slice(&caddr.to_le_bytes());
            }
        }
    }
    (b, base)
}

#[test]
fn a_gestalt_is_covered_exactly_and_survives_a_build() {
    let (bytes, base) = invented_gestalt();
    let g = SoundGestalt::decode(&bytes, base).unwrap();
    assert_eq!(
        (g.count(blocks::PROMOTIONS), g.count(blocks::EXTRA_INFOS)),
        (1, 2)
    );
    assert_eq!(g.nested[blocks::EXTRA_INFOS][1].children[1].count, 2);
    assert_eq!(
        g.nested[blocks::PROMOTIONS][0].children[1].bytes,
        [0, 1, 2, 3, 4, 5, 6, 7]
    );
    assert_eq!(g.chunks[1].pointer, ChunkPointer::Stored(0x1000));

    // An unknown nonzero byte, a stray pointer target, or a child of
    // unknown layout is refused.
    let mut extra = bytes.clone();
    extra.extend([0, 0, 0, 0, 1]);
    assert!(SoundGestalt::decode(&extra, base).is_err());
    let mut unknown = bytes.clone();
    let x = u32::from_le_bytes(
        unknown[blocks::EXTRA_INFOS * 8 + 4..][..4]
            .try_into()
            .unwrap(),
    );
    let e = (x - base) as usize;
    unknown[e..e + 4].copy_from_slice(&1u32.to_le_bytes());
    assert!(matches!(
        SoundGestalt::decode(&unknown, base),
        Err(Error::Unsupported(_))
    ));

    let mut set = TagSet::new(CacheType::Multiplayer, "g", "invented\\g");
    let id = g.install(&mut set, "invented\\gestalt").unwrap();
    let (out, _) = set.build().unwrap();
    let mut m = Map::from_reader(Cursor::new(out)).unwrap();
    let t = m.tag(id).unwrap().clone();
    let again =
        SoundGestalt::decode(&m.read_tag_bytes(id).unwrap(), t.location.unwrap().address).unwrap();
    assert_eq!(again, g);
    let tg = tags::sound::Gestalt::read(&mut m).unwrap();
    assert_eq!(tg.chunks.len(), g.chunks.len());
    assert_eq!(tg.permutations.len(), g.count(blocks::PERMUTATIONS));
}

struct Local<'a>(&'a [u8]);
impl tags::sound::ResourceReader for Local<'_> {
    fn check(&self, p: RawPointer, size: u32) -> tags::Result<()> {
        assert_eq!(p.file, ResourceFile::Local);
        assert!(p.offset as usize + size as usize <= self.0.len());
        Ok(())
    }
    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        Ok(self.0[p.offset as usize..][..size as usize].to_vec())
    }
}

#[test]
fn converted_sounds_read_back_through_tags_and_decode_identically() {
    for channels in [1, 2] {
        let (s, file) = invented(channels);
        let c = convert_sound(&s, &file).unwrap();
        assert_eq!(
            c.ranges[0].permutations[0].chunks.len(),
            (1540 * 36 * channels).div_ceil(CHUNK_LEN)
        );
        assert!(
            c.ranges[0].permutations[0]
                .chunks
                .iter()
                .all(|k| k.len() % (36 * channels) == 0)
        );
        // Two sounds into an existing gestalt.
        let (g, base) = invented_gestalt();
        let mut set = TagSet::new(CacheType::Multiplayer, "s", "invented\\s");
        let g = SoundGestalt::decode(&g, base).unwrap();
        let old_chunks = g.chunks.len();
        g.install(&mut set, "invented\\gestalt").unwrap();
        let (bytes, _) = set.build().unwrap();
        let mut set = TagSet::from_map(&mut Map::from_reader(Cursor::new(bytes)).unwrap()).unwrap();
        crate::convert::make_index_room(&mut set);
        let ids = install_sounds(
            &mut set,
            vec![("a\\one".into(), c.clone()), ("a\\two".into(), c)],
        )
        .unwrap();
        let (out, _) = set.build().unwrap();
        let mut m = Map::from_reader(Cursor::new(&out[..])).unwrap();
        let g = tags::sound::Gestalt::read(&mut m).unwrap();
        assert_eq!(
            g.chunks[..old_chunks]
                .iter()
                .map(|c| c.pointer.offset)
                .collect::<Vec<_>>(),
            (1..=old_chunks as u32)
                .map(|n| n * 0x800)
                .collect::<Vec<_>>()
        );
        let expect = decode_all(&s, &file).unwrap();
        for id in ids {
            let h = tags::sound::read_header(&mut m, id).unwrap();
            assert_eq!(
                (h.class, h.sample_rate.hz(), h.pitch_range_count),
                (23, Some(44100), 2)
            );
            assert_eq!(h.compression, tags::sound::Compression::XboxAdpcm);
            assert_eq!(h.max_play_time, (1540 * 64 * 1000 / 44100) as u32);
            let r = g.resolve(&h).unwrap();
            let got: Vec<Vec<i16>> = r
                .ranges
                .iter()
                .flat_map(|r| &r.permutations)
                .map(|p| {
                    audio::adpcm::decode(&g.read_encoded(p, &mut Local(&out)).unwrap(), channels)
                        .unwrap()
                })
                .collect();
            assert_eq!(
                got,
                expect
                    .iter()
                    .map(|e| e.2.samples.clone())
                    .collect::<Vec<_>>()
            );
            // Runtime flags: the two-permutation range has a byte of its own.
            let pr = &r.ranges[0].range;
            assert_eq!(&pr.raw[4..6], &[0xff, 0xff]);
            assert_eq!(&r.ranges[1].range.raw[4..8], &[0, 0, 0xff, 0xff]);
        }
    }
}

#[test]
fn a_map_without_a_gestalt_gets_one() {
    let (s, file) = invented(1);
    let mut set = TagSet::new(CacheType::Multiplayer, "s", "invented\\s");
    let ids = install_sounds(
        &mut set,
        vec![("x".into(), convert_sound(&s, &file).unwrap())],
    )
    .unwrap();
    let (out, _) = set.build().unwrap();
    let mut m = Map::from_reader(Cursor::new(&out[..])).unwrap();
    let g = tags::sound::Gestalt::read(&mut m).unwrap();
    let h = tags::sound::read_header(&mut m, ids[0]).unwrap();
    assert_eq!(g.resolve(&h).unwrap().ranges.len(), 2);
}
