use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use live_proto::host_pack::{Limits, PackStatus, SlotPorts};
use live_proto::hosts::{HostClass, ProviderKind};

use super::*;

fn lease(id: &str) -> HostLease {
    HostLease {
        id: id.into(),
        assignment_id: "custom-test".into(),
        provider: ProviderKind::Local,
        region: None,
        class: HostClass::Small,
        map: "lockout".into(),
        map_id: None,
        variant: None,
        hopper: None,
        max_players: 2,
        roster: vec![],
        bots: vec![],
        transport: Transport::Tunnel,
        idle_timeout_s: 120,
        max_match_s: 1800,
        created_ms: 0,
    }
}

fn boot() -> HostBoot {
    HostBoot {
        live_url: "http://127.0.0.1:8074".into(),
        token: "lease-token".into(),
        data_url: "https://private.example/bundle?sig=abc".into(),
        data_sha256: "ab".repeat(32),
        archive: live_proto::hosts::HostArchive::Film,
    }
}

/// A stand-in `h2 host pack` control port: accepts up to `cap` matches and
/// records every request line.
fn fake_pack(cap: usize) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&log);
    std::thread::spawn(move || {
        let mut leases: Vec<String> = Vec::new();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let req = loop {
                match http::parse_request(&buf) {
                    http::Parse::Done(req) => break req,
                    http::Parse::Invalid(_) => return,
                    http::Parse::Partial => {}
                }
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    return;
                }
                buf.extend_from_slice(&chunk[..n]);
            };
            seen.lock()
                .unwrap()
                .push(format!("{} {}", req.method, req.path));
            let lease = req
                .path
                .strip_prefix(&format!("{}/", host_pack::MATCHES_PATH))
                .map(str::to_owned);
            let (status, body) = match (req.method.as_str(), lease) {
                ("GET", None) => (
                    200,
                    serde_json::to_vec(&PackStatus {
                        limits: Limits::new(cap, 1.0, 3072),
                        matches: vec![],
                        container_mib: None,
                        draining: false,
                    })
                    .unwrap(),
                ),
                ("POST", None) if req.path == host_pack::WARM_PATH => (204, vec![]),
                ("POST", None) => {
                    let add: AddMatch = serde_json::from_slice(&req.body).unwrap();
                    assert_eq!(add.archive, live_proto::hosts::HostArchive::Film);
                    if leases.len() >= cap {
                        let refusal = Refusal::Full {
                            running: leases.len(),
                            max: cap,
                        };
                        (409, serde_json::to_vec(&refusal).unwrap())
                    } else {
                        let slot = leases.len() as u16;
                        leases.push(add.lease);
                        let added = Added {
                            slot,
                            ports: SlotPorts::for_slot(31000, 31100, 14720, slot).unwrap(),
                        };
                        (200, serde_json::to_vec(&added).unwrap())
                    }
                }
                ("GET", Some(lease)) => {
                    let status = MatchStatus {
                        lease: lease.clone(),
                        slot: 0,
                        alive: leases.contains(&lease),
                        load: Default::default(),
                    };
                    (200, serde_json::to_vec(&status).unwrap())
                }
                ("DELETE", Some(lease)) => {
                    leases.retain(|l| *l != lease);
                    (204, vec![])
                }
                _ => (404, vec![]),
            };
            let _ = stream.write_all(&http::response(status, &body));
        }
    });
    (addr, log)
}

fn sleeper() -> Child {
    let mut cmd = Command::new("sleep");
    cmd.arg("30");
    #[cfg(unix)]
    cmd.process_group(0);
    cmd.spawn().unwrap()
}

fn small_key(name: &str) -> PackKey {
    ((None, HostClass::Small), name.into())
}

async fn with_pack(packs: &LocalPacks, name: &str, window: u16, control: SocketAddr) {
    packs.state.lock().await.packs.insert(
        small_key(name),
        LocalPack {
            window,
            control,
            child: sleeper(),
            idle_task: None,
        },
    );
}

#[test]
fn references_name_the_pack_and_the_lease() {
    let reference = provider_ref("pack-2", "hl-a");
    assert_eq!(reference, "pack:pack-2/hl-a");
    assert_eq!(parse_ref(&reference), Some(("pack-2", "hl-a")));
    // Process IDs from unpacked local hosts are not pack references.
    assert_eq!(parse_ref("4242"), None);
    assert_eq!(parse_ref("pack:pack-x/hl-a"), None);
    assert_eq!(parse_ref("pack:pack-1/"), None);
}

#[test]
fn packing_is_off_unless_configured() {
    assert_eq!(pack_matches(None), None);
    assert_eq!(pack_matches(Some("3")), Some(3));
    assert_eq!(pack_matches(Some("0")), Some(1));
    assert_eq!(pack_matches(Some("auto")), Some(10));
    assert_eq!(pack_matches(Some("99")), Some(10));
    assert_eq!(pack_matches(Some("x")), None);
}

#[test]
fn pack_windows_do_not_overlap() {
    let a = window(31000, 31100, 32000, 0).unwrap();
    let b = window(31000, 31100, 32000, 1).unwrap();
    assert_eq!(
        (a.base_port, a.tunnel_port, a.agent_port, a.control),
        (31000, 31100, 14720, 32000)
    );
    // One large-class window even when the actual pack is smaller.
    assert_eq!(
        (b.base_port, b.tunnel_port, b.agent_port, b.control),
        (31070, 31110, 14730, 32001)
    );
    assert!(window(65530, 31100, 32000, 0).is_err());
    assert!(window(0, 31100, 32000, 0).is_err());
    // After 90 occupied windows the next tunnel range collides with the
    // first control ports. Window 99 collides with its own control port.
    let used: Vec<_> = (0..90).collect();
    assert_eq!(next_window(31000, 31100, 32000, &used).unwrap().0, 101);
}

