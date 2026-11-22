//! Sound audition on invented sounds: Xbox ADPCM blocks built here, a
//! gestalt and a raw-resource file built here. No game data.
use std::sync::Arc;

use audio::sink::{AudioSink, NullSink};
use map::ResourceFile;
use tags::sound::{Compression, Encoding, Gestalt, SoundHeader};

use super::audition::{
    self, Clip, NEEDS_EXECUTABLE, Output, Playback, SoundError, decode, decode_permutation,
};

/// One mono Xbox ADPCM block (36 bytes) whose first sample is `first` and
/// whose nibbles are all zero.
fn block(first: i16) -> Vec<u8> {
    let mut b = first.to_le_bytes().to_vec();
    b.extend([0, 0]);
    b.extend([0u8; 32]);
    b
}

/// One stereo Xbox ADPCM block (72 bytes): the two channels' headers, then
/// their (zero) nibbles.
fn stereo_block(left: i16, right: i16) -> Vec<u8> {
    let mut b = left.to_le_bytes().to_vec();
    b.extend([0, 0]);
    b.extend(right.to_le_bytes());
    b.extend([0, 0]);
    b.extend([0u8; 64]);
    b
}

/// A header: `rate` 1 is 44.1 kHz, `encoding` 0 mono, `compression` 1 Xbox
/// ADPCM, one pitch range from 0.
fn header(rate: u8, encoding: u8, compression: u8) -> SoundHeader {
    let mut b = [0u8; 20];
    b[3] = rate;
    b[4] = encoding;
    b[5] = compression;
    b[10] = 1;
    SoundHeader::decode(&b).unwrap()
}

#[test]
fn an_adpcm_sound_decodes_to_its_frames() {
    let h = header(1, 0, 1);
    let data = [block(1000), block(-5), block(7)].concat();
    let clip = decode(&h, &data, None).unwrap();
    assert_eq!((clip.rate, clip.channels), (44100, 1));
    assert_eq!(clip.frames(), 3 * 64);
    assert_eq!(clip.pcm[0], 1000);
    assert_eq!(clip.pcm[64], -5);
    assert_eq!(clip.duration_ms(), 192 * 1000 / 44100);
    // A stereo sound at 22.05 kHz: one block holds both channels.
    let h = header(0, 1, 1);
    let clip = decode(&h, &stereo_block(3, 4), None).unwrap();
    assert_eq!((clip.rate, clip.channels, clip.frames()), (22050, 2, 64));
    assert_eq!((clip.pcm[0], clip.pcm[1]), (3, 4));
}

#[test]
fn a_bad_length_and_an_unknown_rate_are_named() {
    let h = header(1, 0, 1);
    assert!(matches!(
        decode(&h, &[0; 35], None),
        Err(SoundError::Failed(m)) if m.contains("ADPCM")
    ));
    let h = header(9, 0, 1);
    assert!(matches!(
        decode(&h, &block(1), None),
        Err(SoundError::Refused(m)) if m.contains("sample rate")
    ));
}

#[test]
fn a_codec_stream_without_the_executable_is_refused_by_name() {
    for compression in [1, 4] {
        let h = header(1, 2, compression);
        assert!(audition::needs_executable(&h));
        assert_eq!(
            decode(&h, &[0; 64], None),
            Err(SoundError::Refused(NEEDS_EXECUTABLE.into()))
        );
    }
    // An executable that is not the verified one never supplies tables.
    assert!(matches!(
        audition::load_tables(b"XBEH not the game"),
        Err(SoundError::Refused(_))
    ));
}

#[test]
fn other_compressions_are_refused_by_name() {
    for compression in [0u8, 2, 3, 7] {
        let h = header(1, 0, compression);
        let Err(SoundError::Refused(why)) = decode(&h, &[0; 36], None) else {
            panic!("compression {compression} was not refused");
        };
        assert!(why.contains("not decoded"), "{why}");
    }
    assert_eq!(header(1, 0, 3).compression, Compression::NoneLittleEndian);
    assert_eq!(header(1, 2, 4).encoding, Encoding::Codec);
}

#[test]
fn peaks_cover_the_clip_and_stereo_follows_the_rate() {
    let clip = Clip {
        rate: 1000,
        channels: 1,
        pcm: vec![0, 100, -50, 20, 7, 7, 7, 7].into(),
    };
    assert_eq!(clip.peaks(2), vec![[-50, 100], [7, 7]]);
    assert_eq!(clip.peaks(8).len(), 8);
    assert!(clip.peaks(0).is_empty());
    let s = clip.stereo(1000);
    assert_eq!(s.len(), 16);
    assert_eq!((s[2], s[3]), (100.0 / 32768.0, 100.0 / 32768.0));
    // Twice the rate, twice the frames.
    assert_eq!(clip.stereo(2000).len(), 2 * 16);
    let two = Clip {
        rate: 1000,
        channels: 2,
        pcm: vec![1000, -1000, 2000, -2000].into(),
    };
    let s = two.stereo(1000);
    assert_eq!(
        s,
        vec![
            1000.0 / 32768.0,
            -1000.0 / 32768.0,
            2000.0 / 32768.0,
            -2000.0 / 32768.0
        ]
    );
}

