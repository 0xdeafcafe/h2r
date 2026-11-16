//! The content store for custom maps (docs/game/maps.md §5, docs/live/live-api.md
//! "Custom maps", docs/live/live-admin.md "Custom content"), through the router.
//! Every byte here is invented: the "maps" are a `daeh` tag and filler.
mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::*;
use content::manifest::{Layer, ResultInfo};
use content::{Manifest, MapId, Sha256};
use live_proto::lxm;
use live_proto::maps::{MAP_PUBLISHED, MapDocument, package_stream};
use live_proto::session::{NewSession, Slots};
use live_proto::sign::EventBody;
use live_server::content::{ContentLimits, RETAIL_FILE};
use live_server::store::{EventStore, MemoryStore};
use live_server::{Policy, filestore::FileStore};
use serde_json::{Value, json};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really an image";

fn sha(bytes: &[u8]) -> Sha256 {
    Sha256::of(bytes)
}

fn small() -> Policy {
    Policy {
        content: ContentLimits {
            layer_bytes: 64,
            map_bytes: 256,
            quota_bytes: 400,
            upload_rate: (100.0, 100),
        },
        ..Policy::default()
    }
}

async fn content_harness(policy: Policy) -> Harness {
    let h = start(Arc::new(MemoryStore::new()), policy).await;
    h.live.set_data_dir(&h.dir);
    h
}

async fn put(h: &Harness, u: &User, hash: &Sha256, bytes: &[u8]) -> Resp {
    h.call(
        Method::PUT,
        &format!("/v1/content/{hash}"),
        &[h.bearer(u, lxm::CONTENT_WRITE)],
        Body::from(bytes.to_vec()),
    )
    .await
}

async fn upload(h: &Harness, u: &User, bytes: &[u8]) -> Sha256 {
    let hash = sha(bytes);
    let r = put(h, u, &hash, bytes).await;
    assert!(
        r.status == StatusCode::CREATED || r.status == StatusCode::OK,
        "{}: {}",
        r.status,
        String::from_utf8_lossy(&r.body)
    );
    hash
}

/// A Lockout mod: one invented patch over an invented base hash.
fn manifest(patch: &[u8]) -> Manifest {
    let mut m = Manifest::retail("lockout", Sha256([0x11; 32]));
    m.package.name = "Lockout Snipers".into();
    m.package.description = Some("snipers only".into());
    m.patches = vec![Layer {
        file: "snipers.ips".into(),
        sha256: sha(patch),
    }];
    m
}

fn signed(u: &User, doc: &MapDocument) -> String {
    let id = MapId::of(&doc.manifest);
    EventBody::at_unix(
        &u.key,
        &u.author,
        &package_stream(&id),
        MAP_PUBLISHED,
        serde_json::to_value(doc).unwrap(),
        now(),
    )
    .sign(&u.key)
}

async fn publish(h: &Harness, u: &User, doc: &MapDocument) -> Resp {
    h.call(
        Method::PUT,
        "/v1/maps",
        &[h.bearer(u, lxm::CONTENT_WRITE)],
        signed(u, doc),
    )
    .await
}

fn doc(m: Manifest) -> MapDocument {
    MapDocument {
        manifest: m,
        image: None,
    }
}

fn moderator(h: &Harness) -> (&'static str, String) {
    (
        "authorization",
        format!(
            "Bearer {}",
            h.token(&h.operator, "server:live.test", lxm::CONTENT_MODERATE)
        ),
    )
}

async fn moderate(h: &Harness, what: &str, id: &str, action: &str) -> Resp {
    h.call(
        Method::POST,
        &format!("/v1/admin/{what}/{id}/{action}"),
        &[moderator(h)],
        Body::empty(),
    )
    .await
}

