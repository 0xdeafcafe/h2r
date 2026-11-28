//! The areas a Studio map carries (docs/game/gametype-markers.md, "Areas
//! saved with a map"): the record at the end of the map's file
//! (`game_markers::record`), read when the map is activated and attached
//! to the netgame flags the census labels, so a flag's zone takes its own
//! area (`rules::world::zones`, step 1) and the bots read the hill from it.
//!
//! A flag has no bytes for an area, so the rows are matched to flags by
//! what the flag was when the map was saved: its type, identifier and
//! position ([`game_markers::record::place`]). A flag edited since has no
//! area, and its zone is the default's.
use std::sync::{Arc, Mutex};

use ::game_markers::record::{self, Flag, Row};
use guest_layout::{GuestMemory, MemFault, points};
use forge_doc::{BlockKind, LabelledThing};

use crate::bots_nav_auto::VfsFile;

static LOADED: Mutex<Vec<Row>> = Mutex::new(Vec::new());

/// The rows of the active map's record.
pub fn current() -> Vec<Row> {
    LOADED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Sets the active map's rows (tests, and [`activate`]).
pub fn set(rows: Vec<Row>) {
    *LOADED.lock().unwrap_or_else(|e| e.into_inner()) = rows;
}

/// Reads the record at the end of the map at guest `path` (none when the
/// map is not a file the VFS opens, or has no record) and keeps it for the
/// game that is starting. A record that does not read is named and left
/// out.
pub fn activate(vfs: &Arc<Mutex<vfs::Vfs>>, path: Option<&str>) {
    let rows = path
        .and_then(|p| VfsFile::open(Arc::clone(vfs), p).ok())
        .and_then(|mut f| match record::read_tail(&mut f) {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("map areas: {e}; the map has none");
                None
            }
        })
        .unwrap_or_default();
    set(rows);
}

/// The loaded scenario's netgame flags as an areas record finds them: type
/// (`+0x10`), identifier (`+0x14`) and position (`+0x00`).
pub fn flags(m: &dyn GuestMemory) -> Result<Vec<Flag>, MemFault> {
    let s = m.u32(points::SCENARIO.base)?;
    if s == 0 {
        return Ok(Vec::new());
    }
    let def = map::scenario_layout::block(BlockKind::NetgameFlags);
    let n = (m.i32(s + def.offset)?.max(0) as u32).min(forge_doc::CENSUS_FLAG_LIMIT);
    let at = m.u32(s + def.offset + 4)?;
    engine::objectives::check_span(at, n * def.stride)?;
    let mut out = Vec::with_capacity(n as usize);
    for i in 0..n {
        let f = at + i * def.stride;
        out.push(Flag {
            flag_type: m.u16(f + engine::objectives::FLAG_TYPE)? as i16,
            identifier: m.u16(f + 0x14)? as i16,
            at: [m.f32(f)?, m.f32(f + 4)?, m.f32(f + 8)?],
        });
    }
    Ok(out)
}

/// Gives each netgame flag of `things` the area the record has for it. A
/// flag whose own area is already set (a Forge document's) keeps it. A row
/// that says its flag is shown in play shows it; a row never hides a flag
/// the document shows.
pub fn attach(things: &mut [LabelledThing], flags: &[Flag], rows: &[Row]) {
    let (placed, _lost) = record::place_rows(rows, flags);
    for (index, row) in placed {
        if let Some(t) = things
            .iter_mut()
            .find(|t| t.block == BlockKind::NetgameFlags && t.index as usize == index)
        {
            if t.area.is_none() {
                t.area = row.area.clone();
            }
            t.show |= row.show;
        }
    }
}

#[cfg(test)]
mod tests {
    use ::game_markers::Area;
    use guest_layout::testing::FakeGuest;

    use super::*;

    const SCNR: u32 = 0x1006_0000;
    const FLAGS_AT: u32 = SCNR + 0x400;

    /// A guest with `flags` (type, identifier, position) in its scenario.
    fn guest(flags: &[(u16, i16, [f32; 3])]) -> FakeGuest {
        let mut m = FakeGuest::new();
        m.put_u32(points::SCENARIO.base, SCNR);
        m.map(SCNR, 0x1000);
        m.put_u32(SCNR + engine::objectives::NETGAME_FLAGS, flags.len() as u32)
            .put_u32(SCNR + engine::objectives::NETGAME_FLAGS + 4, FLAGS_AT);
        for (i, (ty, id, p)) in flags.iter().enumerate() {
            let f = FLAGS_AT + i as u32 * engine::objectives::NETGAME_FLAG_STRIDE;
            m.put_f32(f, p[0])
                .put_f32(f + 4, p[1])
                .put_f32(f + 8, p[2])
                .put_u16(f + engine::objectives::FLAG_TYPE, *ty)
                .put_u16(f + 0x14, *id as u16);
        }
        m
    }

    fn thing(index: u32) -> LabelledThing {
        LabelledThing {
            block: BlockKind::NetgameFlags,
            index,
            label: forge_doc::builtin_label(11).unwrap(),
            class: ::rules::labels::Class::Flag,
            team: None,
            area: None,
            show: false,
        }
    }

    #[test]
    fn a_saved_area_goes_to_the_flag_it_was_saved_for() {
        let m = guest(&[
            (0, 0, [9.0; 3]),
            (11, 0, [1.0, 2.0, 0.5]),
            (11, 1, [4.0, 5.0, 0.5]),
        ]);
        let found = flags(&m).unwrap();
        assert_eq!(found.len(), 3);
        assert_eq!(found[1].at, [1.0, 2.0, 0.5]);
        // Saved when the hill was the first flag: the hint is stale, the
        // fingerprint finds it.
        let rows = vec![Row {
            index: 0,
            flag_type: 11,
            identifier: 0,
            at: [1.0, 2.0, 0.5],
            area: Some(Area::radius(3.0)),
            show: true,
        }];
        let mut things = vec![thing(0), thing(1), thing(2)];
        attach(&mut things, &found, &rows);
        assert_eq!(things[0].area, None);
        assert_eq!(things[1].area, Some(Area::radius(3.0)));
        assert_eq!(things[2].area, None);
        // The row's flag is shown in play; the others are as they were.
        assert_eq!(things.iter().map(|t| t.show).collect::<Vec<_>>(), [false, true, false]);
        // A flag with an area of its own keeps it.
        let mut own = vec![thing(0), thing(1), thing(2)];
        own[1].area = Some(Area::radius(5.0));
        attach(&mut own, &found, &rows);
        assert_eq!(own[1].area, Some(Area::radius(5.0)));
    }

    #[test]
    fn a_map_with_no_scenario_has_no_flags() {
        // The scenario pointer is mapped and zero: no scenario loaded.
        let mut m = FakeGuest::new();
        m.put_u32(points::SCENARIO.base, 0);
        assert_eq!(flags(&m).unwrap(), Vec::new());
    }
}
