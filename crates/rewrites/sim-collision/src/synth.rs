//! Synthetic collision BSPs in the guest's layout, and a flat memory to
//! hold them: for the unit tests and the engine's differential test. Every
//! value is made up here; nothing comes from a disc.
use crate::bsp::{Consts, HEADER_BYTES};
use crate::mem::{Fault, GuestRead};

/// Flat guest memory from `base`.
#[derive(Clone, Debug)]
pub struct Image {
    pub base: u32,
    pub bytes: Vec<u8>,
}

impl Image {
    pub fn new(base: u32, len: usize) -> Self {
        Self {
            base,
            bytes: vec![0; len],
        }
    }

    pub fn put(&mut self, addr: u32, data: &[u8]) {
        let at = (addr - self.base) as usize;
        self.bytes[at..at + data.len()].copy_from_slice(data);
    }

    pub fn word(&mut self, addr: u32, v: u32) {
        self.put(addr, &v.to_le_bytes());
    }

    pub fn float(&mut self, addr: u32, v: f32) {
        self.word(addr, v.to_bits());
    }
}

impl GuestRead for Image {
    fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), Fault> {
        let at = addr
            .checked_sub(self.base)
            .map(|a| a as usize)
            .filter(|&a| a + dst.len() <= self.bytes.len())
            .ok_or(Fault { addr })?;
        dst.copy_from_slice(&self.bytes[at..at + dst.len()]);
        Ok(())
    }
}

/// A made-up projection table: for axis `a`, row `2a` projects onto the
/// other two axes in one order and row `2a + 1` in the other.
pub const CONSTS: Consts = Consts {
    zero: 0,
    projection: [
        [1, 2, 0],
        [2, 1, 0],
        [2, 0, 0],
        [0, 2, 0],
        [0, 1, 0],
        [1, 0, 0],
    ],
};

/// A child word: an internal node.
pub fn node(i: u32) -> u32 {
    i
}

/// A child word: leaf `i`.
pub fn leaf(i: u32) -> u32 {
    0x80_0000 | i
}

/// A child word: no leaf (solid).
pub const SOLID: u32 = 0xff_ffff;

/// A 2D child word: surface `s`.
pub fn surface(s: u16) -> u16 {
    0x8000 | s
}

/// A collision BSP's records, written in guest layout by [`Builder::write`].
#[derive(Clone, Debug, Default)]
pub struct Builder {
    /// `plane, back, front`, children as 24-bit words.
    pub nodes: Vec<(u16, u32, u32)>,
    pub planes: Vec<[f32; 4]>,
    /// `flags, ref_count, first_ref`.
    pub leaves: Vec<(u8, u8, u16)>,
    /// `plane (bit 15 flips), 2D root`.
    pub refs: Vec<(u16, u16)>,
    /// `a, b, d`, then back and front.
    pub nodes_2d: Vec<([f32; 3], [u16; 2])>,
    /// `plane, first_edge, flags, breakable, material`.
    pub surfaces: Vec<(u16, u16, u8, u8, u16)>,
    /// `start, end, forward, reverse, left, right`.
    pub edges: Vec<[u16; 6]>,
    pub vertices: Vec<[f32; 3]>,
}

fn align(x: u32) -> u32 {
    (x + 15) & !15
}

impl Builder {
    /// A ring for surface `s` through `points`, in order. `reversed`
    /// stores each edge backwards with `s` as its right surface, so the
    /// guest's walk takes the end vertex and the reverse link.
    pub fn ring(&mut self, s: u16, points: &[[f32; 3]], reversed: bool) -> u16 {
        let v0 = self.vertices.len() as u16;
        self.vertices.extend_from_slice(points);
        let e0 = self.edges.len() as u16;
        let n = points.len() as u16;
        for i in 0..n {
            let (a, b) = (v0 + i, v0 + (i + 1) % n);
            let next = e0 + (i + 1) % n;
            self.edges.push(if reversed {
                [b, a, 0, next, 0xffff, s]
            } else {
                [a, b, next, 0, s, 0xffff]
            });
        }
        e0
    }

