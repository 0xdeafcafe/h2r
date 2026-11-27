//! The parity suite's package cases (docs/live/live-api.md, "Packages"):
//! both homes run these requests against their own event and blob stores.
//! The native server runs it from `tests/packages.rs`, the Worker on its
//! SQL store (`live-worker`, `worker_packages`); both must refuse and
//! answer the same.
//!
//! A good package publishes; a bad signature, a lock hash that is not
//! stored, a lock that does not give the stream's `PackageId`, a retail
//! hash and a model item without the flag are each refused; an admin's
//! credit outranks the publisher's, which outranks the manifest's.
use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use live_proto::{
    Xuid, lxm,
    maps::Sha256,
    packages::{self, Lock, Manifest, PackageDocument},
    sign::{AccountRequest, EventBody, KeyPair, issue_token},
};
use live_server::{
    Live,
    app::now,
    blobs::{BlobStore, Blobs, Meta},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
use tower::ServiceExt;

const HOST: &str = "live.test";

async fn call(
    live: &Arc<Live>,
    method: &str,
    url: &str,
    bearer: Option<String>,
    body: Vec<u8>,
) -> (u16, Value) {
    let mut req = Request::builder().method(method).uri(url);
    if let Some(b) = bearer {
        req = req.header("authorization", format!("Bearer {b}"));
    }
    let res = live_server::http::router(live.clone())
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = res.status().as_u16();
    let bytes = to_bytes(res.into_body(), 4 << 20).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

struct Who {
    key: KeyPair,
    id: String,
    xuid: Xuid,
}

impl Who {
    fn token(&self, method: &str) -> Option<String> {
        Some(issue_token(&self.key, &self.id, HOST, method, now()))
    }
}

async fn account(live: &Arc<Live>, tag: &str) -> Who {
    let key = KeyPair::generate();
    let (status, row) = call(
        live,
        "POST",
        "/v1/accounts",
        None,
        AccountRequest::sign(&key, tag, None, HOST, now()).into_bytes(),
    )
    .await;
    assert_eq!(status, 201, "{row}");
    Who {
        key,
        id: row["id"].as_str().unwrap().to_string(),
        xuid: Xuid::parse(row["xuid"].as_str().unwrap()).unwrap(),
    }
}

async fn upload(live: &Arc<Live>, who: &Who, bytes: &[u8]) -> Sha256 {
    let h = Sha256::of(bytes);
    let (status, v) = call(
        live,
        "PUT",
        &format!("/v1/content/{h}"),
        who.token(lxm::CONTENT_WRITE),
        bytes.to_vec(),
    )
    .await;
    assert!(status == 201 || status == 200, "{status} {v}");
    h
}

/// A one-item package: `kind` from `a.yaml` with hash `h`.
fn document(slug: &str, kind: &str, h: Sha256) -> PackageDocument {
    let manifest = Manifest::from_toml(&format!(
        "[package]\nformat = 1\nid = \"{slug}\"\nname = \"Test {slug}\"\nrequires = [\"core\"]\n\n\
         [credit]\nauthors = [\"Manifest Author\"]\n\n\
         [[item]]\nkind = \"{kind}\"\nsource = \"a.yaml\"\nmaps = \"multiplayer\"\n"
    ))
    .unwrap();
    PackageDocument {
        manifest,
        lock: Lock::new(BTreeMap::from([("a.yaml".to_string(), h)])),
        credit: None,
    }
}

fn signed(who: &Who, stream: &str, doc: &PackageDocument) -> String {
    EventBody::at_unix(
        &who.key,
        &who.id,
        stream,
        packages::PACKAGE_PUBLISHED,
        serde_json::to_value(doc).unwrap(),
        now(),
    )
    .sign(&who.key)
}

async fn publish(live: &Arc<Live>, who: &Who, jws: String) -> (u16, Value) {
    call(
        live,
        "PUT",
        "/v1/packages",
        who.token(lxm::CONTENT_WRITE),
        jws.into_bytes(),
    )
    .await
}

fn stream_of(doc: &PackageDocument) -> String {
    packages::stream(&doc.id().unwrap())
}

pub async fn exercise(live: Arc<Live>) {
    if live.blobs().is_none() {
        live.set_blobs(Blobs {
            private: BlobStore::memory(),
            public: None,
        });
    }
    let maker = account(&live, "Maker").await;
    let admin = account(&live, "Moderator").await;
    live.grant_admin_runtime(admin.xuid, "Moderator");

    // The operator's retail list, read from the blob store as the Worker
    // reads it from R2.
    let retail_bytes = b"retail map bytes".to_vec();
    let retail = Sha256::of(&retail_bytes);
    live.blobs()
        .unwrap()
        .private
        .put(
            live_server::content::RETAIL_FILE,
            format!("{retail}  lockout.map\n").into_bytes().into(),
            &Meta::default(),
        )
        .await
        .unwrap();
    live.load_retail().await;

    let file = upload(&live, &maker, b"like: objects\\weapons\\rifle\n").await;

    // A good package.
    let good = document("mine", "tag", file);
    let (status, v) = publish(&live, &maker, signed(&maker, &stream_of(&good), &good)).await;
    assert_eq!(status, 201, "{v}");
    let id = good.id().unwrap().to_string();
    assert_eq!(v["package"]["id"], id);
    assert_eq!(v["package"]["slug"], "mine");
    assert_eq!(v["package"]["publisher"]["gamertag"], "Maker");
    assert_eq!(v["package"]["sim"], true);
    assert_eq!(v["package"]["kinds"], json!(["tag"]));
    assert_eq!(v["lock"]["files"]["a.yaml"], file.to_string());
    assert_eq!(v["author"], maker.id);
    assert!(v["jws"].as_str().unwrap().split('.').count() == 3);
    let (status, list) = call(&live, "GET", "/v1/packages", None, vec![]).await;
    assert_eq!(status, 200, "{list}");
    assert_eq!(list["items"][0]["id"], id);
    let (status, read) = call(&live, "GET", &format!("/v1/packages/{id}"), None, vec![]).await;
    assert_eq!(status, 200, "{read}");
    assert_eq!(read["jws"], v["jws"]);
    // The manifest's credit, until the publisher or an admin says more.
    assert_eq!(read["package"]["credit"]["authors"], json!(["Manifest Author"]));

    // A bad signature: another document's signature on this one.
    let other = document("other", "tag", file);
    let jws = signed(&maker, &stream_of(&other), &other);
    let decoy = signed(&maker, &stream_of(&good), &good);
    let mut parts: Vec<&str> = jws.split('.').collect();
    parts[2] = decoy.rsplit('.').next().unwrap();
    let (status, v) = publish(&live, &maker, parts.join(".")).await;
    assert_eq!(status, 401, "{v}");

    // A lock hash the store does not hold.
    let unstored = document("unstored", "tag", Sha256([0x5a; 32]));
    let (status, v) = publish(&live, &maker, signed(&maker, &stream_of(&unstored), &unstored)).await;
    assert_eq!(status, 409, "{v}");
    assert!(v["message"].as_str().unwrap().contains("a.yaml"), "{v}");

    // A lock whose hashes do not give the stream's PackageId.
    let mut swapped = document("swapped", "tag", file);
    let claimed = stream_of(&swapped);
    swapped
        .lock
        .files
        .insert("a.yaml".into(), upload(&live, &maker, b"other bytes").await);
    let (status, v) = publish(&live, &maker, signed(&maker, &claimed, &swapped)).await;
    assert_eq!(status, 400, "{v}");

    // A retail hash in the lock.
    let ripped = document("ripped", "tag", retail);
    let (status, v) = publish(&live, &maker, signed(&maker, &stream_of(&ripped), &ripped)).await;
    assert_eq!(status, 403, "{v}");
    assert!(v["message"].as_str().unwrap().contains("retail"), "{v}");

    // A model item without the flag, then with it.
    let model = document("model", "model", file);
    let (status, v) = publish(&live, &maker, signed(&maker, &stream_of(&model), &model)).await;
    assert_eq!(status, 403, "{v}");
    assert!(
        v["message"].as_str().unwrap().contains("model_uploads_not_approved"),
        "{v}"
    );
    let (status, v) = call(
        &live,
        "PUT",
        "/v1/admin/model-uploads/Maker",
        maker.token(lxm::CONTENT_MODERATE),
        json!({"approved": true}).to_string().into_bytes(),
    )
    .await;
    assert_eq!(status, 403, "only an admin sets the flag: {v}");
    let (status, v) = call(
        &live,
        "PUT",
        "/v1/admin/model-uploads/Maker",
        admin.token(lxm::CONTENT_MODERATE),
        json!({"approved": true, "note": "their own work"}).to_string().into_bytes(),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let (status, v) = publish(&live, &maker, signed(&maker, &stream_of(&model), &model)).await;
    assert_eq!(status, 201, "{v}");

    // Credit: the publisher's signed credit over the manifest's, an
    // admin's over both; clearing the admin's shows the publisher's again.
    let mut credited = document("credited", "tag", file);
    credited.credit = Some(live_proto::maps::Credit::authors(["Original Team"]));
    let (status, v) =
        publish(&live, &maker, signed(&maker, &stream_of(&credited), &credited)).await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(v["package"]["credit"]["authors"], json!(["Original Team"]));
    let cid = credited.id().unwrap().to_string();
    let url = format!("/v1/packages/{cid}/credit");
    let body = json!({"authors": ["The Real Author"]}).to_string().into_bytes();
    let (status, v) = call(&live, "PUT", &url, maker.token(lxm::CONTENT_MODERATE), body.clone()).await;
    assert_eq!(status, 403, "{v}");
    let (status, v) = call(&live, "PUT", &url, admin.token(lxm::CONTENT_MODERATE), body).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["package"]["credit"]["authors"], json!(["The Real Author"]));
    let (_, read) = call(&live, "GET", &format!("/v1/packages/{cid}"), None, vec![]).await;
    assert_eq!(read["package"]["credit"]["authors"], json!(["The Real Author"]));
    let (status, v) = call(
        &live,
        "PUT",
        &url,
        admin.token(lxm::CONTENT_MODERATE),
        b"{}".to_vec(),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["package"]["credit"]["authors"], json!(["Original Team"]));
}