fn session(flags: &str, hopper: Option<u32>, map_id: Option<MapId>) -> NewSession {
    NewSession {
        addr: Some("192.168.2.4".parse().unwrap()),
        port: 3074,
        host_address: vec![0; 36],
        slots: Slots {
            public_filled: 1,
            public_open: 7,
            ..Default::default()
        },
        attributes: BTreeMap::new(),
        flags: flags.into(),
        hopper_id: hopper,
        map: Some("lockout".into()),
        name: None,
        language: None,
        visibility: Default::default(),
        map_id,
        skulls: None,
        campaign: None,
    }
}

async fn advertise(h: &Harness, u: &User, s: &NewSession) -> Resp {
    h.call(
        Method::POST,
        "/v1/sessions",
        &[
            h.bearer(u, lxm::SESSIONS_WRITE),
            ("content-type", "application/json".into()),
        ],
        Body::from(serde_json::to_vec(s).unwrap()),
    )
    .await
}

#[tokio::test]
async fn blobs_are_checked_stored_once_and_served() {
    let h = content_harness(small()).await;
    let u = h.create("Modder").await;
    let bytes = b"PATCH invented bytes".as_slice();
    let hash = sha(bytes);

    // Signed writes only, by an account.
    let anon = h
        .call(
            Method::PUT,
            &format!("/v1/content/{hash}"),
            &[],
            Body::from(bytes),
        )
        .await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
    let wrong = h
        .call(
            Method::PUT,
            &format!("/v1/content/{hash}"),
            &[h.bearer(&u, lxm::EVENTS_WRITE)],
            Body::from(bytes),
        )
        .await;
    assert_eq!(
        wrong.status,
        StatusCode::UNAUTHORIZED,
        "token for another method"
    );

    // The body must hash to the path.
    let r = put(&h, &u, &sha(b"something else"), bytes).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.error());
    let r = put(&h, &u, &hash, bytes).await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json()["size"], bytes.len());
    assert_eq!(r.json()["stored"], true);
    // Again: already there, nothing rewritten.
    let r = put(&h, &u, &hash, bytes).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.json()["stored"], false);

    let path = h
        .dir
        .join("content")
        .join(&hash.to_string()[..2])
        .join(hash.to_string());
    assert_eq!(
        std::fs::read(&path).unwrap(),
        bytes,
        "<data>/content/<aa>/<hash>"
    );
    assert!(
        std::fs::read_dir(h.dir.join("tmp"))
            .unwrap()
            .next()
            .is_none(),
        "no temporary files left"
    );

    let got = h.get(&format!("/v1/content/{hash}")).await;
    assert_eq!(got.status, StatusCode::OK);
    assert_eq!(got.body, bytes);
    assert_eq!(got.headers["etag"].to_str().unwrap(), format!("\"{hash}\""));
    assert_eq!(
        h.get(&format!("/v1/content/{}", sha(b"never")))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.get("/v1/content/nothex").await.status,
        StatusCode::NOT_FOUND
    );
    // The metadata is a server-signed event on the blob's stream.
    let ev = h
        .get(&format!("/v1/events?stream=content-{hash}"))
        .await
        .json();
    assert_eq!(ev["items"][0]["type"], "ContentStored");
    assert_eq!(ev["items"][0]["data"]["uploader"], u.xuid.to_string());
    assert_eq!(ev["items"][0]["author"], "server:live.test");
}

