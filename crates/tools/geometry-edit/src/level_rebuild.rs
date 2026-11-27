//! A sculpted structure's collision, portals and pathfinding rebuilt and
//! written with the stroke (docs/editor/modeling.md, "Sculpting";
//! docs/editor/modeling-targets.md, "Structure collision", step 3).
//!
//! [`read`] takes what indexes the level collision: the level
//! ([`crate::level_collision::read`]), the portals (`sbsp+0x5c`, `0x24`
//! bytes each: clusters, collision plane `+0x04`, centre `+0x08`, radius
//! `+0x14`, polygon `+0x1c`) and the pathfinding element (`sbsp+0xc4`).
//! [`StructureLevel::rebuild`] gives a [`LevelChange`]:
//!
//! - the level compiled again (`collision_build::rebuild`), with every
//!   portal's plane kept and renumbered;
//! - each portal whose ground edge (two corners and the middle on the
//!   section's old surface) crosses the changed surface, its edge laid
//!   along the new surface: the portal plane's cut through the new
//!   triangles, from corner to corner, with its centre and radius again. A
//!   cut that leaves a gap under the edge is refused, naming the portal;
//! - the pathfinding's sectors, links, refs, 2D nodes and vertices
//!   generated again (`pathfind::generate`), refused while the old data has
//!   hints or object references that name sectors.
//!
//! [`StructureLevel::writes`] is the save: every block patched where its
//! count holds, grown in the ordinary metadata where it does not.

use std::io::{Read, Seek};

use collision_build::Options;
use collision_build::rebuild::{Rebuilt, SectionEdit, Tri, on_triangle};
use map::{Map, TagId};
use tags::bytes::u32_at;
use tags::pathfinding::{PathfindingData, SBSP_PATHFINDING_FIELD};

use crate::level_collision::{self, BlockAt, LevelSource, LevelWrites, level_writes, read_at};
use crate::session::Error;

type Vec3 = [f32; 3];

/// The structure's portal block.
pub const PORTALS_FIELD: usize = 0x5c;
/// One portal element.
pub const PORTAL_SIZE: usize = 0x24;
/// The portal's polygon block, 12 bytes a point.
const PORTAL_VERTICES: usize = 0x1c;
/// The pathfinding blocks a rebuild writes: offset in the element, stride
/// and name.
const PATH_BLOCKS: [(usize, usize, &str); 5] = [
    (0x00, 8, "pathfinding_sectors"),
    (0x08, 16, "pathfinding_links"),
    (0x10, 4, "pathfinding_refs"),
    (0x18, 0x14, "pathfinding_2d_nodes"),
    (0x28, 12, "pathfinding_vertices"),
];

/// One portal as the map holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct PortalSource {
    pub element: BlockAt,
    pub bytes: Vec<u8>,
    pub vertices: BlockAt,
    pub vertex_bytes: Vec<u8>,
    pub points: Vec<Vec3>,
    pub plane: u32,
}

/// The pathfinding element as the map holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct PathSource {
    pub element: BlockAt,
    pub blocks: Vec<(BlockAt, Vec<u8>)>,
    pub hints: u32,
    pub object_refs: u32,
}

/// What a level collision rebuild must keep in step.
#[derive(Clone, Debug, PartialEq)]
pub struct StructureLevel {
    pub level: LevelSource,
    pub portals: Vec<PortalSource>,
    pub pathfinding: Option<PathSource>,
}

/// A rebuilt level and what follows it.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelChange {
    pub rebuilt: Rebuilt,
    /// Per portal, its polygon now.
    pub portals: Vec<Vec<Vec3>>,
    pub pathfinding: Option<PathfindingData>,
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(b, at))
}

