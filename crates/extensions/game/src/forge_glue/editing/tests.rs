mod area;
mod variants;
mod waves;

use super::*;
use agent::forge::local::OBJECT_GONE;
use forge::{BlockKind, Frame, TagKey};
use forge_doc::{BuilderVersions, ForgeDoc, Limits, MapRef, WorldBounds};

pub(super) fn editor() -> Editor {
    editor_in(Path::new("."))
}

fn editor_in(data: &Path) -> Editor {
    Editor::new(opened(), data, 1, calls::shared())
}

/// The Firefight runtime as Test waves sees it: what it was asked to play
/// and how often it was stopped, with the events the real one gives.
#[derive(Default)]
pub(super) struct FakeTests {
    pub asked: std::sync::Mutex<Vec<String>>,
    pub stops: std::sync::atomic::AtomicUsize,
    running: std::sync::atomic::AtomicBool,
    events: std::sync::Mutex<std::collections::VecDeque<TestEvent>>,
}

impl WaveTests for FakeTests {
    fn start(&self, def: ::firefight::Def) -> Result<(), String> {
        if self.phase() != TestPhase::Off {
            return Err(TEST_RUNNING.into());
        }
        self.asked.lock().unwrap().push(def.name);
        self.running.store(true, Ordering::SeqCst);
        self.events.lock().unwrap().push_back(TestEvent::Started);
        Ok(())
    }
    fn stop(&self) -> bool {
        self.stops.fetch_add(1, Ordering::SeqCst);
        let was = self.running.swap(false, Ordering::SeqCst);
        if was {
            self.events.lock().unwrap().push_back(TestEvent::Ended(WAVES_STOPPED));
        }
        was
    }
    fn phase(&self) -> TestPhase {
        if self.running.load(Ordering::SeqCst) {
            TestPhase::Running
        } else {
            TestPhase::Off
        }
    }
    fn take_event(&self) -> Option<TestEvent> {
        self.events.lock().unwrap().pop_front()
    }
}

impl FakeTests {
    pub fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
    pub fn stops(&self) -> usize {
        self.stops.load(Ordering::SeqCst)
    }
}

/// `e` with a fake runtime for its wave tests.
pub(super) fn with_tests(e: &mut Editor) -> Arc<FakeTests> {
    let tests = Arc::new(FakeTests::default());
    e.wave_tests = Some(tests.clone());
    tests
}

/// The test room as the host opens it: a shelf in the palette, two crates
/// of its own.
/// The Monitor's unit tag in the test room's kit.
const MONITOR_TAG: u32 = 0xe100_0007;

fn opened() -> Opened {
    opened_with(None)
}

fn opened_with(own: Option<forge::BaseRows>) -> Opened {
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
        base_counts: [(item.block, 2)]
            .into_iter()
            // Each block counts its rows; they come in index order.
            .chain(own.iter().flat_map(|b| b.rows.iter().map(|r| (r.block, r.index + 1))))
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
        waves: None,
    };
    let session = Session::new(base, limits, vec![item], 8).unwrap();
    Opened {
        session: match own {
            Some(rows) => session.with_base(rows),
            None => session,
        },
        tags: vec![42],
        extents: vec![Extent {
            radius: 1.,
            offset: [0.; 3],
        }],
        refused: vec![r"test\hum: sound scenery has no body to place or see".into()],
        walls: vec![],
        names: Default::default(),
        // The kit carries the Monitor alone.
        bodies: forge::body::Kit {
            carried: [(forge_doc::bodies::Body::Monitor, MONITOR_TAG)].into(),
            refused: [(forge_doc::bodies::Body::Sentinel, forge_doc::bodies::Refused::NoRoom)].into(),
        },
        // The rifles hold a weapon; the grenades do not.
        armed: [r"test\rifles".to_string()].into(),
    }
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
    assert_eq!(
        objects.rows.len(),
        5,
        "Crates, the block-out Structure and Natural groups, then the AI and Siege pages"
    );
    assert_eq!(objects.rows[0].label, "Crates");
    assert_eq!(objects.rows[3].label, "AI and objectives");
    assert_eq!(objects.rows[4].label, "Siege");
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
        matches!(&call, calls::Call::Forge { steps, .. } if *steps == [
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
        matches!(&call, calls::Call::Forge { steps, .. } if *steps == [
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
        matches!(&call, calls::Call::Forge { steps, .. } if *steps == [
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
    calls::lock(&e.calls).arm();
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
    assert_eq!(screens::take_request(), Some(Kind::ObjectActions));
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
    assert_eq!(screens::take_request(), Some(Kind::ObjectActions));
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
    for want in ["Group", "Lock group", "Pivot", "Ungroup"] {
        assert!(rows.contains(&want.to_string()), "{rows:?}");
    }
    e.choose(28, Choice::SetGroupPivot(groups[0].id, true)).unwrap();
    e.poll(28);
    let groups = e.session.groups();
    assert!(groups[0].pivot_first);
    // The browser lists the group first, and choosing it flies to its centre.
    let folders = sheet::folder_rows(&e.session.groups());
    assert_eq!(folders[0].name, "Group 1");
    assert_eq!(folders[0].line, "Group. 2 objects.");
    let row = sheet::placed_row_with(&folders, &e.placed());
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

/// A label is typed from the Properties page, set in one op and cleared;
/// a prefab of the placed objects is listed by the object menu when the map
/// holds its tags, and placing it is one edit.
#[test]
fn labels_are_typed_and_the_object_menu_places_a_saved_prefab() {
    let mut e = editor();
    calls::lock(&e.calls).arm();
    e.tool.monitor = true;
    let frame = Frame {
        position: [3., 0., 0.],
        rotation: [0.; 3],
    };
    e.command(1, Command::Place { item: 0, frame }).unwrap();
    let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[31]})));
    e.poll(2);
    let a = Endpoint::Added(e.session.objects()[0].id);
    let rows = |e: &Editor| -> Vec<String> {
        let _ = e;
        screens::screen(Kind::Properties)
            .sheet()
            .rows
            .iter()
            .map(|r| r.label.clone())
            .collect()
    };
    e.tool.aimed_thing = Some(a);
    e.command(3, Command::Properties).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::ObjectActions));
    let before = rows(&e);
    assert!(before.contains(&"Label".to_string()), "{before:?}");
    assert!(before.contains(&"Save as prefab".to_string()), "{before:?}");
    assert!(!before.contains(&"Clear label".to_string()));
    // Typed text is lowercased, a number is typed on its own, and the
    // page then offers to clear it.
    e.choose(
        4,
        Choice::Number(sheet::NumberField::Label, "Checkpoint".into()),
    )
    .unwrap();
    e.poll(5);
    assert_eq!(e.session.label_of(a).unwrap().text, "checkpoint");
    e.choose(
        6,
        Choice::Number(sheet::NumberField::LabelNumber, "3".into()),
    )
    .unwrap();
    e.poll(7);
    assert_eq!(e.session.label_of(a).unwrap().number, Some(3));
    assert!(
        e.choose(
            8,
            Choice::Number(sheet::NumberField::LabelNumber, "x".into())
        )
        .is_err()
    );
    assert!(
        e.choose(
            9,
            Choice::Number(sheet::NumberField::Label, "Has Space".into())
        )
        .is_err()
    );
    e.command(10, Command::Properties).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::ObjectActions));
    let after = rows(&e);
    for want in ["Label number", "Clear label"] {
        assert!(after.contains(&want.to_string()), "{after:?}");
    }
    let status = e.status();
    assert_eq!(status["labels"][0]["text"], "checkpoint");
    e.choose(11, Choice::ClearLabel(a)).unwrap();
    e.poll(12);
    assert_eq!(e.session.label_of(a), None);
    // A prefab of the object is on the object menu, and placing it adds one.
    let prefab = e.session.capture_prefab("post", &[a]).unwrap();
    e.prefabs.push(prefab);
    let menu = sheet::with_prefabs(None, &e.prefab_items()).unwrap();
    let row = menu.rows.iter().find(|r| r.label == "Prefabs").unwrap();
    let sheet::Act::Pick(pick) = &row.act else {
        panic!("the prefabs are picked");
    };
    assert_eq!(pick.values, ["post"]);
    assert_eq!(pick.details, ["Prefab. 1 object."]);
    assert_eq!(pick.choices, [Choice::PlacePrefab(0)]);
    assert!(pick.closes);
    assert_eq!(e.status()["prefabs"][0]["fits"], true);
    e.tool.camera = Some(forge::camera::Camera {
        position: [0., 0., 0.],
        yaw: 0.,
        pitch: 0.,
    });
    e.choose(13, Choice::PlacePrefab(0)).unwrap();
    let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[32]})));
    e.poll(14);
    assert_eq!(e.session.objects().len(), 2);
    // A prefab naming a tag the map lacks is not listed, is refused by
    // name when asked for, and says so in the status.
    let mut other = e.prefabs[0].clone();
    other.pieces[0].tag.path = r"elsewhere\crate".into();
    other.name = "far".into();
    e.prefabs.push(other);
    assert_eq!(e.prefab_items().len(), 1);
    let error = e.choose(15, Choice::PlacePrefab(1)).unwrap_err();
    assert!(error.contains(r"elsewhere\crate"), "{error}");
    assert_eq!(e.status()["prefabs"][1]["fits"], false);
    assert_eq!(e.session.objects().len(), 2);
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
}

