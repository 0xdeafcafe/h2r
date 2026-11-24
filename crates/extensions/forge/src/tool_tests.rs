//! The same monitor gestures used by the tick and dialog routes, over an invented map.
use super::{Fake, session};
use crate::{
    Frame,
    camera::Camera,
    tool::{Action, Command, Extent, Input, MIN_HOLD, NEAR_GAP, Tool},
};
use std::sync::Arc;

fn eye() -> Camera {
    Camera {
        position: [0.; 3],
        yaw: 0.,
        pitch: 0.,
    }
}
fn press(action: Action) -> Input {
    Input {
        action: Some(action),
        ..Default::default()
    }
}

#[test]
fn preview_moves_and_turns_before_one_drop_and_cancel_discards_it() {
    let mut session = session(2);
    let mut world = Fake::default();
    let mut tool = Tool::default();
    assert_eq!(
        tool.update(&session, 1, press(Action::Palette)).unwrap(),
        None
    );
    tool.update(&session, 2, press(Action::Toggle)).unwrap();
    tool.camera = Some(eye());
    assert_eq!(
        tool.update(&session, 3, press(Action::Palette)).unwrap(),
        Some(Command::Palette)
    );
    tool.select(0);
    let turn = Input {
        rotating: true,
        look: [0.24, 0.],
        ..press(Action::Accept)
    };
    let cmd = tool
        .update_with_surface(&session, 4, turn, |_| Some(3.))
        .unwrap()
        .unwrap();
    let Command::Place { item, frame } = cmd else {
        panic!("expected placement")
    };
    assert_eq!(
        frame.position,
        [1.25, 0., 0.],
        "a half-unit item's hold, nearer than the wall"
    );
    assert!((frame.rotation[0] - 15f32.to_radians()).abs() < 1e-5);
    assert_eq!(tool.camera.unwrap().yaw, 0.);
    assert!(world.calls.is_empty());
    let id = session.place(&mut world, 4, item, frame).unwrap();
    tool.applied(&cmd);
    tool.update(&session, 5, press(Action::Accept)).unwrap();
    assert_eq!(tool.held, Some(id));
    let movement = Input {
        movement: [1., 0., 0.],
        seconds: 0.1,
        ..Default::default()
    };
    tool.update(&session, 6, movement).unwrap();
    assert_eq!(world.calls.len(), 1);
    assert_eq!(session.objects()[0].frame, frame);
    tool.set_heading(1.);
    let cmd = tool
        .update(&session, 7, press(Action::Accept))
        .unwrap()
        .unwrap();
    let Command::Move { id, frame: moved } = cmd else {
        panic!("expected drop")
    };
    assert!(moved.position[1] < -0.59);
    assert_eq!(moved.rotation[0], 1.);
    session.move_object(&mut world, 7, id, moved).unwrap();
    tool.applied(&cmd);
    tool.update(&session, 8, press(Action::Accept)).unwrap();
    tool.update(&session, 9, movement).unwrap();
    tool.update(&session, 10, press(Action::Back)).unwrap();
    assert_eq!(tool.held, None);
    assert_eq!(world.calls.len(), 2);
    assert_eq!(session.objects()[0].frame, moved);
}

#[test]
fn modal_input_leaves_view_and_selection_alone_and_walls_block_aim() {
    let mut session = session(2);
    let mut world = Fake::default();
    session
        .place(
            &mut world,
            1,
            0,
            Frame {
                position: [3., 0., 0.],
                rotation: [0.; 3],
            },
        )
        .unwrap();
    let mut tool = Tool::default();
    tool.monitor = true;
    tool.camera = Some(eye());
    assert_eq!(
        tool.update_with_surface(&session, 2, press(Action::Delete), |_| Some(2.))
            .unwrap(),
        None
    );
    let modal = Input {
        dialog: true,
        movement: [1., 1., 1.],
        look: [1., 1.],
        seconds: 0.1,
        ..press(Action::Down)
    };
    assert_eq!(
        tool.update(&session, 3, modal).unwrap(),
        Some(Command::DialogDown)
    );
    assert_eq!(tool.camera, Some(eye()));
    assert_eq!(tool.held, None);
}

