//! `playtest.static` over the invented map with sectors
//! (`fixture::fixture_with_pathfinding`: the room at `ROOM_AT`, walled at
//! its low and high x) and an invented bots' mesh: two floor halves
//! walked between, and an island past the room's far wall
//! (docs/editor/playtest.md, "Static checks", slice PT1).

use super::*;

/// The bots' cache for the fixture map: polygon 0 the room's low-x half,
/// polygon 1 its high-x half, linked both ways; polygon 2 a 5 by 5 square
/// 10 units past the far wall, linked to nothing. An empty spots file so
/// nothing is scanned.
fn invented_mesh(data: &Path, map_bytes: &[u8]) {
    use bots::nav::{Edge, Kind, NavMesh, Poly};
    let dir = bots::cache::dir(data);
    std::fs::create_dir_all(&dir).unwrap();
    let fingerprint = bots::autonav::fingerprint(&map_bytes[..bots::autonav::MAP_HEADER_LEN]);
    let [x, y, _] = fixture::ROOM_AT;
    let quad = |x0: f32, y0: f32, w: f32, h: f32| {
        Poly::new(vec![
            [x + x0, y + y0, 0.0],
            [x + x0 + w, y + y0, 0.0],
            [x + x0 + w, y + y0 + h, 0.0],
            [x + x0, y + y0 + h, 0.0],
        ])
    };
    let edge = |from, to| Edge {
        from,
        to,
        a: [x + 5.0, y, 0.0],
        b: [x + 5.0, y + 10.0, 0.0],
        kind: Kind::Walk,
    };
    let nav = NavMesh::new(
        vec![
            quad(0.0, 0.0, 5.0, 10.0),
            quad(5.0, 0.0, 5.0, 10.0),
            quad(20.0, 0.0, 5.0, 5.0),
        ],
        vec![edge(0, 1), edge(1, 0)],
    );
    let file = bots::cache::MapData {
        nav,
        walls: bots::los::Walls::new(Vec::new()),
        places: Default::default(),
    };
    std::fs::write(
        dir.join(bots::cache::file_name("fixture", 0)),
        file.to_bytes(bots::cache::Stamp::now(fingerprint)),
    )
    .unwrap();
    bots::spots::write(&dir, "fixture", bots::spots::stamp(fingerprint), &[]).unwrap();
}

fn set(api: &mut Api, app: &mut App, index: usize, field: &str, value: Value) {
    call(
        api,
        app,
        m::SCENARIO_SET,
        json!({"block": "starting_locations", "index": index, "field": field, "value": value}),
    );
}

fn of_kind(r: &Value, kind: &str) -> Vec<Value> {
    r["problems"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["kind"] == kind)
        .cloned()
        .collect()
}

fn ready(r: &Value, game_type: &str) -> Value {
    r["ready"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["game_type"] == game_type)
        .unwrap_or_else(|| panic!("no {game_type} in {r}"))
        .clone()
}

fn targets(problems: &[Value]) -> Vec<Value> {
    problems.iter().map(|p| p["target"].clone()).collect()
}

