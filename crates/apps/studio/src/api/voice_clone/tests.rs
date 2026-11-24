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

#[test]
fn a_small_dataset_trains_once_the_floor_is_lowered_and_the_zero_shot_model_needs_no_fine_tune() {
    let t = Temp::new("floor");
    let mut app = test_app(&t);
    let fake = test_setup(&mut app, true, true);
    let default_floor = crate::app::voices::MIN_TRAIN_MINUTES;

    // The Voices window's path: the default floor refuses, and says how
    // much speech there is and what it needs.
    let e = app.voice_clone_train("cortana", None, Some(default_floor));
    let e = e.unwrap_err().to_string();
    assert!(e.contains("minutes of speech with text"), "{e}");
    assert!(e.contains("needs 10"), "{e}");
    assert!(fake.calls_of("train").is_empty());
    // Lowered to 0 it starts.
    let r = app.voice_clone_train("cortana", Some(3), Some(0.0)).unwrap();
    assert_eq!(r["state"], "running");
    assert_eq!(r["lines"], 2);
    let id = r["job"].as_u64().unwrap();
    let mut api = Api::new();
    let done = wait(&mut api, &mut app, id);
    assert_eq!(done["state"], "done", "{done}");

    // The picker's choice: the zero-shot model says the line from a
    // reference clip and the dataset's text, with no fine-tune, and
    // cannot be trained.
    app.voices.set_backend(crate::voice::sidecar::Backend::Qwen3Tts);
    let said = call(
        &mut api,
        &mut app,
        m::VOICE_CLONE_SAY,
        json!({"character": "cortana", "text": "Hold on.", "play": false}),
    );
    assert_eq!(said["backend"], "qwen3-tts");
    assert_eq!(said["model"], "qwen3-tts");
    let asked = fake.calls_of("say");
    let last = asked.last().unwrap();
    assert_eq!(last["backend"], "qwen3-tts");
    assert_eq!(last["checkpoint"], Value::Null);
    assert!(last["reference"]["path"].as_str().unwrap().ends_with(".wav"));
    assert!(!last["reference"]["text"].as_str().unwrap().is_empty());
    assert!(app.voice_clone_train("cortana", None, Some(0.0)).is_err());
}

#[test]
fn a_batch_of_lines_is_said_row_by_row_each_with_its_own_state() {
    let t = Temp::new("batch");
    let mut app = test_app(&t);
    let mut api = Api::new();
    let fake = test_setup(&mut app, true, true);
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_SAY_BATCH,
        json!({
            "character": "cortana",
            "rows": [
                {"text": "Hold on, Chief.", "clip": "cortana-hold"},
                {"text": "", "clip": "cortana-empty"},
                {"text": "Bounce.", "clip": "Not A Clip"},
                {"text": "Dup.", "clip": "cortana-hold"},
                {"text": "Wake up.", "character": "johnson"},
            ],
        }),
    );
    // Queued, not said: the window's frames (or `wait`) do that.
    assert_eq!(r["added"], 5, "{r}");
    assert_eq!(r["queued"], 2, "{r}");
    assert_eq!(r["rows"][1]["state"], "failed");
    assert_eq!(r["rows"][1]["why"], "no text");
    assert_eq!(r["rows"][2]["state"], "failed");
    assert!(r["rows"][3]["why"].as_str().unwrap().contains("line 1"), "{r}");
    assert_eq!(fake.calls_of("say").len(), 0);

    // A step starts a row running, the next says it.
    assert!(app.poll_voice_batch());
    let s = call(&mut api, &mut app, m::VOICE_BATCH_STATUS, json!({}));
    assert_eq!(s["counts"]["running"], 1, "{s}");
    assert_eq!(s["rows"][0]["state"], "running");
    assert_eq!(s["active"], true);
    // A second say_batch is refused while rows are left.
    let e = fails(api.call(&mut app, m::VOICE_SAY_BATCH, json!({"lines": "More."})));
    assert_eq!(e.code, code::REFUSED);
    app.voice_batch_run();

    let s = call(&mut api, &mut app, m::VOICE_BATCH_STATUS, json!({"ids": [1]}));
    assert_eq!(s["rows"].as_array().unwrap().len(), 1);
    assert_eq!(s["rows"][0]["state"], "done", "{s}");
    assert_eq!(s["rows"][0]["cached"], false);
    let file = PathBuf::from(s["rows"][0]["file"].as_str().unwrap());
    assert!(file.ends_with("voices/cortana/clips/cortana-hold.wav"), "{file:?}");
    assert!(file.is_file());
    assert_eq!(s["counts"]["done"], 1);
    assert_eq!(s["counts"]["failed"], 4);
    assert_eq!(s["active"], false);
    // Johnson has no dataset: that row fails with why, the others do not.
    let all = call(&mut api, &mut app, m::VOICE_BATCH_STATUS, json!({}));
    assert_eq!(all["rows"][4]["state"], "failed", "{all}");
    assert!(all["rows"][4]["why"].as_str().unwrap().contains("dataset"), "{all}");
    assert_eq!(fake.calls_of("say").len(), 1);
}

