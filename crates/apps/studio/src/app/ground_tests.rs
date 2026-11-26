//! AI ground in Studio on invented maps (docs/game/game-types-without-carriers.md,
//! "In Studio"): a label placed in the window and by `scenario.add` make
//! the same document; a build on the kit's invented court lists its
//! sectors; the stale rule; a failed build says why and goes to Problems.
//! No game data.
use std::path::PathBuf;

use firefight_kit::testkit;
use scenario_edit::{BlockKind, Command, FlagLabel, LabelKind, Value, fixture};
use serde_json::{Value as Json, json};
use studio_proto::method as m;

use super::ground::{GroundState, labels_of, make_for_tests};
use super::{Action, App};
use crate::api::Api;

const FLAGS: BlockKind = BlockKind::NetgameFlags;

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("h2-editor-ground-{tag}-{}", std::process::id()));
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

fn opened(t: &Temp) -> App {
    let map = t.0.join("example.map");
    std::fs::write(&map, fixture::fixture()).unwrap();
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(map));
    app.dispatch(Action::SetAllowGrowth(true));
    app
}

fn call(api: &mut Api, app: &mut App, method: &str, params: Json) -> Json {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

/// The window's click pushes `PlaceLabelAt`; `scenario.add` performs the
/// same action. The flag's bytes and its label are equal.
#[test]
fn a_label_placed_in_the_window_and_by_scenario_add_make_the_same_document() {
    let (ta, tb) = (Temp::new("same-a"), Temp::new("same-b"));
    let (mut window, mut agent) = (opened(&ta), opened(&tb));
    let at = [3.0, 4.0, 1.0];
    window.dispatch(Action::PlaceLabelAt {
        kind: LabelKind::Line,
        at,
        facing: 0.5,
        team: 1,
        phase: 2,
    });
    let mut api = Api::new();
    let added = call(
        &mut api,
        &mut agent,
        m::SCENARIO_ADD,
        json!({"block": "label", "label": "siege_line", "position": at, "facing": 0.5, "team": 1, "phase": 2}),
    );
    let index = added["index"].as_u64().unwrap() as usize;
    let (a, b) = (
        window.session.scenario().unwrap(),
        agent.session.scenario().unwrap(),
    );
    assert_eq!(a.flag_labels(), b.flag_labels());
    assert_eq!(
        a.flag_label(index),
        Some(FlagLabel {
            kind: LabelKind::Line,
            phase: 2
        })
    );
    assert_eq!(a.block(FLAGS).entry(index), b.block(FLAGS).entry(index));
    assert_eq!(a.get(FLAGS, index, "position").unwrap(), Value::Vec3(at));
    // The same labels read into the kit's, whichever way they came.
    assert_eq!(labels_of(a).unwrap(), labels_of(b).unwrap());
    assert_eq!(labels_of(a).unwrap().lines.len(), 1);
    // A label that is none of the six, and a team that is no team.
    let mut bad = |params| {
        api.call(&mut agent, m::SCENARIO_ADD, params)
            .unwrap_err()
            .code
    };
    assert_eq!(
        bad(json!({"block": "label", "label": "ff_nothing"})),
        studio_proto::code::INVALID_PARAMS
    );
    assert_eq!(
        bad(json!({"block": "label", "label": "ff_base", "team": 2})),
        studio_proto::code::INVALID_PARAMS
    );
}

/// The window's next team and phase follow Forge's: bases red then blue,
/// a siege line's phase one for each pair of sides.
#[test]
fn the_next_team_and_phase_follow_what_is_placed() {
    let t = Temp::new("next");
    let mut app = opened(&t);
    let next =
        |app: &App, kind| super::ground::next_team_phase(app.session.scenario().unwrap(), kind);
    assert_eq!(next(&app, LabelKind::Base), (0, 0));
    for (kind, want) in [
        (LabelKind::Line, [(0, 0), (1, 0), (0, 1), (1, 1)]),
        (LabelKind::Base, [(0, 0), (1, 0), (0, 0), (1, 0)]),
    ] {
        for expected in want {
            let (team, phase) = next(&app, kind);
            assert_eq!((team, if kind.has_phase() { phase } else { 0 }), expected);
            app.dispatch(Action::PlaceLabelAt {
                kind,
                at: [1.0, 1.0, 0.0],
                facing: 0.0,
                team,
                phase,
            });
        }
    }
}

/// A build on the kit's invented court lists its sectors, and they read
/// as the Navigation layers read them.
#[test]
fn a_build_on_the_invented_court_lists_its_sectors() {
    let labels = firefight_kit::Labels::default();
    let (built, pathfinding) = make_for_tests(&testkit::court(), testkit::NAME, &labels).unwrap();
    assert!(built.zones > 0 && built.squads > 0, "{built:?}");
    assert_eq!(built.sectors, pathfinding.sectors.len());
    assert!(built.sectors > 0, "the ground has sectors");
    assert!(built.costs.tag_growth <= built.costs.available);
}

/// The stale rule over a map that is open: a flag moved, a label placed
/// and a solid object moved, each as the rule says.
#[test]
fn an_edit_that_moves_collision_marks_the_ground_stale() {
    let t = Temp::new("stale");
    let mut app = opened(&t);
    assert_eq!(app.ground_state(), GroundState::Never);
    app.ground_pretend_built();
    assert_eq!(app.ground_state(), GroundState::Current);
    // A spawn moved touches no collision.
    app.session
        .apply(Command::set(
            BlockKind::StartingLocations,
            0,
            "position",
            Value::Vec3([2.0, 2.0, 2.0]),
        ))
        .unwrap();
    assert_eq!(app.ground_state(), GroundState::Current);
    // A label placed changes the labels.
    app.dispatch(Action::PlaceLabelAt {
        kind: LabelKind::Drop,
        at: [1.0, 1.0, 0.0],
        facing: 0.0,
        team: 0,
        phase: 0,
    });
    assert_eq!(
        app.ground_state(),
        GroundState::Stale(firefight_kit::ground::Stale::Labels)
    );
    app.ground_pretend_built();
    assert_eq!(app.ground_state(), GroundState::Current);
    // A scenery moved is collision.
    let Value::Vec3(p) = app
        .session
        .scenario()
        .unwrap()
        .get(BlockKind::Scenery, 0, "position")
        .unwrap()
    else {
        panic!()
    };
    app.session
        .apply(Command::set(
            BlockKind::Scenery,
            0,
            "position",
            Value::Vec3([p[0] + 3.0, p[1], p[2]]),
        ))
        .unwrap();
    assert_eq!(
        app.ground_state(),
        GroundState::Stale(firefight_kit::ground::Stale::Collision)
    );
    let mut api = Api::new();
    let status = call(&mut api, &mut app, m::GROUND_STATUS, Json::Null);
    assert_eq!(status["state"], "stale");
    assert_eq!(status["stale"], "collision");
    assert_eq!(status["labels"]["ff_drop"], 1);
}

/// The fixture is no kit map: the build fails, says why, and the reason is
/// a row in Problems. Nothing is built and nothing is shown.
#[test]
fn a_failed_build_says_why_and_goes_to_problems() {
    let t = Temp::new("failed");
    let mut app = opened(&t);
    let mut api = Api::new();
    let r = call(&mut api, &mut app, m::GROUND_BUILD, json!({"wait": true}));
    assert_eq!(r["job"], 1);
    assert_eq!(r["status"]["state"], "failed");
    assert!(
        r["status"]["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{r}"
    );
    assert_eq!(r["status"]["built"], Json::Null);
    assert_eq!(r["status"]["shown"], false);
    let problems = call(
        &mut api,
        &mut app,
        m::PROBLEMS_LIST,
        json!({"source": "ground"}),
    );
    assert_eq!(problems["problems"][0]["code"], "ground.build");
    assert_eq!(problems["problems"][0]["severity"], "error");
}

fn place(app: &mut App, kind: LabelKind, at: [f32; 3]) {
    let (team, phase) = super::ground::next_team_phase(app.session.scenario().unwrap(), kind);
    app.dispatch(Action::PlaceLabelAt {
        kind,
        at,
        facing: 0.25,
        team,
        phase,
    });
}

/// The ground file is the labels placed, drops first, in the build's own
/// order, and reads back as what was written.
#[test]
fn the_labels_placed_are_a_ground_file_in_the_builds_order() {
    use ::workspace::{GroundFile, LabelName, Side};
    let t = Temp::new("file");
    let mut app = opened(&t);
    place(&mut app, LabelKind::Line, [5.0, 5.0, 0.0]);
    place(&mut app, LabelKind::Base, [1.0, 1.0, 0.0]);
    place(&mut app, LabelKind::Base, [9.0, 9.0, 0.0]);
    place(&mut app, LabelKind::Drop, [3.0, 3.0, 0.0]);
    place(&mut app, LabelKind::Site, [4.0, 4.0, 0.0]);
    let labels = labels_of(app.session.scenario().unwrap()).unwrap();
    let file = super::ground::ground_file_of(&labels);
    let kinds: Vec<LabelName> = file.labels.iter().map(|l| l.label).collect();
    assert_eq!(
        kinds,
        [
            LabelName::FfDrop,
            LabelName::FfBase,
            LabelName::FfBase,
            LabelName::SiegeSite,
            LabelName::SiegeLine
        ]
    );
    assert_eq!(file.home, Side::Red);
    // Bases are red then blue, as the window places them.
    assert_eq!(file.labels[1].team, Some(Side::Red));
    assert_eq!(file.labels[2].team, Some(Side::Blue));
    assert_eq!(file.labels[0].facing, Some(0.25));
    assert_eq!(file.labels[4].phase, Some(0));
    file.validate().unwrap();
    let yaml = file.to_yaml().unwrap();
    assert_eq!(GroundFile::parse(yaml.as_bytes()).unwrap(), file);
    // The same labels in any order are the same file.
    let mut shuffled = labels.clone();
    shuffled.bases.reverse();
    assert_eq!(super::ground::ground_file_of(&shuffled), file);
}

/// `ground.save` writes a `map` item and its ground file to the edit
/// target, the new map takes the open map's edits, and a second save of the
/// same labels changes nothing.
#[test]
fn saving_the_ground_writes_a_map_item_to_the_edit_target() {
    use super::package_doc;
    use ::workspace::{GroundFile, Loaded, MapSpec};
    use content::package::{Kind, Maps};
    let t = Temp::new("save");
    let mut app = opened(&t);
    let mut api = Api::new();
    let map = app.session.map_name().unwrap().to_owned();
    // No target: NO_TARGET, and nothing is written.
    let none = api.call(&mut app, m::GROUND_SAVE, json!({})).unwrap_err();
    assert_eq!(none.code, studio_proto::code::NO_TARGET);
    let data = app.data_root();
    package_doc::new(&data, "mine", "Mine").unwrap();
    package_doc::edit_target(&data, Some("mine"), false).unwrap();
    let edits = "- tag: 'weap:invented\\other'\n  set:\n  - path: speed\n    value: 1.0\n";
    package_doc::put_edits(&data, "mine", &map, edits).unwrap();

    place(&mut app, LabelKind::Drop, [3.0, 3.0, 0.0]);
    place(&mut app, LabelKind::Base, [1.0, 1.0, 0.0]);
    let saved = call(&mut api, &mut app, m::GROUND_SAVE, json!({"name": "court-ai"}));
    assert_eq!(saved["package"], "mine");
    assert_eq!(saved["name"], "court-ai");
    assert_eq!(saved["labels"], 2);
    assert_eq!(saved["changed"], true);
    assert_eq!(saved["file"], "maps/court-ai/ground.yaml");

    let dir = data.join("packages/mine");
    let spec = MapSpec::parse(&std::fs::read(dir.join("maps/court-ai/map.toml")).unwrap()).unwrap();
    assert_eq!(spec.name, "court-ai");
    assert_eq!(spec.base, map);
    assert_eq!(spec.ground.as_deref(), Some("maps/court-ai/ground.yaml"));
    let file =
        GroundFile::parse(&std::fs::read(dir.join("maps/court-ai/ground.yaml")).unwrap()).unwrap();
    assert_eq!(file.labels.len(), 2);
    assert_eq!(
        file,
        super::ground::ground_file_of(&labels_of(app.session.scenario().unwrap()).unwrap())
    );

    // The package reads, checks, and the map takes what the open map's
    // edits change.
    let loaded = Loaded::from_dir(&dir).unwrap();
    let items = &loaded.package.manifest.items;
    let at = saved["item"].as_u64().unwrap() as usize;
    let found = ::workspace::check_package(&loaded);
    assert!(found.iter().all(|f| f.item != Some(at)), "{found:?}");
    assert_eq!(items[at].kind, Kind::Map);
    let taken = items.iter().find(|i| i.kind == Kind::Edits).unwrap();
    let Maps::Names(names) = &taken.maps else {
        panic!("{:?}", taken.maps)
    };
    assert!(names.contains(&map) && names.iter().any(|n| n == "court-ai"), "{names:?}");

    // Saved again with the same labels: nothing changes, and no second item.
    let again = call(&mut api, &mut app, m::GROUND_SAVE, json!({"name": "court-ai"}));
    assert_eq!(again["changed"], false);
    assert_eq!(again["item"], saved["item"]);
    let loaded = Loaded::from_dir(&dir).unwrap();
    assert_eq!(
        loaded.package.manifest.items.iter().filter(|i| i.kind == Kind::Map).count(),
        1
    );
    // A moved label is a changed save of the same item.
    place(&mut app, LabelKind::Site, [4.0, 4.0, 0.0]);
    let moved = call(&mut api, &mut app, m::GROUND_SAVE, json!({"name": "court-ai"}));
    assert_eq!((moved["changed"].as_bool(), moved["item"].clone()), (Some(true), saved["item"].clone()));

    // The name must be a map's and not the open map's.
    for name in ["bad name", map.as_str()] {
        let e = api
            .call(&mut app, m::GROUND_SAVE, json!({"name": name}))
            .unwrap_err();
        assert_eq!(e.code, studio_proto::code::REFUSED, "{name}");
    }
}
