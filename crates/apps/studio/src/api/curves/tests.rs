//! `function.*` over the scenario crate's invented cache: its weapon tag
//! given thirteen invented data fields, twelve of them `view=function`
//! curves, one of every type, and one plain data reference (no game
//! data).
use std::path::PathBuf;

use map::TagId;
use scenario_edit::fixture::{self, put, putf};
use serde_json::{Value, json};
use studio_proto::{code, method as m};
use tags::effects::function::{Function, FunctionType, HEADER_LEN, Set};
use tags::registry::Registry;

use crate::api::Api;
use crate::app::tag_doc::DATA_GROWTH_REFUSAL;
use crate::app::tag_tests;
use crate::fields::curve::{EVALUATED_AS_ZERO, change_to_unevaluated};

/// The weapon's function fields: (name, the blob's type byte). `curve`
/// is the transition the examples edit.
const FIELDS: &[(&str, u8)] = &[
    ("curve", 2),
    ("identity", 0),
    ("constant", 1),
    ("periodic", 3),
    ("linear", 4),
    ("linear_key", 5),
    ("multi_linear_key", 6),
    ("spline", 7),
    ("multi_spline", 8),
    ("exponent", 9),
    ("spline_2", 10),
    ("curvex", 0x42),
];

/// The weapon's root layout: a float, then twelve function data
/// references and one plain one.
pub(crate) fn registry() -> &'static Registry {
    static R: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let mut schema = String::from("group scnr 0x10 align 4\ngroup weap 0x70 align 4\n");
        let mut defs = String::from("group weap 0x70\n  0x0 f32 speed\n  0x4 unknown 0x4\n");
        for i in 0..FIELDS.len() + 1 {
            let at = 0x8 + i * 8;
            schema.push_str(&format!("  0x{at:x} block 0x1 align 1\n"));
            let view = if i < FIELDS.len() {
                " view=function"
            } else {
                ""
            };
            defs.push_str(&format!(
                "  0x{at:x} data {} align 1{view}\n",
                if i < FIELDS.len() {
                    FIELDS[i].0
                } else {
                    "plain"
                }
            ));
        }
        let schema = tags::schema::Schema::parse(&schema).unwrap();
        let mut d = tags::defs::Defs::default();
        d.parse_into("test", &defs).unwrap();
        Registry::build(&schema, &d)
    })
}

/// Where the invented function blobs are (the metadata past the
/// collection tag, one after another: a blob is as long as its type, up to
/// 272 bytes).
const BLOB: u32 = fixture::META + 0x1300;
const BLOB_FILE: usize = fixture::META_FILE + 0x1300;

/// Where blob `i` starts, past `BLOB`: after the blobs before it.
fn slot(i: usize) -> usize {
    FIELDS[..i.min(FIELDS.len())]
        .iter()
        .map(|(_, kind)| invented(*kind).len().next_multiple_of(4))
        .sum()
}

/// Build a blob: header + payload words.
fn blob(kind: u8, flags: u8, subs: [u8; 2], range: [f32; 2], words: &[f32]) -> Vec<u8> {
    let mut v = vec![kind, flags, subs[0], subs[1]];
    v.extend(range[0].to_le_bytes());
    v.extend(range[1].to_le_bytes());
    v.extend([0u8; 8]);
    for w in words {
        v.extend(w.to_le_bytes());
    }
    v
}

/// An invented blob of `kind`: every payload word distinct (`word j`
/// holds `j * 0.25`), the transition ranged, the periodic a cosine.
fn invented(kind: u8) -> Vec<u8> {
    let words = tags::effects::function::FunctionType::from_byte(kind)
        .blob_len()
        .map_or(4, |n| (n - HEADER_LEN) / 4);
    let all: Vec<f32> = (0..words).map(|i| i as f32 * 0.25).collect();
    let (flags, subs) = match kind {
        2 => (1, [3, 1]),
        3 => (0, [2, 0]),
        _ => (0, [0, 0]),
    };
    blob(kind, flags, subs, [0.0, 1.0], &all)
}

