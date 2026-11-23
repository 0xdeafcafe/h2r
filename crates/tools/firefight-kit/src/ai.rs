//! Author scenario AI against the multiplayer map's own tag index.
use std::io::{Read, Seek};

use h1campaign::ai::convert::{ScenarioBlocks, grenade};
use h1campaign::ai::h2::{AreaRef, Order, ScenarioAi, Squad, StartingLocation, Zone, layout};
use h1campaign::scripts::compile::{Compiled, add_startup_squad, field};
use map::build::Block;
use map::{FourCC, Map, TagId, TagQuery};

mod character;
#[cfg(test)]
mod tests;

pub(crate) const ELITE_PATH: &str = r"firefight\characters\elite";
const ELITE_UNIT: &str = r"objects\characters\elite\elite_mp";
const WEAPONS: &[&str] = &[
    r"objects\weapons\rifle\plasma_rifle\plasma_rifle",
    r"objects\weapons\pistol\needler\needler",
    r"objects\weapons\pistol\plasma_pistol\plasma_pistol",
    r"objects\weapons\rifle\battle_rifle\battle_rifle",
];
const PROBE_SCRIPT: &str = "ff_probe_place";
const PROBE_SQUAD: &str = "ff_probe_elites";
const SQUAD_SLOTS: usize = 16;
const COVENANT_TEAM: u16 = 3;
/// The orders the guest's `hunt` entry finds by name (`AI_SET_ORDERS`,
/// 0x275cb0): `ff_hunt_NN` sends a squad at base NN.
pub(crate) const HUNT_PREFIX: &str = "ff_hunt_";

pub(crate) fn hunt_name(base: usize) -> String {
    format!("{HUNT_PREFIX}{base:02}")
}

pub(crate) struct AiBlocks {
    pub character: Block,
    pub fields: ScenarioBlocks,
}

pub(crate) fn prepare<R: Read + Seek>(
    map: &mut Map<R>,
    drops: Vec<Zone>,
    bases: Vec<Zone>,
    elite: TagId,
    probe: bool,
) -> Result<AiBlocks, String> {
    let location = map
        .scenario()
        .and_then(|tag| tag.location)
        .ok_or("missing scenario data")?;
    if (location.size as usize) < layout::SCENARIO_MIN {
        return Err("truncated scenario".into());
    }
    let at = location.file_offset().map_err(|e| e.to_string())?;
    let root = map
        .read_file_range(at, layout::SCENARIO_MIN as u64)
        .map_err(|e| e.to_string())?;
    for (field, _) in [
        layout::CHARACTER_PALETTE,
        layout::SQUADS,
        layout::ZONES,
        layout::ORDERS,
    ] {
        if root[field..field + 4] != [0; 4] {
            return Err(
                "the multiplayer base already has characters, squads, zones or orders".into(),
            );
        }
    }

    let unit = map
        .tag(TagQuery::NameInGroup(ELITE_UNIT, FourCC(*b"bipd")))
        .map_err(|e| format!("required Elite biped {ELITE_UNIT}: {e}"))?
        .id;
    let weapon = WEAPONS
        .iter()
        .find_map(|path| map.tag(TagQuery::NameInGroup(path, FourCC(*b"weap"))).ok())
        .ok_or("the base has no supported Elite weapon (plasma rifle, needler, plasma pistol or battle rifle)")?
        .id;
    let character = character::elite(unit, weapon).map_err(|e| format!("author Elite: {e}"))?;
    let mut fields = ScenarioBlocks { fields: Vec::new() };
    let weapon_index = weapon_slot(map, &root, weapon, &mut fields)?;
    let squads = squads(&drops, weapon_index, probe)?;
    // The base zones follow the drop zones, so a squad's zone index is a
    // drop zone and base N's zone is `drops.len() + N`.
    let base_zones: Vec<usize> = (drops.len()..drops.len() + bases.len()).collect();
    let mut zones = drops;
    zones.extend(bases);
    let orders = hunt_orders(&zones, &base_zones)?;
    let ai = ScenarioAi {
        orders,
        zones,
        squads,
        character_palette: vec![elite],
        ..Default::default()
    };
    // Preserve the map's scripting data (including point sets) and squad
    // groups. This kit owns four scenario AI fields; the orders block is
    // empty in the base (checked above), so `ff_hunt_NN` is order NN.
    fields
        .fields
        .extend(ai.blocks().fields.into_iter().filter(|(field, _, _)| {
            [
                layout::CHARACTER_PALETTE.0,
                layout::SQUADS.0,
                layout::ZONES.0,
                layout::ORDERS.0,
            ]
            .contains(&(*field as usize))
        }));
    if probe {
        probe_startup(map, &mut fields)?;
    }
    Ok(AiBlocks { character, fields })
}