#[tokio::test]
async fn leases_share_a_pack_until_it_refuses_then_the_next_pack_takes_them() {
    let packs = LocalPacks::new(3, std::env::temp_dir());
    let (full, full_log) = fake_pack(0);
    let (roomy, roomy_log) = fake_pack(3);
    with_pack(&packs, "pack-0", 0, full).await;
    with_pack(&packs, "pack-1", 1, roomy).await;
    let bin = PathBuf::from("/nonexistent/h2");

    // pack-0's measured load refuses; the pool drops it and moves on.
    let a = packs.start(bin.clone(), &lease("hl-a"), &boot()).await.unwrap();
    assert_eq!(a, "pack:pack-1/hl-a");
    assert_eq!(full_log.lock().unwrap().as_slice(), ["POST /v1/pack/matches"]);
    assert!(!packs.state.lock().await.packs.contains_key(&small_key("pack-0")));

    // The next lease joins the running pack instead of starting another.
    let b = packs.start(bin.clone(), &lease("hl-b"), &boot()).await.unwrap();
    assert_eq!(b, "pack:pack-1/hl-b");
    assert_eq!(packs.tunnel_addr("hl-b", &b).unwrap().port(), 31101);
    assert_eq!(packs.tunnel_addr("hl-a", &b), None);
    assert!(packs.alive(&a).await.unwrap());

    // The first stop removes one match; the pack keeps running.
    packs.stop("hl-a", &a).await.unwrap();
    assert!(roomy_log.lock().unwrap().contains(&"DELETE /v1/pack/matches/hl-a".to_owned()));
    assert!(!packs.alive(&a).await.unwrap());
    assert!(packs.state.lock().await.packs.contains_key(&small_key("pack-1")));
    assert_eq!(packs.tunnel_addr("hl-a", &a), None);

    // The last stop removes the match but leaves reusable capacity during linger.
    packs.stop("hl-b", &b).await.unwrap();
    let state = packs.state.lock().await;
    assert_eq!(state.packs.len(), 1);
    assert_eq!(state.pools[&(None, HostClass::Small)].leases("pack-1"), 0);
    drop(state);
    // Stopping again, or with another lease's reference, is harmless.
    packs.stop("hl-b", &b).await.unwrap();
    packs.stop("hl-x", &b).await.unwrap();
}

#[tokio::test]
async fn a_dead_pack_reports_its_matches_dead_and_is_forgotten() {
    let packs = LocalPacks::new(3, std::env::temp_dir());
    let (roomy, _) = fake_pack(3);
    with_pack(&packs, "pack-0", 0, roomy).await;
    let a = packs
        .start(PathBuf::from("/nonexistent/h2"), &lease("hl-a"), &boot())
        .await
        .unwrap();
    {
        let mut state = packs.state.lock().await;
        let local = state.packs.get_mut(&small_key("pack-0")).unwrap();
        stop_child(&mut local.child).unwrap();
    }
    assert!(!packs.alive(&a).await.unwrap());
    let state = packs.state.lock().await;
    assert!(state.packs.is_empty());
    assert_eq!(state.pools[&(None, HostClass::Small)].pack_of("hl-a"), None);
}

#[tokio::test]
async fn search_starts_the_right_class_and_silence_reaps_it_after_sixty_seconds() {
    let mut packs = LocalPacks::new(10, std::env::temp_dir());
    let launches = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&launches);
    packs.launch = Box::new(move |_bin, limits, used| {
        let (window, _) = next_window(31000, 31100, 32000, used)?;
        let (control, log) = fake_pack(limits.max_matches);
        observed.lock().unwrap().push((limits, window, log));
        Ok(LocalPack { window, control, child: sleeper(), idle_task: None })
    });
    let bin = PathBuf::from("/nonexistent/h2");
    let small = HostClass::Small;
    let large = HostClass::Large;
    let searching = WarmDemand { searchers: 1, leases: 0, available: 1 };
    assert_eq!(packs.warm(bin.clone(), None, &small, WarmDemand::default()).await.unwrap(), 0);
    assert!(launches.lock().unwrap().is_empty());

    // A search alone starts a supervisor, and only in its class and region.
    assert_eq!(packs.warm(bin.clone(), None, &small, searching).await.unwrap(), 5);
    assert_eq!(packs.warm(bin.clone(), None, &small, searching).await.unwrap(), 5);
    assert_eq!(packs.warm(bin.clone(), Some("west"), &large, searching).await.unwrap(), 10);
    {
        let rows = launches.lock().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].0.memory_mib, rows[0].0.vcpus, rows[0].1), (1536, 1.0, 0));
        assert_eq!((rows[1].0.memory_mib, rows[1].0.vcpus, rows[1].1), (3072, 2.0, 1));
        assert_eq!(rows[0].2.lock().unwrap().as_slice(), ["GET /v1/pack", "POST /v1/pack/warm"]);
    }
    let reference = packs.start(bin, &lease("hl-search"), &boot()).await.unwrap();
    assert_eq!(launches.lock().unwrap().len(), 2);
    assert!(packs.alive(&reference).await.unwrap());

    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    tokio::task::yield_now().await;
    assert_eq!(packs.state.lock().await.packs.len(), 1, "only occupied small pack remains");
    packs.stop("hl-search", &reference).await.unwrap();
    tokio::time::advance(Duration::from_secs(59)).await;
    assert_eq!(packs.state.lock().await.packs.len(), 1);
    tokio::time::advance(Duration::from_millis(1001)).await;
    tokio::task::yield_now().await;
    assert!(packs.state.lock().await.packs.is_empty());
}
