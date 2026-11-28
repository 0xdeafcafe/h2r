//! A collision BSP decoded from guest memory (docs/formats/collision.md,
//! "BSP element"; the queries are docs/engine/collision-queries.md).
//!
//! The engine names a BSP by a pointer to its eight tag blocks, each a
//! `{count, pointer}` pair: nodes, planes, leaves, references, 2D nodes,
//! surfaces, edges, vertices. [`Bsp::decode`] reads them once into flat
//! arrays in guest numbering (leaf, surface, plane and material indices
//! are guest visible and never renumbered):
//!
//! - a node carries its plane inline, so a visit reads one record;
//! - a reference carries its projection row, fixed by its plane's
//!   dominant axis and its flip bit;
//! - a surface carries its edge ring as `(A, B)` vertex pairs in ring order.
//!   Short rings can also hold packed origins and deltas for the inside test.
//!
//! The guest reads none of the counts; the decoder uses them to bound
//! every index. A record the guest would read out of range, a plane
//! that is not ordinary, or a ring that does not close is marked, and a
//! query that reaches it bails to the lifted code, which then reads what
//! retail reads.
use crate::Bail;
use crate::mem::GuestRead;
use sim_core::det::Fp;
use sim_core::lanes::compared1;
use std::cmp::Ordering;
use std::sync::RwLock;

pub mod point;
mod ring;
pub mod segment;
mod store;
pub use crate::store::Column;

/// A child word with this bit set is a leaf (`0xffff_ffff`: none, solid).
pub const LEAF: u32 = 0x0080_0000;
pub const NONE: u32 = u32::MAX;
/// Segment recursion the kernel follows before it bails; the point walk
/// takes at most this many steps.
pub const MAX_DEPTH: u32 = 256;
/// Edges a ring may have before the decoder leaves it to the guest
/// (retail rings have at most 8).
pub const MAX_RING: usize = 64;
/// Records per block, and source bytes in all, the decoder accepts.
pub const MAX_COUNT: u32 = 1 << 22;
pub const MAX_BYTES: u64 = 128 << 20;

/// The block header: eight `{count, pointer}` pairs.
pub const HEADER_WORDS: usize = 16;
pub const HEADER_BYTES: u32 = 4 * HEADER_WORDS as u32;

/// The blocks, in header order, with their record sizes.
pub mod block {
    pub const NODES: usize = 0;
    pub const PLANES: usize = 1;
    pub const LEAVES: usize = 2;
    pub const REFS: usize = 3;
    pub const NODES_2D: usize = 4;
    pub const SURFACES: usize = 5;
    pub const EDGES: usize = 6;
    pub const VERTICES: usize = 7;
    pub const STRIDE: [u32; 8] = [8, 16, 4, 4, 16, 8, 12, 16];
}

/// [`Node::bad`] bits: the plane index, or a child that is an internal
/// node index, is out of range.
pub const BAD_PLANE: u8 = 1;
pub const BAD_CHILD: [u8; 2] = [2, 4];
/// A projection row or axis the decoder could not fix.
pub const BAD: u8 = 0xff;

/// An internal node: `u16 plane`, then the back and front children as
/// 24-bit signed words.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Zeroable, bytemuck::Pod)]
#[repr(C, align(32))]
pub struct Node {
    /// The plane's `nx, ny, nz, d` bits.
    pub plane: [u32; 4],
    /// Back and front, sign-extended from 24 bits as the guest's `sar`.
    pub child: [u32; 2],
    /// The raw plane index (the guest sign-extends it).
    pub plane_index: u16,
    pub bad: u8,
    reserved: [u8; 5],
}

const _: () = assert!(std::mem::size_of::<Node>() == 32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Zeroable, bytemuck::Pod)]
#[repr(C)]
pub struct Leaf {
    pub flags: u8,
    pub ref_count: u8,
    pub first_ref: u16,
}

/// A 2D reference: the plane it lies on (bit 15 flips it), the root of its
/// 2D tree, and the projection row the matcher uses for it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Zeroable, bytemuck::Pod)]
#[repr(C)]
pub struct Ref {
    pub plane: u16,
    pub node: u16,
    /// `2 * axis + sign`, an index into [`Bsp::projection`]; [`BAD`] when
    /// the plane is out of range or not ordinary.
    pub row: u8,
    reserved: u8,
}

