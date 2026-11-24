mod area;

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

/// The placed objects browser lists what is placed, and choosing a row
/// flies the monitor to that object and faces it.
#[test]
fn the_placed_objects_browser_flies_the_monitor_to_the_chosen_object() {
    let mut e = editor();
    calls::lock(&e.calls).arm();
    e.tool.monitor = true;
    assert!(e.placed().is_empty());
    let frame = Frame {
        position: [3., 0., 0.],
        rotation: [0.; 3],
    };
    e.command(1, Command::Place { item: 0, frame }).unwrap();
    let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[31]})));
    e.poll(2);
    let placed = e.placed();
    assert_eq!(placed.len(), 1);
    assert_eq!(placed[0].name, "shelf");
    assert_eq!(placed[0].id, e.session.objects()[0].id);
    e.tool.camera = Some(forge::camera::Camera {
        position: [13., 0., 0.],
        yaw: 0.,
        pitch: 0.,
    });
    e.choose(3, Choice::FlyTo(placed[0].id)).unwrap();
    let eye = e.tool.camera.unwrap();
    assert!((eye.position[0] - 8.).abs() < 1e-4, "{:?}", eye.position);
    let at = eye.point(FLY_DISTANCE);
    assert!((at[0] - 3.).abs() < 1e-3 && at[1].abs() < 1e-3 && at[2].abs() < 1e-3);
    // A gone object is refused by name, and the view stays.
    let gone = forge_doc::OpId(999);
    assert_eq!(
        e.choose(4, Choice::FlyTo(gone)),
        Err("That object is gone.".into())
    );
    assert_eq!(e.tool.camera, Some(eye));
    // With no monitor view there is nothing to fly.
    e.tool.camera = None;
    assert!(e.choose(5, Choice::FlyTo(placed[0].id)).is_err());
}

/// Two placed objects are selected from their Properties pages, grouped
/// by name, listed first by the browser, locked and ungrouped.
#[test]
fn selecting_two_objects_groups_them_and_the_browser_lists_the_group_first() {
    let mut e = editor();
    e.tool.monitor = true;
    for (n, x) in [3., 5.].into_iter().enumerate() {
        let frame = Frame {
            position: [x, 0., 0.],
            rotation: [0.; 3],
        };
        e.command(1 + n as u32, Command::Place { item: 0, frame })
            .unwrap();
        let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
        calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[31 + n]})));
        e.poll(10 + n as u32);
    }
    let [a, b] = [0, 1].map(|i| Endpoint::Added(e.session.objects()[i].id));
    let labels = || -> Vec<String> {
        screens::screen(Kind::Properties)
            .sheet()
            .rows
            .iter()
            .map(|r| r.label.clone())
            .collect()
    };
    // An ungrouped thing's page offers Select; Group selection comes with
    // two selected.
    e.tool.aimed_thing = Some(a);
    e.command(20, Command::Properties).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Properties));
    assert!(labels().contains(&"Select".to_string()), "{:?}", labels());
    assert!(!labels().contains(&"Group selection".to_string()));
    e.choose(21, Choice::ToggleSelected(a)).unwrap();
    assert!(
        e.command(
            22,
            Command::Group {
                name: "x".into(),
                members: e.tool.selection().to_vec()
            }
        )
        .is_err()
    );
    e.choose(23, Choice::ToggleSelected(b)).unwrap();
    e.tool.aimed_thing = Some(b);
    e.command(24, Command::Properties).unwrap();
    assert!(
        labels().contains(&"Group selection".to_string()),
        "{:?}",
        labels()
    );
    assert!(labels().contains(&"Deselect".to_string()));
    // Group selection: one op, the selection lets go.
    e.choose(25, Choice::GroupSelection).unwrap();
    e.poll(26);
    let groups = e.session.groups();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].name, "Group 1");
    assert!(e.tool.selection().is_empty());
    // A grouped thing's page shows the group.
    e.tool.aimed_thing = Some(a);
    e.command(27, Command::Properties).unwrap();
    let rows = labels();
    for want in ["Group", "Lock group", "Ungroup"] {
        assert!(rows.contains(&want.to_string()), "{rows:?}");
    }
    // The browser lists the group first, and choosing it flies to its centre.
    let folders = sheet::folder_rows(&e.session.groups());
    assert_eq!(folders[0].name, "Group 1");
    assert_eq!(folders[0].line, "Group. 2 objects.");
    let page = sheet::options_with_groups(&folders, &e.placed());
    let row = page
        .rows
        .iter()
        .find(|r| r.label == "Placed objects")
        .unwrap();
    let sheet::Act::Pick(pick) = &row.act else {
        panic!("a browser picks");
    };
    assert_eq!(pick.values[0], "Group 1");
    assert_eq!(pick.choices[0], Choice::FlyToGroup(groups[0].id));
    e.tool.camera = Some(forge::camera::Camera {
        position: [20., 0., 0.],
        yaw: 0.,
        pitch: 0.,
    });
    e.choose(28, Choice::FlyToGroup(groups[0].id)).unwrap();
    let at = e.tool.camera.unwrap().point(FLY_DISTANCE + 1.);
    assert!((at[0] - 4.).abs() < 1e-3 && at[1].abs() < 1e-3, "{at:?}");
    // Lock, then end it.
    e.choose(29, Choice::LockGroup(groups[0].id, true)).unwrap();
    e.poll(30);
    assert!(e.session.groups()[0].locked);
    e.choose(31, Choice::Ungroup(groups[0].id)).unwrap();
    e.poll(32);
    assert!(e.session.groups().is_empty());
    assert_eq!(
        e.choose(33, Choice::FlyToGroup(groups[0].id)),
        Err("That group is gone.".into())
    );
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
    assert_eq!(e.monitor_hud().view.target, "battle rifle");
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
    assert_eq!(e.monitor_hud().view.target, "Starting point");
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

