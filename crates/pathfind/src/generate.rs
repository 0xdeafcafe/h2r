//! Collision BSP to pathfinding data.
//!
//! One sector per walkable collision surface (normal within `max_slope` of
//! straight up), with the surface's own polygon. Links are the collision
//! edges of those polygons: two-sided where the surface on the other side is
//! walkable too, one-sided otherwise. Each walkable surface's ref names its
//! sector directly, so no 2D BSP nodes are needed. See
//! docs/formats/pathfinding.md, "Generator".
use crate::geom::{V3, newell};
use std::collections::HashMap;
use tags::collision::Bsp;
use tags::pathfinding::{Link, PathfindingData, Ref, Sector};

/// Generator settings.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// A surface is walkable when its unit normal's z is at least this.
    /// V: on retail levels the walkable cut is at cos 45° (0.7071).
    pub min_normal_z: f32,
    /// Flags of every generated sector. `0x19` is the most common value on
    /// retail floor sectors that come from the structure BSP.
    pub sector_flags: u16,
    /// Flags of every generated link. Bit 0 is set on links made from
    /// collision edges in retail data (S); nothing else is claimed.
    pub link_flags: u16,
    /// Surfaces with a smaller XY-projected area are skipped (square units).
    pub min_area: f32,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            min_normal_z: std::f32::consts::FRAC_1_SQRT_2 - 1e-4,
            sector_flags: 0x19,
            link_flags: 0x1,
            min_area: 1e-4,
        }
    }
}

/// Why generation failed.
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// More sectors, links or vertices than 16-bit indices can name.
    TooLarge(&'static str, usize),
    /// A surface's edge ring is malformed (does not close, or names a
    /// missing edge/vertex).
    BadRing(usize),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge(what, n) => write!(f, "{n} {what} exceed 16-bit indices"),
            Self::BadRing(s) => write!(f, "surface {s}: malformed edge ring"),
        }
    }
}
impl std::error::Error for Error {}

/// Unit normal of a surface as the engine sees it (plane, bit 15 flips).
pub fn surface_normal(bsp: &Bsp, surface: usize) -> Option<V3> {
    let s = bsp.surfaces.get(surface)?;
    let p = bsp.planes.get(usize::from(s.plane & 0x7fff))?.0;
    let sign = if s.plane & 0x8000 != 0 { -1.0 } else { 1.0 };
    Some([sign * p[0], sign * p[1], sign * p[2]])
}

/// The edges around a surface with the vertex each starts at, following the
/// ring rule of docs/formats/collision.md.
pub fn surface_edges(bsp: &Bsp, surface: usize) -> Option<Vec<(usize, usize)>> {
    let first = usize::from(bsp.surfaces.get(surface)?.first_edge);
    let mut out = Vec::new();
    let mut e = first;
    for _ in 0..=bsp.edges.len() {
        let edge = bsp.edges.get(e)?;
        let right = usize::from(edge.right_surface) == surface;
        let left = usize::from(edge.left_surface) == surface;
        if !right && !left {
            return None;
        }
        let (v, next) = if right {
            (edge.end_vertex, edge.reverse_edge)
        } else {
            (edge.start_vertex, edge.forward_edge)
        };
        if usize::from(v) >= bsp.vertices.len() {
            return None;
        }
        out.push((e, usize::from(v)));
        e = usize::from(next);
        if e == first {
            return Some(out);
        }
    }
    None
}

/// Whether `surface` is walkable under `opts`.
pub fn is_walkable(bsp: &Bsp, surface: usize, opts: &Options) -> bool {
    let Some(n) = surface_normal(bsp, surface) else {
        return false;
    };
    if n[2] < opts.min_normal_z {
        return false;
    }
    let Some(ring) = surface_edges(bsp, surface) else {
        return false;
    };
    if ring.len() < 3 {
        return false;
    }
    let pts: Vec<V3> = ring.iter().map(|&(_, v)| bsp.vertices[v].point).collect();
    // Newell z = 2 × projected area, positive when counter-clockwise from above.
    newell(&pts)[2] * 0.5 >= opts.min_area
}