/// Halo 3's buttons on the tool: B with nothing held does nothing (the
/// Forge menu is Start's), the Tools picker and the Tool settings are
/// commands, and manipulating pushes a held object away and pulls it in.
#[test]
fn b_idle_is_nothing_tools_and_settings_open_and_manipulate_pushes() {
    let mut session = session(2);
    let mut world = Fake::default();
    let mut tool = Tool::default();
    tool.monitor = true;
    tool.camera = Some(eye());
    assert_eq!(tool.update(&session, 1, press(Action::Back)).unwrap(), None);
    assert_eq!(
        tool.update(&session, 2, press(Action::Tools)).unwrap(),
        Some(Command::Tools)
    );
    assert_eq!(
        tool.update(&session, 3, press(Action::Settings)).unwrap(),
        Some(Command::Settings)
    );
    assert_eq!(Action::named("tools"), Some(Action::Tools));
    assert_eq!(Action::named("settings"), Some(Action::Settings));
    let frame = Frame {
        position: [3., 0., 0.],
        rotation: [0.; 3],
    };
    session.place(&mut world, 4, 0, frame).unwrap();
    tool.update(&session, 5, press(Action::Accept)).unwrap();
    assert!(tool.held.is_some());
    let push = |forward: f32| Input {
        rotating: true,
        movement: [0., forward, 0.],
        seconds: 0.1,
        ..Default::default()
    };
    tool.update(&session, 6, push(1.)).unwrap();
    let away = tool.ghost.unwrap().position[0];
    assert!(away > 3.5, "pushed away: {away}");
    for t in 7..40 {
        tool.update(&session, t, push(-1.)).unwrap();
    }
    let near = tool.ghost.unwrap().position[0];
    assert!(
        (near - Extent::default().near()).abs() < 1e-4,
        "pulled in, no nearer than its sphere allows: {near}"
    );
    // In the Links tool too, B with no link started is nothing.
    tool.set_links(true);
    assert!(tool.links);
    assert_eq!(tool.held, None, "choosing a tool drops the preview");
    assert_eq!(
        tool.update(&session, 41, press(Action::Back)).unwrap(),
        None
    );
    assert_eq!(
        tool.update(&session, 42, press(Action::Tools)).unwrap(),
        Some(Command::Tools)
    );
}

fn placing(extent: Extent) -> Tool {
    let mut tool = Tool::with_extents(Arc::from(vec![extent]));
    tool.monitor = true;
    tool.camera = Some(eye());
    tool.select(0);
    tool
}

#[test]
fn a_new_object_is_held_in_front_by_its_size_and_never_around_the_camera() {
    let session = session(2);
    let big = Extent {
        radius: 2.,
        offset: [0., 0., 1.],
    };
    assert_eq!(big.reach(), 3.);
    let mut tool = placing(big);
    let ghost = |tool: &mut Tool, wall: Option<f32>| {
        tool.update_with_surface(&session, 1, Input::default(), |_| wall)
            .unwrap();
        tool.ghost.unwrap().position[0]
    };
    assert_eq!(
        ghost(&mut tool, None),
        7.5,
        "open sky: two and a half reaches"
    );
    assert_eq!(
        ghost(&mut tool, Some(40.)),
        7.5,
        "a far wall does not pull it away"
    );
    assert_eq!(ghost(&mut tool, Some(5.)), 5., "a nearer wall pulls it in");
    assert_eq!(
        ghost(&mut tool, Some(0.5)),
        3. + NEAR_GAP,
        "against a wall the camera stays outside its sphere"
    );
    let mut small = placing(Extent {
        radius: 0.1,
        offset: [0.; 3],
    });
    assert_eq!(ghost(&mut small, None), MIN_HOLD);
    assert_eq!(small.extent(&session).map(|e| e.radius), Some(0.1));
}

#[test]
fn picking_up_a_big_object_up_close_keeps_its_frame_and_turning_keeps_the_view() {
    let mut session = session(2);
    let mut world = Fake::default();
    let frame = Frame {
        position: [1., 0., 0.],
        rotation: [0.; 3],
    };
    let id = session.place(&mut world, 1, 0, frame).unwrap();
    let mut tool = placing(Extent {
        radius: 2.,
        offset: [0.; 3],
    });
    tool.select(0);
    tool.update(&session, 2, press(Action::Back)).unwrap();
    assert_eq!(tool.placing, None);
    tool.update(&session, 3, press(Action::Accept)).unwrap();
    assert_eq!(tool.held, Some(id));
    assert_eq!(tool.ghost, Some(frame), "pickup never moves it");
    let turn = Input {
        rotating: true,
        look: [1., 0.],
        ..Default::default()
    };
    tool.update(&session, 4, turn).unwrap();
    assert_eq!(
        tool.camera,
        Some(eye()),
        "turning the object leaves the view"
    );
    assert_eq!(tool.ghost.unwrap().position, frame.position);
    let look = Input {
        look: [std::f32::consts::PI, 0.],
        ..Default::default()
    };
    tool.update(&session, 5, look).unwrap();
    let behind = tool.ghost.unwrap().position;
    assert!(
        (behind[0] + 1.).abs() < 1e-5 && behind[1].abs() < 1e-5,
        "it turns with the view and stays where it was taken in it: {behind:?}"
    );
}

