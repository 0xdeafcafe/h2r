use std::{net::Ipv4Addr, sync::Arc};

use live_proto::{
    ClientMessage, ErrorCode, IngestReply,
    hosts::{HostClass, ProviderKind, Transport},
    session::{self, NewSession},
    sign::{EventBody, issue_token, sign_batch},
};
use serde_json::{Value, json};

use super::*;
use crate::{
    Policy,
    app::Timers,
    ingest::IngestSession,
    store::{MemoryStore, NewEvent},
    stream::Out,
};

const ASSIGNMENT: &str = "8000000000000001";
const SESSION: &str = "8000000000000003";

fn identity() -> ServerIdentity {
    ServerIdentity {
        id: "live.test".into(),
        key: KeyPair::generate(),
    }
}

fn lease() -> HostLease {
    HostLease {
        id: "hl-AAAAAAAAAAAAAAAAAAAAAA".into(),
        assignment_id: ASSIGNMENT.into(),
        provider: ProviderKind::Local,
        region: None,
        class: HostClass::Small,
        map: "lockout".into(),
        map_id: None,
        variant: None,
        hopper: Some(1),
        max_players: 4,
        roster: vec![Xuid(1), Xuid(2), Xuid(3)],
        bots: vec![],
        transport: Transport::Udp,
        idle_timeout_s: 120,
        max_match_s: 1800,
        created_ms: now() * 1000,
    }
}

#[test]
fn token_scope_subject_issuer_and_expiry() {
    let server = identity();
    let lease = lease();
    let token = HostClaims::issue(&server, &lease, 100);
    let exp = 100 + i64::from(lease.max_match_s) + 300;
    for scope in SCOPES {
        let c = HostClaims::verify(&server, &token, scope, exp - 1).unwrap();
        assert_eq!(lease_id(&c.sub), Some(lease.id.as_str()));
        assert_eq!(c.lxm.iter().map(String::as_str).collect::<Vec<_>>(), SCOPES);
        assert_eq!(c.exp, exp);
    }
    assert!(HostClaims::verify(&server, &token, TELEMETRY_INGEST, 99).is_err());
    assert!(HostClaims::verify(&server, &token, TELEMETRY_INGEST, exp).is_err());
    assert!(HostClaims::verify(&server, &token, lxm::GAMES_WRITE, 100).is_err());
    assert!(HostClaims::verify(&identity(), &token, HOST_REPORT, 100).is_err());
    assert!(
        Token::parse(&token).is_err(),
        "general account authentication must reject host claims"
    );

    let mut c = HostClaims::verify(&server, &token, HOST_REPORT, 100).unwrap();
    c.lxm = vec![HOST_REPORT.into()];
    let limited = server.key.sign_json(TYP_TOKEN, &c);
    assert!(HostClaims::verify(&server, &limited, HOST_REGISTER, 100).is_err());
    c.aud = "another.test".into();
    assert!(
        HostClaims::verify(
            &server,
            &server.key.sign_json(TYP_TOKEN, &c),
            HOST_REPORT,
            100
        )
        .is_err()
    );
}

async fn live() -> Arc<Live> {
    Live::start_with(
        Arc::new(MemoryStore::new()),
        identity(),
        Policy::default(),
        Timers::Driven,
    )
    .await
    .unwrap()
}

async fn record(live: &Live, events: Vec<NewEvent>) {
    let rows = live.store.append(events, &[]).await.unwrap();
    live.wait_applied(rows.last().unwrap().position)
        .await
        .unwrap();
}

