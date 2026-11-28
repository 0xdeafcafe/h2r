//! `anim.read` over an invented two-bone graph (slice AN1 of
//! docs/editor/studio-redesign.md): a base clip of six frames whose arm
//! turns 0, 45, 90, 90, 45, 0 degrees about z while it slides along x,
//! and an overlay holding a quarter about x. The map is the Pogo kit's
//! invented one (`content_kit::fixture`, no game data) with the graph added,
//! written with the Halo 1 converter's own writers (`h1-assets`), so what
//! the decoders read is what a converted map holds.

use std::io::Cursor;
use std::path::PathBuf;

use h1_assets::models::h1;
use map::Map;
use map::build::TagSet;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use crate::api::{Api, Reply};
use crate::app::tag_tests;
use crate::app::{Action, Doc};

const GRAPH: &str = r"jmad:invented\stickman";
const IDLE: &str = "combat:idle";
const ARM_OVER: &str = "combat:arm_over";
const SQRT_HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// The arm's turn at each frame of the base, degrees about z.
const ANGLES: [f32; 6] = [0.0, 45.0, 90.0, 90.0, 45.0, 0.0];
/// The default tolerance's rotation, degrees.
const DEFAULT_DEGREES: f64 = 0.1;

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-animread-{tag}-{}", std::process::id()));
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

fn identity() -> h1::Transform {
    h1::Transform {
        rotation: [0.0, 0.0, 0.0, 1.0],
        translation: [0.0; 3],
        scale: 1.0,
    }
}

/// The arm at base frame `frame`: a turn of `ANGLES[frame]` about z and a
/// slide of a tenth of a unit a frame from x = 1.
fn arm(frame: usize) -> h1::Transform {
    let half = ANGLES[frame].to_radians() * 0.5;
    h1::Transform {
        rotation: [0.0, 0.0, half.sin(), half.cos()],
        translation: [1.0 + 0.1 * frame as f32, 0.0, 0.0],
        scale: 1.0,
    }
}

/// The overlay's arm: a constant quarter about x.
fn overlay_arm() -> h1::Transform {
    h1::Transform {
        rotation: [SQRT_HALF, 0.0, 0.0, SQRT_HALF],
        translation: [0.0; 3],
        scale: 1.0,
    }
}

fn node(name: &str, next: i16, child: i16, parent: i16) -> h1::AnimNode {
    h1::AnimNode {
        name: name.into(),
        next_sibling: next,
        first_child: child,
        parent,
        joint_flags: 0,
        base_vector: [0.0; 3],
        vector_range: 0.0,
    }
}

fn animation(
    name: &str,
    kind: h1::AnimType,
    animated: [u64; 3],
    arm: Vec<h1::Transform>,
) -> h1::Animation {
    h1::Animation {
        name: name.into(),
        kind,
        frame_count: arm.len() as u16,
        frame_info_type: 0,
        frame_info: vec![],
        node_list_checksum: 1,
        node_count: 2,
        loop_frame: 0,
        key_frames: [0, 0],
        foot_frames: [0, 0],
        next_permutation: -1,
        first_permutation: -1,
        cumulative_weight: 0.0,
        flags: 0,
        animated,
        frames: arm.into_iter().map(|t| vec![identity(), t]).collect(),
        zero_rotations: 0,
        compressed: None,
    }
}

/// The invented map: the Pogo kit's, with the two-bone graph in it. Nothing
/// uses the graph: the read needs no model.
pub(crate) fn read_map() -> Vec<u8> {
    let mut map = Map::from_reader(Cursor::new(content_kit::fixture::fixture(0))).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    h1_assets::sounds::convert::make_index_room(&mut set);
    let graph = h1::Graph {
        nodes: vec![node("body", 1, 1, -1), node("arm", -1, -1, 0)],
        animations: vec![
            // The arm's rotation and translation move; everything else is
            // a static default of the base.
            animation(
                "stand idle",
                h1::AnimType::Base,
                [1 << 1, 1 << 1, 0],
                (0..6).map(arm).collect(),
            ),
            animation(
                "stand arm-over",
                h1::AnimType::Overlay,
                [1 << 1, 0, 0],
                (0..3).map(|_| overlay_arm()).collect(),
            ),
        ],
        unit_count: 0,
        weapon_count: 0,
        vehicle_count: 0,
        units: vec![],
        damage_slots: vec![],
        first_person_slots: vec![],
        vehicle: None,
        objects: vec![],
    };
    h1_assets::models::jmad::convert_graph(&graph, None)
        .unwrap()
        .0
        .install(&mut set, r"invented\stickman")
        .unwrap();
    set.build().unwrap().0
}

