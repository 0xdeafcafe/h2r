//! Familiar power-item placements, independent of which objects exist in
//! this match. Unknown/modded collections can still be learned by sight.
use crate::arms::Role;
use crate::decisions::supplies::Powerup;
use crate::places::PowerPad;
use map::{FourCC, Map, TagId};
use std::io::{Read, Seek};
use tags::geometry::TagView;

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn collection<R: Read + Seek>(
    map: &mut Map<R>,
    id: TagId,
) -> Option<(Option<Powerup>, Option<u32>, Role, i16)> {
    let tag = map.tag(id).ok()?;
    if tag.group != FourCC(*b"itmc") {
        return None;
    }
    let addr = tag.location?.address;
    let b = map.read_tag_bytes(id).ok()?;
    let view = TagView::new(&b, addr);
    let period = i16::from_le_bytes(view.at(8, 2).ok()?.try_into().ok()?);
    let mut models = Vec::new();
    let mut roles = Vec::new();
    let mut powerup: Option<Option<Powerup>> = None;
    for e in view.block(&b, 0, 0x10).ok()? {
        let object = map.tag(TagId(word(e, 8))).ok()?;
        let name = object
            .name
            .as_deref()?
            .rsplit(['\\', '/'])
            .next()?
            .to_ascii_lowercase()
            .replace(' ', "_");
        let (equipment, role) = match (&object.group.0, name.as_str()) {
            (b"weap", "shotgun") => (None, Role::Scatter),
            (b"weap", "energy_sword") => (None, Role::Blade),
            (b"weap", "sniper_rifle" | "beam_rifle") => (None, Role::Sniper),
            (b"weap", "rocket_launcher" | "fuel_rod_gun" | "brute_shot" | "flak_cannon") => {
                (None, Role::Blast)
            }
            (b"eqip", "powerup_invisibility" | "active_camouflage") => {
                (Some(Powerup::Camo), Role::Unknown)
            }
            (b"eqip", "powerup_super_shield" | "overshield" | "over_shield") => {
                (Some(Powerup::Overshield), Role::Unknown)
            }
            _ => return None,
        };
        roles.push(role);
        powerup = Some(match powerup {
            Some(previous) if previous != equipment => {
                if previous.is_none() || equipment.is_none() {
                    return None;
                }
                Some(Powerup::Mixed)
            }
            _ => equipment,
        });
        models.push(object.id.0);
    }
    let first = *models.first()?;
    Some((
        powerup?,
        models.iter().all(|&id| id == first).then_some(first),
        roles
            .first()
            .copied()
            .filter(|r| roles.iter().all(|p| p == r))
            .unwrap_or(Role::Unknown),
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
        .take(100)
        .enumerate()
    {
        let id = TagId(word(e, 0x5c));
        let Some((powerup, model, role, default)) =
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
            role,
            model,
        });
    }
    Ok(pads)
}
