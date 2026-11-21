use super::*;
use std::time::{Duration, Instant};

struct Film {
    dir: PathBuf,
}

impl Film {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "theater-transport-{}",
            live_proto::sign::random_id()
        ));
        std::fs::create_dir(&dir).unwrap();
        let fixture = Self { dir };
        let record = Session::open(
            Some(fixture.dir.join("synthetic.h2film")),
            None,
            host_facts(),
            b"xbe",
            &options(&fixture.dir),
        )
        .unwrap()
        .unwrap();
        let guest = arena();
        for _ in 0..60 {
            record.checkpoint(&guest);
        }
        record.finish(90, true).unwrap();
        fixture
    }

    fn open(
        &self,
        target: u64,
        from_tick: Option<u64>,
        paused: bool,
        source: Option<api::SeekRoute>,
    ) -> Arc<Session> {
        Session::open(
            None,
            Some(Replay {
                path: self.dir.join("synthetic.h2film"),
                render_every: 0,
                speed: 1.,
                seek_tick: target,
                from_tick,
                seek_source: source,
                paused_at_target: paused,
                presentation: Presentation::default(),
                restart: self.dir.join("restart.json"),
                camera: Default::default(),
                window_closed: None,
            }),
            host_facts(),
            b"xbe",
            &options(&self.dir),
        )
        .unwrap()
        .unwrap()
    }

    fn restart(&self, player: &Session) -> Restart {
        let state = player.lock();
        let Mode::Replay(_, replay) = &state.mode else {
            unreachable!()
        };
        assert!(state.write_restart(replay).unwrap());
        let file = File::open(self.dir.join("restart.json")).unwrap();
        serde_json::from_reader(file).unwrap()
    }
}

impl Drop for Film {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Transport {
    agent: agent::Agent,
    client: u64,
    replies: std::sync::mpsc::Receiver<String>,
}

impl Transport {
    fn new(player: &Arc<Session>) -> Self {
        let agent = agent::Agent::new();
        let (send, replies) = std::sync::mpsc::sync_channel(4);
        let client = agent.connect(send);
        player.add_methods(&agent);
        Self {
            agent,
            client,
            replies,
        }
    }

    fn call(&self, method: &str, params: Value) -> Value {
        struct Empty;
        impl agent::Reader for Empty {
            fn status(&mut self) -> agent::GameStatus {
                Default::default()
            }
            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
                Ok(Vec::new())
            }
        }
        self.agent.submit(
            self.client,
            &json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}).to_string(),
        );
        self.agent.poll_theater(&mut Empty);
        serde_json::from_str(&self.replies.try_recv().unwrap()).unwrap()
    }

    fn state(&self) -> api::State {
        self.action(api::STATE, json!({}))
    }

    fn action(&self, method: &str, params: Value) -> api::State {
        let reply = self.call(method, params);
        assert!(reply["error"].is_null(), "{reply}");
        serde_json::from_value(reply["result"].clone()).unwrap()
    }
}

#[test]
fn cancelled_seek_returns_to_departure_tick_after_a_cold_restart() {
    let film = Film::new();
    let guest = arena();
    let player = film.open(0, None, false, None);
    let api = Transport::new(&player);
    for _ in 0..5 {
        player.checkpoint(&guest);
    }
    let jump = api.action(api::SEEK, json!({"tick":55, "source":"boot"}));
    assert!(jump.seeking);
    assert_eq!(jump.seek.unwrap().from_tick, 5);
    let launch = film.restart(&player);
    let player = film.open(
        launch.target,
        launch.from_tick,
        launch.paused,
        launch.source,
    );
    let api = Transport::new(&player);
    player.checkpoint(&guest);
    player.checkpoint(&guest);
    let replacement = api.action(api::SEEK, json!({"tick":44}));
    assert_eq!(replacement.seek.unwrap().from_tick, 5);
    let cancelled = api.action(api::SEEK, json!({"cancel":true}));
    assert_eq!(cancelled.seek.unwrap().target, 5);
    assert!(cancelled.paused);

    let launch = film.restart(&player);
    let returning = film.open(
        launch.target,
        launch.from_tick,
        launch.paused,
        launch.source,
    );
    let api = Transport::new(&returning);
    assert!(!returning.lock().holding());
    for _ in 0..5 {
        returning.checkpoint(&guest);
    }
    let state = api.state();
    assert_eq!(state.tick, 5);
    assert!(!state.seeking);
    assert_eq!(state.seek.unwrap().remaining_ticks, 0);
    assert!(returning.lock().holding());
    let idle = api.action(api::SEEK, json!({"cancel":true}));
    assert!(!idle.seeking);
    assert!(returning.lock().restart.is_none());
}

#[test]
fn five_taps_schedule_one_restart_at_the_combined_target() {
    let film = Film::new();
    let player = film.open(0, None, true, None);
    let api = Transport::new(&player);
    let start = Instant::now();
    let mut taps = SeekTaps::default();
    for i in 0..5 {
        let at = start + Duration::from_millis(i * 40);
        taps.push(0, 5, (0, 60), at);
        assert!(taps.take_due(at).is_none());
        assert!(player.lock().restart.is_none());
    }
    let targets: Vec<_> = [409, 410, 411, 800]
        .into_iter()
        .filter_map(|ms| taps.take_due(start + Duration::from_millis(ms)))
        .collect();
    assert_eq!(targets, [25]);
    for tick in targets {
        api.action(api::SEEK, json!({"tick":tick}));
    }
    let restart = film.restart(&player);
    assert_eq!(restart.target, 25);
    assert_eq!(restart.from_tick, Some(0));
}

#[test]
fn step_advances_one_verified_tick_and_holds() {
    let film = Film::new();
    let player = film.open(0, None, false, None);
    let api = Transport::new(&player);
    assert!(api.call(api::STEP, json!({"ticks":1}))["error"].is_object());
    api.action(api::PAUSE, json!({}));
    for ticks in [0, 61, u64::MAX] {
        assert!(api.call(api::STEP, json!({"ticks":ticks}))["error"].is_object());
    }
    let queued = api.action(api::STEP, json!({"ticks":1}));
    assert_eq!(queued.tick, 0);
    assert_eq!(queued.speed_reached, None);
    assert!(!player.lock().holding());
    player.checkpoint(&arena());
    let held = api.state();
    assert_eq!(held.tick, 1);
    assert!(held.paused);
    assert_eq!(held.speed_reached, Some(0.0));
    let state = player.lock();
    assert!(state.holding());
    assert!(state.failure.is_none());
    assert!(state.restart.is_none());
}

#[test]
fn boot_seek_reports_falling_remaining_ticks() {
    let film = Film::new();
    let player = film.open(4, Some(17), true, Some(api::SeekRoute::Boot));
    let api = Transport::new(&player);
    let guest = arena();
    assert!(player.take_keyframe().is_none());
    for remaining in (0..=4).rev() {
        let state = api.state();
        let seek = state.seek.unwrap();
        assert_eq!(seek.source, api::SeekSource::Boot);
        assert_eq!(seek.from_tick, 17);
        assert_eq!(seek.remaining_ticks, remaining);
        assert_eq!(seek.restore_ms, None);
        assert_eq!(state.seeking, remaining != 0);
        if remaining != 0 {
            player.checkpoint(&guest);
        }
    }
    assert!(player.lock().holding());
    assert!(api.call(api::SEEK, json!({"tick":10,"source":"keyframe"}))["error"].is_object());
}