#[test]
fn pasted_lines_and_files_become_rows_and_a_rerun_asks_the_model_again() {
    let t = Temp::new("batch-paste");
    let mut app = test_app(&t);
    let mut api = Api::new();
    let fake = test_setup(&mut app, true, true);
    let file = t.0.join("lines.txt");
    std::fs::write(&file, "From a file.\n\n  \nAnd another.\n").unwrap();
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_SAY_BATCH,
        json!({
            "character": "cortana",
            "lines": "First.\ncortana-second\tSecond, with a name.\n",
            "file": file.display().to_string(),
            "wait": true,
        }),
    );
    assert_eq!(r["added"], 4, "{r}");
    assert_eq!(r["counts"]["done"], 4, "{r}");
    assert_eq!(r["rows"][1]["clip"], "cortana-second");
    assert_eq!(r["rows"][1]["text"], "Second, with a name.");
    assert_eq!(fake.calls_of("say").len(), 4);

    // The same row again is a new take, not the cache.
    let r = call(&mut api, &mut app, m::VOICE_BATCH_RUN, json!({"ids": [1], "wait": true}));
    assert_eq!(r["queued"], 1);
    assert_eq!(r["rows"][0]["state"], "done");
    assert_eq!(r["rows"][0]["cached"], false);
    assert_eq!(fake.calls_of("say").len(), 5);

    // A bad param adds nothing.
    let e = fails(api.call(
        &mut app,
        m::VOICE_SAY_BATCH,
        json!({"append": true, "rows": [{"text": "x", "character": "Bad Id"}]}),
    ));
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert_eq!(app.voices.batch.rows.len(), 4);
    let e = fails(api.call(&mut app, m::VOICE_SAY_BATCH, json!({"character": "cortana"})));
    assert_eq!(e.code, code::INVALID_PARAMS);
}

#[test]
fn a_failing_model_fails_its_rows_and_the_done_ones_are_saved_together() {
    let t = Temp::new("batch-save");
    let mut app = test_app(&t);
    let mut api = Api::new();
    test_setup(&mut app, true, true);
    call(
        &mut api,
        &mut app,
        m::VOICE_SAY_BATCH,
        json!({
            "character": "cortana",
            "rows": [
                {"text": "One.", "clip": "cortana-one"},
                {"text": "Two.", "clip": "cortana-two"},
                {"text": "Three."},
            ],
            "wait": true,
        }),
    );
    let repo = t.0.join("repo");
    std::fs::create_dir_all(repo.join("art/announcer")).unwrap();
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_BATCH_SAVE,
        json!({"repo": repo.display().to_string()}),
    );
    assert_eq!(r["saved"], 2, "{r}");
    assert_eq!(r["failed"], 1, "{r}");
    assert!(r["rows"][2]["why"].as_str().unwrap().contains("output name"));
    assert!(repo.join("art/announcer/cortana-one.wav").is_file());
    assert!(repo.join("art/announcer/cortana-two.wav").is_file());
    let s = call(&mut api, &mut app, m::VOICE_BATCH_STATUS, json!({}));
    assert!(s["rows"][0]["saved"].as_str().unwrap().ends_with("cortana-one.wav"));
    assert!(s["rows"][2]["save_failed"].is_string());

    // A second save does not replace what the checkout has, unless asked.
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_BATCH_SAVE,
        json!({"repo": repo.display().to_string(), "ids": [1]}),
    );
    assert_eq!(r["failed"], 1, "{r}");
    assert!(r["rows"][0]["why"].as_str().unwrap().contains("replace"));
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_BATCH_SAVE,
        json!({"repo": repo.display().to_string(), "ids": [1], "replace": true}),
    );
    assert_eq!(r["saved"], 1, "{r}");

    // A row that fails in the model says why and does not stop the rest.
    let mut fake = crate::voice::fake::FakeLauncher::ready();
    fake.fail_say = Some("out of memory".into());
    app.voices.set_launcher(std::sync::Arc::new(fake));
    let r = call(
        &mut api,
        &mut app,
        m::VOICE_SAY_BATCH,
        json!({"character": "cortana", "lines": "Hm.\nHmm.", "wait": true}),
    );
    assert_eq!(r["counts"]["failed"], 2, "{r}");
    assert!(r["rows"][1]["why"].as_str().unwrap().contains("out of memory"));
}

