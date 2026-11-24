//! `markers.list` and `markers.render` over the invented cache (no game
//! data): what each placement block gives, a hill of corners, the
//! volumes, and the sheet of every kind.

use super::*;

/// The `kind` names of a list's rows.
fn kinds(v: &Value) -> Vec<String> {
    v["markers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["kind"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn markers_list_gives_each_placement_block_its_kind_and_team() {
    let t = Temp::new("markers-list");
    let mut app = app(&t);
    let mut api = Api::new();
    let all = call(&mut api, &mut app, m::MARKERS_LIST, json!({}));
    // Two starting locations, a flag of type 2 (a bomb spawn) and one
    // netgame item naming an item collection, all on team 1.
    assert_eq!(all["counts"], json!({"spawn": 2, "bomb_spawn": 1, "weapon_spawn": 1}));
    let spawns = call(&mut api, &mut app, m::MARKERS_LIST, json!({"kind": "spawn"}));
    let rows = spawns["markers"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    for (i, r) in rows.iter().enumerate() {
        assert_eq!(r["id"], format!("starting_locations[{i}]"));
        assert_eq!(r["layer"], "spawn_points");
        assert_eq!(r["team"], 1);
        assert_eq!(r["area"], Value::Null);
        assert!(r["lines"].as_u64().unwrap() > 0 && r["triangles"].as_u64().unwrap() > 0);
        // The placed mesh's box holds the placement.
        let at: Vec<f64> = r["position"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap()).collect();
        for k in 0..3 {
            let min = r["bounds"]["min"][k].as_f64().unwrap();
            let max = r["bounds"]["max"][k].as_f64().unwrap();
            // The pad lies a hair over the floor the placement is on.
            assert!(min <= at[k] + 0.05 && at[k] <= max + 1e-3, "axis {k}: {min} {at:?} {max}");
        }
    }
    // The first spawn faces half a radian from +x.
    let facing = rows[0]["facing"].as_array().unwrap();
    assert!((facing[0].as_f64().unwrap() - 0.5_f64.cos()).abs() < 1e-5);
    let flag = call(&mut api, &mut app, m::MARKERS_LIST, json!({"layer": "flags"}));
    assert_eq!(kinds(&flag), ["bomb_spawn"]);
    assert_eq!(flag["markers"][0]["id"], "netgame_flags[0]");
    assert_eq!(flag["markers"][0]["team"], 1);
    let items = call(&mut api, &mut app, m::MARKERS_LIST, json!({"layer": "netgame_items"}));
    assert_eq!(kinds(&items), ["weapon_spawn"]);
    assert_eq!(items["markers"][0]["team"], Value::Null);
    assert_eq!(
        kinds(&call(&mut api, &mut app, m::MARKERS_LIST, json!({"kind": "hill"}))),
        Vec::<String>::new()
    );
    for bad in [json!({"kind": "nope"}), json!({"layer": "structure"})] {
        assert_eq!(code_of(api.call(&mut app, m::MARKERS_LIST, bad)), code::INVALID_PARAMS);
    }
}

#[test]
fn markers_list_has_a_box_a_layer_each_volume_kind() {
    let t = Temp::new("markers-volumes");
    let mut app = app(&t);
    let mut api = Api::new();
    open_map(&mut api, &mut app, &t, "volumes.map", fixture::fixture_with_volumes());
    let mut rows = |layer: &str| {
        call(&mut api, &mut app, m::MARKERS_LIST, json!({"layer": layer}))["markers"]
            .as_array()
            .unwrap()
            .clone()
    };
    // Volume 0 is a trigger and a BSP switch; volume 1 a trigger and a
    // kill trigger.
    assert_eq!(rows("trigger_volumes").len(), 2);
    assert_eq!(rows("kill_volumes").len(), 1);
    assert_eq!(rows("bsp_switches").len(), 1);
    let kill = rows("kill_volumes")[0].clone();
    assert_eq!((kill["id"].clone(), kill["kind"].clone()), (json!("volume[1]"), json!("kill_volume")));
    // The box is as long as the volume, along the turned axis.
    let (lo, hi) = (&kill["bounds"]["min"], &kill["bounds"]["max"]);
    assert!((hi[1].as_f64().unwrap() - lo[1].as_f64().unwrap() - 6.0).abs() < 1e-3);
    assert!((hi[0].as_f64().unwrap() - lo[0].as_f64().unwrap() - 2.0).abs() < 1e-3);
}

#[test]
fn a_hills_corners_list_as_one_hill() {
    let t = Temp::new("markers-hill");
    let mut app = app(&t);
    let mut api = Api::new();
    open_map(&mut api, &mut app, &t, "hill.map", fixture::fixture_at_end());
    app.session.set_allow_growth(true);
    for (field, value) in [("type", 11), ("identifier", 1)] {
        call(&mut api, &mut app, m::SCENARIO_SET, json!({
            "block": "netgame_flags", "index": 0, "field": field, "value": value
        }));
    }
    for at in [[10, 0, 1], [10, 6, 1]] {
        call(&mut api, &mut app, m::SCENARIO_ADD, json!({
            "block": "netgame_flags", "like": 0, "position": at
        }));
    }
    let hills = call(&mut api, &mut app, m::MARKERS_LIST, json!({"kind": "hill"}));
    let rows = hills["markers"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "{hills}");
    let hill = &rows[0];
    assert_eq!(hill["members"], json!(["netgame_flags[0]", "netgame_flags[1]", "netgame_flags[2]"]));
    assert_eq!(hill["area_from"], "corners");
    assert!(hill["area"]["path"]["points"].as_array().unwrap().len() >= 3);
    assert_eq!(hill["number"], 1);
}

/// `markers.render` refuses what it cannot draw before it asks for a GPU.
#[test]
fn markers_render_refuses_by_name() {
    let t = Temp::new("markers-refusals");
    let mut app = app(&t);
    let mut api = Api::new();
    let out = t.path("x.png").display().to_string();
    for (params, why) in [
        (json!({"kind": "nope", "out": out}), "kind"),
        (json!({"team": 8, "out": out}), "team"),
        (json!({"view": "side", "out": out}), "view"),
        (json!({"kind": "spawn", "area": {"radius": 3}, "out": out}), "area"),
        (json!({"kind": "hill", "area": {"radius": 100}, "out": out}), "area"),
        (json!({"kind": "hill", "area": {"path": [[0, 0], [1, 1], [2, 2]]}, "out": out}), "area"),
        (json!({"kind": "hill", "area": {}, "out": out}), "area"),
        (json!({"size": [0, 5], "out": out}), "size"),
    ] {
        let e = api.call(&mut app, m::MARKERS_RENDER, params.clone()).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS, "{params}");
        assert!(e.message.starts_with(why), "{params}: {}", e.message);
    }
    // The area's sentence is `Area::validate`'s.
    let e = api
        .call(&mut app, m::MARKERS_RENDER, json!({"kind": "hill", "area": {"radius": 100}, "out": out}))
        .unwrap_err();
    assert!(e.message.contains("radius"), "{}", e.message);
    let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("never-markers.png");
    assert_eq!(
        code_of(api.call(&mut app, m::MARKERS_RENDER, json!({"out": inside}))),
        code::REFUSED
    );
    assert!(!inside.exists());
}

/// The contact sheet draws every kind of `Kind::samples`, and a changed
/// area changes the picture. Needs a GPU.
#[test]
fn markers_render_writes_the_sheet_with_every_kind() {
    let t = Temp::new("markers-sheet");
    let mut app = app(&t);
    let mut api = Api::new();
    let sheet = t.path("sheet.png");
    let v = match api.call(&mut app, m::MARKERS_RENDER, json!({"out": sheet, "size": [640, 360]})) {
        Err(e) if e.code == code::NO_GPU => return,
        Err(e) => panic!("markers.render: {e}"),
        Ok(v) => v,
    };
    assert_eq!((v["width"].clone(), v["height"].clone()), (json!(640), json!(360)));
    assert!(v["covered"].as_u64().unwrap() > 1000, "{v}");
    let drawn: std::collections::BTreeSet<&str> =
        v["kinds"].as_array().unwrap().iter().map(|k| k["kind"].as_str().unwrap()).collect();
    for kind in scenario_edit::marker_view::game_markers::Kind::samples() {
        assert!(drawn.contains(kind.name()), "{} is not on the sheet", kind.name());
    }
    // A kind with a team colour is drawn in red, blue and neutral.
    let spawns: Vec<_> = v["kinds"].as_array().unwrap().iter().filter(|k| k["kind"] == "spawn").collect();
    assert_eq!(spawns.iter().map(|k| k["team"].clone()).collect::<Vec<_>>(), [json!(0), json!(1), Value::Null]);
    assert!(spawns.iter().all(|k| k["lines"].as_u64().unwrap() == 38));
    assert!(sheet.exists());

    // One kind, one team: the picture is the hill's, and a wider radius
    // is another picture.
    let render = |api: &mut Api, app: &mut App, name: &str, area: Value| {
        let out = t.path(name);
        let mut params = json!({"kind": "hill", "team": 1, "out": out, "size": [96, 96], "view": "top"});
        if !area.is_null() {
            params["area"] = area;
        }
        call(api, app, m::MARKERS_RENDER, params);
        png_pixels(&out)
    };
    let one = render(&mut api, &mut app, "one.png", Value::Null);
    let wide = render(&mut api, &mut app, "wide.png", json!({"radius": 2.5}));
    assert!(one != wide, "the area did not change the picture");
}

/// A spawn changes the snapshot where its mesh projects, and nowhere else.
#[test]
fn a_spawn_changes_the_snapshot_where_its_mesh_projects() {
    let t = Temp::new("markers-snapshot");
    let mut app = app(&t);
    let mut api = Api::new();
    let size = [128u32, 96];
    let params = json!({
        "out": t.path("view.png"), "size": size,
        "camera": {"target": [-0.5, -1.5, -2.5], "distance": 6, "yaw": 40, "pitch": 35}
    });
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"preset": "gameplay"}));
    match api.call(&mut app, m::VIEW_SNAPSHOT, params.clone()) {
        Err(e) if e.code == code::NO_GPU => return,
        Err(e) => panic!("view.snapshot: {e}"),
        Ok(_) => {}
    }
    let shown = png_pixels(&t.path("view.png"));
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"set": {"spawn_points": false}}));
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params);
    let hidden = png_pixels(&t.path("view.png"));
    // The screen box of both spawns' meshes, from `markers.list`'s bounds.
    let camera = scenario_edit::geometry::Camera {
        target: [-0.5, -1.5, -2.5],
        distance: 6.0,
        yaw: 40f32.to_radians(),
        pitch: 35f32.to_radians(),
        ..Default::default()
    };
    let list = call(&mut api, &mut app, m::MARKERS_LIST, json!({"kind": "spawn"}));
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for r in list["markers"].as_array().unwrap() {
        let (min, max) = (&r["bounds"]["min"], &r["bounds"]["max"]);
        for corner in 0..8 {
            let p = std::array::from_fn(|k| {
                let side = if corner >> k & 1 == 0 { min } else { max };
                side[k].as_f64().unwrap() as f32
            });
            let [u, v] = camera.project(p, size[0] as f32 / size[1] as f32).expect("in front");
            for (k, s) in [u * size[0] as f32, v * size[1] as f32].into_iter().enumerate() {
                lo[k] = lo[k].min(s);
                hi[k] = hi[k].max(s);
            }
        }
    }
    let mut changed = 0;
    for y in 0..size[1] as usize {
        for x in 0..size[0] as usize {
            let at = (y * size[0] as usize + x) * 4;
            if shown[at..at + 3] != hidden[at..at + 3] {
                changed += 1;
                let (fx, fy) = (x as f32, y as f32);
                assert!(
                    fx >= lo[0] - 2.0 && fx <= hi[0] + 2.0 && fy >= lo[1] - 2.0 && fy <= hi[1] + 2.0,
                    "pixel {x},{y} changed outside the spawns' box {lo:?} {hi:?}"
                );
            }
        }
    }
    assert!(changed > 20, "the spawns drew {changed} pixels");
}
