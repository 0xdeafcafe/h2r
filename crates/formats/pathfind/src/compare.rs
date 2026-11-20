//! Metrics between two pathfinding datasets over the same collision BSP
//! (normally retail against generated).
use crate::generate::surface_edges;
use crate::geom::{V3, contains_xy, newell};
use tags::collision::Bsp;
use tags::pathfinding::PathfindingData;

/// One sample: a point on a collision surface, weighted by XY area.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub surface: usize,
    pub point: V3,
    pub area: f32,
}

/// Area-weighted samples on every upward-facing surface: the centroid of
/// each fan triangle of the surface polygon.
pub fn samples(bsp: &Bsp) -> Vec<Sample> {
    let mut out = Vec::new();
    for s in 0..bsp.surfaces.len() {
        let Some(ring) = surface_edges(bsp, s) else {
            continue;
        };
        let p: Vec<V3> = ring.iter().map(|&(_, v)| bsp.vertices[v].point).collect();
        if p.len() < 3 || newell(&p)[2] <= 0.0 {
            continue;
        }
        for i in 1..p.len() - 1 {
            let t = [p[0], p[i], p[i + 1]];
            let area = newell(&t)[2] * 0.5;
            if area <= 0.0 {
                continue;
            }
            let c = std::array::from_fn(|k| (t[0][k] + t[1][k] + t[2][k]) / 3.0);
            out.push(Sample {
                surface: s,
                point: c,
                area,
            });
        }
    }
    out
}

/// The sector a sample lands in through `pf`'s refs and 2D BSP, if the
/// sector's polygon also contains it.
pub fn locate(pf: &PathfindingData, s: &Sample) -> Option<u32> {
    let k = pf.sector_for_surface(s.surface, s.point[0], s.point[1])?;
    let poly: Vec<V3> = pf
        .sector_polygon(k as usize)?
        .into_iter()
        .map(|i| pf.vertices[i])
        .collect();
    contains_xy(&poly, s.point[0], s.point[1]).then_some(k)
}

/// The sector a sample resolves to through refs and the 2D BSP, without
/// the polygon test.
pub fn resolve(pf: &PathfindingData, s: &Sample) -> Option<u32> {
    pf.sector_for_surface(s.surface, s.point[0], s.point[1])
}

/// Connected components of sectors over two-sided links.
pub fn components(pf: &PathfindingData) -> Vec<u32> {
    let n = pf.sectors.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    fn find(p: &mut [u32], mut x: u32) -> u32 {
        while p[x as usize] != x {
            p[x as usize] = p[p[x as usize] as usize];
            x = p[x as usize];
        }
        x
    }
    for l in &pf.links {
        let (a, b) = (usize::from(l.left_sector), usize::from(l.right_sector));
        if a < n && b < n {
            let (ra, rb) = (find(&mut parent, a as u32), find(&mut parent, b as u32));
            parent[ra as usize] = rb;
        }
    }
    (0..n as u32).map(|x| find(&mut parent, x)).collect()
}

/// Comparison of `a` (reference) with `b` (candidate).
#[derive(Clone, Debug, Default)]
pub struct Metrics {
    /// XY area of samples each dataset covers (resolves and contains).
    pub area_both: f64,
    pub area_a_only: f64,
    pub area_b_only: f64,
    pub area_neither: f64,
    /// Samples whose resolved sector does not contain them, per dataset.
    pub misplaced_a: usize,
    pub misplaced_b: usize,
    pub samples: usize,
    /// Pairs of covered samples on edge-adjacent surfaces: connected in
    /// a and b, a only, b only, neither.
    pub connect_both: usize,
    pub connect_a_only: usize,
    pub connect_b_only: usize,
    pub connect_neither: usize,
    /// Random pairs of samples covered by both: same component in a and
    /// b, a only, b only, neither.
    pub reach_both: usize,
    pub reach_a_only: usize,
    pub reach_b_only: usize,
    pub reach_neither: usize,
}
impl Metrics {
    /// Share of a's covered area that b also covers.
    pub fn coverage(&self) -> f64 {
        self.area_both / (self.area_both + self.area_a_only).max(1e-9)
    }
    /// Share of b's covered area that a does not cover.
    pub fn excess(&self) -> f64 {
        self.area_b_only / (self.area_both + self.area_b_only).max(1e-9)
    }
    /// Agreement on reachability of random sample pairs.
    pub fn reach_agreement(&self) -> f64 {
        let all = self.reach_both + self.reach_a_only + self.reach_b_only + self.reach_neither;
        (self.reach_both + self.reach_neither) as f64 / all.max(1) as f64
    }
    /// Agreement on adjacency connectivity.
    pub fn connect_agreement(&self) -> f64 {
        let all =
            self.connect_both + self.connect_a_only + self.connect_b_only + self.connect_neither;
        (self.connect_both + self.connect_neither) as f64 / all.max(1) as f64
    }
}

/// Compare `a` and `b` on the samples of `bsp`.
pub fn compare(bsp: &Bsp, a: &PathfindingData, b: &PathfindingData) -> Metrics {
    let mut m = Metrics::default();
    let samples = samples(bsp);
    m.samples = samples.len();
    // First covered sample per surface, for connectivity.
    let mut rep: Vec<Option<(Option<u32>, Option<u32>)>> = vec![None; bsp.surfaces.len()];
    for s in &samples {
        let (la, lb) = (locate(a, s), locate(b, s));
        if la.is_none() && resolve(a, s).is_some() {
            m.misplaced_a += 1;
        }
        if lb.is_none() && resolve(b, s).is_some() {
            m.misplaced_b += 1;
        }
        let w = f64::from(s.area);
        match (la.is_some(), lb.is_some()) {
            (true, true) => m.area_both += w,
            (true, false) => m.area_a_only += w,
            (false, true) => m.area_b_only += w,
            (false, false) => m.area_neither += w,
        }
        let r = &mut rep[s.surface];
        if r.is_none() && la.is_some() && lb.is_some() {
            *r = Some((la, lb));
        }
    }
    let (ca, cb) = (components(a), components(b));
    // Reachability over random pairs (fixed-seed LCG, so runs repeat).
    let covered: Vec<(u32, u32)> = samples
        .iter()
        .filter_map(|s| Some((locate(a, s)?, locate(b, s)?)))
        .collect();
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut next = |n: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n
    };
    if !covered.is_empty() {
        for _ in 0..20_000 {
            let (p, q) = (covered[next(covered.len())], covered[next(covered.len())]);
            let ia = ca[p.0 as usize] == ca[q.0 as usize];
            let ib = cb[p.1 as usize] == cb[q.1 as usize];
            match (ia, ib) {
                (true, true) => m.reach_both += 1,
                (true, false) => m.reach_a_only += 1,
                (false, true) => m.reach_b_only += 1,
                (false, false) => m.reach_neither += 1,
            }
        }
    }
    for e in &bsp.edges {
        let (l, r) = (usize::from(e.left_surface), usize::from(e.right_surface));
        let (Some(Some((Some(a1), Some(b1)))), Some(Some((Some(a2), Some(b2))))) =
            (rep.get(l), rep.get(r))
        else {
            continue;
        };
        let ia = ca[*a1 as usize] == ca[*a2 as usize];
        let ib = cb[*b1 as usize] == cb[*b2 as usize];
        match (ia, ib) {
            (true, true) => m.connect_both += 1,
            (true, false) => m.connect_a_only += 1,
            (false, true) => m.connect_b_only += 1,
            (false, false) => m.connect_neither += 1,
        }
    }
    m
}