#[tokio::test]
async fn caps_quota_and_upload_switch() {
    let h = content_harness(small()).await;
    let u = h.create("Modder").await;
    // A layer is at most 64 bytes here; a whole map ("daeh" first) 256.
    let layer = vec![b'p'; 65];
    let r = put(&h, &u, &sha(&layer), &layer).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE, "{}", r.error());
    let mut map = b"daeh".to_vec();
    map.resize(200, 7);
    upload(&h, &u, &map).await;
    let mut big = b"daeh".to_vec();
    big.resize(257, 8);
    let r = put(&h, &u, &sha(&big), &big).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    // Quota: 400 bytes per account; 200 used.
    let mut map2 = b"daeh".to_vec();
    map2.resize(201, 9);
    let r = put(&h, &u, &sha(&map2), &map2).await;
    assert_eq!(r.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert!(r.error().contains("quota") || r.json()["message"].as_str().unwrap().contains("quota"));
    // Another account has its own.
    let other = h.create("Other").await;
    upload(&h, &other, &map2).await;
    // Deleting frees the quota.
    let first = sha(&map);
    assert_eq!(
        moderate(&h, "content", &first.to_string(), "delete")
            .await
            .status,
        StatusCode::OK
    );
    let mut map3 = b"daeh".to_vec();
    map3.resize(150, 3);
    upload(&h, &u, &map3).await;

    // An admin switches an account's uploads off, and on again.
    let off = h
        .call(
            Method::PUT,
            "/v1/admin/uploads/Modder",
            &[moderator(&h)],
            json!({"allowed": false}).to_string(),
        )
        .await;
    assert_eq!(off.status, StatusCode::OK, "{}", off.error());
    let r = put(&h, &u, &sha(b"PATCH x"), b"PATCH x").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let not_admin = h
        .call(
            Method::PUT,
            "/v1/admin/uploads/Modder",
            &[h.bearer(&other, lxm::CONTENT_MODERATE)],
            json!({"allowed": true}).to_string(),
        )
        .await;
    assert_eq!(not_admin.status, StatusCode::FORBIDDEN);
    let on = h
        .call(
            Method::PUT,
            "/v1/admin/uploads/Modder",
            &[moderator(&h)],
            json!({"allowed": true}).to_string(),
        )
        .await;
    assert_eq!(on.status, StatusCode::OK);
    upload(&h, &u, b"PATCH x").await;
}

#[tokio::test]
async fn uploads_have_a_per_account_rate() {
    let mut p = small();
    p.content.upload_rate = (0.001, 2);
    let h = content_harness(p).await;
    let u = h.create("Modder").await;
    upload(&h, &u, b"PATCH 1").await;
    upload(&h, &u, b"PATCH 2").await;
    let r = put(&h, &u, &sha(b"PATCH 3"), b"PATCH 3").await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    let other = h.create("Other").await;
    upload(&h, &other, b"PATCH 3").await;
}