/// Back's command opens the Forge menu while the monitor flies, on the
/// tab it was left on; in the player's body it is refused by name.
#[test]
fn the_menu_command_opens_the_forge_menu_in_the_monitor() {
    let mut e = editor();
    screens::publish(screens::Facts {
        active: true,
        monitor: true,
        ..screens::Facts::default()
    });
    e.command(1, Command::Menu).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Menu));
    assert_eq!(screens::screen(Kind::Menu).sheet().title, "FORGE");
    screens::publish(screens::Facts {
        active: true,
        monitor: false,
        ..screens::Facts::default()
    });
    assert!(e.command(2, Command::Menu).unwrap_err().contains("monitor"));
    assert_eq!(screens::take_request(), None);
    screens::publish(screens::Facts::default());
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
    assert_eq!(e.monitor_hud(0.0).view.target, "battle rifle");
    assert_eq!(e.status()["aimed"], Value::Null, "not an added object");
    assert_eq!(e.status()["aimed_thing"]["base"]["block"], "weapons");
    let cmd = update(&mut e, Some(forge::tool::Action::Properties)).unwrap();
    e.command(2, cmd).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::ObjectActions));
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
    assert_eq!(e.monitor_hud(0.0).view.target, "Starting point");
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
        waves: None,
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
    assert_eq!(screens::take_request(), Some(Kind::ObjectActions));
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
    e.tool.aimed_thing = Some(spawner);
    e.choose(
        9,
        Choice::Number(sheet::NumberField::ReturnTime, "90".into()),
    )
    .unwrap();
    finish(&mut e, 10);
    let thing = e.session.thing(spawner).unwrap();
    assert_eq!(field(&thing, "abandon limit"), forge_doc::Value::Int(90));
    assert_eq!(field(&thing, "respawn time"), forge_doc::Value::Int(45));
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
                classes: vec![],
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
                sectors: vec![vec![[0., 0., 0.], [2., 0., 0.], [0., 2., 0.]]],
                ..GroundReport::default()
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

    fn marker_at(label: forge::GroundLabel, x: f32) -> Command {
        Command::Marker {
            label,
            frame: Frame {
                position: [x, 1., 0.],
                rotation: [0.; 3],
            },
        }
    }

    /// The AI tool over two drops and a cover, one drop aimed at.
    fn ai_editor() -> (Editor, Endpoint) {
        let mut e = ground_editor();
        e.command(1, drop_at(1.)).unwrap();
        e.poll(1);
        e.command(2, drop_at(-1.)).unwrap();
        e.poll(2);
        e.command(3, marker_at(forge::GroundLabel::Cover, 4.)).unwrap();
        e.poll(3);
        e.tool.camera = Some(forge::camera::Camera {
            position: [0.; 3],
            yaw: 0.,
            pitch: 0.,
        });
        e.tool.set_ai(true);
        let second = Endpoint::Added(e.session.drops()[1]);
        e.tool.aimed_thing = Some(second);
        (e, second)
    }

    #[test]
    fn the_readout_names_the_zone_and_its_squads() {
        use hud_kit_core::views::forge_readout::Thing as Fact;
        let (mut e, second) = ai_editor();
        let target = |e: &Editor| e.readout_scene(None).target.expect("a drop is aimed at").target;
        let t = target(&e);
        assert_eq!(t.name, "Drop zone 1");
        assert_eq!(t.thing, Fact::Marker { note: "No squads land here".into() });
        assert_eq!(e.status()["aimed_drop"], json!({"zone": 1, "squads": 0}));
        assert_eq!(e.monitor_hud(0.).view.target, "Drop zone 1");
        assert_eq!(e.monitor_hud(0.).view.tool, hud_kit_core::views::forge::Tool::Ai);
        // A squad of the set that prefers the zone is counted.
        let at = crate::forge_screens::waves::At { round: 0, wave: 0, squad: 0 };
        let edit = crate::forge_screens::waves::Edit::Drop(at, ::firefight::Drop::PreferZone(1));
        e.choose(4, Choice::Waves(edit)).unwrap();
        assert_eq!(target(&e).thing, Fact::Marker { note: "1 squad lands here".into() });
        assert_eq!(e.status()["aimed_drop"], json!({"zone": 1, "squads": 1}));
        // A cover has no zone; the Objects tool names nothing of it.
        let cover = e
            .session
            .markers()
            .unwrap()
            .into_iter()
            .find(|m| m.label == forge::GroundLabel::Cover)
            .unwrap();
        e.tool.aimed_thing = Some(Endpoint::Added(cover.id));
        assert_eq!(target(&e).name, "Cover");
        assert_eq!(e.status()["aimed_drop"], Value::Null);
        e.tool.set_ai(false);
        e.tool.aimed_thing = Some(second);
        assert_ne!(target(&e).name, "Drop zone 1");
    }

    #[test]
    fn add_squad_here_presets_the_drop() {
        let (mut e, _) = ai_editor();
        let edit = crate::forge_screens::waves::Edit::AddSquadAt { round: 0, wave: 1, zone: 1 };
        e.choose(4, Choice::Waves(edit)).unwrap();
        let set = e.session.waves().expect("the session's own set");
        assert_eq!(
            set.rounds[0].waves[1].squads.last().unwrap().drop,
            ::firefight::Drop::PreferZone(1)
        );
        assert_eq!(e.session.zone_squads(1), 1);
        assert_eq!(e.status()["aimed_drop"], json!({"zone": 1, "squads": 1}));
    }

    #[test]
    fn the_overlay_draws_the_built_sectors_and_each_problem() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        crate::forge_glue::reload::set_reloads(false);
        installed::install(Box::new(Kit { refuse: false }));
        let (mut e, _) = ai_editor();
        e.tool.monitor = true;
        assert!(e.nav_lines().is_none(), "nothing is built yet");
        e.command(5, Command::BuildGround).unwrap();
        finish(&mut e, 6);
        // The sectors draw in the AI tool only, while the ground is current.
        assert_eq!(e.nav_lines().map(<[_]>::len), Some(3), "one triangle, a line a side");
        e.tool.set_ai(false);
        assert!(e.nav_lines().is_none());
        e.tool.set_ai(true);
        // The problem is a marker in the problem tone.
        let entries = e.marker_entries();
        let problems = entries
            .iter()
            .filter(|m| m.tone == crate::forge_glue::markers_world::Tone::Problem)
            .count();
        assert_eq!(problems, 1);
        // An edit the build covers makes them stale: no sectors.
        e.command(7, drop_at(3.)).unwrap();
        e.poll(8);
        e.refresh_sectors();
        assert!(e.nav_lines().is_none(), "a stale ground draws no sectors");
        assert_eq!(e.session.ground_state(), GroundState::Stale);
    }

    #[test]
    fn build_ai_ground_runs_on_a_worker_and_the_editor_takes_the_copy_back() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        crate::forge_glue::reload::set_reloads(false);
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

        // Test waves with the ground current builds nothing; this game
        // reloads no map, so the running map cannot hold it to test on.
        e.command(6, Command::TestWaves).unwrap();
        finish(&mut e, 7);
        assert_eq!(
            e.notice.as_ref().unwrap().0,
            format!("AI ground is current. {}", installed::NOT_LOADED)
        );
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

    /// A ground build stops a running test before it builds: the ground's
    /// map has none of the test's actors.
    #[test]
    fn a_ground_build_stops_the_test_first() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reload::set_reloads(false);
        installed::install(Box::new(Kit { refuse: false }));
        let mut e = ground_editor();
        let tests = with_tests(&mut e);
        tests.start(::firefight::builtin::definitions()[0].clone()).unwrap();
        e.poll_test(1);
        assert!(e.testing());
        e.command(2, Command::BuildGround).unwrap();
        assert_eq!(tests.stops(), 1, "stopped before the build");
        assert!(!e.testing());
        assert!(e.ground_building);
        finish(&mut e, 3);
        assert_eq!(e.session.ground_state(), GroundState::Current);
        installed::uninstall();
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

