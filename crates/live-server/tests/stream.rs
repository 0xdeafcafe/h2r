//! `/v1/stream` over a real socket: replay, `caught_up`, live events,
//! filters, and `--private` auth as the first message.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use live_proto::sign::{AccountRequest, KeyPair, issue_token};
use live_proto::{ClientMessage, CreatedAccount, StreamMessage, lxm};
use live_server::store::MemoryStore;
use live_server::{Live, Policy, ServerIdentity, http};
use tokio_tungstenite::tungstenite::Message;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(policy: Policy) -> (Arc<Live>, SocketAddr) {
    let dir = std::env::temp_dir().join(format!(
        "h2live-stream-{}-{}",
        std::process::id(),
        live_proto::sign::random_id()
    ));
    let identity = ServerIdentity::open(&dir, Some("live.test")).unwrap();
    let live = Live::start(Arc::new(MemoryStore::new()), identity, policy)
        .await
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(axum::serve(listener, http::router(live.clone())).into_future());
    (live, addr)
}

async fn create(live: &Live, tag: &str) -> (KeyPair, CreatedAccount) {
    let key = KeyPair::generate();
    let created = live
        .create_account(&AccountRequest::sign(
            &key,
            tag,
            None,
            "live.test",
            live_server::app::now(),
        ))
        .await
        .unwrap();
    (key, created)
}

async fn connect(addr: SocketAddr, query: &str) -> Socket {
    tokio_tungstenite::connect_async(format!("ws://{addr}/v1/stream{query}"))
        .await
        .unwrap()
        .0
}

async fn next(ws: &mut Socket) -> StreamMessage {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("a message in time")
            .unwrap()
            .unwrap();
        match msg {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected {other:?}"),
        }
    }
}

fn event_type(m: &StreamMessage) -> (u64, String) {
    match m {
        StreamMessage::Event { event } => (event.position, event.kind.clone()),
        other => panic!("expected an event, got {other:?}"),
    }
}

#[tokio::test]
async fn replays_then_follows() {
    let (live, addr) = serve(Policy::default()).await;
    create(&live, "First").await;

    let mut ws = connect(addr, "?from=0").await;
    let replayed: Vec<_> = [
        next(&mut ws).await,
        next(&mut ws).await,
        next(&mut ws).await,
    ]
    .iter()
    .map(event_type)
    .collect();
    assert_eq!(
        replayed,
        [
            (1, "GamertagReserved".into()),
            (2, "AccountCreated".into()),
            (3, "KeyAdded".into())
        ]
    );
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 3 });

    create(&live, "Second").await;
    let live_events: Vec<_> = [
        next(&mut ws).await,
        next(&mut ws).await,
        next(&mut ws).await,
    ]
    .iter()
    .map(event_type)
    .collect();
    assert_eq!(
        live_events.iter().map(|e| e.0).collect::<Vec<_>>(),
        [4, 5, 6]
    );

    // `from` is exclusive; no `from` starts at the head.
    let mut ws = connect(addr, "?from=5").await;
    assert_eq!(event_type(&next(&mut ws).await).0, 6);
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 6 });
    let mut ws = connect(addr, "").await;
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 6 });
    create(&live, "Third").await;
    assert_eq!(event_type(&next(&mut ws).await).0, 7);
}

#[tokio::test]
async fn filters_types() {
    let (live, addr) = serve(Policy::default()).await;
    create(&live, "First").await;
    let mut ws = connect(addr, "?from=0&exclude=KeyAdded,GamertagReserved").await;
    assert_eq!(
        event_type(&next(&mut ws).await),
        (2, "AccountCreated".into())
    );
    // caught_up still reports the head, past the filtered-out events.
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 3 });
    let mut ws = connect(addr, "?from=0&types=KeyAdded").await;
    assert_eq!(event_type(&next(&mut ws).await), (3, "KeyAdded".into()));
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 3 });
    create(&live, "Second").await;
    assert_eq!(event_type(&next(&mut ws).await), (6, "KeyAdded".into()));
    // One stream only.
    let mut ws = connect(addr, "?from=0&stream=gamertag-second").await;
    assert_eq!(
        event_type(&next(&mut ws).await),
        (4, "GamertagReserved".into())
    );
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 6 });
}

async fn expect_close(ws: &mut Socket, code: u16) {
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match msg {
        Message::Close(Some(frame)) => assert_eq!(u16::from(frame.code), code),
        other => panic!("expected a close, got {other:?}"),
    }
}

