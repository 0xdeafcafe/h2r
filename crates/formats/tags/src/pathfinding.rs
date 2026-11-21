//! Halo 2 AI pathfinding data, as a structure BSP (`sbsp`) carries it.
//!
//! Hand-written codec. Layout and provenance (V disc checks, E engine code,
//! S public sources) are in `docs/formats/pathfinding.md`. Names are ours.
//! Blocks this module does not interpret (object refs, compiled hints, the
//! editor's user-placed hints) are kept as raw element bytes with their
//! verified strides, so a decoded element re-encodes byte for byte apart
//! from pointers.
use crate::geometry::{TagView, f32_at, u16_at, u32_at};
use crate::{Error, Result};
use map::{AddressSpace, FourCC, Map, TagId};
use std::io::{Read, Seek};

/// Header field of the pathfinding-data block in an `sbsp` (V).
pub const SBSP_PATHFINDING_FIELD: usize = 0xc4;
/// Header field of the collision-BSP block in an `sbsp` (V).
pub const SBSP_COLLISION_FIELD: usize = 0x14;

/// A sector: one convex walkable polygon, bounded by a ring of links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sector {
    /// u16 at +0.
    pub flags: u16,
    /// u16 at +2: compiled hint index, `0xffff` = none.
    pub hint: u16,
    /// u32 at +4: first link of the ring.
    pub first_link: u32,
}
impl Sector {
    pub const SIZE: usize = 8;
}

/// A link: one directed sector edge, shared by the sectors either side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    /// u16 at +0, +2: start and end vertex.
    pub vertex1: u16,
    pub vertex2: u16,
    /// u16 at +4.
    pub flags: u16,
    /// u16 at +6: compiled hint index, `0xffff` = none.
    pub hint: u16,
    /// u16 at +8: next link around `left_sector`.
    pub forward_link: u16,
    /// u16 at +0xa: next link around `right_sector`.
    pub reverse_link: u16,
    /// u16 at +0xc, +0xe; `0xffff` = no sector on that side.
    pub left_sector: u16,
    pub right_sector: u16,
}
impl Link {
    pub const SIZE: usize = 0x10;
}

