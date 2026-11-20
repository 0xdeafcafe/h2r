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
                ("POST", None) => {
                    let add: AddMatch = serde_json::from_slice(&req.body).unwrap();
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

async fn with_pack(packs: &LocalPacks, name: &str, window: u16, control: SocketAddr) {
    packs.state.lock().await.packs.insert(
        name.into(),
        LocalPack {
            window,
            control,
            child: sleeper(),
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
    assert_eq!(pack_matches(Some("x")), None);
}

#[test]
fn pack_windows_do_not_overlap() {
    let a = window(3, 31000, 31100, 32000, 0).unwrap();
    let b = window(3, 31000, 31100, 32000, 1).unwrap();
    assert_eq!(
        (a.base_port, a.tunnel_port, a.agent_port, a.control),
        (31000, 31100, 14720, 32000)
    );
    // Three slots of seven UDP ports, then the next pack.
    assert_eq!(
        (b.base_port, b.tunnel_port, b.agent_port, b.control),
        (31021, 31103, 14723, 32001)
    );
    assert!(window(3, 65530, 31100, 32000, 0).is_err());
    assert!(window(3, 0, 31100, 32000, 0).is_err());
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
    assert!(!packs.state.lock().await.packs.contains_key("pack-0"));

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
    assert!(packs.state.lock().await.packs.contains_key("pack-1"));
    assert_eq!(packs.tunnel_addr("hl-a", &a), None);

    // The last stop ends the pack's process.
    packs.stop("hl-b", &b).await.unwrap();
    let state = packs.state.lock().await;
    assert!(state.packs.is_empty());
    assert!(state.pool.is_empty());
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
        let local = state.packs.get_mut("pack-0").unwrap();
        stop_child(&mut local.child).unwrap();
    }
    assert!(!packs.alive(&a).await.unwrap());
    let state = packs.state.lock().await;
    assert!(state.packs.is_empty());
    assert_eq!(state.pool.pack_of("hl-a"), None);
}
