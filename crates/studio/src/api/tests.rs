//! The agent API over the scenario crate's invented map, the playlist
//! crate's sample file and the fallback variant schema (no game data).
//! Outputs go to the system temp directory, never the checkout.

use std::path::{Path, PathBuf};

use ::variants::save::{CONTENT_TYPES, SaveStore, XbeNames};
use scenario_edit::{BlockKind, fixture};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::*;
use crate::app::{Action, PlaylistEdit};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An app with the invented map open, a variant store, and the sample
/// playlist file at hand (not open).
fn app(t: &Temp) -> App {
    let map = t.path("example.map");
    std::fs::write(&map, fixture::fixture()).unwrap();
    std::fs::write(
        t.path("multiplayer_hopper.dat"),
        hopper_edit::fixture::sample().build(),
    )
    .unwrap();
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    let names = XbeNames {
        file_names: (0..CONTENT_TYPES).map(|i| format!("t{i}")).collect(),
        labels: (0..CONTENT_TYPES).map(|i| format!("L{i}")).collect(),
    };
    // The fallback schema (no disc) has no built-in variants: give it one.
    let mut schema = ::variants::Schema::fallback();
    schema.builtins.push(::variants::schema::Builtin {
        name: "Slayer".into(),
        game_type: ::variants::GameType::Slayer,
        description: 0,
        pairs: Vec::new(),
    });
    app.dispatch(Action::OpenSource(map));
    assert!(app.session.is_open());
    // After the source: opening one replaces a variant editor made for
    // another (App::ensure_variants).
    app.set_variant_editor(::variants::VariantEditor::new(
        schema,
        Some(SaveStore::new(t.path("saves"), Some(names))),
    ));
    app
}

fn call(api: &mut Api, app: &mut App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

fn code_of(r: Reply) -> i64 {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e.code,
    }
}

fn flag_position(app: &App) -> [f32; 3] {
    app.session
        .scenario()
        .unwrap()
        .get(BlockKind::NetgameFlags, 0, "position")
        .unwrap()
        .as_vec3()
        .unwrap()
}

/// The example params of `d` with the test's paths and values in.
fn example(d: &MethodDef, t: &Temp, app: &mut App) -> Value {
    let mut text = d.example.to_owned();
    let out = t.path("out");
    std::fs::create_dir_all(&out).unwrap();
    text = text.replace("$MAP", &t.path("example.map").display().to_string());
    text = text.replace("$OUT", &out.display().to_string());
    if text.contains("$SCREEN") {
        let first = crate::app::UiScreens::tree()[0].0;
        text = text.replace("$SCREEN", first);
    }
    if text.contains("$SETTING") {
        // A setting of the open variant and another value for it.
        app.ensure_variants();
        let e = app.variants.as_ref().unwrap();
        let v = e.current().unwrap();
        // The fallback schema (no disc) knows no choices: any value goes,
        // so flip one between 0 and 1.
        let known: Vec<_> = v
            .known_settings()
            .into_iter()
            .filter(|(id, _)| ::variants::schema::applies(*id, v.game_type()))
            .collect();
        let (id, other) = known
            .iter()
            .find_map(|&(id, now)| {
                let d = e.schema().settings.get(&id)?;
                let other = d.choices.iter().find(|c| c.value != now)?.value;
                Some((id, other))
            })
            .or_else(|| {
                known
                    .iter()
                    .find(|(id, _)| {
                        e.schema()
                            .settings
                            .get(id)
                            .is_some_and(|d| d.choices.is_empty())
                    })
                    .map(|&(id, now)| (id, i32::from(now == 0)))
            })
            .expect("a setting the variant can change");
        text = text
            .replace("\"$SETTING\"", &id.0.to_string())
            .replace("\"$VALUE\"", &other.to_string());
    }
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: example {text}: {e}", d.name))
}

