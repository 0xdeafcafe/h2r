//! Inventory used by the finished-game archiver and its migration CLI.
mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::{start, two_player_game};
use live_proto::Gamertag;
use live_server::{Policy, store::MemoryStore};
use serde_json::json;

#[tokio::test]
async fn archive_inventory_is_admin_only_paged_and_tracks_sealed_games() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            admins: vec![Gamertag::new("Boss").unwrap()],
            ..Default::default()
        },
    )
    .await;
    let admin = h.create("Boss").await;
    let host = h.create("Host").await;
    let older = h.start_game(&host, two_player_game(&host)).await;
    let newer = h.start_game(&host, two_player_game(&host)).await;
    let path = "/v1/admin/replay-archives";
    assert_eq!(h.get(path).await.status, StatusCode::UNAUTHORIZED);
    let denied = h
        .call(
            Method::GET,
            path,
            &[h.bearer(&host, "admin.read")],
            Body::empty(),
        )
        .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let get = |uri: String| {
        let token = h.bearer(&admin, "admin.read");
        let h = &h;
        async move { h.call(Method::GET, &uri, &[token], Body::empty()).await }
    };
    let page = get(format!("{path}?limit=1")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(page.headers["cache-control"], "private, no-store");
    let page = page.json();
    assert_eq!(page["items"][0]["game_id"], newer);
    assert_eq!(page["items"][0]["state"], "live");
    let next = page["next"].as_str().unwrap();
    let second = get(format!("{path}?limit=1&cursor={next}")).await.json();
    assert_eq!(second["items"][0]["game_id"], older);
    assert!(second["next"].is_null());
    let end = h.batch(
        &host,
        &newer,
        &[("GameEnded", json!({"tick": 12, "reason": "finished"}))],
    );
    assert_eq!(
        h.post_batch(&host, &newer, end).await.status,
        StatusCode::CREATED
    );
    let info = get(format!("{path}/{newer}")).await.json();
    assert_eq!(info["ended"], true);
    assert_eq!(info["state"], "empty");
    assert!(
        info["source_position"].as_u64().unwrap()
            > page["items"][0]["source_position"].as_u64().unwrap()
    );
}

#[tokio::test]
async fn dry_run_apply_verify_and_rollback_keep_legacy_sources_exact() {
    check_signed_sources(7).await;
}

#[tokio::test]
async fn v8_signed_keys_reset_the_archived_machine_baseline() {
    check_signed_sources(8).await;
}

