//! `audio.meter` over invented signals: a sine fed through the tap, a
//! WAV clip played into a real mixer that renders into a `NullSink`, and
//! the host clips' names.
use super::*;

const RATE: u32 = 48_000;
/// One render quantum of stereo samples, as the output feeds the tap.
const BLOCK: usize = 256 * 2;

fn near(a: f64, b: f64, within: f64) -> bool {
    (a - b).abs() <= within
}

fn connected() -> (Meter, Arc<Mutex<audio::Mixer>>) {
    let mixer = Arc::new(Mutex::new(audio::Mixer::new(RATE)));
    let mut meter = Meter::new();
    meter.connect(Arc::clone(&mixer));
    (meter, mixer)
}

/// `seconds` of a stereo `hz` sine at `amplitude`, from frame `start`.
fn sine(amplitude: f32, hz: f32, start: usize, frames: usize) -> Vec<f32> {
    (start..start + frames)
        .flat_map(|i| {
            let v = amplitude * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin();
            [v, v]
        })
        .collect()
}

/// A mono 16-bit WAV of a `hz` sine at `amplitude`, `ms` long.
fn wav(amplitude: f64, hz: f64, rate: u32, ms: u32) -> Vec<u8> {
    let frames = (u64::from(rate) * u64::from(ms) / 1000) as usize;
    let data: Vec<u8> = (0..frames)
        .flat_map(|i| {
            let v = amplitude * (std::f64::consts::TAU * hz * i as f64 / f64::from(rate)).sin();
            ((v * 32767.0).round() as i16).to_le_bytes()
        })
        .collect();
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    b
}

#[test]
fn a_sine_through_the_tap_gives_its_peak_rms_and_loudness() {
    let (mut meter, _mixer) = connected();
    // Before arming, the tap keeps nothing.
    meter.feed(&sine(0.5, 997.0, 0, 256));
    assert!(!meter.armed());
    let mut pending = meter.begin(MeterRequest::Window { ms: 1000 }).unwrap();
    assert!(meter.armed());
    // Until a second is captured it waits.
    let mut fed = 0;
    while fed < RATE as usize / 2 {
        meter.feed(&sine(0.5, 997.0, fed, BLOCK / 2));
        fed += BLOCK / 2;
    }
    assert!(pending.poll(&meter).is_none());
    while fed < RATE as usize * 3 / 2 {
        meter.feed(&sine(0.5, 997.0, fed, BLOCK / 2));
        fed += BLOCK / 2;
    }
    let r = pending.poll(&meter).unwrap().unwrap();
    assert_eq!(
        (r.window_ms, r.frames, r.rate, r.channels),
        (1000, 48_000, RATE, 2)
    );
    // Half scale: peak -6.0 dBFS, RMS 3 dB under it. Both channels carry
    // the sine, so the loudness is the RMS plus 3 dB (BS.1770 weighs each
    // channel 1; K-weighting's +0.69 dB at 997 Hz offsets the -0.691).
    assert!(near(r.peak_dbfs, -6.0, 0.1), "{r:?}");
    assert!(near(r.rms_dbfs, -9.0, 0.1), "{r:?}");
    assert!(near(r.lufs, -6.0, 0.2), "{r:?}");
    assert!(near(r.max_momentary_lufs, -6.0, 0.2), "{r:?}");
    assert_eq!(r.clipped, 0);
    assert!(r.play.is_none());
}

#[test]
fn the_capture_keeps_its_last_seconds_and_counts_from_arming() {
    let mut c = Capture::new(1000);
    c.feed(&[0.5; 2 * 4000]);
    c.feed(&[0.25; 2 * 8000]);
    c.feed(&[0.1, 0.1, 0.9]); // a half frame is dropped
    assert_eq!(c.total_frames(), 12_001);
    assert_eq!(c.held_frames(), u64::from(KEEP_SECONDS) * 1000);
    assert_eq!(c.last(1).unwrap(), [0.1, 0.1]);
    assert_eq!(c.span(2000, 2001), None, "fell out of the capture");
    assert_eq!(c.span(2001, 2002).unwrap(), [0.5, 0.5]);
    assert_eq!(c.span(4000, 4001).unwrap(), [0.25, 0.25]);
    assert_eq!(c.span(12_000, 12_002), None, "not rendered yet");
    assert_eq!(c.last(20_000), None);
}

