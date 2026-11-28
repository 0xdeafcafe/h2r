use std::sync::atomic::AtomicUsize;
use std::sync::mpsc::{Receiver, sync_channel};

use serde_json::json;

use super::*;
use crate::{ConnId, GameStatus, Reader};

struct Nothing;

impl Reader for Nothing {
    fn status(&mut self) -> GameStatus {
        GameStatus::default()
    }

    fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
        Ok(vec![])
    }
}

fn connect(agent: &Agent) -> (ConnId, Receiver<String>) {
    let (tx, rx) = sync_channel(16);
    (agent.connect(tx), rx)
}

fn call(agent: &Agent, conn: ConnId, rx: &Receiver<String>, method: &str, params: Value) -> Value {
    agent.submit(
        conn,
        &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string(),
    );
    agent.between_slices(&mut Nothing, 0);
    serde_json::from_str(&rx.try_recv().unwrap()).unwrap()
}

#[test]
fn link_lease_renews_expires_and_clears() {
    let agent = Agent::new();
    let link = EditorLink::default();
    let start = Instant::now();
    let clock = Arc::new(Mutex::new(start));
    let now = Arc::clone(&clock);
    install_with_clock(&agent, Some(link.clone()), move || *now.lock().unwrap());
    let (conn, rx) = connect(&agent);
    let send = |params| call(&agent, conn, &rx, method::LINK, params)["result"].clone();

    let set = send(json!({"editor": "sample", "mode": "view", "text": "One", "ttl_ms": 500}));
    assert_eq!(set["shown"], true);
    assert_eq!(set["expires_ms"], 500);
    let badge = link.current(start + Duration::from_millis(499)).unwrap();
    assert_eq!(badge.editor, "sample");
    assert_eq!(badge.mode, Mode::View);
    assert_eq!(badge.text.as_deref(), Some("One"));
    assert_eq!(badge.expires, start + Duration::from_millis(500));

    let renewed = start + Duration::from_millis(499);
    *clock.lock().unwrap() = renewed;
    let edit = json!({"editor": "sample", "mode": "edit"});
    let set = send(edit.clone());
    assert_eq!(set["expires_ms"], LINK_TTL_MS);
    let original_expiry = start + Duration::from_millis(500);
    assert_eq!(link.current(original_expiry).unwrap().mode, Mode::Edit);

    let expiry = renewed + Duration::from_millis(LINK_TTL_MS);
    assert!(link.current(expiry - Duration::from_millis(1)).is_some());
    assert!(link.current(expiry).is_none());

    *clock.lock().unwrap() = expiry;
    assert_eq!(send(edit)["shown"], true);
    let cleared = send(json!({"clear": true}));
    assert_eq!(cleared["shown"], false);
    assert_eq!(cleared["expires_ms"], 0);
    assert!(link.current(expiry).is_none());
}

#[test]
fn editor_methods_are_listed_and_report_no_window() {
    let agent = Agent::new();
    let (conn, rx) = connect(&agent);
    let send = |method, params| call(&agent, conn, &rx, method, params)["result"].clone();
    let methods = || send(agent_proto::method::METHODS, Value::Null);

    assert!(!methods().as_array().unwrap().contains(&json!(method::LINK)));
    for window in [None, Some(EditorLink::default())] {
        install(&agent, window);
        let methods = methods();
        let methods = methods.as_array().unwrap();
        assert!(methods.contains(&json!(method::LINK)));
        assert!(methods.contains(&json!(method::FOCUS)));
        assert!(!methods.contains(&json!(method::MARKS)));
        assert!(!methods.contains(&json!("editor.camera")));
    }

    install(&agent, None);
    let link = send(method::LINK, json!({"editor": "sample", "mode": "blocked"}));
    assert_eq!(link["shown"], false);
    assert_eq!(link["why"], NO_WINDOW);
    assert_eq!(link["expires_ms"], 0);
    let focus = send(method::FOCUS, Value::Null);
    assert_eq!(focus["focused"], false);
    assert_eq!(focus["why"], NO_WINDOW);
}

