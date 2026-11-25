//! `anim.*` over an invented map: a two-bone model (a body box on node 0
//! and an arm box on node 1, rigid, untextured), a `jmad` whose base
//! animation turns the arm a quarter about z over three frames and whose
//! overlay turns it a quarter about x, and a `bipd` whose `hlmt` names
//! both. Every byte is invented, built with the Halo 1 converter's own
//! writers (`h1-assets`): what the decoders read is what a converted map
//! holds.

use std::path::PathBuf;

use h1_assets::models::h1;
use map::build::{TagPayload, TagSet};
use map::{FourCC, TagId};
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use tags::defs::{self, Defs, Val};

use crate::api::{Api, Reply};
use crate::app::tag_tests;

const GRAPH: &str = r"jmad:invented\stickman";
const IDLE: &str = "combat:idle";
const ARM_OVER: &str = "combat:arm_over";
const SQRT_HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-anim-{tag}-{}", std::process::id()));
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

/// The arm's local transform at `frame` of the base: a turn of `frame`
/// quarters (0, 45, 90 degrees) about z, the node's own translation.
fn turn_of(frame: usize) -> h1::Transform {
    let a = frame as f32 * 45.0_f32.to_radians() * 0.5;
    h1::Transform {
        rotation: [0.0, 0.0, a.sin(), a.cos()],
        translation: [1.0, 0.0, 0.0],
        scale: 1.0,
    }
}

/// The arm's local transform in the overlay: a constant quarter about x
/// and no movement of its own.
fn overlay_turn() -> h1::Transform {
    h1::Transform {
        rotation: [SQRT_HALF, 0.0, 0.0, SQRT_HALF],
        translation: [0.0; 3],
        scale: 1.0,
    }
}

/// One vertex of a quad, rigid on `node`.
fn vertex(p: [f32; 3], node: i16) -> h1::Vertex {
    h1::Vertex {
        position: p,
        normal: [0.0, 0.0, 1.0],
        binormal: [0.0, 1.0, 0.0],
        tangent: [1.0, 0.0, 0.0],
        texcoord: [0.0, 0.0],
        nodes: [node, -1],
        weights: [1.0, 0.0],
    }
}

/// A quad (two triangles as a strip) covering the box from `lo` to `hi`.
fn quad(lo: [f32; 3], hi: [f32; 3], node: i16) -> h1::Part {
    let vertices = vec![
        vertex([lo[0], lo[1], lo[2]], node),
        vertex([hi[0], lo[1], lo[2]], node),
        vertex([lo[0], hi[1], hi[2]], node),
        vertex([hi[0], hi[1], hi[2]], node),
    ];
    let centroid = vertices.iter().map(|v| v.position).fold([0.0; 3], |a, p| {
        [a[0] + p[0] / 4.0, a[1] + p[1] / 4.0, a[2] + p[2] / 4.0]
    });
    h1::Part {
        flags: 0,
        shader: 0,
        centroid_nodes: [node, -1],
        centroid_weights: [1.0, 0.0],
        centroid,
        vertices,
        strip: vec![0, 1, 2, 3],
    }
}

fn animation(name: &str, kind: h1::AnimType, arm: Vec<h1::Transform>) -> h1::Animation {
    h1::Animation {
        name: name.into(),
        kind,
        frame_count: 3,
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
        animated: [1u64 << 1, 0, 0],
        frames: arm.into_iter().map(|t| vec![identity(), t]).collect(),
        zero_rotations: 0,
        compressed: None,
    }
}

