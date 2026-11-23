//! The dataset builder over invented tags and audio: no disc, nothing
//! written outside the temp directory.
use std::path::PathBuf;

use super::*;
use crate::voice::fake::{FakeLines, voice_like};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-dataset-{tag}-{}", std::process::id()));
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

fn rules(id: &str) -> Rules {
    Rules::of(character(id).unwrap())
}

#[test]
fn a_character_is_named_from_the_tag_path() {
    let cortana = rules("cortana");
    assert!(cortana.matches(r"sound\dialog\levels\test_level\mission\l01_0010_cor"));
    assert!(cortana.matches(r"sound\dialog\levels\test_level\mission\l01_0010_cor2"));
    assert!(cortana.matches(r"Sound\Dialog\Combat\Cortana\taunt"));
    assert!(cortana.matches("sound/dialog/combat/cortana/taunt"));
    // A folder is matched whole, the leaf is not a folder, and only dialogue.
    assert!(!cortana.matches(r"sound\dialog\combat\cortana_fan\taunt"));
    assert!(!cortana.matches(r"sound\dialog\combat\elite\cortana"));
    assert!(!cortana.matches(r"sound\weapons\cortana\l01_0010_cor"));
    assert!(!cortana.matches(r"sound\dialog\levels\x\mission\l01_0010_core"));

    let announcer = rules("announcer");
    assert!(announcer.matches(r"sound\dialog\multiplayer\flavor\double_kill"));
    assert!(!announcer.matches(r"sound\dialog\levels\x\mission\l01_0010_cor"));
    assert!(rules("arbiter").matches(r"sound\dialog\combat\dervish\hurt"));
    assert!(rules("johnson").matches(r"sound\dialog\levels\x\cinematic\c05_0100_jon"));

    // Every id is a valid one, and none twice.
    for (i, c) in CHARACTERS.iter().enumerate() {
        assert!(check_id(c.id).is_ok(), "{}", c.id);
        assert!(CHARACTERS[i + 1..].iter().all(|d| d.id != c.id));
    }
    assert!(check_id("../x").is_err());
    assert!(check_id("Cortana").is_err());
}

#[test]
fn speakers_are_grouped_by_code_or_folder() {
    assert_eq!(
        speaker_code(r"sound\dialog\x\l01_0010_jon"),
        Some("jon".into())
    );
    assert_eq!(
        speaker_code(r"sound\dialog\x\l01_0010_jon12"),
        Some("jon".into())
    );
    assert_eq!(speaker_code(r"sound\dialog\x\l01_0010"), None);
    assert_eq!(speaker_code(r"sound\dialog\x\taunt"), None);
    assert_eq!(speaker_code(r"sound\dialog\x\big_kills"), None);
    assert_eq!(
        speaker_of(r"sound\dialog\levels\a\mission\l01_0010_cor"),
        Some(("code", "cor".into()))
    );
    assert_eq!(
        speaker_of(r"sound\dialog\combat\elite\involuntary\pain"),
        Some(("folder", r"combat\elite".into()))
    );
    assert_eq!(speaker_of(r"sound\weapons\x"), None);
}

#[test]
fn a_subtitle_is_cleaned_into_training_text() {
    assert_eq!(
        clean_text("Cortana: Wake up, [static] Chief."),
        Some("Wake up, Chief.".into())
    );
    assert_eq!(
        clean_text("  Two\nlines\t here "),
        Some("Two lines here".into())
    );
    assert_eq!(clean_text("[radio static]"), None);
    // A colon later in the line, or after a long prefix, is kept.
    assert_eq!(
        clean_text("Note this: now."),
        Some("Note this: now.".into())
    );
    assert_eq!(
        clean_text("This is a very long sentence part: then more"),
        Some("This is a very long sentence part: then more".into())
    );
}

