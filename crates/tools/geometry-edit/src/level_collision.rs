//! A structure BSP's level collision with a solid added, read from and
//! written to a map (docs/editor/modeling.md, slice 11, and
//! docs/editor/modeling-targets.md, "Structure collision", step 2).
//!
//! [`read`] takes the structure's collision BSP (`sbsp+0x14`, one 0x40
//! element of eight block pairs), its per-leaf block (`sbsp+0x2c`), its
//! MOPP (`sbsp+0x1e0`) and its collision material count (`sbsp+0x0c`).
//! Each block is read where its pair points, in the structure's own space
//! or in the ordinary metadata, where an earlier save may have moved it.
//! [`LevelSource::add_solid`] compiles and merges the solid
//! (`collision_build::level`), and [`LevelEdit::writes`] is the save:
//!
//! - A block whose count is unchanged is patched where it is, only the
//!   runs that differ.
//! - A block with more elements is a [`GrownBlock`]: a new copy appended to
//!   the ordinary metadata (`map::MapEditor::append_metadata`, 16-aligned)
//!   and its pair pointed there. The old elements stay as dead data.
//!   Adding a solid grows all eight collision blocks and the per-leaf block
//!   (the grown-blocks write class).
//! - The MOPP is written over the old program when it fits, its count set
//!   to the new length and the rest of the old block zeroed; otherwise it
//!   is grown too.
//!
//! The map's header checksum is repaired by the writer
//! (`map::MapEditor::write_to`). The structure checksum (`sbsp+0x08`) is
//! kept. [`LevelEdit::stage`] gives the same bytes for
//! `edit.collision.stage` (docs/editor/realtime-edits.md, "Collision"), so
//! the live game and a saved map agree.

use std::io::{Read, Seek};

use collision_build::level::{Added, LEAF_SIZE, Level, LevelStage};
use collision_build::{Options, Polygon};
use map::{AddressSpace, FourCC, Map, TagId};
use tags::collision::{BSP_BLOCKS_SIZE, BSP_STRIDES, Bsp};
use tags::geometry_write::changed_runs;

use crate::session::{Error, RegionWrite};

/// The structure's collision material block (`0x14` bytes an element).
pub const MATERIALS_FIELD: usize = 0x0c;
/// The structure's collision BSP block: one element of eight pairs.
pub const COLLISION_FIELD: usize = tags::pathfinding::SBSP_COLLISION_FIELD;
/// The per-leaf block, [`LEAF_SIZE`] bytes a leaf.
pub const LEAVES_FIELD: usize = 0x2c;
/// The structure MOPP's byte block.
pub const MOPP_FIELD: usize = 0x1e0;
/// Alignment of a grown block in the metadata.
pub const GROWN_ALIGN: u32 = 16;
/// The root bytes [`read`] needs.
const ROOT_NEEDED: usize = MOPP_FIELD + 8;

/// A block's place: where its bytes are (`space` `None` for the ordinary
/// metadata, else a BSP region), its count and address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockAt {
    pub space: Option<AddressSpace>,
    pub count: u32,
    pub address: u32,
}

/// A structure BSP's level collision as the map holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelSource {
    pub tag: TagId,
    /// Where the structure's payload is (`None`: the ordinary metadata).
    pub space: Option<AddressSpace>,
    /// The payload's address: the root pairs are at fixed offsets from it.
    pub root: u32,
    /// The collision element: where its eight pairs are.
    pub element: BlockAt,
    /// The eight collision blocks, in `collision_build::coll::BLOCK_NAMES`
    /// order, and their bytes as read.
    pub blocks: [BlockAt; 8],
    pub block_bytes: [Vec<u8>; 8],
    pub leaves: BlockAt,
    pub mopp: BlockAt,
    /// The decoded level: BSP, per-leaf block, MOPP and material count.
    pub level: Level,
}

/// A block written to a new place in the ordinary metadata, and the pair
/// that names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrownBlock {
    /// The block's name: a collision block's
    /// (`collision_build::coll::BLOCK_NAMES`), `leaf_clusters` or `mopp`.
    pub name: &'static str,
    /// Where the `(count, address)` pair is: the ordinary metadata (`None`)
    /// or a BSP region.
    pub pair_space: Option<AddressSpace>,
    pub pair: u32,
    pub count: u32,
    /// Every element, in order.
    pub bytes: Vec<u8>,
    pub align: u32,
}

