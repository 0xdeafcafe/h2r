//! Through the same HTTP route on native Live and the Worker. A real v3
//! stream can finish an absent, partly uploaded or already completed game.
use super::*;
use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use live_proto::film::{
    events::{Event, Record},
    range, stream,
};
use tower::ServiceExt;

fn chunk(buf: &mut Vec<u8>, entries: &mut Vec<Value>, tag: &[u8; 4], raw: &[u8]) -> (u64, u32) {
    let compressed =
        ruzstd::encoding::compress_to_vec(raw, ruzstd::encoding::CompressionLevel::Uncompressed);
    let offset = buf.len() as u64;
    let size = compressed.len() as u32;
    entries.push(json!({"tag":tag,"offset":offset,"size":size}));
    buf.extend_from_slice(tag);
    buf.extend(size.to_le_bytes());
    buf.extend(compressed);
    (offset, size)
}

fn kill() -> game::PlayerKilled {
    game::PlayerKilled {
        tick: 35,
        victim: 1,
        killer: Some(0),
        weapon: Some("rifle".into()),
        kind: "kill".into(),
        pos: None,
    }
}

fn fixture(id: &str, new: game::NewGame, kills: usize) -> Vec<u8> {
    fixture_at(id, new, kills, now(), 30)
}

fn fixture_at(id: &str, new: game::NewGame, kills: usize, started: i64, hz: u16) -> Vec<u8> {
    let front = range::Front {
        final_head: range::DATA_OFFSET,
        id: id.into(),
        image_stamp: "test".into(),
        runtime_stamp: "test".into(),
        started_ms: (started * 1000) as u64,
        map_sha256: "".into(),
        map_origin: "retail".into(),
        match_start_tick: Some(1),
        ticks_per_second: Some(hz),
        roster: vec![],
        summary: live_proto::film::Summary {
            level: "lockout".into(),
            ..Default::default()
        },
    };
    let json = serde_json::to_vec(&front).unwrap();
    let mut bytes = range::Prefix {
        json_len: json.len() as u32,
    }
    .encode()
    .to_vec();
    bytes.extend(json);
    bytes.resize(range::DATA_OFFSET as usize, 0);
    let mut entries = Vec::new();
    chunk(
        &mut bytes,
        &mut entries,
        b"HEAD",
        &serde_json::to_vec(&json!({"version":3,"meta":{"started_ms":front.started_ms}})).unwrap(),
    );

    let mut tick = stream::ChunkWriter::new(1, 100).unwrap();
    let mut add = |tick_no, event| {
        let mut payload = vec![stream::STATS_EVENT];
        payload.extend(
            Record {
                tick: tick_no,
                event,
            }
            .encode()
            .unwrap(),
        );
        tick.record(tick_no, stream::EVENT, &payload).unwrap();
    };
    add(
        1,
        Event::GameStarted(game::GameStarted {
            game_id: id.into(),
            host: "host-local-name-is-not-trusted".into(),
            dedicated: Default::default(),
            game: new,
        }),
    );
    for _ in 0..kills {
        add(35, Event::PlayerKilled(kill()));
    }
    add(
        90,
        Event::GameEnded(serde_json::from_value(end().1).unwrap()),
    );
    let raw = tick.finish();
    let (offset, size) = chunk(&mut bytes, &mut entries, b"TICK", &raw);
    let directory = range::Directory {
        chunks: vec![range::Chunk {
            clock: vec![],
            scene: 1,
            first: 1,
            last: 100,
            offset,
            size,
            raw_size: raw.len() as u32,
        }],
        ..Default::default()
    };
    let dir = serde_json::to_vec(&directory).unwrap();
    let (cidx_offset, _) = chunk(&mut bytes, &mut entries, b"CIDX", &dir);
    let index = serde_json::to_vec(&entries).unwrap();
    let (indx_offset, _) = chunk(&mut bytes, &mut vec![], b"INDX", &index);
    let footer = range::Footer {
        cidx_offset,
        indx_offset,
        cidx_raw_size: dir.len() as u32,
        file_len: bytes.len() as u64 + range::FOOTER_BYTES as u64,
    };
    bytes.extend(footer.encode());
    bytes
}

