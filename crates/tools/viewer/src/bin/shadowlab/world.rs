//! What the comparison renders: a level (receiver, optionally a caster) and
//! object casters, from the disc or invented.
use crate::geom::{TopoStats, Topology, TriMesh, V3, WELD_EPSILON, add, bounds, sub};
use crate::level::Tri;

pub struct Caster {
    pub name: String,
    /// World-space triangles at the drawn level of detail.
    pub soup: Vec<Tri>,
    /// Topology per level of detail (label, stats), in the caster's space.
    pub lods: Vec<(String, TopoStats)>,
}

pub struct World {
    pub title: String,
    pub level: Vec<Tri>,
    /// Level pieces counted separately (label, stats), e.g. the instance
    /// definitions, which are the BSP's props.
    pub level_pieces: Vec<(String, TopoStats)>,
    pub casters: Vec<Caster>,
}

pub fn stats(soup: &[Tri]) -> TopoStats {
    Topology::build(&TriMesh::from_soup(soup, WELD_EPSILON)).stats
}

pub fn translate(soup: &[Tri], by: V3) -> Vec<Tri> {
    soup.iter().map(|t| t.map(|p| add(p, by))).collect()
}

/// Rotate about +z by `yaw` radians, then translate.
pub fn place(soup: &[Tri], at: V3, yaw: f32) -> Vec<Tri> {
    let (s, c) = yaw.sin_cos();
    soup.iter()
        .map(|t| t.map(|p| add([c * p[0] - s * p[1], s * p[0] + c * p[1], p[2]], at)))
        .collect()
}

/// The highest point of `level` straight below `(x, y, from_z)`, if any.
pub fn ground_below(level: &[Tri], x: f32, y: f32, from_z: f32) -> Option<f32> {
    let mut best: Option<f32> = None;
    for t in level {
        let [a, b, c] = *t;
        // Barycentric in the xy plane.
        let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if d.abs() < 1e-12 {
            continue;
        }
        let l0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d;
        let l1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d;
        let l2 = 1.0 - l0 - l1;
        if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
            continue;
        }
        let z = l0 * a[2] + l1 * b[2] + l2 * c[2];
        if z <= from_z && best.is_none_or(|b| z > b) {
            best = Some(z);
        }
    }
    best
}

pub fn soup_bounds(soup: &[Tri]) -> Option<(V3, V3)> {
    bounds(soup.iter().flat_map(|t| t.iter().copied()))
}

/// Invented test scene: a ground plane, and over it a cube, an open box and
/// a raised plane with a hole, spread along x.
pub fn synthetic() -> World {
    use crate::geom::shapes::{cube, open_box, plane};
    let ground = plane(8.0, 0.0, 8, None);
    let casters = vec![
        ("cube", cube([-5.0, -1.0, 0.5], [-3.0, 1.0, 2.5])),
        ("open box", open_box([-1.0, -1.0, 0.5], [1.0, 1.0, 2.5])),
        (
            "plane with hole",
            translate(&plane(1.5, 2.5, 6, Some((2, 3))), [4.0, 0.0, 0.0]),
        ),
    ];
    World {
        title: "synthetic".into(),
        level_pieces: Vec::new(),
        casters: casters
            .into_iter()
            .map(|(name, soup)| Caster {
                name: name.into(),
                lods: vec![("lod 0".into(), stats(&soup))],
                soup,
            })
            .collect(),
        level: ground,
    }
}

/// Centre and radius of the casters (or of `fallback` when there are none).
pub fn focus(w: &World) -> (V3, f32) {
    let all: Vec<Tri> = w
        .casters
        .iter()
        .flat_map(|c| c.soup.iter().copied())
        .collect();
    let b = soup_bounds(&all).or_else(|| core_bounds(&w.level));
    let Some((lo, hi)) = b else {
        return ([0.0; 3], 1.0);
    };
    let c = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
    (c, (0.5 * crate::geom::length(sub(hi, lo))).max(0.25))
}

/// The dense core of a level: 1st to 99th percentile of centroids per axis.
pub fn core_bounds(level: &[Tri]) -> Option<(V3, V3)> {
    if level.is_empty() {
        return None;
    }
    let mut lo = [0.0; 3];
    let mut hi = [0.0; 3];
    for k in 0..3 {
        let mut c: Vec<f32> = level
            .iter()
            .map(|t| (t[0][k] + t[1][k] + t[2][k]) / 3.0)
            .collect();
        c.sort_by(f32::total_cmp);
        lo[k] = c[c.len() / 100];
        hi[k] = c[(c.len() * 99 / 100).min(c.len() - 1)];
    }
    Some((lo, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_scene_has_one_closed_and_two_open_casters() {
        let w = synthetic();
        let closed: Vec<bool> = w.casters.iter().map(|c| c.lods[0].1.closed()).collect();
        assert_eq!(closed, [true, false, false]);
        assert_eq!(w.casters[1].lods[0].1.open_edges, 4);
        assert_eq!(w.casters[2].lods[0].1.open_edges, 24 + 8);
        assert!(!stats(&w.level).closed());
    }

    #[test]
    fn ground_is_found_below_a_point() {
        let w = synthetic();
        assert_eq!(ground_below(&w.level, 1.3, -2.2, 10.0), Some(0.0));
        assert_eq!(ground_below(&w.level, 30.0, 0.0, 10.0), None);
        // Below the starting height only.
        assert_eq!(ground_below(&w.level, 1.0, 1.0, -1.0), None);
    }

    #[test]
    fn place_rotates_about_z_then_moves() {
        let t = place(
            &[[[1.0, 0.0, 0.0]; 3]],
            [0.0, 0.0, 5.0],
            std::f32::consts::FRAC_PI_2,
        );
        let p = t[0][0];
        assert!(p[0].abs() < 1e-6 && (p[1] - 1.0).abs() < 1e-6 && p[2] == 5.0);
    }
}
