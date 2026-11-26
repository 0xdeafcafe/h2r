//! Firefight's guest half (docs/game/firefight.md, "Guest boundary"): the
//! AI switch, the kit read from the loaded scenario, and the engine entries
//! the host runtime applies at a tick boundary.
//!
//! Reads and writes take guest memory directly. An entry that calls the
//! engine is an `async` body over a [`Cx`] (`engine::entry`): it runs inside
//! a guest entry at the tick boundary, which makes each guest call. Every
//! call goes to the engine's own routine, named in `points.rs`; nothing
//! here writes an actor record.

use ::firefight::director::SQUAD_SLOTS;
use engine::entry::{Cx, EntryError, GuestCall, Regs};
use engine::{GuestMemory, MemFault, points};

use super::host::{AiGlobals, Kit};
use crate::actor_net::squads::Placement;

/// The two AI globals bytes `ai_update` (`0x1c83c0`) gates on
/// (docs/game/ai-networking.md, "Guest boundary"): `on` (globals `+0`, the
/// script's `ai` switch) and `initialised` (`+1`). `None` before the
/// globals exist.
pub fn ai_globals(m: &dyn GuestMemory) -> Result<Option<AiGlobals>, MemFault> {
    let g = m.u32(points::AI_GLOBALS.base)?;
    if g == 0 {
        return Ok(None);
    }
    Ok(Some(AiGlobals {
        on: m.u8(g)? != 0,
        initialised: m.u8(g.wrapping_add(1))? != 0,
    }))
}

/// Set the on byte, as the script's `ai` switch does (`0x2a4d31` stores its boolean
/// at `+0`). Writes nothing and returns false when the globals do not
/// exist yet or the byte is already set.
pub fn switch_ai_on(m: &mut dyn GuestMemory) -> Result<bool, MemFault> {
    let g = m.u32(points::AI_GLOBALS.base)?;
    if g == 0 || m.u8(g)? != 0 {
        return Ok(false);
    }
    m.write(g, &[1])?;
    Ok(true)
}

/// The loaded scenario's AI blocks: a `(count, address)` pair at the field
/// and the element size. The offsets are those the kit builder writes
/// (`h1campaign::ai::h2::layout`); the engine reads them at the addresses
/// given.
pub mod scenario {
    /// Squads: `0x201539` indexes `+0x164` by `0x74`.
    pub const SQUADS: (u32, u32) = (0x160, 0x74);
    /// Zones: `0x204fbc` indexes `+0x16c` by `0x38`.
    pub const ZONES: (u32, u32) = (0x168, 0x38);
    /// Character palette, a `char` reference each: `0x2037d9` bounds the
    /// index by `+0x178` and reads the datum at `+0x17c` `+ 8i + 4`.
    pub const CHARACTER_PALETTE: (u32, u32) = (0x178, 0x8);
    /// Orders: `0x275cdd` bounds the order by `+0x240`.
    pub const ORDERS: (u32, u32) = (0x240, 0x7c);

    /// Squad and zone and order names: 32 characters at `+0`.
    pub const NAME: u32 = 0x00;
    pub const NAME_LEN: usize = 32;
    /// Squad: team, `u16`.
    pub const SQUAD_TEAM: u32 = 0x24;
    /// Squad: character palette index, `i16`, read per actor at
    /// `0x2037cc` when the start names none.
    pub const SQUAD_CHARACTER: u32 = 0x36;
    /// Squad: zone, `i16`, the squad's areas while it has no order
    /// (`0x204f93`).
    pub const SQUAD_ZONE: u32 = 0x38;
    /// Squad: starting locations (`0x2016c7`).
    pub const SQUAD_STARTS: (u32, u32) = (0x48, 0x64);
    /// Starting location: position, then the reference frame `i16`.
    pub const START_POSITION: u32 = 0x04;
    pub const START_REFERENCE_FRAME: u32 = 0x10;
    /// Zone: firing positions and areas.
    pub const ZONE_FIRING_POSITIONS: (u32, u32) = (0x28, 0x20);
    pub const ZONE_AREAS: (u32, u32) = (0x30, 0x88);
    /// Firing position: position, then the reference frame `i16`.
    pub const FIRING_POSITION: u32 = 0x00;
    pub const FIRING_REFERENCE_FRAME: u32 = 0x0c;
    /// Area: the mean of its firing positions.
    pub const AREA_MEAN: u32 = 0x24;
}

