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
    events: Vec<crate::events::Event>,
    control_phrases: Vec<Option<usize>>,
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

        let (path, messages) = if let Some(reference) = refs.hud_messages {
            let tag = map.tag(reference.id)?;
            let messages = Hmt::read(map, tag)?;
            let (translated, _) = locales.add_mission_hmt(set, &tag.name, &messages)?;
            translated.install(set, &unicode_path(&tag.name))?;
            (tag.name.clone(), messages)
        } else {
            (
                format!("{}\\hud", map.scenario().name),
                Hmt { messages: Vec::new() },
            )
        };
        let help = refs
            .help_text
            .map(|r| Ustr::read(map, map.tag(r.id)?))
            .transpose()?
            .map_or_else(Vec::new, crate::scenario_help::messages);
        let events = crate::events::read(map, refs.object_names)?;
        let hud_messages = if messages.messages.is_empty()
            && help.is_empty()
            && events.entries.is_empty()
        {
            None
        } else {
            Some(hud::rebuild(
                &messages,
                &help,
                &events,
                &help_flash(map)?,
                unit_hud(map)?,
                &set.strings,
            )?
            .install(set, &path)?)
        };
        let message_names = messages.messages.into_iter().map(|m| m.name).collect();
        Ok(Self {
            object_names,
            hud_messages,
            message_names,
            events: events.routes,
            control_phrases: events.control_phrases,
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

    /// Write the event string IDs into the fields that the native interaction
    /// and pickup code reads, while the rebuilt objects are still blocks.
    pub fn write_object_prompts(&self, set: &mut TagSet) {
        for event in &self.events {
            event.write(set);
        }
    }

    /// Index control prompts by the new placements, including gaps from
    /// source controls that could not be converted.
    pub fn write_control_prompts(
        &self,
        set: &mut TagSet,
        placed: impl Iterator<Item = (i16, i16)>,
    ) {
        let Some(tag) = self.hud_messages.and_then(|id| set.tag_mut(id)) else {
            return;
        };
        let map::build::TagPayload::Built(root) = &mut tag.payload else {
            return;
        };
        let mut rows = Vec::new();
        for (source, target) in placed {
            let (Ok(source), Ok(target)) = (usize::try_from(source), usize::try_from(target)) else {
                continue;
            };
            let Some(Some(row)) = self.control_phrases.get(source) else {
                continue;
            };
            let row = u16::try_from(*row).expect("HUD shared rows fit in 16 bits");
            if rows.len() <= target {
                rows.resize(target + 1, u16::MAX);
            }
            rows[target] = row;
        }
        if !rows.is_empty() {
            let count = rows.len() as u32;
            let bytes = rows.into_iter().flat_map(u16::to_le_bytes).collect();
            root.fixups.retain(|fixup| fixup.offset != hud::CONTROL_PROMPTS);
            root.block(hud::CONTROL_PROMPTS, count, Block::new(bytes));
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

fn unit_hud(map: &H1Map) -> Result<&[u8]> {
    let Some(tag) = map.find(h1map::Group::new(b"unhi"), "ui\\hud\\cyborg") else {
        return Ok(&[]);
    };
    Ok(map.tag_data(tag)?)
}