#[tokio::test]
async fn retail_bytes_are_never_kept() {
    let h = content_harness(small()).await;
    let u = h.create("Modder").await;
    // The operator's list: sha256sum output, invented here.
    let mut retail = b"daeh".to_vec();
    retail.resize(100, 0x5a);
    let retail_hash = sha(&retail);
    std::fs::write(
        h.dir.join(RETAIL_FILE),
        format!("# the disc\n{retail_hash}  lockout.map\n"),
    )
    .unwrap();
    let r = put(&h, &u, &retail_hash, &retail).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.error());
    assert!(
        !h.dir
            .join("content")
            .join(&retail_hash.to_string()[..2])
            .exists()
    );

    // A manifest that is only a retail base shares nothing.
    let bare = Manifest::retail("lockout", retail_hash);
    let r = publish(&h, &u, &doc(bare.clone())).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.error());
    // Nor one that names a retail map as its layer.
    let mut m = bare;
    m.base.derived = true;
    m.base.sha256 = None;
    m.patches = vec![Layer {
        file: "lockout.map".into(),
        sha256: retail_hash,
    }];
    let r = publish(&h, &u, &doc(m)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn packages_are_signed_checked_and_listed() {
    let h = content_harness(small()).await;
    let u = h.create("Modder").await;
    let patch = b"PATCH snipers".as_slice();
    let mut m = manifest(patch);
    m.package.image = Some("preview.png".into());
    m.result = Some(ResultInfo {
        geometry: Some(Sha256([0x22; 32])),
    });
    let id = MapId::of(&m);
    let d = MapDocument {
        manifest: m.clone(),
        image: Some(sha(PNG)),
    };

    // Layers first.
    let r = publish(&h, &u, &d).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.error());
    assert!(
        r.json()["message"]
            .as_str()
            .unwrap()
            .contains("snipers.ips")
    );
    upload(&h, &u, patch).await;
    let r = publish(&h, &u, &d).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "the image too");
    // An image must be a PNG or JPEG.
    let gif = b"GIF89a invented".as_slice();
    upload(&h, &u, gif).await;
    let bad = MapDocument {
        manifest: m.clone(),
        image: Some(sha(gif)),
    };
    assert_eq!(publish(&h, &u, &bad).await.status, StatusCode::BAD_REQUEST);
    upload(&h, &u, PNG).await;
    // The manifest names an image exactly when the document gives one.
    let none = MapDocument {
        manifest: m.clone(),
        image: None,
    };
    assert_eq!(publish(&h, &u, &none).await.status, StatusCode::BAD_REQUEST);
    // Signed by someone else, or on the wrong stream: refused.
    let other = h.create("Other").await;
    let forged = h
        .call(
            Method::PUT,
            "/v1/maps",
            &[h.bearer(&other, lxm::CONTENT_WRITE)],
            signed(&u, &d),
        )
        .await;
    assert_eq!(forged.status, StatusCode::FORBIDDEN);
    let wrong_stream = EventBody::at_unix(
        &u.key,
        &u.author,
        &package_stream(&MapId([9; 32])),
        MAP_PUBLISHED,
        serde_json::to_value(&d).unwrap(),
        now(),
    )
    .sign(&u.key);
    let r = h
        .call(
            Method::PUT,
            "/v1/maps",
            &[h.bearer(&u, lxm::CONTENT_WRITE)],
            wrong_stream,
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // An invalid manifest.
    let mut broken = d.clone();
    broken.manifest.base.map = "../lockout".into();
    assert_eq!(
        publish(&h, &u, &broken).await.status,
        StatusCode::BAD_REQUEST
    );

    let r = publish(&h, &u, &d).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let body = r.json();
    assert_eq!(body["map"]["map_id"], id.to_string());
    assert_eq!(body["missing"], json!([]));

    let got = h.get(&format!("/v1/maps/{id}")).await.json();
    let map = &got["map"];
    assert_eq!(map["name"], "Lockout Snipers");
    assert_eq!(map["description"], "snipers only");
    assert_eq!(map["author"]["gamertag"], "Modder");
    assert_eq!(map["author"]["xuid"], u.xuid.to_string());
    assert_eq!(map["base"]["map"], "lockout");
    assert_eq!(map["base"]["derived"], false);
    assert_eq!(map["layers"][0]["kind"], "patch");
    assert_eq!(map["layers"][0]["size"], patch.len());
    assert_eq!(map["size"], patch.len());
    assert_eq!(map["image"], sha(PNG).to_string());
    assert_eq!(map["geometry"], Sha256([0x22; 32]).to_string());
    assert_eq!(map["manifest"]["patch"][0]["file"], "snipers.ips");
    let img = h.get(&format!("/v1/maps/{id}/image")).await;
    assert_eq!(img.status, StatusCode::OK);
    assert_eq!(img.headers["content-type"], "image/png");
    assert_eq!(img.body, PNG);
    assert_eq!(
        h.get(&format!("/v1/maps/{}", MapId([1; 32]))).await.status,
        StatusCode::NOT_FOUND
    );

    // The author again: the same answer; someone else: taken.
    assert_eq!(publish(&h, &u, &d).await.status, StatusCode::OK);
    let theirs = publish(&h, &other, &d).await;
    assert_eq!(theirs.status, StatusCode::CONFLICT);

    // A second package, on Midship, by the other account.
    upload(&h, &other, b"PATCH midship").await;
    let mut m2 = manifest(b"PATCH midship");
    m2.base.map = "midship".into();
    m2.package.name = "Midship Night".into();
    assert_eq!(
        publish(&h, &other, &doc(m2.clone())).await.status,
        StatusCode::CREATED
    );
    let names = |v: &Value| -> Vec<String> {
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        names(&h.get("/v1/maps").await.json()),
        ["Midship Night", "Lockout Snipers"]
    );
    assert_eq!(
        names(&h.get("/v1/maps?base=LOCKOUT").await.json()),
        ["Lockout Snipers"]
    );
    assert_eq!(
        names(&h.get("/v1/maps?author=Other").await.json()),
        ["Midship Night"]
    );
    let page = h.get("/v1/maps?limit=1").await.json();
    assert_eq!(names(&page), ["Midship Night"]);
    let next = page["next"].as_str().unwrap();
    assert_eq!(
        names(
            &h.get(&format!("/v1/maps?limit=1&cursor={next}"))
                .await
                .json()
        ),
        ["Lockout Snipers"]
    );

    // Deleting a layer leaves the package listing it as missing.
    let patch_hash = sha(patch);
    moderate(&h, "content", &patch_hash.to_string(), "delete").await;
    let got = h.get(&format!("/v1/maps/{id}")).await.json();
    assert_eq!(got["missing"], json!([patch_hash.to_string()]));
}

