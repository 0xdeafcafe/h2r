//! `sound.edit`, `sound.render` and `sound.permutation` over an empty
//! local package and an invented ramp (8 kHz, sample n is n: no game data).
//! Each op gives the expected samples, and the source file's bytes never
//! change.

use super::*;

const SRC: &str = "sounds/ramp.wav";

fn ramp_wav(frames: i16) -> Vec<u8> {
    ::sound_edit::Wave {
        channels: 1,
        rate: 8000,
        pcm: (0..frames).collect(),
    }
    .to_wav()
}

/// A package `mine` with the ramp (800 frames, 100 ms) imported as a sound.
fn setup(name: &str) -> (Temp, App, Api, Vec<u8>) {
    let t = Temp::new(name);
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    import_tests::package(&t.path("data/packages/mine"), "mine");
    let mut api = Api::new();
    let wav = ramp_wav(800);
    std::fs::write(t.path("ramp.wav"), &wav).unwrap();
    call(
        &mut api,
        &mut app,
        m::IMPORT_SOUND,
        json!({"file": t.path("ramp.wav").display().to_string(), "package": "mine", "tag": "sounds\\ramp"}),
    );
    (t, app, api, wav)
}

fn edit(api: &mut Api, app: &mut App, p: Value) -> Value {
    let mut p = p;
    p["package"] = json!("mine");
    p["source"] = json!(SRC);
    call(api, app, m::SOUND_EDIT, p)
}

fn render_to(t: &Temp, api: &mut Api, app: &mut App, name: &str, extra: Value) -> Vec<i16> {
    let mut p = json!({"package": "mine", "source": SRC, "out": t.path(name).display().to_string()});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    call(api, app, m::SOUND_RENDER, p);
    ::sound_edit::Wave::from_wav(&std::fs::read(t.path(name)).unwrap()).unwrap().pcm
}

fn source_bytes(t: &Temp) -> Vec<u8> {
    std::fs::read(t.path("data/packages/mine").join(SRC)).unwrap()
}

#[test]
fn each_op_renders_the_expected_samples_and_the_wav_never_changes() {
    let (t, mut app, mut api, wav) = setup("sound-edit-ops");
    // Nothing yet: the render is the source.
    assert_eq!(render_to(&t, &mut api, &mut app, "r0.wav", json!({})), (0..800).collect::<Vec<i16>>());

    let v = edit(&mut api, &mut app, json!({"op": "trim", "start_ms": 10, "end_ms": 20}));
    assert_eq!((v["count"].clone(), v["changed"].clone()), (json!(1), json!(true)));
    assert_eq!(v["sidecar"], "sounds/ramp.sound.yaml");
    assert_eq!(v["render"]["frames"], 80);
    assert_eq!(render_to(&t, &mut api, &mut app, "r1.wav", json!({})), (80..160).collect::<Vec<i16>>());

    edit(&mut api, &mut app, json!({"op": "silence", "start_ms": 1, "end_ms": 2}));
    let r = render_to(&t, &mut api, &mut app, "r2.wav", json!({}));
    let mut want: Vec<i16> = (80..160).collect();
    want[8..16].fill(0);
    assert_eq!(r, want);
    // Only the first edit, by `upto`.
    assert_eq!(render_to(&t, &mut api, &mut app, "r3.wav", json!({"upto": 1})), (80..160).collect::<Vec<i16>>());

    // The source is what it was; the edits are in the sidecar.
    assert_eq!(source_bytes(&t), wav);
    let side = std::fs::read_to_string(t.path("data/packages/mine/sounds/ramp.sound.yaml")).unwrap();
    assert!(side.contains("op: trim") && side.contains("op: silence"), "{side}");
    // The sidecar is a data item and the package checks clean, the build
    // rendering the list.
    let loaded = ::workspace::Loaded::from_dir(&t.path("data/packages/mine")).unwrap();
    assert_eq!(::workspace::check_package(&loaded), vec![]);
}

