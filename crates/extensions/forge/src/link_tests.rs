//! The Links tool and session over an invented map with four teleporter
//! flags and one machine tag.
use std::collections::BTreeMap;

use super::{Call, Fake};
use crate::{
    BlockKind, Frame, Linkable, OpId, PaletteItem, Session, TagKey, Teleporter,
    camera::Camera,
    tool::{Action, Command, Input, Tool},
};
use forge_doc::{
    BuilderVersions, Edit, Endpoint, Forge, ForgeDoc, Limits, Link, MapRef, Sha256, WorldBounds,
};

fn teleporter(index: u32, sender: bool, channel: i16, position: [f32; 3]) -> Teleporter {
    Teleporter {
        index,
        position,
        sender,
        channel,
    }
}

/// Flags 0 and 1 are a pair on channel 1, flags 3 and 2 a pair on channel 2.
fn session() -> Session {
    let base = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions {
            forge_build: 1,
            pathfind: 1,
            bots: 1,
        },
        base: MapRef {
            map: "linkroom".into(),
            sha256: Sha256::of(b"an invented linked room"),
        },
        sources: vec![],
        title: "Links".into(),
        author: "Builder".into(),
        description: String::new(),
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
    };
    let limits = Limits {
        base_counts: BTreeMap::from([(BlockKind::Machines, 2), (BlockKind::NetgameFlags, 5)]),
        world: Some(WorldBounds {
            min: [-20.; 3],
            max: [20.; 3],
        }),
        ..Default::default()
    };
    let palette = vec![PaletteItem {
        block: BlockKind::Machines,
        tag: TagKey {
            group: "mach".into(),
            path: r"fixture\door".into(),
        },
    }];
    Session::new(base, limits, palette, 8)
        .unwrap()
        .with_teleporters(vec![
            teleporter(0, true, 1, [5., 0., 0.]),
            teleporter(1, false, 1, [-5., 0., 0.]),
            teleporter(2, false, 2, [8., 0., 3.]),
            teleporter(3, true, 2, [-8., 0., 3.]),
        ])
        .unwrap()
}

fn channels(s: &Session) -> Vec<i16> {
    s.teleporters().iter().map(|t| t.channel).collect()
}

fn flag(index: u32) -> Endpoint {
    Endpoint::Base {
        block: BlockKind::NetgameFlags,
        index,
    }
}

#[test]
fn a_linked_pair_writes_one_channel_and_undo_and_unlink_restore_it() {
    let mut s = session();
    let base = s.document().clone();
    let mut w = Fake::default();
    let id = s
        .link(&mut w, 1, Linkable::Teleporter(2), Linkable::Teleporter(0))
        .unwrap();
    assert_eq!(
        s.document().ops[0].edit,
        Edit::Forge(Forge::Link(Link::Teleporter {
            sender: flag(0),
            receiver: flag(2),
            channel: 1,
        }))
    );
    // The receiver joins the sender's channel; the sender already has it.
    assert_eq!(w.calls, [Call::Channel(2, 1)]);
    assert_eq!(channels(&s), [1, 1, 1, 2]);
    s.undo(&mut w, 2).unwrap();
    assert_eq!(channels(&s), [1, 1, 2, 2]);
    s.redo(&mut w, 3).unwrap();
    s.unlink(&mut w, 4, id).unwrap();
    assert_eq!(channels(&s), [1, 1, 2, 2]);
    assert_eq!(
        w.calls,
        [
            Call::Channel(2, 1),
            Call::Channel(2, 2),
            Call::Channel(2, 1),
            Call::Channel(2, 2),
        ]
    );
    assert_eq!(forge_doc::fold(&base, s.log()).unwrap(), *s.document());
}

#[test]
fn a_flag_on_an_active_channel_brings_it_and_two_senders_are_refused() {
    let mut s = session();
    let mut w = Fake::default();
    s.link(&mut w, 1, Linkable::Teleporter(0), Linkable::Teleporter(2))
        .unwrap();
    // Flag 2 is linked on channel 1, so the second sender takes channel 1.
    s.link(&mut w, 2, Linkable::Teleporter(3), Linkable::Teleporter(2))
        .unwrap();
    assert_eq!(channels(&s), [1, 1, 1, 1]);
    assert_eq!(w.calls, [Call::Channel(2, 1), Call::Channel(3, 1)]);
    let before = s.document().clone();
    let refusals = [
        (
            Linkable::Teleporter(0),
            Linkable::Teleporter(3),
            "sender to a receiver",
        ),
        (
            Linkable::Teleporter(0),
            Linkable::Teleporter(4),
            "not a teleporter",
        ),
        (
            Linkable::Teleporter(0),
            Linkable::Object(OpId(9)),
            "a machine to a machine",
        ),
        (
            Linkable::Teleporter(0),
            Linkable::Teleporter(2),
            "already exists",
        ),
    ];
    for (a, b, why) in refusals {
        let error = s.link(&mut w, 3, a, b).unwrap_err();
        assert!(error.contains(why), "{error}");
    }
    assert_eq!(*s.document(), before);
    assert_eq!(w.calls.len(), 2);
}

