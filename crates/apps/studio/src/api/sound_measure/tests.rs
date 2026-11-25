//! `sound.measure` and its meter over invented signals: sines of known
//! level written as WAV files, silence, a clipped square, and the sound
//! crate's invented ADPCM `snd!` (no game data).
use std::path::PathBuf;

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::meter::{self, Levels};
use audio::meter::SILENCE_DB;
use crate::api::{Api, Reply};
use crate::app::tag_tests;

const SND: &str = r"snd!:objects\test\blaster";

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-measure-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `seconds` of a `hz` sine at `amplitude` (of full scale) on each of
/// `channels`, 16-bit at `rate`.
fn sine(amplitude: f64, hz: f64, rate: u32, seconds: f64, channels: u16) -> Vec<i16> {
    let frames = (f64::from(rate) * seconds) as usize;
    let mut out = Vec::with_capacity(frames * usize::from(channels));
    for i in 0..frames {
        let v = amplitude * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin();
        let s = (v * 32767.0).round() as i16;
        for _ in 0..channels {
            out.push(s);
        }
    }
    out
}

/// A RIFF WAVE file of 16-bit PCM.
fn wav(pcm: &[i16], channels: u16, rate: u32) -> Vec<u8> {
    let data: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2 * u32::from(channels)).to_le_bytes());
    b.extend_from_slice(&(2 * channels).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(&data);
    b
}

fn near(a: f64, b: f64, within: f64) -> bool {
    (a - b).abs() <= within
}

#[test]
fn a_sine_reads_its_peak_rms_and_loudness() {
    // Half scale: peak -6.02 dBFS, RMS 3.01 dB under it. A mono 997 Hz
    // sine's loudness is its RMS (K-weighting is +0.69 dB there, which
    // BS.1770's -0.691 offsets).
    let l = meter::measure(&sine(0.5, 997.0, 48_000, 2.0, 1), 1, 48_000);
    assert!(near(l.peak_dbfs, -6.02, 0.03), "{l:?}");
    assert!(near(l.rms_dbfs, -9.03, 0.01), "{l:?}");
    assert!(near(l.lufs, -9.03, 0.1), "{l:?}");
    assert!(near(l.max_momentary_lufs, -9.03, 0.1), "{l:?}");
    assert_eq!((l.frames, l.duration_ms, l.clipped), (96_000, 2000, 0));
    // The same level 12 dB down reads 12 dB less, at another rate.
    let q = meter::measure(&sine(0.125, 997.0, 22_050, 2.0, 1), 1, 22_050);
    assert!(near(q.lufs, l.lufs - 12.04, 0.3), "{q:?}");
    assert!(near(q.peak_dbfs, -18.06, 0.05), "{q:?}");
}

#[test]
fn stereo_sums_its_channels_power() {
    // Two equal channels are 3 dB louder than one (each weighs 1).
    let mono = meter::measure(&sine(0.25, 997.0, 48_000, 1.0, 1), 1, 48_000);
    let stereo = meter::measure(&sine(0.25, 997.0, 48_000, 1.0, 2), 2, 48_000);
    assert!(
        near(stereo.lufs, mono.lufs + 3.01, 0.05),
        "{mono:?} {stereo:?}"
    );
    assert!(near(stereo.rms_dbfs, mono.rms_dbfs, 0.01));
    assert_eq!(stereo.frames, mono.frames);
}

#[test]
fn silence_and_a_short_clip_have_levels() {
    let s = meter::measure(&[0; 4800], 1, 48_000);
    assert_eq!(
        (s.peak_dbfs, s.rms_dbfs, s.lufs),
        (SILENCE_DB, SILENCE_DB, SILENCE_DB)
    );
    let e = meter::measure(&[], 1, 48_000);
    assert_eq!((e.frames, e.lufs), (0, SILENCE_DB));
    // 50 ms, shorter than a gating block: one block, still its level.
    let short = meter::measure(&sine(0.5, 997.0, 48_000, 0.05, 1), 1, 48_000);
    assert!(near(short.lufs, -9.03, 0.5), "{short:?}");
}

