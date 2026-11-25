//! The `image.*` methods over the invented textured map, whose floor
//! bitmap is 4 by 4 pixels of one `A8R8G8B8` colour, (200, 60, 20, 255).
//! The generator is always the fake source (or a Codex that is missing or
//! signed out): no test runs the real Codex.

use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::imagegen::codex::CodexCli;
use crate::imagegen::fake::FakeSource;
use crate::imagegen::{GenError, ImageSource};

/// The fake source, and job directories under `t`.
pub(super) fn fake(app: &mut App, t: &Temp) {
    use_source(app, t, Arc::new(FakeSource::new()));
}

fn use_source(app: &mut App, t: &Temp, source: Arc<dyn ImageSource>) {
    app.imagegen.set_source(source);
    app.imagegen.set_root(t.path("imagegen"));
}

fn textured(t: &Temp) -> (Api, App) {
    let mut app = app(t);
    let mut api = Api::new();
    let map = t.path("textured.map");
    std::fs::write(&map, fixture::fixture_with_textured_bsp()).unwrap();
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );
    (api, app)
}

fn floor_pixel(api: &mut Api, app: &mut App, x: u32, y: u32) -> String {
    let r = call(
        api,
        app,
        m::BITMAP_PIXELS,
        json!({"bitmap": "floor", "rect": {"x": x, "y": y, "w": 1, "h": 1}}),
    );
    r["rgba"].as_str().unwrap().to_owned()
}

fn wait(app: &mut App, id: u64) {
    assert!(
        app.imagegen.wait(id, Duration::from_secs(20)),
        "image {id} did not finish"
    );
}

#[test]
fn a_generation_is_a_job_running_then_done_and_fitted_to_its_slot() {
    let t = Temp::new("imagegen-job");
    let (mut api, mut app) = textured(&t);
    let (source, go) = FakeSource::gated();
    let requests = Arc::clone(&source.requests);
    use_source(&mut app, &t, Arc::new(source));
    let r = call(
        &mut api,
        &mut app,
        m::IMAGE_GENERATE,
        json!({"prompt": "a round blue badge", "references": [{"bitmap": "floor"}], "bitmap": "floor"}),
    );
    assert_eq!(r["state"], "running");
    assert_eq!(r["generator"], "fake");
    let id = r["id"].as_u64().unwrap();
    let running = call(&mut api, &mut app, m::IMAGE_RESULT, json!({"id": id}));
    assert_eq!(running["state"], "running", "{running}");
    // One job at a time.
    let again = api.call(
        &mut app,
        m::IMAGE_GENERATE,
        json!({"prompt": "another", "references": []}),
    );
    assert_eq!(code_of(again), code::REFUSED);
    go.send(()).unwrap();
    wait(&mut app, id);
    let done = call(&mut api, &mut app, m::IMAGE_RESULT, json!({}));
    assert_eq!(done["state"], "done", "{done}");
    assert_eq!(done["raw"]["width"], 64);
    assert_eq!(done["fitted"]["width"], 4);
    assert_eq!(done["fitted"]["height"], 4);
    assert_eq!(done["fitted"]["format"], "Argb8888");
    assert_eq!(done["slot"]["name"], "levels\\test\\bitmaps\\floor");
    assert_eq!(done["applied"], false);
    // The map reference was exported for the generator and recorded.
    assert_eq!(done["references"][0]["origin"], "map");
    assert_eq!(done["references"][0]["image"], 0);
    let sent = requests.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].references[0].is_file());
    assert_eq!(sent[0].size, Some((4, 4)));
    assert!(sent[0].prompt.starts_with("a round blue badge"));
    assert!(sent[0].prompt.contains("style reference"));
    let fitted = std::fs::read(done["fitted"]["path"].as_str().unwrap()).unwrap();
    let (w, h, _) = bitmap_edit::decode_png(&fitted).unwrap();
    assert_eq!((w, h), (4, 4));
}

