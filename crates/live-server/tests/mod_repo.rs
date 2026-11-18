//! The mod repository's reads (docs/live/live-api.md "Custom maps"):
//! `GET /v1/maps` sorted newest or most downloaded, filtered by base map,
//! category, publisher and text, paged by cursor, each item with its log
//! position and download count; and the download count itself, one per
//! signed-in account that fetches a manifest, kept in the log. Every byte
//! here is invented.
mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::*;
use content::manifest::Layer;
use content::{Manifest, MapId, Sha256};
use live_proto::lxm;
use live_proto::maps::{MAP_DOWNLOADED, MAP_PUBLISHED, MapDocument, package_stream};
use live_proto::sign::EventBody;
use live_server::content::ContentLimits;
use live_server::store::{EventStore, MemoryStore};
use live_server::{Policy, filestore::FileStore};
use serde_json::Value;

fn policy() -> Policy {
    Policy {
        content: ContentLimits {
            layer_bytes: 256,
            map_bytes: 256,
            quota_bytes: 4096,
            upload_rate: (100.0, 100),
        },
        ..Policy::default()
    }
}

async fn repo(store: Arc<dyn EventStore>, policy: Policy) -> Harness {
    let h = start(store, policy).await;
    h.live.set_data_dir(&h.dir);
    h
}

/// An invented mod of `base`: one patch whose bytes are `seed`.
struct Mod<'a> {
    seed: &'a str,
    base: &'a str,
    name: &'a str,
    description: Option<&'a str>,
    credit: Option<&'a str>,
    category: Option<&'a str>,
    source_url: Option<&'a str>,
}

impl<'a> Mod<'a> {
    fn new(seed: &'a str, base: &'a str, name: &'a str) -> Self {
        Self {
            seed,
            base,
            name,
            description: None,
            credit: None,
            category: None,
            source_url: None,
        }
    }

    fn patch(&self) -> Vec<u8> {
        format!("PATCH invented {}", self.seed).into_bytes()
    }

    fn doc(&self) -> MapDocument {
        let mut m = Manifest::retail(self.base, Sha256([0x31; 32]));
        m.package.name = self.name.into();
        m.package.description = self.description.map(Into::into);
        m.package.author = self.credit.map(Into::into);
        m.patches = vec![Layer {
            file: "mod.bps".into(),
            sha256: Sha256::of(&self.patch()),
        }];
        let mut d = MapDocument::new(m, None);
        d.category = self.category.map(Into::into);
        d.source_url = self.source_url.map(Into::into);
        d
    }
}

async fn put_map(h: &Harness, u: &User, doc: &MapDocument) -> Resp {
    let id = MapId::of(&doc.manifest);
    let jws = EventBody::at_unix(
        &u.key,
        &u.author,
        &package_stream(&id),
        MAP_PUBLISHED,
        serde_json::to_value(doc).unwrap(),
        now(),
    )
    .sign(&u.key);
    h.call(
        Method::PUT,
        "/v1/maps",
        &[h.bearer(u, lxm::CONTENT_WRITE)],
        jws,
    )
    .await
}

/// Uploads the mod's patch and publishes it; its `MapId` and position.
async fn publish(h: &Harness, u: &User, m: &Mod<'_>) -> (MapId, u64) {
    let patch = m.patch();
    let r = h
        .call(
            Method::PUT,
            &format!("/v1/content/{}", Sha256::of(&patch)),
            &[h.bearer(u, lxm::CONTENT_WRITE)],
            Body::from(patch),
        )
        .await;
    assert!(r.status.is_success(), "{}", r.error());
    let doc = m.doc();
    let r = put_map(h, u, &doc).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let position = r.json()["map"]["position"].as_u64().unwrap();
    assert!(position > 0);
    (MapId::of(&doc.manifest), position)
}

async fn list(h: &Harness, query: &str) -> Value {
    let r = h.get(&format!("/v1/maps?{query}")).await;
    assert_eq!(r.status, StatusCode::OK, "{query}: {}", r.error());
    r.json()
}

fn names(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["name"].as_str().unwrap().to_string())
        .collect()
}

