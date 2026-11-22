//! Split in the stored, linear lightmap units (one channel step is one).
//! Each mip has its own footprint and retail samples. The fit is shared by
//! the whole BSP, including surfaces for which no shadow edge can be seen.
use anyhow::{Result, ensure};
use scenario_edit::geometry::{Vec3 as V3, cross, dot, sub};
use std::array::from_fn;
use std::collections::VecDeque;

use super::lamp::PlacedLamp;

type Rgb = [u8; 3];

#[derive(Clone, Copy, Debug)]
pub(super) struct Triangle {
    pub position: [V3; 3],
    pub uv: [[f32; 2]; 3],
    pub chart: u32,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Vertex {
    pub position: V3,
    /// Zero marks a source vertex outside the selected light's receiver set.
    pub normal: V3,
    pub retail: Rgb,
}

fn unit(a: V3) -> Option<V3> {
    let length = dot(a, a).sqrt();
    (length.is_finite() && length > 0.0).then(|| a.map(|x| x / length))
}

const SIDE: usize = 4;
const SAMPLES: usize = SIDE * SIDE;

pub(super) enum Source {
    Sky { direction: V3, rays: [V3; SAMPLES] },
    Placed(PlacedLamp),
}

impl Source {
    pub fn sky(direction: V3, angular_radius: f32) -> Result<Self> {
        let direction = unit(direction).ok_or_else(|| anyhow::anyhow!("invalid sun direction"))?;
        ensure!(
            (0.0..std::f32::consts::FRAC_PI_2).contains(&angular_radius),
            "invalid sun angular radius {angular_radius} radians"
        );
        let axis = if direction[2].abs() < 0.9 {
            [0., 0., 1.]
        } else {
            [1., 0., 0.]
        };
        let u = unit(cross(direction, axis)).unwrap();
        let v = cross(direction, u);
        // Uniform solid angle, stratified in cos(theta). Permute the strata
        // over the 4x4 footprint so adjacent rows do not all see one radius.
        let rays = from_fn(|i| {
            let stratum = (i * 7) % SAMPLES;
            let t = (stratum as f32 + 0.5) / SAMPLES as f32;
            let cos = 1.0 - t * (1.0 - angular_radius.cos());
            let sin = (1.0 - cos * cos).max(0.0).sqrt();
            let phi = stratum as f32 * 2.3999631; // golden angle
            let (az_sin, az_cos) = phi.sin_cos();
            from_fn(|j| direction[j] * cos + sin * (u[j] * az_cos + v[j] * az_sin))
        });
        Ok(Self::Sky { direction, rays })
    }

