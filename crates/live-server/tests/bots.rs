//! Bot accounts and rosters through the router (docs/game/bots.md §9):
//! `POST /v1/games/bots` for a session's host, persistent bots from the
//! operator, and a game record's `bot: true` entries accepted only for the
//! session's roster.

mod common;

use std::collections::BTreeMap;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use common::*;
use live_proto::bots::{self, Roster};
use live_proto::session::{NewSession, Slots};
use live_proto::{Author, Xuid, lxm};
use serde_json::{Value, json};

fn lobby() -> NewSession {
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
        flags: "custom".into(),
        hopper_id: None,
        map: Some("lockout".into()),
        name: Some("Bots".into()),
        language: None,
        visibility: Default::default(),
        map_id: None,
        skulls: None,
        campaign: None,
        ping_ms: None,
    }
}

/// `u` hosts a session; its id.
async fn host(h: &Harness, u: &User) -> String {
    let r = h
        .call(
            Method::POST,
            "/v1/sessions",
            &[
                h.bearer(u, lxm::SESSIONS_WRITE),
                ("content-type", "application/json".into()),
            ],
            Body::from(serde_json::to_vec(&lobby()).unwrap()),
        )
        .await;
    assert_eq!(r.status, StatusCode::CREATED);
    r.json()["session_id"].as_str().unwrap().to_string()
}

async fn ask(h: &Harness, u: &User, req: Value) -> Resp {
    h.call(
        Method::POST,
        "/v1/games/bots",
        &[h.bearer(u, lxm::GAMES_WRITE)],
        req.to_string(),
    )
    .await
}

async fn roster(h: &Harness, u: &User, req: Value) -> Roster {
    let r = ask(h, u, req).await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    serde_json::from_slice(&r.body).unwrap()
}

fn operator(h: &Harness) -> (&'static str, String) {
    let author = Author::Server(h.id.clone()).to_string();
    (
        "authorization",
        format!(
            "Bearer {}",
            h.token(&h.operator, &author, live_server::admin::LXM_ADMIN_WRITE)
        ),
    )
}

fn custom(host: &User, session: &str, players: Value) -> Value {
    let mut all =
        vec![json!({"index": 0, "name": "Host", "xuid": host.xuid, "team": 0, "local": true})];
    all.extend(players.as_array().unwrap().iter().cloned());
    json!({
        "console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Slayer", "team_play": false,
        "tick": 0, "kind": "custom", "session_id": session, "players": all
    })
}

async fn try_start(h: &Harness, u: &User, game: Value) -> Resp {
    h.call(
        Method::POST,
        "/v1/games",
        &[h.bearer(u, lxm::GAMES_WRITE)],
        game.to_string(),
    )
    .await
}

fn bot_entry(index: u32, x: Xuid) -> Value {
    json!({"index": index, "name": "Bot", "xuid": x, "team": 1, "local": true, "bot": true})
}

#[tokio::test]
async fn a_host_gets_a_roster_of_issued_bots_for_its_session() {
    let h = harness().await;
    let a = h.create("Host").await;
    let session = host(&h, &a).await;
    let r = roster(
        &h,
        &a,
        json!({"session_id": session, "count": 3, "skill": 0.67}),
    )
    .await;
    assert_eq!(r.match_id, session);
    assert_eq!(r.bots.len(), 3);
    r.check().unwrap();
    for b in &r.bots {
        assert!(bots::is_server_xuid(b.xuid), "{}", b.xuid);
        assert!(!b.persistent && b.rank.is_none() && b.team.is_none());
        assert_eq!(b.skill, 0.67);
    }
    h.live.read_bots(|s| {
        for b in &r.bots {
            assert_eq!(s.bot(b.xuid).unwrap().gamertag, b.gamertag);
        }
        let (rostered, bot) = s.rostered(&session, r.bots[1].xuid, u64::MAX).unwrap();
        assert_eq!(rostered.host, a.xuid);
        assert!(!bot.persistent);
    });
    // Made-up bots do not hold their names against accounts.
    h.create(r.bots[0].gamertag.as_str()).await;
}

