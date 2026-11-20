mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use common::{Harness, Resp, User, start_driven};
use http_body_util::BodyExt;
use live_proto::{
    hosts::{HostLease, ProviderKind},
    lxm,
    matchmaking::{Assignment, Candidate, MATCH_HEADER},
};
use live_server::{
    ConnectInfo, Live, Policy, ServerIdentity,
    app::Timers,
    host_costs::HostCosts,
    hosts::{HostBoot, HostConfig, HostError, HostProvider},
    store::MemoryStore,
};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::Notify;
use tower::ServiceExt;

const HOPPER: u32 = 2; // Default Team Skirmish, two teams of two.

async fn write(
    h: &Harness,
    user: &User,
    method: Method,
    path: &str,
    payload: Value,
    assignment: Option<&str>,
) -> Resp {
    let (name, token) = h.bearer(user, lxm::SESSIONS_WRITE);
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .extension(ConnectInfo("127.0.0.1:34200".parse::<SocketAddr>().unwrap()))
        .header(name, token)
        .header("content-type", "application/json");
    if let Some(id) = assignment {
        req = req.header(MATCH_HEADER, id);
    }
    let resp = h.app.clone().oneshot(req.body(Body::from(payload.to_string())).unwrap())
        .await.unwrap();
    let (parts, body) = resp.into_parts();
    Resp {
        status: parts.status,
        headers: parts.headers,
        body: body.collect().await.unwrap().to_bytes().to_vec(),
    }
}

async fn ticket(h: &Harness, user: &User, port: u16, dedicated: bool, samples: Value) -> Resp {
    write(h, user, Method::PUT, "/v1/matchmaking/queue", json!({
        "hopper": HOPPER,
        "port": port,
        "dedicated": dedicated,
        "samples": samples,
    }), None).await
}

fn candidates(resp: Resp) -> Vec<Candidate> {
    assert_eq!(resp.status, StatusCode::OK, "{}", resp.error());
    let value = resp.json();
    assert_eq!(value["state"], "probing");
    serde_json::from_value(value["candidates"].clone()).unwrap()
}

#[derive(Default)]
struct GatedProvider {
    started: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl HostProvider for GatedProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Cloudflare
    }

    async fn start(&self, _lease: &HostLease, _boot: &HostBoot) -> Result<String, HostError> {
        self.started.notify_one();
        self.release.notified().await;
        Ok("first-sweep-fixture".into())
    }

    async fn stop(&self, _lease_id: &str, _provider_ref: &str) -> Result<(), HostError> {
        Ok(())
    }

    async fn alive(&self, _provider_ref: &str) -> Result<bool, HostError> {
        Ok(true)
    }
}

#[tokio::test]
async fn matchmaking_exposes_first_host_sweep_deadline_in_the_forming_response() {
    let dir = common::temp_dir();
    let identity = ServerIdentity::open(&dir, Some("live.test")).unwrap();
    let operator = ServerIdentity::load(&dir).unwrap().key;
    let id = identity.id.clone();
    let live = Live::start_with_host_costs(
        Arc::new(MemoryStore::new()),
        identity,
        Policy::default(),
        Timers::Driven,
        HostCosts::default(),
    )
    .await
    .unwrap();
    live.configure_dedicated_matchmaking(|name| match name {
        "LIVE_DEDICATED" => Some("1".into()),
        "LIVE_DEDICATED_HOPPERS" => Some(r#"{"2":{"map":"lockout"}}"#.into()),
        _ => None,
    })
    .unwrap();
    live.set_host_config(HostConfig {
        live_url: "https://live.test".into(),
        data_url: "https://private.test/bundle".into(),
        data_sha256: "ab".repeat(32),
        archive: "dual".into(),
        max_hosts: 4,
    });
    let provider = Arc::new(GatedProvider::default());
    live.set_host_provider(provider.clone());
    let h = Harness {
        app: live_server::http::router(live.clone()),
        live,
        operator,
        id,
        dir,
    };
    let players = [
        h.create("First").await,
        h.create("Second").await,
        h.create("Third").await,
        h.create("Fourth").await,
    ];
    let ports = [31000, 31010, 31020, 31030];
    for (player, port) in players.iter().zip(ports) {
        candidates(ticket(&h, player, port, false, json!([])).await);
    }

    let mut reports = Vec::new();
    for (player, port) in players.iter().zip(ports) {
        let targets = candidates(ticket(&h, player, port, false, json!([])).await);
        let samples: Vec<_> = targets
            .into_iter()
            .map(|target| json!({
                "target": target.xuid, "generation": target.generation, "rtt_ms": 40,
            }))
            .collect();
        reports.push(json!(samples));
    }
    let last_report = reports.pop().unwrap();
    for ((player, port), samples) in players.iter().zip(ports).zip(reports) {
        candidates(ticket(&h, player, port, false, samples).await);
    }

    assert_eq!(h.live.host_sweep_due_ms(), None);
    let forming = ticket(&h, &players[3], ports[3], false, last_report);
    tokio::pin!(forming);
    tokio::select! {
        biased;
        response = &mut forming => {
            panic!("queue replied before provider startup: {}", response.json());
        }
        _ = provider.started.notified() => {}
    }
    provider.release.notify_one();
    let response = forming.await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.error());
    let body = response.json();
    assert_eq!(body["state"], "waiting");
    let id = body["assignment"]["host_lease"].as_str().unwrap();
    let host = h.live.host_view(id).unwrap();
    let due = h.live.host_sweep_due_ms().expect("first host sweep deadline");
    assert!(due > host.lease.created_ms);
    assert!(due <= host.lease.created_ms + 60_000);
    assert!(h
        .live
        .sweep_due_with_hosts()
        .is_some_and(|at| at <= (due + 999).div_euclid(1000)));
}

