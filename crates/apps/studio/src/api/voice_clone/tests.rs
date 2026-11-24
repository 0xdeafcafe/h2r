//! `voice.characters`, `voice.dataset.*` and `voice.clone.*` over invented
//! maps and the fake sidecar: no disc, no Python, no network, no audio
//! device, nothing written outside the temp directory.
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::test_setup;
use crate::App;
use crate::api::{Api, Reply};
use crate::voice::dataset::{self, TextFrom};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-voices-{tag}-{}", std::process::id()));
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

fn test_app(t: &Temp) -> App {
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app
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

/// Waits for job `id` to end; its state.
fn wait(api: &mut Api, app: &mut App, id: u64) -> Value {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let s = call(api, app, m::VOICE_DATASET_STATUS, json!({"job": id}));
        if s["job"]["state"] != "running" {
            return s["job"].clone();
        }
        assert!(Instant::now() < until, "job {id} still running: {s}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_characters_are_listed_with_their_tags_and_the_speakers_found() {
    let t = Temp::new("characters");
    let mut app = test_app(&t);
    let mut api = Api::new();
    test_setup(&mut app, false, false);
    let r = call(&mut api, &mut app, m::VOICE_CHARACTERS, json!({}));
    let of = |id: &str| {
        r["characters"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .cloned()
            .unwrap()
    };
    // Cortana's seven tags (the one in both maps counted once), Johnson's one.
    assert_eq!(of("cortana")["tags"], 7, "{r}");
    assert_eq!(of("johnson")["tags"], 1);
    assert_eq!(of("masterchief")["tags"], 0);
    assert_eq!(of("cortana")["dataset"], Value::Null);
    assert_eq!(r["maps_read"], 2);
    assert_eq!(r["maps_failed"][0]["map"], "broken");
    // The weapon sound is not dialogue.
    assert_eq!(r["dialogue_tags"], 8);
    let speakers = r["speakers"].as_array().unwrap();
    let cor = speakers.iter().find(|s| s["key"] == "cor").unwrap();
    assert_eq!(
        (cor["kind"].as_str(), cor["tags"].as_u64()),
        (Some("code"), Some(6))
    );
    assert!(
        speakers
            .iter()
            .any(|s| s["kind"] == "folder" && s["key"] == r"combat\cortana")
    );

    // One map only.
    let one = call(
        &mut api,
        &mut app,
        m::VOICE_CHARACTERS,
        json!({"map": "test_b"}),
    );
    assert_eq!(one["maps_read"], 1);
    assert_eq!(one["dialogue_tags"], 1);
}

#[test]
fn a_dataset_is_built_transcribed_and_reported() {
    let t = Temp::new("build");
    let mut app = test_app(&t);
    let mut api = Api::new();
    let fake = test_setup(&mut app, false, false);
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_DATASET_BUILD,
        json!({"character": "cortana"}),
    );
    assert_eq!(r["kept"], 5, "{r}");
    assert_eq!(r["tags"], 7);
    assert_eq!(r["dropped"]["too_short"], 1);
    assert_eq!(r["dropped"]["radio"], 1);
    assert_eq!(r["dropped"]["silent"], 1);
    assert_eq!(r["dataset"]["from_subtitle"], 2);
    assert_eq!(r["dataset"]["pending_asr"], 3);
    let dir = t.0.join("data/voices/cortana");
    assert_eq!(PathBuf::from(r["dir"].as_str().unwrap()), dir);

    // The transcription runs as a job: it installs the model first.
    let job = r["asr_job"].as_u64().expect("an asr job");
    let done = wait(&mut api, &mut app, job);
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["kind"], "transcribe");
    assert_eq!(done["result"]["transcribed"], 2);
    assert_eq!(done["result"]["heard_nothing"], 1);
    assert_eq!(fake.installs.load(std::sync::atomic::Ordering::SeqCst), 1);
    let asked = fake.calls_of("transcribe");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["files"].as_array().unwrap().len(), 3);

    let lines = dataset::read_manifest(&dir.join(dataset::MANIFEST)).unwrap();
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.text_from == TextFrom::Asr)
            .count(),
        2
    );
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.text_from == TextFrom::None)
            .count(),
        1
    );

    let s = call(
        &mut api,
        &mut app,
        m::VOICE_DATASET_STATUS,
        json!({"character": "cortana"}),
    );
    assert_eq!(s["sidecar"]["state"], "installed");
    let d = &s["datasets"][0];
    assert_eq!(d["dataset"]["from_asr"], 2);
    assert_eq!(d["dataset"]["usable"], 4);
    assert_eq!(d["build"]["dropped"]["radio"], 1);
    assert_eq!(d["model"]["trained"], false);

    // A rebuild keeps the transcripts and does not ask again.
    let again = call(
        &mut api,
        &mut app,
        m::VOICE_DATASET_BUILD,
        json!({"character": "cortana", "asr": false}),
    );
    assert_eq!(again["dataset"]["from_asr"], 2, "{again}");
    assert_eq!(again["asr_job"], Value::Null);
}

