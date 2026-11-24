//! The sphere mode (1, `0x00132d60`; docs/graphics/native-visibility.md,
//! "The sphere mode"): the clusters a view's sphere reaches from its start
//! cluster, by the cluster flood `0x0014a5b0` with its portal test
//! `0x0014a370` and polygon test `0x0023a160`.
//!
//! The flood is depth first. Through each portal of a cluster to a cluster
//! not yet reached, in the tag's order, the sphere must straddle the
//! portal's plane (its distance under the radius either side), reach the
//! portal's bounding sphere, and its cross-section on the plane must not lie
//! wholly outside an edge of the portal's polygon, dropped onto the two axes
//! the image's projection rows pick for the plane's normal. The set is the
//! clusters connected to the start through such portals, so the walk's
//! order does not change it.
//!
//! The arithmetic is Rust's own: the plane distance and the cross-section
//! radius are taken in double precision (the guest's x87), the rest in
//! single precision in the guest's order. The projection normal is the
//! portal's plane, from the structure's collision BSP; the guest reads it
//! from the active BSP (`[0x004e033c]`), the same tag data for a structure's
//! own BSP.

use super::Decline;
use super::flood::MAX_REGIONS;
use super::graph::{Portal, PortalGraph};

/// The image's projection rows (`0x00440b94`): for row `2 · axis +
/// positive`, the `u` and `v` axes a plane whose largest normal component
/// is `axis` (with that sign) is dropped onto. Read each frame, never
/// committed (`super::guest::projection_rows`).
pub type Rows = [[u8; 2]; 6];

/// The output capacity `0x00132d60` passes, and the count it clamps to.
pub const CAPACITY: usize = 0x200;

/// `0x00120850`: the axis of the largest normal component; ties go to z,
/// then y.
fn dominant_axis(n: [f32; 3]) -> usize {
    let [x, y, z] = n.map(f32::abs);
    if z >= y && z >= x {
        2
    } else if y >= x {
        1
    } else {
        0
    }
}

/// `0x0014a370`: whether the sphere reaches through `portal`.
fn crosses(portal: &Portal, rows: &Rows, centre: [f32; 3], radius: f32) -> bool {
    let [nx, ny, nz, d] = portal.plane;
    let [cx, cy, cz] = centre;
    let wide = |v: f32| f64::from(v);
    let distance = (wide(ny) * wide(cy) + wide(nz) * wide(cz)) + wide(cx) * wide(nx) - wide(d);
    let r = wide(radius);
    if !(r > distance && r > -distance) {
        return false;
    }
    let [sx, sy, sz, sr] = portal.sphere;
    let (dz, dx, dy) = (sz - cz, sx - cx, sy - cy);
    let reach = sr + radius;
    if reach * reach <= (dz * dz + dx * dx) + dy * dy {
        return false;
    }
    let stored = distance as f32;
    let normal = [nx, ny, nz];
    let axis = dominant_axis(normal);
    let [u, v] = rows[2 * axis + usize::from(normal[axis] > 0.0)].map(usize::from);
    let back = 0.0 - stored;
    let point: [f32; 3] = std::array::from_fn(|k| normal[k] * back + centre[k]);
    let across = (r * r - wide(stored) * wide(stored)).sqrt() as f32;
    polygon(portal, [u, v], [point[u], point[v]], across)
}

/// `0x0023a160`: false when the circle lies wholly outside some edge.
fn polygon(portal: &Portal, [u, v]: [usize; 2], p: [f32; 2], radius: f32) -> bool {
    let rr = radius * radius;
    let ring: Vec<[f32; 2]> = portal.vertices.iter().map(|q| [q[u], q[v]]).collect();
    for (i, &[ui, vi]) in ring.iter().enumerate() {
        let [uj, vj] = ring[(i + 1) % ring.len()];
        let (x0, y0) = (p[0] - ui, p[1] - vi);
        let (ex, ey) = (uj - ui, vj - vi);
        let length = ex * ex + ey * ey;
        if length == 0.0 {
            continue;
        }
        let cross = x0 * ey - y0 * ex;
        if cross > 0.0 && cross * cross > length * rr {
            return false;
        }
    }
    true
}