// ---- Generate nav: the generation runs on a worker (docs/game/forge-nav.md) ----

mod nav {
    use super::*;
    use crate::forge_glue::nav::{self as installed, NO_GENERATOR, TEST_GUARD};
    use forge::tool::Overlay;
    use forge::{NavBuilder, NavProblem, NavReport, NavState};

    /// A generator with an invented report, or a refusal.
    struct Gen {
        refuse: bool,
    }

    impl NavBuilder for Gen {
        fn generate(
            &mut self,
            _: &ForgeDoc,
            _: &std::collections::BTreeMap<BlockKind, u32>,
        ) -> Result<NavReport, String> {
            if self.refuse {
                return Err("The map does not build.".into());
            }
            Ok(NavReport {
                sectors: 40,
                out_of_map: 2,
                problems: vec![NavProblem {
                    kind: "start_off_mesh".into(),
                    what: "start 1".into(),
                    position: [1., 1., 3.],
                    reason: "no floor under it".into(),
                }],
                ..Default::default()
            })
        }
    }

    fn nav_editor() -> Editor {
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

    fn finish(e: &mut Editor, tick: u32) {
        for _ in 0..500 {
            e.poll_file(tick);
            if !e.busy() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the nav generation never finished");
    }

    #[test]
    fn generate_nav_runs_on_a_worker_and_its_problems_are_marked() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        installed::install(Box::new(Gen { refuse: false }));
        let mut e = nav_editor();
        assert_eq!(e.status()["nav"]["state"], "not_built");
        assert_eq!(e.status()["nav"]["available"], true);

        e.choose(1, Choice::GenerateNav).unwrap();
        assert!(e.busy() && e.nav_generating);
        assert_eq!(e.status()["nav"]["generating"], true);
        assert_eq!(e.notice.as_ref().unwrap().0, "Generating nav...");
        finish(&mut e, 2);
        assert!(!e.nav_generating);
        assert_eq!(e.session.nav_state(), NavState::Current);
        assert_eq!(
            e.notice,
            Some((
                "Nav generated: 40 sectors, 2 out of the map, 1 problem.".into(),
                2
            ))
        );
        assert_eq!(e.status()["nav"]["sectors"], 40);

        // The problem is marked while the Nav overlay is on, even with the
        // game type markers off.
        let marked = |e: &Editor| {
            e.marker_entries()
                .into_iter()
                .filter(|m| m.name == "nav:start 1")
                .count()
        };
        assert_eq!(marked(&e), 1);
        e.tool.overlays.set(Overlay::Markers, false);
        assert_eq!(marked(&e), 1);
        e.tool.overlays.set(Overlay::Nav, false);
        assert_eq!(marked(&e), 0);
        installed::uninstall();
    }

    #[test]
    fn the_nav_tool_draws_the_sectors_and_a_generates_the_nav() {
        use forge::NavLine;
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        struct Lines;
        impl NavBuilder for Lines {
            fn generate(
                &mut self,
                _: &ForgeDoc,
                _: &std::collections::BTreeMap<BlockKind, u32>,
            ) -> Result<NavReport, String> {
                Ok(NavReport {
                    sectors: 1,
                    lines: vec![NavLine {
                        kind: "sector".into(),
                        a: [0.; 3],
                        b: [1., 0., 0.],
                    }],
                    ..Default::default()
                })
            }
        }
        installed::install(Box::new(Lines));
        let mut e = nav_editor();
        e.choose(1, Choice::Tool(ToolKind::Nav)).unwrap();
        assert!(e.tool.nav && !e.tool.links);
        assert_eq!(e.status()["tool"], "nav");
        assert_eq!(e.monitor_hud(0.).view.tool, hud_kit_core::views::forge::Tool::Nav);
        // Nothing is drawn before a generation.
        assert!(e.nav_lines().is_none());

        e.command(2, Command::GenerateNav).unwrap();
        finish(&mut e, 3);
        assert_eq!(e.nav_lines().map(<[_]>::len), Some(1));
        // The overlay row turns it off; another tool draws no sectors.
        e.tool.overlays.set(Overlay::Nav, false);
        assert!(e.nav_lines().is_none());
        e.tool.overlays.set(Overlay::Nav, true);
        e.choose(4, Choice::Tool(ToolKind::Objects)).unwrap();
        assert!(!e.tool.nav && e.nav_lines().is_none());
        installed::uninstall();
    }

    #[test]
    fn an_agent_asking_for_nav_is_answered_and_a_failure_changes_nothing() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        installed::install(Box::new(Gen { refuse: false }));
        let mut e = nav_editor();
        let answer = e.nav_generate().unwrap();
        finish(&mut e, 3);
        let reply = answer.try_recv().unwrap().unwrap();
        assert_eq!(reply["state"], "current");
        assert_eq!(reply["counts"]["sectors"], 40);
        assert_eq!(reply["problems"][0]["what"], "start 1");

        installed::install(Box::new(Gen { refuse: true }));
        let mut e = nav_editor();
        let answer = e.nav_generate().unwrap();
        finish(&mut e, 4);
        assert_eq!(
            answer.try_recv().unwrap(),
            Err("The map does not build.".to_string())
        );
        assert!(!e.nav_generating);
        assert_eq!(e.session.nav_state(), NavState::NotBuilt);

        installed::uninstall();
        assert_eq!(e.choose(5, Choice::GenerateNav), Err(NO_GENERATOR.into()));
        assert!(!e.busy());
    }
}

