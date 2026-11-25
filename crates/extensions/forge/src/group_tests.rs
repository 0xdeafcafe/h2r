//! Object groups over invented crates: one op to make a group, moving a
//! member moves the group in one undo step, and removing a member keeps
//! the rest grouped.
use super::{Fake, frame, session};
use crate::{Endpoint, Frame, OpId, Session, session::carried};
use crate::{
    camera::Camera,
    tool::{Action, Command, Input, Tool},
};
use forge_doc::{Edit, Forge};

fn near(a: Frame, b: Frame) {
    for i in 0..3 {
        assert!(
            (a.position[i] - b.position[i]).abs() < 1e-4
                && (a.rotation[i] - b.rotation[i]).abs() < 1e-4,
            "{a:?} is not {b:?}"
        );
    }
}

fn frames(s: &Session, ids: &[OpId]) -> Vec<Frame> {
    ids.iter()
        .map(|id| s.thing(Endpoint::Added(*id)).unwrap().frame)
        .collect()
}

/// Three crates at x = 0, 2 and 4, grouped as "Roof".
fn grouped() -> (Session, Fake, Vec<OpId>, OpId) {
    let mut s = session(8);
    let mut w = Fake::default();
    let ids: Vec<OpId> = [0., 2., 4.]
        .map(|x| s.place(&mut w, 1, 0, frame(x, 0.)).unwrap())
        .to_vec();
    let members: Vec<Endpoint> = ids.iter().map(|i| Endpoint::Added(*i)).collect();
    let group = s.group(&mut w, 2, "Roof", &members, false).unwrap();
    (s, w, ids, group)
}

#[test]
fn a_group_is_one_op_and_a_member_knows_its_group() {
    let (s, _, ids, group) = grouped();
    assert!(matches!(
        &s.document().ops.last().unwrap().edit,
        Edit::Forge(Forge::Group(g)) if g.name == "Roof" && g.members.len() == 3
    ));
    assert_eq!(s.document().format, forge_doc::FORMAT_GROUPS);
    let info = s.group_of(Endpoint::Added(ids[1])).unwrap();
    assert_eq!(
        (info.id, info.name.as_str(), info.locked),
        (group, "Roof", false)
    );
    assert_eq!(s.groups().len(), 1);
    assert!(!s.locked(Endpoint::Added(ids[0])));
}

#[test]
fn changing_the_pivot_keeps_the_group_and_is_one_undo_step() {
    let (mut s, mut w, _, group) = grouped();
    s.set_group_locked(&mut w, 3, group, true).unwrap();
    let before = s.groups().pop().unwrap();
    let count = s.document().ops.len();
    let calls = w.calls.len();
    s.set_group_pivot(&mut w, 4, group, false).unwrap();
    assert_eq!(s.document().ops.len(), count);

    s.set_group_pivot(&mut w, 5, group, true).unwrap();
    let after = s.groups().pop().unwrap();
    assert_eq!(after.name, before.name);
    assert_eq!(after.members, before.members);
    assert!(after.locked && after.pivot_first);
    assert_eq!(w.calls.len(), calls);
    assert_eq!(s.document().ops.len(), count + 2);
    assert_eq!(s.document().format, forge_doc::FORMAT_GROUPS);
    assert!(s.set_group_pivot(&mut w, 6, group, false).is_err());
    assert_eq!(s.groups(), [after.clone()]);
    s.undo(&mut w, 7).unwrap();
    assert_eq!(s.groups(), [before]);
    s.redo(&mut w, 8).unwrap();
    assert_eq!(s.groups(), [after.clone()]);

    let mut shared = s.share(crate::net::HOST_SEAT, "Host");
    assert!(shared.set_group_pivot(&mut w, 9, after.id, false).is_err());
    assert!(shared.take_outgoing().is_empty());
    assert_eq!(shared.groups(), [after]);
}

#[test]
fn moving_a_member_moves_the_group_in_one_undo_step_and_undo_puts_it_back() {
    let (mut s, mut w, ids, _) = grouped();
    let before = frames(&s, &ids);
    let ops_before = s.document().ops.len();
    s.move_thing(&mut w, 3, Endpoint::Added(ids[1]), frame(12., 0.))
        .unwrap();
    let after = frames(&s, &ids);
    for (a, b) in after.iter().zip(&before) {
        near(
            *a,
            Frame {
                position: [b.position[0] + 10., b.position[1], b.position[2]],
                rotation: b.rotation,
            },
        );
    }
    // One undo step: every set of every member.
    assert!(s.document().ops.len() > ops_before + 2);
    s.undo(&mut w, 4).unwrap();
    for (a, b) in frames(&s, &ids).iter().zip(&before) {
        near(*a, *b);
    }
    assert_eq!(s.groups().len(), 1, "undoing the move keeps the group");
    // The same through the added object's own gesture.
    s.move_object(&mut w, 5, ids[0], frame(10., 0.)).unwrap();
    assert!((frames(&s, &ids)[2].position[0] - 14.).abs() < 1e-4);
}