async fn check_signed_sources(version: u8) {
    use live_proto::{game, pack, sign::EventBody};
    use live_server::store::NewEvent;
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            admins: vec![Gamertag::new("Boss").unwrap()],
            ..Default::default()
        },
    )
    .await;
    let boss = h.create("Boss").await;
    let id = h.start_game(&boss, two_player_game(&boss)).await;
    let admin = format!("/v1/admin/replay-archives/{id}");
    let step = |request: serde_json::Value| {
        let uri = format!("{admin}/step");
        let token = h.bearer(&boss, "admin.write");
        let h = &h;
        async move {
            h.call(
                Method::POST,
                &uri,
                &[token, ("content-type", "application/json".into())],
                request.to_string(),
            )
            .await
        }
    };
    assert_eq!(
        step(json!({"mode":"apply"})).await.status,
        StatusCode::CONFLICT
    );
    let frames = [0, 2, 901, 903].map(|tick| game::Frame {
        tick,
        machine_positions: match tick {
            0 => vec![game::MachinePosition { i: 3, p: 0 }],
            2 => vec![game::MachinePosition { i: 7, p: 192 }],
            _ => vec![],
        },
        game_id: id.clone(),
        players: vec![game::FramePlayer {
            alive: true,
            pos: [1.0 / 65536.0, -0.0, 1.0 / 37.0],
            ..Default::default()
        }],
        ..Default::default()
    });
    let packed = pack::Encoder::with_version(0, 1, version)
        .unwrap()
        .encode(&frames[3])
        .unwrap()
        .0;
    let data = [
        (game::GAME_FRAME, serde_json::to_value(&frames[0]).unwrap()),
        (game::GAME_FRAME, serde_json::to_value(&frames[1]).unwrap()),
        (game::GAME_FRAME, serde_json::to_value(&frames[2]).unwrap()),
        (
            game::GAME_FRAME,
            json!({"tick":903,"packed":pack::to_base64(&packed)}),
        ),
        (game::GAME_ENDED, json!({"tick":903,"reason":"finished"})),
    ];
    let events = data
        .into_iter()
        .map(|(kind, d)| {
            let body = EventBody::new(
                &h.operator,
                &format!("server:{}", h.id),
                &game::stream(&id),
                kind,
                d,
                time::OffsetDateTime::now_utc(),
            );
            NewEvent {
                jws: body.sign(&h.operator).into(),
                jws_index: None,
                body,
            }
        })
        .collect();
    let written = h.live.store().append(events, &[]).await.unwrap();
    let seal = written.last().unwrap().position;
    // Wait until the signed end is visible in this already-running server.
    let mut applied = h.live.applied();
    applied.wait_for(|n| *n >= seal).await.unwrap();
    let public = format!("/v1/games/{id}/chunks?format=archive");
    let mut dry = json!({"mode":"dry_run","source_position":seal});
    let first = step(dry.clone()).await;
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.json());
    assert_eq!(first.json()["frames"], 2);
    dry["cursor"] = first.json()["next"].clone();
    let dry_end = step(dry).await.json();
    assert_eq!(dry_end["done"], true);
    assert_eq!(
        h.live.store().head().await.unwrap(),
        seal,
        "dry run writes nothing"
    );
    assert!(h.live.store().blob_names().await.unwrap().is_empty());
    assert_eq!(
        step(json!({"mode":"apply","source_position":seal-1}))
            .await
            .status,
        StatusCode::CONFLICT
    );
    let first = step(json!({"mode":"apply","source_position":seal}))
        .await
        .json();
    assert_eq!(first["state"], "staging");
    assert!(h.get(&public).await.json().get("archive_sha256").is_none());
    let next = json!({"mode":"apply","cursor":first["next"],"source_position":seal});
    let end = step(next.clone()).await.json();
    assert_eq!(end["done"], true);
    assert_eq!(end["state"], "published");
    assert_eq!(end["totals"]["frames"], 4);
    assert_eq!(end["totals"], dry_end["totals"]);
    assert_eq!(step(next).await.json()["totals"], end["totals"]);
    let index = h.get(&public).await.json();
    assert_eq!(index["items"].as_array().unwrap().len(), 2);
    let digest = end["archive_sha256"].as_str().unwrap();
    let node = h.live.store().blob(digest).await.unwrap().unwrap();
    let fields: serde_json::Value = serde_json::from_slice(&node).unwrap();
    assert_eq!(fields[0], "H2AR1");
    let entry = json!([{"i":3,"p":0},{"i":7,"p":192}]);
    assert!(index["items"][0].get("machine_entry").is_none());
    assert_eq!(index["items"][1]["machine_entry"], entry);
    assert_eq!(index["items"][1]["keyframes"], json!([1]));
    assert_eq!(fields[4][6], entry);
    let previous = fields[3].as_str().unwrap();
    let first_node = h.live.store().blob(previous).await.unwrap().unwrap();
    assert_eq!(end["totals"]["index_bytes"], first_node.len() + node.len());
    let url = format!("/v1/games/{id}/archive/{digest}/0");
    let first = h.get(&url).await;
    assert_eq!(first.headers["content-encoding"], "br");
    assert_eq!(
        first.headers["cache-control"],
        "public, max-age=31536000, immutable"
    );
    assert_eq!(
        h.get(&format!("{url}?auth=1")).await.headers["cache-control"],
        "private, no-store"
    );
    let authenticated = h
        .call(Method::GET, &url, &[h.bearer(&boss, "read")], Body::empty())
        .await;
    assert_eq!(authenticated.status, StatusCode::OK);
    assert_eq!(authenticated.headers["cache-control"], "private, no-store");
    live_proto::archive::verify(&frames[..2], &first.body).unwrap();
    let second = h.get(&format!("/v1/games/{id}/archive/{digest}/1")).await;
    live_proto::archive::verify(
        &[frames[2].clone(), pack::decode_keyframe(&packed).unwrap()],
        &second.body,
    )
    .unwrap();
    let v = step(json!({"mode":"verify"})).await.json();
    assert_eq!(
        step(json!({"mode":"verify","cursor":v["next"]}))
            .await
            .json()["backup_sha256"],
        end["backup_sha256"]
    );
    let rb = format!("{admin}/rollback");
    let rollback = |backup: serde_json::Value| {
        let token = h.bearer(&boss, "admin.write");
        let h = &h;
        let rb = &rb;
        async move {
            h.call(
                Method::POST,
                rb,
                &[token, ("content-type", "application/json".into())],
                json!({"archive_sha256":digest,"backup_sha256":backup}).to_string(),
            )
            .await
        }
    };
    assert_eq!(rollback(json!("stale")).await.status, StatusCode::CONFLICT);
    assert_eq!(
        rollback(end["backup_sha256"].clone()).await.json()["state"],
        "rolled_back"
    );
    assert_eq!(h.get(&url).await.status, StatusCode::NOT_FOUND);
    assert!(h.get(&public).await.json().get("archive_sha256").is_none());
    assert_eq!(
        h.get(&format!("/v1/games/{id}/frames?format=json"))
            .await
            .json()["items"][0]["players"][0]["pos"][0],
        1.0 / 65536.0
    );
}