#[test]
fn resampling_keeps_the_pitch_and_the_length() {
    // A 440 Hz tone at 44.1 kHz down to 24 kHz: 1 s stays 1 s, and the
    // zero crossings still say 440 Hz.
    let tone: Vec<i16> = (0..44_100u32)
        .map(|i| {
            ((2.0 * std::f64::consts::PI * 440.0 * f64::from(i) / 44_100.0).sin() * 10_000.0) as i16
        })
        .collect();
    let out = resample(&tone, 44_100, RATE);
    assert_eq!(out.len(), 24_000);
    let crossings = out
        .windows(2)
        .skip(1000)
        .filter(|w| (w[0] < 0) != (w[1] < 0))
        .count();
    let hz = crossings as f64 / 2.0 / (23_000.0 / 24_000.0);
    assert!((hz - 440.0).abs() < 5.0, "{hz}");
    // A tone over the new Nyquist is taken out, not folded down.
    let high: Vec<i16> = (0..44_100u32)
        .map(|i| {
            ((2.0 * std::f64::consts::PI * 18_000.0 * f64::from(i) / 44_100.0).sin() * 10_000.0)
                as i16
        })
        .collect();
    let out = resample(&high, 44_100, RATE);
    let peak = out[1000..].iter().map(|s| s.unsigned_abs()).max().unwrap();
    assert!(peak < 2_500, "{peak}");
    assert_eq!(resample(&tone, 44_100, 44_100), tone);
    assert_eq!(downmix(&[100, 300, -50, 50], 2), vec![200, 0]);
}

#[test]
fn a_radio_filtered_line_is_told_from_a_clean_one() {
    let f = Filter::default();
    for (f0, rate) in [
        (110.0, 22_050),
        (210.0, 22_050),
        (210.0, 44_100),
        (110.0, 32_000),
    ] {
        let clean = analyse(&voice_like(rate, f0, 2.0, 0.0, 7_000.0), rate);
        assert!(!f.radio(&clean), "{f0} Hz at {rate}: {clean:?}");
        let radio = analyse(&voice_like(rate, f0, 2.0, 400.0, 3_000.0), rate);
        assert!(f.radio(&radio), "{f0} Hz at {rate}: {radio:?}");
    }
    let mut loud = voice_like(22_050, 120.0, 1.0, 0.0, 7_000.0);
    for s in loud.iter_mut().step_by(50) {
        *s = i16::MAX;
    }
    assert!(analyse(&loud, 22_050).clipped_share > f.max_clipped_share);
}

