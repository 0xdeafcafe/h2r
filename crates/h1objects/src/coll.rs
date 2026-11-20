//! Halo 1 `coll` to Halo 2 `coll`.
//!
//! Halo 1 keeps its BSPs under nodes (a node's BSP `i` is permutation `i`
//! of the node's region); Halo 2 keeps them under region permutations
//! (docs/formats/collision.md). So permutation `p` of region `r` gets, in
//! node order, BSP `p` of every node whose region is `r`, and the BSP's
//! leading word holds that node's index (D: the retail word varies 0..=7,
//! which fits a node index; not established). Each BSP is the same
//! narrowing copy the structure uses ([`h1convert::collision::convert`]).
//!
//! **BSP physics.** Havok, which moves bipeds, vehicles and dropped items,
//! sees an object's collision only through its physics model or, with
//! none, through the permutation's BSP physics slots: one `0x70`-byte slot
//! per BSP, a Havok shape over the BSP inside a MOPP tree whose code keys
//! the BSP's surfaces by index (docs/formats/collision.md, "BSP
//! physics"). Retail writes the slots exactly for the `coll`s of scenery
//! with no `phmo`, so [`convert_collision`] takes that choice. Without
//! them the player walks through converted scenery.
//!
//! Damage (vitality, region damage thresholds, effects) is not in Halo 2's
//! `coll`; it is listed in [`CollisionReport`].

use h1models::Converted;
use map::FourCC;
use map::build::Block;
use tags::collision::{
    BSP_BLOCKS_SIZE, BSP_PHYSICS_SIZE, BSP_SIZE, BSP_STRIDES, Bsp, COLL_ROOT_SIZE,
};

use crate::Result;
use crate::h1::Collision;

/// What the conversion kept and left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollisionReport {
    pub regions: usize,
    pub permutations: usize,
    pub bsps: usize,
    pub surfaces: usize,
    /// BSP physics slots written (one per BSP, or none).
    pub bsp_physics: usize,
    /// Bytes of MOPP code in those slots.
    pub mopp_bytes: usize,
    /// Halo 1 damage values with no home in Halo 2's `coll`.
    pub body_vitality: f32,
    pub shield_vitality: f32,
    pub modifiers: u32,
    pub damage_references: usize,
}

fn array(elements: Vec<Block>, stride: usize) -> Block {
    let mut out = Block::zeroed(elements.len() * stride);
    for (i, e) in elements.into_iter().enumerate() {
        assert_eq!(e.bytes.len(), stride);
        out.bytes[i * stride..(i + 1) * stride].copy_from_slice(&e.bytes);
        for mut f in e.fixups {
            f.offset += (i * stride) as u32;
            out.fixups.push(f);
        }
    }
    out
}

fn block_field(parent: &mut Block, at: u32, child: Block, stride: usize) {
    if child.bytes.is_empty() {
        return;
    }
    let count = (child.bytes.len() / stride) as u32;
    parent.block(at, count, child);
}

/// Sharing key for a BSP's child block or MOPP code: placed once however
/// many `coll`s hold the same bytes (Halo 1 levels carry identical
/// collision under several names, b40's boulders among them). The engine
/// only reads these blocks. The key is the bytes' hash with a tag of its
/// own, so it meets no other sharer's keys.
fn share_key(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b"h1objects coll block".hash(&mut h);
    bytes.hash(&mut h);
    h.finish()
}

/// One BSP element (`0x44`): the leading word, then the eight blocks.
fn bsp_element(bsp: &tags::collision::Bsp, node: u32) -> Block {
    let enc = bsp.encode_blocks(0);
    let mut e = Block::zeroed(BSP_SIZE);
    e.put_u32(0, node);
    let mut at = BSP_BLOCKS_SIZE;
    for (i, stride) in BSP_STRIDES.iter().enumerate() {
        let count = u32::from_le_bytes(enc[i * 8..i * 8 + 4].try_into().unwrap()) as usize;
        let len = count * stride;
        let bytes = enc[at..at + len].to_vec();
        let key = share_key(&bytes);
        block_field(
            &mut e,
            4 + 8 * i as u32,
            Block::new(bytes).shared(key),
            *stride,
        );
        at += len;
    }
    e
}

/// The bounding box of a BSP's vertices: (centre, half extents).
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

