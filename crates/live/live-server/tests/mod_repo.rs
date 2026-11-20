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

fn operator(h: &Harness) -> (&'static str, String) {
    (
        "authorization",
        format!(
            "Bearer {}",
            h.token(&h.operator, "server:live.test", lxm::CONTENT_MODERATE)
        ),
    )
}

async fn put_credit(h: &Harness, auth: (&'static str, String), id: &MapId, body: Value) -> Resp {
    h.call(
        Method::PUT,
        &format!("/v1/maps/{id}/credit"),
        &[auth],
        body.to_string(),
    )
    .await
}

#[tokio::test]
async fn the_credit_is_the_mods_makers_not_the_publishers() {
    let store_dir = temp_dir();
    let data_dir = temp_dir();
    std::fs::create_dir_all(&data_dir).unwrap();
    let mut m = Mod::new("credit", "zanzibar", "Beasthog");
    m.credit = Some("Importer Guess");
    m.source_url = Some("https://example.invalid/old");
    let id = MapId::of(&m.doc().manifest);
    {
        let store: Arc<dyn EventStore> = Arc::new(FileStore::open(&store_dir).await.unwrap());
        let h = start(store, policy()).await;
        h.live.set_data_dir(&data_dir);
        let a = h.create("Publisher").await;
        let x = h.create("Stranger").await;

        // The publisher's document credit wins over the manifest's author.
        let mut doc = m.doc();
        doc.credit = Some(live_proto::maps::Credit {
            authors: vec!["Group Bocked".into()],
            released: Some("2005".into()),
            ..Default::default()
        });
        let patch = m.patch();
        let r = h
            .call(
                Method::PUT,
                &format!("/v1/content/{}", Sha256::of(&patch)),
                &[h.bearer(&a, lxm::CONTENT_WRITE)],
                Body::from(patch),
            )
            .await;
        assert!(r.status.is_success(), "{}", r.error());
        let r = put_map(&h, &a, &doc).await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
        let item = &list(&h, "").await["items"][0];
        assert_eq!(item["author"]["gamertag"], "Publisher");
        assert_eq!(item["credit"], "Group Bocked");
        assert_eq!(item["credits"]["authors"][0], "Group Bocked");
        assert_eq!(item["credits"]["released"], "2005");
        assert_eq!(item["source_url"], "https://example.invalid/old");

        // Only an admin corrects it; a bad credit is refused.
        let fix = serde_json::json!({
            "authors": [" Synch ", "l PPF l NeoN"],
            "release": "Ownage Modz Pack",
            "released": "2006-01",
            "source_url": "https://example.invalid/new",
        });
        let r = put_credit(&h, h.bearer(&a, lxm::CONTENT_MODERATE), &id, fix.clone()).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.error());
        let r = put_credit(&h, h.bearer(&x, lxm::CONTENT_MODERATE), &id, fix.clone()).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.error());
        let r = put_credit(&h, operator(&h), &id, serde_json::json!({"released": "05"})).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
        let r = put_credit(&h, operator(&h), &id, serde_json::json!({"who": "x"})).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
        let r = put_credit(&h, operator(&h), &MapId([9; 32]), fix.clone()).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.error());
        let r = put_credit(&h, operator(&h), &id, fix.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.error());
        let answer = r.json();
        assert_eq!(answer["map"]["credit"], "Synch & l PPF l NeoN");

        let detail = h.get(&format!("/v1/maps/{id}")).await.json();
        let s = &detail["map"];
        assert_eq!(
            s["credits"]["authors"],
            serde_json::json!(["Synch", "l PPF l NeoN"])
        );
        assert_eq!(s["credits"]["release"], "Ownage Modz Pack");
        assert_eq!(s["source_url"], "https://example.invalid/new");
        assert_eq!(s["author"]["gamertag"], "Publisher", "the publisher stays");
        // The manifest is the publisher's, as signed.
        assert_eq!(s["manifest"]["package"]["author"], "Importer Guess");
        assert_eq!(s["map_id"], id.to_string());
        // Searching finds the credited author.
        assert_eq!(names(&list(&h, "q=synch").await), ["Beasthog"]);
        assert_eq!(names(&list(&h, "q=importer").await), Vec::<String>::new());
    }
    // Replayed from the log; then cleared back to the publisher's.
    let store: Arc<dyn EventStore> = Arc::new(FileStore::open(&store_dir).await.unwrap());
    let h = start(store, policy()).await;
    h.live.set_data_dir(&data_dir);
    let item = &list(&h, "").await["items"][0];
    assert_eq!(item["credit"], "Synch & l PPF l NeoN");
    let r = put_credit(&h, operator(&h), &id, serde_json::json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let item = &list(&h, "").await["items"][0];
    assert_eq!(item["credit"], "Group Bocked");
    drop(h);
    let _ = std::fs::remove_dir_all(&store_dir);
    let _ = std::fs::remove_dir_all(&data_dir);
}

