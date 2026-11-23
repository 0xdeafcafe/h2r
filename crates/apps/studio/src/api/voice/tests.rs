//! `voice.*` over the fake voice: no network, no Keychain, no real audio
//! device (a headless output), nothing written outside the temp directory.
use std::path::PathBuf;

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::super::sound_measure::meter;
use crate::api::{Api, Reply};
use crate::voice::audio;
use crate::voice::fake::Fake;
use crate::{App, voice};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-voice-api-{tag}-{}", std::process::id()));
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

/// An app with its data root in the temp directory and the fake voice;
/// the fake's call counter is the second answer.
fn open(t: &Temp) -> (App, Api, Fake) {
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.audition.headless = true;
    let fake = Fake::default();
    app.announcer.source = Box::new(fake.clone());
    (app, Api::new(), fake)
}

fn call(api: &mut Api, app: &mut App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

fn fails(r: Reply) -> agent_proto::RpcError {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e,
    }
}

fn near(a: f64, b: f64, within: f64) -> bool {
    (a - b).abs() <= within
}

#[test]
fn a_line_is_made_normalised_and_saved_in_the_announcer_format() {
    let t = Temp::new("say");
    let (mut app, mut api, fake) = open(&t);
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_SAY,
        json!({"text": "Superbounce", "play": false, "target_lufs": -16.0}),
    );
    assert_eq!(r["cached"], false);
    assert_eq!(r["voice"], voice::DEFAULT_VOICE);
    assert_eq!(r["target"]["from"], "given");
    assert_eq!(fake.calls(), 1);

    // The file is under the data root, never in a checkout.
    let file = PathBuf::from(r["file"].as_str().unwrap());
    assert!(file.starts_with(t.0.join("data/announcer/lines")), "{file:?}");
    let bytes = std::fs::read(&file).unwrap();
    let clip = boot::medal_announcer::parse_wav(&bytes).unwrap();
    assert_eq!((clip.channels, clip.rate), (1, 44_100));

    // Trimmed: the fake's 2 s (half a second of silence at each end of a
    // second of tone) is a little over a second.
    let frames = clip.data.len() / 2;
    assert!(frames < 44_100 + 44_100 / 4, "{frames}");
    assert!(r["trimmed_ms"]["head"].as_u64().unwrap() > 400);
    assert!(r["trimmed_ms"]["tail"].as_u64().unwrap() > 400);

    // Normalised: the file's own loudness is the target's.
    let pcm: Vec<i16> = clip
        .data
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect();
    let levels = meter::measure(&pcm, 1, 44_100);
    assert!(near(levels.lufs, -16.0, 0.15), "{levels:?}");
    assert!(near(r["levels"]["lufs"].as_f64().unwrap(), levels.lufs, 0.1));
    assert!(levels.peak_dbfs <= audio::CEILING_DBFS + 0.01);
    assert_eq!(r["limited"], false);
    assert!(r["gain_db"].as_f64().unwrap() > 0.0);
    assert!(r["raw_levels"]["lufs"].as_f64().unwrap() < levels.lufs);

    // The raw line is in the cache.
    assert!(PathBuf::from(r["raw"].as_str().unwrap()).is_file());
    assert_eq!(app.announcer.last.as_deref(), Some(file.as_path()));
}

#[test]
fn the_same_line_reuses_the_earlier_file_and_asks_once() {
    let t = Temp::new("cache");
    let (mut app, mut api, fake) = open(&t);
    let say = |api: &mut Api, app: &mut App, p: Value| call(api, app, m::VOICE_SAY, p);
    let one = json!({"text": "Double bounce", "play": false});
    let a = say(&mut api, &mut app, one.clone());
    assert_eq!((a["cached"].as_bool(), fake.calls()), (Some(false), 1));
    let b = say(&mut api, &mut app, one.clone());
    assert_eq!((b["cached"].as_bool(), fake.calls()), (Some(true), 1));
    assert_eq!(a["key"], b["key"]);
    assert_eq!(
        std::fs::read(a["file"].as_str().unwrap()).unwrap(),
        std::fs::read(b["file"].as_str().unwrap()).unwrap()
    );

    // Any setting that changes the audio is a new line.
    for (i, change) in [
        json!({"text": "Triple bounce"}),
        json!({"voice": "abc123"}),
        json!({"speed": 1.2}),
        json!({"model": "s2.1-pro"}),
    ]
    .into_iter()
    .enumerate()
    {
        let mut p = one.clone();
        p.as_object_mut().unwrap().extend(change.as_object().unwrap().clone());
        let r = say(&mut api, &mut app, p);
        assert_eq!(r["cached"], false);
        assert_eq!(fake.calls(), 2 + i);
    }

    // `fresh` asks again; the cache is read by a new session too.
    let calls = fake.calls();
    let mut p = one.clone();
    p["fresh"] = json!(true);
    assert_eq!(say(&mut api, &mut app, p)["cached"], false);
    assert_eq!(fake.calls(), calls + 1);
    let (mut app2, mut api2, fake2) = open(&t);
    assert_eq!(say(&mut api2, &mut app2, one)["cached"], true);
    assert_eq!(fake2.calls(), 0);
}