#[tokio::test]
async fn private_streams_authenticate_first() {
    let (live, addr) = serve(Policy {
        private: true,
        ..Policy::default()
    })
    .await;
    let (key, account) = create(&live, "First").await;
    let auth = |method: &str| {
        let token = issue_token(
            &key,
            &account.id,
            "live.test",
            method,
            live_server::app::now(),
        );
        Message::Text(
            serde_json::to_string(&ClientMessage::Auth { token })
                .unwrap()
                .into(),
        )
    };

    let mut ws = connect(addr, "?from=0").await;
    ws.send(Message::Text("{\"kind\":\"hello\"}".into()))
        .await
        .unwrap();
    expect_close(&mut ws, 4401).await;

    let mut ws = connect(addr, "?from=0").await;
    ws.send(auth(lxm::EVENTS_WRITE)).await.unwrap();
    expect_close(&mut ws, 4401).await;

    let mut ws = connect(addr, "?from=0").await;
    ws.send(auth(lxm::READ)).await.unwrap();
    assert_eq!(event_type(&next(&mut ws).await).0, 1);
    for _ in 0..2 {
        next(&mut ws).await;
    }
    assert_eq!(next(&mut ws).await, StreamMessage::CaughtUp { position: 3 });
    // A fresh token is accepted mid-stream; a bad one closes it.
    ws.send(auth(lxm::READ)).await.unwrap();
    create(&live, "Second").await;
    assert_eq!(event_type(&next(&mut ws).await).0, 4);
    ws.send(Message::Text(
        "{\"kind\":\"auth\",\"token\":\"a.b.c\"}".into(),
    ))
    .await
    .unwrap();
    for _ in 0..2 {
        // The rest of Second's events may arrive before the close.
        match tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        {
            Message::Close(Some(frame)) => {
                assert_eq!(u16::from(frame.code), 4401);
                return;
            }
            _ => continue,
        }
    }
    expect_close(&mut ws, 4401).await;
}

fn ingest_frame(game_id: &str, tick: u64) -> Message {
    let frame = live_proto::game::Frame {
        game_id: game_id.into(),
        tick,
        players: vec![live_proto::game::FramePlayer {
            index: 0,
            alive: true,
            pos: [1.0, 2.0, 0.5],
            yaw: 0.25,
            ..Default::default()
        }],
        ..Default::default()
    };
    Message::Text(
        serde_json::to_string(&ClientMessage::Frame {
            frame: Box::new(frame),
        })
        .unwrap()
        .into(),
    )
}

async fn reply(ws: &mut Socket) -> live_proto::IngestReply {
    let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .expect("a reply in time")
        .unwrap()
        .unwrap();
    match msg {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("unexpected {other:?}"),
    }
}