/// Manipulating pushes a new object out from its hold distance and pulls it
/// in, never past a nearer wall nor nearer than its sphere and
/// [`NEAR_GAP`] allow; a push against the wall does not build up behind it.
#[test]
fn push_and_pull_move_the_hold_distance_within_the_wall_and_the_near_gap() {
    let session = session(2);
    let big = Extent {
        radius: 2.,
        offset: [0., 0., 1.],
    };
    let mut tool = placing(big);
    let step = |tool: &mut Tool, forward: f32, wall: Option<f32>| {
        let input = Input {
            rotating: true,
            movement: [0., forward, 0.],
            seconds: 0.1,
            ..Default::default()
        };
        tool.update_with_surface(&session, 1, input, |_| wall)
            .unwrap();
        tool.ghost.unwrap().position[0]
    };
    assert_eq!(
        step(&mut tool, 0., None),
        7.5,
        "starts at its hold distance"
    );
    let pushed = step(&mut tool, 1., None);
    assert!((pushed - 8.1).abs() < 1e-4, "pushed away: {pushed}");
    assert!(
        (step(&mut tool, 0., Some(5.)) - 5.).abs() < 1e-4,
        "a nearer wall pulls it in"
    );
    assert!(
        (step(&mut tool, 0., None) - 8.1).abs() < 1e-4,
        "with the wall gone it is back where it was pushed"
    );
    for _ in 0..10 {
        step(&mut tool, 1., Some(10.));
    }
    assert!(
        (step(&mut tool, 0., Some(10.)) - 10.).abs() < 1e-4,
        "pushing stops at the wall"
    );
    let pulled = step(&mut tool, -1., Some(10.));
    assert!(
        (pulled - 9.4).abs() < 1e-4,
        "pulling moves it in from the wall at once: {pulled}"
    );
    for _ in 0..40 {
        step(&mut tool, -1., None);
    }
    assert_eq!(
        step(&mut tool, 0., None),
        3. + NEAR_GAP,
        "pulled in no nearer than its sphere and the gap"
    );
    assert_eq!(big.near(), 3. + NEAR_GAP);
}

#[test]
fn movement_snap_uses_the_world_grid_or_the_pickups_own_grid() {
    let session = session(2);
    let mut tool = Tool::default();
    tool.monitor = true;
    tool.camera = Some(eye());
    tool.select(0);
    tool.distance = 1.37;
    tool.offset = [0.0, -0.29, 0.12];
    tool.set_movement_step(0.5);
    tool.update(&session, 1, Input::default()).unwrap();
    let world = tool.ghost.unwrap();
    assert_eq!(world.position, [1.5, 0.5, 0.0]);

    tool.set_axes(crate::tool::Axes::Local);
    tool.initial_position = [1.2, 0.0, 0.0];
    tool.initial_rotation = [std::f32::consts::FRAC_PI_2, 0.0, 0.0];
    tool.update(&session, 2, Input::default()).unwrap();
    let local = tool.ghost.unwrap();
    assert_eq!(local.position, [1.2, 0.5, 0.0]);

    // Turning about the pickup's own axes is delta * initial, not a
    // component-wise addition: a quarter pitch after a quarter yaw.
    tool.rotation = [0.0, std::f32::consts::FRAC_PI_2, 0.0];
    tool.update(&session, 3, Input::default()).unwrap();
    let turned = tool.ghost.unwrap();
    assert!(turned.rotation[0].abs() < 1e-4, "{:?}", turned.rotation);
    assert!((turned.rotation[1] - std::f32::consts::FRAC_PI_2).abs() < 1e-4);

    tool.set_movement_step(0.0);
    tool.set_rotation_step(0.0);
    tool.update(&session, 4, Input::default()).unwrap();
    assert_eq!(tool.ghost.unwrap().position, [1.37, 0.29, 0.12]);
}

// ---- aiming at every thing (docs/game/forge-map-variants.md, slice M9) ----