/// **Reset orientation** and **Duplicate** on the Properties page of a
/// placed shelf: one uprighting move, then one add in front of the
/// monitor whose copy the monitor holds at once.
#[test]
fn reset_orientation_and_duplicate_on_the_properties_page() {
    let mut e = editor();
    calls::lock(&e.calls).arm();
    e.tool.monitor = true;
    e.choose(1, Choice::Select(0)).unwrap();
    let frame = Frame {
        position: [3., 0., 0.],
        rotation: [0.7, 0.3, 0.2],
    };
    e.tool.ghost = Some(frame);
    e.command(2, Command::Place { item: 0, frame }).unwrap();
    let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[31]})));
    e.poll(3);
    let id = e.session.objects()[0].id;
    let added = Endpoint::Added(id);
    e.tool.camera = Some(forge::camera::Camera {
        position: [0.; 3],
        yaw: 0.,
        pitch: 0.,
    });
    e.tool.aimed_thing = Some(added);
    e.tool.aimed = Some(id);
    e.command(4, Command::Properties).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::ObjectActions));
    // X on a thing of the map: its actions as a floating menu at it, with
    // its Properties one row of them and shown on their own screen.
    let menu = screens::screen(Kind::ObjectActions).sheet();
    let actions: Vec<&str> = menu.rows.iter().map(|r| r.label.as_str()).collect();
    assert_eq!(actions, ["Edit", "Duplicate", "Delete", "Properties"]);
    assert_eq!(menu.title, screens::screen(Kind::Properties).sheet().title);
    assert!(e.menu_at.is_some(), "the menu is anchored at the thing");
    let labels = || -> Vec<String> {
        screens::screen(Kind::Properties)
            .sheet()
            .rows
            .iter()
            .map(|r| r.label.clone())
            .collect()
    };
    assert!(
        labels().contains(&"Reset orientation".to_string()),
        "{:?}",
        labels()
    );
    assert!(
        labels().contains(&"Duplicate".to_string()),
        "{:?}",
        labels()
    );
    // Reset orientation: one move upright at the 45 degree snap, where it
    // stands.
    e.choose(5, Choice::ResetOrientation).unwrap();
    let (ticket, call) = calls::lock(&e.calls).drain().pop().unwrap();
    let upright = Frame {
        position: [3., 0., 0.],
        rotation: [45f32.to_radians(), 0., 0.],
    };
    assert!(
        matches!(&call, calls::Call::Forge { steps, .. } if *steps == [
            calls::ForgeStep::Move {
                target: calls::ForgeTarget::Live(agent_proto::ObjectId {
                    kind: agent_proto::Kind::Crate,
                    key: 31,
                }),
                frame: upright,
            },
        ]),
        "{call:?}"
    );
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[]})));
    e.poll(6);
    assert_eq!(e.session.log().len(), 2);
    // Duplicate: one add in front of the monitor, and the copy is held at
    // once.
    e.choose(7, Choice::Duplicate(added)).unwrap();
    let (ticket, call) = calls::lock(&e.calls).drain().pop().unwrap();
    let copy = Frame {
        position: [3., 0., 0.],
        rotation: [0.; 3],
    };
    assert!(
        matches!(&call, calls::Call::Forge { steps, .. } if *steps == [
            calls::ForgeStep::Make { tag: 42, frame: copy },
        ]),
        "{call:?}"
    );
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[32]})));
    e.poll(8);
    assert_eq!(e.session.log().len(), 3);
    assert_eq!(e.session.objects()[1].frame, copy);
    assert_eq!(
        e.tool.held_thing,
        Some(Endpoint::Added(e.session.objects()[1].id))
    );
    assert_eq!(e.tool.held, Some(e.session.objects()[1].id));
    assert_eq!(e.tool.ghost, Some(copy));
}

/// **Delete** and **Restore original** on the Properties page of a placed
/// shelf: Delete is one removal; Restore asks first, and No changes
/// nothing.
#[test]
fn delete_and_restore_original_on_the_properties_page() {
    let mut e = editor();
    calls::lock(&e.calls).arm();
    e.tool.monitor = true;
    e.choose(1, Choice::Select(0)).unwrap();
    let frame = Frame {
        position: [3., 0., 0.],
        rotation: [0.; 3],
    };
    e.tool.ghost = Some(frame);
    e.command(2, Command::Place { item: 0, frame }).unwrap();
    let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[31]})));
    e.poll(3);
    let id = e.session.objects()[0].id;
    let added = Endpoint::Added(id);
    e.tool.camera = Some(forge::camera::Camera {
        position: [0.; 3],
        yaw: 0.,
        pitch: 0.,
    });
    e.tool.aimed_thing = Some(added);
    e.tool.aimed = Some(id);
    e.command(4, Command::Properties).unwrap();
    assert_eq!(screens::take_request(), Some(Kind::ObjectActions));
    let labels: Vec<String> = screens::screen(Kind::Properties)
        .sheet()
        .rows
        .iter()
        .map(|r| r.label.clone())
        .collect();
    assert!(labels.contains(&"Delete".to_string()), "{labels:?}");
    // An added thing is not the map's own, so it has no original to
    // restore.
    assert!(!labels.contains(&"Restore original".to_string()), "{labels:?}");
    // Restore asks, and No leaves the log as it was.
    let log = e.session.log().len();
    e.choose(5, Choice::Restore(added)).unwrap();
    screens::take_request();
    assert_eq!(e.restore_asked, Some(added));
    e.choose(
        6,
        Choice::Answer {
            ask: Ask::Restore,
            yes: false,
        },
    )
    .unwrap();
    assert_eq!(e.restore_asked, None);
    assert_eq!(e.session.log().len(), log);
    // Delete removes the shelf in one step.
    e.choose(7, Choice::Delete(added)).unwrap();
    let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[]})));
    e.poll(8);
    assert_eq!(e.session.log().len(), log + 1);
    assert!(e.session.objects().is_empty());
}

