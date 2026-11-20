//! The Live sink against the real embedded server, through a proxy that
//! cuts the ingest socket mid-stream, refuses connections, or swallows the
//! server's answers (docs/live/live-telemetry.md, "Reconnect").

use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use live_proto::game::{self, FramePlayer, GameEnded, PlayerScore, ScoreChanged};
use live_proto::sign::{self, AccountRequest, KeyPair};
use live_proto::{CreatedAccount, Discovery, Page, WireEvent, lxm};

use super::{MAX_AUTH_REFUSALS, ServerSink, Timing};
use crate::config::TelemetryAccount;
use crate::event::{Appearance, GameEvent, GameInfo, Output, PlayerState, PlayerStats};
use crate::session::Session;
use crate::sink::Sink;

#[derive(Default)]
struct ProxyState {
    /// New connections are closed at once.
    down: AtomicBool,
    /// The server's bytes to the client are dropped.
    mute: AtomicBool,
    open: Mutex<Vec<TcpStream>>,
}

/// A TCP proxy in front of the embedded server.
struct Proxy {
    url: String,
    state: Arc<ProxyState>,
}

impl Proxy {
    fn new(upstream: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(ProxyState::default());
        let shared = Arc::clone(&state);
        let upstream = upstream.to_owned();
        std::thread::spawn(move || {
            for client in listener.incoming() {
                let Ok(client) = client else { continue };
                if shared.down.load(Ordering::SeqCst) {
                    drop(client);
                    continue;
                }
                let Ok(server) = TcpStream::connect(&upstream) else {
                    continue;
                };
                shared
                    .open
                    .lock()
                    .unwrap()
                    .extend([client.try_clone().unwrap(), server.try_clone().unwrap()]);
                pump(
                    client.try_clone().unwrap(),
                    server.try_clone().unwrap(),
                    None,
                );
                pump(server, client, Some(Arc::clone(&shared)));
            }
        });
        Proxy { url, state }
    }

    /// Cuts every connection through it, as a redeploy does.
    fn cut(&self) {
        for stream in self.state.open.lock().unwrap().drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }

    fn set_down(&self, down: bool) {
        self.state.down.store(down, Ordering::SeqCst);
    }

    fn set_mute(&self, mute: bool) {
        self.state.mute.store(mute, Ordering::SeqCst);
    }
}

fn pump(mut from: TcpStream, mut to: TcpStream, muted: Option<Arc<ProxyState>>) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 16 * 1024];
        loop {
            match from.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if muted
                        .as_ref()
                        .is_some_and(|s| s.mute.load(Ordering::SeqCst))
                    {
                        continue;
                    }
                    if to.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = to.shutdown(Shutdown::Both);
        let _ = from.shutdown(Shutdown::Both);
    });
}

struct Live {
    embedded: live_server::Embedded,
    dir: PathBuf,
    agent: ureq::Agent,
    discovery: Discovery,
    operator: KeyPair,
}

