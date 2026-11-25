//! The voice pieces over invented signals and a fake source. No network,
//! no Keychain.
use std::path::PathBuf;

use super::fake::{FixedKey, signal};
use super::fish::{self, Fish};
use super::key::{self, KeyError, Secret};
use super::{Cache, DEFAULT_VOICE, Pcm, Request, VoiceError, VoiceSource, audio};
use crate::api::sound_measure::meter;

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-voice-{tag}-{}", std::process::id()));
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

fn near(a: f64, b: f64, within: f64) -> bool {
    (a - b).abs() <= within
}

#[test]
fn normalising_brings_a_quiet_signal_to_the_target() {
    let quiet = signal(0.05, 1.0);
    let before = meter::measure(&quiet, 1, audio::RATE);
    let n = audio::normalise(&quiet, audio::RATE, -14.5, audio::CEILING_DBFS).unwrap();
    assert!(!n.limited);
    assert!(near(n.before.lufs, before.lufs, 1e-9));
    let after = meter::measure(&n.pcm, 1, audio::RATE);
    assert!(near(after.lufs, -14.5, 0.1), "{after:?}");
    assert!(near(n.gain_db, -14.5 - before.lufs, 1e-9));
    assert!(after.peak_dbfs <= audio::CEILING_DBFS + 0.01, "{after:?}");
    assert_eq!(after.clipped, 0);
}

#[test]
fn normalising_brings_a_loud_signal_down() {
    let loud = signal(0.8, 1.0);
    let n = audio::normalise(&loud, audio::RATE, -20.0, audio::CEILING_DBFS).unwrap();
    assert!(n.gain_db < 0.0 && !n.limited);
    let after = meter::measure(&n.pcm, 1, audio::RATE);
    assert!(near(after.lufs, -20.0, 0.1), "{after:?}");
}

#[test]
fn the_peak_ceiling_holds_the_gain_under_the_target() {
    // A quiet tone with one near-full-scale click: reaching the target
    // would put the click over the ceiling.
    let mut s = signal(0.02, 1.0);
    s[30_000] = 30_000;
    let n = audio::normalise(&s, audio::RATE, -14.5, audio::CEILING_DBFS).unwrap();
    assert!(n.limited);
    let after = meter::measure(&n.pcm, 1, audio::RATE);
    assert!(near(after.peak_dbfs, audio::CEILING_DBFS, 0.05), "{after:?}");
    assert!(after.lufs < -14.5, "{after:?}");
}

#[test]
fn a_silent_line_is_refused() {
    let e = audio::normalise(&vec![0; 44_100], audio::RATE, -14.5, audio::CEILING_DBFS).unwrap_err();
    assert!(matches!(e, VoiceError::Audio(ref m) if m.contains("silent")), "{e}");
}

#[test]
fn trimming_takes_the_silence_off_both_ends_and_leaves_a_pad() {
    let s = signal(0.1, 1.0);
    assert_eq!(s.len(), 22_050 + 44_100 + 22_050);
    let (out, cut) = audio::trim_silence(&s, audio::RATE, 20, 60).unwrap();
    let head = 44_100 * 20 / 1000;
    let tail = 44_100 * 60 / 1000;
    // The tone's first sample is zero, so the cut is a frame short of the
    // silence's end.
    assert!((22_050 - head..=22_050 - head + 2).contains(&cut.head), "{cut:?}");
    assert_eq!(cut.tail, 22_050 - tail);
    assert_eq!(out.len(), s.len() - cut.head - cut.tail);
    assert!(out.len().abs_diff(head + 44_100 + tail) <= 2);
    assert!(out[..head].iter().all(|&v| v == 0));
    assert!(out[out.len() - tail..].iter().all(|&v| v == 0));
    // All silence has nothing to keep.
    assert!(audio::trim_silence(&vec![0; 1000], audio::RATE, 20, 60).is_none());
}

#[test]
fn a_wav_written_reads_back_in_the_announcer_format() {
    let s = signal(0.1, 0.2);
    let bytes = audio::write_wav(&s, audio::RATE);
    // The game's own reader accepts it: 44.1 kHz mono 16-bit.
    let clip = boot::medal_announcer::parse_wav(&bytes).unwrap();
    assert_eq!((clip.channels, clip.rate), (1, 44_100));
    assert_eq!(clip.data.len(), s.len() * 2);
    let back = audio::read_wav(&bytes).unwrap();
    assert_eq!(back, Pcm { rate: 44_100, samples: s });
}

#[test]
fn a_raw_reply_is_pcm_and_a_riff_reply_is_a_wav() {
    let s = [1i16, -2, 300];
    let raw: Vec<u8> = s.iter().flat_map(|v| v.to_le_bytes()).chain([9]).collect();
    // The odd byte is dropped.
    assert_eq!(audio::from_response(&raw).unwrap().samples, s);
    let wav = audio::write_wav(&s, 22_050);
    let p = audio::from_response(&wav).unwrap();
    assert_eq!((p.rate, p.samples.as_slice()), (22_050, &s[..]));
    assert!(matches!(audio::from_response(&[]), Err(VoiceError::Audio(_))));
}