#[test]
fn full_scale_samples_count_as_clipped() {
    let square: Vec<i16> = (0..1000)
        .map(|i| if i % 2 == 0 { i16::MAX } else { i16::MIN })
        .collect();
    let l = meter::measure(&square, 1, 48_000);
    assert_eq!(l.clipped, 1000);
    assert!(near(l.peak_dbfs, 0.0, 0.01));
    assert_eq!(
        meter::measure(&sine(0.9, 440.0, 48_000, 0.5, 1), 1, 48_000).clipped,
        0
    );
}

#[test]
fn the_verdict_compares_with_the_p90() {
    let values: Vec<f64> = (1..=10).map(|i| -f64::from(i) * 3.0).collect(); // -3..-30
    let s = meter::spread(&values).unwrap();
    assert_eq!((s.p10, s.p50, s.p90, s.max), (-30.0, -18.0, -6.0, -3.0));
    assert_eq!(meter::spread(&[]), None);
    let at = |lufs: f64| Levels {
        lufs,
        ..meter::measure(&[0; 10], 1, 48_000)
    };
    let loud = meter::verdict(&at(-1.0), Some(s), None, "the map's sounds");
    assert_eq!(loud.level, "loud");
    assert_eq!(loud.over_p90_db, Some(5.0));
    assert_eq!(
        loud.text,
        "louder than the p90 of the map's sounds by 5.0 dB"
    );
    assert_eq!(meter::verdict(&at(-6.5), Some(s), None, "x").level, "ok");
    assert_eq!(
        meter::verdict(&at(-40.0), Some(s), None, "x").level,
        "quiet"
    );
    assert_eq!(
        meter::verdict(&at(-6.0), None, None, "x").level,
        "no_reference"
    );
}

fn open(t: &Temp) -> (crate::App, Api) {
    let (mut app, ..) = tag_tests::app_with_registry(
        &t.0,
        crate::api::sound::tests::sound_map(),
        tag_tests::registry(),
    );
    app.audition.headless = true;
    (app, Api::new())
}

fn call(api: &mut Api, app: &mut crate::App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

fn code_of(r: Reply) -> i64 {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e.code,
    }
}

#[test]
fn sound_measure_reads_a_wav_file_against_the_maps_sounds() {
    let t = Temp::new("file");
    let (mut app, mut api) = open(&t);
    let path = t.0.join("loud.wav");
    std::fs::write(&path, wav(&sine(0.5, 997.0, 48_000, 1.0, 1), 1, 48_000)).unwrap();
    let r = call(
        &mut api,
        &mut app,
        m::SOUND_MEASURE,
        json!({"path": path.display().to_string()}),
    );
    assert_eq!(r["from"], "file");
    assert_eq!(r["levels"]["peak_dbfs"], -6.0);
    assert_eq!(r["levels"]["rms_dbfs"], -9.0);
    assert_eq!(r["levels"]["duration_ms"], 1000);
    // The invented map's one sound is the reference: its DC steps are far
    // quieter than a half-scale sine.
    assert_eq!(r["reference"]["considered"], 1);
    assert_eq!(r["reference"]["decoded"], 1);
    assert_eq!(r["reference"]["class"], Value::Null);
    assert_eq!(r["verdict"]["level"], "loud", "{r}");
    assert!(r["verdict"]["over_p90_db"].as_f64().unwrap() > 1.0);
    assert!(
        r["verdict"]["text"]
            .as_str()
            .unwrap()
            .starts_with("louder than")
    );
}

