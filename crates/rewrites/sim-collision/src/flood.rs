//! The sphere cluster flood, `0x0014a5b0` (docs/engine/collision-queries.md,
//! entry G), with the portal test `0x0014a370`, the dominant axis
//! `0x00120850` and the polygon test `0x0023a160` folded in.
//!
//! The structure's portal graph is static tag data, so it is decoded once
//! into a [`Topology`]: per cluster its portal list, per portal its two
//! clusters, its distance plane, its bounding sphere and its polygon already
//! projected onto the two axes the guest would pick for it. A flood is then
//! a depth-first walk over those arrays with an explicit stack. It reads
//! only the live cluster marks from the guest, and returns the visit order;
//! the adapter writes the output array and the marks.
//!
//! The arithmetic keeps the retail operation order. The plane distance and
//! the cross-section radius are x87 sequences ([`Chain`], in the guest's
//! precision control); the sphere and polygon tests are SSE scalar
//! operations ([`Checked`]). Under the guest's state
//! ([`Topology::cluster_flood_guest`]) every operand runs as the guest's
//! instructions do; [`Topology::cluster_flood`] keeps the checked path.
use crate::{Bail, GuestRead, Raised, store::Column};
use sim_core::det::{Chain, Checked};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;

/// Clusters a flood may visit. The guest's object visit floods into a
/// 512-entry array, and a cluster mark at index 512 or above would land on
/// the instance-trace globals that follow the mark array. A cluster index
/// at or past this bound declines.
pub const MAX_CLUSTERS: u32 = 512;
/// Polygon vertices the portal test copies into its 0x400-byte frame.
pub const MAX_SIDES: u32 = 128;
/// Portal entries one cluster may list: the guest counts them in an `i16`.
pub const MAX_PORTALS: u32 = 0x7fff;

/// Structure tag offsets the flood reads (docs/formats/structure-bsp.md).
pub mod layout {
    /// The pointer to the structure's own collision BSP header.
    pub const COLLISION_BSP: u32 = 0x18;
    /// The portal block's pointer (36-byte records).
    pub const PORTALS: u32 = 0x60;
    /// The cluster block: count, then pointer (0xb0-byte records).
    pub const CLUSTER_COUNT: u32 = 0x9c;
    pub const CLUSTERS: u32 = 0xa0;
    pub const CLUSTER_STRIDE: u32 = 0xb0;
    /// In a cluster: the portal index block (count, then pointer, `i16`s).
    pub const CLUSTER_PORTAL_COUNT: u32 = 0x8c;
    pub const PORTAL_STRIDE: u32 = 36;
    /// In a portal: the two clusters (`i16`), the plane index (`u32`), the
    /// bounding sphere (centre, radius), the vertex block (count, pointer,
    /// 12-byte points).
    pub const PORTAL_PLANE: u32 = 4;
    pub const PORTAL_SPHERE: u32 = 8;
    pub const PORTAL_VERTEX_COUNT: u32 = 0x1c;
    /// A collision BSP header's plane block pointer (16-byte planes).
    pub const BSP_PLANES: u32 = 0xc;
}

/// The two guest pointers that name a topology: the active structure tag
/// (`[0x4e0348]`) and the collision BSP the projection's normal is read
/// from (`[0x4e033c]`). The distance plane comes from the structure's own
/// collision BSP; the guest reads the two separately and so does this.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TopologyKey {
    pub structure: u32,
    pub bsp: u32,
}

/// What the portal test needs of one portal, decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Test {
    /// `nx, ny, nz, d` bits of the distance plane.
    pub plane: [u32; 4],
    /// Bounding sphere centre and radius bits.
    pub sphere: [u32; 4],
    /// The projection, which the guest reads only for a sphere that passes
    /// both the plane and the bounding sphere; or why it cannot be
    /// reproduced (a fault reading the normal or the polygon, a normal off
    /// the fast path, a vertex count that would overrun the guest's frame).
    pub projection: Result<Projection, Bail>,
}