/// The fixture with its weapon tag given the thirteen data references:
/// one function blob of every type, then a plain reference to eight
/// zero bytes.
pub(crate) fn curve_map() -> Vec<u8> {
    let mut b = fixture::fixture_at_end();
    let tags = fixture::INDEX + 32 + 4 * 12;
    put(&mut b, tags + 2 * 16 + 12, 0x70);
    let w = fixture::META_FILE + 0x1100;
    putf(&mut b, w, 2.5);
    for (i, (_, kind)) in FIELDS.iter().enumerate() {
        let bytes = invented(*kind);
        let at = BLOB_FILE + slot(i);
        b[at..at + bytes.len()].copy_from_slice(&bytes);
        put(&mut b, w + 0x8 + i * 8, bytes.len() as u32);
        put(&mut b, w + 0x8 + i * 8 + 4, BLOB + slot(i) as u32);
    }
    // The plain reference: eight zero bytes, not a function blob.
    put(&mut b, w + 0x8 + FIELDS.len() * 8, 8);
    put(
        &mut b,
        w + 0x8 + FIELDS.len() * 8 + 4,
        BLOB + slot(FIELDS.len()) as u32,
    );
    b
}

fn weap_id() -> TagId {
    fixture::id(2)
}

/// The blob of field `name` as the map holds it.
fn field_blob(name: &str) -> Vec<u8> {
    invented(FIELDS.iter().find(|(n, _)| *n == name).unwrap().1)
}

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-curves-{tag}-{}", std::process::id()));
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
    let (app, ..) = tag_tests::app_with_registry(&t.0, curve_map(), registry());
    (app, Api::new())
}

fn call(api: &mut Api, app: &mut crate::App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

fn code_of(r: crate::api::Reply) -> i64 {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e.code,
    }
}

fn message_of(r: crate::api::Reply) -> String {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e.message,
    }
}

#[test]
fn the_methods_are_listed() {
    let t = Temp::new("list");
    let (mut app, mut api) = open(&t);
    let list = call(&mut api, &mut app, m::METHODS, json!({}));
    for name in [m::FUNCTION_READ, m::FUNCTION_SET, m::FUNCTION_SAMPLE, m::FUNCTION_COLOURS] {
        let row = list
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not listed"));
        assert_eq!(row["status"], "ready");
    }
}

/// `function.sample` is `Function::evaluate` itself, for every type on
/// the invented blobs, with and without a range input.
#[test]
fn sample_equals_the_evaluator_for_every_type() {
    let t = Temp::new("sample");
    let (mut app, mut api) = open(&t);
    let inputs = [0.0f32, 0.25, 0.5, 0.75, 1.0, 0.3, 2.0];
    for (name, _) in FIELDS {
        let f = tags::effects::function::Function::parse(&field_blob(name)).unwrap();
        for r in [0.0f32, 0.4, 1.0] {
            let got = call(
                &mut api,
                &mut app,
                m::FUNCTION_SAMPLE,
                json!({"tag": "weap", "path": name, "inputs": inputs, "range_input": r}),
            );
            let want: Vec<f32> = inputs.iter().map(|&x| f.evaluate(x, r)).collect();
            let got: Vec<f32> = got["outputs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect();
            assert_eq!(got, want, "{name} at range input {r}");
        }
        // Without a range input, set 0.
        let got = call(
            &mut api,
            &mut app,
            m::FUNCTION_SAMPLE,
            json!({"tag": "weap", "path": name, "inputs": inputs}),
        );
        let want: Vec<f32> = inputs.iter().map(|&x| f.evaluate(x, 0.0)).collect();
        let got: Vec<f32> = got["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect();
        assert_eq!(got, want, "{name} at the default range input");
    }
    // A bad input is a param error.
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::FUNCTION_SAMPLE,
            json!({"tag": "weap", "path": "curve", "inputs": [0.5, "half"]})
        )),
        code::INVALID_PARAMS
    );
}

