//! Link previews (`crates/live/live-server/src/page_meta.rs`) through the
//! router: the app's pages carry their own title, description and card in
//! the HTML a crawler reads, from what an anonymous reader may see.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::StatusCode;
use common::*;
use http_body_util::BodyExt;
use live_server::store::MemoryStore;
use serde_json::json;
use tower::ServiceExt;

const INDEX: &str = "<!doctype html>\n<html>\n  <head>\n    <meta charset=\"UTF-8\" />\n    \
    <!-- page-meta -->\n    <title>Live Dashboard</title>\n    <!-- /page-meta -->\n    \
    <meta name=\"h2-dash-name\" content=\"Live Dashboard\" />\n    \
    <meta name=\"h2-stats-name\" content=\"h2 Uplink\" />\n  </head>\n  \
    <body><!-- game-header --><!-- /game-header --><div id=\"root\"></div></body>\n</html>\n";

struct Web {
    app: axum::Router,
    dir: std::path::PathBuf,
}

impl Drop for Web {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Web {
    fn new(h: &Harness) -> Web {
        let dir = temp_dir().join("web");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("index.html"), INDEX).unwrap();
        std::fs::write(dir.join("assets/app.js"), "1").unwrap();
        Web {
            app: live_server::http::router_with_web(h.live.clone(), Some(&dir)),
            dir,
        }
    }

    async fn get(&self, uri: &str, host: &str) -> (StatusCode, String, String) {
        let req = axum::http::Request::get(uri)
            .header("host", host)
            .body(Body::empty())
            .unwrap();
        let r = self.app.clone().oneshot(req).await.unwrap();
        let status = r.status();
        let cache = r
            .headers()
            .get("cache-control")
            .map(|v| v.to_str().unwrap().to_string())
            .unwrap_or_default();
        let body = r.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap(), cache)
    }

    async fn page(&self, uri: &str) -> String {
        let (status, body, cache) = self.get(uri, "live.example").await;
        assert_eq!(status, StatusCode::OK, "{uri}");
        assert_eq!(cache, live_server::caching::REVALIDATE, "{uri}");
        assert_eq!(body.matches("<title>").count(), 1, "{uri}: {body}");
        assert!(body.contains("<div id=\"root\"></div>"), "{uri}");
        body
    }
}

fn meta(body: &str, key: &str) -> String {
    let at = body
        .find(&format!("\"{key}\" content=\""))
        .unwrap_or_else(|| panic!("no {key} in {body}"));
    let rest = &body[at + key.len() + 12..];
    rest[..rest.find('"').unwrap()].to_string()
}

fn title(body: &str) -> String {
    let a = body.find("<title>").unwrap() + 7;
    body[a..a + body[a..].find("</title>").unwrap()].to_string()
}

#[tokio::test]
async fn every_app_page_gets_its_sections_preview() {
    let h = harness().await;
    let web = Web::new(&h);
    let home = web.page("/").await;
    assert_eq!(title(&home), "Live Dashboard");
    assert_eq!(meta(&home, "og:site_name"), "Live Dashboard");
    assert_eq!(meta(&home, "theme-color"), "#0c4406");
    assert_eq!(meta(&home, "og:url"), "https://live.example/");
    assert_eq!(
        meta(&home, "og:image"),
        "https://live.example/og/default.jpg"
    );
    assert_eq!(meta(&home, "twitter:card"), "summary_large_image");

    let stats = web.page("/games/halo2").await;
    assert_eq!(title(&stats), "h2 Uplink");
    assert_eq!(meta(&stats, "theme-color"), "#071a35");
    assert_eq!(title(&web.page("/games/halo2/games").await), "Games · h2 Uplink");
    assert_eq!(title(&web.page("/dash/players").await), "Players · Live Dashboard");
    assert_eq!(title(&web.page("/games/halo2/clans").await), "Clans · h2 Uplink");
    // An unknown player or game still gets its section's.
    assert_eq!(
        title(&web.page("/games/halo2/players/nobody").await),
        "Service Record · h2 Uplink"
    );
    assert_eq!(
        title(
            &web.page("/games/halo2/games/0123456789abcdef0123456789abcdef")
                .await
        ),
        "Game · h2 Uplink"
    );
    // Files are files; the API stays JSON.
    assert_eq!(web.get("/assets/app.js", "x").await.1, "1");
    let (status, body, _) = web.get("/v1/nope", "x").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("not_found"));
    // A local server's URLs are its own.
    let (_, local, _) = web.get("/games/halo2", "127.0.0.1:8075").await;
    assert_eq!(meta(&local, "og:url"), "http://127.0.0.1:8075/games/halo2");
}