/// The portal's polygon as the polygon test sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    /// The projection plane's normal bits.
    pub normal: [u32; 3],
    /// The `u` and `v` axes of the projection row (`0..3`).
    pub axes: [u8; 2],
    /// The projected polygon, `(u, v)` bits per vertex, in [`Topology::ring`].
    pub ring: Range<u32>,
    /// The vertex count as the polygon test reads it (the low half of the
    /// count word, signed). Zero or negative: the test passes.
    pub sides: i16,
    /// The MXCSR and x87 flags the axis pick and the sign test raise (a
    /// NaN or subnormal component), which every test that reaches the
    /// projection raises again.
    pub pick_flags: (u32, u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Portal {
    /// The clusters either side, as the guest compares them (`u16` words).
    pub clusters: [u16; 2],
    /// The test, or why it cannot be reproduced (a fault reading the
    /// distance plane).
    pub test: Result<Test, Bail>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cluster {
    /// The portal count word, as the guest leaves it in `ecx`.
    count: u32,
    /// The cluster's portals as slots in [`Topology::portals`], a range in
    /// [`Topology::links`].
    links: Result<Range<u32>, Bail>,
}

/// One structure's portal graph, decoded from the bytes the guest reads.
#[derive(Debug)]
pub struct Topology {
    key: TopologyKey,
    clusters: Vec<Cluster>,
    links: Column<u32>,
    portals: Vec<Result<Portal, Bail>>,
    ring: Column<[u32; 2]>,
    sources: Vec<(u32, u32)>,
}

#[derive(Default)]
struct PortalBuilder {
    links: Vec<u32>,
    portals: Vec<Result<Portal, Bail>>,
    ring: Vec<[u32; 2]>,
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn half(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}

/// Where a portal's records and planes are, and the image's projection rows.
struct Planes<'a> {
    portals: u32,
    distance: u32,
    normal: u32,
    projection: &'a [[i16; 3]; 6],
}

/// Reads that remember their spans, so a provider can watch them.
struct Reader<'a, G> {
    g: &'a G,
    sources: Vec<(u32, u32)>,
}

impl<G: GuestRead> Reader<'_, G> {
    fn bytes(&mut self, addr: u32, len: u32) -> Result<Vec<u8>, Bail> {
        let b = self.g.bytes(addr, len as usize)?;
        if len != 0 {
            self.sources.push((addr, len));
        }
        Ok(b)
    }

    fn u32(&mut self, addr: u32) -> Result<u32, Bail> {
        Ok(word(&self.bytes(addr, 4)?, 0))
    }
}

/// `0x00120850`: the axis of the largest normal component. Ties go to z,
/// then y (the guest compares `|z| < |y|`, then `|z| < |x|`, then
/// `|y| < |x|`).
pub fn dominant_axis(normal: [u32; 3]) -> usize {
    let a = |i: usize| f32::from_bits(normal[i] & 0x7fff_ffff);
    let (x, y, z) = (a(0), a(1), a(2));
    if z >= y && z >= x {
        2
    } else if y >= x {
        1
    } else {
        0
    }
}