/// The test room as `opened`, with one own starting point on team 0.
fn opened_with_own_spawn() -> Opened {
    use forge::{BaseRow, BaseRows};
    use forge_doc::{F32, Value};
    let fields = vec![
        Value::Vec3([0., -4., 0.].map(|c| F32::new(c).unwrap())),
        Value::Float(F32::new(1.5).unwrap()),
        Value::Int(0),
        Value::Int(-1),
    ];
    let stride = map::scenario_layout::block(BlockKind::StartingLocations).stride as usize;
    let row = BaseRow {
        block: BlockKind::StartingLocations,
        index: 0,
        raw: vec![1; stride],
        print: forge_doc::print(BlockKind::StartingLocations, &fields).unwrap(),
        fields,
    };
    opened_with(Some(BaseRows {
        rows: vec![row],
        left_out: vec![],
    }))
}

/// **Restore original**, end to end on an own starting point: it has no
/// row until a team edit changes it; the row asks; Yes is one reset that
/// the world is told, and the row is gone again.
#[test]
fn restore_original_puts_an_own_thing_back_through_its_question() {
    let mut e = Editor::new(opened_with_own_spawn(), Path::new("."), 1, calls::shared());
    calls::lock(&e.calls).arm();
    e.tool.monitor = true;
    let spawn = Endpoint::base(BlockKind::StartingLocations, 0);
    e.tool.camera = Some(forge::camera::Camera {
        position: [0.; 3],
        yaw: 0.,
        pitch: 0.,
    });
    e.tool.aimed_thing = Some(spawn);
    let labels = |e: &Editor| -> Vec<String> {
        let thing = e.session.thing(spawn).unwrap();
        crate::forge_screens::sheet::properties_sheet("Starting point", &thing, 0.)
            .unwrap()
            .rows
            .iter()
            .map(|r| r.label.clone())
            .collect()
    };
    assert!(!labels(&e).contains(&"Restore original".to_string()));
    // A team change makes it a changed own thing.
    e.command(
        2,
        Command::SetField {
            id: spawn,
            name: "team",
            value: forge_doc::Value::Int(2),
        },
    )
    .unwrap();
    let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[]})));
    e.poll(3);
    assert_eq!(e.session.log().len(), 1);
    assert!(e.session.thing(spawn).unwrap().changed);
    assert!(labels(&e).contains(&"Restore original".to_string()), "{:?}", labels(&e));
    // The row asks first; No writes nothing.
    e.choose(4, Choice::Restore(spawn)).unwrap();
    screens::take_request();
    e.choose(5, Choice::Answer { ask: Ask::Restore, yes: false }).unwrap();
    assert!(calls::lock(&e.calls).drain().is_empty());
    assert_eq!(e.session.log().len(), 1);
    // Yes is one reset, and the world is told the team is the map's again.
    e.choose(6, Choice::Restore(spawn)).unwrap();
    screens::take_request();
    e.choose(7, Choice::Answer { ask: Ask::Restore, yes: true }).unwrap();
    let (ticket, call) = calls::lock(&e.calls).drain().pop().unwrap();
    assert!(
        format!("{call:?}").contains("Int(0)") || format!("{call:?}").contains("team"),
        "{call:?}"
    );
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[]})));
    e.poll(8);
    assert_eq!(e.session.log().len(), 2);
    assert!(!e.session.thing(spawn).unwrap().changed);
    assert!(!labels(&e).contains(&"Restore original".to_string()));
}

/// A spawner's Team row writes the document's team property: one op, the
/// world told nothing, and the page shows it.
#[test]
fn a_spawners_team_row_sets_the_property_and_tells_the_world_nothing() {
    use forge::{BaseRow, BaseRows};
    use forge_doc::{F32, Value};
    let mut spawner = vec![Value::Int(0)];
    spawner.extend([12, 12, 12, 12, 0, 30, 0].map(Value::Int));
    spawner.extend([
        Value::Int(0),
        Value::Vec3([0., 6., 0.].map(|c| F32::new(c).unwrap())),
        Value::Vec3([0.; 3].map(|c| F32::new(c).unwrap())),
        Value::Tag(Some(TagKey {
            group: "itmc".into(),
            path: r"test\rifles".into(),
        })),
    ]);
    let block = BlockKind::NetgameEquipment;
    let stride = map::scenario_layout::block(block).stride as usize;
    let row = BaseRow {
        block,
        index: 0,
        raw: vec![1; stride],
        print: forge_doc::print(block, &spawner).unwrap(),
        fields: spawner,
    };
    let opened = opened_with(Some(BaseRows {
        rows: vec![row],
        left_out: vec![],
    }));
    let mut e = Editor::new(opened, Path::new("."), 1, calls::shared());
    calls::lock(&e.calls).arm();
    let id = Endpoint::base(block, 0);
    e.tool.aimed_thing = Some(id);
    e.choose(1, Choice::Set(Prop::SpawnerTeam, 3)).unwrap();
    assert!(calls::lock(&e.calls).drain().is_empty());
    e.poll(2);
    assert_eq!(e.session.team(id), Some(2));
    assert_eq!(e.session.log().len(), 1);
    e.choose(3, Choice::Set(Prop::SpawnerTeam, 0)).unwrap();
    e.poll(4);
    assert_eq!(e.session.team(id), None);
}

#[test]
fn live_forge_row_starts_in_the_monitor_with_the_hud() {
    use crate::{custom_games, lobby_start, lobby_tabs};
    let tab = custom_games::Menu::ALL.row_kind(custom_games::FORGE_ROW).mode().unwrap();
    let plan = lobby_tabs::switch::plan(lobby_tabs::Tab::CustomGames, tab);
    lobby_start::set_mode(plan.mode);
    // The session swaps into the Monitor once the player has a unit.
    let e = editor();
    assert!(e.enter_at_start);
    assert!(!e.monitor(), "in the biped until the swap returns");
    lobby_start::clear();
    assert!(!editor().enter_at_start);
}

// ---- Rebuild map and the reload after an AI ground build
// (docs/game/forge-design.md, "Rebuild map") ----

mod rebuild {
    use super::*;
    use crate::forge_glue::ground::TEST_GUARD;
    use crate::forge_glue::reload::{self, CopyBuilder};

    /// A build that gives the same bytes for any document.
    struct Bytes;

