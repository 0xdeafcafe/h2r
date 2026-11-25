//! The host's view of the map's labels (docs/game/rules-start-gate.md, "The
//! check"; docs/game/rules-engine.md, "Forge labels"): the census the start
//! gate checks, and the labelled things a running script reads.
//!
//! [`rows`] reads the loaded scenario in guest memory: each block's count,
//! the netgame flags' type and team (`scnr+0x118`) and the starting
//! locations' team (`scnr+0x100`). `forge_doc::labelled` turns them into
//! labelled things: each flag with its type's built-in label, and what the
//! session's Forge document labels. At a game's start the session has no
//! document (a Forge session loads its draft at its first tick,
//! `forge_glue`), so the host's census is the flags' built-in labels. The
//! game holds no tag paths (they are only in the map file), so its census
//! has no tags. The one entry a host makes that takes a tag,
//! `set_loadout`, names it by the multiplayer weapon list's value, which
//! the game maps to its own tag (`super::world::loadout`).
//!
//! [`Table`] is what the script reads: each labelled thing with its handle,
//! its placement id, its number and its team designator. A handle is
//! [`HANDLE`] with the placement id: a netgame flag has no object datum,
//! and the VM's object registers hold only values below `2^31`, which no
//! salted datum is (salts have bit 15 set, docs/engine/datum_new.md).

use ::rules::ir::Ir;
use ::rules::labels::{self, Census, Class};
use ::rules::vm::Placed;
use agent_proto::rules as wire;
use engine::objectives::{self as objs, check_span};
use engine::{GuestMemory, MemFault, points};
use forge_doc::{BlockKind, CENSUS_ENTRY_LIMIT, CENSUS_FLAG_LIMIT, FlagRow, LabelledThing, MapRows};

/// The bit every labelled thing's handle carries.
pub const HANDLE: u32 = 0x4000_0000;
/// Engine indices whose objective records' `+4` is a game team (CTF and
/// Assault); Oddball's holds a ball number, Territories' a territory.
pub const TEAM_OBJECTIVES: &[i32] = &[1, 9];

/// The loaded scenario's rows, as a census reads them. No scenario reads
/// as an empty map.
pub fn rows(m: &dyn GuestMemory) -> Result<MapRows, MemFault> {
    let s = m.u32(points::SCENARIO.base)?;
    let mut rows = MapRows::default();
    if s == 0 {
        return Ok(rows);
    }
    let blocks = map::scenario_layout::BLOCKS;
    check_span(s, blocks.iter().map(|d| d.offset + 8).max().unwrap_or(0))?;
    let count = |offset: u32, limit: u32| -> Result<u32, MemFault> {
        Ok((m.i32(s + offset)?.max(0) as u32).min(limit))
    };
    for def in blocks {
        rows.counts.insert(def.kind, count(def.offset, CENSUS_ENTRY_LIMIT)?);
    }
    let flags = map::scenario_layout::block(BlockKind::NetgameFlags);
    let n = count(flags.offset, CENSUS_FLAG_LIMIT)?;
    let at = m.u32(s + flags.offset + 4)?;
    check_span(at, n * flags.stride)?;
    for i in 0..n {
        let f = at + i * flags.stride;
        rows.flags.push(FlagRow {
            kind: m.u16(f + objs::FLAG_TYPE)?,
            team: m.u16(f + objs::FLAG_TEAM)?,
        });
    }
    let starts = map::scenario_layout::block(BlockKind::StartingLocations);
    let n = count(starts.offset, CENSUS_ENTRY_LIMIT)?;
    let at = m.u32(s + starts.offset + 4)?;
    check_span(at, n * starts.stride)?;
    for i in 0..n {
        rows.starts.push(m.u16(at + i * starts.stride + START_TEAM)? as i16);
    }
    rows.counts.insert(BlockKind::NetgameFlags, rows.flags.len() as u32);
    rows.counts
        .insert(BlockKind::StartingLocations, rows.starts.len() as u32);
    Ok(rows)
}

/// In a starting location: its team (`i16`; `scenario_layout`'s `team`).
pub const START_TEAM: u32 = 0x10;