#[test]
fn the_window_will_not_say_a_line_with_no_output_name_and_no_game_line() {
    use crate::app::voice_batch::Batch;
    let mut b = Batch::default();
    b.default_character = "cortana".into();
    let bare = b.add("Hold on, Chief.", None, "");
    let named = b.add("Wake up.", None, "cortana-wake");
    let aimed = b.add("Bounce.", None, "");
    let none = b.add("", None, "x");
    b.rows[2].tag = Some(r"sound\dialog\combat\cortana\bounce".into());
    let why = |b: &Batch, id| b.blocked(b.row(id).unwrap());
    assert!(why(&b, bare).unwrap().contains("output name"), "{:?}", why(&b, bare));
    assert!(why(&b, bare).unwrap().contains("game line"));
    assert_eq!(why(&b, named), None);
    // A game line to replace is as good as a name.
    assert_eq!(why(&b, aimed), None);
    // A problem of the line's own comes first.
    assert_eq!(why(&b, none).as_deref(), Some("no text"));
    // The API's own queue does not ask for a name: bare queues with
    // named and aimed, and only the row with no text fails.
    assert_eq!(b.queue(None), 3);
}

// ---------- Voices as documents (docs/editor/studio-flow.md, FL5) ----------

/// A dialogue sound the invented cortana rules pick.
const CORTANA_LINE: &str = r"sound\dialog\levels\test_level\mission\l01_0010_cor";
const JOHNSON_LINE: &str = r"sound\dialog\levels\test_level\mission\l01_0060_jon";

#[test]
fn the_voices_kind_lists_every_character_with_its_dataset_state() {
    let t = Temp::new("voices-kind");
    let mut app = test_app(&t);
    let mut api = Api::new();
    test_setup(&mut app, true, true);
    let r = call(&mut api, &mut app, m::ASSET_LIST, json!({"kind": "Voices"}));
    let rows = r["rows"].as_array().unwrap();
    // Every character of the table, then the announcer.
    assert_eq!(rows.len(), dataset::CHARACTERS.len() + 1, "{r}");
    assert!(r["kinds"].as_array().unwrap().iter().any(|k| k == "Voices"));
    let of = |id: &str| {
        rows.iter()
            .find(|x| x["asset"] == format!("voice:{id}"))
            .cloned()
            .unwrap_or_else(|| panic!("no row for {id}: {r}"))
    };
    // Cortana's dataset is built; Johnson's is not.
    assert_eq!(of("cortana")["state"], "dataset");
    assert!(of("cortana")["minutes"].as_f64().unwrap() > 0.0);
    assert_eq!(of("cortana")["built"], true);
    assert_eq!(of("johnson")["state"], "no dataset");
    assert_eq!(of("johnson")["built"], false);
    assert_eq!(of("announcer")["state"], "announcer");
    // Words narrow it as the Browser's search does.
    let one = call(
        &mut api,
        &mut app,
        m::ASSET_LIST,
        json!({"kind": "voices", "query": "johnson"}),
    );
    assert_eq!(one["total"], 1, "{one}");
    assert_eq!(one["rows"][0]["asset"], "voice:johnson");
}

