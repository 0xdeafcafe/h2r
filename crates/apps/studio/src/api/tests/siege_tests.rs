//! `siege.layout` over the invented map made into a line: red's flag stand
//! at x 0, blue's at x 100, two territories between and starts along it.

use super::*;

fn line_map(api: &mut Api, app: &mut App, t: &Temp) {
    open_map(api, app, t, "line.map", fixture::fixture_at_end());
    app.session.set_allow_growth(true);
    let set = |api: &mut Api, app: &mut App, block: &str, index: usize, field: &str, value: Value| {
        call(api, app, m::SCENARIO_SET, json!({"block": block, "index": index, "field": field, "value": value}));
    };
    set(api, app, "netgame_flags", 0, "type", json!(0));
    set(api, app, "netgame_flags", 0, "team", json!(0));
    set(api, app, "netgame_flags", 0, "position", json!([0, 0, 1]));
    for (i, (x, kind, team)) in [(100, 0, 1), (40, 10, 0), (60, 10, 0)].into_iter().enumerate() {
        call(api, app, m::SCENARIO_ADD, json!({"block": "netgame_flags", "like": 0, "position": [x, 0, 1]}));
        set(api, app, "netgame_flags", i + 1, "type", json!(kind));
        set(api, app, "netgame_flags", i + 1, "team", json!(team));
    }
    for (i, x) in [0, 20].into_iter().enumerate() {
        set(api, app, "starting_locations", i, "position", json!([x, 2, 1]));
    }
    for x in [70, 90, 100] {
        call(api, app, m::SCENARIO_ADD, json!({"block": "starting_locations", "like": 0, "position": [x, 2, 1]}));
    }
}

#[test]
fn siege_layout_plans_the_open_map_and_writes_the_document() {
    let t = Temp::new("siege-layout");
    let mut app = app(&t);
    let mut api = Api::new();
    line_map(&mut api, &mut app, &t);
    let out = t.path("line.json");
    let r = call(&mut api, &mut app, m::SIEGE_LAYOUT, json!({"per_side": 1, "out": out.display().to_string()}));
    assert_eq!(r["sites"], json!([{"index": 2, "phase": 0}, {"index": 3, "phase": 1}]), "{r}");
    assert_eq!(r["goal"], 1);
    assert_eq!(r["phases"], 2);
    assert_eq!(r["attack"][0], json!({"index": 0, "phase": 0}));
    assert_eq!(r["defend"][0], json!({"index": 2, "phase": 0}), "the start at x 70, past the sites");
    let text = std::fs::read_to_string(&out).unwrap();
    let doc = forge_doc::ForgeDoc::from_bytes(text.as_bytes(), &forge_doc::Limits::default()).unwrap();
    assert_eq!(doc.ops.len(), r["document"]["ops"].as_array().unwrap().len());
    assert_eq!(r["written"], out.display().to_string());
    // The open map is not changed.
    let flags = call(&mut api, &mut app, m::SCENARIO_LIST, json!({"block": "netgame_flags"}));
    assert_eq!(flags.as_array().unwrap().len(), 4);
}

#[test]
fn siege_layout_refuses_a_bad_ask_and_a_map_with_one_stand() {
    let t = Temp::new("siege-layout-bad");
    let mut app = app(&t);
    let mut api = Api::new();
    line_map(&mut api, &mut app, &t);
    for bad in [json!({"attack": "green"}), json!({"barriers": ["netgame_flags:0=1"]}), json!({"barriers": "crates"})] {
        assert_eq!(code_of(api.call(&mut app, m::SIEGE_LAYOUT, bad)), code::INVALID_PARAMS);
    }
    call(&mut api, &mut app, m::SCENARIO_SET, json!({"block": "netgame_flags", "index": 1, "field": "type", "value": 10}));
    assert_eq!(code_of(api.call(&mut app, m::SIEGE_LAYOUT, json!({}))), code::REFUSED);
}
