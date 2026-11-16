//! Cloud saves through the router (docs/live/live-cloud-saves.md): owner
//! only, versions and conflicts, content checks and caps. Every byte here
//! is invented.
mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::*;
use content::Sha256;
use live_proto::lxm;
use live_proto::storage::{StorageItem, StorageList};
use live_server::Policy;
use live_server::cloud_saves::StorageLimits;
use live_server::store::MemoryStore;

async fn saves_harness(limits: StorageLimits) -> Harness {
    let h = start(
        Arc::new(MemoryStore::new()),
        Policy {
            storage: limits,
            write_rate: (1000.0, 1000),
            ..Policy::default()
        },
    )
    .await;
    h.live.set_data_dir(&h.dir);
    h
}

fn small() -> StorageLimits {
    StorageLimits {
        item_bytes: 64,
        quota_bytes: 100,
        max_items: 3,
    }
}

fn item_uri(owner: &User, kind: &str, name: &str) -> String {
    format!(
        "/v1/accounts/{}/storage/{kind}/{}",
        owner.xuid,
        live_proto::storage::encode_name(name)
    )
}

async fn put_as(
    h: &Harness,
    who: &User,
    owner: &User,
    name: &str,
    if_match: &str,
    bytes: &[u8],
) -> Resp {
    h.call(
        Method::PUT,
        &item_uri(owner, "save", name),
        &[
            h.bearer(who, lxm::STORAGE_WRITE),
            ("if-match", if_match.to_string()),
            ("x-h2-sha256", Sha256::of(bytes).to_string()),
            ("x-h2-modified", "1700000000000".to_string()),
        ],
        Body::from(bytes.to_vec()),
    )
    .await
}

async fn put(h: &Harness, u: &User, name: &str, if_match: &str, bytes: &[u8]) -> Resp {
    put_as(h, u, u, name, if_match, bytes).await
}

async fn list(h: &Harness, who: &User, owner: &User) -> Resp {
    h.call(
        Method::GET,
        &format!("/v1/accounts/{}/storage", owner.xuid),
        &[h.bearer(who, lxm::STORAGE_READ)],
        Body::empty(),
    )
    .await
}

async fn get(h: &Harness, who: &User, owner: &User, name: &str) -> Resp {
    h.call(
        Method::GET,
        &item_uri(owner, "save", name),
        &[h.bearer(who, lxm::STORAGE_READ)],
        Body::empty(),
    )
    .await
}

async fn delete(h: &Harness, u: &User, name: &str, if_match: &str) -> Resp {
    h.call(
        Method::DELETE,
        &item_uri(u, "save", name),
        &[
            h.bearer(u, lxm::STORAGE_WRITE),
            ("if-match", if_match.to_string()),
        ],
        Body::empty(),
    )
    .await
}

fn item(r: &Resp) -> StorageItem {
    serde_json::from_slice(&r.body)
        .unwrap_or_else(|_| panic!("{}: {}", r.status, String::from_utf8_lossy(&r.body)))
}

#[tokio::test]
async fn the_owner_writes_lists_and_reads() {
    let h = saves_harness(StorageLimits::default()).await;
    let chief = h.create("Chief").await;
    let name = "0123456789AB/SaveMeta.xbx";
    let r = put(&h, &chief, name, "0", b"profile one").await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let created = item(&r);
    assert_eq!(created.version, 1);
    assert_eq!(created.size, 11);
    assert_eq!(created.modified, 1_700_000_000_000);
    assert_eq!(created.sha256, Some(Sha256::of(b"profile one")));

    let r = list(&h, &chief, &chief).await;
    assert_eq!(r.status, StatusCode::OK);
    let listed: StorageList = serde_json::from_slice(&r.body).unwrap();
    assert_eq!(listed.items, vec![created.clone()]);
    assert_eq!(listed.used_bytes, 11);
    assert_eq!(r.headers["cache-control"], "private, no-store");

    let r = get(&h, &chief, &chief, name).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, b"profile one");
    assert_eq!(r.headers["etag"], "\"1\"");
    assert_eq!(r.headers["x-h2-version"], "1");
    assert_eq!(r.headers["x-h2-modified"], "1700000000000");

    // A name with a space and parentheses survives the URL.
    let odd = "TitleMeta (2).xbx";
    assert_eq!(put(&h, &chief, odd, "0", b"t").await.status, StatusCode::CREATED);
    assert_eq!(get(&h, &chief, &chief, odd).await.body, b"t");
}

