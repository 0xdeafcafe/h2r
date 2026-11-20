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
        mouth_samples: Vec::new(),
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
        promotion_millis: 0,
        promotion_name: None,
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
    put(&mut mem, 0x254, &3u32.to_le_bytes());
    put(&mut mem, 0x25c, &0xdead_beefu32.to_le_bytes());
    put(&mut mem, 0x260, &0x1310u32.to_le_bytes());
    put(&mut mem, 0x310, &[0, 241, 32]);
    let s = Sound::parse(&mem[..0xa4], |b: H1Block, len| {
        let at = (b.address - base) as usize;
        Ok(&mem[at..at + b.count as usize * len])
    })
    .unwrap();
    assert_eq!(
        s.pitch_ranges[0].permutations[0].mouth_samples,
        [0, 241, 32]
    );
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

/// A built gestalt reads back as the one that built it (local chunk
/// pointers included), so a set's gestalt can grow in more than one pass.
#[test]
fn a_built_gestalt_reads_back() {
    let (bytes, base) = invented_gestalt();
    let mut g = SoundGestalt::decode(&bytes, base).unwrap();
    let mut set = TagSet::new(CacheType::Multiplayer, "g", "invented\\g");
    let raw = set.add_raw(vec![1, 2, 3, 4], 4);
    g.chunks.push(crate::gestalt::Chunk {
        pointer: ChunkPointer::Local(raw),
        size: 4,
        runtime: u32::MAX,
    });
    let back = SoundGestalt::from_block(&g.to_block()).unwrap();
    assert_eq!(back, g);
    g.install(&mut set, "g").unwrap();
    assert_eq!(SoundGestalt::from_set(&set).unwrap(), back);
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
            // Permutation `+6`: each one's length in milliseconds, which a
            // script waiting on the playing sound reads.
            let times: Vec<u16> = r
                .ranges
                .iter()
                .flat_map(|r| &r.permutations)
                .map(|p| u16::from_le_bytes([p.raw[6], p.raw[7]]))
                .collect();
            assert_eq!(
                times,
                expect
                    .iter()
                    .map(|e| (e.2.frames() as u64 * 1000 / 44100) as u16)
                    .collect::<Vec<_>>()
            );
            assert!(times.iter().all(|&t| t > 0));
            // Runtime flags: the two-permutation range has a byte of its own.
            let pr = &r.ranges[0].range;
            assert_eq!(&pr.raw[4..6], &[0xff, 0xff]);
            assert_eq!(&r.ranges[1].range.raw[4..8], &[0, 0, 0xff, 0xff]);
        }
    }
}