/// A squad refers to a weapon by palette index, its char by tag datum.
/// Keep every byte and index of the base palette if the weapon needs a new row.
fn weapon_slot<R: Read + Seek>(
    map: &mut Map<R>,
    root: &[u8],
    weapon: TagId,
    fields: &mut ScenarioBlocks,
) -> Result<i16, String> {
    use h1campaign::scenario::PALETTE;
    let (_, field, _) = tags::object::OBJECT_TYPES[2]
        .placement
        .expect("weapon palette");
    let rows = scenario_edit::scene::metadata_block(map, root, usize::from(field), PALETTE)
        .map_err(|e| format!("scenario weapon palette: {e}"))?;
    if let Some(i) = rows
        .iter()
        .position(|row| row[4..8] == weapon.0.to_le_bytes())
    {
        return i16::try_from(i).map_err(|_| "weapon palette index overflow".into());
    }
    let index = i16::try_from(rows.len()).map_err(|_| "weapon palette index overflow")?;
    let mut bytes = rows.concat();
    let at = bytes.len();
    bytes.resize(at + PALETTE, 0);
    let mut palette = Block::new(bytes);
    palette.tag_ref(at as u32, weapon);
    fields
        .fields
        .push((u32::from(field), index as u32 + 1, palette));
    Ok(index)
}

/// The Hunt orders, one per base: `ff_hunt_NN` engages from every area of
/// base NN's zone (`bases[NN]`, a zone index), so a squad sent to it
/// pushes into that base. No style, script or follow target; the guest
/// finds each by its name.
fn hunt_orders(zones: &[Zone], bases: &[usize]) -> Result<Vec<Order>, String> {
    if bases.is_empty() {
        return Err("no base zones for the Hunt orders".into());
    }
    bases
        .iter()
        .enumerate()
        .map(|(base, &zone)| {
            let name = hunt_name(base);
            let areas = zones.get(zone).map_or(0, |z| z.areas.len());
            if areas == 0 {
                return Err(format!(
                    "base {base} has no zone areas for the Hunt order {name}"
                ));
            }
            Ok(Order {
                name,
                style: -1,
                flags: 0,
                force_combat_status: 0,
                entry_script: String::new(),
                entry_script_index: -1,
                follow_squad: -1,
                follow_radius: 0.0,
                primary_areas: (0..areas)
                    .map(|area| AreaRef {
                        kind: 0,
                        zone: zone as i16,
                        area: area as i16,
                    })
                    .collect(),
                secondary_areas: Vec::new(),
                endings: 0,
            })
        })
        .collect()
}

fn squads(zones: &[Zone], weapon: i16, probe: bool) -> Result<Vec<Squad>, String> {
    if zones.is_empty() {
        return Err("no drop zones for the Firefight squads".into());
    }
    let slots = if probe { 1 } else { SQUAD_SLOTS };
    let mut squads = Vec::with_capacity(slots);
    for slot in 0..slots {
        let zone = slot % zones.len();
        let points = zones[zone]
            .firing_positions
            .get(..2)
            .ok_or("a drop zone needs two starts")?;
        let count = if probe { 2 } else { 0 };
        squads.push(Squad {
            name: if probe {
                PROBE_SQUAD.into()
            } else {
                format!("ff_slot_{slot:02}")
            },
            flags: 0,
            team: COVENANT_TEAM,
            parent: -1,
            delay: 0.0,
            normal_count: count,
            insane_count: count,
            major_upgrade: 0,
            character: if probe { 0 } else { -1 },
            initial_zone: if probe { zone as i16 } else { -1 },
            initial_weapon: weapon,
            initial_secondary_weapon: -1,
            vehicle: -1,
            initial_order: -1,
            grenade_type: grenade::COVENANT,
            starting_locations: points
                .iter()
                .map(|point| StartingLocation {
                    name: 0,
                    position: point.position,
                    reference_frame: point.reference_frame,
                    facing: [0.0; 2],
                    flags: 0,
                    character: -1,
                    initial_weapon: -1,
                    initial_secondary_weapon: -1,
                    vehicle: -1,
                    seat_type: 0,
                    grenade_type: 0,
                    swarm_count: 0,
                    emitter_vehicle: -1,
                    placement_script: String::new(),
                    placement_script_index: -1,
                })
                .collect(),
        });
    }
    Ok(squads)
}

fn probe_startup<R: Read + Seek>(
    map: &mut Map<R>,
    fields: &mut ScenarioBlocks,
) -> Result<(), String> {
    let original = hs::Scenario::read(map).map_err(|e| e.to_string())?;
    if original
        .scripts()
        .iter()
        .any(|s| s.name().ok() == Some(PROBE_SCRIPT))
    {
        return Err(format!("a script named {PROBE_SCRIPT} already exists"));
    }
    let mut code = Compiled {
        scripts: original.scripts().iter().map(hs::Script::encode).collect(),
        nodes: original.nodes().to_vec(),
        strings: original.strings().to_vec(),
        ..Compiled::default()
    };
    add_startup_squad(&mut code, PROBE_SCRIPT, PROBE_SQUAD, 0).map_err(|e| e.to_string())?;
    fields.fields.extend([
        (
            field::SCRIPTS,
            code.scripts.len() as u32,
            Block::new(code.scripts.concat()),
        ),
        (
            field::NODES,
            code.nodes.len() as u32,
            Block::new(code.nodes.iter().flat_map(|n| n.encode()).collect()),
        ),
        (
            field::STRINGS,
            code.strings.len() as u32,
            Block::new(code.strings),
        ),
    ]);
    Ok(())
}
