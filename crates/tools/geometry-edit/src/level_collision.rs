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
//! [`LevelEdit::add_solid`] adds another solid to an edit's level, keeping
//! its source, so an edit holds every solid added since the map was read.
//! [`box_solid`] is the box the Modeling tool adds (`add_box`).
//!
//! The map's header checksum is repaired by the writer
//! (`map::MapEditor::write_to`). The structure checksum (`sbsp+0x08`) is
//! kept. [`LevelEdit::stage`] gives the same bytes for
//! `edit.collision.stage` (docs/editor/realtime-edits.md, "Collision"), so
//! the live game and a saved map agree.

use tags::bytes::u32_at;
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

/// `len` bytes at `address`: in `space` when they lie there, else in the
/// ordinary metadata. Returns the bytes and where they were (`None`: the
/// metadata).
pub(crate) fn read_at<R: Read + Seek>(
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

/// A closed box solid inside `[lo, hi]`, its six faces wound outward
/// (counter-clockwise seen from outside), all of `material`: the solid
/// the Modeling tool's "Add collision box" adds.
pub fn box_solid(lo: [f32; 3], hi: [f32; 3], material: u16) -> Vec<Polygon> {
    let c = |i: usize| -> [f32; 3] {
        [
            if i & 1 != 0 { hi[0] } else { lo[0] },
            if i & 2 != 0 { hi[1] } else { lo[1] },
            if i & 4 != 0 { hi[2] } else { lo[2] },
        ]
    };
    let q = |is: [usize; 4]| Polygon::new(is.iter().map(|&i| c(i)).collect(), material);
    vec![
        q([0, 4, 6, 2]), // -x
        q([1, 3, 7, 5]), // +x
        q([0, 1, 5, 4]), // -y
        q([2, 6, 7, 3]), // +y
        q([0, 2, 3, 1]), // -z
        q([4, 5, 7, 6]), // +z
    ]
}

/// Why a box cannot be a solid: a corner not finite, or no volume.
fn box_refusal(lo: [f32; 3], hi: [f32; 3]) -> Option<String> {
    if lo.iter().chain(&hi).any(|v| !v.is_finite()) {
        return Some("the box's corners must be finite".into());
    }
    if (0..3).any(|i| hi[i] <= lo[i]) {
        return Some(format!("the box {lo:?} to {hi:?} has no volume: every hi must be above its lo"));
    }
    None
}

impl LevelSource {
    /// [`Self::add_solid`] of [`box_solid`].
    pub fn add_box(&self, lo: [f32; 3], hi: [f32; 3], material: u16) -> Result<LevelEdit, Error> {
        if let Some(why) = box_refusal(lo, hi) {
            return Err(refused(self.tag, why));
        }
        self.add_solid(&box_solid(lo, hi, material), &Options::default())
    }

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
    pub(crate) fn block(
        &mut self,
        name: &'static str,
        (old, old_bytes): (BlockAt, &[u8]),
        (pair_space, pair): (Option<AddressSpace>, u32),
        (count, bytes): (u32, &[u8]),
    ) {
        if count == old.count && bytes.len() == old_bytes.len() {
            for (offset, run) in changed_runs(old_bytes, bytes) {
                let was = old_bytes[offset..offset + run.len()].to_vec();
                self.patches.push(RegionWrite {
                    space: old.space,
                    address: old.address + offset as u32,
                    bytes: run,
                    was,
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

impl LevelWrites {
    /// The write of a MOPP block: over the old program when the new one
    /// fits, its count set to the new length and the old tail zeroed;
    /// else a grown block. `old` is the block and its bytes as read, `pair`
    /// where its `(count, address)` pair is.
    pub(crate) fn mopp(
        &mut self,
        (old, old_bytes): (BlockAt, &[u8]),
        (pair_space, pair): (Option<AddressSpace>, u32),
        new: &[u8],
    ) {
        let fits = old.count > 0 && new.len() <= old.count as usize;
        if fits {
            let mut bytes = new.to_vec();
            bytes.resize(old.count as usize, 0);
            for (offset, run) in changed_runs(old_bytes, &bytes) {
                let was = old_bytes[offset..offset + run.len()].to_vec();
                self.patches.push(RegionWrite {
                    space: old.space,
                    address: old.address + offset as u32,
                    bytes: run,
                    was,
                });
            }
            if new.len() != old.count as usize {
                self.patches.push(RegionWrite {
                    space: pair_space,
                    address: pair,
                    bytes: (new.len() as u32).to_le_bytes().to_vec(),
                    was: old.count.to_le_bytes().to_vec(),
                });
            }
        } else if !new.is_empty() {
            self.grown.push(GrownBlock {
                name: "mopp",
                pair_space,
                pair,
                count: new.len() as u32,
                bytes: new.to_vec(),
                align: GROWN_ALIGN,
            });
        }
    }
}

impl LevelEdit {
    /// This edit with another solid added to its level: the same source,
    /// so [`Self::writes`] and [`Self::stage`] give the file's blocks
    /// with every solid added so far.
    pub fn add_solid(&self, polygons: &[Polygon], opts: &Options) -> Result<LevelEdit, Error> {
        let added = self
            .added
            .level
            .add_solid(polygons, opts)
            .map_err(|e| refused(self.source.tag, e.to_string()))?;
        Ok(LevelEdit {
            source: self.source.clone(),
            added,
        })
    }

    /// [`Self::add_solid`] of [`box_solid`].
    pub fn add_box(&self, lo: [f32; 3], hi: [f32; 3], material: u16) -> Result<LevelEdit, Error> {
        if let Some(why) = box_refusal(lo, hi) {
            return Err(refused(self.source.tag, why));
        }
        self.add_solid(&box_solid(lo, hi, material), &Options::default())
    }

    /// What a save writes: see the module docs.
    pub fn writes(&self) -> LevelWrites {
        level_writes(&self.source, &self.added.level)
    }

    /// The blocks `edit.collision.stage` sends: the eight collision blocks
    /// by name, the per-leaf block and the MOPP, the bytes
    /// [`Self::writes`] saves.
    pub fn stage(&self) -> LevelStage {
        self.added.stage()
    }
}

/// The writes that turn `src`'s level into `new`: the eight collision
/// blocks, the per-leaf block and the MOPP, each patched where its count
/// holds and grown where it does not.
pub(crate) fn level_writes(src: &LevelSource, new: &Level) -> LevelWrites {
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
    out.mopp(
        (src.mopp, &src.level.mopp),
        (src.space, src.root + MOPP_FIELD as u32),
        &new.mopp,
    );
    out
}

#[cfg(test)]
#[path = "level_collision_tests.rs"]
mod tests;