#[tokio::test]
async fn matchmaking_reserves_dedicated_host_and_publishes_the_rtt_table() {
    let store = Arc::new(MemoryStore::new());
    let h = start_driven(store.clone(), Policy {
        admins: vec![live_proto::Gamertag::new("QueueAdmin").unwrap()],
        ..Default::default()
    }).await;
    let admin = h.create("QueueAdmin").await;
    let host = h.create("Dedicated").await;
    let first = h.create("First").await;
    let second = h.create("Second").await;
    let third = h.create("Third").await;
    let fourth = h.create("Fourth").await;
    let players = [&first, &second, &third, &fourth];
    let host_port = 31040;

    assert_eq!(ticket(&h, &host, host_port, true, json!([])).await.status, StatusCode::FORBIDDEN);
    let path = format!("/v1/matchmaking/dedicated/{}", host.xuid);
    assert_eq!(write(&h, &first, Method::PUT, &path, json!(null), None).await.status, StatusCode::FORBIDDEN);
    assert_eq!(write(&h, &admin, Method::PUT, &path, json!(null), None).await.status, StatusCode::NO_CONTENT);
    candidates(ticket(&h, &host, host_port, true, json!([])).await);

    for (i, user) in players.into_iter().enumerate() {
        candidates(ticket(&h, user, 31000 + i as u16 * 10, false, json!([])).await);
    }
    let mut assignment: Option<Assignment> = None;
    for (i, user) in players.into_iter().enumerate() {
        let port = 31000 + i as u16 * 10;
        let targets = candidates(ticket(&h, user, port, false, json!([])).await);
        assert_eq!(targets.len(), 4);
        let samples = targets.iter().map(|target| json!({
            "target": target.xuid,
            "generation": target.generation,
            "rtt_ms": if target.xuid == host.xuid { 14 + i } else { 55 },
        })).collect::<Vec<_>>();
        let r = ticket(&h, user, port, false, json!(samples)).await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.error());
        if i == 3 {
            assert_eq!(r.json()["state"], "waiting");
            assignment = Some(serde_json::from_value(r.json()["assignment"].clone()).unwrap());
        } else {
            assert_eq!(r.json()["state"], "probing");
        }
    }
    let assignment = assignment.expect("four complete sets of samples form a match");
    assert!(assignment.dedicated);
    assert_eq!(assignment.host, host.xuid);
    assert_eq!(assignment.members.len(), 4);
    assert_eq!(assignment.teams.values().filter(|&&t| t == 0).count(), 2);
    assert_eq!(assignment.teams.values().filter(|&&t| t == 1).count(), 2);
    for (i, user) in players.into_iter().enumerate() {
        assert_eq!(assignment.rtt_ms[&user.xuid], 14 + i as u16);
    }
    let r = ticket(&h, &host, host_port, true, json!([])).await;
    assert_eq!(r.json()["state"], "host");
    assert_eq!(r.json()["assignment"]["id"], assignment.id);

    let advertisement = json!({
        "port": host_port,
        "host_address": vec![0; 36],
        "slots": {"public_filled":0,"public_open":4,"private_filled":0,"private_open":0},
        "attributes": {"1":{"type":"integer","value":HOPPER}},
    });
    let r = write(&h, &first, Method::POST, "/v1/sessions", advertisement.clone(), Some(&assignment.id)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let mut pack_ad = advertisement.clone();
    pack_ad["map"] = json!(r"maps\Turf.MAP");
    let r = write(&h, &host, Method::POST, "/v1/sessions", pack_ad, Some(&assignment.id)).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "the roster has no Pack capability");
    let r = write(&h, &host, Method::POST, "/v1/sessions", advertisement.clone(), Some(&assignment.id)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    assert_eq!(r.json()["session_id"], assignment.id);
    let replay = write(&h, &host, Method::POST, "/v1/sessions", advertisement, Some(&assignment.id)).await;
    assert_eq!(replay.json()["session_id"], assignment.id);
    let event = live_proto::sign::EventBody::at_unix(
        &host.key,
        &host.author,
        &live_proto::session::stream(&assignment.id),
        live_proto::session::UPDATED,
        json!({"map": "Sanctuary"}),
        live_server::app::now(),
    );
    let update = h.call(
        Method::POST,
        &format!("/v1/sessions/{}/events", assignment.id),
        &[h.bearer(&host, lxm::SESSIONS_WRITE)],
        live_proto::sign::sign_batch(&host.key, &[event]),
    ).await;
    assert_eq!(update.status, StatusCode::FORBIDDEN, "an update cannot add a Pack map to this roster");
    let r = ticket(&h, &first, 31000, false, json!([])).await;
    // dh-lease: the registered host leaves the legacy frame source in place.
    assert_eq!(
        r.json(),
        json!({"state":"join", "session_id":assignment.id, "frames_from_host":false})
    );

    let detail = format!("/v1/sessions/{}", assignment.id);
    let read = h.get(&detail).await.json();
    assert_eq!(read["session"]["matchmaking"], json!(assignment));
    assert!(read["session"].get("endpoint").is_none());
    let read = h.call(Method::GET, &detail, &[h.bearer(&first, lxm::READ)], Body::empty()).await.json();
    assert_eq!(read["session"]["endpoint"]["port"], host_port);
    let read = h.call(Method::GET, &detail, &[h.bearer(&admin, lxm::READ)], Body::empty()).await.json();
    assert!(read["session"].get("endpoint").is_none());
    let stream = format!("/v1/events?stream=session-{}", assignment.id);
    let events = h.get(&stream).await.json();
    let events = events["items"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["data"]["matchmaking"], json!(assignment));
}

