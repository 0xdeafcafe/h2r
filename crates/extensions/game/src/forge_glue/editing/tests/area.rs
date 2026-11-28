use std::sync::mpsc::{Receiver, Sender, channel, sync_channel};
use std::time::Duration;

use agent::{Agent, ConnId, GameStatus, Reader};
use guest_layout::{GuestMemory, testing::FakeGuest};
use forge::look::{Refusal, monitors::Who};
use forge_doc::{Edit, EntryOp};
use game_markers::{Area, AreaError, Shape};

use super::*;

struct InputHost(Sender<Input>);

impl local_api::LocalApi for InputHost {
    fn who(&self, _: Option<&dyn GuestMemory>) -> Who {
        unreachable!()
    }
    fn look(&self, _: &Who, _: Option<u64>) -> Result<Value, Refusal> {
        unreachable!()
    }
    fn look_set(&self, _: &Who, _: Value) -> Result<Value, Refusal> {
        unreachable!()
    }
    fn monitors(&self, _: &Who) -> Value {
        unreachable!()
    }
    fn status(&self) -> Result<Value, String> {
        unreachable!()
    }
    fn session(&self) -> Result<Arc<Session>, String> {
        unreachable!()
    }
    fn document(&self) -> Result<ForgeDoc, String> {
        unreachable!()
    }
    fn log(&self, _: u32) -> Result<Vec<Entry>, String> {
        unreachable!()
    }
    fn propose(&self, _: u32, _: Op, _: Option<Sha256>) -> Result<ProposalReply, String> {
        unreachable!()
    }
    fn input(&self, _: &dyn GuestMemory, input: Input) -> Result<(), String> {
        self.0.send(input).map_err(|error| error.to_string())
    }
}

struct Game {
    memory: FakeGuest,
    tick: u32,
}

impl Reader for Game {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(u64::from(self.tick)),
            map: Some("testroom".into()),
            bsp: Some(0),
            mode: Some("multiplayer".into()),
        }
    }

    fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
        Ok(Vec::new())
    }

    fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn GuestMemory)) -> bool {
        f(&mut self.memory);
        true
    }
}

struct Script {
    agent: Agent,
    conn: ConnId,
    replies: Receiver<String>,
    inputs: Receiver<Input>,
    game: Game,
}

impl Script {
    fn new() -> Self {
        let agent = Agent::new();
        let (send, inputs) = channel();
        local_api::install(&agent, InputHost(send));
        let (send, replies) = sync_channel(1);
        let conn = agent.connect(send);
        Self {
            agent,
            conn,
            replies,
            inputs,
            game: Game {
                memory: FakeGuest::new(),
                tick: 2,
            },
        }
    }

    fn input(&mut self, editor: &mut Editor, params: Value) -> Result<(), String> {
        self.game.tick += 1;
        let request = json!({"jsonrpc":"2.0", "id":1, "method":"forge.input", "params":params});
        self.agent.submit(self.conn, &request.to_string());
        self.agent.on_tick(&mut self.game);
        let reply: Value =
            serde_json::from_str(&self.replies.recv_timeout(Duration::from_secs(1)).unwrap())
                .unwrap();
        assert_eq!(reply["result"]["queued"], true, "{reply}");
        editor.take_input(self.game.tick, self.inputs.try_recv().unwrap(), Some(0));
        editor.poll(self.game.tick);
        match &editor.notice {
            Some((text, at)) if *at == self.game.tick => Err(text.clone()),
            _ => Ok(()),
        }
    }

    fn press(&mut self, editor: &mut Editor, action: &str) -> Result<(), String> {
        self.input(editor, json!({"action":action}))
    }
}

fn hill() -> (Editor, Endpoint) {
    let mut e = editor();
    let limits = Limits {
        world: e.session.limits().world,
        base_counts: [(BlockKind::NetgameFlags, 0)].into(),
        ..Limits::default()
    };
    e.session = Arc::new(Session::new(e.session.document().clone(), limits, vec![], 0).unwrap());
    e.tool.monitor = true;
    e.tool.camera = Some(Camera {
        position: [-3., 0., 0.],
        yaw: 0.,
        pitch: 0.,
    });
    let menu = sheet::with_goals(None);
    let sheet::Act::Pick(picker) = &menu.rows[0].act else {
        panic!("Goals opens its picker")
    };
    assert_eq!(menu.rows[0].label, "Goals");
    assert_eq!(picker.values, ["Hill", "Territory", "Siege site"]);
    e.choose(1, picker.choices[0].clone()).unwrap();
    e.poll(1);
    let flags = e.session.things();
    assert_eq!(flags.len(), 1, "one centre flag");
    assert_eq!(e.session.document().ops.len(), 2, "one add and one area op");
    let id = flags[0].id;
    assert_eq!(
        e.session.areas().unwrap()[&id].marker_area(),
        Area::default()
    );
    e.walls = vec![bots::los::Walls::new(vec![
        [[-10., -10., 0.], [10., -10., 0.], [10., 10., 0.]],
        [[-10., -10., 0.], [10., 10., 0.], [-10., 10., 0.]],
    ])];
    (e, id)
}

