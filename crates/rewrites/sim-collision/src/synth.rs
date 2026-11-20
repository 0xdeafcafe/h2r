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