/// The manifest fetch a console makes, signed in as `u`.
async fn fetch_as(h: &Harness, u: &User, id: &MapId) -> Value {
    let r = h
        .call(
            Method::GET,
            &format!("/v1/maps/{id}"),
            &[h.bearer(u, lxm::READ)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    r.json()
}

async fn downloads(h: &Harness, id: &MapId) -> u64 {
    h.get(&format!("/v1/maps/{id}")).await.json()["map"]["downloads"]
        .as_u64()
        .unwrap()
}

#[tokio::test]
async fn lists_sort_filter_and_page() {
    let h = repo(Arc::new(MemoryStore::new()), policy()).await;
    let a = h.create("Modder").await;
    let b = h.create("Tinkerer").await;
    let mut one = Mod::new("one", "lockout", "Lockout Snipers");
    one.category = Some("weapons");
    one.description = Some("Snipers only, all the time.");
    let mut two = Mod::new("two", "midship", "Midship Low Gravity");
    two.category = Some("gameplay");
    two.credit = Some("Old Forum Legend");
    let mut three = Mod::new("three", "Lockout", "Lockout Fog");
    three.category = Some("visuals");
    let four = Mod::new("four", "ascension", "Ascension Plus");
    let (id1, p1) = publish(&h, &a, &one).await;
    let (_, p2) = publish(&h, &b, &two).await;
    let (_, p3) = publish(&h, &a, &three).await;
    let (_, p4) = publish(&h, &b, &four).await;
    assert!(p1 < p2 && p2 < p3 && p3 < p4, "published in this order");

    // Newest first by default, each item with its own log position, and
    // the default order is sort=new.
    let all = list(&h, "").await;
    assert_eq!(
        names(&all),
        [
            "Ascension Plus",
            "Lockout Fog",
            "Midship Low Gravity",
            "Lockout Snipers"
        ]
    );
    let positions: Vec<u64> = all["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["position"].as_u64().unwrap())
        .collect();
    assert_eq!(positions, [p4, p3, p2, p1]);
    assert!(all["position"].as_u64().unwrap() >= p4);
    assert!(all["next"].is_null());
    assert_eq!(list(&h, "sort=new").await, all);
    let first = &all["items"][3];
    assert_eq!(first["map_id"], id1.to_string());
    assert_eq!(first["downloads"], 0);
    assert_eq!(first["category"], "weapons");
    assert_eq!(all["items"][1]["credit"], Value::Null);
    assert_eq!(all["items"][2]["credit"], "Old Forum Legend");

    // Pages of two: the cursor is the last item's position.
    let page1 = list(&h, "limit=2").await;
    assert_eq!(names(&page1), ["Ascension Plus", "Lockout Fog"]);
    assert_eq!(page1["next"], p3.to_string());
    let page2 = list(&h, &format!("limit=2&cursor={p3}")).await;
    assert_eq!(names(&page2), ["Midship Low Gravity", "Lockout Snipers"]);
    let page3 = list(
        &h,
        &format!("limit=2&cursor={}", page2["next"].as_str().unwrap()),
    )
    .await;
    assert!(names(&page3).is_empty());
    assert!(page3["next"].is_null());

    // Filters: base (any case), category, publisher, text.
    assert_eq!(
        names(&list(&h, "base=LOCKOUT").await),
        ["Lockout Fog", "Lockout Snipers"]
    );
    assert_eq!(
        names(&list(&h, "category=gameplay").await),
        ["Midship Low Gravity"]
    );
    assert_eq!(
        names(&list(&h, "author=Tinkerer").await),
        ["Ascension Plus", "Midship Low Gravity"]
    );
    assert!(names(&list(&h, "author=Nobody").await).is_empty());
    assert!(names(&list(&h, "author=not%20a%20tag%20at%20all%21%21").await).is_empty());
    assert_eq!(
        names(&list(&h, "q=SNIPERS").await),
        ["Lockout Snipers"],
        "the name, any case"
    );
    assert_eq!(
        names(&list(&h, "q=all%20the%20time").await),
        ["Lockout Snipers"],
        "the description"
    );
    assert_eq!(
        names(&list(&h, "q=forum").await),
        ["Midship Low Gravity"],
        "the credit"
    );
    assert_eq!(
        names(&list(&h, "q=tinker").await),
        ["Ascension Plus", "Midship Low Gravity"],
        "the publisher"
    );
    assert_eq!(
        names(&list(&h, "q=fog&base=lockout&category=visuals").await),
        ["Lockout Fog"]
    );
    // Empty parameters are absent ones.
    assert_eq!(
        names(&list(&h, "q=&base=&category=&cursor=").await).len(),
        4
    );

    // Bad queries are refused.
    for bad in [
        "sort=random",
        "category=cheats",
        "cursor=abc",
        "sort=popular&cursor=12",
        "sort=new&cursor=1:2",
        "limit=many",
    ] {
        let r = h.get(&format!("/v1/maps?{bad}")).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[tokio::test]
async fn downloads_count_once_per_signed_in_account() {
    let h = repo(Arc::new(MemoryStore::new()), policy()).await;
    let a = h.create("Modder").await;
    let x = h.create("PlayerX").await;
    let y = h.create("PlayerY").await;
    let (old, _) = publish(&h, &a, &Mod::new("old", "lockout", "Old Mod")).await;
    let (new, _) = publish(&h, &a, &Mod::new("new", "midship", "New Mod")).await;

    // Anonymous reads (the web) and a token that does not check out on a
    // public server read the manifest but count nothing.
    assert_eq!(
        h.get(&format!("/v1/maps/{old}")).await.status,
        StatusCode::OK
    );
    let bad = h
        .call(
            Method::GET,
            &format!("/v1/maps/{old}"),
            &[("authorization", "Bearer not.a.token".into())],
            Body::empty(),
        )
        .await;
    assert_eq!(bad.status, StatusCode::OK);
    assert_eq!(downloads(&h, &old).await, 0);

    // A signed-in fetch counts, once per account, and the answer says so.
    assert_eq!(fetch_as(&h, &x, &old).await["map"]["downloads"], 1);
    assert_eq!(fetch_as(&h, &x, &old).await["map"]["downloads"], 1);
    assert_eq!(fetch_as(&h, &y, &old).await["map"]["downloads"], 2);
    assert_eq!(fetch_as(&h, &y, &new).await["map"]["downloads"], 1);
    assert_eq!(downloads(&h, &old).await, 2);
    // An unknown package counts nothing and is still a 404.
    let none = MapId([0x77; 32]);
    let r = h
        .call(
            Method::GET,
            &format!("/v1/maps/{none}"),
            &[h.bearer(&x, lxm::READ)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // Most downloaded first; ties newest first; the cursor carries both.
    let popular = list(&h, "sort=popular").await;
    assert_eq!(names(&popular), ["Old Mod", "New Mod"]);
    assert_eq!(popular["items"][0]["downloads"], 2);
    assert_eq!(names(&list(&h, "sort=new").await), ["New Mod", "Old Mod"]);
    let p1 = list(&h, "sort=popular&limit=1").await;
    assert_eq!(names(&p1), ["Old Mod"]);
    let cursor = p1["next"].as_str().unwrap().to_string();
    assert!(cursor.starts_with("2:"), "{cursor}");
    let p2 = list(&h, &format!("sort=popular&limit=1&cursor={cursor}")).await;
    assert_eq!(names(&p2), ["New Mod"]);
    // A newer mod with as many downloads as New Mod comes before it.
    let (tied, _) = publish(&h, &a, &Mod::new("tied", "lockout", "Tied Mod")).await;
    fetch_as(&h, &x, &tied).await;
    assert_eq!(
        names(&list(&h, "sort=popular").await),
        ["Old Mod", "Tied Mod", "New Mod"],
        "one download each: the newer first"
    );

    // Who downloaded what is not public: the log hides the events from
    // anonymous readers.
    let events = h.get("/v1/events").await.json();
    assert!(
        events["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["type"] != MAP_DOWNLOADED),
        "MapDownloaded events are for admins"
    );
}

#[tokio::test]
async fn a_private_server_counts_the_reader_it_lets_in() {
    let h = repo(
        Arc::new(MemoryStore::new()),
        Policy {
            private: true,
            ..policy()
        },
    )
    .await;
    let a = h.create("Modder").await;
    let x = h.create("PlayerX").await;
    let (id, _) = publish(&h, &a, &Mod::new("p", "lockout", "Private Mod")).await;
    assert_eq!(
        h.get(&format!("/v1/maps/{id}")).await.status,
        StatusCode::UNAUTHORIZED
    );
    // One token, checked once: read and counted.
    assert_eq!(fetch_as(&h, &x, &id).await["map"]["downloads"], 1);
}

#[tokio::test]
async fn repository_extras_are_checked_and_listed() {
    let h = repo(Arc::new(MemoryStore::new()), policy()).await;
    let a = h.create("Modder").await;
    let mut m = Mod::new("extras", "lockout", "Lockout Classic");
    m.category = Some("cheats");
    let r = put_map(&h, &a, &m.doc()).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
    m.category = Some("gameplay");
    m.source_url = Some("javascript:alert(1)");
    let r = put_map(&h, &a, &m.doc()).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
    m.source_url = Some("https://example.invalid/mods/classic");
    m.credit = Some("Someone Else");
    publish(&h, &a, &m).await;
    let item = &list(&h, "").await["items"][0];
    assert_eq!(item["category"], "gameplay");
    assert_eq!(item["source_url"], "https://example.invalid/mods/classic");
    assert_eq!(item["credit"], "Someone Else");
    assert_eq!(item["author"]["gamertag"], "Modder");
}

#[tokio::test]
async fn downloads_are_replayed_from_the_log() {
    let store_dir = temp_dir();
    let data_dir = temp_dir();
    std::fs::create_dir_all(&data_dir).unwrap();
    let m = Mod::new("durable", "lockout", "Durable Mod");
    let id = MapId::of(&m.doc().manifest);
    {
        let store: Arc<dyn EventStore> = Arc::new(FileStore::open(&store_dir).await.unwrap());
        let h = start(store, policy()).await;
        h.live.set_data_dir(&data_dir);
        let a = h.create("Modder").await;
        let x = h.create("PlayerX").await;
        publish(&h, &a, &m).await;
        fetch_as(&h, &x, &id).await;
        fetch_as(&h, &a, &id).await;
    }
    let store: Arc<dyn EventStore> = Arc::new(FileStore::open(&store_dir).await.unwrap());
    let h = start(store, policy()).await;
    h.live.set_data_dir(&data_dir);
    assert_eq!(downloads(&h, &id).await, 2);
    assert_eq!(list(&h, "sort=popular").await["items"][0]["downloads"], 2);
    drop(h);
    let _ = std::fs::remove_dir_all(&store_dir);
    let _ = std::fs::remove_dir_all(&data_dir);
}