#[test]
fn link_rejects_invalid_params_without_replacing_the_lease() {
    let agent = Agent::new();
    let link = EditorLink::default();
    let start = Instant::now();
    install_with_clock(&agent, Some(link.clone()), move || start);
    let (conn, rx) = connect(&agent);
    let send = |params| call(&agent, conn, &rx, method::LINK, params);
    let text = "é".repeat(TEXT_MAX);
    let accepted =
        send(json!({"editor": "sample", "mode": "blocked", "text": text, "ttl_ms": 10000}));
    assert_eq!(accepted["result"]["shown"], true);
    let badge = link.current(start).unwrap();

    for params in [
        json!(["sample", "view", null, null, null]),
        json!({"editor": "sample", "mode": "unknown"}),
        json!({"editor": "sample"}),
        json!({"mode": "edit"}),
        json!({"editor": "sample", "mode": "edit", "text": "é".repeat(TEXT_MAX + 1)}),
        json!({"editor": "sample", "mode": "edit", "ttl_ms": 499}),
        json!({"editor": "sample", "mode": "edit", "ttl_ms": 10001}),
        json!({"editor": "sample", "mode": "edit", "clear": true}),
    ] {
        let response = send(params);
        assert_eq!(response["error"]["code"], code::INVALID_PARAMS, "{response}");
        assert_eq!(link.current(start).as_ref(), Some(&badge));
    }
}

#[test]
fn focus_wakes_the_window_and_obeys_agent_control() {
    let agent = Agent::new();
    let link = EditorLink::default();
    let wakes = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&wakes);
    link.set_waker(Arc::new(move || {
        count.fetch_add(1, Ordering::Relaxed);
    }));
    install(&agent, Some(link.clone()));
    let (controller, controller_rx) = connect(&agent);
    let (other, other_rx) = connect(&agent);
    call(
        &agent,
        controller,
        &controller_rx,
        agent_proto::method::CLAIM,
        Value::Null,
    );

    let denied = call(&agent, other, &other_rx, method::FOCUS, Value::Null);
    assert_eq!(denied["error"]["code"], code::NOT_CONTROLLER);
    assert!(!link.take_focus_request());
    assert_eq!(wakes.load(Ordering::Relaxed), 0);

    let read = call(
        &agent,
        other,
        &other_rx,
        method::LINK,
        json!({"editor": "sample", "mode": "view"}),
    );
    assert_eq!(read["result"]["shown"], true);
    let focus = call(&agent, controller, &controller_rx, method::FOCUS, Value::Null);
    assert_eq!(focus["result"]["focused"], true);
    assert!(link.take_focus_request());
    assert!(!link.take_focus_request());
    assert_eq!(wakes.load(Ordering::Relaxed), 2);
}

struct Game {
    guest: guest_layout::testing::FakeGuest,
    status: GameStatus,
    reads: usize,
}

impl Reader for Game {
    fn status(&mut self) -> GameStatus {
        self.status.clone()
    }

    fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
        Ok(Vec::new())
    }

    fn with_guest(&mut self, f: &mut dyn FnMut(&mut dyn guest_layout::GuestMemory)) -> bool {
        self.reads += 1;
        f(&mut self.guest);
        true
    }
}

struct SceneRig {
    agent: Agent,
    link: EditorLink,
    conn: ConnId,
    rx: Receiver<String>,
    now: Arc<Mutex<Instant>>,
    game: Game,
}

impl SceneRig {
    fn new(window: bool) -> Self {
        let agent = Agent::new();
        let link = EditorLink::default();
        let now = Arc::new(Mutex::new(Instant::now()));
        let clock = now.clone();
        super::scene::install_scene_with_clock(
            &agent,
            window.then(|| link.clone()),
            move || *clock.lock().unwrap(),
        );
        let (conn, rx) = connect(&agent);
        Self {
            agent,
            link,
            conn,
            rx,
            now,
            game: Game {
                guest: guest_layout::testing::FakeGuest::new(),
                status: GameStatus::default(),
                reads: 0,
            },
        }
    }

    fn call(&mut self, method: &str, params: Value) -> Value {
        let request = agent_proto::Request::new(1, method, params);
        self.agent.submit(self.conn, &serde_json::to_string(&request).unwrap());
        self.agent.on_tick(&mut self.game);
        serde_json::from_str(&self.rx.try_recv().unwrap()).unwrap()
    }

    fn capture(&self) -> Option<MarkFrame> {
        self.link.capture_marks(&self.game.guest, || self.now())
    }

    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }

    fn advance(&self, ms: u64) {
        *self.now.lock().unwrap() += Duration::from_millis(ms);
    }
}