/// What a method's example needs before it runs.
fn setup(api: &mut Api, app: &mut App, t: &Temp, name: &str) {
    let mut c = |m: &str, p: Value| {
        call(api, app, m, p);
    };
    let new_variant = json!({"variant": {"new": "slayer"}, "discard": true});
    match name {
        m::DOC_SAVE => c(
            m::DOC_SAVE_AS,
            json!({"doc": "map", "path": t.path("out/first.map").display().to_string()}),
        ),
        m::SCENARIO_ADD | m::SCENARIO_DUPLICATE | m::TAG_BLOCK_ADD | m::TAG_BLOCK_DUPLICATE => {
            // Every block of the invented map is full (the editor does
            // not grow blocks): make room in the starting locations, which
            // each example adds to.
            c(
                m::SCENARIO_DELETE,
                json!({"block": "starting_locations", "index": 1}),
            );
        }
        m::DOC_CLOSE | m::PLAYLIST_LIST | m::PLAYLIST_GET | m::PLAYLIST_SET | m::PLAYLIST_ENTRY => {
            c(
                m::DOC_OPEN,
                json!({"playlists": t.path("multiplayer_hopper.dat").display().to_string()}),
            )
        }
        m::PLAYLIST_ENTRY_VARIANT => {
            c(
                m::DOC_OPEN,
                json!({"playlists": t.path("multiplayer_hopper.dat").display().to_string()}),
            );
            c(m::DOC_OPEN, new_variant);
        }
        m::VARIANT_GET | m::VARIANT_SET | m::VARIANT_RENAME | m::VARIANT_EXPORT => {
            c(m::DOC_OPEN, new_variant)
        }
        m::VARIANT_COPY => c(m::DOC_OPEN, json!({"variant": {"builtin": 0}})),
        m::VARIANT_DELETE => {
            c(m::DOC_OPEN, new_variant);
            c(m::DOC_SAVE, json!({"doc": "variant"}));
        }
        m::VARIANT_IMPORT => {
            c(m::DOC_OPEN, new_variant);
            c(
                m::VARIANT_EXPORT,
                json!({"path": t.path("out/v.h2variant").display().to_string()}),
            );
        }
        m::UI_PRESS | m::UI_WIDGETS | m::UI_RENDER => {
            let first = crate::app::UiScreens::tree()[0].0;
            c(m::UI_PICK, json!({"screen": first}));
        }
        m::DOC_UNDO => c(
            m::SCENARIO_MOVE,
            json!({"block": "netgame_flags", "index": 0, "position": [1, 2, 3]}),
        ),
        m::DOC_REDO => {
            c(
                m::SCENARIO_MOVE,
                json!({"block": "netgame_flags", "index": 0, "position": [1, 2, 3]}),
            );
            c(m::DOC_UNDO, json!({"doc": "map"}));
        }
        n if n.starts_with("hud.") => {
            // The HUD fixture's map as the source (it has no scenario).
            let map = t.path("hudtest.map");
            std::fs::write(&map, hud_edit::fixture::build().0).unwrap();
            app.dispatch(Action::OpenSource(map));
            if n != m::HUD_LIST {
                call(api, app, m::HUD_LIST, json!({"map": "hudtest"}));
            }
        }
        _ => {}
    }
}

#[test]
fn every_method_in_methods_has_a_test() {
    // Each method runs its own example from the table on a fresh app:
    // built ones answer, planned ones answer NOT_YET with their lane.
    let mut ran = Vec::new();
    for d in Api::new().methods() {
        let t = Temp::new("each");
        let mut app = app(&t);
        let mut api = Api::new();
        setup(&mut api, &mut app, &t, d.name);
        let params = example(&d, &t, &mut app);
        let r = api.call(&mut app, d.name, params.clone());
        match (d.planned, r) {
            (Some(lane), Err(e)) => {
                assert_eq!(e.code, code::NOT_YET, "{}: {e}", d.name);
                assert_eq!(e.data.unwrap()["lane"], lane);
            }
            (Some(_), Ok(v)) => panic!("{} is planned but answered {v}", d.name),
            // No renderer on this machine: the snapshot says so.
            (None, Err(e)) if d.name == m::VIEW_SNAPSHOT && e.code == code::NO_GPU => {}
            (None, Err(e)) => panic!("{} {params}: {e}", d.name),
            (None, Ok(v)) => assert!(!v.is_null() || d.name == m::METHODS, "{}", d.name),
        }
        ran.push(d.name);
    }
    // Every method name the protocol crate has is in the table.
    let names: Vec<&str> = METHODS.iter().map(|d| d.name).collect();
    for n in &names {
        assert_eq!(
            names.iter().filter(|x| *x == n).count(),
            1,
            "{n} listed twice"
        );
    }
    assert_eq!(ran.len(), Api::new().methods().len());
}

