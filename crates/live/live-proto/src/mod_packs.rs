//! Owned releases of map packages. Membership changes in one log event.
use crate::Xuid;
use crate::maps::{MapId, MapSummary, PackInfo, Sha256, Who};
use serde::{Deserialize, Serialize};

pub const MOD_PACK_PUBLISHED: &str = "ModPackPublished";
pub const MAX_PACK_MEMBERS: usize = 64;

/// A stable ID for the uploader's release. Old "Release: map" rows and a
/// later explicit import of that release resolve to the same pack.
pub fn mod_pack_id_for(uploader: Xuid, title: &str) -> String {
    let key = format!("{uploader}/{}", title.trim().to_lowercase());
    let hash = Sha256::of(key.as_bytes()).to_string();
    format!("p-{}", &hash[..48])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModPackAdvisory {
    CheatingPack,
    NotForOnline,
}

/// The uploader's metadata. Screenshots are submitted to the image
/// endpoints, so raw content hashes cannot bypass image validation.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModPackDocument {
    pub name: String,
    pub authors: Vec<String>,
    pub members: Vec<MapId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisory: Option<ModPackAdvisory>,
}

impl ModPackDocument {
    pub fn info(&self) -> PackInfo {
        PackInfo {
            name: self.name.clone(),
            authors: self.authors.clone(),
            description: self.description.clone(),
            source_url: self.source_url.clone(),
            ..PackInfo::default()
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        self.info().validate()?;
        if self.authors.is_empty() {
            return Err("name at least one original author".into());
        }
        if self.year.is_some_and(|year| !(1990..=9999).contains(&year)) {
            return Err("year must be a four-digit release year from 1990".into());
        }
        if self.version.as_ref().is_some_and(|v| {
            v.trim().is_empty()
                || v.chars().count() > crate::maps::MAX_VERSION
                || v.chars().any(char::is_control)
        }) {
            return Err(format!(
                "version must be 1 to {} characters on one line",
                crate::maps::MAX_VERSION
            ));
        }
        if !(1..=MAX_PACK_MEMBERS).contains(&self.members.len()) {
            return Err(format!("provide 1 to {MAX_PACK_MEMBERS} members"));
        }
        Ok(())
    }
}

/// Server-signed, after ownership, membership and moderation checks.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ModPackPublished {
    pub id: String,
    pub uploader: Xuid,
    pub pack: ModPackDocument,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ModPackSummary {
    pub id: String,
    #[serde(flatten)]
    pub info: PackInfo,
    pub uploader: Who,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub year: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisory: Option<ModPackAdvisory>,
    pub members: Vec<MapSummary>,
    pub map_count: usize,
    /// Bytes to install the pack, counting shared layers once.
    pub size: u64,
}
