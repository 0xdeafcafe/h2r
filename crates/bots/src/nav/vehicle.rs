//! A driver's searches against one immutable loaded map. The endpoint can
//! move within a polygon even while the ordered walk portals stay the same.
use super::{Kind, Search, V3};
use crate::cache::MapData;
use rustc_hash::FxHashMap;
use std::sync::{Arc, Weak};

#[derive(Clone, Debug, Default)]
pub(crate) struct VehiclePaths {
    owner: Weak<MapData>,
    // Vehicle datum and exact chassis dimensions (including mounted parts).
    chassis: Option<(u32, u32, u32)>,
    portals: FxHashMap<u32, Option<V3>>,
    search: Search,
    exhausted_from: Option<u32>,
    last: Option<(u32, u32, Vec<u32>)>,
    #[cfg(test)]
    pub(crate) searches: usize,
}

impl VehiclePaths {
    // A Weak keeps the allocation identity unique even after an unload,
    // and prevents in-place edits to this MapData while it is cached.
    pub(crate) fn bind_map(&mut self, map: &Arc<MapData>) {
        if self.owner.as_ptr() != Arc::as_ptr(map) {
            self.owner = Arc::downgrade(map);
            self.chassis = None;
        }
    }

    /// Steering found the leg from the current pose blocked. At the next
    /// plan, search afresh. The chassis tests at fixed portals still hold.
    pub(crate) fn discard_corridor(&mut self) {
        self.last = None;
    }

