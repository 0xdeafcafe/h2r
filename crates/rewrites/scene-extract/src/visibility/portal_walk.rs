//! The tangent-space nodes consumed by the window merger at 0x00172c70.
//! Keep every path to a room: two portals to the same room can expose
//! disjoint windows. Nodes are appended in dequeue and BSP portal order.

use super::flood::{
    self, Facing, MAX_CORNERS, MAX_NODES, MAX_QUEUED, MAX_WINDOW_CORNERS, OpenPortals, Point,
    Projected, Rect,
};
use super::graph::{Portal, PortalGraph, portal_flags};
use super::portal_clip::Inputs;
use super::portal_projection::{Projection, ProjectionResult};
use super::{Decline, GraphCache, guest, read};
use crate::GuestRead;

#[derive(Debug, PartialEq)]
pub struct Node {
    pub cluster: u16,
    pub parent: Option<u16>,
    pub portal: Option<u16>,
    pub rect: Rect,
    pub polygon: Vec<Point>,
    pub depth: f32,
    pub crossed: [u32; 16],
}

#[derive(Debug, PartialEq)]
pub struct Walk {
    pub nodes: Vec<Node>,
}

/// The merge at 0x00172c70. Strict lower-bound comparisons retain the
/// destination on ties. Equal upper bounds and depths retain the source.
pub fn merge_window(
    (to, depth): (Rect, f32),
    (from, incoming): (Rect, f32),
    factor: f32,
) -> Option<(Rect, f32)> {
    let (a, b) = (to.0, from.0);
    let union = Rect([
        if a[0] > b[0] { b[0] } else { a[0] },
        if a[1] > b[1] { a[1] } else { b[1] },
        if a[2] > b[2] { b[2] } else { a[2] },
        if a[3] > b[3] { a[3] } else { b[3] },
    ]);
    ((to.area() + from.area()) * factor > union.area())
        .then_some((union, if incoming > depth { depth } else { incoming }))
}

impl Walk {
    /// In 0x001730a0 a cluster is published at its chain head (its
    /// newest node). Its surviving windows follow that chain.
    fn regions(mut self, factor: f32) -> Result<(Vec<u16>, Vec<(u16, Rect)>), Decline> {
        let mut groups = std::collections::BTreeMap::<u16, Vec<usize>>::new();
        for (i, node) in self.nodes.iter().enumerate() {
            groups.entry(node.cluster).or_default().push(i);
        }
        if groups.len() > flood::MAX_REGIONS {
            return Err(Decline::Overflow);
        }
        let mut groups: Vec<_> = groups.into_iter().collect();
        groups.sort_unstable_by_key(|(_, chain)| chain.last().copied());
        let mut regions = Vec::with_capacity(groups.len());
        let mut windows = Vec::with_capacity(self.nodes.len());
        for (cluster, mut chain) in groups {
            regions.push(cluster);
            chain.reverse();
            for (i, &from) in chain.iter().enumerate() {
                let source = (self.nodes[from].rect, self.nodes[from].depth);
                let mut absorbed = false;
                for &into in &chain[i + 1..] {
                    let dest = &mut self.nodes[into];
                    if let Some((rect, depth)) =
                        merge_window((dest.rect, dest.depth), source, factor)
                    {
                        dest.rect = rect;
                        dest.depth = depth;
                        absorbed = true;
                        break;
                    }
                }
                if !absorbed {
                    windows.push((cluster, source.0));
                }
            }
        }
        Ok((regions, windows))
    }
}

/// The bounds leaf grows the runtime empty rectangle with strict
/// comparisons. Equal coordinates (including opposite signed zeroes)
/// keep the bits of the first value encountered.
fn bounds(mut empty: Rect, points: &[Point]) -> Result<Rect, Decline> {
    for &[x, y] in points {
        if !x.is_finite() || !y.is_finite() {
            return Err(Decline::Layout);
        }
        if x < empty.0[0] {
            empty.0[0] = x;
        }
        if x > empty.0[1] {
            empty.0[1] = x;
        }
        if y < empty.0[2] {
            empty.0[2] = y;
        }
        if y > empty.0[3] {
            empty.0[3] = y;
        }
    }
    Ok(empty)
}

fn can_cross(portal: &Portal, facing: Facing, cluster: u16) -> bool {
    match facing {
        Facing::Back => {
            portal.clusters[0] == cluster && portal.flags & portal_flags::CLOSED_FROM_BACK == 0
        }
        Facing::Front => {
            portal.clusters[1] == cluster && portal.flags & portal_flags::CLOSED_FROM_FRONT == 0
        }
        Facing::Straddle => true,
    }
}

