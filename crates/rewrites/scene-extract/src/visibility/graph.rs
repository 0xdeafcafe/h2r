//! The structure's portal graph and potentially visible set, decoded from
//! its tag data into Rust-owned arrays once per structure
//! (docs/graphics/native-visibility.md, "The graph"). Nothing here is read
//! per frame: the flood and the object set work on these arrays.

use super::Decline;
use super::read::{at, f32_at, ptr_at, u8_at, u16_at, u32_at, vec3_at};
use crate::GuestRead;

/// Clusters a structure may have: the flood's cluster ids and the
/// object reference heads are bounded at 512.
pub const MAX_CLUSTERS: usize = 512;
/// Portals a structure may have: the flood's per-path portal set is 16
/// words (`node + 0x2c`), so a portal index of 512 or more would write past it.
pub const MAX_PORTALS: usize = 512;
/// Vertices a portal may have: the projection transforms them into a
/// 64-entry frame array (`0x00173520`).
pub const MAX_PORTAL_VERTICES: usize = 64;

/// Structure tag offsets (docs/formats/structure-bsp.md; the reads of
/// `0x00172840`, `0x00173520` and `0x0016de20`).
pub mod layout {
    /// The structure's own collision BSP header; its plane block is at `+8`
    /// (count) and `+0xc` (pointer), 16-byte planes.
    pub const COLLISION_BSP: u32 = 0x18;
    pub const BSP_PLANE_COUNT: u32 = 0x8;
    pub const BSP_PLANES: u32 = 0xc;
    /// The potentially visible set: a bit row of `(clusters + 31) / 32`
    /// words per cluster.
    pub const PVS: u32 = 0x58;
    pub const PORTAL_COUNT: u32 = 0x5c;
    pub const PORTALS: u32 = 0x60;
    pub const CLUSTER_COUNT: u32 = 0x9c;
    pub const CLUSTERS: u32 = 0xa0;
    pub const CLUSTER_STRIDE: u32 = 0xb0;
    /// A cluster's flags byte; bit 1 makes `0x0016de20` compute the row
    /// instead of copying it.
    pub const CLUSTER_FLAGS: u32 = 0x80;
    pub const CLUSTER_PORTAL_COUNT: u32 = 0x8c;
    pub const CLUSTER_PORTALS: u32 = 0x90;
    /// A portal: `+0` and `+2` its two clusters, `+4` its plane index,
    /// `+8` its bounding sphere, `+0x18` its flags, `+0x1c` the vertex
    /// count, `+0x20` the vertices (12-byte points).
    pub const PORTAL_STRIDE: u32 = 36;
    pub const PORTAL_PLANE: u32 = 0x4;
    pub const PORTAL_FLAGS: u32 = 0x18;
    pub const PORTAL_VERTEX_COUNT: u32 = 0x1c;
    pub const PORTAL_VERTICES: u32 = 0x20;
}

/// Portal flag bits the flood reads.
pub mod portal_flags {
    /// Closed to a camera on the plane's negative side.
    pub const CLOSED_FROM_BACK: u32 = 0x2;
    /// Closed to a camera on the plane's positive side.
    pub const CLOSED_FROM_FRONT: u32 = 0x10;
}

/// One portal between two clusters.
#[derive(Clone, Debug, PartialEq)]
pub struct Portal {
    /// The cluster behind the plane (`+0`) and the one in front (`+2`).
    pub clusters: [u16; 2],
    /// `nx, ny, nz, d`: a point `p` is in front when `n · p - d > 0`.
    pub plane: [f32; 4],
    pub flags: u32,
    /// The polygon, world space.
    pub vertices: Vec<[f32; 3]>,
}

