//! Merge coplanar neighbouring sectors of generated pathfinding data into
//! larger convex sectors, as retail data covers several collision surfaces
//! with one sector. Fewer sectors and the links between them go, so the
//! data is smaller; every surface's ref names its merged sector directly.
//! See docs/formats/pathfinding.md, "Merging".
use std::collections::HashMap;

use crate::geom::{V3, dot, is_convex_xy, newell, signed_area_xy};
use crate::trim::{self, NONE};
use tags::pathfinding::{Link, PathfindingData, Ref, RefTarget, Sector};

/// The most links a merged sector's ring may have (ours, not the engine's:
/// retail rings this long are rare, so a merge stops short of them).
pub const MAX_RING: usize = 16;
/// How far (world units) a vertex may lie off the plane of the sector it
/// joins.
pub const PLANE_TOLERANCE: f32 = 1e-3;
/// The least dot product of two coplanar sectors' unit normals.
const NORMAL_DOT: f32 = 0.9999;

/// One side of a link on a sector's ring, directed counter-clockwise
/// around the sector: `from` to `to`, canonical vertex indices.
#[derive(Clone, Copy, Debug)]
struct Side {
    link: u16,
    /// The sector is the link's left side (its next link is `forward_link`).
    left: bool,
    from: u16,
    to: u16,
}

struct Group {
    /// The ring, in order.
    ring: Vec<Side>,
    /// Unit normal and offset of the first sector's plane.
    normal: V3,
    offset: f32,
}

/// The data with coplanar neighbours merged while the merged polygon stays
/// convex (collinear corners allowed, `geom::is_convex_xy`) and its ring at
/// most [`MAX_RING`] links, and for each old sector its new index. Merged
/// sectors take the flags and hint of their first member and keep the old
/// order of their first members; links between members of one sector go;
/// vertices no link uses go.
///
/// Generated data only, as [`trim::keep_sectors`].
pub fn coplanar(pf: &PathfindingData) -> Result<(PathfindingData, Vec<u16>), &'static str> {
    if !pf.bsp2d_nodes.is_empty() || !pf.object_refs.is_empty() || !pf.hints.is_empty() {
        return Err("only generated data (no 2D BSP nodes, object refs or hints) is merged");
    }
    // Vertices by position, so neighbours' corners compare equal.
    let mut canonical: HashMap<[u32; 3], u16> = HashMap::new();
    let vertex: Vec<u16> = pf
        .vertices
        .iter()
        .enumerate()
        .map(|(i, p)| *canonical.entry(p.map(f32::to_bits)).or_insert(i as u16))
        .collect();
    let at = |v: u16| pf.vertices[usize::from(v)];
    let n = pf.sectors.len();
    let mut groups: Vec<Option<Group>> = Vec::with_capacity(n);
    for s in 0..n {
        let ring = ring(pf, s, &vertex)?;
        let poly: Vec<V3> = ring.iter().map(|e| at(e.from)).collect();
        let a = newell(&poly);
        let len = dot(a, a).sqrt();
        if len == 0.0 {
            return Err("a sector with no area");
        }
        let normal = [a[0] / len, a[1] / len, a[2] / len];
        groups.push(Some(Group {
            offset: dot(normal, poly[0]),
            normal,
            ring,
        }));
    }
    // The group each sector is in: the index of its first member.
    let mut group_of: Vec<usize> = (0..n).collect();
    let mut changed = true;
    while changed {
        changed = false;
        for link in &pf.links {
            let (l, r) = (usize::from(link.left_sector), usize::from(link.right_sector));
            if l >= n || r >= n {
                continue;
            }
            let (a, b) = (group_of[l], group_of[r]);
            if a == b {
                continue;
            }
            let (keep, gone) = (a.min(b), a.max(b));
            let merged = {
                let (Some(x), Some(y)) = (&groups[keep], &groups[gone]) else {
                    return Err("a group merged away");
                };
                join(x, y, |s| group_of[s], keep, gone, pf, &at)
            };
            if let Some(ring) = merged {
                groups[keep].as_mut().expect("kept").ring = ring;
                groups[gone] = None;
                for g in group_of.iter_mut().filter(|g| **g == gone) {
                    *g = keep;
                }
                changed = true;
            }
        }
    }
    // New indices in the order of each group's first member.
    let mut new_of = vec![NONE; n];
    let mut count = 0u16;
    for (first, g) in groups.iter().enumerate() {
        if g.is_some() {
            new_of[first] = count;
            count += 1;
        }
    }
    let sector_of: Vec<u16> = group_of.iter().map(|&g| new_of[g]).collect();
    let side_of = |s: u16| sector_of.get(usize::from(s)).copied().unwrap_or(NONE);
    let mut link_of = vec![NONE; pf.links.len()];
    let mut links: Vec<Link> = Vec::new();
    for (i, l) in pf.links.iter().enumerate() {
        let (left, right) = (side_of(l.left_sector), side_of(l.right_sector));
        if left != NONE && left == right {
            continue;
        }
        let k = u16::try_from(links.len()).map_err(|_| "too many links")?;
        link_of[i] = k;
        links.push(Link {
            forward_link: k,
            reverse_link: k,
            left_sector: left,
            right_sector: right,
            ..*l
        });
    }
    let mut sectors = Vec::with_capacity(usize::from(count));
    for (first, g) in groups.iter().enumerate() {
        let Some(g) = g else { continue };
        for (i, e) in g.ring.iter().enumerate() {
            let next = link_of[usize::from(g.ring[(i + 1) % g.ring.len()].link)];
            let link = &mut links[usize::from(link_of[usize::from(e.link)])];
            if e.left {
                link.forward_link = next;
            } else {
                link.reverse_link = next;
            }
        }
        sectors.push(Sector {
            first_link: u32::from(link_of[usize::from(g.ring[0].link)]),
            ..pf.sectors[first]
        });
    }
    let refs = pf
        .refs
        .iter()
        .map(|&r| match r.target() {
            RefTarget::Sector(s) => match sector_of.get(s as usize) {
                Some(&k) if k != NONE => Ref::sector(u32::from(k)),
                _ => Ref::NONE,
            },
            _ => r,
        })
        .collect();
    let merged = PathfindingData {
        sectors,
        links,
        refs,
        ..pf.clone()
    };
    // Drop the vertices only the gone links used; the order stays.
    let all = vec![true; merged.sectors.len()];
    let (merged, same) = trim::keep_sectors(&merged, &all)?;
    debug_assert!(same.iter().enumerate().all(|(i, &k)| usize::from(k) == i));
    Ok((merged, sector_of))
}

