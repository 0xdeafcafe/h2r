//! The bounce search's samplers (docs/runtime/bounce-search.md,
//! "Sampling"): a seeded generator, a Latin hypercube and a grid over the
//! seven axes, and the refinement that puts new points between the
//! nearest points of opposite outcome.

use serde::{Deserialize, Serialize};

use super::{N, Vector};

/// SplitMix64: a small seeded generator, the same on every host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in 0..1 (never 1).
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// A number in 0..n (n at least 1).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

/// A seed from a base seed and a name (FNV-1a over the name's bytes), so
/// each site samples its own points whatever run it lands in.
pub fn seed_for(seed: u64, name: &str) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64 ^ seed;
    for b in name.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// One axis of the input space: its range and, when `step` is over 0,
/// the grid its values are rounded to (from `lo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Axis {
    pub lo: f32,
    pub hi: f32,
    pub step: f32,
}

impl Axis {
    pub fn span(&self) -> f32 {
        self.hi - self.lo
    }

    /// One value only: the axis is held, not sampled.
    pub fn fixed(&self) -> bool {
        self.span() <= 0.0
    }

    /// `v` on the axis's grid, inside its range.
    pub fn snap(&self, v: f32) -> f32 {
        let v = if self.step > 0.0 {
            self.lo + ((v - self.lo) / self.step).round() * self.step
        } else {
            v
        };
        v.clamp(self.lo, self.hi.max(self.lo))
    }

    /// The value `u` (0..1) of the way along.
    pub fn at(&self, u: f64) -> f32 {
        self.snap(self.lo + (f64::from(self.span()) * u) as f32)
    }
}

/// A Latin hypercube of `n` points: each axis is cut into `n` equal
/// strata and every stratum holds exactly one point, at a random place in
/// it; the strata are paired across axes at random. A fixed axis gives
/// its value to every point.
pub fn lhs(axes: &[Axis; N], n: usize, rng: &mut Rng) -> Vec<Vector> {
    let mut out = vec![[0.0f32; N]; n];
    for (a, axis) in axes.iter().enumerate() {
        let mut strata: Vec<usize> = (0..n).collect();
        // Fisher-Yates.
        for i in (1..n).rev() {
            strata.swap(i, rng.below(i + 1));
        }
        for (p, &s) in out.iter_mut().zip(&strata) {
            let u = (s as f64 + rng.unit()) / n as f64;
            p[a] = if axis.fixed() { axis.lo } else { axis.at(u) };
        }
    }
    out
}

/// The most points a grid plans.
pub const MAX_GRID: usize = 200_000;

/// A full grid: `counts[a]` values on axis `a`, the two ends included (one
/// value: the middle; a fixed axis: its value). The first axis varies
/// slowest. `Err` over [`MAX_GRID`] points.
pub fn grid(axes: &[Axis; N], counts: &[u32; N]) -> Result<Vec<Vector>, String> {
    let values: Vec<Vec<f32>> = axes
        .iter()
        .zip(counts)
        .map(|(axis, &c)| {
            if axis.fixed() {
                vec![axis.lo]
            } else if c <= 1 {
                vec![axis.at(0.5)]
            } else {
                (0..c)
                    .map(|i| axis.at(f64::from(i) / f64::from(c - 1)))
                    .collect()
            }
        })
        .collect();
    let total = values
        .iter()
        .try_fold(1usize, |n, v| n.checked_mul(v.len()))
        .filter(|&n| n <= MAX_GRID)
        .ok_or_else(|| format!("grid: at most {MAX_GRID} points"))?;
    let mut out = Vec::with_capacity(total);
    let mut index = [0usize; N];
    for _ in 0..total {
        let mut p = [0.0f32; N];
        for (a, v) in values.iter().enumerate() {
            p[a] = v[index[a]];
        }
        out.push(p);
        for a in (0..N).rev() {
            index[a] += 1;
            if index[a] < values[a].len() {
                break;
            }
            index[a] = 0;
        }
    }
    Ok(out)
}

/// The distance between two points with each axis in its own unit
/// (`scale`), Euclidean. An axis whose scale is not over 0 is left out.
pub fn distance(a: &Vector, b: &Vector, scale: &[f32; N]) -> f32 {
    let mut sum = 0.0f32;
    for (i, &s) in scale.iter().enumerate() {
        if s > 0.0 {
            let d = (a[i] - b[i]) / s;
            sum += d * d;
        }
    }
    sum.sqrt()
}

/// New points on the edge of the bounce region: for every point, its
/// nearest point of the other outcome ([`distance`] under `scale`); each
/// such pair further apart than `min_gap` gives its midpoint, the widest
/// pairs first, at most `want`. `bounced[i]` is `None` for a point with
/// no outcome (it takes no part). A midpoint that is already a point is
/// left out. No pair of opposite outcomes, no points.
pub fn refine(
    points: &[Vector],
    bounced: &[Option<bool>],
    axes: &[Axis; N],
    scale: &[f32; N],
    want: usize,
    min_gap: f32,
) -> Vec<Vector> {
    let with = |b: bool| -> Vec<usize> {
        (0..points.len().min(bounced.len()))
            .filter(|&i| bounced[i] == Some(b))
            .collect()
    };
    let (yes, no) = (with(true), with(false));
    if yes.is_empty() || no.is_empty() || want == 0 {
        return Vec::new();
    }
    let nearest = |i: usize, others: &[usize]| -> (usize, f32) {
        let mut best = (others[0], f32::INFINITY);
        for &j in others {
            let d = distance(&points[i], &points[j], scale);
            if d < best.1 {
                best = (j, d);
            }
        }
        best
    };
    // (bounced, not bounced, distance).
    let mut pairs: Vec<(usize, usize, f32)> = Vec::new();
    for &i in &yes {
        let (j, d) = nearest(i, &no);
        pairs.push((i, j, d));
    }
    for &j in &no {
        let (i, d) = nearest(j, &yes);
        pairs.push((i, j, d));
    }
    // Widest first; ties by the indices, so the order is the same on
    // every host.
    pairs.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
            .then(a.1.cmp(&b.1))
    });
    let mut out: Vec<Vector> = Vec::new();
    let mut seen: Vec<(usize, usize)> = Vec::new();
    for (i, j, d) in pairs {
        if out.len() >= want {
            break;
        }
        if d.is_nan() || d <= min_gap || seen.contains(&(i, j)) {
            continue;
        }
        seen.push((i, j));
        let mut mid = [0.0f32; N];
        for (a, axis) in axes.iter().enumerate() {
            mid[a] = axis.snap((points[i][a] + points[j][a]) / 2.0);
        }
        if points.contains(&mid) || out.contains(&mid) {
            continue;
        }
        out.push(mid);
    }
    out
}