fn build(
    graph: &PortalGraph,
    open: &OpenPortals,
    root: Node,
    empty: Rect,
    mut project: impl FnMut(&Portal) -> Result<ProjectionResult, Decline>,
    mut clip: impl FnMut(&[Point], &[Point]) -> Result<Vec<Point>, Decline>,
) -> Result<Walk, Decline> {
    let mut corners = root.polygon.len();
    let mut nodes = vec![root];
    let mut projected: Vec<Option<Option<(Projected, f32)>>> = vec![None; graph.portals.len()];
    let mut projected_corners = 0;
    let mut cursor = 0;
    while cursor < nodes.len() {
        let cluster = nodes[cursor].cluster;
        for &id in &graph
            .clusters
            .get(usize::from(cluster))
            .ok_or(Decline::Layout)?
            .portals
        {
            let index = usize::from(id);
            let portal = graph.portals.get(index).ok_or(Decline::Layout)?;
            if !portal.clusters.contains(&cluster) || index / 32 >= 16 {
                return Err(Decline::Layout);
            }
            let cached = &mut projected[index];
            if cached.is_none() {
                if projected_corners > MAX_WINDOW_CORNERS {
                    return Err(Decline::Overflow);
                }
                let projection = project(portal)?;
                projected_corners += projection.corners_used;
                *cached = Some(projection.projected);
            }
            let Some((view, depth)) = cached.as_ref().and_then(Option::as_ref) else {
                continue;
            };
            let parent = &nodes[cursor];
            let word = index / 32;
            let bit = 1u32 << (index % 32);
            if parent.crossed[word] & bit != 0
                || !open.is_open(id)
                || !can_cross(portal, view.facing, cluster)
                || !view.rect.overlaps(&parent.rect)
            {
                continue;
            }
            // The guest reserves one node and a full clip buffer before
            // learning whether the child polygon is empty.
            if nodes.len() >= MAX_NODES || corners > MAX_WINDOW_CORNERS {
                return Err(Decline::Overflow);
            }
            let polygon = clip(&view.polygon, &parent.polygon)?;
            if polygon.len() < 3 {
                continue;
            }
            if polygon.len() > MAX_CORNERS || nodes.len() - cursor > MAX_QUEUED {
                return Err(Decline::Overflow);
            }
            let mut crossed = parent.crossed;
            crossed[word] |= bit;
            corners += polygon.len();
            nodes.push(Node {
                cluster: portal.across(cluster),
                parent: Some(cursor as u16),
                portal: Some(id),
                rect: bounds(empty, &polygon)?,
                polygon,
                depth: *depth,
                crossed,
            });
        }
        cursor += 1;
    }
    Ok(Walk { nodes })
}

pub fn for_view(
    mem: &dyn GuestRead,
    cache: &mut GraphCache,
    epoch: u64,
    frustum: u32,
    start: u16,
) -> Result<Walk, Decline> {
    walk_view(mem, cache, epoch, frustum, start, 1.).map(|(walk, _)| walk)
}

fn walk_view(
    mem: &dyn GuestRead,
    cache: &mut GraphCache,
    epoch: u64,
    frustum: u32,
    start: u16,
    factor: f32,
) -> Result<(Walk, f32), Decline> {
    let structure = read::ptr_at(mem, guest::ACTIVE_STRUCTURE).ok_or(Decline::Unread)?;
    let graph = cache.graph(mem, epoch, structure)?;
    let open = guest::open_portals(mem, graph.portals.len())?;
    let tuning = guest::tuning(mem)?;
    let mut projection = Projection::read(mem, frustum)?;
    if factor != 1. {
        projection.widen(factor)?;
    }
    let clip = Inputs::read(mem)?;
    let root = Node {
        cluster: start,
        parent: None,
        portal: None,
        rect: projection.rect,
        polygon: projection.polygon.clone(),
        depth: 0.,
        crossed: [0; 16],
    };
    let walk = build(
        graph,
        &open,
        root,
        projection.initial_rect,
        |p| projection.project(p, &tuning),
        |portal, parent| clip.clip(portal, parent),
    )?;
    Ok((walk, tuning.merge))
}