#[tokio::test]
async fn ingest_fans_out_and_records_frames() {
    use live_proto::game::{NewGame, Player};
    use live_proto::sign::{EventBody, sign_batch};
    use live_proto::{Author, ErrorCode, IngestReply};

    let (live, addr) = serve(Policy::default()).await;
    let (key, account) = create(&live, "Host").await;
    let (other_key, other) = create(&live, "Other").await;
    let token = |key: &KeyPair, id: &str, method: &str| {
        issue_token(key, id, "live.test", method, live_server::app::now())
    };
    let new = NewGame {
        console: None,
        map: "lockout".into(),
        mode: "multiplayer".into(),
        variant: None,
        team_play: false,
        score_limit: None,
        time_limit_s: None,
        tick: None,
        players: vec![Player {
            index: 0,
            name: "Host".into(),
            xuid: Some(account.xuid),
            team: 0,
            local: Some(true),
            appearance: None,
        }],
        ..NewGame::default()
    };
    let game_id = live
        .start_game(Some(&token(&key, &account.id, lxm::GAMES_WRITE)), new)
        .await
        .unwrap()
        .game_id;

    let mut reader = connect(addr, &format!("?games={game_id}&exclude=GameFrame")).await;
    assert!(matches!(
        next(&mut reader).await,
        StreamMessage::CaughtUp { .. }
    ));

    let mut ws = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/ingest"))
        .await
        .unwrap()
        .0;
    // The wrong method closes the socket.
    let auth = |t: String| {
        Message::Text(
            serde_json::to_string(&ClientMessage::Auth { token: t })
                .unwrap()
                .into(),
        )
    };
    ws.send(auth(token(&key, &account.id, lxm::EVENTS_WRITE)))
        .await
        .unwrap();
    expect_close(&mut ws, 4401).await;

    let mut ws = tokio_tungstenite::connect_async(format!("ws://{addr}/v1/ingest"))
        .await
        .unwrap()
        .0;
    ws.send(auth(token(&key, &account.id, lxm::GAMES_WRITE)))
        .await
        .unwrap();
    // Two frames back to back: the second is inside the 15 Hz window.
    ws.send(ingest_frame(&game_id, 30)).await.unwrap();
    ws.send(ingest_frame(&game_id, 31)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    ws.send(ingest_frame(&game_id, 33)).await.unwrap();
    let ticks: Vec<u64> = [next(&mut reader).await, next(&mut reader).await]
        .into_iter()
        .map(|m| match m {
            StreamMessage::Frame { frame } => frame.tick,
            other => panic!("expected a frame, got {other:?}"),
        })
        .collect();
    assert_eq!(ticks, [30, 33]);

    // Event batches are answered; refused ones too, without closing.
    let author = Author::parse(&account.id).unwrap();
    let body = EventBody::new(
        &key,
        &author.to_string(),
        &format!("game-{game_id}"),
        "ScoreChanged",
        serde_json::json!({"tick": 40, "players": [{"index": 0, "score": 1, "kills": 1, "deaths": 0, "assists": 0}]}),
        time::OffsetDateTime::now_utc(),
    );
    let jws = sign_batch(&key, &[body]);
    ws.send(Message::Text(
        serde_json::to_string(&ClientMessage::Events {
            game_id: game_id.clone(),
            jws: jws.clone(),
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    let IngestReply::Appended { position, .. } = reply(&mut ws).await else {
        panic!("not appended")
    };
    ws.send(Message::Text(
        serde_json::to_string(&ClientMessage::Events {
            game_id: game_id.clone(),
            jws,
        })
        .unwrap()
        .into(),
    ))
    .await
    .unwrap();
    assert!(matches!(
        reply(&mut ws).await,
        IngestReply::Error {
            error: ErrorCode::Conflict,
            ..
        }
    ));
    // Pings need presence; frames for a game the author isn't in are refused.
    ws.send(Message::Text(
        serde_json::to_string(&ClientMessage::Ping).unwrap().into(),
    ))
    .await
    .unwrap();
    assert!(matches!(
        reply(&mut ws).await,
        IngestReply::Error {
            error: ErrorCode::NotFound,
            ..
        }
    ));
    // A fresh token must be the same author's.
    ws.send(auth(token(&other_key, &other.id, lxm::GAMES_WRITE)))
        .await
        .unwrap();
    assert!(matches!(
        reply(&mut ws).await,
        IngestReply::Error {
            error: ErrorCode::Forbidden,
            ..
        }
    ));

    // The reader sees the batch's event after the frames.
    let event = loop {
        if let StreamMessage::Event { event } = next(&mut reader).await {
            break event;
        }
    };
    assert_eq!(event.position, position);

    // One frame a second was recorded, server-signed, with its source.
    let mut recorded = Vec::new();
    for _ in 0..50 {
        recorded = live
            .recorded_frames(&game_id, None, None, 0, 100)
            .await
            .unwrap()
            .items;
        if !recorded.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // Two when the frames straddle a second boundary; the one-a-second rule
    // itself is the frame clock's unit test.
    assert!(matches!(recorded.len(), 1 | 2), "{recorded:?}");
    assert_eq!(recorded[0]["tick"], 30);
    assert_eq!(recorded[0]["source"], account.id.as_str());
}

#[tokio::test]
async fn frames_carry_species_aim_weapon_and_shots() {
    use live_proto::Author;
    use live_proto::game::{Frame, FramePlayer, NewGame, Player};

    let (live, addr) = serve(Policy::default()).await;
    let (key, account) = create(&live, "Host").await;
    let new = NewGame {
        console: None,
        map: "lockout".into(),
        mode: "multiplayer".into(),
        variant: None,
        team_play: false,
        score_limit: None,
        time_limit_s: None,
        tick: None,
        players: vec![Player {
            index: 0,
            name: "Host".into(),
            xuid: Some(account.xuid),
            team: 0,
            local: Some(true),
            appearance: None,
        }],
        ..NewGame::default()
    };
    let token = issue_token(
        &key,
        &account.id,
        "live.test",
        lxm::GAMES_WRITE,
        live_server::app::now(),
    );
    let game_id = live.start_game(Some(&token), new).await.unwrap().game_id;
    let author = Author::parse(&account.id).unwrap();
    let mut reader = connect(addr, &format!("?games={game_id}&exclude=GameFrame")).await;
    assert!(matches!(
        next(&mut reader).await,
        StreamMessage::CaughtUp { .. }
    ));

    let frame = |player: FramePlayer| Frame {
        game_id: game_id.clone(),
        tick: 30,
        players: vec![player],
        ..Frame::default()
    };
    let good = FramePlayer {
        index: 0,
        alive: true,
        pos: [1.0, 2.0, 0.5],
        yaw: 0.25,
        species: Some("elite".into()),
        aim: Some([0.0, 3.0, 4.0]),
        weapon: Some("battle_rifle".into()),
        shots: Some(3),
        ..FramePlayer::default()
    };

    // Refused before anything is sent or stored: non-finite or zero aim, an
    // id that is not [a-z0-9_]{1,32}, too many shots.
    for bad in [
        FramePlayer {
            aim: Some([f64::NAN, 0.0, 1.0]),
            ..good.clone()
        },
        FramePlayer {
            aim: Some([0.0; 3]),
            ..good.clone()
        },
        FramePlayer {
            species: Some("Elite Ultra".into()),
            ..good.clone()
        },
        FramePlayer {
            weapon: Some("objects\\weapons\\rifle\\battle_rifle".into()),
            ..good.clone()
        },
        FramePlayer {
            shots: Some(65),
            ..good.clone()
        },
    ] {
        assert!(
            live.ingest_frame(&author, frame(bad.clone()))
                .await
                .is_err(),
            "{bad:?}"
        );
    }

    live.ingest_frame(&author, frame(good.clone()))
        .await
        .unwrap();
    // Readers get it with the aim made unit length.
    let StreamMessage::Frame { frame: live_frame } = next(&mut reader).await else {
        panic!("expected a frame")
    };
    let served = &live_frame.players[0];
    assert_eq!(served.species.as_deref(), Some("elite"));
    assert_eq!(served.weapon.as_deref(), Some("battle_rifle"));
    assert_eq!(served.shots, Some(3));
    assert_eq!(served.aim, Some([0.0, 0.6, 0.8]));

    // A frame inside the 15 Hz window is dropped, but its shots ride on the
    // next frame that goes out.
    let outcome = live
        .ingest_frame(
            &author,
            Frame {
                tick: 32,
                ..frame(FramePlayer {
                    shots: Some(2),
                    ..good.clone()
                })
            },
        )
        .await
        .unwrap();
    assert_eq!(outcome, live_server::games::FrameOutcome::Throttled);
    tokio::time::sleep(Duration::from_millis(100)).await;
    live.ingest_frame(
        &author,
        Frame {
            tick: 34,
            ..frame(FramePlayer {
                shots: Some(1),
                ..good.clone()
            })
        },
    )
    .await
    .unwrap();
    let StreamMessage::Frame { frame: next_frame } = next(&mut reader).await else {
        panic!("expected a frame")
    };
    assert_eq!(next_frame.tick, 34);
    assert_eq!(next_frame.players[0].shots, Some(3));

    // And the recorded GameFrame keeps the same fields.
    let mut recorded = Vec::new();
    for _ in 0..50 {
        recorded = live
            .recorded_frames(&game_id, None, None, 0, 100)
            .await
            .unwrap()
            .items;
        if !recorded.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!recorded.is_empty());
    assert_eq!(recorded[0]["tick"], 30);
    let player = &recorded[0]["players"][0];
    assert_eq!(player["species"], "elite");
    assert_eq!(player["weapon"], "battle_rifle");
    assert_eq!(player["shots"], 3);
    assert_eq!(player["aim"], serde_json::json!([0.0, 0.6, 0.8]));
}

#[tokio::test]
async fn campaign_frames_carry_vehicles_and_actors() {
    use live_proto::Author;
    use live_proto::game::{Frame, FrameActor, FramePlayer, FrameVehicle, NewGame, Player};

    let (live, addr) = serve(Policy::default()).await;
    let (key, account) = create(&live, "Host").await;
    let new = NewGame {
        console: None,
        map: "03a_oldmombasa".into(),
        mode: "campaign".into(),
        variant: None,
        team_play: false,
        score_limit: None,
        time_limit_s: None,
        tick: None,
        players: vec![Player {
            index: 0,
            name: "Host".into(),
            xuid: Some(account.xuid),
            team: 1,
            local: Some(true),
            appearance: None,
        }],
        difficulty: Some("heroic".into()),
        ..NewGame::default()
    };
    let token = issue_token(
        &key,
        &account.id,
        "live.test",
        lxm::GAMES_WRITE,
        live_server::app::now(),
    );
    let mut bad = new.clone();
    bad.difficulty = Some("mythic".into());
    assert!(live.start_game(Some(&token), bad).await.is_err());
    let token = issue_token(
        &key,
        &account.id,
        "live.test",
        lxm::GAMES_WRITE,
        live_server::app::now(),
    );
    let game_id = live.start_game(Some(&token), new).await.unwrap().game_id;
    let summary = live.games(None, None, 10).items.remove(0);
    assert_eq!(summary.game_id, game_id);
    assert_eq!(summary.kind, "campaign", "inferred from mode");
    assert_eq!(summary.difficulty.as_deref(), Some("heroic"));
    let detail = serde_json::to_value(live.game(&game_id).unwrap()).unwrap();
    assert_eq!(detail["kind"], "campaign");
    assert_eq!(detail["difficulty"], "heroic");
    assert_eq!(detail.get("playlist"), None);
    let author = Author::parse(&account.id).unwrap();
    let mut reader = connect(addr, &format!("?games={game_id}&exclude=GameFrame")).await;
    assert!(matches!(
        next(&mut reader).await,
        StreamMessage::CaughtUp { .. }
    ));

    let good = Frame {
        game_id: game_id.clone(),
        tick: 30,
        players: vec![FramePlayer {
            index: 0,
            alive: true,
            vehicle: Some("90040004".into()),
            ..FramePlayer::default()
        }],
        vehicles: vec![FrameVehicle {
            id: "90040004".into(),
            kind: "warthog".into(),
            pos: [1.0, 2.0, 3.0],
            forward: [2.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            occupied: true,
        }],
        actors: vec![FrameActor {
            id: "90070007".into(),
            species: "grunt".into(),
            pos: [5.0, 6.0, 7.0],
            yaw: 1.0,
            aim: Some([0.0, 3.0, 4.0]),
            weapon: Some("plasma_pistol".into()),
            shots: Some(2),
            team: Some("covenant".into()),
            vehicle: None,
            stance: Some("crouch".into()),
            speed: Some(3.5),
            actions: Some(vec!["grenade".into()]),
            grenade: Some("plasma".into()),
        }],
        hits: vec![live_proto::game::FrameHit {
            from: Some("90070007".into()),
            to: Some("p0".into()),
            weapon: Some("plasma_pistol".into()),
            pos: [1.0, 2.0, 3.0],
            amount: Some(0.3),
            kind: Some("bullet".into()),
        }],
        bsp: Some(1),
        machines: vec![live_proto::game::FrameMachine {
            node: Some("scarab_0".into()),
            id: "90090009".into(),
            kind: "scarab".into(),
            pos: [0.0, 0.0, 10.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            position: None,
        }],
    };
    let mut bad_up = good.clone();
    bad_up.vehicles[0].up = [f64::NAN, 0.0, 1.0];
    let mut bad_species = good.clone();
    bad_species.actors[0].species = "Grunt Major".into();
    let mut bad_stance = good.clone();
    bad_stance.actors[0].stance = Some("prone".into());
    let mut bad_position = good.clone();
    bad_position.machines[0].position = Some(2.0);
    for bad in [bad_up, bad_species, bad_stance, bad_position] {
        assert!(live.ingest_frame(&author, bad).await.is_err());
    }

    live.ingest_frame(&author, good).await.unwrap();
    let StreamMessage::Frame { frame } = next(&mut reader).await else {
        panic!("expected a frame")
    };
    assert_eq!(frame.vehicles[0].kind, "warthog");
    assert_eq!(frame.vehicles[0].forward, [1.0, 0.0, 0.0]);
    assert_eq!(frame.actors[0].aim, Some([0.0, 0.6, 0.8]));
    assert_eq!(frame.actors[0].team.as_deref(), Some("covenant"));
    assert_eq!(frame.actors[0].shots, Some(2));

    let mut recorded = Vec::new();
    for _ in 0..50 {
        recorded = live
            .recorded_frames(&game_id, None, None, 0, 100)
            .await
            .unwrap()
            .items;
        if !recorded.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(recorded[0]["vehicles"][0]["kind"], "warthog");
    assert_eq!(recorded[0]["actors"][0]["species"], "grunt");
    assert_eq!(recorded[0]["actors"][0]["weapon"], "plasma_pistol");
    assert_eq!(
        recorded[0]["actors"][0]["actions"],
        serde_json::json!(["grenade"])
    );
    assert_eq!(recorded[0]["actors"][0]["stance"], "crouch");
    assert_eq!(recorded[0]["hits"][0]["to"], "p0");
    assert_eq!(recorded[0]["bsp"], 1);
    assert_eq!(recorded[0]["machines"][0]["node"], "scarab_0");
}
