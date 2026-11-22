//! 2D trees: the surfaces a reference reaches, partitioned in its plane's
//! projection so the engine's point walk (front when `a*u + b*v - d >= 0`)
//! takes any point of a surface to it.
//!
//! A node's line is an edge of one of its surfaces, facing away from that
//! surface. A surface goes to a side it reaches past [`LINE_EPS`]; one
//! that reaches both goes to both, unsplit (the matcher's polygon test
//! decides). Two convex polygons that do not overlap are separated by an
//! edge line of one of them, so some line always leaves each side with
//! fewer surfaces than the node; when none does, the surfaces overlap and
//! the build is refused. Trees are shared between references naming the
//! same plane, flip and surfaces.

use std::collections::HashMap;

use tags::collision::{Child2d, Node2d};

use crate::geom::V3;
use crate::prep::Prepared;
use crate::query;
use crate::{Error, Limits, Result};

/// How far past a line a surface must reach to be on that side.
const LINE_EPS: f64 = 1e-4;
/// Candidate lines tried per node before trying every one.
const SAMPLE: usize = 96;

type P2 = [f64; 2];

pub(crate) struct Trees2d<'a> {
    prep: &'a Prepared,
    limits: &'a Limits,
    pub nodes: Vec<Node2d>,
    cache: HashMap<(usize, bool, Vec<u32>), Child2d>,
}

struct Line {
    owner: usize,
    n: P2,
    d: f64,
}

impl<'a> Trees2d<'a> {
    pub(crate) fn new(prep: &'a Prepared, limits: &'a Limits) -> Self {
        Self {
            prep,
            limits,
            nodes: Vec::new(),
            cache: HashMap::new(),
        }
    }

    /// The root of the tree over `surfaces` (sorted, distinct) on
    /// `plane` seen with `flip`.
    pub(crate) fn get(&mut self, plane: usize, flip: bool, surfaces: Vec<u32>) -> Result<Child2d> {
        let key = (plane, flip, surfaces);
        if let Some(&c) = self.cache.get(&key) {
            return Ok(c);
        }
        let [u, v] = query::axes(self.prep.planes[plane], flip);
        let polys: Vec<(u32, Vec<P2>)> = key
            .2
            .iter()
            .map(|&s| {
                let pts: Vec<P2> = self
                    .prep
                    .ring_points(s as usize)
                    .into_iter()
                    .map(|p: V3| [p[u], p[v]])
                    .collect();
                (s, pts)
            })
            .collect();
        let all: Vec<usize> = (0..polys.len()).collect();
        let root = self.node(&polys, all, 0)?;
        self.cache.insert(key, root);
        Ok(root)
    }

    fn node(&mut self, polys: &[(u32, Vec<P2>)], set: Vec<usize>, depth: usize) -> Result<Child2d> {
        if set.len() == 1 {
            return Ok(Child2d::Surface(polys[set[0]].0 as u16));
        }
        if depth > polys.len() + 1 {
            return Err(Error::Invalid("a 2D tree does not shrink".into()));
        }
        let lines = candidates(polys, &set);
        let stride = (lines.len() / SAMPLE).max(1);
        let pick = |lines: &[Line], step: usize| {
            let mut best: Option<((usize, usize), usize, Vec<usize>, Vec<usize>)> = None;
            for (i, l) in lines.iter().enumerate().step_by(step) {
                let (front, back) = sides(polys, &set, l);
                if front.is_empty() || back.is_empty() || front.len() == set.len() || back.len() == set.len() {
                    continue;
                }
                let both = front.len() + back.len() - set.len();
                let score = (both, front.len().abs_diff(back.len()));
                if best.as_ref().is_none_or(|b| score < b.0) {
                    best = Some((score, i, front, back));
                }
            }
            best
        };
        let best = pick(&lines, stride).or_else(|| if stride > 1 { pick(&lines, 1) } else { None });
        let Some((_, i, front, back)) = best else {
            return Err(Error::Overlap {
                a: polys[set[0]].0 as usize,
                b: polys[set[1]].0 as usize,
            });
        };
        let at = self.nodes.len();
        self.limits.check("2D node", at + 1)?;
        let l = &lines[i];
        self.nodes.push(Node2d {
            plane: [l.n[0] as f32, l.n[1] as f32, l.d as f32],
            back: Child2d::Surface(0x7fff),
            front: Child2d::Surface(0x7fff),
        });
        let b = self.node(polys, back, depth + 1)?;
        let f = self.node(polys, front, depth + 1)?;
        self.nodes[at].back = b;
        self.nodes[at].front = f;
        Ok(Child2d::Node(at as u16))
    }
}

/// Every edge line of the set's polygons, each facing away from its own
/// polygon.
fn candidates(polys: &[(u32, Vec<P2>)], set: &[usize]) -> Vec<Line> {
    let mut out = Vec::new();
    for &i in set {
        let pts = &polys[i].1;
        let c = pts.iter().fold([0.0, 0.0], |a, p| [a[0] + p[0], a[1] + p[1]]);
        let c = [c[0] / pts.len() as f64, c[1] / pts.len() as f64];
        for k in 0..pts.len() {
            let (p, q) = (pts[k], pts[(k + 1) % pts.len()]);
            let e = [q[0] - p[0], q[1] - p[1]];
            let len = (e[0] * e[0] + e[1] * e[1]).sqrt();
            if len <= 1e-9 {
                continue;
            }
            let mut n = [e[1] / len, -e[0] / len];
            let mut d = n[0] * p[0] + n[1] * p[1];
            if n[0] * c[0] + n[1] * c[1] - d > 0.0 {
                n = [-n[0], -n[1]];
                d = -d;
            }
            out.push(Line { owner: i, n, d });
        }
    }
    out
}

/// The set's polygons in front of and behind a line; the line's own
/// polygon is behind it.
fn sides(polys: &[(u32, Vec<P2>)], set: &[usize], l: &Line) -> (Vec<usize>, Vec<usize>) {
    let (mut front, mut back) = (Vec::new(), Vec::new());
    for &i in set {
        if i == l.owner {
            back.push(i);
            continue;
        }
        let (lo, hi) = polys[i].1.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), p| {
            let v = l.n[0] * p[0] + l.n[1] * p[1] - l.d;
            (lo.min(v), hi.max(v))
        });
        let f = hi > LINE_EPS;
        let b = lo < -LINE_EPS || !f;
        if f {
            front.push(i);
        }
        if b {
            back.push(i);
        }
    }
    (front, back)
}