/// A Halo 1 distance of 0 is the class's default; Halo 2 says so with the
/// class distance flags, or the engine takes the zeros as distances and a
/// positioned sound is inaudible past a millimetre. Cones that are the whole sphere
/// get the no-cone flag every retail sound has.
#[test]
fn default_distances_and_no_cone_become_engine_flags() {
    use crate::convert::{flags, snd};
    let (mut s, file) = invented(1);
    s.flags = 2;
    // Distances of its own: only Halo 1's bits and no cone.
    assert_eq!(flags(&s), 2 | snd::NO_CONE);
    let both = snd::CLASS_MIN_DISTANCE | snd::CLASS_MAX_DISTANCE;
    s.distance = [0.0, 0.0];
    assert_eq!(flags(&s), 2 | both | snd::NO_CONE);
    let c = convert_sound(&s, &file).unwrap();
    assert_eq!(
        u16::from_le_bytes([c.header[snd::FLAGS], c.header[snd::FLAGS + 1]]),
        2 | both | snd::NO_CONE
    );
    // The playback element keeps the zeros; the flags send the engine to
    // the class instead.
    assert_eq!(&c.playback[..8], &[0; 8]);
    s.distance = [0.0, 12.0];
    assert_eq!(flags(&s), 2 | snd::CLASS_MIN_DISTANCE | snd::NO_CONE);
    s.distance = [1.0, 0.0];
    assert_eq!(flags(&s), 2 | snd::CLASS_MAX_DISTANCE | snd::NO_CONE);
    // A real cone keeps the engine's cone path.
    s.cone_angles = [1.0, 2.0];
    assert_eq!(flags(&s), 2 | snd::CLASS_MAX_DISTANCE);
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

/// Maps names by adding `0x1000` and stored pointers by adding `0x100`.
struct Shift(Vec<(u32, u32)>);

impl crate::MergeMap for Shift {
    fn name(&mut self, id: u32) -> crate::Result<u32> {
        Ok(id + 0x1000)
    }
    fn raw(&mut self, p: ChunkPointer, size: u32) -> crate::Result<ChunkPointer> {
        let ChunkPointer::Stored(v) = p else {
            panic!("source pointers are stored")
        };
        self.0.push((v, size));
        Ok(ChunkPointer::Stored(v + 0x100))
    }
}

fn element(i: usize, fill: u8) -> Vec<u8> {
    vec![fill; crate::gestalt::STRIDES[i]]
}

#[test]
fn a_moved_sound_takes_its_gestalt_entries_and_flag_bits() {
    use crate::convert::snd;
    use crate::gestalt::{Chunk, Nested};
    let mut src = SoundGestalt::default();
    src.push(blocks::PLAYBACKS, &element(blocks::PLAYBACKS, 1), false);
    src.push(blocks::PLAYBACKS, &element(blocks::PLAYBACKS, 2), false);
    src.push(blocks::SCALES, &element(blocks::SCALES, 3), false);
    for id in [10u32, 11, 12, 13] {
        src.push(blocks::IMPORT_NAMES, &id.to_le_bytes(), false);
    }
    src.push(blocks::PITCH_RANGE_PARAMETERS, &element(3, 4), false);
    src.push(blocks::PITCH_RANGE_PARAMETERS, &element(3, 5), false);
    // Range 0: one permutation; range 1: three, flags from bit 3.
    let range = |name: u16, params: u16, enc: u16, flags: u16, first: u16, n: u16| {
        let mut e = Vec::new();
        for v in [name, params, enc, flags, first, n] {
            e.extend_from_slice(&v.to_le_bytes());
        }
        e
    };
    src.push(blocks::PITCH_RANGES, &range(0, 0, 0, 0xffff, 0, 1), false);
    src.push(blocks::PITCH_RANGES, &range(1, 1, 0xffff, 3, 1, 3), false);
    let perm = |name: u16, first: u16, n: u16| {
        let mut e = vec![0u8; 16];
        e[0..2].copy_from_slice(&name.to_le_bytes());
        e[5] = 7;
        e[8..12].copy_from_slice(&0x500u32.to_le_bytes());
        e[12..14].copy_from_slice(&first.to_le_bytes());
        e[14..16].copy_from_slice(&n.to_le_bytes());
        e
    };
    src.push(blocks::PERMUTATIONS, &perm(0, 0, 1), false);
    src.push(blocks::PERMUTATIONS, &perm(2, 1, 2), false);
    src.push(blocks::PERMUTATIONS, &perm(3, 3, 0), false);
    src.push(blocks::PERMUTATIONS, &perm(2, 3, 1), false);
    for c in 0..4u32 {
        src.chunks.push(Chunk {
            pointer: ChunkPointer::Stored(0x40_0000 + c * 0x1000),
            size: 0x8000_0000 | (0x200 + c),
            runtime: u32::MAX,
        });
    }
    src.flat[blocks::RUNTIME_PERMUTATION_FLAGS] = vec![0b0010_1000, 0];
    src.nested[blocks::CUSTOM_PLAYBACKS].push(Nested {
        bytes: element(blocks::CUSTOM_PLAYBACKS, 6),
        children: vec![
            crate::gestalt::Child {
                count: 0,
                bytes: Vec::new()
            };
            2
        ],
    });

    // Sound B: playback 1, scale 0, range 1, custom playback 0.
    let mut header = vec![0u8; snd::LEN];
    header[snd::PLAYBACK] = 1;
    header[snd::FIRST_PITCH_RANGE] = 1;
    header[snd::PITCH_RANGE_COUNT] = 1;
    header[snd::PROMOTION] = 0xff;
    header[snd::CUSTOM_PLAYBACK] = 0;
    header[snd::EXTRA_INFO..snd::EXTRA_INFO + 2].fill(0xff);
    header[snd::MAX_PLAY_TIME] = 0x42;

    // The destination already has B's playback and one other chunk.
    let mut dst = SoundGestalt::default();
    dst.push(blocks::PLAYBACKS, &element(blocks::PLAYBACKS, 9), false);
    dst.push(blocks::PLAYBACKS, &element(blocks::PLAYBACKS, 2), false);
    dst.flat[blocks::RUNTIME_PERMUTATION_FLAGS] = vec![0xff];
    dst.chunks.push(src.chunks[0]);
    let mut map = Shift(Vec::new());
    let out = crate::merge_sound(&src, &header, &mut dst, 0xe1e1_0005, &mut map).unwrap();

    assert_eq!(
        out[snd::PLAYBACK],
        1,
        "deduplicated against the destination"
    );
    assert_eq!(out[snd::SCALE], 0);
    assert_eq!(out[snd::FIRST_PITCH_RANGE], 0);
    assert_eq!(out[snd::CUSTOM_PLAYBACK], 0);
    assert_eq!(out[snd::MAX_PLAY_TIME], 0x42, "other fields kept");
    assert_eq!(dst.count(blocks::PITCH_RANGES), 1);
    assert_eq!(dst.count(blocks::PERMUTATIONS), 3);
    assert_eq!(dst.chunks.len(), 1 + 3);
    let r = dst.element(blocks::PITCH_RANGES, 0).to_vec();
    let at = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let name = |n: u16| {
        let e = dst.element(blocks::IMPORT_NAMES, n as usize);
        u32::from_le_bytes(e.try_into().unwrap())
    };
    assert_eq!(name(at(&r, 0)), 11 + 0x1000);
    assert_eq!(dst.element(3, at(&r, 2) as usize), &element(3, 5)[..]);
    assert_eq!(
        (at(&r, 4), at(&r, 6), at(&r, 8), at(&r, 10)),
        (0xffff, 8, 0, 3)
    );
    assert_eq!(
        dst.flat[blocks::RUNTIME_PERMUTATION_FLAGS],
        [0xff, 0b101],
        "bits 3..6 of the source become bits 8..11"
    );
    let p: Vec<_> = (0..3)
        .map(|n| dst.element(blocks::PERMUTATIONS, n).to_vec())
        .collect();
    assert_eq!(name(at(&p[0], 0)), 12 + 0x1000);
    assert_eq!(name(at(&p[1], 0)), 13 + 0x1000);
    assert_eq!((at(&p[0], 12), at(&p[0], 14)), (1, 2));
    assert_eq!((at(&p[1], 12), at(&p[1], 14)), (3, 0));
    assert_eq!((at(&p[2], 12), at(&p[2], 14)), (3, 1));
    assert_eq!(p[0][5], 7);
    assert_eq!(
        dst.chunks[1].pointer,
        ChunkPointer::Stored(0x40_1000 + 0x100)
    );
    assert_eq!(dst.chunks[1].size, src.chunks[1].size, "flag bit kept");
    assert_eq!(
        map.0,
        [(0x40_1000, 0x201), (0x40_2000, 0x202), (0x40_3000, 0x203)]
    );
    assert_eq!(
        dst.nested[blocks::CUSTOM_PLAYBACKS],
        src.nested[blocks::CUSTOM_PLAYBACKS]
    );

    // The merged gestalt still builds and reads back.
    let _ = dst.to_block();
}

fn promotion_entry() -> crate::gestalt::Nested {
    use crate::gestalt::{Child, Nested};

    let mut bytes = vec![0; 0x1c];
    bytes[0x10..0x14].fill(0xff);
    let mut rules = vec![0; 2 * 0x10];
    // First rule selects the default range, the next rule selects range 1.
    rules[..2].fill(0xff);
    rules[2..4].copy_from_slice(&2u16.to_le_bytes());
    rules[4..8].copy_from_slice(&0.4f32.to_le_bytes());
    rules[8..12].copy_from_slice(&1000u32.to_le_bytes());
    rules[12..16].copy_from_slice(&500u32.to_le_bytes());
    rules[0x10..0x12].copy_from_slice(&1u16.to_le_bytes());
    Nested {
        bytes,
        children: vec![
            Child {
                count: 2,
                bytes: rules,
            },
            Child {
                count: 2,
                bytes: vec![0; 8],
            },
        ],
    }
}

#[test]
fn native_promotion_selects_the_same_samples_after_its_sound_moves() {
    use crate::convert::snd;

    let (sound, file) = invented(1);
    let sound = convert_sound(&sound, &file).unwrap();
    let mut src_set = TagSet::new(CacheType::Multiplayer, "donor", "donor");
    let mut src = SoundGestalt::default();
    sound.clone().add_to(&mut src, &mut src_set).unwrap();
    let mut tag = sound.add_to(&mut src, &mut src_set).unwrap();
    for (i, chunk) in src.chunks.iter_mut().enumerate() {
        chunk.pointer = ChunkPointer::Stored(0x8000_0000 + 0x8000 * i as u32);
    }
    src.nested[blocks::PROMOTIONS].push(promotion_entry());
    tag.root.bytes[snd::PROMOTION] = 0;

    let mut dst = SoundGestalt::default();
    dst.push(
        blocks::PITCH_RANGES,
        &element(blocks::PITCH_RANGES, 0),
        false,
    );
    dst.nested[blocks::PROMOTIONS].push(promotion_entry());
    let out =
        crate::merge_sound(&src, &tag.root.bytes, &mut dst, 0, &mut Shift(Vec::new())).unwrap();
    assert_eq!(out[snd::PROMOTION], 1);
    assert_eq!(
        dst.nested[blocks::PROMOTIONS][1],
        src.nested[blocks::PROMOTIONS][0]
    );
    let short = |bytes: &[u8], at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
    let selected_chunk = |g: &SoundGestalt, header: &[u8]| {
        let promotion = &g.nested[blocks::PROMOTIONS][header[snd::PROMOTION] as usize];
        let relative = short(&promotion.children[0].bytes, 0x10);
        let range = g.element(
            blocks::PITCH_RANGES,
            short(header, snd::FIRST_PITCH_RANGE) + relative,
        );
        let perm = g.element(blocks::PERMUTATIONS, short(range, 8));
        g.chunks[short(perm, 12)].pointer
    };
    let ChunkPointer::Stored(source) = selected_chunk(&src, &tag.root.bytes) else {
        panic!("source chunk has a stored pointer");
    };
    assert_eq!(
        selected_chunk(&dst, &out),
        ChunkPointer::Stored(source + 0x100)
    );
    assert_eq!(short(&tag.root.bytes, snd::FIRST_PITCH_RANGE), 2);
    assert_eq!(short(&out, snd::FIRST_PITCH_RANGE), 1);
    assert_eq!(
        SoundGestalt::from_block(&dst.to_block()).unwrap().nested[blocks::PROMOTIONS],
        dst.nested[blocks::PROMOTIONS]
    );
}

#[test]
fn native_promotion_rejects_missing_targets_and_unaddressable_entries() {
    use crate::convert::snd;

    let mut src = SoundGestalt::default();
    src.nested[blocks::PROMOTIONS].push(promotion_entry());
    let mut header = vec![0; snd::LEN];
    header[snd::PITCH_RANGE_COUNT] = 1;
    let mut dst = SoundGestalt::default();
    let mut map = Shift(Vec::new());
    let error = crate::merge_sound(&src, &header, &mut dst, 0, &mut map).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("promotion 0 rule 1 targets pitch range 1 of 1")
    );
    assert_eq!(dst, SoundGestalt::default());

    header[snd::PITCH_RANGE_COUNT] = 2;
    dst.nested[blocks::PROMOTIONS].resize(usize::from(i8::MAX as u8) + 1, promotion_entry());
    let before = dst.clone();
    let error = crate::merge_sound(&src, &header, &mut dst, 0, &mut map).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("destination promotion index exceeds signed 8 bits")
    );
    assert_eq!(dst, before);
}