#[test]
fn fades_gain_and_normalise_apply() {
    let (t, mut app, mut api, _) = setup("sound-edit-levels");
    edit(&mut api, &mut app, json!({"op": "trim", "start_ms": 0, "end_ms": 1}));
    edit(&mut api, &mut app, json!({"op": "fade_in", "ms": 0.5}));
    // 8 frames of 0..8, the first 4 faded: k = f/4.
    assert_eq!(render_to(&t, &mut api, &mut app, "f.wav", json!({})), vec![0, 0, 1, 2, 4, 5, 6, 7]);
    edit(&mut api, &mut app, json!({"op": "gain", "db": 20}));
    assert_eq!(render_to(&t, &mut api, &mut app, "g.wav", json!({})), vec![0, 0, 10, 20, 40, 50, 60, 70]);
    let v = edit(&mut api, &mut app, json!({"op": "fade_out", "ms": 0.5}));
    assert_eq!(v["count"], 4);
    // Normalise brings the loudness to the target, within the meter's reading.
    let v = call(
        &mut api,
        &mut app,
        m::SOUND_EDIT,
        json!({"package": "mine", "source": SRC, "op": "clear"}),
    );
    assert_eq!(v["count"], 0);
    // The bare ramp is slow, so the K-weighting's high pass leaves it under
    // the meter's -70 LUFS gate; lift it first so it reads as a signal. The
    // high pass also leaves it quiet for its peak (-7 dBFS after the lift), so
    // a target far above what it reads is held by the -1 dBFS ceiling: ask for
    // 3 dB over the lifted reading, which the ceiling leaves room for.
    let v = edit(&mut api, &mut app, json!({"op": "gain", "db": 25}));
    let target = (v["render"]["lufs"].as_f64().unwrap() + 3.0).round();
    let v = edit(&mut api, &mut app, json!({"op": "normalise", "lufs": target}));
    assert_eq!(v["edits"][1], json!({"op": "normalise", "lufs": target, "ceiling_dbfs": -1.0}));
    let lufs = v["render"]["lufs"].as_f64().unwrap();
    assert!((lufs - target).abs() <= 0.1, "{v}");
}

#[test]
fn undo_and_reorder_give_the_earlier_render() {
    let (t, mut app, mut api, _) = setup("sound-edit-undo");
    edit(&mut api, &mut app, json!({"op": "trim", "start_ms": 0, "end_ms": 1}));
    let first = render_to(&t, &mut api, &mut app, "a.wav", json!({}));
    edit(&mut api, &mut app, json!({"op": "fade_out", "ms": 1}));
    let faded = render_to(&t, &mut api, &mut app, "b.wav", json!({}));
    assert_ne!(faded, first);
    // Moving the fade before the trim changes what it fades.
    edit(&mut api, &mut app, json!({"op": "move", "from": 1, "to": 0}));
    let moved = render_to(&t, &mut api, &mut app, "c.wav", json!({}));
    assert_ne!(moved, faded);
    edit(&mut api, &mut app, json!({"op": "move", "from": 0, "to": 1}));
    assert_eq!(render_to(&t, &mut api, &mut app, "d.wav", json!({})), faded);
    // Undo drops the last edit and says which.
    let v = edit(&mut api, &mut app, json!({"op": "undo"}));
    assert_eq!(v["removed"], json!({"op": "fade_out", "ms": 1.0}));
    assert_eq!(render_to(&t, &mut api, &mut app, "e.wav", json!({})), first);
    // Remove by index, and an insert `at` a place.
    edit(&mut api, &mut app, json!({"op": "gain", "db": 6, "at": 0}));
    let v = edit(&mut api, &mut app, json!({"op": "remove", "index": 0}));
    assert_eq!(v["removed"]["op"], "gain");
    assert_eq!(render_to(&t, &mut api, &mut app, "f.wav", json!({})), first);
}