/// The read gives the typed sets of every type, and the row its curve.
#[test]
fn read_gives_the_typed_sets() {
    let t = Temp::new("read");
    let (mut app, mut api) = open(&t);
    let mut read = |path: &str| {
        call(
            &mut api,
            &mut app,
            m::FUNCTION_READ,
            json!({"tag": "weap", "path": path}),
        )
    };
    let w = |k: usize| k as f64 * 0.25;

    let r = read("curve");
    assert_eq!(r["type"], "transition");
    assert_eq!(r["ranged"], true);
    assert_eq!(r["sub_types"], json!([3, 1]));
    assert_eq!(r["range"], json!([0.0, 1.0]));
    assert_eq!(r["length"], 36);
    assert_eq!(
        (r["editable"].clone(), r["read_only"].clone()),
        (json!(true), Value::Null)
    );
    let sets = r["sets"].as_array().unwrap();
    assert_eq!(sets.len(), 2);
    assert_eq!(sets[0], json!({"shape": 3, "from": 0.0, "to": 0.25}));
    assert_eq!(sets[1], json!({"shape": 1, "from": 0.5, "to": 0.75}));

    let r = read("identity");
    assert_eq!(r["sets"], json!([{}, {}]));
    let r = read("constant");
    assert_eq!(r["sets"], json!([{"words": [0.0]}, {"words": [0.25]}]));
    let r = read("periodic");
    assert_eq!(
        r["sets"][0],
        json!({"wave": 2, "frequency": 0.0, "phase": 0.25, "minimum": 0.5, "maximum": 0.75})
    );
    let r = read("linear");
    assert_eq!(r["sets"][0], json!({"slope": w(4), "intercept": w(5)}));
    let r = read("linear_key");
    assert_eq!(r["sets"][0]["base"], w(16));
    assert_eq!(
        r["sets"][0]["keys"],
        json!([
            {"value": w(17), "position": w(9), "inverse_span": w(13)},
            {"value": w(18), "position": w(10), "inverse_span": w(14)},
            {"value": w(19), "position": w(11), "inverse_span": w(15)},
        ])
    );
    let points = json!([[w(0), w(1)], [w(2), w(3)], [w(4), w(5)], [w(6), w(7)]]);
    let coefficients = json!([w(8), w(9), w(10), w(11)]);
    for name in ["spline", "spline_2"] {
        let r = read(name);
        assert_eq!(r["sets"][0]["points"], points);
        assert_eq!(r["sets"][0]["coefficients"], coefficients);
    }
    let r = read("exponent");
    assert_eq!(
        r["sets"][0],
        json!({"minimum": w(0), "maximum": w(1), "power": w(2)})
    );
    // Types 6 and 8: words, read-only.
    for (name, len) in [("multi_linear_key", 32usize), ("multi_spline", 4)] {
        let r = read(name);
        assert_eq!(r["editable"], false);
        assert_eq!(r["read_only"], EVALUATED_AS_ZERO);
        // Set 0 is the first `len` words, set 1 the next `len`.
        for set in 0..2 {
            let words: Vec<f64> = (set * len..(set + 1) * len).map(w).collect();
            assert_eq!(r["sets"][set], json!({"words": words}), "{name} set {set}");
        }
    }
    // An unknown type has no layout: no sets.
    let r = read("curvex");
    assert_eq!(r["type"], "unknown_66");
    assert_eq!(r["sets"], Value::Null);
    assert_eq!(r["read_only"], EVALUATED_AS_ZERO);

    // A function row carries its curve; a plain field does not.
    let rows = call(&mut api, &mut app, m::TAG_FIELDS, json!({"tag": "weap"}));
    let row = |name: &str| {
        rows["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not in the rows"))
            .clone()
    };
    let curve = row("curve");
    assert_eq!(curve["view"], "function");
    assert_eq!(
        curve["function"],
        json!({"type": "transition", "ranged": true, "range": [0.0, 1.0], "length": 36})
    );
    assert!(row("speed")["function"].is_null());
    assert!(row("plain")["function"].is_null());
    assert_eq!(row("plain")["view"], Value::Null);
}