#[tokio::test]
async fn the_old_stats_pages_move_permanently() {
    let h = harness().await;
    let web = Web::new(&h);
    for (old, new) in [
        ("/stats", "/games/halo2"),
        ("/stats/", "/games/halo2"),
        ("/stats?window=30d", "/games/halo2?window=30d"),
        (
            "/stats/players/Two%20Words/career?compare=x",
            "/games/halo2/players/Two%20Words/career?compare=x",
        ),
    ] {
        let req = axum::http::Request::get(old)
            .header("host", "live.example")
            .body(Body::empty())
            .unwrap();
        let r = web.app.clone().oneshot(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::MOVED_PERMANENTLY, "{old}");
        assert_eq!(r.headers()["location"], new, "{old}");
    }
    // The API's stats stay where they are.
    let (status, _, _) = web.get("/v1/stats/summary", "x").await;
    assert_ne!(status, StatusCode::MOVED_PERMANENTLY);
}

#[tokio::test]
async fn player_and_game_pages_say_who_and_what() {
    let h = harness().await;
    let host = h.create("Host").await;
    let id = h.start_game(&host, two_player_game(&host)).await;
    let web = Web::new(&h);

    let live = web.page(&format!("/games/halo2/games/{id}")).await;
    assert_eq!(title(&live), "Team Slayer on Lockout · h2 Uplink");
    assert_eq!(meta(&live, "og:title"), "Team Slayer on Lockout");
    let d = meta(&live, "og:description");
    assert!(d.starts_with("Live now"), "{d}");
    assert!(d.contains("with Host, Guest"), "{d}");

    let end = h.batch(
        &host,
        &id,
        &[
            (
                "PlayerKilled",
                json!({"tick": 30, "victim": 1, "killer": 0, "weapon": "battle_rifle", "kind": "kill"}),
            ),
            (
                "ScoreChanged",
                json!({"tick": 40, "players": [{"index": 0, "score": 2, "kills": 2, "deaths": 0, "assists": 0}],
                       "teams": [{"team": 0, "score": 50}, {"team": 1, "score": 43}]}),
            ),
            (
                "GameEnded",
                json!({"tick": 900, "reason": "finished", "winner": {"team": 0}}),
            ),
        ],
    );
    assert_eq!(
        h.post_batch(&host, &id, end).await.status,
        StatusCode::CREATED
    );
    let ended = web.page(&format!("/games/halo2/games/{id}/3d")).await;
    assert_eq!(
        meta(&ended, "og:description"),
        "Final · Red 50, Blue 43 · Red team wins · with Host, Guest."
    );

    let player = web.page("/games/halo2/players/host/career").await;
    assert_eq!(title(&player), "Host · h2 Uplink");
    assert_eq!(meta(&player, "og:type"), "profile");
    assert_eq!(
        meta(&player, "og:description"),
        "Host&#39;s service record: 1 game, 1 win, 1 kill, 1.00 K/D."
    );
    // The old link and the JSON the Worker asks for say the same.
    assert_eq!(title(&web.page("/players/Host").await), "Host · h2 Uplink");
    let j = h.get("/v1/page-meta?path=/games/halo2/players/Host").await;
    assert_eq!(j.status, StatusCode::OK);
    assert_eq!(j.json()["page"], "Host");
    assert_eq!(j.json()["side"], "stats");
    assert_eq!(j.json()["kind"], "profile");
}

#[tokio::test]
async fn names_are_escaped_into_the_html() {
    let h = harness().await;
    let host = h.create("Host").await;
    let mut game = two_player_game(&host);
    game["variant"] = json!("<script>\"x\"</script>");
    let id = h.start_game(&host, game).await;
    let web = Web::new(&h);
    let body = web.page(&format!("/games/halo2/games/{id}")).await;
    assert!(!body.contains("<script>\"x\""), "{body}");
    assert!(body.contains("&lt;script&gt;&quot;x&quot;&lt;/script&gt; on Lockout"));
}

