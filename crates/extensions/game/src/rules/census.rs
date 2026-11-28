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
use guest_layout::{GuestMemory, MemFault, points};
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

/// The labelled things of the loaded game with no layout: [`rows`] and the
/// flags' built-in labels ([`things_for`] with no script).
pub fn things(m: &dyn GuestMemory) -> Result<Vec<LabelledThing>, String> {
    things_for(m, None)
}

/// The labelled things of the loaded game, by [`rows`] and the built-in
/// layout of `script` on the loaded map ([`layout_doc`]), if there is one.
/// A map with no layout, or a script with none, keeps the flags' built-in
/// labels. The lobby's check reads the same layout
/// (`super::gate::Sources::census`), so the check and the match agree.
pub fn things_for(m: &dyn GuestMemory, script: Option<&str>) -> Result<Vec<LabelledThing>, String> {
    let rows = rows(m).map_err(|e| format!("guest memory at {:#x} faulted", e.addr))?;
    let layout = script
        .and_then(|_| crate::health::loaded_map_name(m).ok())
        .and_then(|map| layout_doc(script, &map, &rows.counts));
    let mut things = forge_doc::labelled(&rows, layout.as_ref()).map_err(|e| e.to_string())?;
    // The areas a Studio map was saved with (`crate::map_areas`).
    let saved = crate::map_areas::current();
    if !saved.is_empty()
        && let Ok(flags) = crate::map_areas::flags(m)
    {
        crate::map_areas::attach(&mut things, &flags, &saved);
    }
    Ok(things)
}

/// The layout `ir`'s script reads, by the name `rules::builtin::layout`
/// keys it: Siege, found by the label it declares for its sites.
pub fn layout_script(ir: &Ir) -> Option<&'static str> {
    ir.labels.iter().any(|l| l.text == "siege_site").then_some("siege")
}

/// The built-in layout of `script` on the map `map` (its short name) as a
/// document over a map whose block counts are `counts`: its label ops on
/// entries the map has. An op on an entry the map lacks is skipped, which
/// is what a placeholder index (a place not yet read off the map) is, so a
/// layout never blocks a map. None for a script or map with no layout.
pub fn layout_doc(
    script: Option<&str>,
    map: &str,
    counts: &std::collections::BTreeMap<BlockKind, u32>,
) -> Option<forge_doc::ForgeDoc> {
    let json = ::rules::builtin::layout(script?, &map.to_ascii_lowercase())?;
    let mut doc = match forge_doc::ForgeDoc::from_bytes(json.as_bytes(), &forge_doc::Limits::default()) {
        Ok(doc) => doc,
        Err(e) => {
            eprintln!("rules: the built-in layout of {map} does not read, the map keeps its own labels: {e}");
            return None;
        }
    };
    retain_present(&mut doc, counts);
    Some(doc)
}