#[test]
fn a_build_is_checked_before_anything_is_read() {
    let t = Temp::new("build-bad");
    let mut app = test_app(&t);
    let mut api = Api::new();
    test_setup(&mut app, false, false);
    let e = fails(api.call(
        &mut app,
        m::VOICE_DATASET_BUILD,
        json!({"character": "nobody"}),
    ));
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(e.message.contains("paths"), "{}", e.message);
    let e = fails(api.call(
        &mut app,
        m::VOICE_DATASET_BUILD,
        json!({"character": "../x"}),
    ));
    assert_eq!(e.code, code::INVALID_PARAMS);
    let e = fails(api.call(
        &mut app,
        m::VOICE_DATASET_BUILD,
        json!({"character": "cortana", "min_ms": 5000, "max_ms": 100}),
    ));
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(!t.0.join("data/voices").exists());

    // Any speaker by its tag paths.
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_DATASET_BUILD,
        json!({"character": "sergeant", "paths": [r"mission\l01_0060_jon"], "asr": false}),
    );
    assert_eq!(r["kept"], 1, "{r}");
    assert_eq!(r["dataset"]["from_subtitle"], 1);
}

#[test]
fn a_line_is_said_by_the_clone_at_the_characters_loudness() {
    let t = Temp::new("say");
    let mut app = test_app(&t);
    let mut api = Api::new();
    let fake = test_setup(&mut app, true, true);
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "cortana", "text": "Hold on, Chief.", "play": false}),
    );
    assert_eq!(r["engine"], "clone");
    assert_eq!(r["cached"], false);
    assert_eq!(r["backend"], "f5-mlx");
    // The reference: the subtitled line nearest 7 s.
    assert_eq!(r["reference"]["text"], "I'll be right here.", "{r}");
    assert_eq!(r["target"]["from"], "character");
    assert_eq!(r["target"]["lines"], 5);

    // The sidecar was given the 24 kHz reference and its text, offline.
    let asked = fake.calls_of("say");
    assert_eq!(asked.len(), 1);
    let refp = PathBuf::from(asked[0]["reference"]["path"].as_str().unwrap());
    assert!(
        refp.starts_with(t.0.join("data/voices/cortana/dataset")),
        "{refp:?}"
    );
    assert_eq!(asked[0]["checkpoint"], Value::Null);
    assert_eq!(asked[0]["text"], "Hold on, Chief.");

    // Saved under the character, 44.1 kHz mono, at the dataset's median.
    let file = PathBuf::from(r["file"].as_str().unwrap());
    assert!(
        file.starts_with(t.0.join("data/voices/cortana/lines")),
        "{file:?}"
    );
    let clip = boot::medal_announcer::parse_wav(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!((clip.channels, clip.rate), (1, 44_100));
    let lufs = r["levels"]["lufs"].as_f64().unwrap();
    let target = r["target"]["lufs"].as_f64().unwrap();
    assert!((lufs - target).abs() < 0.6 || r["limited"] == true, "{r}");

    // The same line again comes from the cache.
    let again = call(
        &mut api,
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "cortana", "text": "Hold on, Chief.", "play": false}),
    );
    assert_eq!(again["cached"], true);
    assert_eq!(fake.calls_of("say").len(), 1);

    // A named reference, by source tag.
    let named = call(
        &mut api,
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({
            "character": "cortana",
            "text": "Hold on, Chief.",
            "reference": r"sound\dialog\levels\test_level\mission\l01_0010_cor",
            "play": false,
        }),
    );
    assert_eq!(named["cached"], false);
    assert_eq!(named["reference"]["text"], "Wake up, Chief.");
}

