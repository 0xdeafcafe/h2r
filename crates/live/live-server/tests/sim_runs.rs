//! The private training API and the same ingest and stream sessions used
//! by axum on a machine and LiveHub on the Worker.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::{Harness, Resp, User, start, start_driven};
use live_proto::game::{self, Frame, FramePlayer};
use live_proto::{ClientMessage, Gamertag, lxm, pack};
use live_server::Policy;
use live_server::blobs::{BlobStore, Blobs};
use live_server::ingest::IngestSession;
use live_server::store::MemoryStore;
use live_server::stream::{StreamFilter, StreamSession};
use serde_json::{Value, json};

const RUNS: &str = "/v1/admin/sim-runs";
const ID: &str = "sim-a-run";

fn policy() -> Policy {
    Policy {
        admins: ["Boss", "Other Admin"]
            .map(|tag| Gamertag::new(tag).unwrap())
            .to_vec(),
        ..Policy::default()
    }
}

fn bots() -> Value {
    json!([{ "name": "Bolt", "arm": "aggressive", "tuning": { "reach": 4 } }])
}

fn game(deferred: bool) -> Value {
    json!({
        "map": "lockout", "mode": "multiplayer", "variant": "Slayer", "team_play": false,
        "tick": 0, "players": [{ "index": 0, "name": "Bolt", "team": 0, "bot": true }],
        "sim": { "run_id": "a-run", "batch_id": "a-batch", "seed": "18446744073709551615",
            "gametype": "slayer", "pace": "unpaced", "bots": bots(), "deferred": deferred },
    })
}

fn auth(h: &Harness, user: &User, method: &str) -> String {
    serde_json::to_string(&ClientMessage::Auth {
        token: h.token(&user.key, &user.author, method),
    })
    .unwrap()
}

async fn start_game(h: &Harness, user: &User, deferred: bool) -> Resp {
    h.call(
        Method::POST,
        RUNS,
        &[h.bearer(user, lxm::GAMES_WRITE)],
        game(deferred).to_string(),
    )
    .await
}

async fn read(h: &Harness, user: &User, path: &str) -> Resp {
    h.call(
        Method::GET,
        path,
        &[h.bearer(user, lxm::READ)],
        Body::empty(),
    )
    .await
}

async fn upload(h: &Harness, user: &User, body: &Value) -> Resp {
    h.call(
        Method::PUT,
        &format!("{RUNS}/{ID}/report"),
        &[h.bearer(user, lxm::GAMES_WRITE)],
        body.to_string(),
    )
    .await
}

async fn end_game(h: &Harness, user: &User) {
    let end = json!({"tick": 6, "reason": "finished", "players": [
        {"index": 0, "score": 2, "kills": 2, "deaths": 0, "assists": 0},
    ]});
    let jws = h.batch(user, ID, &[(game::GAME_ENDED, end)]);
    let r = h.post_batch(user, ID, jws).await;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
}