/// The kit's squad and zone names (`crates/tools/firefight-kit/src/ai.rs`
/// and `nav.rs`): `ff_slot_00` to `ff_slot_15`, drop zones `ff_drop_00`
/// up, and one zone per base, `ff_base_00` (the home base) and
/// `ff_base_01`.
const SLOT_PREFIX: &str = "ff_slot_";
const ZONE_PREFIX: &str = "ff_drop_";
const BASE_PREFIX: &str = "ff_base_";
/// The orders a kit authors for `Hunt`: `ff_hunt_NN` sends a squad at
/// base NN.
pub const HUNT_PREFIX: &str = "ff_hunt_";

/// The name of the order that hunts at `base`.
pub fn hunt_order_name(base: u8) -> String {
    format!("{HUNT_PREFIX}{base:02}")
}
/// The kit's record of the classes it built: a squad `ff_class_<name>`
/// per character palette row, whose character is that row
/// (`firefight-kit`, `ai::class_squads`). The Elite is row 0 and each
/// transplanted class follows in the order the kit kept them.
const CLASS_PREFIX: &str = "ff_class_";

/// One kit zone as the scenario holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct KitZone {
    /// The scenario zone index.
    pub index: u16,
    /// Its first area's mean: where the director measures from.
    pub point: [f32; 3],
    /// Its firing positions and their reference frames: where a squad
    /// placed into the zone starts.
    pub starts: Vec<([f32; 3], u16)>,
}

/// The kit on the loaded map, read from the scenario's AI blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct KitLayout {
    /// Each slot's scenario squad index.
    pub squads: [Option<u16>; SQUAD_SLOTS],
    /// The drop zones, `ff_drop_NN`, in scenario order.
    pub zones: Vec<KitZone>,
    /// The base zones by base, `ff_base_00` first: the run of base
    /// ordinals from 0 the scenario holds.
    pub bases: Vec<KitZone>,
    /// Rows in the character palette.
    pub characters: u16,
    /// The classes the kit built and each one's palette row, from its
    /// `ff_class_` squads in scenario order. A row outside the palette
    /// is not a class; a name recorded twice keeps its first row.
    pub classes: Vec<(String, u16)>,
    /// The team of the first slot's squad.
    pub team: u8,
    /// Each base's hunt order, `ff_hunt_NN`, by base: the scenario
    /// order index, none where the scenario has no order of that name.
    pub hunt_orders: Vec<Option<u16>>,
}

impl KitLayout {
    /// What the host runtime needs of it.
    pub fn kit(&self) -> Kit {
        Kit {
            zones: self.zones.iter().map(|z| z.point).collect(),
            bases: self.bases.iter().map(|z| z.point).collect(),
            classes: self.classes.iter().cloned().collect(),
            team: self.team,
        }
    }
}

/// A block's elements: `(address, count)`.
fn block(m: &dyn GuestMemory, owner: u32, (field, size): (u32, u32)) -> Result<Vec<u32>, MemFault> {
    let count = m.u32(owner + field)?;
    let at = m.u32(owner + field + 4)?;
    if at == 0 {
        return Ok(Vec::new());
    }
    // Tag blocks hold at most 32767 elements; more is not a block.
    Ok((0..count.min(0x7fff))
        .map(|i| at.wrapping_add(i.wrapping_mul(size)))
        .collect())
}

