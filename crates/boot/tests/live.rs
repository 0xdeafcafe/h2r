//! Opt-in boot wiring, synthetic runtime only. No module build or game run.
use boot::{LiveConfig, live_glue};
use runtime::{Dispatch, Guest, Runtime};

fn world() -> (std::sync::Arc<Runtime>, std::sync::Arc<kernel::Kernel>) {
    let rt = Runtime::new(Guest::new(), Dispatch::default());
    let k = kernel::Kernel::install(&rt, kernel::Config::default()).unwrap();
    (rt, k)
}
#[test]
fn disabled_live_leaves_console_dispatch_and_guest_mappings_unchanged() {
    let (rt, k) = world();
    let before = rt.memory().mappings().to_vec();
    let console = k.console_config();
    let config = LiveConfig {
        enabled: false,
        accounts_path: Some("/nonexistent/live.json".into()),
        server_url: None,
    };
    assert!(
        live_glue::install(&rt, &k, &config, None)
            .unwrap()
            .is_none()
    );
    assert_eq!(k.console_config(), console);
    assert_eq!(rt.memory().mappings(), before);
    rt.edit_dispatch(|d| {
        for e in live_hle::entries::ENTRIES {
            assert!(d.override_name(e.va).is_none());
        }
    });
}
#[test]
fn enabled_live_registers_boundary_sets_existing_phy_config_and_stores_url() {
    let (rt, k) = world();
    let mut console = k.console_config();
    console.language = 3;
    k.set_console_config(console);
    let config = LiveConfig {
        enabled: true,
        accounts_path: None,
        server_url: Some("https://live.example/base".into()),
    };
    let hle = live_glue::install(&rt, &k, &config, None).unwrap().unwrap();
    assert_eq!(hle.config(), &config);
    assert_eq!(k.console_config().phy_link, 0x0d);
    assert_eq!(k.console_config().language, 3);
    rt.edit_dispatch(|d| {
        for e in live_hle::entries::ENTRIES {
            assert_eq!(
                d.override_name(e.va),
                Some(format!("live_hle::{}", e.name).as_str())
            );
        }
    });
}
#[test]
fn invalid_account_configuration_does_not_enable_phy_or_register_partial_boundary() {
    let (rt, k) = world();
    let config = LiveConfig {
        enabled: true,
        accounts_path: Some("/nonexistent/live.json".into()),
        server_url: None,
    };
    assert!(live_glue::install(&rt, &k, &config, None).is_err());
    assert_eq!(k.console_config().phy_link, 0);
    rt.edit_dispatch(|d| assert!(d.override_name(0x3a0641).is_none()));
}
#[test]
fn registration_conflict_leaves_console_unchanged() {
    let (rt, k) = world();
    rt.edit_dispatch(|d| {
        d.register_override(0x3a0641, "prior", |_, _, _| {
            Ok(runtime::TransferAbi::stop(0))
        });
    });
    assert!(
        live_glue::install(
            &rt,
            &k,
            &LiveConfig {
                enabled: true,
                ..LiveConfig::default()
            },
            None,
        )
        .is_err()
    );
    assert_eq!(k.console_config().phy_link, 0);
    rt.edit_dispatch(|d| assert!(d.override_name(0x36c47d).is_none()));
}

#[test]
fn enabled_live_reaches_the_kernel_phy_boundary() {
    use runtime::{CpuAbi, Event, ExecCtx, Runner, exec::HARNESS_EXIT, guest::Permissions};
    let (rt, k) = world();
    live_glue::install(
        &rt,
        &k,
        &LiveConfig {
            enabled: true,
            ..LiveConfig::default()
        },
        None,
    )
    .unwrap();
    rt.memory()
        .map_zeroed(0x10000, 0x1000, Permissions::RW)
        .unwrap();
    let mut ctx = ExecCtx::new(rt.clone(), 1);
    let mut cpu = CpuAbi::default();
    for (ordinal, args, expected) in [(253, vec![0, 0], 0), (252, vec![0], 0x0d)] {
        cpu.esp = 0x10000;
        rt.memory().set_u32(cpu.esp, HARNESS_EXIT).unwrap();
        for (i, value) in args.iter().enumerate() {
            rt.memory()
                .set_u32(cpu.esp + 4 + 4 * i as u32, *value)
                .unwrap();
        }
        let t = Runner::default().run(&mut cpu, &mut ctx, k.entry(ordinal).unwrap());
        assert!(matches!(ctx.event(t.event_id), Some(Event::HarnessExit)));
        assert_eq!(cpu.eax, expected);
    }
}

#[test]
fn explicit_network_settings_reach_hle_without_changing_live_config_contract() {
    let (rt, k) = world();
    let before = rt.memory().mappings().to_vec();
    let config = LiveConfig {
        enabled: true,
        accounts_path: None,
        server_url: Some("https://service.example/base".into()),
    };
    let network = live_hle::NetworkConfig::from_values(
        Some("192.168.1.50:31000"),
        Some("192.168.1.50:31010"),
    )
    .unwrap();
    let hle = live_glue::install_with_network(&rt, &k, &config, network.clone(), None)
        .unwrap()
        .unwrap();
    assert_eq!(hle.network_config(), &network);
    assert_eq!(hle.config(), &config);
    assert_eq!(k.console_config().phy_link, 0xd);
    // Installing the HLE does not bind any of the title's channels.
    assert_eq!(rt.memory().mappings(), before);
}
#[test]
fn invalid_network_configuration_leaves_phy_and_dispatch_unchanged() {
    let (rt, k) = world();
    let config = LiveConfig {
        enabled: true,
        ..LiveConfig::default()
    };
    let network = live_hle::NetworkConfig::new("0.0.0.0:0".parse().unwrap());
    assert!(live_glue::install_with_network(&rt, &k, &config, network, None).is_err());
    assert_eq!(k.console_config().phy_link, 0);
    rt.edit_dispatch(|d| assert!(d.override_name(0x3cd188).is_none()));
}
#[test]
fn disabled_explicit_network_install_ignores_invalid_network_and_accounts() {
    let (rt, k) = world();
    let before = rt.memory().mappings().to_vec();
    let config = LiveConfig {
        enabled: false,
        accounts_path: Some("/nonexistent/live-hle.json".into()),
        server_url: None,
    };
    let network = live_hle::NetworkConfig::new("0.0.0.0:0".parse().unwrap());
    assert!(
        live_glue::install_with_network(&rt, &k, &config, network, None)
            .unwrap()
            .is_none()
    );
    assert_eq!(k.console_config().phy_link, 0);
    assert_eq!(rt.memory().mappings(), before);
}
