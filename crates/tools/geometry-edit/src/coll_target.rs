//! A `coll` BSP as a mesh target (docs/editor/modeling.md, slice 8, and
//! docs/editor/modeling-targets.md, "Collision and physics models").
//!
//! The target is `coll_bsp:<tag>:<n>`, `n` counting the tag's BSPs in
//! region, permutation, BSP order (`Collision::bsps`). [`CollSource::read`]
//! decodes the tag and walks it for where every block of each BSP, and each
//! BSP physics slot's box and MOPP, lies in the ordinary metadata.
//! [`CollSource::mesh`] opens a BSP as a polygon mesh
//! (`collision_build::coll::mesh_from_bsp`), and [`CollSource::writes`]
//! compiles an edited mesh back (`collision_build::coll::rebuild`, which
//! keeps the BSP's leading word and rebuilds its slot) and gives what a
//! save writes, as [`LevelWrites`] does for a structure's level collision:
//!
//! - A block whose count holds is patched where it is, only the runs that
//!   differ; one with more or fewer elements is a grown block, appended to
//!   the ordinary metadata with its pair pointed there.
//! - A slot's box is patched in place; its MOPP goes over the old program
//!   when it fits, else it grows.
//!
//! An unedited BSP writes nothing.

use std::io::{Read, Seek};

use collision_build::coll::{self, BspAt};
use collision_build::{Options, Polygon};
use map::{Map, TagId};
use mesh_edit::Mesh;
use tags::collision::{BSP_PHYSICS_SIZE, BSP_SIZE, BSP_STRIDES, COLL, Collision};
use tags::geometry_write::changed_runs;

use crate::level_collision::{BlockAt, LevelWrites};
use crate::session::{Error, RegionWrite};
use crate::target::{Target, TargetKind};

/// A BSP physics slot's place: its element address and its MOPP block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotAt {
    pub address: u32,
    pub mopp: BlockAt,
    pub mopp_bytes: Vec<u8>,
}

/// One BSP's place in the ordinary metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BspSite {
    pub at: BspAt,
    /// The BSP element's address: its eight pairs are at `+4`.
    pub element: u32,
    pub blocks: [BlockAt; 8],
    pub block_bytes: [Vec<u8>; 8],
    /// The permutation's slot for this BSP, if it has one.
    pub slot: Option<SlotAt>,
}

/// A `coll` tag as the map holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct CollSource {
    pub tag: TagId,
    pub collision: Collision,
    pub sites: Vec<BspSite>,
}

fn refused(tag: TagId, index: usize, reason: impl Into<String>) -> Error {
    Error::Refused {
        target: Target::new(TargetKind::Coll, tag, index as u32).id(),
        reason: reason.into(),
    }
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("four bytes"))
}

fn f3(v: [f32; 3]) -> Vec<u8> {
    v.iter().flat_map(|c| c.to_bits().to_le_bytes()).collect()
}

/// Reads `len` bytes at `address` of the ordinary metadata.
fn read_meta<R: Read + Seek>(map: &mut Map<R>, address: u32, len: usize) -> Result<Vec<u8>, Error> {
    let space = map.metadata_space().ok_or(Error::Map(map::Error::NoTagData))?;
    let len32 = u32::try_from(len).map_err(|_| Error::Map(map::Error::Limit))?;
    let at = space.translate(address, len32)?;
    Ok(map.read_file_range(at, len as u64)?)
}

/// The `(count, address)` pair at `address`.
fn pair<R: Read + Seek>(map: &mut Map<R>, address: u32) -> Result<(u32, u32), Error> {
    let b = read_meta(map, address, 8)?;
    Ok((u32_at(&b, 0), u32_at(&b, 4)))
}

/// The block a pair names, as `BlockAt` and its `count * stride` bytes.
fn block<R: Read + Seek>(map: &mut Map<R>, at: u32, stride: usize) -> Result<(BlockAt, Vec<u8>), Error> {
    let (count, address) = pair(map, at)?;
    let bytes = if count == 0 {
        Vec::new()
    } else {
        read_meta(map, address, count as usize * stride)?
    };
    Ok((
        BlockAt {
            space: None,
            count,
            address,
        },
        bytes,
    ))
}