impl Topology {
    /// Decode the graph behind `key`. `projection` is the image's table of
    /// projection rows (`2 * axis + positive`, the `u` and `v` axes first).
    ///
    /// The cluster count bounds the decode only: the guest never reads it.
    /// Clusters past [`MAX_CLUSTERS`] are not decoded; a flood that reaches
    /// one declines. A portal is decoded once however many clusters list it.
    pub fn decode(
        g: &impl GuestRead,
        key: TopologyKey,
        projection: &[[i16; 3]; 6],
    ) -> Result<Self, Bail> {
        use layout::*;
        let mut r = Reader {
            g,
            sources: Vec::new(),
        };
        let s = key.structure;
        let own_bsp = r.u32(s.wrapping_add(COLLISION_BSP))?;
        let portals_at = r.u32(s.wrapping_add(PORTALS))?;
        let head = r.bytes(s.wrapping_add(CLUSTER_COUNT), 8)?;
        let (count, clusters_at) = (word(&head, 0), word(&head, 4));
        let distance_planes = r.u32(own_bsp.wrapping_add(BSP_PLANES))?;
        let normal_planes = r.u32(key.bsp.wrapping_add(BSP_PLANES))?;
        let n = count.min(MAX_CLUSTERS);
        let block = r.bytes(clusters_at, n * CLUSTER_STRIDE)?;
        let mut builder = PortalBuilder::default();
        let mut clusters = Vec::with_capacity(n as usize);
        let mut slots = HashMap::<u16, u32>::new();
        let planes = Planes {
            portals: portals_at,
            distance: distance_planes,
            normal: normal_planes,
            projection,
        };
        for c in 0..n as usize {
            let at = c * CLUSTER_STRIDE as usize + CLUSTER_PORTAL_COUNT as usize;
            let (count, list) = (word(&block, at), word(&block, at + 4));
            let links = builder.cluster_links(&mut r, &mut slots, &planes, count, list);
            clusters.push(Cluster { count, links });
        }
        Ok(Self {
            key,
            clusters,
            links: builder.links.into(),
            portals: builder.portals,
            ring: builder.ring.into(),
            sources: r.sources,
        })
    }

    pub(crate) fn share_columns(&mut self, dir: &std::path::Path) {
        self.links.share(dir);
        self.ring.share(dir);
    }
}

impl PortalBuilder {

    /// One cluster's portal list, decoding each portal the first time any
    /// cluster names it.
    fn cluster_links<G: GuestRead>(
        &mut self,
        r: &mut Reader<'_, G>,
        slots: &mut HashMap<u16, u32>,
        planes: &Planes<'_>,
        count: u32,
        list: u32,
    ) -> Result<Range<u32>, Bail> {
        let k = count as i32;
        if k <= 0 {
            return Ok(0..0);
        }
        if k as u32 > MAX_PORTALS {
            return Err(Bail::Data);
        }
        let indices = r.bytes(list, 2 * k as u32)?;
        let start = self.links.len() as u32;
        for i in 0..k as usize {
            let index = half(&indices, 2 * i);
            let slot = match slots.get(&index) {
                Some(&slot) => slot,
                None => {
                    let at = planes.portals.wrapping_add(
                        (index as i16 as i32 as u32).wrapping_mul(layout::PORTAL_STRIDE),
                    );
                    let portal = self.portal(r, at, planes);
                    self.portals.push(portal);
                    let slot = self.portals.len() as u32 - 1;
                    slots.insert(index, slot);
                    slot
                }
            };
            self.links.push(slot);
        }
        Ok(start..self.links.len() as u32)
    }

    fn portal<G: GuestRead>(
        &mut self,
        r: &mut Reader<'_, G>,
        at: u32,
        planes: &Planes<'_>,
    ) -> Result<Portal, Bail> {
        let p = r.bytes(at, layout::PORTAL_STRIDE)?;
        let clusters = [half(&p, 0), half(&p, 2)];
        let test = self.test(r, &p, planes);
        Ok(Portal { clusters, test })
    }

    fn test<G: GuestRead>(
        &mut self,
        r: &mut Reader<'_, G>,
        p: &[u8],
        planes: &Planes<'_>,
    ) -> Result<Test, Bail> {
        use layout::*;
        let plane_offset = word(p, PORTAL_PLANE as usize).wrapping_shl(4);
        let plane = r.bytes(planes.distance.wrapping_add(plane_offset), 16)?;
        let plane: [u32; 4] = std::array::from_fn(|k| word(&plane, 4 * k));
        let sphere: [u32; 4] = std::array::from_fn(|k| word(p, PORTAL_SPHERE as usize + 4 * k));
        let projection = self.projection(r, p, planes, plane_offset);
        Ok(Test {
            plane,
            sphere,
            projection,
        })
    }

