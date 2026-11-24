//! The view's portal flood (`0x00173130`, its walk `0x00172840`, the
//! portal projection `0x00173520` and the window merge `0x00172c70`;
//! docs/graphics/native-visibility.md, "The flood"), and the potentially
//! visible set of mode 2 (`0x001733e0`, `0x0016de20`).
//!
//! The flood is breadth first from the camera's cluster. A node is a
//! cluster seen through a window: a convex polygon on the screen and its
//! bounding rectangle, and the portals its path has crossed. Through each
//! of the cluster's portals that faces away from the camera, is open, and
//! has not been crossed on the path, the portal's projected polygon is
//! clipped to the node's window; three or more corners left make a node in
//! the cluster across. A cluster is visible when a node reaches it. Each
//! cluster's windows are then merged pairwise where their union is not much
//! larger than the two, and each window left makes a frustum objects are
//! tested against ([`super::frusta`]).
//!
//! The arithmetic is Rust's own: screen units are the projection's
//! normalised device coordinates, the near clip is the camera's near plane,
//! and the polygon clip has no tolerance. A set that differs from the
//! guest's because of it shows in the differential ([`super::parity`]).

use std::collections::VecDeque;

use frame_scene::RenderCamera;

use super::Decline;
use super::graph::{Portal, PortalGraph, portal_flags};

/// A point on the screen, normalised device units (x right, y up).
pub type Point = [f32; 2];

/// Nodes the guest's pool holds (`0x00172840`'s count against `0x100`).
pub const MAX_NODES: usize = 256;
/// Nodes its ring queue holds at once (256 slots, one kept free).
pub const MAX_QUEUED: usize = 255;
/// Corners its vertex pools hold (`0x1400`, with `0x40` kept free for the
/// next window); the projected portals have a pool of the same size.
pub const MAX_WINDOW_CORNERS: usize = 0x1400 - 0x40;
/// Corners one clipped window may have (the clip's output capacity).
pub const MAX_CORNERS: usize = 0x40;
/// Visible clusters the region list holds (`0x00172ee0`, `0x001733e0`).
pub const MAX_REGIONS: usize = 128;
/// Windows the frustum list holds.
pub const MAX_WINDOWS: usize = 0x200;

/// Values the flood reads from the running image's data, not committed:
/// the scene step reads them each frame (`super::guest::tuning`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tuning {
    /// The distance from a portal's plane at or under which the camera
    /// straddles it (the double at `0x0045e060`).
    pub straddle: f64,
    /// How far a straddled portal's rectangle is widened (`0x0045dfa0`).
    pub widen: f32,
    /// Two windows merge when this times the sum of their areas exceeds
    /// the area of their union (`0x0043ff08`).
    pub merge: f32,
}

/// A screen rectangle: min x, max x, min y, max y.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect(pub [f32; 4]);

impl Rect {
    /// The whole view.
    pub const SCREEN: Rect = Rect([-1.0, 1.0, -1.0, 1.0]);

    pub fn bounds(points: &[Point]) -> Self {
        let mut r = [
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
        ];
        for &[x, y] in points {
            r = [r[0].min(x), r[1].max(x), r[2].min(y), r[3].max(y)];
        }
        Rect(r)
    }

    /// The guest's overlap test, edges touching included; a NaN edge does
    /// not overlap.
    pub fn overlaps(&self, other: &Rect) -> bool {
        let ([a0, a1, b0, b1], [c0, c1, d0, d1]) = (self.0, other.0);
        a1 >= c0 && c1 >= a0 && b1 >= d0 && d1 >= b0
    }

    pub fn union(&self, other: &Rect) -> Rect {
        let ([a0, a1, b0, b1], [c0, c1, d0, d1]) = (self.0, other.0);
        Rect([a0.min(c0), a1.max(c1), b0.min(d0), b1.max(d1)])
    }

