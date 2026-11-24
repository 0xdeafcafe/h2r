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
        extents: vec![Extent {
            radius: 1.,
            offset: [0.; 3],
        }],
        refused: vec![r"test\hum: sound scenery has no body to place or see".into()],
        walls: vec![],
        names: Default::default(),
    };
    Editor::new(opened, Path::new("."), 1, calls::shared())
}

#[test]
fn palette_to_properties_to_confirmed_engine_handle() {
    let mut e = editor();
    calls::lock(&e.calls).arm();
    e.tool.monitor = true;
    // X with nothing aimed at: the object menu, on its groups.
    e.command(1, Command::Palette).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Objects));
    let objects = screens::screen(Kind::Objects).sheet();
    assert_eq!(objects.rows.len(), 2, "one group, Crates, and the AI page");
    assert_eq!(objects.rows[0].label, "Crates");
    assert_eq!(objects.rows[1].label, "AI and objectives");
    // The picker's choice: the shelf.
    e.choose(2, Choice::Select(0)).unwrap();
    assert_eq!(e.tool.placing, Some(0));
    let frame = Frame {
        position: [3., 0., 0.],
        rotation: [0.; 3],
    };
    e.tool.ghost = Some(frame);
    // X while placing: Properties, its Coordinates row first.
    e.command(4, Command::Properties).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Properties));
    let props = screens::screen(Kind::Properties).sheet();
    assert_eq!(props.title, "SHELF");
    assert_eq!(props.rows[0].label, "Coordinates...");
    e.choose(7, Choice::Set(Prop::Heading, 1)).unwrap();
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

/// The tools and their settings are screens of their own; a choice on
/// them sets the tool, and the screens' facts follow it.
#[test]
fn tools_and_settings_open_and_their_choices_set_the_tool() {
    let mut e = editor();
    e.tool.monitor = true;
    e.command(1, Command::Tools).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Tools));
    e.choose(2, Choice::Tool(ToolKind::Links)).unwrap();
    assert!(e.tool.links);
    e.command(3, Command::Settings).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Settings));
    e.choose(4, Choice::Set(Prop::RotationSnap, 3)).unwrap();
    assert_eq!(e.rotation_step, 3);
    e.choose(5, Choice::Set(Prop::MovementSnap, 2)).unwrap();
    assert_eq!(e.movement_step, 2);
    assert_eq!(
        e.status()["movement_snap"].as_f64().unwrap() as f32,
        0.1_f32
    );
    e.choose(6, Choice::Set(Prop::Axes, 1)).unwrap();
    assert!(e.local_axes && e.local_axes());
    assert_eq!(e.status()["axes"], "local");
    // The Forge menu is GAME MENU's, refused by name here.
    assert!(e.command(7, Command::Menu).unwrap_err().contains("Start"));
}

/// A second screen while one is opening is refused by name, never
/// stacked.
#[test]
fn one_screen_at_a_time() {
    let mut e = editor();
    e.tool.monitor = true;
    e.command(1, Command::Palette).unwrap();
    assert_eq!(
        e.command(2, Command::Tools),
        Err("A Forge screen is up.".into())
    );
    screens::take_request();
}

#[test]
fn the_status_names_what_the_palette_left_out_and_the_ghost_carries_its_size() {
    let mut e = editor();
    let status = e.status();
    assert_eq!(status["palette"], 1);
    assert_eq!(
        status["palette_refused"][0],
        r"test\hum: sound scenery has no body to place or see"
    );
    // The map variant the session would save, and what the tool aims at.
    assert_eq!(status["variant"]["name"], Value::Null);
    assert_eq!(status["variant"]["title"], "Test room");
    assert_eq!(status["variant"]["tier"], "variant");
    assert_eq!(status["variant"]["dirty"], false);
    assert_eq!(status["show"], "this_game_type");
    assert_eq!(status["aimed_thing"], Value::Null);
    e.tool.monitor = true;
    e.tool.select(0);
    e.tool.ghost = Some(Frame::default());
    let (_, extent) = e.ghost().unwrap();
    assert_eq!(extent.radius, 1.);
}