#[tokio::test]
async fn only_the_sessions_host_asks_and_counts_are_bounded() {
    let h = harness().await;
    let a = h.create("Host").await;
    let b = h.create("Other").await;
    let session = host(&h, &a).await;
    let not_host = ask(&h, &b, json!({"session_id": session, "count": 1})).await;
    assert_eq!(not_host.status, StatusCode::NOT_FOUND);
    let none = ask(
        &h,
        &a,
        json!({"session_id": "0123456789abcdef", "count": 1}),
    )
    .await;
    assert_eq!(none.status, StatusCode::NOT_FOUND);
    for count in [0, 16] {
        let r = ask(&h, &a, json!({"session_id": session, "count": count})).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "count {count}");
    }
    let skill = ask(
        &h,
        &a,
        json!({"session_id": session, "count": 1, "skill": 1.5}),
    )
    .await;
    assert_eq!(skill.status, StatusCode::BAD_REQUEST);
    let anon = h
        .call(
            Method::POST,
            "/v1/games/bots",
            &[],
            json!({"session_id": session, "count": 1}).to_string(),
        )
        .await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
    // A full roster: 15 bots beside the host.
    assert_eq!(
        roster(&h, &a, json!({"session_id": session, "count": 15}))
            .await
            .bots
            .len(),
        15
    );
}

#[tokio::test]
async fn nobody_else_writes_bot_streams() {
    let h = harness().await;
    let a = h.create("Host").await;
    let x = Xuid(bots::SERVER_XUID_MARK | 1);
    let data = json!({"xuid": x, "gamertag": "Fake Bot", "persistent": true,
        "emblem": {"foreground": 0, "background": 0, "colors": [0, 0, 0]},
        "colors": {"primary": 0, "secondary": 0},
        "personality": {"seed": 0, "aggression": 0.5, "caution": 0.5, "objective": 0.5,
            "teamwork": 0.5, "taunt": 0.3}});
    for (stream, kind) in [
        (bots::bot_stream(x), bots::ISSUED),
        (bots::roster_stream("0123456789abcdef"), bots::ROSTERED),
        (live_proto::streams::account(a.xuid), bots::ISSUED),
    ] {
        let jws = h.event(&a, &stream, kind, data.clone());
        let r = h.post_event(&a, jws).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{stream} {kind}");
    }
}