#[test]
fn a_reused_donor_sound_must_keep_its_promotion() {
    use crate::convert::snd;

    let (sound, file) = invented(1);
    let sound = convert_sound(&sound, &file).unwrap();
    let mut donor = TagSet::new(CacheType::Multiplayer, "donor", "donor");
    let mut source_gestalt = SoundGestalt::default();
    let mut tag = sound
        .clone()
        .add_to(&mut source_gestalt, &mut donor)
        .unwrap();
    source_gestalt.nested[blocks::PROMOTIONS].push(promotion_entry());
    tag.root.bytes[snd::PROMOTION] = 0;
    let root = tag.install(&mut donor, "crash").unwrap();
    source_gestalt.install(&mut donor, "sounds").unwrap();
    let (bytes, _) = donor.build().unwrap();
    let mut donor = Map::from_reader(Cursor::new(bytes)).unwrap();
    let mut dest = TagSet::new(CacheType::Multiplayer, "destination", "destination");
    install_sounds(&mut dest, vec![("crash".to_owned(), sound)]).unwrap();
    let mut gestalt = SoundGestalt::from_set(&dest).unwrap();
    let error = crate::transplant(
        tags::schema::Schema::builtin(),
        &mut donor,
        &source_gestalt,
        root,
        &mut dest,
        &mut gestalt,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("donor: snd! crash"), "{error}");
    assert!(
        error.contains("destination has a different promotion"),
        "{error}"
    );
}