    impl CopyBuilder for Bytes {
        fn build(&mut self, _: &forge_doc::ForgeDoc) -> Result<Vec<u8>, String> {
            Ok(b"rebuilt map".to_vec())
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("forge-rebuild-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Wait for the editor's file job to end.
    fn settle(e: &mut Editor, tick: u32) {
        for _ in 0..500 {
            e.poll_file(tick);
            if e.file_job.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the file job never finished");
    }

    /// Place the palette's shelf at x = 3 and answer it with `handle`.
    fn place(e: &mut Editor, handle: u32) {
        calls::lock(&e.calls).arm();
        let frame = Frame {
            position: [3., 0., 0.],
            rotation: [0.; 3],
        };
        e.command(1, Command::Place { item: 0, frame }).unwrap();
        let (ticket, _) = calls::lock(&e.calls).drain().pop().unwrap();
        calls::lock(&e.calls).finish(ticket, Ok(json!({ "handles": [handle] })));
        e.poll(2);
        assert_eq!(e.session.objects()[0].handle, handle);
    }

    #[test]
    fn a_rebuild_with_no_edits_does_nothing() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reload::set_reloads(true);
        reload::install_builder(Box::new(Bytes));
        let mut e = editor();
        e.choose(1, Choice::Rebuild).unwrap();
        assert_eq!(e.notice, Some((reload::NOTHING.into(), 1)));
        assert!(!e.busy());
        assert!(e.take_copy().is_none());
        reload::uninstall_builder();
        reload::set_reloads(false);
    }

    #[test]
    fn a_rebuild_refuses_by_name_where_the_game_cannot_build_or_reload() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = editor();
        place(&mut e, 31);
        reload::set_reloads(false);
        reload::install_builder(Box::new(Bytes));
        assert_eq!(e.choose(3, Choice::Rebuild), Err(reload::NO_RELOAD.into()));
        reload::set_reloads(true);
        reload::uninstall_builder();
        assert_eq!(e.choose(4, Choice::Rebuild), Err(reload::NO_BUILDER.into()));
        assert!(!e.busy());
        reload::set_reloads(false);
    }

    #[test]
    fn a_rebuild_writes_the_copy_for_the_cache_and_hands_it_to_the_driver() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reload::set_reloads(true);
        reload::install_builder(Box::new(Bytes));
        let data = scratch("copy");
        let mut e = editor_in(&data);
        e.set_map_name("testroom".into());
        place(&mut e, 31);
        e.choose(3, Choice::Rebuild).unwrap();
        assert_eq!(e.notice, Some((reload::REBUILDING.into(), 3)));
        settle(&mut e, 4);
        // Until the driver takes the copy, edits wait.
        assert!(e.busy());
        let copy = e.take_copy().expect("the copy is handed over");
        assert_eq!(copy.kind, forge::CopyKind::Rebuild);
        assert_eq!(copy.file, data.join("cache/forge-play/testroom.map"));
        assert_eq!(std::fs::read(&copy.file).unwrap(), b"rebuilt map");
        assert_eq!(copy.built, e.session.rebuild_form().unwrap());
        assert!(!e.busy());
        reload::uninstall_builder();
        reload::set_reloads(false);
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn the_session_goes_on_over_the_reloaded_map_with_what_it_holds() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = editor();
        place(&mut e, 31);
        let head = e.session.head();
        let built = e.session.rebuild_form().unwrap();
        let copy = reload::PlayCopy {
            file: PathBuf::from("/tmp/testroom.map"),
            built,
            kind: forge::CopyKind::Rebuild,
        };
        e.tool.monitor = true;
        // The map the reload loaded: the same palette, another scenario.
        let opened = opened();
        let mut asked = vec![];
        let mut e = e
            .reopen(
                opened,
                9,
                &copy,
                |block, index| {
                    asked.push((block, index));
                    Some(55)
                },
                5,
            )
            .map_err(|(why, _)| why)
            .unwrap();
        // The shelf is the crate block's third row; its object is found,
        // not placed again.
        assert_eq!(asked, [(BlockKind::Crates, 2)]);
        e.poll(6);
        assert!(calls::lock(&e.calls).drain().is_empty(), "nothing is placed");
        assert_eq!(e.session.objects()[0].handle, 55);
        assert_eq!(e.session.head(), head);
        assert_eq!(e.guard.scenario, 9);
        // The reloaded map holds no body: the player is in their biped
        // and is swapped into the body again once they have a unit.
        assert!(!e.tool.monitor, "the body went with the old map");
        assert!(e.enter_at_start, "the session goes back into the body");
        assert_eq!(e.notice, Some((reload::REBUILT.into(), 5)));
        assert!(!e.session.rebuild_needed());
        assert!(!e.busy());
        // Undo goes back over the rebuild: the found object is deleted.
        e.command(7, Command::Undo).unwrap();
        let (_, call) = calls::lock(&e.calls).drain().pop().unwrap();
        assert!(
            matches!(&call, calls::Call::Forge { steps, .. } if *steps == [
                calls::ForgeStep::Delete { target: calls::ForgeTarget::Live(agent_proto::ObjectId {
                    kind: agent_proto::Kind::Crate,
                    key: 55,
                }) },
            ]),
            "{call:?}"
        );
    }

    #[test]
    fn a_map_with_another_palette_is_not_gone_on_over() {
        let e = editor();
        let copy = reload::PlayCopy {
            file: PathBuf::from("/tmp/testroom.map"),
            built: e.session.rebuild_form().unwrap(),
            kind: forge::CopyKind::Rebuild,
        };
        let mut opened = opened();
        opened.session = Session::new(
            e.session.document().clone(),
            forge_doc::Limits {
                world: Some(WorldBounds {
                    min: [-10.; 3],
                    max: [10.; 3],
                }),
                ..Limits::default()
            },
            vec![],
            8,
        )
        .unwrap();
        let (why, back) = match e.reopen(opened, 9, &copy, |_, _| None, 5) {
            Err(refused) => refused,
            Ok(_) => panic!("another palette is refused"),
        };
        assert!(why.contains("palette"), "{why}");
        assert!(back.session.palette().is_empty(), "the opened map is given back");
    }

    #[test]
    fn a_ground_build_hands_its_map_to_the_driver_when_the_game_reloads() {
        use crate::forge_glue::ground as installed;
        use forge::{GroundBuilder, GroundRecipe, GroundReport};

        /// A builder that leaves its map as the app's does.
        struct Leaves;

        impl GroundBuilder for Leaves {
            fn recipe(&self) -> GroundRecipe {
                GroundRecipe {
                    kit_version: 3,
                    home: 0,
                    probe: false,
                    classes: vec![],
                }
            }

            fn build(&mut self, _: &forge_doc::ForgeDoc) -> Result<GroundReport, String> {
                reload::keep_ground_copy(b"ground map".to_vec());
                Ok(GroundReport {
                    zones: 1,
                    squads: 4,
                    ..GroundReport::default()
                })
            }
        }

        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reload::set_reloads(true);
        installed::install(Box::new(Leaves));
        let data = scratch("ground");
        let mut e = editor_in(&data);
        e.set_map_name("testroom".into());
        e.command(1, Command::BuildGround).unwrap();
        settle(&mut e, 2);
        assert_eq!(
            e.notice.as_ref().unwrap().0,
            "AI ground built: 1 zones, 4 squad slots. Loading the map with it..."
        );
        let copy = e.take_copy().expect("the ground's map is handed over");
        assert_eq!(copy.kind, forge::CopyKind::Ground);
        assert_eq!(std::fs::read(&copy.file).unwrap(), b"ground map");
        assert_eq!(copy.built, e.session.ground_copy_form().unwrap());
        // Built, but not loaded until the session goes on over it: Test
        // waves builds again rather than run waves without it.
        assert!(!e.session.ground_loaded());
        e.command(3, Command::TestWaves).unwrap();
        settle(&mut e, 4);
        assert!(e.take_copy().is_some());
        installed::uninstall();
        reload::set_reloads(false);
        std::fs::remove_dir_all(data).unwrap();
    }