/// The MOPP code object over a BSP's surfaces, keyed by surface index
/// (V: every retail slot's code emits keys `0..surfaces`, each once).
pub fn bsp_mopp(bsp: &Bsp) -> Vec<u8> {
    let surfaces: Vec<mopp::Surface> = (0..bsp.surfaces.len())
        .map(|i| mopp::Surface {
            key: i as u32,
            vertices: bsp
                .surface_ring(i)
                .into_iter()
                .map(|v| bsp.vertices[v].point)
                .collect(),
        })
        .collect();
    mopp::build(&surfaces).to_block()
}

/// One BSP physics slot (`0x70`): the box at `+0x10`/`+0x20`, the `coll`'s
/// own datum at `+0x30`, the MOPP code block at `+0x64`. The words at
/// `+0x06`, `+0x46` and `+0x56` are retail's (`0x81`, `0x80`, `0x81`); the
/// engine rewrites the vtables and pointers at load.
fn bsp_physics(bsp: &Bsp) -> (Block, usize) {
    let (center, half) = bsp_box(bsp);
    let mut e = Block::zeroed(BSP_PHYSICS_SIZE);
    for a in 0..3 {
        e.put_f32(0x10 + 4 * a, center[a]);
        e.put_f32(0x20 + 4 * a, half[a]);
    }
    e.put_u16(0x06, 0x81)
        .put_u16(0x46, 0x80)
        .put_u16(0x56, 0x81);
    e.tag_id(0x30, h1models::SELF_ID);
    let code = bsp_mopp(bsp);
    let len = code.len();
    if len > 0 {
        let key = share_key(&code);
        e.block(0x64, len as u32, Block::new(code).aligned(16).shared(key));
    }
    (e, len)
}

/// Builds the Halo 2 `coll`. `bsp_physics`: write the BSP physics slots
/// (a scenery object with no `phmo`).
pub fn convert_collision(c: &Collision, bsp_physics: bool) -> Result<(Converted, CollisionReport)> {
    let mut report = CollisionReport {
        regions: c.regions.len(),
        body_vitality: c.body_vitality,
        shield_vitality: c.shield_vitality,
        modifiers: c.modifiers,
        damage_references: c.references.iter().filter(|r| r.tag.is_some()).count(),
        ..Default::default()
    };
    let mut root = Block::zeroed(COLL_ROOT_SIZE);

    let materials = c
        .materials
        .iter()
        .map(|m| {
            let mut b = Block::zeroed(4);
            put_sid(&mut b, 0, m);
            b
        })
        .collect();
    block_field(&mut root, 0x14, array(materials, 4), 4);

    let mut regions = Vec::new();
    for (r, region) in c.regions.iter().enumerate() {
        let mut perms = Vec::new();
        for (p, perm) in region.permutations.iter().enumerate() {
            let mut bsps = Vec::new();
            let mut slots = Vec::new();
            for (n, node) in c.nodes.iter().enumerate() {
                if node.region as usize != r {
                    continue;
                }
                if let Some(src) = node.bsps.get(p) {
                    let bsp = h1convert::collision::convert(src)?;
                    report.surfaces += bsp.surfaces.len();
                    bsps.push(bsp_element(&bsp, n as u32));
                    if bsp_physics {
                        let (slot, len) = self::bsp_physics(&bsp);
                        report.mopp_bytes += len;
                        slots.push(slot);
                    }
                }
            }
            report.permutations += 1;
            report.bsps += bsps.len();
            report.bsp_physics += slots.len();
            let mut e = Block::zeroed(0x14);
            put_sid(&mut e, 0, perm);
            block_field(&mut e, 4, array(bsps, BSP_SIZE), BSP_SIZE);
            block_field(
                &mut e,
                0xc,
                array(slots, BSP_PHYSICS_SIZE).aligned(16),
                BSP_PHYSICS_SIZE,
            );
            perms.push(e);
        }
        let mut e = Block::zeroed(12);
        put_sid(&mut e, 0, &region.name);
        block_field(&mut e, 4, array(perms, 0x14), 0x14);
        regions.push(e);
    }
    block_field(&mut root, 0x1c, array(regions, 12), 12);

    let spheres = c
        .spheres
        .iter()
        .map(|s| {
            let mut b = Block::zeroed(0x14);
            b.put_u16(0, s.node as u16);
            for (i, v) in s.center.iter().enumerate() {
                b.put_f32(4 + 4 * i, *v);
            }
            b.put_f32(0x10, s.radius);
            b
        })
        .collect();
    block_field(&mut root, 0x24, array(spheres, 0x14), 0x14);

    let nodes = c
        .nodes
        .iter()
        .map(|n| {
            let mut b = Block::zeroed(12);
            put_sid(&mut b, 0, &n.name);
            b.put_u16(4, n.region as u16);
            b.put_u16(6, n.parent as u16);
            b.put_u16(8, n.next_sibling as u16);
            b.put_u16(10, n.first_child as u16);
            b
        })
        .collect();
    block_field(&mut root, 0x2c, array(nodes, 12), 12);

    Ok((
        Converted {
            group: FourCC(*b"coll"),
            root,
            raw: Vec::new(),
        },
        report,
    ))
}

