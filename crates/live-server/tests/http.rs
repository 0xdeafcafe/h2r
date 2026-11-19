//! The HTTP API end to end through the router (tower `oneshot`), over the
//! in-memory store; the Postgres variants at the bottom run when
//! `H2_LIVE_TEST_DB` is set.

mod common;

use axum::body::Body;
use axum::http::{Method, StatusCode, header};
use common::*;
use live_proto::sign::{self, AccountRequest, EventBody, Invite, KeyPair, issue_token};
use live_proto::{Author, lxm, streams, types};
use live_server::Policy;
use live_server::store::MemoryStore;
use serde_json::{Value, json};
use std::sync::Arc;

#[tokio::test]
async fn api_reads_share_a_per_ip_limit_on_native_and_worker() {
    use axum::http::Request;
    use std::net::SocketAddr;
    use tower::ServiceExt;

    for edge in [false, true] {
        let policy = Policy {
            read_rate: (0.0, 2),
            ..Policy::default()
        };
        if edge {
            policy.public.set_edge("https://live.test");
        }
        let h = start(Arc::new(MemoryStore::new()), policy).await;
        let request = |method, path, port, distinct| {
            let ip = if distinct { "192.0.2.2" } else { "192.0.2.1" };
            let (peer, cf_ip) = if edge {
                (None, ip.to_owned())
            } else {
                let peer = format!("{ip}:{port}").parse::<SocketAddr>().unwrap();
                // On a direct native connection the header cannot select the bucket.
                (Some(peer), format!("203.0.113.{port}"))
            };
            let mut req = Request::builder()
                .method(method)
                .uri(path)
                .header("cf-connecting-ip", cf_ip)
                .body(Body::empty())
                .unwrap();
            if let Some(peer) = peer {
                req.extensions_mut().insert(live_server::ConnectInfo(peer));
            }
            let app = h.app.clone();
            async move { app.oneshot(req).await.unwrap().status() }
        };

        assert_eq!(
            request(Method::GET, "/v1/accounts", 1, false).await,
            StatusCode::OK
        );
        assert_eq!(
            request(Method::HEAD, "/v1/presence", 2, false).await,
            StatusCode::OK
        );
        assert_eq!(
            request(Method::GET, "/v1/content/missing", 3, false).await,
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            request(Method::GET, "/v1/accounts", 4, true).await,
            StatusCode::OK
        );
    }
}

