use forge::{BlockKind, GroundLabel, OpId, TagKey};
use game_markers::LINE_ALPHA;

use super::*;

static GUARD: Mutex<()> = Mutex::new(());

fn frame(position: [f32; 3], yaw: f32) -> forge::Frame {
    forge::Frame {
        position,
        rotation: [yaw, 0., 0.],
    }
}

fn thing(
    block: BlockKind,
    index: u32,
    kind: ThingKind,
    fields: Vec<(&'static str, Value)>,
    at: [f32; 3],
) -> Thing {
    Thing {
        id: Endpoint::base(block, index),
        block,
        kind,
        own: true,
        frame: frame(at, 0.5),
        fields,
        changed: false,
    }
}

fn flag(index: u32, ty: i64, team: i64, at: [f32; 3]) -> Thing {
    let kind = if ty == 7 || ty == 8 {
        ThingKind::Teleporter
    } else {
        ThingKind::Goal
    };
    thing(
        BlockKind::NetgameFlags,
        index,
        kind,
        vec![("type", Value::Int(ty)), ("team", Value::Int(team))],
        at,
    )
}

fn spawn(index: u32, team: i64, at: [f32; 3]) -> Thing {
    thing(
        BlockKind::StartingLocations,
        index,
        ThingKind::StartingPoint,
        vec![("team", Value::Int(team))],
        at,
    )
}

fn spawner(index: u32, group: &str, path: &str) -> Thing {
    let key = TagKey {
        group: group.into(),
        path: path.into(),
    };
    thing(
        BlockKind::NetgameEquipment,
        index,
        ThingKind::ItemSpawner,
        vec![
            ("game type 1", Value::Int(12)),
            ("item", Value::Tag(Some(key))),
        ],
        [0.; 3],
    )
}

fn facts<'a>(things: &'a [Thing]) -> Facts<'a> {
    Facts {
        monitor: true,
        things,
        ground: &[],
        problems: &[],
        game: None,
        show: Show::ThisGameType,
    }
}

fn names(entries: &[Entry]) -> Vec<&'static str> {
    entries.iter().map(|e| e.marker.kind.name()).collect()
}

#[test]
fn every_thing_gives_its_marker_with_the_monitor_on_and_none_with_it_off() {
    let things = [
        spawn(0, 1, [1., 2., 0.]),
        flag(0, 0, 0, [3., 0., 0.]),
        flag(1, 7, 8, [4., 0., 0.]),
        flag(2, 8, 8, [5., 0., 0.]),
        // A type no reader traced draws nothing.
        flag(3, 5, 0, [6., 0., 0.]),
        spawner(0, "weap", "weapons\\pistol"),
        spawner(1, "vehc", "vehicles\\ghost"),
        spawner(2, "itmc", "items\\grenades"),
        thing(
            BlockKind::Scenery,
            0,
            ThingKind::Object,
            vec![],
            [9., 9., 0.],
        ),
    ];
    let mut f = facts(&things);
    let got = entries(&f);
    assert_eq!(
        names(&got),
        [
            "spawn",
            "flag_stand",
            "teleporter_sender",
            "teleporter_receiver",
            "weapon_spawn",
            "vehicle_spawn",
            "equipment_spawn",
        ]
    );
    // Each carries its place, facing and team.
    assert_eq!(got[0].place.at, [1., 2., 0.]);
    assert_eq!(got[0].marker.team, Some(1));
    assert_eq!(got[0].label, "Blue spawn");
    assert_eq!(got[0].name, "starting_locations:0");
    assert_eq!(got[2].marker.team, None, "neutral has no team");
    assert!(got[0].place.forward[0] > 0.8, "facing 0.5 rad");
    f.monitor = false;
    assert!(entries(&f).is_empty());
}