/// Reads structure `tag`'s level collision, portals and pathfinding.
pub fn read<R: Read + Seek>(map: &mut Map<R>, tag: TagId) -> Result<StructureLevel, Error> {
    let level = level_collision::read(map, tag)?;
    let loc = map.tag(tag)?.clone().location.ok_or_else(|| Error::NoTarget(format!("{:08x}", tag.0)))?;
    let root = map.read_tag_bytes(tag)?;
    let pair = |at: usize| (u32_at(&root, at), u32_at(&root, at + 4));
    let mut portals = Vec::new();
    let (count, address) = pair(PORTALS_FIELD);
    if count > 0 {
        let (bytes, space) = read_at(map, loc.space, address, count as usize * PORTAL_SIZE)?;
        for k in 0..count as usize {
            let e = bytes[k * PORTAL_SIZE..(k + 1) * PORTAL_SIZE].to_vec();
            let (n, at) = (u32_at(&e, PORTAL_VERTICES), u32_at(&e, PORTAL_VERTICES + 4));
            let (vertex_bytes, vspace) = if n == 0 {
                (Vec::new(), space)
            } else {
                read_at(map, loc.space, at, n as usize * 12)?
            };
            let points = vertex_bytes
                .chunks_exact(12)
                .map(|c| [f32_at(c, 0), f32_at(c, 4), f32_at(c, 8)])
                .collect();
            portals.push(PortalSource {
                element: BlockAt { space, count: 1, address: address + (k * PORTAL_SIZE) as u32 },
                plane: u32_at(&e, 4),
                bytes: e,
                vertices: BlockAt { space: vspace, count: n, address: at },
                vertex_bytes,
                points,
            });
        }
    }
    let (count, address) = pair(SBSP_PATHFINDING_FIELD);
    let pathfinding = if count == 0 {
        None
    } else {
        let (e, space) = read_at(map, loc.space, address, PathfindingData::SIZE)?;
        let mut blocks = Vec::new();
        for (offset, stride, _) in PATH_BLOCKS {
            let (n, at) = (u32_at(&e, offset), u32_at(&e, offset + 4));
            let (bytes, bspace) = if n == 0 {
                (Vec::new(), space)
            } else {
                read_at(map, loc.space, at, n as usize * stride)?
            };
            blocks.push((BlockAt { space: bspace, count: n, address: at }, bytes));
        }
        Some(PathSource {
            element: BlockAt { space, count: 1, address },
            blocks,
            hints: u32_at(&e, 0x38),
            object_refs: u32_at(&e, 0x30),
        })
    };
    Ok(StructureLevel { level, portals, pathfinding })
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mix(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

/// Whether `p` lies on any of `tris`.
pub fn on_any(p: Vec3, tris: &[Tri]) -> bool {
    tris.iter().any(|t| on_triangle(p, t.points))
}

/// Where the plane `n . x = d` cuts a triangle: the segment, if it does.
fn cut(t: &Tri, n: Vec3, d: f32) -> Option<(Vec3, Vec3)> {
    let mut hits = Vec::new();
    for i in 0..3 {
        let (p, q) = (t.points[i], t.points[(i + 1) % 3]);
        let (sp, sq) = (dot(n, p) - d, dot(n, q) - d);
        if sp.abs() < 1e-6 {
            hits.push(p);
        } else if sp * sq < 0.0 && sq.abs() >= 1e-6 {
            hits.push(mix(p, q, sp / (sp - sq)));
        }
    }
    match hits.as_slice() {
        [a, b, ..] => Some((*a, *b)),
        _ => None,
    }
}

/// The new surface along a portal's ground edge `a` to `b`, as points from
/// the point over `a` to the point over `b`: the portal plane's cut through
/// `after`, kept between the two corners. `None` when the cut does not
/// reach from one corner to the other without a gap.
fn ground(a: Vec3, b: Vec3, n: Vec3, d: f32, changed: &[Tri], after: &[Tri]) -> Option<Vec<Vec3>> {
    let dir = sub(b, a);
    let len2 = dot(dir, dir);
    if len2 <= 0.0 {
        return None;
    }
    let s = |p: Vec3| dot(sub(p, a), dir) / len2;
    let near = |t: &Tri| {
        changed.iter().any(|c| c.points == t.points)
            || (0..=8).any(|k| on_triangle(mix(a, b, k as f32 / 8.0), t.points))
    };
    let mut segments: Vec<(f32, Vec3, f32, Vec3)> = after
        .iter()
        .filter(|t| near(t))
        .filter_map(|t| cut(t, n, d))
        .map(|(p, q)| {
            let (sp, sq) = (s(p), s(q));
            if sp <= sq { (sp, p, sq, q) } else { (sq, q, sp, p) }
        })
        .filter(|&(s0, _, s1, _)| s1 >= -1e-4 && s0 <= 1.0 + 1e-4)
        .collect();
    segments.sort_by(|x, y| x.0.total_cmp(&y.0));
    // The cut must cover 0 to 1.
    let mut reach = 0.0f32;
    if segments.first().is_none_or(|x| x.0 > 1e-4) {
        return None;
    }
    for x in &segments {
        if x.0 > reach + 1e-4 {
            return None;
        }
        reach = reach.max(x.2);
    }
    if reach < 1.0 - 1e-4 {
        return None;
    }
    let at = |t: f32| {
        segments.iter().find(|x| x.0 <= t + 1e-6 && x.2 >= t - 1e-6).map(|x| {
            let span = x.2 - x.0;
            if span <= 1e-9 { x.1 } else { mix(x.1, x.3, ((t - x.0) / span).clamp(0.0, 1.0)) }
        })
    };
    let mut inner: Vec<(f32, Vec3)> = segments
        .iter()
        .flat_map(|x| [(x.0, x.1), (x.2, x.3)])
        .filter(|&(t, _)| t > 1e-4 && t < 1.0 - 1e-4)
        .collect();
    inner.sort_by(|x, y| x.0.total_cmp(&y.0));
    inner.dedup_by(|x, y| (x.0 - y.0).abs() < 1e-5);
    let mut out = vec![at(0.0)?];
    out.extend(inner.into_iter().map(|(_, p)| p));
    out.push(at(1.0)?);
    Some(out)
}

/// The portal polygon `points` (in the plane `n . x = d`) with each ground
/// edge on the changed surface laid along the new one. `Ok(None)` when no
/// ground edge crosses the change.
pub fn follow(
    points: &[Vec3],
    n: Vec3,
    d: f32,
    before: &[Tri],
    changed: (&[Tri], &[Tri]),
    after: &[Tri],
) -> Result<Option<Vec<Vec3>>, String> {
    let (gone, made) = changed;
    let k = points.len();
    let mut out: Vec<Vec3> = points.to_vec();
    let mut inserts: Vec<Vec<Vec3>> = vec![Vec::new(); k];
    let mut any = false;
    for i in 0..k {
        let (a, b) = (points[i], points[(i + 1) % k]);
        let on_ground = [a, mix(a, b, 0.5), b].iter().all(|&p| on_any(p, before));
        let crosses = (0..=8).any(|j| on_any(mix(a, b, j as f32 / 8.0), gone));
        if !on_ground || !crosses {
            continue;
        }
        let edge = ground(a, b, n, d, made, after)
            .ok_or_else(|| format!("the stroke leaves a gap under the portal edge from {a:?} to {b:?}"))?;
        out[i] = edge[0];
        out[(i + 1) % k] = *edge.last().expect("two ends");
        inserts[i] = edge[1..edge.len() - 1].to_vec();
        any = true;
    }
    if !any {
        return Ok(None);
    }
    Ok(Some((0..k).flat_map(|i| std::iter::once(out[i]).chain(inserts[i].iter().copied())).collect()))
}

impl StructureLevel {
    /// The level rebuilt after `sections` changed, with the portals and
    /// pathfinding that follow it. The error is the refusal's reason.
    pub fn rebuild(&self, sections: &[SectionEdit]) -> Result<LevelChange, String> {
        let planes: Vec<usize> = self.portals.iter().map(|p| p.plane as usize).collect();
        let rebuilt = self
            .level
            .level
            .rebuild(sections, &planes, &Options::default())
            .map_err(|e| format!("the collision compiler refuses the stroke: {e}"))?;
        let before: Vec<Tri> = sections.iter().flat_map(|s| s.before.iter().copied()).collect();
        let after: Vec<Tri> = sections.iter().flat_map(|s| s.after.iter().copied()).collect();
        let (mut gone, mut made) = (Vec::new(), Vec::new());
        for s in sections {
            let (g, m) = s.changes();
            gone.extend(g);
            made.extend(m);
        }
        let mut portals = Vec::with_capacity(self.portals.len());
        for (k, p) in self.portals.iter().enumerate() {
            let plane = self.level.level.bsp.planes.get(p.plane as usize).map(|q| q.0).ok_or_else(|| {
                format!("portal {k} names plane {}, which the level does not have", p.plane)
            })?;
            let n = [plane[0], plane[1], plane[2]];
            let moved = follow(&p.points, n, plane[3], &before, (&gone, &made), &after)
                .map_err(|why| format!("portal {k}: {why}"))?;
            portals.push(moved.unwrap_or_else(|| p.points.clone()));
        }
        let pathfinding = match &self.pathfinding {
            None => None,
            Some(old) if old.hints > 0 || old.object_refs > 0 => {
                return Err(format!(
                    "the structure's pathfinding has {} hints and {} object references that name sectors, which a rebuilt level cannot keep",
                    old.hints, old.object_refs
                ));
            }
            Some(_) => Some(
                pathfind::generate::generate(&rebuilt.level.bsp, 0, 0, &pathfind::generate::Options::default())
                    .map_err(|e| format!("the pathfinding cannot be generated again: {e}"))?,
            ),
        };
        Ok(LevelChange { rebuilt, portals, pathfinding })
    }

    /// What a save writes for `change`.
    pub fn writes(&self, change: &LevelChange) -> LevelWrites {
        let mut out = level_writes(&self.level, &change.rebuilt.level);
        for (k, p) in self.portals.iter().enumerate() {
            let points = &change.portals[k];
            let mut e = p.bytes.clone();
            e[4..8].copy_from_slice(&change.rebuilt.planes[k].to_le_bytes());
            if *points != p.points {
                let n = points.len().max(1) as f32;
                let c = points.iter().fold([0.0f32; 3], |a, q| [a[0] + q[0] / n, a[1] + q[1] / n, a[2] + q[2] / n]);
                let r = points.iter().map(|q| dot(sub(*q, c), sub(*q, c)).sqrt()).fold(0.0, f32::max);
                for (i, v) in c.iter().chain([&r]).enumerate() {
                    e[8 + 4 * i..12 + 4 * i].copy_from_slice(&v.to_le_bytes());
                }
                let bytes: Vec<u8> = points.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
                out.block(
                    "portal_vertices",
                    (p.vertices, &p.vertex_bytes),
                    (p.element.space, p.element.address + PORTAL_VERTICES as u32),
                    (points.len() as u32, &bytes),
                );
            }
            out.block("portal", (p.element, &p.bytes), (p.element.space, p.element.address), (1, &e));
        }
        if let (Some(old), Some(pf)) = (&self.pathfinding, &change.pathfinding) {
            let encoded = path_blocks(pf);
            for (i, (offset, _, name)) in PATH_BLOCKS.into_iter().enumerate() {
                let (count, bytes) = &encoded[i];
                out.block(
                    name,
                    (old.blocks[i].0, &old.blocks[i].1),
                    (old.element.space, old.element.address + offset as u32),
                    (*count, bytes),
                );
            }
        }
        out
    }
}

/// The five rewritten pathfinding blocks, as `pathfind::build::element`
/// lays them out: count and bytes each.
fn path_blocks(pf: &PathfindingData) -> [(u32, Vec<u8>); 5] {
    let mut sectors = Vec::new();
    for s in &pf.sectors {
        sectors.extend(s.flags.to_le_bytes());
        sectors.extend(s.hint.to_le_bytes());
        sectors.extend(s.first_link.to_le_bytes());
    }
    let mut links = Vec::new();
    for l in &pf.links {
        for v in [l.vertex1, l.vertex2, l.flags, l.hint, l.forward_link, l.reverse_link, l.left_sector, l.right_sector] {
            links.extend(v.to_le_bytes());
        }
    }
    let refs = pf.refs.iter().flat_map(|r| r.0.to_le_bytes()).collect();
    let mut nodes = Vec::new();
    for n in &pf.bsp2d_nodes {
        for f in n.plane {
            nodes.extend(f.to_le_bytes());
        }
        nodes.extend(n.positive.0.to_le_bytes());
        nodes.extend(n.negative.0.to_le_bytes());
    }
    let vertices = pf.vertices.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
    [
        (pf.sectors.len() as u32, sectors),
        (pf.links.len() as u32, links),
        (pf.refs.len() as u32, refs),
        (pf.bsp2d_nodes.len() as u32, nodes),
        (pf.vertices.len() as u32, vertices),
    ]
}

#[cfg(test)]
#[path = "level_rebuild_tests.rs"]
mod tests;