#[tokio::test]
async fn accounts_are_created_and_readable() {
    let h = harness().await;
    let user = h.create("Sneaky Pete").await;
    assert_eq!(user.xuid.0 >> 48, 0x0009);
    assert_eq!(user.author, format!("acct:{}@live.test", user.xuid));

    let list = h.get("/v1/accounts").await.json();
    assert_eq!(list["items"][0]["gamertag"], "Sneaky Pete");
    assert_eq!(list["items"][0]["online"], false);
    assert_eq!(list["next"], Value::Null);
    // GamertagReserved, AccountCreated, KeyAdded.
    assert_eq!(list["position"], 3);

    // Lookup is case-insensitive; display case is kept.
    let one = h.get("/v1/accounts/sneaky%20PETE").await;
    assert_eq!(one.status, StatusCode::OK);
    assert_eq!(one.json()["gamertag"], "Sneaky Pete");
    assert_eq!(one.json()["recent_games"], json!([]));
    assert_eq!(
        h.get("/v1/accounts/nobody").await.status,
        StatusCode::NOT_FOUND
    );

    let keys = h.get("/v1/accounts/Sneaky%20Pete/keys").await.json();
    assert_eq!(keys["keys"][0]["kid"], user.key.kid());
    assert_eq!(keys["keys"][0]["jwk"]["x"], user.key.public().jwk().x);
    assert_eq!(keys["keys"][0]["valid_from"], 3);
    assert_eq!(keys["keys"][0]["valid_to"], Value::Null);
    let by_xuid = h
        .get(&format!("/v1/accounts/by-xuid/{}/keys", user.xuid))
        .await
        .json();
    assert_eq!(by_xuid["keys"], keys["keys"]);
    assert_eq!(
        h.get("/v1/accounts/by-xuid/0009000000000000/keys")
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    let wk = h.get("/.well-known/h2live.json").await.json();
    assert_eq!(wk["id"], "live.test");
    assert_eq!(wk["api"], "/v1");
    assert_eq!(wk["ingest_frame_versions"], json!([8]));
    assert_eq!(wk["federation"], false);
    assert_eq!(wk["keys"][0]["kid"], h.operator.kid());
    assert_eq!(
        h.get("/healthz").await.json(),
        json!({"ok": true, "position": 3})
    );

    // The server signed its events, and the KeyAdded carries the user's
    // signed request as proof.
    let events = h.get("/v1/events?from=0").await.json();
    let items = events["items"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["GamertagReserved", "AccountCreated", "KeyAdded"]
    );
    for e in items {
        assert_eq!(e["author"], "server:live.test");
        let opened = sign::SignedEvent::open(e["jws"].as_str().unwrap()).unwrap();
        opened.verify(h.operator.public()).unwrap();
        assert_eq!(opened.body.data, e["data"]);
    }
    let proof = items[2]["data"]["proof"].as_str().unwrap();
    assert_eq!(
        AccountRequest::open(proof, "live.test", now()).unwrap().1,
        *user.key.public()
    );
}

#[tokio::test]
async fn account_creation_rejections() {
    let h = harness().await;
    let user = h.create("NoIcE").await;
    let post = |body: String| h.call(Method::POST, "/v1/accounts", &[], body);

    // Taken, whatever the case.
    let r = post(AccountRequest::sign(
        &KeyPair::generate(),
        "noice",
        None,
        "live.test",
        now(),
    ))
    .await;
    assert_eq!(
        (r.status, r.error().as_str()),
        (StatusCode::CONFLICT, "conflict")
    );
    // One key, one account.
    assert_eq!(
        post(AccountRequest::sign(
            &user.key,
            "Other",
            None,
            "live.test",
            now()
        ))
        .await
        .status,
        StatusCode::CONFLICT
    );
    // Not a gamertag.
    assert_eq!(
        post(AccountRequest::sign(
            &KeyPair::generate(),
            "a  b",
            None,
            "live.test",
            now()
        ))
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
    // For another server, or stale.
    let elsewhere = AccountRequest::sign(&KeyPair::generate(), "Roamer", None, "elsewhere", now());
    assert_eq!(post(elsewhere).await.status, StatusCode::BAD_REQUEST);
    let stale = AccountRequest::sign(
        &KeyPair::generate(),
        "Roamer",
        None,
        "live.test",
        now() - 600,
    );
    assert_eq!(post(stale).await.status, StatusCode::BAD_REQUEST);
    // Not a JWS.
    assert_eq!(post("hello".into()).await.status, StatusCode::BAD_REQUEST);
    // Signed by a key other than the one it registers.
    let key = KeyPair::generate();
    let body = AccountRequest {
        gamertag: "Mallory".into(),
        jwk: key.public().jwk(),
        invite: None,
        aud: "live.test".into(),
        iat: now(),
    };
    let r = post(KeyPair::generate().sign_json(sign::TYP_ACCOUNT, &body)).await;
    assert_eq!(
        (r.status, r.error().as_str()),
        (StatusCode::UNAUTHORIZED, "unauthorized")
    );
    assert!(r.headers.contains_key(header::WWW_AUTHENTICATE));
}

#[tokio::test]
async fn invite_only() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            invite_only: true,
            ..Policy::default()
        },
    )
    .await;
    let post = |body: String| h.call(Method::POST, "/v1/accounts", &[], body);
    let r = post(AccountRequest::sign(
        &KeyPair::generate(),
        "First",
        None,
        "live.test",
        now(),
    ))
    .await;
    assert_eq!(
        (r.status, r.error().as_str()),
        (StatusCode::FORBIDDEN, "forbidden")
    );

    let signup = |tag: &str, invite: &str| {
        AccountRequest::sign(
            &KeyPair::generate(),
            tag,
            Some(invite.to_string()),
            "live.test",
            now(),
        )
    };
    let invite = Invite::issue(&h.operator, &h.id, now() + 3600);
    assert_eq!(
        post(signup("First", &invite)).await.status,
        StatusCode::CREATED
    );
    // Single use.
    assert_eq!(
        post(signup("Second", &invite)).await.status,
        StatusCode::FORBIDDEN
    );
    // Another server's invite, an expired one, one not signed by the server.
    for bad in [
        Invite::issue(&h.operator, "elsewhere", now() + 3600),
        Invite::issue(&h.operator, &h.id, now() - 1),
        Invite::issue(&KeyPair::generate(), &h.id, now() + 3600),
    ] {
        assert_eq!(
            post(signup("Third", &bad)).await.status,
            StatusCode::FORBIDDEN
        );
    }
    // A failed creation does not burn the invite.
    let invite = Invite::issue(&h.operator, &h.id, now() + 3600);
    assert_eq!(
        post(signup("first", &invite)).await.status,
        StatusCode::CONFLICT
    );
    assert_eq!(
        post(signup("Fourth", &invite)).await.status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn signed_events_append_and_chain() {
    let h = harness().await;
    let user = h.create("Pete").await;
    let r = h.sign_in(&user).await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let first = r.json();
    assert_eq!(
        (first["position"].as_u64(), first["version"].as_u64()),
        (Some(4), Some(0))
    );
    assert_eq!(first["author"], user.author.as_str());
    assert_eq!(
        first["hash"],
        sign::chain_hash(None, first["jws"].as_str().unwrap())
    );

    // Read-your-writes: the reply means it is visible.
    let presence = h.get("/v1/presence").await.json();
    assert_eq!(presence["items"][0]["state"], "online");
    assert_eq!(presence["position"], 4);
    assert_eq!(h.get("/v1/accounts/Pete").await.json()["online"], true);

    let jws = h.event(
        &user,
        &streams::presence(user.xuid),
        types::PRESENCE_CHANGED,
        json!({"xuid": user.xuid, "state": "playing", "game_id": "g1"}),
    );
    let second = h.post_event(&user, jws).await.json();
    assert_eq!(second["version"], 1);
    assert_eq!(
        second["hash"],
        sign::chain_hash(first["hash"].as_str(), second["jws"].as_str().unwrap())
    );

    // The stream as a reader sees it: continuous versions, linked hashes.
    let log = h
        .get(&format!(
            "/v1/events?stream={}",
            streams::presence(user.xuid)
        ))
        .await
        .json();
    let items = log["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    let mut prev: Option<String> = None;
    for (v, e) in items.iter().enumerate() {
        assert_eq!(e["version"], v);
        assert_eq!(
            e["hash"],
            sign::chain_hash(prev.as_deref(), e["jws"].as_str().unwrap())
        );
        prev = Some(e["hash"].as_str().unwrap().into());
    }

    let jws = h.event(
        &user,
        &streams::presence(user.xuid),
        types::SIGNED_OUT,
        json!({"xuid": user.xuid}),
    );
    assert_eq!(h.post_event(&user, jws).await.status, StatusCode::CREATED);
    assert_eq!(h.get("/v1/presence").await.json()["items"], json!([]));
}

#[tokio::test]
async fn token_failures_are_401() {
    let h = &harness().await;
    let user = h.create("Pete").await;
    let event = || {
        h.event(
            &user,
            &streams::presence(user.xuid),
            types::SIGNED_IN,
            json!({"xuid": user.xuid, "state": "online"}),
        )
    };
    let with = |token: String| {
        let jws = event();
        async move {
            h.call(
                Method::POST,
                "/v1/events",
                &[("authorization", format!("Bearer {token}"))],
                jws,
            )
            .await
        }
    };
    let no_token = h.call(Method::POST, "/v1/events", &[], event()).await;
    assert_eq!(
        (no_token.status, no_token.error().as_str()),
        (StatusCode::UNAUTHORIZED, "unauthorized")
    );

    let k = &user.key;
    let a = user.author.as_str();
    for (why, token) in [
        (
            "wrong aud",
            issue_token(k, a, "elsewhere", lxm::EVENTS_WRITE, now()),
        ),
        (
            "wrong lxm",
            issue_token(k, a, &h.id, lxm::TITLES_WRITE, now()),
        ),
        (
            "expired",
            issue_token(k, a, &h.id, lxm::EVENTS_WRITE, now() - 120),
        ),
        (
            "future",
            issue_token(k, a, &h.id, lxm::EVENTS_WRITE, now() + 120),
        ),
        (
            "wrong key",
            issue_token(&KeyPair::generate(), a, &h.id, lxm::EVENTS_WRITE, now()),
        ),
        (
            "unknown author",
            issue_token(
                k,
                "acct:0009000000000001@live.test",
                &h.id,
                lxm::EVENTS_WRITE,
                now(),
            ),
        ),
        (
            "other server",
            issue_token(
                k,
                &format!("acct:{}@elsewhere", user.xuid),
                &h.id,
                lxm::EVENTS_WRITE,
                now(),
            ),
        ),
        ("garbage", "a.b.c".into()),
    ] {
        assert_eq!(with(token).await.status, StatusCode::UNAUTHORIZED, "{why}");
    }

    // A token is single use.
    let token = h.token(k, a, lxm::EVENTS_WRITE);
    assert_eq!(with(token.clone()).await.status, StatusCode::CREATED);
    let replay = with(token).await;
    assert_eq!(replay.status, StatusCode::UNAUTHORIZED);
    assert!(
        replay.json()["message"]
            .as_str()
            .unwrap()
            .contains("already used")
    );
}

#[tokio::test]
async fn event_rule_failures() {
    let h = harness().await;
    let pete = h.create("Pete").await;
    let other = h.create("Other").await;
    let online = |u: &User| json!({"xuid": u.xuid, "state": "online"});

    // Another account's stream, server-only types, games: 403.
    for (stream, kind, data) in [
        (
            streams::presence(other.xuid),
            types::SIGNED_IN,
            online(&other),
        ),
        (
            streams::account(pete.xuid),
            types::ACCOUNT_CREATED,
            json!({}),
        ),
        (
            streams::presence(pete.xuid),
            types::PRESENCE_EXPIRED,
            online(&pete),
        ),
        ("game-1".to_string(), "PlayerKilled", json!({})),
        (
            streams::title_file(live_proto::HALO_2_TITLE_ID, "motd"),
            types::TITLE_FILE_PUBLISHED,
            json!({}),
        ),
    ] {
        let r = h
            .post_event(&pete, h.event(&pete, &stream, kind, data))
            .await;
        assert_eq!(
            (r.status, r.error().as_str()),
            (StatusCode::FORBIDDEN, "forbidden"),
            "{stream} {kind}"
        );
    }

    // An event authored by someone other than the token's issuer.
    let theirs = h.event(
        &other,
        &streams::presence(other.xuid),
        types::SIGNED_IN,
        online(&other),
    );
    assert_eq!(
        h.post_event(&pete, theirs).await.status,
        StatusCode::FORBIDDEN
    );

    // Claiming Pete as author but signed by another key.
    let forged = EventBody::new(
        &other.key,
        &pete.author,
        &streams::presence(pete.xuid),
        types::SIGNED_IN,
        online(&pete),
        time::OffsetDateTime::now_utc(),
    )
    .sign(&other.key);
    assert_eq!(
        h.post_event(&pete, forged).await.status,
        StatusCode::UNAUTHORIZED
    );

    // A tampered payload.
    let good = h.event(
        &pete,
        &streams::presence(pete.xuid),
        types::SIGNED_IN,
        online(&pete),
    );
    let mut parts: Vec<String> = good.split('.').map(String::from).collect();
    let mut body: Value = serde_json::from_slice(&base64_decode(&parts[1])).unwrap();
    body["data"]["state"] = json!("away");
    parts[1] = base64_encode(live_proto::jcs::canonicalize(&body).as_bytes());
    assert_eq!(
        h.post_event(&pete, parts.join(".")).await.status,
        StatusCode::UNAUTHORIZED
    );

    // `at` more than five minutes off.
    let at = time::OffsetDateTime::now_utc() - time::Duration::minutes(6);
    let stale = EventBody::new(
        &pete.key,
        &pete.author,
        &streams::presence(pete.xuid),
        types::SIGNED_IN,
        online(&pete),
        at,
    )
    .sign(&pete.key);
    assert_eq!(
        h.post_event(&pete, stale).await.status,
        StatusCode::BAD_REQUEST
    );

    // Bad presence data.
    for data in [
        json!({"xuid": pete.xuid, "state": "dancing"}),
        json!({"xuid": pete.xuid, "state": "playing"}),
        online(&other),
    ] {
        let r = h
            .post_event(
                &pete,
                h.event(&pete, &streams::presence(pete.xuid), types::SIGNED_IN, data),
            )
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST);
    }

    // The same signed event twice: the nonce is spent.
    assert_eq!(
        h.post_event(&pete, good.clone()).await.status,
        StatusCode::CREATED
    );
    let again = h.post_event(&pete, good).await;
    assert_eq!(
        (again.status, again.error().as_str()),
        (StatusCode::CONFLICT, "conflict")
    );
}

fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .unwrap()
}