fn name(m: &dyn GuestMemory, at: u32) -> Result<String, MemFault> {
    let bytes = m.bytes(at + scenario::NAME, scenario::NAME_LEN)?;
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

/// The numbered suffix of a kit name, as in `ff_slot_07`.
fn ordinal(name: &str, prefix: &str) -> Option<usize> {
    let digits = name.strip_prefix(prefix)?;
    if digits.len() != 2 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The kit, or none on a map whose scenario has no `ff_slot_` squad. The
/// `firefight_kit_vN` string ID the builder interns is not checked: the
/// game has no reader for the loaded map's string IDs. A base zone's point
/// is its first area's mean, as a drop zone's is.
pub fn read_kit(m: &dyn GuestMemory) -> Result<Option<KitLayout>, MemFault> {
    let scnr = m.u32(points::SCENARIO.base)?;
    if scnr == 0 {
        return Ok(None);
    }
    let characters = m
        .u32(scnr + scenario::CHARACTER_PALETTE.0)?
        .min(u16::MAX.into()) as u16;
    let mut squads = [None; SQUAD_SLOTS];
    let mut team = None;
    let mut classes: Vec<(String, u16)> = Vec::new();
    for (index, at) in block(m, scnr, scenario::SQUADS)?.into_iter().enumerate() {
        let squad_name = name(m, at)?;
        if let Some(class) = squad_name.strip_prefix(CLASS_PREFIX) {
            let row = m.u16(at + scenario::SQUAD_CHARACTER)?;
            if !class.is_empty()
                && row < characters
                && !classes.iter().any(|(known, _)| known == class)
            {
                classes.push((class.to_string(), row));
            }
            continue;
        }
        let Some(slot) = ordinal(&squad_name, SLOT_PREFIX).filter(|&s| s < SQUAD_SLOTS) else {
            continue;
        };
        squads[slot] = Some(index as u16);
        if slot == 0 || team.is_none() {
            team = Some(m.u16(at + scenario::SQUAD_TEAM)? as u8);
        }
    }
    let Some(team) = team else {
        return Ok(None);
    };
    let mut zones = Vec::new();
    let mut bases = std::collections::BTreeMap::new();
    for (index, at) in block(m, scnr, scenario::ZONES)?.into_iter().enumerate() {
        let zone_name = name(m, at)?;
        let base = ordinal(&zone_name, BASE_PREFIX);
        if ordinal(&zone_name, ZONE_PREFIX).is_none() && base.is_none() {
            continue;
        }
        let Some(&area) = block(m, at, scenario::ZONE_AREAS)?.first() else {
            continue;
        };
        let starts = block(m, at, scenario::ZONE_FIRING_POSITIONS)?
            .into_iter()
            .map(|fp| {
                Ok((
                    m.vec3(fp + scenario::FIRING_POSITION)?,
                    m.u16(fp + scenario::FIRING_REFERENCE_FRAME)?,
                ))
            })
            .collect::<Result<_, MemFault>>()?;
        let zone = KitZone {
            index: index as u16,
            point: m.vec3(area + scenario::AREA_MEAN)?,
            starts,
        };
        match base {
            Some(n) => {
                bases.entry(n).or_insert(zone);
            }
            None => zones.push(zone),
        }
    }
    let bases = bases
        .into_iter()
        .enumerate()
        .take_while(|(i, (n, _))| i == n)
        .map(|(_, (_, zone))| zone)
        .collect();
    let mut hunt_orders: Vec<Option<u16>> = Vec::new();
    for (index, at) in block(m, scnr, scenario::ORDERS)?.into_iter().enumerate() {
        let Some(base) = ordinal(&name(m, at)?, HUNT_PREFIX) else {
            continue;
        };
        if hunt_orders.len() <= base {
            hunt_orders.resize(base + 1, None);
        }
        hunt_orders[base].get_or_insert(index as u16);
    }
    Ok(Some(KitLayout {
        squads,
        zones,
        bases,
        characters,
        classes,
        team,
        hunt_orders,
    }))
}

fn guest(e: impl Into<EntryError>) -> anyhow::Error {
    anyhow::anyhow!("{}", e.into())
}

async fn call(cx: &Cx, c: GuestCall) -> anyhow::Result<Regs> {
    cx.call(c).await.map_err(guest)
}

fn squad_index(kit: &KitLayout, slot: u8) -> anyhow::Result<u16> {
    kit.squads
        .get(usize::from(slot))
        .copied()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("the kit reserves no squad for slot {slot}"))
}

/// Place a squad: set the slot squad's character, zone and starts in the
/// scenario, place `count` actors through the engine's squad placement as
/// the counted `ai_place` does, then set their team. The scenario writes
/// are the same on every machine that applies the entry. The placement's
/// weapon tier is not applied: the stage-one kit authors one weapon.
pub async fn place_squad(cx: &Cx, kit: &KitLayout, p: Placement) -> anyhow::Result<()> {
    let squad = squad_index(kit, p.slot)?;
    let zone = kit
        .zones
        .get(usize::from(p.zone))
        .ok_or_else(|| anyhow::anyhow!("the kit has no zone {}", p.zone))?;
    anyhow::ensure!(
        !zone.starts.is_empty(),
        "kit zone {} has no firing positions",
        p.zone
    );
    anyhow::ensure!(
        p.character < kit.characters,
        "character {} is outside the map's {} palette rows",
        p.character,
        kit.characters
    );
    cx.write(|m| {
        let scnr = m.u32(points::SCENARIO.base)?;
        let at = *block(m, scnr, scenario::SQUADS)?
            .get(usize::from(squad))
            .ok_or(MemFault {
                addr: scnr + scenario::SQUADS.0,
                len: 4,
                write: false,
            })?;
        m.set_u16(at + scenario::SQUAD_CHARACTER, p.character)?;
        m.set_u16(at + scenario::SQUAD_ZONE, zone.index)?;
        for (i, start) in block(m, at, scenario::SQUAD_STARTS)?
            .into_iter()
            .enumerate()
        {
            let (position, frame) = zone.starts[i % zone.starts.len()];
            for (k, v) in position.into_iter().enumerate() {
                m.set_f32(start + scenario::START_POSITION + 4 * k as u32, v)?;
            }
            m.set_u16(start + scenario::START_REFERENCE_FRAME, frame)?;
        }
        Ok::<_, MemFault>(())
    })
    .map_err(guest)?;
    // Any start (-1), not forced, spawn flag 1: the arguments the counted
    // evaluator passes at 0x2a5052.
    call(
        cx,
        GuestCall::new(points::AI_SQUAD_PLACE.base)
            .eax(u32::from(p.count))
            .args([u32::from(squad), u32::MAX, 0, 1]),
    )
    .await?;
    call(
        cx,
        GuestCall::new(points::AI_SET_TEAM.base)
            .eax(u32::from(squad))
            .ebx(u32::from(p.team)),
    )
    .await?;
    Ok(())
}

/// Send a slot's squad at base `base` (0 the home base) through the kit's
/// hunt order for it, `ff_hunt_NN`. A kit without that order is an error
/// naming it: no other order stands in.
pub async fn hunt(cx: &Cx, kit: &KitLayout, slot: u8, base: u8) -> anyhow::Result<()> {
    let squad = squad_index(kit, slot)?;
    let order = kit
        .hunt_orders
        .get(usize::from(base))
        .copied()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("the kit has no {} order", hunt_order_name(base)))?;
    call(
        cx,
        GuestCall::new(points::AI_SET_ORDERS.base)
            .eax(u32::from(squad))
            .ecx(u32::from(order)),
    )
    .await?;
    Ok(())
}