use crate::base_tests::{self as base, Call};
use crate::tool::{MARK_RADIUS, Show, in_game};
use crate::{BlockKind, Endpoint, Session, Thing, ThingKind, Value};

/// The invented map of `base_tests` (weapons at x -3 and 4 on the x axis, a
/// starting point at y -4, the netgame flags 0, 1 and 2 at x 0, 1 and 2 on
/// y 5: a teleporter sender, a CTF flag spawn and a teleporter receiver,
/// and an item spawner of every game type at y 6) with an added crate at
/// (6, -4, 0) whose palette item is a sphere of radius 1.
fn things_map() -> (Session, Tool, Endpoint) {
    let mut session = base::open(base::rows());
    let mut world = base::Fake::default();
    let id = session
        .place(
            &mut world,
            1,
            0,
            Frame {
                position: [6., -4., 0.],
                rotation: [0.; 3],
            },
        )
        .unwrap();
    let mut tool = Tool::with_extents(Arc::from(vec![Extent {
        radius: 1.,
        offset: [0.; 3],
    }]));
    tool.monitor = true;
    (session, tool, Endpoint::Added(id))
}

/// What the reticle picks from `position`, looking along `yaw`, with a wall
/// `wall` units away if any.
fn aim_from(
    session: &Session,
    tool: &mut Tool,
    position: [f32; 3],
    yaw: f32,
    wall: Option<f32>,
) -> Option<Endpoint> {
    tool.camera = Some(Camera {
        position,
        yaw,
        pitch: 0.,
    });
    tool.update_with_surface(session, 2, Input::default(), |_| wall)
        .unwrap();
    tool.aimed_thing.map(Endpoint::identity)
}

#[test]
fn the_nearest_thing_within_its_pick_radius_wins() {
    let (session, mut tool, crate_id) = things_map();
    let weapon = |i| Some(Endpoint::base(BlockKind::Weapons, i));
    let flag = |i| Some(Endpoint::base(BlockKind::NetgameFlags, i));
    let start = Some(Endpoint::base(BlockKind::StartingLocations, 0));
    let spawner = Some(Endpoint::base(BlockKind::NetgameEquipment, 0));
    let north = std::f32::consts::FRAC_PI_2;
    let this = Show::ThisGameType;
    // Engine indices: 1 CTF, 2 Slayer.
    type Row = (
        [f32; 3],
        f32,
        Option<f32>,
        Option<i32>,
        Show,
        Option<Endpoint>,
    );
    // (eye, yaw, wall, game, show, what the reticle picks)
    let table: [Row; 11] = [
        // Two weapons on the line: the nearer.
        ([-10., 0., 0.], 0., None, None, this, weapon(0)),
        // The nearer is outside its pick radius (0.5, widened 0.012 a
        // unit), the farther within its own, wider at its distance.
        ([-10., 0.6, 0.], 0., None, None, this, weapon(1)),
        // A wall before the first weapon: nothing.
        ([-10., 0., 0.], 0., Some(5.), None, this, None),
        // The marks: a teleporter, a starting point, an item spawner.
        ([-10., 5., 0.], 0., None, Some(1), this, flag(0)),
        ([0., -10., 0.], north, None, None, this, start),
        ([-10., 6., 0.], 0., None, Some(2), this, spawner),
        // A starting point's mark passed by, and the added crate behind
        // it, whose sphere is wider.
        ([-10., -3.2, 0.], 0., None, None, this, Some(crate_id)),
        // The CTF flag spawn is CTF's: aimed at in CTF, passed over in
        // Slayer for the teleporter behind it, aimed at in Slayer with Show
        // at All.
        ([0.5, 5., 0.], 0., None, Some(1), this, flag(1)),
        ([0.5, 5., 0.], 0., None, Some(2), this, flag(2)),
        (
            [0.5, 5., 0.],
            0.,
            None,
            Some(2),
            Show::AllGameTypes,
            flag(1),
        ),
        // With no game type known, every thing is this game type's.
        ([0.5, 5., 0.], 0., None, None, this, flag(1)),
    ];
    for (i, (eye, yaw, wall, game, show, want)) in table.into_iter().enumerate() {
        tool.game = game;
        tool.set_show(show);
        assert_eq!(
            aim_from(&session, &mut tool, eye, yaw, wall),
            want,
            "row {i}"
        );
        let added = match want {
            Some(Endpoint::Added(id)) => Some(id),
            _ => None,
        };
        assert_eq!(
            tool.aimed, added,
            "row {i}: `aimed` is an added object only"
        );
    }
    assert_eq!(MARK_RADIUS, 0.5);
}