// ---- AI ground: the build runs on a worker (docs/game/forge-map-variants.md, "AI ground") ----

mod ground {
    use super::*;
    use crate::forge_glue::ground::{self as installed, NO_BUILDER, TEST_GUARD};
    use forge::{GroundBuilder, GroundProblem, GroundRecipe, GroundReport, GroundState};

    /// A builder with an invented report, or a refusal.
    struct Kit {
        refuse: bool,
    }

    impl GroundBuilder for Kit {
        fn recipe(&self) -> GroundRecipe {
            GroundRecipe {
                kit_version: 3,
                home: 0,
                probe: false,
            }
        }

        fn build(&mut self, _: &ForgeDoc) -> Result<GroundReport, String> {
            if self.refuse {
                return Err("No room: the sectors take 9 KB too many.".into());
            }
            Ok(GroundReport {
                zones: 2,
                squads: 16,
                problems: vec![GroundProblem {
                    label: "ff_drop".into(),
                    position: [1., 1., 0.],
                    reason: "off the walkable area".into(),
                }],
                receipt: None,
            })
        }
    }

    /// An editor over a map with three netgame flags of its own, so a marker
    /// is a flag after them.
    fn ground_editor() -> Editor {
        let mut e = editor();
        let base = e.session.document().clone();
        let limits = Limits {
            world: Some(WorldBounds {
                min: [-10.; 3],
                max: [10.; 3],
            }),
            base_counts: [(BlockKind::Crates, 2), (BlockKind::NetgameFlags, 3)]
                .into_iter()
                .collect(),
            ..Limits::default()
        };
        e.session = Arc::new(Session::new(base, limits, vec![], 8).unwrap());
        e.tool.monitor = true;
        e
    }

    /// Poll the editor's file job until its worker is done.
    fn finish(e: &mut Editor, tick: u32) {
        for _ in 0..500 {
            e.poll_file(tick);
            if !e.busy() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the ground build never finished");
    }

    fn drop_at(x: f32) -> Command {
        Command::Marker {
            label: forge::GroundLabel::Drop,
            frame: Frame {
                position: [x, 1., 0.],
                rotation: [0.; 3],
            },
        }
    }

    #[test]
    fn build_ai_ground_runs_on_a_worker_and_the_editor_takes_the_copy_back() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        installed::loads_map(false);
        installed::install(Box::new(Kit { refuse: false }));
        let mut e = ground_editor();
        e.command(1, drop_at(1.)).unwrap();
        e.poll(2);
        assert_eq!(e.session.ground_state(), GroundState::NotBuilt);
        assert_eq!(e.status()["ground"]["state"], "not_built");
        assert_eq!(e.status()["ground"]["available"], true);
        assert_eq!(e.status()["ground"]["markers"], 1);

        e.command(3, Command::BuildGround).unwrap();
        assert!(e.busy() && e.ground_building);
        assert_eq!(e.status()["ground"]["building"], true);
        assert_eq!(e.notice.as_ref().unwrap().0, "Building AI ground...");
        // While it runs nothing else is asked of the session.
        let asked = e.command(4, Command::TestWaves).unwrap_err();
        assert_eq!(asked, "A Forge edit is waiting for the game.");

        finish(&mut e, 5);
        assert!(!e.ground_building);
        assert_eq!(e.session.ground_state(), GroundState::Current);
        assert_eq!(
            e.notice,
            Some((
                "AI ground built: 2 zones, 16 squad slots, 1 problem. The running map is not changed."
                    .into(),
                5
            ))
        );
        let status = e.status();
        assert_eq!(status["ground"]["state"], "current");
        assert_eq!(status["ground"]["zones"], 2);
        assert_eq!(status["ground"]["problems"], 1);
        assert_eq!(status["ground"]["loaded"], false);

        // Test waves with the ground current builds nothing.
        e.command(6, Command::TestWaves).unwrap();
        finish(&mut e, 7);
        assert_eq!(e.notice.as_ref().unwrap().0, "AI ground is current.");
        // Another marker makes it stale, and Test waves builds again.
        e.command(8, drop_at(2.)).unwrap();
        e.poll(9);
        assert_eq!(e.session.ground_state(), GroundState::Stale);
        e.command(10, Command::TestWaves).unwrap();
        finish(&mut e, 11);
        assert_eq!(e.session.ground_state(), GroundState::Current);
        installed::uninstall();
    }