#[test]
fn methods_describe_their_params_results_and_examples() {
    let t = Temp::new("methods");
    let mut app = app(&t);
    let mut api = Api::new();
    let list = call(&mut api, &mut app, m::METHODS, Value::Null);
    let list = list.as_array().unwrap();
    // The table with the providers' methods over and after it.
    let defs = api.methods();
    assert_eq!(list.len(), defs.len());
    for (d, v) in defs.iter().zip(list) {
        assert_eq!(v["name"], d.name);
        assert!(
            v["params"].is_object(),
            "{}: params shape {}",
            d.name,
            d.params
        );
        assert!(
            !v["result"].is_string(),
            "{}: result shape {}",
            d.name,
            d.result
        );
        assert!(!d.summary.is_empty());
        assert_eq!(
            v["status"],
            if d.planned.is_some() {
                "planned"
            } else {
                "ready"
            }
        );
        // The example names only params the shape has.
        if let Value::Object(o) = &v["example"] {
            let keys = shape_keys(d.params);
            for k in o.keys() {
                assert!(
                    keys.iter().any(|(n, _)| n == k),
                    "{}: example param {k}",
                    d.name
                );
            }
        }
    }
    let planned: Vec<&str> = list
        .iter()
        .filter(|v| v["status"] == "planned")
        .map(|v| v["name"].as_str().unwrap().split('.').next().unwrap())
        .collect();
    for ns in ["model", "custom"] {
        assert!(planned.contains(&ns), "{ns}.* is a planned namespace");
    }
    // The HUD tool's provider answers all of hud.*.
    assert!(!planned.contains(&"hud"));
    for name in [m::HUD_LIST, m::HUD_READ, m::HUD_SET, m::HUD_MOVE, m::HUD_RENDER, m::HUD_CHECK, m::HUD_SAVE_AS] {
        assert!(list.iter().any(|v| v["name"] == name && v["status"] == "ready"), "{name}");
    }
}

#[test]
fn params_are_checked_before_anything_runs() {
    let t = Temp::new("params");
    let mut app = app(&t);
    let mut api = Api::new();
    let before = flag_position(&app);
    let set = |p: Value| (m::SCENARIO_SET, p);
    for (method, p) in [
        set(
            json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [1, 2, 3], "typo": 1}),
        ),
        set(json!({"block": "netgame_flags", "index": 0, "field": "position"})),
        set(json!({"block": "nope", "index": 0, "field": "position", "value": [1, 2, 3]})),
        set(json!({"block": "netgame_flags", "index": 9, "field": "position", "value": [1, 2, 3]})),
        set(json!({"block": "netgame_flags", "index": 0, "field": "nope", "value": 1})),
        set(json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [1, 2]})),
        set(json!({"block": "netgame_flags", "index": 0, "field": "position", "value": "up"})),
        (m::SCENARIO_GET, json!([1, 2])),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, method, p.clone())),
            code::INVALID_PARAMS,
            "{p}"
        );
    }
    assert_eq!(
        code_of(api.call(&mut app, "scenario.nope", Value::Null)),
        code::METHOD_NOT_FOUND
    );
    assert_eq!(flag_position(&app), before, "nothing was applied");
    assert!(!app.is_dirty());
}