impl Portal {
    /// The cluster on the other side from `cluster`: the second when the
    /// first is `cluster`, else the first (the guest's comparison).
    pub fn across(&self, cluster: u16) -> u16 {
        if self.clusters[0] == cluster {
            self.clusters[1]
        } else {
            self.clusters[0]
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cluster {
    /// Indices into [`PortalGraph::portals`], in the tag's order.
    pub portals: Vec<u16>,
    /// Its flags byte's bit 1: the potentially visible set is computed at
    /// run time, which is not ported.
    pub computed_pvs: bool,
}

/// A structure's clusters, portals and potentially visible set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PortalGraph {
    pub clusters: Vec<Cluster>,
    pub portals: Vec<Portal>,
    /// Words per row of [`Self::pvs`]: `(clusters + 31) / 32`.
    pub pvs_words: usize,
    /// One bit row per cluster; empty when the structure has none.
    pub pvs: Vec<u32>,
}

impl PortalGraph {
    /// Whether `to` is in `from`'s potentially visible set. `None` when the
    /// structure has no set or either cluster is out of range.
    pub fn potentially_visible(&self, from: u16, to: u16) -> Option<bool> {
        let (from, to) = (usize::from(from), usize::from(to));
        if from >= self.clusters.len() || to >= self.clusters.len() {
            return None;
        }
        let word = self.pvs.get(from * self.pvs_words + to / 32)?;
        Some(word & (1u32 << (to % 32)) != 0)
    }

    /// The graph's identity for the cache: what a new structure at the same
    /// address would change.
    pub fn shape(&self) -> (usize, usize) {
        (self.clusters.len(), self.portals.len())
    }
}

/// Decodes the structure tag at `structure`.
pub fn read(mem: &dyn GuestRead, structure: u32) -> Result<PortalGraph, Decline> {
    use layout::*;
    let unread = Decline::Unread;
    let cluster_count =
        u32_at(mem, at(structure, CLUSTER_COUNT).ok_or(unread)?).ok_or(unread)? as usize;
    let portal_count =
        u32_at(mem, at(structure, PORTAL_COUNT).ok_or(unread)?).ok_or(unread)? as usize;
    if cluster_count > MAX_CLUSTERS || portal_count > MAX_PORTALS {
        return Err(Decline::Layout);
    }
    let bsp = ptr_at(mem, at(structure, COLLISION_BSP).ok_or(unread)?).ok_or(unread)?;
    let plane_count = u32_at(mem, at(bsp, BSP_PLANE_COUNT).ok_or(unread)?).ok_or(unread)?;
    let planes = if portal_count == 0 {
        0
    } else {
        ptr_at(mem, at(bsp, BSP_PLANES).ok_or(unread)?).ok_or(unread)?
    };
    let mut portals = Vec::with_capacity(portal_count);
    if portal_count != 0 {
        let base = ptr_at(mem, at(structure, PORTALS).ok_or(unread)?).ok_or(unread)?;
        for i in 0..portal_count as u32 {
            let p = at(base, i.checked_mul(PORTAL_STRIDE).ok_or(Decline::Layout)?).ok_or(unread)?;
            let clusters = [
                u16_at(mem, p).ok_or(unread)?,
                u16_at(mem, at(p, 2).ok_or(unread)?).ok_or(unread)?,
            ];
            if clusters.iter().any(|&c| usize::from(c) >= cluster_count) {
                return Err(Decline::Layout);
            }
            let plane_index = u32_at(mem, at(p, PORTAL_PLANE).ok_or(unread)?).ok_or(unread)?;
            if plane_index >= plane_count {
                return Err(Decline::Layout);
            }
            let plane_at =
                at(planes, plane_index.checked_mul(16).ok_or(Decline::Layout)?).ok_or(unread)?;
            let [nx, ny, nz] = vec3_at(mem, plane_at).ok_or(unread)?;
            let d = f32_at(mem, at(plane_at, 12).ok_or(unread)?).ok_or(unread)?;
            let flags = u32_at(mem, at(p, PORTAL_FLAGS).ok_or(unread)?).ok_or(unread)?;
            let count =
                u32_at(mem, at(p, PORTAL_VERTEX_COUNT).ok_or(unread)?).ok_or(unread)? as usize;
            if count > MAX_PORTAL_VERTICES {
                return Err(Decline::Layout);
            }
            let mut vertices = Vec::with_capacity(count);
            if count != 0 {
                let v = ptr_at(mem, at(p, PORTAL_VERTICES).ok_or(unread)?).ok_or(unread)?;
                for k in 0..count as u32 {
                    vertices.push(vec3_at(mem, at(v, 12 * k).ok_or(unread)?).ok_or(unread)?);
                }
            }
            portals.push(Portal {
                clusters,
                plane: [nx, ny, nz, d],
                flags,
                vertices,
            });
        }
    }
    let mut clusters = Vec::with_capacity(cluster_count);
    if cluster_count != 0 {
        let base = ptr_at(mem, at(structure, CLUSTERS).ok_or(unread)?).ok_or(unread)?;
        for i in 0..cluster_count as u32 {
            let c = at(base, i * CLUSTER_STRIDE).ok_or(unread)?;
            let flags = u8_at(mem, at(c, CLUSTER_FLAGS).ok_or(unread)?).ok_or(unread)?;
            let count =
                u32_at(mem, at(c, CLUSTER_PORTAL_COUNT).ok_or(unread)?).ok_or(unread)? as i32;
            let mut list = Vec::new();
            if count > 0 {
                if count as usize > MAX_PORTALS {
                    return Err(Decline::Layout);
                }
                let indices = ptr_at(mem, at(c, CLUSTER_PORTALS).ok_or(unread)?).ok_or(unread)?;
                for k in 0..count as u32 {
                    let index = u16_at(mem, at(indices, 2 * k).ok_or(unread)?).ok_or(unread)?;
                    if usize::from(index) >= portal_count {
                        return Err(Decline::Layout);
                    }
                    list.push(index);
                }
            }
            clusters.push(Cluster {
                portals: list,
                computed_pvs: flags & 2 != 0,
            });
        }
    }
    let pvs_words = cluster_count.div_ceil(32);
    let pvs = match ptr_at(mem, at(structure, PVS).ok_or(unread)?) {
        Some(rows) if cluster_count != 0 => (0..(cluster_count * pvs_words) as u32)
            .map(|w| u32_at(mem, at(rows, 4 * w)?))
            .collect::<Option<Vec<u32>>>()
            .ok_or(unread)?,
        _ => Vec::new(),
    };
    Ok(PortalGraph {
        clusters,
        portals,
        pvs_words,
        pvs,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::testing::Mem;

    /// Two clusters joined by one square portal in the plane x = 4, a PVS
    /// that sees both from 0 and only itself from 1.
    pub(crate) fn two_rooms(m: &mut Mem, structure: u32) {
        let (bsp, planes, portals, vertices, clusters, links, pvs) =
            (0x9000, 0x9100, 0x9200, 0x9300, 0x9400, 0x9600, 0x9700);
        m.w(structure + layout::COLLISION_BSP, bsp);
        m.w(bsp + layout::BSP_PLANE_COUNT, 2);
        m.w(bsp + layout::BSP_PLANES, planes);
        // Plane 1: x = 4.
        for (i, v) in [1.0, 0.0, 0.0, 4.0].into_iter().enumerate() {
            m.f(planes + 16 + 4 * i as u32, v);
        }
        m.w(structure + layout::PORTAL_COUNT, 1);
        m.w(structure + layout::PORTALS, portals);
        m.w(portals, 1 << 16); // back 0, front 1
        m.w(portals + layout::PORTAL_PLANE, 1);
        m.w(portals + layout::PORTAL_FLAGS, 0x10);
        m.w(portals + layout::PORTAL_VERTEX_COUNT, 4);
        m.w(portals + layout::PORTAL_VERTICES, vertices);
        for (k, [y, z]) in [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]
            .into_iter()
            .enumerate()
        {
            m.f(vertices + 12 * k as u32, 4.0);
            m.f(vertices + 12 * k as u32 + 4, y);
            m.f(vertices + 12 * k as u32 + 8, z);
        }
        m.w(structure + layout::CLUSTER_COUNT, 2);
        m.w(structure + layout::CLUSTERS, clusters);
        for c in 0..2 {
            let at = clusters + c * layout::CLUSTER_STRIDE;
            m.w(at + layout::CLUSTER_FLAGS, if c == 1 { 2 } else { 0 });
            m.w(at + layout::CLUSTER_PORTAL_COUNT, 1);
            m.w(at + layout::CLUSTER_PORTALS, links);
        }
        m.w(links, 0);
        m.w(structure + layout::PVS, pvs);
        m.w(pvs, 0b11);
        m.w(pvs + 4, 0b10);
    }

    #[test]
    fn a_structure_decodes_into_clusters_portals_and_its_visible_set() {
        let mut m = Mem::default();
        two_rooms(&mut m, 0x8000);
        let g = read(&m, 0x8000).unwrap();
        assert_eq!(g.shape(), (2, 1));
        let p = &g.portals[0];
        assert_eq!(p.clusters, [0, 1]);
        assert_eq!(p.plane, [1.0, 0.0, 0.0, 4.0]);
        assert_eq!(p.flags, portal_flags::CLOSED_FROM_FRONT);
        assert_eq!(p.vertices.len(), 4);
        assert_eq!((p.across(0), p.across(1)), (1, 0));
        assert_eq!(g.clusters[0].portals, [0]);
        assert!(!g.clusters[0].computed_pvs && g.clusters[1].computed_pvs);
        assert_eq!(g.potentially_visible(0, 1), Some(true));
        assert_eq!(g.potentially_visible(1, 0), Some(false));
        assert_eq!(g.potentially_visible(2, 0), None);
    }

    #[test]
    fn out_of_range_links_and_planes_decline_and_missing_memory_is_unread() {
        let mut m = Mem::default();
        two_rooms(&mut m, 0x8000);
        m.w(0x9200 + layout::PORTAL_PLANE, 2);
        assert_eq!(read(&m, 0x8000), Err(Decline::Layout));
        two_rooms(&mut m, 0x8000);
        m.w(0x9200, 2 << 16);
        assert_eq!(
            read(&m, 0x8000),
            Err(Decline::Layout),
            "a portal to cluster 2 of 2"
        );
        two_rooms(&mut m, 0x8000);
        m.w(0x9600, 1);
        assert_eq!(
            read(&m, 0x8000),
            Err(Decline::Layout),
            "a cluster names portal 1 of 1"
        );
        two_rooms(&mut m, 0x8000);
        m.w(0x8000 + layout::PORTAL_COUNT, MAX_PORTALS as u32 + 1);
        assert_eq!(read(&m, 0x8000), Err(Decline::Layout));
        assert_eq!(read(&m, 0x1_0000), Err(Decline::Unread));
    }
}
