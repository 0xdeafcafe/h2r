//! The hopper gate (docs/game/rules-start-gate.md, "Hoppers, System Link,
//! offline and Theater"): a hopper edit that pairs a script with a map it
//! does not fit is refused by name, a map's census is an admin's
//! `PUT /v1/maps/{map}/census`, and a new census lists the published
//! entries it blocks. The routes are the ones the Worker serves too.

mod common;

use axum::http::{Method, StatusCode};
use common::*;
use live_proto::hopper::variant::{GameType, VariantRecord};
use live_proto::hopper::{Entry, HopperFile, set_slot_entry};
use live_proto::{Author, lxm};
use rules::labels::{Census, minimal};
use serde_json::json;

const FILE: &str = "/v1/titles/4D530064/files/multiplayer_hopper.dat";
const DOC: &str = "/v1/titles/4D530064/hopper";
/// Zanzibar's level id: a retail map a hopper may name.
const MAP: u32 = 30;

fn operator(h: &Harness, method: &str) -> (&'static str, String) {
    let author = Author::Server(h.id.clone()).to_string();
    (
        "authorization",
        format!("Bearer {}", h.token(&h.operator, &author, method)),
    )
}

/// Territories declares `label territory: zones 1..8`: a map with no zone
/// of that label is blocked.
fn territories() -> rules::ir::Ir {
    let b = rules::builtin::ALL
        .iter()
        .find(|b| b.name == "Territories")
        .expect("a built-in Territories");
    rules::compile(b.source).expect("it compiles")
}

/// A hopper file whose first playlist's first entry runs `ir` on `MAP`.
fn file_with_script(ir: &rules::ir::Ir) -> Vec<u8> {
    let mut file = HopperFile::default_set();
    let stock = VariantRecord::stock(GameType::Slayer, "");
    let mut v = variants::record::GameVariant::from_bytes(stock.bytes()).unwrap();
    let mut e = v.ext();
    let id = ir.id().to_string();
    let mut prefix = [0u8; 8];
    for (i, b) in prefix.iter_mut().enumerate() {
        *b = u8::from_str_radix(&id[2 * i..2 * i + 2], 16).unwrap();
    }
    e.script = variants::ext::Script {
        mode: variants::ext::Mode::Rules,
        prefix,
        options: 0,
    };
    v.set_ext(&e).unwrap();
    let mut slot = file.slot(0, 0).unwrap().to_vec();
    set_slot_entry(
        &mut slot,
        &Entry {
            map_id: MAP,
            weight: 1,
            map_name: String::new(),
            variant: VariantRecord::from_bytes(v.bytes()).unwrap(),
        },
    )
    .unwrap();
    file.set_slot(0, 0, &slot).unwrap();
    file.build_checked().unwrap()
}

#[tokio::test]
async fn a_blocked_pair_is_refused_and_a_good_one_publishes() {
    let h = harness().await;
    let ir = territories();
    let put = h
        .call(
            Method::PUT,
            FILE,
            &[operator(&h, lxm::TITLES_WRITE)],
            file_with_script(&ir),
        )
        .await;
    assert_eq!(put.status, StatusCode::OK, "{}", put.error());
    let edit = || json!({"hoppers": [{"index": 0, "title": "Territories 4v4"}]}).to_string();

    // A map with no census: refused, by name.
    let r = h
        .call(Method::PUT, DOC, &[operator(&h, lxm::TITLES_WRITE)], edit())
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", String::from_utf8_lossy(&r.body));
    assert!(
        String::from_utf8_lossy(&r.body).contains("playlist 1 entry 1") && String::from_utf8_lossy(&r.body).contains("no census for map 30"),
        "{}",
        String::from_utf8_lossy(&r.body)
    );

    // Only an admin sets a census, and it is a map id.
    let user = h.create("Maker").await;
    let census = serde_json::to_string(&Census::default()).unwrap();
    let r = h
        .call(
            Method::PUT,
            &format!("/v1/maps/{MAP}/census"),
            &[h.bearer(&user, lxm::CONTENT_MODERATE)],
            census.clone(),
        )
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{}", String::from_utf8_lossy(&r.body));
    let r = h
        .call(
            Method::PUT,
            "/v1/maps/not-a-map/census",
            &[operator(&h, lxm::CONTENT_MODERATE)],
            census.clone(),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", String::from_utf8_lossy(&r.body));

    // A census with no zone: kept, and the answer lists the published
    // entry it blocks.
    let r = h
        .call(
            Method::PUT,
            &format!("/v1/maps/{MAP}/census"),
            &[operator(&h, lxm::CONTENT_MODERATE)],
            census,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
    let a = r.json();
    assert_eq!(a["set"], true);
    assert_eq!(a["blocked"].as_array().unwrap().len(), 1, "{a}");
    assert_eq!(a["blocked"][0]["hopper"], 1);
    assert_eq!(a["blocked"][0]["entry"], 1);
    assert_eq!(a["blocked"][0]["map_id"], MAP);
    assert!(a["blocked"][0]["problems"][0].as_str().unwrap().contains("territory"));

    // The edit is refused with the problem's text.
    let r = h
        .call(Method::PUT, DOC, &[operator(&h, lxm::TITLES_WRITE)], edit())
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", String::from_utf8_lossy(&r.body));
    assert!(String::from_utf8_lossy(&r.body).contains("territory"), "{}", String::from_utf8_lossy(&r.body));

    // A census the map satisfies: nothing blocked, and the edit publishes.
    let good = serde_json::to_string(&minimal(&ir, &[0, 1])).unwrap();
    let r = h
        .call(
            Method::PUT,
            &format!("/v1/maps/{MAP}/census"),
            &[operator(&h, lxm::CONTENT_MODERATE)],
            good,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["blocked"], json!([]));
    let r = h
        .call(Method::PUT, DOC, &[operator(&h, lxm::TITLES_WRITE)], edit())
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
    let published = HopperFile::parse(&h.get(FILE).await.body).unwrap();
    assert_eq!(published.text(0).unwrap().title, "Territories 4v4");

    // Clearing the census blocks the entry again, by name.
    let r = h
        .call(
            Method::PUT,
            &format!("/v1/maps/{MAP}/census"),
            &[operator(&h, lxm::CONTENT_MODERATE)],
            "null",
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", String::from_utf8_lossy(&r.body));
    assert_eq!(r.json()["set"], false);
    assert_eq!(r.json()["blocked"].as_array().unwrap().len(), 1);
}