/// A 2D node: `a * u + b * v - d`, then the back and front children
/// (bit 15 set: a surface, `0xffff` none).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Zeroable, bytemuck::Pod)]
#[repr(C)]
pub struct Node2d {
    pub a: u32,
    pub b: u32,
    pub d: u32,
    pub child: [u16; 2],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Zeroable, bytemuck::Pod)]
#[repr(C)]
pub struct Surface {
    pub plane: u16,
    pub first_edge: u16,
    pub flags: u8,
    pub breakable: u8,
    pub material: u16,
    /// The ring's first cached edge. [`NONE`] means no predecoded ring.
    /// The scalar path uses `first_edge` and validates closure at query time.
    pub ring: u32,
    pub ring_len: u32,
    /// First packed edge group, or `NONE` if this ring uses scalar tests.
    packed_ring: u32,
}

/// One edge of a surface ring as the inside test reads it: vertex A (the
/// end vertex when the edge's right surface is this one, else the start)
/// and vertex B (the other), each `x, y, z` bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, bytemuck::Zeroable, bytemuck::Pod)]
#[repr(C)]
pub struct RingEdge {
    pub a: [u32; 3],
    pub b: [u32; 3],
}

/// Image constants the matcher reads besides the BSP: the zero it compares
/// a normal against, and the projection table (three `i16` per row, the
/// `u` and `v` axes first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Consts {
    pub zero: u32,
    pub projection: [[i16; 3]; 6],
}

impl Consts {
    pub(crate) fn fingerprint(&self, data_hash: u64) -> u64 {
        let mut h = hash(data_hash, &self.zero.to_le_bytes());
        for word in self.projection.iter().flatten() {
            h = hash(h, &word.to_le_bytes());
        }
        h
    }
}

pub struct Bsp {
    addr: u32,
    header: [u32; HEADER_WORDS],
    mapped_overhead: usize,
    pub nodes: Column<Node>,
    pub planes: Column<[u32; 4]>,
    /// The dominant axis of each plane as the matcher picks it, [`BAD`]
    /// when a normal component is not ordinary.
    pub axis: Column<u8>,
    pub leaves: Column<Leaf>,
    pub refs: Column<Ref>,
    pub nodes_2d: Column<Node2d>,
    pub surfaces: Column<Surface>,
    /// Optional expanded edges. The world can release them even while a
    /// query holds an Arc to the essential arrays. Only edge arithmetic
    /// runs under the read lock.
    ring: RwLock<Option<ring::Rings>>,
    /// Per projection row, the `u` and `v` axes; [`BAD`] when the image's
    /// table names something else.
    pub projection: [[u8; 2]; 6],
    hash: u64,
    pub(crate) fingerprint: u64,
}

impl std::fmt::Debug for Bsp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Bsp")
            .field("addr", &format_args!("{:#010x}", self.addr))
            .field("nodes", &self.nodes.len())
            .field("planes", &self.planes.len())
            .field("leaves", &self.leaves.len())
            .field("surfaces", &self.surfaces.len())
            .field("hash", &format_args!("{:#018x}", self.hash))
            .finish()
    }
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn half(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}

