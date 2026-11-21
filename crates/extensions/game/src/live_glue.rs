//! Live public-boundary registration and the existing console PHY switch.
use anyhow::Context;
use live_hle::{Hle, LiveConfig, ServerOptions, maps::MapGate};
use std::sync::Arc;

mod browser_advertisement;
pub use browser_advertisement::install as install_browser_advertisement;

/// Install before guest execution, after attaching the kernel. Disabled is a
/// complete no-op, including account I/O, guest allocation and console settings.
/// `maps` is the join-time map gate (docs/game/maps.md §5; the boot builds it with
/// `map_bases::gate` over the disc and `<data>/maps`); None takes sessions
/// as they come.
pub fn install(
    rt: &runtime::Runtime,
    k: &kernel::Kernel,
    config: &LiveConfig,
    maps: Option<Arc<MapGate>>,
) -> anyhow::Result<Option<Arc<Hle>>> {
    install_configured(rt, k, config, None, maps, None)
}

/// Explicit network settings for embedding/tests; install() reads the host env.
pub fn install_with_network(
    rt: &runtime::Runtime,
    k: &kernel::Kernel,
    config: &LiveConfig,
    network: live_hle::NetworkConfig,
    maps: Option<Arc<MapGate>>,
) -> anyhow::Result<Option<Arc<Hle>>> {
    install_configured(rt, k, config, Some(network), maps, None)
}

pub fn install_configured(
    rt: &runtime::Runtime,
    k: &kernel::Kernel,
    config: &LiveConfig,
    network: Option<live_hle::NetworkConfig>,
    maps: Option<Arc<MapGate>>,
    publisher: Option<live_hle::HostPublisher>,
) -> anyhow::Result<Option<Arc<Hle>>> {
    if !config.enabled {
        return Ok(None);
    }
    let network = match network {
        Some(network) => network,
        None => live_hle::NetworkConfig::from_env().map_err(anyhow::Error::msg)?,
    };
    let options = ServerOptions {
        publisher,
        maps,
        build: build(),
        capabilities: CAPABILITIES.iter().map(|c| c.to_string()).collect(),
        ..ServerOptions::default()
    };
    let hle = Hle::with_server_options(config.clone(), network, options)
        .context("configuring Live client")?;
    register_network(rt, k, hle)
}

pub fn install_film(
    rt: &runtime::Runtime,
    k: &kernel::Kernel,
    network: live_hle::NetworkConfig,
    live_service: bool,
) -> anyhow::Result<Option<Arc<Hle>>> {
    register_network(rt, k, Hle::for_film(network, live_service)?)
}

fn register_network(
    rt: &runtime::Runtime,
    k: &kernel::Kernel,
    hle: Arc<Hle>,
) -> anyhow::Result<Option<Arc<Hle>>> {
    rt.edit_dispatch(|d| live_hle::register(d, &hle))
        .context("registering Live boundary")?;
    let mut console = k.console_config();
    console.phy_link =
        kernel::net::LINK_UP | kernel::net::LINK_100_MBPS | kernel::net::LINK_FULL_DUPLEX;
    k.set_console_config(console);
    Ok(Some(hle))
}

/// What this build tells Live it can do, on every presence set
/// (docs/game/game-options.md section 3, "Older builds").
pub const CAPABILITIES: [&str; 1] = [live_proto::capability::VARIANT_EXT];

static BUILD: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// This build's id (its git short hash), from the binary's own build
/// metadata (`app::BuildInfo`): the binary sets it once before the game
/// starts, and Live presence carries it. A value that isn't a short hash
/// ("unknown", a source tarball) is ignored.
pub fn set_build(sha: &str) {
    if live_proto::valid_build(sha) {
        let _ = BUILD.set(sha.to_string());
    }
}

/// The build id [`set_build`] gave, if any.
pub fn build() -> Option<String> {
    BUILD.get().cloned()
}

/// The HLE visibility for a `live.visibility` value.
pub fn visibility(v: i64) -> live_hle::Visibility {
    use crate::setting_defs::{VISIBILITY_FRIENDS, VISIBILITY_INVITE};
    match v {
        VISIBILITY_FRIENDS => live_hle::Visibility::Friends,
        VISIBILITY_INVITE => live_hle::Visibility::Invite,
        _ => live_hle::Visibility::Public,
    }
}

/// Keep the HLE's hosted-session visibility on the `live.visibility`
/// setting: now, and on every change (a row press, `h2 settings set`), which
/// also updates an open lobby (docs/live/live-custom-games.md).
pub fn bind_visibility(hle: &Arc<Hle>, store: &settings::Store) {
    let key = crate::setting_defs::LIVE_VISIBILITY.key;
    if store.def(key).is_none() {
        return;
    }
    let setting = store.handle::<i64>(key);
    hle.set_session_visibility(visibility(setting.get()));
    let weak = Arc::downgrade(hle);
    store.on_change(key, move |v| {
        if let (Some(hle), Some(v)) = (weak.upgrade(), v.as_int()) {
            hle.set_session_visibility(visibility(v));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setting_defs::{ALL, LIVE_VISIBILITY};
    use settings::{Store, Value};

    #[test]
    fn the_visibility_setting_reaches_the_hle_now_and_on_every_change() {
        let config = LiveConfig {
            enabled: true,
            ..Default::default()
        };
        let hle = Hle::with_address(config, "127.0.0.1:3074".parse().unwrap()).unwrap();
        let env = |k: &str| (k == "H2_LIVE_VISIBILITY").then(|| "friends".to_string());
        let store = Store::load(ALL, None, &env, &[]).0;
        bind_visibility(&hle, &store);
        assert_eq!(hle.session_visibility(), live_hle::Visibility::Friends);
        let store = Store::defaults(ALL);
        bind_visibility(&hle, &store);
        assert_eq!(hle.session_visibility(), live_hle::Visibility::Public);
        store.step(LIVE_VISIBILITY.key).unwrap();
        assert_eq!(hle.session_visibility(), live_hle::Visibility::Friends);
        store.step(LIVE_VISIBILITY.key).unwrap();
        assert_eq!(hle.session_visibility(), live_hle::Visibility::Invite);
        assert_eq!(store.get(LIVE_VISIBILITY.key), Some(Value::Int(2)));
        store.step(LIVE_VISIBILITY.key).unwrap();
        assert_eq!(hle.session_visibility(), live_hle::Visibility::Public);
    }
}