#[tokio::test]
async fn moderation_hides_deletes_blocks_and_refuses_sessions() {
    let h = content_harness(small()).await;
    let u = h.create("Modder").await;
    let patch = b"PATCH moderated".as_slice();
    upload(&h, &u, patch).await;
    let m = manifest(patch);
    let id = MapId::of(&m);
    assert_eq!(
        publish(&h, &u, &doc(m.clone())).await.status,
        StatusCode::CREATED
    );
    assert_eq!(
        advertise(&h, &u, &session("custom", None, Some(id)))
            .await
            .status,
        StatusCode::CREATED
    );

    // Accounts that are not admins cannot moderate.
    let r = h
        .call(
            Method::POST,
            &format!("/v1/admin/maps/{id}/hide"),
            &[h.bearer(&u, lxm::CONTENT_MODERATE)],
            Body::empty(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(
        h.get("/v1/admin/content").await.status,
        StatusCode::UNAUTHORIZED
    );

    // Hide: 410, out of the list, sessions refused; unhide undoes it.
    assert_eq!(
        moderate(&h, "maps", &id.to_string(), "hide").await.status,
        StatusCode::OK
    );
    assert_eq!(
        h.get(&format!("/v1/maps/{id}")).await.status,
        StatusCode::GONE
    );
    assert_eq!(h.get("/v1/maps").await.json()["items"], json!([]));
    let r = advertise(&h, &u, &session("custom", None, Some(id))).await;
    assert_eq!(r.status, StatusCode::GONE, "{}", r.error());
    assert_eq!(
        publish(&h, &u, &doc(m.clone())).await.status,
        StatusCode::GONE
    );
    assert_eq!(
        moderate(&h, "maps", &id.to_string(), "unhide").await.status,
        StatusCode::OK
    );
    assert_eq!(
        h.get(&format!("/v1/maps/{id}")).await.status,
        StatusCode::OK
    );

    // Block a layer by hash: the bytes go, the hash is refused, and every
    // package on it is gone too.
    let ph = sha(patch);
    assert_eq!(
        moderate(&h, "content", &ph.to_string(), "block")
            .await
            .status,
        StatusCode::OK
    );
    let path = h
        .dir
        .join("content")
        .join(&ph.to_string()[..2])
        .join(ph.to_string());
    assert!(!path.exists());
    assert_eq!(
        h.get(&format!("/v1/content/{ph}")).await.status,
        StatusCode::GONE
    );
    assert_eq!(put(&h, &u, &ph, patch).await.status, StatusCode::GONE);
    assert_eq!(
        h.get(&format!("/v1/maps/{id}")).await.status,
        StatusCode::GONE
    );
    assert_eq!(
        advertise(&h, &u, &session("custom", None, Some(id)))
            .await
            .status,
        StatusCode::GONE
    );
    // Unblocking leaves it deleted: a fresh upload brings it back.
    moderate(&h, "content", &ph.to_string(), "unblock").await;
    assert_eq!(
        h.get(&format!("/v1/content/{ph}")).await.status,
        StatusCode::GONE
    );
    upload(&h, &u, patch).await;
    assert_eq!(
        h.get(&format!("/v1/content/{ph}")).await.status,
        StatusCode::OK
    );

    // A hash can be blocked before anyone uploads it.
    let ahead = sha(b"PATCH known bad");
    assert_eq!(
        moderate(&h, "content", &ahead.to_string(), "block")
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        put(&h, &u, &ahead, b"PATCH known bad").await.status,
        StatusCode::GONE
    );
    // Other actions need something to act on.
    assert_eq!(
        moderate(&h, "content", &sha(b"nothing").to_string(), "hide")
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        moderate(&h, "maps", &id.to_string(), "purge").await.status,
        StatusCode::BAD_REQUEST
    );

    // Reports reach the admins.
    let reporter = h.create("Reporter").await;
    let r = h
        .call(
            Method::POST,
            &format!("/v1/maps/{id}/report"),
            &[h.bearer(&reporter, lxm::CONTENT_WRITE)],
            json!({"reason": "offensive name"}).to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    let view = h
        .call(
            Method::GET,
            "/v1/admin/content",
            &[moderator(&h)],
            Body::empty(),
        )
        .await;
    assert_eq!(view.status, StatusCode::OK, "{}", view.error());
    let view = view.json();
    assert_eq!(view["maps"][0]["map_id"], id.to_string());
    assert_eq!(view["maps"][0]["state"], "ok");
    assert_eq!(view["maps"][0]["reports"], 1);
    assert_eq!(view["reports"][0]["by"], "Reporter");
    assert_eq!(view["reports"][0]["reason"], "offensive name");
    assert_eq!(view["reports"][0]["target"], "map");
    let states: BTreeMap<String, String> = view["blobs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| {
            (
                b["sha256"].as_str().unwrap().to_string(),
                b["state"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(states[&ph.to_string()], "ok");
    assert_eq!(states.len(), 1, "a block ahead is not a stored blob");

    // Deleting the package: gone, and republishable.
    assert_eq!(
        moderate(&h, "maps", &id.to_string(), "delete").await.status,
        StatusCode::OK
    );
    assert_eq!(
        h.get(&format!("/v1/maps/{id}")).await.status,
        StatusCode::GONE
    );
    assert_eq!(publish(&h, &u, &doc(m)).await.status, StatusCode::CREATED);
    assert_eq!(
        h.get(&format!("/v1/maps/{id}")).await.status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn ranked_sessions_take_retail_or_pinned_maps_only() {
    let h = content_harness(small()).await;
    let u = h.create("Host").await;
    let patch = b"PATCH ranked".as_slice();
    upload(&h, &u, patch).await;
    let m = manifest(patch);
    let modded = MapId::of(&m);
    publish(&h, &u, &doc(m)).await;
    let base = Sha256([0x11; 32]);
    std::fs::write(h.dir.join(RETAIL_FILE), format!("{base}  lockout.map\n")).unwrap();
    let retail = MapId::retail("lockout", base);

    // Custom games take any map that is not taken down.
    assert_eq!(
        advertise(&h, &u, &session("custom", None, Some(modded)))
            .await
            .status,
        StatusCode::CREATED
    );
    // A session that says nothing is as before.
    assert_eq!(
        advertise(&h, &u, &session("ranked", Some(3), None))
            .await
            .status,
        StatusCode::CREATED
    );
    assert_eq!(
        advertise(&h, &u, &session("ranked", Some(3), Some(retail)))
            .await
            .status,
        StatusCode::CREATED
    );
    let r = advertise(&h, &u, &session("hopper", Some(3), Some(modded))).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.error());

    // An admin pins it on hopper 3.
    async fn pin(h: &Harness, ids: Vec<String>) -> Resp {
        h.call(
            Method::PUT,
            "/v1/hoppers/3/pins",
            &[moderator(h)],
            json!({"map_ids": ids}).to_string(),
        )
        .await
    }
    assert_eq!(
        pin(&h, vec![modded.to_string()]).await.status,
        StatusCode::OK
    );
    assert_eq!(
        h.get("/v1/hoppers/pins").await.json()["pins"],
        json!([{"hopper_id": 3, "map_ids": [modded.to_string()]}])
    );
    let r = advertise(&h, &u, &session("hopper", Some(3), Some(modded))).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    assert_eq!(
        r.json()["session"]["advertisement"]["map_id"],
        modded.to_string()
    );
    assert_eq!(
        advertise(&h, &u, &session("hopper", Some(4), Some(modded)))
            .await
            .status,
        StatusCode::FORBIDDEN,
        "pinned on another hopper"
    );
    // A host moving a ranked session onto an unpinned map is refused too.
    let created = advertise(&h, &u, &session("ranked", Some(4), Some(retail))).await;
    let sid = created.json()["session_id"].as_str().unwrap().to_string();
    let body = EventBody::at_unix(
        &u.key,
        &u.author,
        &live_proto::session::stream(&sid),
        live_proto::session::UPDATED,
        json!({"map_id": modded.to_string()}),
        now(),
    );
    let r = h
        .call(
            Method::POST,
            &format!("/v1/sessions/{sid}/events"),
            &[h.bearer(&u, lxm::SESSIONS_WRITE)],
            live_proto::sign::sign_batch(&u.key, &[body]),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", r.error());
    // Unpinning ends it.
    assert_eq!(pin(&h, vec![]).await.status, StatusCode::OK);
    assert_eq!(h.get("/v1/hoppers/pins").await.json()["pins"], json!([]));
    assert_eq!(
        advertise(&h, &u, &session("hopper", Some(3), Some(modded)))
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn the_catalog_is_replayed_from_the_log() {
    let store_dir = temp_dir();
    let patch = b"PATCH durable".as_slice();
    let m = manifest(patch);
    let id = MapId::of(&m);
    let data_dir = temp_dir();
    std::fs::create_dir_all(&data_dir).unwrap();
    {
        let store: Arc<dyn EventStore> = Arc::new(FileStore::open(&store_dir).await.unwrap());
        let h = start(store, small()).await;
        h.live.set_data_dir(&data_dir);
        let u = h.create("Modder").await;
        upload(&h, &u, patch).await;
        assert_eq!(
            publish(&h, &u, &doc(m.clone())).await.status,
            StatusCode::CREATED
        );
        moderate(&h, "maps", &id.to_string(), "hide").await;
    }
    let store: Arc<dyn EventStore> = Arc::new(FileStore::open(&store_dir).await.unwrap());
    let h = start(store, small()).await;
    h.live.set_data_dir(&data_dir);
    assert_eq!(
        h.get(&format!("/v1/maps/{id}")).await.status,
        StatusCode::GONE
    );
    moderate(&h, "maps", &id.to_string(), "unhide").await;
    let got = h.get(&format!("/v1/maps/{id}")).await.json();
    assert_eq!(got["map"]["author"]["gamertag"], "Modder");
    assert_eq!(
        h.get(&format!("/v1/content/{}", sha(patch))).await.body,
        patch
    );
    drop(h);
    let _ = std::fs::remove_dir_all(&store_dir);
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[tokio::test]
async fn no_data_directory_no_content() {
    let h = harness().await;
    let u = h.create("Modder").await;
    let r = put(&h, &u, &sha(b"PATCH"), b"PATCH").await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
}
