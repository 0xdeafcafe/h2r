//! Per-playlist ranks through the router: a finished ranked matchmaking
//! game writes one `RankChanged` per player; unranked, custom and lost
//! games write none; the standings, history and leaderboard read them back.

mod common;

use axum::http::StatusCode;
use common::*;
use live_proto::rank::{self, Standing};
use serde_json::{Value, json};

/// A two-player matchmaking game on `playlist` in `host`'s Live session
/// `session`, both players with accounts.
fn duel(session: &str, host: &User, guest: &User, playlist: &str, kind: &str) -> Value {
    json!({
        "console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Team Slayer",
        "team_play": true, "tick": 0, "kind": kind,
        "playlist": "Synthetic", "playlist_id": playlist,
        "session_id": session,
        "players": [
            {"index": 0, "name": "Host", "xuid": host.xuid, "team": 0, "local": true},
            {"index": 1, "name": "Guest", "xuid": guest.xuid, "team": 1}
        ]
    })
}

/// A fresh arbitration round id.
fn round() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    format!(
        "{:016x}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// `user` registers arbitration round `round` of `session`.
async fn register(h: &Harness, session: &str, user: &User, round: &str) -> Resp {
    h.call(
        axum::http::Method::POST,
        &format!("/v1/arbitration/{session}/{round}/register"),
        &[h.bearer(user, live_proto::lxm::GAMES_WRITE)],
        r#"{"duration":300,"flags":12}"#.to_string(),
    )
    .await
}

/// Plays `game` to its end, host and guest having registered a fresh
/// round of its session first, as their consoles do.
async fn play(h: &Harness, host: &User, guest: &User, game: Value, reason: &str) -> String {
    if let Some(session) = game["session_id"].as_str() {
        let r = round();
        for u in [host, guest] {
            assert_eq!(
                register(h, session, u, &r).await.status,
                StatusCode::CREATED
            );
        }
    }
    let id = h.start_game(host, game).await;
    let end = h.batch(
        host,
        &id,
        &[
            (
                "ScoreChanged",
                json!({"tick": 800, "players": [
                    {"index": 0, "score": 5, "kills": 5, "deaths": 1, "assists": 0},
                    {"index": 1, "score": 1, "kills": 1, "deaths": 5, "assists": 0}],
                    "teams": [{"team": 0, "score": 5}, {"team": 1, "score": 1}]}),
            ),
            (
                "GameEnded",
                json!({"tick": 900, "reason": reason, "winner": {"team": 0}}),
            ),
        ],
    );
    let r = h.post_batch(host, &id, end).await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    id
}

fn changes(h: &Value) -> Vec<Value> {
    h["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == rank::CHANGED)
        .cloned()
        .collect()
}

#[tokio::test]
async fn a_finished_ranked_game_ranks_both_players_once() {
    let h = harness().await;
    let host = h.create("Host").await;
    let s = host_session(&h, &host).await;
    let guest = h.create("Guest").await;
    // The default playlists: 1 Team Slayer is ranked teams, 7 Unranked Teams.
    let id = play(
        &h,
        &host,
        &guest,
        duel(&s, &host, &guest, "1", "matchmaking"),
        "finished",
    )
    .await;

    let host_rank = h
        .get(&format!("/v1/events?stream=rank-{}", host.xuid))
        .await
        .json();
    let c = changes(&host_rank);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["author"], "server:live.test");
    assert_eq!(c[0]["data"]["game_id"], id.as_str());
    assert_eq!(c[0]["data"]["playlist"], "1");
    let after: Standing = serde_json::from_value(c[0]["data"]["after"].clone()).unwrap();
    assert_eq!((after.level, after.experience, after.wins), (2, 100, 1));
    // A level 1 loser loses nothing, but the game counts.
    let guest_rank = h
        .get(&format!("/v1/events?stream=rank-{}", guest.xuid))
        .await
        .json();
    let after: Standing =
        serde_json::from_value(changes(&guest_rank)[0]["data"]["after"].clone()).unwrap();
    assert_eq!((after.level, after.experience, after.games), (1, 0, 1));

    let ranks = h.get("/v1/accounts/Host/ranks").await.json();
    assert_eq!(ranks["ranks"]["1"]["level"], 2);
    let history = h.get("/v1/accounts/Host/ranks/1").await.json();
    assert_eq!(history["standing"]["experience"], 100);
    assert_eq!(history["history"][0]["game_id"], id.as_str());
    assert_eq!(history["history"][0]["before"]["level"], 1);
    let board = h.get("/v1/ranks/1").await.json();
    assert_eq!(board["items"][0]["gamertag"], "Host");
    assert_eq!(board["items"][1]["gamertag"], "Guest");

    // A second win climbs again.
    play(
        &h,
        &host,
        &guest,
        duel(&s, &host, &guest, "1", "matchmaking"),
        "finished",
    )
    .await;
    let history = h.get("/v1/accounts/Host/ranks/1").await.json();
    // Beating a lower level is worth less: 89 points from level 2.
    assert_eq!(history["standing"]["experience"], 189);
    assert_eq!(history["standing"]["games"], 2);
    assert_eq!(history["history"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn unranked_custom_and_lost_games_rank_nobody() {
    let h = harness().await;
    let host = h.create("Host").await;
    let s = host_session(&h, &host).await;
    let guest = h.create("Guest").await;
    play(
        &h,
        &host,
        &guest,
        duel(&s, &host, &guest, "7", "matchmaking"),
        "finished",
    )
    .await;
    play(
        &h,
        &host,
        &guest,
        duel(&s, &host, &guest, "1", "matchmaking"),
        "quit",
    )
    .await;
    let mut custom = duel(&s, &host, &guest, "1", "custom");
    custom.as_object_mut().unwrap().remove("playlist");
    custom.as_object_mut().unwrap().remove("playlist_id");
    custom.as_object_mut().unwrap().remove("session_id");
    play(&h, &host, &guest, custom, "finished").await;
    let ranks = h.get("/v1/accounts/Host/ranks").await.json();
    assert_eq!(ranks["ranks"], json!({}));
    assert_eq!(h.get("/v1/ranks/1").await.json()["items"], json!([]));
    assert_eq!(
        h.get("/v1/accounts/Nobody/ranks").await.status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        h.get("/v1/ranks/1?limit=0").await.status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn players_cannot_write_their_own_rank() {
    let h = harness().await;
    let host = h.create("Host").await;
    let forged = json!({"xuid": host.xuid, "playlist": "1", "game_id": "g",
        "before": Standing::default(), "after": Standing { level: 50, ..Standing::default() }});
    for stream in [
        format!("rank-{}", host.xuid),
        format!("account-{}", host.xuid),
    ] {
        let jws = h.event(&host, &stream, rank::CHANGED, forged.clone());
        assert_eq!(h.post_event(&host, jws).await.status, StatusCode::FORBIDDEN);
    }
}

#[tokio::test]
async fn the_games_stats_read_gets_every_player_on_every_ranked_leaderboard() {
    let h = harness().await;
    let host = h.create("Host").await;
    let s = host_session(&h, &host).await;
    let guest = h.create("Guest").await;
    play(
        &h,
        &host,
        &guest,
        duel(&s, &host, &guest, "1", "matchmaking"),
        "finished",
    )
    .await;
    // Leaderboards are playlist ids: 1 Rumble Pit (descriptor 0) and
    // 2 Team Slayer (descriptor 1) are ranked, 99 is no playlist. A player
    // never seen is level 1.
    let nobody = "0009000000000099";
    let r = h
        .get(&format!(
            "/v1/rank-stats?leaderboards=99,2,1,2&xuids={},{},{nobody}",
            guest.xuid, host.xuid
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let items = r.json()["items"].as_array().unwrap().clone();
    let rows: Vec<(u64, String, String, u64)> = items
        .iter()
        .map(|i| {
            (
                i["leaderboard"].as_u64().unwrap(),
                i["playlist"].as_str().unwrap().to_owned(),
                i["xuid"].as_str().unwrap().to_owned(),
                i["standing"]["level"].as_u64().unwrap(),
            )
        })
        .collect();
    let mut want = Vec::new();
    let mut xuids = [host.xuid.to_string(), guest.xuid.to_string(), nobody.into()];
    xuids.sort();
    for (board, playlist) in [(1, "0"), (2, "1")] {
        for x in &xuids {
            let level = if board == 2 && *x == host.xuid.to_string() {
                2
            } else {
                1
            };
            want.push((board, playlist.to_owned(), x.clone(), level));
        }
    }
    assert_eq!(rows, want);
    let host_row = items
        .iter()
        .find(|i| i["leaderboard"] == 2 && i["xuid"] == host.xuid.to_string())
        .unwrap();
    assert_eq!(host_row["standing"]["experience"], 100);

    assert_eq!(
        h.get("/v1/rank-stats?leaderboards=1&xuids=").await.json()["items"],
        json!([])
    );
    for bad in [
        "/v1/rank-stats?leaderboards=x&xuids=0009000000000001",
        "/v1/rank-stats?leaderboards=1&xuids=nope",
    ] {
        assert_eq!(h.get(bad).await.status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let many: Vec<String> = (1..=65)
        .map(|i| format!("{:016x}", 0x0009_0000_0000_0000u64 + i))
        .collect();
    assert_eq!(
        h.get(&format!(
            "/v1/rank-stats?leaderboards=1&xuids={}",
            many.join(",")
        ))
        .await
        .status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_matchmaking_game_without_a_playlist_name_gets_the_hoppers_title() {
    let h = harness().await;
    let host = h.create("Host").await;
    let s = host_session(&h, &host).await;
    let guest = h.create("Guest").await;
    let mut game = duel(&s, &host, &guest, "1", "matchmaking");
    game.as_object_mut().unwrap().remove("playlist");
    let id = h.start_game(&host, game).await;
    let g = h.get(&format!("/v1/games/{id}")).await.json();
    assert_eq!(
        (&g["playlist"], &g["playlist_id"]),
        (&json!("Team Slayer"), &json!("1"))
    );
    // A name the console sent stays; an id naming no hopper gets none.
    let named = h
        .start_game(&host, duel(&s, &host, &guest, "1", "matchmaking"))
        .await;
    assert_eq!(
        h.get(&format!("/v1/games/{named}")).await.json()["playlist"],
        "Synthetic"
    );
    let mut unknown = duel(&s, &host, &guest, "40", "matchmaking");
    unknown.as_object_mut().unwrap().remove("playlist");
    let unknown = h.start_game(&host, unknown).await;
    assert_eq!(
        h.get(&format!("/v1/games/{unknown}")).await.json()["playlist"],
        Value::Null
    );
}

/// Ends `game` as a host win without registering any round.
async fn play_unregistered(h: &Harness, host: &User, game: Value) {
    let id = h.start_game(host, game).await;
    let end = h.batch(
        host,
        &id,
        &[(
            "GameEnded",
            json!({"tick": 900, "reason": "finished", "winner": {"team": 0}}),
        )],
    );
    assert_eq!(
        h.post_batch(host, &id, end).await.status,
        StatusCode::CREATED
    );
}

async fn board(h: &Harness) -> Value {
    h.get("/v1/ranks/1").await.json()["items"].clone()
}

async fn arbitrate(h: &Harness, user: &User, path: &str, body: &str) -> StatusCode {
    h.call(
        axum::http::Method::POST,
        &format!("/v1/arbitration/{path}"),
        &[h.bearer(user, live_proto::lxm::GAMES_WRITE)],
        body.to_string(),
    )
    .await
    .status
}

#[tokio::test]
async fn only_the_players_who_registered_the_round_are_ranked() {
    let h = harness().await;
    let host = h.create("Host").await;
    let s = host_session(&h, &host).await;
    let guest = h.create("Guest").await;

    // The guest never registered: only the host would be ranked, and one
    // player is no ranked game.
    let r = round();
    assert_eq!(
        register(&h, &s, &host, &r).await.status,
        StatusCode::CREATED
    );
    play_unregistered(&h, &host, duel(&s, &host, &guest, "1", "matchmaking")).await;
    assert_eq!(board(&h).await, json!([]));

    // The guest alone registered a round: without the host's, no round.
    let r = round();
    assert_eq!(
        register(&h, &s, &guest, &r).await.status,
        StatusCode::CREATED
    );
    play_unregistered(&h, &host, duel(&s, &host, &guest, "1", "matchmaking")).await;
    assert_eq!(board(&h).await, json!([]));

    // No session in the record: no round to find.
    let r = round();
    for u in [&host, &guest] {
        assert_eq!(register(&h, &s, u, &r).await.status, StatusCode::CREATED);
    }
    let mut sessionless = duel(&s, &host, &guest, "1", "matchmaking");
    sessionless.as_object_mut().unwrap().remove("session_id");
    play_unregistered(&h, &host, sessionless).await;
    assert_eq!(board(&h).await, json!([]));

    // Both registered (that same round, still unused): ranked, once.
    play_unregistered(&h, &host, duel(&s, &host, &guest, "1", "matchmaking")).await;
    let host_rank = h
        .get(&format!("/v1/events?stream=rank-{}", host.xuid))
        .await
        .json();
    let c = changes(&host_rank);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0]["data"]["round"], format!("{s}-{r}"));
    // A round ranks one game: the next game without a new round is not.
    play_unregistered(&h, &host, duel(&s, &host, &guest, "1", "matchmaking")).await;
    let again = h
        .get(&format!("/v1/events?stream=rank-{}", host.xuid))
        .await
        .json();
    assert_eq!(changes(&again).len(), 1);
}

#[tokio::test]
async fn arbitration_calls_are_each_accounts_own() {
    let h = harness().await;
    let host = h.create("Host").await;
    let s = host_session(&h, &host).await;
    let r = round();
    let at = format!("{s}/{r}");
    let report = r#"{"flags":3,"procedures":[32769,32771,32775]}"#;
    // Report and extend need the account's registration first.
    assert_eq!(
        arbitrate(&h, &host, &format!("{at}/report"), report).await,
        StatusCode::CONFLICT
    );
    assert_eq!(
        register(&h, &s, &host, &r).await.status,
        StatusCode::CREATED
    );
    assert_eq!(register(&h, &s, &host, &r).await.status, StatusCode::OK);
    assert_eq!(
        arbitrate(&h, &host, &format!("{at}/extend"), r#"{"duration":60}"#).await,
        StatusCode::CREATED
    );
    assert_eq!(
        arbitrate(&h, &host, &format!("{at}/report"), report).await,
        StatusCode::CREATED
    );
    assert_eq!(
        arbitrate(&h, &host, &format!("{at}/report"), report).await,
        StatusCode::OK
    );
    for bad in [format!("{s}/nothex/register"), format!("{s}/{r}/cancel")] {
        assert_eq!(
            arbitrate(&h, &host, &bad, r#"{"duration":1,"flags":0}"#).await,
            StatusCode::NOT_FOUND,
            "{bad}"
        );
    }
    let log = h
        .get(&format!(
            "/v1/events?stream={}",
            live_proto::arbitration::stream(&s, &r)
        ))
        .await
        .json();
    let kinds: Vec<&str> = log["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "ArbitrationRegistered",
            "ArbitrationExtended",
            "ArbitrationReported"
        ]
    );
    assert_eq!(log["items"][0]["data"]["xuid"], host.xuid.to_string());
    assert_eq!(log["items"][0]["author"], "server:live.test");
    // Nobody writes a round's events themselves.
    let forged = h.event(
        &host,
        &live_proto::arbitration::stream(&s, &round()),
        live_proto::arbitration::REGISTERED,
        json!({"session_id": s, "round_id": "0000000000000001", "xuid": host.xuid,
            "duration": 1, "flags": 0}),
    );
    assert_eq!(
        h.post_event(&host, forged).await.status,
        StatusCode::FORBIDDEN
    );
}

/// `u` hosts a Live session (docs/live/live-api.md "Sessions"); its id.
async fn host_session(h: &Harness, u: &User) -> String {
    let lobby = live_proto::session::NewSession {
        addr: Some("192.168.2.4".parse().unwrap()),
        port: 3074,
        host_address: vec![0; 36],
        slots: live_proto::session::Slots {
            public_filled: 1,
            public_open: 7,
            ..Default::default()
        },
        attributes: Default::default(),
        flags: "custom".into(),
        hopper_id: None,
        map: Some("lockout".into()),
        name: Some("Ranked".into()),
        language: None,
        visibility: Default::default(),
        map_id: None,
        skulls: None,
        campaign: None,
        ping_ms: None,
    };
    let r = h
        .call(
            axum::http::Method::POST,
            "/v1/sessions",
            &[
                h.bearer(u, live_proto::lxm::SESSIONS_WRITE),
                ("content-type", "application/json".into()),
            ],
            serde_json::to_vec(&lobby).unwrap(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    r.json()["session_id"].as_str().unwrap().to_string()
}

/// A roster of `count` bots for `session` on playlist 1.
async fn roster(h: &Harness, u: &User, session: &str, count: u32) -> live_proto::bots::Roster {
    let r = h
        .call(
            axum::http::Method::POST,
            "/v1/games/bots",
            &[h.bearer(u, live_proto::lxm::GAMES_WRITE)],
            json!({"session_id": session, "playlist": "1", "count": count}).to_string(),
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    serde_json::from_slice(&r.body).unwrap()
}

#[tokio::test]
async fn bots_rank_with_the_humans_and_made_up_bots_answer_their_roster_level() {
    let h = harness().await;
    let host = h.create("Host").await;
    let session = host_session(&h, &host).await;
    // One persistent bot, from the operator.
    let author = live_proto::Author::Server(h.id.clone()).to_string();
    let operator = format!(
        "Bearer {}",
        h.token(&h.operator, &author, live_server::admin::LXM_ADMIN_WRITE)
    );
    let r = h
        .call(
            axum::http::Method::POST,
            "/v1/admin/bots",
            &[("authorization", operator)],
            r#"{"count":1}"#.to_string(),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    let first = roster(&h, &host, &session, 2).await;
    let persistent = first.bots.iter().find(|b| b.persistent).unwrap().xuid;
    let made_up = first.bots.iter().find(|b| !b.persistent).unwrap().xuid;

    // The host registers the round; bots never do. The host beats both.
    let r = round();
    let path = format!("/v1/arbitration/{session}/{r}/register");
    let reg = h
        .call(
            axum::http::Method::POST,
            &path,
            &[h.bearer(&host, live_proto::lxm::GAMES_WRITE)],
            r#"{"duration":300,"flags":12}"#.to_string(),
        )
        .await;
    assert_eq!(reg.status, StatusCode::CREATED);
    let game = json!({
        "console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Slayer",
        "team_play": true, "tick": 0, "kind": "matchmaking", "playlist_id": "1",
        "session_id": session,
        "players": [
            {"index": 0, "name": "Host", "xuid": host.xuid, "team": 0, "local": true},
            {"index": 1, "name": "Bot", "xuid": persistent, "team": 1, "local": true, "bot": true},
            {"index": 2, "name": "Bot", "xuid": made_up, "team": 1, "local": true, "bot": true}
        ]
    });
    let id = h.start_game(&host, game).await;
    let end = h.batch(
        &host,
        &id,
        &[(
            "GameEnded",
            json!({"tick": 900, "reason": "finished", "winner": {"team": 0}}),
        )],
    );
    assert_eq!(
        h.post_batch(&host, &id, end).await.status,
        StatusCode::CREATED
    );

    let events = |x: live_proto::Xuid| {
        let h = &h;
        async move { changes(&h.get(&format!("/v1/events?stream=rank-{x}")).await.json()) }
    };
    let won = events(host.xuid).await;
    assert_eq!(won.len(), 1);
    assert!(won[0]["data"].get("bot").is_none());
    assert_eq!(won[0]["data"]["after"]["level"], 2);
    // The persistent bot is ranked, marked; it lost at level 1, so only its
    // games moved.
    let lost = events(persistent).await;
    assert_eq!(lost.len(), 1);
    assert_eq!(lost[0]["data"]["bot"], true);
    assert_eq!(lost[0]["data"]["after"]["games"], 1);
    // The made-up bot was an opponent, with no standing to keep.
    assert!(events(made_up).await.is_empty());

    // Bots are on the leaderboard, named and marked, and have rank pages.
    let board = h.get("/v1/ranks/1").await.json();
    let bot_row = board["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["xuid"] == persistent.to_string())
        .unwrap()
        .clone();
    assert_eq!(bot_row["bot"], true);
    assert_ne!(bot_row["gamertag"], "");
    assert!(board["items"][0].get("bot").is_none(), "the host leads");
    let page = h.get(&format!("/v1/accounts/{persistent}/ranks")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(page.json()["bot"], true);

    // A new roster at the host's level 2: its made-up bot's stat read, in
    // that session, answers level 2; outside it, the unseen level 1.
    let second = roster(&h, &host, &session, 2).await;
    let fresh = second.bots.iter().find(|b| !b.persistent).unwrap();
    assert_eq!(fresh.rank.as_ref().unwrap().level, 2);
    let read = |q: String| {
        let h = &h;
        async move {
            // Leaderboard 2 is Team Slayer, descriptor 1.
            let r = h
                .get(&format!(
                    "/v1/rank-stats?leaderboards=2&xuids={}{q}",
                    fresh.xuid
                ))
                .await;
            assert_eq!(r.status, StatusCode::OK);
            r.json()["items"][0]["standing"]["level"].as_u64().unwrap()
        }
    };
    assert_eq!(read(format!("&session={session}")).await, 2);
    assert_eq!(read(String::new()).await, 1);
    assert_eq!(
        h.get("/v1/rank-stats?leaderboards=1&xuids=0009000000000001&session=nope")
            .await
            .status,
        StatusCode::BAD_REQUEST
    );
}

/// Ends game `id` as a win for team 0.
async fn end_game(h: &Harness, host: &User, id: &str) {
    let end = h.batch(
        host,
        id,
        &[(
            "GameEnded",
            json!({"tick": 900, "reason": "finished", "winner": {"team": 0}}),
        )],
    );
    assert_eq!(
        h.post_batch(host, id, end).await.status,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn only_the_session_hosts_record_is_ranked() {
    let h = harness().await;
    let host = h.create("Host").await;
    let guest = h.create("Guest").await;
    let s = host_session(&h, &host).await;
    // A session the server does not know has no rounds.
    assert_eq!(
        register(&h, "0123456789abcdef", &host, &round())
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    // The guest's own record of the host's session, both registered: not
    // the session host's game, so nobody is ranked.
    play(
        &h,
        &guest,
        &host,
        duel(&s, &guest, &host, "1", "matchmaking"),
        "finished",
    )
    .await;
    assert_eq!(board(&h).await, json!([]));
}

#[tokio::test]
async fn a_game_uses_a_round_registered_before_it_started() {
    let h = harness().await;
    let host = h.create("Host").await;
    let guest = h.create("Guest").await;
    let s = host_session(&h, &host).await;
    // Registered only after the game started: not this game's round.
    let id = h
        .start_game(&host, duel(&s, &host, &guest, "1", "matchmaking"))
        .await;
    let r = round();
    for u in [&host, &guest] {
        assert_eq!(register(&h, &s, u, &r).await.status, StatusCode::CREATED);
    }
    end_game(&h, &host, &id).await;
    assert_eq!(board(&h).await, json!([]));
    // It was registered before the next one started: that game's.
    play_unregistered(&h, &host, duel(&s, &host, &guest, "1", "matchmaking")).await;
    assert_eq!(board(&h).await.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_game_that_moves_nobody_still_spends_its_round() {
    let h = harness().await;
    let host = h.create("Host").await;
    let guest = h.create("Guest").await;
    let s = host_session(&h, &host).await;
    let r = round();
    for u in [&host, &guest] {
        assert_eq!(register(&h, &s, u, &r).await.status, StatusCode::CREATED);
    }
    // One side only: the rules move nobody.
    let mut one_side = duel(&s, &host, &guest, "1", "matchmaking");
    one_side["players"][1]["team"] = json!(0);
    play_unregistered(&h, &host, one_side).await;
    assert_eq!(board(&h).await, json!([]));
    let log = h
        .get(&format!(
            "/v1/events?stream={}",
            live_proto::arbitration::stream(&s, &r)
        ))
        .await
        .json();
    let used = log["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["type"] == live_proto::arbitration::USED)
        .expect("the round is marked used")
        .clone();
    assert_eq!(used["author"], "server:live.test");
    // So the next game, with no round of its own, is not ranked by it.
    play_unregistered(&h, &host, duel(&s, &host, &guest, "1", "matchmaking")).await;
    assert_eq!(board(&h).await, json!([]));
}
