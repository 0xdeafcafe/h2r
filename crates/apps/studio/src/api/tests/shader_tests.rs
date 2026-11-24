//! `shader.read`, `shader.set`, `shader.preview` and `shader.code` over the scenario
//! crate's invented material (`fixture::fixture_with_shader_colour`: the
//! floor shader with one colour its template's one pass reads at the
//! final combiner; no game data). docs/editor/rich-editors.md, slice S1.

use super::*;

const SHADER: &str = "shad:levels\\test\\shaders\\floor";

/// The invented material map with its one pass in `layer`.
pub(super) fn material_map(layer: u8) -> Vec<u8> {
    fixture::fixture_with_shader_colour(layer)
}

fn open(t: &Temp, layer: u8) -> (Api, App) {
    let mut app = app(t);
    let mut api = Api::new();
    let map = t.path(&format!("shader-{layer}.map"));
    std::fs::write(&map, material_map(layer)).unwrap();
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );
    (api, app)
}

#[test]
fn shader_read_lists_the_slots_and_what_reads_them() {
    let t = Temp::new("shader-read");
    let (mut api, mut app) = open(&t, fixture::LAYERED_LAYER);
    let r = call(&mut api, &mut app, m::SHADER_READ, json!({"tag": SHADER}));
    assert_eq!(r["tag"], SHADER);
    assert_eq!(r["template"], "stem:levels\\test\\shaders\\layered");
    assert_eq!((r["lod"].clone(), r["lods"].clone()), (json!(0), json!(1)));
    assert_eq!(r["names_known"], false);
    assert!(r["names"].is_string());
    assert!(r["read_only"].is_null(), "{r}");
    assert_eq!(r["refusals"], json!([]));
    assert_eq!(r["passes"], json!({"5": 1}));
    let bitmaps = r["bitmaps"].as_array().unwrap();
    assert_eq!(bitmaps.len(), 1);
    assert_eq!(bitmaps[0]["bitmap"], "bitm:levels\\test\\bitmaps\\floor");
    assert_eq!(bitmaps[0]["used_by"], json!([]));
    let colours = r["colours"].as_array().unwrap();
    assert_eq!(colours.len(), 1);
    let c = &colours[0];
    assert_eq!(c["path"], "postprocess[0].colors[0].color");
    assert_eq!(c["hex"], format!("#{:08x}", fixture::SHADER_COLOUR));
    assert_eq!(
        c["used_by"],
        json!([{"layer": 5, "pass": 0, "implementation": 0, "reads": "final combiner c0"}])
    );
    assert_eq!(r["used_by"]["colour"][0], c["used_by"]);
    assert_eq!(r["vectors"], json!([]));
    // Another group is refused, by name.
    let e = api
        .call(&mut app, m::SHADER_READ, json!({"tag": "bitm:levels\\test\\bitmaps\\floor"}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{e}");
}

/// `shader.set` is the `tag.set` it stands for: the same answer, the
/// same edit and the same read after, each from a fresh app.
#[test]
fn shader_set_is_the_tag_set_at_the_slots_path() {
    let t = Temp::new("shader-set");
    let red = json!("#ffff0000");
    let (mut api, mut by_slot) = open(&t, fixture::LAYERED_LAYER);
    let s = call(
        &mut api,
        &mut by_slot,
        m::SHADER_SET,
        json!({"tag": SHADER, "slot": {"kind": "colour", "index": 0}, "value": red}),
    );
    assert_eq!(s["hex"], "#ffff0000");
    assert_eq!(s["kind"], "colour");
    let path = s["path"].as_str().unwrap().to_owned();

    let t2 = Temp::new("shader-set-tag");
    let (mut api2, mut by_path) = open(&t2, fixture::LAYERED_LAYER);
    let v = call(
        &mut api2,
        &mut by_path,
        m::TAG_SET,
        json!({"tag": SHADER, "path": path, "value": red}),
    );
    assert_eq!(s["value"], v["value"]);
    assert_eq!(by_slot.tags.patches(), by_path.tags.patches());
    assert!(!by_slot.tags.patches().is_empty());
    assert_eq!(
        call(&mut api, &mut by_slot, m::TAG_READ, json!({"tag": SHADER})),
        call(&mut api2, &mut by_path, m::TAG_READ, json!({"tag": SHADER})),
    );
    // One undo step: undone, the shader reads as loaded.
    call(&mut api, &mut by_slot, m::DOC_UNDO, json!({"doc": "tags"}));
    let r = call(&mut api, &mut by_slot, m::SHADER_READ, json!({"tag": SHADER}));
    assert_eq!(r["colours"][0]["hex"], format!("#{:08x}", fixture::SHADER_COLOUR));
}

#[test]
fn shader_set_writes_a_bitmap_and_refuses_what_the_shader_lacks() {
    let t = Temp::new("shader-set-bitmap");
    let (mut api, mut app) = open(&t, fixture::LAYERED_LAYER);
    let s = call(
        &mut api,
        &mut app,
        m::SHADER_SET,
        json!({"tag": SHADER, "slot": {"kind": "bitmap", "index": 0}, "value": null}),
    );
    assert!(s["bitmap"].is_null(), "{s}");
    // A slot past the shader's, a kind it does not know, a bad value.
    for (slot, value) in [
        (json!({"kind": "colour", "index": 1}), json!("#ff000000")),
        (json!({"kind": "vector", "index": 0}), json!([0, 0, 0, 0])),
        (json!({"kind": "scalar", "index": 0}), json!(1)),
        (json!({"kind": "colour", "index": 0}), json!("not a colour")),
    ] {
        let e = api
            .call(&mut app, m::SHADER_SET, json!({"tag": SHADER, "slot": slot, "value": value}))
            .unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{slot} {value}: {e}");
    }
}

/// S1's acceptance: a changed colour changes the preview's pixels, and a
/// refused pass (layer 1) is counted and absent.
#[test]
fn shader_preview_follows_an_edit_and_counts_a_refused_pass() {
    let t = Temp::new("shader-preview");
    let (mut api, mut app) = open(&t, fixture::LAYERED_LAYER);
    let out = |name: &str| t.path(&format!("out/{name}.png")).display().to_string();
    let before = call(
        &mut api,
        &mut app,
        m::SHADER_PREVIEW,
        json!({"tag": SHADER, "size": [64, 48], "out": out("before")}),
    );
    assert_eq!(before["shape"], "sphere");
    // Layer 5's pass, and the base look when the glue pass compiles.
    let drawn = before["passes_drawn"].as_u64().unwrap();
    assert!(drawn >= 1, "{before}");
    assert!(
        before["refusals"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["layer"] != 5),
        "{before}"
    );
    call(
        &mut api,
        &mut app,
        m::SHADER_SET,
        json!({"tag": SHADER, "slot": {"kind": "colour", "index": 0}, "value": "#ffff0000"}),
    );
    let after = call(
        &mut api,
        &mut app,
        m::SHADER_PREVIEW,
        json!({"tag": SHADER, "size": [64, 48], "out": out("after")}),
    );
    assert_eq!(after["passes_drawn"], drawn);
    if before["png"].is_string() {
        let a = std::fs::read(before["png"].as_str().unwrap()).unwrap();
        let b = std::fs::read(after["png"].as_str().unwrap()).unwrap();
        assert_ne!(a, b, "the colour edit changed no pixel");
        assert!(before["covered"].as_u64().unwrap() > 0);
    } else {
        assert!(after["notes"][0].as_str().unwrap().starts_with("no renderer here"));
    }

    // The pass in layer 1: refused, counted, and nothing of it drawn.
    let t1 = Temp::new("shader-preview-1");
    let (mut api1, mut app1) = open(&t1, 1);
    let one = call(
        &mut api1,
        &mut app1,
        m::SHADER_PREVIEW,
        json!({"tag": SHADER, "shape": "quad", "size": [64, 48], "out": t1.path("out/one.png").display().to_string()}),
    );
    assert_eq!(one["shape"], "quad");
    // The same material less its one pass.
    assert_eq!(one["passes_drawn"].as_u64().unwrap(), drawn - 1, "{one}");
    let refused: Vec<&Value> = one["refusals"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["layer"] == 1)
        .collect();
    assert_eq!(refused.len(), 1, "{one}");
    assert_eq!(refused[0]["passes"], 1);
    assert!(refused[0]["why"].as_str().unwrap().contains("layer 1"), "{one}");
}

#[test]
fn shader_preview_refuses_bad_shapes_and_sizes() {
    let t = Temp::new("shader-preview-bad");
    let (mut api, mut app) = open(&t, fixture::LAYERED_LAYER);
    for p in [
        json!({"tag": SHADER, "shape": "cube"}),
        json!({"tag": SHADER, "shape": "object"}),
        json!({"tag": SHADER, "size": [8, 8]}),
        json!({"tag": SHADER, "size": [64]}),
    ] {
        let e = api.call(&mut app, m::SHADER_PREVIEW, p.clone()).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{p}: {e}");
    }
}

/// The shader's colour is a colour row of `tag.colours` (shad.txt names
/// its view), so the C1 dialog edits it as the tab does.
#[test]
fn the_shaders_colour_is_a_colour_row() {
    let t = Temp::new("shader-colours");
    let (mut api, mut app) = open(&t, fixture::LAYERED_LAYER);
    let v = call(&mut api, &mut app, m::TAG_COLOURS, json!({"tag": SHADER}));
    let rows = v["colours"].as_array().unwrap();
    let row = rows
        .iter()
        .find(|r| r["path"] == "postprocess[0].colors[0].color")
        .unwrap_or_else(|| panic!("{v}"));
    assert_eq!(row["hex"], format!("#{:08x}", fixture::SHADER_COLOUR));
}

/// S4's acceptance: an invented pass lists both programs, a pass or
/// implementation the shader lacks is refused, and so is a tag that is
/// not a shader.
#[test]
fn shader_code_lists_both_programs_of_a_pass() {
    let t = Temp::new("shader-code");
    let (mut api, mut app) = open(&t, fixture::LAYERED_LAYER);
    let r = call(&mut api, &mut app, m::SHADER_CODE, json!({"tag": SHADER}));
    assert_eq!(r["tag"], SHADER);
    assert_eq!(
        (r["pass"].clone(), r["passes"].clone(), r["layer"].clone(), r["pass_in_layer"].clone()),
        (json!(0), json!(1), json!(fixture::LAYERED_LAYER), json!(0))
    );
    assert_eq!((r["implementation"].clone(), r["implementations"].clone()), (json!(0), json!(1)));
    assert!(r["pass_tag"].as_str().unwrap().starts_with("spas:"), "{r}");
    assert_eq!(r["vertex"]["classification"], "world");
    for k in ["vertex", "pixel"] {
        assert!(r[k]["listing"].as_str().is_some_and(|l| !l.is_empty()), "{k}: {r}");
        assert!(r[k]["wgsl"].as_str().is_some_and(|w| !w.is_empty()), "{k}: {r}");
        assert!(r[k]["warnings"].is_array(), "{k}: {r}");
    }
    assert_eq!(r["vertex"]["warnings"], json!([]));
    // The same pass named in full.
    let again = call(
        &mut api,
        &mut app,
        m::SHADER_CODE,
        json!({"tag": SHADER, "pass": 0, "implementation": 0}),
    );
    assert_eq!(r, again);
    for p in [json!({"pass": 1}), json!({"implementation": 1})] {
        let mut params = json!({"tag": SHADER});
        params.as_object_mut().unwrap().extend(p.as_object().unwrap().clone());
        let e = api.call(&mut app, m::SHADER_CODE, params).unwrap_err();
        assert_eq!(e.code, code::REFUSED, "{p}: {e}");
    }
    let e = api
        .call(&mut app, m::SHADER_CODE, json!({"tag": "bitm:levels\\test\\bitmaps\\floor"}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{e}");
}

/// S2: `shader.read` and `shader.preview` take the preview clock; the
/// invented shader has no animated property, so the clock changes no
/// value, and a time out of range is refused by name.
#[test]
fn the_preview_clock_is_read_and_checked() {
    let t = Temp::new("shader-clock");
    let (mut api, mut app) = open(&t, fixture::LAYERED_LAYER);
    let r = call(&mut api, &mut app, m::SHADER_READ, json!({"tag": SHADER, "time": 1.5}));
    assert_eq!(r["time"], 1.5);
    assert_eq!(r["clock_length"], 1.0);
    assert_eq!(r["animations"], json!([]));
    let p = call(
        &mut api,
        &mut app,
        m::SHADER_PREVIEW,
        json!({"tag": SHADER, "size": [32, 32], "time": 0.25, "out": t.path("out/clock.png").display().to_string()}),
    );
    assert_eq!(p["time"], 0.25);
    assert_eq!(p["animation"], json!([]));
    for bad in [json!(-1.0), json!(90000.0), json!("soon")] {
        for method in [m::SHADER_READ, m::SHADER_PREVIEW] {
            let e = api
                .call(&mut app, method, json!({"tag": SHADER, "time": bad}))
                .unwrap_err();
            assert_eq!(e.code, code::INVALID_PARAMS, "{method} {bad}: {e}");
        }
    }
}