    fn projection<G: GuestRead>(
        &mut self,
        r: &mut Reader<'_, G>,
        p: &[u8],
        planes: &Planes<'_>,
        plane_offset: u32,
    ) -> Result<Projection, Bail> {
        use layout::*;
        let normal = r.bytes(planes.normal.wrapping_add(plane_offset), 12)?;
        let normal: [u32; 3] = std::array::from_fn(|k| word(&normal, 4 * k));
        let crate::bsp::Pick {
            axis,
            positive,
            flags: pick_flags,
        } = crate::bsp::pick(normal, 0);
        let row = planes.projection[2 * axis + usize::from(positive)];
        let axes = [row[0], row[1]];
        if axes.iter().any(|&a| !(0..3).contains(&a)) {
            return Err(Bail::Data);
        }
        let axes = axes.map(|a| a as u8);
        let count = word(p, PORTAL_VERTEX_COUNT as usize);
        let sides = count as u16 as i16;
        let start = self.ring.len() as u32;
        if count as i32 <= 0 {
            // Nothing is copied, but the polygon test reads the low half:
            // a positive one would walk stale frame words.
            if sides > 0 {
                return Err(Bail::Data);
            }
        } else {
            if count > MAX_SIDES {
                return Err(Bail::Data);
            }
            let vertices = r.bytes(word(p, PORTAL_VERTEX_COUNT as usize + 4), 12 * count)?;
            for k in 0..count as usize {
                let v = |a: u8| word(&vertices, 12 * k + 4 * a as usize);
                self.ring.push([v(axes[0]), v(axes[1])]);
            }
        }
        Ok(Projection {
            normal,
            axes,
            ring: start..self.ring.len() as u32,
            sides,
            pick_flags,
        })
    }

}

impl Topology {
    pub fn key(&self) -> TopologyKey {
        self.key
    }

    /// Every guest span the decode read, for a provider's write watch.
    pub fn sources(&self) -> &[(u32, u32)] {
        &self.sources
    }

    /// Clusters decoded (at most [`MAX_CLUSTERS`]).
    pub fn clusters(&self) -> usize {
        self.clusters.len()
    }

    /// The portals cluster `c` lists, in guest order.
    pub fn portals_of(&self, c: u16) -> Result<impl Iterator<Item = &Portal>, Bail> {
        let range = self.links_of(c)?;
        Ok(self.links[range.start as usize..range.end as usize]
            .iter()
            .map(|&slot| self.portals[slot as usize].as_ref().expect("listed portals decode")))
    }

    fn links_of(&self, c: u16) -> Result<Range<u32>, Bail> {
        let cluster = self.cluster(c)?;
        let range = cluster.links.clone()?;
        // The guest reads every listed portal's record once it enters the
        // cluster, so one that could not be read declines here.
        for &slot in &self.links[range.start as usize..range.end as usize] {
            if let Err(why) = &self.portals[slot as usize] {
                return Err(*why);
            }
        }
        Ok(range)
    }

    fn cluster(&self, c: u16) -> Result<&Cluster, Bail> {
        let i = c as i16;
        if i < 0 {
            return Err(Bail::Data);
        }
        self.clusters.get(i as usize).ok_or(Bail::Data)
    }

    pub fn ring(&self, t: &Projection) -> &[[u32; 2]] {
        &self.ring[t.ring.start as usize..t.ring.end as usize]
    }

    /// Approximate resident size of the decoded arrays.
    pub fn footprint(&self) -> usize {
        std::mem::size_of_val(&*self.clusters)
            + std::mem::size_of_val(&*self.links)
            + std::mem::size_of_val(&*self.portals)
            + std::mem::size_of_val(&*self.ring)
            + std::mem::size_of_val(&*self.sources)
    }
}

/// One flood, as the guest's arguments and live words give it.
#[derive(Clone, Copy, Debug)]
pub struct FloodQuery {
    /// The first cluster (the argument's low half).
    pub cluster: u16,
    /// Sphere centre and radius bits.
    pub centre: [u32; 3],
    pub radius: u32,
    /// The output capacity argument (a whole word; the guest tests its low
    /// half per cluster). Values past `0x7fff` decline.
    pub capacity: u32,
    /// The x87 control word the guest runs the plane distance under.
    pub control: u16,
    /// The cluster mark array and the epoch the caller set.
    pub marks: u32,
    pub epoch: u32,
}