#[test]
fn marks_hold_a_box_in_its_frame_and_expire_on_the_wall_clock() {
    let mut rig = SceneRig::new(true);
    let reply = rig.call(method::MARKS, json!({
        "marks": [{
            "box": {
                "origin": [1.0, 2.0, 3.0], "forward": [0.0, 1.0, 0.0],
                "up": [0.0, 0.0, 1.0], "size": [2.0, 4.0, 6.0]
            },
            "color": [1.0, 0.5, 0.0], "style": "candidate", "label": "box", "xray": true
        }],
        "ttl_ms": 500
    }));
    assert_eq!(reply["result"]["held"], 1);
    assert_eq!(reply["result"]["drawn"], false);
    assert_eq!(reply["result"]["why"], "waiting for a guest frame with marks");
    assert_eq!(rig.game.reads, 0);
    let (marks, expires) = rig.link.marks(rig.now()).unwrap();
    assert_eq!(marks.len(), 1);
    let expected = [
        [[1.0, 2.0, 3.0], [1.0, 4.0, 3.0]],
        [[1.0, 2.0, 3.0], [-3.0, 2.0, 3.0]],
        [[1.0, 2.0, 3.0], [1.0, 2.0, 9.0]],
        [[1.0, 4.0, 3.0], [-3.0, 4.0, 3.0]],
        [[1.0, 4.0, 3.0], [1.0, 4.0, 9.0]],
        [[-3.0, 2.0, 3.0], [-3.0, 4.0, 3.0]],
        [[-3.0, 2.0, 3.0], [-3.0, 2.0, 9.0]],
        [[-3.0, 4.0, 3.0], [-3.0, 4.0, 9.0]],
        [[1.0, 2.0, 9.0], [1.0, 4.0, 9.0]],
        [[1.0, 2.0, 9.0], [-3.0, 2.0, 9.0]],
        [[1.0, 4.0, 9.0], [-3.0, 4.0, 9.0]],
        [[-3.0, 2.0, 9.0], [-3.0, 4.0, 9.0]],
    ];
    assert_eq!(marks[0].edges, expected);
    assert_eq!(marks[0].label.as_deref(), Some("box"));
    assert_eq!(marks[0].style, Some(agent_proto::editor::MarkStyle::Candidate));
    assert_eq!(expires, rig.now() + Duration::from_millis(500));
    rig.advance(499);
    assert!(rig.link.marks(rig.now()).is_some());
    rig.advance(1);
    assert!(rig.link.marks(rig.now()).is_none());
}

fn mark_object(
    guest: &mut guest_layout::testing::FakeGuest,
) -> (guest_layout::DatumHandle, u32, guest_layout::Object) {
    use guest_layout::view::{fake, objects};
    use guest_layout::{Datum, Object};

    const ARRAY: u32 = 0x8000;
    const BODY: u32 = 0x9000;
    guest.put_u32(guest_layout::points::OBJECTS.base, ARRAY);
    let table = fake::datums::<objects::Header>(guest, ARRAY, 4);
    let (handle, header) = fake::push(guest, &table);
    header.set_body(guest, BODY).unwrap();
    guest.map(BODY, Object::SIZE);
    let body = Object::at(BODY);
    body.set(guest, Object::POSITION, [12.0, 12.0, 12.0]).unwrap();
    body.set(guest, Object::FORWARD, [0.0, 1.0, 0.0]).unwrap();
    body.set(guest, Object::UP, [0.0, 0.0, 1.0]).unwrap();
    body.set(guest, Object::SCALE, 3.0).unwrap();
    body.set(guest, Object::PARENT, Datum::NONE).unwrap();
    body.set(guest, Object::BOUND_CENTER, [10.0, 20.0, 30.0]).unwrap();
    body.set(guest, Object::BOUND_RADIUS, 2.0).unwrap();

    (handle, header.addr, body)
}

