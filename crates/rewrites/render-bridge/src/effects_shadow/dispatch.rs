//! The sorted dispatch `0x0001e370` as a plan: its entries in the order
//! the loop meets them, each named by its handler, and the runs of
//! particle and decal entries the display copy draws from
//! (docs/graphics/retire-d3d.md, "R2's shadow arm").
//!
//! The loop (`0x0001e3d0..0x0001e404`) calls `[entry+0x20]` with
//! `entry+0x24` for each index of `[0x004b628c]`, skipping a null handler,
//! and reads the list's end again after every entry.

use guest_layout::memory::GuestMemory;

use super::view_update::{
    LIST_CAPACITY, LIST_END, LIST_ENTRIES, LIST_FIRST, LIST_ORDER, LIST_STRIDE, PARTICLE_HANDLER,
};

/// The standard geometry handler (`points::TRANSPARENT_GEOMETRY`).
pub const GEOMETRY_HANDLER: u32 = 0x0004_1020;
/// The polygon handler `0x0001e5e0` queues: contrails and effect strips.
pub const CONTRAIL_HANDLER: u32 = 0x0003_5b00;
/// The handlers the decal list builders `0x000420a0` and `0x00042760`
/// queue (docs/graphics/native-decals-research.md, "The sorted-list
/// handlers are not decals"): layer replays of decal-list records.
pub const DECAL_HANDLERS: [u32; 2] = [0x0004_1cc0, 0x0004_23c0];

/// What an entry's handler is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Geometry,
    Particle,
    Contrail,
    Decal,
    /// A null handler: the loop skips the call.
    Null,
    /// A handler the plan does not name.
    Other,
}

impl Kind {
    pub fn of(handler: u32) -> Self {
        match handler {
            0 => Kind::Null,
            GEOMETRY_HANDLER => Kind::Geometry,
            PARTICLE_HANDLER => Kind::Particle,
            CONTRAIL_HANDLER => Kind::Contrail,
            h if DECAL_HANDLERS.contains(&h) => Kind::Decal,
            _ => Kind::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Geometry => "geometry",
            Kind::Particle => "particle",
            Kind::Contrail => "contrail",
            Kind::Decal => "decal",
            Kind::Null => "null",
            Kind::Other => "other",
        }
    }

    pub const ALL: [Kind; 6] = [
        Kind::Geometry,
        Kind::Particle,
        Kind::Contrail,
        Kind::Decal,
        Kind::Null,
        Kind::Other,
    ];
}

/// One entry as the loop meets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Planned {
    /// Its place in the loop.
    pub ordinal: u16,
    /// Its row in the list.
    pub index: u16,
    pub handler: u32,
    pub kind: Kind,
}

/// A run of consecutive entries of one kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub kind: Kind,
    pub ordinals: Vec<u16>,
}

/// The dispatch's entries in loop order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub entries: Vec<Planned>,
}

impl Plan {
    /// The plan of the list as the sort left it, or `None` when the list
    /// does not read or its order array is not a permutation of the live
    /// rows.
    pub fn read(m: &dyn GuestMemory) -> Option<Plan> {
        let first = m.u32(LIST_FIRST).ok()?;
        let end = m.u32(LIST_END).ok()?;
        if first > end || end > LIST_CAPACITY {
            return None;
        }
        let rows = m.u32(LIST_ENTRIES).ok()?;
        let order = m.u32(LIST_ORDER).ok()?;
        let mut seen = vec![false; (end - first) as usize];
        let mut entries = Vec::with_capacity(seen.len());
        for ordinal in 0..end - first {
            let index = m.u16(order.wrapping_add(ordinal * 2)).ok()?;
            let slot = seen.get_mut(u32::from(index).checked_sub(first)? as usize)?;
            if std::mem::replace(slot, true) {
                return None;
            }
            let handler = m
                .u32(rows.wrapping_add(u32::from(index) * LIST_STRIDE + 0x20))
                .ok()?;
            entries.push(Planned {
                ordinal: ordinal as u16,
                index,
                handler,
                kind: Kind::of(handler),
            });
        }
        Some(Plan { entries })
    }