/// What a flood leaves behind, decided before any of it is written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Flood {
    /// Every cluster visited, in visit order. Each gets the epoch as its
    /// mark; `clusters[..written]` go to the output array in that order.
    pub clusters: Vec<u16>,
    pub written: usize,
    /// The deepest nesting (0 is the first cluster) at which a portal test
    /// ran, for the dead stack the original uses; `None` when none ran.
    pub deepest: Option<u32>,
    /// The first cluster's portal count word. Its sign selects the last
    /// flag-producing instruction.
    pub portal_count: u32,
}

impl Flood {
    /// The count the guest returns (it counts clusters past a full output).
    pub fn count(&self) -> u16 {
        self.clusters.len() as u16
    }
}

struct Frame {
    cluster: u16,
    at: u32,
    end: u32,
    depth: u32,
}

impl Topology {
    /// Entry G inside a checked region the caller opened.
    pub fn flood(&self, g: &impl GuestRead, r: &Checked, q: &FloodQuery) -> Result<Flood, Bail> {
        if q.capacity > 0x7fff {
            return Err(Bail::Data);
        }
        let mut seen = vec![false; self.clusters.len()];
        let marked = |c: u16, seen: &[bool]| -> Result<bool, Bail> {
            Ok(seen[c as usize] || g.u32(q.marks.wrapping_add(4 * u32::from(c)))? == q.epoch)
        };
        let top = self.cluster(q.cluster)?;
        let range = self.links_of(q.cluster)?;
        let mut out = Flood {
            portal_count: top.count,
            ..Flood::default()
        };
        seen[q.cluster as usize] = true;
        out.clusters.push(q.cluster);
        let mut stack = vec![Frame {
            cluster: q.cluster,
            at: range.start,
            end: range.end,
            depth: 0,
        }];
        while let Some(f) = stack.last_mut() {
            if f.at == f.end {
                stack.pop();
                continue;
            }
            let slot = self.links[f.at as usize];
            f.at += 1;
            let (from, depth) = (f.cluster, f.depth);
            let portal = self.portals[slot as usize].as_ref().map_err(|w| *w)?;
            let [a, b] = portal.clusters;
            let other = if a == from { b } else { a };
            self.cluster(other)?;
            if marked(other, &seen)? {
                continue;
            }
            let test = portal.test.as_ref().map_err(|w| *w)?;
            out.deepest = Some(out.deepest.map_or(depth, |d| d.max(depth)));
            if !self.portal_test(r, q, test)? {
                continue;
            }
            let range = self.links_of(other)?;
            seen[other as usize] = true;
            out.clusters.push(other);
            stack.push(Frame {
                cluster: other,
                at: range.start,
                end: range.end,
                depth: depth + 1,
            });
        }
        out.written = out.clusters.len().min(q.capacity as usize);
        Ok(out)
    }