#[test]
fn apply_writes_the_slot_as_an_undo_step_and_discard_takes_it_out() {
    let t = Temp::new("imagegen-apply");
    let (mut api, mut app) = textured(&t);
    fake(&mut app, &t);
    let before = floor_pixel(&mut api, &mut app, 1, 1);
    assert_eq!(before, "c83c14ff");
    let id = call(
        &mut api,
        &mut app,
        m::IMAGE_GENERATE,
        json!({"prompt": "a round blue badge", "references": [], "bitmap": "floor"}),
    )["id"]
        .as_u64()
        .unwrap();
    wait(&mut app, id);
    let a = call(&mut api, &mut app, m::IMAGE_APPLY, json!({"id": id}));
    assert_eq!(a["applied"], true);
    // The slot holds the fitted picture, pixel for pixel.
    let fitted = app.imagegen.job(id).unwrap().outcome().unwrap().fitted.clone();
    let (_, _, rgba) = bitmap_edit::decode_png(&std::fs::read(fitted).unwrap()).unwrap();
    let at = (4 + 1) * 4;
    let want = rgba[at..at + 4].iter().map(|b| format!("{b:02x}")).collect::<String>();
    let now = floor_pixel(&mut api, &mut app, 1, 1);
    assert_ne!(now, before);
    assert_eq!(now, want);
    let read = call(&mut api, &mut app, m::BITMAP_READ, json!({"bitmap": "floor"}));
    assert_eq!(read["edited"], true);
    assert_eq!(
        call(&mut api, &mut app, m::IMAGE_RESULT, json!({"id": id}))["applied"],
        true
    );
    let d = call(&mut api, &mut app, m::IMAGE_DISCARD, json!({"id": id}));
    assert!(d["line"].as_str().unwrap().contains("undone"), "{d}");
    assert_eq!(floor_pixel(&mut api, &mut app, 1, 1), before);
    let r = call(&mut api, &mut app, m::IMAGE_RESULT, json!({"id": id}));
    assert_eq!(r["applied"], false);
    assert_eq!(r["discarded"], true);
}

#[test]
fn discard_after_later_edits_imports_the_original_back() {
    let t = Temp::new("imagegen-discard-late");
    let (mut api, mut app) = textured(&t);
    fake(&mut app, &t);
    let id = call(
        &mut api,
        &mut app,
        m::IMAGE_GENERATE,
        json!({"prompt": "a badge", "references": [], "bitmap": "floor"}),
    )["id"]
        .as_u64()
        .unwrap();
    wait(&mut app, id);
    call(&mut api, &mut app, m::IMAGE_APPLY, json!({"id": id}));
    call(
        &mut api,
        &mut app,
        m::BITMAP_FILL,
        json!({"bitmap": "floor", "rect": {"x": 0, "y": 0, "w": 1, "h": 1}, "color": "#00ff00"}),
    );
    let d = call(&mut api, &mut app, m::IMAGE_DISCARD, json!({"id": id}));
    assert!(d["line"].as_str().unwrap().contains("original"), "{d}");
    assert_eq!(floor_pixel(&mut api, &mut app, 2, 2), "c83c14ff");
}

