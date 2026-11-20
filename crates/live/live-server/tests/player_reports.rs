//! Player reports (docs/live/live-api.md "Player reports"): an account
//! reports another; repeats of an open report are folded into it; reports
//! are rate limited; only admins and the operator read them, in the queue
//! or the log; admins dismiss them or close them with a note. Run with the
//! server's own timers and with none (the Worker's Durable Object), and
//! replayed from the log by a second server.
mod common;
use axum::{body::Body, http::Method, http::StatusCode};
use common::*;
use live_proto::{Gamertag, lxm, reports};
use live_server::app::Timers;
use live_server::store::{EventStore, MemoryStore};
use serde_json::{Value, json};
use std::sync::Arc;

fn policy() -> live_server::Policy {
    live_server::Policy {
        admins: vec![Gamertag::new("Boss").unwrap()],
        ..Default::default()
    }
}

async fn report(h: &Harness, by: &User, target: &str, body: Value) -> Resp {
    h.call(
        Method::POST,
        &format!("/v1/accounts/by-xuid/{target}/report"),
        &[
            h.bearer(by, lxm::REPORTS_WRITE),
            ("content-type", "application/json".into()),
        ],
        body.to_string(),
    )
    .await
}

async fn queue(h: &Harness, who: &User, state: &str) -> Resp {
    h.call(
        Method::GET,
        &format!("/v1/admin/reports?state={state}"),
        &[h.bearer(who, lxm::REPORTS_MODERATE)],
        Body::empty(),
    )
    .await
}

async fn review(h: &Harness, who: &User, id: u64, action: &str, note: &str) -> Resp {
    h.call(
        Method::POST,
        &format!("/v1/admin/reports/{id}/{action}"),
        &[h.bearer(who, lxm::REPORTS_MODERATE)],
        json!({ "note": note }).to_string(),
    )
    .await
}

