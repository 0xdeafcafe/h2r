use super::*;
use crate::app::Timers;
use crate::store::{MemoryStore, Query};
use crate::{Policy, ServerIdentity};
use live_proto::hosts::{HostClass, Transport};
use live_proto::sign::{AccountRequest, KeyPair};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::Notify;

struct Fake {
    kind: ProviderKind,
    starts: AtomicUsize,
    running: RwLock<BTreeSet<String>>,
    fail_start: AtomicBool,
    fail_stop: AtomicBool,
    pause: AtomicBool,
    entered: Notify,
    resume: Notify,
}

impl Fake {
    fn new(kind: ProviderKind) -> Self {
        Self {
            kind,
            starts: Default::default(),
            running: Default::default(),
            fail_start: Default::default(),
            fail_stop: Default::default(),
            pause: Default::default(),
            entered: Notify::new(),
            resume: Notify::new(),
        }
    }
}

#[async_trait]
impl HostProvider for Fake {
    fn kind(&self) -> ProviderKind {
        self.kind.clone()
    }
    async fn start(&self, lease: &HostLease, boot: &HostBoot) -> Result<String, HostError> {
        self.starts.fetch_add(1, Ordering::Relaxed);
        self.entered.notify_one();
        if self.pause.load(Ordering::Relaxed) {
            self.resume.notified().await;
        }
        if self.fail_start.load(Ordering::Relaxed) {
            return Err(HostError::new("launch failed"));
        }
        if self.kind == ProviderKind::Fly {
            assert_eq!(lease.transport, Transport::Udp);
        }
        assert!(!boot.token.is_empty());
        assert_eq!(boot.live_url, "https://live.test");
        assert_eq!(
            boot.data_url,
            "https://private.test/bundle?signature=secret"
        );
        assert_eq!(boot.data_sha256, "ab".repeat(32));
        let id = format!("machine-{}", lease.id);
        self.running.write().unwrap().insert(id.clone());
        Ok(id)
    }
    async fn stop(&self, lease_id: &str, id: &str) -> Result<(), HostError> {
        assert_eq!(id, format!("machine-{lease_id}"));
        if self.fail_stop.load(Ordering::Relaxed) {
            return Err(HostError::new("stop failed"));
        }
        self.running.write().unwrap().remove(id);
        Ok(())
    }
    async fn alive(&self, id: &str) -> Result<bool, HostError> {
        Ok(self.running.read().unwrap().contains(id))
    }
}

fn config(max_hosts: usize) -> HostConfig {
    HostConfig {
        max_hosts,
        live_url: "https://live.test".into(),
        data_url: "https://private.test/bundle?signature=secret".into(),
        data_sha256: "ab".repeat(32),
        archive: "dual".into(),
    }
}

async fn start(store: Arc<MemoryStore>, key: &str, provider: Arc<Fake>) -> Arc<Live> {
    let live = Live::start_with_host_costs(
        store,
        ServerIdentity::from_secret("live.test", key).unwrap(),
        Policy {
            create_rate: (100.0, 10),
            ..Policy::default()
        },
        Timers::Driven,
        HostCosts::default(),
    )
    .await
    .unwrap();
    live.set_host_provider(provider);
    live.set_host_config(config(4));
    live
}

async fn setup() -> (Arc<Live>, Arc<Fake>) {
    let fake = Arc::new(Fake::new(ProviderKind::Local));
    let live = start(
        Arc::new(MemoryStore::new()),
        &KeyPair::generate().secret_b64(),
        fake.clone(),
    )
    .await;
    (live, fake)
}

fn request(name: &str) -> HostLease {
    HostLease {
        id: String::new(),
        assignment_id: format!("custom-{name}"),
        provider: ProviderKind::Local,
        region: None,
        class: HostClass::Small,
        map: "lockout".into(),
        map_id: None,
        variant: None,
        hopper: None,
        max_players: 4,
        roster: vec![],
        bots: vec![],
        transport: Transport::Udp,
        idle_timeout_s: 120,
        max_match_s: 1800,
        created_ms: 0,
    }
}

fn endpoint() -> HostEndpoint {
    HostEndpoint {
        transport: Transport::Udp,
        udp: Some("127.0.0.1:31000".into()),
        tunnel_url: None,
        xnaddr: "00".repeat(36),
        session_id: Some("8111111111111111".into()),
    }
}