    pub fn area(&self) -> f32 {
        let [x0, x1, y0, y1] = self.0;
        (y1 - y0) * (x1 - x0)
    }

    fn widened(&self, by: f32) -> Rect {
        let [x0, x1, y0, y1] = self.0;
        Rect([x0 - by, x1 + by, y0 - by, y1 + by])
    }

    /// Its corners, counter-clockwise from min x, min y.
    pub fn corners(&self) -> Vec<Point> {
        let [x0, x1, y0, y1] = self.0;
        vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(a: [f32; 3]) -> Option<[f32; 3]> {
    let l = dot(a, a).sqrt();
    (l.is_finite() && l > 1e-6).then(|| [a[0] / l, a[1] / l, a[2] / l])
}

/// The camera as the flood sees it: a view basis and the projection's
/// scales and offsets ([`RenderCamera::projection`]'s), so a view-space
/// point `(x, y, w)` (`w` the depth along forward) lands at
/// `(sx · x / w − ox, sy · y / w − oy)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lens {
    pub position: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
    forward: [f32; 3],
    scale: [f32; 2],
    offset: [f32; 2],
    pub near: f32,
    pub far: f32,
}

impl Lens {
    pub fn new(camera: &RenderCamera) -> Option<Self> {
        if !camera.is_usable() {
            return None;
        }
        let forward = normalize(camera.forward)?;
        let right = normalize(cross(forward, camera.up))?;
        let up = cross(right, forward);
        let t = (camera.fov * 0.5).tan();
        let [sx, sy, ox, oy] =
            camera
                .projection_xy
                .unwrap_or([1.0 / (t * camera.aspect()), 1.0 / t, 0.0, 0.0]);
        (sx.is_finite() && sy.is_finite() && sx > 0.0 && sy > 0.0).then_some(Self {
            position: camera.position,
            right,
            up,
            forward,
            scale: [sx, sy],
            offset: [ox, oy],
            near: camera.near,
            far: camera.far,
        })
    }

    /// A light frustum already supplies its world axes and bounds in tangent units.
    pub(super) fn from_tangents(
        position: [f32; 3],
        [right, up, forward]: [[f32; 3]; 3],
        [left, right_edge, bottom, top]: [f32; 4],
        near: f32,
        far: f32,
    ) -> Option<Self> {
        let scale = [2.0 / (right_edge - left), 2.0 / (top - bottom)];
        let offset = [(right_edge + left) * 0.5 * scale[0], (top + bottom) * 0.5 * scale[1]];
        let finite = [position, right, up, forward].into_iter().flatten().all(f32::is_finite)
            && offset.into_iter().all(f32::is_finite)
            && scale.into_iter().all(|s| s.is_finite() && s > 0.0);
        (finite && near.is_finite() && near > 0.0 && far.is_finite() && far > 0.0)
            .then_some(Self { position, right, up, forward, scale, offset, near, far })
    }

    /// World to `(x, y, w)`.
    pub fn to_view(&self, p: [f32; 3]) -> [f32; 3] {
        let d = sub(p, self.position);
        [dot(d, self.right), dot(d, self.up), dot(d, self.forward)]
    }

    /// `(x, y, w)` with `w > 0` to the screen.
    pub fn to_screen(&self, [x, y, w]: [f32; 3]) -> Point {
        [
            self.scale[0] * x / w - self.offset[0],
            self.scale[1] * y / w - self.offset[1],
        ]
    }

    /// The view's right, up and forward axes.
    pub fn axes(&self) -> [[f32; 3]; 3] {
        [self.right, self.up, self.forward]
    }

    /// A screen point in tangent units (`x / w`, `y / w`): the inverse of
    /// [`Self::to_screen`]'s divide, what the window frusta are built from.
    pub fn tangent(&self, [x, y]: Point) -> Point {
        [
            (x + self.offset[0]) / self.scale[0],
            (y + self.offset[1]) / self.scale[1],
        ]
    }
}

/// How the camera stands to a portal's plane (`0x00173890`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Facing {
    /// Behind the plane: the portal is crossed from its first cluster.
    Back,
    /// In front: crossed from its second.
    Front,
    /// On the plane, or the portal clipped to one or two corners: crossed
    /// either way.
    Straddle,
}