#[tokio::test]
async fn fragmented_chunks_resume_at_first_unread_object() {
    use live_proto::{chunk, game, sign::EventBody};
    use live_server::store::NewEvent;
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            admins: vec![Gamertag::new("Boss").unwrap()],
            ..Default::default()
        },
    )
    .await;
    let boss = h.create("Boss").await;
    let id = h.start_game(&boss, two_player_game(&boss)).await;
    let mut events = Vec::new();
    let signed = |kind: &str, data| {
        let body = EventBody::new(
            &h.operator,
            &format!("server:{}", h.id),
            &game::stream(&id),
            kind,
            data,
            time::OffsetDateTime::now_utc(),
        );
        NewEvent {
            jws: body.sign(&h.operator).into(),
            jws_index: None,
            body,
        }
    };
    let mut writer = chunk::ChunkWriter::new(300);
    let mut bytes = 0;
    for tick in 0..33 {
        assert!(
            writer
                .push(&game::Frame {
                    game_id: id.clone(),
                    tick,
                    ..Default::default()
                })
                .is_none()
        );
        let chunk = writer.finish().unwrap();
        bytes += chunk.meta.bytes;
        h.live
            .store()
            .put_blob(&chunk.meta.sha256, &chunk.bytes)
            .await
            .unwrap();
        events.push(signed(
            chunk::FRAME_CHUNK,
            serde_json::to_value(chunk.meta).unwrap(),
        ));
    }
    events.push(signed(
        game::GAME_ENDED,
        json!({"tick":32,"reason":"finished"}),
    ));
    let seal = h
        .live
        .store()
        .append(events, &[])
        .await
        .unwrap()
        .last()
        .unwrap()
        .position;
    h.live.applied().wait_for(|n| *n >= seal).await.unwrap();
    let uri = format!("/v1/admin/replay-archives/{id}/step");
    for mode in ["dry_run", "apply", "verify"] {
        let mut request = json!({"mode":mode,"source_position":seal});
        for (first, frames) in [(true, 32), (false, 1)] {
            let response = h
                .call(
                    Method::POST,
                    &uri,
                    &[
                        h.bearer(&boss, "admin.write"),
                        ("content-type", "application/json".into()),
                    ],
                    request.to_string(),
                )
                .await;
            assert_eq!(response.status, StatusCode::OK, "{:?}", response.json());
            let page = response.json();
            assert_eq!(page["frames"], frames);
            assert_eq!(page["done"], !first);
            if first {
                request["cursor"] = page["next"].clone();
            } else {
                assert_eq!(page["totals"]["frames"], 33);
                assert_eq!(page["totals"]["source_bytes"], bytes);
                assert!(page["totals"]["index_bytes"].as_u64().unwrap() > 0);
            }
        }
    }
    let index = h
        .get(&format!("/v1/games/{id}/chunks?format=archive"))
        .await
        .json();
    for (i, expected) in [(0, (0..32).collect::<Vec<_>>()), (1, vec![32])] {
        let url = format!(
            "/v1/games/{id}/archive/{}/{i}",
            index["archive_sha256"].as_str().unwrap()
        );
        let response = h.get(&url).await;
        let frames = live_proto::archive::decode(&response.body).unwrap();
        assert_eq!(frames.iter().map(|f| f.tick).collect::<Vec<_>>(), expected);
    }
}

