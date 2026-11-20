//! Sign-ins (docs/live/live-api.md "Sign-ins"): an account lists the
//! devices it is signed in on (its keys) and signs any other one out; a
//! signed-out key's next request is refused.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::*;
use live_proto::recovery::{RecoverRequest, RecoveryInfo, TotpSecret, format_code};
use live_proto::sign::{AccountRequest, KeyPair};
use live_proto::{Xuid, lxm};
use live_server::{Policy, ServerIdentity};
use live_server::store::{EventStore, MemoryStore};
use serde_json::{Value, json};

const CHROME_MAC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0.0.0 Safari/537.36";
const IPHONE: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 19_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/19.0 Mobile/15E148 Safari/604.1";
const GAME: &str = "ureq/2.12.1";

/// An account made from a browser; its user and sign-in code secret.
async fn create(h: &Harness, tag: &str) -> (User, TotpSecret) {
    let key = KeyPair::generate();
    let r = h
        .call(
            Method::POST,
            "/v1/accounts",
            &[("user-agent", CHROME_MAC.into())],
            AccountRequest::sign(&key, tag, None, "live.test", now()),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let b = r.json();
    let info: RecoveryInfo = serde_json::from_value(b["recovery"].clone()).unwrap();
    let user = User {
        key,
        xuid: Xuid::parse(b["xuid"].as_str().unwrap()).unwrap(),
        author: b["id"].as_str().unwrap().into(),
    };
    (user, TotpSecret::from_base32(&info.secret).unwrap())
}

/// Signs `of` in on a new device with the code of step `at`; that device.
async fn sign_in_with_code(h: &Harness, tag: &str, of: &User, secret: &TotpSecret, at: i64, ua: &str) -> User {
    let device = KeyPair::generate();
    let body = json!({
        "request": RecoverRequest::sign(&device, tag, "live.test", now()),
        "code": format_code(secret.code_at(at)),
    });
    let r = h
        .call(
            Method::POST,
            &format!("/v1/accounts/{tag}/recover"),
            &[("user-agent", ua.into())],
            body.to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    User {
        key: device,
        xuid: of.xuid,
        author: of.author.clone(),
    }
}

async fn list(h: &Harness, user: &User) -> Resp {
    h.call(
        Method::GET,
        &format!("/v1/accounts/by-xuid/{}/signins", user.xuid),
        &[h.bearer(user, lxm::SIGNINS_READ), ("user-agent", CHROME_MAC.into())],
        Body::empty(),
    )
    .await
}

async fn sign_out(h: &Harness, user: &User, kid: &str) -> Resp {
    h.call(
        Method::DELETE,
        &format!("/v1/accounts/by-xuid/{}/signins/{kid}", user.xuid),
        &[h.bearer(user, lxm::SIGNINS_WRITE)],
        Body::empty(),
    )
    .await
}

async fn sign_out_others(h: &Harness, user: &User) -> Resp {
    h.call(
        Method::POST,
        &format!("/v1/accounts/by-xuid/{}/signins/sign-out-others", user.xuid),
        &[h.bearer(user, lxm::SIGNINS_WRITE)],
        Body::empty(),
    )
    .await
}

fn items(r: &Resp) -> Vec<Value> {
    r.json()["items"].as_array().unwrap().clone()
}

fn find<'a>(items: &'a [Value], key: &KeyPair) -> &'a Value {
    items
        .iter()
        .find(|i| i["kid"] == key.kid())
        .unwrap_or_else(|| panic!("{} not in {items:?}", key.kid()))
}