/// An editor over a map with one weapon of its own, a rifle at (2, 0, 0),
/// and its starting point 0 at (0, 3, 0).
fn editor_with_own_things() -> Editor {
    use forge::{BaseRow, BaseRows, Value};
    let float = |v: f32| forge_doc::F32::new(v).unwrap();
    let vec3 = |v: [f32; 3]| Value::Vec3(v.map(float));
    let row = |block: BlockKind, fields: Vec<Value>| BaseRow {
        block,
        index: 0,
        raw: vec![1; map::scenario_layout::block(block).stride as usize],
        print: forge_doc::print(block, &fields).unwrap(),
        fields,
    };
    let rifle = row(
        BlockKind::Weapons,
        vec![
            Value::Tag(Some(TagKey {
                group: "weap".into(),
                path: r"test\battle_rifle".into(),
            })),
            Value::Int(-1),
            Value::Int(0),
            vec3([2., 0., 0.]),
            vec3([0.; 3]),
            Value::Float(float(0.)),
        ],
    );
    let start = row(
        BlockKind::StartingLocations,
        vec![
            vec3([0., 3., 0.]),
            Value::Float(float(0.)),
            Value::Int(0),
            Value::Int(-1),
        ],
    );
    let limits = Limits {
        world: Some(WorldBounds {
            min: [-10.; 3],
            max: [10.; 3],
        }),
        base_counts: [(BlockKind::Weapons, 1), (BlockKind::StartingLocations, 1)]
            .into_iter()
            .collect(),
        ..Limits::default()
    };
    let mut e = editor();
    let base = e.session.document().clone();
    e.session = Arc::new(
        Session::new(base, limits, vec![], 8)
            .unwrap()
            .with_base(BaseRows {
                rows: vec![rifle, start],
                left_out: vec![],
            }),
    );
    e
}

/// The map's own weapon and starting point are aimed at: the readout names
/// them, X opens their Properties, and delete removes the map's entry.
#[test]
fn an_own_thing_aimed_at_is_named_opened_and_removed() {
    let mut e = editor_with_own_things();
    calls::lock(&e.calls).arm();
    e.tool.monitor = true;
    e.tool.camera = Some(forge::camera::Camera {
        position: [-5., 0., 0.],
        yaw: 0.,
        pitch: 0.,
    });
    let update = |e: &mut Editor, action| {
        let input = forge::tool::Input {
            action,
            ..Default::default()
        };
        e.tool.update(&e.session, 1, input).unwrap()
    };
    update(&mut e, None);
    let rifle = Endpoint::base(BlockKind::Weapons, 0);
    assert_eq!(e.tool.aimed_thing.map(Endpoint::identity), Some(rifle));
    assert_eq!(e.hud().target, "battle rifle");
    assert_eq!(e.status()["aimed"], Value::Null, "not an added object");
    assert_eq!(e.status()["aimed_thing"]["base"]["block"], "weapons");
    let cmd = update(&mut e, Some(forge::tool::Action::Properties)).unwrap();
    e.command(2, cmd).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Properties));
    assert_eq!(
        screens::screen(Kind::Properties).sheet().title,
        "BATTLE RIFLE"
    );
    // A starting point is its kind by name.
    e.tool.camera = Some(forge::camera::Camera {
        position: [0., -5., 0.],
        yaw: std::f32::consts::FRAC_PI_2,
        pitch: 0.,
    });
    update(&mut e, None);
    assert_eq!(e.hud().target, "Starting point");
    // Delete on the rifle is its removal: one inert row at the next tick.
    e.tool.camera = Some(forge::camera::Camera {
        position: [-5., 0., 0.],
        yaw: 0.,
        pitch: 0.,
    });
    let cmd = update(&mut e, Some(forge::tool::Action::Delete)).unwrap();
    assert!(matches!(cmd, Command::Remove { id } if id.identity() == rifle));
    e.command(3, cmd).unwrap();
    let (ticket, call) = calls::lock(&e.calls).drain().pop().unwrap();
    assert!(
        matches!(&call, calls::Call::Forge { steps, .. } if matches!(
            steps.as_slice(),
            [calls::ForgeStep::Base(agent::forge::base::BaseStep::Inert {
                block: BlockKind::Weapons,
                index: 0,
            })]
        )),
        "{call:?}"
    );
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[]})));
    e.poll(4);
    assert!(e.session.thing(rifle).is_none(), "removed");
    assert_eq!(e.status()["variant"]["dirty"], true);
}