#[test]
fn a_missing_key_is_refused_with_the_setup_step_and_no_secret() {
    let t = Temp::new("nokey");
    let (mut app, mut api, _) = open(&t);
    let nokey = Fake {
        no_key: true,
        ..Fake::default()
    };
    app.announcer.source = Box::new(nokey.clone());
    let e = fails(api.call(&mut app, m::VOICE_SAY, json!({"text": "Superbounce", "play": false})));
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("add your Fish Audio key"), "{}", e.message);
    assert!(
        e.message.contains("security add-generic-password -s fish-audio"),
        "{}",
        e.message
    );
    assert!(e.message.contains("<your-fish-audio-key>"));
    let data = e.data.unwrap();
    assert_eq!(data["step"], "add_key");
    assert_eq!(data["service"], "fish-audio");
    assert!(data["command"].as_str().unwrap().contains("<your-fish-audio-key>"));
    // Nothing was written.
    assert!(!t.0.join("data/announcer/lines").exists());

    // The list says so without asking the service.
    let l = call(&mut api, &mut app, m::VOICE_LIST, json!({}));
    assert_eq!(l["key"]["state"], "missing");
    assert_eq!(nokey.calls(), 1);

    // A line that was cached before needs no key.
    app.announcer.source = Box::new(Fake::default());
    call(&mut api, &mut app, m::VOICE_SAY, json!({"text": "Superbounce", "play": false}));
    app.announcer.source = Box::new(nokey);
    let r = call(&mut api, &mut app, m::VOICE_SAY, json!({"text": "Superbounce", "play": false}));
    assert_eq!(r["cached"], true);
}

#[test]
fn a_line_plays_through_the_sound_tools_output_unless_asked_not_to() {
    let t = Temp::new("play");
    let (mut app, mut api, _) = open(&t);
    let r = call(&mut api, &mut app, m::VOICE_SAY, json!({"text": "Bouncetacular"}));
    assert_eq!(r["playback"]["playing"], true);
    assert_eq!(r["playback"]["output"], "headless");
    assert!(r["playback"]["duration_ms"].as_u64().unwrap() > 900);
    assert!(app.sound_state().playing);
    app.sound_stop();
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_SAY,
        json!({"text": "Bouncetacular", "play": false}),
    );
    assert_eq!(r["playback"]["playing"], false);
}

#[test]
fn a_clip_id_names_the_file_and_says_how_to_add_it() {
    let t = Temp::new("clip");
    let (mut app, mut api, _) = open(&t);
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_SAY,
        json!({"text": "Quad bounce", "clip": "quad-bounce-2", "play": false}),
    );
    assert!(r["file"].as_str().unwrap().ends_with("announcer/clips/quad-bounce-2.wav"));
    let ready = &r["ready"];
    assert_eq!(ready["repo_path"], "art/announcer/quad-bounce-2.wav");
    assert_eq!(ready["exists"], false);
    assert!(ready["clips_entry"].as_str().unwrap().contains("include_bytes!"));
    // One that is in CLIPS is marked as existing.
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_SAY,
        json!({"text": "Superbounce", "clip": "superbounce", "play": false}),
    );
    assert_eq!(r["ready"]["exists"], true);
}

#[test]
fn bad_params_are_refused_before_the_voice_is_asked() {
    let t = Temp::new("bad");
    let (mut app, mut api, fake) = open(&t);
    for p in [
        json!({"text": "  "}),
        json!({"text": "x".repeat(501)}),
        json!({"text": "hi", "speed": 5}),
        json!({"text": "hi", "voice": "../x"}),
        json!({"text": "hi", "clip": "Bad Id"}),
        json!({"text": "hi", "out": "line.mp3"}),
        json!({"text": "hi", "target_lufs": 12}),
        json!({"text": "hi", "play": "yes"}),
    ] {
        let e = fails(api.call(&mut app, m::VOICE_SAY, p.clone()));
        assert_eq!(e.code, code::INVALID_PARAMS, "{p}: {e}");
    }
    assert_eq!(fake.calls(), 0);
}

