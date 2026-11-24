//! `sound.face` (docs/editor/rich-editors.md, slice A3) over two invented
//! maps: the two-bone stickman of `anim.*`'s tests for the pose, and the
//! sound with a mouth curve of `sound.*`'s tests for the weight. Every
//! byte is invented.

use std::path::PathBuf;

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use crate::api::Api;
use crate::app::tag_tests;

const BIPED: &str = r"bipd:invented\stickman";
const SOUND: &str = r"snd!:objects\test\blaster";
const ARM_OVER: &str = "combat:arm_over";
const SQRT_HALF: f32 = std::f32::consts::FRAC_1_SQRT_2;

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-face-{tag}-{}", std::process::id()));
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

fn open(t: &Temp, map: Vec<u8>) -> (crate::App, Api) {
    let (mut app, ..) = tag_tests::app_with_registry(&t.0, map, tag_tests::registry());
    app.audition.headless = true;
    (app, Api::new())
}

fn call(api: &mut Api, app: &mut crate::App, params: Value) -> Value {
    match api.call(app, m::SOUND_FACE, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("sound.face {params}: {e}"),
    }
}

fn arm_rotation(r: &Value) -> Vec<f32> {
    r["nodes"][1]["rotation"]
        .as_array()
        .expect("the arm's rotation")
        .iter()
        .map(|v| v.as_f64().unwrap() as f32)
        .collect()
}

fn close(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
}

/// The acceptance test's first half: the overlay at weight 0 and at 1
/// gives two poses on the picked model. Weight 0 is the idle's frame 0
/// (the arm unturned); weight 1 is the overlay's quarter turn about x.
#[test]
fn weight_zero_and_one_give_two_poses_of_the_picked_model() {
    let t = Temp::new("weights");
    let (mut app, mut api) = open(&t, super::tests::anim_map(true));
    let at = |api: &mut Api, app: &mut crate::App, w: f32| {
        call(
            api,
            app,
            json!({"on": BIPED, "weight": w, "overlay": ARM_OVER}),
        )
    };
    let zero = at(&mut api, &mut app, 0.0);
    let one = at(&mut api, &mut app, 1.0);
    assert!(
        close(&arm_rotation(&zero), &[0.0, 0.0, 0.0, 1.0]),
        "{}",
        zero["nodes"][1]
    );
    assert!(
        close(&arm_rotation(&one), &[SQRT_HALF, 0.0, 0.0, SQRT_HALF]),
        "{}",
        one["nodes"][1]
    );
    for r in [&zero, &one] {
        assert!(
            r["model"]["name"].as_str().unwrap().contains("stickman"),
            "{r}"
        );
        assert_eq!(r["talk"], ARM_OVER);
        assert!(
            r["base"].as_str().unwrap().ends_with(":idle"),
            "{}",
            r["base"]
        );
        assert_eq!(r["at_ms"], Value::Null);
    }
    assert_eq!(zero["weight"], 0.0);
    assert_eq!(one["weight"], 1.0);
}

/// The stickman's graph has no `talk` overlay in its mode tree: the
/// lookup's refusal names what it looked for.
#[test]
fn a_graph_without_talk_is_refused_by_name() {
    let t = Temp::new("no-talk");
    let (mut app, mut api) = open(&t, super::tests::anim_map(true));
    let e = api
        .call(&mut app, m::SOUND_FACE, json!({"on": BIPED, "weight": 0.5}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(
        e.message.contains("no talk overlay at combat:any:any"),
        "{}",
        e.message
    );
}

/// The acceptance test's second half: the weight at an instant is that
/// tick's byte over 255. The invented curve rises from 0 to 255 over 52
/// ticks (`i * 255 / 51`); 500 ms is tick 15.
#[test]
fn the_weight_at_an_instant_uses_that_ticks_byte() {
    let t = Temp::new("tick");
    let (mut app, mut api) = open(&t, crate::api::sound::tests::mouth_map());
    let r = call(&mut api, &mut app, json!({"tag": SOUND, "at_ms": 500}));
    let byte = (15 * 255 / 51) as u64;
    assert_eq!(r["tick"], 15);
    assert_eq!(r["ticks"], 52);
    assert_eq!(r["byte"], byte);
    assert_eq!(r["voiced"], true);
    let w = r["weight"].as_f64().unwrap() as f32;
    assert!((w - byte as f32 / 255.0).abs() < 1e-6, "{r}");
    // Past the curve's end the mouth is closed.
    let past = call(&mut api, &mut app, json!({"tag": SOUND, "at_ms": 60_000}));
    assert_eq!(
        (past["byte"].clone(), past["weight"].clone()),
        (Value::Null, json!(0.0))
    );
    // An unvoiced permutation gives weight 0 and says so.
    let quiet = call(
        &mut api,
        &mut app,
        json!({"tag": SOUND, "permutation": 1, "at_ms": 500}),
    );
    assert_eq!(
        (quiet["voiced"].clone(), quiet["weight"].clone()),
        (json!(false), json!(0.0))
    );
}

/// Tag and weight are one or the other.
#[test]
fn tag_and_weight_are_exclusive() {
    let t = Temp::new("params");
    let (mut app, mut api) = open(&t, crate::api::sound::tests::mouth_map());
    for params in [
        json!({"tag": SOUND, "weight": 1}),
        json!({}),
        json!({"weight": 2}),
    ] {
        let e = api
            .call(&mut app, m::SOUND_FACE, params.clone())
            .unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{params}");
    }
}
