//! Clankermatch through the router (docs/game/bots.md §8, §9.4): the
//! clanker hoppers are served by descriptor index, and a game recorded as
//! `kind: "clankermatch"` reads back as its own kind and ranks in its
//! clanker hopper like a human game.

mod common;

use axum::http::StatusCode;
use common::*;
use live_proto::hopper::clankers::{self, ClankerHopper, ClankerHoppers};
use live_proto::rank::{self, Standing};
use serde_json::{Value, json};

/// A two-player game of `kind` in hopper `playlist` (a descriptor index)
/// in `host`'s Live session `session`, both players with accounts.
fn duel(session: &str, host: &User, guest: &User, playlist: &str, kind: &str) -> Value {
    json!({
        "console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Rumble Slayer",
        "team_play": true, "tick": 0, "kind": kind,
        "playlist_id": playlist,
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

/// Plays `game` to a finish, host and guest having registered a fresh
/// round of its session first; the host wins.
async fn play(h: &Harness, host: &User, guest: &User, game: Value) -> String {
    let session = game["session_id"].as_str().unwrap().to_string();
    let r = round();
    for u in [host, guest] {
        assert_eq!(
            register(h, &session, u, &r).await.status,
            StatusCode::CREATED
        );
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
                json!({"tick": 900, "reason": "finished", "winner": {"team": 0}}),
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

/// `u` hosts a Live session; its id.
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
        name: Some("Clankers".into()),
        language: None,
        visibility: Default::default(),
        map_id: None,
        skulls: None,
        campaign: None,
        ping_ms: None,
        variant_ext: None,
        rules: None,
        firefight: None,
        browser: None,
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

#[tokio::test]
async fn a_fresh_server_serves_the_default_clanker_hoppers() {
    let h = harness().await;
    let r = h.get(&format!("/v1{}", clankers::PATH)).await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let marks: ClankerHoppers = serde_json::from_slice(&r.body).unwrap();
    let hopper = |hopper_id, playlist_id, players, teams, ranked| ClankerHopper {
        hopper_id,
        playlist_id,
        players,
        teams,
        ranked,
        ranks_as: None,
    };
    assert_eq!(
        marks.hoppers,
        [
            hopper(8, 9, 8, 0, true),
            hopper(9, 10, 8, 2, true),
            hopper(10, 11, 4, 2, true),
            hopper(11, 12, 8, 2, true),
            hopper(12, 13, 16, 2, true),
            hopper(13, 14, 16, 2, true),
            hopper(14, 15, 8, 0, false),
            hopper(15, 16, 8, 2, false),
        ]
    );
}

#[tokio::test]
async fn a_clankermatch_game_reads_back_as_its_own_kind_and_ranks() {
    let h = harness().await;
    let host = h.create("Host").await;
    let s = host_session(&h, &host).await;
    let guest = h.create("Guest").await;
    // Descriptor 8 is the clanker Rumble Pit: an unranked descriptor type,
    // marked ranked in its clanker mark.
    let id = play(
        &h,
        &host,
        &guest,
        duel(&s, &host, &guest, "8", "clankermatch"),
    )
    .await;

    // Its own kind in the list and on the game page, named from the
    // served hopper.
    let list = h.get("/v1/games").await.json();
    let item = list["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|g| g["game_id"] == id.as_str())
        .unwrap()
        .clone();
    assert_eq!(item["kind"], "clankermatch");
    assert_eq!(item["playlist_id"], "8");
    assert_eq!(item["playlist"], "Rumble Pit");
    let page = h.get(&format!("/v1/games/{id}")).await;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(page.json()["kind"], "clankermatch");

    // Ranked like a human game, in its own hopper (no ranks_as).
    let events = h
        .get(&format!("/v1/events?stream=rank-{}", host.xuid))
        .await
        .json();
    let changed: Vec<Value> = events["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == rank::CHANGED)
        .cloned()
        .collect();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["data"]["game_id"], id.as_str());
    assert_eq!(changed[0]["data"]["playlist"], "8");
    let after: Standing = serde_json::from_value(changed[0]["data"]["after"].clone()).unwrap();
    assert_eq!((after.level, after.wins), (2, 1));

    // The game's stats read for its playlist id (9) answers that rank.
    let r = h
        .get(&format!(
            "/v1/rank-stats?leaderboards=9&xuids={}",
            host.xuid
        ))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let items = r.json()["items"].clone();
    assert_eq!(items[0]["playlist"], "8");
    assert_eq!(items[0]["standing"]["level"], 2);

    // A human hopper's game recorded as clankermatch ranks nobody.
    play(
        &h,
        &host,
        &guest,
        duel(&s, &host, &guest, "6", "clankermatch"),
    )
    .await;
    // Neither does an unranked clanker training game (Team Training).
    play(
        &h,
        &host,
        &guest,
        duel(&s, &host, &guest, "15", "clankermatch"),
    )
    .await;
    let ranks = h.get("/v1/accounts/Host/ranks").await.json();
    assert_eq!(ranks["ranks"].as_object().unwrap().len(), 1);
    let training = h
        .get(&format!(
            "/v1/rank-stats?leaderboards=16&xuids={}",
            host.xuid
        ))
        .await;
    assert_eq!(training.status, StatusCode::OK);
    assert_eq!(training.json()["items"], json!([]));
}

/// The match lobby a Team Skirmish search hosts (descriptor 10): a
/// session with the hopper attribute, `filled` of four seats.
async fn doubles_session(h: &Harness, u: &User, filled: u32) -> String {
    let mut attributes = std::collections::BTreeMap::new();
    attributes.insert(
        live_proto::optimatch::attr::HOPPER,
        live_proto::session::AttributeValue::Integer(10),
    );
    let lobby = live_proto::session::NewSession {
        addr: Some("192.168.2.4".parse().unwrap()),
        port: 3074,
        host_address: vec![0; 36],
        slots: live_proto::session::Slots {
            public_filled: filled,
            public_open: 4 - filled,
            ..Default::default()
        },
        attributes,
        // The HLE advertises a matchmaking lobby as it does any other
        // (live-hle sessions.rs): "custom" flags, the hopper an attribute.
        flags: "custom".into(),
        hopper_id: None,
        map: Some("lockout".into()),
        name: Some("Team Skirmish".into()),
        language: None,
        visibility: Default::default(),
        map_id: None,
        skulls: None,
        campaign: None,
        ping_ms: None,
        variant_ext: None,
        rules: None,
        firefight: None,
        browser: None,
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
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    r.json()["session_id"].as_str().unwrap().to_string()
}

/// Team Skirmish from the lobby to a ranked game, for a party of one
/// and of two (docs/game/bots.md §8.4): the host's match session, a roster
/// of the seats the party leaves (three bots, or two) for the clanker
/// playlist, every person's arbitration registration of the round the
/// full lobby makes, and the host's game record with the party and the
/// bots, which reads back as a Clankermatch in Team Skirmish and ranks
/// the people there.
#[tokio::test]
async fn a_clanker_doubles_match_reaches_its_game_for_a_party_of_one_or_two() {
    for party in [1usize, 2] {
        let h = harness().await;
        let host = h.create("Host").await;
        let guest = h.create("Guest").await;
        let people: Vec<(&str, &User)> = [("Host", &host), ("Guest", &guest)]
            .into_iter()
            .take(party)
            .collect();
        let s = doubles_session(&h, &host, party as u32).await;

        // The fill's one roster: the seats the party leaves, for the
        // clanker playlist, with teams, skill from the party's level.
        let bots = 4 - party;
        let r = h
            .call(
                axum::http::Method::POST,
                "/v1/games/bots",
                &[h.bearer(&host, live_proto::lxm::GAMES_WRITE)],
                json!({"session_id": s, "playlist": "10", "count": bots, "teams": true})
                    .to_string(),
            )
            .await;
        assert_eq!(
            r.status,
            StatusCode::CREATED,
            "party {party}: {}",
            String::from_utf8_lossy(&r.body)
        );
        let roster: live_proto::bots::Roster = serde_json::from_slice(&r.body).unwrap();
        assert_eq!(roster.match_id, s);
        assert_eq!(roster.bots.len(), bots, "party {party}");
        roster.check().unwrap();
        assert!(
            roster.bots.iter().all(|b| b.rank.is_some()),
            "a playlist roster shows each bot's rank"
        );

        // The full lobby's round: each person's console registers it.
        let rd = round();
        for (name, u) in &people {
            assert_eq!(
                register(&h, &s, u, &rd).await.status,
                StatusCode::CREATED,
                "party {party}: {name}"
            );
        }

        // The game: the party on red, the bots wherever the host put them.
        let mut players: Vec<Value> = people
            .iter()
            .enumerate()
            .map(|(i, (name, u))| {
                json!({"index": i, "name": name, "xuid": u.xuid, "team": 0, "local": i == 0})
            })
            .collect();
        for (i, b) in roster.bots.iter().enumerate() {
            let team = if party + i < 2 { 0 } else { 1 };
            players.push(json!({
                "index": party + i, "name": b.gamertag, "xuid": b.xuid,
                "team": team, "local": false, "bot": true
            }));
        }
        assert_eq!(players.len(), 4, "2 v 2");
        let game = json!({
            "console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Team Slayer",
            "team_play": true, "tick": 0, "kind": "clankermatch",
            "playlist_id": "10", "session_id": s, "players": players
        });
        let id = h.start_game(&host, game).await;
        let scores: Vec<Value> = (0..4)
            .map(|i| json!({"index": i, "score": 4 - i, "kills": 4 - i, "deaths": i, "assists": 0}))
            .collect();
        let end = h.batch(
            &host,
            &id,
            &[
                (
                    "ScoreChanged",
                    json!({"tick": 800, "players": scores,
                        "teams": [{"team": 0, "score": 50}, {"team": 1, "score": 30}]}),
                ),
                (
                    "GameEnded",
                    json!({"tick": 900, "reason": "finished", "winner": {"team": 0}}),
                ),
            ],
        );
        let r = h.post_batch(&host, &id, end).await;
        assert_eq!(
            r.status,
            StatusCode::CREATED,
            "party {party}: {}",
            String::from_utf8_lossy(&r.body)
        );

        let page = h.get(&format!("/v1/games/{id}")).await;
        assert_eq!(page.status, StatusCode::OK);
        let page = page.json();
        assert_eq!(page["kind"], "clankermatch");
        assert_eq!(page["playlist"], "Team Skirmish");
        // Each person ranks in Team Skirmish.
        for (name, u) in &people {
            let events = h
                .get(&format!("/v1/events?stream=rank-{}", u.xuid))
                .await
                .json();
            let changed: Vec<&Value> = events["items"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|e| e["type"] == rank::CHANGED)
                .collect();
            assert_eq!(changed.len(), 1, "party {party}: {name}");
            assert_eq!(changed[0]["data"]["playlist"], "10");
        }
    }
}