fn kinds(v: &Value) -> Vec<(String, String)> {
    v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["target"]["gamertag"].as_str().unwrap().to_string(),
                i["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// Private report events a reader finds in `/v1/events`.
async fn logged(h: &Harness, reader: Option<(&live_proto::sign::KeyPair, &str)>) -> Vec<String> {
    let headers: Vec<_> = reader
        .map(|(k, iss)| {
            (
                "authorization",
                format!("Bearer {}", h.token(k, iss, lxm::READ)),
            )
        })
        .into_iter()
        .collect();
    let r = h
        .call(Method::GET, "/v1/events?limit=200", &headers, Body::empty())
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    r.json()["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["type"].as_str())
        .filter(|t| reports::KINDS.contains(t))
        .map(str::to_string)
        .collect()
}

async fn flow(timers: Timers) {
    let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
    let h = start_with(store.clone(), policy(), timers).await;
    let reporter = h.create("Reporter").await;
    let target = h.create("Target").await;
    let other = h.create("Other").await;
    let boss = h.create("Boss").await;
    let t = target.xuid.to_string();

    // A complaint, then the same one again: folded into the first.
    let first = report(
        &h,
        &reporter,
        &t,
        json!({"kind": "harassment", "game_id": "0123456789abcdef"}),
    )
    .await;
    assert_eq!(first.status, StatusCode::CREATED, "{}", first.error());
    let id = first.json()["position"].as_u64().unwrap();
    assert_eq!(first.json()["duplicate"], false);
    let again = report(&h, &reporter, &t, json!({"kind": "harassment"})).await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.error());
    assert_eq!(again.json()["position"].as_u64(), Some(id));
    assert_eq!(again.json()["duplicate"], true);
    // Another kind, another reporter, a commendation: separate reports.
    for (by, kind) in [
        (&reporter, "gamertag"),
        (&other, "harassment"),
        (&reporter, "good_attitude"),
    ] {
        let r = report(&h, by, &t, json!({"kind": kind, "reason": "  words  "})).await;
        assert_eq!(r.status, StatusCode::CREATED, "{kind}: {}", r.error());
    }

    // What is refused.
    let me = reporter.xuid.to_string();
    for (body, to, status) in [
        (
            json!({"kind": "lewdness"}),
            me.as_str(),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind": "lewdness"}),
            "0009ffffffffffff",
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"kind": "lewdness"}),
            "not-a-xuid",
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"kind": "cheating"}),
            t.as_str(),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind": "lewdness", "extra": 1}),
            t.as_str(),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind": "lewdness", "game_id": "../x"}),
            t.as_str(),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"kind": "lewdness", "reason": "x".repeat(501)}),
            t.as_str(),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let r = report(&h, &reporter, to, body.clone()).await;
        assert_eq!(r.status, status, "{body} to {to}: {}", r.error());
    }
    // The wrong scope, and the operator (reports come from accounts).
    let r = h
        .call(
            Method::POST,
            &format!("/v1/accounts/by-xuid/{t}/report"),
            &[h.bearer(&reporter, lxm::CONTENT_WRITE)],
            json!({"kind": "lewdness"}).to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.error());
    let op = format!(
        "Bearer {}",
        h.token(&h.operator, "server:live.test", lxm::REPORTS_WRITE)
    );
    let r = h
        .call(
            Method::POST,
            &format!("/v1/accounts/by-xuid/{t}/report"),
            &[("authorization", op)],
            json!({"kind": "lewdness"}).to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.error());
    // Nobody posts the events by hand.
    let forged = h.event(
        &reporter,
        &reports::stream(target.xuid),
        reports::PLAYER_REPORTED,
        json!({"target": target.xuid, "by": reporter.xuid, "kind": "lewdness"}),
    );
    assert_eq!(
        h.post_event(&reporter, forged).await.status,
        StatusCode::FORBIDDEN
    );

    // Who reads them: admins and the operator only.
    for (who, reader) in [
        ("anonymous", None),
        ("reporter", Some((&reporter.key, reporter.author.as_str()))),
        ("target", Some((&target.key, target.author.as_str()))),
        ("other", Some((&other.key, other.author.as_str()))),
    ] {
        assert_eq!(logged(&h, reader).await, Vec::<String>::new(), "{who}");
    }
    assert_eq!(
        logged(&h, Some((&boss.key, boss.author.as_str())))
            .await
            .len(),
        4
    );
    assert_eq!(
        logged(&h, Some((&h.operator, "server:live.test")))
            .await
            .len(),
        4
    );
    for who in [&reporter, &target] {
        assert_eq!(queue(&h, who, "open").await.status, StatusCode::FORBIDDEN);
    }

    // The queue: open complaints, newest first, with the player's record.
    let q = queue(&h, &boss, "open").await;
    assert_eq!(q.status, StatusCode::OK, "{}", q.error());
    let q = q.json();
    assert_eq!(
        kinds(&q),
        [
            ("Target", "harassment"),
            ("Target", "gamertag"),
            ("Target", "harassment")
        ]
        .map(|(a, b)| (a.to_string(), b.to_string()))
    );
    let oldest = &q["items"][2];
    assert_eq!(oldest["id"].as_u64(), Some(id));
    assert_eq!(oldest["by"]["gamertag"], "Reporter");
    assert_eq!(oldest["game_id"], "0123456789abcdef");
    assert_eq!(oldest["negative"], true);
    assert!(oldest.get("review").is_none());
    assert_eq!(q["items"][0]["reason"], "words", "trimmed");
    let record = &q["players"][&t];
    assert_eq!(
        (
            record["complaints"].as_u64(),
            record["open"].as_u64(),
            record["commendations"].as_u64()
        ),
        (Some(3), Some(3), Some(1))
    );
    let all = queue(&h, &boss, "all").await.json();
    assert_eq!(all["items"].as_array().unwrap().len(), 4);
    assert_eq!(all["items"][0]["kind"], "good_attitude");
    assert_eq!(all["items"][0]["negative"], false);
    assert_eq!(
        queue(&h, &boss, "nope").await.status,
        StatusCode::BAD_REQUEST
    );

    // Reviews.
    let commendation = all["items"][0]["id"].as_u64().unwrap();
    assert_eq!(
        review(&h, &boss, commendation, "dismiss", "").await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        review(&h, &boss, id, "note", "  ").await.status,
        StatusCode::BAD_REQUEST,
        "a note needs text"
    );
    assert_eq!(
        review(&h, &boss, id, "ban", "").await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        review(&h, &boss, 999_999, "dismiss", "").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        review(&h, &reporter, id, "dismiss", "").await.status,
        StatusCode::FORBIDDEN
    );
    let r = review(&h, &boss, id, "note", "warned in chat").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    assert_eq!(
        review(&h, &boss, id, "dismiss", "").await.status,
        StatusCode::CONFLICT
    );
    let gamertag = q["items"][1]["id"].as_u64().unwrap();
    assert_eq!(
        review(&h, &boss, gamertag, "dismiss", "").await.status,
        StatusCode::OK
    );

    let open = queue(&h, &boss, "open").await.json();
    assert_eq!(kinds(&open).len(), 1);
    let closed = queue(&h, &boss, "closed").await.json();
    assert_eq!(closed["items"].as_array().unwrap().len(), 2);
    let noted = closed["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"].as_u64() == Some(id))
        .unwrap();
    assert_eq!(noted["review"]["action"], "note");
    assert_eq!(noted["review"]["note"], "warned in chat");
    assert_eq!(noted["review"]["by"], "Boss");
    let record = &closed["players"][&t];
    assert_eq!(record["open"].as_u64(), Some(1));
    assert_eq!(record["notes"][0]["note"], "warned in chat");
    assert_eq!(record["notes"][0]["report"].as_u64(), Some(id));

    // Once reviewed, the same complaint may be filed again.
    let r = report(&h, &reporter, &t, json!({"kind": "harassment"})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let refiled = r.json()["position"].as_u64().unwrap();
    // The commendation stays filed once.
    let r = report(&h, &reporter, &t, json!({"kind": "good_attitude"})).await;
    assert_eq!(r.json()["duplicate"], true);

    // A second server on the same log reads the same reports and reviews.
    let h2 = start_with(store, policy(), timers).await;
    let replayed = queue(&h2, &boss, "all").await.json();
    assert_eq!(
        replayed["items"],
        queue(&h, &boss, "all").await.json()["items"]
    );
    let r = report(&h2, &reporter, &t, json!({"kind": "harassment"})).await;
    assert_eq!(
        r.json()["position"].as_u64(),
        Some(refiled),
        "still open after replay"
    );
}

