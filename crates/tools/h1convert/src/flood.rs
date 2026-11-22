//! Halo 2's portal flood, run offline over a converted structure's portals.
//!
//! The engine picks a view's structure clusters in `0x00173130`: from the
//! camera's cluster it floods breadth first through portals
//! (`0x00172840`), one node per chain of portals, and draws every cluster
//! a node reaches (`0x00173b00`). For each portal on a node's cluster's
//! list (cluster `+0x8c`, i16) it:
//!
//! - projects the portal's polygon (portal `+0x1c`, `0x24` each at
//!   `sbsp +0x5c`) to the screen after clipping it to the near plane
//!   (`0x00173520`); a portal wholly behind the camera is not crossed;
//! - classifies the camera against the portal's plane, the collision
//!   plane the portal names (`+0x04`; `0x00173890`): behind it (1),
//!   in front (2), or on it / edge-on (3);
//! - crosses from cluster `+0x00` when the camera is behind, from `+0x02`
//!   when in front, and either way when on it (unless the portal is
//!   already on the chain, `0x00173350`);
//! - skips a portal already on this node's chain (the node's 512-bit set,
//!   `+0x2c`);
//! - intersects the portal's screen polygon with the node's
//!   (`0x00173380`, `0x0023a2b0`) and needs at least 3 vertices left;
//! - makes a node for the far cluster, unless the flood already has
//!   [`NODES`] nodes (`0x001729d9`: `cmp edx, 100h`), in which case the
//!   portal is dropped for this frame and nothing behind it is drawn.
//!
//! Nodes are per chain, not per cluster: a cluster reached by several
//! chains is expanded once per chain. [`flood`] counts the nodes so a
//! structure whose flood runs out can be found offline.

/// Nodes one flood may make (`0x001729d9`).
pub const NODES: usize = 256;

/// A view: position, an orthonormal frame and the frustum's half-angle
/// tangents.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub right: [f32; 3],
    pub up: [f32; 3],
    /// `tan` of the horizontal and vertical half-angles.
    pub tan: [f32; 2],
    /// Near plane distance.
    pub near: f32,
}

impl View {
    /// A level view at `yaw` radians about +z, looking along
    /// `(cos yaw, sin yaw, 0)`, with a horizontal field of view of
    /// `fov` radians and a 4:3 screen.
    pub fn level(position: [f32; 3], yaw: f32, fov: f32) -> Self {
        let (s, c) = yaw.sin_cos();
        let tx = (fov * 0.5).tan();
        View {
            position,
            forward: [c, s, 0.0],
            right: [s, -c, 0.0],
            up: [0.0, 0.0, 1.0],
            tan: [tx, tx * 0.75],
            near: 1.0 / 128.0,
        }
    }

    fn to_view(self, p: [f32; 3]) -> [f32; 3] {
        let d = sub(p, self.position);
        [dot(d, self.right), dot(d, self.up), dot(d, self.forward)]
    }

    /// The screen polygon, in projected units (x / z, y / z).
    fn screen(&self) -> Vec<[f32; 2]> {
        let [x, y] = self.tan;
        vec![[-x, -y], [x, -y], [x, y], [-x, y]]
    }

    /// The four side planes and the near plane, each `(n, d)` with the
    /// inside where `n . p >= d`.
    pub fn planes(&self) -> [([f32; 3], f32); 5] {
        let p = self.position;
        let f = self.forward;
        let side = |axis: [f32; 3], t: f32, sign: f32| {
            // Inside: t * (f . d) - sign * (axis . d) >= 0.
            let n = [
                t * f[0] - sign * axis[0],
                t * f[1] - sign * axis[1],
                t * f[2] - sign * axis[2],
            ];
            (n, dot(n, p))
        };
        [
            side(self.right, self.tan[0], 1.0),
            side(self.right, self.tan[0], -1.0),
            side(self.up, self.tan[1], 1.0),
            side(self.up, self.tan[1], -1.0),
            (f, dot(f, p) + self.near),
        ]
    }
}

/// One portal as the flood reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Portal {
    /// The cluster behind the plane and the one in front.
    pub clusters: [u16; 2],
    /// Its plane `[nx, ny, nz, d]`.
    pub plane: [f32; 4],
    /// Its polygon.
    pub vertices: Vec<[f32; 3]>,
}

/// What a flood reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reached {
    /// Per cluster, whether a node reached it.
    pub clusters: Vec<bool>,
    /// Nodes made.
    pub nodes: usize,
    /// Portals dropped because the node limit was reached.
    pub dropped: usize,
}

#[derive(Clone)]
enum Side {
    Hidden,
    Behind,
    Front,
    On,
}

