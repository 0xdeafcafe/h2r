//! Reusable A* workspace. The order of the heap and outgoing edges also
//! determines which of two equally good paths the bot takes.
use super::{Edge, Kind, NavMesh, dist, mid, penalty};
use std::collections::BinaryHeap;

const WALK_TICKS_PER_UNIT: f32 = 6.0;
const TELEPORTER_TRANSIT_TICKS: f32 = 6.0;

#[derive(Clone, Debug, PartialEq)]
struct Open(f32, u32);
impl Eq for Open {}
impl PartialOrd for Open {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Open {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        o.0.total_cmp(&self.0)
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct Search {
    best: Vec<f32>,
    came: Vec<u32>,
    touched: Vec<u32>,
    heap: BinaryHeap<Open>,
}

impl Search {
    /// After exhaustion this is the entire directed reachable set.
    pub(super) fn reached(&self, i: u32) -> bool {
        self.best[i as usize] < f32::INFINITY
    }

    pub(super) fn corridor(
        &mut self,
        mesh: &NavMesh,
        start: u32,
        goal: u32,
        skip: impl FnMut(u32, &Edge) -> bool,
    ) -> Option<Vec<u32>> {
        let mut path = Vec::new();
        self.corridor_into(mesh, start, goal, skip, &mut path)
            .then_some(path)
    }

    pub(super) fn corridor_into(
        &mut self,
        mesh: &NavMesh,
        start: u32,
        goal: u32,
        mut skip: impl FnMut(u32, &Edge) -> bool,
        path: &mut Vec<u32>,
    ) -> bool {
        path.clear();
        let n = mesh.polys.len();
        if start as usize >= n || goal as usize >= n {
            return false;
        }
        let goal_at = mesh.polys[goal as usize].center;
        // A straight-line lower bound stops being admissible when a link
        // can cross that distance more quickly than walking.
        let estimate = |at| {
            if mesh.shortcuts {
                0.0
            } else {
                dist(at, goal_at)
            }
        };
        for i in self.touched.drain(..) {
            self.best[i as usize] = f32::INFINITY;
        }
        self.best.resize(n, f32::INFINITY);
        self.came.resize(n, 0);
        self.heap.clear();
        self.best[start as usize] = 0.0;
        self.touched.push(start);
        self.heap
            .push(Open(estimate(mesh.polys[start as usize].center), start));
        while let Some(Open(_, i)) = self.heap.pop() {
            if i == goal {
                let mut at = goal;
                while at != start {
                    let e = self.came[at as usize];
                    path.push(e);
                    at = mesh.edges[e as usize].from;
                }
                path.reverse();
                return true;
            }
            let here = mesh.polys[i as usize].center;
            for &ei in mesh.out.get(i as usize).into_iter().flatten() {
                let e = &mesh.edges[ei as usize];
                if skip(ei, e) {
                    continue;
                }
                let there = mesh.polys[e.to as usize].center;
                let travel = if e.kind.traversal() {
                    let transit = match e.kind {
                        // Cost in walking-distance equivalents. At six
                        // ticks per unit, include trigger/platform wait and
                        // charge exposed airborne time by duration. These
                        // rates are estimates shared with supply travel.
                        Kind::Teleporter => TELEPORTER_TRANSIT_TICKS / WALK_TICKS_PER_UNIT,
                        Kind::Lift => (8.0 + dist(e.a, e.b) * 2.7 * 1.25) / WALK_TICKS_PER_UNIT,
                        Kind::Elevator => {
                            (24.0 + dist(e.a, e.b) * 4.8 * 1.25) / WALK_TICKS_PER_UNIT
                        }
                        _ => dist(e.a, e.b),
                    };
                    dist(here, e.a) + transit + dist(e.b, there)
                } else {
                    let m = mid(e.a, e.b);
                    dist(here, m) + dist(m, there)
                };
                let g = self.best[i as usize] + travel + penalty(e.kind);
                if g < self.best[e.to as usize] {
                    if !self.reached(e.to) {
                        self.touched.push(e.to);
                    }
                    self.best[e.to as usize] = g;
                    self.came[e.to as usize] = ei;
                    self.heap.push(Open(g + estimate(there), e.to));
                }
            }
        }
        false
    }
}