#[test]
fn object_marks_use_world_bounds_refresh_and_refuse_stale_salts_by_index() {
    use guest_layout::{Object, ObjectHeader};

    let mut rig = SceneRig::new(true);
    let (handle, header, body) = mark_object(&mut rig.game.guest);
    let point = json!({"point": [0.0, 0.0, 0.0], "color": [1.0, 0.0, 0.0]});
    let reply = rig.call(method::MARKS, json!({
        "marks": [
            {"object": format!("{:08x}", handle.raw() ^ 0x0001_0000), "color": [1.0, 1.0, 0.0]},
            {"object": format!("{:08x}", handle.raw()), "color": [0.0, 0.0, 1.0]},
            point.clone()
        ]
    }));
    assert_eq!(reply["result"]["held"], 2, "{reply}");
    assert_eq!(reply["result"]["drawn"], false);
    assert_eq!(reply["result"]["refused"], json!([{"index": 0, "why": "object is gone"}]));
    let first = rig.capture().unwrap();
    let marks = &first.shapes;
    assert_eq!(marks[0].anchor, [10.0, 20.0, 30.0]);
    assert_eq!(marks[0].edges.len(), 12);
    assert_eq!(marks[0].edges[0], [[8.0, 18.0, 28.0], [8.0, 22.0, 28.0]]);

    body.set(&mut rig.game.guest, Object::BOUND_CENTER, [11.0, 22.0, 33.0]).unwrap();
    rig.agent.on_tick(&mut rig.game);
    assert_eq!(rig.capture().unwrap().shapes[0].anchor, [11.0, 22.0, 33.0]);
    assert_eq!(first.shapes[0].anchor, [10.0, 20.0, 30.0]);

    ObjectHeader::at(header).set(&mut rig.game.guest, ObjectHeader::SALT, 0).unwrap();
    rig.agent.on_tick(&mut rig.game);
    // A pool reset can give the same slot its original salt. The gone
    // object has already left the lease, so it cannot rebind.
    ObjectHeader::at(header)
        .set(&mut rig.game.guest, ObjectHeader::SALT, (handle.raw() >> 16) as u16)
        .unwrap();
    let frame = rig.capture().unwrap();
    assert_eq!(frame.shapes.len(), 1);
    assert_eq!(frame.shapes[0].edges.len(), 3);
    rig.call(method::MARKS, json!({"marks": [point]}));
    assert_ne!(first.generation(), frame.generation());
    assert_eq!(rig.capture().unwrap().generation(), frame.generation());
    assert!(rig.link.frame_mark_deadline(&first, || rig.now()).is_none());
}

#[test]
fn retiring_the_last_object_wakes_the_window_and_invalidates_queued_marks() {
    use guest_layout::ObjectHeader;

    let mut rig = SceneRig::new(true);
    let (handle, header, _) = mark_object(&mut rig.game.guest);
    rig.call(method::MARKS, json!({
        "marks": [{"object": format!("{:08x}", handle.raw()), "color": [0, 0, 1]}]
    }));
    let frame = rig.capture().unwrap();
    let (wake, woke) = std::sync::mpsc::channel();
    let link = Arc::downgrade(&rig.link.0);
    rig.link.set_waker(Arc::new(move || {
        // A wake callback may immediately take the editor state lock.
        assert!(link.upgrade().unwrap().state.try_lock().is_ok());
        wake.send(()).unwrap();
    }));

    ObjectHeader::at(header).set(&mut rig.game.guest, ObjectHeader::SALT, 0).unwrap();
    rig.agent.on_tick(&mut rig.game);
    assert_eq!(woke.try_recv(), Ok(()));
    assert!(rig.link.frame_mark_deadline(&frame, || rig.now()).is_none());
}

#[test]
fn an_older_render_with_no_shapes_waits_for_the_next_image() {
    let mut rig = SceneRig::new(true);
    let (handle, _, _) = mark_object(&mut rig.game.guest);
    let params = json!({
        "marks": [{"object": format!("{:08x}", handle.raw()), "color": [0, 0, 1]}]
    });
    assert_eq!(rig.call(method::MARKS, params.clone())["result"]["held"], 1);

    let older = guest_layout::testing::FakeGuest::new();
    let empty = rig.link.capture_marks(&older, || rig.now()).unwrap();
    assert!(empty.shapes.is_empty());
    assert!(rig.link.frame_mark_deadline(&empty, || rig.now()).is_none());
    let waiting = rig.call(method::MARKS, params.clone());
    assert_eq!(waiting["result"]["drawn"], false);
    assert_eq!(waiting["result"]["held"], 1);
    let ready = rig.capture().unwrap();
    assert_eq!(ready.generation(), empty.generation());
    assert!(rig.link.frame_mark_deadline(&ready, || rig.now()).is_some());
    rig.link.set_drawn_mark_generation(ready.generation());
    assert_eq!(rig.call(method::MARKS, params)["result"]["drawn"], true);
}