    /// Bytes the BSP needs at [`Builder::write`].
    pub fn size(&self) -> u32 {
        HEADER_BYTES + 16 * 8 + self.blocks().iter().map(|b| b.len() as u32).sum::<u32>()
    }

    fn blocks(&self) -> [Vec<u8>; 8] {
        let mut b: [Vec<u8>; 8] = Default::default();
        for &(plane, back, front) in &self.nodes {
            b[0].extend_from_slice(&plane.to_le_bytes());
            b[0].extend_from_slice(&back.to_le_bytes()[..3]);
            b[0].extend_from_slice(&front.to_le_bytes()[..3]);
        }
        for p in &self.planes {
            p.iter()
                .for_each(|v| b[1].extend_from_slice(&v.to_le_bytes()));
        }
        for &(flags, count, first) in &self.leaves {
            b[2].extend_from_slice(&[flags, count]);
            b[2].extend_from_slice(&first.to_le_bytes());
        }
        for &(plane, root) in &self.refs {
            b[3].extend_from_slice(&plane.to_le_bytes());
            b[3].extend_from_slice(&root.to_le_bytes());
        }
        for (abd, child) in &self.nodes_2d {
            abd.iter()
                .for_each(|v| b[4].extend_from_slice(&v.to_le_bytes()));
            child
                .iter()
                .for_each(|c| b[4].extend_from_slice(&c.to_le_bytes()));
        }
        for &(plane, edge, flags, breakable, material) in &self.surfaces {
            b[5].extend_from_slice(&plane.to_le_bytes());
            b[5].extend_from_slice(&edge.to_le_bytes());
            b[5].extend_from_slice(&[flags, breakable]);
            b[5].extend_from_slice(&material.to_le_bytes());
        }
        for e in &self.edges {
            e.iter()
                .for_each(|v| b[6].extend_from_slice(&v.to_le_bytes()));
        }
        for v in &self.vertices {
            v.iter()
                .for_each(|x| b[7].extend_from_slice(&x.to_le_bytes()));
            b[7].extend_from_slice(&0u32.to_le_bytes());
        }
        b
    }

    /// Write the header at `at` and the blocks after it, each 16-byte
    /// aligned, through `put`. Returns the end address.
    pub fn write(&self, at: u32, mut put: impl FnMut(u32, &[u8])) -> u32 {
        let counts = [
            self.nodes.len(),
            self.planes.len(),
            self.leaves.len(),
            self.refs.len(),
            self.nodes_2d.len(),
            self.surfaces.len(),
            self.edges.len(),
            self.vertices.len(),
        ];
        let mut next = align(at + HEADER_BYTES);
        for (i, data) in self.blocks().iter().enumerate() {
            put(at + 8 * i as u32, &(counts[i] as u32).to_le_bytes());
            put(at + 8 * i as u32 + 4, &next.to_le_bytes());
            put(next, data);
            next = align(next + data.len() as u32);
        }
        next
    }
}

/// A small xorshift generator for synthetic data.
#[derive(Clone, Debug)]
pub struct Rng(pub u64);

impl Rng {
    #[allow(clippy::should_implement_trait, reason = "the fixture RNG returns a word on every call")]
    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }

    pub fn below(&mut self, n: u32) -> u32 {
        self.next() % n.max(1)
    }

    pub fn chance(&mut self, one_in: u32) -> bool {
        self.below(one_in) == 0
    }

    /// A multiple of 1/8 in `[-r, r]`: small dyadic values, so ties (a
    /// point on a plane, a split exactly at a bound) come up often.
    pub fn eighths(&mut self, r: u32) -> f32 {
        (self.below(16 * r + 1) as i32 - 8 * r as i32) as f32 / 8.0
    }
}