#[test]
fn growth_where_the_map_editor_cannot_grow_safely_is_refused_with_a_reason() {
    let t = Temp::new("growth");
    let mut app = app(&t);
    let mut api = Api::new();
    let e = api
        .call(&mut app, m::SCENARIO_ADD, json!({"block": "netgame_flags"}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("full"), "{}", e.message);
    assert_eq!(e.data.unwrap(), json!({"count": 1, "capacity": 1}));
    let e = api
        .call(
            &mut app,
            m::TAG_BLOCK_ADD,
            json!({"tag": "scnr", "path": "netgame_flags"}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(!app.is_dirty());
    // With room made, the same add goes in.
    call(
        &mut api,
        &mut app,
        m::SCENARIO_DELETE,
        json!({"block": "netgame_flags", "index": 0}),
    );
    let v = call(
        &mut api,
        &mut app,
        m::SCENARIO_ADD,
        json!({"block": "netgame_flags", "position": [9, 9, 9]}),
    );
    assert_eq!(v["index"], 0);
    assert_eq!(flag_position(&app), [9.0, 9.0, 9.0]);
}

#[test]
fn unsaved_edits_are_never_dropped_without_discard() {
    let t = Temp::new("unsaved");
    let mut app = app(&t);
    let mut api = Api::new();
    call(
        &mut api,
        &mut app,
        m::SCENARIO_MOVE,
        json!({"block": "netgame_flags", "index": 0, "position": [1, 1, 1]}),
    );
    let path = t.path("example.map").display().to_string();
    assert_eq!(
        code_of(api.call(&mut app, m::SOURCE_OPEN, json!({"path": path}))),
        code::UNSAVED
    );
    assert_eq!(
        code_of(api.call(&mut app, m::DOC_CLOSE, json!({"doc": "map"}))),
        code::UNSAVED
    );
    assert!(
        app.is_dirty() && app.pending().is_none(),
        "refused, not held for a prompt"
    );
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": path, "discard": true}),
    );
    assert!(!app.is_dirty());
}

#[test]
fn outputs_stay_out_of_git_and_off_the_source() {
    let t = Temp::new("git");
    let mut app = app(&t);
    let mut api = Api::new();
    call(
        &mut api,
        &mut app,
        m::SCENARIO_MOVE,
        json!({"block": "netgame_flags", "index": 0, "position": [1, 1, 1]}),
    );
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("never.map");
    let e = api
        .call(
            &mut app,
            m::DOC_SAVE_AS,
            json!({"doc": "map", "path": repo.display().to_string()}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{}", e.message);
    assert!(!repo.exists());
    let src = t.path("example.map").display().to_string();
    assert_eq!(
        code_of(api.call(&mut app, m::DOC_SAVE_AS, json!({"doc": "map", "path": src}))),
        code::REFUSED
    );
    assert_eq!(
        std::fs::read(t.path("example.map")).unwrap(),
        fixture::fixture()
    );
    // Saving in place needs a file of its own first.
    assert_eq!(
        code_of(api.call(&mut app, m::DOC_SAVE, json!({"doc": "map"}))),
        code::REFUSED
    );
    let first = crate::app::UiScreens::tree()[0].0;
    call(&mut api, &mut app, m::UI_PICK, json!({"screen": first}));
    let png = Path::new(env!("CARGO_MANIFEST_DIR")).join("never.png");
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::UI_RENDER,
            json!({"out": png.display().to_string()})
        )),
        code::REFUSED
    );
    assert!(!png.exists());
}

#[test]
fn undo_and_redo_through_the_api_for_each_document() {
    let t = Temp::new("undo");
    let mut app = app(&t);
    let mut api = Api::new();
    let before = flag_position(&app);
    call(
        &mut api,
        &mut app,
        m::SCENARIO_SET,
        json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [4, 5, 6]}),
    );
    // The variant tool is shown now: undo still names the map.
    call(
        &mut api,
        &mut app,
        m::DOC_OPEN,
        json!({"variant": {"new": "slayer"}}),
    );
    let v = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(v["done"], true);
    assert_eq!(flag_position(&app), before);
    let v = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(v["done"], false, "nothing left");
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "map"}));
    assert_eq!(flag_position(&app), [4.0, 5.0, 6.0]);

    call(
        &mut api,
        &mut app,
        m::VARIANT_RENAME,
        json!({"name": "Renamed"}),
    );
    let name = |app: &App| app.variants.as_ref().unwrap().current().unwrap().name();
    assert_eq!(name(&app), "Renamed");
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "variant"}));
    assert_ne!(name(&app), "Renamed");
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "variant"}));
    assert_eq!(name(&app), "Renamed");

    let hop = t.path("multiplayer_hopper.dat").display().to_string();
    call(&mut api, &mut app, m::DOC_OPEN, json!({"playlists": hop}));
    call(
        &mut api,
        &mut app,
        m::PLAYLIST_SET,
        json!({"playlist": 0, "title": "Agent"}),
    );
    let title = |app: &App| app.hoppers.hoppers().unwrap()[0].title.clone();
    assert_eq!(title(&app), "Agent");
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "playlists"}));
    assert_ne!(title(&app), "Agent");
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "playlists"}));
    assert_eq!(title(&app), "Agent");
    let docs = call(&mut api, &mut app, m::DOC_LIST, Value::Null);
    let dirty: Vec<&str> = docs
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["dirty"] == true)
        .map(|d| d["doc"].as_str().unwrap())
        .collect();
    assert_eq!(dirty, ["map", "variant", "playlists"]);
}

#[test]
fn the_window_and_the_api_make_identical_documents() {
    // The inspector's path: a dragged field (merged under one key), then
    // the end of the drag, as dispatched actions.
    let t = Temp::new("same");
    let mut ui = app(&t);
    ui.dispatch(Action::SetField {
        block: BlockKind::NetgameFlags,
        index: 0,
        field: "position".into(),
        value: scenario_edit::Value::Vec3([2.0, 2.0, 2.0]),
        merge: Some(7),
    });
    ui.dispatch(Action::SetField {
        block: BlockKind::NetgameFlags,
        index: 0,
        field: "position".into(),
        value: scenario_edit::Value::Vec3([4.0, 5.0, 6.0]),
        merge: Some(7),
    });
    ui.dispatch(Action::SealEdit);
    // The API's path.
    let t2 = Temp::new("same-api");
    let mut agent = app(&t2);
    let mut api = Api::new();
    call(
        &mut api,
        &mut agent,
        m::SCENARIO_SET,
        json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [4, 5, 6]}),
    );
    let (a, b) = (
        ui.session.scenario().unwrap(),
        agent.session.scenario().unwrap(),
    );
    assert_eq!(a.changes(), b.changes());
    assert!(!a.changes().is_empty());
    // One undo step each.
    ui.dispatch(Action::Undo);
    call(&mut api, &mut agent, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(flag_position(&ui), flag_position(&agent));
    ui.dispatch(Action::Redo);
    call(&mut api, &mut agent, m::DOC_REDO, json!({"doc": "map"}));
    let (pa, pb) = (t.path("ui.map"), t2.path("api.map"));
    ui.dispatch(Action::SaveMapAs(pa.clone()));
    call(
        &mut api,
        &mut agent,
        m::DOC_SAVE_AS,
        json!({"doc": "map", "path": pb.display().to_string()}),
    );
    assert_eq!(std::fs::read(pa).unwrap(), std::fs::read(pb).unwrap());

    // Playlists: the table's row buttons apply the same edits.
    let hop = |t: &Temp| t.path("multiplayer_hopper.dat");
    ui.dispatch(Action::OpenHopper(hop(&t)));
    let edits = [
        PlaylistEdit::Weight {
            playlist: 0,
            entry: 0,
            weight: 9,
        },
        PlaylistEdit::Duplicate {
            playlist: 0,
            entry: 0,
        },
        PlaylistEdit::Move {
            playlist: 0,
            entry: 1,
            to: 0,
        },
    ];
    for e in &edits {
        // As entries_table does inside its frame's edit.
        ui.hoppers.edit(|hs| e.apply(hs).unwrap());
    }
    call(
        &mut api,
        &mut agent,
        m::DOC_OPEN,
        json!({"playlists": hop(&t2).display().to_string()}),
    );
    call(
        &mut api,
        &mut agent,
        m::PLAYLIST_ENTRY,
        json!({"playlist": 0, "op": "weight", "entry": 0, "weight": 9}),
    );
    call(
        &mut api,
        &mut agent,
        m::PLAYLIST_ENTRY,
        json!({"playlist": 0, "op": "duplicate", "entry": 0}),
    );
    call(
        &mut api,
        &mut agent,
        m::PLAYLIST_ENTRY,
        json!({"playlist": 0, "op": "move", "entry": 1, "to": 0}),
    );
    assert_eq!(ui.hoppers.hoppers(), agent.hoppers.hoppers());
}

