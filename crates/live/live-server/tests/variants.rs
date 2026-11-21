//! Published game variants (docs/live/live-api.md "Game variants"): an
//! account publishes a whole `.h2variant` file, or one built from a
//! script; anyone reads it, its file and an author's public list; its
//! author deletes it; a second server on the log reads the same. Run with
//! the server's own timers and with none (the Worker's Durable Object).
mod common;
use axum::{http::Method, http::StatusCode};
use common::*;
use live_proto::lxm;
use live_server::app::Timers;
use live_server::store::{EventStore, MemoryStore};
use serde_json::{Value, json};
use std::sync::Arc;
use variants::portable::{self, PortScript};
use variants::record::{GameType, GameVariant, SettingId};

async fn send(h: &Harness, method: Method, path: &str, body: Value, who: Option<&User>) -> Resp {
    let mut headers = vec![("content-type", "application/json".to_string())];
    if let Some(u) = who {
        headers.push(h.bearer(u, lxm::CONTENT_WRITE));
    }
    h.call(method, path, &headers, body.to_string()).await
}

async fn publish(h: &Harness, me: &User, body: Value) -> Resp {
    send(h, Method::POST, "/v1/variants", body, Some(me)).await
}

fn slayer_hash() -> String {
    rules::compile(rules::builtin::SLAYER).unwrap().id().to_string()
}

fn record_of(hex: &str) -> GameVariant {
    let bytes: Vec<u8> = (0..hex.len() / 2)
        .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    GameVariant::from_bytes(&bytes).unwrap()
}

fn plain_king() -> GameVariant {
    let mut v = GameVariant::blank(GameType::King);
    v.set_name("Hill").unwrap();
    v
}