/// The game page's header (crates/live/live-server/src/page_header.rs), through
/// the router: pre-rendered into the HTML above the app, with the emblem
/// every player carries, at a fixed size — so the page's first paint is
/// the final header and nothing swaps once the app boots.
#[tokio::test]
async fn a_game_page_carries_its_header_pre_rendered() {
    let h = harness().await;
    let host = h.create("Host").await;
    let id = h.start_game(&host, two_player_game(&host)).await;
    let web = Web::new(&h);

    // A live game: the header says what, who and how it stands.
    let live = web.page(&format!("/games/halo2/games/{id}")).await;
    assert!(
        live.contains(r#"<div id="game-header" class="s-gamehead-shell" data-game="#),
        "{live}"
    );
    assert!(
        live.contains(r#"<script type="application/json" id="game-header-model">"#),
        "{live}"
    );
    assert!(live.contains("<b>Team Slayer</b> on Lockout"), "{live}");
    assert!(live.contains(r#"s-gamehead-flag--live">LIVE<"#), "{live}");
    // Both players, each with their emblem at a fixed size.
    assert_eq!(
        live.matches(r#"<img class="emblem emblem--game""#).count(),
        2,
        "{live}"
    );
    for img in live.split(r#"<img class="emblem emblem--game""#).skip(1) {
        let tag = &img[..img.find('>').unwrap()];
        assert!(tag.contains(r#"src="/art/emblem/"#), "{tag}");
        assert!(tag.contains(r#"width="16""#), "{tag}");
        assert!(tag.contains(r#"height="16""#), "{tag}");
    }
    assert!(live.contains("<span>Host</span>"), "{live}");
    assert!(live.contains("<span>Guest</span>"), "{live}");
    // Above the app, and the model beside it is the JSON the client
    // re-derives, so it adopts what is already there.
    assert!(
        live.find(r#"id="game-header""#).unwrap() < live.find(r#"<div id="root">"#).unwrap(),
        "{live}"
    );
    let model = live
        .split(r#"id="game-header-model">"#)
        .nth(1)
        .unwrap()
        .split("</script>")
        .next()
        .unwrap()
        .to_string();
    let parsed: serde_json::Value = serde_json::from_str(&model).unwrap();
    assert_eq!(parsed["game"], json!(id));
    assert_eq!(parsed["live"], json!(true));

    // Only a game page carries one.
    let list = web.page("/games/halo2/games").await;
    assert!(!list.contains(r#"id="game-header""#), "{list}");

    // The game ends: the header follows, without being asked again.
    let end = h.batch(
        &host,
        &id,
        &[
            (
                "ScoreChanged",
                json!({"tick": 40, "players": [{"index": 0, "score": 2, "kills": 2, "deaths": 0, "assists": 0}],
                       "teams": [{"team": 0, "score": 50}, {"team": 1, "score": 43}]}),
            ),
            (
                "GameEnded",
                json!({"tick": 900, "reason": "finished", "winner": {"team": 0}}),
            ),
        ],
    );
    assert_eq!(
        h.post_batch(&host, &id, end).await.status,
        StatusCode::CREATED
    );
    let ended = web.page(&format!("/games/halo2/games/{id}")).await;
    assert!(ended.contains(r#"s-gamehead-flag">FINAL<"#), "{ended}");
    assert!(ended.contains("<span>Red Team won</span>"), "{ended}");
    assert!(ended.contains("<header><span>Red</span><b>50</b></header>"), "{ended}");

    // The JSON the Worker asks for carries the header too.
    let j = h
        .get(&format!("/v1/page-meta?path=/games/halo2/games/{id}"))
        .await;
    assert_eq!(j.status, StatusCode::OK);
    assert_eq!(j.json()["header"]["game"], json!(id));
    assert_eq!(j.json()["header"]["result"], json!("Red Team won"));
}

#[tokio::test]
async fn a_private_server_previews_nothing_of_its_players() {
    let h = start(
        Arc::new(MemoryStore::new()),
        live_server::Policy {
            private: true,
            ..Default::default()
        },
    )
    .await;
    let web = Web::new(&h);
    let page = web.page("/games/halo2/players/Anyone").await;
    assert_eq!(title(&page), "Service Record · h2 Uplink");
    // The JSON answer, which the Worker asks for, says no more.
    let j = h
        .get("/v1/page-meta?path=/games/halo2/players/Anyone")
        .await
        .json();
    assert_eq!(j["page"], "Service Record");
}