async fn register(live: &Live, lease: &HostLease) -> String {
    let token = live.issue_host_token(lease);
    assert_eq!(
        live.host_register(Some(&token), &lease.id, endpoint())
            .await
            .unwrap(),
        *lease
    );
    token
}

fn operator(live: &Live, scope: &str) -> String {
    sign::issue_token(
        &live.identity.key,
        &live.identity.author(),
        live.id(),
        scope,
        crate::app::now(),
    )
}

#[tokio::test]
async fn fly_requests_normalize_tunnel_before_recording_and_retrying() {
    let fake = Arc::new(Fake::new(ProviderKind::Fly));
    let live = start(
        Arc::new(MemoryStore::new()),
        &KeyPair::generate().secret_b64(),
        fake.clone(),
    )
    .await;
    let mut input = request("fly");
    input.provider = ProviderKind::Fly;
    input.transport = Transport::Tunnel;

    let lease = live.request_host(input.clone()).await.unwrap();
    assert_eq!(lease.transport, Transport::Udp);
    assert_eq!(live.require_host(&lease.id).unwrap().lease, lease);
    assert_eq!(live.request_host(input).await.unwrap(), lease);
    assert_eq!(fake.starts.load(Ordering::Relaxed), 1);
    register(&live, &lease).await;
}

#[tokio::test]
async fn lifecycle_replays_and_keeps_boot_credentials_out_of_the_log() {
    let store = Arc::new(MemoryStore::new());
    let key = KeyPair::generate().secret_b64();
    let fake = Arc::new(Fake::new(ProviderKind::Local));
    let live = start(store.clone(), &key, fake.clone()).await;
    let lease = live.request_host(request("round")).await.unwrap();
    assert!(lease.id.starts_with("hl-"));
    assert_eq!(
        live.require_host(&lease.id).unwrap().state,
        HostState::Booting
    );
    assert_eq!(live.request_host(request("round")).await.unwrap(), lease);
    assert_eq!(fake.starts.load(Ordering::Relaxed), 1);
    let token = register(&live, &lease).await;
    assert_eq!(
        live.require_host(&lease.id).unwrap().state,
        HostState::Joinable
    );
    assert_eq!(
        live.expire_host_start(&lease.id, lease.created_ms + BOOT_MS)
            .await
            .unwrap(),
        lease
    );
    // Retransmitted registration is harmless. A conflicting one cannot repoint clients.
    register(&live, &lease).await;
    let mut changed = endpoint();
    changed.udp = Some("127.0.0.1:32000".into());
    assert!(
        live.host_register(Some(&token), &lease.id, changed)
            .await
            .is_err()
    );
    for n in [4, 2] {
        let row = live
            .host_report(Some(&token), &lease.id, HostState::Playing, n, None)
            .await
            .unwrap();
        assert_eq!(row.state, HostState::Playing);
        assert_eq!(row.players_peak, 4);
    }
    let prior = live.require_host(&lease.id).unwrap();
    drop(live);
    let live = start(store.clone(), &key, fake.clone()).await;
    assert_eq!(live.host_for_assignment(&lease.assignment_id), Some(prior));
    // Let one millisecond elapse on the server clock for this microsecond sample.
    tokio::time::sleep(Duration::from_millis(2)).await;
    let usage = live_proto::hosts::PackUsage {
        slot_us: 1_000, container_share_us: 500, charge_start: false,
        cpu_us: Some(100), peak_pss_bytes: Some(1024),
    };
    let row = live
        .host_report_with_usage(
            Some(&token),
            &lease.id,
            HostState::Ended,
            1,
            Some(EndReason::MatchOver),
            Some(usage),
        )
        .await
        .unwrap();
    assert_eq!(row.state, HostState::Ended);
    assert_eq!(row.players_peak, 4);
    assert_eq!(row.reason, Some(EndReason::MatchOver));
    // The final response reaches the supervisor before forced cleanup.
    assert_eq!(fake.running.read().unwrap().len(), 1);
    live.set_host_config(config(1));
    assert_eq!(
        live.request_host(request("still-exiting"))
            .await
            .unwrap_err()
            .code,
        ErrorCode::RateLimited
    );
    live.host_report(Some(&token), &lease.id, HostState::Ended, 0, None)
        .await
        .unwrap();
    assert!(
        live.host_report(Some(&token), &lease.id, HostState::Playing, 1, None)
            .await
            .is_err()
    );
    assert!(live.request_host(request("round")).await.is_err());
    let events = live
        .store
        .read(&Query {
            stream: Some(STREAM.into()),
            ..Query::after(0, 100)
        })
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| e.body.kind.as_str())
            .collect::<Vec<_>>(),
        [
            HOST_REQUESTED,
            HOST_BOOTING,
            HOST_READY,
            HOST_PLAYING,
            HOST_PLAYING,
            HOST_ENDED
        ]
    );
    assert_eq!(events.last().unwrap().body.data["pack_usage"], json!(usage));
    for e in events {
        assert_eq!(e.body.author, live.identity.author());
        assert!(!e.body.data.to_string().contains(&token));
        assert!(!e.body.data.to_string().contains("signature=secret"));
    }
    assert_eq!(
        live.host_sweep_due_ms(),
        Some(row.updated_ms + EXIT_GRACE_MS)
    );
    let admin = operator(&live, crate::invites::LXM_ADMIN_READ);
    let inventory = live.admin_hosts(Some(&admin)).unwrap();
    assert_eq!(inventory["recent"][0]["lease"]["id"], lease.id);
    assert_eq!(inventory["recent"][0]["cost_usd"], 0.0);
    assert_eq!(inventory["month_to_date_usd"], 0.0);
    assert!(inventory["budget_usd"].is_null());
    assert!(inventory["running"].as_array().unwrap().is_empty());
    drop(live);
    let restarted = start(store.clone(), &key, fake.clone()).await;
    assert_eq!(restarted.require_host(&lease.id).unwrap(), row);
    restarted
        .sweep_hosts_at(row.updated_ms + EXIT_GRACE_MS)
        .await;
    assert!(fake.running.read().unwrap().is_empty());
    assert_eq!(restarted.host_sweep_due_ms(), None);
    drop(restarted);
    assert_eq!(start(store, &key, fake).await.host_sweep_due_ms(), None);
}