#[test]
fn a_silent_line_is_an_error_and_a_relative_out_is_under_the_data_root() {
    let t = Temp::new("silent");
    let (mut app, mut api, _) = open(&t);
    app.announcer.source = Box::new(Fake {
        silent: true,
        ..Fake::default()
    });
    let e = fails(api.call(&mut app, m::VOICE_SAY, json!({"text": "hi", "play": false})));
    assert_eq!(e.code, code::FAILED);
    assert!(e.message.contains("silent"));
    // Another text: the silent line above is what the cache holds for "hi".
    app.announcer.source = Box::new(Fake::default());
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_SAY,
        json!({"text": "hi again", "play": false, "out": "mine/hi.wav"}),
    );
    assert_eq!(
        PathBuf::from(r["file"].as_str().unwrap()),
        t.0.join("data/announcer/mine/hi.wav")
    );
}

#[test]
fn the_list_names_the_default_voice_and_the_lines_said() {
    let t = Temp::new("list");
    let (mut app, mut api, _) = open(&t);
    let l = call(&mut api, &mut app, m::VOICE_LIST, json!({}));
    assert_eq!(l["default"], "86126c4c4dad4911979556b4569802cd");
    assert_eq!(l["key"]["state"], "present");
    assert_eq!(l["voices"][0]["default"], true);
    assert_eq!(l["cache"]["lines"], 0);
    call(&mut api, &mut app, m::VOICE_SAY, json!({"text": "One", "play": false}));
    call(&mut api, &mut app, m::VOICE_SAY, json!({"text": "Two", "voice": "abc", "play": false}));
    let l = call(&mut api, &mut app, m::VOICE_LIST, json!({}));
    assert_eq!(l["cache"]["lines"], 2);
    assert_eq!(l["voices"].as_array().unwrap().len(), 2);
    assert_eq!(l["recent"].as_array().unwrap().len(), 2);
}

#[test]
fn commit_copies_a_line_into_the_checkout_only_when_asked() {
    let t = Temp::new("commit");
    let (mut app, mut api, _) = open(&t);
    let repo = t.0.join("repo");
    std::fs::create_dir_all(repo.join("art/announcer")).unwrap();
    let repo_s = repo.display().to_string();

    // Making a line never writes to the checkout.
    let r = call(&mut api, &mut app, m::VOICE_SAY, json!({"text": "Bounce", "play": false}));
    assert_eq!(std::fs::read_dir(repo.join("art/announcer")).unwrap().count(), 0);

    let c = call(
        &mut api,
        &mut app,
        m::VOICE_COMMIT,
        json!({"clip": "test-line", "repo": repo_s}),
    );
    let to = repo.join("art/announcer/test-line.wav");
    assert_eq!(PathBuf::from(c["file"].as_str().unwrap()), to);
    assert_eq!(c["exists"], false);
    assert!(c["next"].as_str().unwrap().contains("git add art/announcer/test-line.wav"));
    assert_eq!(
        std::fs::read(&to).unwrap(),
        std::fs::read(r["file"].as_str().unwrap()).unwrap()
    );

    // An existing clip is not overwritten without `replace`.
    let e = fails(api.call(&mut app, m::VOICE_COMMIT, json!({"clip": "test-line", "repo": repo_s})));
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(e.message.contains("replace"));
    let c = call(
        &mut api,
        &mut app,
        m::VOICE_COMMIT,
        json!({"clip": "test-line", "repo": repo_s, "replace": true}),
    );
    assert_eq!(c["exists"], true);

    // Not a checkout, a bad id, a file that is not a clip.
    let nope = t.0.join("elsewhere");
    std::fs::create_dir_all(&nope).unwrap();
    for p in [
        json!({"clip": "x", "repo": nope.display().to_string()}),
        json!({"clip": "Bad", "repo": repo_s}),
        json!({"clip": "x", "repo": repo_s, "from": t.0.join("nothing.wav").display().to_string()}),
    ] {
        assert!(api.call(&mut app, m::VOICE_COMMIT, p.clone()).is_err(), "{p}");
    }
    // Nothing made yet: nothing to copy.
    let (mut app2, mut api2, _) = open(&Temp::new("commit-none"));
    let e = fails(api2.call(&mut app2, m::VOICE_COMMIT, json!({"clip": "x", "repo": repo_s})));
    assert_eq!(e.code, code::INVALID_PARAMS);
}