#[test]
fn a_thing_of_another_game_type_is_left_out_or_faded() {
    let things = [spawn(0, 0, [0.; 3]), flag(0, 0, 0, [1., 0., 0.])];
    // Engine 2 is Slayer: a CTF flag spawn is not its own.
    let mut f = facts(&things);
    f.game = Some(2);
    assert_eq!(names(&entries(&f)), ["spawn"]);
    f.show = Show::AllGameTypes;
    let all = entries(&f);
    assert_eq!(names(&all), ["spawn", "flag_stand"]);
    assert!(!all[0].other_game && all[1].other_game);
}

#[test]
fn a_hills_corners_are_one_hill_and_a_lone_corner_is_a_hill() {
    let things = [
        flag(0, 11, 0, [0., 0., 0.]),
        flag(1, 11, 0, [4., 0., 0.]),
        flag(2, 11, 0, [4., 4., 1.]),
        flag(3, 11, 0, [0., 4., 0.]),
        flag(4, 13, 1, [20., 0., 0.]),
    ];
    let got = entries(&facts(&things));
    assert_eq!(names(&got), ["hill", "hill"]);
    assert_eq!(got[0].label, "Hill 1");
    assert_eq!(got[1].label, "Hill 3");
    // Four corners: the mean of the hull, the lowest corner's height.
    assert_eq!(got[0].place.at, [2., 2., 0.]);
    assert_eq!(got[0].name, "netgame_flags:0");
    assert!(
        got[0]
            .marker
            .kind
            .area()
            .is_some_and(|a| a.outline().len() == 4)
    );
    // One corner: the default area at the flag.
    assert_eq!(got[1].place.at, [20., 0., 0.]);
    assert_eq!(got[1].marker.team, Some(1));
    assert_eq!(got[1].marker.kind.area(), Some(&Area::default()));
}

fn placed(id: u32, label: GroundLabel, team: u8, at: [f32; 3]) -> PlacedMarker {
    PlacedMarker {
        id: OpId(id),
        label,
        number: None,
        frame: frame(at, 0.),
        team,
    }
}

#[test]
fn ground_labels_are_markers_and_a_problem_takes_the_place_of_its_label() {
    let mut own = flag(0, 0, 0, [1., 1., 0.]);
    own.id = Endpoint::Added(OpId(7));
    let things = [own];
    let ground = [
        placed(7, GroundLabel::Drop, 0, [1., 1., 0.]),
        placed(8, GroundLabel::Base, 1, [2., 2., 0.]),
        placed(9, GroundLabel::Cover, 0, [3., 3., 0.]),
    ];
    let mut f = facts(&things);
    f.ground = &ground;
    let got = entries(&f);
    // The flag under a ground label is its label, not a flag stand.
    assert_eq!(names(&got), ["ff_drop", "ff_base", "ff_cover"]);
    assert_eq!(got[0].name, "added:7");
    assert_eq!(got[1].label, "Blue ff base");
    assert_eq!(got[0].marker.team, None);
    assert!(got.iter().all(|e| e.tone == Tone::Normal));

    let problems = [GroundProblem {
        label: "ff_drop".into(),
        position: [1., 1., 0.],
        reason: "no floor".into(),
    }];
    f.problems = &problems;
    let got = entries(&f);
    assert_eq!(names(&got), ["ff_base", "ff_cover", "ff_drop"]);
    assert_eq!(got[2].tone, Tone::Problem);
    assert_eq!(got[2].name, "problem:ff_drop");
    assert_eq!(got[2].id, None);
}

fn lined(n: usize, spacing: f32) -> Vec<Entry> {
    let things: Vec<Thing> = (0..n)
        .map(|i| spawn(i as u32, 0, [i as f32 * spacing, 0., 0.]))
        .collect();
    entries(&facts(&things))
}

fn eye0() -> Option<[f32; 3]> {
    Some([0., 0., 0.])
}