/// An edit round-trips through `to_bytes`: the bytes the tag holds equal
/// the function's own `to_bytes`, the words the type does not name keep
/// their values, and one undo step restores the blob.
#[test]
fn an_edit_round_trips_through_to_bytes() {
    let t = Temp::new("set");
    let (mut app, mut api) = open(&t);
    let r = call(
        &mut api,
        &mut app,
        m::FUNCTION_SET,
        json!({
            "tag": "weap", "path": "curve",
            "ranged": false, "range": [0.5, 2.5],
            "sets": [
                {"from": 0.125, "to": 0.875, "shape": 2},
                {"from": 0.25, "to": 0.5},
            ],
        }),
    );
    assert_eq!(r["ranged"], false);
    assert_eq!(r["range"], json!([0.5, 2.5]));
    assert_eq!(r["sub_types"], json!([2, 1]));
    assert_eq!(
        r["sets"][0],
        json!({"shape": 2, "from": 0.125, "to": 0.875})
    );
    assert_eq!(r["sets"][1], json!({"shape": 1, "from": 0.25, "to": 0.5}));

    let bytes = app.tags.read(weap_id(), BLOB, 36).unwrap();
    let f = tags::effects::function::Function::parse(&bytes).unwrap();
    assert!(!f.is_ranged());
    assert_eq!(f.range, [0.5, 2.5]);
    assert_eq!(f.sub_types, [2, 1]);
    assert_eq!(
        f.set_values(0),
        Some(Set::Transition {
            from: 0.125,
            to: 0.875
        })
    );
    // The round trip itself: the tag's bytes are the function's.
    assert_eq!(f.to_bytes(), bytes);

    // A partial edit of a linear keeps the words the type does not name
    // (its first four, and the intercept).
    let r = call(
        &mut api,
        &mut app,
        m::FUNCTION_SET,
        json!({"tag": "weap", "path": "linear", "sets": [{"slope": 2.0}]}),
    );
    assert_eq!(r["sets"][0], json!({"slope": 2.0, "intercept": 1.25}));
    let at = BLOB + slot(4) as u32;
    let bytes = app.tags.read(weap_id(), at, 68).unwrap();
    let word = |k: usize| {
        f32::from_le_bytes(
            bytes[HEADER_LEN + 4 * k..HEADER_LEN + 4 * k + 4]
                .try_into()
                .unwrap(),
        )
    };
    assert_eq!((word(0), word(2), word(3)), (0.0, 0.5, 0.75));
    assert_eq!((word(4), word(5)), (2.0, 1.25));

    // One undo step each.
    app.dispatch(crate::app::Action::Undo);
    assert_eq!(
        app.tags.read(weap_id(), at, 68).unwrap(),
        field_blob("linear")
    );
    app.dispatch(crate::app::Action::Undo);
    assert_eq!(
        app.tags.read(weap_id(), BLOB, 36).unwrap(),
        field_blob("curve")
    );
}

/// Types the game does not evaluate are read-only, a type change is
/// refused, and so is anything that is not a function curve field.
#[test]
fn refusals() {
    let t = Temp::new("refuse");
    let (mut app, mut api) = open(&t);
    for name in ["multi_linear_key", "multi_spline", "curvex"] {
        let r = api.call(
            &mut app,
            m::FUNCTION_SET,
            json!({"tag": "weap", "path": name, "ranged": true}),
        );
        assert_eq!(code_of(r), code::REFUSED, "{name}");
        assert_eq!(
            message_of(api.call(
                &mut app,
                m::FUNCTION_SET,
                json!({"tag": "weap", "path": name, "ranged": true})
            )),
            EVALUATED_AS_ZERO
        );
    }
    // A change to a type the game evaluates as 0 has nothing to build.
    for (word, kind) in [
        ("multi_linear_key", FunctionType::MultiLinearKey),
        ("multi_spline", FunctionType::MultiSpline),
    ] {
        assert_eq!(
            message_of(api.call(
                &mut app,
                m::FUNCTION_SET,
                json!({"tag": "weap", "path": "curve", "type": word})
            )),
            change_to_unevaluated(kind)
        );
    }
    // A longer blob is refused while growth is off, and nothing changes.
    assert_eq!(
        message_of(api.call(
            &mut app,
            m::FUNCTION_SET,
            json!({"tag": "weap", "path": "curve", "type": "linear_key"})
        )),
        DATA_GROWTH_REFUSAL
    );
    assert!(!app.tags.has_edits());
    // The same type is taken (nothing changes).
    let r = call(
        &mut api,
        &mut app,
        m::FUNCTION_SET,
        json!({"tag": "weap", "path": "curve", "type": "transition"}),
    );
    assert_eq!(r["type"], "transition");
    assert_eq!(
        app.tags.read(weap_id(), BLOB, 36).unwrap(),
        field_blob("curve")
    );
    // A word that is not a type.
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::FUNCTION_SET,
            json!({"tag": "weap", "path": "curve", "type": "ribbon"})
        )),
        code::INVALID_PARAMS
    );
    // A handle the type does not have.
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::FUNCTION_SET,
            json!({"tag": "weap", "path": "curve", "sets": [{"slope": 1.0}]})
        )),
        code::INVALID_PARAMS
    );
    // Fields that are not function curves.
    assert!(
        message_of(api.call(
            &mut app,
            m::FUNCTION_READ,
            json!({"tag": "weap", "path": "speed"})
        ))
        .contains("is not a function curve field")
    );
    assert!(
        message_of(api.call(
            &mut app,
            m::FUNCTION_READ,
            json!({"tag": "weap", "path": "plain"})
        ))
        .contains("is not a function curve field")
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::FUNCTION_READ,
            json!({"tag": "weap", "path": "nonsense"})
        )),
        code::INVALID_PARAMS
    );
}