#[test]
fn turning_a_member_turns_the_group_about_its_centre() {
    let (mut s, mut w, ids, _) = grouped();
    // The middle crate is the centre; a quarter turn about it.
    let quarter = std::f32::consts::FRAC_PI_2;
    s.move_thing(&mut w, 3, Endpoint::Added(ids[1]), frame(2., quarter))
        .unwrap();
    let after = frames(&s, &ids);
    near(
        after[0],
        Frame {
            position: [2., 1. - 2., 2.],
            rotation: [quarter, 0., 0.],
        },
    );
    near(after[1], frame(2., quarter));
    near(
        after[2],
        Frame {
            position: [2., 1. + 2., 2.],
            rotation: [quarter, 0., 0.],
        },
    );
}

#[test]
fn the_pivot_is_the_first_member_when_the_group_says_so() {
    let now = [frame(0., 0.), frame(2., 0.), frame(4., 0.)];
    let quarter = std::f32::consts::FRAC_PI_2;
    // Carry the last member; the group turns about the first.
    let to = carried(
        &now,
        true,
        2,
        Frame {
            position: [0., 1. + 4., 2.],
            rotation: [quarter, 0., 0.],
        },
    );
    near(
        to[0],
        Frame {
            position: [0., 1., 2.],
            rotation: [quarter, 0., 0.],
        },
    );
    near(
        to[1],
        Frame {
            position: [0., 3., 2.],
            rotation: [quarter, 0., 0.],
        },
    );
    near(
        to[2],
        Frame {
            position: [0., 5., 2.],
            rotation: [quarter, 0., 0.],
        },
    );
}

#[test]
fn removing_a_member_keeps_the_rest_grouped_and_undo_restores_all() {
    let (mut s, mut w, ids, _) = grouped();
    s.delete(&mut w, 3, ids[0]).unwrap();
    let info = s.group_of(Endpoint::Added(ids[1])).unwrap();
    assert_eq!(info.name, "Roof");
    assert_eq!(
        info.members,
        [Endpoint::Added(ids[1]), Endpoint::Added(ids[2])]
    );
    // One more member goes: a group of one is no group.
    s.delete(&mut w, 4, ids[1]).unwrap();
    assert!(s.groups().is_empty());
    s.undo(&mut w, 5).unwrap();
    s.undo(&mut w, 6).unwrap();
    assert_eq!(s.groups()[0].members.len(), 3);
}

#[test]
fn ungroup_and_lock_are_ops_and_a_second_group_refuses_a_grouped_member() {
    let (mut s, mut w, ids, group) = grouped();
    s.set_group_locked(&mut w, 3, group, true).unwrap();
    assert!(s.locked(Endpoint::Added(ids[2])));
    s.set_group_locked(&mut w, 4, group, true).unwrap();
    assert_eq!(s.document().ops.len(), 5, "a lock it already has is no op");
    let err = s
        .group(
            &mut w,
            5,
            "Again",
            &[Endpoint::Added(ids[0]), Endpoint::Added(ids[1])],
            false,
        )
        .unwrap_err();
    assert!(err.contains("already in a group"), "{err}");
    s.ungroup(&mut w, 6, group).unwrap();
    assert!(s.groups().is_empty());
    assert!(s.ungroup(&mut w, 7, group).is_err());
    let err = s
        .group(
            &mut w,
            8,
            "",
            &[Endpoint::Added(ids[0]), Endpoint::Added(ids[1])],
            false,
        )
        .unwrap_err();
    assert!(err.contains("must have a name"), "{err}");
}

#[test]
fn a_saved_group_loads_back() {
    let (s, _, ids, _) = grouped();
    let json = s.document().canonical_json().unwrap();
    let doc = forge_doc::ForgeDoc::from_bytes(
        json.as_bytes(),
        &forge_doc::Limits {
            base_counts: std::collections::BTreeMap::from([(crate::BlockKind::Crates, 7)]),
            ..Default::default()
        },
    )
    .unwrap();
    let groups = doc
        .group_targets(&std::collections::BTreeMap::from([(
            crate::BlockKind::Crates,
            7,
        )]))
        .unwrap();
    assert_eq!(groups[0].1.members[0], Endpoint::Added(ids[0]));
}

