//! Same HTTP scenarios on the native SQLite and Durable Object SQL stores.
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use live_proto::{
    lxm,
    sign::{AccountRequest, KeyPair, issue_token},
};
use serde_json::{Value, json};
use std::fmt::Write;
use tower::ServiceExt;

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    body: String,
    expected: StatusCode,
) -> Value {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(token) = bearer {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, expected, "{method} {uri}: {value}");
    value
}

fn encode(value: Value) -> String {
    let mut encoded = String::new();
    for b in value.to_string().bytes() {
        write!(encoded, "%{b:02X}").unwrap();
    }
    encoded
}

async fn list(app: &Router, filters: Value, more: &str, bearer: Option<&str>) -> Value {
    call(
        app,
        "GET",
        &format!("/v1/sessions?filters={}{more}", encode(filters)),
        bearer,
        String::new(),
        StatusCode::OK,
    )
    .await
}

fn names(page: &Value) -> Vec<&str> {
    let mut names: Vec<_> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["advertisement"]["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    names
}

pub async fn run(app: Router) {
    use live_server::app::now;
    let key = KeyPair::generate();
    let host = call(
        &app,
        "POST",
        "/v1/accounts",
        None,
        AccountRequest::sign(&key, "TestHost", None, "live.test", now()),
        StatusCode::CREATED,
    )
    .await;
    let token = |method| {
        issue_token(
            &key,
            host["id"].as_str().unwrap(),
            "live.test",
            method,
            now(),
        )
    };
    for (name, extra) in [
        (
            "slayer",
            json!({
                "map": "levels/mp/lockout.map",
                "browser": {"clankers": true, "game_type": 2, "variant": "Team Slayer"}
            }),
        ),
        (
            "ctf",
            json!({
                "map": "Turf",
                "browser": {"clankers": false, "game_type": 1, "variant": "One Flag"}
            }),
        ),
        (
            "mods",
            json!({
                "map": "lockout",
                "slots": {"public_filled": 2, "public_open": 0, "private_filled": 0, "private_open": 1},
                "browser": {"clankers": false, "game_type": 2, "variant": "Shotguns", "map_category": "mods"}
            }),
        ),
        (
            "halo2 coop",
            json!({
                "flags": "coop",
                "browser": {"clankers": false},
                "campaign": {"level": "03a_oldmombasa", "difficulty": 2}
            }),
        ),
        (
            "ce coop",
            json!({
                "flags": "coop",
                "browser": {"clankers": false},
                "campaign": {"level": "a30", "difficulty": 3}
            }),
        ),
        ("legacy mp", json!({"map": "unclassified"})),
        ("legacy coop", json!({"flags": "coop"})),
        (
            "private",
            json!({
                "visibility": "invite",
                "browser": {"game_type": 2, "variant": "Private Variant"}
            }),
        ),
        (
            "ranked",
            json!({
                "flags": "ranked",
                "attributes": {"1": {"type": "integer", "value": 0}},
                "browser": {"game_type": 2, "variant": "Ranked Variant"}
            }),
        ),
    ] {
        let mut ad = json!({
            "name": name,
            "addr": "192.168.1.10",
            "port": 3074,
            "host_address": vec![0u8; 36],
            "slots": {"public_filled": 1, "public_open": 1, "private_filled": 0, "private_open": 0},
            "attributes": {}
        });
        ad.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        call(
            &app,
            "POST",
            "/v1/sessions",
            Some(&token(lxm::SESSIONS_WRITE)),
            ad.to_string(),
            StatusCode::CREATED,
        )
        .await;
    }

    for (filters, expected) in [
        (
            json!({"kind":"coop"}),
            vec!["ce coop", "halo2 coop", "legacy coop"],
        ),
        (
            json!({"kind":"multiplayer"}),
            vec!["ctf", "legacy mp", "mods", "ranked", "slayer"],
        ),
        (
            json!({"kind":"multiplayer", "clankers":false}),
            vec!["ctf", "legacy mp", "mods", "ranked"],
        ),
        (
            json!({"clankers":true}),
            vec!["legacy coop", "legacy mp", "ranked", "slayer"],
        ),
        (json!({"game_type":1}), vec!["ctf", "legacy mp"]),
        (
            json!({"variant":" TEAM SLAYER ", "has_space":true}),
            vec!["legacy mp", "slayer"],
        ),
        (
            json!({"map_category":"halo2"}),
            vec!["halo2 coop", "legacy coop", "legacy mp", "ranked", "slayer"],
        ),
        (
            json!({"map_category":"halo_ce"}),
            vec!["ce coop", "legacy coop", "legacy mp", "ranked"],
        ),
        (
            json!({"map_category":"map_pack"}),
            vec!["ctf", "legacy coop", "legacy mp", "ranked"],
        ),
        (
            json!({"map_category":"mods"}),
            vec!["legacy coop", "legacy mp", "mods", "ranked"],
        ),
        (
            json!({"has_space":true}),
            vec![
                "ce coop",
                "ctf",
                "halo2 coop",
                "legacy coop",
                "legacy mp",
                "ranked",
                "slayer",
            ],
        ),
        (
            json!({"campaign":"halo2", "difficulty":2}),
            vec!["halo2 coop", "legacy coop"],
        ),
        (
            json!({"campaign":"halo_ce", "difficulty":3}),
            vec!["ce coop", "legacy coop"],
        ),
    ] {
        let page = list(&app, filters.clone(), "", None).await;
        assert_eq!(names(&page), expected, "{filters}: {page}");
        assert!(page.get("variants").is_none());
    }

    let legacy = call(
        &app,
        "GET",
        "/v1/sessions",
        None,
        String::new(),
        StatusCode::OK,
    )
    .await;
    assert!(
        legacy["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| { item["advertisement"].get("browser").is_none() })
    );
    let modern = list(&app, json!({"variant":"Team Slayer"}), "", None).await;
    let slayer = modern["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["advertisement"]["name"] == "slayer")
        .unwrap();
    assert_eq!(slayer["advertisement"]["browser"]["clankers"], true);
    let id = slayer["session_id"].as_str().unwrap();
    let detail = call(
        &app,
        "GET",
        &format!("/v1/sessions/{id}"),
        None,
        String::new(),
        StatusCode::OK,
    )
    .await;
    assert!(detail["session"]["advertisement"].get("browser").is_none());

    let filters = json!({"kind":"coop", "difficulty":2});
    let first = list(
        &app,
        filters.clone(),
        "&limit=1&browser_variants=true",
        None,
    )
    .await;
    let cursor = first["next"].as_str().unwrap();
    let second = list(
        &app,
        filters,
        &format!("&limit=1&browser_variants=true&cursor={cursor}"),
        None,
    )
    .await;
    let mut paginated = names(&first);
    paginated.extend(names(&second));
    paginated.sort_unstable();
    assert_eq!(paginated, ["halo2 coop", "legacy coop"]);
    assert!(second["next"].is_null());
    assert!(second.get("variants").is_none());
    assert_eq!(
        first["variants"],
        json!(["One Flag", "Ranked Variant", "Shotguns", "Team Slayer"])
    );

    let reader_key = KeyPair::generate();
    let reader = call(
        &app,
        "POST",
        "/v1/accounts",
        None,
        AccountRequest::sign(&reader_key, "TestReader", None, "live.test", now()),
        StatusCode::CREATED,
    )
    .await;
    let read_token = issue_token(
        &reader_key,
        reader["id"].as_str().unwrap(),
        "live.test",
        lxm::READ,
        now(),
    );
    let optimatch = encode(json!({"hopper":-1, "average":0}));
    let page = list(
        &app,
        json!({"game_type":2, "clankers":true, "has_space":true}),
        &format!("&optimatch={optimatch}&browser_variants=true"),
        Some(&read_token),
    )
    .await;
    assert_eq!(names(&page), ["legacy mp", "slayer"]);
    assert_eq!(
        page["variants"],
        json!(["One Flag", "Shotguns", "Team Slayer"])
    );
    assert!(page["items"][0]["endpoint"]["addr"].is_string());
    let page = list(
        &app,
        json!({"kind":"coop"}),
        &format!("&optimatch={optimatch}"),
        None,
    )
    .await;
    assert!(page["items"][0]["advertisement"]["host_address"].is_null());

    for filters in [
        json!({"difficulty":4}),
        json!({"game_type":5}),
        json!({"variant":"   "}),
        json!({"kind":"unknown"}),
        json!({"unsupported":true}),
    ] {
        call(
            &app,
            "GET",
            &format!("/v1/sessions?filters={}", encode(filters)),
            None,
            String::new(),
            StatusCode::BAD_REQUEST,
        )
        .await;
    }
}