    /// A ground builder that leaves its map for the reload, as the app's.
    struct LeavesGround;

    impl forge::GroundBuilder for LeavesGround {
        fn recipe(&self) -> forge::GroundRecipe {
            forge::GroundRecipe {
                kit_version: 3,
                home: 0,
                probe: false,
                classes: vec![],
            }
        }

        fn build(&mut self, _: &forge_doc::ForgeDoc) -> Result<forge::GroundReport, String> {
            reload::keep_ground_copy(b"ground map".to_vec());
            Ok(forge::GroundReport {
                zones: 1,
                squads: 4,
                ..forge::GroundReport::default()
            })
        }
    }

    /// Test waves runs the director only on a map that holds the current
    /// ground: without it the row builds the ground and asks the runtime
    /// nothing until the session goes on over the map reloaded onto it;
    /// with it the runtime is asked at once. Stop waves ends the test.
    #[test]
    fn a_test_starts_only_on_loaded_ground() {
        use crate::forge_glue::ground as installed;
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reload::set_reloads(true);
        installed::install(Box::new(LeavesGround));
        let data = scratch("test-waves");
        let mut e = editor_in(&data);
        e.set_map_name("testroom".into());
        let tests = with_tests(&mut e);
        assert!(!e.session.ground_loaded());
        let answer = e.waves_test(false).unwrap();
        assert_eq!(answer.try_recv().unwrap().unwrap()["building"], true);
        assert!(e.ground_building, "the ground builds first");
        settle(&mut e, 2);
        let copy = e.take_copy().expect("the ground's map is handed over");
        assert!(tests.asked().is_empty(), "not before the map holds the ground");

        // The session goes on over the map reloaded onto it: the test
        // starts there, with the session's set.
        let mut e = e
            .reopen(opened(), 9, &copy, |_, _| None, 5)
            .map_err(|(why, _)| why)
            .unwrap();
        assert!(e.session.ground_loaded());
        assert_eq!(tests.asked(), ["Holdout"]);
        e.poll(6);
        e.poll_test(6);
        assert!(e.testing());
        // One at a time.
        assert_eq!(e.test_waves(7, None), Err(TEST_RUNNING.into()));
        assert_eq!(tests.asked().len(), 1);

        // Stop waves ends it, and the session says so.
        e.choose(8, Choice::StopWaves).unwrap();
        assert_eq!(tests.stops(), 1);
        assert!(!e.testing());
        assert_eq!(e.notice, Some((WAVES_STOPPED.into(), 8)));
        assert_eq!(
            e.choose(9, Choice::StopWaves),
            Err("No wave test is running.".into())
        );

        // On loaded ground Test waves asks at once and builds nothing; the
        // agent hears once it started, with the set it plays.
        let answer = e.waves_test(false).unwrap();
        assert!(e.file_job.is_none(), "no build");
        assert_eq!(tests.asked(), ["Holdout", "Holdout"]);
        assert!(answer.try_recv().is_err(), "not before the runtime takes it");
        e.poll_test(10);
        let reply = answer.try_recv().unwrap().unwrap();
        assert_eq!(reply["testing"], true);
        assert_eq!(reply["set"], "Holdout");
        // An agent's stop.
        let stopped = e.waves_test(true).unwrap();
        assert_eq!(stopped.try_recv().unwrap().unwrap()["stopped"], true);
        assert!(!e.testing());

        installed::uninstall();
        reload::set_reloads(false);
        std::fs::remove_dir_all(data).unwrap();
    }

    /// Rebuild map stops a running test before it builds: the rebuilt map
    /// has none of the test's actors.
    #[test]
    fn a_rebuild_stops_the_test_first() {
        let _one_at_a_time = TEST_GUARD.lock().unwrap_or_else(|e| e.into_inner());
        reload::set_reloads(true);
        reload::install_builder(Box::new(Bytes));
        let data = scratch("rebuild-test");
        let mut e = editor_in(&data);
        e.set_map_name("testroom".into());
        place(&mut e, 31);
        let tests = with_tests(&mut e);
        tests.start(::firefight::builtin::definitions()[0].clone()).unwrap();
        e.poll_test(2);
        e.choose(3, Choice::Rebuild).unwrap();
        assert_eq!(tests.stops(), 1);
        assert!(!e.testing());
        settle(&mut e, 4);
        reload::uninstall_builder();
        reload::set_reloads(false);
        std::fs::remove_dir_all(data).unwrap();
    }
}

/// A collection spawner's Spare clips row writes the document's property
/// and stages the game's `SpareClips` step for the entry's row; Weapon's
/// own stages the clear.
#[test]
fn a_spawners_spare_clips_row_sets_the_property_and_stages_the_games_step() {
    use forge::{BaseRow, BaseRows};
    use forge_doc::{F32, Value};
    let mut spawner = vec![Value::Int(0)];
    spawner.extend([12, 12, 12, 12, 0, 30, 0].map(Value::Int));
    spawner.extend([
        Value::Int(0),
        Value::Vec3([0., 6., 0.].map(|c| F32::new(c).unwrap())),
        Value::Vec3([0.; 3].map(|c| F32::new(c).unwrap())),
        Value::Tag(Some(TagKey {
            group: "itmc".into(),
            path: r"test\rifles".into(),
        })),
    ]);
    let block = BlockKind::NetgameEquipment;
    let stride = map::scenario_layout::block(block).stride as usize;
    let row = BaseRow {
        block,
        index: 0,
        raw: vec![1; stride],
        print: forge_doc::print(block, &spawner).unwrap(),
        fields: spawner,
    };
    let opened = opened_with(Some(BaseRows {
        rows: vec![row],
        left_out: vec![],
    }));
    let mut e = Editor::new(opened, Path::new("."), 1, calls::shared());
    calls::lock(&e.calls).arm();
    let id = Endpoint::base(block, 0);
    assert_eq!(e.spare_clips_row(id), Some(None), "an itmc spawner has the row");
    e.tool.aimed_thing = Some(id);
    e.choose(1, Choice::Set(Prop::SpareClips, 3)).unwrap();
    let (ticket, call) = calls::lock(&e.calls).drain().pop().unwrap();
    assert!(
        matches!(&call, calls::Call::Forge { steps, .. } if *steps == [
            calls::ForgeStep::SpareClips { index: 0, clips: Some(2) },
        ]),
        "{call:?}"
    );
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[]})));
    e.poll(2);
    assert_eq!(e.session.spare_clips(id), Some(2));
    assert_eq!(e.spare_clips_row(id), Some(Some(2)));
    e.choose(3, Choice::Set(Prop::SpareClips, 0)).unwrap();
    let (ticket, call) = calls::lock(&e.calls).drain().pop().unwrap();
    assert!(
        matches!(&call, calls::Call::Forge { steps, .. } if *steps == [
            calls::ForgeStep::SpareClips { index: 0, clips: None },
        ]),
        "{call:?}"
    );
    calls::lock(&e.calls).finish(ticket, Ok(json!({"handles":[]})));
    e.poll(4);
    assert_eq!(e.session.spare_clips(id), None);
}

