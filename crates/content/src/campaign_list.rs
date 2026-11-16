//! More than one campaign (docs/game/multiple-campaigns.md): a campaign's
//! manifest, and the level-list records 1.0 reads, built from it.
//!
//! 1.0 keeps its campaign levels in two lists in the globals tag (`matg`) of
//! `mainmenu.map`, each with free slots:
//!
//! | list | where | element | retail on the user's disc |
//! |---|---|---|---|
//! | the levels the game starts | `+0x170`, element 0, block `+0` | [`START_LEN`]: campaign id, map id, scenario path | 20 slots, 15 used (campaign 1) |
//! | the level select's campaigns | `+0x178`, element 0, block `+0` | [`UI_CAMPAIGN_LEN`]: campaign id, names, descriptions | 4 slots, 1 used |
//! | the level select's levels | `+0x178`, element 0, block `+8` | [`UI_LEVEL_LEN`]: campaign id, map id, picture, names, descriptions | 20 slots, 15 used |
//!
//! A free slot holds campaign id and map id `-1`. Nothing checks these
//! records the way `patch.lvl` is checked (no version, no CRC): the code
//! only compares ids. What the records must satisfy is what the code that
//! reads them assumes, and [`check`] asserts exactly that:
//!
//! - every map id is unique across all campaigns: the level select's
//!   record lookup (`0x19c1f0`) matches the map id alone;
//! - at most [`MAX_LEVELS`] levels a campaign: `0x19c6d0` gathers one
//!   campaign's map ids into a 20-entry stack buffer without a bound;
//! - ids rise in play order: `0x19c6d0` sorts by map id, the next level
//!   (`0x19c440`) is the smallest id above the current one in the same
//!   campaign, and the level select shows the levels up to the first one
//!   above the profile's progress;
//! - ids are at least [`FIRST_SHOWN_ID`]: the lobby's level select hides
//!   lower ones (`0x2c4f5d`).
//!
//! Nothing here holds game data: a manifest names levels by their map
//! file's name, and the Halo 1 manifest ([`HALO1`]) holds only level names.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::manifest::LANGUAGES;

/// A campaign's manifest file, in its directory under `<data>/campaigns/`.
pub const MANIFEST_FILE: &str = "campaign.toml";
/// The manifest format this build reads.
pub const FORMAT: u32 = 1;

/// One start-level record (`matg +0x170`): campaign id, map id, scenario
/// path.
pub const START_LEN: usize = 0x108;
pub const START_CAMPAIGN: usize = 0;
pub const START_MAP: usize = 4;
pub const START_PATH: usize = 8;
pub const PATH_LEN: usize = 0x100;

/// One level-select campaign (`matg +0x178`, element 0, block `+0`).
pub const UI_CAMPAIGN_LEN: usize = 0xb44;
pub const UI_CAMPAIGN_NAMES: usize = 4;
pub const UI_CAMPAIGN_DESCRIPTIONS: usize = 0x244;

/// One level-select level (`matg +0x178`, element 0, block `+8`).
pub const UI_LEVEL_LEN: usize = 0xb50;
pub const UI_LEVEL_CAMPAIGN: usize = 0;
pub const UI_LEVEL_MAP: usize = 4;
/// The picture: a tag reference, group then datum.
pub const UI_LEVEL_PICTURE: usize = 8;
pub const UI_LEVEL_NAMES: usize = 0x10;
pub const UI_LEVEL_DESCRIPTIONS: usize = 0x250;

const NAME_UNITS: usize = 32;
const DESCRIPTION_UNITS: usize = 128;

/// Levels a campaign may have: `0x19c6d0`'s buffer.
pub const MAX_LEVELS: usize = 20;
/// The smallest map id the lobby's level select shows (`0x2c4f5d` skips
/// ids below `0x69`).
pub const FIRST_SHOWN_ID: u32 = 0x69;
/// Map ids of the `n`th added campaign (from 0) start at
/// `ID_STRIDE * (n + 1)`. Retail ids on the user's disc are `0x1` to
/// `0x325`.
pub const ID_STRIDE: u32 = 0x1000;
/// A free slot's campaign id and map id.
pub const FREE: u32 = u32::MAX;

/// The longest map name: a cache header holds it in 32 bytes with its NUL.
pub const MAX_MAP_NAME: usize = 31;