impl Live {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "h2-telemetry-live-{}-{}",
            std::process::id(),
            sign::random_id()
        ));
        let embedded = live_server::start_embedded(
            live_server::MEMORY_DB,
            "127.0.0.1:0".parse().unwrap(),
            &dir,
            live_server::http::Site::default(),
        )
        .unwrap();
        let operator = live_server::ServerIdentity::load(&dir).unwrap().key;
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(3))
            .build();
        let discovery = agent
            .get(&format!("{}/.well-known/h2live.json", embedded.url))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        Live {
            embedded,
            dir,
            agent,
            discovery,
            operator,
        }
    }

    fn addr(&self) -> &str {
        self.embedded.url.strip_prefix("http://").unwrap()
    }

    fn account(&self, tag: &str) -> TelemetryAccount {
        let key = KeyPair::generate();
        let body = AccountRequest::sign(
            &key,
            tag,
            None,
            &self.discovery.id,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        );
        let created: CreatedAccount = self
            .agent
            .post(&format!("{}/v1/accounts", self.embedded.url))
            .set("content-type", "application/jose")
            .send_string(&body)
            .unwrap()
            .into_json()
            .unwrap();
        TelemetryAccount {
            xuid: created.xuid.to_string(),
            gamertag: created.gamertag.to_string(),
            key: key.private_jwk(),
        }
    }

    /// The kinds of the game's starts, scores and ends, in order, read as
    /// the operator.
    fn kinds(&self, server_id: &str) -> Vec<String> {
        let token = sign::issue_token(
            &self.operator,
            &format!("server:{}", self.discovery.id),
            &self.discovery.id,
            lxm::READ,
            time::OffsetDateTime::now_utc().unix_timestamp(),
        );
        let page: Page<WireEvent> = self
            .agent
            .get(&format!(
                "{}/v1/events?stream={}&limit=1000",
                self.embedded.url,
                game::stream(server_id)
            ))
            .set("authorization", &format!("Bearer {token}"))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        page.items
            .into_iter()
            .map(|e| e.kind)
            .filter(|k| {
                [game::GAME_STARTED, game::SCORE_CHANGED, game::GAME_ENDED].contains(&k.as_str())
            })
            .collect()
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn fast() -> Timing {
    Timing {
        backoff_min: Duration::from_millis(20),
        backoff_max: Duration::from_millis(200),
        ack_timeout: Duration::from_millis(500),
        settle: Duration::from_millis(100),
        steady: Duration::from_secs(30),
    }
}

/// Starts a one-player game on `sink`; its local id.
fn start(sink: &mut ServerSink) -> String {
    start_session(sink, Session::new("test", None))
}

fn start_session(sink: &mut ServerSink, mut session: Session) -> String {
    session.start(
        &GameInfo {
            map: "lockout".into(),
            mode: "multiplayer".into(),
            difficulty: None,
            skulls: None,
            variant: Some("Slayer".into()),
            team_play: false,
            score_limit: Some(25),
            time_limit_s: Some(600),
            tick_rate: 30,
            tick: 2,
        },
        vec![PlayerState {
            datum: 0x80010000,
            player: game::Player {
                index: 0,
                name: "A".into(),
                xuid: None,
                team: 0,
                local: Some(true),
                appearance: None,
                bot: None,
            },
            appearance: Appearance::default(),
            local_user: None,
            controller: None,
            unit: u32::MAX,
            stats: PlayerStats::ZERO,
            machine: [0; 6],
        }],
    );
    let mut local = None;
    for output in session.take_output() {
        match output {
            Output::Event(event) => {
                if event.kind == game::GAME_STARTED {
                    local = Some(event.game_id.clone());
                }
                sink.event(event).unwrap();
            }
            Output::Frame(frame) => sink.frame(frame).unwrap(),
        }
    }
    local.expect("a GameStarted")
}

fn frame(local: &str, tick: u64) -> game::Frame {
    game::Frame {
        game_id: local.into(),
        tick,
        players: vec![FramePlayer {
            index: 0,
            alive: true,
            pos: [1.0, 2.0, tick as f64 * 0.01],
            yaw: 0.0,
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn score(local: &str, tick: u64, kills: u32) -> GameEvent {
    GameEvent::new(
        local,
        game::SCORE_CHANGED,
        ScoreChanged {
            tick,
            players: vec![PlayerScore {
                index: 0,
                score: kills as i64,
                kills,
                deaths: 0,
                assists: 0,
            }],
            teams: None,
        },
    )
}

fn ended(local: &str, tick: u64) -> GameEvent {
    GameEvent::new(
        local,
        game::GAME_ENDED,
        GameEnded {
            tick,
            reason: "finished".into(),
            winner: None,
            players: vec![],
        },
    )
}

/// Frames and flushes, as the worker does, until the outbox is empty.
fn drive(sink: &mut ServerSink, local: &str, mut tick: u64) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while sink.waiting_events() > 0 {
        assert!(Instant::now() < deadline, "the outbox never drained");
        sink.frame(frame(local, tick)).unwrap();
        sink.flush().unwrap();
        tick += 2;
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn count(kinds: &[String], kind: &str) -> usize {
    kinds.iter().filter(|k| *k == kind).count()
}

#[test]
fn sim_sink_finishes_live_and_deferred_packed_recordings() {
    let live = Live::new();
    let account = live.account("Trainer");
    live.embedded.live.grant_admin_runtime(
        live_proto::Xuid::parse(&account.xuid).unwrap(),
        &account.gamertag,
    );
    let bots = vec![serde_json::json!({"name": "A", "tuning": {"reach": 4}})];
    for deferred in [false, true] {
        let metadata = game::SimGame {
            run_id: sign::random_id(),
            batch_id: None,
            seed: "42".into(),
            gametype: "slayer".into(),
            pace: "unpaced".into(),
            bots: bots.clone(),
            deferred,
        };
        let samples = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = samples.clone();
        let feed = crate::hooks::CombatFeed::default();
        assert!(feed.set(Box::new(move || {
            seen.fetch_add(1, Ordering::Relaxed);
            vec![game::SimCombatSample { index: 0, tick: 4, ..Default::default() }]
        })).is_ok());
        let mut sink = ServerSink::new(live.embedded.url.clone(), account.clone())
            .unwrap()
            .with_sim(&metadata, deferred.then(|| live.dir.join("deferred.lock")), feed);
        let id = start_session(&mut sink, Session::new("test", None).with_sim(metadata));
        let mut live_frames = live.embedded.live.frames();
        let stopped = game::MachinePosition { i: 7, p: 0 };
        for tick in [4, 6, 8] {
            let mut f = frame(&id, tick);
            if tick == 4 {
                f.cut = Some(2);
                f.machine_positions.push(stopped);
            }
            if tick == 6 && deferred {
                let capture = sink.sim.as_mut().unwrap().deferred.as_mut().unwrap();
                capture.encoder.force_keyframe();
            }
            sink.frame(f).unwrap();
            if tick == 6 && deferred {
                // This sampled frame was quiet. A fresh reader of its queued
                // keyframe must recover the previous explicit zero.
                let capture = sink.sim.as_ref().unwrap().deferred.as_ref().unwrap();
                let Some(super::DeferredItem::Frame(tungstenite::Message::Binary(bytes))) =
                    capture.items.last()
                else {
                    panic!("expected a queued binary frame");
                };
                let record = live_proto::pack::records(bytes).next().unwrap().unwrap();
                let d = live_proto::pack::Decoder::new().decode(record).unwrap();
                assert!(d.machine_baseline);
                assert_eq!(d.frame.machine_positions, [stopped]);
                assert_eq!(d.frame.cut, None);
            }
        }
        assert_eq!(samples.load(Ordering::Relaxed), usize::from(!deferred));
        if deferred {
            assert!(live_frames.try_recv().is_err());
        }
        sink.event(ended(&id, 8)).unwrap();
        sink.finish_sim(serde_json::json!({
            "format": "h2-sim-report/1",
            "match": {"map": "lockout", "game_type": "slayer", "seed": 42, "bot": bots},
            "result": {"end": "tick_limit"}, "ticks": 6, "wall": {"speedup": 2.8}, "players": [],
        }))
        .unwrap();
        assert_eq!(samples.load(Ordering::Relaxed), usize::from(!deferred));
        if deferred {
            assert!(live_frames.try_recv().is_err());
        } else {
            assert_eq!(live_frames.try_recv().unwrap().frame.tick, 4);
        }
        assert_eq!(live.kinds(&id), [game::GAME_STARTED, game::GAME_ENDED]);
        let read_token = || {
            sign::issue_token(
                &live.operator,
                &format!("server:{}", live.discovery.id),
                &live.discovery.id,
                lxm::READ,
                time::OffsetDateTime::now_utc().unix_timestamp(),
            )
        };
        let list: serde_json::Value = live
            .agent
            .get(&format!(
                "{}/v1/admin/sim-runs?include_options=1&tuning_choice=custom",
                live.embedded.url,
            ))
            .set("authorization", &format!("Bearer {}", read_token()))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(list["items"][0]["game_id"].as_str(), Some(id.as_str()));
        assert_eq!(list["items"][0]["sim"]["seed"], "42");
        assert_eq!(list["items"][0]["has_report"], true);
        assert_eq!(list["options"]["custom_tuning"], true);
        let detail: serde_json::Value = live
            .agent
            .get(&format!("{}/v1/admin/sim-runs/{id}", live.embedded.url))
            .set("authorization", &format!("Bearer {}", read_token()))
            .call()
            .unwrap()
            .into_json()
            .unwrap();
        assert_eq!(detail["run"]["status"], "finished");
        assert_eq!(detail["report"]["ticks"], 6);
        assert_eq!(detail["game"]["players"][0]["bot"], true);
    }
}

#[test]
fn a_cut_socket_reconnects_and_the_events_made_while_down_land_once() {
    let live = Live::new();
    let proxy = Proxy::new(live.addr());
    let mut sink = ServerSink::new(proxy.url.clone(), live.account("Alpha"))
        .unwrap()
        .with_timing(fast());
    let local = start(&mut sink);
    let server_id = sink.games[&local].clone();
    for t in 1..5 {
        sink.frame(frame(&local, t * 2)).unwrap();
    }
    sink.event(score(&local, 10, 1)).unwrap();
    sink.flush().unwrap();
    assert_eq!(sink.waiting_events(), 0);

    // The server goes away (a redeploy) and stays away a while.
    proxy.cut();
    proxy.set_down(true);
    for t in 6..12 {
        sink.frame(frame(&local, t * 2)).unwrap();
    }
    sink.event(score(&local, 24, 2)).unwrap();
    sink.event(ended(&local, 26)).unwrap();
    sink.flush().unwrap();
    // Nothing failed: the events wait, the connection is gone.
    assert!(sink.connection.is_none());
    assert_eq!(sink.waiting_events(), 2);
    assert!(
        sink.dropped_frames > 0,
        "frames made while down are dropped"
    );

    proxy.set_down(false);
    drive(&mut sink, &local, 28);
    let kinds = live.kinds(&server_id);
    assert_eq!(
        kinds,
        [
            game::GAME_STARTED,
            game::SCORE_CHANGED,
            game::SCORE_CHANGED,
            game::GAME_ENDED
        ],
        "every event once, in order"
    );
    assert_eq!(sink.dropped_events, 0);
}

#[test]
fn a_batch_sent_but_unanswered_is_sent_again_and_lands_once() {
    let live = Live::new();
    let proxy = Proxy::new(live.addr());
    let mut sink = ServerSink::new(proxy.url.clone(), live.account("Alpha"))
        .unwrap()
        .with_timing(fast());
    let local = start(&mut sink);
    let server_id = sink.games[&local].clone();
    sink.frame(frame(&local, 4)).unwrap();

    // The server takes the batch, but its answer is lost.
    proxy.set_mute(true);
    sink.event(score(&local, 10, 1)).unwrap();
    sink.event(ended(&local, 12)).unwrap();
    sink.flush().unwrap();
    assert!(
        sink.connection.is_none(),
        "no answer: the connection is lost"
    );
    assert_eq!(sink.waiting_events(), 2, "the batch and GameEnded wait");
    let deadline = Instant::now() + Duration::from_secs(5);
    while count(&live.kinds(&server_id), game::SCORE_CHANGED) == 0 {
        assert!(Instant::now() < deadline, "the server took the batch");
        std::thread::sleep(Duration::from_millis(20));
    }

    // Back: the same signed batch goes again, the server says it has it
    // ("nonce already used"), and the sink counts it delivered.
    proxy.set_mute(false);
    proxy.cut();
    drive(&mut sink, &local, 14);
    let kinds = live.kinds(&server_id);
    assert_eq!(count(&kinds, game::SCORE_CHANGED), 1, "{kinds:?}");
    assert_eq!(count(&kinds, game::GAME_ENDED), 1, "{kinds:?}");
    assert_eq!(sink.dropped_events, 0);
}

#[test]
fn a_game_started_while_down_is_created_when_the_server_is_back() {
    let live = Live::new();
    let proxy = Proxy::new(live.addr());
    proxy.set_down(true);
    let mut sink = ServerSink::new(proxy.url.clone(), live.account("Alpha"))
        .unwrap()
        .with_timing(fast());
    let local = start(&mut sink);
    sink.event(score(&local, 10, 1)).unwrap();
    sink.event(ended(&local, 12)).unwrap();
    assert!(sink.games.is_empty());
    assert_eq!(sink.waiting_events(), 3);

    proxy.set_down(false);
    drive(&mut sink, &local, 14);
    let server_id = sink.games[&local].clone();
    assert_eq!(
        live.kinds(&server_id),
        [game::GAME_STARTED, game::SCORE_CHANGED, game::GAME_ENDED]
    );
}

#[test]
fn an_account_the_server_refuses_stops_the_sink_after_a_few_tries() {
    let live = Live::new();
    let unknown = TelemetryAccount {
        xuid: "0009000000000001".into(),
        gamertag: "Nobody".into(),
        key: KeyPair::generate().private_jwk(),
    };
    let mut sink = ServerSink::new(live.embedded.url.clone(), unknown)
        .unwrap()
        .with_timing(fast());
    let local = start(&mut sink);
    let deadline = Instant::now() + Duration::from_secs(10);
    let error = loop {
        assert!(Instant::now() < deadline, "the sink never gave up");
        if let Err(error) = sink.flush() {
            break error;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(error.contains("refused the account"), "{error}");
    assert_eq!(sink.auth_refusals, MAX_AUTH_REFUSALS);
    // It stays stopped.
    assert!(sink.frame(frame(&local, 2)).is_err());
}

#[test]
fn large_sim_upload_retains_bot_stats_and_small_reports_keep_their_traces() {
    let stats = serde_json::json!([{"combat":{"melee_presses":3},
        "weapon_accuracy":[{"damage_code":8,"range":"close","shots":4,"hits":2,"accuracy":0.5}]}]);
    let small = serde_json::json!({"players":stats, "state_hashes_v1":[{"tick":5,"hash":"abc"}]});
    let decode = |r| serde_json::from_slice::<serde_json::Value>(&super::encode_sim_report(r).unwrap()).unwrap();
    assert_eq!(decode(small.clone()), small);
    let mut large = small.clone();
    large["state_components_v1"] = serde_json::json!(["x".repeat(game::MAX_SIM_REPORT_BYTES)]);
    let saved = decode(large);
    assert_eq!(saved["players"], stats);
    assert!(saved.get("state_hashes_v1").is_none());
    assert!(saved.get("state_components_v1").is_none());
    assert!(saved["notes"][0].as_str().unwrap().contains("determinism"));
}