/// A portal projected for the view (`0x00173520`).
#[derive(Clone, Debug, PartialEq)]
pub struct Projected {
    pub facing: Facing,
    pub rect: Rect,
    pub polygon: Vec<Point>,
}

/// The camera's signed distance from the plane, in the guest's order.
fn plane_distance(plane: [f32; 4], p: [f32; 3]) -> f64 {
    let [nx, ny, nz, d] = plane.map(f64::from);
    let [x, y, z] = p.map(f64::from);
    ((z * nz + y * ny) + nx * x) - d
}

/// Clips a view-space polygon to `w >= near`.
fn clip_near(points: &[[f32; 3]], near: f32) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(points.len() + 1);
    for (i, &cur) in points.iter().enumerate() {
        let prev = points[(i + points.len() - 1) % points.len()];
        let (dc, dp) = (cur[2] - near, prev[2] - near);
        if (dc >= 0.0) != (dp >= 0.0) {
            let t = dp / (dp - dc);
            out.push([
                prev[0] + t * (cur[0] - prev[0]),
                prev[1] + t * (cur[1] - prev[1]),
                near,
            ]);
        }
        if dc >= 0.0 {
            out.push(cur);
        }
    }
    out
}

/// Whether the foot of the camera on the portal's plane is inside its
/// polygon (`0x00173910`): the polygon and the foot dropped onto the two
/// axes off the normal's largest, inside when no edge has it strictly on a
/// different side from another.
fn foot_inside(portal: &Portal, eye: [f32; 3], distance: f64) -> bool {
    let n = [portal.plane[0], portal.plane[1], portal.plane[2]];
    let foot = [0, 1, 2].map(|i| eye[i] - n[i] * distance as f32);
    let axis = if n[2].abs() >= n[1].abs() && n[2].abs() >= n[0].abs() {
        2
    } else if n[1].abs() >= n[0].abs() {
        1
    } else {
        0
    };
    let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
    let points: Vec<Point> = portal.vertices.iter().map(|p| [p[u], p[v]]).collect();
    if points.len() < 3 {
        return false;
    }
    let f = [foot[u], foot[v]];
    let (mut positive, mut negative) = (false, false);
    for (i, &a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        let c = (b[0] - a[0]) * (f[1] - a[1]) - (b[1] - a[1]) * (f[0] - a[0]);
        positive |= c > 0.0;
        negative |= c < 0.0;
    }
    !(positive && negative)
}

/// Projects `portal` for the view; `None` when it is culled: behind the
/// near plane, beyond the far one, or wholly clipped away with the camera
/// off its plane.
pub fn project(portal: &Portal, lens: &Lens, tuning: &Tuning) -> Option<Projected> {
    let distance = plane_distance(portal.plane, lens.position);
    let view: Vec<[f32; 3]> = portal.vertices.iter().map(|&v| lens.to_view(v)).collect();
    let clipped = if view.is_empty() {
        view
    } else {
        clip_near(&view, lens.near)
    };
    let (facing, inside) = if distance.abs() > tuning.straddle {
        match clipped.len() {
            0 => return None,
            1 | 2 => (Facing::Straddle, false),
            _ if distance < 0.0 => (Facing::Back, false),
            _ => (Facing::Front, false),
        }
    } else {
        let inside = foot_inside(portal, lens.position, distance);
        if !inside && clipped.is_empty() {
            return None;
        }
        (Facing::Straddle, inside)
    };
    let mut nearest = clipped.iter().map(|v| v[2]).fold(f32::INFINITY, f32::min);
    let mut polygon: Vec<Point> = clipped.iter().map(|&v| lens.to_screen(v)).collect();
    if facing == Facing::Back {
        polygon.reverse();
    }
    let mut rect = Rect::bounds(&polygon);
    if facing == Facing::Straddle {
        if inside {
            nearest = nearest.min(tuning.straddle as f32);
            rect = Rect::SCREEN;
        } else {
            rect = rect.widened(tuning.widen);
        }
        polygon = rect.corners();
    }
    if nearest > lens.far {
        return None;
    }
    Some(Projected {
        facing,
        rect,
        polygon,
    })
}