fn aim_at_floor(e: &mut Editor, [x, y]: [f32; 2]) {
    e.tool.camera = Some(Camera {
        position: [x - 4., y, 4.],
        yaw: 0.,
        pitch: -std::f32::consts::FRAC_PI_4,
    });
}

#[test]
fn forge_input_closes_four_points_in_one_op_and_cancel_keeps_the_last_area() {
    let (mut e, id) = hill();
    let mut script = Script::new();
    // The first X aims at the flag and opens its Properties. Select the
    // screen's actual Edit area row, then send the world input over RPC.
    script.press(&mut e, "properties").unwrap();
    assert_eq!(screens::take_request(), Some(Kind::ObjectActions));
    let sheet = screens::screen(Kind::Properties).sheet();
    let row = sheet
        .rows
        .iter()
        .find(|row| row.label == "Edit area")
        .unwrap();
    let sheet::Act::Choose(choice) = &row.act else {
        panic!("Edit area starts a draft")
    };
    e.choose(2, choice.clone()).unwrap();
    // Y cycles the four shapes: a box, a sphere, then an empty path.
    for shape in ["box", "sphere", "path"] {
        script.press(&mut e, "area_shape").unwrap();
        assert_eq!(e.status()["area_edit"]["shape"], shape);
    }
    assert_eq!(e.status()["area_edit"]["below"], json!(0.5), "the path takes the heights back");

    let expected = [[0., 0.], [4., 0.], [4., 4.], [0., 4.]];
    for p in expected {
        aim_at_floor(&mut e, p);
        script.press(&mut e, "accept").unwrap();
    }
    let draft = e.status()["area_edit"].clone();
    let preview = e.marker_entries();
    assert_eq!(preview.len(), 1);
    assert!(preview[0].draft_path);
    aim_at_floor(&mut e, [2., -1.]);
    let refused = script.press(&mut e, "accept").unwrap_err();
    assert_eq!(refused, AreaError::Crosses(0, 3).to_string());
    assert_eq!(e.status()["notice"], refused);
    assert_eq!(e.status()["area_edit"], draft);
    assert_eq!(e.session.document().ops.len(), 2);

    script.press(&mut e, "area_close").unwrap();
    let saved = e.session.document().clone();
    let [_, _, last] = saved.ops.as_slice() else {
        panic!("close writes one op")
    };
    let Edit::Entry(EntryOp::Area {
        target,
        area: Some(area),
    }) = &last.edit
    else {
        panic!("close writes an area op")
    };
    assert_eq!(*target, id);
    let Shape::Path(points) = area.marker_area().shape else {
        panic!("the op holds the path")
    };
    assert_eq!(points.len(), 4);
    for (actual, expected) in points.iter().zip(expected) {
        for (a, b) in actual.iter().zip(expected) {
            assert!((a - b).abs() < 0.001, "{points:?}");
        }
    }
    assert!(!e.marker_entries()[0].draft_path);

    e.choose(11, Choice::EditArea(id)).unwrap();
    script.press(&mut e, "area_shape").unwrap();
    let radius = e.status()["area_edit"]["radius"].as_f64().unwrap();
    assert!((radius - (16. / std::f64::consts::PI).sqrt()).abs() < 0.001);
    script.press(&mut e, "back").unwrap();
    e.choose(11, Choice::EditArea(id)).unwrap();
    aim_at_floor(&mut e, [4., 0.]);
    script.press(&mut e, "area_grab").unwrap();
    aim_at_floor(&mut e, [5., 0.]);
    script.press(&mut e, "area_release").unwrap();
    let point = e.status()["area_edit"]["points"][1][0].as_f64().unwrap();
    assert!((point - 5.).abs() < 0.001);
    script.press(&mut e, "properties").unwrap();
    assert_eq!(
        e.status()["area_edit"]["points"].as_array().unwrap().len(),
        3
    );
    script.press(&mut e, "back").unwrap();
    assert!(e.status()["area_edit"].is_null());
    assert_eq!(e.session.document(), &saved);
    let stored = e.session.areas().unwrap()[&id].clone();
    e.command(
        20,
        Command::MoveThing {
            id,
            frame: Frame {
                position: [1., 2., 0.],
                rotation: [0.5, 0., 0.],
            },
        },
    )
    .unwrap();
    e.poll(20);
    assert_eq!(e.session.areas().unwrap()[&id], stored);
    assert_eq!(e.marker_entries()[0].place.at, [1., 2., 0.]);
    assert!(
        e.session.document().ops[3..]
            .iter()
            .all(|op| { matches!(op.edit, Edit::Entry(EntryOp::Set { .. })) })
    );
}