/// A random BSP with every index in range: a tree of up to `depth`
/// levels over a handful of planes, leaves with references back onto the
/// planes above them, small 2D trees and rings of 3 to 8 edges.
pub fn random(rng: &mut Rng, depth: u32) -> Builder {
    let mut b = Builder::default();
    let planes = 3 + rng.below(5);
    for _ in 0..planes {
        let mut n = [0.0f32; 3];
        match rng.below(3) {
            0 => n[rng.below(3) as usize] = if rng.chance(2) { 1.0 } else { -1.0 },
            1 => {
                let a = rng.below(3) as usize;
                n[a] = 0.6;
                n[(a + 1) % 3] = if rng.chance(2) { 0.8 } else { -0.8 };
            }
            _ => n = [rng.eighths(1), rng.eighths(1), rng.eighths(1)],
        }
        b.planes.push([n[0], n[1], n[2], rng.eighths(4)]);
    }
    let surfaces = 2 + rng.below(5) as u16;
    for s in 0..surfaces {
        let k = 3 + rng.below(6) as usize;
        let scale = [1.0f32, 4.0, 32.0][rng.below(3) as usize];
        let (axis, third) = (rng.below(3) as usize, rng.eighths(4));
        let points: Vec<[f32; 3]> = (0..k)
            .map(|i| {
                // A convex polygon around the origin in one axis plane,
                // its corners on the eighth grid.
                let corners = [
                    (1.0, 0.0),
                    (0.5, 1.0),
                    (-0.5, 1.0),
                    (-1.0, 0.0),
                    (-0.5, -1.0),
                    (0.5, -1.0),
                    (0.75, -0.75),
                    (1.0, -0.25),
                ];
                let (x, y) = corners[i * corners.len() / k];
                let mut p = [0.0; 3];
                p[axis] = third;
                p[(axis + 1) % 3] = x * scale;
                p[(axis + 2) % 3] = y * scale;
                p
            })
            .collect();
        let edge = b.ring(s, &points, rng.chance(2));
        let flags = [0u8, 0, 2, 8, 0x0a, 1][rng.below(6) as usize];
        b.surfaces.push((
            rng.below(planes) as u16,
            edge,
            flags,
            rng.below(12) as u8,
            rng.below(40) as u16,
        ));
    }
    let n2d = 1 + rng.below(5) as u16;
    for i in 0..n2d {
        let mut child = || {
            if i + 1 < n2d && rng.chance(2) {
                i + 1 + rng.below(u32::from(n2d - i - 1)) as u16
            } else if rng.chance(12) {
                0xffff
            } else {
                surface(rng.below(u32::from(surfaces)) as u16)
            }
        };
        let c = [child(), child()];
        b.nodes_2d
            .push(([rng.eighths(1), rng.eighths(1), rng.eighths(8)], c));
    }

    // The tree, depth first; leaves remember the planes above them.
    fn grow(b: &mut Builder, rng: &mut Rng, depth: u32, path: &mut Vec<u16>, planes: u32) -> u32 {
        if depth == 0 || (path.len() > 1 && rng.chance(4)) {
            if rng.chance(4) {
                return SOLID;
            }
            let first = b.refs.len() as u16;
            let count = rng.below(4) as u8;
            for _ in 0..count {
                let plane = if path.is_empty() || rng.chance(5) {
                    rng.below(planes) as u16
                } else {
                    path[rng.below(path.len() as u32) as usize]
                };
                let flip = if rng.chance(2) { 0x8000 } else { 0 };
                let root = if rng.chance(3) {
                    surface(rng.below(b.surfaces.len() as u32) as u16)
                } else {
                    rng.below(b.nodes_2d.len() as u32) as u16
                };
                b.refs.push((plane | flip, root));
            }
            let flags = if rng.chance(3) { 1 } else { 0 };
            b.leaves.push((
                flags,
                count,
                if count == 0 && rng.chance(2) {
                    0xffff
                } else {
                    first
                },
            ));
            return leaf(b.leaves.len() as u32 - 1);
        }
        let index = b.nodes.len();
        let plane = rng.below(planes) as u16;
        b.nodes.push((plane, 0, 0));
        path.push(plane);
        let back = grow(b, rng, depth - 1, path, planes);
        let front = grow(b, rng, depth - 1, path, planes);
        path.pop();
        b.nodes[index].1 = back;
        b.nodes[index].2 = front;
        node(index as u32)
    }
    let root = grow(&mut b, rng, depth.max(1), &mut Vec::new(), planes);
    if root & 0x80_0000 != 0 {
        // Keep node 0 a node: one split with the leaf on both sides.
        b.nodes.push((0, root, root));
    }
    b
}