#[test]
fn shared_names_are_told_apart_by_their_folder_then_their_path() {
    let item = |block, path: &str| PaletteItem {
        block,
        tag: TagKey {
            group: "scen".into(),
            path: path.into(),
        },
    };
    let items = [
        item(BlockKind::Scenery, r"scenery\rocks\boulder\boulder"),
        item(BlockKind::Scenery, r"scenery\cave\boulder"),
        item(BlockKind::Scenery, r"scenery\light_post\post"),
        item(BlockKind::Crates, r"crates\boulder"),
        item(BlockKind::Scenery, r"a\x\twin"),
        item(BlockKind::Scenery, r"b\x\twin"),
    ];
    let names: Vec<_> = groups(&items).into_iter().collect();
    assert_eq!(
        names,
        [
            (Group::Scenery, "boulder (rocks)".into()),
            (Group::Scenery, "boulder (cave)".into()),
            (Group::Scenery, "post".into()),
            (Group::Crates, "boulder".into()),
            (Group::Scenery, r"a\x\twin".into()),
            (Group::Scenery, r"b\x\twin".into()),
        ]
    );
    assert_eq!(qualified("solo"), "solo");
    assert_eq!(
        qualified(r"objects\light_post\light_post"),
        "light post (objects)"
    );
}

fn property_session() -> Session {
    let float = |v: f32| forge_doc::F32::new(v).unwrap();
    let vec3 = |v: [f32; 3]| forge_doc::Value::Vec3(v.map(float));
    let row = |block: BlockKind, fields: Vec<forge_doc::Value>| forge::BaseRow {
        block,
        index: 0,
        raw: vec![1; map::scenario_layout::block(block).stride as usize],
        print: forge_doc::print(block, &fields).unwrap(),
        fields,
    };
    let start = row(
        BlockKind::StartingLocations,
        vec![
            vec3([0.0, 3.0, 0.0]),
            forge_doc::Value::Float(float(0.0)),
            forge_doc::Value::Int(0),
            forge_doc::Value::Int(-1),
        ],
    );
    let goal = row(
        BlockKind::NetgameFlags,
        vec![
            vec3([1.0, 5.0, 0.0]),
            forge_doc::Value::Float(float(0.0)),
            forge_doc::Value::Int(0),
            forge_doc::Value::Int(0),
            forge_doc::Value::Int(0),
        ],
    );
    let mut spawner_fields = vec![forge_doc::Value::Int(0)];
    spawner_fields.extend([12, 12, 12, 12, 0, 30, 0].map(forge_doc::Value::Int));
    spawner_fields.extend([
        forge_doc::Value::Int(0),
        vec3([0.0, 6.0, 0.0]),
        vec3([0.0; 3]),
        forge_doc::Value::Tag(Some(TagKey {
            group: "itmc".into(),
            path: r"test\rifles".into(),
        })),
    ]);
    let spawner = row(BlockKind::NetgameEquipment, spawner_fields);
    let limits = Limits {
        world: Some(WorldBounds {
            min: [-10.0; 3],
            max: [10.0; 3],
        }),
        base_counts: [
            (BlockKind::StartingLocations, 1),
            (BlockKind::NetgameFlags, 1),
            (BlockKind::NetgameEquipment, 1),
        ]
        .into_iter()
        .collect(),
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
    Session::new(base, limits, vec![], 8)
        .unwrap()
        .with_base(forge::BaseRows {
            rows: vec![start, goal, spawner],
            left_out: vec![],
        })
}

#[test]
fn coordinates_change_the_ghost_then_drop_once() {
    let mut e = editor();
    calls::lock(&e.calls).arm();
    e.tool.monitor = true;
    e.tool.select(0);
    e.tool.ghost = Some(Frame {
        position: [3.0, 0.0, 0.0],
        rotation: [0.0; 3],
    });
    e.command(1, Command::Properties).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Properties));
    e.movement_step = 2;
    e.coordinate_number(2, sheet::Coordinate::X, "2.5").unwrap();
    e.choose(3, Choice::Nudge(sheet::Coordinate::X, true))
        .unwrap();
    assert_eq!(e.tool.ghost.unwrap().position, [2.6, 0.0, 0.0]);
    e.choose(4, Choice::CoordinatesDone).unwrap();
    let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles": [51]})));
    e.poll(5);
    let placed = &e.session.objects()[0];
    assert_eq!(placed.handle, 51);
    assert_eq!(placed.frame.position, [2.6, 0.0, 0.0]);
    assert_eq!(e.session.log().len(), 1, "one placement, one undo step");

    // An existing thing moves live first, then the close writes its op.
    e.tool.held_thing = Some(Endpoint::Added(placed.id));
    e.command(6, Command::Properties).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Properties));
    e.coordinate_number(7, sheet::Coordinate::Y, "1.5").unwrap();
    let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles": []})));
    e.poll(8);
    assert_eq!(e.session.log().len(), 1, "holding writes no op");
    e.choose(9, Choice::CoordinatesDone).unwrap();
    // The hold already moved the world, so the drop writes its op and asks
    // the game for nothing.
    assert!(calls::lock(&e.calls).drain().is_empty());
    e.poll(10);
    assert_eq!(e.session.log().len(), 2, "the drop is the second op");
    assert_eq!(e.session.objects()[0].frame.position, [2.6, 1.5, 0.0]);
}