#[tokio::test]
async fn matchmaking_rejects_invalid_rtt_and_ignores_stale_generations() {
    let h = common::harness().await;
    let user = h.create("Client").await;
    let target = h.create("Target").await;
    candidates(ticket(&h, &target, 31010, false, json!([])).await);
    let old = candidates(ticket(&h, &user, 31000, false, json!([])).await).remove(0);
    candidates(ticket(&h, &target, 31020, false, json!([])).await);
    let invalid = ticket(&h, &user, 31000, false, json!([{
        "target": target.xuid, "generation": old.generation, "rtt_ms": 0,
    }])).await;
    assert_eq!(invalid.status, StatusCode::BAD_REQUEST);
    let fresh = candidates(ticket(&h, &user, 31000, false, json!([{
        "target": target.xuid, "generation": old.generation, "rtt_ms": 55,
    }])).await).remove(0);
    assert_ne!(old.generation, fresh.generation);
    assert_eq!(fresh.endpoint.port(), 31020);
}

const AVAILABILITY: &str = "/v1/matchmaking/availability";

async fn availability(h: &Harness, user: &User) -> Value {
    let response = h.call(Method::GET, AVAILABILITY, &[h.bearer(user, lxm::READ)], Body::empty()).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers["cache-control"], "private, no-store");
    response.json()["has_map_pack"].clone()
}