/// One portal of a synthetic structure: the clusters either side, the
/// index of its plane, a bounding sphere and its polygon.
#[derive(Clone, Debug, Default)]
pub struct PortalSpec {
    pub clusters: [i16; 2],
    pub plane: u32,
    pub sphere: [f32; 4],
    pub vertices: Vec<[f32; 3]>,
    /// Overrides the vertex count word (to test counts the guest cannot use).
    pub count: Option<u32>,
}

/// A structure's portal graph in the guest's layout: a structure tag with
/// its collision BSP pointer, portal block and cluster block; the BSP's
/// plane block; each cluster's portal list; each portal's vertices.
#[derive(Clone, Debug, Default)]
pub struct Portals {
    /// Each cluster's portal indices, in list order.
    pub clusters: Vec<Vec<i16>>,
    pub portals: Vec<PortalSpec>,
    /// The structure collision BSP's planes, `nx, ny, nz, d`.
    pub planes: Vec<[f32; 4]>,
}

/// Bytes [`Portals::write`] gives the structure tag, then the BSP header.
pub const STRUCTURE_BYTES: u32 = 0x100;

impl Portals {
    /// Write the structure tag at `at`, its collision BSP header after it,
    /// then every block, through `put`. Returns the end address and the
    /// BSP header's address.
    pub fn write(&self, at: u32, mut put: impl FnMut(u32, &[u8])) -> (u32, u32) {
        use crate::flood::layout::*;
        let bsp = at + STRUCTURE_BYTES;
        put(at, &[0; STRUCTURE_BYTES as usize]);
        put(bsp, &[0; HEADER_BYTES as usize]);
        let mut next = align(bsp + HEADER_BYTES);
        let mut block = |data: &[u8], put: &mut dyn FnMut(u32, &[u8])| {
            let start = next;
            put(start, data);
            next = align(next + data.len() as u32);
            start
        };
        let planes: Vec<u8> = self
            .planes
            .iter()
            .flat_map(|p| p.iter().flat_map(|v| v.to_le_bytes()))
            .collect();
        let planes_at = block(&planes, &mut put);
        put(bsp + 8, &(self.planes.len() as u32).to_le_bytes());
        put(bsp + BSP_PLANES, &planes_at.to_le_bytes());
        put(at + COLLISION_BSP, &bsp.to_le_bytes());
        let mut portals = Vec::new();
        for p in &self.portals {
            let vertices: Vec<u8> = p
                .vertices
                .iter()
                .flat_map(|v| v.iter().flat_map(|x| x.to_le_bytes()))
                .collect();
            let vertices_at = block(&vertices, &mut put);
            let mut r = Vec::with_capacity(PORTAL_STRIDE as usize);
            r.extend_from_slice(&p.clusters[0].to_le_bytes());
            r.extend_from_slice(&p.clusters[1].to_le_bytes());
            r.extend_from_slice(&p.plane.to_le_bytes());
            p.sphere
                .iter()
                .for_each(|v| r.extend_from_slice(&v.to_le_bytes()));
            r.extend_from_slice(&0u32.to_le_bytes());
            let count = p.count.unwrap_or(p.vertices.len() as u32);
            r.extend_from_slice(&count.to_le_bytes());
            r.extend_from_slice(&vertices_at.to_le_bytes());
            portals.extend_from_slice(&r);
        }
        let portals_at = block(&portals, &mut put);
        put(at + PORTALS - 4, &(self.portals.len() as u32).to_le_bytes());
        put(at + PORTALS, &portals_at.to_le_bytes());
        let mut clusters = vec![0u8; self.clusters.len() * CLUSTER_STRIDE as usize];
        for (c, list) in self.clusters.iter().enumerate() {
            let indices: Vec<u8> = list.iter().flat_map(|i| i.to_le_bytes()).collect();
            let list_at = block(&indices, &mut put);
            let o = c * CLUSTER_STRIDE as usize + CLUSTER_PORTAL_COUNT as usize;
            clusters[o..o + 4].copy_from_slice(&(list.len() as u32).to_le_bytes());
            clusters[o + 4..o + 8].copy_from_slice(&list_at.to_le_bytes());
        }
        let clusters_at = block(&clusters, &mut put);
        put(at + CLUSTER_COUNT, &(self.clusters.len() as u32).to_le_bytes());
        put(at + CLUSTERS, &clusters_at.to_le_bytes());
        (next, bsp)
    }

