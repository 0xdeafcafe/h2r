//! Account recovery by TOTP (docs/live/live-security.md §1, Recovery): any
//! signed-in device shows the current code; a new device types gamertag
//! and code and gets its key added.

mod common;

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::*;
use live_proto::recovery::{RecoverRequest, RecoveryInfo, TotpSecret, format_code};
use live_proto::sign::{AccountRequest, Invite, KeyPair};
use live_proto::{Xuid, lxm};
use live_server::Policy;
use live_server::store::{EventStore, MemoryStore};
use serde_json::json;

/// Creates `tag`; the user and the recovery info the reply carried.
async fn create(h: &Harness, tag: &str, invite: Option<String>) -> (User, RecoveryInfo) {
    let key = KeyPair::generate();
    let r = h
        .call(
            Method::POST,
            "/v1/accounts",
            &[],
            AccountRequest::sign(&key, tag, invite, "live.test", now()),
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let b = r.json();
    let info: RecoveryInfo = serde_json::from_value(b["recovery"].clone()).unwrap();
    let user = User {
        key,
        xuid: Xuid::parse(b["xuid"].as_str().unwrap()).unwrap(),
        author: b["id"].as_str().unwrap().into(),
    };
    (user, info)
}

fn secret(info: &RecoveryInfo) -> TotpSecret {
    TotpSecret::from_base32(&info.secret).unwrap()
}

async fn recover_with(h: &Harness, tag: &str, code: &str, device: &KeyPair) -> Resp {
    recover_from(h, &[], tag, code, device).await
}

async fn recover_from(
    h: &Harness,
    headers: &[(&str, String)],
    tag: &str,
    code: &str,
    device: &KeyPair,
) -> Resp {
    let body = json!({
        "request": RecoverRequest::sign(device, tag, "live.test", now()),
        "code": code,
    });
    h.call(
        Method::POST,
        &format!("/v1/accounts/{tag}/recover"),
        headers,
        body.to_string(),
    )
    .await
}

fn as_user(key: &KeyPair, of: &User) -> User {
    User {
        key: KeyPair::from_secret_b64(&key.secret_b64()).unwrap(),
        xuid: of.xuid,
        author: of.author.clone(),
    }
}

async fn get_info(h: &Harness, user: &User) -> Resp {
    h.call(
        Method::GET,
        &format!("/v1/accounts/by-xuid/{}/recovery", user.xuid),
        &[h.bearer(user, lxm::RECOVERY_READ)],
        Body::empty(),
    )
    .await
}

async fn reset(h: &Harness, user: &User) -> Resp {
    h.call(
        Method::POST,
        &format!("/v1/accounts/by-xuid/{}/recovery", user.xuid),
        &[h.bearer(user, lxm::RECOVERY_WRITE)],
        Body::empty(),
    )
    .await
}

#[tokio::test]
async fn a_current_code_adds_a_new_device() {
    let h = harness().await;
    let (user, info) = create(&h, "Keeper", None).await;
    assert_eq!(info.period, 30);
    assert!(
        info.otpauth
            .starts_with("otpauth://totp/live.test:Keeper?secret=")
    );
    assert_eq!(info.code, format_code(secret(&info).code_at(now())));
    // The secret is not in the log in the clear.
    let log = h.get("/v1/events?from=0").await.json().to_string();
    assert!(!log.contains(&info.secret));

    // A signed-in device reads the same secret and the current code.
    let r = get_info(&h, &user).await;
    assert_eq!(r.status, StatusCode::OK);
    let read: RecoveryInfo = serde_json::from_value(r.json()).unwrap();
    assert_eq!(read.secret, info.secret);

    let device = KeyPair::generate();
    let code = format_code(secret(&info).code_at(now()));
    let r = recover_with(&h, "keeper", &code, &device).await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    assert_eq!(r.json()["gamertag"], "Keeper");
    assert_eq!(r.json()["id"], user.author.as_str());
    let new = as_user(&device, &user);
    assert_eq!(h.sign_in(&new).await.status, StatusCode::CREATED);
    // The old device still works; the new one can read the secret too.
    assert_eq!(h.sign_in(&user).await.status, StatusCode::CREATED);
    assert_eq!(get_info(&h, &new).await.status, StatusCode::OK);

    // The KeyAdded is the server's, marked totp, with the device's request.
    let events = h.get("/v1/events?type=KeyAdded").await.json();
    let added = events["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["data"]["kid"] == device.kid())
        .unwrap()
        .clone();
    assert_eq!(added["author"], "server:live.test");
    assert_eq!(added["data"]["via"], "totp");
    let proof = added["data"]["proof"].as_str().unwrap();
    assert_eq!(
        &RecoverRequest::open(proof, "live.test", now()).unwrap().1,
        device.public()
    );
    assert!(!added.to_string().contains(&code.replace('-', "")));

    // The same code once only.
    let r = recover_with(&h, "Keeper", &code, &KeyPair::generate()).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn codes_one_step_either_side_are_accepted() {
    let h = harness().await;
    let (_, info) = create(&h, "Keeper", None).await;
    let s = secret(&info);
    let r = recover_with(
        &h,
        "Keeper",
        &format!("{:06}", s.code_at(now() - 30)),
        &KeyPair::generate(),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let r = recover_with(
        &h,
        "Keeper",
        &format!("{:06}", s.code_at(now() + 30)),
        &KeyPair::generate(),
    )
    .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
}

#[tokio::test]
async fn wrong_codes_lock_only_the_gamertag_and_client_ip_pair() {
    let policy = Policy {
        recover_lockout: (3, Duration::from_secs(600), Duration::from_secs(3600)),
        ..Policy::default()
    };
    policy.public.set_edge("https://live.test");
    let h = start(Arc::new(MemoryStore::new()), policy).await;
    let (_, keeper) = create(&h, "Keeper", None).await;
    let (_, other) = create(&h, "Other", None).await;
    let attacker = [("cf-connecting-ip", "192.0.2.1".into())];
    let owner = [("cf-connecting-ip", "192.0.2.2".into())];
    let keeper_secret = secret(&keeper);
    let at = now();
    let wrong = (0..1_000_000)
        .find(|&code| keeper_secret.verify(code, at).is_none())
        .map(format_code)
        .unwrap();
    for _ in 0..3 {
        let r = recover_from(&h, &attacker, "Keeper", &wrong, &KeyPair::generate()).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
    }
    let good = format_code(keeper_secret.code_at(now()));
    assert_eq!(
        recover_from(&h, &attacker, "keeper", &good, &KeyPair::generate())
            .await
            .status,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        recover_from(&h, &owner, "Keeper", &good, &KeyPair::generate())
            .await
            .status,
        StatusCode::CREATED
    );
    let code = format_code(secret(&other).code_at(now()));
    assert_eq!(
        recover_from(&h, &attacker, "Other", &code, &KeyPair::generate())
            .await
            .status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn recovery_budget_follows_the_account_across_clients() {
    let policy = Policy::default();
    policy.public.set_edge("https://live.test");
    let h = start(Arc::new(MemoryStore::new()), policy).await;
    let (_, info) = create(&h, "Alice", None).await;
    let alice = secret(&info);
    let at = now();
    // Use a known incorrect test code across the adjacent time steps.
    let wrong = format_code(
        (0..1_000_000)
            .find(|&code| {
                [-30, 0, 30]
                    .iter()
                    .all(|offset| alice.verify(code, at + offset).is_none())
            })
            .unwrap(),
    );
    let device = KeyPair::generate();
    for client in 1..=10 {
        let peer = [("cf-connecting-ip", format!("192.0.2.{client}"))];
        assert_eq!(
            recover_from(&h, &peer, "Alice", &wrong, &device)
                .await
                .status,
            StatusCode::FORBIDDEN
        );
    }
    let peer = [("cf-connecting-ip", "192.0.2.11".into())];
    assert_eq!(
        recover_from(&h, &peer, "ALICE", &wrong, &device)
            .await
            .status,
        StatusCode::TOO_MANY_REQUESTS
    );
    let (_, other) = create(&h, "Other", None).await;
    let good = format_code(secret(&other).code_at(now()));
    assert_eq!(
        recover_from(&h, &peer, "Other", &good, &device)
            .await
            .status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn recovery_request_budget_is_per_ip_and_independent_of_signup() {
    let policy = Policy {
        create_rate: (0.0, 1),
        recover_rate: (0.0, 1),
        ..Policy::default()
    };
    policy.public.set_edge("https://live.test");
    let h = start(Arc::new(MemoryStore::new()), policy).await;
    let attacker = [("cf-connecting-ip", "192.0.2.1".into())];
    let owner = [("cf-connecting-ip", "192.0.2.2".into())];
    let device = KeyPair::generate();
    assert_eq!(
        recover_from(&h, &attacker, "Nobody", "123456", &device)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        recover_from(&h, &attacker, "Other", "123456", &device)
            .await
            .status,
        StatusCode::TOO_MANY_REQUESTS
    );
    let (_, keeper) = create(&h, "Keeper", None).await;
    let code = format_code(secret(&keeper).code_at(now()));
    assert_eq!(
        recover_from(&h, &owner, "Keeper", &code, &device)
            .await
            .status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn bad_requests_are_refused_without_counting() {
    let h = harness().await;
    let (_, info) = create(&h, "Keeper", None).await;
    let code = format_code(secret(&info).code_at(now()));
    assert_eq!(
        recover_with(&h, "Nobody", &code, &KeyPair::generate())
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        recover_with(&h, "Keeper", "12345", &KeyPair::generate())
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
    // Path and signed gamertag disagree.
    let body = json!({
        "request": RecoverRequest::sign(&KeyPair::generate(), "Keeper", "live.test", now()),
        "code": code,
    });
    let r = h
        .call(
            Method::POST,
            "/v1/accounts/Other/recover",
            &[],
            body.to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // For another server, stale, or signed by a key it does not carry.
    let device = KeyPair::generate();
    for request in [
        RecoverRequest::sign(&device, "Keeper", "elsewhere", now()),
        RecoverRequest::sign(&device, "Keeper", "live.test", now() - 600),
        "not a jws".to_string(),
    ] {
        let body = json!({"request": request, "code": code}).to_string();
        let r = h
            .call(Method::POST, "/v1/accounts/Keeper/recover", &[], body)
            .await;
        assert!(
            matches!(r.status, StatusCode::BAD_REQUEST | StatusCode::UNAUTHORIZED),
            "{}",
            r.status
        );
    }
    // None of those counted: the code still works.
    assert_eq!(
        recover_with(&h, "Keeper", &code, &KeyPair::generate())
            .await
            .status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn recovery_needs_no_invite_on_an_invite_only_server() {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            invite_only: true,
            ..Policy::default()
        },
    )
    .await;
    let invite = Invite::issue(&h.operator, "live.test", now() + 3600);
    let (user, info) = create(&h, "Invited", Some(invite)).await;
    let device = KeyPair::generate();
    let code = format_code(secret(&info).code_at(now()));
    let r = recover_with(&h, "Invited", &code, &device).await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    assert_eq!(
        h.sign_in(&as_user(&device, &user)).await.status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn only_the_account_reads_or_resets_its_secret_and_reset_retires_the_old() {
    let h = harness().await;
    let (user, info) = create(&h, "Keeper", None).await;
    let (other, _) = create(&h, "Other", None).await;
    // Another account's token, a token for another method, no token.
    let r = h
        .call(
            Method::GET,
            &format!("/v1/accounts/by-xuid/{}/recovery", user.xuid),
            &[h.bearer(&other, lxm::RECOVERY_READ)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = h
        .call(
            Method::GET,
            &format!("/v1/accounts/by-xuid/{}/recovery", user.xuid),
            &[h.bearer(&user, lxm::READ)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = h
        .call(
            Method::GET,
            &format!("/v1/accounts/by-xuid/{}/recovery", user.xuid),
            &[],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    let r = reset(&h, &user).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let fresh: RecoveryInfo = serde_json::from_value(r.json()).unwrap();
    assert_ne!(fresh.secret, info.secret);
    let read: RecoveryInfo = serde_json::from_value(get_info(&h, &user).await.json()).unwrap();
    assert_eq!(read.secret, fresh.secret);
    let old = secret(&info);
    let new = secret(&fresh);
    let old_code = old.code_at(now());
    if ![-1, 0, 1]
        .iter()
        .any(|d| new.code_at(now() + d * 30) == old_code)
    {
        let r = recover_with(
            &h,
            "Keeper",
            &format!("{old_code:06}"),
            &KeyPair::generate(),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
    }
    let r = recover_with(
        &h,
        "Keeper",
        &format!("{:06}", new.code_at(now())),
        &KeyPair::generate(),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED);
}

#[tokio::test]
async fn old_accounts_set_up_recovery_and_a_restart_keeps_secrets() {
    let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let boot = |store: Arc<dyn EventStore>, dir: std::path::PathBuf| async move {
        let identity = live_server::ServerIdentity::open(&dir, Some("live.test")).unwrap();
        let live = live_server::Live::start(store, identity, Policy::default())
            .await
            .unwrap();
        Harness {
            app: live_server::http::router(live.clone()),
            live,
            operator: live_server::ServerIdentity::load(&dir).unwrap().key,
            id: "live.test".into(),
            dir,
        }
    };
    let (old, info);
    {
        let h = boot(store.clone(), dir.clone()).await;
        // An account from before recovery: an AccountCreated without it.
        let server = |kind: &str, data: serde_json::Value| {
            let body = live_proto::sign::EventBody::new(
                &h.operator,
                "server:live.test",
                "account-0009000000000abc",
                kind,
                data,
                time::OffsetDateTime::now_utc(),
            );
            live_server::store::NewEvent {
                jws: body.sign(&h.operator).into(),
                jws_index: None,
                body,
            }
        };
        let key = KeyPair::generate();
        let ev = server(
            "AccountCreated",
            json!({"xuid": "0009000000000abc", "gamertag": "OldTimer"}),
        );
        let added = server(
            "KeyAdded",
            json!({"kid": key.kid(), "jwk": key.public().jwk()}),
        );
        h.live.store().append(vec![ev, added], &[]).await.unwrap();
        old = User {
            key,
            xuid: Xuid::parse("0009000000000abc").unwrap(),
            author: "acct:0009000000000abc@live.test".into(),
        };
        // The append above is not awaited by the read model; wait for it.
        let head = h.live.store().head().await.unwrap();
        h.live.applied().wait_for(|p| *p >= head).await.unwrap();
        assert_eq!(get_info(&h, &old).await.status, StatusCode::NOT_FOUND);
        let r = recover_with(&h, "OldTimer", "123456", &KeyPair::generate()).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN);
        let r = reset(&h, &old).await;
        assert_eq!(r.status, StatusCode::CREATED);
        info = serde_json::from_value::<RecoveryInfo>(r.json()).unwrap();
        std::mem::forget(h); // keep the dir for the restart
    }
    let h = boot(store, dir).await;
    let read: RecoveryInfo = serde_json::from_value(get_info(&h, &old).await.json()).unwrap();
    assert_eq!(read.secret, info.secret);
    let code = format_code(secret(&info).code_at(now()));
    assert_eq!(
        recover_with(&h, "OldTimer", &code, &KeyPair::generate())
            .await
            .status,
        StatusCode::CREATED
    );
}
