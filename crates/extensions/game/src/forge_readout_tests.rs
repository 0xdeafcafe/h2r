use super::*;
use forge::{BlockKind, Frame};
use hud_kit_core::huds::forge_readout::Axes;

fn entry(
    block: BlockKind,
    kind: ThingKind,
    index: u32,
    at: [f32; 3],
    fields: &[(&'static str, i64)],
) -> Entry {
    Entry {
        id: Endpoint::base(block, index),
        block,
        kind,
        own: true,
        frame: Frame {
            position: at,
            rotation: [0.0; 3],
        },
        fields: fields.iter().map(|(n, v)| (*n, Field::Int(*v))).collect(),
        changed: false,
    }
}

fn spawn(index: u32, at: [f32; 3], team: i64) -> Entry {
    entry(
        BlockKind::StartingLocations,
        ThingKind::StartingPoint,
        index,
        at,
        &[("team", team)],
    )
}

fn shown(values: &Values, id: &str) -> bool {
    values.get(id).is_some_and(|v| v.shown)
}

fn aim(at: [f32; 3]) -> Aimed {
    aimed(
        &spawn(99, at, 1),
        "Starting point".into(),
        at,
        90.0,
        0.5,
        None,
    )
}

#[test]
fn near_marks_are_the_twelve_nearest_of_thirty_by_distance_then_identity() {
    // Thirty things on a line, one unit apart from 1, in reverse identity
    // order; two share a distance with another.
    let mut things: Vec<Placed> = (0..30u32)
        .map(|i| placed(&spawn(100 - i, [1.0 + i as f32, 0.0, 0.0], 0), None).unwrap())
        .collect();
    // Same distance as the thing at x 3 (index 98), lower identity.
    things.push(placed(&spawn(5, [0.0, 3.0, 0.0], 0), None).unwrap());
    let chosen = choose_near(&things, [0.0; 3], None);
    assert_eq!(chosen.len(), NEAR);
    // Within the radius: x 1 to 12.
    assert!(chosen.iter().all(|p| dist2(p.at, [0.0; 3]) <= 144.0));
    let ids: Vec<_> = chosen.iter().map(|p| p.id).collect();
    // Nearest first; at distance 3 the lower identity (index 5) comes first.
    assert_eq!(ids[0], Endpoint::base(BlockKind::StartingLocations, 100));
    assert_eq!(ids[1], Endpoint::base(BlockKind::StartingLocations, 99));
    assert_eq!(ids[2], Endpoint::base(BlockKind::StartingLocations, 5));
    assert_eq!(ids[3], Endpoint::base(BlockKind::StartingLocations, 98));
    // The twelfth is the eleventh of the line (the extra one took a place).
    assert_eq!(ids[11], Endpoint::base(BlockKind::StartingLocations, 90));
}

#[test]
fn nothing_beyond_the_reach_is_near_and_the_aimed_thing_is_not() {
    let far = placed(&spawn(1, [NEAR_RADIUS + 0.5, 0.0, 0.0], 0), None).unwrap();
    let on = placed(&spawn(2, [NEAR_RADIUS, 0.0, 0.0], 0), None).unwrap();
    let skipped = placed(&spawn(3, [1.0, 0.0, 0.0], 0), None).unwrap();
    let things = [far, on.clone(), skipped.clone()];
    let ids: Vec<_> = choose_near(&things, [0.0; 3], Some(skipped.id))
        .iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, [on.id]);
}

#[test]
fn with_the_monitor_off_every_element_is_hidden_and_no_mark_is_posed() {
    let scene = Scene {
        monitor: false,
        target: Some(aim([1.0, 2.0, 3.0])),
        things: vec![placed(&spawn(1, [1.0, 0.0, 0.0], 0), None).unwrap()],
        ..Scene::default()
    };
    let (values, poses) = frame(&scene);
    assert!(poses.is_empty());
    assert!(values.0.iter().all(|(_, v)| !v.shown));
    assert_eq!(values.layout_problems(&fr::hud()), Vec::<String>::new());
}

#[test]
fn an_aimed_thing_has_its_selection_and_face_and_the_near_pool_the_rest() {
    let scene = Scene {
        monitor: true,
        eye: [0.0; 3],
        target: Some(aim([4.0, 0.0, 1.0])),
        things: vec![
            placed(&spawn(1, [2.0, 0.0, 0.0], 3), None).unwrap(),
            placed(&spawn(99, [4.0, 0.0, 1.0], 1), None).unwrap(),
        ],
        ..Scene::default()
    };
    let (values, poses) = frame(&scene);
    let ids: Vec<_> = poses.iter().map(|(i, _)| *i).collect();
    assert_eq!(ids, [fr::SEL, fr::FACE, "fgr.near.1"]);
    assert!(shown(&values, fr::SEL) && shown(&values, fr::FACE));
    assert!(shown(&values, "fgr.near.1") && !shown(&values, "fgr.near.2"));
    // The aimed thing is not also a near mark; it is in its own row.
    assert_eq!(values.get(fr::NAME_ROW).unwrap().text, "Starting point");
    assert_eq!(poses[2].1.at, [2.0, 0.0, 0.0]);
    assert_eq!(values.layout_problems(&fr::hud()), Vec::<String>::new());
}

#[test]
fn a_held_thing_has_under_on_the_ground_and_from_where_it_was_taken() {
    let hold = |height: f32, from: Option<[f32; 3]>| Hold {
        position: [4.0, 0.0, 1.0],
        height,
        snap: Some(0.25),
        axes: Axes::World,
        blocked: None,
        from,
    };
    let scene = |hold| Scene {
        monitor: true,
        target: Some(aim([4.0, 0.0, 1.0])),
        hold: Some(hold),
        ..Scene::default()
    };
    let (values, poses) = frame(&scene(hold(0.85, Some([1.0, 1.0, 0.0]))));
    let pose = |id| poses.iter().find(|(i, _)| *i == id).map(|(_, p)| *p);
    // The ground is the height below the thing.
    let under = pose(fr::UNDER).unwrap().at;
    assert_eq!(under[..2], [4.0, 0.0]);
    assert!((under[2] - 0.15).abs() < 1e-5, "{under:?}");
    assert_eq!(pose(fr::SEL).unwrap().at, pose(fr::UNDER).unwrap().at);
    assert_eq!(pose(fr::FROM).unwrap().at, [1.0, 1.0, 0.0]);
    assert!(shown(&values, fr::UNDER) && shown(&values, fr::FROM));
    // On the ground: no under mark; a new thing: no from mark.
    let (values, poses) = frame(&scene(hold(0.0, None)));
    assert!(!poses.iter().any(|(i, _)| *i == fr::UNDER || *i == fr::FROM));
    assert!(!shown(&values, fr::UNDER) && !shown(&values, fr::FROM));
    // A hold with no target shows nothing of it.
    let (values, poses) = frame(&Scene {
        monitor: true,
        hold: Some(hold(0.85, None)),
        ..Scene::default()
    });
    assert!(poses.is_empty());
    assert!(!shown(&values, fr::UNDER));
}

#[test]
fn a_zone_has_a_line_between_each_pair_of_corners() {
    let (values, poses) = frame(&Scene {
        monitor: true,
        zone: vec![[0.0; 3], [4.0, 0.0, 0.0], [4.0, 3.0, 0.0]],
        ..Scene::default()
    });
    assert_eq!(poses.len(), 2);
    assert_eq!(
        poses[0],
        (
            "fgr.zone.1",
            Pose::at([2.0, 0.0, 0.0], [2.0, 0.0]).heading([4.0, 0.0])
        )
    );
    assert!(shown(&values, "fgr.zone.2") && !shown(&values, "fgr.zone.3"));
}

#[test]
fn placed_things_by_kind_with_their_team_and_game_type() {
    // A body is not a near mark.
    let object = entry(BlockKind::Scenery, ThingKind::Object, 0, [0.0; 3], &[]);
    assert!(placed(&object, None).is_none());
    let flag = |ty, team, id| {
        entry(
            BlockKind::NetgameFlags,
            ThingKind::Goal,
            0,
            [0.0; 3],
            &[("type", ty), ("team", team), ("identifier", id)],
        )
    };
    let p = placed(&flag(0, 1, 0), None).unwrap().near;
    assert_eq!(
        (p.label.as_str(), p.team, p.in_game_type),
        ("Flag", Some(1), true)
    );
    assert_eq!(
        placed(&flag(12, 255, 2), None).unwrap().near.label,
        "Hill 2"
    );
    // A team the game has no colour for is the HUD's colour; the engine's
    // no-team value is none.
    assert_eq!(placed(&flag(0, 255, 0), None).unwrap().near.team, None);
    // CTF's flag is not in Slayer (engine 2) and is in CTF (1).
    assert!(!placed(&flag(0, 0, 0), Some(2)).unwrap().near.in_game_type);
    assert!(placed(&flag(0, 0, 0), Some(1)).unwrap().near.in_game_type);
    let tele = entry(
        BlockKind::NetgameFlags,
        ThingKind::Teleporter,
        1,
        [0.0; 3],
        &[("type", 8), ("identifier", 3)],
    );
    assert_eq!(placed(&tele, None).unwrap().near.label, "Receiver");
}

#[test]
fn aimed_things_say_their_kind_source_and_game_type() {
    let tele = entry(
        BlockKind::NetgameFlags,
        ThingKind::Teleporter,
        1,
        [0.0; 3],
        &[("type", 7), ("identifier", 3)],
    );
    let a = aimed(
        &tele,
        "Teleporter".into(),
        [1.0, 2.0, 3.0],
        450.0,
        0.1,
        None,
    );
    assert_eq!(
        a.target.thing,
        Thing::Teleporter {
            role: "Sender".into(),
            channel: 3
        }
    );
    assert_eq!(a.target.source, Source::FromMap);
    assert_eq!(a.id, tele.id);
    let mut moved = tele.clone();
    moved.changed = true;
    assert_eq!(
        aimed(&moved, "T".into(), [0.0; 3], 0.0, 0.1, None)
            .target
            .source,
        Source::FromMapChanged
    );
    moved.own = false;
    assert_eq!(
        aimed(&moved, "T".into(), [0.0; 3], 0.0, 0.1, None)
            .target
            .source,
        Source::Added
    );
    // A goal of another game type says so on the guide row.
    let flag = entry(
        BlockKind::NetgameFlags,
        ThingKind::Goal,
        0,
        [0.0; 3],
        &[("type", 0)],
    );
    let a = aimed(&flag, "Flag".into(), [0.0; 3], 0.0, 0.1, Some(2));
    assert_eq!(a.target.not_in.as_deref(), Some("this game type"));
}