#[tokio::test]
async fn reports_with_the_servers_own_timers() {
    flow(Timers::Spawned).await;
}

#[tokio::test]
async fn reports_with_no_timers() {
    flow(Timers::Driven).await;
}

#[tokio::test]
async fn reports_are_rate_limited_per_reporter() {
    let h = start(Arc::new(MemoryStore::new()), policy()).await;
    let reporter = h.create("Reporter").await;
    let a = h.create("Alpha").await;
    let b = h.create("Bravo").await;
    let kinds = [
        "gamertag",
        "gameplay",
        "screaming",
        "harassment",
        "lewdness",
        "good_attitude",
        "good_session",
        "type_10",
        "type_11",
        "type_12",
    ];
    for kind in kinds {
        let r = report(&h, &reporter, &a.xuid.to_string(), json!({"kind": kind})).await;
        assert_eq!(r.status, StatusCode::CREATED, "{kind}: {}", r.error());
    }
    // A repeat costs nothing; a new report is over the limit.
    let r = report(
        &h,
        &reporter,
        &a.xuid.to_string(),
        json!({"kind": "gameplay"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = report(
        &h,
        &reporter,
        &b.xuid.to_string(),
        json!({"kind": "gameplay"}),
    )
    .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.error());
    // Others are not held back by it.
    let r = report(&h, &a, &b.xuid.to_string(), json!({"kind": "gameplay"})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
}
