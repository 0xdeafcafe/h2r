//! Discovery filters and the additional facts hosts advertise for them.
use super::Session;
use crate::hopper::{defaults::RETAIL_MAPS, variant::GameType};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserKind {
    Multiplayer,
    Coop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MapCategory {
    Halo2,
    MapPack,
    Mods,
    HaloCe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CampaignTitle {
    Halo2,
    HaloCe,
}

impl CampaignTitle {
    /// A campaign title we recognize from a stock level's map stem.
    pub fn from_level(level: &str) -> Option<Self> {
        match level {
            "a10" | "a30" | "a50" | "b30" | "b40" | "c10" | "c20" | "c40" | "d20" | "d40" => {
                Some(Self::HaloCe)
            }
            "00a_introduction" | "01a_tutorial" | "01b_spacestation" | "03a_oldmombasa"
            | "03b_newmombasa" | "04a_gasgiant" | "04b_floodlab" | "05a_deltaapproach"
            | "05b_deltatowers" | "06a_sentinelwalls" | "06b_floodzone" | "07a_highcharity"
            | "07b_forerunnership" | "08a_deltacliffs" | "08b_deltacontrol" => Some(Self::Halo2),
            _ => None,
        }
    }
}

impl MapCategory {
    /// Recognizes stock map stems, display titles and scenario paths. The
    /// Halo: CE converter reserves the `h1_` prefix for multiplayer maps.
    pub fn from_map_name(map: &str) -> Option<Self> {
        if content::map_pack::is_map_name(map) {
            return Some(Self::MapPack);
        }
        let name = map.trim().rsplit(['/', '\\']).next()?.to_ascii_lowercase();
        let stem = name.strip_suffix(".map").unwrap_or(&name);
        match CampaignTitle::from_level(stem) {
            Some(CampaignTitle::HaloCe) => return Some(Self::HaloCe),
            Some(CampaignTitle::Halo2) => return Some(Self::Halo2),
            None => {}
        }
        if stem.starts_with("h1_") && stem.len() > 3 {
            return Some(Self::HaloCe);
        }
        let words = stem.replace('_', " ");
        let is_retail = RETAIL_MAPS
            .iter()
            .any(|(_, title)| title.eq_ignore_ascii_case(&words));
        (is_retail || matches!(stem, "cyclotron" | "beavercreek")).then_some(Self::Halo2)
    }
}

/// Session facts for discovery. Each field can be unknown independently.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clankers: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_type: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_category: Option<MapCategory>,
}

impl BrowserMetadata {
    pub(super) fn validate(&self) -> Result<(), String> {
        validate_game_type(self.game_type)?;
        validate_variant(self.variant.as_deref())
    }
}

/// The JSON `filters` query parameter of `GET /v1/sessions`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserFilters {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<BrowserKind>,
    /// True requires Clankers, false requires no Clankers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clankers: Option<bool>,
    /// The multiplayer game engine index (see [`GameType`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_type: Option<u32>,
    /// The host's full variant name, matched without regard to case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_category: Option<MapCategory>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_space: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign: Option<CampaignTitle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<u8>,
}

impl BrowserFilters {
    pub fn validate(&self) -> Result<(), String> {
        validate_game_type(self.game_type)?;
        validate_variant(self.variant.as_deref())?;
        if self
            .difficulty
            .is_some_and(|d| usize::from(d) >= super::DIFFICULTIES.len())
        {
            return Err("invalid campaign difficulty".into());
        }
        Ok(())
    }

    /// Only a known mismatch excludes a session. The service supplies the
    /// map category it can recover from its catalogue or the older map name.
    pub fn matches(
        &self,
        session: &Session,
        map_category: impl FnOnce() -> Option<MapCategory>,
    ) -> bool {
        let ad = &session.advertisement;
        let coop = ad.is_coop();
        let kind = if coop {
            BrowserKind::Coop
        } else {
            BrowserKind::Multiplayer
        };
        if self.kind.is_some_and(|expected| expected != kind)
            || (coop && (self.game_type.is_some() || self.variant.is_some()))
            || (!coop && (self.campaign.is_some() || self.difficulty.is_some()))
        {
            return false;
        }

        let browser = ad.browser.as_ref();
        let campaign = ad.campaign.as_ref();
        if !allows(self.clankers, browser.and_then(|b| b.clankers))
            || !allows(self.game_type, browser.and_then(|b| b.game_type))
            || !allows(self.difficulty, campaign.map(|c| c.difficulty))
            || !allows(
                self.campaign,
                campaign.and_then(|c| CampaignTitle::from_level(&c.level)),
            )
        {
            return false;
        }
        if let Some((expected, actual)) = self
            .variant
            .as_deref()
            .zip(browser.and_then(|b| b.variant.as_deref()))
            && expected.trim().to_lowercase() != actual.trim().to_lowercase()
        {
            return false;
        }
        if let Some(expected) = self.map_category
            && !allows(
                Some(expected),
                browser.and_then(|b| b.map_category).or_else(map_category),
            )
        {
            return false;
        }

        let slots = ad.slots;
        !self.has_space
            || (slots.public_open > 0 && session.players.len() < slots.capacity() as usize)
    }
}

fn allows<T: Eq>(expected: Option<T>, advertised: Option<T>) -> bool {
    expected
        .zip(advertised)
        .is_none_or(|(expected, actual)| expected == actual)
}

fn validate_game_type(engine: Option<u32>) -> Result<(), String> {
    if engine.is_some_and(|value| GameType::from_engine(value).is_none()) {
        return Err("invalid multiplayer game type".into());
    }
    Ok(())
}

fn validate_variant(variant: Option<&str>) -> Result<(), String> {
    if variant
        .is_some_and(|v| v.trim().is_empty() || v.len() > 256 || v.chars().any(char::is_control))
    {
        return Err("invalid variant name".into());
    }
    Ok(())
}