#[tokio::test]
async fn impossible_usage_ends_the_lease_without_signing_the_discount() {
    let (live, _) = setup().await;
    let lease = live.request_host(request("bad-usage")).await.unwrap();
    let token = register(&live, &lease).await;
    let usage = live_proto::hosts::PackUsage {
        slot_us: u64::MAX,
        container_share_us: u64::MAX / 2,
        charge_start: false,
        cpu_us: None,
        peak_pss_bytes: None,
    };
    let row = live.host_report_with_usage(Some(&token), &lease.id, HostState::Ended, 2, None, Some(usage))
        .await.unwrap();
    assert_eq!(row.state, HostState::Ended);
    let events = live.store.read(&Query {
        stream: Some(STREAM.into()),
        ..Query::after(0, 100)
    }).await.unwrap();
    let ended = events.iter().find(|e| e.body.kind == HOST_ENDED).unwrap();
    assert!(ended.body.data.get("pack_usage").is_none());
}

#[tokio::test]
async fn pending_allocations_reserve_capacity_and_late_starts_are_stopped() {
    let (live, fake) = setup().await;
    live.set_host_config(config(1));
    fake.pause.store(true, Ordering::Relaxed);
    let requester = live.clone();
    let started = tokio::spawn(async move { requester.request_host(request("pending")).await });
    fake.entered.notified().await;
    let row = live.host_for_assignment("custom-pending").unwrap();
    assert_eq!(row.state, HostState::Requested);
    let full = live.request_host(request("blocked")).await.unwrap_err();
    assert_eq!(full.code, ErrorCode::RateLimited);
    assert!(live.host_for_assignment("custom-blocked").is_none());
    let due = row.lease.created_ms + BOOT_MS;
    assert_eq!(live.host_sweep_due_ms(), Some(due));
    live.sweep_hosts_at(due - 1).await;
    assert_eq!(
        live.require_host(&row.lease.id).unwrap().state,
        HostState::Requested
    );
    live.sweep_hosts_at(due).await;
    assert_eq!(
        live.require_host(&row.lease.id).unwrap().reason,
        Some(EndReason::BootTimeout)
    );
    assert_eq!(
        live.request_host(request("still-allocating"))
            .await
            .unwrap_err()
            .code,
        ErrorCode::RateLimited
    );
    fake.fail_stop.store(true, Ordering::Relaxed);
    fake.resume.notify_one();
    assert!(started.await.unwrap().is_err());
    assert_eq!(fake.running.read().unwrap().len(), 1);
    fake.fail_stop.store(false, Ordering::Relaxed);
    live.sweep_hosts_at(due + 1).await;
    assert!(fake.running.read().unwrap().is_empty());
    fake.pause.store(false, Ordering::Relaxed);
    assert!(live.request_host(request("next")).await.is_ok());
}