fn press(action: Action) -> Input {
    Input {
        action: Some(action),
        ..Default::default()
    }
}

fn monitor() -> Tool {
    let mut tool = Tool::default();
    tool.monitor = true;
    tool.camera = Some(Camera {
        position: [0.; 3],
        yaw: 0.,
        pitch: 0.,
    });
    tool
}

/// Two crates in line with the view, at 5 and 9 units, grouped.
fn in_line() -> (Session, Fake, [OpId; 2], OpId) {
    let mut s = session(8);
    let mut w = Fake::default();
    let at = |x: f32| Frame {
        position: [x, 0., 0.],
        rotation: [0.; 3],
    };
    let ids = [5., 9.].map(|x| s.place(&mut w, 1, 0, at(x)).unwrap());
    let group = s
        .group(
            &mut w,
            2,
            "Pair",
            &[Endpoint::Added(ids[0]), Endpoint::Added(ids[1])],
            false,
        )
        .unwrap();
    (s, w, ids, group)
}

#[test]
fn a_selection_is_tool_state_that_b_clears_and_a_gone_thing_leaves() {
    let (mut s, mut w, ids, _) = in_line();
    let mut tool = monitor();
    tool.toggle_selected(Endpoint::Added(ids[0]));
    tool.toggle_selected(Endpoint::Added(ids[1]));
    assert_eq!(tool.selection().len(), 2);
    tool.toggle_selected(Endpoint::Added(ids[1]));
    assert!(tool.is_selected(Endpoint::Added(ids[0])));
    assert!(!tool.is_selected(Endpoint::Added(ids[1])));
    let log = s.document().ops.len();
    // B with nothing held lets it go, and nothing reaches the document.
    assert_eq!(tool.update(&s, 3, press(Action::Back)).unwrap(), None);
    assert!(tool.selection().is_empty());
    assert_eq!(s.document().ops.len(), log);
    // A selected thing that is deleted leaves the selection.
    let lone = s.place(&mut w, 4, 0, frame(1., 0.)).unwrap();
    tool.toggle_selected(Endpoint::Added(lone));
    s.delete(&mut w, 5, lone).unwrap();
    tool.update(&s, 6, Input::default()).unwrap();
    assert!(tool.selection().is_empty());
    // Making the group lets the selection go.
    tool.toggle_selected(Endpoint::Added(ids[0]));
    tool.applied(&Command::Group {
        name: "x".into(),
        members: vec![],
    });
    assert!(tool.selection().is_empty());
}

#[test]
fn a_locked_group_is_not_aimed_at_and_an_unlocked_one_is() {
    let (mut s, mut w, ids, group) = in_line();
    let mut tool = monitor();
    tool.update(&s, 3, Input::default()).unwrap();
    assert_eq!(tool.aimed_thing, Some(Endpoint::Added(ids[0])));
    s.set_group_locked(&mut w, 4, group, true).unwrap();
    tool.update(&s, 5, Input::default()).unwrap();
    assert_eq!(tool.aimed_thing, None);
    s.set_group_locked(&mut w, 6, group, false).unwrap();
    tool.update(&s, 7, Input::default()).unwrap();
    assert_eq!(tool.aimed_thing, Some(Endpoint::Added(ids[0])));
}

#[test]
fn a_on_a_member_picks_the_group_up_and_the_drop_moves_every_member() {
    let (mut s, mut w, ids, _) = in_line();
    let mut tool = monitor();
    tool.update(&s, 3, Input::default()).unwrap();
    tool.update(&s, 4, press(Action::Accept)).unwrap();
    assert_eq!(tool.held_thing, Some(Endpoint::Added(ids[0])));
    let cmd = tool.update(&s, 5, press(Action::Accept)).unwrap().unwrap();
    let Command::Move { id, frame: to } = cmd else {
        panic!("a held object drops with a move")
    };
    let moved = Frame {
        position: [to.position[0], to.position[1] + 3., to.position[2]],
        ..to
    };
    s.move_object(&mut w, 6, id, moved).unwrap();
    let after = frames(&s, &ids);
    assert!(
        (after[1].position[1] - 3.).abs() < 1e-4,
        "the other member followed"
    );
    assert!((after[0].position[1] - 3.).abs() < 1e-4);
    assert!((after[1].position[0] - 9.).abs() < 1e-3, "{after:?}");
}
