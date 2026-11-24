//! Both homes execute these requests against their own event store: the
//! script view of `/v1/rules/check` and a variant made from a script
//! (docs/game/rules-variants.md, "Copies, saves, files and Live"). The
//! native server runs it on SQLite, the Worker on its SQL store, and both
//! must answer the same fields.
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use live_proto::{
    lxm,
    sign::{AccountRequest, KeyPair, issue_token},
};
use live_server::{Live, app::now};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

/// A script that scores through Slayer (`uses slayer`), with settings, quick marks
/// and an `about` (a copy of the listed Duel, so the Worker's tests need
/// no `rules` dependency).
const DUEL: &str = r#"rules "Duel" 1
type slayer
uses slayer
about "One on one, magnum and a random weapon, nothing on the map. Winner stays on. Try Lockout or Ascension."
quick score_limit, time_limit, starting_weapon, secondary_weapon, weapons_on_map

settings {
    max_players = 2
    starting_weapon = magnum
    secondary_weapon = random
    weapons_on_map = none
    score_limit = 15
}
"#;

async fn call(
    live: &Arc<Live>,
    method: &str,
    url: &str,
    bearer: Option<String>,
    body: Vec<u8>,
) -> (u16, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(url)
        .header("content-type", "application/json");
    if let Some(b) = bearer {
        req = req.header("authorization", format!("Bearer {b}"));
    }
    let res = live_server::http::router(live.clone())
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = res.status().as_u16();
    let bytes = to_bytes(res.into_body(), 4 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

pub async fn exercise(live: Arc<Live>) {
    // The script view: settings in words, the quick marks, who scores.
    let check = json!({"source": DUEL}).to_string().into_bytes();
    let (status, v) = call(&live, "POST", "/v1/rules/check", None, check).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["scoring"], "carrier");
    assert!(v["about"].as_str().unwrap().starts_with("One on one"));
    let settings = v["settings"].as_array().unwrap();
    let score = settings
        .iter()
        .find(|s| s["name"] == "score_limit")
        .unwrap();
    assert_eq!(score["value"], "15");
    assert_eq!(v["quick"][0]["kind"], "setting");
    assert_eq!(v["quick"][0]["name"], "score_limit");
    assert_eq!(v["quick"][0]["label"], score["label"]);

    // A variant made from it takes the script's description and record.
    let key = KeyPair::generate();
    let (status, row) = call(
        &live,
        "POST",
        "/v1/accounts",
        None,
        AccountRequest::sign(&key, "Maker", None, "live.test", now()).into_bytes(),
    )
    .await;
    assert_eq!(status, 201, "{row}");
    let issuer = row["id"].as_str().unwrap();
    let bearer = issue_token(&key, issuer, "live.test", lxm::CONTENT_WRITE, now());
    let (status, made) = call(
        &live,
        "POST",
        "/v1/variants/from-script",
        Some(bearer),
        json!({"source": DUEL, "name": "My Duel", "visibility": "public"})
            .to_string()
            .into_bytes(),
    )
    .await;
    assert_eq!(status, 201, "{made}");
    assert_eq!(made["description"], v["about"]);
    assert_eq!(made["game_type"], "Slayer");
    assert_eq!(made["script_name"], "Duel");
    assert!(!made["settings"].as_array().unwrap().is_empty());
    // The same fields on a read.
    let id = made["id"].as_str().unwrap();
    let (status, read) = call(&live, "GET", &format!("/v1/variants/{id}"), None, vec![]).await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["record_hex"], made["record_hex"]);
    assert_eq!(read["description"], made["description"]);
}