fn thing(kind: ThingKind, fields: Vec<(&'static str, Value)>) -> Thing {
    Thing {
        id: Endpoint::base(BlockKind::NetgameEquipment, 0),
        block: BlockKind::NetgameEquipment,
        kind,
        own: true,
        frame: Frame::default(),
        fields,
        changed: false,
    }
}

#[test]
fn a_things_game_types_are_its_slots_or_its_flag_type() {
    let spawner = |slots: [i64; 4]| {
        thing(
            ThingKind::ItemSpawner,
            vec![
                ("game type 1", Value::Int(slots[0])),
                ("game type 2", Value::Int(slots[1])),
                ("game type 3", Value::Int(slots[2])),
                ("game type 4", Value::Int(slots[3])),
            ],
        )
    };
    let goal = |ty: i64| thing(ThingKind::Goal, vec![("type", Value::Int(ty))]);
    // Engine indices: 1 CTF, 2 Slayer, 3 Oddball, 4 King of the Hill,
    // 7 Juggernaut, 8 Territories, 9 Assault.
    let all: &[i32] = &[1, 2, 3, 4, 7, 8, 9];
    let table: [(Thing, &[i32], &[i32]); 9] = [
        (spawner([12, 12, 12, 12]), all, &[]),
        (spawner([2, -1, -1, -1]), &[2], &[1, 3, 4, 9]),
        // 13 is every engine but CTF and Assault.
        (spawner([13, -1, -1, -1]), &[2, 3, 4, 7, 8], &[1, 9]),
        // King of the Hill's slot is 0, its index 4.
        (spawner([0, -1, -1, -1]), &[4], &[2]),
        // CTF's slot matches Assault's engine, as the matcher has it.
        (spawner([1, -1, -1, -1]), &[1, 9], &[2]),
        (goal(0), &[1], &[2, 9]),
        // A hill corner.
        (goal(13), &[4], &[1, 2]),
        // A teleporter's type is every game type's.
        (goal(7), all, &[]),
        (thing(ThingKind::StartingPoint, vec![]), all, &[]),
    ];
    for (i, (t, has, lacks)) in table.iter().enumerate() {
        for &game in *has {
            assert!(in_game(t, Some(game)), "row {i} is in game type {game}");
        }
        for &game in *lacks {
            assert!(
                !in_game(t, Some(game)),
                "row {i} is not in game type {game}"
            );
        }
        assert!(in_game(t, None), "row {i}: no game type known");
    }
}

/// A on the map's own weapon picks it up without a word to the world; the
/// drop is one `MoveThing`, which the session makes one base move; delete
/// on an own thing is a `Remove`, and X opens its Properties.
#[test]
fn an_own_thing_is_picked_up_dropped_removed_and_opened_like_an_added_one() {
    let (mut session, mut tool, _) = things_map();
    let mut world = base::Fake::default();
    let weapon = Endpoint::base(BlockKind::Weapons, 0);
    assert_eq!(
        aim_from(&session, &mut tool, [-10., 0., 0.], 0., None),
        Some(weapon)
    );
    assert_eq!(
        tool.update(&session, 3, press(Action::Properties)).unwrap(),
        Some(Command::Properties)
    );
    tool.update(&session, 4, press(Action::Accept)).unwrap();
    assert_eq!(tool.held_thing.map(Endpoint::identity), Some(weapon));
    assert_eq!(tool.held, None, "an own thing is not an added object");
    assert_eq!(
        tool.ghost.unwrap().position,
        [-3., 0., 0.],
        "pickup never moves it"
    );
    assert_eq!(
        tool.extent(&session),
        Some(Extent::default()),
        "a rifle the Objects palette lacks"
    );
    let forward = Input {
        movement: [0., 1., 0.],
        seconds: 0.1,
        ..Default::default()
    };
    tool.update(&session, 5, forward).unwrap();
    assert!(
        world.calls.is_empty(),
        "a hold is the tool's until the drop"
    );
    let cmd = tool
        .update(&session, 6, press(Action::Accept))
        .unwrap()
        .unwrap();
    let Command::MoveThing { id, frame } = cmd else {
        panic!("expected the drop of an own thing, not {cmd:?}")
    };
    assert_eq!(id.identity(), weapon);
    assert!(id.print().is_some(), "an own thing is named with its print");
    assert!((frame.position[0] + 2.4).abs() < 1e-4, "{frame:?}");
    session.move_thing(&mut world, 6, id, frame).unwrap();
    tool.applied(&cmd);
    assert_eq!(tool.held_thing, None);
    assert_eq!(world.calls, [Call::BaseMove(BlockKind::Weapons, 0, frame)]);
    // Aimed at where it was dropped, delete is its removal.
    let cmd = tool
        .update(&session, 7, press(Action::Delete))
        .unwrap()
        .unwrap();
    assert_eq!(cmd, Command::Remove { id });
    session.remove(&mut world, 7, id).unwrap();
    tool.applied(&cmd);
    assert_eq!(world.calls[1..], [Call::BaseInert(BlockKind::Weapons, 0)]);
    // A removed thing is not aimed at: the next weapon on the line is.
    assert_eq!(
        aim_from(&session, &mut tool, [-10., 0., 0.], 0., None),
        Some(Endpoint::base(BlockKind::Weapons, 1))
    );
}

/// Overlays are all on until turned off; the Overlays action is a command
/// and an agent name, and a turned-off overlay is tool state only.
#[test]
fn overlays_start_on_and_the_action_opens_their_page() {
    use crate::tool::Overlay;
    let session = session(2);
    let mut tool = Tool::default();
    tool.monitor = true;
    tool.camera = Some(eye());
    assert!(Overlay::ALL.iter().all(|o| tool.overlays.on(*o)));
    assert_eq!(
        tool.update(&session, 1, press(Action::Overlays)).unwrap(),
        Some(Command::Overlays)
    );
    assert_eq!(Action::named("overlays"), Some(Action::Overlays));
    tool.overlays.set(Overlay::Markers, false);
    assert!(!tool.overlays.on(Overlay::Markers));
    assert!(tool.overlays.on(Overlay::Monitors) && tool.overlays.on(Overlay::Gizmo));
    tool.overlays.set(Overlay::Markers, true);
    assert_eq!(tool.overlays, crate::tool::Overlays::default());
    tool.set_links(true);
    assert_eq!(
        tool.update(&session, 2, press(Action::Overlays)).unwrap(),
        Some(Command::Overlays),
        "the Links tool has the same button"
    );
}

/// Reset orientation (Properties) sets what is held upright at the yaw
/// the editor's snap rounding gives, and `take_up` holds a thing at once
/// where it was made, as Duplicate's copy is.
#[test]
fn reset_orientation_uprights_the_hold_and_take_up_holds_a_new_thing() {
    let (mut session, mut tool, first) = things_map();
    assert_eq!(
        aim_from(&session, &mut tool, [2., -4., 0.], 0., None),
        Some(first.identity())
    );
    tool.update(&session, 3, press(Action::Accept)).unwrap();
    assert!(tool.held_thing.is_some());
    let turn = Input {
        rotating: true,
        look: [0.5, 0.3],
        ..Default::default()
    };
    tool.update(&session, 4, turn).unwrap();
    assert_ne!(tool.ghost.unwrap().rotation, [0.; 3]);
    tool.set_orientation(0.75);
    assert_eq!(tool.ghost.unwrap().rotation, [0.75, 0., 0.]);
    let Command::MoveThing { frame, .. } = tool
        .update(&session, 5, press(Action::Accept))
        .unwrap()
        .unwrap()
    else {
        panic!("the drop")
    };
    assert_eq!(frame.rotation, [0.75, 0., 0.], "the reset is what drops");
    // The copy of another object is held where the add put it.
    let mut world = base::Fake::default();
    let second = session
        .place(
            &mut world,
            6,
            0,
            Frame {
                position: [0., 0., -4.],
                rotation: [0.; 3],
            },
        )
        .unwrap();
    let at = Frame {
        position: [1., 0., -3.],
        rotation: [0.25, 0., 0.],
    };
    tool.take_up(&session, Endpoint::Added(second), Some(second), at);
    assert_eq!(tool.held_thing, Some(Endpoint::Added(second)));
    assert_eq!(tool.held, Some(second));
    assert_eq!(tool.ghost.unwrap().rotation, [0.25, 0., 0.]);
    let Command::Move { id, frame } = tool
        .update(&session, 7, press(Action::Accept))
        .unwrap()
        .unwrap()
    else {
        panic!("the drop of the copy")
    };
    assert_eq!(id, second);
    assert_eq!(frame.rotation, [0.25, 0., 0.]);
}
