//! `coll` BSPs as editable meshes, compiled back, with their BSP physics
//! slots (docs/editor/modeling-targets.md, "Collision and physics
//! models"), and the eight blocks as `edit.collision.stage` takes them
//! (docs/editor/realtime-edits.md, "Collision").
//!
//! **Mesh.** One mesh vert per BSP vertex, one face per surface, wound
//! counter-clockwise seen from the surface's open side (the ring is
//! reversed when it runs the other way about its plane). A face's material
//! is the surface's; its group holds the surface's flags in the low byte
//! and its breakable-surface byte in the high byte ([`group_of`]), so the
//! kernel's ops carry both to new faces as they carry a part.
//!
//! **Slots.** A permutation with BSP physics pairs slot `i` with BSP `i`
//! (collision.md, "BSP physics"). A recompiled BSP's slot gets the box of
//! its vertices and a MOPP code object keyed by plain surface index, as
//! `h1objects::coll::{bsp_box, bsp_mopp}` write them; the slot's `+0x30`
//! datum is kept.
//!
//! **Write.** [`encode_collision`] lays out a whole `coll` payload, every
//! block after the root, 16-aligned, as the codec reads it back; a save
//! that cannot patch blocks in place writes this. [`stage_blocks`] gives
//! one BSP's eight blocks by name for the live path.

use mesh_edit::{Mesh, Poly};
use tags::collision::{BSP_BLOCKS_SIZE, BSP_PHYSICS_SIZE, BSP_SIZE, BSP_STRIDES, Bsp, BspPhysics, Collision};

use crate::geom;
use crate::{Compiled, Error, Options, Polygon, Result, build_with};

/// A face group from a surface's flags and breakable-surface byte.
pub fn group_of(flags: u8, breakable_surface: u8) -> u16 {
    u16::from(flags) | u16::from(breakable_surface) << 8
}

/// A face group's surface flags and breakable-surface byte.
pub fn surface_of(group: u16) -> (u8, u8) {
    (group as u8, (group >> 8) as u8)
}

/// The BSP as a mesh: see the module docs.
pub fn mesh_from_bsp(bsp: &Bsp) -> Result<Mesh> {
    let mut mesh = Mesh::new();
    let verts: Vec<_> = bsp
        .vertices
        .iter()
        .map(|v| mesh.add_vert(v.point, None))
        .collect();
    for (s, surf) in bsp.surfaces.iter().enumerate() {
        let ring = bsp.surface_ring(s);
        let pts: Vec<_> = ring
            .iter()
            .map(|&v| geom::wide(bsp.vertices[v].point))
            .collect();
        let mut ids: Vec<_> = ring.iter().map(|&v| verts[v]).collect();
        if let Some((plane, flip)) = crate::query::surface_plane(bsp, s) {
            let n = geom::wide([plane[0], plane[1], plane[2]]);
            let open = if flip { n.map(|c| -c) } else { n };
            if geom::dot(geom::newell(&pts), open) < 0.0 {
                ids.reverse();
            }
        }
        let mut poly = Poly::new(ids);
        poly.material = surf.material;
        poly.group = group_of(surf.flags, surf.breakable_surface);
        mesh.add_face(&poly)
            .map_err(|e| Error::Invalid(format!("surface {s} does not open as a face: {e:?}")))?;
    }
    Ok(mesh)
}

/// The mesh's faces as polygons to compile.
pub fn polygons_from_mesh(mesh: &Mesh) -> Vec<Polygon> {
    mesh.face_ids()
        .map(|f| {
            let (flags, breakable_surface) = surface_of(mesh.face_group(f));
            Polygon {
                points: mesh.face_positions(f),
                material: mesh.face_material(f),
                flags,
                breakable_surface,
            }
        })
        .collect()
}

/// The box of a BSP's vertices: (centre, half extents).
pub fn bsp_box(bsp: &Bsp) -> ([f32; 3], [f32; 3]) {
    if bsp.vertices.is_empty() {
        return ([0.0; 3], [0.0; 3]);
    }
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for v in &bsp.vertices {
        for a in 0..3 {
            lo[a] = lo[a].min(v.point[a]);
            hi[a] = hi[a].max(v.point[a]);
        }
    }
    (
        std::array::from_fn(|a| (lo[a] + hi[a]) / 2.0),
        std::array::from_fn(|a| (hi[a] - lo[a]) / 2.0),
    )
}

