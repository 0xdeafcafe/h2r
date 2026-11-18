//! Friends (docs/live/live-api.md, Friends): requests on the account streams,
//! the answers, and the friends read, end to end through the router.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use live_proto::{lxm, streams, types};
use serde_json::{Value, json};

/// A signed friend event by `user`, with `data`, on their account stream.
fn friend_event(h: &Harness, user: &User, kind: &str, data: Value) -> String {
    h.event(user, &streams::account(user.xuid), kind, data)
}

/// `tag`'s friends read as `user` (the account itself, so pending requests
/// are in it: they show to no one else but admins).
async fn own_friends(h: &Harness, user: &User, tag: &str) -> Value {
    let r = h
        .call(
            Method::GET,
            &format!("/v1/accounts/{tag}/friends"),
            &[h.bearer(user, lxm::READ)],
            axum::body::Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    r.json()
}

#[tokio::test]
async fn requests_are_directional_until_answered() {
    let h = harness().await;
    let a = h.create("Alpha").await;
    let b = h.create("Bravo").await;

    // Alpha asks Bravo: written by Alpha, on Alpha's account stream.
    let r = h
        .post_event(
            &a,
            friend_event(
                &h,
                &a,
                types::FRIEND_REQUESTED,
                json!({"from": a.xuid, "to": b.xuid}),
            ),
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );

    // Pending in and out, on the two sides of it.
    let alpha = own_friends(&h, &a, "Alpha").await;
    assert_eq!(alpha["requests_out"][0]["gamertag"], "Bravo");
    assert_eq!(alpha["requests_out"][0]["xuid"], b.xuid.to_string());
    assert_eq!(alpha["requests_in"], json!([]));
    let bravo = own_friends(&h, &b, "Bravo").await;
    assert_eq!(bravo["requests_in"][0]["gamertag"], "Alpha");
    assert_eq!(bravo["requests_out"], json!([]));
    assert_eq!(bravo["friends"], json!([]));

    // While pending, no second request fits in either direction.
    for (user, data) in [
        (&a, json!({"from": a.xuid, "to": b.xuid})),
        (&b, json!({"from": b.xuid, "to": a.xuid})),
    ] {
        let r = h
            .post_event(
                user,
                friend_event(&h, user, types::FRIEND_REQUESTED, data.clone()),
            )
            .await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{data} -> {}", r.error());
    }
    // A request to oneself or to a missing account is invalid.
    for (user, data) in [
        (&a, json!({"from": a.xuid, "to": a.xuid})),
        (&a, json!({"from": a.xuid, "to": "0009ffffffffffff"})),
    ] {
        let r = h
            .post_event(
                user,
                friend_event(&h, user, types::FRIEND_REQUESTED, data.clone()),
            )
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{data} -> {}", r.error());
    }

    // The request is answered on the recipient's account only: answering
    // on the requester's own stream is invalid...
    let r = h
        .post_event(
            &a,
            friend_event(
                &h,
                &a,
                types::FRIEND_ACCEPTED,
                json!({"from": a.xuid, "to": b.xuid}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
    let r = h
        .post_event(
            &b,
            friend_event(
                &h,
                &b,
                types::FRIEND_ACCEPTED,
                json!({"from": b.xuid, "to": a.xuid}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());

    // Bravo accepts: mutual friends, nothing pending, on both sides.
    let r = h
        .post_event(
            &b,
            friend_event(
                &h,
                &b,
                types::FRIEND_ACCEPTED,
                json!({"from": a.xuid, "to": b.xuid}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    for (tag, user) in [("Alpha", &a), ("Bravo", &b)] {
        let doc = own_friends(&h, user, tag).await;
        assert_eq!(doc["requests_in"], json!([]), "{tag}");
        assert_eq!(doc["requests_out"], json!([]), "{tag}");
        assert_eq!(doc["friends"].as_array().map(Vec::len), Some(1), "{tag}");
    }

    // Answering it again finds nothing pending.
    let r = h
        .post_event(
            &b,
            friend_event(
                &h,
                &b,
                types::FRIEND_ACCEPTED,
                json!({"from": a.xuid, "to": b.xuid}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn friends_read_carries_presence() {
    let h = harness().await;
    let a = h.create("Alpha").await;
    let b = h.create("Bravo").await;
    assert_eq!(h.sign_in(&a).await.status, StatusCode::CREATED);
    h.post_event(
        &b,
        friend_event(
            &h,
            &b,
            types::FRIEND_REQUESTED,
            json!({"from": b.xuid, "to": a.xuid}),
        ),
    )
    .await;
    h.post_event(
        &a,
        friend_event(
            &h,
            &a,
            types::FRIEND_ACCEPTED,
            json!({"from": b.xuid, "to": a.xuid}),
        ),
    )
    .await;

    // Alpha is signed in, so Bravo sees the friendship with presence.
    let bravo = h.get("/v1/accounts/Bravo/friends").await.json();
    let friend = &bravo["friends"][0];
    assert_eq!(friend["gamertag"], "Alpha");
    assert_eq!(friend["xuid"], a.xuid.to_string());
    assert_eq!(friend["presence"]["state"], "online");
    assert!(
        friend["friends_since"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "friends_since is the FriendAccepted event's at"
    );

    // Offline is the absence of presence: Alpha sees Bravo without any.
    let alpha = h.get("/v1/accounts/Alpha/friends").await.json();
    assert_eq!(alpha["friends"][0]["gamertag"], "Bravo");
    assert_eq!(alpha["friends"][0]["presence"], Value::Null);

    // An unknown gamertag is a 404, an invalid one too.
    assert_eq!(
        h.get("/v1/accounts/nobody/friends").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.get("/v1/accounts/1%20bad/friends").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn declining_and_removing() {
    let h = harness().await;
    let a = h.create("Alpha").await;
    let b = h.create("Bravo").await;
    let c = h.create("Charlie").await;

    // A declined request simply disappears.
    h.post_event(
        &a,
        friend_event(
            &h,
            &a,
            types::FRIEND_REQUESTED,
            json!({"from": a.xuid, "to": b.xuid}),
        ),
    )
    .await;
    let r = h
        .post_event(
            &b,
            friend_event(
                &h,
                &b,
                types::FRIEND_DECLINED,
                json!({"from": a.xuid, "to": b.xuid}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let bravo = h.get("/v1/accounts/Bravo/friends").await.json();
    assert_eq!(bravo["requests_in"], json!([]));
    // And can be sent again.
    assert_eq!(
        h.post_event(
            &a,
            friend_event(
                &h,
                &a,
                types::FRIEND_REQUESTED,
                json!({"from": a.xuid, "to": b.xuid})
            ),
        )
        .await
        .status,
        StatusCode::CREATED
    );
    h.post_event(
        &b,
        friend_event(
            &h,
            &b,
            types::FRIEND_ACCEPTED,
            json!({"from": a.xuid, "to": b.xuid}),
        ),
    )
    .await;

    // A removal needs a friendship or a pending request between the two.
    let r = h
        .post_event(
            &a,
            friend_event(
                &h,
                &a,
                types::FRIEND_REMOVED,
                json!({"by": a.xuid, "friend": c.xuid}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    // Either side removes; the friendship ends both ways.
    let r = h
        .post_event(
            &b,
            friend_event(
                &h,
                &b,
                types::FRIEND_REMOVED,
                json!({"by": b.xuid, "friend": a.xuid}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    for tag in ["Alpha", "Bravo"] {
        let doc = h.get(&format!("/v1/accounts/{tag}/friends")).await.json();
        assert_eq!(doc["friends"], json!([]), "{tag}");
    }

    // A pending request can also be cancelled, by either of the two.
    h.post_event(
        &c,
        friend_event(
            &h,
            &c,
            types::FRIEND_REQUESTED,
            json!({"from": c.xuid, "to": a.xuid}),
        ),
    )
    .await;
    let r = h
        .post_event(
            &c,
            friend_event(
                &h,
                &c,
                types::FRIEND_REMOVED,
                json!({"by": c.xuid, "friend": a.xuid}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let alpha = own_friends(&h, &a, "Alpha").await;
    assert_eq!(alpha["requests_in"], json!([]));
}

#[tokio::test]
async fn friend_writes_are_checked_like_other_events() {
    let h = harness().await;
    let a = h.create("Alpha").await;
    let b = h.create("Bravo").await;

    // Only the account's own stream: Alpha cannot write Bravo's stream.
    let jws = h.event(
        &a,
        &streams::account(b.xuid),
        types::FRIEND_REQUESTED,
        json!({"from": b.xuid, "to": a.xuid}),
    );
    let r = h.post_event(&a, jws).await;
    assert_eq!(
        (r.status, r.error().as_str()),
        (StatusCode::FORBIDDEN, "forbidden")
    );

    // A token for another method is not enough (the bearer path itself).
    let jws = friend_event(
        &h,
        &a,
        types::FRIEND_REQUESTED,
        json!({"from": a.xuid, "to": b.xuid}),
    );
    let r = h
        .call(
            Method::POST,
            "/v1/events",
            &[h.bearer(&a, lxm::GAMES_WRITE)],
            jws,
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // Reads need no token on a public server, and the events carry the
    // author's signature like every account event.
    let r = h
        .post_event(
            &a,
            friend_event(
                &h,
                &a,
                types::FRIEND_REQUESTED,
                json!({"from": a.xuid, "to": b.xuid}),
            ),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let stored = r.json();
    assert_eq!(stored["author"], a.author.as_str());
    assert_eq!(stored["stream"], format!("account-{}", a.xuid));
    assert_eq!(stored["type"], "FriendRequested");
    assert_eq!(
        own_friends(&h, &a, "Alpha").await["requests_out"][0]["gamertag"],
        "Bravo"
    );
}
