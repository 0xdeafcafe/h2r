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
//! Damage (vitality, region damage thresholds, effects) is not in Halo 2's
//! `coll`; it is listed in [`CollisionReport`].

use h1models::Converted;
use map::FourCC;
use map::build::Block;
use tags::collision::{BSP_BLOCKS_SIZE, BSP_SIZE, BSP_STRIDES, COLL_ROOT_SIZE};

use crate::Result;
use crate::h1::Collision;

/// What the conversion kept and left out.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollisionReport {
    pub regions: usize,
    pub permutations: usize,
    pub bsps: usize,
    pub surfaces: usize,
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

/// One BSP element (`0x44`): the leading word, then the eight blocks.
fn bsp_element(bsp: &tags::collision::Bsp, node: u32) -> Block {
    let enc = bsp.encode_blocks(0);
    let mut e = Block::zeroed(BSP_SIZE);
    e.put_u32(0, node);
    let mut at = BSP_BLOCKS_SIZE;
    for (i, stride) in BSP_STRIDES.iter().enumerate() {
        let count = u32::from_le_bytes(enc[i * 8..i * 8 + 4].try_into().unwrap()) as usize;
        let len = count * stride;
        block_field(
            &mut e,
            4 + 8 * i as u32,
            Block::new(enc[at..at + len].to_vec()),
            *stride,
        );
        at += len;
    }
    e
}

/// Builds the Halo 2 `coll`.
pub fn convert_collision(c: &Collision) -> Result<(Converted, CollisionReport)> {
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
            for (n, node) in c.nodes.iter().enumerate() {
                if node.region as usize != r {
                    continue;
                }
                if let Some(src) = node.bsps.get(p) {
                    let bsp = h1convert::collision::convert(src)?;
                    report.surfaces += bsp.surfaces.len();
                    bsps.push(bsp_element(&bsp, n as u32));
                }
            }
            report.permutations += 1;
            report.bsps += bsps.len();
            let mut e = Block::zeroed(0x14);
            put_sid(&mut e, 0, perm);
            block_field(&mut e, 4, array(bsps, BSP_SIZE), BSP_SIZE);
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