/// The curve field's pair: the weapon's root, then the field's offset.
const CURVE_PAIR: u32 = fixture::META + 0x1100 + 0x8;

/// The blob field `curve` points at, as `function.read` and the document
/// have it.
fn curve_bytes(app: &crate::App) -> Vec<u8> {
    let (n, at) = app.tags.pair(weap_id(), CURVE_PAIR).unwrap();
    app.tags.read(weap_id(), at, n).unwrap()
}

/// A type change saves at the new `blob_len` and reads back equal: a
/// shorter type in place, a longer one grown (with growth on), the saved
/// map holding the same bytes; one undo step each, and the sets given in
/// the same call apply to the new type.
#[test]
fn a_type_change_saves_at_the_new_length() {
    let t = Temp::new("retype");
    let (mut app, mut api) = open(&t);
    app.session.set_allow_growth(true);
    app.tags.allow_growth = true;
    let from = Function::parse(&field_blob("curve")).unwrap();

    // Shorter: a constant (a ranged transition keeps its flags and range).
    let r = call(
        &mut api,
        &mut app,
        m::FUNCTION_SET,
        json!({"tag": "weap", "path": "curve", "type": "constant"}),
    );
    let want = from.with_kind(FunctionType::Constant).unwrap();
    assert_eq!(r["type"], "constant");
    assert_eq!(r["length"], FunctionType::Constant.blob_len().unwrap());
    assert_eq!(r["ranged"], true);
    assert_eq!(curve_bytes(&app), want.to_bytes());

    // Longer, with a set edited in the same call.
    let r = call(
        &mut api,
        &mut app,
        m::FUNCTION_SET,
        json!({"tag": "weap", "path": "curve", "type": "linear_key",
               "sets": [{"base": 0.125}]}),
    );
    let len = FunctionType::LinearKey.blob_len().unwrap();
    assert_eq!(
        (r["type"].clone(), r["length"].clone()),
        (json!("linear_key"), json!(len))
    );
    assert_eq!(r["sets"][0]["base"], 0.125);
    let mut want = from.with_kind(FunctionType::LinearKey).unwrap();
    let Some(Set::LinearKey { keys, .. }) = want.set_values(0) else {
        unreachable!()
    };
    want.write_set(0, &Set::LinearKey { base: 0.125, keys })
        .unwrap();
    assert_eq!(curve_bytes(&app), want.to_bytes());
    // The read, the sample and the row agree with the new blob.
    let s = call(
        &mut api,
        &mut app,
        m::FUNCTION_SAMPLE,
        json!({"tag": "weap", "path": "curve", "inputs": [0.5], "range_input": 0.5}),
    );
    assert_eq!(
        s["outputs"][0].as_f64().unwrap() as f32,
        want.evaluate(0.5, 0.5)
    );

    // The saved map holds the same bytes at the same length.
    let out = t.0.join("retype.map");
    app.dispatch(crate::app::Action::SaveMapAs(out.clone()));
    let mut saved =
        map::Map::from_reader(std::io::Cursor::new(std::fs::read(&out).unwrap())).unwrap();
    saved.validate_checksum().unwrap();
    let mut doc = crate::app::TagDoc::new(registry());
    doc.load(&mut saved).unwrap();
    let (n, at) = doc.pair(weap_id(), CURVE_PAIR).unwrap();
    assert_eq!(n as usize, len);
    assert_eq!(doc.read(weap_id(), at, n).unwrap(), want.to_bytes());

    // One undo step per change.
    app.dispatch(crate::app::Action::Undo);
    assert_eq!(
        curve_bytes(&app),
        from.with_kind(FunctionType::Constant).unwrap().to_bytes()
    );
    app.dispatch(crate::app::Action::Undo);
    assert_eq!(curve_bytes(&app), field_blob("curve"));
}