    /// Add a portal between `a` and `b` and list it in both clusters.
    pub fn link(&mut self, a: i16, b: i16, p: PortalSpec) -> i16 {
        let i = self.portals.len() as i16;
        self.portals.push(PortalSpec {
            clusters: [a, b],
            ..p
        });
        for c in [a, b] {
            if let Some(list) = usize::try_from(c).ok().and_then(|c| self.clusters.get_mut(c)) {
                list.push(i);
            }
        }
        i
    }
}

/// A square of half-size `h` around `(u, v)` in the plane `axis = at`.
pub fn square(axis: usize, at: f32, (u, v): (f32, f32), h: f32) -> Vec<[f32; 3]> {
    [(-h, -h), (h, -h), (h, h), (-h, h)]
        .iter()
        .map(|&(du, dv)| {
            let mut p = [0.0; 3];
            p[axis] = at;
            p[(axis + 1) % 3] = u + du;
            p[(axis + 2) % 3] = v + dv;
            p
        })
        .collect()
}

/// A random portal graph over `n` clusters: a chain of walls `x = k`
/// between clusters `k` and `k + 1`, plus random cross links with tilted
/// planes, polygons of 3 to 8 sides, spheres around them, and now and
/// then a portal neither of whose clusters names its lister.
pub fn random_portals(rng: &mut Rng, n: u32) -> Portals {
    let mut s = Portals {
        clusters: vec![Vec::new(); n as usize],
        ..Portals::default()
    };
    fn plane(s: &mut Portals, rng: &mut Rng, k: f32) -> (u32, usize) {
        let (normal, axis) = match rng.below(4) {
            0 | 1 => ([if rng.chance(2) { 1.0 } else { -1.0 }, 0.0, 0.0], 0),
            2 => ([0.6, if rng.chance(2) { 0.8 } else { -0.8 }, 0.0], 1),
            _ => {
                let a = rng.below(3) as usize;
                let mut n = [0.0f32; 3];
                n[a] = if rng.chance(2) { 1.0 } else { -1.0 };
                (n, a)
            }
        };
        let d = if axis == 0 { k * normal[0] } else { rng.eighths(4) };
        s.planes.push([normal[0], normal[1], normal[2], d]);
        (s.planes.len() as u32 - 1, axis)
    }
    for k in 0..n.saturating_sub(1) {
        let (p, axis) = plane(&mut s, rng, k as f32 + 0.5);
        let at = s.planes[p as usize][3] * s.planes[p as usize][axis];
        let h = [0.5f32, 1.0, 2.0, 4.0][rng.below(4) as usize];
        let mut vertices = square(axis, at, (rng.eighths(2), rng.eighths(2)), h);
        if rng.chance(3) {
            vertices.pop();
        }
        if rng.chance(4) {
            vertices.reverse();
        }
        let mut sphere = vertices.iter().fold([0.0f32; 3], |c, v| {
            [c[0] + v[0] / 4.0, c[1] + v[1] / 4.0, c[2] + v[2] / 4.0]
        });
        sphere[0] = at;
        let r = h * [1.5f32, 0.25, 3.0][rng.below(3) as usize];
        s.link(
            k as i16,
            k as i16 + 1,
            PortalSpec {
                plane: p,
                sphere: [sphere[0], sphere[1], sphere[2], r],
                vertices,
                ..PortalSpec::default()
            },
        );
    }
    for _ in 0..rng.below(n) {
        let (a, b) = (rng.below(n) as i16, rng.below(n) as i16);
        let k = rng.eighths(8);
        let (p, axis) = plane(&mut s, rng, k);
        let at = s.planes[p as usize][3] * s.planes[p as usize][axis];
        let vertices = square(axis, at, (rng.eighths(4), rng.eighths(4)), 2.0);
        let mut spec = PortalSpec {
            plane: p,
            sphere: [rng.eighths(8), rng.eighths(8), rng.eighths(8), 6.0],
            vertices,
            ..PortalSpec::default()
        };
        if rng.chance(16) {
            spec.count = Some(0);
        }
        let i = s.link(a, b, spec);
        if rng.chance(10) {
            // Listed by a third cluster: its far side is the portal's first.
            let c = rng.below(n) as usize;
            s.clusters[c].push(i);
        }
    }
    s
}

