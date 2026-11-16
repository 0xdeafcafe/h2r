//! Superset and tightness of a program against brute force.
#![allow(dead_code)]
use mopp::geom::{Surface, polygon_overlaps};
use mopp::{Aabb, Code};

/// xorshift64*, deterministic.
pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// Boxes around random surface vertices: half-sizes from 2 cm to 8 m, the
/// sizes of grenades, bodies and vehicles, plus a few spanning a room.
pub fn sample_boxes(surfaces: &[Surface], n: usize, seed: u64) -> Vec<Aabb> {
    let mut rng = Rng(seed | 1);
    let verts: Vec<[f32; 3]> = surfaces
        .iter()
        .flat_map(|s| s.vertices.iter().copied())
        .collect();
    (0..n)
        .map(|i| {
            let v = verts[(rng.next() % verts.len() as u64) as usize];
            let half = if i % 16 == 0 {
                2.0 + 20.0 * rng.unit()
            } else {
                0.02 * 400f32.powf(rng.unit())
            };
            let c = [0, 1, 2].map(|a| v[a] + (rng.unit() - 0.5) * 2.0 * half);
            let h = [0, 1, 2].map(|_| half * (0.25 + rng.unit()));
            Aabb {
                min: [0, 1, 2].map(|a| c[a] - h[a]),
                max: [0, 1, 2].map(|a| c[a] + h[a]),
            }
        })
        .collect()
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Metrics {
    pub queries: usize,
    /// Surfaces whose polygon truly touches the box (summed over queries).
    pub truth: usize,
    /// Surfaces whose bounding box overlaps the query box.
    pub bounds: usize,
    /// Distinct structure keys the program returned.
    pub returned: usize,
    /// True surfaces the program failed to return.
    pub misses: usize,
    /// The same through the engine-exact sphere query, with the sphere
    /// that circumscribes the box: returned keys and misses.
    pub sphere_returned: usize,
    pub sphere_misses: usize,
}
impl Metrics {
    pub fn add(&mut self, o: &Metrics) {
        self.queries += o.queries;
        self.truth += o.truth;
        self.bounds += o.bounds;
        self.returned += o.returned;
        self.misses += o.misses;
        self.sphere_returned += o.sphere_returned;
        self.sphere_misses += o.sphere_misses;
    }
    /// Returned per truly touching surface.
    pub fn ratio(&self) -> f64 {
        self.returned as f64 / self.truth.max(1) as f64
    }
    /// Returned per bounds-overlapping surface (1.0 is as tight as a box
    /// test on each surface can be).
    pub fn bounds_ratio(&self) -> f64 {
        self.returned as f64 / self.bounds.max(1) as f64
    }
}

fn shrink(b: &Aabb, e: f32) -> Aabb {
    Aabb {
        min: b.min.map(|v| v + e),
        max: b.max.map(|v| v - e),
    }
}
fn overlaps(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| a.min[i] <= b.max[i] && b.min[i] <= a.max[i])
}

/// Query each box through `code` (box model) and compare with brute force
/// over `surfaces` (structure keys only). Truth uses the box shrunk by
/// `eps` world units so float ties at a face don't count.
pub fn measure(code: &Code, surfaces: &[Surface], boxes: &[Aabb], eps: f32) -> Metrics {
    let bounds: Vec<Aabb> = surfaces.iter().map(|s| s.bounds()).collect();
    let mut m = Metrics::default();
    let mut hit = vec![false; surfaces.len()];
    for b in boxes {
        let keys = mopp::query_aabb(code, b).expect("program faults");
        let mark = |hit: &mut [bool], keys: Vec<u32>| {
            hit.iter_mut().for_each(|h| *h = false);
            for k in keys {
                if k >> 29 == mopp::geom::STRUCTURE_KEY_TYPE
                    && let Some(h) = hit.get_mut((k & 0xffff) as usize)
                {
                    *h = true;
                }
            }
        };
        let center = [0, 1, 2].map(|a| (b.min[a] + b.max[a]) * 0.5);
        let radius = (0..3)
            .map(|a| (b.max[a] - b.min[a]) * 0.5)
            .map(|h| h * h)
            .sum::<f32>()
            .sqrt();
        let mut sphere_hit = vec![false; surfaces.len()];
        mark(
            &mut sphere_hit,
            mopp::query_sphere(code, center, radius).expect("program faults"),
        );
        m.sphere_returned += sphere_hit.iter().filter(|h| **h).count();
        mark(&mut hit, keys);
        m.queries += 1;
        m.returned += hit.iter().filter(|h| **h).count();
        let t = shrink(b, eps);
        for (i, s) in surfaces.iter().enumerate() {
            if !overlaps(&bounds[i], b) {
                continue;
            }
            m.bounds += 1;
            if overlaps(&bounds[i], &t) && polygon_overlaps(&s.vertices, &t) {
                m.truth += 1;
                if !hit[i] {
                    m.misses += 1;
                }
                if !sphere_hit[i] {
                    m.sphere_misses += 1;
                }
            }
        }
    }
    m
}