fn open(t: &Temp) -> (crate::App, Api) {
    let (app, ..) = tag_tests::app_with_registry(&t.0, read_map(), tag_tests::registry());
    (app, Api::new())
}

fn call(api: &mut Api, app: &mut crate::App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

fn refused(r: Reply) -> agent_proto::RpcError {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e,
    }
}

fn floats(v: &Value) -> Vec<f32> {
    v.as_array()
        .expect("an array")
        .iter()
        .map(|x| x.as_f64().expect("a number") as f32)
        .collect()
}

fn close(a: &Value, b: &[f32], eps: f32) -> bool {
    let a = floats(a);
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < eps)
}

/// The channel of `kind` on node `node` of a read.
fn channel<'a>(r: &'a Value, node: usize, kind: &str) -> &'a Value {
    r["nodes"][node]["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == kind)
        .unwrap_or_else(|| panic!("node {node} has no {kind} channel: {}", r["nodes"][node]))
}

fn key_frames(c: &Value) -> Vec<u64> {
    c["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["frame"].as_u64().unwrap())
        .collect()
}

fn read(api: &mut Api, app: &mut crate::App, params: Value) -> Value {
    call(api, app, m::ANIM_READ, params)
}

#[test]
fn the_method_is_listed_ready() {
    let t = Temp::new("listed");
    let (mut app, mut api) = open(&t);
    let list = call(&mut api, &mut app, m::METHODS, json!({}));
    let row = list
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == m::ANIM_READ)
        .expect("anim.read is listed");
    assert_eq!(row["status"], "ready");
}