/// What a save writes for an added solid: fixed-size patches and grown
/// blocks.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LevelWrites {
    pub patches: Vec<RegionWrite>,
    pub grown: Vec<GrownBlock>,
}

/// A level with a solid added, ready to save or stage.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelEdit {
    pub source: LevelSource,
    pub added: Added,
}

/// The id a refusal names: `structure_collision:<tag>:0`, the target kind
/// docs/editor/modeling.md lists.
fn target_id(tag: TagId) -> String {
    format!("structure_collision:{:08x}:0", tag.0)
}

fn refused(tag: TagId, reason: impl Into<String>) -> Error {
    Error::Refused {
        target: target_id(tag),
        reason: reason.into(),
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("four bytes"))
}

/// `len` bytes at `address`: in `space` when they lie there, else in the
/// ordinary metadata. Returns the bytes and where they were (`None`: the
/// metadata).
fn read_at<R: Read + Seek>(
    map: &mut Map<R>,
    space: AddressSpace,
    address: u32,
    len: usize,
) -> Result<(Vec<u8>, Option<AddressSpace>), Error> {
    let meta = map.metadata_space();
    let len32 = u32::try_from(len).map_err(|_| Error::Map(map::Error::Limit))?;
    let (offset, at) = match space.translate(address, len32) {
        Ok(o) if Some(space) != meta => (o, Some(space)),
        _ => {
            let m = meta.ok_or(Error::Map(map::Error::NoTagData))?;
            (m.translate(address, len32)?, None)
        }
    };
    if len == 0 {
        return Ok((Vec::new(), at));
    }
    Ok((map.read_file_range(offset, len as u64)?, at))
}

/// Reads structure BSP `tag`'s level collision.
pub fn read<R: Read + Seek>(map: &mut Map<R>, tag: TagId) -> Result<LevelSource, Error> {
    let t = map.tag(tag)?.clone();
    if t.group != FourCC(*b"sbsp") {
        return Err(Error::NoTarget(target_id(tag)));
    }
    let loc = t.location.ok_or_else(|| Error::NoTarget(target_id(tag)))?;
    let space = (Some(loc.space) != map.metadata_space()).then_some(loc.space);
    let root = map.read_tag_bytes(tag)?;
    if root.len() < ROOT_NEEDED {
        return Err(refused(tag, "the structure's root is too short"));
    }
    let pair = |at: usize| (u32_at(&root, at), u32_at(&root, at + 4));
    let (n, element_address) = pair(COLLISION_FIELD);
    if n != 1 {
        return Err(refused(tag, format!("the structure has {n} collision BSPs, not one")));
    }
    let (bytes, element_space) = read_at(map, loc.space, element_address, BSP_BLOCKS_SIZE)?;
    let element = BlockAt {
        space: element_space,
        count: 1,
        address: element_address,
    };
    // The blocks gathered into one payload the codec reads.
    const BASE: u32 = 0x1000_0000;
    let mut joined = vec![0u8; BSP_BLOCKS_SIZE];
    let mut blocks = [BlockAt {
        space: None,
        count: 0,
        address: 0,
    }; 8];
    let mut block_bytes: [Vec<u8>; 8] = Default::default();
    for (i, stride) in BSP_STRIDES.iter().enumerate() {
        let (count, address) = (u32_at(&bytes, i * 8), u32_at(&bytes, i * 8 + 4));
        let len = count as usize * stride;
        let (data, at) = if count == 0 {
            (Vec::new(), element_space)
        } else {
            read_at(map, loc.space, address, len)?
        };
        blocks[i] = BlockAt {
            space: at,
            count,
            address,
        };
        let here = if count == 0 { 0 } else { BASE + joined.len() as u32 };
        joined[i * 8..i * 8 + 4].copy_from_slice(&count.to_le_bytes());
        joined[i * 8 + 4..i * 8 + 8].copy_from_slice(&here.to_le_bytes());
        joined.extend_from_slice(&data);
        block_bytes[i] = data;
    }
    let bsp = Bsp::decode_blocks(&joined, BASE, 0)?;
    let at = |(count, address): (u32, u32), stride: usize, map: &mut Map<R>| -> Result<(BlockAt, Vec<u8>), Error> {
        let (data, space) = if count == 0 {
            (Vec::new(), space)
        } else {
            read_at(map, loc.space, address, count as usize * stride)?
        };
        Ok((BlockAt { space, count, address }, data))
    };
    let (leaves, leaf_bytes) = at(pair(LEAVES_FIELD), LEAF_SIZE, map)?;
    if leaves.count as usize != bsp.leaves.len() {
        return Err(refused(
            tag,
            format!(
                "the per-leaf block has {} elements for {} leaves",
                leaves.count,
                bsp.leaves.len()
            ),
        ));
    }
    let (mopp, mopp_bytes) = at(pair(MOPP_FIELD), 1, map)?;
    let materials = pair(MATERIALS_FIELD).0 as usize;
    Ok(LevelSource {
        tag,
        space,
        root: loc.address,
        element,
        blocks,
        block_bytes,
        leaves,
        mopp,
        level: Level {
            bsp,
            leaves: leaf_bytes,
            mopp: mopp_bytes,
            materials,
        },
    })
}

