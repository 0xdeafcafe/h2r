//! Collision BSP to pathfinding data.
//!
//! One sector per walkable collision surface (normal within `max_slope` of
//! straight up), with the surface's own polygon. Links are the collision
//! edges of those polygons: two-sided where the surface on the other side is
//! walkable too, one-sided otherwise. Each walkable surface's ref names its
//! sector directly, so no 2D BSP nodes are needed. See
//! docs/formats/pathfinding.md, "Generator".
use crate::geom::V3;
use std::cmp::Ordering;
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
    let mut ring = Vec::new();
    read_surface_edges(bsp, surface, &mut ring)?;
    Some(ring)
}

fn read_surface_edges(bsp: &Bsp, surface: usize, out: &mut Vec<(usize, usize)>) -> Option<()> {
    out.clear();
    let first = usize::from(bsp.surfaces.get(surface)?.first_edge);
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
            return Some(());
        }
    }
    None
}

/// Whether `surface` is walkable under `opts`.
pub fn is_walkable(bsp: &Bsp, surface: usize, opts: &Options) -> bool {
    normal_walkable(bsp, surface, opts)
        && surface_edges(bsp, surface).is_some_and(|ring| area_walkable(bsp, &ring, opts))
}

fn normal_walkable(bsp: &Bsp, surface: usize, opts: &Options) -> bool {
    surface_normal(bsp, surface)
        .is_some_and(|n| n[2].partial_cmp(&opts.min_normal_z) != Some(Ordering::Less))
}

fn area_walkable(bsp: &Bsp, ring: &[(usize, usize)], opts: &Options) -> bool {
    if ring.len() < 3 {
        return false;
    }
    // Newell z = 2 × projected area, positive when counter-clockwise from above.
    let mut twice_area = 0.0;
    for (&(_, a), &(_, b)) in ring.iter().zip(ring.iter().cycle().skip(1)) {
        let u = bsp.vertices[a].point;
        let v = bsp.vertices[b].point;
        twice_area += (u[0] - v[0]) * (u[1] + v[1]);
    }
    twice_area * 0.5 >= opts.min_area
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
    generate_impl(bsp, instances, checksum, opts, None::<fn(&[(usize, usize)])>)
}

/// Generates pathfinding data and visits every valid surface ring in BSP
/// order. A caller that also needs non-walkable collision can consume
/// those rings here without walking each surface again.
pub fn generate_with_surfaces(
    bsp: &Bsp,
    instances: usize,
    checksum: u32,
    opts: &Options,
    surface: impl FnMut(&[(usize, usize)]),
) -> Result<PathfindingData, Error> {
    generate_impl(bsp, instances, checksum, opts, Some(surface))
}

fn generate_impl(
    bsp: &Bsp,
    instances: usize,
    checksum: u32,
    opts: &Options,
    mut surface: Option<impl FnMut(&[(usize, usize)])>,
) -> Result<PathfindingData, Error> {
    let n = bsp.surfaces.len();
    let mut sector_of = vec![u16::MAX; n];
    let mut rings = Vec::new();
    let mut ring = Vec::new();
    for (s, slot) in sector_of.iter_mut().enumerate() {
        let walkable = normal_walkable(bsp, s, opts);
        if !walkable && surface.is_none() {
            continue;
        }
        if read_surface_edges(bsp, s, &mut ring).is_none() {
            continue;
        }
        if let Some(visit) = &mut surface {
            visit(&ring);
        }
        if !walkable || !area_walkable(bsp, &ring, opts) {
            continue;
        }
        let index = rings.len();
        if index >= usize::from(u16::MAX) {
            return Err(Error::TooLarge("sectors", index + 1));
        }
        *slot = index as u16;
        rings.push(std::mem::take(&mut ring));
    }

    // Both remaps are indexed by the collision's 16-bit IDs. Assign output
    // IDs on first encounter as before, regardless of input ID.
    let index_count = usize::from(u16::MAX) + 1;
    let mut link_of = vec![u16::MAX; bsp.edges.len().min(index_count)];
    let mut vertex_of = vec![u16::MAX; bsp.vertices.len().min(index_count)];
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut links: Vec<Link> = Vec::new();
    let mut vertex = |v: usize, vertices: &mut Vec<[f32; 3]>| -> Result<u16, Error> {
        let slot = &mut vertex_of[v];
        if *slot != u16::MAX {
            return Ok(*slot);
        }
        let i = vertices.len();
        if i >= usize::from(u16::MAX) {
            return Err(Error::TooLarge("vertices", i + 1));
        }
        vertices.push(bsp.vertices[v].point);
        *slot = i as u16;
        Ok(*slot)
    };
    let side = |s: u16| -> u16 {
        if usize::from(s) < n {
            sector_of[usize::from(s)]
        } else {
            u16::MAX
        }
    };
    for ring in &rings {
        for &(e, _) in ring {
            if link_of[e] != u16::MAX {
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
            link_of[e] = i as u16;
        }
    }
    let mut sectors = Vec::with_capacity(rings.len());
    for (k, ring) in rings.iter().enumerate() {
        for (j, &(e, _)) in ring.iter().enumerate() {
            let next = link_of[ring[(j + 1) % ring.len()].0];
            let l = &mut links[usize::from(link_of[e])];
            if usize::from(l.left_sector) == k {
                l.forward_link = next;
            } else {
                l.reverse_link = next;
            }
        }
        sectors.push(Sector {
            flags: opts.sector_flags,
            hint: u16::MAX,
            first_link: u32::from(link_of[ring[0].0]),
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