/// The accepted test of the slice: an invented two-bone clip lists its
/// channels.
#[test]
fn a_two_bone_clip_lists_its_channels_as_keys() {
    let t = Temp::new("channels");
    let (mut app, mut api) = open(&t);
    let r = read(&mut api, &mut app, json!({"tag": GRAPH, "animation": IDLE}));
    assert_eq!(r["animation"]["name"], IDLE);
    assert_eq!(r["animation"]["type"], "base");
    assert_eq!(r["frames"], 6);
    assert_eq!(r["frame_rate"], 30.0);
    assert!((r["duration"].as_f64().unwrap() - 0.2).abs() < 1e-6);
    let nodes = r["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["name"], "body");
    assert_eq!(nodes[0]["parent"], -1);
    assert_eq!(nodes[1]["name"], "arm");
    assert_eq!(nodes[1]["parent"], 0);

    // Every node has its three channels: the arm's rotation and
    // translation from the animated stream, the rest from the static one.
    for n in nodes {
        let kinds: Vec<&str> = n["channels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["rotation", "translation", "scale"], "{n}");
    }
    // The body holds still: one key per channel, at the identity.
    for kind in ["rotation", "translation", "scale"] {
        let c = channel(&r, 0, kind);
        assert_eq!(c["moves"], false, "{c}");
        assert_eq!(key_frames(c), [0]);
    }
    assert!(close(
        &channel(&r, 0, "rotation")["keys"][0]["value"],
        &[0.0, 0.0, 0.0, 1.0],
        1e-4
    ));
    assert!(close(
        &channel(&r, 0, "scale")["keys"][0]["value"],
        &[1.0],
        1e-6
    ));
    // The arm's turn: frames 1 and 4 lie on the line between their
    // neighbours, so four keys carry six frames.
    let rotation = channel(&r, 1, "rotation");
    assert_eq!(rotation["moves"], true);
    assert_eq!(rotation["key_count"], 4);
    assert_eq!(key_frames(rotation), [0, 2, 3, 5]);
    assert!(close(
        &rotation["keys"][1]["value"],
        &[0.0, 0.0, SQRT_HALF, SQRT_HALF],
        1e-3
    ));
    // The slide is one line: its ends.
    let slide = channel(&r, 1, "translation");
    assert_eq!(key_frames(slide), [0, 5]);
    assert!(close(&slide["keys"][0]["value"], &[1.0, 0.0, 0.0], 1e-5));
    assert!(close(&slide["keys"][1]["value"], &[1.5, 0.0, 0.0], 1e-5));
    // The arm's scale is a static default.
    assert_eq!(key_frames(channel(&r, 1, "scale")), [0]);

    let reduction = &r["reduction"];
    assert_eq!(reduction["channels"], 6);
    assert_eq!(reduction["dense_values"], 36);
    assert_eq!(reduction["keys"], 4 + 2 + 4);
    assert!(reduction["max_error"]["rotation_degrees"].as_f64().unwrap() <= DEFAULT_DEGREES);
    assert!(reduction["max_error"]["translation"].as_f64().unwrap() <= 0.001);
    assert!((r["tolerance"]["rotation_degrees"].as_f64().unwrap() - DEFAULT_DEGREES).abs() < 1e-6);
    assert_eq!(
        r["events"],
        json!({"frame": 0, "sound": 0, "effect": 0, "decoded": false})
    );
}

/// The other half of the acceptance: reduce then bake is within the
/// tolerance at every frame. The keys the API lists are played back with
/// the rule it states (linear, a rotation along the shorter arc) and set
/// against `anim.pose` at every frame, which samples the clip itself.
#[test]
fn reduce_then_bake_is_within_the_tolerance_at_every_frame() {
    let t = Temp::new("bake");
    let (mut app, mut api) = open(&t);
    for tolerance in [
        None,
        Some(json!({"rotation_degrees": 0.5, "translation": 0.01})),
    ] {
        let mut params = json!({"tag": GRAPH, "animation": IDLE});
        let (degrees, distance) = match &tolerance {
            Some(t) => {
                params["tolerance"] = t.clone();
                (0.5, 0.01)
            }
            None => (DEFAULT_DEGREES, 0.001),
        };
        let r = read(&mut api, &mut app, params);
        let rotation = channel(&r, 1, "rotation");
        let slide = channel(&r, 1, "translation");
        for frame in 0..6 {
            let pose = call(
                &mut api,
                &mut app,
                m::ANIM_POSE,
                json!({"tag": GRAPH, "animation": IDLE, "frame": frame}),
            );
            let dense = &pose["nodes"][1];
            let baked = bake(rotation, frame, true);
            let a = floats(&dense["rotation"]);
            let sign = if a.iter().zip(&baked).map(|(x, y)| x * y).sum::<f32>() < 0.0 {
                -1.0
            } else {
                1.0
            };
            let chord = (0..4)
                .map(|k| (a[k] - sign * baked[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            let angle = (4.0 * (chord * 0.5).asin()).to_degrees();
            // The decoded clip itself is quantised: a hair of slack for it.
            assert!(
                f64::from(angle) <= degrees + 0.01,
                "frame {frame}: {angle} degrees off"
            );
            let want = floats(&dense["translation"]);
            let got = bake(slide, frame, false);
            let off = (0..3)
                .map(|k| (want[k] - got[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            assert!(f64::from(off) <= distance, "frame {frame}: {off} off");
        }
    }
}

/// The keys of `channel` played back at `frame`: held outside the keys,
/// linear between, a rotation re-normalised on the shorter arc.
fn bake(channel: &Value, frame: u64, rotation: bool) -> Vec<f32> {
    let keys = channel["keys"].as_array().unwrap();
    let at = |k: &Value| (k["frame"].as_u64().unwrap(), floats(&k["value"]));
    let (first, last) = (at(&keys[0]), at(keys.last().unwrap()));
    if frame <= first.0 {
        return first.1;
    }
    if frame >= last.0 {
        return last.1;
    }
    for pair in keys.windows(2) {
        let (a, b) = (at(&pair[0]), at(&pair[1]));
        if (a.0..=b.0).contains(&frame) {
            let t = (frame - a.0) as f32 / (b.0 - a.0) as f32;
            let flip = if rotation && a.1.iter().zip(&b.1).map(|(x, y)| x * y).sum::<f32>() < 0.0 {
                -1.0
            } else {
                1.0
            };
            let mut v: Vec<f32> =
                a.1.iter()
                    .zip(&b.1)
                    .map(|(x, y)| x + (flip * y - x) * t)
                    .collect();
            if rotation {
                let l = v.iter().map(|x| x * x).sum::<f32>().sqrt();
                v.iter_mut().for_each(|x| *x /= l);
            }
            return v;
        }
    }
    unreachable!("a frame between the keys is in a pair")
}

#[test]
fn the_tolerance_trades_keys_for_error_and_the_error_reported_is_measured() {
    let t = Temp::new("tolerance");
    let (mut app, mut api) = open(&t);
    // A hundred degrees swallows the whole turn: the channel is one key,
    // and the error reported is the turn the key leaves at its peak.
    let r = read(
        &mut api,
        &mut app,
        json!({"tag": GRAPH, "animation": IDLE, "tolerance": {"rotation_degrees": 100.0}}),
    );
    let rotation = channel(&r, 1, "rotation");
    assert_eq!(rotation["key_count"], 1);
    assert_eq!(rotation["moves"], false);
    let worst = r["reduction"]["max_error"]["rotation_degrees"]
        .as_f64()
        .unwrap();
    assert!((89.0..=91.0).contains(&worst), "{worst}");
    assert_eq!(
        r["tolerance"]["rotation_degrees"].as_f64().unwrap().round(),
        100.0
    );
    // Ten degrees keeps the shape: the peak and the ends.
    let r = read(
        &mut api,
        &mut app,
        json!({"tag": GRAPH, "animation": IDLE, "tolerance": {"rotation_degrees": 10.0}}),
    );
    assert_eq!(key_frames(channel(&r, 1, "rotation")), [0, 2, 3, 5]);
}

#[test]
fn keys_false_lists_counts_only() {
    let t = Temp::new("counts");
    let (mut app, mut api) = open(&t);
    let r = read(
        &mut api,
        &mut app,
        json!({"tag": GRAPH, "animation": 0, "keys": false}),
    );
    let c = channel(&r, 1, "rotation");
    assert_eq!(c["key_count"], 4);
    assert_eq!(c.get("keys"), None);
    assert_eq!(r["reduction"]["keys"], 10);
}

#[test]
fn an_overlay_lists_only_the_channels_it_drives() {
    let t = Temp::new("overlay");
    let (mut app, mut api) = open(&t);
    let r = read(
        &mut api,
        &mut app,
        json!({"tag": GRAPH, "animation": ARM_OVER}),
    );
    assert_eq!(r["animation"]["type"], "overlay");
    assert_eq!(r["frames"], 3);
    assert_eq!(r["nodes"][0]["channels"], json!([]));
    let channels = r["nodes"][1]["channels"].as_array().unwrap();
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0]["kind"], "rotation");
    assert_eq!(channels[0]["moves"], false);
    assert!(close(
        &channels[0]["keys"][0]["value"],
        &[SQRT_HALF, 0.0, 0.0, SQRT_HALF],
        1e-3
    ));
}

#[test]
fn refusals_are_named() {
    let t = Temp::new("refusals");
    let (mut app, mut api) = open(&t);
    let e = refused(api.call(
        &mut app,
        m::ANIM_READ,
        json!({"tag": r"scnr:invented\court", "animation": 0}),
    ));
    assert_eq!(e.code, code::REFUSED);
    assert!(
        e.message.contains("not an animation graph"),
        "{}",
        e.message
    );
    // An animation that is not there lists the ones that are.
    let e = refused(api.call(
        &mut app,
        m::ANIM_READ,
        json!({"tag": GRAPH, "animation": "nope"}),
    ));
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(e.message.contains(IDLE), "{}", e.message);
    let e = refused(api.call(&mut app, m::ANIM_READ, json!({"tag": GRAPH})));
    assert_eq!(e.code, code::INVALID_PARAMS);
    // A tolerance that is negative, or names nothing, is refused.
    for bad in [
        json!({"rotation_degrees": -1.0}),
        json!({"translation": "far"}),
        json!({"angle": 1.0}),
    ] {
        let e = refused(api.call(
            &mut app,
            m::ANIM_READ,
            json!({"tag": GRAPH, "animation": IDLE, "tolerance": bad}),
        ));
        assert_eq!(e.code, code::INVALID_PARAMS, "{bad}");
        assert!(e.message.contains("tolerance"), "{}", e.message);
    }
}

fn create_doc(api: &mut Api, app: &mut crate::App, name: &str) -> u64 {
    let result = call(
        api,
        app,
        m::ANIM_NEW,
        json!({"tag": GRAPH, "name": name, "frames": 6}),
    );
    assert_eq!(result["name"], name);
    assert_eq!(result["frames"], 6);
    assert_eq!(result["nodes"], 2);
    assert_eq!(result["revision"], 0);
    result["doc"].as_u64().unwrap()
}

#[test]
fn document_edits_drive_the_pose_and_a_duplicate_has_independent_keys() {
    let t = Temp::new("document");
    let (mut app, mut api) = open(&t);
    let doc = create_doc(&mut api, &mut app, "wave");
    let blank = read(&mut api, &mut app, json!({"doc": doc}));
    assert_eq!(blank["name"], "wave");
    assert_eq!(blank["nodes"][1]["name"], "arm");
    assert_eq!(blank["nodes"][1]["channels"], json!([]));

    let mut key = json!({"doc": doc, "node": "arm", "channel": "translation", "frame": 0});
    key["value"] = json!([1, 0, 0]);
    assert_eq!(call(&mut api, &mut app, m::ANIM_KEY, key.clone())["revision"], 1);
    key["frame"] = json!(5);
    key["value"] = json!([3, 0, 0]);
    call(&mut api, &mut app, m::ANIM_KEY, key.clone());
    key["frame"] = json!(0);
    key.as_object_mut().unwrap().remove("value");
    key["interp"] = json!("step");
    let step = call(&mut api, &mut app, m::ANIM_KEY, key.clone());
    assert_eq!(step["changed"], true);
    assert_eq!(step["key"]["interp"], "step");
    assert!(close(&step["key"]["value"], &[1.0, 0.0, 0.0], 1e-6));
    assert!(close(&step["key"]["tangent"], &[0.0; 3], 1e-6));
    let unchanged = call(&mut api, &mut app, m::ANIM_KEY, key);
    assert_eq!(unchanged["changed"], false);
    assert_eq!(unchanged["revision"], step["revision"]);

    for (frame, x) in [(3, 1.0), (5, 3.0)] {
        let pose = call(&mut api, &mut app, m::ANIM_POSE, json!({"doc": doc, "frame": frame}));
        assert_eq!(pose["animation"]["name"], "wave");
        assert_eq!(pose["nodes"][1]["has"]["translation"], true);
        assert!(close(&pose["nodes"][1]["translation"], &[x, 0.0, 0.0], 1e-6));
    }

    let duplicate = call(
        &mut api, &mut app, m::ANIM_DUPLICATE, json!({"doc": doc, "name": "wave copy"}),
    );
    let copy = duplicate["doc"].as_u64().unwrap();
    assert_ne!(copy, doc);
    assert_eq!(duplicate["revision"], 0);
    let before = read(&mut api, &mut app, json!({"doc": doc}));
    assert_eq!(before["nodes"], read(&mut api, &mut app, json!({"doc": copy}))["nodes"]);
    let removal = call(
        &mut api,
        &mut app,
        m::ANIM_KEY,
        json!({"doc": copy, "node": 1, "channel": "translation", "frame": 5, "remove": true}),
    );
    assert_eq!(removal["changed"], true);
    assert_eq!(removal["key"], Value::Null);
    let after = read(&mut api, &mut app, json!({"doc": copy, "keys": false}));
    let channel = channel(&after, 1, "translation");
    assert_eq!(channel["moves"], false);
    assert_eq!(channel["key_count"], 1);
    assert!(channel.get("keys").is_none());
    assert_eq!(before, read(&mut api, &mut app, json!({"doc": doc})));
}

/// A graph's tab is one document (docs/editor/studio-flow.md, "Shared
/// patterns"): the steps of its animation documents undo in the order they
/// were made, across the documents, with Cmd+Z and the window's own Undo,
/// and redo from the oldest.
#[test]
fn a_graphs_tab_undoes_its_documents_steps_in_the_order_made() {
    let t = Temp::new("document-undo");
    let (mut app, mut api) = open(&t);
    app.dispatch(Action::OpenAsset {
        asset: format!("tag:{GRAPH}"),
        map: None,
    });
    let Some(tab @ Doc::Tag(graph)) = app.front_doc() else {
        panic!("the graph has no tab: {:?}", app.tabs());
    };
    assert_eq!(app.doc_can(tab), (false, false));
    let (first, second) = (
        create_doc(&mut api, &mut app, "wave"),
        create_doc(&mut api, &mut app, "wave 2"),
    );
    assert_ne!(first, second);
    assert_eq!(app.anim_docs(graph).len(), 2);
    let key = |api: &mut Api, app: &mut crate::App, doc: u64, x: i64| {
        call(
            api,
            app,
            m::ANIM_KEY,
            json!({"doc": doc, "node": "arm", "channel": "translation", "frame": 0, "value": [x, 0, 0]}),
        );
    };
    let keys = |app: &crate::App| {
        (
            app.anim_doc(first).unwrap().can_undo(),
            app.anim_doc(second).unwrap().can_undo(),
        )
    };
    key(&mut api, &mut app, first, 1);
    key(&mut api, &mut app, second, 2);
    assert_eq!(keys(&app), (true, true));
    assert_eq!(app.doc_can(tab), (true, false));
    assert_eq!((app.can_undo(), app.can_redo()), (true, false));

    // The newest step first, whichever document made it.
    app.dispatch(Action::Undo);
    assert_eq!(keys(&app), (true, false));
    assert_eq!(app.doc_can(tab), (true, true));
    app.dispatch(Action::UndoDoc(tab));
    assert_eq!(keys(&app), (false, false));
    assert_eq!(app.doc_can(tab), (false, true));

    // Redo takes the oldest: the first document's key, then the second's.
    app.dispatch(Action::RedoDoc(tab));
    assert_eq!(keys(&app), (true, false));
    app.dispatch(Action::Redo);
    assert_eq!(keys(&app), (true, true));
    assert_eq!(app.doc_can(tab), (true, false));

    // A step made after two undone ones leaves the older of them wrong.
    app.dispatch(Action::UndoDoc(tab));
    app.dispatch(Action::UndoDoc(tab));
    assert_eq!(app.doc_can(tab), (false, true));
    key(&mut api, &mut app, second, 3);
    assert_eq!(
        app.doc_can(tab),
        (true, false),
        "nothing older than the new step is redone"
    );
}

#[test]
fn a_retail_read_returns_the_canonical_doc_and_can_start_a_fresh_copy() {
    let t = Temp::new("editable-retail");
    let (mut app, mut api) = open(&t);
    let retail = read(&mut api, &mut app, json!({"tag": GRAPH, "animation": IDLE}));
    let doc = retail["doc"].as_u64().expect("an editable document");
    let snapshot = read(&mut api, &mut app, json!({"doc": doc}));
    let overlaid = call(
        &mut api,
        &mut app,
        m::ANIM_POSE,
        json!({"doc": doc, "overlay": {"animation": ARM_OVER, "weight": 1.0}}),
    );
    assert!(close(
        &overlaid["nodes"][1]["rotation"],
        &[SQRT_HALF, 0.0, 0.0, SQRT_HALF],
        1e-3,
    ));

    call(
        &mut api,
        &mut app,
        m::ANIM_KEY,
        json!({"doc": doc, "node": 1, "channel": "rotation", "frame": 0, "value": [1, 0, 0, 0]}),
    );
    assert_eq!(retail, read(&mut api, &mut app, json!({"tag": GRAPH, "animation": IDLE})));
    let loose = read(
        &mut api,
        &mut app,
        json!({"tag": GRAPH, "animation": 0, "tolerance": {"rotation_degrees": 100}}),
    );
    assert_eq!(loose["doc"], doc);
    assert_eq!(channel(&loose, 1, "rotation")["key_count"], 1);

    let overlay = read(&mut api, &mut app, json!({"tag": GRAPH, "animation": ARM_OVER}));
    let with_base = call(
        &mut api,
        &mut app,
        m::ANIM_POSE,
        json!({"doc": overlay["doc"], "base": IDLE}),
    );
    let as_overlay = call(
        &mut api,
        &mut app,
        m::ANIM_POSE,
        json!({"doc": doc, "overlay": {"animation": ARM_OVER, "weight": 1.0}}),
    );
    assert_eq!(with_base["nodes"], as_overlay["nodes"]);

    let edited = read(&mut api, &mut app, json!({"doc": doc}));
    assert!(close(
        &channel(&edited, 1, "rotation")["keys"][0]["value"],
        &[1.0, 0.0, 0.0, 0.0],
        1e-6,
    ));
    let copy = call(
        &mut api,
        &mut app,
        m::ANIM_DUPLICATE,
        json!({"tag": GRAPH, "animation": IDLE, "name": "decoded copy"}),
    );
    assert_ne!(copy["doc"], doc);
    assert_eq!(
        read(&mut api, &mut app, json!({"doc": copy["doc"]}))["nodes"],
        snapshot["nodes"]
    );
}

#[test]
fn edit_validation_preserves_the_document_and_check_reports_the_seam() {
    let t = Temp::new("check");
    let (mut app, mut api) = open(&t);
    let doc = create_doc(&mut api, &mut app, "lurch");
    let blank = read(&mut api, &mut app, json!({"doc": doc}));
    for (node, channel, frame, value, remove) in [
        (json!("unknown"), "rotation", 0, json!([0, 0, 0, 1]), false),
        (json!(1), "rotation", 6, json!([0, 0, 0, 1]), false),
        (json!(1), "rotation", 0, json!([0, 0, 0, 0]), false),
        (json!(1), "translation", 0, json!([1e100, 0, 0]), false),
        (json!(1), "scale", 0, json!([1, 2]), false),
        (json!(1), "scale", 0, json!([1]), true),
    ] {
        let error = refused(api.call(
            &mut app,
            m::ANIM_KEY,
            json!({"doc": doc, "node": node, "channel": channel, "frame": frame, "value": value, "remove": remove}),
        ));
        assert_eq!(error.code, code::INVALID_PARAMS, "{error}");
    }
    assert_eq!(blank, read(&mut api, &mut app, json!({"doc": doc})));
    for (frame, x) in [(0, 0), (5, 2)] {
        call(
            &mut api,
            &mut app,
            m::ANIM_KEY,
            json!({"doc": doc, "node": 1, "channel": "translation", "frame": frame, "value": [x, 0, 0]}),
        );
    }

    let report = call(&mut api, &mut app, m::ANIM_CHECK, json!({"doc": doc}));
    assert_eq!(report["doc"], doc);
    let error = report["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node"] == 1)
        .unwrap();
    assert_eq!(error["translation"].as_f64(), Some(0.0));
    let seam = report["loop_seam"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["node"] == 1)
        .unwrap();
    assert_eq!(seam["translation"].as_f64(), Some(2.0));
    assert_eq!(report["feet_checked"], false);
    assert_eq!(report["feet_below_ground"], json!([]));
}

#[test]
fn a_gizmo_drag_and_a_socket_key_write_the_same_document() {
    use tags::anim_keys::Kind;

    let t = Temp::new("gizmo-key");
    let (mut app, mut api) = open(&t);
    let socket = create_doc(&mut api, &mut app, "socket");
    let gizmo = create_doc(&mut api, &mut app, "gizmo");
    call(
        &mut api,
        &mut app,
        m::ANIM_KEY,
        json!({"doc": socket, "node": 1, "channel": "translation", "frame": 2, "value": [2, 3, 4]}),
    );
    {
        let doc = app.anim_doc_mut(gizmo).unwrap();
        let gesture = doc.begin_gesture();
        for x in [1.0, 2.0] {
            let local = [
                1.0, 0.0, 0.0, 0.0,
                0.0, 1.0, 0.0, 0.0,
                0.0, 0.0, 1.0, 0.0,
                x, 3.0, 4.0, 1.0,
            ];
            assert!(crate::ui::anim::key_gizmo(
                doc,
                1,
                Kind::Translation,
                2,
                local,
                None,
                Some(gesture),
            )
            .unwrap());
        }
    }
    let socket_keys = read(&mut api, &mut app, json!({"doc": socket}))["nodes"].clone();
    assert_eq!(socket_keys, read(&mut api, &mut app, json!({"doc": gizmo}))["nodes"]);

    let doc = app.anim_doc_mut(gizmo).unwrap();
    assert!(doc.undo());
    assert!(!doc.can_undo());
    assert!(doc.key(1, Kind::Translation, 2).is_none());
    assert!(doc.redo());
    assert_eq!(socket_keys, read(&mut api, &mut app, json!({"doc": gizmo}))["nodes"]);
}