/// Remove a slot's squad: its actors, their units and what it placed.
pub async fn erase_squad(cx: &Cx, kit: &KitLayout, slot: u8) -> anyhow::Result<()> {
    let squad = squad_index(kit, slot)?;
    call(
        cx,
        GuestCall::new(points::AI_ERASE.base).args([u32::from(squad)]),
    )
    .await?;
    Ok(())
}

/// The players array's record size and its data pointer
/// (docs/live/live-gamestate.md, section 4).
const PLAYER_SIZE: u32 = 0x21c;
const PLAYERS_DATA: u32 = 0x44;
/// Player: lives remaining, `i16`, -1 unlimited.
pub const PLAYER_LIVES: u32 = 0x1ac;
/// The player entity's replication bit for lives (`0x15d03d`, `0x24e257`).
pub const LIVES_DIRTY: u32 = 0x20;
/// Game globals: the variant's lives setting (`0x15db37`), the mode byte
/// the engine's game end skips (`0x15b5f9`), and the end latch.
pub const GLOBALS_LIVES: u32 = 0x1b8;
pub const GLOBALS_MODE: u32 = 0x0c;
pub const GLOBALS_MODE_NO_END: u8 = 4;
pub const GLOBALS_ENDED: u32 = 0x1128;
/// Game engine globals: flags (bit `0x20` set after a score or round
/// change, `0x15b911`) and the engine state (1 in progress, 2 over,
/// `0x15b5fe`).
pub const ENGINE_FLAGS: u32 = 0x00;
pub const ENGINE_CHANGED: u32 = 0x20;
pub const ENGINE_STATE: u32 = 0xc04;
pub const ENGINE_OVER: u32 = 2;
/// The update helpers' clamp.
pub const SCORE_MAX: u32 = 30_000;

