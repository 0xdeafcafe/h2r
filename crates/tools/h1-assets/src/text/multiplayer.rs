//! Carry Halo 1 pickup messages into a multiplayer scenario.

use std::collections::HashMap;
use std::ops::Range;

use h1map::H1Map;
use map::build::{TagPayload, TagSet};

use crate::text::events::{self, Events, Scope};
use crate::text::h1::Hmt;
use crate::text::{Error, Result, hud};

/// Source pickup templates, names and silhouettes for the rebuilt items
/// in one multiplayer sandbox.
pub struct PickupText(Events);

impl PickupText {
    /// Read the shared HUD data. Halo 2 sandbox weapons use the retail
    /// Halo 2 HUD strings, so only Halo 1 equipment needs routes there.
    pub fn read(map: &H1Map, weapons: bool) -> Result<Self> {
        let events = events::read(map, Scope::Multiplayer { weapons })?;
        if events.entries.is_empty() {
            return Err(Error::Invalid("missing shared HUD messages".into()));
        }
        Ok(Self(events))
    }

    /// Bind only rebuilt object slots and attach the event tag to the
    /// pinned scenario. The normal scenario relocator carries the reference
    /// into fresh maps.
    pub fn install(&self, set: &mut TagSet, objects: Range<usize>) -> Result<()> {
        let mut ids = HashMap::new();
        for slot in set
            .slots
            .get(objects)
            .ok_or_else(|| Error::Invalid("pickup text object slots".into()))?
        {
            let (Some(tag), Some(name)) = (&slot.tag, slot.name.as_deref()) else {
                continue;
            };
            if matches!(&tag.group.0, b"weap" | b"eqip")
                && ids.insert((tag.group, name), tag.id).is_some()
            {
                return Err(Error::Invalid(format!(
                    "duplicate pickup {} {name}",
                    tag.group
                )));
            }
        }
        let mut events = self.0.clone();
        events
            .routes
            .retain(|route| ids.contains_key(&route.target()));
        events
            .subjects
            .retain(|subject| ids.contains_key(&(subject.group, subject.object.as_str())));
        if events.routes.is_empty() && events.subjects.is_empty() {
            return Ok(());
        }
        let routes: Vec<_> = events
            .routes
            .iter()
            .map(|route| ids[&route.target()])
            .collect();
        let subjects: Vec<_> = events
            .subjects
            .iter()
            .enumerate()
            .map(|(i, subject)| (i, ids[&(subject.group, subject.object.as_str())]))
            .collect();

        let mut converted = hud::rebuild(
            &Hmt {
                messages: Vec::new(),
            },
            &[],
            &events,
            &[0; 0x18],
            &[],
            None,
        )?;
        hud::bind_subjects(&mut converted.root, subjects);
        for (route, id) in events.routes.iter().zip(routes) {
            if !route.write_at(set, id) {
                let (group, name) = route.target();
                return Err(Error::Invalid(format!(
                    "pickup field on rebuilt {group} {name}"
                )));
            }
        }
        let name = format!("{}\\h1_pickup_text", set.scenario_path);
        let id = converted.install(set, &name)?;

        let scenario = set.scenario;
        let Some(TagPayload::Pinned { bytes, .. }) =
            set.tag_mut(scenario).map(|tag| &mut tag.payload)
        else {
            return Err(Error::Invalid(
                "expected pinned scenario for pickup text".into(),
            ));
        };
        let start = hud::SCENARIO_HMT as usize;
        let reference = bytes
            .get_mut(start..start + 8)
            .ok_or_else(|| Error::Invalid("scenario shorter than its HUD reference".into()))?;
        reference[..4].copy_from_slice(&hud::GROUP.as_u32().to_le_bytes());
        reference[4..].copy_from_slice(&id.0.to_le_bytes());
        Ok(())
    }
}