#[test]
fn the_cache_key_follows_every_setting_and_only_those() {
    let a = Request::new("Superbounce").unwrap();
    assert_eq!(a.voice, DEFAULT_VOICE);
    assert_eq!(a.key(), Request::new("  Superbounce ").unwrap().key());
    assert_eq!(a.key().len(), 64);
    let mut b = a.clone();
    b.text = "Double bounce".into();
    let mut c = a.clone();
    c.voice = "abc".into();
    let mut d = a.clone();
    d.speed = 1.1;
    let mut e = a.clone();
    e.model = Some("s2.1-pro".into());
    let keys = [a.key(), b.key(), c.key(), d.key(), e.key()];
    for (i, x) in keys.iter().enumerate() {
        for y in &keys[i + 1..] {
            assert_ne!(x, y);
        }
    }
}

#[test]
fn a_request_is_checked() {
    assert!(matches!(Request::new("   "), Err(VoiceError::Invalid(_))));
    assert!(matches!(Request::new(&"x".repeat(501)), Err(VoiceError::Invalid(_))));
    let mut r = Request::new("hi").unwrap();
    r.speed = 3.0;
    assert!(matches!(r.checked(), Err(VoiceError::Invalid(m)) if m.starts_with("speed")));
    let mut r = Request::new("hi").unwrap();
    r.voice = "../etc".into();
    assert!(matches!(r.checked(), Err(VoiceError::Invalid(m)) if m.starts_with("voice")));
}

#[test]
fn the_cache_keeps_a_line_and_lists_what_was_said() {
    let t = Temp::new("cache");
    let cache = Cache::new(&t.0);
    let req = Request::new("Monster bounce").unwrap();
    assert!(cache.get(&req.key()).is_none());
    let pcm = Pcm { rate: 44_100, samples: signal(0.1, 0.1) };
    cache.put(&req, &pcm).unwrap();
    assert_eq!(cache.get(&req.key()), Some(pcm));
    let lines = cache.lines();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].1, req);
    // No temporary file is left behind.
    assert!(std::fs::read_dir(cache.dir()).unwrap().all(|e| {
        e.unwrap().path().extension().is_some_and(|x| x == "wav" || x == "json")
    }));
}

#[test]
fn the_request_body_names_the_voice_and_asks_for_pcm() {
    let mut r = Request::new("Superbounce").unwrap();
    let b = fish::body(&r);
    assert_eq!(b["reference_id"], DEFAULT_VOICE);
    assert_eq!(b["format"], "pcm");
    assert_eq!(b["sample_rate"], 44_100);
    assert_eq!(b["text"], "Superbounce");
    assert!(b.get("prosody").is_none());
    r.speed = 1.2;
    assert!(near(fish::body(&r)["prosody"]["speed"].as_f64().unwrap(), 1.2, 1e-6));
}

#[test]
fn a_missing_key_says_how_to_add_it_and_holds_no_secret() {
    let secret = "sk-test-0123456789abcdef";
    let mut f = Fish::new(Box::new(FixedKey(Err(KeyError::Missing))));
    let req = Request::new("Superbounce").unwrap();
    // It fails before any request: no network is reached.
    let e = f.say(&req).unwrap_err();
    assert_eq!(e, VoiceError::NoKey);
    let text = e.to_string();
    assert!(text.contains("add your Fish Audio key"), "{text}");
    assert!(text.contains("security add-generic-password -s fish-audio"), "{text}");
    assert!(text.contains("<your-fish-audio-key>"), "{text}");
    assert!(!text.contains(secret));
    assert_eq!(f.ready().unwrap_err(), VoiceError::NoKey);

    // A key that is there is never printed by the types that hold it.
    let s = Secret::new(secret);
    assert!(!format!("{s} {s:?}").contains(secret));
    assert_eq!(s.scrub(&format!("bad key {secret}!")), "bad key [key]!");
    assert!(Fish::new(Box::new(FixedKey(Ok(secret.into())))).ready().is_ok());

    // A Keychain that cannot be read says why, without the key.
    let mut f = Fish::new(Box::new(FixedKey(Err(KeyError::Unreadable("denied".into())))));
    assert_eq!(f.say(&req).unwrap_err(), VoiceError::Key("denied".into()));
    assert!(key::ADD_COMMAND.contains("<your-fish-audio-key>"));
}

#[test]
fn a_reference_is_in_the_key_and_checked() {
    let a = Request::new("Hold on.").unwrap();
    let mut b = a.clone();
    b.reference = Some(r"sound\dialog\x\l01_0010_cor".into());
    assert_ne!(a.key(), b.key());
    assert!(b.clone().checked().is_ok());
    b.reference = Some("  ".into());
    assert!(matches!(b.checked(), Err(VoiceError::Invalid(m)) if m.starts_with("reference")));
}