#[test]
fn a_proposed_link_to_a_flag_that_is_not_a_teleporter_is_refused() {
    let mut s = session();
    let mut w = Fake::default();
    let op = forge_doc::Op {
        id: OpId(1),
        edit: Edit::Forge(Forge::Link(Link::Teleporter {
            sender: flag(1),
            receiver: flag(2),
            channel: 4,
        })),
    };
    let error = s.propose(&mut w, 1, op, None).unwrap_err();
    assert!(error.contains("not a teleporter sender"), "{error}");
    assert!(s.document().ops.is_empty() && w.calls.is_empty());
}

#[test]
fn machines_join_a_group_for_the_rebuild_and_delete_ends_their_link() {
    let mut s = session();
    let mut w = Fake::default();
    let at = |x| Frame {
        position: [x, 0., 0.],
        rotation: [0.; 3],
    };
    let a = s.place(&mut w, 1, 0, at(1.)).unwrap();
    let b = s.place(&mut w, 2, 0, at(2.)).unwrap();
    let calls = w.calls.len();
    s.link(&mut w, 3, Linkable::Object(b), Linkable::Object(a))
        .unwrap();
    assert_eq!(
        s.document().ops.last().unwrap().edit,
        Edit::Forge(Forge::Link(Link::Device {
            from: Endpoint::Added(a),
            to: Endpoint::Added(b),
        }))
    );
    assert_eq!(w.calls.len(), calls, "a device group waits for the rebuild");
    assert_eq!(s.pending_rebuild(), 1);
    s.delete(&mut w, 4, a).unwrap();
    assert_eq!(s.pending_rebuild(), 0);
    assert!(matches!(
        s.document().ops[s.document().ops.len() - 2].edit,
        Edit::Forge(Forge::Unlink { .. })
    ));
    // One undo brings back the machine and its link together.
    s.undo(&mut w, 5).unwrap();
    assert_eq!(s.pending_rebuild(), 1);
    assert_eq!(s.objects().len(), 2);
}

fn press(action: Action) -> Input {
    Input {
        action: Some(action),
        ..Default::default()
    }
}

fn eye(z: f32) -> Option<Camera> {
    Some(Camera {
        position: [0., 0., z],
        yaw: 0.,
        pitch: 0.,
    })
}

#[test]
fn the_links_tool_starts_on_one_end_links_on_the_other_and_unlinks() {
    let mut s = session();
    let mut w = Fake::default();
    let mut tool = Tool::default();
    tool.update(&s, 1, press(Action::Toggle)).unwrap();
    tool.camera = eye(0.);
    assert_eq!(tool.update(&s, 2, press(Action::Link)).unwrap(), None);
    assert!(tool.links);
    assert_eq!(tool.update(&s, 3, press(Action::Accept)).unwrap(), None);
    assert_eq!(tool.link_from, Some(Linkable::Teleporter(0)));
    // The first end is not a target for the second.
    assert_eq!(tool.update(&s, 4, press(Action::Accept)).unwrap(), None);
    tool.camera = eye(3.);
    let cmd = tool.update(&s, 5, press(Action::Accept)).unwrap().unwrap();
    assert_eq!(
        cmd,
        Command::Link {
            from: Linkable::Teleporter(0),
            to: Linkable::Teleporter(2),
        }
    );
    let Command::Link { from, to } = cmd else {
        unreachable!()
    };
    let id = s.link(&mut w, 5, from, to).unwrap();
    tool.applied(&cmd);
    assert_eq!(tool.link_from, None);
    assert_eq!(
        tool.update(&s, 6, press(Action::Delete)).unwrap(),
        Some(Command::Unlink { link: id })
    );
    assert_eq!(
        tool.update(&s, 7, press(Action::Properties)).unwrap(),
        Some(Command::Properties)
    );
    // Back cancels a started link before it opens the menu.
    tool.update(&s, 8, press(Action::Accept)).unwrap();
    assert!(tool.link_from.is_some());
    assert_eq!(tool.update(&s, 9, press(Action::Back)).unwrap(), None);
    assert_eq!(tool.link_from, None);
    // Link with nothing aimed returns to the Objects tool.
    tool.camera = eye(10.);
    tool.update(&s, 10, press(Action::Link)).unwrap();
    assert!(!tool.links);
}