/// One object reference in a synthetic cluster list.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reference {
    pub handle: u32,
    pub kind: u8,
    /// Centre and radius.
    pub bounds: [f32; 4],
}

/// A per-cluster reference list in the guest's layout: a head table of
/// datum words, and a datum array header whose records carry the handle,
/// the next datum, the type byte and the bounding sphere.
#[derive(Clone, Debug, Default)]
pub struct References {
    pub clusters: Vec<Vec<Reference>>,
}

impl References {
    /// Record size [`References::write`] gives the datum array.
    pub const SIZE: u32 = 0x24;

    /// Write the head table at `heads`, the datum array header at `array`
    /// and the records at `data`. Datum words carry a salt in the high half.
    pub fn write(&self, heads: u32, array: u32, data: u32, mut put: impl FnMut(u32, &[u8])) {
        use crate::objects::layout::*;
        let mut index = 0u32;
        put(array, &[0; 0x48]);
        put(array + ELEMENT_SIZE, &Self::SIZE.to_le_bytes());
        put(array + DATA, &data.to_le_bytes());
        for (c, list) in self.clusters.iter().enumerate() {
            let datum = |i: u32| (0x5a00 + i) << 16 | i;
            let head = if list.is_empty() { u32::MAX } else { datum(index) };
            put(heads + 4 * c as u32, &head.to_le_bytes());
            for (k, r) in list.iter().enumerate() {
                let i = index + k as u32;
                let next = if k + 1 == list.len() { u32::MAX } else { datum(i + 1) };
                let mut b = vec![0u8; Self::SIZE as usize];
                b[HANDLE as usize..][..4].copy_from_slice(&r.handle.to_le_bytes());
                b[NEXT as usize..][..4].copy_from_slice(&next.to_le_bytes());
                b[BOUNDS as usize] = r.kind;
                for (j, v) in r.bounds.iter().enumerate() {
                    b[BOUNDS as usize + 4 + 4 * j..][..4].copy_from_slice(&v.to_le_bytes());
                }
                put(data + i * Self::SIZE, &b);
            }
            index += list.len() as u32;
        }
    }
}