#[test]
fn the_sidecar_protocol_reads_progress_and_the_answer() {
    use super::sidecar::{Progress, read_answer};
    let wire = concat!(
        "some library printed this\n",
        "{\"id\": 1, \"ok\": true, \"result\": {\"stale\": true}}\n",
        "{\"id\": 2, \"progress\": {\"stage\": \"train\", \"done\": 1, \"total\": 4, \"text\": \"loss 0.5\"}}\n",
        "{\"id\": 2, \"ok\": true, \"result\": {\"checkpoint\": \"/x\"}}\n",
        "{\"id\": 3, \"ok\": false, \"error\": \"out of memory\"}\n",
    );
    let mut r = std::io::Cursor::new(wire.as_bytes());
    let mut seen = Vec::new();
    let v = read_answer(2, &mut r, &mut |p: &Progress| seen.push(p.clone())).unwrap();
    assert_eq!(v.unwrap()["checkpoint"], "/x");
    assert_eq!(seen.len(), 1);
    assert_eq!((seen[0].stage.as_str(), seen[0].done, seen[0].total), ("train", 1, 4));
    assert_eq!(seen[0].text.as_deref(), Some("loss 0.5"));
    let e = read_answer(3, &mut r, &mut |_| {}).unwrap_err();
    assert_eq!(e, VoiceError::Model("out of memory".into()));
    // The process ended: no answer.
    assert_eq!(read_answer(4, &mut r, &mut |_| {}).unwrap(), None);
}

#[test]
fn the_sidecar_is_installed_only_by_a_matching_stamp() {
    use super::sidecar::{Backend, Launcher, Managed, SCRIPT, stamp};
    let t = Temp::new("sidecar");
    let m = Managed::new(t.0.clone());
    assert!(!m.installed(Backend::F5Mlx));
    assert_eq!(m.dir(Backend::F5Mlx), t.0.join("f5-mlx"));
    assert!(matches!(m.launch(Backend::F5Mlx), Err(VoiceError::Setup(_))));
    // A venv and a stamp of another script are not an install.
    let home = t.0.join("f5-mlx");
    std::fs::create_dir_all(home.join("venv/bin")).unwrap();
    std::fs::write(home.join("venv/bin/python"), b"").unwrap();
    std::fs::write(home.join("installed.json"), r#"{"stamp": "old"}"#).unwrap();
    assert!(!m.installed(Backend::F5Mlx));
    let ok = serde_json::json!({"stamp": stamp(Backend::F5Mlx)});
    std::fs::write(home.join("installed.json"), ok.to_string()).unwrap();
    assert!(m.installed(Backend::F5Mlx));
    assert!(!m.installed(Backend::Qwen3Tts));
    assert_ne!(stamp(Backend::F5Mlx), stamp(Backend::Qwen3Tts));

    // The script answers every op Studio sends, for both backends.
    for op in ["\"hello\"", "\"prepare\"", "\"say\"", "\"transcribe\"", "\"train\""] {
        assert!(SCRIPT.contains(op), "{op}");
    }
    for b in Backend::ALL {
        assert!(SCRIPT.contains(&format!("\"{}\"", b.id())), "{}", b.id());
        assert_eq!(Backend::parse(b.id()).unwrap(), b);
        assert!(b.requirements().contains("mlx-whisper"));
    }
    assert!(Backend::parse("whisper").is_err());
    assert!(SCRIPT.contains("HF_HUB_OFFLINE"));
}

#[test]
fn a_library_failure_is_said_as_a_sentence_not_a_traceback() {
    use super::readable;
    // The MLX call that took a tuple it could not read.
    let raw = "TypeError: normal(): incompatible function arguments. The following argument \
               types are supported: 1. normal(shape: Sequence[int] = [], dtype: Dtype | None = \
               float32, loc: scalar | array | None = None) Invoked with types: tuple";
    let said = readable(raw);
    assert!(said.contains("reinstall the model"), "{said}");
    assert!(said.contains("normal"), "{said}");
    assert!(!said.contains("Sequence[int]"), "{said}");
    assert_eq!(VoiceError::Model(raw.into()).readable(), said);
    // A traceback keeps its last line.
    let tb = "Traceback (most recent call last):\n  File \"x.py\", line 3, in say\n    boom()\nValueError: bad shape\n";
    assert_eq!(readable(tb), "The local voice model failed: ValueError: bad shape");
    // Memory, and a long plain text.
    assert!(readable("RuntimeError: Out of memory").contains("shorter line"));
    let long = "word ".repeat(200);
    assert!(readable(&long).chars().count() <= 243);
    // Other errors are said as they are.
    assert_eq!(VoiceError::Invalid("a .wav file".into()).readable(), "a .wav file");
}
