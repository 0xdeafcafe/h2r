//! `h2 live store copy --to https://…` and `--from https://…`: a log moved
//! into a running server through its operator endpoints
//! (live_server::logio), and back out, as the Worker migration does it.
mod common;

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use common::{Harness, harness};
use live_proto::lxm;
use live_server::copy::{copy_log, verify};
use live_server::httpstore::HttpStore;
use live_server::store::{EventStore, MemoryStore, Query};
use live_server::{Live, Policy, ServerIdentity, http};
use serde_json::json;

/// A server with the same identity as `source` (the cutover: the Worker
/// gets the machine's id and key), empty, listening on loopback.
async fn twin(source: &Harness) -> (Arc<Live>, String, std::path::PathBuf) {
    let dir = common::temp_dir();
    std::fs::create_dir_all(&dir).unwrap();
    for f in ["server.key", "server.json"] {
        std::fs::copy(source.dir.join(f), dir.join(f)).unwrap();
    }
    let live = Live::start(
        Arc::new(MemoryStore::new()),
        ServerIdentity::load(&dir).unwrap(),
        Policy::default(),
    )
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(axum::serve(listener, http::router(live.clone())).into_future());
    (live, url, dir)
}

async fn all(store: &dyn EventStore) -> Vec<live_server::store::RecordedEvent> {
    store.read(&Query::after(0, usize::MAX)).await.unwrap()
}

