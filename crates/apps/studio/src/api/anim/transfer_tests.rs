//! `anim.retarget` and `anim.import` over the invented two-bone graph of
//! `tests.rs`, and a glTF file written here (invented nodes and keys).

use std::path::PathBuf;

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::tests::anim_map;
use crate::api::Api;
use crate::app::tag_tests;

const GRAPH: &str = r"jmad:invented\stickman";
const IDLE: &str = "combat:idle";

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-animxfer-{tag}-{}", std::process::id()));
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

fn open(t: &Temp) -> (crate::App, Api) {
    let (app, ..) = tag_tests::app_with_registry(&t.0, anim_map(true), tag_tests::registry());
    (app, Api::new())
}

fn call(api: &mut Api, app: &mut crate::App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

/// An invented glTF file in `dir`: two nodes and a quarter turn of the
/// second over three frames, `animations` times.
pub(crate) fn gltf(dir: &std::path::Path, node_names: [&str; 2], animations: usize) -> String {
    let half = std::f32::consts::FRAC_1_SQRT_2;
    let floats = [0.0f32, 0.1, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, half, half];
    let bytes: Vec<u8> = floats.iter().flat_map(|v| v.to_le_bytes()).collect();
    std::fs::write(dir.join("wave.bin"), &bytes).unwrap();
    let one = |name: &str| {
        format!(
            r#"{{"name":"{name}","samplers":[{{"input":0,"output":1}}],
            "channels":[{{"sampler":0,"target":{{"node":1,"path":"rotation"}}}}]}}"#
        )
    };
    let clips: Vec<String> = (0..animations).map(|i| one(&format!("wave{i}"))).collect();
    let json = format!(
        r#"{{"asset":{{"version":"2.0"}},
        "nodes":[{{"name":"{}","children":[1]}},{{"name":"{}","translation":[1,0,0]}}],
        "buffers":[{{"uri":"wave.bin","byteLength":40}}],
        "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":8}},{{"buffer":0,"byteOffset":8,"byteLength":32}}],
        "accessors":[{{"bufferView":0,"componentType":5126,"count":2,"type":"SCALAR"}},
                     {{"bufferView":1,"componentType":5126,"count":2,"type":"VEC4"}}],
        "animations":[{}]}}"#,
        node_names[0],
        node_names[1],
        clips.join(",")
    );
    let path = dir.join("wave.gltf");
    std::fs::write(&path, json).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn the_methods_are_listed() {
    let t = Temp::new("list");
    let (mut app, mut api) = open(&t);
    let list = call(&mut api, &mut app, m::METHODS, json!({}));
    for name in [m::ANIM_RETARGET, m::ANIM_IMPORT] {
        let row = list.as_array().unwrap().iter().find(|r| r["name"] == name);
        assert_eq!(row.unwrap_or_else(|| panic!("{name} missing"))["status"], "ready");
    }
}

#[test]
fn a_clip_retargets_onto_a_graph_by_name_and_the_table_lists_every_node() {
    let t = Temp::new("retarget");
    let (mut app, mut api) = open(&t);
    let from = call(
        &mut api,
        &mut app,
        m::ANIM_DUPLICATE,
        json!({"tag": GRAPH, "animation": IDLE, "name": "idle copy"}),
    )["doc"]
        .as_u64()
        .unwrap();
    let preview = call(
        &mut api,
        &mut app,
        m::ANIM_RETARGET,
        json!({"from": from, "to": GRAPH, "preview": true}),
    );
    assert!(preview["doc"].is_null());
    let out = call(
        &mut api,
        &mut app,
        m::ANIM_RETARGET,
        json!({"from": from, "to": GRAPH, "name": "idle again"}),
    );
    let doc = out["doc"].as_u64().unwrap();
    assert_ne!(doc, from);
    assert_eq!(out["unmatched_source"], json!([]));
    assert_eq!(out["unmatched_target"], json!([]));
    assert_eq!(out["nodes"][1]["source_name"], "arm");
    assert_eq!(out["nodes"][1]["target"], 1);
    let old = call(&mut api, &mut app, m::ANIM_READ, json!({"doc": from}));
    let new = call(&mut api, &mut app, m::ANIM_READ, json!({"doc": doc}));
    assert_eq!(old["nodes"], new["nodes"]);
    assert_eq!(new["name"], "idle again");
}

#[test]
fn a_dropped_node_and_a_bad_mapping_are_reported() {
    let t = Temp::new("map");
    let (mut app, mut api) = open(&t);
    let from = call(
        &mut api,
        &mut app,
        m::ANIM_DUPLICATE,
        json!({"tag": GRAPH, "animation": IDLE, "name": "idle copy"}),
    )["doc"]
        .as_u64()
        .unwrap();
    let out = call(
        &mut api,
        &mut app,
        m::ANIM_RETARGET,
        json!({"from": from, "to": GRAPH, "map": [{"source": 1, "target": null}], "preview": true}),
    );
    assert_eq!(out["unmatched_source"], json!([1]));
    assert_eq!(out["unmatched_target"], json!([1]));
    assert_eq!(out["dropped_channels"][0]["node"], 1);
    for map in [json!([{"source": 1}]), json!({"source": 1})] {
        let e = api
            .call(&mut app, m::ANIM_RETARGET, json!({"from": from, "to": GRAPH, "map": map}))
            .unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
    }
    let e = api
        .call(
            &mut app,
            m::ANIM_RETARGET,
            json!({"from": from, "to": GRAPH, "map": [{"source": 7, "target": 0}]}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
}

#[test]
fn a_gltf_clip_imports_onto_the_graph_and_lists_its_unmatched_nodes() {
    let t = Temp::new("import");
    let (mut app, mut api) = open(&t);
    let file = gltf(&t.0, ["body", "arm"], 1);
    let out = call(&mut api, &mut app, m::ANIM_IMPORT, json!({"file": file, "to": GRAPH}));
    assert_eq!(out["name"], "wave0");
    assert_eq!(out["unmatched_source"], json!([]));
    let doc = out["doc"].as_u64().unwrap();
    let read = call(&mut api, &mut app, m::ANIM_READ, json!({"doc": doc}));
    assert_eq!(read["frames"], 4);

    let file = gltf(&t.0, ["pelvis", "arm"], 1);
    let out = call(
        &mut api,
        &mut app,
        m::ANIM_IMPORT,
        json!({"file": file, "to": GRAPH, "preview": true}),
    );
    assert_eq!(out["unmatched_source"], json!([0]));
    assert_eq!(out["unmatched_target"], json!([0]));
}

#[test]
fn several_clips_need_a_choice_and_a_missing_file_is_refused() {
    let t = Temp::new("choose");
    let (mut app, mut api) = open(&t);
    let file = gltf(&t.0, ["body", "arm"], 2);
    let e = api
        .call(&mut app, m::ANIM_IMPORT, json!({"file": file, "to": GRAPH}))
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert_eq!(e.data.unwrap()["clips"][1]["name"], "wave1");
    let out = call(
        &mut api,
        &mut app,
        m::ANIM_IMPORT,
        json!({"file": file, "to": GRAPH, "clip": "wave1"}),
    );
    assert_eq!(out["name"], "wave1");
    let e = api
        .call(&mut app, m::ANIM_IMPORT, json!({"file": "/invented/none.gltf", "to": GRAPH}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
}