/// FNV-1a, 64-bit: the source hash the cache checks compare.
pub(crate) fn hash(seed: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(seed, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

pub(crate) const HASH_SEED: u64 = 0xcbf2_9ce4_8422_2325;

fn abs(x: u32) -> f32 {
    f32::from_bits(x & 0x7fff_ffff)
}

/// The matcher's dominant axis (`0x1dea57`..`0x1deab8`): z unless
/// `|nz| < |ny|` or `|nz| < |nx|`, else y when `|ny| >= |nx|`, else x.
///
/// The row is decided once per BSP, so its compares (`fld`, `fcomip`,
/// `comiss`) must raise no flag: zero, normal and infinite components
/// raise none. A NaN or subnormal one would, so its row is `BAD`.
fn dominant_axis(n: &[u32; 4]) -> u8 {
    if !(compared1(n[0]) && compared1(n[1]) && compared1(n[2])) {
        return BAD;
    }
    // Compared values are never NaN, so `>=` is the guest's "not below".
    let (x, y, z) = (abs(n[0]), abs(n[1]), abs(n[2]));
    if z >= y && z >= x {
        2
    } else if y >= x {
        1
    } else {
        0
    }
}

impl Bsp {
    /// The eight `{count, pointer}` pairs at `addr`.
    pub fn read_header(g: &impl GuestRead, addr: u32) -> Result<[u32; HEADER_WORDS], Bail> {
        addr.checked_add(HEADER_BYTES - 1).ok_or(Bail::Fault)?;
        let mut b = [0; HEADER_BYTES as usize];
        g.read(addr, &mut b)?;
        Ok(std::array::from_fn(|i| word(&b, 4 * i)))
    }

    pub fn addr(&self) -> u32 {
        self.addr
    }

    pub fn header(&self) -> &[u32; HEADER_WORDS] {
        &self.header
    }

    /// The guest address of block `i`'s records.
    pub fn block_addr(&self, i: usize) -> u32 {
        self.header[2 * i + 1]
    }

    /// The guest address of plane `index`, as the result record stores it
    /// (`planes + (index << 4)`, wrapping like the guest's `shl`/`add`).
    pub fn plane_addr(&self, index: u32) -> u32 {
        self.block_addr(block::PLANES)
            .wrapping_add(index.wrapping_shl(4))
    }

    /// Every guest byte range the decode read: the header and the eight
    /// blocks.
    pub fn sources(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        std::iter::once((self.addr, HEADER_BYTES)).chain((0..8).map(|i| {
            (
                self.header[2 * i + 1],
                self.header[2 * i].wrapping_mul(block::STRIDE[i]),
            )
        }))
    }

    /// Whether `[addr, addr + len)` overlaps anything the decode read.
    pub fn overlaps(&self, addr: u32, len: u32) -> bool {
        self.sources()
            .any(|(a, n)| crate::mem::overlaps(addr, len, a, n))
    }

    /// The hash of every source byte at decode time.
    pub fn source_hash(&self) -> u64 {
        self.hash
    }

    /// Re-read every source range and compare its hash with the one taken
    /// at decode: false when guest memory under the BSP changed.
    pub fn verify(&self, g: &impl GuestRead) -> bool {
        let mut h = HASH_SEED;
        for (a, n) in self.sources() {
            match g.bytes(a, n as usize) {
                Ok(b) => h = hash(h, &b),
                Err(_) => return false,
            }
        }
        h == self.hash
    }

    /// Required decoded array bytes before allocating or reading block bodies.
    pub fn essential_bytes(header: &[u32; HEADER_WORDS]) -> Result<u64, Bail> {
        use std::mem::size_of;
        let sizes = [
            size_of::<Node>(),
            size_of::<[u32; 4]>() + 1,
            size_of::<Leaf>(),
            size_of::<Ref>(),
            size_of::<Node2d>(),
            size_of::<Surface>(),
            0,
            0,
        ];
        let mut raw = 0u64;
        let mut required = 0u64;
        for (i, size) in sizes.into_iter().enumerate() {
            let count = header[2 * i];
            raw += u64::from(count) * u64::from(block::STRIDE[i]);
            if count > MAX_COUNT || raw > MAX_BYTES {
                return Err(Bail::Data);
            }
            required += u64::from(count) * size as u64;
        }
        Ok(required)
    }

    pub(crate) fn scalar_rings(&self) -> bool {
        self.ring
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .is_none()
    }

    /// Release optional edges without cloning essential arrays or waiting
    /// for another query to drop its BSP.
    pub(crate) fn discard_rings(&self) -> usize {
        let old = self.ring.write().unwrap_or_else(|e| e.into_inner()).take();
        old.map_or(0, |ring| ring.footprint())
    }

    /// Bytes reserved by the decoded arrays, including spare vector capacity.
    pub fn footprint(&self) -> usize {
        use std::mem::size_of;
        self.mapped_overhead
            + self.nodes.capacity() * size_of::<Node>()
            + self.planes.capacity() * size_of::<[u32; 4]>()
            + self.axis.capacity()
            + self.leaves.capacity() * size_of::<Leaf>()
            + self.refs.capacity() * size_of::<Ref>()
            + self.nodes_2d.capacity() * size_of::<Node2d>()
            + self.surfaces.capacity() * size_of::<Surface>()
            + self
                .ring
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .map_or(0, ring::Rings::footprint)
    }

    /// Decode the BSP whose header (read at `addr`) is `header`.
    pub fn decode(
        g: &impl GuestRead,
        addr: u32,
        header: [u32; HEADER_WORDS],
        consts: &Consts,
    ) -> Result<Bsp, Bail> {
        Self::decode_with_ring_limit(g, addr, header, consts, crate::world::MAP_BUDGET)
    }

    pub(crate) fn decode_with_ring_limit(
        g: &impl GuestRead,
        addr: u32,
        header: [u32; HEADER_WORDS],
        consts: &Consts,
        ring_bytes: u64,
    ) -> Result<Bsp, Bail> {
        Self::decode_cached(g, addr, header, consts, ring_bytes, None)
    }

    pub(crate) fn decode_cached(
        g: &impl GuestRead,
        addr: u32,
        header: [u32; HEADER_WORDS],
        consts: &Consts,
        ring_bytes: u64,
        directory: Option<&std::path::Path>,
    ) -> Result<Bsp, Bail> {
        let essential = Self::essential_bytes(&header)?;
        let mut h = hash(
            HASH_SEED,
            &header
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>(),
        );
        let mut raw: [Vec<u8>; 8] = Default::default();
        for (i, r) in raw.iter_mut().enumerate() {
            let count = header[2 * i];
            *r = g.bytes(header[2 * i + 1], (count * block::STRIDE[i]) as usize)?;
            h = hash(h, r);
        }
        let key = directory
            .filter(|_| cfg!(target_endian = "little") && essential >= store::MIN_BYTES)
            .map(|dir| store::Key::new(dir, &raw, consts));
        if let Some(bsp) = key
            .as_ref()
            .and_then(|k| k.open(addr, header, consts, h, ring_bytes))
        {
            return Ok(bsp);
        }
        let n = |i: usize| raw[i].len() / block::STRIDE[i] as usize;
        let (n_nodes, n_planes) = (n(block::NODES), n(block::PLANES));
        let n_edges = n(block::EDGES);

        let planes: Vec<[u32; 4]> = raw[block::PLANES]
            .as_chunks::<16>()
            .0
            .iter()
            .map(|p| std::array::from_fn(|k| word(p, 4 * k)))
            .collect();
        let axis = planes.iter().map(dominant_axis).collect::<Vec<_>>();

        let nodes = raw[block::NODES]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|b| {
                let plane_index = half(b, 0);
                let p = plane_index as i16;
                let mut bad = 0;
                let plane = if p >= 0 && (p as usize) < n_planes {
                    planes[p as usize]
                } else {
                    bad |= BAD_PLANE;
                    [0; 4]
                };
                // The guest reads `i32 at +1 + 3 * side` and shifts it
                // right by 8: bytes 2..5 (back) and 5..8 (front).
                let child = [2usize, 5].map(|at| {
                    let w =
                        u32::from(b[at]) | u32::from(b[at + 1]) << 8 | u32::from(b[at + 2]) << 16;
                    ((w << 8) as i32 >> 8) as u32
                });
                for side in 0..2 {
                    let c = child[side];
                    if c & LEAF == 0 && c as usize >= n_nodes {
                        bad |= BAD_CHILD[side];
                    }
                }
                Node {
                    plane,
                    child,
                    plane_index,
                    bad,
                    reserved: [0; 5],
                }
            })
            .collect();

        let leaves = raw[block::LEAVES]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| Leaf {
                flags: b[0],
                ref_count: b[1],
                first_ref: half(b, 2),
            })
            .collect();

        let zero_ok = compared1(consts.zero);
        let refs = raw[block::REFS]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| {
                let plane = half(b, 0);
                let index = usize::from(plane & 0x7fff);
                let flip = plane >> 15 != 0;
                let row = match axis.get(index) {
                    Some(&a) if a != BAD && zero_ok => {
                        let n = f32::from_bits(planes[index][usize::from(a)]);
                        let sign = n > f32::from_bits(consts.zero);
                        2 * a + u8::from(sign != flip)
                    }
                    _ => BAD,
                };
                Ref {
                    plane,
                    node: half(b, 2),
                    row,
                    reserved: 0,
                }
            })
            .collect();

        let nodes_2d = raw[block::NODES_2D]
            .as_chunks::<16>()
            .0
            .iter()
            .map(|b| Node2d {
                a: word(b, 0),
                b: word(b, 4),
                d: word(b, 8),
                child: [half(b, 12), half(b, 14)],
            })
            .collect();

        let edges = &raw[block::EDGES];
        let vertices = &raw[block::VERTICES];
        let vertex = |i: u16| {
            let at = usize::from(i) * 16;
            let v = vertices.get(at..at + 12).ok_or(Bail::Data)?;
            Ok(std::array::from_fn(|k| word(v, 4 * k)))
        };
        let edge = |i: u16| {
            let at = usize::from(i) * 12;
            edges
                .get(at..at + 12)
                .and_then(|b| b.try_into().ok())
                .ok_or(Bail::Data)
        };
        let max_edges = (ring_bytes / std::mem::size_of::<RingEdge>() as u64) as usize;
        let mut ring = (max_edges != 0 || n(block::SURFACES) == 0).then(Vec::<RingEdge>::new);
        let mut scratch = [RingEdge::default(); MAX_RING];
        let mut surfaces: Vec<_> = raw[block::SURFACES]
            .as_chunks::<8>()
            .0
            .iter()
            .enumerate()
            .map(|(s, b)| {
                let first = half(b, 2);
                let mut start = NONE;
                let mut len = 0;
                if let Some(buffer) = ring.as_mut()
                    && let Ok(n) = read_ring(s as u32, first, n_edges, edge, vertex, &mut scratch)
                {
                    let needed = buffer.len() + n;
                    if needed <= max_edges {
                        if needed > buffer.capacity() {
                            let target = needed
                                .max(buffer.capacity().saturating_mul(2))
                                .min(max_edges);
                            buffer.reserve_exact(target - buffer.len());
                        }
                        start = buffer.len() as u32;
                        len = n as u32;
                        buffer.extend_from_slice(&scratch[..n]);
                    } else {
                        // One transition for the whole BSP, including the
                        // rings already built. Later surfaces skip expansion.
                        ring = None;
                    }
                }
                Surface {
                    plane: half(b, 0),
                    first_edge: first,
                    flags: b[4],
                    breakable: b[5],
                    material: half(b, 6),
                    ring: start,
                    ring_len: len,
                    packed_ring: NONE,
                }
            })
            .collect();
        let ring = ring.map(|edges| ring::Rings::new(edges, &mut surfaces, ring_bytes));

        let projection = consts.projection.map(projection_axes);

        let bsp = Bsp {
            mapped_overhead: 0,
            addr,
            header,
            nodes,
            planes: planes.into(),
            axis: axis.into(),
            leaves,
            refs,
            nodes_2d,
            surfaces: surfaces.into(),
            ring: RwLock::new(ring),
            projection,
            hash: h,
            fingerprint: consts.fingerprint(h),
        };
        if let Some(key) = key {
            let _ = key.publish(&bsp, ring_bytes);
            if let Some(mapped) = key.open(addr, header, consts, h, ring_bytes) {
                return Ok(mapped);
            }
        }
        Ok(bsp)
    }

    /// A copy of the cached surface edges. A budget-limited BSP reads its
    /// ring from guest memory through `with_surface_ring` during the matcher.
    pub fn surface_ring(&self, surface: &Surface) -> Result<Vec<RingEdge>, Bail> {
        let ring = self.ring.read().unwrap_or_else(|e| e.into_inner());
        let ring = ring.as_ref().ok_or(Bail::Budget)?;
        Ok(cached_ring(&ring.edges, surface)?.to_vec())
    }

    /// The edges of a numbered surface in either storage mode.
    pub fn read_surface_ring(&self, g: &impl GuestRead, index: u32) -> Result<Vec<RingEdge>, Bail> {
        let surface = self.surfaces.get(index as usize).ok_or(Bail::Data)?;
        self.with_surface_ring(g, index, surface, |edges, _| Ok(edges.to_vec()))
    }

    fn with_surface_ring<T>(
        &self,
        g: &impl GuestRead,
        index: u32,
        surface: &Surface,
        test: impl FnOnce(&[RingEdge], &[ring::EdgeGroup]) -> Result<T, Bail>,
    ) -> Result<T, Bail> {
        {
            let ring = self.ring.read().unwrap_or_else(|e| e.into_inner());
            if let Some(ring) = ring.as_ref() {
                return test(cached_ring(&ring.edges, surface)?, ring.groups(surface));
            }
        }
        let edge = |i: u16| {
            let mut row = [0; 12];
            g.read(
                self.block_addr(block::EDGES)
                    .wrapping_add(12 * u32::from(i)),
                &mut row,
            )?;
            Ok(row)
        };
        let vertex = |i: u16| {
            if u32::from(i) >= self.header[2 * block::VERTICES] {
                return Err(Bail::Data);
            }
            Ok(g.vec3(
                self.block_addr(block::VERTICES)
                    .wrapping_add(16 * u32::from(i)),
            )?)
        };
        let mut scratch = [RingEdge::default(); MAX_RING];
        let len = read_ring(
            index,
            surface.first_edge,
            self.header[2 * block::EDGES] as usize,
            edge,
            vertex,
            &mut scratch,
        )?;
        test(&scratch[..len], &[])
    }

    /// The `u` and `v` axes of projection row `row`.
    pub fn row_axes(&self, row: u8) -> Result<[usize; 2], Bail> {
        match self.projection.get(usize::from(row)) {
            Some(&[u, v]) if u != BAD => Ok([usize::from(u), usize::from(v)]),
            _ => Err(Bail::Data),
        }
    }
}