async fn upload(live: &Arc<Live>, id: &str, token: &str, bytes: Vec<u8>) -> StatusCode {
    let req = Request::post(format!("/v1/games/{id}/film"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(bytes))
        .unwrap();
    crate::http::router(live.clone())
        .oneshot(req)
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn film_finishes_absent_partial_and_completed_games_exactly_once() {
    for previous in [0, 1, 2] {
        let live = live().await;
        let lease = lease();
        let id = &lease.id;
        let token = assigned_lease(&live, &lease).await;
        let store = crate::blobs::BlobStore::memory();
        live.set_blobs(crate::blobs::Blobs {
            private: store.clone(),
            public: None,
        });
        if previous > 0 {
            live.start_game(Some(&token), new_game()).await.unwrap();
            let mut rows = vec![(game::PLAYER_KILLED, json!(kill()))];
            if previous == 2 {
                rows.push(end());
            }
            live.append_game_events(Some(&token), id, &batch(&live, &token, id, &rows))
                .await
                .unwrap();
            // A pending/live or finalised telemetry stream cannot attach a
            // conflicting film. A later matching upload remains possible.
            assert_eq!(
                upload(&live, id, &token, fixture(id, new_game(), 0)).await,
                StatusCode::CONFLICT
            );
        }
        let film = fixture(id, new_game(), 1);
        for _ in 0..2 {
            assert_eq!(
                upload(&live, id, &token, film.clone()).await,
                StatusCode::NO_CONTENT
            );
        }
        let game = live.model.read().games[id].clone();
        assert!(game.has_host_film && game.ended_by_host);
        assert_eq!(game.started.host, format!("host:{id}"));
        assert_eq!(game.started.game.session_id.as_deref(), Some(SESSION));
        assert_eq!(game.film_summary.unwrap().level, "lockout");
        let key = game.film_blob.unwrap();
        assert!(key.starts_with(&format!("films/{id}/")));
        assert_eq!(
            store.get(&key).await.unwrap().unwrap().as_ref(),
            film.as_slice()
        );
        assert!(
            store
                .list(&format!("{key}.import/"))
                .await
                .unwrap()
                .is_empty()
        );
        let events = live
            .store
            .read(&crate::store::Query {
                stream: Some(game::stream(id)),
                types: vec![
                    game::GAME_STARTED.into(),
                    game::PLAYER_KILLED.into(),
                    game::GAME_ENDED.into(),
                ],
                limit: 8,
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            events
                .iter()
                .map(|e| e.body.kind.as_str())
                .collect::<Vec<_>>(),
            [game::GAME_STARTED, game::PLAYER_KILLED, game::GAME_ENDED]
        );
        let req = Request::get(format!("/v1/games/{id}/film"))
            .header(header::RANGE, "bytes=0-31")
            .body(Body::empty())
            .unwrap();
        let res = crate::http::router(live.clone())
            .oneshot(req)
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            &to_bytes(res.into_body(), 32).await.unwrap()[..],
            &film[..32]
        );
    }
}

#[tokio::test]
async fn multiple_film_stats_pages_keep_every_event_in_order() {
    let live = live().await;
    let lease = lease();
    let token = assigned_lease(&live, &lease).await;
    live.set_blobs(crate::blobs::Blobs {
        private: crate::blobs::BlobStore::memory(),
        public: None,
    });
    let bytes = fixture(&lease.id, new_game(), 260);
    assert_eq!(
        upload(&live, &lease.id, &token, bytes).await,
        StatusCode::NO_CONTENT
    );
    let rows = live
        .store
        .read(&crate::store::Query {
            stream: Some(game::stream(&lease.id)),
            types: vec![game::PLAYER_KILLED.into(), game::GAME_ENDED.into()],
            limit: 300,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(rows.len(), 261);
    assert_eq!(rows.last().unwrap().body.kind, game::GAME_ENDED);
    assert!(live.model.read().films.imports.is_empty());
}

#[tokio::test]
async fn sweeper_resumes_a_selected_page_without_host_credentials() {
    use sha2::{Digest, Sha256};
    let live = live().await;
    let (token, id) = assigned_game(&live, &lease()).await;
    let store = crate::blobs::BlobStore::memory();
    live.set_blobs(crate::blobs::Blobs {
        private: store.clone(),
        public: None,
    });
    live.append_game_events(
        Some(&token),
        &id,
        &batch(&live, &token, &id, &[(game::PLAYER_KILLED, json!(kill()))]),
    )
    .await
    .unwrap();
    let blob = format!("films/{id}/selected.h2film");
    let page =
        serde_json::to_vec(&json!([{"tick":90,"kind":game::GAME_ENDED,"data":end().1}])).unwrap();
    let page_sha = format!("{:x}", Sha256::digest(&page));
    store
        .put(
            &format!("{blob}.import/1"),
            page.into(),
            &Default::default(),
        )
        .await
        .unwrap();
    let pending = json!({
        "receipt":{"game":id,"blob":blob},
        "pages":[{"count":1,"sha256":"already-applied"},{"count":1,"sha256":page_sha}],
        "next":0,"skip":0,"origin":1,"hz":30,"start":now()-3,"sealed":now()
    });
    // These two server-signed records are the durable state after page zero.
    // Its transient blob was deleted, so trying to replay that page would fail.
    record(
        &live,
        vec![
            live.server_event(
                crate::films::import_stream(&id),
                "HostFilmImportStarted",
                pending,
            ),
            live.server_event(
                crate::films::import_stream(&id),
                "HostFilmImportAdvanced",
                json!({"game":id,"page":0}),
            ),
        ],
    )
    .await;
    let retry = batch(&live, &token, &id, &[end()]);
    assert!(
        live.append_game_events(Some(&token), &id, &retry)
            .await
            .is_err()
    );
    live.sweep().await;
    let g = live.model.read().games[&id].clone();
    assert!(g.has_host_film && g.ended_by_host);
    assert_eq!(g.kills.len(), 1);
    assert!(
        store
            .get(&format!("{blob}.import/1"))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn simulated_film_first_uses_the_private_assignment_id() {
    let live = live().await;
    let mut lease = lease();
    lease.assignment_id = "sim-bot-run".into();
    lease.roster.clear();
    lease.bots = vec!["One".into(), "Two".into()];
    record(
        &live,
        vec![live.server_event(
            live_proto::hosts::STREAM.into(),
            live_proto::hosts::HOST_REQUESTED,
            json!({"lease":lease}),
        )],
    )
    .await;
    let token = live.issue_host_token(&lease);
    live.set_blobs(crate::blobs::Blobs {
        private: crate::blobs::BlobStore::memory(),
        public: None,
    });
    let mut game = new_game();
    for p in &mut game.players {
        p.xuid = None;
        p.bot = Some(true);
    }
    game.sim = Some(game::SimGame {
        run_id: "bot-run".into(),
        batch_id: None,
        seed: "42".into(),
        gametype: "slayer".into(),
        pace: "unpaced".into(),
        bots: vec![json!({}), json!({})],
        deferred: true,
    });
    let id = &lease.assignment_id;
    assert_eq!(
        upload(&live, id, &token, fixture(id, game, 1)).await,
        StatusCode::NO_CONTENT
    );
    let g = live.model.read().games[id].clone();
    assert!(g.has_host_film && g.ended_by_host);
    assert_eq!(g.started.game.sim.unwrap().run_id, "bot-run");
    let req = Request::get(format!("/v1/games/{id}/film"))
        .header(header::RANGE, "bytes=0-31")
        .body(Body::empty())
        .unwrap();
    let res = crate::http::router(live).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn custom_unadvertised_film_is_bound_to_its_signed_map() {
    let live = live().await;
    let mut lease = lease();
    lease.assignment_id = "custom-training".into();
    lease.hopper = None;
    lease.roster.clear();
    record(
        &live,
        vec![live.server_event(
            live_proto::hosts::STREAM.into(),
            live_proto::hosts::HOST_REQUESTED,
            json!({"lease":lease}),
        )],
    )
    .await;
    let token = live.issue_host_token(&lease);
    live.set_blobs(crate::blobs::Blobs {
        private: crate::blobs::BlobStore::memory(),
        public: None,
    });
    let mut wrong = new_game();
    wrong.map = "midship".into();
    assert_eq!(
        upload(&live, &lease.id, &token, fixture(&lease.id, wrong, 1)).await,
        StatusCode::FORBIDDEN
    );
    assert!(!live.model.read().games.contains_key(&lease.id));
    assert_eq!(
        upload(&live, &lease.id, &token, fixture(&lease.id, new_game(), 1)).await,
        StatusCode::NO_CONTENT
    );
    assert!(
        live.model.read().games[&lease.id]
            .started
            .game
            .session_id
            .is_none()
    );
}

#[tokio::test]
async fn simultaneous_different_films_select_one_object_and_its_stats() {
    let live = live().await;
    let lease = lease();
    let token = assigned_lease(&live, &lease).await;
    let store = crate::blobs::BlobStore::memory();
    live.set_blobs(crate::blobs::Blobs {
        private: store.clone(),
        public: None,
    });
    let first = fixture(&lease.id, new_game(), 1);
    let second = fixture(&lease.id, new_game(), 2);
    let (a, b) = tokio::join!(
        upload(&live, &lease.id, &token, first.clone()),
        upload(&live, &lease.id, &token, second.clone())
    );
    assert_eq!((a, b), (StatusCode::NO_CONTENT, StatusCode::NO_CONTENT));
    let game = live.model.read().games[&lease.id].clone();
    let stored = store
        .get(game.film_blob.as_deref().unwrap())
        .await
        .unwrap()
        .unwrap();
    if stored.as_ref() == first.as_slice() {
        assert_eq!(game.kills.len(), 1);
    } else {
        assert_eq!(stored.as_ref(), second.as_slice());
        assert_eq!(game.kills.len(), 2);
    }
    assert_eq!(
        store
            .list(&format!("films/{}/", lease.id))
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn event_wall_times_use_the_recorded_guest_rate() {
    let live = live().await;
    let mut lease = lease();
    let started = now() - 120;
    lease.created_ms = started * 1000;
    let token = assigned_lease(&live, &lease).await;
    live.set_blobs(crate::blobs::Blobs {
        private: crate::blobs::BlobStore::memory(),
        public: None,
    });
    let mut game = new_game();
    game.tick = Some(1);
    let film = fixture_at(&lease.id, game, 1, started, 45);
    assert_eq!(
        upload(&live, &lease.id, &token, film).await,
        StatusCode::NO_CONTENT
    );
    let g = live.model.read().games[&lease.id].clone();
    assert_eq!(
        live_proto::sign::parse_time(&g.started_at)
            .unwrap()
            .unix_timestamp(),
        started
    );
    let (_, at) = g.ended.unwrap();
    assert_eq!(
        live_proto::sign::parse_time(&at).unwrap().unix_timestamp(),
        started + 1
    ); // (90 − 1) / 45
    let request = Request::get("/v1/films?kind=recent&player=0000000000000001")
        .body(Body::empty())
        .unwrap();
    let res = crate::http::router(live).oneshot(request).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = to_bytes(res.into_body(), 64 << 10).await.unwrap();
    let page: Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(page["items"][0]["ticks_per_second"], 45);
    assert_eq!(page["items"][0]["duration_s"], 1);
}