#[test]
fn an_edit_that_cannot_render_writes_nothing() {
    let (t, mut app, mut api, wav) = setup("sound-edit-refuse");
    let side = t.path("data/packages/mine/sounds/ramp.sound.yaml");
    let try_edit = |api: &mut Api, app: &mut App, p: Value| {
        let mut p = p;
        p["package"] = json!("mine");
        p["source"] = json!(SRC);
        api.call(app, m::SOUND_EDIT, p)
    };
    // A trim past the clip leaves nothing, named by its place.
    let r = try_edit(&mut api, &mut app, json!({"op": "trim", "start_ms": 5000}));
    let e = r.unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.starts_with("edit 0: trim leaves nothing"), "{}", e.message);
    assert!(!side.exists());
    for (p, want) in [
        (json!({"op": "reverb"}), code::INVALID_PARAMS),
        (json!({"op": "fade_in"}), code::INVALID_PARAMS),
        (json!({"op": "gain", "db": 1, "ms": 2}), code::INVALID_PARAMS),
        (json!({"op": "gain", "db": 100}), code::REFUSED),
        (json!({"op": "undo"}), code::REFUSED),
        (json!({"op": "remove", "index": 0}), code::INVALID_PARAMS),
        (json!({"op": "trim", "start_ms": 0, "permutation": 0}), code::INVALID_PARAMS),
    ] {
        assert_eq!(code_of(try_edit(&mut api, &mut app, p.clone())), want, "{p}");
    }
    // A package that is not there, a path that is not an item.
    assert_eq!(
        code_of(api.call(&mut app, m::SOUND_EDIT, json!({"package": "nope", "source": SRC, "op": "list"}))),
        code::INVALID_PARAMS
    );
    for source in ["sounds/other.wav", "../x.wav"] {
        let r = api.call(&mut app, m::SOUND_EDIT, json!({"package": "mine", "source": source, "op": "list"}));
        assert!(matches!(code_of(r), c if c == code::REFUSED || c == code::INVALID_PARAMS), "{source}");
    }
    assert!(!side.exists());
    assert_eq!(source_bytes(&t), wav);
}

#[test]
fn a_render_goes_outside_the_package() {
    let (t, mut app, mut api, wav) = setup("sound-edit-out");
    for out in [
        t.path("data/packages/mine/sounds/ramp.wav"),
        t.path("data/packages/mine/new/render.wav"),
    ] {
        let r = api.call(
            &mut app,
            m::SOUND_RENDER,
            json!({"package": "mine", "source": SRC, "out": out.display().to_string()}),
        );
        assert_eq!(code_of(r), code::REFUSED, "{}", out.display());
    }
    assert_eq!(source_bytes(&t), wav);
    // With no `out` it is numbers: levels before and after, and peaks.
    edit(&mut api, &mut app, json!({"op": "trim", "start_ms": 0, "end_ms": 50}));
    let v = call(&mut api, &mut app, m::SOUND_RENDER, json!({"package": "mine", "source": SRC, "bins": 4}));
    assert_eq!(v["before"]["frames"], 800);
    assert_eq!(v["render"]["frames"], 400);
    assert_eq!(v["edits"], 1);
    assert_eq!(v["peaks"], json!([[0, 99], [100, 199], [200, 299], [300, 399]]));
    assert!(v.get("out").is_none());
    let bad = api.call(&mut app, m::SOUND_RENDER, json!({"package": "mine", "source": SRC, "bins": 0}));
    assert_eq!(code_of(bad), code::INVALID_PARAMS);
    let bad = api.call(&mut app, m::SOUND_RENDER, json!({"package": "mine", "source": SRC, "upto": 5}));
    assert_eq!(code_of(bad), code::INVALID_PARAMS);
}

fn perm(api: &mut Api, app: &mut App, p: Value) -> Value {
    let mut p = p;
    p["package"] = json!("mine");
    p["source"] = json!(SRC);
    call(api, app, m::SOUND_PERMUTATION, p)
}