#[tokio::test]
async fn reaper_honors_boot_idle_and_hard_deadlines_after_replay() {
    let store = Arc::new(MemoryStore::new());
    let key = KeyPair::generate().secret_b64();
    let fake = Arc::new(Fake::new(ProviderKind::Local));
    let live = start(store.clone(), &key, fake.clone()).await;
    let boot = live.request_host(request("boot")).await.unwrap();
    let mut short = request("hard");
    short.max_match_s = 90;
    let hard = live.request_host(short).await.unwrap();
    let idle = live.request_host(request("idle")).await.unwrap();
    register(&live, &hard).await;
    let token = register(&live, &idle).await;
    let since = live.require_host(&idle.id).unwrap().idle_since_ms;
    live.host_report(Some(&token), &idle.id, HostState::Playing, 0, None)
        .await
        .unwrap();
    assert_eq!(live.require_host(&idle.id).unwrap().idle_since_ms, since);
    drop(live);
    let live = start(store, &key, fake.clone()).await;
    live.sweep_hosts_at(boot.created_ms + BOOT_MS).await;
    let row = live.require_host(&boot.id).unwrap();
    assert_eq!(
        (row.state, row.reason),
        (HostState::Failed, Some(EndReason::BootTimeout))
    );
    live.sweep_hosts_at(hard.created_ms + 90_000).await;
    assert_eq!(
        live.require_host(&hard.id).unwrap().reason,
        Some(EndReason::MaxDuration)
    );
    live.sweep_hosts_at(since + 120_000).await;
    assert_eq!(
        live.require_host(&idle.id).unwrap().reason,
        Some(EndReason::Idle)
    );
    assert!(fake.running.read().unwrap().is_empty());
}

#[tokio::test]
async fn occupied_host_needs_no_heartbeat_between_playing_and_the_hard_limit() {
    let (live, _) = setup().await;
    let lease = live.request_host(request("occupied")).await.unwrap();
    let token = register(&live, &lease).await;
    live.host_report(Some(&token), &lease.id, HostState::Playing, 2, None)
        .await
        .unwrap();
    live.sweep_hosts_at(lease.created_ms + 121_000).await;
    assert_eq!(
        live.require_host(&lease.id).unwrap().state,
        HostState::Playing
    );
    live.sweep_hosts_at(lease.created_ms + 1_800_000).await;
    assert_eq!(
        live.require_host(&lease.id).unwrap().reason,
        Some(EndReason::MaxDuration)
    );
}

#[tokio::test]
async fn start_failure_crash_and_retriable_admin_stop() {
    let (live, fake) = setup().await;
    fake.fail_start.store(true, Ordering::Relaxed);
    assert!(live.request_host(request("failure")).await.is_err());
    assert_eq!(
        live.host_for_assignment("custom-failure").unwrap().reason,
        Some(EndReason::Crashed)
    );
    fake.fail_start.store(false, Ordering::Relaxed);
    let crash = live.request_host(request("crash")).await.unwrap();
    fake.running.write().unwrap().clear();
    live.sweep_hosts().await;
    assert_eq!(
        live.require_host(&crash.id).unwrap().state,
        HostState::Failed
    );

    let stop = live.request_host(request("stop")).await.unwrap();
    let admin = || operator(&live, crate::admin::LXM_ADMIN_WRITE);
    fake.fail_stop.store(true, Ordering::Relaxed);
    assert!(
        live.admin_stop_host(Some(&admin()), &stop.id)
            .await
            .is_err()
    );
    assert!(live.require_host(&stop.id).unwrap().active());
    fake.fail_stop.store(false, Ordering::Relaxed);
    let row = live
        .admin_stop_host(Some(&admin()), &stop.id)
        .await
        .unwrap();
    assert_eq!(
        (row.state, row.reason),
        (HostState::Ended, Some(EndReason::Stopped))
    );
    assert!(live.admin_stop_host(Some(&admin()), &stop.id).await.is_ok());
    assert!(fake.running.read().unwrap().is_empty());
}