#[test]
fn tags_list_read_and_follow_references() {
    let t = Temp::new("tags");
    let mut app = app(&t);
    let mut api = Api::new();
    let all = call(&mut api, &mut app, m::TAG_LIST, Value::Null);
    assert_eq!(all["total"], 4);
    let scnr = call(&mut api, &mut app, m::TAG_LIST, json!({"group": "scnr"}));
    assert_eq!(scnr["tags"][0]["name"], "levels\\test\\fixture");
    let raw = |t: Value| json!({"tag": t, "raw": true});
    let byname = call(&mut api, &mut app, m::TAG_READ, raw(json!("levels\\test\\fixture")));
    let bygroup = call(&mut api, &mut app, m::TAG_READ, raw(json!("scnr")));
    let byid = call(&mut api, &mut app, m::TAG_READ, raw(scnr["tags"][0]["id"].clone()));
    assert_eq!(byname, bygroup);
    assert_eq!(byname, byid);
    assert_eq!(byname["layout"], "schema");
    assert!(byname["root"]["fields"].as_array().unwrap().len() > 3);
    // The weapon palette names the blaster.
    let refs = call(&mut api, &mut app, m::TAG_REFS, json!({"tag": "scnr"}));
    let blaster = refs
        .as_array()
        .unwrap()
        .iter()
        .position(|r| r["name"] == "objects\\test\\blaster")
        .expect("the palette's reference");
    let to = call(&mut api, &mut app, m::TAG_FOLLOW, json!({"tag": "scnr", "ref": blaster}));
    assert_eq!(to["group"], "weap");
    let path = refs[blaster]["path"].clone();
    assert_eq!(call(&mut api, &mut app, m::TAG_FOLLOW, json!({"tag": "scnr", "ref": path})), to);

    // Any tag's fields through the Tags tool's registry and commands.
    let kit = call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc:objects\\test\\kit"}));
    assert_eq!(kit["access"], "edit", "{kit}");
    assert_eq!(kit, call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"})));
    let v = call(&mut api, &mut app, m::TAG_SET, json!({"tag": "itmc", "path": "+0x8", "value": 7}));
    assert_eq!(v["value"], 7);
    assert!(app.is_dirty(), "a tag edit is the map's to save");
    let diff = call(&mut api, &mut app, m::DOC_DIFF, json!({"doc": "tags"}));
    assert_eq!(diff["patches"][0]["tag"], "itmc:objects\\test\\kit", "{diff}");
    assert_eq!(diff["patches"][0]["offset"], "0x8");
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "tags"}));
    assert!(!app.is_dirty());
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "tags"}));
    assert_eq!(call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"}))["edited"], true);
    // A tag too short for its group's layout is refused with the reason.
    let e = api
        .call(&mut app, m::TAG_SET, json!({"tag": "weap", "path": "+0x4", "value": 1}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{}", e.message);
    assert!(e.message.contains("shorter than"), "{}", e.message);
    // The scenario's placements still edit as scenario.set.
    call(&mut api, &mut app, m::TAG_SET, json!({"tag": "scnr", "path": "netgame_flags[0].position", "value": [7, 7, 7]}));
    assert_eq!(flag_position(&app), [7.0, 7.0, 7.0]);
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_READ, json!({"tag": "nope"}))),
        code::INVALID_PARAMS
    );
}