#[test]
fn radius_steps_with_movement_snap_and_typed_dimensions_commit_together() {
    let (mut e, id) = hill();
    let mut script = Script::new();
    e.choose(2, Choice::EditArea(id)).unwrap();
    script.input(&mut e, json!({"movement":[0, 1, 0]})).unwrap();
    assert_eq!(e.status()["area_edit"]["radius"], json!(1.25));
    e.choose(3, Choice::Set(Prop::MovementSnap, 5)).unwrap();
    script
        .input(&mut e, json!({"movement":[0, -1, 0]}))
        .unwrap();
    assert_eq!(e.status()["area_edit"]["radius"], json!(0.25));
    script.press(&mut e, "properties").unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Properties));
    let sheet = screens::screen(Kind::Properties).sheet();
    let fields: Vec<_> = sheet.rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(fields, ["Radius", "Below", "Above"]);
    for (field, value) in [
        (sheet::AreaField::Radius, "2.75"),
        (sheet::AreaField::Below, "0.75"),
        (sheet::AreaField::Above, "2"),
    ] {
        e.choose(
            5,
            Choice::Number(sheet::NumberField::Area(field), value.into()),
        )
        .unwrap();
    }
    let bad = e
        .choose(
            6,
            Choice::Number(
                sheet::NumberField::Area(sheet::AreaField::Radius),
                "NaN".into(),
            ),
        )
        .unwrap_err();
    assert_eq!(bad, AreaError::NotFinite.to_string());
    assert_eq!(e.session.document().ops.len(), 2);
    script.press(&mut e, "accept").unwrap();
    assert_eq!(e.session.document().ops.len(), 3);
    assert_eq!(
        e.session.areas().unwrap()[&id].marker_area(),
        Area {
            shape: Shape::Radius(2.75),
            below: 0.75,
            above: 2.0
        }
    );
}