#[tokio::test]
async fn host_lifecycle_events_are_admin_only_in_feed_and_stream() {
    use crate::{sessions::Viewer, store::Query};
    use live_proto::hosts::{HOST_BOOTING, HOST_REQUESTED, STREAM};

    let live = live().await;
    let lease = lease();
    let roster_member = lease.roster[0];
    let requested = live.server_event(STREAM.into(), HOST_REQUESTED, json!({ "lease": &lease }));
    let booting = live.server_event(
        STREAM.into(),
        HOST_BOOTING,
        json!({ "lease_id": lease.id, "provider_ref": "test-provider" }),
    );
    let rows = live.store.append(vec![requested, booting], &[]).await.unwrap();
    let last = rows.last().unwrap().position;
    live.wait_applied(last).await.unwrap();

    for (viewer, visible) in [
        (Viewer::Anonymous, false),
        (Viewer::Account(roster_member), false),
        (Viewer::Account(Xuid(99)), false),
        (Viewer::Admin, true),
        (Viewer::Operator, true),
    ] {
        for event in &rows {
            assert_eq!(
                live.wire_for(event, viewer).is_some(),
                visible,
                "{viewer:?} WebSocket: {}", event.body.kind,
            );
        }
        for filter in [None, Some(STREAM.to_owned())] {
            let query = Query {
                stream: filter,
                ..Query::after(rows[0].position - 1, rows.len())
            };
            let page = live.events_for(&query, viewer).await.unwrap();
            for event in &rows {
                assert_eq!(
                    page.items.iter().any(|e| e.position == event.position),
                    visible,
                    "{viewer:?} event feed: {}", event.body.kind,
                );
            }
            assert_eq!(page.next, Some(last.to_string()));
        }
    }
}

fn ad() -> NewSession {
    serde_json::from_value(json!({
        "port":3074, "host_address":vec![0; 36],
        "slots":{"public_open":4, "public_filled":0,"private_open":0,"private_filled":0},
        "attributes":{}, "map":"lockout"
    }))
    .unwrap()
}

fn new_game() -> game::NewGame {
    serde_json::from_value(json!({
        "map":"lockout","mode":"multiplayer","team_play":false,
        "players":[{"index":0,"name":"One","xuid":Xuid(1),"team":0},
                   {"index":1,"name":"Two","xuid":Xuid(2),"team":1}]
    }))
    .unwrap()
}

fn batch(live: &Live, token: &str, game_id: &str, events: &[(&str, Value)]) -> String {
    let c = HostClaims::verify(&live.identity, token, TELEMETRY_INGEST, now()).unwrap();
    let key = KeyPair::from_secret_b64(&c.event_key).unwrap();
    let stream = game::stream(game_id);
    let bodies: Vec<_> = events
        .iter()
        .map(|(kind, data)| EventBody::at_unix(&key, &c.sub, &stream, kind, data.clone(), now()))
        .collect();
    sign_batch(&key, &bodies)
}

fn end() -> (&'static str, Value) {
    (
        game::GAME_ENDED,
        json!({"reason":"finished","tick":90,"winner":{"player":0}}),
    )
}

/// A publication already persisted by matchmaking. Exercising the log also
/// checks that rank admission works after the transient ticket expires.
async fn assigned_game(live: &Live, lease: &HostLease) -> (String, String) {
    let assignment = json!({"id":ASSIGNMENT, "hopper":1, "host":Xuid(0), "dedicated":true,
        "members":lease.roster, "rtt_ms":{}, "host_lease":lease.id});
    let mut advertisement = ad();
    advertisement.flags = "ranked".into();
    advertisement.hopper_id = Some(1);
    advertisement.attributes.insert(
        live_proto::optimatch::attr::HOPPER,
        session::AttributeValue::Integer(1),
    );
    let item = json!({"session_id":SESSION, "host":format!("host:{}",lease.id),
        "host_xuid":Xuid(0), "host_gamertag":"", "xnkid":SESSION,
        "endpoint":{"addr":"127.0.0.1","port":3074,"source":"observed"},
        "advertisement":advertisement, "matchmaking":assignment,
        "since":live_proto::sign::format_unix(now()), "last_activity":now(), "state":"open"});
    record(
        live,
        vec![
            live.server_event(
                live_proto::hosts::STREAM.into(),
                live_proto::hosts::HOST_REQUESTED,
                json!({"lease":lease}),
            ),
            live.server_event(session::stream(SESSION), session::CREATED, item),
        ],
    )
    .await;
    let token = live.issue_host_token(lease);
    let game = live.start_game(Some(&token), new_game()).await.unwrap();
    assert_eq!(
        live.start_game(Some(&token), new_game()).await.unwrap(),
        game
    );
    (token, game.game_id)
}