/// The labelled things of the loaded game, by [`rows`] and no document.
pub fn things(m: &dyn GuestMemory) -> Result<Vec<LabelledThing>, String> {
    let rows = rows(m).map_err(|e| format!("guest memory at {:#x} faulted", e.addr))?;
    let mut things = forge_doc::labelled(&rows, None).map_err(|e| e.to_string())?;
    // The areas a Studio map was saved with (`crate::map_areas`).
    let saved = crate::map_areas::current();
    if !saved.is_empty()
        && let Ok(flags) = crate::map_areas::flags(m)
    {
        crate::map_areas::attach(&mut things, &flags, &saved);
    }
    Ok(things)
}

/// The census of `things`, with no tags (the game holds no tag paths).
pub fn census(things: &[LabelledThing]) -> Census {
    forge_doc::census(things, &Default::default())
}

/// One labelled thing a script can read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thing {
    pub text: String,
    /// The index of the script's label with this text, if it declares one.
    pub label: Option<u16>,
    pub number: Option<i32>,
    pub class: Class,
    /// The placement's team designator, 0 to 7.
    pub designator: Option<u8>,
    pub block: BlockKind,
    pub index: u32,
    /// The area on a netgame flag, which the flag's zone takes
    /// (`super::world::zones`).
    pub area: Option<forge_doc::Area>,
}

impl Thing {
    /// The placement id `zone.id` reads, and the order a set visits in:
    /// the block's number in the high half, the final index in the low.
    pub fn id(&self) -> u32 {
        (self.block as u32) << 16 | self.index
    }

    /// The object handle the script holds.
    pub fn handle(&self) -> u32 {
        HANDLE | self.id()
    }
}

/// The labelled things of the game the script runs in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Table {
    things: Vec<Thing>,
}

impl Table {
    pub fn new(things: &[LabelledThing], ir: &Ir) -> Table {
        let mut t = Table::of_census(things);
        t.relabel(ir);
        t
    }

    /// A table of the census's things with no script's labels: what a
    /// machine that does not run the script reads zones from
    /// (`super::world::walls::client`).
    pub fn of_census(things: &[LabelledThing]) -> Table {
        Table {
            things: things
                .iter()
                .map(|l| Thing {
                    text: l.label.text.clone(),
                    label: None,
                    number: l.label.number,
                    class: l.class,
                    designator: l.team,
                    block: l.block,
                    index: l.index,
                    area: l.area.as_ref().and_then(|a| forge_doc::Area::try_from(a).ok()),
                })
                .collect(),
        }
    }

    /// A table of `things` as they are, for tests of what reads it.
    #[cfg(test)]
    pub fn from_things(things: Vec<Thing>) -> Table {
        Table { things }
    }

    /// Points each thing at `ir`'s label of its text, after a swap.
    pub fn relabel(&mut self, ir: &Ir) {
        // A label reads another text's things when the map places none of
        // its own (`labels::reads`), and those things are its members.
        let reads: Vec<String> = ir
            .labels
            .iter()
            .map(|d| labels::reads(&d.text, |x| self.things.iter().any(|t| t.text == x)).to_string())
            .collect();
        for t in &mut self.things {
            t.label = reads
                .iter()
                .position(|r| *r == t.text)
                .and_then(|i| u16::try_from(i).ok());
        }
    }

    /// The members of `objects labelled "<text>"`: every thing with the
    /// text but a starting location, which has nothing to visit.
    pub fn labelled(&self, text: &str) -> Vec<Placed> {
        let text = labels::reads(text, |x| {
            self.things.iter().any(|t| t.text == x && t.class != Class::Spawn)
        });
        self.things
            .iter()
            .filter(|t| t.text == text && t.class != Class::Spawn)
            .map(|t| Placed {
                placement: t.id(),
                object: t.handle(),
            })
            .collect()
    }

    /// The area of each netgame flag that has one, as `(flag index, area)`:
    /// what the bots read a hill from (`bots::objective::set_flag_areas`).
    pub fn flag_areas(&self) -> Vec<(u32, game_markers::Area)> {
        self.things
            .iter()
            .filter(|t| t.block == BlockKind::NetgameFlags)
            .filter_map(|t| Some((t.index, t.area.as_ref()?.marker_area())))
            .collect()
    }

    /// The thing a handle names.
    pub fn get(&self, handle: u32) -> Option<&Thing> {
        (handle & HANDLE != 0)
            .then(|| self.things.iter().find(|t| t.handle() == handle))
            .flatten()
    }

    /// Every labelled thing, in census order.
    pub fn things(&self) -> &[Thing] {
        &self.things
    }

    pub fn len(&self) -> usize {
        self.things.len()
    }