    fn sample(
        &self,
        position: V3,
        normal: V3,
        i: usize,
        visible: &impl Fn(V3, V3, f32) -> [bool; 2],
    ) -> (f32, [bool; 2]) {
        // Keep the offset small compared with a world unit, including on
        // thin double-sided scenery. A point lamp's range is from this
        // offset origin, so the ray ends exactly at the emitter.
        let origin = from_fn(|k| position[k] + normal[k] * 0.0001);
        match self {
            Self::Sky { direction, rays } => (
                dot(normal, *direction).clamp(0.0, 1.0),
                visible(origin, rays[i], f32::INFINITY),
            ),
            Self::Placed(lamp) => match (lamp.sample(position), lamp.sample(origin)) {
                (Some((l, _, gain)), Some((dir, distance, _))) => (
                    dot(normal, l).clamp(0.0, 1.0) * gain,
                    visible(origin, dir, distance),
                ),
                // Outside the sphere the falloff has already reached zero.
                // Counting this as a hit would apply the falloff a second
                // time on texels straddling the radius.
                _ => (0.0, [true; 2]),
            },
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Cell {
    chart: u32,
    mixed_chart: bool,
    mask: u16,
    // During rasterization the sums span unique covered subpixels only.
    gain: f32,
    lit: [u8; 2],
}

impl Cell {
    fn add(&mut self, i: usize, gain: f32, lit: [bool; 2]) {
        self.mask |= 1 << i;
        self.gain += gain;
        for (sum, visible) in self.lit.iter_mut().zip(lit) {
            *sum += u8::from(visible);
        }
    }

    fn visibility(self, arm: usize) -> f32 {
        self.lit[arm] as f32 / self.mask.count_ones().max(1) as f32
    }

    fn connected(self, other: Self) -> bool {
        self.mask != 0
            && other.mask != 0
            && !self.mixed_chart
            && !other.mixed_chart
            && self.chart == other.chart
    }
}

pub(super) struct Trace {
    width: usize,
    cells: Vec<Cell>,
}

fn cross2(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

pub(super) fn trace(
    width: usize,
    height: usize,
    triangles: &[Triangle],
    source: &Source,
    visible: &impl Fn(V3, V3, f32) -> [bool; 2],
) -> Result<Trace> {
    let len = width
        .checked_mul(height)
        .ok_or_else(|| anyhow::anyhow!("page dimensions overflow"))?;
    ensure!(len > 0, "empty page dimensions {width} × {height}");
    let mut cells = vec![Cell::default(); len];
    for tri in triangles {
        ensure!(
            tri.uv
                .iter()
                .flatten()
                .chain(tri.position.iter().flatten())
                .all(|n| n.is_finite()),
            "non-finite receiver geometry in chart {}",
            tri.chart
        );
        let [p, q, r] = tri.position;
        let pq = sub(q, p);
        let pr = sub(r, p);
        let Some(normal) = unit(cross(pq, pr)) else {
            continue;
        };
        let uv = tri.uv.map(|[u, v]| [u * width as f32, v * height as f32]);
        let ab = from_fn(|i| uv[1][i] - uv[0][i]);
        let ac = from_fn(|i| uv[2][i] - uv[0][i]);
        let det = cross2(ab, ac);
        if det.abs() < 1e-10 {
            continue;
        }
        let range = |axis: usize, limit: usize| {
            let values = uv.map(|p| p[axis]);
            let lo = values.into_iter().fold(f32::INFINITY, f32::min).floor() as usize;
            let hi = values.into_iter().fold(f32::NEG_INFINITY, f32::max).ceil() as usize;
            lo.min(limit)..hi.min(limit)
        };
        for y in range(1, height) {
            for x in range(0, width) {
                let cell = &mut cells[y * width + x];
                for i in 0..SAMPLES {
                    let sample = [
                        x as f32 + ((i % SIDE) as f32 + 0.5) / SIDE as f32 - uv[0][0],
                        y as f32 + ((i / SIDE) as f32 + 0.5) / SIDE as f32 - uv[0][1],
                    ];
                    let b = cross2(sample, ac) / det;
                    let c = cross2(ab, sample) / det;
                    if b < 0.0 || c < 0.0 || b + c > 1.0 {
                        continue;
                    }
                    if cell.mask == 0 {
                        cell.chart = tri.chart;
                    } else if cell.chart != tri.chart {
                        cell.mixed_chart = true;
                    }
                    if cell.mask & (1 << i) != 0 {
                        continue;
                    }
                    let position = from_fn(|j| p[j] + b * pq[j] + c * pr[j]);
                    let (gain, lit) = source.sample(position, normal, i, visible);
                    cell.add(i, gain, lit);
                }
            }
        }
    }
    for cell in &mut cells {
        cell.gain /= cell.mask.count_ones().max(1) as f32;
    }
    Ok(Trace { width, cells })
}

// Four-connected, symmetric adjacency. Reject an atlas neighbour across a
// chart seam even when both charts reach into the same low-mip texel.
impl Trace {
    fn neighbours(&self, index: usize, visit: &mut dyn FnMut(usize)) {
        let x = index % self.width;
        let mut emit = |n| {
            if self.cells[index].connected(self.cells[n]) {
                visit(n);
            }
        };
        if x > 0 {
            emit(index - 1);
        }
        if x + 1 < self.width {
            emit(index + 1);
        }
        if index >= self.width {
            emit(index - self.width);
        }
        if index + self.width < self.cells.len() {
            emit(index + self.width);
        }
    }
}

#[derive(Clone, Copy)]
struct Pair {
    delta: [f32; 3],
    weight: f32,
}

#[derive(Default)]
pub(super) struct FitSamples {
    pairs: [Vec<Pair>; 2],
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Fit {
    pub k: V3,
    pub pairs: usize,
    /// Median absolute error in the predicted RGB difference, in channel steps.
    pub median_error: f32,
}

fn median(values: &mut [f32]) -> f32 {
    values.sort_unstable_by(f32::total_cmp);
    let n = values.len();
    (values[(n - 1) / 2] + values[n / 2]) * 0.5
}

impl FitSamples {
    pub fn add(&mut self, trace: &Trace, retail: &[Rgb]) {
        assert_eq!(trace.cells.len(), retail.len());
        for (i, &a) in trace.cells.iter().enumerate() {
            trace.neighbours(i, &mut |j| {
                let b = trace.cells[j];
                if i >= j
                    || (a.gain - b.gain).abs() > 0.125
                    || retail[i].contains(&255)
                    || retail[j].contains(&255)
                {
                    return;
                }
                for arm in 0..2 {
                    let va = a.visibility(arm);
                    let vb = b.visibility(arm);
                    let weight = a.gain * va - b.gain * vb;
                    // Flat illumination gradients and chart boundaries cannot
                    // tell us how much of a retail edge is due to this light.
                    if (va - vb).abs() < 0.125 || weight.abs() < 0.0625 {
                        continue;
                    }
                    self.pairs[arm].push(Pair {
                        delta: from_fn(|k| retail[i][k] as f32 - retail[j][k] as f32),
                        weight,
                    });
                }
            });
        }
    }

    pub fn fit(&self, arm: usize) -> Option<Fit> {
        let pairs = &self.pairs[arm];
        if pairs.is_empty() {
            return None;
        }
        // The median resists an unrelated painted edge, emitter or palette
        // jump. Negative estimates indicate that the supposed sun edge is
        // brighter in shade and go into the residual, never a negative lamp.
        let k: V3 = from_fn(|channel| {
            let mut slopes: Vec<_> = pairs.iter().map(|p| p.delta[channel] / p.weight).collect();
            median(&mut slopes).max(0.0)
        });
        let mut errors: Vec<_> = pairs
            .iter()
            .map(|p| {
                (0..3)
                    .map(|c| (p.delta[c] - k[c] * p.weight).abs())
                    .sum::<f32>()
                    / 3.0
            })
            .collect();
        Some(Fit {
            k,
            pairs: pairs.len(),
            median_error: median(&mut errors),
        })
    }
}

#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Split {
    pub closure_i: Rgb,
    pub baked: Rgb,
    pub lamp_off: Rgb,
    pub direct: Rgb,
    pub lamp_on: Rgb,
    pub visibility: u8,
}

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Stats {
    pub covered: usize,
    pub clamped: usize,
    pub edge: usize,
    pub unfilled: usize,
    pub max_closure_error: u8,
    pub max_play_error: u8,
}

pub(super) struct PageSplit {
    pub texels: Vec<Split>,
    pub stats: Stats,
}

fn byte(x: f32) -> u8 {
    x.round().clamp(0.0, 255.0) as u8
}

pub(super) fn split_page(trace: &Trace, retail: &[Rgb], k: V3, arm: usize) -> PageSplit {
    split(&trace.cells, retail, k, arm, |i, visit| {
        trace.neighbours(i, visit)
    })
}

pub(super) fn split_vertices(
    vertices: &[Vertex],
    edges: &[[usize; 2]],
    source: &Source,
    visible: &impl Fn(V3, V3, f32) -> [bool; 2],
    k: V3,
    arm: usize,
) -> Result<PageSplit> {
    let mut adjacency = vec![Vec::new(); vertices.len()];
    for &[a, b] in edges {
        ensure!(
            a < vertices.len() && b < vertices.len(),
            "invalid vertex adjacency {a}–{b}"
        );
        adjacency[a].push(b);
        adjacency[b].push(a);
    }
    let mut cells = Vec::with_capacity(vertices.len());
    let mut retail = Vec::with_capacity(vertices.len());
    for vertex in vertices {
        ensure!(
            vertex.position.iter().all(|n| n.is_finite()),
            "invalid receiver position"
        );
        if vertex.normal == [0.; 3] {
            cells.push(Cell::default());
            retail.push(vertex.retail);
            continue;
        }
        let normal =
            unit(vertex.normal).ok_or_else(|| anyhow::anyhow!("invalid vertex receiver normal"))?;
        let mut cell = Cell::default();
        for i in 0..SAMPLES {
            let (gain, lit) = source.sample(vertex.position, normal, i, visible);
            cell.add(i, gain, lit);
        }
        cell.gain /= SAMPLES as f32;
        cells.push(cell);
        retail.push(vertex.retail);
    }
    Ok(split(&cells, &retail, k, arm, |i, visit| {
        for &j in &adjacency[i] {
            visit(j);
        }
    }))
}

fn split(
    cells: &[Cell],
    retail: &[Rgb],
    k: V3,
    arm: usize,
    neighbours: impl Fn(usize, &mut dyn FnMut(usize)),
) -> PageSplit {
    assert_eq!(cells.len(), retail.len());
    let mut stats = Stats::default();
    let mut edge = vec![false; cells.len()];
    let mut texels = Vec::with_capacity(cells.len());
    for (i, (&cell, &rgb)) in cells.iter().zip(retail).enumerate() {
        let v = cell.visibility(arm);
        let gain = cell.gain;
        let direct = k.map(|x| byte(x * gain));
        let raw = k.map(|x| x * gain * v);
        let baked = from_fn(|c| byte(raw[c]).min(rgb[c]));
        let closure_i = from_fn(|c| rgb[c] - baked[c]);
        if cell.mask != 0 {
            stats.covered += 1;
            stats.clamped += usize::from((0..3).any(|c| raw[c] > rgb[c] as f32));
            neighbours(i, &mut |j| {
                if (v - cells[j].visibility(arm)).abs() >= 1.0 / SAMPLES as f32 {
                    edge[i] = true;
                }
            });
            stats.edge += usize::from(edge[i]);
        }
        texels.push(Split {
            closure_i,
            baked,
            lamp_off: closure_i,
            direct,
            lamp_on: rgb,
            visibility: byte(v * 255.0),
        });
    }

    // Propagate the nearest stable indirect sample from EACH side, through
    // edge texels on this same surface only. Inverse-distance interpolation
    // reproduces a linear gradient across a straight band. If a chart ends
    // before the other side, keep the actual residual and count the gap.
    if stats.edge != 0 {
        let anchors = |sun_side: bool| {
            let mut seeds = vec![None; cells.len()];
            let mut queue = VecDeque::new();
            for (i, &cell) in cells.iter().enumerate() {
                if edge[i] || cell.mask == 0 || (cell.visibility(arm) >= 0.5) != sun_side {
                    continue;
                }
                neighbours(i, &mut |j| {
                    if edge[j] && seeds[j].is_none() {
                        seeds[j] = Some((i, 1usize));
                        queue.push_back(j);
                    }
                });
            }
            while let Some(i) = queue.pop_front() {
                let (seed, distance) = seeds[i].unwrap();
                neighbours(i, &mut |j| {
                    if edge[j] && seeds[j].is_none() {
                        seeds[j] = Some((seed, distance + 1));
                        queue.push_back(j);
                    }
                });
            }
            seeds
        };
        let dark = anchors(false);
        let light = anchors(true);
        for (i, &in_band) in edge.iter().enumerate() {
            if !in_band {
                continue;
            }
            match (dark[i], light[i]) {
                (Some((a, da)), Some((b, db))) => {
                    texels[i].lamp_off = from_fn(|c| {
                        byte(
                            (texels[a].closure_i[c] as f32 * db as f32
                                + texels[b].closure_i[c] as f32 * da as f32)
                                / (da + db) as f32,
                        )
                    });
                }
                _ => stats.unfilled += 1,
            }
        }
    }
    for (i, texel) in texels.iter_mut().enumerate() {
        for c in 0..3 {
            // Play uses exactly the stored page bytes, including the rounded
            // visibility. Compare these same bytes in the contact report.
            texel.lamp_on[c] = byte(
                texel.lamp_off[c] as f32 + texel.direct[c] as f32 * texel.visibility as f32 / 255.0,
            );
            stats.max_closure_error = stats
                .max_closure_error
                .max(retail[i][c].abs_diff(texel.closure_i[c].saturating_add(texel.baked[c])));
            stats.max_play_error = stats
                .max_play_error
                .max(retail[i][c].abs_diff(texel.lamp_on[c]));
        }
    }
    PageSplit { texels, stats }
}

#[cfg(test)]
mod tests;