struct Seen {
    side: Side,
    poly: Vec<[f32; 2]>,
}

/// The engine's side test and screen polygon for one portal.
fn see(view: &View, p: &Portal) -> Seen {
    let n = [p.plane[0], p.plane[1], p.plane[2]];
    let d = dot(n, view.position) - p.plane[3];
    let v: Vec<[f32; 3]> = p.vertices.iter().map(|&q| view.to_view(q)).collect();
    let clipped = clip_near(&v, view.near);
    let poly: Vec<[f32; 2]> = clipped.iter().map(|q| [q[0] / q[2], q[1] / q[2]]).collect();
    // The engine's epsilon, a double at 0x0045e060.
    const ON: f32 = 1.0 / 64.0;
    let side = if d.abs() <= ON {
        Side::On
    } else if clipped.is_empty() {
        Side::Hidden
    } else if clipped.len() < 3 {
        Side::On
    } else if d < 0.0 {
        Side::Behind
    } else {
        Side::Front
    };
    let poly = if matches!(side, Side::On) { view.screen() } else { poly };
    Seen { side, poly }
}

/// Floods `portals` from `start` as the engine does, `lists[c]` cluster
/// `c`'s portals, making at most `limit` nodes ([`NODES`] in the engine).
pub fn flood(
    view: &View,
    portals: &[Portal],
    lists: &[Vec<u16>],
    start: usize,
    limit: usize,
) -> Reached {
    flood_with(view, portals, lists, start, limit, true)
}

/// [`flood`], with `clip` false crossing every portal that faces the
/// right way and is in front of the camera without narrowing the view to
/// it (to tell a portal the screen test drops from one the side test
/// does).
pub fn flood_with(
    view: &View,
    portals: &[Portal],
    lists: &[Vec<u16>],
    start: usize,
    limit: usize,
    clip: bool,
) -> Reached {
    struct Node {
        cluster: usize,
        poly: Vec<[f32; 2]>,
        chain: Vec<u64>,
    }
    let seen: Vec<Seen> = portals.iter().map(|p| see(view, p)).collect();
    let words = portals.len().div_ceil(64).max(1);
    let mut clusters = vec![false; lists.len()];
    clusters[start] = true;
    let mut nodes = vec![Node {
        cluster: start,
        poly: view.screen(),
        chain: vec![0; words],
    }];
    let mut dropped = 0;
    let mut head = 0;
    while head < nodes.len() {
        let cur = head;
        head += 1;
        for &pi in &lists[nodes[cur].cluster] {
            let pi = usize::from(pi);
            let (p, s) = (&portals[pi], &seen[pi]);
            if nodes[cur].chain[pi / 64] & (1 << (pi % 64)) != 0 {
                continue;
            }
            let c = nodes[cur].cluster as u16;
            let ok = match s.side {
                Side::Hidden => false,
                Side::Behind => p.clusters[0] == c,
                Side::Front => p.clusters[1] == c,
                Side::On => true,
            };
            if !ok {
                continue;
            }
            let poly = if clip {
                clip_convex(&nodes[cur].poly, &s.poly)
            } else {
                nodes[cur].poly.clone()
            };
            if poly.len() < 3 {
                continue;
            }
            if nodes.len() >= limit {
                dropped += 1;
                continue;
            }
            let next = if p.clusters[0] == c {
                p.clusters[1]
            } else {
                p.clusters[0]
            };
            let mut chain = nodes[cur].chain.clone();
            chain[pi / 64] |= 1 << (pi % 64);
            clusters[usize::from(next)] = true;
            nodes.push(Node {
                cluster: usize::from(next),
                poly,
                chain,
            });
        }
    }
    Reached {
        clusters,
        nodes: nodes.len(),
        dropped,
    }
}

/// Whether any of `triangles` (over `vertices`) has a part inside the
/// view's frustum (its near and side planes; no far plane, no occlusion).
pub fn in_frustum(view: &View, vertices: &[[f32; 3]], triangles: &[[u32; 3]]) -> bool {
    let planes = view.planes();
    triangles.iter().any(|t| {
        let mut poly: Vec<[f32; 3]> = t.iter().map(|&i| vertices[i as usize]).collect();
        for (n, d) in planes {
            poly = clip3(&poly, n, d);
            if poly.is_empty() {
                return false;
            }
        }
        true
    })
}

fn clip3(poly: &[[f32; 3]], n: [f32; 3], d: f32) -> Vec<[f32; 3]> {
    let mut out = Vec::with_capacity(poly.len() + 1);
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let (da, db) = (dot(n, a) - d, dot(n, b) - d);
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push(lerp3(a, b, t));
        }
    }
    out
}