async fn upload(h: &Harness, u: &User, bytes: Vec<u8>) {
    let r = h
        .call(
            Method::PUT,
            &format!("/v1/content/{}", Sha256::of(&bytes)),
            &[h.bearer(u, lxm::CONTENT_WRITE)],
            Body::from(bytes),
        )
        .await;
    assert!(r.status.is_success(), "{}", r.error());
}

async fn put_admin(h: &Harness, auth: (&'static str, String), path: &str, body: Value) -> Resp {
    h.call(Method::PUT, path, &[auth], body.to_string()).await
}

/// An admin's details (docs/game/maps.md §5, "Details"): a longer
/// description, screenshots served from the content store, a contents
/// report and the map pack, over the publisher's manifest, kept in the
/// log; and the pack's own info, listed with the maps that name it.
#[tokio::test]
async fn details_and_packs_are_an_admins_and_are_listed() {
    let store_dir = temp_dir();
    let data_dir = temp_dir();
    std::fs::create_dir_all(&data_dir).unwrap();
    let mut m = Mod::new("details", "zanzibar", "Zombiebar");
    m.description = Some("Short.");
    let id = MapId::of(&m.doc().manifest);
    let shot = b"\x89PNG\r\n\x1a\ninvented screenshot".to_vec();
    let menu = b"\xff\xd8\xffinvented pack menu".to_vec();
    let text = b"not an image".to_vec();
    {
        let store: Arc<dyn EventStore> = Arc::new(FileStore::open(&store_dir).await.unwrap());
        let h = start(store, policy()).await;
        h.live.set_data_dir(&data_dir);
        let a = h.create("Publisher").await;
        upload(&h, &a, m.patch()).await;
        let r = put_map(&h, &a, &m.doc()).await;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
        for b in [&shot, &menu, &text] {
            upload(&h, &a, b.clone()).await;
        }
        let path = format!("/v1/maps/{id}/details");
        let details = serde_json::json!({
            "description": "A longer one.\nWith zombies.",
            "images": [{"sha256": Sha256::of(&shot).to_string(), "caption": "the bar",
                        "credit": "Badsmiley", "source_url": "https://example.invalid/post"}],
            "contents": {"lines": ["new AI characters: 2"], "scripts": true},
            "features": ["scripts", "ai"],
            "pack": "ai-revision"
        });
        // Only an admin; checked; the images must be stored images.
        let r = put_admin(
            &h,
            h.bearer(&a, lxm::CONTENT_MODERATE),
            &path,
            details.clone(),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.error());
        for bad in [
            serde_json::json!({"blurb": "x"}),
            serde_json::json!({"features": ["cheats"]}),
            serde_json::json!({"pack": "AI Revision"}),
            serde_json::json!({"images": [{"sha256": "07".repeat(32)}]}),
            serde_json::json!({"images": [{"sha256": Sha256::of(&text).to_string()}]}),
        ] {
            let r = put_admin(&h, operator(&h), &path, bad.clone()).await;
            assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}: {}", r.error());
        }
        let r = put_admin(
            &h,
            operator(&h),
            &format!("/v1/maps/{}/details", MapId([9; 32])),
            details.clone(),
        )
        .await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{}", r.error());
        let r = put_admin(&h, operator(&h), &path, details.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.error());
        assert_eq!(
            r.json()["map"]["description"],
            "A longer one.\nWith zombies."
        );

        // The pack's info.
        let pack = serde_json::json!({
            "name": "Halo 2 Remapped AI Revision", "authors": ["Zone 117"],
            "released": "2007-06", "description": "Custom maps with AI.",
            "images": [{"sha256": Sha256::of(&menu).to_string(), "caption": "the menu"}]
        });
        let r = put_admin(
            &h,
            h.bearer(&a, lxm::CONTENT_MODERATE),
            "/v1/mappacks/ai-revision",
            pack.clone(),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.error());
        let r = put_admin(&h, operator(&h), "/v1/mappacks/AI%20Revision", pack.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
        let r = put_admin(
            &h,
            operator(&h),
            "/v1/mappacks/ai-revision",
            serde_json::json!({"name": " "}),
        )
        .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
        let r = put_admin(&h, operator(&h), "/v1/mappacks/ai-revision", pack.clone()).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    }
    // Replayed from the log.
    let store: Arc<dyn EventStore> = Arc::new(FileStore::open(&store_dir).await.unwrap());
    let h = start(store, policy()).await;
    h.live.set_data_dir(&data_dir);
    let page = list(&h, "").await;
    let item = &page["items"][0];
    assert_eq!(item["description"], "A longer one.\nWith zombies.");
    assert_eq!(item["images"][0]["caption"], "the bar");
    assert_eq!(
        item["image"],
        Sha256::of(&shot).to_string(),
        "no preview: the first shot"
    );
    assert_eq!(item["features"], serde_json::json!(["scripts", "ai"]));
    assert_eq!(
        item["contents"],
        serde_json::json!(["new AI characters: 2"])
    );
    assert_eq!(item["pack"], "ai-revision");
    assert_eq!(item["pack_name"], "Halo 2 Remapped AI Revision");
    assert_eq!(page["packs"][0]["id"], "ai-revision");
    assert_eq!(page["packs"][0]["authors"][0], "Zone 117");
    assert_eq!(
        names(&list(&h, "q=zombies").await),
        ["Zombiebar"],
        "the long description is searched"
    );

    let detail = h.get(&format!("/v1/maps/{id}")).await.json();
    let d = &detail["map"];
    assert_eq!(d["report"]["scripts"], true);
    assert_eq!(d["pack_info"]["name"], "Halo 2 Remapped AI Revision");
    assert_eq!(
        d["manifest"]["package"]["description"], "Short.",
        "the manifest is as signed"
    );

    let r = h.get(&format!("/v1/maps/{id}/images/0")).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.headers["content-type"], "image/png");
    assert_eq!(r.body, shot);
    assert_eq!(h.get(&format!("/v1/maps/{id}/image")).await.body, shot);
    assert_eq!(
        h.get(&format!("/v1/maps/{id}/images/1")).await.status,
        StatusCode::NOT_FOUND
    );
    let r = h.get("/v1/mappacks/ai-revision/images/0").await;
    assert_eq!(r.headers["content-type"], "image/jpeg");
    assert_eq!(r.body, menu);
    let packs = h.get("/v1/mappacks").await.json();
    assert_eq!(packs["items"][0]["id"], "ai-revision");

    // Cleared: the publisher's again, and the pack gone.
    let r = put_admin(
        &h,
        operator(&h),
        &format!("/v1/maps/{id}/details"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let r = put_admin(&h, operator(&h), "/v1/mappacks/ai-revision", Value::Null).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.error());
    let page = list(&h, "").await;
    let item = &page["items"][0];
    assert_eq!(item["description"], "Short.");
    assert!(
        item.get("images").is_none() && item.get("pack").is_none() && item.get("image").is_none()
    );
    assert!(page.get("packs").is_none());
    assert!(
        h.get("/v1/mappacks").await.json()["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop(h);
    let _ = std::fs::remove_dir_all(&store_dir);
    let _ = std::fs::remove_dir_all(&data_dir);
}

/// The Mods page's checklist data (docs/game/maps.md §5, "A mod's page"):
/// features, game types and version are checked at publish and listed,
/// in the list and on the map's own answer; a document without them
/// lists none.
#[tokio::test]
async fn features_game_types_and_version_are_checked_and_listed() {
    let h = repo(Arc::new(MemoryStore::new()), policy()).await;
    let a = h.create("Modder").await;
    let m = Mod::new("features", "lockout", "Lockout Hogs");
    let patch = m.patch();
    let r = h
        .call(
            Method::PUT,
            &format!("/v1/content/{}", Sha256::of(&patch)),
            &[h.bearer(&a, lxm::CONTENT_WRITE)],
            Body::from(patch),
        )
        .await;
    assert!(r.status.is_success(), "{}", r.error());
    let mut doc = m.doc();
    doc.features = vec!["vehicles".into(), "cheats".into()];
    let r = put_map(&h, &a, &doc).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
    doc.features = vec!["vehicles".into(), "scripts".into()];
    doc.gametypes = vec!["ctf".into(), "slayer".into()];
    doc.version = Some("1.2".into());
    let r = put_map(&h, &a, &doc).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let item = &list(&h, "").await["items"][0];
    assert_eq!(item["features"], serde_json::json!(["vehicles", "scripts"]));
    assert_eq!(item["gametypes"], serde_json::json!(["ctf", "slayer"]));
    assert_eq!(item["version"], "1.2");
    let id = MapId::of(&doc.manifest);
    let one = h.get(&format!("/v1/maps/{id}")).await.json();
    assert_eq!(one["map"]["features"], item["features"]);

    publish(&h, &a, &Mod::new("plain", "lockout", "Lockout Plain")).await;
    let plain = &list(&h, "").await["items"][0];
    assert_eq!(plain["name"], "Lockout Plain");
    assert!(plain.get("features").is_none() && plain.get("version").is_none());
}

#[tokio::test]
async fn staged_manifests_are_private_in_the_log_and_snapshot() {
    let h = repo(Arc::new(MemoryStore::new()), policy()).await;
    let owner = h.create("Uploader").await;
    let reader = h.create("Reader").await;
    let member = Mod::new("staged", "lockout", "Sample: Lockout");
    let patch = member.patch();
    let upload = h.call(
        Method::PUT,
        &format!("/v1/content/{}", Sha256::of(&patch)),
        &[h.bearer(&owner, lxm::CONTENT_WRITE)],
        Body::from(patch),
    ).await;
    assert!(upload.status.is_success(), "{}", upload.error());
    let mut doc = member.doc();
    doc.staged_for = Some(live_proto::maps::mod_pack_id_for(owner.xuid, "Sample"));
    let staged = put_map(&h, &owner, &doc).await;
    assert_eq!(staged.status, StatusCode::CREATED, "{}", staged.error());
    for (path, field) in [("/v1/events", "items"), ("/v1/snapshot", "tail")] {
        for (account, visible) in [(None, false), (Some(&reader), false), (Some(&owner), true)] {
            let headers: Vec<_> = account.map(|u| h.bearer(u, lxm::READ)).into_iter().collect();
            let response = h.call(Method::GET, path, &headers, Body::empty()).await;
            assert_eq!(response.status, StatusCode::OK, "{}", response.error());
            let events = response.json();
            let found = events[field].as_array().unwrap().iter().any(|e| e["type"] == MAP_PUBLISHED);
            assert_eq!(found, visible, "{path}");
        }
    }
}

#[tokio::test]
async fn uploader_gallery_slots_are_bounded_replayable_and_independent() {
    use image::{ExtendedColorType, ImageEncoder};
    fn shot(rgb: [u8; 3]) -> Vec<u8> {
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&rgb, 1, 1, ExtendedColorType::Rgb8)
            .unwrap();
        png
    }
    async fn put(h: &Harness, u: &User, uri: &str, bytes: Vec<u8>) -> Resp {
        h.call(
            Method::PUT,
            uri,
            &[h.bearer(u, lxm::CONTENT_WRITE)],
            Body::from(bytes),
        )
        .await
    }

    let store = Arc::new(MemoryStore::new());
    let h = repo(store.clone(), policy()).await;
    let owner = h.create("Painter").await;
    let visitor = h.create("Viewer").await;
    let (id, _) = publish(&h, &owner, &Mod::new("paint", "lockout", "Colors: lockout")).await;
    let pack_list = h.get("/v1/mod-packs").await.json();
    let pack_id = pack_list["items"][0]["id"].as_str().unwrap();
    let map = format!("/v1/maps/{id}");
    let pack = format!("/v1/mod-packs/{pack_id}");
    let cover = shot([32, 64, 128]);
    let scene = shot([32, 128, 64]);
    for uri in [&map, &pack] {
        let upload = format!("{uri}/screenshots/upload/0");
        let capture = format!("{uri}/screenshots/capture/4");
        assert_eq!(
            put(&h, &visitor, &upload, cover.clone()).await.status,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            put(
                &h,
                &owner,
                &format!("{uri}/screenshots/capture/5"),
                scene.clone()
            )
            .await
            .status,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            put(&h, &owner, &capture, b"\x89PNG\r\n\x1a\n".to_vec())
                .await
                .status,
            StatusCode::BAD_REQUEST
        );
        let captured = put(&h, &owner, &capture, scene.clone()).await;
        assert_eq!(captured.status, StatusCode::OK);
        let uploaded = put(&h, &owner, &upload, cover.clone()).await;
        assert_eq!(uploaded.status, StatusCode::OK);
        let read = h.get(&format!("{uri}/images/0")).await;
        assert_eq!(read.status, StatusCode::OK);
        assert_eq!(read.headers["content-type"], "image/webp");
        assert_ne!(read.body, cover);
        assert_eq!(
            Sha256::of(&read.body).to_string(),
            uploaded.json()["sha256"].as_str().unwrap()
        );
        let read = h.get(&format!("{uri}/images/1")).await;
        assert_eq!(
            Sha256::of(&read.body).to_string(),
            captured.json()["sha256"].as_str().unwrap()
        );
        let repeated = put(&h, &owner, &capture, scene.clone()).await;
        assert_eq!(repeated.json()["sha256"], captured.json()["sha256"]);
        assert_eq!(
            h.get(&format!("{uri}/images/2")).await.status,
            StatusCode::NOT_FOUND
        );
    }

    let before = h.get(&map).await.json()["map"]["images"].clone();
    let loaded = repo(store, policy()).await;
    assert_eq!(loaded.get(&map).await.json()["map"]["images"], before);
    assert_eq!(loaded.get(&pack).await.json()["pack"]["images"], before);
}

#[tokio::test]
async fn pack_migration_groups_by_base_suffix_and_uploader_and_replays() {
    let store: Arc<dyn EventStore> = Arc::new(MemoryStore::new());
    let expected;
    {
        let h = repo(store.clone(), policy()).await;
        let a = h.create("PublisherA").await;
        let b = h.create("PublisherB").await;
        let first = Mod::new("a", "lockout", "Night: Lockout");
        let second = Mod::new("b", "midship", "Night: midship");
        let third = Mod::new("c", "lockout", "Night: Lockout");
        let solo = Mod::new("solo", "zanzibar", "Night: Low Gravity");
        let (member, _) = publish(&h, &a, &first).await;
        publish(&h, &a, &second).await;
        publish(&h, &b, &third).await;
        publish(&h, &a, &solo).await;
        let id = live_proto::maps::mod_pack_id_for(a.xuid, "Night");
        let page = h.get("/v1/mod-packs").await.json();
        let packs = page["items"].as_array().unwrap();
        assert_eq!(packs.len(), 3);
        assert_eq!(packs[0]["name"], "Night: Low Gravity");
        let migrated = h.get(&format!("/v1/mod-packs/{id}")).await.json();
        assert_eq!(migrated["pack"]["map_count"], 2);
        assert_eq!(migrated["pack"]["name"], "Night");
        assert_eq!(migrated["pack"]["members"][0]["map_id"], member.to_string());
        assert_eq!(migrated["pack"]["uploader"]["gamertag"], "PublisherA");
        assert_eq!(h.get(&format!("/v1/maps/{member}")).await.json()["map"]["mod_pack"], id);
        expected = page["items"].clone();
    }
    let h = repo(store, policy()).await;
    assert_eq!(h.get("/v1/mod-packs").await.json()["items"], expected);
}

#[tokio::test]
async fn publishing_an_owned_pack_is_atomic_and_keeps_member_credits() {
    let h = repo(Arc::new(MemoryStore::new()), policy()).await;
    let owner = h.create("Uploader").await;
    let stranger = h.create("Stranger").await;
    let path = "/v1/mod-packs/lan-edition";
    let a = Mod::new("shared", "lockout", "First Map");
    let mut b = Mod::new("shared", "midship", "Second Map");
    b.credit = Some("Map Maker");
    let layer = a.patch();
    let r = h.call(Method::PUT, &format!("/v1/content/{}", Sha256::of(&layer)),
        &[h.bearer(&owner, lxm::CONTENT_WRITE)], Body::from(layer.clone())).await;
    assert!(r.status.is_success());
    let mut ids = Vec::new();
    for m in [a, b] {
        let mut doc = m.doc();
        doc.staged_for = Some("lan-edition".into());
        let r = put_map(&h, &owner, &doc).await;
        assert_eq!(r.status, StatusCode::CREATED);
        ids.push(MapId::of(&doc.manifest));
    }
    assert!(h.get("/v1/mod-packs").await.json()["items"].as_array().unwrap().is_empty());
    assert!(list(&h, "").await["items"].as_array().unwrap().is_empty());
    assert_eq!(h.get(&format!("/v1/maps/{}", ids[0])).await.status, StatusCode::GONE);
    let body = serde_json::json!({"name":"LAN Edition","authors":["Editor","Builder"],
        "description":"Two maps","version":"1.2","source_url":"https://example.test/mods/lan",
        "year":2006,"advisory":"not_for_online","members":ids});
    let put = |user: &User, body: Value| put_admin(&h, h.bearer(user, lxm::CONTENT_WRITE), path, body);
    assert_eq!(put(&stranger, body.clone()).await.status, StatusCode::FORBIDDEN);
    for patch in [
        serde_json::json!({"members":[ids[0], ids[0]]}),
        serde_json::json!({"authors":[]}),
        serde_json::json!({"year":12}),
        serde_json::json!({"advisory":"nonsense"}),
        serde_json::json!({"images":[]}),
    ] {
        let mut invalid = body.clone();
        invalid.as_object_mut().unwrap().extend(patch.as_object().unwrap().clone());
        assert_eq!(put(&owner, invalid).await.status, StatusCode::BAD_REQUEST);
        assert_eq!(h.get(path).await.status, StatusCode::NOT_FOUND);
    }
    let mut missing = body.clone();
    missing["members"] = serde_json::json!([ids[0], MapId([0x11;32])]);
    assert_eq!(put(&owner, missing).await.status, StatusCode::NOT_FOUND);
    let response = put(&owner, body.clone()).await.json();
    let release = &response["pack"];
    let typed: live_proto::maps::ModPackSummary = serde_json::from_value(release.clone()).unwrap();
    assert_eq!(typed.map_count, 2);
    assert_eq!(typed.size, layer.len() as u64);
    assert_eq!(release["authors"], body["authors"]);
    assert_eq!(release["year"], 2006);
    assert_eq!(release["advisory"], "not_for_online");
    assert_eq!(release["members"][0]["credit"], "Editor & Builder");
    assert_eq!(release["members"][1]["credit"], "Map Maker");
    assert_eq!(h.get(&format!("/v1/maps/{}", ids[0])).await.json()["map"]["credit"], "Editor & Builder");
    assert_eq!(put(&owner, body.clone()).await.json(), response, "idempotent retry");
    assert_eq!(put(&stranger, body).await.status, StatusCode::FORBIDDEN);
    assert_eq!(h.get("/v1/mod-packs").await.json()["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn curated_pack_credit_and_release_year_survive_migration() {
    let h = repo(Arc::new(MemoryStore::new()), policy()).await;
    let a = h.create("FirstUploader").await;
    let b = h.create("NextUploader").await;
    let (one, _) = publish(&h, &a, &Mod::new("one", "lockout", "Custom A")).await;
    let (two, _) = publish(&h, &b, &Mod::new("two", "lockout", "Custom B")).await;
    let details = serde_json::json!({"pack": "classic-edition"});
    for member in [one, two] {
        let r = put_admin(&h, operator(&h), &format!("/v1/maps/{member}/details"), details.clone()).await;
        assert_eq!(r.status, StatusCode::OK);
    }
    let r = put_admin(&h, operator(&h), "/v1/mappacks/classic-edition", serde_json::json!({
        "name":"Classic Edition", "authors":["The Cartographers"], "released":"2005-09",
        "source_url":"https://example.test/editions/classic"
    })).await;
    assert_eq!(r.status, StatusCode::OK);
    let page = h.get("/v1/mod-packs").await.json();
    let packs = page["items"].as_array().unwrap();
    assert_eq!(packs.len(), 2);
    for p in packs {
        assert_eq!(p["map_count"], 1);
        assert_eq!(p["year"], 2005);
        assert_eq!(p["authors"][0], "The Cartographers");
        assert_eq!(p["members"][0]["credit"], "The Cartographers");
        assert_eq!(p["members"][0]["source_url"], p["source_url"]);
    }
    assert_ne!(packs[0]["id"], packs[1]["id"]);
    assert_ne!(packs[0]["uploader"], packs[1]["uploader"]);
}

#[tokio::test]
async fn abandoned_staging_can_be_retargeted_by_the_uploader() {
    let store = Arc::new(MemoryStore::new());
    let h = repo(store.clone(), policy()).await;
    let u = h.create("Uploader").await;
    let m = Mod::new("abandoned", "lockout", "One");
    let patch = m.patch();
    let upload = h.call(Method::PUT, &format!("/v1/content/{}", Sha256::of(&patch)),
        &[h.bearer(&u, lxm::CONTENT_WRITE)], Body::from(patch)).await;
    assert!(upload.status.is_success());
    let mut doc = m.doc();
    let id = MapId::of(&doc.manifest);
    for key in ["old-name", "final-name"] {
        doc.staged_for = Some(key.into());
        assert_eq!(put_map(&h, &u, &doc).await.status, StatusCode::CREATED);
        assert!(list(&h, "").await["items"].as_array().unwrap().is_empty());
    }
    let path = "/v1/mod-packs/final-name";
    let r = put_admin(&h, h.bearer(&u, lxm::CONTENT_WRITE), path, serde_json::json!({
        "name":"Final Name", "authors":["A"], "members":[id]
    })).await;
    assert_eq!(r.status, StatusCode::OK);
    let pack = r.json()["pack"].clone();
    drop(h);
    let loaded = repo(store, policy()).await;
    assert_eq!(loaded.get(path).await.json()["pack"], pack);
    assert_eq!(loaded.get("/v1/mod-packs/old-name").await.status, StatusCode::NOT_FOUND);
    assert_eq!(loaded.get(&format!("/v1/maps/{id}")).await.json()["map"]["mod_pack"], "final-name");
}
