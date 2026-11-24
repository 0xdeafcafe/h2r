//! The recordings read over the scenario crate's invented map: invented
//! streams, an invented executable, no game data.
use hs::fixture::{Builtin, Exe, Tables};
use scenario_edit::fixture;
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::*;
use crate::api::Api;
use crate::app::map_cards_tests::Temp;
use crate::app::scripts::tests as scripts;
use crate::app::{Action, App};

fn open(map: std::path::PathBuf, t: &Temp) -> App {
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(map));
    assert!(app.session.is_open());
    app
}

#[test]
fn the_block_reads_and_each_stream_decodes_or_says_why() {
    let t = Temp::new("recordings-read");
    let mut app = open(scripts::scripted_source(&t.0, true), &t);
    app.sync_scripts();
    let rows = app.recordings_loaded().unwrap();
    let names: Vec<_> = rows.iter().map(|r| r.recorded.name.as_str()).collect();
    assert_eq!(names, ["arrive", "broken", "empty"]);
    let d = rows[0].decoded.as_ref().unwrap();
    assert_eq!(d.events.len(), 2);
    assert_eq!(d.ticks, 5);
    let why = rows[1].decoded.as_ref().unwrap_err();
    assert!(why.contains("type 30") && why.contains("0x48"), "{why}");
    assert!(rows[2].decoded.as_ref().unwrap_err().contains("initial state"));
    // Another stream version is named, not guessed at.
    let mut other = rows[0].recorded.clone();
    other.version = 2;
    assert!(Row::new(other).decoded.unwrap_err().contains("version 2"));
}

#[test]
fn a_map_without_a_block_reads_empty_and_without_a_map_refuses() {
    let t = Temp::new("recordings-none");
    let mut api = Api::new();
    let mut app = App::new();
    let e = api.call(&mut app, m::RECORDING_LIST, json!({})).unwrap_err();
    assert_eq!(e.code, code::NO_DOCUMENT);
    let mut app = open(
        {
            let map = t.0.join("plain.map");
            std::fs::write(&map, fixture::fixture()).unwrap();
            map
        },
        &t,
    );
    let v = api.call(&mut app, m::RECORDING_LIST, json!({})).unwrap();
    assert_eq!(v["recordings"], json!([]));
    assert_eq!(v["note"], NONE);
}

fn call(api: &mut Api, app: &mut App, method: &str, params: Value) -> Value {
    api.call(app, method, params).unwrap()
}

#[test]
fn recording_list_and_read_answer_over_the_api() {
    let t = Temp::new("recordings-api");
    let mut app = open(scripts::scripted_source(&t.0, true), &t);
    let mut api = Api::new();
    let v = call(&mut api, &mut app, m::RECORDING_LIST, json!({}));
    let rows = v["recordings"].as_array().unwrap();
    assert_eq!(rows.len(), 3, "{v}");
    assert_eq!(rows[0]["name"], "arrive");
    assert_eq!(rows[0]["events"], 2);
    assert_eq!(rows[0]["ticks"], 5);
    assert_eq!(rows[0]["played_by"], json!([]));
    assert_eq!(rows[1]["events"], Value::Null);
    assert!(rows[1]["problem"].as_str().unwrap().contains("type 30"));

    let v = call(&mut api, &mut app, m::RECORDING_READ, json!({"name": "ARRIVE"}));
    assert_eq!(v["walked_ticks"], 5);
    assert_eq!(v["initial"]["fields"]["throttle"], json!([0.0, 0.0]));
    assert_eq!(v["initial"]["trailer"].as_str().unwrap().len(), 24);
    let events = v["events"].as_array().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["name"], "throttle");
    assert_eq!(events[0]["tick"], 4);
    assert_eq!(events[0]["fields"]["value"], json!([0.5, 0.0]));
    assert_eq!(events[0]["payload"], "0000003f00000000");
    assert_eq!((events[1]["name"].as_str(), events[1]["tick"].as_u64()), (Some("end"), Some(5)));
    assert_eq!(v["truncated"], false);
    let v = call(&mut api, &mut app, m::RECORDING_READ, json!({"name": "arrive", "limit": 1, "offset": 1}));
    assert_eq!(v["events"].as_array().unwrap().len(), 1);
    assert_eq!(v["events"][0]["name"], "end");
    assert_eq!(v["total"], 2);

    // A broken stream is refused with the offset; an unknown name and a
    // bad limit are bad params.
    let e = api.call(&mut app, m::RECORDING_READ, json!({"name": "broken"})).unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("0x48"), "{}", e.message);
    for params in [json!({"name": "absent"}), json!({"name": "arrive", "limit": 0})] {
        let e = api.call(&mut app, m::RECORDING_READ, params).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
    }
}

/// The invented executable with one builtin that takes a recording.
fn playing_exe() -> Exe {
    let mut types = scripts::types();
    types[40] = "cutscene_recording".to_owned();
    Exe {
        types,
        builtins: vec![Builtin {
            returns: 4,
            parameters: vec![40],
            ..Builtin::default()
        }],
        ..Exe::default()
    }
}

#[test]
fn scripts_that_play_a_recording_are_its_users() {
    let t = Temp::new("recordings-played");
    let mut b = fixture::fixture();
    let payload = &mut b[fixture::sfile(0)..fixture::sfile(fixture::SCNR_SIZE)];
    let base = fixture::META + fixture::SCNR as u32;
    let mut tables = Tables::new();
    // `cut` and `cut_again` each play `arrive` (names match without case);
    // nothing plays the others.
    let first = tables.literal(40, "Arrive", 0);
    let second = tables.literal(40, "arrive", 0);
    let a = tables.call(0, "invented_play", 4, &[first]);
    let again = tables.call(0, "invented_play", 4, &[second]);
    tables.script("cut", 0, 4, a);
    tables.script("cut_again", 0, 4, again);
    tables.write(payload, base, 0xa00);
    hs::fixture::write_recordings(payload, base, 0xd00, &scripts::recordings());
    let map = t.0.join("played.map");
    std::fs::write(&map, b).unwrap();
    std::fs::write(t.0.join("default.xbe"), playing_exe().xbe()).unwrap();
    let mut app = open(map, &t);
    let mut api = Api::new();
    let v = call(&mut api, &mut app, m::RECORDING_LIST, json!({}));
    let by = &v["recordings"][0]["played_by"];
    assert_eq!(by.as_array().unwrap().len(), 2, "{v}");
    assert_eq!(by[0]["count"], 1);
    let v = call(&mut api, &mut app, m::SCRIPT_USES, json!({"kind": "recording", "name": "arrive"}));
    assert_eq!(v["uses"].as_array().unwrap().len(), 2, "{v}");
    let v = call(&mut api, &mut app, m::SCRIPT_USES, json!({"kind": "recording", "name": "empty"}));
    assert_eq!(v["uses"], json!([]));
    let e = api
        .call(&mut app, m::SCRIPT_USES, json!({"kind": "recording", "name": "absent"}))
        .unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
}

#[test]
fn without_the_executable_recordings_list_and_played_by_says_why() {
    let t = Temp::new("recordings-noexe");
    let mut app = open(scripts::scripted_source(&t.0, false), &t);
    let mut api = Api::new();
    let v = call(&mut api, &mut app, m::RECORDING_LIST, json!({}));
    assert_eq!(v["recordings"].as_array().unwrap().len(), 3);
    assert_eq!(v["recordings"][0]["played_by"], Value::Null);
    assert!(v["played_by_unavailable"].as_str().unwrap().contains("executable"), "{v}");
}
