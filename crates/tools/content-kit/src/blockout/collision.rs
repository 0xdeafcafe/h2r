//! A definition's `coll`: its shell compiled to one collision BSP.
//!
//! Every face of the shell is one polygon, whatever its material: a
//! `Nodraw` or `Clip` face collides as any other. The shell is wound
//! outward (the open side is outside), the form `collision_build::Polygon`
//! takes. A BSP that fails `collision_build::verify::check` is refused with
//! the check's sentences.
//!
//! The tag has one material (`collision_material.as_str()`), one region and permutation
//! (`region` in `content/blockout/kit.yaml`), one node (`node`) and no BSP physics slots:
//! the `phmo` beside it stands in front of Havok
//! (docs/formats/collision.md, "BSP physics").

use anyhow::{Result, bail};
use collision_build::{Compiled, Polygon, verify};
use forge_shapes::Solid;
use map::build::Block;
use tags::collision::{BSP_BLOCKS_SIZE, BSP_SIZE, BSP_STRIDES, Bsp, COLL_ROOT_SIZE};



/// The shell's faces as collision polygons, in shell order, all material 0.
pub fn polygons(solid: &Solid) -> Vec<Polygon> {
    solid
        .shell
        .iter()
        .map(|f| Polygon::new(f.ring.clone(), 0))
        .collect()
}

/// The shell compiled and checked.
pub fn compile(solid: &Solid) -> Result<Compiled> {
    let compiled = collision_build::build(&polygons(solid))
        .map_err(|e| anyhow::anyhow!("the collision does not compile: {e}"))?;
    let problems = verify::check(&compiled.bsp, 1);
    if !problems.is_empty() {
        bail!("the collision fails its check: {}", problems.join("; "));
    }
    Ok(compiled)
}

/// One BSP element (`0x44`): the node index, then the eight blocks.
fn bsp_element(bsp: &Bsp) -> Block {
    let enc = bsp.encode_blocks(0);
    let mut e = Block::zeroed(BSP_SIZE);
    e.put_u32(0, 0);
    let mut at = BSP_BLOCKS_SIZE;
    for (i, stride) in BSP_STRIDES.iter().enumerate() {
        let count = u32::from_le_bytes(enc[i * 8..i * 8 + 4].try_into().expect("four bytes"));
        let len = count as usize * stride;
        if count > 0 {
            e.block(4 + 8 * i as u32, count, Block::new(enc[at..at + len].to_vec()));
        }
        at += len;
    }
    e
}

/// The `coll` root holding `bsp`: material `collision_material.as_str()`, region and
/// permutation `region.as_str()`, node `node.as_str()`.
pub fn coll(bsp: &Bsp) -> Block {
    let mut root = Block::zeroed(COLL_ROOT_SIZE);

    let mut material = Block::zeroed(4);
    material.string_id(0, crate::blockout::data::data().collision_material.as_str());
    root.block(0x14, 1, material);

    let mut perm = Block::zeroed(0x14);
    perm.string_id(0, crate::blockout::data::data().region.as_str());
    perm.block(4, 1, bsp_element(bsp));
    let mut region = Block::zeroed(12);
    region.string_id(0, crate::blockout::data::data().region.as_str());
    region.block(4, 1, perm);
    root.block(0x1c, 1, region);

    let mut node = Block::zeroed(12);
    node.string_id(0, crate::blockout::data::data().node.as_str());
    node.put_u16(4, 0)
        .put_u16(6, u16::MAX)
        .put_u16(8, u16::MAX)
        .put_u16(10, u16::MAX);
    root.block(0x2c, 1, node);
    root
}