#[test]
fn forge_input_grows_a_box_on_the_sticks_two_axes_and_saves_it_in_one_op() {
    let (mut e, id) = hill();
    let mut script = Script::new();
    let facing = e.session.thing(id).unwrap().frame.rotation[0];
    // The monitor looks along the flag's facing.
    let look = |e: &mut Editor, yaw: f32| {
        e.tool.camera = Some(Camera {
            position: [-3., 0., 0.],
            yaw,
            pitch: 0.,
        });
    };
    look(&mut e, facing);
    e.choose(2, Choice::EditArea(id)).unwrap();
    script.press(&mut e, "area_shape").unwrap();
    assert_eq!(e.status()["area_edit"]["shape"], "box");
    assert_eq!(e.status()["area_edit"]["half"], json!([1.0, 1.0]), "the circle's radius each way");
    // Forward steps the half length, right the half width: one step a lean.
    script.input(&mut e, json!({"movement":[0, 1, 0]})).unwrap();
    script.input(&mut e, json!({"movement":[1, 0, 0]})).unwrap();
    script.input(&mut e, json!({"movement":[0, 1, 0]})).unwrap();
    assert_eq!(e.status()["area_edit"]["half"], json!([1.5, 1.25]));
    // The box in hand is drawn on the flag's facing.
    let entry = &e.marker_entries()[0];
    assert!((entry.place.forward[0] - facing.cos()).abs() < 1e-4);
    assert!((entry.place.forward[1] - facing.sin()).abs() < 1e-4);
    // Looking across the facing, forward steps the half width and right
    // the half length.
    look(&mut e, facing + std::f32::consts::FRAC_PI_2);
    script.input(&mut e, json!({"movement":[0, -1, 0]})).unwrap();
    assert_eq!(e.status()["area_edit"]["half"], json!([1.5, 1.0]));
    script.input(&mut e, json!({"movement":[0, 1, 0]})).unwrap();
    assert_eq!(e.status()["area_edit"]["half"], json!([1.5, 1.25]));
    look(&mut e, facing);
    script.press(&mut e, "properties").unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Properties));
    let sheet = screens::screen(Kind::Properties).sheet();
    let fields: Vec<_> = sheet.rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(fields, ["Half length", "Half width", "Below", "Above"]);
    let bad = e
        .choose(
            5,
            Choice::Number(sheet::NumberField::Area(sheet::AreaField::Width), "0.1".into()),
        )
        .unwrap_err();
    assert_eq!(bad, AreaError::Half([1.5, 0.1]).to_string());
    assert_eq!(e.session.document().ops.len(), 2, "nothing is written in the draft");
    script.press(&mut e, "accept").unwrap();
    let ops = &e.session.document().ops;
    assert_eq!(ops.len(), 3, "one area op");
    let Edit::Entry(EntryOp::Area {
        target,
        area: Some(area),
    }) = &ops[2].edit
    else {
        panic!("accept writes an area op")
    };
    assert_eq!(*target, id);
    assert_eq!(
        area.marker_area(),
        Area {
            shape: Shape::Box { half: [1.5, 1.25] },
            below: 0.5,
            above: 1.5
        }
    );
    // Edit area opens the stored box as it is.
    e.choose(9, Choice::EditArea(id)).unwrap();
    assert_eq!(e.status()["area_edit"]["half"], json!([1.5, 1.25]));
}

#[test]
fn a_box_turns_into_a_sphere_of_its_floor_and_the_sphere_saves_with_no_height() {
    let (mut e, id) = hill();
    let mut script = Script::new();
    e.choose(2, Choice::EditArea(id)).unwrap();
    script.press(&mut e, "area_shape").unwrap();
    script.press(&mut e, "area_shape").unwrap();
    let draft = e.status()["area_edit"].clone();
    assert_eq!(draft["shape"], "sphere");
    let radius = draft["radius"].as_f64().unwrap();
    assert!((radius - (4. / std::f64::consts::PI).sqrt()).abs() < 0.001, "{radius}");
    assert_eq!((&draft["below"], &draft["above"]), (&json!(0.0), &json!(0.0)));
    // A sphere grows on the forward axis alone.
    script.input(&mut e, json!({"movement":[1, 1, 0]})).unwrap();
    let grown = e.status()["area_edit"]["radius"].as_f64().unwrap();
    assert!((grown - radius - 0.25).abs() < 0.001, "{grown}");
    script.press(&mut e, "properties").unwrap();
    assert_eq!(screens::take_request(), Some(Kind::Properties));
    let sheet = screens::screen(Kind::Properties).sheet();
    let fields: Vec<_> = sheet.rows.iter().map(|row| row.label.as_str()).collect();
    assert_eq!(fields, ["Radius"], "a sphere has no height to type");
    e.choose(
        5,
        Choice::Number(sheet::NumberField::Area(sheet::AreaField::Radius), "2".into()),
    )
    .unwrap();
    let refused = e
        .choose(
            6,
            Choice::Number(sheet::NumberField::Area(sheet::AreaField::Below), "1".into()),
        )
        .unwrap_err();
    assert!(refused.contains("no height"), "{refused}");
    script.press(&mut e, "accept").unwrap();
    assert_eq!(e.session.document().ops.len(), 3);
    assert_eq!(e.session.areas().unwrap()[&id].marker_area(), Area::sphere(2.0));
    // Opened again, Y makes an empty path at the default heights, and Y
    // again the sphere's radius as a circle.
    e.choose(12, Choice::EditArea(id)).unwrap();
    assert_eq!(e.status()["area_edit"]["shape"], "sphere");
    script.press(&mut e, "area_shape").unwrap();
    let draft = e.status()["area_edit"].clone();
    assert_eq!(draft["points"], json!([]));
    assert_eq!((&draft["below"], &draft["above"]), (&json!(0.5), &json!(1.5)));
    script.press(&mut e, "area_shape").unwrap();
    assert_eq!(e.status()["area_edit"]["shape"], "radius");
    assert_eq!(e.status()["area_edit"]["radius"], json!(2.0));
}