/// A surface or 2D-BSP reference (4 bytes). Bit 31 set: a 2D BSP node index
/// in the low bits. Otherwise a sector index. `0xffffffff`: nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ref(pub u32);
impl Ref {
    pub const NONE: Ref = Ref(u32::MAX);
    pub fn sector(i: u32) -> Self {
        Ref(i & 0x7fff_ffff)
    }
    pub fn node(i: u32) -> Self {
        Ref(i | 0x8000_0000)
    }
    pub fn target(self) -> RefTarget {
        if self.0 == u32::MAX {
            RefTarget::None
        } else if self.0 & 0x8000_0000 != 0 {
            RefTarget::Node(self.0 & 0x7fff_ffff)
        } else {
            RefTarget::Sector(self.0)
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefTarget {
    None,
    Sector(u32),
    Node(u32),
}

/// A 2D BSP node: line `a*x + b*y - d`, and a child per side (each a
/// [`Ref`]: node, sector or none). V: nodes are stored children first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bsp2dNode {
    pub plane: [f32; 3],
    /// u32 at +0xc: child where `a*x + b*y - d >= 0`.
    pub positive: Ref,
    /// u32 at +0x10: child where `a*x + b*y - d < 0`.
    pub negative: Ref,
}
impl Bsp2dNode {
    pub const SIZE: usize = 0x14;
}

/// The pathfinding-data element (0x74 bytes).
#[derive(Clone, Debug, PartialEq)]
pub struct PathfindingData {
    /// Block at +0x00.
    pub sectors: Vec<Sector>,
    /// Block at +0x08.
    pub links: Vec<Link>,
    /// Block at +0x10: one per collision surface (V: count equals the
    /// collision BSP's surface count on every disc BSP).
    pub refs: Vec<Ref>,
    /// Block at +0x18.
    pub bsp2d_nodes: Vec<Bsp2dNode>,
    /// Block at +0x20: empty on every disc BSP; stride unknown.
    /// Block at +0x28: 12-byte points.
    pub vertices: Vec<[f32; 3]>,
    /// Block at +0x30 (0x1c-byte elements with child blocks): raw elements.
    pub object_refs: Vec<Vec<u8>>,
    /// Block at +0x38 (0x14-byte elements): compiled hints, raw.
    pub hints: Vec<[u8; 0x14]>,
    /// Block at +0x40: one u32 per instanced-geometry placement.
    pub instance_refs: Vec<u32>,
    /// u32 at +0x48.
    pub checksum: u32,
    /// Block at +0x6c (0x48-byte elements with child blocks): raw elements.
    /// V: exactly one element on every disc BSP that has pathfinding.
    pub user_hints: Vec<Vec<u8>>,
}

impl PathfindingData {
    /// Element size (V: children of the one element start 0x74 bytes after it).
    pub const SIZE: usize = 0x74;
    pub const OBJECT_REF_SIZE: usize = 0x1c;
    pub const HINT_SIZE: usize = 0x14;
    pub const USER_HINTS_SIZE: usize = 0x48;

    /// The pathfinding data of an `sbsp` payload, `None` when the BSP has
    /// none (V: multiplayer maps, the intro and the shared caches).
    pub fn from_sbsp(view: &TagView<'_>) -> Result<Option<Self>> {
        let header = view.at(0, SBSP_PATHFINDING_FIELD + 8)?;
        let elements = view.block(header, SBSP_PATHFINDING_FIELD, Self::SIZE)?;
        match elements.as_slice() {
            [] => Ok(None),
            [one] => Self::decode(view, one).map(Some),
            _ => Err(Error::Invalid("more than one pathfinding-data element")),
        }
    }

    /// Read an `sbsp` whose pathfinding arrays may also point into ordinary
    /// metadata. A BSP's virtual addresses belong to that scenario BSP alone.
    /// Charge each table against its owning region before reading it.
    pub fn read<R: Read + Seek>(map: &mut Map<R>, bsp: TagId) -> Result<Option<Self>> {
        let tag = map.tag(bsp)?;
        if tag.group != FourCC(*b"sbsp") {
            return Err(Error::UnsupportedGroup(tag.group));
        }
        let loc = tag.location.ok_or(map::Error::NoTagData)?;
        if loc.size < (SBSP_PATHFINDING_FIELD + 8) as u32 {
            return Err(Error::Invalid("BSP missing pathfinding block field"));
        }
        let native = AddressSpace {
            virtual_base: loc.address,
            size: loc.size,
            file_offset: loc.file_offset()?,
        };
        let pair = map.read_file_range(native.file_offset + SBSP_PATHFINDING_FIELD as u64, 8)?;
        match u32_at(&pair, 0) {
            0 => return Ok(None),
            1 => {}
            _ => return Err(Error::Invalid("more than one pathfinding-data element")),
        }
        let metadata = map.metadata_space().ok_or(map::Error::NoTagData)?;
        let mut budgets = [native.size, metadata.size];
        let mut read_block = |record: &[u8], field: usize, stride: usize| -> Result<Vec<u8>> {
            let count = u32_at(record, field);
            if count == 0 {
                return Ok(Vec::new());
            }
            let address = u32_at(record, field + 4);
            if !address.is_multiple_of(4) {
                return Err(Error::Invalid("unaligned pathfinding block pointer"));
            }
            let len = count
                .checked_mul(stride as u32)
                .ok_or(Error::Invalid("pathfinding block too large"))?;
            let (region, offset) = match native.translate(address, len) {
                Ok(offset) => (0, offset),
                Err(_) => (1, metadata.translate(address, len)?),
            };
            budgets[region] = budgets[region].checked_sub(len).ok_or(Error::Invalid(
                "pathfinding blocks exceed the payload budget",
            ))?;
            Ok(map.read_file_range(offset, len.into())?)
        };
        let element = read_block(&pair, 0, Self::SIZE)?;
        Self::decode_tables(&element, read_block).map(Some)
    }

    /// Decode one 0x74-byte element.
    pub fn decode(view: &TagView<'_>, e: &[u8]) -> Result<Self> {
        Self::decode_tables(e, |record, field, stride| {
            view.block_bytes(record, field, stride)
        })
    }

    fn decode_tables<B: AsRef<[u8]>>(
        e: &[u8],
        mut read: impl FnMut(&[u8], usize, usize) -> Result<B>,
    ) -> Result<Self> {
        if e.len() != Self::SIZE {
            return Err(Error::RecordSize {
                actual: e.len(),
                expected: Self::SIZE,
            });
        }
        let sectors = read(e, 0x00, Sector::SIZE)?
            .as_ref()
            .chunks_exact(Sector::SIZE)
            .map(|c| Sector {
                flags: u16_at(c, 0),
                hint: u16_at(c, 2),
                first_link: u32_at(c, 4),
            })
            .collect();
        let links = read(e, 0x08, Link::SIZE)?
            .as_ref()
            .chunks_exact(Link::SIZE)
            .map(|c| Link {
                vertex1: u16_at(c, 0),
                vertex2: u16_at(c, 2),
                flags: u16_at(c, 4),
                hint: u16_at(c, 6),
                forward_link: u16_at(c, 8),
                reverse_link: u16_at(c, 0xa),
                left_sector: u16_at(c, 0xc),
                right_sector: u16_at(c, 0xe),
            })
            .collect();
        let refs = read(e, 0x10, 4)?
            .as_ref()
            .chunks_exact(4)
            .map(|c| Ref(u32_at(c, 0)))
            .collect();
        let bsp2d_nodes = read(e, 0x18, Bsp2dNode::SIZE)?
            .as_ref()
            .chunks_exact(Bsp2dNode::SIZE)
            .map(|c| Bsp2dNode {
                plane: [f32_at(c, 0), f32_at(c, 4), f32_at(c, 8)],
                positive: Ref(u32_at(c, 0xc)),
                negative: Ref(u32_at(c, 0x10)),
            })
            .collect();
        if u32_at(e, 0x20) != 0 {
            return Err(Error::UnsupportedBlock("pathfinding +0x20"));
        }
        let vertices = read(e, 0x28, 12)?
            .as_ref()
            .chunks_exact(12)
            .map(|c| [f32_at(c, 0), f32_at(c, 4), f32_at(c, 8)])
            .collect();
        let object_refs = read(e, 0x30, Self::OBJECT_REF_SIZE)?
            .as_ref()
            .chunks_exact(Self::OBJECT_REF_SIZE)
            .map(<[u8]>::to_vec)
            .collect();
        let hints = read(e, 0x38, Self::HINT_SIZE)?
            .as_ref()
            .chunks_exact(Self::HINT_SIZE)
            .map(|c| c.try_into().expect("stride"))
            .collect();
        let instance_refs = read(e, 0x40, 4)?
            .as_ref()
            .chunks_exact(4)
            .map(|c| u32_at(c, 0))
            .collect();
        for field in (0x4c..0x6c).step_by(8) {
            if u32_at(e, field) != 0 {
                return Err(Error::UnsupportedBlock("pathfinding +0x4c..+0x6c"));
            }
        }
        let user_hints = read(e, 0x6c, Self::USER_HINTS_SIZE)?
            .as_ref()
            .chunks_exact(Self::USER_HINTS_SIZE)
            .map(<[u8]>::to_vec)
            .collect();
        Ok(Self {
            sectors,
            links,
            refs,
            bsp2d_nodes,
            vertices,
            object_refs,
            hints,
            instance_refs,
            checksum: u32_at(e, 0x48),
            user_hints,
        })
    }

    /// Links around `sector`, following the ring rule: on a link whose
    /// `left_sector` is the sector continue at `forward_link`, otherwise at
    /// `reverse_link`. `None` if the ring does not close within the link
    /// count or meets a link that does not border the sector.
    pub fn sector_ring(&self, sector: usize) -> Option<Vec<usize>> {
        let first = self.sectors.get(sector)?.first_link as usize;
        let mut out = Vec::new();
        let mut l = first;
        for _ in 0..=self.links.len() {
            let link = self.links.get(l)?;
            let s = sector as u16;
            let next = if link.left_sector == s {
                link.forward_link
            } else if link.right_sector == s {
                link.reverse_link
            } else {
                return None;
            };
            out.push(l);
            l = usize::from(next);
            if l == first {
                return Some(out);
            }
        }
        None
    }

    /// The sector's polygon as vertex indices, in ring order: the start
    /// vertex of each link as the sector sees it (`vertex1` when the sector is
    /// on the left, `vertex2` otherwise).
    pub fn sector_polygon(&self, sector: usize) -> Option<Vec<usize>> {
        let ring = self.sector_ring(sector)?;
        Some(
            ring.into_iter()
                .map(|l| {
                    let link = &self.links[l];
                    if usize::from(link.left_sector) == sector {
                        usize::from(link.vertex1)
                    } else {
                        usize::from(link.vertex2)
                    }
                })
                .collect(),
        )
    }

    /// Resolve a collision surface and a point to a sector, as the refs and
    /// the 2D BSP encode it. Bounded by the node count.
    pub fn sector_for_surface(&self, surface: usize, x: f32, y: f32) -> Option<u32> {
        let mut r = *self.refs.get(surface)?;
        for _ in 0..=self.bsp2d_nodes.len() {
            match r.target() {
                RefTarget::None => return None,
                RefTarget::Sector(s) => return Some(s),
                RefTarget::Node(n) => {
                    let node = self.bsp2d_nodes.get(n as usize)?;
                    let side = node.plane[0] * x + node.plane[1] * y - node.plane[2];
                    r = if side < 0.0 {
                        node.negative
                    } else {
                        node.positive
                    };
                }
            }
        }
        None
    }

    /// Links with no sector on either side (V: present in retail data).
    pub fn orphan_links(&self) -> usize {
        self.links
            .iter()
            .filter(|l| l.left_sector == u16::MAX && l.right_sector == u16::MAX)
            .count()
    }

    /// Index and structure checks the engine's lookups would rely on.
    /// Returns one message per problem (empty when sound).
    pub fn validate(&self, surface_count: Option<usize>) -> Vec<String> {
        let mut out = Vec::new();
        let ns = self.sectors.len();
        let nl = self.links.len();
        let nv = self.vertices.len();
        let nn = self.bsp2d_nodes.len();
        if ns > 0xffff || nl > 0xffff || nv > 0xffff {
            out.push(format!(
                "counts exceed u16: {ns} sectors, {nl} links, {nv} vertices"
            ));
        }
        if let Some(n) = surface_count
            && n != self.refs.len()
        {
            out.push(format!(
                "{} refs for {n} collision surfaces",
                self.refs.len()
            ));
        }
        let side_ok = |s: u16| s == u16::MAX || usize::from(s) < ns;
        for (i, l) in self.links.iter().enumerate() {
            // V: retail data keeps orphan links (no sector on either side);
            // no ring reaches them, so they are skipped.
            if l.left_sector == u16::MAX && l.right_sector == u16::MAX {
                continue;
            }
            if usize::from(l.vertex1) >= nv || usize::from(l.vertex2) >= nv {
                out.push(format!("link {i}: vertex out of range"));
            }
            // Next links are checked by the ring walks below: V: retail data
            // also keeps stale links that name a sector but sit on no ring,
            // with next links of 0xffff.
            if !side_ok(l.left_sector) || !side_ok(l.right_sector) {
                out.push(format!("link {i}: sector out of range"));
            }
            if l.left_sector == l.right_sector {
                out.push(format!("link {i}: same sector on both sides"));
            }
        }
        for i in 0..ns {
            if self.sector_ring(i).is_none() {
                out.push(format!("sector {i}: link ring does not close"));
            }
        }
        let ref_ok = |r: Ref| match r.target() {
            RefTarget::None => true,
            RefTarget::Sector(s) => (s as usize) < ns,
            RefTarget::Node(n) => (n as usize) < nn,
        };
        for (i, r) in self.refs.iter().enumerate() {
            if !ref_ok(*r) {
                out.push(format!("ref {i}: target out of range"));
            }
        }
        for (i, n) in self.bsp2d_nodes.iter().enumerate() {
            if !n.plane.iter().all(|f| f.is_finite()) {
                out.push(format!("2d node {i}: non-finite plane"));
            }
            if !ref_ok(n.positive) || !ref_ok(n.negative) {
                out.push(format!("2d node {i}: child out of range"));
            }
            for c in [n.positive, n.negative] {
                if let RefTarget::Node(k) = c.target()
                    && k as usize >= i
                {
                    out.push(format!("2d node {i}: child {k} does not precede it"));
                }
            }
        }
        if self.vertices.iter().flatten().any(|f| !f.is_finite()) {
            out.push("non-finite vertex".into());
        }
        out
    }
}