async fn opti_search(h: &Harness, user: &User, query: Value) -> Vec<Value> {
    let query = query.to_string();
    let query = percent_encoding::utf8_percent_encode(&query, percent_encoding::NON_ALPHANUMERIC);
    let response = h.call(
        Method::GET,
        &format!("/v1/sessions?optimatch={query}"),
        &[h.bearer(user, lxm::READ)],
        Body::empty(),
    ).await;
    assert_eq!(response.status, StatusCode::OK, "{}", response.error());
    response.json()["items"].as_array().unwrap().clone()
}

#[tokio::test]
async fn optimatch_filters_pack_maps_and_reports_capability_only_to_its_account() {
    let h = common::harness().await;
    let host = h.create("Match Host").await;
    let guest = h.create("Match Guest").await;
    assert_eq!(h.get(AVAILABILITY).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(availability(&h, &guest).await, Value::Null);
    let old = ticket(&h, &guest, 31000, false, json!([])).await;
    assert_eq!(old.status, StatusCode::OK);
    assert_eq!(availability(&h, &guest).await, Value::Null);
    for map in ["lockout", r"T:\$c\example\dune.map"] {
        let ad = write(&h, &host, Method::POST, "/v1/sessions", json!({
            "port": 31010,
            "host_address": vec![0; 36],
            "slots": {"public_filled":1,"public_open":3,"private_filled":0,"private_open":0},
            "attributes": {
                "1": {"type":"integer","value":HOPPER},
                "8": {"type":"integer","value":7},
            },
            "map": map,
        }), None).await;
        assert_eq!(ad.status, StatusCode::CREATED, "{}", ad.error());
    }

    let query = |has_map_pack, party| json!({
        "hopper": HOPPER, "average": 0, "has_map_pack": has_map_pack, "party": party,
    });
    let items = opti_search(&h, &guest, query(false, 1)).await;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["advertisement"]["map"], "lockout");
    assert_eq!(availability(&h, &guest).await, false);
    assert_eq!(opti_search(&h, &guest, query(true, 1)).await.len(), 2);
    assert_eq!(opti_search(&h, &guest, query(true, 2)).await.len(), 1);
    assert_eq!(availability(&h, &guest).await, true, "the searching console still owns the Pack");
    assert_eq!(availability(&h, &host).await, Value::Null);

    opti_search(&h, &guest, json!({"hopper": -1, "average": 0, "has_map_pack": false})).await;
    assert_eq!(availability(&h, &guest).await, true, "a custom search is not a report");

    let reply = write(&h, &guest, Method::PUT, "/v1/matchmaking/queue", json!({
        "hopper": HOPPER,
        "port": 31000,
        "has_map_pack": true,
    }), None).await;
    assert!(candidates(reply).is_empty());
    assert_eq!(availability(&h, &guest).await, true);
}

const COHORT_PORTS: [u16; 4] = [31000, 31010, 31020, 31030];

async fn pack_ticket(h: &Harness, player: &User, port: u16, has_map_pack: bool, samples: Value) -> Resp {
    write(h, player, Method::PUT, "/v1/matchmaking/queue", json!({
        "hopper": HOPPER, "port": port, "has_map_pack": has_map_pack, "samples": samples,
    }), None).await
}

async fn pack_cohort(h: &Harness) -> ([User; 4], Assignment) {
    let players = [
        h.create("Pack First").await,
        h.create("Pack Second").await,
        h.create("Pack Third").await,
        h.create("Pack Fourth").await,
    ];
    for (player, port) in players.iter().zip(COHORT_PORTS) {
        candidates(pack_ticket(h, player, port, true, json!([])).await);
    }
    let mut assignment = None;
    for (i, (player, port)) in players.iter().zip(COHORT_PORTS).enumerate() {
        let targets = candidates(pack_ticket(h, player, port, true, json!([])).await);
        let samples = targets.iter().map(|target| json!({
            "target": target.xuid, "generation": target.generation, "rtt_ms": 40,
        })).collect::<Vec<_>>();
        let result = pack_ticket(h, player, port, true, json!(samples)).await;
        assert_eq!(result.status, StatusCode::OK, "{}", result.error());
        if i == players.len() - 1 {
            assignment = Some(serde_json::from_value::<Assignment>(result.json()["assignment"].clone()).unwrap());
        } else {
            assert_eq!(result.json()["state"], "probing");
        }
    }
    let assignment = assignment.expect("all four players formed an assignment");
    assert!(assignment.has_map_pack);
    (players, assignment)
}