/// A source with accounts, sign-ins and a game's batch of events.
async fn busy(h: &Harness) -> Vec<common::User> {
    let mut users = Vec::new();
    for tag in ["Alpha", "Bravo", "Charlie"] {
        let u = h.create(tag).await;
        assert_eq!(h.sign_in(&u).await.status, StatusCode::CREATED);
        users.push(u);
    }
    let game = h
        .start_game(&users[0], common::two_player_game(&users[0]))
        .await;
    let kill = |tick: u64, victim: u32, killer: u32| {
        (
            "PlayerKilled",
            json!({"tick": tick, "victim": victim, "killer": killer, "weapon": "battle_rifle", "kind": "kill", "pos": [1.5, -2.0, 3.25]}),
        )
    };
    let batch = h.batch(&users[0], &game, &[kill(300, 1, 0), kill(330, 0, 1)]);
    let r = h.post_batch(&users[0], &game, batch).await;
    assert!(
        r.status.is_success(),
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    users
}

#[tokio::test]
async fn a_log_moves_into_a_server_and_back_out_over_https() {
    let src = harness().await;
    let users = busy(&src).await;
    let source = src.live.store();
    let head = source.head().await.unwrap();
    assert!(head > 8, "a real log: {head}");

    let (dst, url, dir) = twin(&src).await;
    let http = HttpStore::connect(&url, ServerIdentity::load(&dir).unwrap())
        .await
        .unwrap();
    let report = copy_log(source, &http, |_, _| {}).await.unwrap();
    assert_eq!((report.events, report.already), (head, 0));
    assert_eq!(all(dst.store()).await, all(source).await, "kept exactly");
    assert_eq!(verify(source, &http).await.unwrap(), (head, head));
    // The read model has it: the same accounts, and their keys sign in.
    let dst_app = http::router(dst.clone());
    let accounts = |app: axum::Router| async move {
        use tower::ServiceExt;
        let r = app
            .oneshot(
                axum::http::Request::get("/v1/accounts")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = http_body_util::BodyExt::collect(r.into_body())
            .await
            .unwrap()
            .to_bytes();
        serde_json::from_slice::<serde_json::Value>(&body).unwrap()
    };
    assert_eq!(accounts(dst_app).await, accounts(src.app.clone()).await);
    let token = src.token(&users[1].key, &users[1].author, lxm::EVENTS_WRITE);
    assert!(dst.authenticate(Some(&token), lxm::EVENTS_WRITE).is_ok());

    // The tail: the source grows, the copy carries on from the head.
    let d = src.create("Delta").await;
    src.sign_in(&d).await;
    let report = copy_log(source, &http, |_, _| {}).await.unwrap();
    assert_eq!(report.already, head);
    assert_eq!(report.events, source.head().await.unwrap() - head);
    assert_eq!(all(dst.store()).await, all(source).await);

    // Nothing of the server's own lands between copies: writes are held
    // (sweeps too) until the operator releases them.
    assert!(http.held().await.unwrap());
    let key = live_proto::sign::KeyPair::generate();
    let signup = |tag: &str| {
        live_proto::sign::AccountRequest::sign(&key, tag, None, "live.test", common::now())
    };
    let err = dst.create_account(&signup("Echo")).await.unwrap_err();
    assert!(err.message.contains("held"), "{}", err.message);
    dst.sweep().await;
    assert_eq!(
        dst.store().head().await.unwrap(),
        source.head().await.unwrap()
    );

    // And back out (the rollback), into an empty log; the hold is the
    // server's own, not part of the log.
    let back = MemoryStore::new();
    copy_log(&http, &back, |_, _| {}).await.unwrap();
    assert_eq!(all(&back).await, all(source).await);
    assert!(back.checkpoints().await.unwrap().is_empty());

    // Released, it writes again.
    assert_eq!(http.release().await.unwrap(), source.head().await.unwrap());
    assert!(!http.held().await.unwrap());
    dst.create_account(&signup("Echo")).await.unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn only_the_operator_and_only_a_continuation() {
    let src = harness().await;
    let users = busy(&src).await;

    // An account's token, even an admin's, is not the operator's.
    let auth = src.bearer(&users[0], lxm::LOG_READ);
    let r = src
        .call(
            Method::GET,
            "/v1/admin/log",
            &[auth],
            axum::body::Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let r = src.get("/v1/admin/log").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // A token for reading does not write.
    let read = format!(
        "Bearer {}",
        src.token(&src.operator, "server:live.test", lxm::LOG_READ)
    );
    let r = src
        .call(
            Method::POST,
            "/v1/admin/log",
            &[("authorization", read)],
            r#"{"head":0,"events":[]}"#,
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.error());

    // Another server's key is refused before anything is sent.
    let (_dst, url, dir) = twin(&src).await;
    let other = common::temp_dir();
    let stranger = ServerIdentity::open(&other, Some("live.test")).unwrap();
    assert!(HttpStore::connect(&url, stranger).await.is_err());
    let _ = std::fs::remove_dir_all(&other);
    // Tokens never go over plain HTTP to another host.
    assert!(
        HttpStore::connect(
            "http://live.example.net",
            ServerIdentity::load(&dir).unwrap()
        )
        .await
        .is_err()
    );

    // A server that has a log of its own is not merged into.
    let (forked, url, dir2) = twin(&src).await;
    let key = live_proto::sign::KeyPair::generate();
    forked
        .create_account(&live_proto::sign::AccountRequest::sign(
            &key,
            "Interloper",
            None,
            "live.test",
            common::now(),
        ))
        .await
        .unwrap();
    let http = HttpStore::connect(&url, ServerIdentity::load(&dir2).unwrap())
        .await
        .unwrap();
    let err = copy_log(src.live.store(), &http, |_, _| {})
        .await
        .unwrap_err();
    assert!(err.to_string().contains("different log"), "{err}");

    // An event that does not continue the chain is refused by the server
    // with the reason, and nothing of its page is written.
    let (empty, url, dir3) = twin(&src).await;
    let http = HttpStore::connect(&url, ServerIdentity::load(&dir3).unwrap())
        .await
        .unwrap();
    let mut events = all(src.live.store()).await;
    events[2].hash = "0".repeat(64);
    let err = http.import(&events[..4]).await.unwrap_err();
    assert!(err.to_string().contains("hash chain"), "{err}");
    assert_eq!(empty.store().head().await.unwrap(), 0);
    for d in [dir, dir2, dir3] {
        let _ = std::fs::remove_dir_all(d);
    }
}