    pub(crate) fn vehicle_path(
        &mut self,
        map: &MapData,
        vehicle: u32,
        from: V3,
        to: V3,
        half_width: f32,
        height: f32,
    ) -> Option<Vec<V3>> {
        // A caller passing a plain borrowed map can edit it between ticks.
        if !std::ptr::eq(self.owner.as_ptr(), map) {
            self.owner = Weak::new();
            self.chassis = None;
        }
        let chassis = (vehicle, half_width.to_bits(), height.to_bits());
        if self.chassis != Some(chassis) {
            self.portals.clear();
            self.last = None;
            self.exhausted_from = None;
            self.chassis = Some(chassis);
        }

        let nav = &map.nav;
        let (start, _) = nav.nearest(from, 1.2, 0.5, 1.5)?;
        let (goal, to) = nav.nearest(to, 2.0, 0.5, 1.5)?;
        if self.exhausted_from == Some(start) && !self.search.reached(goal) {
            return None;
        }
        if self
            .last
            .as_ref()
            .is_none_or(|(s, g, _)| (*s, *g) != (start, goal))
        {
            #[cfg(test)]
            {
                self.searches += 1;
            }
            let corridor = self.search.corridor(nav, start, goal, |i, e| {
                e.kind != Kind::Walk
                    || self
                        .portals
                        .entry(i)
                        .or_insert_with(|| nav.vehicle_portal(e, half_width, height, &map.walls))
                        .is_none()
            });
            self.exhausted_from = corridor.is_none().then_some(start);
            self.last = corridor.map(|edges| (start, goal, edges));
        }
        let (_, _, edges) = self.last.as_ref()?;
        let mut points = Vec::with_capacity(edges.len() + 1);
        for edge in edges {
            points.push(self.portals[edge]?);
        }
        points.push(to);
        Some(points)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::los::Walls;
    use crate::nav::{Edge, NavMesh, Poly};

    // Two equal-cost ways across the four squares. A fifth square is
    // disconnected. All edges are directed.
    fn map() -> MapData {
        let polys = [(0., 0.), (4., 0.), (0., 4.), (4., 4.), (16., 0.)].map(|(x, y)| {
            Poly::new(vec![
                [x, y, 0.],
                [x + 4., y, 0.],
                [x + 4., y + 4., 0.],
                [x, y + 4., 0.],
            ])
        });
        let edge = |from, to, a, b| Edge {
            from,
            to,
            a,
            b,
            kind: Kind::Walk,
        };
        MapData {
            nav: NavMesh::new(
                polys.into(),
                vec![
                    edge(0, 1, [4., 0., 0.], [4., 4., 0.]),
                    edge(0, 2, [0., 4., 0.], [4., 4., 0.]),
                    edge(1, 3, [4., 4., 0.], [8., 4., 0.]),
                    edge(2, 3, [4., 4., 0.], [4., 8., 0.]),
                ],
            ),
            ..MapData::default()
        }
    }

    const START: V3 = [2., 2., 0.];
    const GOAL: V3 = [6., 6., 0.];
    const ISLAND: V3 = [18., 2., 0.];

    // The pre-cache vehicle algorithm: fresh search, then test every
    // winning portal again and append the current snapped destination.
    fn uncached(map: &MapData, from: V3, to: V3, width: f32, height: f32) -> Option<Vec<V3>> {
        let nav = &map.nav;
        let (s, _) = nav.nearest(from, 1.2, 0.5, 1.5)?;
        let (g, to) = nav.nearest(to, 2., 0.5, 1.5)?;
        let portal = |e: &Edge| nav.vehicle_portal(e, width, height, &map.walls);
        let corridor =
            nav.corridor_avoiding(s, g, &|e| e.kind != Kind::Walk || portal(e).is_none())?;
        let mut points: Vec<_> = corridor
            .iter()
            .filter_map(|&i| portal(&nav.edges[i as usize]))
            .collect();
        points.push(to);
        Some(points)
    }

    #[test]
    fn cached_paths_match_fresh_search_on_ties_and_moving_endpoints() {
        let map = Arc::new(map());
        assert_eq!(map.nav.corridor(0, 3), Some(vec![0, 2]));
        let mut paths = VehiclePaths::default();
        paths.bind_map(&map);
        for (from, to, searches) in [
            (START, GOAL, 1),
            (START, [6.5, 6., 0.], 1),
            ([3., 2., 0.], [8.2, 6., 0.], 1), // snapped to same polygon
            (START, [2., 6., 0.], 2),
            ([2., 6., 0.], GOAL, 3),
            (START, ISLAND, 4),
            (START, [19., 3., 0.], 4),
            (ISLAND, START, 5),
            (ISLAND, GOAL, 5), // another unreachable polygon
            (ISLAND, ISLAND, 6),
            (START, GOAL, 7),
        ] {
            assert_eq!(
                paths.vehicle_path(&map, 7, from, to, 0.5, 1.),
                uncached(&map, from, to, 0.5, 1.)
            );
            assert_eq!(paths.searches, searches);
        }
    }

    #[test]
    fn blocked_leg_discards_the_corridor_for_the_next_plan() {
        let map = Arc::new(map());
        let mut paths = VehiclePaths::default();
        paths.bind_map(&map);
        let first = paths.vehicle_path(&map, 7, START, GOAL, 0.5, 1.);
        assert!(first.is_some());
        let portals = paths.portals.clone();
        paths.discard_corridor();
        assert_eq!(paths.vehicle_path(&map, 7, START, GOAL, 0.5, 1.), first);
        assert_eq!(paths.searches, 2);
        assert_eq!(paths.portals, portals);
    }

    #[test]
    fn map_vehicle_and_hull_changes_invalidate_portals_and_reachability() {
        let open = Arc::new(map());
        let mut paths = VehiclePaths::default();
        paths.bind_map(&open);
        let plan = |paths: &mut VehiclePaths, map: &MapData, id, width, height| {
            paths.vehicle_path(map, id, START, GOAL, width, height)
        };
        assert!(plan(&mut paths, &open, 1, 0.5, 1.).is_some());
        assert!(plan(&mut paths, &open, 2, 0.5, 1.).is_some());
        assert_eq!(paths.searches, 2);
        assert!(plan(&mut paths, &open, 2, 5., 1.).is_none());
        assert!(plan(&mut paths, &open, 2, 0.5, 1.).is_some());

        let mut roofed = map();
        let roof = [
            [0., 0., 0.75],
            [8., 0., 0.75],
            [8., 8., 0.75],
            [0., 8., 0.75],
        ];
        roofed.walls = Walls::from_polygons([roof.as_slice()]);
        let roofed = Arc::new(roofed);
        paths.bind_map(&roofed);
        assert!(plan(&mut paths, &roofed, 2, 0.5, 1.).is_none());
        assert!(plan(&mut paths, &roofed, 2, 0.5, 1.).is_none());
        assert_eq!(paths.searches, 5);
        assert!(plan(&mut paths, &roofed, 2, 0.5, 0.5).is_some());
        assert_eq!(paths.searches, 6);
        // A borrowed fixture can replace collision in-place between calls.
        let mut editable = (*roofed).clone();
        assert!(plan(&mut paths, &editable, 2, 0.5, 1.).is_none());
        editable.walls = Walls::default();
        assert!(plan(&mut paths, &editable, 2, 0.5, 1.).is_some());
        assert!(plan(&mut paths, &roofed, 2, 0.5, 1.).is_none());
    }
}