#[tokio::test]
async fn end_of_new_game_queues_a_verified_archive_on_the_shared_alarm() {
    let h = common::start_driven(Arc::new(MemoryStore::new()), Policy::default()).await;
    let host = h.create("Host").await;
    let id = h.start_game(&host, two_player_game(&host)).await;
    let author = live_proto::Author::parse(&host.author).unwrap();
    let frame = live_proto::game::Frame {
        game_id: id.clone(),
        tick: 40,
        ..Default::default()
    };
    h.live.ingest_frame(&author, frame.clone()).await.unwrap();
    let url = format!("/v1/games/{id}/chunks?format=archive");
    assert!(h.get(&url).await.json().get("archive_sha256").is_none());
    let end = h.batch(
        &host,
        &id,
        &[("GameEnded", json!({"tick":40,"reason":"finished"}))],
    );
    assert_eq!(
        h.post_batch(&host, &id, end).await.status,
        StatusCode::CREATED
    );
    assert!(h.get(&url).await.json().get("archive_sha256").is_none());
    assert!(h.live.sweep_due().is_some_and(|at| at <= common::now()));
    h.live.sweep().await;
    let index = h.get(&url).await.json();
    let digest = index["archive_sha256"].as_str().unwrap();
    let segment = h.get(&format!("/v1/games/{id}/archive/{digest}/0")).await;
    live_proto::archive::verify(&[frame], &segment.body).unwrap();
    let head = h.live.store().head().await.unwrap();
    h.live.sweep().await;
    assert_eq!(h.live.store().head().await.unwrap(), head);
}

#[tokio::test]
async fn a_failed_final_blob_write_keeps_the_source_open_for_retry() {
    let h = common::start_driven(Arc::new(MemoryStore::new()), Policy::default()).await;
    h.live
        .set_blobs(live_server::blobs::Blobs::dir(&h.dir).unwrap());
    let host = h.create("Host").await;
    let id = h.start_game(&host, two_player_game(&host)).await;
    let author = live_proto::Author::parse(&host.author).unwrap();
    let frame = live_proto::game::Frame {
        game_id: id.clone(),
        tick: 40,
        ..Default::default()
    };
    h.live.ingest_frame(&author, frame.clone()).await.unwrap();
    let end = || {
        h.batch(
            &host,
            &id,
            &[("GameEnded", json!({"tick":40,"reason":"finished"}))],
        )
    };
    // A regular file where the private chunk directory belongs makes the
    // write fail even when the test runs as root.
    let obstruction = h.dir.join("frames");
    std::fs::write(&obstruction, b"occupied").unwrap();
    assert_eq!(
        h.post_batch(&host, &id, end()).await.status,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert!(h.live.frame_chunks(&id).await.unwrap().is_empty());
    std::fs::remove_file(obstruction).unwrap();
    assert_eq!(
        h.post_batch(&host, &id, end()).await.status,
        StatusCode::CREATED
    );
    assert_eq!(h.live.chunk_frames(&id).await.unwrap(), vec![frame]);
    h.live.sweep().await;
    let index = h
        .get(&format!("/v1/games/{id}/chunks?format=archive"))
        .await
        .json();
    assert!(index["archive_sha256"].is_string());
}

#[tokio::test]
async fn rewound_chunk_starts_retain_each_distinct_original_in_the_blob_store() {
    let h = common::start_driven(Arc::new(MemoryStore::new()), Policy::default()).await;
    h.live
        .set_blobs(live_server::blobs::Blobs::dir(&h.dir).unwrap());
    let host = h.create("Host").await;
    let id = h.start_game(&host, two_player_game(&host)).await;
    let author = live_proto::Author::parse(&host.author).unwrap();
    for (n, tick) in [40, 340, 40].into_iter().enumerate() {
        if n != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(75)).await;
        }
        h.live
            .ingest_frame(
                &author,
                live_proto::game::Frame {
                    game_id: id.clone(),
                    tick,
                    bsp: Some(n as u32),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    let end = h.batch(
        &host,
        &id,
        &[("GameEnded", json!({"tick":40,"reason":"finished"}))],
    );
    assert_eq!(
        h.post_batch(&host, &id, end).await.status,
        StatusCode::CREATED
    );
    for m in h.live.frame_chunks(&id).await.unwrap() {
        let response = h
            .get(&format!(
                "/v1/games/{id}/chunks/{}?sha256={}",
                m.from_tick, m.sha256
            ))
            .await;
        assert_eq!(response.status, StatusCode::OK);
        let header = live_proto::chunk::frames(&response.body).unwrap();
        assert_eq!(header[0].tick, m.from_tick);
        assert_eq!(
            live_proto::sign::hex(&<sha2::Sha256 as sha2::Digest>::digest(&response.body)),
            m.sha256
        );
    }
    let frames = h.live.chunk_frames(&id).await.unwrap();
    let mut values: Vec<_> = frames.iter().map(|f| (f.bsp, f.tick)).collect();
    values.sort();
    assert_eq!(values, vec![(Some(0), 40), (Some(1), 340), (Some(2), 40)]);
}