#[test]
fn mouth_descriptors_span_ranges_and_survive_gestalt_reinstallation() {
    use crate::convert::snd;
    let (mut sound, file) = invented(1);
    // Range 0 has a silent split permutation and one voiced permutation.
    // Range 1 starts at descriptor 2, even though its range-local index is 0.
    for (r, p, bytes) in [(0, 1, &[0, 120, 255][..]), (1, 0, &[24, 0][..])] {
        let perm = &mut sound.pitch_ranges[r].permutations[p];
        perm.mouth.size = bytes.len() as u32;
        perm.mouth_samples = bytes.into();
    }
    let mut g = SoundGestalt::default();
    let mut set = TagSet::new(CacheType::Campaign, "mouth", "mouth");
    let converted = convert_sound(&sound, &file)
        .unwrap()
        .add_to(&mut g, &mut set)
        .unwrap();
    assert_eq!(
        &converted.root.bytes[snd::EXTRA_INFO..][..2],
        &0u16.to_le_bytes()
    );
    let owner = g.install(&mut set, "gestalt").unwrap();
    let g = SoundGestalt::from_set(&set).unwrap();
    assert_eq!(g.install(&mut set, "gestalt").unwrap(), owner);
    let e = &g.nested[blocks::EXTRA_INFOS][0];
    let word = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    assert_eq!(word(&e.bytes, 0x20), owner.0);
    let raw = &set.raw[g.extra_raws[&0].0].bytes;
    assert_eq!(&raw[28..33], &[0, 120, 255, 24, 0]);
    let at = 24 + word(&e.children[1].bytes, 28) as usize;
    assert_eq!(&raw[at - 4..at], b"crsr");
    for (i, (offset, len)) in [(0, 0), (0, 3), (3, 2)].into_iter().enumerate() {
        assert_eq!(g.element(blocks::PERMUTATIONS, i)[5], i as u8);
        let d = &raw[at + i * 16..][..16];
        assert_eq!((word(d, 0), word(d, 4), word(d, 12)), (offset, len, 0));
    }
}