    /// `0x0014a370`: whether the sphere reaches through the portal.
    fn portal_test(&self, r: &Checked, q: &FloodQuery, t: &Test) -> Result<bool, Bail> {
        let ld = |x: u32| Chain::load(r, q.control, f32::from_bits(x)).ok_or(Bail::Float);
        let [nx, ny, nz, d] = t.plane;
        let [cx, cy, cz] = q.centre;
        // ((ny*cy + nz*cz) + cx*nx) - d on the x87, stored once for the
        // projection; the radius compares against the unrounded value.
        let dist = (ld(ny)? * ld(cy)? + ld(nz)? * ld(cz)?) + ld(cx)? * ld(nx)?;
        let dist = dist - ld(d)?;
        let stored = dist.store();
        let radius = ld(q.radius)?;
        // `fabs; fld radius; fcomip; jbe`: on unless the radius is above
        // the distance, ordered.
        if radius.compare(dist.abs()) != Some(Ordering::Greater) {
            return Ok(false);
        }
        // Bounding sphere, SSE: ((dz*dz + dx*dx) + dy*dy) < (r_p + r)^2.
        let f = f32::from_bits;
        let [sx, sy, sz, sr] = t.sphere.map(f);
        let dz = r.sub(sz, f(cz));
        let dx = r.sub(sx, f(cx));
        let dy = r.sub(sy, f(cy));
        let reach = r.add(sr, f(q.radius));
        let zz = r.mul(dz, dz);
        let xx = r.mul(dx, dx);
        let yy = r.mul(dy, dy);
        let apart = r.add(r.add(zz, xx), yy);
        // `comiss reach², apart; jbe`.
        if r.comiss(r.mul(reach, reach), apart, false) != Some(Ordering::Greater) {
            return Ok(false);
        }
        // The centre dropped onto the plane along the projection normal.
        let t = t.projection.as_ref().map_err(|w| *w)?;
        r.raise(t.pick_flags.0, t.pick_flags.1);
        let back = r.sub(0.0, stored);
        let point: [f32; 3] =
            std::array::from_fn(|k| r.add(r.mul(f(t.normal[k]), back), f(q.centre[k])));
        let point = [point[t.axes[0] as usize], point[t.axes[1] as usize]];
        // sqrt(r*r - d*d) on the x87, stored single.
        let stored = stored.to_bits();
        let across = (radius * ld(q.radius)? - ld(stored)? * ld(stored)?).sqrt().store();
        Ok(self.polygon(r, t, point, across))
    }

    /// `0x0023a160`: false when the circle lies wholly outside some edge.
    fn polygon(&self, r: &Checked, t: &Projection, p: [f32; 2], radius: f32) -> bool {
        let rr = r.mul(radius, radius);
        if t.sides <= 0 {
            return true;
        }
        let ring = self.ring(t);
        let f = |v: [u32; 2]| v.map(f32::from_bits);
        for i in 0..ring.len() {
            let j = if i + 1 >= ring.len() { 0 } else { i + 1 };
            let ([ui, vi], [uj, vj]) = (f(ring[i]), f(ring[j]));
            let x0 = r.sub(p[0], ui);
            let y0 = r.sub(p[1], vi);
            let ex = r.sub(uj, ui);
            let ey = r.sub(vj, vi);
            let length = r.add(r.mul(ex, ex), r.mul(ey, ey));
            // `ucomiss length, 0; lahf; test ah, 44h; jnp`: skip only an
            // ordered zero.
            if r.comiss(length, 0.0, true) == Some(Ordering::Equal) {
                continue;
            }
            let cross = r.sub(r.mul(x0, ey), r.mul(y0, ex));
            // `comiss cross, 0; jbe`, then `comiss cross², length·r²; ja`.
            if r.comiss(cross, 0.0, false) == Some(Ordering::Greater)
                && r.comiss(r.mul(cross, cross), r.mul(length, rr), false) == Some(Ordering::Greater)
            {
                return false;
            }
        }
        true
    }

    /// Entry G on its own: one checked region around the flood.
    pub fn cluster_flood(&self, g: &impl GuestRead, q: &FloodQuery) -> Result<Flood, Bail> {
        sim_core::det::checked(|r| Some(self.flood(g, r, q))).unwrap_or(Err(Bail::Float))
    }

    /// [`Topology::cluster_flood`] under the guest's `mxcsr` and
    /// [`FloodQuery::control`] for any operand, and the flags it raised.
    /// Only an unmasked exception or a host without the region's controls
    /// declines ([`Bail::Env`]).
    pub fn cluster_flood_guest(
        &self,
        g: &impl GuestRead,
        q: &FloodQuery,
        mxcsr: u32,
    ) -> Result<(Flood, Raised), Bail> {
        sim_core::det::checked_guest(mxcsr, q.control, |r| Some(self.flood(g, r, q).map(|f| (f, Raised::of(r)))))
            .unwrap_or(Err(Bail::Env))
    }
}

#[cfg(test)]
mod tests;