fn projection_axes(row: [i16; 3]) -> [u8; 2] {
    if (0..3).contains(&row[0]) && (0..3).contains(&row[1]) {
        [row[0] as u8, row[1] as u8]
    } else {
        [BAD; 2]
    }
}

fn cached_ring<'a>(edges: &'a [RingEdge], surface: &Surface) -> Result<&'a [RingEdge], Bail> {
    let start = surface.ring as usize;
    edges
        .get(start..)
        .and_then(|tail| tail.get(..surface.ring_len as usize))
        .ok_or(Bail::Data)
}

/// Both paths validate and gather the same bounded ring before testing its
/// edges. A malformed suffix therefore declines even after an outside edge.
fn read_ring(
    surface: u32,
    first: u16,
    n_edges: usize,
    edge: impl Fn(u16) -> Result<[u8; 12], Bail>,
    vertex: impl Fn(u16) -> Result<[u32; 3], Bail>,
    scratch: &mut [RingEdge; MAX_RING],
) -> Result<usize, Bail> {
    let mut e = first;
    for (i, out) in scratch.iter_mut().take(n_edges).enumerate() {
        if usize::from(e) >= n_edges {
            return Err(Bail::Data);
        }
        let r = edge(e)?;
        let right = half(&r, 10) as i16 as i32 == surface as i32;
        let [start, end] = [half(&r, 0), half(&r, 2)];
        let [a, b] = if right { [end, start] } else { [start, end] };
        *out = RingEdge {
            a: vertex(a)?,
            b: vertex(b)?,
        };
        e = half(&r, if right { 6 } else { 4 });
        if e == first {
            return Ok(i + 1);
        }
    }
    Err(Bail::Data)
}

/// `comiss a, 0` then `jb` not taken: `a >= 0`, ordered. A NaN sets CF,
/// so `jb` is taken. `fp` gains the compare's flags.
#[inline]
pub(crate) fn non_negative(fp: &mut Fp, a: u32) -> bool {
    matches!(fp.comiss(a, 0), Some(Ordering::Greater | Ordering::Equal))
}

/// `comiss a, b` then `ja` (or `jbe` not taken): `a > b`, ordered.
#[inline]
pub(crate) fn above(fp: &mut Fp, a: u32, b: u32) -> bool {
    fp.comiss(a, b) == Some(Ordering::Greater)
}