#[test]
fn a_voice_with_no_dataset_opens_on_setup_and_a_built_one_on_lines() {
    use crate::app::Doc;
    use crate::app::voices::Pane;

    let t = Temp::new("voices-doc");
    let mut app = test_app(&t);
    let mut api = Api::new();
    test_setup(&mut app, true, true);

    let r = call(&mut api, &mut app, m::SHELL_OPEN, json!({"asset": "voice:johnson"}));
    assert_eq!(r["front"], "voice:johnson", "{r}");
    assert_eq!(r["tab"]["kind"], "voice");
    assert_eq!(r["tab"]["title"], "Sergeant Johnson");
    assert_eq!(app.front_doc(), Some(Doc::Voice));
    assert_eq!(app.voices.pane, Pane::Setup);

    // One tab: opening another voice shows it there, on its own pane, with
    // that voice the batch's default.
    let r = call(&mut api, &mut app, m::SHELL_OPEN, json!({"asset": "voice:cortana"}));
    assert_eq!(r["front"], "voice:cortana", "{r}");
    assert_eq!(app.voices.pane, Pane::Lines);
    assert_eq!(app.voices.batch.default_character, "cortana");
    let tabs = call(&mut api, &mut app, m::SHELL_TABS, json!({}));
    let voices: Vec<&Value> = tabs["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tab| tab["kind"] == "voice")
        .collect();
    assert_eq!(voices.len(), 1, "{tabs}");

    // The announcer has no setup to do.
    call(&mut api, &mut app, m::SHELL_OPEN, json!({"asset": "voice:announcer"}));
    assert_eq!(app.voices.pane, Pane::Lines);

    // Closing the tab keeps the batch; an unknown voice is refused.
    let r = call(&mut api, &mut app, m::SHELL_CLOSE, json!({"doc": "voice:announcer"}));
    assert_eq!(r["closed"], true, "{r}");
    assert_eq!(app.voices.doc, None);
    let e = fails(api.call(&mut app, m::SHELL_OPEN, json!({"asset": "voice:nobody"})));
    assert_eq!(e.code, code::REFUSED, "{}", e.message);
    assert_eq!(app.voices.doc, None);
}

#[test]
fn a_line_said_from_a_sounds_pane_is_a_row_whose_target_is_that_sound() {
    use crate::app::audition::Clip;
    use crate::app::voice_batch::{RowState, axis_columns};

    let t = Temp::new("voices-sound-line");
    let mut app = test_app(&t);
    let mut api = Api::new();
    let fake = test_setup(&mut app, true, true);

    // **New line** on a sound whose path the cortana rules pick: the row
    // is said by her, and one press again is the same row.
    let id = app.sound_new_line(CORTANA_LINE);
    assert_eq!(app.sound_new_line(CORTANA_LINE), id);
    {
        let row = app.voices.batch.row(id).unwrap();
        assert_eq!(row.tag.as_deref(), Some(CORTANA_LINE));
        assert_eq!(row.character.as_deref(), Some("cortana"));
        assert_eq!(row.state, RowState::Idle);
    }
    // Typed and said, as the pane's **Say** does.
    app.voices.batch.rows[0].text = "Hold on, Chief.".into();
    assert_eq!(app.voices.batch.queue(Some(&[id])), 1);
    app.voice_batch_run();
    assert_eq!(fake.calls_of("say").len(), 1);

    let lines = app.sound_lines(CORTANA_LINE);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].state, RowState::Done, "{:?}", lines[0].state);
    let said = lines[0].said.as_ref().expect("the line was said");
    let ms = said.levels.duration_ms;
    assert!(ms > 0);

    // The levels draw under the original's, on its axis: a sound as long
    // as the line fills the pane, a sound twice as long half of it, and a
    // longer line is cut where the axis ends.
    assert_eq!(axis_columns(ms, ms, 400), 400);
    assert_eq!(axis_columns(ms * 2, ms, 400), 200);
    assert_eq!(axis_columns(ms / 2, ms, 400), 400);
    assert_eq!(axis_columns(0, ms, 400), 0);
    let clip = Clip {
        rate: said.levels.rate,
        channels: 1,
        pcm: said.pcm.clone(),
    };
    assert_eq!(clip.peaks(axis_columns(ms * 2, ms, 400)).len(), 200);

    // The voice's links name the sound its line replaces.
    let v = call(&mut api, &mut app, m::ASSET_LINKS, json!({"asset": "voice:cortana"}));
    let uses = v["uses"].as_array().unwrap();
    assert!(
        uses.iter().any(|l| l["asset"] == format!("tag:snd!:{CORTANA_LINE}")
            && l["role"] == "Sounds"
            && l["via"] == "line 1"),
        "{v}"
    );
    assert_eq!(v["editors"][0]["id"], "voices");

    // A voice with no dataset says why on its row, and its pane links to
    // its Setup (`voice:johnson`).
    let jon = app.sound_new_line(JOHNSON_LINE);
    app.voices.batch.rows.iter_mut().find(|r| r.id == jon).unwrap().text = "Wake up.".into();
    app.voices.batch.queue(Some(&[jon]));
    app.voice_batch_run();
    let row = app.voices.batch.row(jon).unwrap();
    assert_eq!(row.character.as_deref(), Some("johnson"));
    assert!(
        matches!(&row.state, RowState::Failed(w) if w.contains("dataset")),
        "{:?}",
        row.state
    );
    assert!(app.voice_status("johnson").unwrap().needs_setup());
}