/// An off-mesh spawn, an island, two spawns in sight and a missing flag
/// are each found; moving a start onto the island makes it unreachable,
/// and the room's wall hides it from the other.
#[test]
fn playtest_static_finds_each_problem_on_the_invented_map() {
    let t = Temp::new("playtest-static");
    let mut app = app(&t);
    let mut api = Api::new();
    let bytes = fixture::fixture_with_pathfinding();
    invented_mesh(&t.path("data"), &bytes);
    open_map(&mut api, &mut app, &t, "playtest.map", bytes);
    let [x, y, _] = fixture::ROOM_AT;

    // Start 0 on the low half; start 1 where the fixture has it, far off
    // the mesh. Both are team 1.
    set(
        &mut api,
        &mut app,
        0,
        "position",
        json!([x + 2.0, y + 5.0, 0.0]),
    );
    let r = call(
        &mut api,
        &mut app,
        m::PLAYTEST_STATIC,
        json!({"wait": true}),
    );
    assert_eq!(r["nav"], "ready", "{r}");
    assert_eq!(r["ok"], false);
    assert_eq!(
        targets(&of_kind(&r, "off_mesh")),
        [
            json!({"block": "starting_locations", "index": 1}),
            json!({"block": "netgame_equipment", "index": 0}),
            json!({"block": "netgame_flags", "index": 0}),
        ],
        "{r}"
    );
    let islands = of_kind(&r, "island");
    assert_eq!(
        targets(&islands),
        [json!({"kind": "poly", "index": 2})],
        "{r}"
    );
    assert!(
        islands[0]["text"]
            .as_str()
            .unwrap()
            .contains("25.0 square units")
    );
    assert!(of_kind(&r, "in_sight").is_empty(), "one team: {r}");
    let slayer = ready(&r, "Slayer");
    assert_eq!(slayer["ok"], false, "one start on the mesh: {r}");
    // The fixture's one flag is an Assault bomb spawn: no CTF flag.
    let ctf = ready(&r, "Capture the Flag");
    assert_eq!(ctf["ok"], false);
    assert!(
        ctf["missing"]
            .as_array()
            .unwrap()
            .contains(&json!("a ctf_flag_spawn flag for team 0")),
        "{ctf}"
    );

    // Start 1 on the high half, on team 0: both on the mesh, in sight.
    set(
        &mut api,
        &mut app,
        1,
        "position",
        json!([x + 8.0, y + 5.0, 0.0]),
    );
    set(&mut api, &mut app, 1, "team", json!(0));
    let r = call(
        &mut api,
        &mut app,
        m::PLAYTEST_STATIC,
        json!({"wait": true}),
    );
    let off = targets(&of_kind(&r, "off_mesh"));
    assert!(
        !off.contains(&json!({"block": "starting_locations", "index": 1})),
        "{r}"
    );
    let seen = of_kind(&r, "in_sight");
    assert_eq!(
        targets(&seen),
        [json!({"block": "starting_locations", "index": 0})],
        "{r}"
    );
    assert!(seen[0]["text"].as_str().unwrap().contains("0 and 1"), "{r}");
    assert!(of_kind(&r, "unreachable").is_empty(), "{r}");
    assert_eq!(ready(&r, "Slayer")["ok"], true, "{r}");
    assert_eq!(ready(&r, "Capture the Flag")["ok"], false);

    // Start 1 on the island: neither start reaches the other, the island
    // is reached, and the far wall hides one from the other.
    set(
        &mut api,
        &mut app,
        1,
        "position",
        json!([x + 22.0, y + 2.0, 0.0]),
    );
    let r = call(
        &mut api,
        &mut app,
        m::PLAYTEST_STATIC,
        json!({"wait": true}),
    );
    assert_eq!(
        targets(&of_kind(&r, "unreachable")),
        [
            json!({"block": "starting_locations", "index": 0}),
            json!({"block": "starting_locations", "index": 1}),
        ],
        "{r}"
    );
    assert!(of_kind(&r, "island").is_empty(), "{r}");
    assert!(of_kind(&r, "in_sight").is_empty(), "the wall: {r}");
}

/// With no map open there is nothing to check.
#[test]
fn playtest_static_needs_a_map() {
    let mut app = App::new();
    let mut api = Api::new();
    assert_eq!(
        code_of(api.call(&mut app, m::PLAYTEST_STATIC, json!({}))),
        code::NO_DOCUMENT
    );
}

// ---- playtest.run and its readers, over a fake sim (slice PT4) ----

use crate::app::playtest::fake::Fake;

fn provider(t: &Temp) -> (crate::api::playtest::Playtest, Fake) {
    let fake = Fake::new(t.path("playtest"));
    (
        crate::api::playtest::Playtest::with_source(Box::new(fake.clone())),
        fake,
    )
}

fn ask(
    p: &mut crate::api::playtest::Playtest,
    app: &mut App,
    method: &str,
    params: Value,
) -> Reply {
    use crate::api::Provider;
    p.call(app, method, &params)
        .expect("the provider answers it")
}

