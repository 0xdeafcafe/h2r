//! SN2 (docs/editor/studio-redesign.md): a `sound` item with a `tag` builds
//! to a `snd!` and rows of the map's gestalt. The clips are invented
//! sines made here; no disc bytes.
use audio::adpcm;
use h1_assets::sounds::gestalt::{Chunk, ChunkPointer, SoundGestalt, UGH, blocks};
use map::FourCC;
use map::build::{TagPayload, TagSet};
use tags::repo::TagKey;
use tags::sound::{Compression, Encoding, SampleRate, SoundHeader};

use super::*;
use crate::import::{Clip, class_of, encode};
use crate::tests::{base, pkg, target};

const SND: FourCC = FourCC(*b"snd!");

/// `frames` frames of sines: 440 Hz on the left, 660 Hz at half the
/// amplitude on the right (stereo).
fn sines(channels: u16, rate: u32, frames: usize) -> Clip {
    let tone = |hz: f32, amp: f32, i: usize| {
        ((i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin() * amp) as i16
    };
    let mut pcm = Vec::new();
    for i in 0..frames {
        pcm.push(tone(440.0, 12000.0, i));
        if channels == 2 {
            pcm.push(tone(660.0, 6000.0, i));
        }
    }
    Clip {
        channels,
        rate,
        pcm,
    }
}

/// The 16-bit PCM WAV file of `clip`.
fn wav_of(clip: &Clip) -> Vec<u8> {
    let data = clip.pcm.len() as u32 * 2;
    let mut b = b"RIFF".to_vec();
    b.extend((36 + data).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes());
    b.extend(1u16.to_le_bytes());
    b.extend(clip.channels.to_le_bytes());
    b.extend(clip.rate.to_le_bytes());
    b.extend((clip.rate * u32::from(clip.channels) * 2).to_le_bytes());
    b.extend((clip.channels * 2).to_le_bytes());
    b.extend(16u16.to_le_bytes());
    b.extend(b"data");
    b.extend(data.to_le_bytes());
    for s in &clip.pcm {
        b.extend(s.to_le_bytes());
    }
    b
}

/// A package of sound items: (file, tag, the file's bytes).
fn sounds(id: &str, items: &[(&str, &str, &[u8])]) -> Loaded {
    let list: Vec<(&str, &str, Option<&str>, &str)> = items
        .iter()
        .map(|(f, t, _)| ("sound", *f, Some(*t), "all"))
        .collect();
    let files: Vec<(&str, &[u8])> = items.iter().map(|(f, _, b)| (*f, *b)).collect();
    pkg(id, &[], &list, &files)
}

/// `base` with a gestalt of invented rows: a playback, a pitch range, a
/// permutation and a chunk of a file the set does not hold.
fn base_with_gestalt() -> TagSet {
    let mut set = base();
    let mut g = SoundGestalt::default();
    g.push(blocks::PLAYBACKS, &[7; 0x38], false);
    g.push(blocks::PITCH_RANGES, &[9; 12], false);
    g.push(blocks::PERMUTATIONS, &[5; 16], false);
    g.chunks.push(Chunk {
        pointer: ChunkPointer::Stored(0x8000_0010),
        size: 0x20,
        runtime: u32::MAX,
    });
    g.install(&mut set, "invented\\gestalt").unwrap();
    set
}

fn header_of(set: &TagSet, path: &str) -> SoundHeader {
    let id = set.find(SND, path).expect("the snd! is in the map");
    let TagPayload::Built(b) = &set.tag(id).unwrap().payload else {
        panic!("not built");
    };
    SoundHeader::decode(&b.bytes).unwrap()
}

/// The ADPCM bytes of permutation `n` of the set's gestalt, from its
/// chunks' raw resources.
fn adpcm_of(set: &TagSet, g: &SoundGestalt, n: usize) -> Vec<u8> {
    let p = &g.flat[blocks::PERMUTATIONS][n * 16..n * 16 + 16];
    let (first, count) = (
        u16::from_le_bytes([p[12], p[13]]) as usize,
        u16::from_le_bytes([p[14], p[15]]) as usize,
    );
    let mut out = Vec::new();
    for c in &g.chunks[first..first + count] {
        let ChunkPointer::Local(r) = c.pointer else {
            panic!("a chunk this build did not add");
        };
        assert_eq!(set.raw[r.0].bytes.len() as u32, c.size);
        out.extend(&set.raw[r.0].bytes);
    }
    out
}

#[test]
fn an_invented_wav_builds_to_a_snd_that_decodes_to_the_same_samples() {
    let clip = sines(1, 22_050, 1000);
    let wav = wav_of(&clip);
    let pkg = sounds("s", &[("a.wav", "invented\\snd\\a", wav.as_slice())]);
    assert_eq!(check_package(&pkg), vec![]);
    let mut set = base();
    let report = build(&mut set, &[pkg], &target(), None).unwrap();
    assert_eq!(
        report.added,
        vec![
            TagKey::new(SND, "invented\\snd\\a"),
            TagKey::new(UGH, h1_assets::sounds::gestalt::DEFAULT_NAME),
        ]
    );
    assert!(report.changed.is_empty());

    let h = header_of(&set, "invented\\snd\\a");
    assert_eq!(h.sample_rate, SampleRate::Hz22050);
    assert_eq!(h.encoding, Encoding::Mono);
    assert_eq!(h.compression, Compression::XboxAdpcm);
    assert_eq!((h.first_pitch_range, h.pitch_range_count), (0, 1));
    assert_eq!(h.class, 51);
    assert_eq!(h.extra_info, 0xffff);
    assert_eq!(h.max_play_time, 1000 * 1000 / 22_050);

    let g = SoundGestalt::from_set(&set).unwrap();
    assert_eq!(g.count(blocks::PITCH_RANGES), 1);
    assert_eq!(g.count(blocks::PERMUTATIONS), 1);
    assert_eq!(g.chunks.len(), 1);
    let bytes = adpcm_of(&set, &g, 0);
    // 1000 frames are 16 blocks of 64.
    assert_eq!(bytes.len(), 16 * 36);
    let perm = &g.flat[blocks::PERMUTATIONS];
    assert_eq!(
        u32::from_le_bytes(perm[8..12].try_into().unwrap()) as usize,
        bytes.len()
    );
    let back = adpcm::decode(&bytes, 1).unwrap();
    assert_eq!(back.len(), 1024);
    for (i, (&a, &b)) in clip.pcm.iter().zip(&back).enumerate().skip(16) {
        assert!((i32::from(a) - i32::from(b)).abs() <= 1500, "frame {i}: {a} vs {b}");
    }
}

#[test]
fn a_sound_joins_the_gestalt_and_keeps_the_rows_already_in_it() {
    let (a, b) = (sines(1, 22_050, 300), sines(2, 44_100, 700));
    let (wa, wb) = (wav_of(&a), wav_of(&b));
    let pkg = sounds(
        "s",
        &[
            ("a.wav", "invented\\snd\\a", wa.as_slice()),
            ("b.wav", "invented\\snd\\b", wb.as_slice()),
        ],
    );
    let mut set = base_with_gestalt();
    let report = build(&mut set, &[pkg], &target(), None).unwrap();
    // The gestalt is a tag of the base, changed, not one added.
    assert_eq!(
        report.added,
        vec![
            TagKey::new(SND, "invented\\snd\\a"),
            TagKey::new(SND, "invented\\snd\\b"),
        ]
    );
    assert_eq!(
        report.changed.iter().cloned().collect::<Vec<_>>(),
        vec![TagKey::new(UGH, "invented\\gestalt")]
    );

    let g = SoundGestalt::from_set(&set).unwrap();
    // The rows that were there.
    assert_eq!(g.element(blocks::PLAYBACKS, 0), &[7; 0x38]);
    assert_eq!(g.element(blocks::PITCH_RANGES, 0), &[9; 12]);
    assert_eq!(g.element(blocks::PERMUTATIONS, 0), &[5; 16]);
    assert_eq!(g.chunks[0].pointer, ChunkPointer::Stored(0x8000_0010));
    assert_eq!(g.chunks[0].size, 0x20);
    // And the new ones after them: the two sounds' playback is one row.
    assert_eq!(g.count(blocks::PLAYBACKS), 2);
    assert_eq!(g.count(blocks::PITCH_RANGES), 3);
    assert_eq!(g.count(blocks::PERMUTATIONS), 3);
    assert_eq!(g.chunks.len(), 3);
    let (ha, hb) = (
        header_of(&set, "invented\\snd\\a"),
        header_of(&set, "invented\\snd\\b"),
    );
    assert_eq!((ha.first_pitch_range, hb.first_pitch_range), (1, 2));
    assert_eq!(hb.sample_rate, SampleRate::Hz44100);
    assert_eq!(hb.encoding, Encoding::Stereo);
    // Each sound's own samples are its own chunk.
    assert_eq!(adpcm_of(&set, &g, 1).len(), 5 * 36);
    assert_eq!(adpcm_of(&set, &g, 2).len(), 11 * 72);
}

#[test]
fn a_long_sound_is_cut_into_chunks_that_add_up() {
    // 2067 blocks of 64 frames at 44.1 kHz: 74,412 bytes.
    let clip = sines(1, 44_100, 2067 * 64);
    let wav = wav_of(&clip);
    let mut set = base();
    build(
        &mut set,
        &[sounds("s", &[("a.wav", "invented\\snd\\long", wav.as_slice())])],
        &target(),
        None,
    )
    .unwrap();
    let g = SoundGestalt::from_set(&set).unwrap();
    assert_eq!(g.chunks.len(), 3);
    assert!(g.chunks.iter().all(|c| c.size as usize <= h1_assets::sounds::convert::CHUNK_LEN));
    assert_eq!(g.chunks.iter().map(|c| c.size as usize).sum::<usize>(), 2067 * 36);
    // A chunk is whole blocks, so a stream can be cut anywhere between them.
    assert!(g.chunks.iter().all(|c| c.size % 36 == 0));
    assert_eq!(header_of(&set, "invented\\snd\\long").max_play_time, 2067 * 64 * 1000 / 44_100);
    let whole = adpcm_of(&set, &g, 0);
    assert_eq!(adpcm::decode(&whole, 1).unwrap().len(), 2067 * 64);
}

#[test]
fn a_clip_is_resampled_to_a_rate_the_format_names() {
    assert_eq!(import::target_rate(8_000), (0, 22_050));
    assert_eq!(import::target_rate(22_050), (0, 22_050));
    assert_eq!(import::target_rate(24_000), (2, 32_000));
    assert_eq!(import::target_rate(44_100), (1, 44_100));
    assert_eq!(import::target_rate(48_000), (1, 44_100));

    let e = encode(&sines(2, 48_000, 4800)).unwrap();
    assert_eq!((e.rate_code, e.rate, e.channels, e.frames), (1, 44_100, 2, 4410));
    // 4410 frames are 69 blocks of two channels.
    assert_eq!(e.adpcm.len(), 69 * 72);
    let back = adpcm::decode(&e.adpcm, 2).unwrap();
    assert_eq!(back.len(), 69 * 64 * 2);
    // The left tone keeps its level and the right stays at half of it.
    let peak = |ch: usize| {
        back.iter()
            .skip(ch)
            .step_by(2)
            .take(4000)
            .skip(200)
            .map(|v| i32::from(*v).abs())
            .max()
            .unwrap()
    };
    assert!((10_000..14_000).contains(&peak(0)), "{}", peak(0));
    assert!((4_000..8_000).contains(&peak(1)), "{}", peak(1));
}

#[test]
fn what_cannot_be_a_sound_is_refused_by_name() {
    // A tag the base has.
    let wav = wav_of(&sines(1, 22_050, 200));
    let taken = sounds("s", &[("a.wav", "invented\\snd\\boom", wav.as_slice())]);
    let err = build(&mut base(), &[taken], &target(), None).unwrap_err();
    assert!(err.why.contains("already in the map"), "{err}");
    // Two items on one path.
    let twice = sounds(
        "s",
        &[("a.wav", "invented\\snd\\a", wav.as_slice()), ("b.wav", "invented\\snd\\a", wav.as_slice())],
    );
    let err = build(&mut base(), &[twice], &target(), None).unwrap_err();
    assert!(err.why.contains("already added"), "{err}");
    // Not a WAV: the static check says so, before any map.
    let bad = sounds("s", &[("a.wav", "invented\\snd\\a", b"not a wav")]);
    assert!(check_package(&bad)[0].what.contains("RIFF"));
    assert!(encode(&Clip { channels: 3, rate: 22_050, pcm: vec![0; 30] }).is_err());
    assert!(encode(&Clip { channels: 1, rate: 22_050, pcm: Vec::new() }).is_err());
}

#[test]
fn a_sound_with_a_tag_changes_the_map_and_one_without_does_not() {
    let wav = wav_of(&sines(1, 22_050, 100));
    let tagged = sounds("s", &[("a.wav", "invented\\snd\\a", wav.as_slice())]);
    assert!(changes_map(&tagged.package.manifest.items[0]));
    let bare = pkg(
        "s",
        &[],
        &[("sound", "a.wav", None, "all")],
        &[("a.wav", wav.as_slice())],
    );
    assert!(!changes_map(&bare.package.manifest.items[0]));
    // The rewrite serves a map the tagged package changes.
    assert!(!PackageRewrite::new(vec![tagged]).is_empty());
    assert!(PackageRewrite::new(vec![bare]).is_empty());
}

#[test]
fn a_class_is_a_name_or_a_number() {
    assert_eq!(class_of("ui"), Ok(51));
    assert_eq!(class_of("Weapon_Fire"), Ok(4));
    assert_eq!(class_of("4"), Ok(4));
    assert!(class_of("3").is_err(), "3 is not a class");
    assert!(class_of("nope").unwrap_err().contains("weapon_fire"));
    assert_eq!(import::DEFAULT_CLASS_NAME, "ui");
    assert_eq!(class_of(import::DEFAULT_CLASS_NAME), Ok(import::DEFAULT_CLASS));
}

#[test]
fn a_sidecar_beside_the_wav_names_the_class() {
    let wav = wav_of(&sines(1, 22_050, 200));
    let with = |yaml: &'static str| {
        pkg(
            "s",
            &[],
            &[
                ("sound", "sounds/a.wav", Some("invented\\snd\\a"), "all"),
                ("data", "sounds/a.sound.yaml", None, "all"),
            ],
            &[
                ("sounds/a.wav", wav.as_slice()),
                ("sounds/a.sound.yaml", yaml.as_bytes()),
            ],
        )
    };
    assert_eq!(import::sidecar_path("sounds/a.wav"), "sounds/a.sound.yaml");
    assert_eq!(import::sidecar_path("a"), "a.sound.yaml");
    let mut set = base();
    build(&mut set, &[with("class: weapon_fire\n")], &target(), None).unwrap();
    assert_eq!(header_of(&set, "invented\\snd\\a").class, 4);
    // A class that is not one is refused by the static check, by name.
    let found = check_package(&with("class: nonsense\n"));
    assert!(found[0].what.contains("class"), "{found:?}");
    assert!(found[0].what.contains("nonsense"), "{found:?}");
}