    pub fn is_empty(&self) -> bool {
        self.things.is_empty()
    }

    /// `rules.state`'s `labelled`.
    pub fn view(&self) -> Vec<wire::Labelled> {
        self.things
            .iter()
            .map(|t| wire::Labelled {
                label: t.text.clone(),
                class: class_name(t.class).into(),
                number: t.number,
                team: t.designator,
                id: t.id(),
                declared: t.label.is_some(),
            })
            .collect()
    }
}

pub fn class_name(c: Class) -> &'static str {
    match c {
        Class::Object => "object",
        Class::Flag => "flag",
        Class::Volume => "volume",
        Class::Spawn => "spawn",
    }
}

/// The game team of a netgame team designator (`E+0x10 + 2d`, `i16`);
/// none for an unused designator or without the engine globals.
pub fn designator_team(m: &dyn GuestMemory, d: u8) -> Result<Option<u8>, MemFault> {
    let e = m.u32(points::GAME_ENGINE.base)?;
    if e == 0 || u16::from(d) >= objs::DESIGNATORS {
        return Ok(None);
    }
    check_span(e, objs::DESIGNATOR_TEAMS + 2 * objs::DESIGNATORS as u32)?;
    let t = m.u16(e + objs::DESIGNATOR_TEAMS + 2 * u32::from(d))? as i16;
    Ok(u8::try_from(t).ok().filter(|&t| t < 8))
}

/// `o.team` (`object_team`): a labelled thing's team through its
/// designator, or an objective record's team (`+4`) on the carriers whose
/// record holds one ([`TEAM_OBJECTIVES`]). Neutral (8) and anything else
/// are none.
pub fn object_team(
    m: &dyn GuestMemory,
    table: Option<&Table>,
    engine: Option<i32>,
    object: u32,
) -> Result<Option<u8>, MemFault> {
    if let Some(t) = table.and_then(|t| t.get(object)) {
        return match t.designator {
            Some(d) => designator_team(m, d),
            None => Ok(None),
        };
    }
    if !engine.is_some_and(|e| TEAM_OBJECTIVES.contains(&e)) {
        return Ok(None);
    }
    Ok(super::objectives::records(m)?
        .into_iter()
        .find(|r| r.object.raw() == object)
        .and_then(|r| u8::try_from(r.designator).ok())
        .filter(|&t| t < 8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(text: &str, index: u32) -> Thing {
        Thing {
            text: text.into(),
            label: None,
            number: None,
            class: Class::Flag,
            designator: None,
            block: BlockKind::NetgameFlags,
            index,
            area: None,
        }
    }

    fn siege_ir() -> Ir {
        let mut ir = ::rules::compile(::rules::builtin::SIEGE).unwrap();
        ir.header.name = "Siege".into();
        ir
    }

    fn objects(table: &Table, text: &str) -> Vec<u32> {
        table.labelled(text).iter().map(|p| p.placement & 0xffff).collect()
    }

    /// Siege's sites are the territories until the author places one;
    /// then they are the placed ones alone.
    #[test]
    fn siege_sites_are_the_placed_ones_else_the_territories() {
        let ir = siege_ir();
        let mut table = Table::from_things(vec![flag("territory", 0), flag("territory", 1)]);
        table.relabel(&ir);
        assert_eq!(objects(&table, "siege_site"), vec![0, 1]);
        assert!(table.things().iter().all(|t| t.label.is_some()), "the territories are the site label's");

        let mut table = Table::from_things(vec![
            flag("territory", 0),
            flag("siege_site", 1),
            flag("territory", 2),
        ]);
        table.relabel(&ir);
        assert_eq!(objects(&table, "siege_site"), vec![1]);
        let declared: Vec<(&str, bool)> = table.things().iter().map(|t| (t.text.as_str(), t.label.is_some())).collect();
        assert_eq!(declared, vec![("territory", false), ("siege_site", true), ("territory", false)]);
    }

    /// The label Siege reads for its barriers is the one Forge's AI and
    /// objectives page offers, so an author picks it instead of typing it.
    #[test]
    fn siege_reads_the_barrier_label_forge_offers() {
        let ir = siege_ir();
        let text = forge::GroundLabel::Barrier.text();
        let def = ir.labels.iter().find(|l| l.text == text).expect("Siege declares the barrier label");
        assert_eq!(def.text, "siege_barrier");
    }
}