#[tokio::test]
async fn an_account_sees_where_it_is_signed_in_and_signs_one_out() {
    let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
    let h = start(store.clone(), Policy::default()).await;
    let (laptop, secret) = create(&h, "Roamer").await;
    let t = now();
    let game = sign_in_with_code(&h, "Roamer", &laptop, &secret, t, GAME).await;
    let phone = sign_in_with_code(&h, "Roamer", &laptop, &secret, t - 30, IPHONE).await;

    // The laptop's list: itself first, each named from the User-Agent that
    // made its key.
    let r = list(&h, &laptop).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let all = items(&r);
    assert_eq!(all.len(), 3);
    assert_eq!(all[0]["kid"], laptop.key.kid());
    assert_eq!(all[0]["current"], true);
    let me = find(&all, &laptop.key);
    assert_eq!((me["kind"].as_str(), me["name"].as_str(), me["via"].as_str()), (Some("browser"), Some("Chrome on macOS"), Some("signup")));
    let g = find(&all, &game.key);
    assert_eq!((g["kind"].as_str(), g["name"].as_str(), g["via"].as_str()), (Some("game"), Some("Halo 2"), Some("code")));
    assert_eq!(g["current"], false);
    let p = find(&all, &phone.key);
    assert_eq!(p["name"], "Safari on iPhone");
    // The laptop's request just now is its last use; the phone hasn't used
    // its key yet.
    assert!(me["last_seen"].is_string());
    assert!(p["last_seen"].is_null());
    assert_eq!(h.sign_in(&phone).await.status, StatusCode::CREATED);
    assert!(find(&items(&list(&h, &laptop).await), &phone.key)["last_seen"].is_string());

    // The device names are not in the public log in the clear.
    let log = h.get("/v1/events?from=0").await.json().to_string();
    assert!(!log.contains("Safari on iPhone") && !log.contains("Chrome on macOS"));

    // Signing the game out: its next request is refused, the others go on.
    assert_eq!(h.sign_in(&game).await.status, StatusCode::CREATED);
    let r = sign_out(&h, &laptop, game.key.kid()).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    assert_eq!(r.json()["revoked"], json!([game.key.kid()]));
    let refused = h.sign_in(&game).await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
    assert!(refused.json()["message"].as_str().unwrap_or("").contains("revoked"), "{}", refused.json());
    assert_eq!(list(&h, &game).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.sign_in(&laptop).await.status, StatusCode::CREATED);
    assert_eq!(h.sign_in(&phone).await.status, StatusCode::CREATED);
    let left = items(&list(&h, &laptop).await);
    assert_eq!(left.len(), 2);
    assert!(left.iter().all(|i| i["kid"] != game.key.kid()));

    // The log says so: a KeyRevoked the server signed, naming the key
    // that asked.
    let events = h.get("/v1/events?type=KeyRevoked").await.json();
    let e = &events["items"][0];
    assert_eq!(e["stream"], format!("account-{}", laptop.xuid));
    assert_eq!(e["author"], "server:live.test");
    assert_eq!(e["data"]["kid"], game.key.kid());
    assert_eq!(e["data"]["by"], laptop.key.kid());

    // Not this one (sign out here instead), nor one it doesn't have.
    assert_eq!(sign_out(&h, &laptop, laptop.key.kid()).await.status, StatusCode::CONFLICT);
    assert_eq!(sign_out(&h, &laptop, game.key.kid()).await.status, StatusCode::NOT_FOUND);
    assert_eq!(sign_out(&h, &laptop, "nope").await.status, StatusCode::NOT_FOUND);

    // A restarted server (same data directory, so the same server key the
    // device names are sealed under) reads the revocation and the names
    // back from the log.
    let identity = ServerIdentity::load(&h.dir).unwrap();
    let operator = ServerIdentity::load(&h.dir).unwrap().key;
    drop(h);
    let live = live_server::Live::start_with(
        store,
        identity,
        Policy::default(),
        live_server::app::Timers::Spawned,
    )
    .await
    .unwrap();
    let h = Harness {
        app: live_server::http::router(live.clone()),
        live,
        operator,
        id: "live.test".into(),
        dir: temp_dir(),
    };
    assert_eq!(h.sign_in(&game).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.sign_in(&phone).await.status, StatusCode::CREATED);
    let after = items(&list(&h, &laptop).await);
    assert_eq!(find(&after, &phone.key)["name"], "Safari on iPhone");
}

#[tokio::test]
async fn signing_out_everywhere_else_keeps_only_the_caller() {
    let h = harness().await;
    let (laptop, secret) = create(&h, "Homebody").await;
    let t = now();
    let game = sign_in_with_code(&h, "Homebody", &laptop, &secret, t, GAME).await;
    let phone = sign_in_with_code(&h, "Homebody", &laptop, &secret, t + 30, IPHONE).await;

    let r = sign_out_others(&h, &phone).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let mut revoked: Vec<String> = serde_json::from_value(r.json()["revoked"].clone()).unwrap();
    revoked.sort();
    let mut want = vec![laptop.key.kid().to_string(), game.key.kid().to_string()];
    want.sort();
    assert_eq!(revoked, want);
    for gone in [&laptop, &game] {
        assert_eq!(h.sign_in(gone).await.status, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(h.sign_in(&phone).await.status, StatusCode::CREATED);
    let left = items(&list(&h, &phone).await);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0]["current"], true);

    // Nothing else to sign out: an empty answer, no events.
    let r = sign_out_others(&h, &phone).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["revoked"], json!([]));
    assert_eq!(
        h.get("/v1/events?type=KeyRevoked").await.json()["items"].as_array().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn only_the_account_itself_with_the_right_scope() {
    let h = harness().await;
    let (a, _) = create(&h, "Owner").await;
    let b = h.create("Stranger").await;
    let path = format!("/v1/accounts/by-xuid/{}/signins", a.xuid);
    // No token, another account's token, a token for another method.
    assert_eq!(h.get(&path).await.status, StatusCode::UNAUTHORIZED);
    let r = h
        .call(Method::GET, &path, &[h.bearer(&b, lxm::SIGNINS_READ)], Body::empty())
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = h
        .call(Method::GET, &path, &[h.bearer(&a, lxm::RECOVERY_READ)], Body::empty())
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = h
        .call(
            Method::POST,
            &format!("{path}/sign-out-others"),
            &[h.bearer(&b, lxm::SIGNINS_WRITE)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = h
        .call(
            Method::POST,
            &format!("{path}/sign-out-others"),
            &[h.bearer(&a, lxm::SIGNINS_READ)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // A key made before devices were named lists as unknown until it asks
    // itself, then by the asking request's User-Agent.
    let r = list(&h, &b).await;
    let me = &items(&r)[0];
    assert_eq!(me["name"], "Chrome on macOS");
}