/// Sector `s`'s ring as directed sides.
fn ring(pf: &PathfindingData, s: usize, vertex: &[u16]) -> Result<Vec<Side>, &'static str> {
    let first = usize::try_from(pf.sectors[s].first_link).map_err(|_| "first link")?;
    let mut at = first;
    let mut out = Vec::new();
    loop {
        let l = pf.links.get(at).ok_or("a ring leaves the links")?;
        let (v1, v2) = (
            *vertex.get(usize::from(l.vertex1)).ok_or("link vertex")?,
            *vertex.get(usize::from(l.vertex2)).ok_or("link vertex")?,
        );
        let side = if usize::from(l.left_sector) == s {
            Side { link: at as u16, left: true, from: v1, to: v2 }
        } else if usize::from(l.right_sector) == s {
            Side { link: at as u16, left: false, from: v2, to: v1 }
        } else {
            return Err("a ring link without its sector");
        };
        out.push(side);
        at = usize::from(if side.left { l.forward_link } else { l.reverse_link });
        if at == first {
            return Ok(out);
        }
        if out.len() > pf.links.len() {
            return Err("a ring does not close");
        }
    }
}

/// The ring of `x` and `y` joined across every link between them, when the
/// two are coplanar and the joined polygon is one convex ring of at most
/// [`MAX_RING`] links; `None` otherwise.
fn join(
    x: &Group,
    y: &Group,
    group_of: impl Fn(usize) -> usize,
    gx: usize,
    gy: usize,
    pf: &PathfindingData,
    at: &impl Fn(u16) -> V3,
) -> Option<Vec<Side>> {
    if dot(x.normal, y.normal) < NORMAL_DOT {
        return None;
    }
    if y
        .ring
        .iter()
        .any(|e| (dot(x.normal, at(e.from)) - x.offset).abs() > PLANE_TOLERANCE)
    {
        return None;
    }
    let between = |e: &Side| {
        let l = &pf.links[usize::from(e.link)];
        let side = |s: u16| (usize::from(s) < pf.sectors.len()).then(|| group_of(usize::from(s)));
        let (a, b) = (side(l.left_sector), side(l.right_sector));
        matches!((a, b), (Some(a), Some(b)) if (a == gx && b == gy) || (a == gy && b == gx))
    };
    let sides: Vec<Side> = x.ring.iter().chain(&y.ring).filter(|e| !between(e)).copied().collect();
    if sides.len() < 3 || sides.len() > MAX_RING {
        return None;
    }
    // One side leaves each corner, or the union pinches.
    let mut from: HashMap<u16, usize> = HashMap::new();
    for (i, e) in sides.iter().enumerate() {
        if from.insert(e.from, i).is_some() {
            return None;
        }
    }
    // Start at the ring's lowest link, so the order is the same everywhere.
    let start = (0..sides.len()).min_by_key(|&i| sides[i].link)?;
    let mut ring = Vec::with_capacity(sides.len());
    let mut i = start;
    loop {
        ring.push(sides[i]);
        i = *from.get(&sides[i].to)?;
        if i == start {
            break;
        }
        if ring.len() > sides.len() {
            return None;
        }
    }
    if ring.len() != sides.len() {
        return None;
    }
    let poly: Vec<V3> = ring.iter().map(|e| at(e.from)).collect();
    (signed_area_xy(&poly) > 0.0 && is_convex_xy(&poly, true)).then_some(ring)
}