#[test]
fn marks_reject_an_oversized_update_and_clear_on_an_empty_set() {
    let mut rig = SceneRig::new(true);
    let point = json!({"point": [1.0, 2.0, 3.0], "color": [1.0, 1.0, 0.0]});
    let first = rig.call(method::MARKS, json!({"marks": [point.clone()]}));
    assert_eq!(first["result"]["held"], 1);
    let reply = rig.call(method::MARKS, json!({"marks": vec![point; 257]}));
    assert_eq!(reply["error"]["code"], code::INVALID_PARAMS);
    assert_eq!(rig.link.marks(rig.now()).unwrap().0.len(), 1);
    let clear = rig.call(method::MARKS, json!({"marks": []}));
    assert_eq!(clear["result"]["held"], 0);
    assert!(rig.link.marks(rig.now()).is_none());
    assert_eq!(rig.game.reads, 0);
}

#[test]
fn headless_marks_are_held_and_the_guest_camera_is_absent() {
    let mut rig = SceneRig::new(false);
    let marks = rig.call(method::MARKS, json!({"marks": [{"point": [1, 2, 3], "color": [1, 1, 0]}]}));
    assert_eq!(marks["result"]["held"], 1);
    assert_eq!(marks["result"]["drawn"], false);
    assert_eq!(marks["result"]["why"], NO_WINDOW);
    let methods = rig.call(agent_proto::method::METHODS, Value::Null);
    assert!(methods["result"].as_array().unwrap().contains(&json!(method::MARKS)));
    assert!(!methods["result"].as_array().unwrap().contains(&json!("editor.camera")));
}

fn point_set() -> Value {
    json!({"marks": [{"point": [1, 2, 3], "color": [1, 1, 0]}]})
}

#[test]
fn mixed_geometry_refuses_the_whole_replacement() {
    let mut rig = SceneRig::new(true);
    rig.call(method::MARKS, point_set());
    let first = rig.capture().unwrap();
    for object in [json!("12345678"), json!(null)] {
        let reply = rig.call(method::MARKS, json!({
            "marks": [
                {"point": [10, 20, 30], "color": [1, 1, 0]},
                {"object": object, "point": [4, 5, 6], "color": [1, 0, 0]}
            ]
        }));
        assert_eq!(reply["error"]["code"], code::INVALID_PARAMS);
        assert_eq!(rig.capture().unwrap().shapes, first.shapes);
        assert!(rig.link.frame_mark_deadline(&first, || rig.now()).is_some());
    }
}

#[test]
fn editor_mark_sets_and_queued_samples_expire_on_epoch_changes() {
    let mut rig = SceneRig::new(true);
    rig.game.status.map = Some("example-one".into());
    rig.call(method::MARKS, point_set());
    let first = rig.capture().unwrap();
    rig.agent.live_cut();
    assert!(rig.capture().is_none());
    assert!(rig.link.frame_mark_deadline(&first, || rig.now()).is_none());

    rig.call(method::MARKS, point_set());
    let second = rig.capture().unwrap();
    rig.agent.load_state(&rig.agent.save_state()).unwrap();
    assert!(rig.capture().is_none());
    assert!(rig.link.frame_mark_deadline(&second, || rig.now()).is_none());

    rig.call(method::MARKS, point_set());
    let third = rig.capture().unwrap();
    rig.game.status.map = Some("example-two".into());
    rig.agent.on_tick(&mut rig.game);
    assert!(rig.capture().is_none());
    assert!(rig.link.frame_mark_deadline(&third, || rig.now()).is_none());

    rig.call(method::MARKS, point_set());
    assert!(rig.link.frame_mark_deadline(&third, || rig.now()).is_none());
}

#[test]
fn object_entries_are_refused_without_the_tick_observer() {
    let mut rig = SceneRig::new(true);
    rig.agent.set_no_tick_hook();
    let request = agent_proto::Request::new(1, method::MARKS, json!({
        "marks": [
            {"object": "12345678", "color": [1, 0, 0]},
            {"point": [1, 2, 3], "color": [1, 1, 0]}
        ]
    }));
    rig.agent.submit(rig.conn, &serde_json::to_string(&request).unwrap());
    rig.agent.between_slices(&mut rig.game, 0);
    let reply: Value = serde_json::from_str(&rig.rx.try_recv().unwrap()).unwrap();
    assert_eq!(reply["result"]["held"], 1);
    assert_eq!(reply["result"]["refused"], json!([
        {"index": 0, "why": "object marks need the agent tick observer"}
    ]));
    assert_eq!(rig.capture().unwrap().shapes.len(), 1);
    assert_eq!(rig.game.reads, 0);
}