impl CollSource {
    /// Reads `coll` tag `tag`. Its blocks must lie in the ordinary
    /// metadata, where every `coll` in a map is.
    pub fn read<R: Read + Seek>(map: &mut Map<R>, tag: TagId) -> Result<Self, Error> {
        let missing = || Error::NoTarget(Target::new(TargetKind::Coll, tag, 0).id());
        let t = map.tag(tag).map_err(|_| missing())?.clone();
        if t.group != COLL {
            return Err(missing());
        }
        let loc = t.location.ok_or_else(missing)?;
        if Some(loc.space) != map.metadata_space() {
            return Err(refused(tag, 0, "its blocks are not in the ordinary metadata"));
        }
        let collision = Collision::read(map, tag)?;
        let root = loc.address;
        let mut sites = Vec::new();
        let (regions, region_at) = pair(map, root + 0x1c)?;
        for ri in 0..regions as usize {
            let (perms, perm_at) = pair(map, region_at + (ri * 12 + 4) as u32)?;
            for pi in 0..perms as usize {
                let p = perm_at + (pi * 0x14) as u32;
                let (bsps, bsp_at) = pair(map, p + 4)?;
                let (slots, slot_at) = pair(map, p + 0xc)?;
                for bi in 0..bsps as usize {
                    let element = bsp_at + (bi * BSP_SIZE) as u32;
                    let mut blocks = [BlockAt {
                        space: None,
                        count: 0,
                        address: 0,
                    }; 8];
                    let mut block_bytes: [Vec<u8>; 8] = Default::default();
                    for (k, stride) in BSP_STRIDES.iter().enumerate() {
                        (blocks[k], block_bytes[k]) = block(map, element + 4 + 8 * k as u32, *stride)?;
                    }
                    let slot = if (bi as u32) < slots {
                        let address = slot_at + (bi * BSP_PHYSICS_SIZE) as u32;
                        let (mopp, mopp_bytes) = block(map, address + 0x64, 1)?;
                        Some(SlotAt {
                            address,
                            mopp,
                            mopp_bytes,
                        })
                    } else {
                        None
                    };
                    sites.push(BspSite {
                        at: BspAt {
                            region: ri,
                            permutation: pi,
                            bsp: bi,
                        },
                        element,
                        blocks,
                        block_bytes,
                        slot,
                    });
                }
            }
        }
        if sites.len() != collision.bsps().count() {
            return Err(refused(tag, 0, "the tag's blocks do not walk to the BSPs it decodes to"));
        }
        Ok(Self {
            tag,
            collision,
            sites,
        })
    }

    /// BSP `index` as a polygon mesh.
    pub fn mesh(&self, index: usize) -> Result<Mesh, Error> {
        let bsp = self
            .collision
            .bsps()
            .nth(index)
            .ok_or_else(|| Error::NoTarget(Target::new(TargetKind::Coll, self.tag, index as u32).id()))?;
        coll::mesh_from_bsp(bsp).map_err(|e| refused(self.tag, index, e.to_string()))
    }

    /// What a save writes for BSP `index` compiled from `polygons`, or why
    /// it cannot be compiled.
    pub fn writes(&self, index: usize, polygons: &[Polygon]) -> Result<LevelWrites, String> {
        let site = self.sites.get(index).ok_or("no such BSP")?;
        let mut tag = self.collision.clone();
        coll::rebuild(&mut tag, site.at, polygons, &Options::default()).map_err(|e| e.to_string())?;
        let perm = &tag.regions[site.at.region].permutations[site.at.permutation];
        let mut out = LevelWrites::default();
        for (k, b) in coll::stage_blocks(&perm.bsps[site.at.bsp]).into_iter().enumerate() {
            out.block(
                b.name,
                (site.blocks[k], &site.block_bytes[k]),
                (None, site.element + 4 + 8 * k as u32),
                (b.count, &b.bytes),
            );
        }
        if let Some(slot) = &site.slot {
            let new = &perm.bsp_physics[site.at.bsp];
            let old = &self.collision.regions[site.at.region].permutations[site.at.permutation].bsp_physics
                [site.at.bsp];
            for (offset, was, now) in [
                (0x10, f3(old.center), f3(new.center)),
                (0x20, f3(old.half_extents), f3(new.half_extents)),
            ] {
                for (run_at, bytes) in changed_runs(&was, &now) {
                    out.patches.push(RegionWrite {
                        space: None,
                        address: slot.address + offset + run_at as u32,
                        was: was[run_at..run_at + bytes.len()].to_vec(),
                        bytes,
                    });
                }
            }
            out.mopp((slot.mopp, &slot.mopp_bytes), (None, slot.address + 0x64), &new.mopp);
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "coll_target_tests.rs"]
mod tests;
