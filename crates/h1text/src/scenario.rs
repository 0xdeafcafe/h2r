//! Install a campaign scenario's text without changing Halo 1's message order.

use h1map::H1Map;
use map::build::{Block, TagSet};
use map::{FourCC, TagId};

use crate::Result;
use crate::convert::Locales;
use crate::h1::{Hmt, Ustr, scenario_text};
use crate::hud;

/// The scenario's legacy localized object-name reference.
const SCENARIO_OBJECT_NAMES: u32 = 0x1f8;

/// The converted data for a campaign scenario's text references and script
/// message literals. The help list and HUD messages also have `unic` tags
/// with English rows in their source order.
pub struct LevelText {
    object_names: Option<TagId>,
    hud_messages: Option<TagId>,
    message_names: Vec<String>,
}

impl LevelText {
    /// Rebuild the scenario's lists and intern their string IDs. The caller
    /// installs the enlarged English table in the globals after conversion.
    pub fn install(map: &H1Map, set: &mut TagSet, locales: &mut Locales) -> Result<Self> {
        let refs = scenario_text(map)?;
        let mut object_names = None;
        for tag in map
            .tags()
            .iter()
            .filter(|tag| tag.group == h1map::Group::new(b"ustr"))
        {
            let list = Ustr::read(map, tag)?;
            let (converted, _) = locales.add_ustr(set, &tag.name, &list)?;
            let id = converted.install(set, &unicode_path(&tag.name))?;
            if refs
                .object_names
                .is_some_and(|reference| reference.id == tag.id)
            {
                object_names = Some(id);
            }
        }

        let mut hud_messages = None;
        let mut message_names = Vec::new();
        if let Some(reference) = refs.hud_messages {
            let tag = map.tag(reference.id)?;
            let messages = Hmt::read(map, tag)?;
            let (translated, _) = locales.add_mission_hmt(set, &tag.name, &messages)?;
            translated.install(set, &unicode_path(&tag.name))?;
            hud_messages = Some(
                hud::rebuild(
                    &messages,
                    &help_flash(map)?,
                    &component_flash(map)?,
                    &set.strings,
                )?
                .install(set, &tag.name)?,
            );
            message_names = messages.messages.into_iter().map(|m| m.name).collect();
        }
        Ok(Self {
            object_names,
            hud_messages,
            message_names,
        })
    }

    /// Replaces the scenario references whose Halo 1 equivalents were
    /// rebuilt, or clears those references when Halo 1 leaves them null.
    pub fn write(&self, scenario: &mut Block) {
        for (at, group, id) in [
            (SCENARIO_OBJECT_NAMES, FourCC(*b"unic"), self.object_names),
            (hud::SCENARIO_HMT, hud::GROUP, self.hud_messages),
        ] {
            scenario.fixups.retain(|f| f.offset != at);
            scenario.put_u32(at as usize, group.as_u32());
            scenario.put_u32(at as usize + 4, u32::MAX);
            if let Some(id) = id {
                scenario.tag_ref(at, id);
            }
        }
    }

    /// Retail Halo 2's legacy script setters take a signed short index.
    pub fn message_index(&self, token: &str) -> Option<u32> {
        self.message_names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(token))
            .and_then(|index| i16::try_from(index).ok())
            .map(|index| index as u32)
    }
}

/// Tag path used for the localized counterpart of a Halo 1 text list.
pub fn unicode_path(h1_path: &str) -> String {
    format!("h1\\{h1_path}")
}

// Halo 1's help draw passes this hudg record to the flash interpolator
// (0xdf9cd). A missing record is an invalid campaign source.
fn help_flash(map: &H1Map) -> Result<[u8; 0x18]> {
    let tag = map
        .tags()
        .iter()
        .find(|tag| tag.group == h1map::Group::new(b"hudg"))
        .ok_or_else(|| crate::Error::Invalid("level has no HUD globals".into()))?;
    let bytes = map.tag_data(tag)?;
    bytes
        .get(0xd0..0xe8)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| crate::Error::Invalid("HUD globals shorter than their help flash".into()))
}

// The unit HUD uses its panel/background flash record for each component.
// E: 0xdcfc7 (shield), 0xdd33f (health), 0xdd57e (motion sensor) and
// 0xe18bb..0xe18ed (the static widget's flash fields).
fn component_flash(map: &H1Map) -> Result<[u8; 0x48]> {
    let mut profiles = [0; 0x48];
    let Some(tag) = map.find(h1map::Group::new(b"unhi"), "ui\\hud\\cyborg") else {
        return Ok(profiles);
    };
    let bytes = map.tag_data(tag)?;
    for (i, start) in [0x17c + 0x34, 0x8c + 0x34, 0x26c + 0x34]
        .into_iter()
        .enumerate()
    {
        let record = bytes.get(start..start + 0x18).ok_or_else(|| {
            crate::Error::Invalid("unit HUD shorter than its flash records".into())
        })?;
        profiles[i * 0x18..(i + 1) * 0x18].copy_from_slice(record);
    }
    Ok(profiles)
}