/// A guest with one local player (local user 0) standing at `at` facing
/// +y in biped `biped`; the player's handle and the biped's.
fn local_player_guest(at: [f32; 3]) -> (guest_layout::testing::FakeGuest, u32, u32) {
    use guest_layout::view::{fake, objects, players};
    const PLAYERS: u32 = 0x8000_0000;
    const OBJECTS: u32 = 0x8010_0000;
    const BODY: u32 = 0x8020_0000;
    let mut g = guest_layout::testing::FakeGuest::new();
    g.put_u32(guest_layout::points::PLAYERS.base, PLAYERS)
        .put_u32(guest_layout::points::OBJECTS.base, OBJECTS);
    let ps = fake::datums::<players::Player>(&mut g, PLAYERS, 16);
    let os = fake::datums::<objects::Header>(&mut g, OBJECTS, 16);
    let (unit, header) = fake::push(&mut g, &os);
    g.map(BODY, 0x400);
    header.set_body(&mut g, BODY).unwrap();
    let body = objects::Body::at(BODY);
    body.set_position(&mut g, at).unwrap();
    body.set_forward(&mut g, [0., 1., 0.]).unwrap();
    let (player, p) = fake::push(&mut g, &ps);
    p.set_unit(&mut g, Some(unit)).unwrap();
    p.set_local_user(&mut g, 0).unwrap();
    (g, player.raw(), unit.raw())
}

/// D-pad Up queues Enter with the kit's Monitor where the player stands;
/// its answer puts the tool in the monitor with the biped parked, and a
/// second press queues Leave with that biped. Fly follows a moved camera.
#[test]
fn the_body_swap_enters_the_monitor_and_comes_back() {
    use agent::sim::calls::{BodyStep, Call};
    let (m, player, biped) = local_player_guest([1., 2., 3.]);
    let mut e = editor();
    calls::lock(&e.calls).arm();
    e.toggle_body(&m).unwrap();
    let (ticket, call) = calls::lock(&e.calls).drain().pop().unwrap();
    let Call::ForgeBody { player: p, step: BodyStep::Enter { tag, pos, .. }, .. } = call else {
        panic!("an Enter: {call:?}");
    };
    assert_eq!((p, tag), (player, MONITOR_TAG));
    assert_eq!(pos, [1., 2., 3. + 0.64], "the standing eye with no drawn view");
    assert!(!e.monitor(), "nothing changes before the answer");
    calls::lock(&e.calls).finish(ticket, Ok(json!({"body":"e2000009","biped":format!("{biped:08x}")})));
    e.poll_body(5);
    assert!(e.monitor());
    assert_eq!(e.flying(), Some((forge_doc::bodies::Body::Monitor, 0xe200_0009)));
    assert_eq!(e.swap.parked(), Some(biped));

    // The camera moves: one Fly to it.
    e.player = Some(player);
    let mut camera = e.tool.camera.unwrap();
    camera.position[0] += 2.;
    e.tool.camera = Some(camera);
    e.fly_body();
    e.fly_body();
    let flies = calls::lock(&e.calls).drain();
    assert_eq!(flies.len(), 1, "one at a time");
    assert!(matches!(flies[0].1, Call::ForgeBody { step: BodyStep::Fly { body: 0xe200_0009, pos, .. }, .. } if pos == camera.position));
    calls::lock(&e.calls).finish(flies[0].0, Ok(json!({})));
    e.poll_body(6);

    e.toggle_body(&m).unwrap();
    let (ticket, call) = calls::lock(&e.calls).drain().pop().unwrap();
    assert!(matches!(call, Call::ForgeBody { step: BodyStep::Leave { body: 0xe200_0009, biped: b }, .. } if b == biped));
    calls::lock(&e.calls).finish(ticket, Ok(json!({"biped":format!("{biped:08x}"),"placed":true})));
    e.poll_body(7);
    assert!(!e.monitor());
    assert_eq!(e.swap.parked(), None);
}

/// A body the kit lacks is refused by name, and nothing is queued; with
/// no Monitor, Forge itself is.
#[test]
fn a_body_the_kit_lacks_is_refused_by_name() {
    let (m, ..) = local_player_guest([0.; 3]);
    let mut e = editor();
    calls::lock(&e.calls).arm();
    e.set_body(forge_doc::bodies::Body::Sentinel);
    assert_eq!(
        e.toggle_body(&m),
        Err("Sentinel is not in this map's kit: the map has no room for it.".into())
    );
    e.set_body(forge_doc::bodies::Body::Monitor);
    e.bodies = forge::body::Kit::default();
    assert!(e.toggle_body(&m).unwrap_err().starts_with("Forge needs the Monitor"));
    assert!(calls::lock(&e.calls).drain().is_empty());
    assert!(!e.monitor());
}

/// The Spare clips row is a spawner's of a collection that holds a weapon:
/// an equipment collection's, a vehicle's and an object's have none.
#[test]
fn spare_clips_rows_by_kind_and_collection() {
    use forge::{BaseRow, BaseRows};
    use forge_doc::{F32, Value};
    let spawner = |group: &str, path: &str| {
        let mut fields = vec![Value::Int(0)];
        fields.extend([12, 12, 12, 12, 0, 30, 0].map(Value::Int));
        fields.extend([
            Value::Int(0),
            Value::Vec3([0., 6., 0.].map(|c| F32::new(c).unwrap())),
            Value::Vec3([0.; 3].map(|c| F32::new(c).unwrap())),
            Value::Tag(Some(TagKey {
                group: group.into(),
                path: path.into(),
            })),
        ]);
        fields
    };
    let block = BlockKind::NetgameEquipment;
    let stride = map::scenario_layout::block(block).stride as usize;
    let rows = [
        spawner("itmc", r"test\rifles"),
        spawner("itmc", r"test\grenades"),
        spawner("vehc", r"test\hogs"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, fields)| BaseRow {
        block,
        index: i as u32,
        raw: vec![i as u8 + 1; stride],
        print: forge_doc::print(block, &fields).unwrap(),
        fields,
    })
    .collect();
    let e = Editor::new(
        opened_with(Some(BaseRows { rows, left_out: vec![] })),
        Path::new("."),
        1,
        calls::shared(),
    );
    let row = |i| e.spare_clips_row(Endpoint::base(block, i));
    assert_eq!(
        [row(0), row(1), row(2)],
        [Some(None), None, None],
        "weapons, equipment, a vehicle"
    );
}
