//! Test helpers: the scenario crate's invented map (no game data) in a
//! scratch directory.

use std::path::PathBuf;

use scenario_edit::{MapSource, Session, fixture};

/// A scratch directory, removed on drop.
pub struct Temp(pub PathBuf);

impl Temp {
    pub fn new(tag: &str) -> Temp {
        let dir = std::env::temp_dir().join(format!("h2e-play-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
    /// The invented map, written as `<name>.map`.
    pub fn map(&self, name: &str) -> PathBuf {
        let p = self.0.join(format!("{name}.map"));
        std::fs::write(&p, fixture::fixture()).unwrap();
        p
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A session with the invented map open.
pub fn session(tag: &str) -> (Temp, Session) {
    let t = Temp::new(tag);
    let mut s = Session::new();
    s.open(MapSource::file(t.map("example"))).unwrap();
    (t, s)
}

#[cfg(unix)]
pub(crate) mod fake {
    mod object_game;

    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use agent::{Access, Agent, GameStatus, Reader, Server};
    use agent_proto::sim::SessionKind;
    use agent_proto::{Object, RpcError, code, live};
    use serde_json::{Value, json};

    use crate::app::App;
    use crate::play::Play;

    pub(crate) const PLAYER_AT: [f32; 3] = [10.0, -4.0, 2.0];
    pub(crate) const VEHICLE: u32 = 0x0010_0002;
    pub(crate) const UNIT: u32 = 0x0010_0001;
    pub(crate) const SCNR: &str = "0xe1780003";

    // The fixture starts at BSP 0 (epoch 1) and can advance to BSP 1.
    fn fixture_epoch(ctx: &agent::GameCtx<'_>) -> u64 {
        1 + u64::from(ctx.at.bsp.unwrap_or_default())
    }

    struct World {
        map: String,
        tick: u64,
        edits: Arc<Mutex<FakeEdits>>,
        moved: Arc<AtomicBool>,
        bsp: Arc<AtomicU32>,
    }

    impl Reader for World {
        fn status(&mut self) -> GameStatus {
            GameStatus {
                tick: Some(self.tick),
                map: Some(self.edits.lock().unwrap().map.clone().unwrap_or_else(|| self.map.clone())),
                mode: Some("multiplayer".into()),
                bsp: Some(self.bsp.load(Ordering::Relaxed)),
            }
        }

        fn live_metadata(&mut self) -> (u16, Option<u32>) {
            (30, Some(self.edits.lock().unwrap().checksum))
        }

        fn objects(&mut self) -> Result<Vec<Object>, String> {
            panic!("the editor must read the mirror, not poll world.query")
        }

        fn live_capture(&mut self, layers: live::Layers, world: &mut live::World) -> bool {
            world.objects.clear();
            world.players.clear();
            let local_unit = self.edits.lock().unwrap().replacement_unit.unwrap_or(UNIT);
            if layers.objects {
                let unit = live::Object {
                    id: local_unit,
                    tag: Some(0x1234),
                    kind: 0,
                    position: PLAYER_AT,
                    scale: 1.0,
                    forward: [0.0, 1.0, 0.0],
                    up: [0.0, 0.0, 1.0],
                    ..Default::default()
                };
                world.objects.push(unit);
                world.objects.push(live::Object {
                    id: VEHICLE,
                    kind: 1,
                    position: if self.moved.load(Ordering::Relaxed) {
                        [5.0, 2.0, 3.0]
                    } else {
                        [1.0, 2.0, 3.0]
                    },
                    ..unit
                });
                self.edits.lock().unwrap().objects.capture(world);
            }
            if layers.objects && let Some(object) = self.edits.lock().unwrap().extra_object {
                world.objects.push(object);
            }
            if layers.players {
                world.players.push(live::Player {
                    index: 0,
                    name: "Tester".into(),
                    unit: Some(local_unit),
                    local_user: Some(0),
                    aim: [0.0, 1.0, 0.0],
                    ..Default::default()
                });
            }
            true
        }
    }

    /// The invented game: what the host methods were sent.
    #[derive(Default)]
    pub(crate) struct FakeEdits {
        objects: object_game::Objects,
        pub(crate) checksum: u32,
        pub(crate) extra_object: Option<live::Object>,
        pub(crate) replacement_unit: Option<u32>,
        map: Option<String>,
        // While the new tag index loads, edit.map is unavailable, then can still
        // report the former checksum at the new epoch for one poll.
        pub(crate) loading: bool,
        former_checksum: Option<u32>,
        pub(crate) hold_reload_reply: bool,
        pub(crate) release_batch_reply: Option<Arc<AtomicBool>>,
        pub(crate) batch_error: Option<RpcError>,
        pub(crate) indicator_failure: Option<&'static str>,
        pub(crate) mark_replies: std::collections::VecDeque<Result<Value, RpcError>>,
        pub(crate) teleport_map_ticks: u64,
        pub(crate) external_live: bool,
        pub(crate) peers: Option<u32>,
        pub(crate) bytes: std::collections::HashMap<String, String>,
        // The epoch the edited bytes belong to: a new map load starts from its file.
        bytes_epoch: Option<u64>,
        pub(crate) placements: std::collections::HashMap<String, u32>,
        pub(crate) journal: Vec<Value>,
        /// The sealed scene's views, once [`Game::scene`] gave the game one.
        pub(crate) views: Vec<live::View>,
    }
    /// The agent server and the state observed at its synthetic game boundaries.
    pub(crate) struct Game {
        kind: SessionKind,
        pub(crate) edits: Arc<Mutex<FakeEdits>>,
        pub(crate) path: std::path::PathBuf,
        pub(crate) agent: Agent,
        _server: Server,
        /// Hold this lock to keep a queued request from reaching the next boundary.
        pub(crate) boundary: Arc<Mutex<()>>,
        pub(crate) bsp: Arc<AtomicU32>,
        stop: Arc<AtomicBool>,
        pub(crate) moved: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
        calls: Arc<Mutex<Vec<(String, Value)>>>,
    }

    impl Game {
        pub(crate) fn start(tag: &str, map: &str, kind: SessionKind) -> Game {
            let path = std::env::temp_dir().join(format!("h2e-{}-{tag}.sock", std::process::id()));
            let agent = Agent::new();
            let calls: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
            let rec = |name: &'static str| {
                let calls = Arc::clone(&calls);
                move |v: Value| {
                    calls.lock().unwrap().push((name.to_owned(), v.clone()));
                    v
                }
            };
            let edits = Arc::new(Mutex::new(FakeEdits {
                peers: (kind == SessionKind::MpHost).then_some(1),
                ..Default::default()
            }));
            let loaded = map.to_owned();
            let state = edits.clone();
            agent.add_game_method("edit.map", Access::Read, move |_, ctx| {
                let mut state = state.lock().unwrap();
                if state.loading {
                    state.loading = false;
                    return Err(RpcError::new(code::UNAVAILABLE, "tag index is loading"));
                }
                let checksum = state.former_checksum.take().unwrap_or(state.checksum);
                let mut session = agent_proto::sim::SessionView {
                    kind,
                    peers: state.peers,
                    ..Default::default()
                }
                .settle();
                if kind == SessionKind::MpHost && state.peers.is_none() {
                    session.writes = false;
                    session.why = Some("multiplayer host peer count is unknown".into());
                }
                if state.external_live {
                    session.writes = false;
                    session.why = Some("external Live session".into());
                }
                Ok(
                    json!({"name":state.map.as_ref().unwrap_or(&loaded),"file":"invented.map","checksum":checksum,"epoch":fixture_epoch(ctx),
                    "scenario":SCNR,"session":session}),
                )
            });
            let state = edits.clone();
            agent.add_method("edit.changes", Access::Read, move |_| {
                Ok(json!({"changes":state.lock().unwrap().journal}))
            });
            let r = rec("edit.read");
            let state = Arc::clone(&edits);
            agent.add_game_method("edit.read", Access::Read, move |params, ctx| {
                let params = r(params);
                if params["epoch"] != fixture_epoch(ctx) {
                    return Err(RpcError::new(code::INVALID_REQUEST, "map epoch changed"));
                }
                let at = params["at"].as_u64().unwrap();
                let state = state.lock().unwrap();
                let bytes = state.bytes.get(&at.to_string()).unwrap();
                assert_eq!(params["len"], (bytes.len() / 2) as u64);
                Ok(json!({"at":at, "holds_bytes":bytes}))
            });
            let r = rec("edit.get");
            let state = Arc::clone(&edits);
            agent.add_game_method("edit.get", Access::Read, move |params, ctx| {
                let params = r(params);
                if params["epoch"] != fixture_epoch(ctx) {
                    return Err(RpcError::new(code::INVALID_REQUEST, "map epoch changed"));
                }
                let state = state.lock().unwrap();
                let key = params["path"].to_string();
                let at = state.placements[&key];
                Ok(json!({"addr":format!("{at:#010x}"), "value":state.bytes[&key]}))
            });
            let r = rec("edit.revert");
            let state = Arc::clone(&edits);
            agent.add_method("edit.revert", Access::Act, move |v| {
                kind.writes()
                    .map_err(|e| RpcError::new(code::INVALID_REQUEST, e))?;
                r(v);
                let mut state = state.lock().unwrap();
                let mut dropped = Vec::new();
                let mut reverted = Vec::new();
                for field in std::mem::take(&mut state.journal).into_iter().rev() {
                    let key = field["at"].to_string();
                    let holds = state.bytes.get_mut(&key).unwrap();
                    if Some(holds.as_str()) == field["new_bytes"].as_str() {
                        *holds = field["was_bytes"].as_str().unwrap().into();
                        reverted.push(json!({"at":field["at"], "holds_bytes":holds}));
                    } else {
                        dropped.push(json!({
                            "id":field["id"], "at":field["at"], "holds_bytes":holds,
                            "why":"the game changed the field"
                        }));
                    }
                }
                Ok(json!({"reverted":reverted, "dropped":dropped}))
            });
            let r = rec("edit.batch");
            let state = Arc::clone(&edits);
            agent.add_game_method("edit.batch", Access::Act, move |params, ctx| {
                kind.writes()
                    .map_err(|e| RpcError::new(code::INVALID_REQUEST, e))?;
                let params = r(params);
                if params["epoch"] != fixture_epoch(ctx) {
                    return Err(RpcError::new(code::INVALID_REQUEST, "map epoch changed"));
                }
                let writes = params["writes"].as_array().unwrap();
                let mut state = state.lock().unwrap();
                if let Some(error) = state.batch_error.take() {
                    return Err(error);
                }
                // A map change loads the file again: the old epoch's edits are gone.
                if state.bytes_epoch.is_some_and(|epoch| epoch != fixture_epoch(ctx)) {
                    state.bytes.clear();
                    state.journal.clear();
                }
                state.bytes_epoch = Some(fixture_epoch(ctx));
                let key = |w: &Value| w.get("at").unwrap_or(&w["path"]).to_string();
                for w in writes {
                    if let Some(holds) = state.bytes.get(&key(w))
                        && w["was"].as_str() != Some(holds.as_str())
                        && w["value"].as_str() != Some(holds.as_str())
                    {
                        let at = w["at"].as_u64().or_else(|| {
                            state.placements.get(&key(w)).copied().map(u64::from)
                        });
                        return Err(RpcError {
                            code: code::MISMATCH,
                            message: "game changed the field".into(),
                            data: Some(json!({"fields":[{
                                "at":at, "path":w["path"], "was":w["was"], "holds_bytes":holds
                            }]})),
                        });
                    }
                }
                for w in writes {
                    state.bytes.insert(key(w), w["value"].as_str().unwrap().into());
                    if let Some(at) = w["at"].as_u64() {
                        state.journal.push(json!({
                            "id":1, "at":at, "addr":format!("{at:#010x}"), "holds":w["value"],
                            "new_bytes":w["value"], "was_bytes":w["was"],
                            "path":"fixture field", "stale":false
                        }));
                    }
                }
                let reply = json!({"id":1,"applied":writes.len(),"changes":writes});
                if let Some(release) = state.release_batch_reply.take() {
                    let held = reply.clone();
                    ctx.defer(600, Box::new(move |_| {
                        release.load(Ordering::Acquire).then(|| Ok(held.clone()))
                    }));
                }
                Ok(reply)
            });
            let r = rec("sim.state.set");
            let state = Arc::clone(&edits);
            agent.add_game_method(agent_proto::sim::method::STATE_SET, Access::Act, move |v, ctx| {
                state.lock().unwrap().teleport_map_ticks = ctx.at.map_ticks;
                // The game's own refusal (boot's sim glue).
                kind.writes()
                    .map_err(|m| RpcError::new(code::INVALID_REQUEST, m))?;
                r(v);
                Ok(json!({"wrote": ["pos"]}))
            });
            let bsp = Arc::new(AtomicU32::new(0));
            let reload_bsp = Arc::clone(&bsp);
            let state = Arc::clone(&edits);
            let r = rec(agent_proto::sim::method::MAP_RELOAD);
            agent.add_game_method(agent_proto::sim::method::MAP_RELOAD, Access::Act, move |v, ctx| {
                let params = r(v);
                let file = params["file"].as_str().unwrap();
                let map = scenario_edit::MapSource::file(file).open().unwrap();
                let checksum = map.header().checksum;
                let mut state = state.lock().unwrap();
                state.former_checksum = Some(state.checksum);
                state.map = Some(map.header().name.clone());
                state.loading = true;
                state.checksum = checksum;
                state.journal.clear();
                reload_bsp.fetch_add(1, Ordering::Relaxed);
                let reply = json!({"queued": true, "map": state.map, "file": file, "checksum": checksum});
                if state.hold_reload_reply {
                    ctx.defer(300, Box::new(|_| None));
                }
                Ok(reply)
            });
            let r = rec(agent_proto::editor::method::FOCUS);
            agent.add_method(agent_proto::editor::method::FOCUS, Access::Act, move |params| {
                r(params);
                Ok(json!({}))
            });
            let server = Server::listen(&path, agent.clone()).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let (s, a) = (Arc::clone(&stop), agent.clone());
            let map = map.to_owned();
            let moved = Arc::new(AtomicBool::new(false));
            let game_moved = Arc::clone(&moved);
            let boundary = Arc::new(Mutex::new(()));
            let game_boundary = Arc::clone(&boundary);
            let game_bsp = Arc::clone(&bsp);
            let world_edits = Arc::clone(&edits);
            let thread = thread::spawn(move || {
                let mut w = World {
                    map,
                    tick: 100,
                    edits: world_edits,
                    moved: game_moved,
                    bsp: game_bsp,
                };
                while !s.load(Ordering::Relaxed) {
                    let boundary = game_boundary.lock().unwrap();
                    w.tick += 1;
                    a.on_tick(&mut w);
                    a.between_slices(&mut w, 0);
                    drop(boundary);
                    thread::sleep(Duration::from_millis(5));
                }
            });
            Game {
                kind,
                edits,
                path,
                agent,
                _server: server,
                boundary,
                bsp,
                stop,
                moved,
                thread: Some(thread),
                calls,
            }
        }

        /// Register presentation on this game before the editor asks for its methods.
        pub(crate) fn presentation(&self, window: bool) {
            use agent_proto::editor::{LinkParams, MarksParams, method};

            let calls = self.calls.clone();
            let edits = self.edits.clone();
            self.agent.add_method(method::LINK, Access::Read, move |params| {
                let link: LinkParams = serde_json::from_value(params.clone()).unwrap();
                calls.lock().unwrap().push((method::LINK.into(), params));
                if let Some(why) = edits.lock().unwrap().indicator_failure {
                    return Err(RpcError::new(code::UNAVAILABLE, why));
                }
                let shown = window && link.clear != Some(true);
                Ok(json!({"shown":shown, "expires_ms":if shown { 3000 } else { 0 },
                    "why":(!window).then_some("this host has no window")}))
            });
            let calls = self.calls.clone();
            let edits = self.edits.clone();
            self.agent.add_method(method::MARKS, Access::Read, move |params| {
                let marks: MarksParams = serde_json::from_value(params.clone()).unwrap();
                calls.lock().unwrap().push((method::MARKS.into(), params));
                if let Some(reply) = edits.lock().unwrap().mark_replies.pop_front() {
                    return reply;
                }
                Ok(json!({"held":marks.marks.len(), "drawn":window && !marks.marks.is_empty(),
                    "why":(!window).then_some("this host has no window"), "refused":[]}))
            });
        }

        /// A scene source with these views, as the native renderer's scene
        /// glue registers one. Call before pairing: the subscription
        /// negotiates the `camera` layer.
        pub(crate) fn scene(&self, views: Vec<live::View>) {
            self.edits.lock().unwrap().views = views;
            let edits = Arc::clone(&self.edits);
            self.agent
                .set_camera_source(Arc::new(move |out: &mut Vec<live::View>| {
                    out.extend(edits.lock().unwrap().views.iter().cloned());
                    Ok::<(), String>(())
                }));
        }

        pub(crate) fn calls(&self, name: &str) -> Vec<Value> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, v)| v.clone())
                .collect()
        }
    }

    impl Drop for Game {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            self.agent.shutdown();
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    /// Runs the editor's frames until `done` holds (or fails after 25 s; a call to a game that
    /// stopped answering times out after 10).
    pub(crate) fn until(
        play: &mut Play,
        app: &mut App,
        what: &str,
        mut done: impl FnMut(&Play, &App) -> bool,
    ) {
        let end = Instant::now() + Duration::from_secs(25);
        while Instant::now() < end {
            play.update(app);
            if done(play, app) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!(
            "timed out waiting for {what}; log: {:?}",
            app.log().iter().map(|l| &l.status).collect::<Vec<_>>()
        );
    }

    /// Advance just the presentation clock. The ordered control reply proves that
    /// every replacement issued at this instant has reached the fake game.
    pub(crate) fn presence_at(play: &mut Play, app: &mut App, now: Instant) {
        use crate::play::link::{Event, Purpose};

        play.update_presence(app, now);
        play.call(agent_proto::method::STATUS, Value::Null, Purpose::Other);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            let event = play.link.as_ref().unwrap().next(wait)
                .expect("the fake game's ordered control reply");
            if matches!(&event, Event::Answer { purpose: Purpose::Other, method, result: Ok(_) }
                if method == agent_proto::method::STATUS)
            {
                break;
            }
            play.on_event(app, event);
        }
        play.publish(app);
    }

    pub(crate) fn batch_pending(play: &Play) -> bool {
        play.edits.sent.is_some()
    }

    pub(crate) fn pair_for_view(play: &mut Play, app: &mut App, game: &Game) {
        play.pair(app, agent_proto::Endpoint::Unix(game.path.clone()));
        until(play, app, "the loaded map and its journal", |p, a| {
            p.edits.journal_ready && p.edit_refusal(a).is_none()
        });
    }
}
