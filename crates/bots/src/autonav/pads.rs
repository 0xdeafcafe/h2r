//! Familiar power-item placements, independent of which objects exist in
//! this match. Unknown/modded collections can still be learned by sight.
use crate::places::PowerPad;
use map::{FourCC, Map, TagId};
use std::io::{Read, Seek};
use tags::geometry::TagView;

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn collection<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Option<(bool, Option<u32>, i16)> {
    let tag = map.tag(id).ok()?;
    if tag.group != FourCC(*b"itmc") {
        return None;
    }
    let addr = tag.location?.address;
    let b = map.read_tag_bytes(id).ok()?;
    let view = TagView::new(&b, addr);
    let period = i16::from_le_bytes(view.at(8, 2).ok()?.try_into().ok()?);
    let mut models = Vec::new();
    let mut powerup = None;
    for e in view.block(&b, 0, 0x10).ok()? {
        let object = map.tag(TagId(word(e, 8))).ok()?;
        let name = object
            .name
            .as_deref()?
            .rsplit(['\\', '/'])
            .next()?
            .to_ascii_lowercase()
            .replace(' ', "_");
        let equipment = match (&object.group.0, name.as_str()) {
            (
                b"weap",
                "shotgun" | "energy_sword" | "sniper_rifle" | "beam_rifle" | "rocket_launcher"
                | "fuel_rod_gun" | "brute_shot" | "flak_cannon",
            ) => false,
            (
                b"eqip",
                "powerup_invisibility"
                | "powerup_super_shield"
                | "active_camouflage"
                | "overshield"
                | "over_shield",
            ) => true,
            _ => return None,
        };
        if powerup.is_some_and(|previous| equipment != previous) {
            return None;
        }
        powerup = Some(equipment);
        models.push(object.id.0);
    }
    let first = *models.first()?;
    Some((
        powerup?,
        models.iter().all(|&id| id == first).then_some(first),
        period,
    ))
}

pub(super) fn read<R: Read + Seek>(
    view: &TagView<'_>,
    map: &mut Map<R>,
) -> Result<Vec<PowerPad>, String> {
    let mut collections = std::collections::HashMap::new();
    let mut pads = Vec::new();
    for (site, e) in view
        .block(view.bytes(), 0x120, 0x90)
        .map_err(|e| e.to_string())?
        .into_iter()
        .take(128)
        .enumerate()
    {
        let id = TagId(word(e, 0x5c));
        let Some((powerup, model, default)) =
            *collections.entry(id).or_insert_with(|| collection(map, id))
        else {
            continue;
        };
        let at: [f32; 3] = std::array::from_fn(|i| f32::from_bits(word(e, 0x40 + i * 4)));
        if !at.iter().all(|v| v.is_finite()) {
            continue;
        }
        let seconds = i16::from_le_bytes(e[0x0e..0x10].try_into().unwrap());
        let seconds = if seconds == 0 { default } else { seconds };
        pads.push(PowerPad {
            site: site as u8,
            at,
            period: (seconds > 0).then(|| seconds as u32 * 30),
            powerup,
            model,
        });
    }
    Ok(pads)
}
