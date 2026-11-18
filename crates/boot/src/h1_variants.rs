//! Which variant of a converted Halo 1 map the next game loads: Halo 1's
//! sandbox (or assets) or Halo 2's (docs/game/h1-multiplayer.md "Setting,
//! Halo 1 maps sandbox", docs/game/campaigns.md §5).
//!
//! The choice is made per game on the pregame screen and applies at the
//! map's next open, with no restart:
//!
//! - **Multiplayer.** [`set_mp_sandbox`] flips the [`vfs::ContentVariants`]
//!   the data partition is served through ([`mp_variants`], given to the
//!   VFS's config at boot). Each converted map's content package path
//!   (`T:\$c\<offer>\h1_<map>.map`, what 1.0 opens) is registered by
//!   `boot::new_maps` with its two variant files, and served from the one
//!   chosen when it is opened. The health model and the precision hook
//!   follow it (`health::set_halo1_sandbox`).
//! - **Campaign.** [`set_campaign_assets`] flips the [`vfs::SandboxChoice`]
//!   the campaign maps backend serves by ([`campaign_choice`]): a level's
//!   Halo 2-assets map ([`campaign_alternate`]) when there is one, else
//!   its Halo 1 map.
//!
//! Both are safe from any thread and take no guest memory. A map open now
//! keeps what it had; `boot::map_slots` drops the game's cached copy of a
//! map whose variant changed.
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The multiplayer table and choice, shared with the VFS.
static MP: LazyLock<vfs::ContentVariants> = LazyLock::new(vfs::ContentVariants::default);
/// The campaign choice, shared with the campaign maps backend.
static CAMPAIGN: LazyLock<vfs::SandboxChoice> = LazyLock::new(vfs::SandboxChoice::default);

/// The settings key of the campaign choice (`h1` or `h2`).
pub const CAMPAIGN_KEY: &str = "campaign.halo1_assets";

/// Converted Halo 1 multiplayer maps play in Halo 1's sandbox (`true`) or
/// Halo 2's from their next load.
pub fn set_mp_sandbox(halo1: bool) {
    MP.choice().set_halo1(halo1);
    crate::health::set_halo1_sandbox(halo1);
}

/// Whether converted Halo 1 multiplayer maps load in Halo 1's sandbox.
pub fn mp_sandbox_halo1() -> bool {
    MP.choice().halo1()
}

/// Halo 1 campaign levels load with Halo 1's assets (`true`) or Halo 2's
/// from their next load.
pub fn set_campaign_assets(halo1: bool) {
    CAMPAIGN.set_halo1(halo1);
}

/// Whether Halo 1 campaign levels load with Halo 1's assets.
pub fn campaign_assets_halo1() -> bool {
    CAMPAIGN.halo1()
}

/// The table the data partition serves converted maps by: put in the
/// VFS's config before it is mounted.
pub fn mp_variants() -> vfs::ContentVariants {
    MP.clone()
}

/// The choice the campaign maps backend serves by.
pub fn campaign_choice() -> vfs::SandboxChoice {
    CAMPAIGN.clone()
}

/// Where a campaign level's Halo 2-assets map goes, beside its Halo 1 map
/// `<data>/campaigns/<key>/<level>.map`: `<data>/campaigns/<key>/h2/<level>.map`
/// (the multiplayer packages' `h2` folder). The importer does not write it
/// yet (docs/game/campaigns.md §5).
pub fn campaign_alternate(level_map: &Path) -> Option<PathBuf> {
    let name = level_map.file_name()?;
    Some(level_map.parent()?.join("h2").join(name))
}

/// [`campaign_alternate`] of each served campaign map.
pub fn campaign_alternates(maps: &[(String, PathBuf)]) -> Vec<(String, PathBuf)> {
    maps.iter()
        .filter_map(|(n, p)| Some((n.clone(), campaign_alternate(p)?)))
        .collect()
}

/// Sets both choices from the settings store at boot; returns the
/// multiplayer sandbox's name (`h1` or `h2`) for what boot still picks by
/// it (the packages library).
pub fn init_from_store(store: &settings::Store) -> &'static str {
    let mp = crate::setting_defs::halo1_sandbox(store);
    set_mp_sandbox(mp == "h1");
    set_campaign_assets(campaign_setting(store) != "h2");
    mp
}

/// The campaign setting's value name, `h1` unless it says Halo 2.
fn campaign_setting(store: &settings::Store) -> &'static str {
    match store.get(CAMPAIGN_KEY) {
        Some(settings::Value::Int(2)) => "h2",
        _ => "h1",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_campaign_alternate_sits_in_an_h2_folder_beside_the_level() {
        let p = Path::new("/data/campaigns/halo1/a30.map");
        assert_eq!(
            campaign_alternate(p).unwrap(),
            Path::new("/data/campaigns/halo1/h2/a30.map")
        );
        let maps = vec![("a30".to_string(), p.to_path_buf())];
        assert_eq!(campaign_alternates(&maps)[0].0, "a30");
    }

    #[test]
    fn the_switches_hold_what_was_set_and_keep_health_in_step() {
        set_mp_sandbox(false);
        assert!(!mp_sandbox_halo1());
        assert!(!mp_variants().choice().halo1(), "the VFS's table sees it");
        assert!(!crate::health::halo1_sandbox());
        set_mp_sandbox(true);
        assert!(crate::health::halo1_sandbox());
        set_campaign_assets(false);
        assert!(!campaign_assets_halo1());
        assert!(!campaign_choice().halo1());
        set_campaign_assets(true);
        assert!(campaign_choice().halo1());
    }
}
