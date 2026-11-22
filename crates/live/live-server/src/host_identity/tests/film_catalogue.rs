//! A signed film receipt is enough for Theater. Only genuine game events
//! can create a match or contribute a player's statistics.
use super::*;
use axum::{
    body::{Body, Bytes, to_bytes},
    http::{Request, StatusCode, header},
};
use live_proto::film::{DiscrepancyCause, StatsDiscrepancy};
use tower::ServiceExt;

async fn store_receipt(live: &Live, id: &str, lease: &HostLease) -> String {
    let blob = format!("films/{id}/catalogue.h2film");
    live.blobs()
        .unwrap()
        .private
        .put(
            &blob,
            Bytes::from_static(b"0123456789abcdef"),
            &crate::blobs::Meta::default(),
        )
        .await
        .unwrap();
    let receipt = json!({
        "game": id,
        "catalogue_host": format!("host:{}", lease.id),
        "catalogue_started_ms": 1_700_000_000_031_u64,
        "catalogue_players": [Xuid(1)],
        "summary": {
            "level": "lockout", "mode":"custom", "game_type":"slayer", "variant_name":"Snipers",
            "duration_ticks":3_600, "players":[
                {"seat":0, "team":0, "name":"Guest", "xuid":Xuid(98), "score":7, "medals":{}}
            ]
        },
        "stats_discrepancy": StatsDiscrepancy {
            cause: DiscrepancyCause::MissingStart, event_index: 0,
            film_tick: None, telemetry_position: None,
        },
        "blob": &blob,
        "web_ranges": {
            "public_safe": true, "size":16,"summary_bytes":0,"index_offset":14,
            "index_size":1, "index_sha256":"test", "ticks_per_second":60
        }
    });
    record(
        live,
        vec![live.server_event(crate::films::import_stream(id), "HostFilmStored", receipt)],
    )
    .await;
    blob
}

async fn list(live: &Arc<Live>, kind: &str, player: Xuid) -> Vec<Value> {
    let req = Request::get(format!("/v1/films?kind={kind}&player={player}"))
        .body(Body::empty())
        .unwrap();
    let resp = crate::http::router(live.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let json: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 32 * 1024).await.unwrap()).unwrap();
    json["items"].as_array().unwrap().clone()
}

async fn range(live: &Arc<Live>, id: &str) -> StatusCode {
    let req = Request::get(format!("/v1/games/{id}/film"))
        .header(header::RANGE, "bytes=0-7")
        .body(Body::empty())
        .unwrap();
    crate::http::router(live.clone())
        .oneshot(req)
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn incomplete_stats_create_only_a_film_and_late_telemetry_attaches_it() {
    let live = live().await;
    let lease = lease();
    let token = assigned_lease(&live, &lease).await;
    let id = &lease.id;
    live.set_blobs(crate::blobs::Blobs {
        private: crate::blobs::BlobStore::memory(),
        public: None,
    });
    let blob = store_receipt(&live, id, &lease).await;
    {
        let model = live.model.read();
        assert!(!model.games.contains_key(id));
        assert_eq!(model.films.import_version(id), 1);
        assert_eq!(model.films.standalone_host_blob(id).unwrap().1, blob);
    }
    let rows = list(&live, "recent", Xuid(1)).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["stats_discrepancy"]["cause"], "missing_start");
    assert_eq!(rows[0]["duration_s"], 60);
    assert_eq!(rows[0]["has_host_film"], true);
    assert!(rows[0]["summary"]["players"][0]["xuid"].is_null());
    assert!(list(&live, "recent", Xuid(98)).await.is_empty());
    assert_eq!(range(&live, id).await, StatusCode::PARTIAL_CONTENT);

    record(
        &live,
        vec![live.server_event(
            crate::films::STREAM.into(),
            "FilmSaved",
            json!({"game":id,"by":Xuid(1)}),
        )],
    )
    .await;
    assert_eq!(list(&live, "saved", Xuid(1)).await.len(), 1);
    let created = live.start_game(Some(&token), new_game()).await.unwrap();
    assert_eq!(created.game_id, *id);
    live.append_game_events(Some(&token), id, &batch(&live, &token, id, &[end()]))
        .await
        .unwrap();
    {
        let model = live.model.read();
        let game = &model.games[id];
        assert!(game.ended_by_host);
        assert_eq!(game.film_blob.as_deref(), Some(blob.as_str()));
        assert_eq!(game.recording.saved_by, 1);
        assert!(model.films.standalone_host_blob(id).is_none());
    }
    let rows = list(&live, "recent", Xuid(1)).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["duration_s"], 60);
    assert_eq!(list(&live, "saved", Xuid(1)).await.len(), 1);
}

#[tokio::test]
async fn simulation_catalogue_receipts_hide_ranges_and_participation() {
    let live = live().await;
    live.set_blobs(crate::blobs::Blobs {
        private: crate::blobs::BlobStore::memory(),
        public: None,
    });
    store_receipt(&live, "sim-hidden", &lease()).await;
    assert!(list(&live, "recent", Xuid(1)).await.is_empty());
    assert_eq!(range(&live, "sim-hidden").await, StatusCode::NOT_FOUND);
}
