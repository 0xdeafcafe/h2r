//! A server with no hopper file published serves the built-in playlists
//! (default_hopper.rs) through every path that reads the file, until one is
//! published.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use live_proto::hopper::defaults::{playlists, retail_map_ids};
use live_proto::hopper::{HopperFile, MAX_FILE};
use live_proto::{Author, lxm};
use serde_json::json;

const FILE: &str = "/v1/titles/4D530064/files/multiplayer_hopper.dat";
const DOC: &str = "/v1/titles/4D530064/hopper";

fn operator(h: &Harness) -> (&'static str, String) {
    let author = Author::Server(h.id.clone()).to_string();
    (
        "authorization",
        format!(
            "Bearer {}",
            h.token(&h.operator, &author, lxm::TITLES_WRITE)
        ),
    )
}

#[tokio::test]
async fn the_default_lineup_is_served_until_a_file_is_published() {
    let h = harness().await;

    // The game's download: the built-in file, a valid envelope.
    let got = h.get(FILE).await;
    assert_eq!(got.status, StatusCode::OK);
    assert!(got.body.len() <= MAX_FILE);
    let file = HopperFile::parse(&got.body).unwrap();
    assert!(file.image() == HopperFile::default_set().image());
    assert_eq!(file.problems(&retail_map_ids()), vec![]);
    let etag = got
        .headers
        .get("etag")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let again = h
        .call(Method::GET, FILE, &[("if-none-match", etag.clone())], "")
        .await;
    assert_eq!(again.status, StatusCode::NOT_MODIFIED);

    // The admin view lists the default playlists.
    let doc = h.get(DOC).await;
    assert_eq!(
        doc.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&doc.body)
    );
    let hoppers = doc.json()["hoppers"].as_array().unwrap().clone();
    assert_eq!(hoppers.len(), 16);
    for (i, p) in playlists().iter().enumerate() {
        assert_eq!(hoppers[i]["title"], p.title);
        assert_eq!(hoppers[i]["type"], p.rules.hopper_type().raw());
        assert_eq!(hoppers[i]["record_count"], p.entries.len());
        assert_eq!(
            hoppers[i]["records"].as_array().unwrap().len(),
            p.entries.len()
        );
    }
    assert_eq!(hoppers[8]["type"], 0, "unused slots are empty");

    // Other titles have no default.
    assert_eq!(
        h.get("/v1/titles/4D530065/files/multiplayer_hopper.dat")
            .await
            .status,
        StatusCode::NOT_FOUND
    );

    // An edit through the builder publishes on top of the default.
    let r = h
        .call(
            Method::PUT,
            DOC,
            &[operator(&h)],
            json!({"hoppers": [{"index": 1, "title": "Slayer 4v4"}]}).to_string(),
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let edited = HopperFile::parse(&h.get(FILE).await.body).unwrap();
    assert_eq!(edited.text(1).unwrap().title, "Slayer 4v4");
    assert_eq!(edited.descriptor(0).unwrap(), file.descriptor(0).unwrap());
    assert_eq!(edited.problems(&retail_map_ids()), vec![]);

    // An import (a raw publish) replaces it outright.
    let mut imported = HopperFile::blank();
    imported.image_mut()[0x6140 + 7] = 0x5a;
    let built = imported.build();
    let r = h
        .call(Method::PUT, FILE, &[operator(&h)], built.clone())
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(h.get(FILE).await.body, built);
}

#[tokio::test]
async fn pins_are_independent_of_the_default_file() {
    let h = harness().await;
    assert_eq!(h.get("/v1/hoppers/pins").await.json()["pins"], json!([]));
    assert_eq!(h.get(FILE).await.status, StatusCode::OK);
}