fn ok(p: &mut crate::api::playtest::Playtest, app: &mut App, method: &str, params: Value) -> Value {
    ask(p, app, method, params).unwrap_or_else(|e| panic!("{method} {params}: {e}"))
}

/// The invented map with both starts on the bots' mesh, so Slayer is
/// ready.
fn ready_map(t: &Temp, api: &mut Api, app: &mut App) {
    let bytes = fixture::fixture_with_pathfinding();
    invented_mesh(&t.path("data"), &bytes);
    open_map(api, app, t, "playtest.map", bytes);
    let [x, y, _] = fixture::ROOM_AT;
    set(api, app, 0, "position", json!([x + 2.0, y + 5.0, 0.0]));
    set(api, app, 1, "position", json!([x + 8.0, y + 5.0, 0.0]));
    set(api, app, 1, "team", json!(0));
}

/// Without a runner the call is refused; the readers say there is nothing.
#[test]
fn playtest_run_is_refused_with_no_sim_runner() {
    let t = Temp::new("playtest-no-runner");
    let mut app = app(&t);
    let mut api = Api::new();
    assert_eq!(
        code_of(api.call(&mut app, m::PLAYTEST_RUN, json!({"preset": "probe"}))),
        code::REFUSED
    );
    let s = call(&mut api, &mut app, m::PLAYTEST_STATUS, json!({}));
    assert_eq!(
        (s["state"].clone(), s["run"].clone()),
        (json!("none"), Value::Null)
    );
    assert_eq!(
        call(&mut api, &mut app, m::PLAYTEST_RESULT, json!({}))["state"],
        "none"
    );
    assert_eq!(
        call(&mut api, &mut app, m::PLAYTEST_CANCEL, json!({})),
        json!({"cancelled": false})
    );
}