fn base64_encode(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

#[tokio::test]
async fn key_rotation_and_revocation() {
    let h = harness().await;
    let old = h.create("Pete").await;
    let new = User {
        key: KeyPair::generate(),
        xuid: old.xuid,
        author: old.author.clone(),
    };
    let stream = streams::account(old.xuid);

    // The old key adds the new one, once.
    let add = h.event(
        &old,
        &stream,
        types::KEY_ADDED,
        json!({"kid": new.key.kid(), "jwk": new.key.public().jwk()}),
    );
    let added = h.post_event(&old, add).await;
    assert_eq!(
        added.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&added.body)
    );
    let again = h.event(
        &old,
        &stream,
        types::KEY_ADDED,
        json!({"jwk": new.key.public().jwk()}),
    );
    assert_eq!(h.post_event(&old, again).await.status, StatusCode::CONFLICT);
    // A kid that is not the key's thumbprint.
    let other = KeyPair::generate();
    let lying = h.event(
        &old,
        &stream,
        types::KEY_ADDED,
        json!({"kid": new.key.kid(), "jwk": other.public().jwk()}),
    );
    assert_eq!(
        h.post_event(&old, lying).await.status,
        StatusCode::BAD_REQUEST
    );

    // The new key revokes the old one.
    let revoke = h.event(
        &new,
        &stream,
        types::KEY_REVOKED,
        json!({"kid": old.key.kid()}),
    );
    let revoked = h.post_event(&new, revoke).await;
    assert_eq!(revoked.status, StatusCode::CREATED);
    let revoked_at = revoked.json()["position"].as_u64().unwrap();
    let keys = h.get("/v1/accounts/Pete/keys").await.json();
    assert_eq!(keys["keys"][0]["kid"], old.key.kid());
    assert_eq!(keys["keys"][0]["valid_to"], revoked_at);
    assert_eq!(keys["keys"][1]["kid"], new.key.kid());
    assert_eq!(keys["keys"][1]["valid_to"], Value::Null);

    // The revoked key can neither authenticate nor sign.
    assert_eq!(h.sign_in(&old).await.status, StatusCode::UNAUTHORIZED);
    let by_old = h.event(
        &old,
        &streams::presence(old.xuid),
        types::SIGNED_IN,
        json!({"xuid": old.xuid, "state": "online"}),
    );
    let r = h.post_event(&new, by_old).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.sign_in(&new).await.status, StatusCode::CREATED);

    // The last key stays; a revoked one cannot be revoked again.
    let last = h.event(
        &new,
        &stream,
        types::KEY_REVOKED,
        json!({"kid": new.key.kid()}),
    );
    assert_eq!(h.post_event(&new, last).await.status, StatusCode::CONFLICT);
    let gone = h.event(
        &new,
        &stream,
        types::KEY_REVOKED,
        json!({"kid": old.key.kid()}),
    );
    assert_eq!(
        h.post_event(&new, gone).await.status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn event_log_filters_and_cursors() {
    let h = harness().await;
    for tag in ["A", "B", "C"] {
        h.create(tag).await;
    }
    let page = h.get("/v1/events?limit=4").await.json();
    assert_eq!(page["items"].as_array().unwrap().len(), 4);
    assert_eq!(page["next"], "4");
    let rest = h.get("/v1/events?limit=100&cursor=4").await.json();
    assert_eq!(rest["items"][0]["position"], 5);
    assert_eq!(rest["items"].as_array().unwrap().len(), 5);
    assert_eq!(rest["next"], Value::Null);
    assert_eq!(
        h.get("/v1/events?from=8").await.json()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let typed = h
        .get("/v1/events?type=AccountCreated,KeyAdded")
        .await
        .json();
    assert_eq!(typed["items"].as_array().unwrap().len(), 6);
    let excluded = h.get("/v1/events?exclude=KeyAdded").await.json();
    assert_eq!(excluded["items"].as_array().unwrap().len(), 6);
    let stream = h.get("/v1/events?stream=gamertag-b").await.json();
    assert_eq!(stream["items"][0]["type"], "GamertagReserved");
    assert_eq!(stream["position"], 9);
    assert_eq!(
        h.get("/v1/events?limit=x").await.status,
        StatusCode::BAD_REQUEST
    );

    let accounts = h.get("/v1/accounts?limit=2").await.json();
    assert_eq!(accounts["items"].as_array().unwrap().len(), 2);
    let next = accounts["next"].as_str().unwrap();
    let more = h
        .get(&format!("/v1/accounts?limit=2&cursor={next}"))
        .await
        .json();
    assert_eq!(more["items"][0]["gamertag"], "C");
    assert_eq!(more["next"], Value::Null);
}

#[tokio::test]
async fn title_files() {
    let h = harness().await;
    let user = h.create("Pete").await;
    let uri = "/v1/titles/4D530064/files/message_of_the_day.dat";
    let motd = live_proto::motd::Motd {
        header: "Hi".into(),
        messages: vec!["Welcome".into()],
    }
    .build()
    .unwrap();

    assert_eq!(h.get(uri).await.status, StatusCode::NOT_FOUND);
    let as_user = format!(
        "Bearer {}",
        h.token(&user.key, &user.author, lxm::TITLES_WRITE)
    );
    let r = h
        .call(
            Method::PUT,
            uri,
            &[("authorization", as_user)],
            motd.clone(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(
        h.call(Method::PUT, uri, &[], motd.clone()).await.status,
        StatusCode::UNAUTHORIZED
    );

    let operator = Author::Server(h.id.clone()).to_string();
    let auth = || {
        format!(
            "Bearer {}",
            h.token(&h.operator, &operator, lxm::TITLES_WRITE)
        )
    };
    let r = h
        .call(Method::PUT, uri, &[("authorization", auth())], motd.clone())
        .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let sha = r.json()["sha256"].as_str().unwrap().to_string();
    assert_eq!(r.json()["size"], motd.len());

    let got = h.get(uri).await;
    assert_eq!(got.body, motd);
    assert_eq!(got.headers[header::ETAG], format!("\"{sha}\""));
    let cached = h
        .call(
            Method::GET,
            uri,
            &[("if-none-match", format!("\"{sha}\""))],
            Body::empty(),
        )
        .await;
    assert_eq!(cached.status, StatusCode::NOT_MODIFIED);
    // Case-insensitive title id.
    assert_eq!(
        h.get("/v1/titles/4d530064/files/message_of_the_day.dat")
            .await
            .status,
        StatusCode::OK
    );

    // A playlist-sized file fits; more than 2 MiB does not.
    let big = vec![7u8; 1_457_344];
    let r = h
        .call(
            Method::PUT,
            "/v1/titles/4D530064/files/multiplayer_hopper.dat",
            &[("authorization", auth())],
            big,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let huge = vec![0u8; 2 * 1024 * 1024 + 1];
    let r = h
        .call(Method::PUT, uri, &[("authorization", auth())], huge)
        .await;
    assert_eq!(
        (r.status, r.error().as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, "too_large")
    );
    let r = h
        .call(
            Method::PUT,
            "/v1/titles/4D530064/files/a%20b",
            &[("authorization", auth())],
            "x",
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = h
        .call(
            Method::PUT,
            "/v1/titles/nothex/files/a",
            &[("authorization", auth())],
            "x",
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn private_reads_need_a_token() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            private: true,
            ..Policy::default()
        },
    )
    .await;
    let user = h.create("Pete").await;
    for uri in [
        "/v1/accounts",
        "/v1/accounts/Pete",
        "/v1/events",
        "/v1/presence",
        "/v1/accounts/Pete/keys",
    ] {
        assert_eq!(h.get(uri).await.status, StatusCode::UNAUTHORIZED, "{uri}");
        let auth = format!("Bearer {}", h.token(&user.key, &user.author, lxm::READ));
        assert_eq!(
            h.call(Method::GET, uri, &[("authorization", auth)], Body::empty())
                .await
                .status,
            StatusCode::OK,
            "{uri}"
        );
    }
    // Discovery and health stay public: a client needs the id to sign.
    assert_eq!(
        h.get("/.well-known/h2live.json").await.status,
        StatusCode::OK
    );
    assert_eq!(h.get("/healthz").await.status, StatusCode::OK);
}

#[tokio::test]
async fn cors_reads_only() {
    let h = &harness().await;
    let r = h
        .call(
            Method::GET,
            "/v1/accounts",
            &[("origin", "https://elsewhere".into())],
            Body::empty(),
        )
        .await;
    assert_eq!(r.headers["access-control-allow-origin"], "*");

    let preflight = |method: &str| {
        let headers = [
            ("origin", "https://elsewhere".to_string()),
            ("access-control-request-method", method.to_string()),
        ];
        async move {
            h.call(Method::OPTIONS, "/v1/events", &headers, Body::empty())
                .await
        }
    };
    let read = preflight("GET").await;
    assert_eq!(read.headers["access-control-allow-origin"], "*");
    assert!(
        read.headers["access-control-allow-headers"]
            .to_str()
            .unwrap()
            .contains("authorization")
    );
    let write = preflight("POST").await;
    assert!(!write.headers.contains_key("access-control-allow-origin"));

    let post = h
        .call(
            Method::POST,
            "/v1/events",
            &[("origin", "https://elsewhere".into())],
            "x",
        )
        .await;
    assert!(!post.headers.contains_key("access-control-allow-origin"));
}

#[tokio::test]
async fn unknown_routes_and_caps() {
    let h = harness().await;
    for (method, uri) in [
        (Method::GET, "/v1/games/nope"),
        (Method::GET, "/v1/maps/lockout/outline"),
        (Method::GET, "/nope"),
    ] {
        let r = h.call(method.clone(), uri, &[], Body::empty()).await;
        assert_eq!(
            (r.status, r.error().as_str()),
            (StatusCode::NOT_FOUND, "not_found"),
            "{method} {uri}"
        );
    }
    let r = h.get("/v1/ingest").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = h.get("/v1/nope").await;
    assert_eq!(
        (r.status, r.error().as_str()),
        (StatusCode::NOT_FOUND, "not_found")
    );
    let r = h
        .call(Method::DELETE, "/v1/accounts", &[], Body::empty())
        .await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.error(), "invalid");
    // Event bodies are capped at 64 KiB.
    let r = h
        .call(Method::POST, "/v1/events", &[], vec![b'a'; 64 * 1024 + 1])
        .await;
    assert_eq!(
        (r.status, r.error().as_str()),
        (StatusCode::PAYLOAD_TOO_LARGE, "too_large")
    );
    let r = h.get("/v1/stream").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn writes_are_rate_limited_per_author() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            write_rate: (0.001, 2),
            create_rate: (0.001, 3),
            ..Policy::default()
        },
    )
    .await;
    let pete = h.create("Pete").await;
    let other = h.create("Other").await;
    assert_eq!(h.sign_in(&pete).await.status, StatusCode::CREATED);
    assert_eq!(h.sign_in(&pete).await.status, StatusCode::CREATED);
    let r = h.sign_in(&pete).await;
    assert_eq!(
        (r.status, r.error().as_str()),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limited")
    );
    assert_eq!(h.sign_in(&other).await.status, StatusCode::CREATED);
    h.create("Third").await;
    let r = h
        .call(
            Method::POST,
            "/v1/accounts",
            &[],
            AccountRequest::sign(&KeyPair::generate(), "Fourth", None, "live.test", now()),
        )
        .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn restart_replays_the_log() {
    let store = Arc::new(MemoryStore::new());
    let pete_key;
    {
        let h = start(store.clone(), Policy::default()).await;
        let pete = h.create("Pete").await;
        assert_eq!(h.sign_in(&pete).await.status, StatusCode::CREATED);
        pete_key = pete;
    }
    let h = start(store, Policy::default()).await;
    assert_eq!(h.get("/v1/accounts/Pete").await.json()["online"], true);
    assert_eq!(h.live.position(), 4);
    // Keys survive the restart: Pete can still write.
    let jws = h.event(
        &pete_key,
        &streams::presence(pete_key.xuid),
        types::SIGNED_OUT,
        json!({"xuid": pete_key.xuid}),
    );
    assert_eq!(
        h.post_event(&pete_key, jws).await.status,
        StatusCode::CREATED
    );
}

/// The same flows against Postgres (`H2_LIVE_TEST_DB`).
#[tokio::test]
async fn postgres_end_to_end() {
    let Some(url) = std::env::var("H2_LIVE_TEST_DB")
        .ok()
        .filter(|s| !s.is_empty())
    else {
        eprintln!("skipping: H2_LIVE_TEST_DB is not set");
        return;
    };
    let store = Arc::new(live_server::pg::PostgresStore::connect(&url).await.unwrap());
    // Unique per run, so a reused database passes.
    let tag = format!("Pg{}", sign::hex(&sign::random_id().as_bytes()[..5]));
    let tag = &tag[..tag.len().min(15)];
    let user;
    {
        let h = start(store.clone(), Policy::default()).await;
        user = h.create(tag).await;
        let taken = h
            .call(
                Method::POST,
                "/v1/accounts",
                &[],
                AccountRequest::sign(
                    &KeyPair::generate(),
                    &tag.to_uppercase(),
                    None,
                    "live.test",
                    now(),
                ),
            )
            .await;
        assert_eq!(taken.status, StatusCode::CONFLICT);
        let r = h.sign_in(&user).await;
        assert_eq!(
            r.status,
            StatusCode::CREATED,
            "{}",
            String::from_utf8_lossy(&r.body)
        );
        assert_eq!(
            r.json()["hash"],
            sign::chain_hash(None, r.json()["jws"].as_str().unwrap())
        );
        let operator = Author::Server(h.id.clone()).to_string();
        let auth = format!(
            "Bearer {}",
            h.token(&h.operator, &operator, lxm::TITLES_WRITE)
        );
        let file = format!("/v1/titles/4D530064/files/{tag}.dat");
        assert_eq!(
            h.call(Method::PUT, &file, &[("authorization", auth)], "hello")
                .await
                .status,
            StatusCode::OK
        );
        assert_eq!(h.get(&file).await.body, b"hello");
    }
    // A restart over the same database replays it.
    let h = start(store, Policy::default()).await;
    assert_eq!(
        h.get(&format!("/v1/accounts/{tag}")).await.json()["online"],
        true
    );
    let jws = h.event(
        &user,
        &streams::presence(user.xuid),
        types::SIGNED_OUT,
        json!({"xuid": user.xuid}),
    );
    let out = h.post_event(&user, jws).await.json();
    assert_eq!(out["version"], 1);
}