#[test]
fn accept_keeps_the_edit_and_records_provenance() {
    let t = Temp::new("imagegen-accept");
    let (mut api, mut app) = textured(&t);
    fake(&mut app, &t);
    let png = bitmap_edit::encode_png(2, 2, &[1, 2, 3, 255].repeat(4)).unwrap();
    std::fs::create_dir_all(t.path("refs")).unwrap();
    std::fs::write(t.path("refs/other.png"), png).unwrap();
    let id = call(
        &mut api,
        &mut app,
        m::IMAGE_GENERATE,
        json!({
            "prompt": "a round blue badge",
            "references": [t.path("refs/other.png").display().to_string()],
            "bitmap": "floor",
        }),
    )["id"]
        .as_u64()
        .unwrap();
    wait(&mut app, id);
    let save = t.path("art");
    let a = call(
        &mut api,
        &mut app,
        m::IMAGE_ACCEPT,
        json!({"id": id, "save": save.display().to_string()}),
    );
    assert_eq!(a["accepted"], true);
    // Accepting applies it: the bitmap is edited and saves with the map.
    assert_eq!(
        call(&mut api, &mut app, m::BITMAP_READ, json!({"bitmap": "floor"}))["edited"],
        true
    );
    let files: Vec<String> = a["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap().to_owned())
        .collect();
    assert!(files.iter().any(|f| f.ends_with("floor-0.png")));
    assert!(files.iter().any(|f| f.ends_with("floor-0.fitted.png")));
    let prov_path = files
        .iter()
        .find(|f| f.ends_with("floor-0.provenance.json"))
        .unwrap();
    let prov: Value = serde_json::from_str(&std::fs::read_to_string(prov_path).unwrap()).unwrap();
    assert_eq!(prov["prompt"], "a round blue badge");
    assert_eq!(prov["generator"], "fake");
    assert_eq!(prov["references"][0]["origin"], "file");
    assert_eq!(prov["slot"]["format"], "Argb8888");
    assert_eq!(prov["accepted"], true);
    assert!(prov["note"].as_str().unwrap().contains("the user picked"));
    // The job's own record, and a log line naming it.
    let dir = call(&mut api, &mut app, m::IMAGE_RESULT, json!({"id": id}))["dir"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(Path::new(&dir).join("provenance.json").is_file());
    assert!(app.log().iter().any(|l| l.status.text().contains("accepted into")));
}

#[test]
fn a_regeneration_names_its_job_and_bad_calls_are_refused() {
    let t = Temp::new("imagegen-refused");
    let (mut api, mut app) = textured(&t);
    fake(&mut app, &t);
    // No job yet.
    assert_eq!(code_of(api.call(&mut app, m::IMAGE_RESULT, json!({}))), code::REFUSED);
    assert_eq!(
        code_of(api.call(&mut app, m::IMAGE_RESULT, json!({"id": 9}))),
        code::INVALID_PARAMS
    );
    // A slot and a size together, an empty prompt, a bad alpha.
    for p in [
        json!({"prompt": "x", "references": [], "bitmap": "floor", "size": [8, 8]}),
        json!({"prompt": " ", "references": []}),
        json!({"prompt": "x", "references": [], "alpha": "half"}),
        json!({"prompt": "x", "references": [{"image": 1}]}),
        json!({"prompt": "x", "references": [], "from": 4}),
    ] {
        assert_eq!(code_of(api.call(&mut app, m::IMAGE_GENERATE, p.clone())), code::INVALID_PARAMS, "{p}");
    }
    // A slot the map does not have.
    assert_ne!(
        code_of(api.call(
            &mut app,
            m::IMAGE_GENERATE,
            json!({"prompt": "x", "references": [], "bitmap": "floor", "image": 3})
        )),
        0
    );
    // Without a slot: fitted to the size, and nothing to apply it to.
    let id = call(
        &mut api,
        &mut app,
        m::IMAGE_GENERATE,
        json!({"prompt": "x", "references": [], "size": [16, 8], "out": t.path("out/x.png").display().to_string()}),
    )["id"]
        .as_u64()
        .unwrap();
    wait(&mut app, id);
    let r = call(&mut api, &mut app, m::IMAGE_RESULT, json!({"id": id}));
    assert_eq!(r["fitted"]["width"], 16);
    assert_eq!(r["fitted"]["height"], 8);
    assert!(t.path("out/x.png").is_file());
    assert_eq!(code_of(api.call(&mut app, m::IMAGE_APPLY, json!({"id": id}))), code::REFUSED);
    // Accepted with no slot: the art and its provenance, for a slot made
    // later; the map is untouched.
    let a = call(
        &mut api,
        &mut app,
        m::IMAGE_ACCEPT,
        json!({"id": id, "save": t.path("art").display().to_string()}),
    );
    let files = a["files"].to_string();
    assert!(files.contains(&format!("image-{id}.png")), "{files}");
    assert!(files.contains(&format!("image-{id}.provenance.json")), "{files}");
    assert_eq!(
        call(&mut api, &mut app, m::BITMAP_READ, json!({"bitmap": "floor"}))["edited"],
        false
    );
    // A tweak of it: a new job that names the one it came from.
    let again = call(
        &mut api,
        &mut app,
        m::IMAGE_GENERATE,
        json!({"prompt": "x, but red", "references": [], "size": [16, 8], "from": id}),
    )["id"]
        .as_u64()
        .unwrap();
    wait(&mut app, again);
    assert_eq!(
        call(&mut api, &mut app, m::IMAGE_RESULT, json!({"id": again}))["from"],
        id
    );
}

#[test]
fn a_failed_generation_says_why() {
    let t = Temp::new("imagegen-failed");
    let (mut api, mut app) = textured(&t);
    use_source(
        &mut app,
        &t,
        Arc::new(FakeSource::failing(GenError::Quota("out of allowance".into()))),
    );
    let id = call(
        &mut api,
        &mut app,
        m::IMAGE_GENERATE,
        json!({"prompt": "x", "references": [], "bitmap": "floor"}),
    )["id"]
        .as_u64()
        .unwrap();
    wait(&mut app, id);
    let r = call(&mut api, &mut app, m::IMAGE_RESULT, json!({"id": id}));
    assert_eq!(r["state"], "failed");
    assert_eq!(r["error"], "quota");
    assert_eq!(r["why"], "out of allowance");
    assert_eq!(code_of(api.call(&mut app, m::IMAGE_APPLY, json!({"id": id}))), code::REFUSED);
}

#[test]
fn a_missing_codex_says_how_to_install_and_sign_in() {
    let t = Temp::new("imagegen-missing");
    let (mut api, mut app) = textured(&t);
    let codex = CodexCli {
        program: t.path("no-such-dir/codex"),
        home: Some(t.path("codex-home")),
        ..CodexCli::default()
    };
    use_source(&mut app, &t, Arc::new(codex));
    let id = call(
        &mut api,
        &mut app,
        m::IMAGE_GENERATE,
        json!({"prompt": "x", "references": [], "bitmap": "floor"}),
    )["id"]
        .as_u64()
        .unwrap();
    wait(&mut app, id);
    let r = call(&mut api, &mut app, m::IMAGE_RESULT, json!({"id": id}));
    assert_eq!(r["state"], "failed");
    assert_eq!(r["error"], "missing");
    let why = r["why"].as_str().unwrap();
    assert!(why.contains("not found"), "{why}");
    assert!(why.contains("codex login"), "{why}");
}

#[cfg(unix)]
#[test]
fn a_signed_out_codex_says_to_sign_in() {
    let t = Temp::new("imagegen-signed-out");
    let (mut api, mut app) = textured(&t);
    let program = crate::imagegen::tests::script(
        &t.path("bin"),
        "case \"$1\" in --version) echo 'codex-cli 0.170.0';; login) echo 'Not logged in'; exit 1;; *) exit 9;; esac",
    );
    let codex = CodexCli {
        program,
        home: Some(t.path("codex-home")),
        ..CodexCli::default()
    };
    use_source(&mut app, &t, Arc::new(codex));
    let id = call(
        &mut api,
        &mut app,
        m::IMAGE_GENERATE,
        json!({"prompt": "x", "references": [], "bitmap": "floor"}),
    )["id"]
        .as_u64()
        .unwrap();
    wait(&mut app, id);
    let r = call(&mut api, &mut app, m::IMAGE_RESULT, json!({"id": id}));
    assert_eq!(r["state"], "failed");
    assert_eq!(r["error"], "unauthenticated");
    let why = r["why"].as_str().unwrap();
    assert!(why.contains("codex login"), "{why}");
    assert!(why.contains("Not logged in"), "{why}");
}