#[test]
fn the_257th_marker_is_counted_and_not_drawn() {
    // 257 spawns from 0 to 25.6 units: all in range.
    let all = lined(257, 0.1);
    let (groups, report) = draw(&all, [&None, &None], eye0(), &Palette::default(), BUDGET);
    assert_eq!(groups.len(), 256);
    assert_eq!(report.shown, 256);
    assert_eq!(report.hidden_by_budget, 1);
    assert_eq!(report.markers.len(), 256);
    // The nearest are kept: the farthest is the one left out.
    assert!(
        report
            .markers
            .iter()
            .all(|m| m.thing != "starting_locations:256")
    );
    assert_eq!(report.markers[0].thing, "starting_locations:0");
    // 256 fit.
    let (_, report) = draw(
        &all[..256],
        [&None, &None],
        eye0(),
        &Palette::default(),
        BUDGET,
    );
    assert_eq!((report.shown, report.hidden_by_budget), (256, 0));
}

#[test]
fn a_marker_past_its_fade_is_neither_drawn_nor_counted() {
    let mut spawns = lined(1, 1.);
    spawns.extend(entries(&facts(&[spawn(1, 0, [61., 0., 0.])])));
    let (groups, report) = draw(&spawns, [&None, &None], eye0(), &Palette::default(), 1);
    assert_eq!((groups.len(), report.hidden_by_budget), (1, 0));
    // An area's wall reaches farther.
    let hill = entries(&facts(&[flag(0, 11, 0, [100., 0., 0.])]));
    let (groups, _) = draw(&hill, [&None, &None], eye0(), &Palette::default(), BUDGET);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].fade, Some(AREA_FADE));
    let far = entries(&facts(&[flag(0, 11, 0, [200., 0., 0.])]));
    let (groups, _) = draw(&far, [&None, &None], eye0(), &Palette::default(), BUDGET);
    assert!(groups.is_empty());
}

#[test]
fn a_group_is_the_markers_lines_in_its_colour_faded_with_distance() {
    let got = entries(&facts(&[
        spawn(0, 0, [5., 0., 0.]),
        spawn(1, 1, [30., 0., 0.]),
    ]));
    let palette = Palette::default();
    let (groups, _) = draw(&got, [&None, &None], eye0(), &palette, BUDGET);
    let mesh = got[0].marker.mesh(&palette).placed(&got[0].place);
    assert_eq!(groups[0].edges.len(), mesh.lines.len());
    assert_eq!(groups[0].edges[0], [mesh.lines[0].a, mesh.lines[0].b]);
    assert_eq!(groups[0].colour, palette.teams[0]);
    assert_eq!(groups[1].colour, palette.teams[1]);
    assert_eq!(groups[0].alpha, LINE_ALPHA);
    assert_eq!(groups[0].fade, Some(FADE));
    // The label is over the marker's top, within 20 units only.
    assert_eq!(groups[0].label.as_deref(), Some("Red spawn"));
    assert_eq!(groups[1].label, None);
    let top = mesh.bounds().unwrap().1[2];
    assert_eq!(groups[0].anchor, [5., 0., top + LABEL_LIFT]);
    // With no eye everything is in range and labelled.
    let (groups, _) = draw(&got, [&None, &None], None, &palette, BUDGET);
    assert!(groups.iter().all(|g| g.label.is_some()));
}