/// The invented map: the two-bone model, its graph, their `hlmt` and the
/// `bipd` that uses it. With `link` false the map holds no `bipd`, so
/// nothing uses the graph.
pub(crate) fn anim_map(link: bool) -> Vec<u8> {
    use h1_assets::models::{jmad, mode};

    // A fresh multiplayer set (not a decomposed map, whose pinned index
    // has no room for more tags): globals, a scenario, then the model.
    let mut set = TagSet::new(map::CacheType::Multiplayer, "court", r"invented\court");
    for group in [b"scnr", b"matg", b"mode", b"jmad", b"hlmt"] {
        set.add_group(FourCC(*group), None, None);
    }
    set.add_group(FourCC(*b"bipd"), Some(FourCC(*b"unit")), Some(FourCC(*b"obje")));
    let mut names = map::build::Block::zeroed(0xb4);
    names.string_id(0, "soft_organic_flesh");
    let mut globals = map::build::Block::zeroed(0x284);
    globals.block(0x150, 1, names);
    set.globals = set
        .add_tag(FourCC(*b"matg"), r"globals\globals", TagPayload::Built(globals))
        .unwrap();
    set.scenario = set
        .add_tag(
            FourCC(*b"scnr"),
            r"invented\court",
            TagPayload::Built(map::build::Block::zeroed(0x3e0)),
        )
        .unwrap();

    let model = h1::Model {
        flags: 0,
        node_list_checksum: 1,
        lod_cutoffs: [0.0; 5],
        base_map_scale: [1.0, 1.0],
        markers: vec![],
        nodes: vec![
            h1::Node {
                name: "body".into(),
                next_sibling: 1,
                first_child: 1,
                parent: -1,
                translation: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
                distance_from_parent: 0.0,
            },
            h1::Node {
                name: "arm".into(),
                next_sibling: -1,
                first_child: -1,
                parent: 0,
                translation: [1.0, 0.0, 0.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                distance_from_parent: 1.0,
            },
        ],
        regions: vec![h1::Region {
            name: "one".into(),
            permutations: vec![h1::Permutation {
                name: "base".into(),
                flags: 0,
                lods: [0; 5],
            }],
        }],
        geometries: vec![h1::Geometry {
            flags: 0,
            parts: vec![
                quad([-0.1; 3], [0.1; 3], 0),
                quad([0.9, -0.05, -0.05], [1.6, 0.05, 0.05], 1),
            ],
        }],
        // An opaque part: the writer draws a `soso` part opaque. The null
        // shader leaves it untextured, drawn grey.
        shaders: vec![h1::ShaderRef {
            group: h1map::Group::new(b"soso"),
            tag: h1map::TagId(0),
            permutation: 0,
        }],
    };
    let render = mode::convert_model(
        &model,
        r"invented\stickman",
        &mode::ModelOptions {
            shaders: vec![TagId::NULL],
            strips: false,
        },
    )
    .unwrap()
    .install(&mut set, r"invented\stickman")
    .unwrap();

    let graph = h1::Graph {
        nodes: vec![
            h1::AnimNode {
                name: "body".into(),
                next_sibling: 1,
                first_child: 1,
                parent: -1,
                joint_flags: 0,
                base_vector: [0.0; 3],
                vector_range: 0.0,
            },
            h1::AnimNode {
                name: "arm".into(),
                next_sibling: -1,
                first_child: -1,
                parent: 0,
                joint_flags: 0,
                base_vector: [0.0; 3],
                vector_range: 0.0,
            },
        ],
        animations: vec![
            animation(
                "stand idle",
                h1::AnimType::Base,
                (0..3).map(turn_of).collect(),
            ),
            animation(
                "stand arm-over",
                h1::AnimType::Overlay,
                (0..3).map(|_| overlay_turn()).collect(),
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
    let graph = jmad::convert_graph(&graph, None)
        .unwrap()
        .0
        .install(&mut set, r"invented\stickman")
        .unwrap();

    let hlmt = h1objects::rebuild::model(&h1objects::rebuild::Model {
        render,
        collision: None,
        animation: Some(graph),
        physics: None,
        nodes: mode::hlmt_nodes(&model),
        regions: vec![h1objects::rebuild::Region {
            name: "one".into(),
            collision: -1,
            physics: -1,
            permutations: vec![("base".into(), -1, -1)],
        }],
        materials: vec![h1objects::rebuild::ModelMaterial {
            name: "soft_organic_flesh".into(),
            global: "soft_organic_flesh".into(),
            global_index: 0,
        }],
    })
    .unwrap();
    let hlmt = set
        .add_tag(
            FourCC(*b"hlmt"),
            r"invented\stickman",
            TagPayload::Built(hlmt),
        )
        .unwrap();

    if link {
        let bipd = Defs::builtin().group(FourCC(*b"bipd")).unwrap();
        let mut r = defs::default_record(bipd);
        r.set("unit.obje.model", Val::tag(FourCC(*b"hlmt"), hlmt))
            .unwrap();
        let root = defs::to_built(bipd, &r, &mut |_| None).unwrap();
        set.add_tag(
            FourCC(*b"bipd"),
            r"invented\stickman",
            TagPayload::Built(root),
        )
        .unwrap();
    }
    set.build().unwrap().0
}

fn open(t: &Temp, link: bool) -> (crate::App, Api) {
    let (app, ..) = tag_tests::app_with_registry(&t.0, anim_map(link), tag_tests::registry());
    (app, Api::new())
}

fn call(api: &mut Api, app: &mut crate::App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

/// The error of `r`: its code and message.
fn refused(r: Reply) -> agent_proto::RpcError {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e,
    }
}

/// `a` within `eps` of `b`, element-wise.
fn close(a: &Value, b: &[f32], eps: f32) -> bool {
    let a = a.as_array().expect("an array");
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.as_f64()
                .is_some_and(|x| (x - f64::from(*y)).abs() < f64::from(eps))
        })
}

fn rotation(node: &Value) -> &Value {
    &node["rotation"]
}

fn world_column(node: &Value, column: usize) -> &Value {
    &node["world"].as_array().expect("four columns")[column]
}

fn floats(v: &Value) -> Vec<f32> {
    v.as_array()
        .expect("an array")
        .iter()
        .map(|x| x.as_f64().expect("a number") as f32)
        .collect()
}

#[test]
fn the_methods_are_listed() {
    let t = Temp::new("list-methods");
    let (mut app, mut api) = open(&t, true);
    let list = call(&mut api, &mut app, m::METHODS, json!({}));
    for name in [m::ANIM_LIST, m::ANIM_POSE] {
        let row = list
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not listed"));
        assert_eq!(row["status"], "ready");
    }
}

#[test]
fn anim_list_names_the_animations_the_codecs_and_the_model() {
    let t = Temp::new("list");
    let (mut app, mut api) = open(&t, true);
    let r = call(&mut api, &mut app, m::ANIM_LIST, json!({"tag": GRAPH}));
    assert_eq!(r["nodes"], 2);
    let animations = r["animations"].as_array().unwrap();
    assert_eq!(animations.len(), 2);
    assert_eq!(animations[0]["name"], IDLE);
    assert_eq!(animations[0]["type"], "base");
    assert_eq!(animations[0]["frames"], 3);
    assert_eq!(animations[0]["codecs"][0], "uncompressed_static_data");
    assert_eq!(animations[0]["codecs"][1], "8byte_quantized_rotation_only");
    assert_eq!(animations[0]["aim_grid"], false);
    assert_eq!(animations[0]["error"], Value::Null);
    assert_eq!(animations[1]["name"], ARM_OVER);
    assert_eq!(animations[1]["type"], "overlay");
    assert_eq!(animations[1]["frames"], 3);
    let users = r["users"].as_array().unwrap();
    assert_eq!(users.len(), 1);
    assert!(users[0]["name"].as_str().unwrap().contains("stickman"));
    assert!(users[0]["object"].as_str().unwrap().starts_with("0x"));
}

/// The accepted test of the slice: an invented two-bone model posed at a
/// frame gives the expected node transforms, and differs from the rest
/// pose where the clip moves it and nowhere else.
#[test]
fn a_posed_two_bone_model_gives_the_expected_node_transforms() {
    let t = Temp::new("pose");
    let (mut app, mut api) = open(&t, true);
    let rest = call(
        &mut api,
        &mut app,
        m::ANIM_POSE,
        json!({"tag": GRAPH, "animation": IDLE, "frame": 0}),
    );
    let r = call(
        &mut api,
        &mut app,
        m::ANIM_POSE,
        json!({"tag": GRAPH, "animation": IDLE, "frame": 2}),
    );
    assert_eq!(r["animation"]["name"], IDLE);
    assert_eq!(r["frames"], 3);
    assert_eq!(r["frame"], 2.0);
    let model = &r["model"];
    assert!(model["name"].as_str().unwrap().contains("stickman"));
    assert!(model["triangles"].as_u64().unwrap() > 0);
    assert_eq!(model["untextured"], 1);
    let nodes = r["nodes"].as_array().unwrap();
    let rest_nodes = rest["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2);
    assert_eq!(nodes[0]["name"], "body");
    assert_eq!(nodes[0]["parent"], -1);
    assert_eq!(nodes[1]["name"], "arm");
    assert_eq!(nodes[1]["parent"], 0);

    // Node 0 is moved nowhere: the same transforms as frame 0, and its
    // world matrix the identity.
    for n in [&nodes[0], &rest_nodes[0]] {
        assert!(close(rotation(n), &[0.0, 0.0, 0.0, 1.0], 1e-5), "{n}");
        assert!(close(&n["translation"], &[0.0, 0.0, 0.0], 1e-6), "{n}");
        assert!(
            close(world_column(n, 0), &[1.0, 0.0, 0.0, 0.0], 1e-5),
            "{n}"
        );
        assert!(
            close(world_column(n, 3), &[0.0, 0.0, 0.0, 1.0], 1e-5),
            "{n}"
        );
    }
    // Node 1: the animation supplies its rotation (the quantised quarter
    // turn about z); the translation is its own default, at x = 1; the
    // world matrix is the turn about the parent's origin.
    assert!(close(
        rotation(&nodes[1]),
        &[0.0, 0.0, SQRT_HALF, SQRT_HALF],
        1e-3
    ));
    assert!(close(&nodes[1]["translation"], &[1.0, 0.0, 0.0], 1e-6));
    assert_eq!(nodes[1]["has"]["rotation"], true);
    assert_eq!(nodes[1]["has"]["translation"], true);
    // A base animation carries every channel of every node (its static
    // stream holds the default scale of 1).
    assert_eq!(nodes[1]["has"]["scale"], true);
    assert_eq!(nodes[1]["scale"], 1.0);
    assert!(close(
        world_column(&nodes[1], 0),
        &[0.0, 1.0, 0.0, 0.0],
        1e-3
    ));
    assert!(close(
        world_column(&nodes[1], 3),
        &[1.0, 0.0, 0.0, 1.0],
        1e-6
    ));
    // Where the clip moves it, and nowhere else: frame 0 holds the rest
    // rotation and the rest world, and only node 1 differs.
    assert!(close(rotation(&rest_nodes[1]), &[0.0, 0.0, 0.0, 1.0], 1e-3));
    assert!(close(
        world_column(&rest_nodes[1], 0),
        &[1.0, 0.0, 0.0, 0.0],
        1e-3
    ));
    assert!(close(
        &rest_nodes[1]["translation"],
        &floats(&nodes[1]["translation"]),
        1e-6
    ));
    // A frame past the end holds the last one.
    let past = call(
        &mut api,
        &mut app,
        m::ANIM_POSE,
        json!({"tag": GRAPH, "animation": IDLE, "frame": 99.0}),
    );
    assert!(close(
        rotation(&past["nodes"][1]),
        &[0.0, 0.0, SQRT_HALF, SQRT_HALF],
        1e-3
    ));
}

#[test]
fn the_overlay_applies_with_the_engines_scaled_add_rule() {
    let t = Temp::new("overlay");
    let (mut app, mut api) = open(&t, true);
    let plain = call(
        &mut api,
        &mut app,
        m::ANIM_POSE,
        json!({"tag": GRAPH, "animation": IDLE, "frame": 2}),
    );
    // Weight 0 changes nothing.
    let zero = call(
        &mut api,
        &mut app,
        m::ANIM_POSE,
        json!({"tag": GRAPH, "animation": IDLE, "frame": 2,
               "overlay": {"animation": ARM_OVER, "weight": 0.0}}),
    );
    let want = floats(rotation(&plain["nodes"][1]));
    assert!(
        close(rotation(&zero["nodes"][1]), &want, 1e-5),
        "{}",
        zero["nodes"][1]
    );
    // Weight 1 composes the base's quarter about z with the overlay's
    // quarter about x: a third of a turn about the diagonal.
    let one = call(
        &mut api,
        &mut app,
        m::ANIM_POSE,
        json!({"tag": GRAPH, "animation": IDLE, "frame": 2,
               "overlay": {"animation": ARM_OVER, "weight": 1.0}}),
    );
    assert!(
        close(rotation(&one["nodes"][1]), &[0.5, 0.5, 0.5, 0.5], 1e-3),
        "{}",
        one["nodes"][1]
    );
    // A weight out of range is refused by name.
    let e = refused(api.call(
        &mut app,
        m::ANIM_POSE,
        json!({"tag": GRAPH, "animation": IDLE,
               "overlay": {"animation": ARM_OVER, "weight": 1.5}}),
    ));
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(e.message.contains("overlay.weight"));
}

#[test]
fn refusals_are_named() {
    let t = Temp::new("refusals");
    let (mut app, mut api) = open(&t, true);
    // Not a graph.
    let e = refused(api.call(
        &mut app,
        m::ANIM_LIST,
        json!({"tag": r"hlmt:invented\stickman"}),
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
        m::ANIM_POSE,
        json!({"tag": GRAPH, "animation": "nope"}),
    ));
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(e.message.contains(IDLE), "{}", e.message);
    // A model that does not use the graph.
    let e = refused(api.call(
        &mut app,
        m::ANIM_POSE,
        json!({"tag": GRAPH, "animation": IDLE, "model": r"weap:objects\test\blaster"}),
    ));
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("not one of the models"), "{}", e.message);
}

#[test]
fn a_graph_nothing_uses_answers_its_own_channels() {
    let t = Temp::new("unlinked");
    let (mut app, mut api) = open(&t, false);
    let r = call(
        &mut api,
        &mut app,
        m::ANIM_POSE,
        json!({"tag": GRAPH, "animation": IDLE, "frame": 2}),
    );
    assert_eq!(r["model"], Value::Null);
    assert!(r["png"].is_null() || r["png"].is_string());
    let notes = r["notes"].as_array().unwrap().iter().filter_map(|n| n.as_str()).collect::<Vec<_>>().join(" ");
    assert!(
        notes.contains("no model of this map uses this graph"),
        "{notes}"
    );
    // The channels are the animation's own: the arm's rotation and the
    // default the base carries as its static stream, with no skeleton to
    // merge or compose on.
    assert!(close(
        rotation(&r["nodes"][1]),
        &[0.0, 0.0, SQRT_HALF, SQRT_HALF],
        1e-3
    ));
    assert!(close(&r["nodes"][1]["translation"], &[1.0, 0.0, 0.0], 1e-6));
    assert_eq!(r["nodes"][1]["has"]["translation"], true);
    assert_eq!(r["nodes"][1]["world"], Value::Null);
}
