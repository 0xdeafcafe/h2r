//! `anim.read` over an invented two-bone graph (slice AN1 of
//! docs/editor/studio-redesign.md): a base clip of six frames whose arm
//! turns 0, 45, 90, 90, 45, 0 degrees about z while it slides along x,
//! and an overlay holding a quarter about x. The map is the Pogo kit's
//! invented one (`pogo_kit::fixture`, no game data) with the graph added,
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
    let mut map = Map::from_reader(Cursor::new(pogo_kit::fixture::fixture(0))).unwrap();
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
