use std::ffi::OsStr;

use super::*;
use live_proto::hosts::{HostClass, Transport};

fn lease(id: &str) -> HostLease {
    HostLease {
        id: id.into(),
        assignment_id: "custom-test".into(),
        provider: ProviderKind::Local,
        region: Some("lhr".into()),
        class: HostClass::Small,
        map: "lockout".into(),
        map_id: None,
        variant: None,
        hopper: None,
        max_players: 2,
        roster: vec![],
        bots: vec![],
        transport: Transport::Udp,
        idle_timeout_s: 120,
        max_match_s: 1800,
        created_ms: 0,
    }
}

fn boot() -> HostBoot {
    HostBoot {
        live_url: "http://127.0.0.1:8074".into(),
        token: "lease-token".into(),
        data_url: "https://private.example/bundle?sig=abc&expires=42".into(),
        data_sha256: "ab".repeat(32),
    }
}

#[test]
fn command_passes_boot_values_verbatim_and_isolates_writable_data() {
    let root = crate::blobs::tests::temp_dir("host-command");
    let provider = LocalProvider::new(&root);
    let boot = boot();
    for id in ["hl-first", "hl-second"] {
        let cmd = provider.command(&lease(id), &boot, 0).unwrap();
        let (_, encoded) = cmd
            .get_envs()
            .find(|(key, _)| *key == OsStr::new("H2_HOST_LEASE_JSON"))
            .unwrap();
        let decoded: HostLease = serde_json::from_str(encoded.unwrap().to_str().unwrap()).unwrap();
        assert_eq!(decoded, lease(id));
        let data = root.join("hosts").join(id);
        assert!(data.is_dir());
        let args: Vec<&OsStr> = cmd.get_args().collect();
        for (flag, value) in [
            ("--lease", id),
            ("--live", boot.live_url.as_str()),
            ("--token", boot.token.as_str()),
            ("--data-url", boot.data_url.as_str()),
            ("--data-sha", boot.data_sha256.as_str()),
            ("--region", "lhr"),
        ] {
            assert!(
                args.windows(2)
                    .any(|pair| pair == [OsStr::new(flag), OsStr::new(value)])
            );
        }
        assert_eq!(&args[..2], &[OsStr::new("host"), OsStr::new("run")]);
        assert!(
            args.windows(2)
                .any(|pair| pair == [OsStr::new("--data"), data.as_os_str()])
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_lease_cannot_create_another_data_directory() {
    let root = crate::blobs::tests::temp_dir("host-invalid");
    let provider = LocalProvider::new(&root);
    for id in ["", "..", "../outside", "/absolute", "a/b", r"a\b"] {
        assert!(provider.command(&lease(id), &boot(), 0).is_err());
    }
    assert!(!root.exists());
}

#[test]
fn local_hosts_use_disjoint_port_ranges() {
    assert_eq!(port_for(31000, 7, 0).unwrap(), 31000);
    assert_eq!(port_for(31000, 7, 1).unwrap(), 31007);
    assert_eq!(port_for(31100, 1, 1).unwrap(), 31101);
    assert_eq!(port_for(65529, 7, 0).unwrap(), 65529);
    assert!(port_for(65529, 7, 1).is_err());
    assert!(port_for(0, 1, 0).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn stopping_checks_lease_and_pid_and_closes_the_games_inherited_socket() {
    use std::io::{BufRead, BufReader, Read};
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let provider = LocalProvider::new(crate::blobs::tests::temp_dir("host-stop"));
    let (reader, writer) = UnixStream::pair().unwrap();
    reader
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let child = Command::new("sh")
        .args(["-c", "sleep 30 & echo ready; wait"])
        .process_group(0)
        .stdout(Stdio::from(OwnedFd::from(writer)))
        .spawn()
        .unwrap();
    let pid = child.id().to_string();
    provider.children.lock().unwrap().insert(
        pid.clone(),
        LocalHost {
            lease_id: "hl-owner".into(),
            slot: 0,
            child,
        },
    );

    let mut output = BufReader::new(reader);
    let mut ready = String::new();
    output.read_line(&mut ready).unwrap();
    assert_eq!(ready, "ready\n");
    provider.stop("hl-other", &pid).await.unwrap();
    assert!(provider.alive(&pid).await.unwrap());
    provider.stop("hl-owner", &pid).await.unwrap();
    assert!(!provider.alive(&pid).await.unwrap());
    // Both the supervisor and its game held the write end.
    assert_eq!(output.read(&mut [0]).unwrap(), 0);
    provider.stop("hl-owner", &pid).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn start_reuses_the_same_lease_and_releases_the_slot_on_stop() {
    use std::os::unix::fs::PermissionsExt;
    let root = crate::blobs::tests::temp_dir("host-start");
    std::fs::create_dir(&root).unwrap();
    let bin = root.join("fake-h2");
    std::fs::write(&bin, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut provider = LocalProvider::new(&root);
    provider.bin = Some(bin);
    let first = lease("hl-first");
    let second = lease("hl-second");
    let a = provider.start(&first, &boot()).await.unwrap();
    assert_eq!(provider.start(&first, &boot()).await.unwrap(), a);
    let b = provider.start(&second, &boot()).await.unwrap();
    assert_ne!(a, b);
    assert_eq!(provider.children.lock().unwrap()[&b].slot, 1);
    provider.stop(&first.id, &a).await.unwrap();
    let c = provider.start(&lease("hl-third"), &boot()).await.unwrap();
    assert_eq!(provider.children.lock().unwrap()[&c].slot, 0);
    drop(provider);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn polling_forgets_an_exited_child() {
    let provider = LocalProvider::new(crate::blobs::tests::temp_dir("host-exit"));
    let mut child = Command::new("true").process_group(0).spawn().unwrap();
    let pid = child.id().to_string();
    child.wait().unwrap();
    provider.children.lock().unwrap().insert(
        pid.clone(),
        LocalHost {
            lease_id: "hl-ended".into(),
            slot: 0,
            child,
        },
    );

    assert!(!provider.alive(&pid).await.unwrap());
    assert!(!provider.children.lock().unwrap().contains_key(&pid));
}

#[cfg(unix)]
#[tokio::test]
async fn retrying_a_failed_launch_cannot_return_a_dead_pid() {
    let root = crate::blobs::tests::temp_dir("host-retry");
    let mut provider = LocalProvider::new(&root);
    provider.bin = Some("/usr/bin/true".into());
    let lease = lease("hl-exit");
    let pid = provider.start(&lease, &boot()).await.unwrap();
    provider
        .children
        .lock()
        .unwrap()
        .get_mut(&pid)
        .unwrap()
        .child
        .wait()
        .unwrap();
    assert!(provider.start(&lease, &boot()).await.is_err());
    assert!(!provider.alive(&pid).await.unwrap());
    std::fs::remove_dir_all(root).unwrap();
}
