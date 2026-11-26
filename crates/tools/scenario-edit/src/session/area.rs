//! Areas on netgame flags (docs/game/gametype-markers.md, "Studio"): what
//! a flag's area is, setting and clearing it, and converting a retail
//! hill's corners to one centre flag with an area. Each is one undo step;
//! a drag of a handle merges into one.
use game_markers::Area;

use super::*;
use crate::edit::{Op, SetField};
use crate::geometry::Gameplay;
use crate::marker_view::flag_kind;

/// Where a flag's area comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AreaFrom {
    /// Stored for the flag.
    Stored,
    /// Nobody has shaped it: the default cylinder.
    Default,
}

impl Session {
    /// The stored area of netgame flag `index`.
    pub fn flag_area(&self, index: usize) -> Option<&Area> {
        self.scenario()?.area(index)
    }

    /// The stored area of the flag `m` is, for [`crate::marker_view::placed_with`].
    pub fn marker_area(&self, m: &geometry::Marker) -> Option<Area> {
        (m.block == BlockKind::NetgameFlags)
            .then(|| self.flag_area(m.index).cloned())
            .flatten()
    }

    /// The area of netgame flag `index` as the handles edit it: its own,
    /// else the default of a flag of a hill, territory or site type that
    /// is not a corner of a hill with corners. With the flag's place.
    pub fn editable_area(&self, index: usize) -> Option<(Vec3, Area, AreaFrom)> {
        let scenario = self.scenario()?;
        let m = self
            .markers()
            .iter()
            .find(|m| m.block == BlockKind::NetgameFlags && m.index == index)?;
        if let Some(a) = scenario.area(index) {
            return Some((m.position, a.clone(), AreaFrom::Stored));
        }
        let ty = Gameplay::of(Some(scenario), m).flag_type?;
        let default = flag_kind(ty).and_then(|k| k.area().cloned())?;
        (!scenario.is_hill_corner(index)).then_some((m.position, default, AreaFrom::Default))
    }

    /// The yaw netgame flag `index` faces, in radians counter-clockwise
    /// from +x: the turn of a box area. 0 for a flag not placed.
    pub fn flag_yaw(&self, index: usize) -> f32 {
        self.markers()
            .iter()
            .find(|m| m.block == BlockKind::NetgameFlags && m.index == index)
            .map_or(0.0, |m| crate::area_edit::yaw_of(m.axes[0]))
    }

    /// Sets or clears netgame flag `index`'s area as one undo step, or
    /// merged into the last with the same `merge` key (a drag).
    pub fn set_area(&mut self, index: usize, area: Option<Area>, merge: Option<u64>) -> Result<()> {
        let mut command = Command::area(index, area);
        command.merge = merge;
        self.apply(command)
    }

    /// The corner flags of the hill flag `index` is one of: three or more
    /// of one hill type and identifier, in block order.
    pub fn hill_corners(&self, index: usize) -> Vec<usize> {
        let Some(s) = self.scenario() else {
            return Vec::new();
        };
        if !s.is_hill_corner(index) {
            return Vec::new();
        }
        let key = |i: usize| {
            let flags = BlockKind::NetgameFlags;
            (
                s.get(flags, i, "type").ok().and_then(Value::as_int),
                s.get(flags, i, "identifier").ok().and_then(Value::as_int),
            )
        };
        let mine = key(index);
        (0..s.block(BlockKind::NetgameFlags).len())
            .filter(|&i| key(i) == mine)
            .collect()
    }

    /// Converts the retail hill that corner `index` belongs to into one
    /// flag with an area (`Area::of_corners`), as one undo step: the first
    /// corner takes the hull's centre and the area, and the other corners
    /// are removed. Returns the converted flag's index after the removals,
    /// or why not (the hull's area is refused, or the flags may not be
    /// removed).
    pub fn convert_hill(&mut self, index: usize) -> Result<usize> {
        let corners = self.hill_corners(index);
        if corners.is_empty() {
            return Err(Error::Declined(
                "that flag is not a corner of a hill with corners".into(),
            ));
        }
        let scenario = self.scenario().ok_or(Error::Save("no map is open"))?;
        let places: Vec<Vec3> = corners
            .iter()
            .map(|&i| {
                self.markers()
                    .iter()
                    .find(|m| m.block == BlockKind::NetgameFlags && m.index == i)
                    .map(|m| m.position)
            })
            .collect::<Option<_>>()
            .ok_or(Error::Invalid("a corner has no place"))?;
        let (centre, area) = Area::of_corners(&places)
            .ok_or_else(|| Error::Declined("the corners make no polygon".into()))?;
        area.validate().map_err(|e| Error::Declined(e.to_string()))?;
        let flags = BlockKind::NetgameFlags;
        let keep = corners[0];
        // Swap-removals, highest first, with the kept flag followed to
        // the slot a removal moves it into.
        let mut kept = keep;
        let mut len = scenario.block(flags).len();
        let mut ops = Vec::new();
        for &i in corners.iter().skip(1).rev() {
            ops.push(Op::Remove { block: flags, index: i });
            if kept == len - 1 {
                kept = i;
            }
            len -= 1;
        }
        ops.push(Op::Set(SetField {
            block: flags,
            index: kept,
            field: "position",
            value: Value::Vec3(centre),
        }));
        ops.push(Op::Area {
            index: kept,
            area: Some(area),
        });
        self.apply(Command {
            label: "convert hill to area".into(),
            ops,
            merge: None,
        })?;
        Ok(kept)
    }
}
