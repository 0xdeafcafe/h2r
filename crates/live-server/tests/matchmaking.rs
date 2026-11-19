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

    assert_eq!(h.live.sweep_due_with_hosts(), None);
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
    assert_eq!(
        h.live.sweep_due_with_hosts(),
        Some((due + 999).div_euclid(1000))
    );
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
    let r = write(&h, &host, Method::POST, "/v1/sessions", advertisement.clone(), Some(&assignment.id)).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.error());
    assert_eq!(r.json()["session_id"], assignment.id);
    let replay = write(&h, &host, Method::POST, "/v1/sessions", advertisement, Some(&assignment.id)).await;
    assert_eq!(replay.json()["session_id"], assignment.id);
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
