//! `show_area` and `hide_area` on the host (docs/game/gametype-markers.md,
//! "The boundary in play"): the walls of the areas a script shows in play.
//!
//! The VM keeps the shown areas in its state (`rules::vm::RulesExt::shown`,
//! at most eight: a zone object and a team). [`of`] gives each its wall
//! from the zone's shape (`super::zones::Zones`), and the runtime
//! publishes them after every tick (`Runtime::walls`);
//! the world-line source of Forge's markers draws each alone
//! (`crate::forge_glue::markers_world::draw_walls`), with no pip and no
//! label, whatever the tool is doing.

use crate::forge_glue::markers_world::Wall;

use guest_layout::GuestMemory;

use super::zones::{Shape, Zones};
use crate::rules::census::{self, Table};

/// The walls of `shown`, in order: a zone with no shape (the map has no
/// flag for it) has none.
pub fn of(shown: &[(u32, Option<u8>)], zones: &Zones) -> Vec<Wall> {
    shown
        .iter()
        .filter_map(|&(zone, team)| {
            let Shape::Area { at, yaw, area } = &zones.get(zone)?.shape;
            Some(Wall {
                name: format!("zone:{zone:x}"),
                team,
                at: *at,
                yaw: *yaw,
                area: area.clone(),
            })
        })
        .collect()
}

/// The walls of `shown` on a machine that does not run the script (a
/// client that received a walls cue, `::rules::show::Cue::Walls`): the
/// zones are read from its own copy of the map, which is the one the
/// host's script ran over (the census with the areas the map was saved
/// with), so the same flags give the same shapes.
pub fn client(m: &dyn GuestMemory, shown: &[(u32, Option<u8>)]) -> Vec<Wall> {
    let Ok(things) = census::things(m) else {
        return Vec::new();
    };
    of(shown, &Zones::read(m, &Table::of_census(&things)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wire's zone base is a netgame flag's handle with no index.
    #[test]
    fn the_cues_zone_base_is_a_netgame_flags_handle() {
        let flag = census::HANDLE | (forge_doc::BlockKind::NetgameFlags as u32) << 16;
        assert_eq!(flag, ::rules::show::ZONE_BASE);
    }

    #[test]
    fn a_zone_the_map_has_no_flag_for_has_no_wall() {
        assert!(of(&[(9, Some(1)), (4, None)], &Zones::default()).is_empty());
    }
}
