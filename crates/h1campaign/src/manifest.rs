//! Where a converted level goes: `<data>/campaigns/halo1/<level>.map`,
//! listed by that folder's `campaign.toml` (`content::campaign_list`,
//! docs/halo1/campaign-picker.md). The VFS serves the file as
//! `maps\<level>.map`, the name the level's start record's scenario path
//! ends in.

use content::campaign_list::{CampaignManifest, HALO1, MANIFEST_FILE};

use crate::{Result, invalid};

/// The campaign folder's key.
pub const KEY: &str = "halo1";

/// The scenario path a converted level holds, which its start record
/// names: `scenarios\solo\<level>\<level>`.
pub fn scenario_path(level: &str) -> String {
    format!("scenarios\\solo\\{0}\\{0}", level.to_ascii_lowercase())
}

/// The manifest's file name.
pub fn file_name() -> &'static str {
    MANIFEST_FILE
}

/// The map's file name in the campaign folder.
pub fn map_file(level: &str) -> String {
    format!("{}.map", level.to_ascii_lowercase())
}

/// The campaign manifest to write beside a converted level: `existing`
/// when there is one and it lists the level, else Halo 1's own
/// (`campaign_list::HALO1`). Fails when the level would not be listed or
/// its record would name another scenario path.
pub fn manifest_for(existing: Option<&str>, level: &str) -> Result<String> {
    let text = match existing {
        Some(t) if CampaignManifest::parse(t).is_ok() => t.to_owned(),
        _ => HALO1.to_owned(),
    };
    check(&text, level)?;
    Ok(text)
}

/// Checks that `text` lists `level` with the scenario path a converted map
/// holds.
pub fn check(text: &str, level: &str) -> Result<()> {
    let m = CampaignManifest::parse(text).map_err(|e| invalid(e.to_string()))?;
    if m.campaign.key != KEY {
        return Err(invalid(format!(
            "campaign key {} is not {KEY}",
            m.campaign.key
        )));
    }
    let l = m
        .levels
        .iter()
        .find(|l| l.map.eq_ignore_ascii_case(level))
        .ok_or_else(|| invalid(format!("{MANIFEST_FILE} does not list {level}")))?;
    if l.scenario_path() != scenario_path(level) {
        return Err(invalid(format!(
            "{MANIFEST_FILE} names {} for {level}, the map holds {}",
            l.scenario_path(),
            scenario_path(level)
        )));
    }
    Ok(())
}