/// Generate pathfinding data from a structure BSP's collision BSP.
/// `instances` is the BSP's instanced-geometry placement count (one
/// instance ref each); `checksum` goes to `+0x48`.
pub fn generate(
    bsp: &Bsp,
    instances: usize,
    checksum: u32,
    opts: &Options,
) -> Result<PathfindingData, Error> {
    let n = bsp.surfaces.len();
    let mut sector_of = vec![u16::MAX; n];
    let mut rings: Vec<(usize, Vec<(usize, usize)>)> = Vec::new();
    for (s, slot) in sector_of.iter_mut().enumerate() {
        if !is_walkable(bsp, s, opts) {
            continue;
        }
        let ring = surface_edges(bsp, s).ok_or(Error::BadRing(s))?;
        let index = rings.len();
        if index >= usize::from(u16::MAX) {
            return Err(Error::TooLarge("sectors", index + 1));
        }
        *slot = index as u16;
        rings.push((s, ring));
    }

    // One link per collision edge, oriented start -> end: the surface that
    // walks it that way (its left surface) is the link's left sector.
    let mut link_of: HashMap<usize, usize> = HashMap::new();
    let mut vertex_of: HashMap<usize, u16> = HashMap::new();
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let mut vertex = |v: usize, vertices: &mut Vec<[f32; 3]>| -> Result<u16, Error> {
        if let Some(&i) = vertex_of.get(&v) {
            return Ok(i);
        }
        let i = vertices.len();
        if i >= usize::from(u16::MAX) {
            return Err(Error::TooLarge("vertices", i + 1));
        }
        vertices.push(bsp.vertices[v].point);
        vertex_of.insert(v, i as u16);
        Ok(i as u16)
    };
    let side = |s: u16| -> u16 {
        if usize::from(s) < n {
            sector_of[usize::from(s)]
        } else {
            u16::MAX
        }
    };
    for (_, ring) in &rings {
        for &(e, _) in ring {
            if link_of.contains_key(&e) {
                continue;
            }
            let edge = &bsp.edges[e];
            let i = links.len();
            if i >= usize::from(u16::MAX) {
                return Err(Error::TooLarge("links", i + 1));
            }
            links.push(Link {
                vertex1: vertex(usize::from(edge.start_vertex), &mut vertices)?,
                vertex2: vertex(usize::from(edge.end_vertex), &mut vertices)?,
                flags: opts.link_flags,
                hint: u16::MAX,
                // Unused sides point at the link itself: in range, never walked.
                forward_link: i as u16,
                reverse_link: i as u16,
                left_sector: side(edge.left_surface),
                right_sector: side(edge.right_surface),
            });
            link_of.insert(e, i);
        }
    }
    let mut sectors = Vec::with_capacity(rings.len());
    for (k, (_, ring)) in rings.iter().enumerate() {
        for (j, &(e, _)) in ring.iter().enumerate() {
            let next = link_of[&ring[(j + 1) % ring.len()].0] as u16;
            let l = &mut links[link_of[&e]];
            if usize::from(l.left_sector) == k {
                l.forward_link = next;
            } else {
                l.reverse_link = next;
            }
        }
        sectors.push(Sector {
            flags: opts.sector_flags,
            hint: u16::MAX,
            first_link: link_of[&ring[0].0] as u32,
        });
    }
    let refs = sector_of
        .iter()
        .map(|&s| {
            if s == u16::MAX {
                Ref::NONE
            } else {
                Ref::sector(u32::from(s))
            }
        })
        .collect();
    Ok(PathfindingData {
        sectors,
        links,
        refs,
        bsp2d_nodes: Vec::new(),
        vertices,
        object_refs: Vec::new(),
        hints: Vec::new(),
        instance_refs: vec![0xffff; instances],
        checksum,
        user_hints: vec![vec![0; PathfindingData::USER_HINTS_SIZE]],
    })
}
