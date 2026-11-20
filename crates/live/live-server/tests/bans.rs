//! Account bans (docs/live/live-api.md "Bans"): an admin bans an account;
//! while it is in force every write token the account presents is refused
//! with `banned` (403) and reads still work; it ends when lifted, when its
//! end date passes or when a newer ban replaces it; only admins, the
//! operator and the player read it. Run with the server's own timers and
//! with none (the Worker's Durable Object), and replayed from the log by a
//! second server. Also: the public commendation counts.
mod common;
use axum::{body::Body, http::Method, http::StatusCode};
use common::*;
use live_proto::{Gamertag, bans, lxm};
use live_server::app::Timers;
use live_server::store::{EventStore, MemoryStore};
use serde_json::{Value, json};
use std::sync::Arc;
use std::time::Duration;

fn policy() -> live_server::Policy {
    live_server::Policy {
        admins: vec![Gamertag::new("Boss").unwrap()],
        ..Default::default()
    }
}

async fn ban(h: &Harness, who: &User, target: &User, body: Value) -> Resp {
    h.call(
        Method::POST,
        &format!("/v1/admin/bans/{}", target.xuid),
        &[h.bearer(who, lxm::REPORTS_MODERATE)],
        body.to_string(),
    )
    .await
}

async fn lift(h: &Harness, who: &User, target: &User, note: &str) -> Resp {
    h.call(
        Method::POST,
        &format!("/v1/admin/bans/{}/lift", target.xuid),
        &[h.bearer(who, lxm::REPORTS_MODERATE)],
        json!({ "note": note }).to_string(),
    )
    .await
}

async fn list(h: &Harness, who: &User, state: &str) -> Resp {
    h.call(
        Method::GET,
        &format!("/v1/admin/bans?state={state}"),
        &[h.bearer(who, lxm::REPORTS_MODERATE)],
        Body::empty(),
    )
    .await
}

async fn status(h: &Harness, reader: &User, target: &User) -> Resp {
    h.call(
        Method::GET,
        &format!("/v1/accounts/by-xuid/{}/ban", target.xuid),
        &[h.bearer(reader, lxm::READ)],
        Body::empty(),
    )
    .await
}

async fn report(h: &Harness, by: &User, target: &User, kind: &str) -> Resp {
    h.call(
        Method::POST,
        &format!("/v1/accounts/by-xuid/{}/report", target.xuid),
        &[h.bearer(by, lxm::REPORTS_WRITE)],
        json!({ "kind": kind }).to_string(),
    )
    .await
}

/// Ban events a reader finds in `/v1/events`.
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
        .filter(|e| e["type"].as_str().is_some_and(|t| bans::KINDS.contains(&t)))
        .count()
}

/// Every write a banned account tries, refused as `banned`.
async fn writes_refused(h: &Harness, u: &User) {
    let r = h.sign_in(u).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "sign in");
    assert_eq!(r.error(), "banned");
    let r = h.ping(u).await;
    assert_eq!(r.error(), "banned", "ping");
    for (path, scope, body) in [
        (
            "/v1/sessions/x/relay/ticket".to_string(),
            lxm::SESSIONS_RELAY,
            "",
        ),
        (
            format!("/v1/accounts/by-xuid/{}/report", u.xuid),
            lxm::REPORTS_WRITE,
            r#"{"kind":"gameplay"}"#,
        ),
    ] {
        let r = h
            .call(Method::POST, &path, &[h.bearer(u, scope)], body.to_string())
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}: {}", r.error());
        assert_eq!(r.error(), "banned", "{path}");
    }
}