impl LevelSource {
    /// Compiles `polygons` (materials indexing the structure's collision
    /// materials) and appends them to the level. Refused, naming the
    /// target, when `collision_build::level::Level::add_solid` refuses.
    pub fn add_solid(&self, polygons: &[Polygon], opts: &Options) -> Result<LevelEdit, Error> {
        let added = self
            .level
            .add_solid(polygons, opts)
            .map_err(|e| refused(self.tag, e.to_string()))?;
        Ok(LevelEdit {
            source: self.clone(),
            added,
        })
    }
}

impl LevelWrites {
    /// The write of one block: patched in place, only the runs that
    /// differ, when its count holds; else grown. `old` is the block and its
    /// bytes as read, `pair` where its pair is, `new` the count and bytes.
    fn block(
        &mut self,
        name: &'static str,
        (old, old_bytes): (BlockAt, &[u8]),
        (pair_space, pair): (Option<AddressSpace>, u32),
        (count, bytes): (u32, &[u8]),
    ) {
        if count == old.count && bytes.len() == old_bytes.len() {
            for (offset, run) in changed_runs(old_bytes, bytes) {
                self.patches.push(RegionWrite {
                    space: old.space,
                    address: old.address + offset as u32,
                    bytes: run,
                });
            }
            return;
        }
        self.grown.push(GrownBlock {
            name,
            pair_space,
            pair,
            count,
            bytes: bytes.to_vec(),
            align: GROWN_ALIGN,
        });
    }
}

impl LevelEdit {
    /// What a save writes: see the module docs.
    pub fn writes(&self) -> LevelWrites {
        let src = &self.source;
        let new = &self.added.level;
        let mut out = LevelWrites::default();
        for (i, b) in collision_build::coll::stage_blocks(&new.bsp).into_iter().enumerate() {
            out.block(
                b.name,
                (src.blocks[i], &src.block_bytes[i]),
                (src.element.space, src.element.address + 8 * i as u32),
                (b.count, &b.bytes),
            );
        }
        out.block(
            "leaf_clusters",
            (src.leaves, &src.level.leaves),
            (src.space, src.root + LEAVES_FIELD as u32),
            ((new.leaves.len() / LEAF_SIZE) as u32, &new.leaves),
        );
        let old = src.mopp;
        let fits = old.count > 0 && new.mopp.len() <= old.count as usize;
        if fits {
            // Over the old program: the new code, then zeros to the old
            // block's end, and the count set to the new length.
            let mut bytes = new.mopp.clone();
            bytes.resize(old.count as usize, 0);
            for (offset, run) in changed_runs(&src.level.mopp, &bytes) {
                out.patches.push(RegionWrite {
                    space: old.space,
                    address: old.address + offset as u32,
                    bytes: run,
                });
            }
            if new.mopp.len() != old.count as usize {
                out.patches.push(RegionWrite {
                    space: src.space,
                    address: src.root + MOPP_FIELD as u32,
                    bytes: (new.mopp.len() as u32).to_le_bytes().to_vec(),
                });
            }
        } else if !new.mopp.is_empty() {
            out.grown.push(GrownBlock {
                name: "mopp",
                pair_space: src.space,
                pair: src.root + MOPP_FIELD as u32,
                count: new.mopp.len() as u32,
                bytes: new.mopp.clone(),
                align: GROWN_ALIGN,
            });
        }
        out
    }

    /// The blocks `edit.collision.stage` sends: the eight collision blocks
    /// by name, the per-leaf block and the MOPP, the bytes
    /// [`Self::writes`] saves.
    pub fn stage(&self) -> LevelStage {
        self.added.stage()
    }
}

#[cfg(test)]
#[path = "level_collision_tests.rs"]
mod tests;