#[test]
fn spawn_goal_and_item_properties_write_their_fields() {
    let mut e = editor();
    e.session = Arc::new(property_session());
    calls::lock(&e.calls).arm();
    let finish = |e: &mut Editor, tick| {
        let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
        calls::lock(&e.calls).finish(ticket, Ok(json!({"handles": []})));
        e.poll(tick);
    };
    let start = Endpoint::base(BlockKind::StartingLocations, 0);
    e.tool.aimed_thing = Some(start);
    e.choose(1, Choice::Set(Prop::Team, 1)).unwrap();
    finish(&mut e, 2);
    let field = |thing: &forge::Thing, name: &str| {
        thing
            .fields
            .iter()
            .find(|(field, _)| *field == name)
            .map(|(_, value)| value.clone())
            .unwrap()
    };
    assert_eq!(
        field(&e.session.thing(start).unwrap(), "team"),
        forge_doc::Value::Int(1)
    );

    let goal = Endpoint::base(BlockKind::NetgameFlags, 0);
    e.tool.aimed_thing = Some(goal);
    e.choose(3, Choice::Set(Prop::Role, 4)).unwrap();
    finish(&mut e, 4);
    e.tool.aimed_thing = Some(goal);
    e.choose(5, Choice::Set(Prop::Order, 7)).unwrap();
    finish(&mut e, 6);
    let goal_thing = e.session.thing(goal).unwrap();
    assert_eq!(field(&goal_thing, "type"), forge_doc::Value::Int(4));
    assert_eq!(field(&goal_thing, "identifier"), forge_doc::Value::Int(7));

    let spawner = Endpoint::base(BlockKind::NetgameEquipment, 0);
    e.tool.aimed_thing = Some(spawner);
    e.choose(
        7,
        Choice::Number(sheet::NumberField::RespawnTime, "45".into()),
    )
    .unwrap();
    finish(&mut e, 8);
    assert_eq!(
        field(&e.session.thing(spawner).unwrap(), "respawn time"),
        forge_doc::Value::Int(45)
    );
}