#[tokio::test]
async fn losing_pack_capability_invalidates_the_old_assignment() {
    let h = common::harness().await;
    let (players, assignment) = pack_cohort(&h).await;
    let changed = pack_ticket(&h, &players[3], COHORT_PORTS[3], false, json!([])).await;
    assert_eq!(changed.status, StatusCode::FORBIDDEN);
    assert_eq!(availability(&h, &players[3]).await, false);
    let host = players.iter().position(|player| player.xuid == assignment.host).unwrap();
    let next = pack_ticket(&h, &players[host], COHORT_PORTS[host], true, json!([])).await;
    assert!(candidates(next).is_empty(), "the former host cannot use the old lease either");
}

#[tokio::test]
async fn pack_assignment_keeps_join_addresses_and_invites_within_its_roster() {
    let h = common::harness().await;
    let (players, assignment) = pack_cohort(&h).await;
    let host_index = players.iter().position(|player| player.xuid == assignment.host).unwrap();
    let host = &players[host_index];
    let member = players.iter().find(|player| player.xuid != assignment.host).unwrap();
    let outsider = h.create("Unassigned").await;
    assert_eq!(h.sign_in(host).await.status, StatusCode::CREATED);
    let response = write(
        &h, host, Method::POST, "/v1/sessions", json!({
            "port": COHORT_PORTS[host_index],
            "host_address": vec![0; 36],
            "slots": {"public_filled":1, "public_open":7, "private_filled":0, "private_open":0},
            "attributes": {"1":{"type":"integer","value":HOPPER}},
            "map": "turf",
        }), Some(&assignment.id),
    ).await;
    assert_eq!(response.status, StatusCode::CREATED, "{}", response.error());

    let path = format!("/v1/sessions/{}", assignment.id);
    let detail = h.call(Method::GET, &path, &[h.bearer(&outsider, lxm::READ)], Body::empty()).await;
    assert_eq!(detail.status, StatusCode::OK);
    let public = &detail.json()["session"];
    assert_eq!(public["advertisement"]["map"], "turf");
    assert!(public.get("endpoint").is_none());
    assert!(public["advertisement"].get("host_address").is_none());
    let member_detail = h.call(Method::GET, &path, &[h.bearer(member, lxm::READ)], Body::empty()).await;
    assert_eq!(member_detail.json()["session"]["endpoint"]["port"], COHORT_PORTS[host_index]);
    assert_eq!(h.get("/v1/sessions").await.json()["items"].as_array().unwrap().len(), 1);
    let listed = h.call(Method::GET, "/v1/sessions", &[h.bearer(&outsider, lxm::READ)], Body::empty()).await;
    assert_eq!(listed.json()["items"], json!([]));

    use live_proto::{mailbox, session, sign};
    for (guest, expected) in [(member, StatusCode::CREATED), (&outsider, StatusCode::FORBIDDEN)] {
        let invite = h.event(
            host, &mailbox::stream(guest.xuid), mailbox::SENT,
            json!({"from": host.xuid, "session_id": assignment.id}),
        );
        let sent = h.post_event(host, invite).await;
        assert_eq!(sent.status, expected);
        if expected == StatusCode::CREATED {
            let answer = h.event(
                guest, &mailbox::stream(guest.xuid), mailbox::ANSWERED, json!({
                    "by": guest.xuid, "session_id": assignment.id,
                    "invite_id": sent.json()["position"], "answer": "accept",
                }),
            );
            assert_eq!(h.post_event(guest, answer).await.status, StatusCode::CREATED);
        }

        let join = sign::EventBody::at_unix(
            &host.key, &host.author, &session::stream(&assignment.id), session::JOINED,
            json!({"xuid": guest.xuid}), live_server::app::now(),
        );
        let response = h.call(
            Method::POST,
            &format!("{path}/events"),
            &[h.bearer(host, lxm::SESSIONS_WRITE)],
            sign::sign_batch(&host.key, &[join]),
        ).await;
        assert_eq!(response.status, expected);
    }
}