#[test]
fn a_clone_line_with_a_clip_id_can_be_committed() {
    let t = Temp::new("say-clip");
    let mut app = test_app(&t);
    let mut api = Api::new();
    test_setup(&mut app, true, true);
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "cortana", "text": "Bounce.", "clip": "cortana-bounce", "play": false}),
    );
    assert_eq!(r["ready"]["repo_path"], "art/announcer/cortana-bounce.wav");
    let repo = t.0.join("repo");
    std::fs::create_dir_all(repo.join("art/announcer")).unwrap();
    let c = call(
        &mut api,
        &mut app,
        m::VOICE_COMMIT,
        json!({"clip": "cortana-bounce", "repo": repo.display().to_string()}),
    );
    assert!(
        repo.join("art/announcer/cortana-bounce.wav").is_file(),
        "{c}"
    );
}

#[test]
fn saying_before_the_model_is_installed_starts_the_install() {
    let t = Temp::new("say-install");
    let mut app = test_app(&t);
    let mut api = Api::new();
    let fake = test_setup(&mut app, false, true);
    let e = fails(api.call(
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "cortana", "text": "Hold on.", "play": false}),
    ));
    assert_eq!(e.code, code::REFUSED);
    assert_eq!(e.data.as_ref().unwrap()["step"], "install", "{e:?}");
    assert!(fake.calls_of("say").is_empty());
    let s = call(&mut api, &mut app, m::VOICE_DATASET_STATUS, json!({}));
    let job = s["job"]["id"].as_u64().unwrap();
    assert_eq!(s["job"]["kind"], "install");
    assert_eq!(wait(&mut api, &mut app, job)["state"], "done");
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "cortana", "text": "Hold on.", "play": false}),
    );
    assert_eq!(r["cached"], false);
}

#[test]
fn a_character_with_no_dataset_or_text_is_refused() {
    let t = Temp::new("say-nodata");
    let mut app = test_app(&t);
    let mut api = Api::new();
    test_setup(&mut app, true, false);
    let e = fails(api.call(
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "arbiter", "text": "Hm.", "play": false}),
    ));
    assert_eq!(e.code, code::REFUSED);
    assert_eq!(e.data.as_ref().unwrap()["step"], "dataset");
    assert!(e.message.contains("voice.dataset.build"), "{}", e.message);

    // A reference that is not in the dataset.
    test_setup(&mut app, true, true);
    let e = fails(api.call(
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "cortana", "text": "Hm.", "reference": "nope", "play": false}),
    ));
    assert_eq!(e.code, code::INVALID_PARAMS);
    // One still waiting for its transcript.
    let lines =
        dataset::read_manifest(&t.0.join("data/voices/cortana").join(dataset::MANIFEST)).unwrap();
    let pending = lines
        .iter()
        .find(|l| l.text_from == TextFrom::Pending)
        .unwrap();
    let e = fails(api.call(
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "cortana", "text": "Hm.", "reference": pending.id, "play": false}),
    ));
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(e.message.contains("pending"), "{}", e.message);
}