struct Models;
impl Provider for Models {
    fn methods(&self) -> Vec<MethodDef> {
        vec![MethodDef {
            name: m::MODEL_LIST,
            summary: "test models",
            params: "{}",
            result: "[string]",
            example: "null",
            planned: None,
        }]
    }
    fn call(&mut self, _: &mut App, method: &str, _: &Value) -> Option<Reply> {
        (method == m::MODEL_LIST).then(|| Ok(json!(["masterchief"])))
    }
}

#[test]
fn a_registered_provider_answers_a_planned_method() {
    let t = Temp::new("provider");
    let mut app = app(&t);
    let mut api = Api::new();
    assert_eq!(
        code_of(api.call(&mut app, m::MODEL_LIST, Value::Null)),
        code::NOT_YET
    );
    api.register(Box::new(Models));
    assert_eq!(
        call(&mut api, &mut app, m::MODEL_LIST, Value::Null),
        json!(["masterchief"])
    );
    let listed = call(&mut api, &mut app, m::METHODS, Value::Null);
    let model = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == m::MODEL_LIST)
        .unwrap();
    assert_eq!(model["status"], "ready");
    // The rest of the namespace stays planned.
    assert_eq!(
        code_of(api.call(&mut app, m::MODEL_READ, json!({"tag": "x"}))),
        code::NOT_YET
    );
}

/// A headless editor on a socket, as `h2-editor --serve` runs: the app
/// lives on its own thread, which answers the queue.
fn serve(
    sock: PathBuf,
    map: PathBuf,
    data: PathBuf,
) -> (server::Listener, std::thread::JoinHandle<()>) {
    let (listener, rx) = server::listen(&sock).unwrap();
    let h = std::thread::spawn(move || {
        let mut app = App::new();
        app.set_data_root(data);
        let _ = map;
        let mut api = Api::new();
        for inc in rx.iter() {
            let r = api.handle(&mut app, inc.request.clone());
            inc.answer(r);
        }
    });
    (listener, h)
}

#[test]
fn a_headless_editor_opens_sets_saves_and_patches_only_those_bytes() {
    let t = Temp::new("e2e");
    let map = t.path("example.map");
    std::fs::write(&map, fixture::fixture()).unwrap();
    let sock = t.path("s.sock");
    let (listener, _h) = serve(sock.clone(), map.clone(), t.path("data"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&sock).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the socket is the user's alone");
    }
    let mut c = studio_proto::cli::connect(&sock).unwrap();
    c.set_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let status = c.call(m::STATUS, Value::Null).unwrap();
    assert_eq!(status["window"], false);
    c.call(m::SOURCE_OPEN, json!({"path": map.display().to_string()}))
        .unwrap();
    let got = c
        .call(m::SCENARIO_SET, json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [4.5, -2, 8]}))
        .unwrap();
    assert_eq!(got["value"], json!([4.5, -2.0, 8.0]));
    let diff = c.call(m::DOC_DIFF, json!({"doc": "map"})).unwrap();
    let patches = diff["patches"].as_array().unwrap();
    assert!(!patches.is_empty(), "{diff}");
    for p in patches {
        assert_eq!(p["block"], "netgame_flags", "{diff}");
        assert_eq!(p["fields"], json!(["position"]), "{diff}");
    }
    let out = t.path("patched.map");
    let saved = c
        .call(
            m::DOC_SAVE_AS,
            json!({"doc": "map", "path": out.display().to_string()}),
        )
        .unwrap();
    assert_eq!(saved["bytes"], fixture::fixture().len());
    // Errors come back as errors, on the same connection.
    let e = c
        .call(m::SCENARIO_GET, json!({"block": "nope", "index": 0}))
        .unwrap_err();
    assert!(
        matches!(e, agent_proto::CallError::Rpc(ref e) if e.code == code::INVALID_PARAMS),
        "{e}"
    );
    drop(c);

    let (a, b) = (fixture::fixture(), std::fs::read(&out).unwrap());
    assert_eq!(a.len(), b.len());
    let flag = fixture::sfile(fixture::FLAGS_AT);
    let changed: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    let body: Vec<usize> = changed.iter().copied().filter(|&i| i >= 0x800).collect();
    let mut want = Vec::new();
    for (k, v) in [4.5f32, -2.0, 8.0].iter().enumerate() {
        let old = 3.0f32.to_le_bytes();
        for (j, (x, y)) in v.to_le_bytes().iter().zip(old).enumerate() {
            if *x != y {
                want.push(flag + 4 * k + j);
            }
        }
    }
    assert_eq!(body, want, "only the position's bytes past the header");
    // In the header, at most the checksum word.
    let header: Vec<usize> = changed.iter().copied().filter(|&i| i < 0x800).collect();
    assert!(
        header.len() <= 4 && header.windows(2).all(|w| w[1] - w[0] < 4),
        "{header:?}"
    );

    // A notification gets no answer; a bad line gets a parse error.
    #[cfg(unix)]
    {
        use std::io::{BufRead, Write};
        let mut s = std::os::unix::net::UnixStream::connect(&sock).unwrap();
        s.write_all(b"{\"method\":\"session.status\"}\nnot json\n")
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(s).read_line(&mut line).unwrap();
        let r: agent_proto::Response = serde_json::from_str(&line).unwrap();
        assert_eq!(r.error.unwrap().code, code::PARSE);
    }
    drop(listener);
    assert!(!sock.exists(), "the socket file goes with the listener");
}