async fn flow(timers: Timers) {
    let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
    let h = start_with(store.clone(), live_server::Policy::default(), timers).await;
    let me = h.create("Maker").await;
    let other = h.create("Other").await;

    let body = json!({
        "script_hash": slayer_hash(),
        "name": "Triple Kills",
        "description": "Three points a kill",
        "visibility": "public",
        "options": {"kill_points": 3, "suicide_costs": false},
    });
    let r = send(&h, Method::POST, "/v1/variants/from-script", body.clone(), None).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = send(&h, Method::POST, "/v1/variants/from-script", body.clone(), Some(&me)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let v = r.json();
    let id = v["id"].as_str().unwrap().to_string();
    assert_eq!(v["name"], "Triple Kills");
    assert_eq!(v["description"], "Three points a kill");
    assert_eq!(v["author"]["gamertag"], "Maker");
    assert_eq!(v["author"]["xuid"], me.xuid.to_string());
    assert_eq!(v["engine"], 2);
    assert_eq!(v["game_type"], "Slayer");
    assert_eq!(v["script_hash"], slayer_hash());
    assert_eq!(v["script_name"], "Slayer");
    assert_eq!(v["visibility"], "public");
    assert_eq!(v["bundle_url"], format!("/v1/variants/{id}/h2variant"));
    assert!(v["settings"].as_array().unwrap().iter().any(|s| s["label"] == "Time limit (seconds)"));
    // The record names the script and packs the options in order:
    // kill_points 3 is 2 above its low end in bits 0..3, suicide_costs off.
    let rec = record_of(v["record_hex"].as_str().unwrap());
    let script = rec.ext().script;
    assert_eq!(script.mode, variants::ext::Mode::Rules);
    assert!(slayer_hash().starts_with(
        &script.prefix.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ));
    assert_eq!(script.options, 2);
    assert_eq!(rec.name(), "Triple Kills");

    // The same request again publishes nothing new.
    let r = send(&h, Method::POST, "/v1/variants/from-script", body.clone(), Some(&me)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    assert_eq!(r.json()["id"], id);

    // An unlisted one, from a whole file.
    let file = portable::port(&plain_king(), None, Some("No script"), PortScript::Keep).unwrap();
    let r = send(
        &h,
        Method::POST,
        "/v1/variants",
        json!({"bundle": file, "visibility": "unlisted"}),
        Some(&me),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let unlisted = r.json()["id"].as_str().unwrap().to_string();
    assert_eq!(r.json()["script_hash"], Value::Null);
    assert_eq!(r.json()["game_type"], "King of the Hill");

    // Nobody posts the event by hand.
    let forged = h.event(
        &me,
        &live_proto::game_variants::stream(&"ab".repeat(32)),
        live_proto::game_variants::PUBLISHED,
        json!({"id": "ab".repeat(32), "author": me.xuid, "visibility": "public", "bundle": "{}"}),
    );
    assert_eq!(h.post_event(&me, forged).await.status, StatusCode::FORBIDDEN);

    for h in [&h, &start_with(store.clone(), live_server::Policy::default(), timers).await] {
        let r = h.get(&format!("/v1/variants/{id}")).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.error());
        assert_eq!(r.json()["name"], "Triple Kills");

        // The file is the canonical bytes its id hashes, and reads back.
        let r = h.get(&format!("/v1/variants/{id}/h2variant")).await;
        assert_eq!(r.status, StatusCode::OK);
        assert!(
            r.headers["content-disposition"]
                .to_str()
                .unwrap()
                .contains("Triple Kills.h2variant")
        );
        let text = String::from_utf8(r.body.clone()).unwrap();
        let b = portable::read(&text).unwrap();
        assert_eq!(b.id(), id);
        assert_eq!(b.author.as_ref().unwrap().gamertag, "Maker");
        let s = b.script.as_ref().unwrap();
        assert_eq!(s.source.as_deref(), Some(rules::builtin::SLAYER));
        assert!(b.check().is_ok());

        for author in ["Maker".to_string(), me.xuid.to_string()] {
            let r = h.get(&format!("/v1/variants?author={author}")).await;
            assert_eq!(r.status, StatusCode::OK, "{}", r.error());
            let items = r.json()["items"].as_array().unwrap().clone();
            assert_eq!(items.len(), 1, "unlisted ones stay off the public list");
            assert_eq!(items[0]["id"], id);
        }
        // Its author sees the unlisted one too, newest first.
        let r = h
            .call(
                Method::GET,
                "/v1/variants?author=Maker",
                &[h.bearer(&me, lxm::READ)],
                axum::body::Body::empty(),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.error());
        let ids: Vec<Value> = r.json()["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].clone())
            .collect();
        assert_eq!(ids, [json!(unlisted), json!(id)]);
        // Unlisted is readable by id.
        assert_eq!(h.get(&format!("/v1/variants/{unlisted}")).await.status, StatusCode::OK);

        assert_eq!(h.get("/v1/variants?author=Other").await.json()["items"], json!([]));
        assert_eq!(h.get("/v1/variants?author=Nobody").await.status, StatusCode::NOT_FOUND);
        assert_eq!(h.get("/v1/variants").await.status, StatusCode::BAD_REQUEST);
        assert_eq!(
            h.get(&format!("/v1/variants/{}", "cd".repeat(32))).await.status,
            StatusCode::NOT_FOUND
        );
        assert_eq!(h.get("/v1/variants/nope").await.status, StatusCode::NOT_FOUND);
    }

    // Only its author deletes it.
    let path = format!("/v1/variants/{id}");
    let r = send(&h, Method::DELETE, &path, json!({}), Some(&other)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.error());
    let r = send(&h, Method::DELETE, &path, json!({}), Some(&me)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    assert_eq!(h.get(&path).await.status, StatusCode::NOT_FOUND);
    assert_eq!(h.get(&format!("{path}/h2variant")).await.status, StatusCode::NOT_FOUND);
    let r = send(&h, Method::DELETE, &path, json!({}), Some(&me)).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let h2 = start_with(store.clone(), live_server::Policy::default(), timers).await;
    assert_eq!(h2.get(&path).await.status, StatusCode::NOT_FOUND);

    // Publishing it again after the tombstone brings it back.
    let r = send(&h, Method::POST, "/v1/variants/from-script", body, Some(&me)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    assert_eq!(r.json()["id"], id);
    assert_eq!(h.get(&path).await.status, StatusCode::OK);
}

#[tokio::test]
async fn publishing_with_the_servers_own_timers() {
    flow(Timers::Spawned).await;
}

#[tokio::test]
async fn publishing_with_no_timers() {
    flow(Timers::Driven).await;
}

#[tokio::test]
async fn bad_variants_are_refused() {
    let h = harness().await;
    let me = h.create("Maker").await;

    // A file whose script does not hash to its hash.
    let good = portable::port(&plain_king(), None, None, PortScript::Builtin).unwrap();
    let mut bad = serde_json::to_value(&good).unwrap();
    bad["script"]["hash"] = json!("00".repeat(32));
    let r = publish(&h, &me, json!({"bundle": bad, "visibility": "public"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());

    // A record the game would clean up.
    let mut v = plain_king();
    v.set(SettingId(19), 5).unwrap();
    let r = publish(&h, &me, json!({"bundle": portable::export(&v), "visibility": "public"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());

    // A name the game cannot hold.
    let hex: String = plain_king().bytes().iter().map(|b| format!("{b:02x}")).collect();
    let r = publish(&h, &me, json!({"name": "a|b", "record_hex": hex, "visibility": "public"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
    let r = publish(&h, &me, json!({"name": "Hill", "record_hex": "00", "visibility": "public"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
    // A record with no script and a script hash.
    let r = publish(&h, &me, json!({
        "name": "Hill", "record_hex": hex, "script_hash": slayer_hash(), "visibility": "public"
    }))
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
    // The fields way: a plain record publishes.
    let r = publish(&h, &me, json!({"name": "Hill", "description": "Up", "record_hex": hex, "visibility": "public"}))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());

    // A scripted record needs its script, and the script must be the one
    // the prefix names and ride the record's engine.
    let scripted = portable::port(&plain_king(), None, None, PortScript::Builtin)
        .unwrap()
        .variant()
        .unwrap();
    let shex: String = scripted.bytes().iter().map(|b| format!("{b:02x}")).collect();
    let r = publish(&h, &me, json!({"name": "Hill", "record_hex": shex, "visibility": "public"})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
    let r = publish(&h, &me, json!({
        "name": "Hill", "record_hex": shex, "script_hash": slayer_hash(), "visibility": "public"
    }))
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
    let king = rules::compile(rules::builtin::KING).unwrap().id().to_string();
    let r = publish(&h, &me, json!({
        "name": "Hill", "record_hex": shex, "script_hash": king, "visibility": "public"
    }))
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    assert_eq!(r.json()["script_name"], "King of the Hill");

    // Options the script does not declare, or out of range.
    for options in [json!({"nope": 1}), json!({"kill_points": 9}), json!({"kill_points": "x"})] {
        let r = send(
            &h,
            Method::POST,
            "/v1/variants/from-script",
            json!({"script_hash": slayer_hash(), "name": "X", "options": options}),
            Some(&me),
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{options}: {}", r.error());
    }
    let r = send(
        &h,
        Method::POST,
        "/v1/variants/from-script",
        json!({"script_hash": "ef".repeat(32), "name": "X"}),
        Some(&me),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
}

#[tokio::test]
async fn a_file_is_built_with_no_account() {
    let h = harness().await;
    let r = send(
        &h,
        Method::POST,
        "/v1/variants/bundle",
        json!({"source": rules::builtin::SLAYER, "name": "Fists", "options": {"kill_points": 2}}),
        None,
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let v = r.json();
    assert_eq!(v["file_name"], "Fists.h2variant");
    let b = portable::read(&v["bundle"].to_string()).unwrap();
    assert_eq!(b.id(), v["id"]);
    assert!(b.author.is_none());
    assert_eq!(b.script.as_ref().unwrap().hash, slayer_hash());
    assert_eq!(b.variant().unwrap().ext().script.options, 1 | 1 << 3);

    // The check report lists the options a variant form offers.
    let r = send(&h, Method::POST, "/v1/rules/check", json!({"source": rules::builtin::SLAYER}), None)
        .await;
    let o = &r.json()["options"];
    assert_eq!(o[0]["name"], "kill_points");
    assert_eq!(o[0]["kind"], "int");
    assert_eq!(o[0]["lo"], 1);
    assert_eq!(o[1]["kind"], "bool");
    assert_eq!(o[1]["default"], 1);
}