#[test]
fn a_meter_with_no_mixer_is_unavailable() {
    let mut meter = Meter::new();
    for request in [
        MeterRequest::Window { ms: 400 },
        MeterRequest::Play {
            clip: "pogo/spring".into(),
        },
    ] {
        let err = meter.begin(request).err().unwrap();
        // An LFS-pointer clip fails before the mixer is asked; either way
        // nothing plays.
        assert_eq!(err.code, code::UNAVAILABLE, "{}", err.message);
    }
    assert!(!meter.armed());
}

#[test]
fn every_host_clip_has_a_name_and_an_unknown_one_lists_them() {
    let names = clip_names();
    for n in [
        "pogo/compress",
        "pogo/spring",
        "pogo/settle",
        "announcer/superbounce",
    ] {
        assert!(names.iter().any(|x| x == n), "{n}");
        assert!(clip_bytes(n).is_some(), "{n}");
    }
    assert_eq!(names.len(), 3 + crate::medal_announcer::CLIPS.len());
    assert_eq!(
        clip_bytes("pogo/spring"),
        Some(crate::pogo_sound::CLIPS[1].2)
    );
    for bad in ["spring", "pogo/", "hud/spring", "announcer/incel"] {
        assert!(clip_bytes(bad).is_none(), "{bad}");
    }
    let (mut meter, mixer) = connected();
    let err = meter
        .begin(MeterRequest::Play {
            clip: "pogo/boing".into(),
        })
        .err()
        .unwrap();
    assert_eq!(err.code, code::INVALID_PARAMS);
    assert!(err.message.contains("pogo/settle"), "{}", err.message);
    assert_eq!(lock(&mixer).buffer_count(), 0);
}

#[test]
fn play_starts_a_named_cue() {
    let (mut meter, mixer) = connected();
    for (_, id, bytes) in crate::pogo_sound::CLIPS {
        let clip = format!("pogo/{id}");
        match meter.begin(MeterRequest::Play { clip: clip.clone() }) {
            Ok(pending) => {
                // The clip is a playing buffer in the mixer until answered.
                let m = lock(&mixer);
                assert_eq!(m.buffer_count(), 1, "{clip}");
                assert_eq!(m.active_voices(), 1, "{clip}");
                drop(m);
                drop(pending);
                assert_eq!(lock(&mixer).buffer_count(), 0, "{clip}");
            }
            // A checkout without LFS holds pointers: named, and silent.
            Err(e) => {
                assert!(
                    bytes.starts_with(b"version https://git-lfs"),
                    "{clip}: {}",
                    e.message
                );
                assert_eq!(e.code, code::UNAVAILABLE);
            }
        }
    }
}

#[test]
fn a_played_clip_meters_through_a_mixer_into_the_null_sink() {
    // No device: the mixer renders into a NullSink, the tap sees each block.
    let (mut meter, mixer) = connected();
    let mut sink = audio::NullSink::new(RATE, RATE as usize).capture();
    let mut pending = meter
        .play("test/sine".into(), &wav(0.5, 997.0, 44_100, 300))
        .unwrap();
    let mut blocks = 0;
    let reply = loop {
        let mut block = [0f32; BLOCK];
        lock(&mixer).render(&mut block);
        meter.feed(&block);
        assert_eq!(audio::AudioSink::write(&mut sink, &block), BLOCK / 2);
        sink.advance(BLOCK / 2);
        blocks += 1;
        if let Some(r) = pending.poll(&meter) {
            break r.unwrap();
        }
        assert!(blocks < 200, "the clip never finished");
    };
    let play = reply.play.as_ref().unwrap();
    assert_eq!((play.clip.as_str(), play.duration_ms), ("test/sine", 300));
    assert_eq!(reply.window_ms, 300 + TAIL_MS);
    assert_eq!(reply.frames, (300 + TAIL_MS) * u64::from(RATE) / 1000);
    // What the sink played is what was metered.
    let played = audio::meter::measure_f32(sink.captured(), CHANNELS, RATE);
    assert!(
        near(reply.peak_dbfs, round1(played.peak_dbfs), 0.05),
        "{reply:?} {played:?}"
    );
    assert!(
        reply.peak_dbfs > -30.0 && reply.peak_dbfs < 0.0,
        "{reply:?}"
    );
    assert!(reply.lufs > -40.0, "{reply:?}");
    assert_eq!(reply.clipped, 0);
    // Answered: the buffer left the mixer.
    assert_eq!(lock(&mixer).buffer_count(), 0);
}