/// The salted player datum of an absolute player slot, or none.
pub fn player_datum(m: &dyn GuestMemory, slot: u8) -> Result<Option<u32>, MemFault> {
    let players = m.u32(points::PLAYERS.base)?;
    if players == 0 || slot >= 16 {
        return Ok(None);
    }
    let at = m.u32(players + PLAYERS_DATA)? + u32::from(slot) * PLAYER_SIZE;
    let salt = m.u16(at)?;
    Ok((salt != 0).then_some(u32::from(salt) << 16 | u32::from(slot)))
}

/// The player record of an absolute player slot (the rules runtime reads
/// lives there too).
pub fn player_record(m: &dyn GuestMemory, slot: u8) -> Result<u32, MemFault> {
    let players = m.u32(points::PLAYERS.base)?;
    Ok(m.u32(players + PLAYERS_DATA)? + u32::from(slot) * PLAYER_SIZE)
}

/// Add `points` to the player's score through the engine's score entry,
/// which also updates the team score and the statborg's replication. An
/// engine that takes no score (no game engine, a game not in progress, a
/// machine that is not the authority) is an error.
pub async fn credit_score(cx: &Cx, slot: u8, points: u32) -> anyhow::Result<()> {
    anyhow::ensure!(
        points <= SCORE_MAX,
        "{points} points is over the engine's {SCORE_MAX}"
    );
    let datum = cx
        .read(|m| player_datum(m, slot))
        .map_err(guest)?
        .ok_or_else(|| anyhow::anyhow!("no player in slot {slot}"))?;
    let r = call(
        cx,
        GuestCall::new(points::SCORE_ADD.base)
            .eax(points)
            .args([datum]),
    )
    .await?;
    anyhow::ensure!(r.eax as u8 != 0, "the engine took no score for slot {slot}");
    Ok(())
}

/// The engine holds a dead player while the variant's lives setting is
/// positive and the player's lives are 0 (`PLAYER_OUT_OF_LIVES`). Firefight
/// runs with the setting at 1: a death takes the player to 0, and only
/// [`respawn`] lets it back.
pub fn engine_holds_respawns(m: &dyn GuestMemory) -> Result<bool, MemFault> {
    let g = m.u32(points::GAME_GLOBALS.base)?;
    Ok(g != 0 && m.i32(g + GLOBALS_LIVES)? > 0)
}