#[test]
fn a_fine_tune_runs_as_a_job_and_the_clone_uses_it() {
    let t = Temp::new("train");
    let mut app = test_app(&t);
    let mut api = Api::new();
    let fake = test_setup(&mut app, true, true);

    // Too little speech by default.
    let e = fails(api.call(
        &mut app,
        m::VOICE_CLONE_TRAIN,
        json!({"character": "cortana"}),
    ));
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("minutes"), "{}", e.message);
    // No fine-tune for the zero-shot backend.
    let e = fails(api.call(
        &mut app,
        m::VOICE_CLONE_TRAIN,
        json!({"character": "cortana", "backend": "qwen3-tts", "min_minutes": 0}),
    ));
    assert_eq!(e.code, code::INVALID_PARAMS);

    // The fake's training waits until it is let go.
    fake.hold.store(true, std::sync::atomic::Ordering::SeqCst);
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_CLONE_TRAIN,
        json!({"character": "cortana", "backend": "f5-mlx", "min_minutes": 0, "steps": 3}),
    );
    assert_eq!(r["state"], "running");
    assert_eq!(r["lines"], 2);
    let id = r["job"].as_u64().unwrap();
    // One job at a time: another, and a dataset build, are refused.
    let busy = fails(api.call(
        &mut app,
        m::VOICE_CLONE_TRAIN,
        json!({"character": "cortana", "min_minutes": 0}),
    ));
    assert_eq!(busy.code, code::REFUSED, "{busy:?}");
    let busy = fails(api.call(
        &mut app,
        m::VOICE_DATASET_BUILD,
        json!({"character": "cortana"}),
    ));
    assert_eq!(busy.code, code::REFUSED, "{busy:?}");
    let s = call(
        &mut api,
        &mut app,
        m::VOICE_DATASET_STATUS,
        json!({"job": id}),
    );
    assert_eq!(s["job"]["state"], "running");
    fake.hold.store(false, std::sync::atomic::Ordering::SeqCst);
    let done = wait(&mut api, &mut app, id);
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["result"]["lines"], 2);
    let asked = fake.calls_of("train");
    assert_eq!(asked[0]["steps"], 3);
    let train =
        dataset::read_manifest(&PathBuf::from(asked[0]["manifest"].as_str().unwrap())).unwrap();
    assert!(train.iter().all(|l| l.usable()));
    assert_eq!(train.len(), 2);

    let s = call(
        &mut api,
        &mut app,
        m::VOICE_DATASET_STATUS,
        json!({"character": "cortana"}),
    );
    assert_eq!(s["datasets"][0]["model"]["trained"], true, "{s}");

    // The fine-tune changes the model, so the line is said again with it.
    let said = call(
        &mut api,
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "cortana", "text": "Hold on.", "play": false}),
    );
    assert!(
        said["model"].as_str().unwrap().starts_with("f5-mlx.ft-"),
        "{said}"
    );
    let asked = fake.calls_of("say");
    assert!(
        asked.last().unwrap()["checkpoint"]
            .as_str()
            .unwrap()
            .ends_with("model.safetensors")
    );
}

#[test]
fn a_failing_model_is_reported_and_the_job_list_says_why() {
    let t = Temp::new("say-fail");
    let mut app = test_app(&t);
    let mut api = Api::new();
    test_setup(&mut app, true, true);
    let mut fake = crate::voice::fake::FakeLauncher::ready();
    fake.fail_say = Some("out of memory".into());
    app.voices.set_launcher(std::sync::Arc::new(fake));
    let e = fails(api.call(
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "cortana", "text": "Hm.", "play": false}),
    ));
    assert_eq!(e.code, code::FAILED);
    assert!(e.message.contains("out of memory"));
    let e = fails(api.call(
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "cortana", "text": "Hm.", "backend": "whisper", "play": false}),
    ));
    assert_eq!(e.code, code::INVALID_PARAMS);
}