#[test]
fn sound_measure_reads_each_permutation_of_a_tag() {
    let t = Temp::new("tag");
    let (mut app, mut api) = open(&t);
    let r = call(&mut api, &mut app, m::SOUND_MEASURE, json!({"tag": SND}));
    assert_eq!(r["from"], "tag");
    assert_eq!(r["class"], 0);
    // Block k holds the constant k * 10 (sound::tests::sound_map).
    assert_eq!(r["levels"]["peak_dbfs"], -14.8);
    assert_eq!(r["levels"]["rms_dbfs"], -19.5);
    assert_eq!(r["levels"]["channels"], 1);
    assert_eq!(r["permutations"].as_array().unwrap().len(), 1);
    // The tag is left out of its own reference set: nothing is left.
    assert_eq!(r["reference"]["considered"], 0);
    assert_eq!(r["verdict"]["level"], "no_reference");
    // Out of range.
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::SOUND_MEASURE,
            json!({"tag": SND, "permutation": 3})
        )),
        code::INVALID_PARAMS
    );
}

#[test]
fn sound_measure_takes_exactly_one_subject() {
    let t = Temp::new("params");
    let (mut app, mut api) = open(&t);
    for bad in [
        json!({}),
        json!({"tag": SND, "clip": "pogo/spring"}),
        json!({"clip": "pogo/nothing"}),
        json!({"tag": SND, "limit": 0}),
        json!({"tag": SND, "class": 300}),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, m::SOUND_MEASURE, bad.clone())),
            code::INVALID_PARAMS,
            "{bad}"
        );
    }
    // An unknown clip's error lists ours.
    let e = api
        .call(&mut app, m::SOUND_MEASURE, json!({"clip": "nope"}))
        .unwrap_err();
    assert!(e.message.contains("pogo/spring"), "{}", e.message);
    // A file that is not a WAV fails by name.
    let junk = t.0.join("junk.wav");
    std::fs::write(&junk, b"OggS....").unwrap();
    let e = api
        .call(
            &mut app,
            m::SOUND_MEASURE,
            json!({"path": junk.display().to_string()}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::FAILED);
    assert!(e.message.contains("not a RIFF WAVE"), "{}", e.message);
}

#[test]
fn our_clips_are_the_pogo_and_announcer_wavs() {
    let ids: Vec<String> = super::our_clips().into_iter().map(|(id, _)| id).collect();
    for want in [
        "pogo/compress",
        "pogo/spring",
        "pogo/settle",
        "announcer/superbounce",
    ] {
        assert!(ids.iter().any(|i| i == want), "{want} in {ids:?}");
    }
    // Each one reads, or (a checkout without Git LFS) is a named pointer.
    for (id, bytes) in super::our_clips() {
        match super::wav_levels(bytes) {
            Ok(l) => assert!(l.frames > 0, "{id}"),
            Err(e) => assert!(e.contains("git lfs pull"), "{id}: {e}"),
        }
    }
}

#[test]
fn the_sounds_check_names_a_loud_clip() {
    // The check over a tag of the map: no reference (it is the map's only
    // sound), so a note says so; nothing is loud or clipped.
    let t = Temp::new("check");
    let (mut app, _) = open(&t);
    let found = super::check(&mut app, Some(SND)).unwrap();
    assert!(
        found.iter().any(|f| f.code == "sound.no_reference"),
        "{found:?}"
    );
    assert!(found.iter().all(|f| f.area == "sounds"));
    assert!(
        !found
            .iter()
            .any(|f| f.code == "sound.loud" || f.code == "sound.clipping")
    );
    // Our clips, by prefix: each is measured or named unreadable.
    let found = super::check(&mut app, Some("pogo")).unwrap();
    for f in &found {
        assert!(
            [
                "sound.loud",
                "sound.clipping",
                "sound.unreadable",
                "sound.no_reference"
            ]
            .contains(&f.code.as_str()),
            "{f:?}"
        );
        if f.code != "sound.no_reference" {
            assert!(f.target.starts_with("pogo/"), "{f:?}");
            assert_eq!(
                f.path.as_deref(),
                Some(format!("art/{}.wav", f.target).as_str())
            );
        }
    }
}