#[tokio::test]
async fn sim_reports_are_admin_only_and_survive_in_the_local_blob_store() {
    let h = start(Arc::new(MemoryStore::new()), policy()).await;
    let blobs = Blobs::dir(&h.dir).unwrap();
    h.live.set_blobs(blobs.clone());
    let boss = h.create("Boss").await;
    let other = h.create("Other Admin").await;
    let user = h.create("Player").await;

    assert_eq!(h.get(RUNS).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(read(&h, &user, RUNS).await.status, StatusCode::FORBIDDEN);
    assert_eq!(
        start_game(&h, &user, false).await.status,
        StatusCode::FORBIDDEN
    );
    let started = start_game(&h, &boss, false).await;
    assert_eq!(started.status, StatusCode::CREATED, "{:?}", started.body);
    assert_eq!(started.json()["game_id"], ID);
    assert_eq!(start_game(&h, &boss, false).await.json(), started.json());

    let running = read(
        &h,
        &boss,
        &format!("{RUNS}?map=LOCKOUT&tuning=AGGRESSIVE&batch=a-batch"),
    )
    .await;
    assert_eq!(running.json()["items"][0]["status"], "running");
    assert_eq!(running.json()["items"][0]["sim"]["bots"], bots());
    for query in ["map=ascension", "tuning=conservative", "batch=another"] {
        assert_eq!(
            read(&h, &boss, &format!("{RUNS}?{query}")).await.json()["items"],
            json!([])
        );
    }

    let listed = read(
        &h,
        &boss,
        &format!("{RUNS}?include_options=1&tuning_choice=arm:aggressive"),
    )
    .await;
    assert_eq!(listed.status, StatusCode::OK);
    assert_eq!(listed.json()["items"][0]["game_id"], ID);
    let filtered = read(
        &h,
        &boss,
        &format!("{RUNS}?include_options=1&tuning_choice=arm:aggress"),
    )
    .await;
    assert_eq!(filtered.status, StatusCode::OK);
    assert_eq!(filtered.json()["items"], json!([]));
    assert_eq!(
        filtered.json()["options"],
        json!({
            "maps": ["lockout"],
            "batches": ["a-batch"],
            "arms": ["aggressive"],
            "custom_tuning": false,
            "default_tuning": false,
        })
    );

    let report = json!({
        "format": "h2-sim-report/1",
        "match": { "map": "lockout", "seed": u64::MAX, "game_type": "slayer", "bot": bots() },
        "result": { "end": "tick_limit", "winner": { "player": 0 } },
        "determinism": { "passed": true },
        "ticks": 6, "wall": { "speedup": 2.8 },
        "players": [{ "name": "Bolt", "index": 0, "bot": true, "accuracy": 0.75 }],
        "notes": ["x".repeat(70 * 1024)],
    });
    assert_eq!(
        h.call(
            Method::PUT,
            &format!("{RUNS}/{ID}/report"),
            &[],
            report.to_string()
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        upload(&h, &user, &report).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        upload(&h, &other, &report).await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        upload(&h, &boss, &report).await.status,
        StatusCode::CONFLICT
    );
    end_game(&h, &boss).await;

    let mut wrong = report.clone();
    wrong["match"]["seed"] = json!(0);
    assert_eq!(
        upload(&h, &boss, &wrong).await.status,
        StatusCode::BAD_REQUEST
    );
    wrong = report.clone();
    wrong["notes"] = json!(["x".repeat(512 * 1024)]);
    assert_eq!(
        upload(&h, &boss, &wrong).await.status,
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        upload(&h, &boss, &report).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        upload(&h, &boss, &report).await.status,
        StatusCode::NO_CONTENT
    );
    wrong = report.clone();
    wrong["ticks"] = json!(5);
    assert_eq!(upload(&h, &boss, &wrong).await.status, StatusCode::CONFLICT);
    assert_eq!(
        blobs
            .private
            .list(&format!("sim/{ID}/"))
            .await
            .unwrap()
            .len(),
        1
    );

    let path = format!("{RUNS}/{ID}");
    assert_eq!(h.get(&path).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(read(&h, &user, &path).await.status, StatusCode::FORBIDDEN);
    let detail = read(&h, &boss, &path).await;
    assert_eq!(detail.headers["cache-control"], "private, no-store");
    let detail = detail.json();
    assert_eq!(detail["run"]["status"], "finished");
    assert_eq!(detail["run"]["speedup"], 2.8);
    assert_eq!(detail["run"]["determinism"]["passed"], true);
    assert_eq!(detail["game"]["scoreboard"][0]["kills"], 2);
    assert_eq!(detail["report"], report);

    assert_eq!(h.get("/v1/games").await.json()["items"], json!([]));
    assert_eq!(
        read(&h, &boss, "/v1/games").await.json()["items"],
        json!([])
    );
    assert_eq!(
        h.get("/v1/stats/summary?window=today").await.json()["totals"]["games"],
        0
    );
    assert_eq!(
        read(&h, &boss, "/v1/stats/summary?window=today")
            .await
            .json()["totals"]["games"],
        0
    );
    assert!(!h.get("/v1/snapshot").await.json().to_string().contains(ID));
    assert!(
        !read(&h, &boss, "/v1/snapshot")
            .await
            .json()
            .to_string()
            .contains(ID)
    );
}

async fn stream_test(deferred: bool) {
    // Driven timers match LiveHub. No native socket code is involved.
    let h = start_driven(Arc::new(MemoryStore::new()), policy()).await;
    h.live.set_blobs(Blobs {
        private: BlobStore::memory(),
        public: None,
    });
    let admin = h.create("Boss").await;
    let gamer = h.create("Player").await;
    let start = start_game(&h, &admin, deferred).await;
    assert_eq!(start.status, StatusCode::CREATED);
    let position = start.json()["position"].as_u64().unwrap();

    let mut reader = StreamSession::new(
        h.live.clone(),
        StreamFilter {
            from: Some(position),
            games: vec![ID.into()],
            stream: Some(game::stream(ID)),
            auth: true,
            ..Default::default()
        },
    );
    assert!(reader.on_text(&auth(&h, &admin, lxm::READ)).unwrap());
    assert_eq!(
        reader
            .replay_page(reader.head(), true)
            .await
            .unwrap()
            .0
            .len(),
        1
    );
    let mut public = StreamSession::new(h.live.clone(), reader.filter().clone());
    assert!(public.on_text(&auth(&h, &gamer, lxm::READ)).unwrap());
    public.replay_page(public.head(), true).await.unwrap();

    let mut frames = h.live.frames();
    let mut ingest = IngestSession::new(h.live.clone());
    assert!(
        ingest
            .on_text(&auth(&h, &admin, lxm::GAMES_WRITE))
            .await
            .is_empty()
    );
    let mut encoder = pack::Encoder::new(0, 150);
    let mut body = Vec::new();
    let stopped = game::MachinePosition { i: 7, p: 0 };
    let moving = game::MachinePosition { i: 9, p: 192 };
    for tick in [2, 4, 6] {
        if tick == 4 {
            encoder.force_keyframe();
        }
        let frame = Frame {
            game_id: ID.into(),
            tick,
            machine_positions: match tick {
                2 => vec![stopped],
                6 => vec![moving],
                _ => Vec::new(),
            },
            players: vec![FramePlayer {
                index: 0,
                alive: true,
                pos: [1.0, 2.0, 3.0],
                yaw: 0.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let (record, _) = encoder.encode(&frame).unwrap();
        pack::push_record(&mut body, &record);
    }
    assert!(ingest.on_binary(&body).await.is_empty());
    if deferred {
        assert!(frames.try_recv().is_err());
    } else {
        let frame = frames.try_recv().unwrap();
        assert!(reader.on_frame(&frame).is_some());
        assert!(public.on_frame(&frame).is_none());
        let mut unscoped = StreamSession::new(
            h.live.clone(),
            StreamFilter {
                auth: true,
                from: Some(position),
                ..Default::default()
            },
        );
        assert!(unscoped.on_text(&auth(&h, &admin, lxm::READ)).unwrap());
        unscoped.replay_page(unscoped.head(), true).await.unwrap();
        assert!(unscoped.on_frame(&frame).is_none());
    }
    let running = read(&h, &admin, RUNS).await.json();
    assert_eq!(running["items"][0]["status"], "running");
    assert_eq!(running["items"][0]["sim"]["deferred"], deferred);
    assert_eq!(h.get("/v1/server").await.json()["counts"]["live_games"], 0);

    end_game(&h, &admin).await;
    let archive = h.live.chunk_frames(ID).await.unwrap();
    assert_eq!(
        archive.iter().map(|f| f.tick).collect::<Vec<_>>(),
        [2, 4, 6]
    );
    assert_eq!(archive[0].machine_positions, [stopped]);
    assert!(archive[1].machine_positions.is_empty());
    assert_eq!(archive[2].machine_positions, [moving]);
    // The recording consumes the empty replacement once, even while live
    // fan-out is throttled or disabled for deferred uploads.
    let chunk = read(&h, &admin, &format!("/v1/games/{ID}/chunks/2")).await;
    let raw = live_proto::chunk::container(&chunk.body).unwrap();
    let mut decoder = pack::Decoder::new();
    let keys: Vec<_> = pack::records(&raw)
        .map(|r| decoder.decode(r.unwrap()).unwrap().machine_baseline)
        .collect();
    assert_eq!(keys, [true, true, false]);
    let finished = read(&h, &admin, RUNS).await.json();
    assert_eq!(finished["items"][0]["status"], "finished");
    for suffix in ["", "/chunks", "/chunks/2", "/frames"] {
        let path = format!("/v1/games/{ID}{suffix}");
        assert_eq!(h.get(&path).await.status, StatusCode::NOT_FOUND);
        assert_eq!(read(&h, &gamer, &path).await.status, StatusCode::NOT_FOUND);
        assert_eq!(read(&h, &admin, &path).await.status, StatusCode::OK);
    }
    let stream = format!("/v1/events?stream=game-{ID}");
    assert_eq!(h.get(&stream).await.json()["items"], json!([]));
    assert_eq!(read(&h, &gamer, &stream).await.json()["items"], json!([]));
    assert!(
        !read(&h, &admin, &stream).await.json()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn sim_streams_to_admin_and_archives_faster_than_the_wall_clock() {
    stream_test(false).await;
}

#[tokio::test]
async fn sim_deferred_ingest_archives_without_broadcasting() {
    stream_test(true).await;
}

#[tokio::test]
async fn private_live_combat_keeps_the_newest_sample_per_bot() {
    use live_proto::game::{SimCombatCounters, SimCombatSample};
    use live_server::stream::Out;

    let h = start_driven(Arc::new(MemoryStore::new()), policy()).await;
    let boss = h.create("Boss").await;
    let other = h.create("Other Admin").await;
    let player = h.create("Player").await;
    let mut body = game(false);
    body["players"].as_array_mut().unwrap().push(json!({
        "index": 3, "name": "Nut", "team": 0, "bot": true
    }));
    body["sim"]["bots"].as_array_mut().unwrap().push(json!({"name": "Nut"}));
    let start = h.call(Method::POST, RUNS, &[h.bearer(&boss, lxm::GAMES_WRITE)], body.to_string()).await;
    assert_eq!(start.status, StatusCode::CREATED);
    let sample = |index, tick, melees| {
        serde_json::to_string(&ClientMessage::SimCombat {
            game_id: ID.into(),
            sample: Box::new(SimCombatSample {
                index,
                tick,
                combat: SimCombatCounters { melee_presses: melees, ..Default::default() },
                ..Default::default()
            }),
        }).unwrap()
    };
    let refused = |reply: &[Out], code: &str| {
        let [Out::Text(text)] = reply else { panic!("expected one refusal") };
        let reply: Value = serde_json::from_str(text.as_ref()).unwrap();
        assert_eq!(reply["error"], code);
    };
    let mut ingest = IngestSession::new(h.live.clone());
    assert!(matches!(ingest.on_text(&sample(0, 1, 0)).await.as_slice(), [Out::Close(_, _)]));
    assert!(ingest.on_text(&auth(&h, &other, lxm::GAMES_WRITE)).await.is_empty());
    refused(&ingest.on_text(&sample(0, 10, 1)).await, "forbidden");

    let mut ingest = IngestSession::new(h.live.clone());
    assert!(ingest.on_text(&auth(&h, &boss, lxm::GAMES_WRITE)).await.is_empty());
    let mut deferred = game(true);
    deferred["sim"]["run_id"] = json!("deferred-sample");
    let deferred = h.call(Method::POST, RUNS, &[h.bearer(&boss, lxm::GAMES_WRITE)],
        deferred.to_string()).await;
    assert_eq!(deferred.status, StatusCode::CREATED);
    let mut ordinary = game(false);
    ordinary.as_object_mut().unwrap().remove("sim");
    let ordinary = h.call(Method::POST, "/v1/games", &[h.bearer(&boss, lxm::GAMES_WRITE)],
        ordinary.to_string()).await;
    assert_eq!(ordinary.status, StatusCode::CREATED);
    for game in [deferred.json(), ordinary.json()] {
        let mut message: Value = serde_json::from_str(&sample(0, 10, 1)).unwrap();
        message["game_id"] = game["game_id"].clone();
        refused(&ingest.on_text(&message.to_string()).await, "invalid");
    }
    refused(&ingest.on_text(&sample(2, 10, 1)).await, "invalid");
    refused(&ingest.on_text(&sample(0, u32::MAX, 1)).await, "invalid");
    let mut malformed: Value = serde_json::from_str(&sample(0, 12, 0)).unwrap();
    malformed["sample"]["combat"]["range"] = json!(-1);
    refused(&ingest.on_text(&malformed.to_string()).await, "invalid");
    let mut frames = h.live.frames();
    for (index, tick, melee) in [(3, 11, 3), (0, 10, 1), (0, 11, 2), (0, 10, 9), (0, 11, 8)] {
        assert!(ingest.on_text(&sample(index, tick, melee)).await.is_empty());
    }
    assert!(frames.try_recv().is_err());
    let detail = format!("{RUNS}/{ID}");
    assert_eq!(h.get(&detail).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(read(&h, &player, &detail).await.status, StatusCode::FORBIDDEN);
    let rows = read(&h, &other, &detail).await.json()["live_combat"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["index"], 0);
    assert_eq!(rows[0]["tick"], 11);
    assert_eq!(rows[0]["combat"]["melee_presses"], 2);
    assert_eq!(rows[1]["index"], 3);
    let summary = read(&h, &boss, &format!("/v1/games/{ID}")).await.json();
    assert!(summary.get("live_combat").is_none());
    end_game(&h, &boss).await;
    refused(&ingest.on_text(&sample(0, 12, 9)).await, "conflict");
    assert_eq!(read(&h, &boss, &detail).await.json()["live_combat"], json!([]));
}