/// Drops `doc`'s label ops on base entries a map of block counts `counts`
/// lacks, and sets the format the rest need.
fn retain_present(doc: &mut forge_doc::ForgeDoc, counts: &std::collections::BTreeMap<BlockKind, u32>) {
    use forge_doc::{Edit, Endpoint, Forge};
    doc.ops.retain(|op| match &op.edit {
        Edit::Forge(Forge::Label { target: Endpoint::Base { block, index, .. }, .. }) => {
            counts.get(block).is_some_and(|n| index < n)
        }
        _ => true,
    });
    doc.format = doc.required_format();
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

    /// A layout's labels are what a script reads: its sites are numbered,
    /// its starts labelled, and an op on an entry the map lacks is skipped.
    #[test]
    fn a_layout_numbers_sites_and_labels_starts() {
        use forge_doc::{FlagRow, MapRows};
        let rows = MapRows {
            counts: [(BlockKind::NetgameFlags, 3), (BlockKind::StartingLocations, 2)].into(),
            flags: vec![FlagRow { kind: 10, team: 0 }, FlagRow { kind: 10, team: 0 }, FlagRow { kind: 0, team: 1 }],
            starts: vec![0, 1],
            ..MapRows::default()
        };
        // Without a layout the territories keep their built-in label.
        let plain = forge_doc::labelled(&rows, None).unwrap();
        assert!(plain.iter().filter(|t| t.block == BlockKind::NetgameFlags).take(2).all(|t| t.label.text == "territory" && t.label.number.is_none()));
        assert!(plain.iter().all(|t| t.class != Class::Spawn));

        // A layout over invented entries: two sites in phase 0 and 1, a
        // start of each side, and one op on an entry the map lacks.
        let op = |id: u32, block: &str, index: u32, text: &str, number: Option<u32>| {
            let number = number.map(|n| format!(r#","number":{n}"#)).unwrap_or_default();
            format!(r#"{{"edit":{{"forge":{{"label":{{"label":{{"text":"{text}"{number}}},"target":{{"base":{{"block":"{block}","index":{index}}}}}}}}}}},"id":{id}}}"#)
        };
        let ops = [
            op(1, "netgame_flags", 0, "siege_site", Some(0)),
            op(2, "netgame_flags", 1, "siege_site", Some(1)),
            op(3, "starting_locations", 0, "siege_attack", Some(0)),
            op(4, "starting_locations", 1, "siege_defend", Some(0)),
            op(5, "starting_locations", 32000, "siege_attack", Some(2)),
        ]
        .join(",");
        let json = format!(
            r#"{{"author":"t","base":{{"map":"m","sha256":"{}"}},"builder":{{"bots":5,"forge_build":1,"pathfind":1}},"description":"","format":2,"imports":[],"ops":[{ops}],"recipes":[],"sources":[],"title":"t"}}"#,
            "0".repeat(64)
        );
        let mut doc = forge_doc::ForgeDoc::from_bytes(json.as_bytes(), &forge_doc::Limits::default()).unwrap();
        retain_present(&mut doc, &rows.counts);
        let things = forge_doc::labelled(&rows, Some(&doc)).unwrap();
        let find = |block, index| things.iter().find(|t| t.block == block && t.index == index).unwrap();
        let site = find(BlockKind::NetgameFlags, 1);
        assert_eq!((site.label.text.as_str(), site.label.number), ("siege_site", Some(1)));
        let start = find(BlockKind::StartingLocations, 0);
        assert_eq!((start.label.text.as_str(), start.label.number, start.class, start.team), ("siege_attack", Some(0), Class::Spawn, Some(0)));
        let start = find(BlockKind::StartingLocations, 1);
        assert_eq!((start.label.text.as_str(), start.team), ("siege_defend", Some(1)));
        assert_eq!(find(BlockKind::NetgameFlags, 2).label.text, "ctf_flag_spawn", "an unlabelled flag keeps its built-in label");
        assert_eq!(things.iter().filter(|t| t.class == Class::Spawn).count(), 2, "the op on a missing start is skipped");

        // The shipped Zanzibar layout is read off the map: on a map of
        // Zanzibar's flags it numbers four of its territories as sites and
        // labels its starts by side and phase, and it reads as a document.
        let kinds: [u16; 72] = [0,1,1,0,0,2,2,2,3,3,4,4,4,10,10,10,10,10,10,10,10,12,12,12,12,12,12,12,12,13,14,10,10,14,14,14,14,14,14,14,13,13,13,13,13,13,13,13,13,3,3,10,16,11,11,11,11,11,11,11,11,11,11,11,11,16,16,16,16,16,16,16];
        let teams: [u16; 72] = [0,0,1,1,8,1,0,8,1,0,8,8,8,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0];
        let zan = MapRows {
            counts: [(BlockKind::NetgameFlags, 72), (BlockKind::StartingLocations, 48)].into(),
            flags: kinds.iter().zip(teams).map(|(&kind, team)| FlagRow { kind, team }).collect(),
            starts: vec![0; 48],
            ..MapRows::default()
        };
        let doc = layout_doc(Some("siege"), "Zanzibar", &zan.counts).unwrap();
        assert_eq!(doc.ops.len(), 4 + 1 + 5 + 6 + 6 + 6 + 6 + 10, "every op of the layout is on an entry Zanzibar has");
        let things = forge_doc::labelled(&zan, Some(&doc)).unwrap();
        let sites: Vec<_> = things.iter().filter(|t| t.label.text == "siege_site").map(|t| (t.index, t.label.number)).collect();
        assert_eq!(sites, [(13, Some(1)), (15, Some(0)), (17, Some(0)), (19, Some(1))]);
        // The final stand is blue's flag stand, labelled so it stays put when the sides swap.
        let goals: Vec<_> = things.iter().filter(|t| t.label.text == "siege_goal").map(|t| t.index).collect();
        assert_eq!(goals, [3]);
        let starts = |text: &str, n: i32| things.iter().filter(|t| t.label.text == text && t.label.number == Some(n)).count();
        assert_eq!([0, 1, 2].map(|n| starts("siege_attack", n)), [5, 6, 6]);
        assert_eq!([0, 1, 2].map(|n| starts("siege_defend", n)), [6, 6, 10]);
        // On the fake three-flag map only the ops on its two starts stay.
        let doc = layout_doc(Some("siege"), "Zanzibar", &rows.counts).unwrap();
        assert!(doc.ops.len() == 2 && forge_doc::labelled(&rows, Some(&doc)).unwrap().iter().filter(|t| t.class == Class::Spawn).count() == 2);
        assert!(layout_doc(Some("siege"), "lockout", &rows.counts).is_none());
        assert!(layout_doc(None, "zanzibar", &rows.counts).is_none());
        assert_eq!(layout_script(&siege_ir()), Some("siege"));
    }
}