#[tokio::test]
async fn bot_entries_are_accepted_only_for_the_sessions_roster() {
    let h = harness().await;
    let a = h.create("Host").await;
    let human = h.create("Human").await;
    let session = host(&h, &a).await;
    let r = roster(&h, &a, json!({"session_id": session, "count": 2})).await;
    let (b0, b1) = (r.bots[0].xuid, r.bots[1].xuid);

    // The roster's bots, as bots: accepted.
    let ok = custom(&a, &session, json!([bot_entry(1, b0), bot_entry(2, b1)]));
    let id = h.start_game(&a, ok).await;

    // A human claimed as a bot, a bot as a human, a host-made xuid, a bot
    // without its session: each refused whole.
    let refused = [
        custom(&a, &session, json!([bot_entry(1, human.xuid)])),
        custom(
            &a,
            &session,
            json!([{"index": 1, "name": "Bot", "xuid": b0, "team": 1}]),
        ),
        custom(
            &a,
            &session,
            json!([{"index": 1, "name": "Clanker 4", "xuid": bots::local_xuid(4), "team": 1}]),
        ),
        {
            let mut g = custom(&a, &session, json!([bot_entry(1, b0)]));
            g.as_object_mut().unwrap().remove("session_id");
            g
        },
    ];
    for g in refused {
        let r = try_start(&h, &a, g.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{g}");
    }
    // Another session's roster does not carry over.
    let c = h.create("Second").await;
    let other = host(&h, &c).await;
    let r = try_start(&h, &c, custom(&c, &other, json!([bot_entry(1, b0)]))).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Joining mid-game: a rostered bot is fine, anything else is not.
    let join = |p: Value| {
        h.batch(
            &a,
            &id,
            &[("PlayerJoined", json!({"tick": 10, "player": p}))],
        )
    };
    let r = h
        .post_batch(
            &a,
            &id,
            join(json!({"index": 3, "name": "Late", "xuid": human.xuid, "team": 0, "bot": true})),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = h
        .post_batch(
            &a,
            &id,
            join(json!({"index": 3, "name": "Human", "xuid": human.xuid, "team": 0})),
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
}

#[tokio::test]
async fn a_game_reads_the_roster_in_force_when_it_started() {
    let h = harness().await;
    let a = h.create("Host").await;
    let session = host(&h, &a).await;
    let first = roster(&h, &a, json!({"session_id": session, "count": 1})).await;
    let started = try_start(
        &h,
        &a,
        custom(&a, &session, json!([bot_entry(1, first.bots[0].xuid)])),
    )
    .await;
    assert_eq!(started.status, StatusCode::CREATED);
    let id = started.json()["game_id"].as_str().unwrap().to_string();
    let started = started.json()["position"].as_u64().unwrap();
    let second = roster(&h, &a, json!({"session_id": session, "count": 1})).await;
    let (x1, x2) = (first.bots[0].xuid, second.bots[0].xuid);
    h.live.read_bots(|s| {
        assert!(s.rostered(&session, x1, started).is_some());
        assert!(s.rostered(&session, x2, started).is_none());
        assert!(s.rostered(&session, x2, u64::MAX).is_some());
        assert!(s.rostered(&session, x1, u64::MAX).is_none(), "replaced");
    });
    // The game's own bots stay its bots for a mid-game join; the new
    // roster's are the next game's.
    let late = h.batch(
        &a,
        &id,
        &[(
            "PlayerJoined",
            json!({"tick": 10, "player": bot_entry(2, x2)}),
        )],
    );
    assert_eq!(
        h.post_batch(&a, &id, late).await.status,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn persistent_bots_come_from_the_operator_and_fill_playlist_rosters_first() {
    let h = harness().await;
    let a = h.create("Host").await;
    let session = host(&h, &a).await;
    let not_operator = h
        .call(
            Method::POST,
            "/v1/admin/bots",
            &[h.bearer(&a, live_server::admin::LXM_ADMIN_WRITE)],
            r#"{"count":2}"#.to_string(),
        )
        .await;
    assert_eq!(not_operator.status, StatusCode::FORBIDDEN);
    let r = h
        .call(
            Method::POST,
            "/v1/admin/bots",
            &[operator(&h)],
            r#"{"count":2}"#.to_string(),
        )
        .await;
    assert_eq!(
        r.status,
        StatusCode::CREATED,
        "{}",
        String::from_utf8_lossy(&r.body)
    );
    let issued: Vec<Xuid> = r.json()["bots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| serde_json::from_value(b["xuid"].clone()).unwrap())
        .collect();
    assert_eq!(issued.len(), 2);
    {
        // A persistent bot holds its gamertag: no account can take it.
        let tag = h
            .live
            .read_bots(|s| s.bot(issued[0]).unwrap().gamertag.clone());
        let key = live_proto::sign::KeyPair::generate();
        let r = h
            .call(
                Method::POST,
                "/v1/accounts",
                &[],
                live_proto::sign::AccountRequest::sign(
                    &key,
                    tag.as_str(),
                    None,
                    "live.test",
                    now(),
                ),
            )
            .await;
        assert_eq!(
            r.status,
            StatusCode::CONFLICT,
            "{}",
            String::from_utf8_lossy(&r.body)
        );
    }

    let r = roster(
        &h,
        &a,
        json!({"session_id": session, "playlist": "7", "count": 3}),
    )
    .await;
    let persistent: Vec<Xuid> = r
        .bots
        .iter()
        .filter(|b| b.persistent)
        .map(|b| b.xuid)
        .collect();
    assert_eq!(persistent.len(), 2);
    assert!(persistent.iter().all(|x| issued.contains(x)));
    for b in &r.bots {
        let rank = b.rank.as_ref().unwrap();
        assert_eq!(rank.playlist, "7");
        assert_eq!(rank.level, 1, "nobody has played: level 1");
        assert_eq!(b.skill, live_proto::rank::bot_skill(1));
    }
    h.live.read_bots(|s| {
        let (_, bot) = s.rostered(&session, persistent[0], u64::MAX).unwrap();
        assert!(bot.persistent && bot.level == 1);
    });
}
