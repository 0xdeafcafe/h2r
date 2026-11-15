//! Live public-boundary registration and the existing console PHY switch.
use anyhow::Context;
use live_hle::{Hle, LiveConfig};
use std::sync::Arc;

/// Install before guest execution, after attaching the kernel. Disabled is a
/// complete no-op, including account I/O, guest allocation and console settings.
pub fn install(
    rt: &runtime::Runtime,
    k: &kernel::Kernel,
    config: &LiveConfig,
) -> anyhow::Result<Option<Arc<Hle>>> {
    if !config.enabled {
        return Ok(None);
    }
    let hle = Hle::new(config.clone()).context("configuring Live client")?;
    rt.edit_dispatch(|d| live_hle::register(d, &hle))
        .context("registering Live boundary")?;
    let mut console = k.console_config();
    console.phy_link =
        kernel::net::LINK_UP | kernel::net::LINK_100_MBPS | kernel::net::LINK_FULL_DUPLEX;
    k.set_console_config(console);
    Ok(Some(hle))
}