/// The MOPP code object over a BSP's surfaces, keyed by surface index.
/// Empty for a BSP with no surfaces (the engine skips its slot).
pub fn bsp_mopp(bsp: &Bsp) -> Vec<u8> {
    if bsp.surfaces.is_empty() {
        return Vec::new();
    }
    let surfaces: Vec<mopp::Surface> = (0..bsp.surfaces.len())
        .map(|i| mopp::Surface {
            key: i as u32,
            vertices: crate::query::ring(bsp, i),
        })
        .collect();
    mopp::build(&surfaces).to_block()
}

/// A slot for `bsp`, keeping `owner` (the `+0x30` datum).
pub fn physics_slot(bsp: &Bsp, owner: u32) -> BspPhysics {
    let (center, half_extents) = bsp_box(bsp);
    BspPhysics {
        center,
        half_extents,
        owner,
        mopp: bsp_mopp(bsp),
    }
}

/// One BSP of a `coll`: region, permutation and BSP index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BspAt {
    pub region: usize,
    pub permutation: usize,
    pub bsp: usize,
}

/// Compiles `polygons` into the BSP at `at`, keeping its leading word,
/// and rebuilds its BSP physics slot when the permutation has one for it.
/// Every material must be in the tag's list. Nothing changes on an error.
pub fn rebuild(coll: &mut Collision, at: BspAt, polygons: &[Polygon], opts: &Options) -> Result<Compiled> {
    let perm = coll
        .regions
        .get(at.region)
        .and_then(|r| r.permutations.get(at.permutation))
        .ok_or_else(|| Error::Invalid(format!("no permutation {}:{}", at.region, at.permutation)))?;
    let old = perm
        .bsps
        .get(at.bsp)
        .ok_or_else(|| Error::Invalid(format!("no BSP {} in permutation {}:{}", at.bsp, at.region, at.permutation)))?;
    let mut compiled = build_with(polygons, opts)?;
    let materials = coll.materials.len();
    if let Some((surface, s)) = compiled
        .bsp
        .surfaces
        .iter()
        .enumerate()
        .find(|(_, s)| usize::from(s.material) >= materials)
    {
        return Err(Error::Material {
            surface,
            material: s.material,
            materials,
        });
    }
    compiled.bsp.unknown_00 = old.unknown_00;
    let perm = &mut coll.regions[at.region].permutations[at.permutation];
    if let Some(slot) = perm.bsp_physics.get_mut(at.bsp) {
        *slot = physics_slot(&compiled.bsp, slot.owner);
    }
    perm.bsps[at.bsp] = compiled.bsp.clone();
    Ok(compiled)
}

/// Names of the eight blocks, in order, as `edit.collision.stage` names
/// them: the fields of `tags::collision::Bsp`.
pub const BLOCK_NAMES: [&str; 8] = [
    "nodes", "planes", "leaves", "refs", "nodes_2d", "surfaces", "edges", "vertices",
];

/// One block's elements, encoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedBlock {
    pub name: &'static str,
    pub count: u32,
    pub bytes: Vec<u8>,
}

/// The eight blocks of `bsp` by name, each its elements' bytes as
/// `Bsp::encode_blocks` lays them out.
pub fn stage_blocks(bsp: &Bsp) -> Vec<StagedBlock> {
    let enc = bsp.encode_blocks(0);
    let mut at = BSP_BLOCKS_SIZE;
    BLOCK_NAMES
        .iter()
        .zip(BSP_STRIDES)
        .enumerate()
        .map(|(i, (&name, stride))| {
            let count = u32::from_le_bytes(enc[i * 8..i * 8 + 4].try_into().unwrap());
            let len = count as usize * stride;
            let bytes = enc[at..at + len].to_vec();
            at += len;
            StagedBlock { name, count, bytes }
        })
        .collect()
}

/// A payload being laid out at a virtual base.
struct Writer {
    buf: Vec<u8>,
    base: u32,
}