#[test]
fn a_flag_return_edits_its_capture_plate() {
    use crate::forge_glue::editing::area::default_area;
    assert_eq!(default_area("ctf_flag_return"), Some(Area::plate()));
    assert_eq!(default_area("assault_bomb_return"), Some(Area::plate()));
    assert_eq!(default_area("siege_goal"), Some(Area::plate()));
    assert_eq!(default_area("hill_corner"), Some(Area::default()));
    assert_eq!(default_area("territory"), Some(Area::default()));
    assert_eq!(default_area("ctf_flag_spawn"), None);

    // The hill's flag made a flag return, with its area cleared.
    let (mut e, id) = hill();
    let mut script = Script::new();
    e.tool.aimed_thing = Some(id);
    e.choose(2, Choice::Set(Prop::Role, 1)).unwrap();
    e.poll(2);
    e.stage(After::Area, |s, w| s.set_area(w, 3, id, None)).unwrap();
    e.poll(3);
    assert!(!e.session.areas().unwrap().contains_key(&id.identity()));
    assert_eq!(
        e.marker_entries()[0].marker.kind,
        game_markers::Kind::FlagReturn(Area::plate())
    );
    let ops = e.session.document().ops.len();
    // Its Properties offer Edit area, which starts from the capture plate.
    script.press(&mut e, "properties").unwrap();
    assert_eq!(screens::take_request(), Some(Kind::ObjectActions));
    let sheet = screens::screen(Kind::Properties).sheet();
    let row = sheet
        .rows
        .iter()
        .find(|row| row.label == "Edit area")
        .expect("a flag return has Edit area");
    let sheet::Act::Choose(choice) = &row.act else {
        panic!("Edit area starts a draft")
    };
    e.choose(5, choice.clone()).unwrap();
    let draft = e.status()["area_edit"].clone();
    assert_eq!(draft["shape"], "radius");
    assert_eq!(draft["radius"], json!(0.5));
    script.press(&mut e, "area_shape").unwrap();
    // Looking along the flag's facing, right steps the half width.
    e.tool.camera = Some(Camera {
        position: [-3., 0., 0.],
        yaw: e.session.thing(id).unwrap().frame.rotation[0],
        pitch: 0.,
    });
    script.input(&mut e, json!({"movement":[1, 0, 0]})).unwrap();
    script.press(&mut e, "accept").unwrap();
    assert_eq!(e.session.document().ops.len(), ops + 1);
    let stored = e.session.areas().unwrap()[&id.identity()].marker_area();
    assert_eq!(stored.shape, Shape::Box { half: [0.5, 0.75] });
    assert_eq!((stored.below, stored.above), (Area::plate().below, Area::plate().above));
}

#[test]
fn a_flags_properties_offer_show_in_play_and_on_writes_one_show_op() {
    let (mut e, id) = hill();
    let mut script = Script::new();
    e.tool.aimed_thing = Some(id);
    script.press(&mut e, "properties").unwrap();
    assert_eq!(screens::take_request(), Some(Kind::ObjectActions));
    let sheet = screens::screen(Kind::Properties).sheet();
    let row = sheet
        .rows
        .iter()
        .find(|row| row.label == "Show in play")
        .expect("a flag has Show in play");
    // The hill's flag is one the document added (the base has none), so
    // it is shown in play until an op turns it off.
    assert_eq!(row.value.as_deref(), Some("On"));
    let sheet::Act::Pick(pick) = &row.act else {
        panic!("Show in play is a picker")
    };
    assert_eq!(pick.values, ["On", "Off"]);
    assert_eq!(pick.choices[1], Choice::Set(Prop::ShowInPlay, 1));
    let ops = e.session.document().ops.len();
    e.choose(5, pick.choices[1].clone()).unwrap();
    e.poll(5);
    assert!(!e.session.shown(id));
    assert_eq!(e.session.document().ops.len(), ops + 1);
    let Some(Edit::Entry(EntryOp::Show { on, .. })) =
        e.session.document().ops.last().map(|op| op.edit.clone())
    else {
        panic!("the last op is a show")
    };
    assert!(!on);
    // Its Properties now say Off, and On turns it back.
    script.press(&mut e, "properties").ok();
    e.choose(6, Choice::Set(Prop::ShowInPlay, 0)).unwrap();
    e.poll(6);
    assert!(e.session.shown(id));
}
