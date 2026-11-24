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

use super::zones::{Shape, Zones};

/// The walls of `shown`, in order: a zone with no shape (the map has no
/// flag for it) has none.
pub fn of(shown: &[(u32, Option<u8>)], zones: &Zones) -> Vec<Wall> {
    shown
        .iter()
        .filter_map(|&(zone, team)| {
            let Shape::Area { at, area } = &zones.get(zone)?.shape;
            Some(Wall {
                name: format!("zone:{zone:x}"),
                team,
                at: *at,
                area: area.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zone_the_map_has_no_flag_for_has_no_wall() {
        assert!(of(&[(9, Some(1)), (4, None)], &Zones::default()).is_empty());
    }
}