async fn flow(timers: Timers) {
    let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
    let h = start_with(store.clone(), policy(), timers).await;
    let cheat = h.create("Cheat").await;
    let reporter = h.create("Reporter").await;
    let other = h.create("Other").await;
    let boss = h.create("Boss").await;

    // Signed in before the ban.
    assert_eq!(h.sign_in(&cheat).await.status, StatusCode::CREATED);
    let r = report(&h, &reporter, &cheat, "gameplay").await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let report_id = r.json()["position"].as_u64().unwrap();

    // What is refused.
    for (who, body, want) in [
        (&reporter, json!({"reason": "x"}), StatusCode::FORBIDDEN),
        (&boss, json!({"reason": "  "}), StatusCode::BAD_REQUEST),
        (&boss, json!({}), StatusCode::BAD_REQUEST),
        (
            &boss,
            json!({"reason": "x", "until": "tomorrow"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            &boss,
            json!({"reason": "x", "until": "2001-01-01T00:00:00Z"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            &boss,
            json!({"reason": "x", "report": 999_999}),
            StatusCode::BAD_REQUEST,
        ),
        (
            &boss,
            json!({"reason": "x", "extra": 1}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let r = ban(&h, who, &cheat, body.clone()).await;
        assert_eq!(r.status, want, "{body}: {}", r.error());
    }
    assert_eq!(
        ban(&h, &boss, &boss, json!({"reason": "x"})).await.status,
        StatusCode::FORBIDDEN,
        "admins can't be banned"
    );
    let r = h
        .call(
            Method::POST,
            "/v1/admin/bans/0009ffffffffffff",
            &[h.bearer(&boss, lxm::REPORTS_MODERATE)],
            json!({"reason": "x"}).to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(
        lift(&h, &boss, &cheat, "").await.status,
        StatusCode::CONFLICT,
        "nothing to lift"
    );
    // Nobody posts the events by hand.
    let forged = h.event(
        &boss,
        &bans::stream(cheat.xuid),
        bans::ACCOUNT_BANNED,
        json!({"target": cheat.xuid, "reason": "x"}),
    );
    assert_eq!(
        h.post_event(&boss, forged).await.status,
        StatusCode::FORBIDDEN
    );

    // A ban until lifted, answering the report.
    let r = ban(
        &h,
        &boss,
        &cheat,
        json!({"reason": "  aimbot  ", "report": report_id}),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let first = r.json()["position"].as_u64().unwrap();
    writes_refused(&h, &cheat).await;
    // Reads still work, and the player reads their notice.
    assert_eq!(
        h.call(
            Method::GET,
            "/v1/sessions",
            &[h.bearer(&cheat, lxm::READ)],
            Body::empty()
        )
        .await
        .status,
        StatusCode::OK
    );
    let s = status(&h, &cheat, &cheat).await;
    assert_eq!(s.status, StatusCode::OK, "{}", s.error());
    let s = s.json();
    assert_eq!(s["banned"], true);
    assert_eq!(s["ban"]["reason"], "aimbot");
    assert_eq!(s["ban"]["id"].as_u64(), Some(first));
    assert!(s["ban"].get("until").is_none());
    assert_eq!(status(&h, &boss, &cheat).await.json()["banned"], true);
    assert_eq!(
        status(&h, &other, &cheat).await.status,
        StatusCode::FORBIDDEN
    );
    // Others are not held back.
    assert_eq!(h.sign_in(&other).await.status, StatusCode::CREATED);

    // Who reads the events: admins, the operator and the player.
    assert_eq!(logged(&h, None).await, 0);
    assert_eq!(logged(&h, Some(&other)).await, 0);
    assert_eq!(logged(&h, Some(&cheat)).await, 1);
    assert_eq!(logged(&h, Some(&boss)).await, 1);

    // The admins' list.
    let l = list(&h, &boss, "active").await;
    assert_eq!(l.status, StatusCode::OK, "{}", l.error());
    let l = l.json();
    assert_eq!(l["items"][0]["target"]["gamertag"], "Cheat");
    assert_eq!(l["items"][0]["state"], "active");
    assert_eq!(l["items"][0]["by"], "Boss");
    assert_eq!(l["items"][0]["report"].as_u64(), Some(report_id));
    assert_eq!(
        list(&h, &cheat, "active").await.status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        list(&h, &boss, "nope").await.status,
        StatusCode::BAD_REQUEST
    );

    // A new ban replaces it: this one ends in two seconds.
    let until = time::OffsetDateTime::from_unix_timestamp(now() + 2)
        .unwrap()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    let r = ban(
        &h,
        &boss,
        &cheat,
        json!({"reason": "cool off", "until": until}),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let all = list(&h, &boss, "all").await.json();
    let states: Vec<_> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["state"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(states, ["active", "replaced"]);
    assert_eq!(all["items"][0]["until"], until.as_str());
    let r = h.sign_in(&cheat).await;
    assert!(r.error() == "banned" && r.json()["message"].as_str().unwrap().contains(&until));

    // It ends by itself.
    while now()
        <= time::OffsetDateTime::parse(&until, &time::format_description::well_known::Rfc3339)
            .unwrap()
            .unix_timestamp()
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(h.sign_in(&cheat).await.status, StatusCode::CREATED);
    assert_eq!(status(&h, &cheat, &cheat).await.json()["banned"], false);
    assert_eq!(list(&h, &boss, "active").await.json()["items"], json!([]));
    assert_eq!(
        list(&h, &boss, "all").await.json()["items"][0]["state"],
        "expired"
    );

    // Banned again, then lifted by the operator.
    let r = ban(&h, &boss, &cheat, json!({"reason": "again"})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    assert_eq!(h.ping(&cheat).await.error(), "banned");
    let op = format!(
        "Bearer {}",
        h.token(&h.operator, "server:live.test", lxm::REPORTS_MODERATE)
    );
    let r = h
        .call(
            Method::POST,
            &format!("/v1/admin/bans/{}/lift", cheat.xuid),
            &[("authorization", op)],
            json!({"note": "appeal upheld"}).to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    assert_eq!(
        lift(&h, &boss, &cheat, "").await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(h.sign_in(&cheat).await.status, StatusCode::CREATED);
    let all = list(&h, &boss, "all").await.json();
    assert_eq!(all["items"][0]["state"], "lifted");
    assert_eq!(all["items"][0]["lifted"]["note"], "appeal upheld");
    assert!(
        all["items"][0]["lifted"].get("by").is_none(),
        "the operator"
    );

    // A second server on the same log reads the same bans.
    let h2 = start_with(store.clone(), policy(), timers).await;
    assert_eq!(list(&h2, &boss, "all").await.json()["items"], all["items"]);
    let r = ban(&h2, &boss, &cheat, json!({"reason": "third"})).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let h3 = start_with(store, policy(), timers).await;
    assert_eq!(h3.sign_in(&cheat).await.error(), "banned", "after replay");
}

#[tokio::test]
async fn bans_with_the_servers_own_timers() {
    flow(Timers::Spawned).await;
}

#[tokio::test]
async fn bans_with_no_timers() {
    flow(Timers::Driven).await;
}

#[tokio::test]
async fn commendations_are_counted_in_public() {
    let h = start(Arc::new(MemoryStore::new()), policy()).await;
    let star = h.create("Star").await;
    let a = h.create("Alpha").await;
    let b = h.create("Bravo").await;
    for (by, kind) in [
        (&a, "good_attitude"),
        (&b, "good_attitude"),
        (&a, "good_session"),
        (&a, "good_session"),
        (&b, "harassment"),
    ] {
        let r = report(&h, by, &star, kind).await;
        assert!(r.status.is_success(), "{kind}: {}", r.error());
    }
    for who in ["Star", "star", &star.xuid.to_string()] {
        let r = h.get(&format!("/v1/accounts/{who}/commendations")).await;
        assert_eq!(r.status, StatusCode::OK, "{who}: {}", r.error());
        assert_eq!(
            r.json(),
            json!({"xuid": star.xuid, "good_attitude": 2, "good_session": 1})
        );
    }
    assert_eq!(
        h.get("/v1/accounts/Nobody/commendations").await.status,
        StatusCode::NOT_FOUND
    );
}