fn signed_area(points: &[Point]) -> f32 {
    let mut sum = 0.0;
    for (i, a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        sum += a[0] * b[1] - b[0] * a[1];
    }
    sum * 0.5
}

/// `subject` clipped to the convex `window`, either winding.
pub fn clip(subject: &[Point], window: &[Point]) -> Vec<Point> {
    let area = signed_area(window);
    if window.len() < 3 || area == 0.0 || !area.is_finite() {
        return Vec::new();
    }
    let sign = area.signum();
    let mut out = subject.to_vec();
    for (i, &a) in window.iter().enumerate() {
        if out.is_empty() {
            break;
        }
        let b = window[(i + 1) % window.len()];
        let side =
            |p: Point| sign * ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]));
        let input = std::mem::take(&mut out);
        for (j, &cur) in input.iter().enumerate() {
            let prev = input[(j + input.len() - 1) % input.len()];
            let (dc, dp) = (side(cur), side(prev));
            if (dc >= 0.0) != (dp >= 0.0) {
                let t = dp / (dp - dc);
                out.push([
                    prev[0] + t * (cur[0] - prev[0]),
                    prev[1] + t * (cur[1] - prev[1]),
                ]);
            }
            if dc >= 0.0 {
                out.push(cur);
            }
        }
    }
    out
}

/// Which portals are open: the guest's per-portal bits at `[0x004f93a4]`
/// (doors and other devices close them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenPortals(pub Vec<u32>);

impl OpenPortals {
    pub fn all(portals: usize) -> Self {
        Self(vec![u32::MAX; portals.div_ceil(32)])
    }

    pub fn is_open(&self, portal: u16) -> bool {
        self.0
            .get(usize::from(portal) / 32)
            .is_some_and(|w| w & (1u32 << (portal % 32)) != 0)
    }
}

/// What a flood or a set saw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reached {
    /// Visible clusters, in the order they were first reached.
    pub clusters: Vec<u16>,
    /// The windows left after merging, which the object frusta are built
    /// from. Empty for the potentially visible set, which does not clip.
    pub windows: Vec<(u16, Rect)>,
}

struct Node {
    cluster: u16,
    rect: Rect,
    polygon: Vec<Point>,
    crossed: [u32; 16],
}