/// Let one held player respawn: one life, marked for replication; the
/// engine spawns the player when its respawn delay runs out and takes the
/// life again at the next death. Refused while the engine does not hold
/// respawns, since it would respawn the player by itself.
pub async fn respawn(cx: &Cx, slot: u8) -> anyhow::Result<()> {
    anyhow::ensure!(
        cx.read(engine_holds_respawns).map_err(guest)?,
        "the variant's lives are unlimited, so the engine respawns players itself"
    );
    let datum = cx
        .read(|m| player_datum(m, slot))
        .map_err(guest)?
        .ok_or_else(|| anyhow::anyhow!("no player in slot {slot}"))?;
    cx.write(|m| {
        let at = player_record(m, slot)?;
        m.set_u16(at + PLAYER_LIVES, 1)
    })
    .map_err(guest)?;
    call(
        cx,
        GuestCall::new(points::PLAYER_ENTITY_DIRTY.base).args([datum, LIVES_DIRTY]),
    )
    .await?;
    Ok(())
}

/// End the game as the engine's round-over path does at its end
/// (`0x15b5f9` to `0x15b624`): the engine state to over, the game end
/// latch through `GAME_END` once, then the engine's changed flag.
/// Refused before the globals exist and in the mode the engine never ends
/// a game from.
pub async fn end_game(cx: &Cx) -> anyhow::Result<()> {
    let (e, latched) = cx
        .read(|m| {
            let g = m.u32(points::GAME_GLOBALS.base)?;
            let e = m.u32(points::GAME_ENGINE.base)?;
            if g == 0 || e == 0 {
                return Ok(None);
            }
            if m.u8(g + GLOBALS_MODE)? == GLOBALS_MODE_NO_END {
                return Ok(Some((e, None)));
            }
            Ok::<_, MemFault>(Some((e, Some(m.u8(g + GLOBALS_ENDED)? != 0))))
        })
        .map_err(guest)?
        .ok_or_else(|| anyhow::anyhow!("no game to end"))?;
    let latched = latched.ok_or_else(|| {
        anyhow::anyhow!(
            "game globals +{GLOBALS_MODE:#x} is {GLOBALS_MODE_NO_END}: the engine does not end games there"
        )
    })?;
    cx.write(|m| m.set_u32(e + ENGINE_STATE, ENGINE_OVER))
        .map_err(guest)?;
    if !latched {
        call(cx, GuestCall::new(points::GAME_END.base)).await?;
    }
    cx.write(|m| {
        let flags = m.u32(e + ENGINE_FLAGS)?;
        m.set_u32(e + ENGINE_FLAGS, flags | ENGINE_CHANGED)
    })
    .map_err(guest)?;
    Ok(())
}

/// Whether the AI's clear-line test passes from `from` to `to`, ignoring
/// the two `ignore` datums (`u32::MAX` for an unused slot). `scratch` is 24 bytes of
/// guest memory the entry owns, for the two points.
pub async fn line_of_sight(
    cx: &Cx,
    scratch: u32,
    from: [f32; 3],
    to: [f32; 3],
    ignore: [u32; 2],
) -> anyhow::Result<bool> {
    cx.write(|m| {
        for (k, v) in from.into_iter().chain(to).enumerate() {
            m.set_f32(scratch + 4 * k as u32, v)?;
        }
        Ok::<_, MemFault>(())
    })
    .map_err(guest)?;
    let r = call(
        cx,
        GuestCall::new(points::AI_LINE_OF_SIGHT.base)
            .esi(scratch)
            .edi(scratch + 12)
            .edx(ignore[0])
            .ecx(ignore[1]),
    )
    .await?;
    Ok(r.eax as u8 != 0)
}

#[cfg(test)]
mod tests;