#[test]
fn permutations_are_files_of_the_package() {
    let (t, mut app, mut api, wav) = setup("sound-edit-perm");
    let dir = t.path("data/packages/mine");
    std::fs::write(dir.join("sounds/ramp_b.wav"), ramp_wav(400)).unwrap();
    std::fs::write(dir.join("sounds/ramp_c.wav"), ramp_wav(200)).unwrap();

    let v = perm(&mut api, &mut app, json!({"op": "add", "file": "sounds/ramp_b.wav", "weight": 3}));
    assert_eq!(v["count"], 1);
    assert_eq!(v["permutations"][0]["file"], "sounds/ramp_b.wav");
    assert_eq!(v["permutations"][0]["weight"], 3.0);
    assert_eq!(v["permutations"][0]["levels"]["frames"], 400);
    assert_eq!(v["source_row"]["levels"]["frames"], 800);
    perm(&mut api, &mut app, json!({"op": "add", "file": "sounds/ramp_c.wav"}));
    let v = perm(&mut api, &mut app, json!({"op": "set", "index": 1, "gain_db": -3, "skip_fraction": 0.5}));
    assert_eq!((v["permutations"][1]["gain_db"].clone(), v["permutations"][1]["skip_fraction"].clone()), (json!(-3.0), json!(0.5)));
    let v = perm(&mut api, &mut app, json!({"op": "move", "from": 1, "to": 0}));
    assert_eq!(v["permutations"][0]["file"], "sounds/ramp_c.wav");

    // The files are sound items now, and the package checks clean.
    let loaded = ::workspace::Loaded::from_dir(&dir).unwrap();
    let manifest =
        content::package::Manifest::from_toml(&std::fs::read_to_string(dir.join("package.toml")).unwrap()).unwrap();
    assert!(manifest.items.iter().any(|i| i.source.as_deref() == Some("sounds/ramp_b.wav")));
    assert_eq!(::workspace::check_package(&loaded), vec![]);

    // A permutation has its own edit list; the others are as they were.
    let v = call(
        &mut api,
        &mut app,
        m::SOUND_EDIT,
        json!({"package": "mine", "source": SRC, "permutation": 0, "op": "trim", "start_ms": 0, "end_ms": 5}),
    );
    assert_eq!(v["render"]["frames"], 40);
    let v = perm(&mut api, &mut app, json!({"op": "list"}));
    assert_eq!(v["permutations"][0]["edits"], 1);
    assert_eq!(v["permutations"][0]["levels"]["frames"], 40);
    assert_eq!(v["permutations"][1]["levels"]["frames"], 400);
    let own = call(&mut api, &mut app, m::SOUND_EDIT, json!({"package": "mine", "source": SRC, "op": "list"}));
    assert_eq!(own["count"], 0);

    let v = perm(&mut api, &mut app, json!({"op": "remove", "index": 0}));
    assert_eq!(v["count"], 1);
    assert_eq!(v["permutations"][0]["file"], "sounds/ramp_b.wav");
    // Every file stays as it was.
    assert_eq!(source_bytes(&t), wav);
    assert_eq!(std::fs::read(dir.join("sounds/ramp_b.wav")).unwrap(), ramp_wav(400));
}

#[test]
fn permutation_refusals() {
    let (t, mut app, mut api, _) = setup("sound-edit-perm-refuse");
    std::fs::write(t.path("data/packages/mine/sounds/b.wav"), ramp_wav(40)).unwrap();
    std::fs::write(t.path("data/packages/mine/sounds/bad.wav"), b"not a wav").unwrap();
    perm(&mut api, &mut app, json!({"op": "add", "file": "sounds/b.wav"}));
    let go = |api: &mut Api, app: &mut App, p: Value| {
        let mut p = p;
        p["package"] = json!("mine");
        p["source"] = json!(SRC);
        code_of(api.call(app, m::SOUND_PERMUTATION, p))
    };
    for (p, want) in [
        (json!({"op": "add", "file": "sounds/b.wav"}), code::INVALID_PARAMS),
        (json!({"op": "add", "file": "sounds/none.wav"}), code::FAILED),
        (json!({"op": "add", "file": "sounds/bad.wav"}), code::REFUSED),
        (json!({"op": "add", "file": "../b.wav"}), code::INVALID_PARAMS),
        (json!({"op": "set", "index": 0, "weight": 0}), code::INVALID_PARAMS),
        (json!({"op": "set", "index": 0, "skip_fraction": 2}), code::INVALID_PARAMS),
        (json!({"op": "set", "index": 5, "weight": 1}), code::INVALID_PARAMS),
        (json!({"op": "remove", "index": 5}), code::INVALID_PARAMS),
        (json!({"op": "move", "from": 0, "to": 3}), code::INVALID_PARAMS),
        (json!({"op": "shuffle"}), code::INVALID_PARAMS),
    ] {
        assert_eq!(go(&mut api, &mut app, p.clone()), want, "{p}");
    }
    let v = perm(&mut api, &mut app, json!({"op": "list"}));
    assert_eq!(v["count"], 1);
    assert_eq!(v["permutations"][0]["weight"], 1.0);
}