impl Writer {
    fn u32(&mut self, at: usize, v: u32) {
        self.buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn u16(&mut self, at: usize, v: u16) {
        self.buf[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn f32s(&mut self, at: usize, v: [f32; 3]) {
        for (i, c) in v.iter().enumerate() {
            self.u32(at + 4 * i, c.to_bits());
        }
    }
    /// Appends `len` zero bytes, 16-aligned, and points the descriptor at
    /// `desc` there with `count`. Returns the offset; a zero count writes
    /// a null descriptor.
    fn block(&mut self, desc: usize, count: usize, len: usize) -> usize {
        if count == 0 {
            self.u32(desc, 0);
            self.u32(desc + 4, 0);
            return self.buf.len();
        }
        while !self.buf.len().is_multiple_of(16) {
            self.buf.push(0);
        }
        let at = self.buf.len();
        self.buf.resize(at + len, 0);
        self.u32(desc, count as u32);
        self.u32(desc + 4, self.base + at as u32);
        at
    }
}

/// A whole `coll` payload loaded at `base`: the root, then every block.
/// `Collision::decode(&bytes, base)` gives `coll` back.
pub fn encode_collision(coll: &Collision, base: u32) -> Vec<u8> {
    let mut w = Writer {
        buf: vec![0; tags::collision::COLL_ROOT_SIZE],
        base,
    };
    for (i, r) in coll.reserved.iter().enumerate() {
        w.u32(i * 4, *r);
    }
    let at = w.block(0x14, coll.materials.len(), coll.materials.len() * 4);
    for (i, m) in coll.materials.iter().enumerate() {
        w.u32(at + i * 4, m.0);
    }
    let regions = w.block(0x1c, coll.regions.len(), coll.regions.len() * 12);
    for (ri, region) in coll.regions.iter().enumerate() {
        let r = regions + ri * 12;
        w.u32(r, region.name.0);
        let perms = w.block(r + 4, region.permutations.len(), region.permutations.len() * 0x14);
        for (pi, perm) in region.permutations.iter().enumerate() {
            let p = perms + pi * 0x14;
            w.u32(p, perm.name.0);
            let bsps = w.block(p + 4, perm.bsps.len(), perm.bsps.len() * BSP_SIZE);
            for (bi, bsp) in perm.bsps.iter().enumerate() {
                let e = bsps + bi * BSP_SIZE;
                w.u32(e, bsp.unknown_00);
                for (k, block) in stage_blocks(bsp).into_iter().enumerate() {
                    let at = w.block(e + 4 + 8 * k, block.count as usize, block.bytes.len());
                    w.buf[at..at + block.bytes.len()].copy_from_slice(&block.bytes);
                }
            }
            let n = perm.bsp_physics.len();
            let slots = w.block(p + 0xc, n, n * BSP_PHYSICS_SIZE);
            for (si, slot) in perm.bsp_physics.iter().enumerate() {
                let s = slots + si * BSP_PHYSICS_SIZE;
                // Retail's words; the engine writes vtables and pointers
                // over the rest at load.
                w.u16(s + 0x06, 0x81);
                w.u16(s + 0x46, 0x80);
                w.u16(s + 0x56, 0x81);
                w.f32s(s + 0x10, slot.center);
                w.f32s(s + 0x20, slot.half_extents);
                w.u32(s + 0x30, slot.owner);
                let at = w.block(s + 0x64, slot.mopp.len(), slot.mopp.len());
                w.buf[at..at + slot.mopp.len()].copy_from_slice(&slot.mopp);
            }
        }
    }
    let n = coll.pathfinding_spheres.len();
    let at = w.block(0x24, n, n * 0x14);
    for (i, s) in coll.pathfinding_spheres.iter().enumerate() {
        let e = at + i * 0x14;
        w.u16(e, s.node as u16);
        w.u16(e + 2, s.flags);
        w.f32s(e + 4, s.center);
        w.u32(e + 0x10, s.radius.to_bits());
    }
    let n = coll.nodes.len();
    let at = w.block(0x2c, n, n * 12);
    for (i, node) in coll.nodes.iter().enumerate() {
        let e = at + i * 12;
        w.u32(e, node.name.0);
        w.u16(e + 4, node.region as u16);
        w.u16(e + 6, node.parent as u16);
        w.u16(e + 8, node.next_sibling as u16);
        w.u16(e + 10, node.first_child as u16);
    }
    while !w.buf.len().is_multiple_of(16) {
        w.buf.push(0);
    }
    w.buf
}