#[test]
fn a_sidecars_edit_list_is_rendered_before_the_encode() {
    // 22,050 Hz: a 1000 ms clip trimmed to its first 500 ms is half the
    // frames, and the WAV in the package is the file it was.
    let wav = wav_of(&sines(1, 22_050, 22_050));
    let with = |yaml: &'static str| {
        pkg(
            "s",
            &[],
            &[
                ("sound", "sounds/a.wav", Some("invented\\snd\\a"), "all"),
                ("data", "sounds/a.sound.yaml", None, "all"),
            ],
            &[
                ("sounds/a.wav", wav.as_slice()),
                ("sounds/a.sound.yaml", yaml.as_bytes()),
            ],
        )
    };
    let whole = with("class: weapon_fire\n");
    let trimmed = with("edits:\n- op: trim\n  start_ms: 0\n  end_ms: 500\n- op: fade_out\n  ms: 100\n");
    assert_eq!(check_package(&trimmed), vec![]);
    let (mut a, mut b) = (base(), base());
    build(&mut a, &[whole], &target(), None).unwrap();
    build(&mut b, &[trimmed], &target(), None).unwrap();
    assert_eq!(header_of(&a, "invented\\snd\\a").max_play_time, 1000);
    assert_eq!(header_of(&b, "invented\\snd\\a").max_play_time, 500);
    // An edit that cannot apply is a named failure of the item.
    let bad = with("edits:\n- op: trim\n  start_ms: 5000\n");
    let found = check_package(&bad);
    assert!(found[0].what.contains("edit 0"), "{found:?}");
}
