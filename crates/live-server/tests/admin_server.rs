//! The server screen's endpoints (docs/live/live-admin.md §5): the public
//! `GET /v1/server`, the admin-only `GET /v1/admin/status`, and invites
//! made by admin accounts (`POST /v1/invites` with a signed event, the
//! list, revoke) through the router.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Method, Request, StatusCode};
use common::{Harness, User, now, start};
use http_body_util::BodyExt;
use live_proto::sign::{AccountRequest, KeyPair, random_id};
use live_proto::{Gamertag, lxm};
use live_server::Policy;
use live_server::store::MemoryStore;
use serde_json::{Value, json};
use tower::ServiceExt;

const ADMIN_READ: &str = "admin.read";

async fn server(invite_only: bool, private: bool) -> Harness {
    start(
        Arc::new(MemoryStore::new()),
        Policy {
            invite_only,
            private,
            admins: vec![Gamertag::new("Boss").unwrap()],
            ..Policy::default()
        },
    )
    .await
}

fn operator_auth(h: &Harness, method: &str) -> (&'static str, String) {
    (
        "authorization",
        format!(
            "Bearer {}",
            h.token(&h.operator, "server:live.test", method)
        ),
    )
}

/// An account made through an operator invite (the server may be
/// invite-only).
async fn account(h: &Harness, tag: &str) -> User {
    if !h.live.policy().invite_only {
        return h.create(tag).await;
    }
    let key = KeyPair::generate();
    let code = live_proto::sign::Invite::issue(&h.operator, &h.id, now() + 3600);
    let r = h
        .call(
            Method::POST,
            "/v1/accounts",
            &[],
            AccountRequest::sign(&key, tag, Some(code), "live.test", now()),
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let b = r.json();
    User {
        key,
        xuid: live_proto::Xuid::parse(b["xuid"].as_str().unwrap()).unwrap(),
        author: b["id"].as_str().unwrap().into(),
    }
}

async fn sign_up(h: &Harness, tag: &str, code: &str) -> common::Resp {
    h.call(
        Method::POST,
        "/v1/accounts",
        &[],
        AccountRequest::sign(
            &KeyPair::generate(),
            tag,
            Some(code.to_string()),
            "live.test",
            now(),
        ),
    )
    .await
}

fn created_event(h: &Harness, user: &User, jti: &str, exp: i64) -> String {
    h.event(
        user,
        "invites",
        "InviteCreated",
        json!({"jti": jti, "exp": exp}),
    )
}

async fn create_invite(h: &Harness, user: &User, jws: String) -> common::Resp {
    h.call(
        Method::POST,
        "/v1/invites",
        &[h.bearer(user, lxm::INVITES_WRITE)],
        jws,
    )
    .await
}

async fn revoke(h: &Harness, user: &User, jti: &str) -> common::Resp {
    let jws = h.event(user, "invites", "InviteRevoked", json!({"jti": jti}));
    h.call(
        Method::POST,
        &format!("/v1/invites/{jti}/revoke"),
        &[h.bearer(user, lxm::INVITES_WRITE)],
        jws,
    )
    .await
}

/// `GET /v1/admin/status` with `auth`, from `peer` (None: unknown) and
/// extra headers.
async fn status(
    h: &Harness,
    auth: Option<(&'static str, String)>,
    peer: Option<SocketAddr>,
    extra: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(Method::GET)
        .uri("/v1/admin/status");
    if let Some((k, v)) = auth {
        req = req.header(k, v);
    }
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let mut req = req.body(Body::empty()).unwrap();
    if let Some(p) = peer {
        req.extensions_mut().insert(ConnectInfo(p));
    }
    let resp = h.app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn public_facts_for_everyone() {
    let h = server(true, false).await;
    let a = account(&h, "Somebody").await;
    h.sign_in(&a).await;
    let r = h.get("/v1/server").await;
    assert_eq!(r.status, StatusCode::OK);
    let v = r.json();
    assert_eq!(v["id"], "live.test");
    assert_eq!(v["invite_only"], true);
    assert_eq!(v["private"], false);
    assert_eq!(v["federation"], false);
    assert!(
        v["software"]
            .as_str()
            .unwrap()
            .starts_with("h2recomp live-server")
    );
    assert_eq!(v["counts"]["accounts"], 1);
    assert_eq!(v["counts"]["online"], 1);
    assert_eq!(v["counts"]["live_games"], 0);
    assert!(v.get("uptime_s").is_none(), "uptime is the admin's");

    // A private server answers it like any read: with a token.
    let p = server(false, true).await;
    assert_eq!(p.get("/v1/server").await.status, StatusCode::UNAUTHORIZED);
    let u = p.create("Reader").await;
    let r = p
        .call(
            Method::GET,
            "/v1/server",
            &[p.bearer(&u, lxm::READ)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["private"], true);
}

#[tokio::test]
async fn admin_status_is_for_admins_only() {
    let h = server(false, false).await;
    h.live.set_data_dir(&h.dir);
    let nobody = h.create("Nobody").await;
    let boss = h.create("Boss").await;

    assert_eq!(
        status(&h, None, None, &[]).await.0,
        StatusCode::UNAUTHORIZED
    );
    let (code, body) = status(&h, Some(h.bearer(&nobody, ADMIN_READ)), None, &[]).await;
    assert_eq!(code, StatusCode::FORBIDDEN, "{body}");
    // A token for another method does not do.
    let (code, _) = status(&h, Some(h.bearer(&boss, lxm::READ)), None, &[]).await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);

    let (code, v) = status(&h, Some(h.bearer(&boss, ADMIN_READ)), None, &[]).await;
    assert_eq!(code, StatusCode::OK, "{v}");
    assert_eq!(v["id"], "live.test");
    assert!(v["uptime_s"].as_i64().unwrap() >= 0);
    assert!(v["started_at"].as_str().unwrap().contains('T'));
    assert_eq!(v["public"], json!({"url": null, "tunnel": null}));
    assert_eq!(v["game_art"], json!({"present": false, "shared": false}));
    assert_eq!(
        v["admins"],
        json!([{"gamertag": "Boss", "source": "configured"}])
    );
    assert_eq!(v["log"]["store"], "memory");
    assert_eq!(v["log"]["head"], v["log"]["position"]);
    assert_eq!(
        v["invites"],
        json!({"open": 0, "used": 0, "revoked": 0, "expired": 0})
    );
    assert_eq!(v["counts"]["accounts"], 2);
    assert!(v.get("data_dir").is_none(), "an unknown peer gets no paths");
    // Nothing secret: not the server key.
    let secret = h.operator.secret_b64();
    assert!(!v.to_string().contains(&secret));

    // The operator may read it too.
    let (code, _) = status(&h, Some(operator_auth(&h, ADMIN_READ)), None, &[]).await;
    assert_eq!(code, StatusCode::OK);

    // The data directory only for this machine, not through a proxy.
    let lo: SocketAddr = "127.0.0.1:5000".parse().unwrap();
    let lan: SocketAddr = "192.168.1.20:5000".parse().unwrap();
    let (_, v) = status(&h, Some(h.bearer(&boss, ADMIN_READ)), Some(lo), &[]).await;
    assert_eq!(v["data_dir"], h.dir.display().to_string());
    let (_, v) = status(&h, Some(h.bearer(&boss, ADMIN_READ)), Some(lan), &[]).await;
    assert!(v.get("data_dir").is_none());
    let (_, v) = status(
        &h,
        Some(h.bearer(&boss, ADMIN_READ)),
        Some(lo),
        &[("cf-connecting-ip", "203.0.113.9")],
    )
    .await;
    assert!(
        v.get("data_dir").is_none(),
        "a tunnelled request is not local"
    );

    // The tunnel, as discovery's URL says.
    h.live
        .policy()
        .public
        .set("https://some-words-here.trycloudflare.com", true);
    let (_, v) = status(&h, Some(h.bearer(&boss, ADMIN_READ)), None, &[]).await;
    assert_eq!(v["public"]["tunnel"], "quick");
    assert_eq!(v["url"], "https://some-words-here.trycloudflare.com");
}

#[tokio::test]
async fn an_admin_makes_an_invite_that_signs_someone_up_once() {
    let h = server(true, false).await;
    let boss = account(&h, "Boss").await;
    let jti = random_id();
    let exp = now() + 7 * 86_400;
    let r = create_invite(&h, &boss, created_event(&h, &boss, &jti, exp)).await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let v = r.json();
    assert_eq!(v["jti"], jti.as_str());
    assert_eq!(v["exp"], exp);
    let code = v["invite"].as_str().unwrap().to_string();

    // The audit trail is the admin's own signed event.
    let log = h.get("/v1/events?stream=invites").await.json();
    let ev = &log["items"][0];
    assert_eq!(ev["type"], "InviteCreated");
    assert_eq!(ev["author"], boss.author.as_str());
    assert_eq!(ev["data"]["jti"], jti.as_str());
    assert!(
        !log.to_string().contains(&code),
        "the code is not in the log"
    );

    assert_eq!(
        sign_up(&h, "Newcomer", &code).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        sign_up(&h, "Second", &code).await.status,
        StatusCode::FORBIDDEN
    );

    let list = h
        .call(
            Method::GET,
            "/v1/invites",
            &[h.bearer(&boss, ADMIN_READ)],
            Body::empty(),
        )
        .await;
    assert_eq!(list.status, StatusCode::OK);
    let item = &list.json()["items"][0];
    assert_eq!(item["state"], "used");
    assert_eq!(item["by_gamertag"], "Boss");
    assert_eq!(item["used_by_gamertag"], "Newcomer");
    assert!(
        item.get("invite").is_none(),
        "only open invites carry their code"
    );
}

#[tokio::test]
async fn only_admins_make_invites_and_the_event_must_be_right() {
    let h = server(true, false).await;
    let boss = account(&h, "Boss").await;
    let nobody = account(&h, "Nobody").await;
    let exp = now() + 86_400;

    // Anonymous, a normal account, a token for another method.
    let anon = h
        .call(
            Method::POST,
            "/v1/invites",
            &[],
            created_event(&h, &boss, &random_id(), exp),
        )
        .await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
    let r = create_invite(&h, &nobody, created_event(&h, &nobody, &random_id(), exp)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = h
        .call(
            Method::POST,
            "/v1/invites",
            &[h.bearer(&boss, lxm::EVENTS_WRITE)],
            created_event(&h, &boss, &random_id(), exp),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // Someone else's event under the admin's token.
    let r = create_invite(&h, &boss, created_event(&h, &nobody, &random_id(), exp)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Too long, already past, a bad jti, another stream or type, extra fields.
    for jws in [
        created_event(&h, &boss, &random_id(), now() + 31 * 86_400),
        created_event(&h, &boss, &random_id(), now() - 10),
        created_event(&h, &boss, "short", exp),
        h.event(
            &boss,
            "invites-x",
            "InviteCreated",
            json!({"jti": random_id(), "exp": exp}),
        ),
        h.event(
            &boss,
            "invites",
            "InviteRevoked",
            json!({"jti": random_id()}),
        ),
        h.event(
            &boss,
            "invites",
            "InviteCreated",
            json!({"jti": random_id(), "exp": exp, "uses": 5}),
        ),
    ] {
        let r = create_invite(&h, &boss, jws).await;
        assert_eq!(
            r.status,
            StatusCode::BAD_REQUEST,
            "{}",
            String::from_utf8_lossy(&r.body)
        );
    }
    // A jti once is enough; a replayed event is refused.
    let jti = random_id();
    let jws = created_event(&h, &boss, &jti, exp);
    assert_eq!(
        create_invite(&h, &boss, jws.clone()).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        create_invite(&h, &boss, jws).await.status,
        StatusCode::CONFLICT
    );
    let r = create_invite(&h, &boss, created_event(&h, &boss, &jti, exp)).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    // Nobody but admins lists them.
    let r = h
        .call(
            Method::GET,
            "/v1/invites",
            &[h.bearer(&nobody, ADMIN_READ)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(h.get("/v1/invites").await.status, StatusCode::UNAUTHORIZED);

    // The operator's own invites work as before.
    let r = h
        .call(
            Method::POST,
            "/v1/invites?days=2",
            &[operator_auth(&h, lxm::INVITES_WRITE)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let code = r.json()["invite"].as_str().unwrap().to_string();
    assert_eq!(
        sign_up(&h, "ViaOperator", &code).await.status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn invite_creation_is_rate_limited_per_admin() {
    let h = server(true, false).await;
    let boss = account(&h, "Boss").await;
    let exp = now() + 86_400;
    for i in 0..10 {
        let r = create_invite(&h, &boss, created_event(&h, &boss, &random_id(), exp)).await;
        assert_eq!(r.status, StatusCode::CREATED, "invite {i}");
    }
    let r = create_invite(&h, &boss, created_event(&h, &boss, &random_id(), exp)).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn revoked_invites_and_ex_admins_invites_do_not_sign_up() {
    let h = server(true, false).await;
    let boss = account(&h, "Boss").await;
    let nobody = account(&h, "Nobody").await;
    let exp = now() + 86_400;

    let jti = random_id();
    let r = create_invite(&h, &boss, created_event(&h, &boss, &jti, exp)).await;
    let code = r.json()["invite"].as_str().unwrap().to_string();
    // The list re-mints the same code while it is open.
    let list = h
        .call(
            Method::GET,
            "/v1/invites",
            &[h.bearer(&boss, ADMIN_READ)],
            Body::empty(),
        )
        .await
        .json();
    assert_eq!(list["items"][0]["invite"], code.as_str());
    assert_eq!(list["items"][0]["state"], "open");

    assert_eq!(
        revoke(&h, &nobody, &jti).await.status,
        StatusCode::FORBIDDEN
    );
    let r = revoke(&h, &boss, &jti).await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    assert_eq!(revoke(&h, &boss, &jti).await.status, StatusCode::CONFLICT);
    assert_eq!(
        revoke(&h, &boss, &random_id()).await.status,
        StatusCode::NOT_FOUND
    );
    let r = sign_up(&h, "TooLate", &code).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.json()["message"].as_str().unwrap().contains("revoked"));

    // A granted admin's invites stop working when the grant ends.
    let grant = operator_auth(&h, "admin.write");
    let r = h
        .call(Method::POST, "/v1/admins/Nobody", &[grant], Body::empty())
        .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let r = create_invite(&h, &nobody, created_event(&h, &nobody, &random_id(), exp)).await;
    assert_eq!(r.status, StatusCode::CREATED);
    let code = r.json()["invite"].as_str().unwrap().to_string();
    let ungrant = operator_auth(&h, "admin.write");
    let r = h
        .call(
            Method::DELETE,
            "/v1/admins/Nobody",
            &[ungrant],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let r = sign_up(&h, "Orphan", &code).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(
        r.json()["message"]
            .as_str()
            .unwrap()
            .contains("no longer an admin")
    );

    let (_, v) = status(&h, Some(h.bearer(&boss, ADMIN_READ)), None, &[]).await;
    assert_eq!(v["invites"]["revoked"], 1);
    assert_eq!(v["invites"]["open"], 1);
}
