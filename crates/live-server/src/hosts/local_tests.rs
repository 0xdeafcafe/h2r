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
        let cmd = provider.command(&lease(id), &boot).unwrap();
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
        assert!(provider.command(&lease(id), &boot()).is_err());
    }
    assert!(!root.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn stopping_checks_both_lease_and_pid_and_reaps_the_child() {
    let provider = LocalProvider::new(crate::blobs::tests::temp_dir("host-stop"));
    let child = Command::new("sleep").arg("30").spawn().unwrap();
    let pid = child.id().to_string();
    provider
        .children
        .lock()
        .unwrap()
        .insert(pid.clone(), ("hl-owner".into(), child));

    assert!(provider.alive(&pid).await.unwrap());
    provider.stop("hl-other", &pid).await.unwrap();
    assert!(provider.alive(&pid).await.unwrap());
    provider.stop("hl-owner", &pid).await.unwrap();
    assert!(!provider.alive(&pid).await.unwrap());
    provider.stop("hl-owner", &pid).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn polling_forgets_an_exited_child() {
    let provider = LocalProvider::new(crate::blobs::tests::temp_dir("host-exit"));
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id().to_string();
    child.wait().unwrap();
    provider
        .children
        .lock()
        .unwrap()
        .insert(pid.clone(), ("hl-ended".into(), child));

    assert!(!provider.alive(&pid).await.unwrap());
    assert!(!provider.children.lock().unwrap().contains_key(&pid));
}