#[tokio::test]
async fn only_lease_identity_may_register_or_report_and_only_roster_or_admin_may_read() {
    let (live, _) = setup().await;
    let account = KeyPair::generate();
    let player = live
        .create_account(&AccountRequest::sign(
            &account,
            "Roster",
            None,
            live.id(),
            crate::app::now(),
        ))
        .await
        .unwrap();
    let read = || {
        sign::issue_token(
            &account,
            &player.id,
            live.id(),
            lxm::READ,
            crate::app::now(),
        )
    };
    let mut input = request("private");
    input.roster.push(player.xuid);
    let lease = live.request_host(input).await.unwrap();
    let other = live.request_host(request("other")).await.unwrap();
    let wrong = live.issue_host_token(&other);
    assert_eq!(
        live.host_register(Some(&wrong), &lease.id, endpoint())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    assert!(
        live.host_report(Some(&wrong), &lease.id, HostState::Ended, 0, None)
            .await
            .is_err()
    );
    let own = live.issue_host_token(&lease);
    assert!(live.host_get(Some(&own), &lease.id).is_err());
    assert!(
        live.host_report(Some(&own), &lease.id, HostState::Playing, 1, None)
            .await
            .is_err()
    );
    assert_eq!(
        live.host_get(Some(&read()), &lease.id).unwrap().lease,
        lease
    );
    assert_eq!(
        live.host_get(Some(&read()), &other.id).unwrap_err().code,
        ErrorCode::Forbidden
    );
    assert!(live.host_get(None, &lease.id).is_err());
    assert!(
        live.host_get(Some(&operator(&live, lxm::READ)), &other.id)
            .is_ok()
    );
}

#[test]
fn max_hosts_defaults_to_four_and_zero_disables_launches() {
    assert_eq!(HostConfig::from_env(&|_| None).max_hosts, 4);
    assert_eq!(HostConfig::from_env(&|_| None).archive, "dual");
    assert_eq!(HostConfig::from_env(&|k| (k == "LIVE_HOST_ARCHIVE").then(|| "film".into())).archive, "film");
    assert_eq!(
        HostConfig::from_env(&|k| (k == "LIVE_MAX_HOSTS").then(|| "0".into())).max_hosts,
        0
    );
}

#[test]
fn max_hosts_accepts_operator_selected_steps_and_fails_closed() {
    let cap = |value: &str| {
        HostConfig::from_env(&|key| (key == "LIVE_MAX_HOSTS").then(|| value.to_owned())).max_hosts
    };
    for n in [0, 1, 2, 3, 4, 16, 64, 256, 1024] {
        assert_eq!(cap(&n.to_string()), n);
    }
    for invalid in ["5", "65", "1025", "-1", "", "auto"] {
        assert_eq!(cap(invalid), 0, "{invalid}");
    }
}

#[tokio::test]
async fn monthly_budget_refuses_before_reservation_or_provider_call() {
    let live = Live::start_with_host_costs(
        Arc::new(MemoryStore::new()),
        ServerIdentity {
            id: "live.test".into(),
            key: KeyPair::generate(),
        },
        Policy::default(),
        Timers::Driven,
        HostCosts::configured(None, Some("0.01")).unwrap(),
    )
    .await
    .unwrap();
    let fake = Arc::new(Fake::new(ProviderKind::Cloudflare));
    live.set_host_provider(fake.clone());
    live.set_host_config(config(1));
    let mut lease = request("budget");
    lease.provider = ProviderKind::Cloudflare;
    lease.transport = Transport::Tunnel;
    let refused = live.request_host(lease).await.unwrap_err();
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert_eq!(live.position(), 0);
    assert!(live.host_for_assignment("custom-budget").is_none());
    assert_eq!(fake.starts.load(Ordering::Relaxed), 0);
    let inventory = live
        .admin_hosts(Some(&operator(&live, crate::invites::LXM_ADMIN_READ)))
        .unwrap();
    assert_eq!(inventory["month_to_date_usd"], 0.0);
    assert_eq!(inventory["budget_usd"], 0.01);
}

#[tokio::test]
async fn reaper_recovers_the_reserved_room_after_a_lost_start_reply() {
    let store = Arc::new(MemoryStore::new());
    let key = KeyPair::generate().secret_b64();
    let fake = Arc::new(Fake::new(ProviderKind::Cloudflare));
    let live = start(store.clone(), &key, fake.clone()).await;
    let mut lease = request("lost-reply");
    lease.id = HostLease::new_id();
    lease.provider = ProviderKind::Cloudflare;
    lease.transport = Transport::Tunnel;
    lease.max_match_s = 1;
    lease.created_ms = now_ms() - 2_000;
    let reference = format!("machine-{}", lease.id);
    live.host_event(
        0,
        HOST_REQUESTED,
        HostRequested {
            lease: lease.clone(),
            provider_ref: Some(reference.clone()),
            pack_linger: false,
        },
    )
    .await
    .unwrap();
    drop(live);

    let live = start(store.clone(), &key, fake.clone()).await;
    let deadline = lease.deadline_ms();
    live.sweep_hosts_at(deadline - 1).await;
    assert_eq!(
        live.require_host(&lease.id).unwrap().state,
        HostState::Requested,
    );
    assert_eq!(live.hosts.costs.stop_confirmed(&lease.id), Some(false));

    fake.running.write().unwrap().insert(reference.clone());
    fake.fail_stop.store(true, Ordering::Relaxed);
    live.sweep_hosts_at(deadline).await;
    assert!(fake.running.read().unwrap().contains(&reference));
    assert_eq!(live.hosts.costs.stop_confirmed(&lease.id), Some(false));

    fake.fail_stop.store(false, Ordering::Relaxed);
    live.sweep_hosts_at(deadline + 1).await;
    assert!(!fake.running.read().unwrap().contains(&reference));
    assert_eq!(
        live.require_host(&lease.id).unwrap().reason,
        Some(EndReason::MaxDuration),
    );
    assert_eq!(live.hosts.costs.stop_confirmed(&lease.id), Some(true));
    let settled = live.hosts.costs.lease_cost(&lease.id, now_ms()).unwrap();
    drop(live);

    let live = start(store, &key, fake).await;
    assert_eq!(live.hosts.costs.stop_confirmed(&lease.id), Some(true));
    assert_eq!(live.hosts.costs.lease_cost(&lease.id, now_ms()), Some(settled));
}

#[tokio::test]
async fn early_registration_can_finish_after_the_requesting_client_disconnects() {
    let (live, fake) = setup().await;
    fake.pause.store(true, Ordering::Relaxed);
    let caller = live.clone();
    let allocating = tokio::spawn(async move { caller.request_host(request("fast")).await });
    fake.entered.notified().await;
    let lease = live.host_for_assignment("custom-fast").unwrap().lease;
    allocating.abort();
    assert!(allocating.await.unwrap_err().is_cancelled());

    let token = live.issue_host_token(&lease);
    let (result, ()) = tokio::join!(
        biased;
        live.host_register(Some(&token), &lease.id, endpoint()),
        async { fake.resume.notify_one() }
    );
    assert_eq!(result.unwrap(), lease);
    assert_eq!(
        live.require_host(&lease.id).unwrap().state,
        HostState::Joinable
    );
    assert_eq!(fake.starts.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn retired_matchmaking_assignment_never_reaches_provider_start() {
    let (live, fake) = setup().await;
    let mut input = request("unused");
    input.assignment_id = "8000000000000001".into();
    input.hopper = Some(1);
    assert_eq!(
        live.request_host(input).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    let row = live.host_for_assignment("8000000000000001").unwrap();
    assert_eq!(row.reason, Some(EndReason::Stopped));
    assert_eq!(row.ran_ms, Some(0));
    assert_eq!(fake.starts.load(Ordering::Relaxed), 0);
}

// POST /v1/admin/hosts: an admin's manual start.

fn admin_start(map: &str) -> AdminHostStart {
    AdminHostStart {
        map: map.into(),
        variant: None,
        max_players: 8,
        class: HostClass::Small,
        region: None,
        transport: None,
        launch: true,
    }
}

/// POSTs a start request through the shared router, answering the response.
async fn post_start(
    router: axum::Router,
    token: Option<String>,
    body: &str,
) -> (axum::http::StatusCode, serde_json::Value) {
    use axum::body::to_bytes;
    use tower::ServiceExt;

    let mut builder = axum::http::Request::post("/v1/admin/hosts")
        .header(axum::http::header::CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        builder = builder.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let response = router
        .oneshot(builder.body(axum::body::Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    let body = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, body)
}

#[tokio::test]
async fn admin_start_is_admin_only_and_launches_through_the_provider() {
    let (live, fake) = setup().await;
    let account = KeyPair::generate();
    let player = live
        .create_account(&AccountRequest::sign(
            &account,
            "Player",
            None,
            live.id(),
            crate::app::now(),
        ))
        .await
        .unwrap();
    let player_token = || {
        sign::issue_token(
            &account,
            &player.id,
            live.id(),
            crate::admin::LXM_ADMIN_WRITE,
            crate::app::now(),
        )
    };
    let read_token = || {
        sign::issue_token(
            &account,
            &player.id,
            live.id(),
            lxm::READ,
            crate::app::now(),
        )
    };
    let router = crate::http::router(live.clone());
    let body = r#"{"map":"lockout","max_players":4}"#;
    assert_eq!(
        post_start(router.clone(), None, body).await.0,
        axum::http::StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post_start(router.clone(), Some(player_token()), body)
            .await
            .0,
        axum::http::StatusCode::FORBIDDEN
    );
    assert_eq!(
        post_start(router.clone(), Some(read_token()), body).await.0,
        axum::http::StatusCode::UNAUTHORIZED
    );
    let (status, reply) = post_start(
        router,
        Some(operator(&live, crate::admin::LXM_ADMIN_WRITE)),
        body,
    )
    .await;
    assert_eq!(status, axum::http::StatusCode::CREATED);
    let lease: HostLease = serde_json::from_value(reply["lease"].clone()).unwrap();
    assert!(lease.id.starts_with("hl-"));
    assert!(lease.assignment_id.starts_with("custom-"));
    assert_eq!(lease.map, "lockout");
    assert_eq!(lease.max_players, 4);
    assert_eq!(lease.provider, ProviderKind::Local);
    // A launch answers the lease only: the boot credential stays private.
    assert!(reply.get("token").is_none());
    assert_eq!(fake.starts.load(Ordering::Relaxed), 1);
    assert_eq!(
        live.require_host(&lease.id).unwrap().state,
        HostState::Booting
    );
    register(&live, &lease).await;
}

#[tokio::test]
async fn admin_manual_lease_answers_a_token_that_registers_and_reports() {
    let (live, fake) = setup().await;
    let admin = || operator(&live, crate::admin::LXM_ADMIN_WRITE);
    let mut request = admin_start("lockout");
    request.launch = false;
    request.variant = Some("Slayer".into());
    request.region = Some("lhr".into());
    request.class = HostClass::Large;
    request.max_players = 12;
    let (lease, token) = live
        .admin_start_host(Some(&admin()), request)
        .await
        .unwrap();
    assert_eq!(fake.starts.load(Ordering::Relaxed), 0);
    assert_eq!(lease.variant.as_deref(), Some("Slayer"));
    assert_eq!(lease.region.as_deref(), Some("lhr"));
    assert_eq!(lease.class, HostClass::Large);
    assert!(lease.roster.is_empty() && lease.bots.is_empty());
    let token = token.expect("the manual lease answers its token");
    assert_eq!(
        live.verify_host_token(Some(&token), crate::host_identity::HOST_REGISTER)
            .unwrap(),
        lease.id
    );
    // The inventory shows the reserved lease as Requested, holding a slot.
    let inventory = live
        .admin_hosts(Some(&operator(&live, crate::invites::LXM_ADMIN_READ)))
        .unwrap();
    assert_eq!(inventory["running"][0]["lease"]["id"], json!(lease.id));
    assert_eq!(inventory["running"][0]["state"], json!("requested"));
    live.set_host_config(config(1));
    let refused = live
        .admin_start_host(Some(&admin()), admin_start("blocked"))
        .await
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::RateLimited);
    assert!(live.host_for_assignment("custom-blocked").is_none());

    // The manual container registers straight out of Requested.
    assert_eq!(
        live.host_register(Some(&token), &lease.id, endpoint())
            .await
            .unwrap(),
        lease
    );
    assert_eq!(
        live.require_host(&lease.id).unwrap().state,
        HostState::Joinable
    );
    for n in [3, 1] {
        let row = live
            .host_report(Some(&token), &lease.id, HostState::Playing, n, None)
            .await
            .unwrap();
        assert_eq!(row.state, HostState::Playing);
        assert_eq!(row.players_peak, 3);
    }
    live.host_report(
        Some(&token),
        &lease.id,
        HostState::Ended,
        0,
        Some(EndReason::MatchOver),
    )
    .await
    .unwrap();
    assert_eq!(
        live.require_host(&lease.id).unwrap().state,
        HostState::Ended
    );
    // No HostBooting ever happened, and the token stayed out of the log.
    let events = live
        .store
        .read(&Query {
            stream: Some(STREAM.into()),
            ..Query::after(0, 100)
        })
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .map(|e| e.body.kind.as_str())
            .collect::<Vec<_>>(),
        [
            HOST_REQUESTED,
            HOST_READY,
            HOST_PLAYING,
            HOST_PLAYING,
            HOST_ENDED
        ]
    );
    for e in events {
        assert!(!e.body.data.to_string().contains(&token));
    }
}

#[tokio::test]
async fn never_registered_manual_lease_times_out_like_any_requested_lease() {
    let (live, _) = setup().await;
    live.set_host_config(config(1));
    let mut start = admin_start("lockout");
    start.launch = false;
    let admin = || operator(&live, crate::admin::LXM_ADMIN_WRITE);
    let (lease, token) = live
        .admin_start_host(Some(&admin()), start)
        .await
        .unwrap();
    assert_eq!(
        live.require_host(&lease.id).unwrap().state,
        HostState::Requested
    );
    let due = lease.created_ms + BOOT_MS;
    assert_eq!(live.host_sweep_due_ms(), Some(due));
    live.sweep_hosts_at(due - 1).await;
    assert_eq!(
        live.require_host(&lease.id).unwrap().state,
        HostState::Requested
    );
    live.sweep_hosts_at(due).await;
    let row = live.require_host(&lease.id).unwrap();
    assert_eq!(
        (row.state, row.reason),
        (HostState::Failed, Some(EndReason::BootTimeout))
    );
    // The timed-out slot is free again, and the token no longer helps: an
    // ended lease refuses its reports.
    let token = token.expect("the manual lease answers its token");
    assert!(
        live.host_report(Some(&token), &lease.id, HostState::Ended, 0, None)
            .await
            .is_err()
    );
    assert!(live.request_host(request("next")).await.is_ok());
}

#[test]
fn admin_start_validates_map_and_seats() {
    let mut request = admin_start("lockout");
    for players in [0, 17] {
        request.max_players = players;
        assert_eq!(
            request.validate().unwrap_err().message,
            "max_players must be between 1 and 16"
        );
    }
    for players in [1, 8, 16] {
        request.max_players = players;
        assert_eq!(request.validate(), Ok(()));
    }
    request.max_players = 8;
    let long = "l".repeat(65);
    for map in ["", "lockoüt", long.as_str()] {
        request.map = map.into();
        assert_eq!(
            request.validate().unwrap_err().message,
            "map must be 1 to 64 ASCII characters",
            "{map:?}"
        );
    }
    request.map = "m".repeat(64);
    assert_eq!(request.validate(), Ok(()));
}

#[tokio::test]
async fn manual_start_still_enforces_budget_and_needs_a_provider_kind() {
    // No provider configured: neither launch mode can build a lease.
    let bare = Live::start_with_host_costs(
        Arc::new(MemoryStore::new()),
        ServerIdentity {
            id: "live.test".into(),
            key: KeyPair::generate(),
        },
        Policy::default(),
        Timers::Driven,
        HostCosts::default(),
    )
    .await
    .unwrap();
    let admin = || operator(&bare, crate::admin::LXM_ADMIN_WRITE);
    let mut request = admin_start("lockout");
    for launch in [true, false] {
        request.launch = launch;
        let refused = bare
            .admin_start_host(Some(&admin()), request.clone())
            .await
            .unwrap_err();
        assert_eq!(refused.code, ErrorCode::Unavailable);
        assert_eq!(refused.message, "no host provider");
        assert_eq!(bare.position(), 0);
    }
    drop(bare);

    // An exhausted monthly budget refuses before any event is recorded.
    let broke = Live::start_with_host_costs(
        Arc::new(MemoryStore::new()),
        ServerIdentity {
            id: "live.test".into(),
            key: KeyPair::generate(),
        },
        Policy::default(),
        Timers::Driven,
        HostCosts::configured(None, Some("0")).unwrap(),
    )
    .await
    .unwrap();
    let fake = Arc::new(Fake::new(ProviderKind::Local));
    broke.set_host_provider(fake.clone());
    broke.set_host_config(config(1));
    let mut manual = admin_start("lockout");
    manual.launch = false;
    let admin = || operator(&broke, crate::admin::LXM_ADMIN_WRITE);
    let refused = broke
        .admin_start_host(Some(&admin()), manual)
        .await
        .unwrap_err();
    assert_eq!(refused.code, ErrorCode::Unavailable);
    assert!(refused.message.contains("budget"));
    assert_eq!(broke.position(), 0);
    assert_eq!(fake.starts.load(Ordering::Relaxed), 0);
}