#[test]
fn another_game_types_marker_is_faded_and_the_aimed_one_is_white_and_never_fades() {
    let things = [
        spawn(0, 0, [60., 0., 0.]),
        flag(0, 0, 0, [2., 0., 0.]),
        flag(1, 0, 0, [3., 0., 0.]),
    ];
    let mut f = facts(&things);
    f.game = Some(2);
    f.show = Show::AllGameTypes;
    let got = entries(&f);
    let aimed = Some(Endpoint::base(BlockKind::StartingLocations, 0));
    let (groups, report) = draw(&got, [&aimed, &None], eye0(), &Palette::default(), BUDGET);
    // The aimed spawn is first although it is the farthest, white, with no
    // fade and a label.
    assert_eq!(report.markers[0].thing, "starting_locations:0");
    assert_eq!(groups[0].colour, SELECTED);
    assert_eq!(groups[0].fade, None);
    assert!(groups[0].label.is_some());
    assert_eq!(groups[1].alpha, OTHER_GAME);
    assert!(report.markers[1].faded && !report.markers[0].faded);
    // The held thing is selected the same way, by identity.
    let held = Some(Endpoint::base(BlockKind::NetgameFlags, 1).identity());
    let (groups, report) = draw(&got, [&None, &held], eye0(), &Palette::default(), BUDGET);
    assert_eq!(report.markers[0].thing, "netgame_flags:1");
    assert_eq!(groups[0].colour, SELECTED);
    // A problem is orange.
    let problems = [GroundProblem {
        label: "ff_cover".into(),
        position: [1., 0., 0.],
        reason: String::new(),
    }];
    let mut f = facts(&[]);
    f.problems = &problems;
    let (groups, _) = draw(
        &entries(&f),
        [&None, &None],
        eye0(),
        &Palette::default(),
        BUDGET,
    );
    assert_eq!(groups[0].colour, PROBLEM);
}

#[test]
fn render_markers_answers_what_was_drawn_and_filters_by_kind() {
    let all = lined(3, 1.);
    let (_, report) = draw(&all, [&None, &None], eye0(), &Palette::default(), 2);
    let r = answer(Json::Null, &report).unwrap();
    assert_eq!(r["shown"], 2);
    assert_eq!(r["hidden_by_budget"], 1);
    let first = &r["markers"][0];
    assert_eq!(first["thing"], "starting_locations:0");
    assert_eq!(first["kind"], "spawn");
    assert_eq!(first["team"], 0);
    assert_eq!(first["faded"], false);
    assert!(first["lines"].as_u64().unwrap() > 0);
    assert_eq!(first["anchor"].as_array().unwrap().len(), 3);
    // A kind filter lists only that kind; the counts stay whole.
    let r = answer(json!({"kind": "hill"}), &report).unwrap();
    assert_eq!(r["markers"], json!([]));
    assert_eq!(r["shown"], 2);
    let r = answer(json!({"kind": "spawn"}), &report).unwrap();
    assert_eq!(r["markers"].as_array().unwrap().len(), 2);
}

#[test]
fn render_markers_refuses_an_unknown_kind_and_stray_params() {
    let report = Report::default();
    let e = answer(json!({"kind": "flag"}), &report).unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(
        e.message.contains("unknown marker kind `flag`"),
        "{}",
        e.message
    );
    assert!(e.message.contains("flag_stand"), "{}", e.message);
    for bad in [json!({"team": 1}), json!({"kind": 3}), json!([1])] {
        let e = answer(bad, &report).unwrap_err();
        assert_eq!(e.code, code::INVALID_PARAMS);
    }
    let empty = answer(json!({}), &report).unwrap();
    assert_eq!(
        empty,
        json!({"shown": 0, "hidden_by_budget": 0, "markers": []})
    );
}

#[test]
fn the_method_is_registered_and_reads_the_last_report() {
    use std::sync::mpsc::sync_channel;
    struct Reader;
    impl agent::Reader for Reader {
        fn status(&mut self) -> agent::GameStatus {
            Default::default()
        }
        fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
            Ok(Vec::new())
        }
    }
    let _one = GUARD.lock().unwrap_or_else(|e| e.into_inner());
    clear();
    let a = agent::Agent::new();
    add_method(&a);
    let (tx, rx) = sync_channel(8);
    let conn = a.connect(tx);
    let ask = |id: u32, params: Json| {
        a.submit(
            conn,
            &json!({"jsonrpc":"2.0","id":id,"method":method::RENDER_MARKERS,"params":params})
                .to_string(),
        );
        a.between_slices(&mut Reader, 0);
        serde_json::from_str::<Json>(&rx.try_recv().unwrap()).unwrap()
    };
    let r = ask(1, json!({}));
    assert_eq!(r["result"]["shown"], 0);
    assert_eq!(r["result"]["markers"], json!([]));

    let all = lined(2, 1.);
    let (_, report) = draw(&all, [&None, &None], eye0(), &Palette::default(), BUDGET);
    *LAST.lock().unwrap() = report;
    let r = ask(2, json!({}));
    assert_eq!(r["result"]["shown"], 2);
    assert_eq!(r["result"]["markers"][1]["thing"], "starting_locations:1");

    let r = ask(3, json!({"kind": "nope"}));
    assert_eq!(r["error"]["code"], code::INVALID_PARAMS);
    clear();
}

