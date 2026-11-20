//! The mute list (docs/live/live-api.md "Mute list"): an account mutes and
//! unmutes players; repeats change nothing; the list is capped at the
//! game's 250; only the account itself changes it, and only it, admins and
//! the operator read it, in the list or the log. Run with the server's own
//! timers and with none (the Worker's Durable Object), and replayed from
//! the log by a second server.
mod common;
use axum::{body::Body, http::Method, http::StatusCode};
use common::*;
use live_proto::{Gamertag, lxm, mutes};
use live_server::app::Timers;
use live_server::store::{EventStore, MemoryStore};
use serde_json::Value;
use std::sync::Arc;

fn policy() -> live_server::Policy {
    live_server::Policy {
        admins: vec![Gamertag::new("Boss").unwrap()],
        ..Default::default()
    }
}

async fn set(h: &Harness, who: &User, owner: &User, target: &str, mute: bool) -> Resp {
    h.call(
        if mute { Method::PUT } else { Method::DELETE },
        &format!("/v1/accounts/by-xuid/{}/mutes/{target}", owner.xuid),
        &[h.bearer(who, lxm::MUTES_WRITE)],
        Body::empty(),
    )
    .await
}

async fn list(h: &Harness, reader: &User, owner: &User) -> Resp {
    h.call(
        Method::GET,
        &format!("/v1/accounts/by-xuid/{}/mutes", owner.xuid),
        &[h.bearer(reader, lxm::READ)],
        Body::empty(),
    )
    .await
}

fn tags(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["gamertag"].as_str().unwrap().to_string())
        .collect()
}

/// Mute events a reader finds in `/v1/events`.
async fn logged(h: &Harness, reader: Option<&User>) -> usize {
    let headers: Vec<_> = reader.map(|u| h.bearer(u, lxm::READ)).into_iter().collect();
    let r = h
        .call(Method::GET, "/v1/events?limit=500", &headers, Body::empty())
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["type"]
                .as_str()
                .is_some_and(|t| mutes::KINDS.contains(&t))
        })
        .count()
}

async fn flow(timers: Timers) {
    let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
    let h = start_with(store.clone(), policy(), timers).await;
    let me = h.create("Muter").await;
    let loud = h.create("Loud").await;
    let rude = h.create("Rude").await;
    let other = h.create("Other").await;
    let boss = h.create("Boss").await;

    let r = set(&h, &me, &me, &loud.xuid.to_string(), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    assert_eq!(r.json()["changed"], true);
    let r = set(&h, &me, &me, &loud.xuid.to_string(), true).await;
    assert_eq!(r.status, StatusCode::OK, "a repeat");
    assert_eq!(r.json()["changed"], false);
    let r = set(&h, &me, &me, &rude.xuid.to_string(), true).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());

    // What is refused.
    let mine = me.xuid.to_string();
    for (who, owner, target, want) in [
        (&me, &me, mine.as_str(), StatusCode::BAD_REQUEST),
        (&me, &me, "0009ffffffffffff", StatusCode::NOT_FOUND),
        (&me, &me, "nope", StatusCode::NOT_FOUND),
        (&other, &me, "0009ffffffffffff", StatusCode::FORBIDDEN),
        (&boss, &me, mine.as_str(), StatusCode::FORBIDDEN),
    ] {
        let r = set(&h, who, owner, target, true).await;
        assert_eq!(r.status, want, "{target}: {}", r.error());
    }
    let r = h
        .call(
            Method::PUT,
            &format!("/v1/accounts/by-xuid/{}/mutes/{}", me.xuid, other.xuid),
            &[h.bearer(&me, lxm::EVENTS_WRITE)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "the wrong scope");
    // Nobody posts the events by hand.
    let forged = h.event(
        &me,
        &mutes::stream(me.xuid),
        mutes::PLAYER_MUTED,
        serde_json::json!({"by": me.xuid, "target": other.xuid}),
    );
    assert_eq!(
        h.post_event(&me, forged).await.status,
        StatusCode::FORBIDDEN
    );

    // Who reads it.
    let l = list(&h, &me, &me).await;
    assert_eq!(l.status, StatusCode::OK, "{}", l.error());
    assert_eq!(tags(&l.json()), ["Loud", "Rude"]);
    assert_eq!(l.json()["items"][0]["xuid"], loud.xuid.to_string());
    assert_eq!(tags(&list(&h, &boss, &me).await.json()), ["Loud", "Rude"]);
    assert_eq!(list(&h, &loud, &me).await.status, StatusCode::FORBIDDEN);
    assert_eq!(logged(&h, None).await, 0);
    assert_eq!(logged(&h, Some(&loud)).await, 0);
    assert_eq!(logged(&h, Some(&me)).await, 2);
    assert_eq!(logged(&h, Some(&boss)).await, 2);

    // Unmute, and an unmute of someone not muted changes nothing.
    let r = set(&h, &me, &me, &loud.xuid.to_string(), false).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    assert_eq!(r.json()["changed"], true);
    let r = set(&h, &me, &me, &loud.xuid.to_string(), false).await;
    assert_eq!(r.json()["changed"], false);
    assert_eq!(tags(&list(&h, &me, &me).await.json()), ["Rude"]);

    // A second server on the same log reads the same list.
    let h2 = start_with(store, policy(), timers).await;
    assert_eq!(tags(&list(&h2, &me, &me).await.json()), ["Rude"]);
    let r = set(&h2, &me, &me, &rude.xuid.to_string(), true).await;
    assert_eq!(r.json()["changed"], false, "still muted after replay");
}

#[tokio::test]
async fn mutes_with_the_servers_own_timers() {
    flow(Timers::Spawned).await;
}

#[tokio::test]
async fn mutes_with_no_timers() {
    flow(Timers::Driven).await;
}

#[tokio::test]
async fn a_mute_list_holds_the_games_250() {
    let h = start(
        Arc::new(MemoryStore::new()),
        live_server::Policy {
            write_rate: (1000.0, 1000),
            create_rate: (1000.0, 1000),
            ..policy()
        },
    )
    .await;
    let me = h.create("Muter").await;
    let mut others = Vec::new();
    for i in 0..=mutes::MAX {
        others.push(h.create(&format!("Player {i}")).await);
    }
    for o in &others[..mutes::MAX] {
        let r = set(&h, &me, &me, &o.xuid.to_string(), true).await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    }
    let last = others[mutes::MAX].xuid.to_string();
    let r = set(&h, &me, &me, &last, true).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.error());
    let r = set(&h, &me, &me, &others[0].xuid.to_string(), false).await;
    assert_eq!(r.json()["changed"], true);
    let r = set(&h, &me, &me, &last, true).await;
    assert_eq!(r.status, StatusCode::CREATED, "room again");
}
