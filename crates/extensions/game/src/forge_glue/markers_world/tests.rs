use forge::{BlockKind, GroundLabel, OpId, TagKey};
use game_markers::form::{EDIT_ALPHA, ICON_NEAR, PLATE};
use game_markers::{Icon, LINE_ALPHA};

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
    static LABELS: BTreeMap<Endpoint, forge_doc::Label> = BTreeMap::new();
    static AREAS: BTreeMap<Endpoint, forge_doc::Area> = BTreeMap::new();
    Facts {
        monitor: true,
        things,
        ground: &[],
        labels: &LABELS,
        areas: &AREAS,
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
    // A spawn has no play form: every line of it is dim, the plate's too.
    assert!(groups[0].edges.is_empty());
    assert_eq!(groups[0].dim.len(), mesh.lines.len());
    assert_eq!(groups[0].dim[0], [mesh.lines[0].a, mesh.lines[0].b]);
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
    assert_eq!(first["form"], "edit");
    assert_eq!(first["icon"], "respawn");
    assert_eq!(first["plate_scale"], 1.0);
    assert!(first["lines"].as_u64().unwrap() > 0);
    assert_eq!(first["anchor"].as_array().unwrap().len(), 3);
    // A kind filter lists only that kind; the counts stay whole.
    let r = answer(json!({"kind": "hill"}), &report).unwrap();
    assert_eq!(r["markers"], json!([]));
    assert_eq!(r["shown"], 2);
    let r = answer(json!({"kind": "spawn"}), &report).unwrap();
    assert_eq!(r["markers"].as_array().unwrap().len(), 2);
    // So does a form filter: Forge draws the edit form.
    let r = answer(json!({"form": "edit"}), &report).unwrap();
    assert_eq!(r["markers"].as_array().unwrap().len(), 2);
    let r = answer(json!({"form": "play", "kind": "spawn"}), &report).unwrap();
    assert_eq!(r["markers"], json!([]));
    assert_eq!(r["shown"], 2);
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
    let e = answer(json!({"form": "solid"}), &report).unwrap_err();
    assert_eq!(e.code, code::INVALID_PARAMS);
    assert!(e.message.contains("unknown marker form `solid`"), "{}", e.message);
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
        yaw: 0.0,
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
        let wall = Kind::wall(&Area::default(), [1.0; 3]);
        assert_eq!(g.edges.len(), wall.lines.len(), "no pip");
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

/// An invented hill at the origin and a wall 20 units along x, as the
/// window sees them from `eye`.
#[cfg(feature = "gpu")]
fn hill_and_wall(eye: Option<[f32; 3]>) -> (Vec<Group>, Entry) {
    let palette = Palette::default();
    let hill = entry(
        None,
        "hill".into(),
        Kind::Hill(Area::default()),
        None,
        Some(0),
        [0.; 3],
        0.0,
        Tone::Normal,
        false,
    );
    let (mut groups, _) = draw(std::slice::from_ref(&hill), [&None, &None], eye, &palette, BUDGET);
    let (walls, _) = draw_walls(&[wall(1, Some(0), [20., 0., 0.])], eye, &palette);
    groups.extend(walls);
    (groups, hill)
}

#[cfg(feature = "gpu")]
#[test]
fn the_batch_of_a_hill_and_a_wall_has_their_faces_and_lines() {
    let palette = Palette::default();
    let (groups, hill) = hill_and_wall(None);
    assert_eq!(groups.len(), 2);
    let ring = Area::default().outline().len();
    let hill_faces = hill.marker.mesh(&palette).tris.len();
    assert!(hill_faces > 0, "the hill has a skin");
    assert_eq!(groups[0].tris.len(), hill_faces);
    assert_eq!(groups[1].tris.len(), 2 * ring, "two faces a side of the wall");
    let got = batch(&groups, None);
    // Three vertices a face, two a line: the hill's, then the wall's.
    assert_eq!(got.tris.len(), 3 * (hill_faces + 2 * ring));
    let hill_lines = groups[0].edges.len() + groups[0].dim.len();
    assert_eq!(got.lines.len(), 2 * (hill_lines + groups[1].edges.len()));
    // The wall's faces keep their alpha; its lines are its team's colour
    // at the group's alpha.
    let first_wall_face = 3 * hill_faces;
    assert_eq!(got.tris[first_wall_face].colour, groups[1].tris[0].colours[0]);
    assert!(got.tris[first_wall_face].colour[3] < 1.0, "a skin, not a solid");
    let line = got.lines[2 * hill_lines].colour;
    assert_eq!(line, [palette.teams[0][0], palette.teams[0][1], palette.teams[0][2], groups[1].alpha]);
    // Positions are the mesh's world points.
    assert_eq!(got.lines[2 * hill_lines].position, groups[1].edges[0][0]);
    // The hill's wall is its play form, at the group's alpha; its pip,
    // post and plate follow at the dim alpha.
    let dim = got.lines[2 * groups[0].edges.len()].colour;
    assert_eq!(dim[3], groups[0].dim_alpha());
    assert_eq!(got.lines[0].colour[3], groups[0].alpha);
}

#[cfg(feature = "gpu")]
#[test]
fn the_batch_fades_each_vertex_with_its_distance_from_the_eye() {
    let (groups, _) = hill_and_wall(None);
    let near = batch(&groups, Some([20., 0., 0.]));
    let none = batch(&groups, None);
    assert_eq!(near, none, "the eye in the middle of both: full strength");
    let far = batch(&groups, Some([1000., 0., 0.]));
    assert_eq!(far.tris.len(), none.tris.len());
    assert!(far.tris.iter().chain(&far.lines).all(|v| v.colour[3] == 0.0), "past every fade");
    // The colour is not touched by the fade.
    assert_eq!(&far.tris[0].colour[..3], &none.tris[0].colour[..3]);
}

#[cfg(feature = "gpu")]
#[test]
fn with_the_native_pass_drawing_the_window_keeps_the_labels_and_drops_the_lines() {
    use gfx::world_lines::{ImageMarks, LabelFace};
    let (groups, _) = hill_and_wall(None);
    assert!(groups[0].label.is_some() && groups[1].label.is_none());
    let seen = |native_lines| {
        let sample = gpu::Sample { groups: groups.clone(), face: LabelFace::Debug, native_lines };
        let mut out = Vec::new();
        sample.lines(&mut |l| out.push((l.edges.len(), l.label.map(str::to_owned))));
        out
    };
    // The window alone draws every line and the label: the hill's play
    // form with its label, its edit-only parts in a dim visit of their
    // own, then the wall.
    let alone = seen(false);
    assert_eq!(alone.len(), 3);
    assert_eq!(alone[0], (groups[0].edges.len(), Some("Hill 1".to_owned())));
    assert_eq!(alone[1], (groups[0].dim.len(), None));
    assert_eq!(alone[2], (groups[1].edges.len(), None));
    // The native pass drew them: the label is all the window draws.
    assert_eq!(seen(true), [(0, Some("Hill 1".to_owned()))]);
}

#[test]
fn with_the_monitor_on_each_thing_draws_its_edit_form_its_play_parts_bright_and_the_rest_dim() {
    let things = [
        spawn(0, 1, [1., 2., 0.]),
        flag(0, 0, 0, [3., 0., 0.]),
        flag(1, 1, 1, [6., 0., 0.]),
        flag(2, 10, 0, [9., 0., 0.]),
        flag(3, 7, 8, [12., 0., 0.]),
        flag(4, 12, 0, [15., 0., 0.]),
        spawner(0, "weap", "weapons\\pistol"),
    ];
    let got = entries(&facts(&things));
    assert_eq!(
        names(&got),
        [
            "spawn",
            "flag_stand",
            "flag_return",
            "territory",
            "teleporter_sender",
            "weapon_spawn",
            "hill",
        ]
    );
    assert!(got.iter().all(|e| e.form == Form::Edit));
    // A hill's plate counts from 1: hill type 12 is the second hill.
    assert_eq!(got[6].number, Some(2));
    assert_eq!(got[3].number, None, "a territory with no number has none");
    let palette = Palette::default();
    let (groups, report) = draw(&got, [&None, &None], None, &palette, BUDGET);
    assert_eq!(groups.len(), got.len());
    for ((g, e), m) in groups.iter().zip(&got).zip(&report.markers) {
        let c = e.marker.colour(&palette);
        let kind = &e.marker.kind;
        let plate = kind.plate(e.number, c).map_or(0, |p| p.lines.len());
        let plate_in_play = kind.parts(Form::Play).has(Part::Plate);
        let play = kind.mesh_in(Form::Play, c).lines.len() + if plate_in_play { plate } else { 0 };
        assert_eq!(g.edges.len(), play, "{}: the play form is bright", m.kind);
        assert_eq!(
            g.edges.len() + g.dim.len(),
            kind.mesh_in(Form::Edit, c).lines.len() + plate,
            "{}: the edit form, all of it",
            m.kind
        );
        assert_eq!(m.lines, g.edges.len() + g.dim.len());
        assert_eq!(m.form, Form::Edit);
        assert_eq!(m.icon, kind.icon().map(Icon::name));
        assert_eq!(m.plate_scale, Some(1.0), "{}", m.kind);
        assert_eq!(g.dim_alpha(), g.alpha * EDIT_ALPHA);
        // The dim faces come last, faded by EDIT_ALPHA.
        let plate_tris = kind.plate(e.number, c).map_or(0, |p| p.tris.len());
        let dim_tris = kind.edit_only(c).tris.len() + if plate_in_play { 0 } else { plate_tris };
        let dim = &g.tris[g.tris.len() - dim_tris..];
        assert!(dim.iter().all(|t| t.colours[0][3] <= EDIT_ALPHA), "{}", m.kind);
    }
    // A spawn and an item have no play form; a flag return's is its
    // capture plate; a territory's takes its plate with it.
    assert!(groups[0].edges.is_empty() && !groups[0].dim.is_empty());
    assert!(groups[5].edges.is_empty());
    assert!(!groups[2].edges.is_empty());
    assert!(got[3].marker.kind.parts(Form::Play).has(Part::Plate));
    assert_eq!(report.markers[2].icon, Some("flag_return"));
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let n = v.iter().map(|c| c * c).sum::<f32>().sqrt();
    v.map(|c| c / n)
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

#[test]
fn a_plates_normal_points_at_the_eye_and_its_size_doubles_from_8_to_16_units_away() {
    let got = entries(&facts(&[spawn(0, 0, [2., 3., 0.])]));
    let e = &got[0];
    let centre = e.place.point(e.marker.kind.plate_at().unwrap());
    let colour = [1.0; 3];
    for dir in [[1., 0., 0.], [0., 1., 0.], [-0.6, -0.8, 0.]] {
        let eye = [centre[0] + 8. * dir[0], centre[1] + 8. * dir[1], centre[2]];
        let (place, scale) = plate_place(e, Some(eye)).unwrap();
        assert!((scale - 1.0).abs() < 1e-4, "{scale}");
        assert!((dot(place.forward, unit(sub(eye, place.at))) - 1.0).abs() < 1e-4, "{dir:?}");
        // The plate's disc lies across the line to the eye.
        let (mesh, _) = plate(e, colour, Some(eye)).unwrap();
        let t = mesh.tris[0].points;
        let [u, v] = [sub(t[1], t[0]), sub(t[2], t[0])];
        let normal = unit([
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ]);
        assert!((dot(normal, dir).abs() - 1.0).abs() < 1e-4, "{dir:?}: {normal:?}");
    }
    let seen = |d: f32| {
        let eye = [centre[0] + d, centre[1], centre[2]];
        let (mesh, scale) = plate(e, colour, Some(eye)).unwrap();
        let (min, max) = mesh.bounds().unwrap();
        (max[2] - min[2], min[2], scale)
    };
    let (near, _, _) = seen(4.);
    let (at8, foot8, scale8) = seen(ICON_NEAR);
    let (at16, foot16, scale16) = seen(2. * ICON_NEAR);
    assert!((scale8 - 1.0).abs() < 1e-4 && (scale16 - 2.0).abs() < 1e-4, "{scale8} {scale16}");
    assert!((at8 - 2. * PLATE).abs() < 1e-4, "{at8}");
    assert!((near - at8).abs() < 1e-5, "nearer than 8 it keeps its size");
    assert!((at16 / at8 - 2.0).abs() < 1e-4, "{at8} {at16}");
    assert!((foot16 - foot8).abs() < 1e-4, "its foot stays on the post");
    // The drawer reports the scale it drew.
    let eye = Some([centre[0] + 16., centre[1], centre[2]]);
    let (_, report) = draw(&got, [&None, &None], eye, &Palette::default(), BUDGET);
    let drawn_scale = report.markers[0].plate_scale.unwrap();
    assert!((drawn_scale - 2.0).abs() < 1e-4, "{drawn_scale}");
}

#[test]
fn the_play_form_draws_only_the_play_parts_and_a_plate_only_where_play_has_one() {
    let mut got = entries(&facts(&[
        spawn(0, 0, [0.; 3]),
        flag(0, 1, 0, [3., 0., 0.]),
        flag(1, 10, 0, [6., 0., 0.]),
    ]));
    for e in &mut got {
        e.form = Form::Play;
    }
    let palette = Palette::default();
    let (groups, report) = draw(&got, [&None, &None], None, &palette, BUDGET);
    assert!(groups.iter().all(|g| g.dim.is_empty()), "nothing dim in play");
    assert!(groups[0].edges.is_empty() && groups[0].tris.is_empty(), "a spawn has no play form");
    assert_eq!(report.markers[0].plate_scale, None);
    let c = got[1].marker.colour(&palette);
    assert_eq!(groups[1].edges.len(), got[1].marker.kind.mesh_in(Form::Play, c).lines.len());
    assert_eq!(report.markers[1].plate_scale, None, "a capture plate has no sign in play");
    assert_eq!(report.markers[2].plate_scale, Some(1.0), "a territory's sign is in play");
    assert!(report.markers.iter().all(|m| m.form == Form::Play));
}

#[test]
fn a_box_area_draws_on_its_markers_facing_and_any_other_area_on_the_worlds_axes() {
    let things = [flag(0, 10, 0, [0.; 3]), flag(1, 10, 0, [20., 0., 0.])];
    let boxed = Area::boxed([2.0, 0.5]);
    let areas: BTreeMap<Endpoint, forge_doc::Area> = [
        (things[0].id.identity(), forge_doc::Area::try_from(&boxed).unwrap()),
        (things[1].id.identity(), forge_doc::Area::try_from(&Area::radius(2.0)).unwrap()),
    ]
    .into();
    let mut f = facts(&things);
    f.areas = &areas;
    let got = entries(&f);
    // `flag` faces 0.5 rad.
    assert!((got[0].place.forward[0] - 0.5f32.cos()).abs() < 1e-6);
    assert!((got[0].place.forward[1] - 0.5f32.sin()).abs() < 1e-6);
    assert_eq!(got[1].place.forward, [1.0, 0.0, 0.0]);
    // The drawn box's floor corners are the ones contains_turned holds.
    let (groups, _) = draw(&got[..1], [&None, &None], None, &Palette::default(), BUDGET);
    let floor = groups[0]
        .edges
        .iter()
        .flatten()
        .map(|p| p[2])
        .fold(f32::INFINITY, f32::min);
    let corners: Vec<[f32; 3]> = groups[0]
        .edges
        .iter()
        .flatten()
        .copied()
        .filter(|p| (p[2] - floor).abs() < 1e-4)
        .collect();
    assert!(!corners.is_empty());
    for c in &corners {
        assert!(boxed.contains_turned([0.; 3], 0.5, [c[0], c[1], 0.0]), "{c:?}");
        assert!(!boxed.contains_turned([0.; 3], 0.5, [c[0] * 1.05, c[1] * 1.05, 0.0]), "{c:?}");
    }
}

#[cfg(feature = "rules")]
fn labelled_flag(index: u32, flag_type: u16, show: bool) -> forge_doc::LabelledThing {
    forge_doc::LabelledThing {
        block: BlockKind::NetgameFlags,
        index,
        label: forge_doc::builtin_label(flag_type).unwrap(),
        class: ::rules::labels::Class::Flag,
        team: Some(0),
        area: None,
        show,
    }
}

#[cfg(feature = "rules")]
#[test]
fn only_a_flag_the_census_shows_has_a_play_entry_at_its_place_and_facing() {
    let mut start = labelled_flag(0, 0, true);
    start.block = BlockKind::StartingLocations;
    let things = [
        labelled_flag(0, 1, true),
        labelled_flag(1, 1, false),
        labelled_flag(2, 10, true),
        // A hill corner has no area of its own: nothing to draw.
        labelled_flag(3, 11, true),
        // Not a flag: never drawn.
        start,
    ];
    let flags = [
        ([1., 2., 0.5], 0.25),
        ([9., 9., 9.], 0.),
        ([4., 5., 0.], 1.5),
        ([7., 7., 7.], 0.),
    ];
    let got = play_entries(&things, &flags);
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|e| e.form == Form::Play && e.id.is_some() && !e.other_game));
    assert_eq!(got[0].marker.kind, Kind::FlagReturn(Area::plate()));
    assert_eq!(got[0].place.at, [1., 2., 0.5]);
    assert_eq!(got[0].name, thing_name(Endpoint::base(BlockKind::NetgameFlags, 0)));
    assert_eq!(got[1].marker.kind.name(), Kind::Territory(Area::default()).name());
    assert_eq!(got[1].place.at, [4., 5., 0.]);
    // A flag's own area is the one drawn; a hill with one is a hill.
    let mut shaped = labelled_flag(3, 11, true);
    shaped.area = Some(Area::radius(4.0));
    let got = play_entries(&[shaped], &flags);
    assert_eq!(got[0].marker.kind, Kind::Hill(Area::radius(4.0)));
    // No place for the flag, no marker.
    assert!(play_entries(&[labelled_flag(5, 1, true)], &flags).is_empty());
}