/// The collision of the Halo 1 object `obj` in object space, as Halo 2
/// BSPs: for each collision node with any BSP, its first (permutation 0
/// of the node's region: what a converted placement, which names no
/// variant, draws), converted ([`h1convert::collision::convert`]) and
/// placed by the render model's default pose, as
/// `Converter::built_physics` places a node's cells (the model node of the
/// same name, else of the same index, else node 0). Empty with no
/// collision. For [`h1convert::convert::halo1::merge_scenery_collision`],
/// which makes placed scenery solid as structure.
pub fn object_bsps(
    h1: &h1map::H1Map,
    obj: &crate::h1::Object,
    globals: &[String],
) -> Result<Vec<Bsp>> {
    use h1convert::scenery_collision::{Pose, transformed};
    let Some(c) = obj.collision else {
        return Ok(Vec::new());
    };
    let tag = h1.tag(c)?;
    let coll = Collision::read(h1, tag)?;
    let materials = coll
        .material_types
        .iter()
        .map(|&kind| {
            crate::arsenal::MATERIALS
                .get(usize::from(kind))
                .and_then(|(_, name)| globals.iter().position(|g| g == *name))
                .and_then(|i| u16::try_from(i).ok())
                .ok_or_else(|| {
                    crate::Error::Invalid(format!(
                        "{}: {}: unknown collision substance {kind}",
                        h1.header().name,
                        tag.name
                    ))
                })
        })
        .collect::<Result<Vec<_>>>()?;
    // (name, default pose) per model node.
    let nodes: Vec<(String, Pose)> = match obj.model {
        Some(m) => {
            let t = h1.tag(m)?;
            h1.tag_data(t)?;
            let model = h1models::h1::Model::read(h1, t.data_address)?;
            let world = h1models::mode::world_transforms(
                &model
                    .nodes
                    .iter()
                    .map(|n| (n.parent, n.rotation, n.translation))
                    .collect::<Vec<_>>(),
            );
            model
                .nodes
                .iter()
                .zip(world)
                .map(|(n, (r, t))| (n.name.clone(), Pose { r, t }))
                .collect()
        }
        None => Vec::new(),
    };
    let mut out = Vec::new();
    for (i, n) in coll.nodes.iter().enumerate() {
        let Some(src) = n.bsps.first() else { continue };
        let mut bsp = h1convert::collision::convert(src)?;
        for surface in &mut bsp.surfaces {
            surface.material = *materials
                .get(usize::from(surface.material))
                .ok_or_else(|| {
                    crate::Error::Invalid(format!(
                        "{}: {}: missing collision material {}",
                        h1.header().name,
                        tag.name,
                        surface.material
                    ))
                })?;
        }
        let pose = nodes
            .iter()
            .find(|m| m.0 == n.name)
            .or_else(|| nodes.get(i))
            .or_else(|| nodes.first())
            .map_or(Pose::IDENTITY, |m| m.1);
        out.push(if pose == Pose::IDENTITY {
            bsp
        } else {
            transformed(&bsp, &pose)?
        });
    }
    Ok(out)
}

/// Halo 1 names to string IDs: lower case, spaces as underscores (the
/// form of Halo 2's own names).
pub fn sid_name(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

/// Writes `name` as a string ID at `at`; an empty name stays 0.
pub(crate) fn put_sid(b: &mut Block, at: usize, name: &str) {
    let s = sid_name(name);
    if !s.is_empty() {
        b.string_id(at as u32, &s);
    }
}