/// A run, its progress, its partial and whole result, a repeat from the
/// cache and a cancel.
#[test]
fn playtest_run_status_result_and_cancel_over_a_fake_sim() {
    let t = Temp::new("playtest-run");
    let mut app = app(&t);
    let mut api = Api::new();
    ready_map(&t, &mut api, &mut app);
    let (mut p, fake) = provider(&t);

    // Bad calls start nothing.
    for params in [
        json!({"preset": "sprint"}),
        json!({"seeds": 0}),
        json!({"bots": 99}),
        json!({"game_types": "slayer"}),
        json!({"minutes": -1}),
    ] {
        let e = ask(&mut p, &mut app, m::PLAYTEST_RUN, params.clone()).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{params}");
    }
    let e = ask(
        &mut p,
        &mut app,
        m::PLAYTEST_RUN,
        json!({"game_types": ["ctf"]}),
    )
    .unwrap_err();
    assert_eq!(e.code, code::FAILED, "Capture the Flag is not ready: {e}");
    assert!(fake.w().batches.is_empty());

    let r = ok(
        &mut p,
        &mut app,
        m::PLAYTEST_RUN,
        json!({"preset": "probe", "seeds": 2}),
    );
    assert_eq!(
        r,
        json!({"run": "r1", "build": "b1", "matches": 2, "parallel": 2, "cached": false})
    );
    let s = ok(&mut p, &mut app, m::PLAYTEST_STATUS, json!({}));
    assert_eq!(
        (s["state"].clone(), s["done"].clone(), s["total"].clone()),
        (json!("running"), json!(0), json!(2))
    );
    let nothing = ok(&mut p, &mut app, m::PLAYTEST_RESULT, json!({}));
    assert_eq!(
        (nothing["traces"].clone(), nothing["partial"].clone()),
        (json!(0), json!(true))
    );

    // One match ends: its heat shows, its other measures are refused.
    let jobs = fake.jobs();
    fake.end(&jobs[0]);
    fake.fail(&jobs[1], "the guest faulted");
    let s = ok(&mut p, &mut app, m::PLAYTEST_STATUS, json!({"run": "r1"}));
    assert_eq!(s["done"], 2);
    assert_eq!(s["state"], "done");
    assert_eq!(s["coarse"], true);
    assert_eq!(
        s["failed"],
        json!([{"game_type": "slayer", "seed": 2, "swapped": false, "why": "the guest faulted"}])
    );
    let r = ok(&mut p, &mut app, m::PLAYTEST_RESULT, json!({}));
    assert_eq!(
        (
            r["game_type"].clone(),
            r["traces"].clone(),
            r["partial"].clone()
        ),
        (json!("slayer"), json!(1), json!(false))
    );
    assert_eq!(r["seeds"], json!([1, 2]));
    assert!(
        r["match_file"].as_str().unwrap().ends_with("r1/match.json"),
        "{r}"
    );
    assert_eq!(r["measures"]["heat"], json!({"ok": true}));
    assert_eq!(r["measures"]["spawns"]["ok"], false);
    assert_eq!(r["measures"]["spawns"]["refused"]["need"], 8);
    let heat = ok(
        &mut p,
        &mut app,
        m::PLAYTEST_RESULT,
        json!({"measure": "heat"}),
    );
    assert_eq!(heat["ok"], true);
    assert!(
        heat["value"]["deaths"]
            .as_array()
            .is_some_and(|d| !d.is_empty()),
        "{heat}"
    );
    let spawns = ok(
        &mut p,
        &mut app,
        m::PLAYTEST_RESULT,
        json!({"measure": "spawns"}),
    );
    assert_eq!(spawns["ok"], false);
    assert_eq!(spawns["refused"]["what"], "spawns");
    for bad in [
        json!({"measure": "nope"}),
        json!({"game_type": "ctf"}),
        json!({"run": "r9"}),
    ] {
        let e = ask(&mut p, &mut app, m::PLAYTEST_RESULT, bad.clone()).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{bad}");
    }

    // A run that had a failure plays again; one that did not reads the
    // cache.
    let again = ok(&mut p, &mut app, m::PLAYTEST_RUN, json!({"seeds": 1}));
    assert_eq!(again["cached"], false);
    fake.end_all();
    ok(&mut p, &mut app, m::PLAYTEST_STATUS, json!({}));
    let hit = ok(&mut p, &mut app, m::PLAYTEST_RUN, json!({"seeds": 1}));
    assert_eq!(
        (hit["run"].clone(), hit["cached"].clone()),
        (again["run"].clone(), json!(true))
    );

    // A cancel stops a running one.
    let third = ok(&mut p, &mut app, m::PLAYTEST_RUN, json!({"seeds": 3}));
    assert_eq!(
        ok(&mut p, &mut app, m::PLAYTEST_CANCEL, json!({})),
        json!({"cancelled": true})
    );
    let s = ok(
        &mut p,
        &mut app,
        m::PLAYTEST_STATUS,
        json!({"run": third["run"].clone()}),
    );
    assert_eq!(s["state"], "cancelled");
    assert_eq!(
        ok(&mut p, &mut app, m::PLAYTEST_CANCEL, json!({})),
        json!({"cancelled": false})
    );
}

/// With no game type ready the call says what each lacks.
#[test]
fn playtest_run_names_what_a_map_lacks() {
    let t = Temp::new("playtest-lacks");
    let mut app = app(&t);
    let mut api = Api::new();
    let bytes = fixture::fixture_with_pathfinding();
    invented_mesh(&t.path("data"), &bytes);
    open_map(&mut api, &mut app, &t, "playtest.map", bytes);
    let (mut p, fake) = provider(&t);
    let e = ask(&mut p, &mut app, m::PLAYTEST_RUN, json!({})).unwrap_err();
    assert_eq!(e.code, code::FAILED);
    assert!(
        e.message.contains("no game type is ready") && e.message.contains("Slayer lacks"),
        "{}",
        e.message
    );
    assert!(fake.w().batches.is_empty());
    assert_eq!(fake.w().builds, 0);
}