#[cfg(feature = "rules")]
#[test]
fn a_match_on_a_variants_map_lists_the_capture_plate_with_the_monitor_off() {
    use guest_layout::{points, testing::FakeGuest};
    use game_markers::record::Row;

    const SCNR: u32 = 0x1006_0000;
    const FLAGS_AT: u32 = SCNR + 0x1000;
    let _one = GUARD.lock().unwrap_or_else(|e| e.into_inner());
    clear();
    publish_play(Vec::new());
    // A scenario with one CTF flag return (type 1).
    let mut m = FakeGuest::new();
    m.put_u32(points::SCENARIO.base, SCNR);
    m.map(SCNR, 0x2000);
    m.put_u32(SCNR + engine::objectives::NETGAME_FLAGS, 1)
        .put_u32(SCNR + engine::objectives::NETGAME_FLAGS + 4, FLAGS_AT);
    m.put_f32(FLAGS_AT, 1.0)
        .put_f32(FLAGS_AT + 4, 2.0)
        .put_f32(FLAGS_AT + 8, 0.5)
        .put_u16(FLAGS_AT + engine::objectives::FLAG_TYPE, 1);
    let row = |show| Row {
        index: 0,
        flag_type: 1,
        identifier: 0,
        at: [1.0, 2.0, 0.5],
        area: None,
        show,
    };

    // The map's record shows the flag: the capture plate is a row of
    // `render.markers`, in its play form, with no session and no monitor.
    crate::map_areas::set(vec![row(true)]);
    play_stale();
    assert!(PLAY_STALE.load(Ordering::Acquire));
    assert!(refresh_play(&m));
    assert!(!PLAY_STALE.load(Ordering::Acquire));
    assert!(PLAY_ACTIVE.load(Ordering::Acquire));
    let drawn = frame_entries(Arc::new(Vec::new()));
    let (groups, report) = draw(&drawn, [&None, &None], None, &Palette::default(), BUDGET);
    assert_eq!(groups.len(), 1);
    let rows = report_json(&report, None, Some(Form::Play));
    assert_eq!(rows["shown"], 1);
    assert_eq!(rows["markers"][0]["kind"], Kind::FlagReturn(Area::plate()).name());
    assert_eq!(rows["markers"][0]["form"], "play");
    assert_eq!(rows["markers"][0]["thing"], "netgame_flags:0");
    // The edit form's rows are none: it is a match.
    assert_eq!(report_json(&report, None, Some(Form::Edit))["markers"], json!([]));
    // A Forge session's own markers come first when it has any.
    let session = Arc::new(lined(1, 1.));
    assert_eq!(*frame_entries(session.clone()), *session);

    // A retail map with no record draws nothing.
    crate::map_areas::set(Vec::new());
    play_stale();
    assert!(refresh_play(&m));
    assert!(!PLAY_ACTIVE.load(Ordering::Acquire));
    assert!(frame_entries(Arc::new(Vec::new())).is_empty());
    // A record that does not show the flag draws nothing either.
    crate::map_areas::set(vec![row(false)]);
    play_stale();
    assert!(refresh_play(&m));
    assert!(frame_entries(Arc::new(Vec::new())).is_empty());
    // Nothing to read twice: the next frame reads nothing.
    assert!(!refresh_play(&m));
    crate::map_areas::set(Vec::new());
    publish_play(Vec::new());
}

#[cfg(feature = "rules")]
#[test]
fn a_map_that_is_not_loaded_yet_leaves_the_play_forms_to_the_next_frame() {
    use guest_layout::{points, testing::FakeGuest};
    let _one = GUARD.lock().unwrap_or_else(|e| e.into_inner());
    let mut m = FakeGuest::new();
    m.put_u32(points::SCENARIO.base, 0);
    play_stale();
    assert!(!refresh_play(&m));
    assert!(PLAY_STALE.load(Ordering::Acquire), "still stale: try again");
    publish_play(Vec::new());
    assert!(!PLAY_STALE.load(Ordering::Acquire));
}