/// The clip plays through the headless sink to its sample count, and the
/// sink was fed exactly the clip's samples.
#[test]
fn a_clip_plays_through_the_headless_sink_to_its_sample_count() {
    let h = header(1, 0, 1);
    let data: Vec<u8> = (0..20).flat_map(|k| block(k * 300)).collect();
    let clip = decode(&h, &data, None).unwrap();
    assert_eq!(clip.frames(), 20 * 64);
    let samples: Arc<[f32]> = clip.stereo(48000).into();
    let frames = samples.len() / 2;
    assert_eq!(frames, 1280 * 48000 / 44100);

    let mut sink = NullSink::new(48000, 1 << 12).capture();
    let mut play = Playback::new(samples.clone(), 0);
    let mut guard = 0;
    while !play.done() || sink.queued_frames() > 0 {
        play.feed(&mut sink);
        // Never more than the sink holds, so no silence is substituted.
        let q = sink.queued_frames();
        sink.advance(q.min(256));
        guard += 1;
        assert!(guard < 10_000);
    }
    assert_eq!(sink.captured().len(), samples.len());
    assert_eq!(sink.captured(), &samples[..]);
    assert_eq!(sink.stats().underruns, 0);
    assert_eq!(play.position(&sink), frames);
}

#[test]
fn playback_starts_at_the_frame_asked_for() {
    let samples: Arc<[f32]> = (0..2000).map(|k| k as f32).collect::<Vec<_>>().into();
    let mut sink = NullSink::new(1000, 1 << 10).capture();
    let mut play = Playback::new(samples.clone(), 400);
    assert_eq!(play.position(&sink), 400);
    while !play.done() || sink.queued_frames() > 0 {
        play.feed(&mut sink);
        let q = sink.queued_frames();
        sink.advance(q.min(64));
    }
    assert_eq!(sink.captured(), &samples[800..]);
    // Past the end is the end.
    let play = Playback::new(samples, 5000);
    assert!(play.done());
}

/// An invented gestalt: one pitch range, one permutation of one chunk of
/// `blocks` mono ADPCM blocks at offset 0x10 of the local resource file.
fn gestalt(blocks: u32) -> Vec<u8> {
    const BASE: u32 = 0x8000_0000;
    let mut b = vec![0u8; 0x58];
    let put = |b: &mut Vec<u8>, idx: usize, body: &[u8]| {
        let at = BASE + b.len() as u32;
        b[idx * 8..idx * 8 + 4].copy_from_slice(&1u32.to_le_bytes());
        b[idx * 8 + 4..idx * 8 + 8].copy_from_slice(&at.to_le_bytes());
        b.extend_from_slice(body);
    };
    let mut range = vec![0u8; 8];
    range.extend([0, 0, 1, 0]);
    put(&mut b, 4, &range);
    let size = blocks * 36;
    let mut perm = vec![0u8; 8];
    perm.extend(size.to_le_bytes());
    perm.extend([0, 0, 1, 0]);
    put(&mut b, 5, &perm);
    let mut chunk = 0x10u32.to_le_bytes().to_vec();
    chunk.extend(size.to_le_bytes());
    chunk.extend(u32::MAX.to_le_bytes());
    put(&mut b, 8, &chunk);
    b
}

/// A permutation is read from the raw-resource file through the same reader
/// the window uses, then decoded.
#[test]
fn a_permutation_is_read_from_the_resource_file_and_decoded() {
    let h = header(1, 0, 1);
    let g = Gestalt::decode(&gestalt(2), 0x8000_0000).unwrap();
    let sound = g.resolve(&h).unwrap();
    let p = sound.ranges[0].permutations[0];
    let mut file = vec![0xeeu8; 0x10];
    file.extend([block(11), block(22)].concat());
    let mut files = assets::ResourceFiles::new();
    files.set_bytes(ResourceFile::Local, file.into());
    let clip = decode_permutation(&h, &g, &p, &mut super::audition::raw(&mut files), None).unwrap();
    assert_eq!(clip.frames(), 128);
    assert_eq!((clip.pcm[0], clip.pcm[64]), (11, 22));

    // A file too short for the chunk is a named failure, not a short clip.
    let mut short = assets::ResourceFiles::new();
    short.set_bytes(ResourceFile::Local, vec![0u8; 0x20].into());
    assert!(matches!(
        decode_permutation(&h, &g, &p, &mut super::audition::raw(&mut short), None),
        Err(SoundError::Failed(_))
    ));
    // No Local file at all is named too.
    let mut none = assets::ResourceFiles::new();
    assert!(matches!(
        decode_permutation(&h, &g, &p, &mut super::audition::raw(&mut none), None),
        Err(SoundError::Failed(m)) if m.contains("size was not supplied")
    ));
    // A codec stream is refused before anything is read.
    let wma = header(1, 2, 4);
    assert_eq!(
        decode_permutation(&wma, &g, &p, &mut super::audition::raw(&mut none), None),
        Err(SoundError::Refused(NEEDS_EXECUTABLE.into()))
    );
}

/// The output thread plays a clip in real time on the headless sink,
/// reports its position and stops on request.
#[test]
fn the_headless_output_plays_reports_a_position_and_stops() {
    let out = Output::paced_null(8000).unwrap();
    assert_eq!(
        (out.rate(), out.name(), out.playing()),
        (8000, "headless", false)
    );
    // Ten seconds, so it is still playing when it is stopped.
    let samples: Arc<[f32]> = vec![0.25f32; 8000 * 10 * 2].into();
    out.play(samples, 4000);
    assert!(out.playing());
    let start = std::time::Instant::now();
    while out.position() <= 4000 {
        assert!(start.elapsed().as_secs() < 5, "the position never moved");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(out.position() < 8000 * 10);
    assert!(out.playing());
    out.stop();
    assert!(!out.playing());
    // A short clip ends by itself.
    out.play(vec![0.0f32; 200].into(), 0);
    let start = std::time::Instant::now();
    while out.playing() {
        assert!(start.elapsed().as_secs() < 5, "the clip never ended");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