#[tokio::test]
async fn only_the_owner_reads_or_writes_even_on_a_public_server() {
    let h = saves_harness(StorageLimits::default()).await;
    let chief = h.create("Chief").await;
    let arbiter = h.create("Arbiter").await;
    let name = "0123456789AB/data";
    assert_eq!(put(&h, &chief, name, "0", b"mine").await.status, StatusCode::CREATED);

    assert_eq!(list(&h, &arbiter, &chief).await.status, StatusCode::FORBIDDEN);
    assert_eq!(get(&h, &arbiter, &chief, name).await.status, StatusCode::FORBIDDEN);
    let r = put_as(&h, &arbiter, &chief, name, "1", b"theirs").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // No token at all.
    let r = h.get(&format!("/v1/accounts/{}/storage", chief.xuid)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = h.get(&item_uri(&chief, "save", name)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // A read token does not write, and a write token does not read.
    let r = h
        .call(
            Method::PUT,
            &item_uri(&chief, "save", name),
            &[
                h.bearer(&chief, lxm::STORAGE_READ),
                ("if-match", "1".into()),
                ("x-h2-sha256", Sha256::of(b"x").to_string()),
            ],
            Body::from("x"),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = h
        .call(
            Method::GET,
            &item_uri(&chief, "save", name),
            &[h.bearer(&chief, lxm::STORAGE_WRITE)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    // The server's own key is not the owner either.
    let r = h
        .call(
            Method::GET,
            &format!("/v1/accounts/{}/storage", chief.xuid),
            &[(
                "authorization",
                format!(
                    "Bearer {}",
                    h.token(&h.operator, "server:live.test", lxm::STORAGE_READ)
                ),
            )],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    // Arbiter's own storage is empty, and the bytes are not public content.
    let r = list(&h, &arbiter, &arbiter).await;
    assert_eq!(r.json()["items"], serde_json::json!([]));
    let r = h.get(&format!("/v1/content/{}", Sha256::of(b"mine"))).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(get(&h, &chief, &chief, name).await.body, b"mine");
}

#[tokio::test]
async fn versions_refuse_stale_writes() {
    let h = saves_harness(StorageLimits::default()).await;
    let chief = h.create("Chief").await;
    let name = "0123456789AB/data";
    assert_eq!(put(&h, &chief, name, "0", b"one").await.status, StatusCode::CREATED);
    // Another machine that never saw version 1 tries to create it.
    let r = put(&h, &chief, name, "0", b"other").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.error(), "conflict");
    assert_eq!(r.json()["current"]["version"], 1);
    assert_eq!(
        r.json()["current"]["sha256"],
        Sha256::of(b"one").to_string()
    );
    // The machine that saw it moves it on; an ETag-quoted version works.
    let r = put(&h, &chief, name, "\"1\"", b"two").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(item(&r).version, 2);
    // A write based on version 1 is now stale.
    let r = put(&h, &chief, name, "1", b"stale").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.json()["current"]["version"], 2);
    assert_eq!(get(&h, &chief, &chief, name).await.body, b"two");
    // Creating something with a version it never had.
    let r = put(&h, &chief, "FFFFFFFFFFFF/x", "4", b"x").await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert!(r.json()["current"].is_null());
}

#[tokio::test]
async fn deletes_leave_tombstones_that_a_write_revives() {
    let h = saves_harness(StorageLimits::default()).await;
    let chief = h.create("Chief").await;
    let name = "0123456789AB/data";
    put(&h, &chief, name, "0", b"one").await;
    assert_eq!(delete(&h, &chief, name, "0").await.status, StatusCode::CONFLICT);
    let r = delete(&h, &chief, name, "1").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let tomb = item(&r);
    assert!(tomb.deleted);
    assert_eq!((tomb.version, tomb.size, tomb.sha256), (2, 0, None));
    assert_eq!(get(&h, &chief, &chief, name).await.status, StatusCode::NOT_FOUND);
    assert_eq!(delete(&h, &chief, name, "2").await.status, StatusCode::NOT_FOUND);
    let listed: StorageList = serde_json::from_slice(&list(&h, &chief, &chief).await.body).unwrap();
    assert_eq!(listed.items, vec![tomb]);
    assert_eq!(listed.used_bytes, 0);
    // Recreating names the tombstone's version.
    assert_eq!(put(&h, &chief, name, "0", b"again").await.status, StatusCode::CONFLICT);
    let r = put(&h, &chief, name, "2", b"again").await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(item(&r).version, 3);
    assert_eq!(get(&h, &chief, &chief, name).await.body, b"again");
}

#[tokio::test]
async fn bodies_must_match_their_hash_and_headers_are_required() {
    let h = saves_harness(StorageLimits::default()).await;
    let chief = h.create("Chief").await;
    let uri = item_uri(&chief, "save", "0123456789AB/data");
    let send = |headers: Vec<(&'static str, String)>| {
        let h = &h;
        let uri = uri.clone();
        async move { h.call(Method::PUT, &uri, &headers, Body::from("body")).await }
    };
    let auth = || h.bearer(&chief, lxm::STORAGE_WRITE);
    let r = send(vec![
        auth(),
        ("if-match", "0".into()),
        ("x-h2-sha256", Sha256::of(b"something else").to_string()),
    ])
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.json()["message"].as_str().unwrap().contains("SHA-256"));
    let r = send(vec![auth(), ("x-h2-sha256", Sha256::of(b"body").to_string())]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = send(vec![auth(), ("if-match", "0".into())]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = send(vec![
        auth(),
        ("if-match", "zero".into()),
        ("x-h2-sha256", Sha256::of(b"body").to_string()),
    ])
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // Nothing was stored.
    let listed: StorageList = serde_json::from_slice(&list(&h, &chief, &chief).await.body).unwrap();
    assert!(listed.items.is_empty());
    // Bad names and kinds.
    let r = put(&h, &chief, "a/../b", "0", b"x").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = h
        .call(
            Method::PUT,
            &format!("/v1/accounts/{}/storage/maps/x", chief.xuid),
            &[
                h.bearer(&chief, lxm::STORAGE_WRITE),
                ("if-match", "0".into()),
                ("x-h2-sha256", Sha256::of(b"x").to_string()),
            ],
            Body::from("x"),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn caps_quota_and_item_count() {
    let h = saves_harness(small()).await;
    let chief = h.create("Chief").await;
    // Over the item cap.
    let r = put(&h, &chief, "A/big", "0", &[7u8; 65]).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(put(&h, &chief, "A/one", "0", &[1u8; 60]).await.status, StatusCode::CREATED);
    // 60 + 50 is over the 100-byte quota...
    let r = put(&h, &chief, "A/two", "0", &[2u8; 50]).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(r.json()["message"].as_str().unwrap().contains("quota"));
    // ...but replacing an item counts only the new size.
    assert_eq!(put(&h, &chief, "A/one", "1", &[1u8; 64]).await.status, StatusCode::OK);
    assert_eq!(put(&h, &chief, "A/two", "0", &[2u8; 30]).await.status, StatusCode::CREATED);
    assert_eq!(put(&h, &chief, "A/three", "0", b"3").await.status, StatusCode::CREATED);
    // Three items is the most.
    let r = put(&h, &chief, "A/four", "0", b"4").await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    // Another account has its own quota.
    let arbiter = h.create("Arbiter").await;
    assert_eq!(put(&h, &arbiter, "A/one", "0", &[1u8; 60]).await.status, StatusCode::CREATED);
}

#[tokio::test]
async fn a_server_without_a_data_directory_keeps_none() {
    let h = start(Arc::new(MemoryStore::new()), Policy::default()).await;
    let chief = h.create("Chief").await;
    let r = list(&h, &chief, &chief).await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
}