/// `0x0014a5b0`: the clusters reached from `start`, in visit order, at most
/// `capacity`.
pub fn sphere_clusters(
    graph: &PortalGraph,
    rows: &Rows,
    start: u16,
    centre: [f32; 3],
    radius: f32,
    capacity: usize,
) -> Result<Vec<u16>, Decline> {
    if rows.iter().flatten().any(|&a| a > 2) {
        return Err(Decline::Layout);
    }
    let cluster = |c: u16| graph.clusters.get(usize::from(c)).ok_or(Decline::Layout);
    cluster(start)?;
    let mut seen = vec![false; graph.clusters.len()];
    seen[usize::from(start)] = true;
    let mut out = vec![start];
    let mut stack: Vec<(u16, usize)> = vec![(start, 0)];
    while let Some(top) = stack.last_mut() {
        let (from, at) = *top;
        let Some(&p) = cluster(from)?.portals.get(at) else {
            stack.pop();
            continue;
        };
        top.1 += 1;
        let portal = graph.portals.get(usize::from(p)).ok_or(Decline::Layout)?;
        let other = portal.across(from);
        cluster(other)?;
        if seen[usize::from(other)] || !crosses(portal, rows, centre, radius) {
            continue;
        }
        seen[usize::from(other)] = true;
        out.push(other);
        stack.push((other, 0));
    }
    out.truncate(capacity);
    Ok(out)
}

/// `0x00132d60`: the sphere mode's clusters. None outside the structure
/// (`start` is `0xffff`); the start cluster alone when the radius is not
/// above `flood_min` (the float at `0x0045dbd8`, read each frame); else the
/// flood, clamped to [`CAPACITY`]. More than [`MAX_REGIONS`] would run the
/// guest's region list into what follows it: that declines.
pub fn view_clusters(
    graph: &PortalGraph,
    rows: &Rows,
    start: u16,
    centre: [f32; 3],
    radius: f32,
    flood_min: f32,
) -> Result<Vec<u16>, Decline> {
    if start == u16::MAX {
        return Ok(Vec::new());
    }
    // `comiss radius, min` then `jbe`: a NaN radius takes the single
    // cluster.
    let clusters = if radius > flood_min {
        sphere_clusters(graph, rows, start, centre, radius, CAPACITY)?
    } else {
        vec![start]
    };
    if clusters.len() > MAX_REGIONS {
        return Err(Decline::Overflow);
    }
    Ok(clusters)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::visibility::flood::tests::corridor;

    /// Rows that drop x onto (y, z), y onto (z, x) and z onto (x, y) for
    /// either sign: an invented table, not the image's.
    pub(crate) const ROWS: Rows = [[1, 2], [1, 2], [2, 0], [2, 0], [0, 1], [0, 1]];

    #[test]
    fn a_sphere_across_a_doorway_reaches_the_room_beyond_and_no_further() {
        // Rooms along x, doorways at x = 0, 4, 8, each a 2 by 2 square.
        let g = corridor(&[0.0, 0.0, 0.0]);
        let c = |x: f32, r: f32| sphere_clusters(&g, &ROWS, 1, [x, 0.0, 0.0], r, CAPACITY);
        assert_eq!(c(3.5, 1.0).unwrap(), [1, 2], "through the doorway at x = 4");
        assert_eq!(c(2.0, 1.0).unwrap(), [1], "it straddles no plane");
        assert_eq!(c(4.0, 4.5).unwrap(), [1, 0, 2, 3], "across x = 0 and x = 8 too");
        assert_eq!(c(4.0, 4.5).unwrap().len(), 4);
        assert_eq!(
            sphere_clusters(&g, &ROWS, 1, [4.0, 0.0, 0.0], 4.5, 2).unwrap(),
            [1, 0],
            "clamped to the capacity in visit order"
        );
    }

    #[test]
    fn a_sphere_beside_a_doorway_does_not_reach_through_its_polygon() {
        let g = corridor(&[0.0]);
        // On the plane x = 0 but 3 units to the side of the 2 by 2 doorway:
        // it straddles the plane and touches the bounding sphere (radius
        // sqrt 2), but its cross-section is outside the polygon's edge.
        let beside = sphere_clusters(&g, &ROWS, 0, [0.0, 2.5, 0.0], 1.2, CAPACITY).unwrap();
        assert_eq!(beside, [0]);
        let over = sphere_clusters(&g, &ROWS, 0, [0.0, 1.5, 0.0], 1.2, CAPACITY).unwrap();
        assert_eq!(over, [0, 1], "overlapping the edge, it reaches through");
    }

    #[test]
    fn the_view_takes_the_start_alone_under_the_minimum_and_none_outside() {
        let g = corridor(&[0.0, 0.0, 0.0]);
        let v = |start: u16, r: f32| view_clusters(&g, &ROWS, start, [3.5, 0.0, 0.0], r, 0.5);
        assert_eq!(v(1, 1.0).unwrap(), [1, 2]);
        assert_eq!(v(1, 0.5).unwrap(), [1], "not above the minimum");
        assert_eq!(v(1, f32::NAN).unwrap(), [1]);
        assert_eq!(v(0xffff, 1.0).unwrap(), Vec::<u16>::new());
        assert_eq!(v(9, 1.0), Err(Decline::Layout));
        let mut bad = ROWS;
        bad[0][0] = 3;
        assert_eq!(
            view_clusters(&g, &bad, 1, [3.5, 0.0, 0.0], 1.0, 0.5),
            Err(Decline::Layout)
        );
    }
}