#[test]
fn publishing_turns_the_source_on_and_clearing_turns_it_off() {
    let _one = GUARD.lock().unwrap_or_else(|e| e.into_inner());
    clear();
    assert!(!ACTIVE.load(Ordering::Acquire));
    let all = Arc::new(lined(1, 1.));
    publish(&all, None, None);
    assert!(ACTIVE.load(Ordering::Acquire));
    assert_eq!(published().unwrap().entries.len(), 1);
    // A session with the monitor off publishes none.
    publish(&Arc::new(Vec::new()), None, None);
    assert!(!ACTIVE.load(Ordering::Acquire));
    clear();
    assert!(published().is_none());
}

fn wall(zone: u32, team: Option<u8>, at: [f32; 3]) -> Wall {
    Wall {
        name: format!("zone:{zone:x}"),
        team,
        at,
        area: Area::default(),
    }
}

#[test]
fn a_wall_is_the_boundary_alone_in_its_teams_colour_with_no_label() {
    let walls = [wall(1, Some(1), [0.; 3]), wall(2, None, [10., 0., 0.])];
    let palette = Palette::default();
    let (groups, shown) = draw_walls(&walls, eye0(), &palette);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].colour, palette.teams[1]);
    assert_eq!(groups[1].colour, palette.neutral);
    for g in &groups {
        assert_eq!(g.label, None, "no label");
        assert_eq!(g.fade, Some(AREA_FADE));
        assert_eq!(g.edges.len(), 3 * Area::default().outline().len(), "no pip");
    }
    assert_eq!(shown.iter().map(|s| s.kind).collect::<Vec<_>>(), ["wall", "wall"]);
    assert_eq!(shown[0].thing, "zone:1");
    assert_eq!(shown[0].team, Some(1));
    assert_eq!(shown[1].team, None);
    assert_eq!(shown[0].lines, groups[0].edges.len());
}

#[test]
fn a_wall_past_its_fade_is_not_drawn_and_with_no_eye_every_wall_is() {
    let near = wall(1, None, [0.; 3]);
    let far = wall(2, None, [500., 0., 0.]);
    let (groups, shown) = draw_walls(&[near.clone(), far.clone()], eye0(), &Palette::default());
    assert_eq!((groups.len(), shown.len()), (1, 1));
    assert_eq!(shown[0].thing, near.name);
    let (groups, _) = draw_walls(&[near, far], None, &Palette::default());
    assert_eq!(groups.len(), 2);
}

#[test]
fn render_markers_lists_the_walls_shown_and_filters_by_the_wall_kind() {
    let (_, shown) = draw_walls(&[wall(7, Some(0), [0.; 3])], eye0(), &Palette::default());
    let report = Report {
        shown: 1,
        hidden_by_budget: 0,
        markers: shown,
    };
    let r = answer(json!({"kind": "wall"}), &report).unwrap();
    assert_eq!(r["markers"].as_array().unwrap().len(), 1);
    assert_eq!(r["markers"][0]["kind"], "wall");
    assert_eq!(r["markers"][0]["thing"], "zone:7");
    let r = answer(json!({"kind": "hill"}), &report).unwrap();
    assert_eq!(r["markers"], json!([]));
    assert_eq!(r["shown"], 1);
}