/// The portal flood from `start` (mode 0).
pub fn flood(
    graph: &PortalGraph,
    lens: &Lens,
    tuning: &Tuning,
    open: &OpenPortals,
    start: u16,
) -> Result<Reached, Decline> {
    if usize::from(start) >= graph.clusters.len() {
        return Err(Decline::Layout);
    }
    let root = Node {
        cluster: start,
        rect: Rect::SCREEN,
        polygon: Rect::SCREEN.corners(),
        crossed: [0; 16],
    };
    let mut corners = root.polygon.len();
    let mut nodes = vec![root];
    let mut projected: Vec<Option<Option<Projected>>> = vec![None; graph.portals.len()];
    let mut projected_corners = 0usize;
    let mut queue = VecDeque::from([0usize]);
    while let Some(n) = queue.pop_front() {
        let cluster = nodes[n].cluster;
        for &p in &graph.clusters[usize::from(cluster)].portals {
            let portal = &graph.portals[usize::from(p)];
            let slot = &mut projected[usize::from(p)];
            if slot.is_none() {
                if projected_corners > MAX_WINDOW_CORNERS {
                    return Err(Decline::Overflow);
                }
                let made = project(portal, lens, tuning);
                projected_corners += made.as_ref().map_or(0, |m| m.polygon.len());
                *slot = Some(made);
            }
            let Some(Some(view)) = slot.as_ref() else {
                continue;
            };
            let (word, bit) = (usize::from(p) / 32, 1u32 << (p % 32));
            if nodes[n].crossed[word] & bit != 0 || !open.is_open(p) {
                continue;
            }
            let crossable = match view.facing {
                Facing::Back => {
                    portal.flags & portal_flags::CLOSED_FROM_BACK == 0
                        && portal.clusters[0] == cluster
                }
                Facing::Front => {
                    portal.flags & portal_flags::CLOSED_FROM_FRONT == 0
                        && portal.clusters[1] == cluster
                }
                Facing::Straddle => true,
            };
            if !crossable || !view.rect.overlaps(&nodes[n].rect) {
                continue;
            }
            if nodes.len() >= MAX_NODES || corners > MAX_WINDOW_CORNERS {
                return Err(Decline::Overflow);
            }
            let polygon = clip(&view.polygon, &nodes[n].polygon);
            if polygon.len() < 3 {
                continue;
            }
            if polygon.len() > MAX_CORNERS {
                return Err(Decline::Overflow);
            }
            let mut crossed = nodes[n].crossed;
            crossed[word] |= bit;
            corners += polygon.len();
            nodes.push(Node {
                cluster: portal.across(cluster),
                rect: Rect::bounds(&polygon),
                polygon,
                crossed,
            });
            if queue.len() >= MAX_QUEUED {
                return Err(Decline::Overflow);
            }
            queue.push_back(nodes.len() - 1);
        }
    }
    let mut clusters: Vec<u16> = Vec::new();
    for node in &nodes {
        if !clusters.contains(&node.cluster) {
            clusters.push(node.cluster);
        }
    }
    if clusters.len() > MAX_REGIONS {
        return Err(Decline::Overflow);
    }
    let windows = merge(&nodes, tuning);
    if windows.len() > MAX_WINDOWS {
        return Err(Decline::Overflow);
    }
    Ok(Reached { clusters, windows })
}

/// `0x00172c70`: per cluster, ascending, its nodes newest first; each node
/// merges into the first later one in that list whose union with it is
/// smaller than `merge` times their two areas, which takes the union.
fn merge(nodes: &[Node], tuning: &Tuning) -> Vec<(u16, Rect)> {
    let mut by_cluster: Vec<(u16, Vec<usize>)> = Vec::new();
    for (i, node) in nodes.iter().enumerate().rev() {
        match by_cluster.iter_mut().find(|(c, _)| *c == node.cluster) {
            Some((_, list)) => list.push(i),
            None => by_cluster.push((node.cluster, vec![i])),
        }
    }
    by_cluster.sort_by_key(|(c, _)| *c);
    let mut rects: Vec<Rect> = nodes.iter().map(|n| n.rect).collect();
    let mut merged = vec![false; nodes.len()];
    for (_, list) in &by_cluster {
        for (i, &a) in list.iter().enumerate() {
            for &b in &list[i + 1..] {
                let union = rects[b].union(&rects[a]);
                if (rects[b].area() + rects[a].area()) * tuning.merge > union.area() {
                    merged[a] = true;
                    rects[b] = union;
                    break;
                }
            }
        }
    }
    nodes
        .iter()
        .enumerate()
        .filter(|&(i, _)| !merged[i])
        .map(|(i, n)| (n.cluster, rects[i]))
        .collect()
}