/// A same-length type change (spline to spline 2) is a rebuild too, and
/// the type words round-trip through `function.read`.
#[test]
fn every_editable_type_reads_back_what_was_written() {
    let t = Temp::new("retype-all");
    let (mut app, mut api) = open(&t);
    app.session.set_allow_growth(true);
    app.tags.allow_growth = true;
    for word in [
        "linear_key",
        "spline",
        "spline_2",
        "periodic",
        "exponent",
        "linear",
        "transition",
        "identity",
        "constant",
    ] {
        let r = call(
            &mut api,
            &mut app,
            m::FUNCTION_SET,
            json!({"tag": "weap", "path": "curve", "type": word}),
        );
        let kind = FunctionType::from_word(word).unwrap();
        assert_eq!(r["type"], word);
        assert_eq!(
            r["length"].as_u64().unwrap() as usize,
            kind.blob_len().unwrap()
        );
        let read = call(
            &mut api,
            &mut app,
            m::FUNCTION_READ,
            json!({"tag": "weap", "path": "curve"}),
        );
        assert_eq!(read, r, "{word}");
        let bytes = curve_bytes(&app);
        assert_eq!(bytes.len(), kind.blob_len().unwrap(), "{word}");
        assert_eq!(Function::parse(&bytes).unwrap().kind, kind);
    }
}

/// `function.colours` reads the header's colours the way the engine's
/// colour reader does: the stops by mode, and a gradient that is
/// `output_colour` at each input.
#[test]
fn colours_read_the_stops_and_the_gradient() {
    let t = Temp::new("colours");
    let (mut app, mut api) = open(&t);
    // An identity blob has flags 0: mode grey, no stops.
    let r = call(
        &mut api,
        &mut app,
        m::FUNCTION_COLOURS,
        json!({"tag": "weap", "path": "identity", "samples": 3}),
    );
    let f = Function::parse(&field_blob("identity")).unwrap();
    let (mode, stops) = f.colour_stops();
    assert_eq!(r["mode"], mode);
    assert_eq!(r["stops"].as_array().unwrap().len(), stops.len());
    let g = r["gradient"].as_array().unwrap();
    assert_eq!(g.len(), 3);
    for (row, x) in g.iter().zip([0.0f32, 0.5, 1.0]) {
        assert_eq!(row["at"].as_f64().unwrap() as f32, x);
        assert_eq!(
            row["colour"],
            format!("#{:08x}", f.output_colour(x)).as_str()
        );
    }
    // The default is sixteen samples; a count out of range, and a field
    // that is not a curve, are refused.
    let r = call(
        &mut api,
        &mut app,
        m::FUNCTION_COLOURS,
        json!({"tag": "weap", "path": "curve"}),
    );
    assert_eq!(r["gradient"].as_array().unwrap().len(), 16);
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::FUNCTION_COLOURS,
            json!({"tag": "weap", "path": "curve", "samples": 1})
        )),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::FUNCTION_COLOURS,
            json!({"tag": "weap", "path": "plain"})
        )),
        code::REFUSED
    );
}