#[test]
fn cyclic_promotion_targets_resolve_to_the_new_sound_datums() {
    use tags::sound::h1_promotion::*;

    let (mut snd, file) = invented(1);
    snd.promotion.id = H1TagId(0xe111_0002);
    snd.promotion_count = 2;
    snd.promotion_millis = 750;
    snd.promotion_name = Some("large".to_owned());
    let small = convert_sound(&snd, &file).unwrap();
    snd.promotion.id = H1TagId(0xe112_0003);
    snd.promotion_name = Some("small".to_owned());
    let large = convert_sound(&snd, &file).unwrap();
    let mut set = TagSet::new(CacheType::Multiplayer, "synthetic", "synthetic");
    let ids = install_sounds(
        &mut set,
        vec![("small".to_owned(), small), ("large".to_owned(), large)],
    )
    .unwrap();
    let (bytes, _) = set.build().unwrap();
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    for (i, id) in ids.iter().enumerate() {
        let bytes = map.read_tag_bytes(*id).unwrap();
        assert!(present(&bytes));
        let at = |offset: u32| {
            let offset = offset as usize;
            u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
        };
        assert_eq!(at(TARGET), ids[1 - i].0);
        assert_eq!(
            (at(COUNT), at(MILLIS), at(CREDIT), at(LAST_TIME)),
            (2, 750, 0, 0)
        );
    }
}