    #[test]
    fn an_agent_asking_for_a_build_is_answered_when_it_ends() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        installed::install(Box::new(Kit { refuse: false }));
        let mut e = ground_editor();
        let answer = e.ground_build(true).unwrap();
        assert!(answer.try_recv().is_err(), "not before the worker is done");
        finish(&mut e, 3);
        let reply = answer.try_recv().unwrap().unwrap();
        assert_eq!(reply["built"], true);
        assert_eq!(reply["state"], "current");
        assert_eq!(reply["report"]["zones"], 2);
        assert_eq!(reply["problems"][0]["reason"], "off the walkable area");
        assert_eq!(reply["building"], false);

        // Already current: answered, and not built.
        let answer = e.ground_build(true).unwrap();
        finish(&mut e, 4);
        assert_eq!(answer.try_recv().unwrap().unwrap()["built"], false);
        installed::uninstall();
    }

    #[test]
    fn a_build_that_fails_says_why_changes_nothing_and_answers_the_agent() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        installed::install(Box::new(Kit { refuse: true }));
        let mut e = ground_editor();
        let answer = e.ground_build(false).unwrap();
        finish(&mut e, 3);
        let reason = "No room: the sectors take 9 KB too many.";
        assert_eq!(answer.try_recv().unwrap(), Err(reason.to_string()));
        assert_eq!(e.notice, Some((reason.into(), 3)));
        assert!(!e.ground_building);
        assert_eq!(e.session.ground_state(), GroundState::NotBuilt);
        installed::uninstall();
    }

    #[test]
    fn with_no_builder_the_rows_and_the_agent_refuse_and_change_nothing() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        installed::uninstall();
        let mut e = ground_editor();
        assert_eq!(e.command(1, Command::BuildGround), Err(NO_BUILDER.into()));
        assert_eq!(e.command(1, Command::TestWaves), Err(NO_BUILDER.into()));
        assert_eq!(e.ground_build(false).unwrap_err(), NO_BUILDER);
        assert!(!e.busy());
        assert_eq!(e.status()["ground"]["available"], false);
        assert_eq!(e.ground_json()["state"], "not_built");
    }

    #[test]
    fn the_markers_are_the_ground_labels_and_the_problems_the_last_build_made() {
        use crate::forge_glue::markers_world::Tone;
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        installed::install(Box::new(Kit { refuse: false }));
        let mut e = ground_editor();
        let drops = |e: &Editor| {
            e.marker_entries()
                .into_iter()
                .filter(|m| m.marker.kind.name() == "ff_drop")
                .collect::<Vec<_>>()
        };
        assert!(drops(&e).is_empty());
        e.command(1, drop_at(1.)).unwrap();
        e.poll(2);
        let placed = drops(&e);
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].place.at, [1., 1., 0.]);
        assert_eq!(placed[0].tone, Tone::Normal);

        e.command(3, Command::BuildGround).unwrap();
        finish(&mut e, 4);
        // The problem is at the label's place and draws in its stead.
        let built = drops(&e);
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].tone, Tone::Problem);
        // The monitor off draws none.
        e.tool.monitor = false;
        assert!(e.marker_entries().is_empty());
        installed::uninstall();
    }
}

/// The Overlays page opens, a choice on it turns an overlay off, and the
/// markers and the status follow; the overlay is tool state, never an op.
#[test]
fn the_overlays_page_turns_overlays_off() {
    use forge::tool::Overlay;
    let mut e = editor();
    e.tool.monitor = true;
    let head = e.session.head();
    e.command(1, Command::Overlays).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Settings));
    assert!(e.overlay(Overlay::Markers));
    assert_eq!(e.status()["overlays"]["markers"], true);
    e.choose(2, Choice::Set(Prop::Overlay(Overlay::Markers), 1))
        .unwrap();
    assert!(!e.overlay(Overlay::Markers));
    assert!(e.marker_entries().is_empty());
    assert_eq!(e.status()["overlays"]["markers"], false);
    assert_eq!(e.status()["overlays"]["gizmo"], true);
    assert!(
        e.choose(3, Choice::Set(Prop::Overlay(Overlay::Gizmo), 2))
            .is_err()
    );
    e.choose(4, Choice::Set(Prop::Overlay(Overlay::Markers), 0))
        .unwrap();
    assert!(e.overlay(Overlay::Markers));
    assert_eq!(e.session.head(), head, "no op was made");
}