#[tokio::test]
async fn signed_finish_ranks_a_roster_subset_once_and_rejects_wrong_lease() {
    let live = live().await;
    let lease = lease();
    let (token, id) = assigned_game(&live, &lease).await;
    assert_eq!(
        live.verify_host_token(Some(&token), HOST_REGISTER).unwrap(),
        lease.id
    );
    let mut other = lease.clone();
    other.id = "hl-BBBBBBBBBBBBBBBBBBBBBB".into();
    let other_token = live.issue_host_token(&other);
    let body = batch(&live, &token, &id, &[end()]);
    assert!(
        live.append_game_events(Some(&other_token), &id, &body)
            .await
            .is_err()
    );
    // A correctly signed batch by the other lease cannot write here either.
    let other_body = batch(&live, &other_token, &id, &[end()]);
    assert!(
        live.append_game_events(Some(&other_token), &id, &other_body)
            .await
            .is_err()
    );
    let impostor = KeyPair::generate();
    let (kind, data) = end();
    let forged = EventBody::at_unix(
        &impostor,
        &format!("host:{}", lease.id),
        &game::stream(&id),
        kind,
        data,
        now(),
    );
    let forged = sign_batch(&impostor, &[forged]);
    assert!(
        live.append_game_events(Some(&token), &id, &forged)
            .await
            .is_err()
    );
    let account = Author::Account {
        xuid: Xuid(1),
        server: live.identity.id.clone(),
    };
    assert!(
        live.ingest_frame(
            &account,
            game::Frame {
                game_id: id.clone(),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );

    let secondary = "secondary";
    let mut client_game = new_game();
    client_game.session_id = Some(SESSION.into());
    record(
        &live,
        vec![live.server_event(
            game::stream(secondary),
            game::GAME_STARTED,
            json!(game::GameStarted {
                game_id: secondary.into(),
                host: account.to_string(),
                game: client_game,
            }),
        )],
    )
    .await;
    assert!(
        live.ingest_frame(
            &account,
            game::Frame {
                game_id: secondary.into(),
                ..Default::default()
            }
        )
        .await
        .is_err()
    );

    let mut socket = IngestSession::new(live.clone());
    let auth = serde_json::to_string(&ClientMessage::Auth { token }).unwrap();
    assert!(socket.on_text(&auth).await.is_empty());
    let mut resumed = IngestSession::resumed(live.clone(), socket.resume());
    let frame = ClientMessage::Frame {
        frame: Box::new(game::Frame {
            game_id: id.clone(),
            tick: 2,
            ..Default::default()
        }),
    };
    assert!(
        resumed
            .on_text(&serde_json::to_string(&frame).unwrap())
            .await
            .is_empty()
    );
    let events = ClientMessage::Events {
        game_id: id.clone(),
        jws: body,
    };
    let out = resumed
        .on_text(&serde_json::to_string(&events).unwrap())
        .await;
    let [Out::Text(reply)] = &out[..] else {
        panic!("expected one acknowledgement")
    };
    assert!(matches!(
        serde_json::from_str::<IngestReply>(reply).unwrap(),
        IngestReply::Appended { .. }
    ));
    live.rank_game(&id).await.unwrap();
    let state = live.model.read();
    assert_eq!(state.ranks.standing(Xuid(1), "1").games, 1);
    assert_eq!(state.ranks.standing(Xuid(2), "1").games, 1);
    assert_eq!(state.ranks.standings["1"].len(), 2);
    assert!(state.ranks.host_assignments.contains(ASSIGNMENT));
    assert!(state.presence.is_empty());
}

#[tokio::test]
async fn unsigned_finish_stranger_and_unidentified_quitter_never_rank() {
    for extra in [None, Some(Some(Xuid(99))), Some(None)] {
        let live = live().await;
        let (token, id) = assigned_game(&live, &lease()).await;
        if let Some(xuid) = extra {
            let joined = (
                game::PLAYER_JOINED,
                json!({"player":{
                    "index":2, "xuid":xuid,"team":1,"name":"Third"
                }}),
            );
            let left = (game::PLAYER_LEFT, json!({"index":2}));
            let signed = batch(&live, &token, &id, &[joined, left, end()]);
            live.append_game_events(Some(&token), &id, &signed)
                .await
                .unwrap();
        } else {
            // Even a server-written finish on the assigned host's own game
            // cannot substitute for the host-signed result.
            record(
                &live,
                vec![live.server_event(game::stream(&id), game::GAME_ENDED, end().1)],
            )
            .await;
            live.rank_game(&id).await.unwrap();
        }
        assert!(live.model.read().ranks.games.is_empty());
    }
}

#[tokio::test]
async fn custom_session_has_no_host_player_and_is_bound_to_its_token() {
    let live = live().await;
    let mut lease = lease();
    lease.assignment_id = "custom-test".into();
    lease.hopper = None;
    lease.roster.clear();
    record(
        &live,
        vec![live.server_event(
            live_proto::hosts::STREAM.into(),
            live_proto::hosts::HOST_REQUESTED,
            json!({"lease":lease}),
        )],
    )
    .await;
    let token = live.issue_host_token(&lease);
    let addr = Some(Ipv4Addr::LOCALHOST.into());
    let session = live.create_session(Some(&token), ad(), addr).await.unwrap();
    assert_eq!(session.host, format!("host:{}", lease.id));
    assert_eq!(session.host_xuid, Xuid(0));
    assert!(session.players.is_empty());
    assert_eq!(
        live.create_session(Some(&token), ad(), addr).await.unwrap(),
        session
    );
    let mut wrong = new_game();
    wrong.session_id = Some("8000000000000002".into());
    assert_eq!(
        live.start_game(Some(&token), wrong).await.unwrap_err().code,
        ErrorCode::Forbidden
    );
    let game = live.start_game(Some(&token), new_game()).await.unwrap();
    assert!(
        live.game(&game.game_id)
            .unwrap()
            .summary
            .players
            .iter()
            .all(|p| p.xuid != Some(Xuid(0)))
    );
    let operator = issue_token(
        &live.identity.key,
        &live.identity.author(),
        &live.identity.id,
        lxm::GAMES_WRITE,
        now(),
    );
    assert!(
        live.verify_host_token(Some(&operator), HOST_REPORT)
            .is_err()
    );
}

/// Old sessions have neither a lease nor a service identity. An ordinary
/// account still publishes, registers its round and signs the result.
#[tokio::test]
async fn peer_session_keeps_account_arbitration() {
    use crate::arbitration::Call;
    use live_proto::{arbitration::Register, sign::AccountRequest};

    let live = live().await;
    let server = &live.identity.id;
    let mut peers = Vec::new();
    for tag in ["One", "Two"] {
        let key = KeyPair::generate();
        let account = live
            .create_account(&AccountRequest::sign(&key, tag, None, server, now()))
            .await
            .unwrap();
        peers.push((key, account));
    }
    let (key, host) = &peers[0];
    let token = |scope| issue_token(key, &host.id, server, scope, now());
    let session = live
        .create_session(
            Some(&token(lxm::SESSIONS_WRITE)),
            ad(),
            Some(Ipv4Addr::LOCALHOST.into()),
        )
        .await
        .unwrap();
    assert!(session.matchmaking.is_none());
    for (key, account) in &peers {
        live.arbitrate(
            Some(&issue_token(
                key,
                &account.id,
                server,
                lxm::GAMES_WRITE,
                now(),
            )),
            &session.session_id,
            "0000000000000001",
            Call::Register(Register {
                duration: 300,
                flags: 12,
            }),
        )
        .await
        .unwrap();
    }
    let mut game = new_game();
    game.session_id = Some(session.session_id);
    game.kind = Some("matchmaking".into());
    game.playlist_id = Some("1".into());
    for (p, (_, account)) in game.players.iter_mut().zip(&peers) {
        p.xuid = Some(account.xuid);
    }
    let id = live
        .start_game(Some(&token(lxm::GAMES_WRITE)), game)
        .await
        .unwrap()
        .game_id;
    let (kind, data) = end();
    let finish = EventBody::at_unix(key, &host.id, &game::stream(&id), kind, data, now());
    live.append_game_events(
        Some(&token(lxm::GAMES_WRITE)),
        &id,
        &sign_batch(key, &[finish]),
    )
    .await
    .unwrap();
    for (_, account) in &peers {
        assert_eq!(live.model.read().ranks.standing(account.xuid, "1").games, 1);
    }
}