/// Main-view sets and clipping volumes from the input frustum used by
/// the traversal, walked from `origin`. A display factor changes the root
/// tangent polygon.
pub fn scene_visibility(
    mem: &dyn GuestRead,
    cache: &mut GraphCache,
    epoch: u64,
    factor: f32,
    origin: super::Origin,
) -> Result<(super::VisibilitySets, super::frusta::WindowFrusta), Decline> {
    use super::{frusta, objects, window_record::WindowInputs};
    use engine::render_loop::occlusion::guest::MAIN_VIS;
    let view = guest::view(mem)?.from_origin(origin);
    if view.mode != 0 {
        return Err(Decline::Mode);
    }
    if view.frusta != 1 {
        return Err(Decline::Frusta);
    }
    let mut volumes = frusta::WindowFrusta::default();
    let clusters = if view.start == u16::MAX {
        vec![]
    } else {
        let regions = read::ptr_at(mem, MAIN_VIS).ok_or(Decline::Unread)?;
        let frustum = read::at(regions, guest::view_frustum::FIRST).ok_or(Decline::Layout)?;
        let (walk, merge) = walk_view(mem, cache, epoch, frustum, view.start, factor)?;
        let (clusters, windows) = walk.regions(merge)?;
        let window = WindowInputs::read(
            mem,
            frustum,
            read::at(frustum, 0xa0).ok_or(Decline::Layout)?,
            0,
        )?;
        for (cluster, rect) in windows {
            if let Some(volume) = window.volume(rect)? {
                volumes.0.push((cluster, volume));
            }
        }
        clusters
    };
    let world = guest::object_world(mem, &view, &clusters)?;
    let reached = flood::Reached {
        clusters,
        windows: Vec::new(),
    };
    let clip = objects::Clip::Frusta(volumes);
    let handles = objects::object_set_with_clip(&world, &reached, &clip)?;
    let objects::Clip::Frusta(volumes) = clip else {
        unreachable!()
    };
    Ok((
        super::VisibilitySets::new(reached.clusters, handles),
        volumes,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::graph::Cluster;
    use super::*;

    fn walk(facing: Facing, open: OpenPortals) -> Walk {
        // Both portals connect the same pair of rooms. Each path may
        // return via the other door, but never via one it crossed.
        let shared = Cluster {
            portals: vec![1, 0],
            computed_pvs: false,
        };
        let room = Rect([-2., 1., -3., 1.]);
        let root = Node {
            cluster: 0,
            parent: None,
            portal: None,
            rect: room,
            polygon: room.corners(),
            depth: 0.,
            crossed: [0; 16],
        };
        let door = Portal {
            clusters: [0, 1],
            plane: [0.; 4],
            flags: 0,
            sphere: [0.; 4],
            vertices: vec![],
        };
        let graph = PortalGraph {
            clusters: vec![shared.clone(), shared],
            portals: vec![door.clone(), door],
            ..Default::default()
        };
        let mut projections = 0;
        let walk = build(
            &graph,
            &open,
            root,
            Rect([10., -10., 10., -10.]),
            |_| {
                projections += 1;
                Ok(ProjectionResult {
                    projected: Some((
                        Projected {
                            facing,
                            rect: room,
                            polygon: room.corners(),
                        },
                        2.5,
                    )),
                    corners_used: 4,
                })
            },
            |portal, _| Ok(portal.to_vec()),
        )
        .unwrap();
        assert_eq!(projections, 2);
        walk
    }

    #[test]
    fn parallel_portals_keep_separate_paths_in_queue_order() {
        let walk = walk(Facing::Straddle, OpenPortals::all(2));
        let path = walk
            .nodes
            .iter()
            .map(|n| (n.cluster, n.parent, n.portal, n.crossed[0]))
            .collect::<Vec<_>>();
        assert_eq!(
            path,
            [
                (0, None, None, 0),
                (1, Some(0), Some(1), 2),
                (1, Some(0), Some(0), 1),
                (0, Some(1), Some(0), 3),
                (0, Some(2), Some(1), 3)
            ]
        );
        assert_eq!(walk.nodes[4].depth, 2.5);
    }

    #[test]
    fn region_order_uses_the_newest_path_and_windows_use_the_cluster_chain() {
        let mut walk = walk(Facing::Straddle, OpenPortals::all(2));
        // Heads at nodes 2 (cluster 1) and 4 (cluster 0).
        for (i, n) in walk.nodes.iter_mut().enumerate() {
            n.rect = Rect([i as f32, i as f32 + 1., 0., 1.]);
        }
        let (clusters, windows) = walk.regions(0.).unwrap();
        assert_eq!(clusters, [1, 0]);
        assert_eq!(
            windows
                .iter()
                .map(|&(c, r)| (c, r.0[0] as i32))
                .collect::<Vec<_>>(),
            [(1, 2), (1, 1), (0, 4), (0, 3), (0, 0)]
        );
    }

    #[test]
    fn facing_and_live_open_bits_limit_the_walk() {
        let walk = walk(Facing::Back, OpenPortals(vec![1]));
        assert_eq!(walk.nodes.len(), 2);
        assert_eq!(walk.nodes[1].portal, Some(0));
    }

    #[test]
    fn equal_edges_retain_the_first_bits() {
        let rect = bounds(Rect([5., -5., 5., -5.]), &[[-0., 0.], [0., -0.]]).unwrap();
        assert_eq!(
            rect.0.map(f32::to_bits),
            [-0.0f32, -0., 0., 0.].map(f32::to_bits)
        );
    }
}
