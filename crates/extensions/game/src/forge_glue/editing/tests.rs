use super::*;
use agent::forge::local::OBJECT_GONE;
use forge::{BlockKind, Frame, TagKey};
use forge_doc::{BuilderVersions, ForgeDoc, Limits, MapRef, WorldBounds};

fn editor() -> Editor {
    let item = PaletteItem {
        block: BlockKind::Crates,
        tag: TagKey {
            group: "bloc".into(),
            path: r"test\shelf".into(),
        },
    };
    let limits = Limits {
        world: Some(WorldBounds {
            min: [-10.; 3],
            max: [10.; 3],
        }),
        base_counts: [(item.block, 2)].into_iter().collect(),
        ..Limits::default()
    };
    let base = ForgeDoc {
        format: forge_doc::FORMAT,
        builder: BuilderVersions::CURRENT,
        base: MapRef {
            map: "testroom".into(),
            sha256: Sha256::of(b"test room"),
        },
        sources: vec![],
        recipes: vec![],
        imports: vec![],
        ops: vec![],
        expect: None,
        title: "Test room".into(),
        author: "Builder".into(),
        description: String::new(),
    };
    let opened = Opened {
        session: Session::new(base, limits, vec![item], 8).unwrap(),
        tags: vec![42],
        walls: vec![],
    };
    Editor::new(opened, Path::new("."), 1, calls::shared())
}

#[test]
fn palette_to_properties_to_confirmed_engine_handle() {
    let mut e = editor();
    calls::lock(&e.calls).arm();
    e.tool.monitor = true;
    e.command(1, Command::Palette).unwrap();
    e.command(2, Command::DialogAccept).unwrap(); // Crates
    e.command(3, Command::DialogAccept).unwrap(); // Shelf
    assert!(e.dialog.is_none());
    assert_eq!(e.tool.placing, Some(0));
    let frame = Frame {
        position: [3., 0., 0.],
        rotation: [0.; 3],
    };
    e.tool.ghost = Some(frame);
    e.command(4, Command::Properties).unwrap();
    e.command(5, Command::DialogAccept).unwrap(); // Heading
    e.command(6, Command::DialogDown).unwrap();
    e.command(7, Command::DialogAccept).unwrap();
    let frame = e.tool.ghost.unwrap();
    assert_eq!(frame.rotation[0], 45f32.to_radians());
    e.command(8, Command::Place { item: 0, frame }).unwrap();
    assert!(e.session.log().is_empty());
    assert_eq!(e.tool.placing, Some(0));
    let work = calls::lock(&e.calls).drain();
    assert_eq!(work.len(), 1);
    let (ticket, call) = work.into_iter().next().unwrap();
    assert!(
        matches!(call, calls::Call::Forge { steps, .. } if steps == [
            calls::ForgeStep::Make { tag: 42, frame },
        ])
    );
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[31]})));
    e.poll(9);
    assert_eq!(e.session.log().len(), 1);
    assert_eq!(e.session.objects()[0].handle, 31);
    assert_eq!(e.tool.placing, None);
    let id = e.session.objects()[0].id;
    let live = calls::ForgeTarget::Live(agent_proto::ObjectId {
        kind: agent_proto::Kind::Crate,
        key: 31,
    });

    // The game refuses a stale object by name: the notice says so and the
    // log stays where it was.
    e.command(10, Command::Delete { id }).unwrap();
    let (ticket, call) = calls::lock(&e.calls).drain().pop().unwrap();
    assert!(
        matches!(call, calls::Call::Forge { steps, .. } if steps == [
            calls::ForgeStep::Delete { target: live },
        ])
    );
    let gone = format!("{}: crate.0000001f is no longer in the world", OBJECT_GONE);
    calls::lock(&e.calls).finish(
        ticket,
        Err(agent_proto::RpcError::new(
            agent_proto::sim::REFUSED,
            gone.clone(),
        )),
    );
    e.poll(11);
    assert_eq!(e.notice, Some((gone, 11)));
    assert_eq!(e.session.log().len(), 1);
    assert_eq!(e.session.objects().len(), 1);

    // A confirmed delete removes the live object, and undo restores it.
    e.command(12, Command::Delete { id }).unwrap();
    let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[]})));
    e.poll(13);
    assert_eq!(e.session.log().len(), 2);
    assert!(e.session.objects().is_empty());
    e.command(14, Command::Undo).unwrap();
    let (ticket, call) = calls::lock(&e.calls).drain().pop().unwrap();
    assert!(
        matches!(call, calls::Call::Forge { steps, .. } if steps == [
            calls::ForgeStep::Make { tag: 42, frame },
        ])
    );
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[32]})));
    e.poll(15);
    assert_eq!(e.session.objects()[0].id, id);
    assert_eq!(e.session.objects()[0].handle, 32);
}