#[test]
fn the_socket_is_local_only_and_one_editor_per_path() {
    let t = Temp::new("sock");
    assert!(server::listen(Path::new("tcp://127.0.0.1:4721")).is_err());
    let sock = t.path("one.sock");
    let (_l, _rx) = server::listen(&sock).unwrap();
    let e = server::listen(&sock)
        .err()
        .expect("a second listener is refused");
    assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse);
    let file = t.path("plain");
    std::fs::write(&file, "x").unwrap();
    assert!(
        server::listen(&file).is_err(),
        "a file that isn't a socket stays"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
}

#[test]
fn a_window_answers_between_frames() {
    // The window's loop: pump drains the queue on the app it draws.
    let t = Temp::new("pump");
    let mut app = app(&t);
    let mut api = Api::new();
    api.window = true;
    let sock = t.path("w.sock");
    let (_l, rx) = server::listen(&sock).unwrap();
    let client = std::thread::spawn(move || {
        let mut c = studio_proto::cli::connect(&sock).unwrap();
        c.call(
            m::SCENARIO_MOVE,
            json!({"block": "netgame_flags", "index": 0, "position": [3, 2, 1]}),
        )
        .unwrap();
        c.call(m::STATUS, Value::Null).unwrap()
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !client.is_finished() {
        api.pump(&mut app, &rx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let status = client.join().unwrap();
    assert_eq!(status["window"], true);
    assert_eq!(
        flag_position(&app),
        [3.0, 2.0, 1.0],
        "the window's own document changed"
    );
    assert_eq!(
        app.log().last().unwrap().status.text(),
        "moved NetgameFlags 0"
    );
}

/// An app with `hud_edit`'s invented map as the source, open in the HUD
/// tool through the API (no game data).
fn hud_app(t: &Temp) -> (Api, App) {
    let map = t.path("hudtest.map");
    std::fs::write(&map, hud_edit::fixture::build().0).unwrap();
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    app.dispatch(Action::OpenSource(map));
    let mut api = Api::new();
    let list = call(&mut api, &mut app, m::HUD_LIST, json!({"map": "hudtest"}));
    assert_eq!(list["huds"].as_array().unwrap().len(), 2, "{list}");
    assert_eq!(list["current"], "invented\\hud\\player");
    (api, app)
}

/// The fixture's stray bitmap: full-screen offset (40, -48), flagged by
/// the checks.
const STRAY: hud_edit::WidgetKey = hud_edit::WidgetKey {
    kind: hud_edit::WidgetKind::Bitmap,
    index: 2,
};

/// The stray's entry in a hud.read.
fn stray_of(read: &Value) -> Value {
    read["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["widget"] == "bitmap:2")
        .expect("the stray")
        .clone()
}

#[test]
fn hud_read_check_move_set_render_and_save_through_the_hud_tool() {
    let t = Temp::new("hud");
    let (mut api, mut app) = hud_app(&t);
    let read = call(&mut api, &mut app, m::HUD_READ, Value::Null);
    let stray = stray_of(&read);
    assert_eq!((stray["fields"]["x0"].clone(), stray["fields"]["y0"].clone()), (json!(40), json!(-48)));
    assert!(stray["placed"].is_object(), "{stray}");
    assert!(read["fields"]["bitmap"].as_array().unwrap().iter().any(|f| f["key"] == "anchor"));
    // The checks name the stray.
    let found = call(&mut api, &mut app, m::HUD_CHECK, Value::Null);
    assert!(
        found.as_array().unwrap().iter().any(|f| f["widgets"]
            .as_array()
            .unwrap()
            .contains(&json!({"hud": 0, "widget": "bitmap:2"}))),
        "{found}"
    );
    // A move is the HUD tool's undo step and patch.
    let moved = call(&mut api, &mut app, m::HUD_MOVE, json!({"widget": "bitmap:2", "x": 60, "y": -48}));
    assert_eq!(moved["offset"], json!([60, -48]));
    assert_eq!(app.hud.offset(STRAY), Some((60, -48)));
    assert!(app.tool_dirty(crate::ToolKind::Hud));
    let diff = call(&mut api, &mut app, m::DOC_DIFF, json!({"doc": "hud"}));
    assert_eq!(diff["patches"][0]["widget"], "bitmap:2", "{diff}");
    assert_eq!(diff["patches"][0]["fields"], json!(["x0"]));
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "hud"}));
    assert_eq!(app.hud.offset(STRAY), Some((40, -48)));
    assert!(!app.tool_dirty(crate::ToolKind::Hud));
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "hud"}));
    assert_eq!(app.hud.offset(STRAY), Some((60, -48)));
    // Another mode's offset: the state's mode follows.
    call(&mut api, &mut app, m::HUD_MOVE, json!({"widget": "bitmap:2", "x": 21, "y": -24, "mode": "half"}));
    assert_eq!(app.hud.state.mode, hud_edit::Mode::Half);
    let half = call(&mut api, &mut app, m::HUD_READ, Value::Null);
    assert_eq!(half["state"]["mode"], "half");
    assert_eq!(stray_of(&half)["fields"]["x1"], 21, "{half}");
    // Fields by key; string IDs are the Tags tool's; unknown keys say so.
    let set = call(&mut api, &mut app, m::HUD_SET, json!({"widget": "bitmap:2", "field": "anchor", "value": 3}));
    assert_eq!(set["value"], 3);
    assert_eq!(
        app.hud.session.as_ref().unwrap().hud().unwrap().widget(STRAY).unwrap().anchor(),
        3
    );
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_SET, json!({"widget": "bitmap:2", "field": "name", "value": 1}))),
        code::REFUSED
    );
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_SET, json!({"widget": "bitmap:2", "field": "nope", "value": 1}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_MOVE, json!({"widget": "bitmap:99", "x": 0, "y": 0}))),
        code::INVALID_PARAMS
    );
    // Opening another map would drop the edits: refused without discard.
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_LIST, json!({"map": "hudtest"}))),
        code::UNSAVED
    );
    // The picture, never into the checkout.
    let png = t.path("out/hud.png");
    let r = call(&mut api, &mut app, m::HUD_RENDER, json!({"out": png.display().to_string(), "state": {"mode": "full", "shield": 0.5}}));
    assert_eq!(r["width"], 640);
    assert!(png.exists());
    assert_eq!(app.hud.state.shield, 0.5);
    let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("hud.png");
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_RENDER, json!({"out": inside.display().to_string()}))),
        code::REFUSED
    );
    assert!(!inside.exists());
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_RENDER, json!({"state": {"shield": 2}}))),
        code::INVALID_PARAMS
    );
    // The save: never over the source; the saved map has the edits.
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_SAVE_AS, json!({"path": t.path("hudtest.map").display().to_string()}))),
        code::REFUSED
    );
    let out = t.path("out/hud.map");
    call(&mut api, &mut app, m::HUD_SAVE_AS, json!({"path": out.display().to_string()}));
    assert!(!app.tool_dirty(crate::ToolKind::Hud));
    // (No scenario in it, so the scenario tool's open logs an error.)
    app.dispatch(Action::OpenSource(out));
    call(&mut api, &mut app, m::HUD_LIST, json!({"map": "hud"}));
    let back = call(&mut api, &mut app, m::HUD_READ, json!({"state": {"mode": "full"}}));
    assert_eq!(stray_of(&back)["fields"]["x0"], 60);
    assert_eq!(stray_of(&back)["fields"]["anchor"], 3);
}

#[test]
fn the_hud_window_and_the_api_make_identical_documents() {
    let t = Temp::new("hud-ui");
    let (_, mut ui) = hud_app(&t);
    // The window's drag: three moves merged, then a field.
    for (i, x) in [41, 45, 60].into_iter().enumerate() {
        ui.dispatch(Action::HudMove(STRAY, (x, -48), i > 0));
    }
    ui.dispatch(Action::HudSet(STRAY, "anchor".into(), hud_edit::Value::Int(3), false));
    let t2 = Temp::new("hud-api");
    let (mut api, mut agent) = hud_app(&t2);
    call(&mut api, &mut agent, m::HUD_MOVE, json!({"widget": "bitmap:2", "x": 60, "y": -48}));
    call(&mut api, &mut agent, m::HUD_SET, json!({"widget": "bitmap:2", "field": "anchor", "value": 3}));
    let patches = |a: &App| a.hud.session.as_ref().unwrap().patches();
    assert_eq!(patches(&ui), patches(&agent));
    assert_eq!(
        ui.hud.session.as_ref().unwrap().history().count(),
        agent.hud.session.as_ref().unwrap().history().count()
    );
}
