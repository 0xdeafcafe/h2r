//! Bind ground firing positions to the generated sectors in their own BSP.
//! Halo 1 stores a collision surface at +0x14. Halo 2 stores a sector there.

use pathfind::geom::{V3, contains_xy, dot, newell, sub};
use tags::pathfinding::PathfindingData;

use crate::ai::{Error, Result, h2::Zone};

struct Floor {
    ring: Vec<V3>,
    normal: V3,
    bounds: [V3; 2],
}

impl Floor {
    fn new(ring: Vec<V3>) -> Option<Self> {
        let n = newell(&ring);
        if n[2].abs() < f32::EPSILON {
            return None;
        }
        let len = dot(n, n).sqrt().copysign(n[2]);
        let normal = n.map(|x| x / len);
        let min = std::array::from_fn(|i| ring.iter().map(|v| v[i]).fold(f32::INFINITY, f32::min));
        let max =
            std::array::from_fn(|i| ring.iter().map(|v| v[i]).fold(f32::NEG_INFINITY, f32::max));
        Some(Self {
            ring,
            normal,
            bounds: [min, max],
        })
    }

    fn nearest(&self, p: V3) -> V3 {
        let n = self.normal;
        if contains_xy(&self.ring, p[0], p[1]) {
            // Keep the author's x and y, including on a ramp.
            return [p[0], p[1], p[2] - dot(n, sub(p, self.ring[0])) / n[2]];
        }
        let mut best = self.ring[0];
        let mut distance = f32::INFINITY;
        for (a, b) in self.ring.iter().zip(self.ring.iter().cycle().skip(1)) {
            let edge = sub(*b, *a);
            let t = (dot(sub(p, *a), edge) / dot(edge, edge)).clamp(0.0, 1.0);
            let q = std::array::from_fn(|i| a[i] + t * edge[i]);
            let delta = sub(p, q);
            let d = dot(delta, delta);
            if d < distance {
                distance = d;
                best = q;
            }
        }
        best
    }
}

/// Polygon and plane data decoded once per BSP.
pub struct Floors(Vec<Option<Floor>>);

impl Floors {
    pub fn new(pf: &PathfindingData) -> Self {
        Self(
            (0..pf.sectors.len())
                .map(|i| {
                    let ring = pf
                        .sector_polygon(i)?
                        .into_iter()
                        .map(|v| pf.vertices.get(v).copied())
                        .collect::<Option<Vec<_>>>()?;
                    Floor::new(ring)
                })
                .collect(),
        )
    }

    fn project(&self, p: V3, source: Option<u32>) -> Option<(usize, V3, V3)> {
        if let Some(index) = source.map(|s| s as usize)
            && let Some(Some(f)) = self.0.get(index)
            && contains_xy(&f.ring, p[0], p[1])
        {
            let q = f.nearest(p);
            if (q[2] - p[2]).abs() <= 0.25 {
                return Some((index, q, f.normal));
            }
        }
        let mut best = None;
        let mut limit = f32::INFINITY;
        // Prefer the authored surface when two sectors meet at an edge.
        for i in source
            .map(|s| s as usize)
            .into_iter()
            .chain(0..self.0.len())
        {
            let Some(Some(floor)) = self.0.get(i) else {
                continue;
            };
            let delta =
                std::array::from_fn(|j| p[j] - p[j].clamp(floor.bounds[0][j], floor.bounds[1][j]));
            if dot(delta, delta) >= limit {
                continue;
            }
            let q = floor.nearest(p);
            let delta = sub(p, q);
            let d = dot(delta, delta);
            if d < limit {
                best = Some((i, q, floor.normal));
                limit = d;
                if limit == 0.0 {
                    break;
                }
            }
        }
        best
    }

    /// Project a script-created ground start onto the same mesh as its firing position.
    pub fn ground_point(&self, point: V3) -> Option<V3> {
        self.project(point, None).map(|(_, point, _)| point)
    }

    /// Resolve a ground zone in its assigned BSP and recompute the statistics
    /// of its areas from the projected points. The cluster query samples just
    /// above the floor (the boundary itself may belong to solid space).
    pub fn bind(
        &self,
        zone: &mut Zone,
        cluster: impl Fn(V3) -> Option<i16>,
    ) -> Result<()> {
        for f in &mut zone.firing_positions {
            let source = (f.sector != u32::MAX).then_some(f.sector);
            let (sector, point, n) = self.project(f.position, source).ok_or_else(|| {
                Error::Invalid(format!("zone {} has no walkable sectors", zone.name))
            })?;
            f.position = point;
            f.sector = sector as u32;
            f.normal = [n[1].atan2(n[0]), n[2].clamp(-1.0, 1.0).asin()];
            f.cluster = cluster([point[0], point[1], point[2] + 0.01]).unwrap_or(f.cluster);
        }
        for area in &mut zone.areas {
            if area.count == 0 {
                continue;
            }
            let points = usize::try_from(area.first)
                .ok()
                .zip(usize::try_from(area.count).ok())
                .and_then(|(first, count)| zone.firing_positions.get(first..first + count))
                .ok_or_else(|| {
                    Error::Invalid(format!(
                        "zone {}: area {} point range",
                        zone.name, area.name
                    ))
                })?;
            area.clusters.fill(0);
            for point in points {
                if let Ok(c) = usize::try_from(point.cluster)
                    && let Some(word) = area.clusters.get_mut(c / 32)
                {
                    *word |= 1 << (c % 32);
                }
            }
            let count = points.len() as f32;
            area.mean =
                std::array::from_fn(|j| points.iter().map(|f| f.position[j]).sum::<f32>() / count);
            area.deviation = (points
                .iter()
                .map(|f| {
                    let d = sub(f.position, area.mean);
                    dot(d, d)
                })
                .sum::<f32>()
                / count)
                .sqrt();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floors_keep_xy_on_ramps_and_snap_to_the_edge_off_the_mesh() {
        let ramp = |z| {
            Floor::new(vec![
                [0., 0., z],
                [1., 0., z + 0.5],
                [1., 1., z + 0.5],
                [0., 1., z],
            ])
        };
        let floors = Floors(vec![ramp(0.), ramp(5.)]);
        let (i, p, _) = floors.project([0.5, 0.25, 5.4], Some(0)).unwrap();
        assert_eq!((i, p), (1, [0.5, 0.25, 5.25]));
        let (i, p, _) = floors.project([1.75, 0.5, 0.5], None).unwrap();
        assert_eq!((i, p), (0, [1., 0.5, 0.5]));
    }
}