#[test]
fn mark_capture_does_no_guest_reads_without_a_mark_set() {
    struct Unreadable;
    impl guest_layout::GuestMemory for Unreadable {
        fn read(&self, _: u32, _: &mut [u8]) -> Result<(), guest_layout::MemFault> {
            panic!("the idle path must not read the guest");
        }
        fn write(&mut self, _: u32, _: &[u8]) -> Result<(), guest_layout::MemFault> {
            panic!("mark capture is read-only");
        }
    }

    let mut rig = SceneRig::new(true);
    assert!(rig.link.capture_marks(&Unreadable, || panic!("the idle path must not read the clock")).is_none());
    rig.call(method::MARKS, point_set());
    assert!(rig.link.capture_marks(&Unreadable, || rig.now()).is_some());
    rig.call(method::MARKS, json!({"marks": []}));
    assert!(rig.link.capture_marks(&Unreadable, || panic!("the idle path must not read the clock")).is_none());
}

#[test]
fn mark_refusals_name_the_displayed_frame_source() {
    use MarkAvailability::*;
    let mut rig = SceneRig::new(true);
    for (status, why) in [
        (NoWorldView, "this guest frame has no captured world camera"),
        (RawFrame, "raw frame has no world camera"),
        (Loading, "the window is showing a loading picture"),
        (Synthetic, "synthetic frame has no camera for marks"),
    ] {
        rig.link.set_mark_frame_status(status);
        let reply = rig.call(method::MARKS, point_set());
        assert_eq!(reply["result"]["drawn"], false);
        assert_eq!(reply["result"]["why"], why);
    }
    rig.link.set_mark_frame_status(Waiting);
    rig.link.set_mark_capture_disabled(SceneOff);
    assert_eq!(rig.call(method::MARKS, point_set())["result"]["why"], "frame scene capture is off");

    rig.link.set_mark_frame_status(NoWorldView);
    rig.link.set_mark_capture_disabled(NativeOff);
    assert_eq!(
        rig.call(method::MARKS, point_set())["result"]["why"],
        "native rendering is off"
    );
}

#[test]
fn an_identical_renewal_waits_for_its_own_frame() {
    let mut rig = SceneRig::new(true);
    let guest = guest_layout::testing::FakeGuest::new();
    rig.call(method::MARKS, point_set());
    let first = rig.link.capture_marks(&guest, || rig.now()).unwrap();
    rig.link.set_drawn_mark_generation(first.generation());
    assert_eq!(rig.call(method::MARKS, point_set())["result"]["drawn"], true);

    let replacement = json!({"marks": [{"point": [2, 3, 4], "color": [1, 0, 0]}]});
    for _ in 0..2 {
        let reply = rig.call(method::MARKS, replacement.clone());
        assert_eq!(reply["result"]["drawn"], false);
        assert_eq!(
            reply["result"]["why"],
            "waiting for a guest frame with this mark set"
        );
    }

    let frame = rig.link.capture_marks(&guest, || rig.now()).unwrap();
    assert_ne!(first.generation(), frame.generation());
    rig.link.set_drawn_mark_generation(frame.generation());
    assert_eq!(rig.call(method::MARKS, replacement)["result"]["drawn"], true);
}

#[test]
fn an_old_image_skips_the_clock_after_the_marks_are_cleared() {
    let mut rig = SceneRig::new(true);
    rig.call(method::MARKS, point_set());
    let frame = rig.capture().unwrap();
    rig.call(method::MARKS, json!({"marks": []}));
    assert!(rig
        .link
        .frame_mark_deadline(&frame, || panic!("a stale image must not read the clock"))
        .is_none());
}

#[test]
fn render_capture_leaves_expiry_to_the_tick() {
    let mut rig = SceneRig::new(true);
    rig.call(method::MARKS, point_set());
    let frame = rig.capture().unwrap();
    rig.advance(LINK_TTL_MS);
    assert!(rig.capture().is_none());
    assert_eq!(
        rig.link.0.marks_generation.load(Ordering::Acquire),
        frame.generation()
    );
    rig.agent.on_tick(&mut rig.game);
    assert_eq!(rig.link.0.marks_generation.load(Ordering::Acquire), 0);
}
