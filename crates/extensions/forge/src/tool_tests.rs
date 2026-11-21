//! The same monitor gestures used by the tick and dialog routes, over an invented map.
use super::{Fake, session};
use crate::{
    Frame,
    camera::Camera,
    tool::{Action, Command, Input, Tool},
};

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
    assert_eq!(frame.position, [3., 0., 0.]);
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
        near < 3. && near >= 0.5 - 1e-4,
        "pulled in, no nearer than the floor: {near}"
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