/// `campaign.toml`.
///
/// ```toml
/// format = 1
///
/// [campaign]
/// key = "halo1"              # its directory under <data>/campaigns
/// name = "Halo: Combat Evolved"
/// description = "..."        # optional
/// order = 10                 # optional: the picker's order (then key)
///
/// [[level]]
/// map = "a10"                # maps\a10.map
/// name = "The Pillar of Autumn"
/// description = "..."        # optional
/// scenario = "..."           # optional; scenarios\solo\<map>\<map>
/// picture = "01b_spacestation" # optional: a retail level whose picture it shows
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignManifest {
    pub format: u32,
    pub campaign: CampaignInfo,
    #[serde(rename = "level")]
    pub levels: Vec<LevelInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CampaignInfo {
    pub key: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LevelInfo {
    pub map: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("campaign manifest: {0}")]
pub struct CampaignError(pub String);

fn err<T>(msg: impl Into<String>) -> Result<T, CampaignError> {
    Err(CampaignError(msg.into()))
}

/// Whether `name` can be a map file's name: lower-case ASCII letters,
/// digits and `_`, 1 to [`MAX_MAP_NAME`] bytes.
pub fn valid_map_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_MAP_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl CampaignManifest {
    pub fn parse(text: &str) -> Result<Self, CampaignError> {
        let m: Self = toml::from_str(text).map_err(|e| CampaignError(e.to_string()))?;
        m.validate()?;
        Ok(m)
    }

    pub fn validate(&self) -> Result<(), CampaignError> {
        if self.format != FORMAT {
            return err(format!("format {} (this build reads {FORMAT})", self.format));
        }
        let key = &self.campaign.key;
        if !valid_map_name(key) {
            return err(format!("key {key:?}: lower-case letters, digits and _"));
        }
        if self.campaign.name.trim().is_empty() {
            return err(format!("{key}: no name"));
        }
        if self.levels.is_empty() || self.levels.len() > MAX_LEVELS {
            return err(format!(
                "{key}: {} levels (1 to {MAX_LEVELS})",
                self.levels.len()
            ));
        }
        let mut seen = BTreeSet::new();
        for l in &self.levels {
            if !valid_map_name(&l.map) {
                return err(format!("{key}: map {:?}: lower-case letters, digits and _, at most {MAX_MAP_NAME}", l.map));
            }
            if !seen.insert(l.map.as_str()) {
                return err(format!("{key}: map {} twice", l.map));
            }
            if l.name.trim().is_empty() {
                return err(format!("{key}: {} has no name", l.map));
            }
            let path = l.scenario_path();
            if !path.is_ascii() || path.len() >= PATH_LEN || path.contains('\0') {
                return err(format!("{key}: {}: scenario path {path:?} does not fit", l.map));
            }
            if path.rsplit('\\').next() != Some(l.map.as_str()) {
                // 1.0 opens d:\maps\<basename of the path>.map (0x214db0).
                return err(format!(
                    "{key}: {}: the scenario path {path:?} must end in \\{}",
                    l.map, l.map
                ));
            }
            if let Some(p) = &l.picture
                && !valid_map_name(p)
            {
                return err(format!("{key}: {}: picture {p:?} is not a map name", l.map));
            }
        }
        Ok(())
    }
}

impl LevelInfo {
    /// The scenario path its records hold: its own, else
    /// `scenarios\solo\<map>\<map>`. 1.0 loads the map from the basename.
    pub fn scenario_path(&self) -> String {
        self.scenario
            .clone()
            .unwrap_or_else(|| format!("scenarios\\solo\\{0}\\{0}", self.map))
    }
}

/// The Halo 1 campaign (`h2 import halo1` writes the levels' maps beside
/// it): the ten levels in play order, with their names. No picture: until
/// the import makes them, each shows its free slot's own.
pub const HALO1: &str = r#"format = 1

[campaign]
key = "halo1"
name = "Halo: Combat Evolved"
order = 10

[[level]]
map = "a10"
name = "The Pillar of Autumn"

[[level]]
map = "a30"
name = "Halo"

[[level]]
map = "a50"
name = "The Truth and Reconciliation"

[[level]]
map = "b30"
name = "The Silent Cartographer"

[[level]]
map = "b40"
name = "Assault on the Control Room"

[[level]]
map = "c10"
name = "343 Guilty Spark"

[[level]]
map = "c20"
name = "The Library"

[[level]]
map = "c40"
name = "Two Betrayals"

[[level]]
map = "d20"
name = "Keyes"

[[level]]
map = "d40"
name = "The Maw"
"#;

/// A campaign made of retail Halo 2 levels, so the picker and the second
/// list can be seen before any converted map exists. Install it by
/// writing it as `<data>/campaigns/test/campaign.toml`.
pub const FIXTURE: &str = r#"format = 1

[campaign]
key = "test"
name = "Second Campaign (test)"
description = "Retail Halo 2 levels, listed as a second campaign."
order = 90

[[level]]
map = "03a_oldmombasa"
name = "Outskirts (2)"
picture = "03a_oldmombasa"

[[level]]
map = "05a_deltaapproach"
name = "Delta Halo (2)"
picture = "05a_deltaapproach"

[[level]]
map = "07a_highcharity"
name = "Gravemind (2)"
picture = "07a_highcharity"
"#;

/// A campaign as the game will see it: its ids, and its levels' records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Built {
    pub key: String,
    pub campaign_id: u32,
    pub names: [String; 9],
    pub descriptions: [String; 9],
    pub levels: Vec<BuiltLevel>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltLevel {
    pub map: String,
    pub map_id: u32,
    pub scenario: String,
    pub names: [String; 9],
    pub descriptions: [String; 9],
    /// The retail level whose picture it borrows, by map name.
    pub picture: Option<String>,
}

fn all_languages(s: &str) -> [String; 9] {
    LANGUAGES.map(|_| s.to_owned())
}

/// Builds `manifests` against the retail lists: campaign ids from one past
/// the largest retail id, in picker order ([`CampaignInfo::order`], then
/// key); map ids `ID_STRIDE * (n + 1) + 1 + level`. A campaign whose key or
/// map names repeat an earlier one's is refused, as is any id a retail
/// record already holds.
pub fn build(
    manifests: &[CampaignManifest],
    retail_campaigns: &[u32],
    retail_maps: &[u32],
) -> Result<Vec<Built>, CampaignError> {
    let mut order: Vec<&CampaignManifest> = manifests.iter().collect();
    order.sort_by(|a, b| {
        (a.campaign.order.unwrap_or(0), &a.campaign.key)
            .cmp(&(b.campaign.order.unwrap_or(0), &b.campaign.key))
    });
    let first = retail_campaigns
        .iter()
        .filter(|&&c| c != FREE)
        .max()
        .map_or(1, |m| m + 1);
    let mut keys = BTreeSet::new();
    let mut maps = BTreeSet::new();
    let mut out = Vec::new();
    for (n, m) in order.into_iter().enumerate() {
        m.validate()?;
        if !keys.insert(m.campaign.key.clone()) {
            return err(format!("campaign {} twice", m.campaign.key));
        }
        let campaign_id = first + n as u32;
        if retail_campaigns.contains(&campaign_id) {
            return err(format!("campaign id {campaign_id} is taken"));
        }
        let base = ID_STRIDE
            .checked_mul(n as u32 + 1)
            .ok_or_else(|| CampaignError("too many campaigns".into()))?;
        let mut levels = Vec::new();
        for (i, l) in m.levels.iter().enumerate() {
            if !maps.insert(l.map.clone()) {
                return err(format!("map {} is in two campaigns", l.map));
            }
            let map_id = base + 1 + i as u32;
            if retail_maps.contains(&map_id) {
                return err(format!("map id {map_id:#x} is taken"));
            }
            levels.push(BuiltLevel {
                map: l.map.clone(),
                map_id,
                scenario: l.scenario_path(),
                names: all_languages(&l.name),
                descriptions: all_languages(l.description.as_deref().unwrap_or("")),
                picture: l.picture.clone(),
            });
        }
        out.push(Built {
            key: m.campaign.key.clone(),
            campaign_id,
            names: all_languages(&m.campaign.name),
            descriptions: all_languages(m.campaign.description.as_deref().unwrap_or("")),
            levels,
        });
    }
    Ok(out)
}

/// Checks the rules the module docs list over a whole list (retail
/// records and built ones): `(campaign id, map id)` of every used start
/// record.
pub fn check(levels: &[(u32, u32)]) -> Result<(), CampaignError> {
    let mut ids = BTreeSet::new();
    let mut per: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &(c, m) in levels.iter().filter(|(c, _)| *c != FREE) {
        if !ids.insert(m) {
            return err(format!("map id {m:#x} is used twice"));
        }
        per.entry(c).or_default().push(m);
    }
    for (c, ms) in per {
        if ms.len() > MAX_LEVELS {
            return err(format!("campaign {c} has {} levels", ms.len()));
        }
    }
    Ok(())
}

/// The order and range rules for one built campaign.
pub fn check_built(b: &Built) -> Result<(), CampaignError> {
    if b.levels.len() > MAX_LEVELS {
        return err(format!("{}: {} levels", b.key, b.levels.len()));
    }
    for w in b.levels.windows(2) {
        if w[0].map_id >= w[1].map_id {
            return err(format!("{}: ids do not rise at {}", b.key, w[1].map));
        }
    }
    if let Some(l) = b.levels.iter().find(|l| l.map_id < FIRST_SHOWN_ID) {
        return err(format!("{}: {} has id {:#x}", b.key, l.map, l.map_id));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Records.

/// A start-level record.
pub fn start_record(campaign_id: u32, l: &BuiltLevel) -> [u8; START_LEN] {
    let mut r = [0u8; START_LEN];
    put(&mut r, START_CAMPAIGN, campaign_id);
    put(&mut r, START_MAP, l.map_id);
    r[START_PATH..START_PATH + l.scenario.len()].copy_from_slice(l.scenario.as_bytes());
    r
}

/// A free start-level slot, as the disc's are.
pub fn free_start() -> [u8; START_LEN] {
    let mut r = [0u8; START_LEN];
    put(&mut r, START_CAMPAIGN, FREE);
    put(&mut r, START_MAP, FREE);
    r
}

/// `(campaign id, map id, scenario path)` of a start record.
pub fn read_start(r: &[u8]) -> Option<(u32, u32, String)> {
    let r = r.get(..START_LEN)?;
    let p = &r[START_PATH..START_PATH + PATH_LEN];
    let p = &p[..p.iter().position(|&b| b == 0).unwrap_or(p.len())];
    Some((
        u32_at(r, START_CAMPAIGN),
        u32_at(r, START_MAP),
        String::from_utf8_lossy(p).into_owned(),
    ))
}

/// A level-select campaign record.
pub fn ui_campaign_record(b: &Built) -> Vec<u8> {
    let mut r = vec![0u8; UI_CAMPAIGN_LEN];
    put(&mut r, 0, b.campaign_id);
    texts_into(&mut r, UI_CAMPAIGN_NAMES, NAME_UNITS, &b.names);
    texts_into(&mut r, UI_CAMPAIGN_DESCRIPTIONS, DESCRIPTION_UNITS, &b.descriptions);
    r
}

/// A level-select level record, with the picture reference `(group,
/// datum)`.
pub fn ui_level_record(campaign_id: u32, l: &BuiltLevel, picture: [u32; 2]) -> Vec<u8> {
    let mut r = vec![0u8; UI_LEVEL_LEN];
    put(&mut r, UI_LEVEL_CAMPAIGN, campaign_id);
    put(&mut r, UI_LEVEL_MAP, l.map_id);
    put(&mut r, UI_LEVEL_PICTURE, picture[0]);
    put(&mut r, UI_LEVEL_PICTURE + 4, picture[1]);
    texts_into(&mut r, UI_LEVEL_NAMES, NAME_UNITS, &l.names);
    texts_into(&mut r, UI_LEVEL_DESCRIPTIONS, DESCRIPTION_UNITS, &l.descriptions);
    r
}

/// `(campaign id, map id, picture, English name)` of a level-select level.
pub fn read_ui_level(r: &[u8]) -> Option<(u32, u32, [u32; 2], String)> {
    let r = r.get(..UI_LEVEL_LEN)?;
    Some((
        u32_at(r, UI_LEVEL_CAMPAIGN),
        u32_at(r, UI_LEVEL_MAP),
        [u32_at(r, UI_LEVEL_PICTURE), u32_at(r, UI_LEVEL_PICTURE + 4)],
        utf16_from(&r[UI_LEVEL_NAMES..UI_LEVEL_NAMES + NAME_UNITS * 2]),
    ))
}

/// `(campaign id, English name)` of a level-select campaign.
pub fn read_ui_campaign(r: &[u8]) -> Option<(u32, String)> {
    let r = r.get(..UI_CAMPAIGN_LEN)?;
    Some((
        u32_at(r, 0),
        utf16_from(&r[UI_CAMPAIGN_NAMES..UI_CAMPAIGN_NAMES + NAME_UNITS * 2]),
    ))
}

fn texts_into(r: &mut [u8], at: usize, units: usize, texts: &[String; 9]) {
    for (i, t) in texts.iter().enumerate() {
        utf16_into(&mut r[at + i * units * 2..][..units * 2], t);
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn put(r: &mut [u8], at: usize, v: u32) {
    r[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// `text` as UTF-16LE into `field`, cut at a character boundary so a
/// terminating zero always fits.
pub fn utf16_into(field: &mut [u8], text: &str) {
    let room = field.len() / 2 - 1;
    let mut units = Vec::new();
    for c in text.chars() {
        let mut buf = [0u16; 2];
        let enc = c.encode_utf16(&mut buf);
        if units.len() + enc.len() > room {
            break;
        }
        units.extend_from_slice(enc);
    }
    field.fill(0);
    for (i, u) in units.iter().enumerate() {
        field[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
    }
}

fn utf16_from(field: &[u8]) -> String {
    let units: Vec<u16> = field
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests;