    /// The loop's indices, in order.
    pub fn order(&self) -> Vec<u16> {
        self.entries.iter().map(|e| e.index).collect()
    }

    /// How many entries of each kind, in [`Kind::ALL`] order.
    pub fn census(&self) -> [u64; 6] {
        let mut n = [0; 6];
        for e in &self.entries {
            n[Kind::ALL.iter().position(|k| *k == e.kind).unwrap_or(5)] += 1;
        }
        n
    }

    /// The runs of consecutive particle or decal entries, in loop order.
    /// A null entry between two particle entries ends the run: the loop
    /// reaches the join after it, so the run is drawn there.
    pub fn runs(&self) -> Vec<Run> {
        let mut runs: Vec<Run> = Vec::new();
        let mut open = false;
        for e in &self.entries {
            if !matches!(e.kind, Kind::Particle | Kind::Decal) {
                open = false;
                continue;
            }
            match runs.last_mut() {
                Some(run) if open && run.kind == e.kind => run.ordinals.push(e.ordinal),
                _ => runs.push(Run {
                    kind: e.kind,
                    ordinals: vec![e.ordinal],
                }),
            }
            open = true;
        }
        runs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const ROWS: u32 = 0x004c_0000;
    const ORDER: u32 = 0x004d_0000;

    fn list(handlers: &[u32], order: &[u16]) -> FlatMemory {
        let mut m = FlatMemory::new(0x004b_0000, 0x0003_0000);
        m.set_u32(LIST_ENTRIES, ROWS).unwrap();
        m.set_u32(LIST_ORDER, ORDER).unwrap();
        m.set_u32(LIST_FIRST, 0).unwrap();
        m.set_u32(LIST_END, handlers.len() as u32).unwrap();
        for (i, h) in handlers.iter().enumerate() {
            m.set_u32(ROWS + i as u32 * LIST_STRIDE + 0x20, *h).unwrap();
        }
        for (i, o) in order.iter().enumerate() {
            m.set_u16(ORDER + 2 * i as u32, *o).unwrap();
        }
        m
    }

    #[test]
    fn the_plan_names_each_handler_in_loop_order_and_groups_runs() {
        let p = PARTICLE_HANDLER;
        let handlers = [
            p,
            GEOMETRY_HANDLER,
            p,
            DECAL_HANDLERS[0],
            p,
            0,
            p,
            0x1234,
            DECAL_HANDLERS[1],
        ];
        // The loop meets rows 0, 2, 4 (three particles), 3, 8 (decals),
        // 1 (geometry), 6 (particle), 5 (null), 7 (other).
        let m = list(&handlers, &[0, 2, 4, 3, 8, 1, 6, 5, 7]);
        let plan = Plan::read(&m).unwrap();
        assert_eq!(plan.order(), [0, 2, 4, 3, 8, 1, 6, 5, 7]);
        assert_eq!(plan.census(), [1, 4, 0, 2, 1, 1]);
        assert_eq!(
            plan.runs(),
            [
                Run {
                    kind: Kind::Particle,
                    ordinals: vec![0, 1, 2]
                },
                Run {
                    kind: Kind::Decal,
                    ordinals: vec![3, 4]
                },
                Run {
                    kind: Kind::Particle,
                    ordinals: vec![6]
                },
            ]
        );
    }

    #[test]
    fn an_order_that_is_not_the_lists_has_no_plan() {
        let p = PARTICLE_HANDLER;
        assert!(Plan::read(&list(&[p, p], &[1, 1])).is_none());
        assert!(Plan::read(&list(&[p, p], &[0, 2])).is_none());
        assert_eq!(Plan::read(&list(&[], &[])), Some(Plan::default()));
    }
}