fn clip_near(poly: &[[f32; 3]], near: f32) -> Vec<[f32; 3]> {
    clip3(poly, [0.0, 0.0, 1.0], near)
}

/// `subject` clipped to the convex `clip` (either winding).
fn clip_convex(subject: &[[f32; 2]], clip: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if clip.len() < 3 {
        return Vec::new();
    }
    let area: f32 = (0..clip.len())
        .map(|i| {
            let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    let sign = if area >= 0.0 { 1.0 } else { -1.0 };
    let mut out = subject.to_vec();
    for i in 0..clip.len() {
        let (a, b) = (clip[i], clip[(i + 1) % clip.len()]);
        let side = |p: [f32; 2]| sign * ((b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]));
        let input = std::mem::take(&mut out);
        for k in 0..input.len() {
            let (p, q) = (input[k], input[(k + 1) % input.len()]);
            let (sp, sq) = (side(p), side(q));
            if sp >= 0.0 {
                out.push(p);
            }
            if (sp >= 0.0) != (sq >= 0.0) {
                let t = sp / (sp - sq);
                out.push([p[0] + (q[0] - p[0]) * t, p[1] + (q[1] - p[1]) * t]);
            }
        }
        if out.is_empty() {
            break;
        }
    }
    // A sliver with no area is no opening.
    let area: f32 = (0..out.len())
        .map(|i| {
            let (a, b) = (out[i], out[(i + 1) % out.len()]);
            a[0] * b[1] - b[0] * a[1]
        })
        .sum();
    if area.abs() < 1e-9 { Vec::new() } else { out }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square window in the plane x = `x` from y, z in -1..1.
    fn window(x: f32, clusters: [u16; 2]) -> Portal {
        Portal {
            clusters,
            plane: [1.0, 0.0, 0.0, x],
            vertices: vec![[x, -1.0, -1.0], [x, 1.0, -1.0], [x, 1.0, 1.0], [x, -1.0, 1.0]],
        }
    }

    #[test]
    fn a_window_ahead_is_crossed_from_the_cluster_behind_its_plane() {
        // Cluster 0 is x < 5, cluster 1 beyond.
        let portals = vec![window(5.0, [0, 1])];
        let lists = vec![vec![0], vec![0]];
        let view = View::level([0.0; 3], 0.0, 1.5);
        let r = flood(&view, &portals, &lists, 0, NODES);
        assert_eq!(r.clusters, vec![true, true]);
        // Looking away, the window is behind the camera.
        let back = View::level([0.0; 3], std::f32::consts::PI, 1.5);
        assert_eq!(flood(&back, &portals, &lists, 0, NODES).clusters, vec![true, false]);
        // The same portal with its clusters swapped is never crossed from
        // cluster 0: the camera is behind the plane, so the engine wants
        // the camera's side to be `+0x00`.
        let swapped = vec![window(5.0, [1, 0])];
        assert_eq!(flood(&view, &swapped, &lists, 0, NODES).clusters, vec![true, false]);
    }

    #[test]
    fn the_node_limit_drops_what_lies_past_it() {
        // A row of windows; each chain makes one node per cluster.
        let portals: Vec<Portal> = (0..4).map(|k| window(5.0 + k as f32, [k, k + 1])).collect();
        let lists: Vec<Vec<u16>> = (0..5u16)
            .map(|c| (0..4u16).filter(|&p| p == c || p + 1 == c).collect())
            .collect();
        let view = View::level([0.0; 3], 0.0, 1.5);
        let all = flood(&view, &portals, &lists, 0, NODES);
        assert_eq!(all.clusters, vec![true; 5]);
        assert_eq!((all.nodes, all.dropped), (5, 0));
        let cut = flood(&view, &portals, &lists, 0, 3);
        assert_eq!(cut.clusters, vec![true, true, true, false, false]);
        assert_eq!(cut.dropped, 1);
    }

    #[test]
    fn a_triangle_beside_the_view_is_out_of_the_frustum() {
        let view = View::level([0.0; 3], 0.0, 1.5);
        let v = [[10.0, -1.0, 0.0], [10.0, 1.0, 0.0], [10.0, 0.0, 1.0], [-10.0, 0.0, 0.0]];
        assert!(in_frustum(&view, &v, &[[0, 1, 2]]));
        let side = [[0.0, 10.0, 0.0], [1.0, 10.0, 0.0], [0.0, 10.0, 1.0]];
        assert!(!in_frustum(&view, &side, &[[0, 1, 2]]));
    }
}