/// Mode 2: the start cluster's row of the potentially visible set
/// (`0x0016de20`, computed at run time for a cluster that asks for it:
/// [`super::pvs::row`]), ascending, at most [`MAX_REGIONS`] (`0x001733e0`
/// stops adding there).
pub fn potentially_visible(
    graph: &PortalGraph,
    start: u16,
    open: &OpenPortals,
) -> Result<Reached, Decline> {
    if usize::from(start) >= graph.clusters.len() || graph.pvs.is_empty() {
        return Err(Decline::Layout);
    }
    let row = super::pvs::row(graph, open, start)?;
    let clusters = (0..graph.clusters.len() as u16)
        .filter(|&c| row[usize::from(c) / 32] & (1u32 << (c % 32)) != 0)
        .take(MAX_REGIONS)
        .collect();
    Ok(Reached {
        clusters,
        windows: Vec::new(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::visibility::graph::Cluster;

    pub(crate) const TUNING: Tuning = Tuning {
        straddle: 0.01,
        widen: 0.05,
        merge: 0.8,
    };

    /// At the origin looking down +x, z up, 90 degrees square.
    pub(crate) fn camera_at(position: [f32; 3], forward: [f32; 3]) -> RenderCamera {
        RenderCamera {
            position,
            forward,
            up: [0.0, 0.0, 1.0],
            fov: std::f32::consts::FRAC_PI_2,
            projection_xy: None,
            viewport: [0, 0, 480, 480],
            near: 0.0625,
            far: 1000.0,
        }
    }

    /// A square portal of half size `h` in the plane x = `x`, centred on
    /// (y, z), facing +x, from cluster `back` (x below) to `front`.
    pub(crate) fn wall(x: f32, y: f32, z: f32, h: f32, back: u16, front: u16) -> Portal {
        Portal {
            clusters: [back, front],
            plane: [1.0, 0.0, 0.0, x],
            flags: 0,
            sphere: [x, y, z, h * std::f32::consts::SQRT_2],
            vertices: vec![
                [x, y - h, z - h],
                [x, y + h, z - h],
                [x, y + h, z + h],
                [x, y - h, z + h],
            ],
        }
    }

    /// A row of rooms along +x: room i is x in (4i − 4, 4i), joined to
    /// room i + 1 by portal i at x = 4i, a 2 by 2 square centred on
    /// y = `offsets[i]`.
    pub(crate) fn corridor(offsets: &[f32]) -> PortalGraph {
        let portals: Vec<Portal> = offsets
            .iter()
            .enumerate()
            .map(|(i, &y)| wall(4.0 * i as f32, y, 0.0, 1.0, i as u16, i as u16 + 1))
            .collect();
        let clusters = (0..=offsets.len())
            .map(|c| Cluster {
                portals: [c.checked_sub(1), (c < offsets.len()).then_some(c)]
                    .into_iter()
                    .flatten()
                    .map(|p| p as u16)
                    .collect(),
                computed_pvs: false,
            })
            .collect();
        PortalGraph {
            clusters,
            portals,
            pvs_words: 0,
            pvs: Vec::new(),
            pvs_alt: Vec::new(),
        }
    }

    fn reach(graph: &PortalGraph, camera: &RenderCamera, start: u16) -> Result<Reached, Decline> {
        let lens = Lens::new(camera).unwrap();
        flood(
            graph,
            &lens,
            &TUNING,
            &OpenPortals::all(graph.portals.len()),
            start,
        )
    }

    #[test]
    fn a_straight_corridor_is_seen_to_its_end_and_not_behind_the_camera() {
        let g = corridor(&[0.0, 0.0, 0.0]);
        let r = reach(&g, &camera_at([1.0, 0.0, 0.0], [1.0, 0.0, 0.0]), 1).unwrap();
        assert_eq!(r.clusters, [1, 2, 3], "room 0 is behind the camera");
        let back = reach(&g, &camera_at([1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]), 1).unwrap();
        assert_eq!(back.clusters, [1, 0]);
    }

    #[test]
    fn a_portal_outside_the_window_it_is_seen_through_stops_the_flood() {
        // From x = 1 the doorway at x = 4 is a window a third of the screen
        // wide; the one at x = 8, five units to the side, is on the screen
        // but off that window.
        let g = corridor(&[0.0, 0.0, 5.0]);
        let r = reach(&g, &camera_at([1.0, 0.0, 0.0], [1.0, 0.0, 0.0]), 1).unwrap();
        assert_eq!(r.clusters, [1, 2]);
        // Nearly in line, it is seen through the first.
        let g = corridor(&[0.0, 0.0, 0.3]);
        let r = reach(&g, &camera_at([1.0, 0.0, 0.0], [1.0, 0.0, 0.0]), 1).unwrap();
        assert_eq!(r.clusters, [1, 2, 3]);
    }

    #[test]
    fn closed_portals_and_one_way_flags_hold_the_flood() {
        let mut g = corridor(&[0.0, 0.0, 0.0]);
        let lens = Lens::new(&camera_at([1.0, 0.0, 0.0], [1.0, 0.0, 0.0])).unwrap();
        let mut open = OpenPortals::all(3);
        open.0[0] &= !(1 << 2);
        assert_eq!(
            flood(&g, &lens, &TUNING, &open, 1).unwrap().clusters,
            [1, 2],
            "the door at x = 8 is shut"
        );
        // The camera is behind portal 2's plane: a portal closed from the
        // back stops it, one closed from the front does not.
        g.portals[2].flags = portal_flags::CLOSED_FROM_FRONT;
        assert_eq!(
            reach(&g, &camera_at([1.0, 0.0, 0.0], [1.0, 0.0, 0.0]), 1)
                .unwrap()
                .clusters,
            [1, 2, 3]
        );
        g.portals[2].flags = portal_flags::CLOSED_FROM_BACK;
        assert_eq!(
            reach(&g, &camera_at([1.0, 0.0, 0.0], [1.0, 0.0, 0.0]), 1)
                .unwrap()
                .clusters,
            [1, 2]
        );
    }

    #[test]
    fn a_portal_is_crossed_only_from_the_cluster_on_the_cameras_side() {
        // From room 0 behind portal 0's plane, the portal is crossed from its
        // first cluster; with its clusters swapped that is the far room.
        let mut g = corridor(&[0.0]);
        let cam = camera_at([-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]);
        assert_eq!(reach(&g, &cam, 0).unwrap().clusters, [0, 1]);
        g.portals[0].clusters = [1, 0];
        assert_eq!(reach(&g, &cam, 0).unwrap().clusters, [0]);
    }

    #[test]
    fn standing_in_a_doorway_sees_through_it_whole() {
        let g = corridor(&[0.0, 0.0]);
        // On portal 1's plane (x = 4), inside its square, looking along it.
        let cam = camera_at([4.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        let lens = Lens::new(&cam).unwrap();
        let view = project(&g.portals[1], &lens, &TUNING).unwrap();
        assert_eq!((view.facing, view.rect), (Facing::Straddle, Rect::SCREEN));
        assert_eq!(reach(&g, &cam, 1).unwrap().clusters, [1, 2]);
        assert_eq!(reach(&g, &cam, 2).unwrap().clusters, [2, 1]);
    }

    #[test]
    fn a_portal_wholly_behind_the_near_plane_is_culled() {
        let g = corridor(&[0.0]);
        let lens = Lens::new(&camera_at([5.0, 0.0, 0.0], [1.0, 0.0, 0.0])).unwrap();
        assert_eq!(project(&g.portals[0], &lens, &TUNING), None);
        let far = Lens::new(&RenderCamera {
            far: 2.0,
            ..camera_at([-3.0, 0.0, 0.0], [1.0, 0.0, 0.0])
        })
        .unwrap();
        assert_eq!(
            project(&g.portals[0], &far, &TUNING),
            None,
            "beyond the far plane"
        );
    }

    #[test]
    fn a_loop_of_rooms_does_not_cross_a_portal_twice_on_one_path() {
        // Two portals between rooms 0 and 1 side by side, both in view: the
        // flood reaches 1 twice, once through each, and stops.
        let portals = vec![
            wall(4.0, -1.5, 0.0, 1.0, 0, 1),
            wall(4.0, 1.5, 0.0, 1.0, 0, 1),
        ];
        let g = PortalGraph {
            clusters: vec![
                Cluster {
                    portals: vec![0, 1],
                    computed_pvs: false,
                },
                Cluster {
                    portals: vec![0, 1],
                    computed_pvs: false,
                },
            ],
            portals,
            pvs_words: 0,
            pvs: Vec::new(),
            pvs_alt: Vec::new(),
        };
        let r = reach(&g, &camera_at([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]), 0).unwrap();
        assert_eq!(r.clusters, [0, 1]);
        // The two windows into room 1 are far apart: both stay.
        assert_eq!(r.windows.iter().filter(|(c, _)| *c == 1).count(), 2);
    }

    #[test]
    fn overlapping_windows_merge_into_their_union() {
        let a = Node {
            cluster: 3,
            rect: Rect([0.0, 0.5, 0.0, 0.5]),
            polygon: Vec::new(),
            crossed: [0; 16],
        };
        let b = Node {
            cluster: 3,
            rect: Rect([0.1, 0.6, 0.0, 0.5]),
            polygon: Vec::new(),
            crossed: [0; 16],
        };
        let far = Node {
            cluster: 3,
            rect: Rect([-1.0, -0.9, -1.0, -0.9]),
            polygon: Vec::new(),
            crossed: [0; 16],
        };
        let w = merge(&[a, b, far], &TUNING);
        // Newest first: the far window does not merge with b; b merges into
        // a, which takes the union.
        assert_eq!(w.len(), 2);
        assert!(w.contains(&(3, Rect([0.0, 0.6, 0.0, 0.5]))));
        assert!(w.contains(&(3, Rect([-1.0, -0.9, -1.0, -0.9]))));
    }

    #[test]
    fn mode_two_takes_the_start_clusters_visible_set_row() {
        let mut g = corridor(&[0.0, 0.0, 0.0]);
        g.pvs_words = 1;
        g.pvs = vec![0b0011, 0b1110, 0b0110, 0b1000];
        let open = OpenPortals::all(3);
        assert_eq!(potentially_visible(&g, 1, &open).unwrap().clusters, [1, 2, 3]);
        assert_eq!(potentially_visible(&g, 3, &open).unwrap().clusters, [3]);
        // Room 1's row is computed: from the second table through the open
        // portals that join computed sets, masked by its own row.
        g.clusters[1].computed_pvs = true;
        g.pvs_alt = vec![0b0001, 0b0010, 0b0100, 0b1000];
        g.portals[1].flags = portal_flags::JOINS_COMPUTED_PVS;
        assert_eq!(potentially_visible(&g, 1, &open).unwrap().clusters, [1, 2]);
        assert_eq!(potentially_visible(&g, 9, &open), Err(Decline::Layout));
    }

    #[test]
    fn the_clip_keeps_the_overlap_of_either_winding() {
        let square = Rect([0.0, 2.0, 0.0, 2.0]).corners();
        let mut shifted = Rect([1.0, 3.0, 1.0, 3.0]).corners();
        assert_eq!(
            Rect::bounds(&clip(&shifted, &square)),
            Rect([1.0, 2.0, 1.0, 2.0])
        );
        shifted.reverse();
        let mut window = square.clone();
        window.reverse();
        assert_eq!(
            Rect::bounds(&clip(&shifted, &window)),
            Rect([1.0, 2.0, 1.0, 2.0])
        );
        assert!(clip(&Rect([5.0, 6.0, 5.0, 6.0]).corners(), &square).len() < 3);
    }
}