#[test]
fn the_dataset_is_built_with_its_manifest() {
    let t = Temp::new("build");
    let dir = t.0.join("cortana");
    let mut src = FakeLines::sample();
    let r = build(
        &mut src,
        "cortana",
        &rules("cortana"),
        &Options::default(),
        &dir,
    )
    .unwrap();
    assert_eq!(r.tags, 7);
    assert_eq!(r.lines.len(), 5);
    assert_eq!(r.decoded, 8);
    assert_eq!(r.maps_read, 2);
    assert_eq!(r.maps_failed.len(), 1);
    assert_eq!(r.dropped.get("too_short"), Some(&1));
    assert_eq!(r.dropped.get("radio"), Some(&1));
    assert_eq!(r.dropped.get("silent"), Some(&1));

    let lines = read_manifest(&dir.join(MANIFEST)).unwrap();
    let ids = |v: &[Line]| {
        v.iter()
            .map(|l| (l.id.clone(), l.text.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&lines), ids(&r.lines));
    let first = lines
        .iter()
        .find(|l| l.source_tag.ends_with("l01_0010_cor"))
        .unwrap();
    assert_eq!(first.text.as_deref(), Some("Wake up, Chief."));
    assert_eq!(first.text_from, TextFrom::Subtitle);
    assert_eq!(first.map, "test_a");
    assert!(
        (4_000..4_300).contains(&first.duration_ms),
        "{}",
        first.duration_ms
    );
    // The taunt's two permutations wait for the Whisper: a subtitle is
    // only a one-permutation sound's.
    let taunts: Vec<&Line> = lines
        .iter()
        .filter(|l| l.source_tag.ends_with("taunt"))
        .collect();
    assert_eq!(taunts.len(), 2);
    assert!(
        taunts
            .iter()
            .all(|l| l.text_from == TextFrom::Pending && l.text.is_none())
    );
    assert_ne!(taunts[0].id, taunts[1].id);

    // Every clip is a 24 kHz mono WAV under the character's directory.
    for l in &lines {
        assert!(l.path.starts_with(dir.join(CLIPS)), "{:?}", l.path);
        let c = boot::medal_announcer::parse_wav(&std::fs::read(&l.path).unwrap()).unwrap();
        assert_eq!((c.channels, c.rate), (1, RATE));
        assert!(l.lufs > -80.0 && l.lufs < 0.0);
    }
    let s: Value = serde_json::from_slice(&std::fs::read(dir.join(SUMMARY)).unwrap()).unwrap();
    assert_eq!(s["character"], "cortana");
    assert_eq!(s["dataset"]["lines"], 5);
    let sum = summarize(&lines);
    assert_eq!((sum.usable, sum.from_subtitle, sum.pending_asr), (2, 2, 3));

    // Keeping radio lines keeps it; the limit stops early.
    let opts = Options {
        filter: Filter {
            keep_radio: true,
            ..Filter::default()
        },
        limit: Some(3),
        ..Options::default()
    };
    let r = build(&mut src, "cortana", &rules("cortana"), &opts, &dir).unwrap();
    assert_eq!(r.lines.len(), 3);
    let r = build(
        &mut src,
        "cortana",
        &rules("cortana"),
        &Options {
            filter: Filter {
                keep_radio: true,
                ..Filter::default()
            },
            ..Options::default()
        },
        &dir,
    )
    .unwrap();
    assert_eq!(r.lines.len(), 6);
    // A rebuild leaves no clip of the last one behind.
    let files = std::fs::read_dir(dir.join(CLIPS)).unwrap().count();
    assert_eq!(files, 6);
}

#[test]
fn a_reference_is_picked_or_found() {
    let t = Temp::new("reference");
    let dir = t.0.join("cortana");
    let mut src = FakeLines::sample();
    let r = build(
        &mut src,
        "cortana",
        &rules("cortana"),
        &Options::default(),
        &dir,
    )
    .unwrap();
    let auto = pick_reference(&r.lines, None).unwrap();
    assert_eq!(auto.text.as_deref(), Some("I'll be right here."));
    let by_tag = pick_reference(
        &r.lines,
        Some(r"SOUND\dialog\levels\test_level\mission\l01_0010_cor"),
    )
    .unwrap();
    assert_eq!(by_tag.text.as_deref(), Some("Wake up, Chief."));
    let by_id = pick_reference(&r.lines, Some(&auto.id)).unwrap();
    assert_eq!(by_id, auto);
    let taunt = r
        .lines
        .iter()
        .find(|l| l.source_tag.ends_with("taunt") && l.permutation == 1)
        .unwrap();
    let tag_perm = format!("{}#1", taunt.source_tag);
    assert!(
        matches!(pick_reference(&r.lines, Some(&tag_perm)), Err(VoiceError::Invalid(m)) if m.contains("pending"))
    );
    assert!(matches!(
        pick_reference(&r.lines, Some("nope")),
        Err(VoiceError::Invalid(_))
    ));
    let none: Vec<Line> = r.lines.iter().filter(|l| !l.usable()).cloned().collect();
    assert!(matches!(
        pick_reference(&none, None),
        Err(VoiceError::Setup(_))
    ));
}

#[test]
fn a_manifest_line_reads_back() {
    let l = Line {
        id: "l01_0010_cor-0123abcd".into(),
        path: PathBuf::from("/tmp/x.wav"),
        text: Some("Wake up.".into()),
        text_from: TextFrom::Asr,
        duration_ms: 1234,
        source_tag: r"sound\dialog\x\l01_0010_cor".into(),
        map: "01a".into(),
        permutation: 2,
        class: 9,
        lufs: -23.4,
        peak_dbfs: -3.2,
        low_share: 0.25,
        high_share: 0.012,
    };
    assert_eq!(Line::from_json(&l.json()), Some(l.clone()));
    let t = Temp::new("manifest");
    let p = t.0.join(MANIFEST);
    write_manifest(&p, &[l.clone(), l.clone()]).unwrap();
    assert_eq!(read_manifest(&p).unwrap().len(), 2);
    std::fs::write(&p, "{\"id\": 1}\n").unwrap();
    assert!(read_manifest(&p).is_err());
}
