//! A structure BSP's level collision with a solid added
//! (docs/editor/modeling.md, slice 11; docs/editor/modeling-targets.md,
//! "Structure collision", step 2), as plain data: no map is read or
//! written here. `geometry_edit::level_collision` reads a [`Level`] out of
//! a map and writes an [`Added`] back; the live path stages
//! [`Added::stage`] (docs/editor/realtime-edits.md, "Collision").
//!
//! What changes when a solid is added:
//!
//! - **The collision BSP** (`sbsp+0x14`): the polygons compile to their own
//!   BSP ([`crate::build_with`]) and [`crate::merge`] appends it, so every
//!   existing index holds.
//! - **The per-leaf block** (`sbsp+0x2c`, [`LEAF_SIZE`] bytes a leaf, the
//!   cluster an `i16` at `+0`): each added leaf takes a copy of its
//!   stand-in's whole element, so it is in the stand-in's cluster and
//!   keeps whatever else retail stores beside it.
//! - **The structure MOPP** (`sbsp+0x1e0`): rebuilt over every structure
//!   surface of the merged BSP, plus every other key of the old program
//!   (instanced geometry and the other key types) as the box its old
//!   path guarantees (`mopp::bounds::key_bounds`), as the disc test
//!   `replaced_retail_levels` (`crates/formats/mopp/tests/disc.rs`) does.
//!   A level with no program gets one over its structure surfaces.
//!
//! The structure checksum (`sbsp+0x08`) is not a hash of the collision and
//! is kept: lightmap groups and clusters repeat it.

use mopp::geom::{STRUCTURE_KEY_TYPE, Surface, structure_surfaces};
use tags::collision::Bsp;

use crate::coll::{StagedBlock, stage_blocks};
use crate::merge::{Merged, merge_with};
use crate::{Compiled, Error, Options, Polygon, Result, build_with};

/// Size of one element of the per-leaf block at `sbsp+0x2c`.
pub const LEAF_SIZE: usize = 8;

/// A level's collision as a structure BSP holds it.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub bsp: Bsp,
    /// The per-leaf block's elements: [`LEAF_SIZE`] bytes per leaf of
    /// `bsp`.
    pub leaves: Vec<u8>,
    /// The `sbsp+0x1e0` block's bytes; empty when the level has none.
    pub mopp: Vec<u8>,
    /// Elements of the structure's collision material block (`sbsp+0x0c`):
    /// every surface's material is below this.
    pub materials: usize,
}

/// A level with a solid added.
#[derive(Clone, Debug, PartialEq)]
pub struct Added {
    pub level: Level,
    pub merged: Merged,
    /// The solid on its own, with each surface's source polygon.
    pub piece: Compiled,
}

/// What `edit.collision.stage` takes for an added solid: the eight blocks
/// by name, the per-leaf block and the MOPP code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelStage {
    /// The collision BSP's eight blocks, in [`crate::coll::BLOCK_NAMES`]
    /// order.
    pub blocks: Vec<StagedBlock>,
    /// The per-leaf block (`leaves` in the request), [`LEAF_SIZE`] bytes an
    /// element.
    pub leaves: StagedBlock,
    /// The `sbsp+0x1e0` block (`mopp` in the request): element size 1.
    pub mopp: Vec<u8>,
}

impl Level {
    /// Checks the per-leaf block holds one element per leaf.
    pub(crate) fn check(&self) -> Result<()> {
        if self.leaves.len() != self.bsp.leaves.len() * LEAF_SIZE {
            return Err(Error::Invalid(format!(
                "the per-leaf block holds {} bytes for {} leaves",
                self.leaves.len(),
                self.bsp.leaves.len()
            )));
        }
        Ok(())
    }

    /// Compiles `polygons` (materials indexing the structure's collision
    /// materials) and appends them to the level. Refused, with nothing
    /// changed, when a material is past the list, the solid reaches no
    /// open space of the level, a limit is passed, or the merged BSP
    /// fails `Bsp::validate`.
    pub fn add_solid(&self, polygons: &[Polygon], opts: &Options) -> Result<Added> {
        self.check()?;
        let piece = build_with(polygons, opts)?;
        if let Some((surface, s)) = piece
            .bsp
            .surfaces
            .iter()
            .enumerate()
            .find(|(_, s)| usize::from(s.material) >= self.materials)
        {
            return Err(Error::Material {
                surface,
                material: s.material,
                materials: self.materials,
            });
        }
        let mut bsp = self.bsp.clone();
        let merged = merge_with(&mut bsp, &piece.bsp, &opts.limits)?;
        if merged.slots == 0 {
            return Err(Error::Invalid(
                "the solid reaches no open space of the level".into(),
            ));
        }
        if let Some(problem) = bsp.validate(self.materials).into_iter().next() {
            return Err(Error::Invalid(format!("the merged level does not validate: {problem}")));
        }
        let mut leaves = self.leaves.clone();
        for &host in &merged.stand_ins {
            let at = host * LEAF_SIZE;
            let element = self
                .leaves
                .get(at..at + LEAF_SIZE)
                .ok_or_else(|| Error::Invalid(format!("no per-leaf element for leaf {host}")))?;
            leaves.extend_from_slice(element);
        }
        let mopp = rebuild_mopp(&bsp, &self.mopp)?;
        Ok(Added {
            level: Level {
                bsp,
                leaves,
                mopp,
                materials: self.materials,
            },
            merged,
            piece,
        })
    }
}

/// The structure MOPP over `bsp`'s surfaces, carrying every key of `old`
/// (the current `sbsp+0x1e0` block, or empty) that is not a structure
/// surface as the box its old path guarantees.
pub fn rebuild_mopp(bsp: &Bsp, old: &[u8]) -> Result<Vec<u8>> {
    let mut inputs = structure_surfaces(bsp);
    if !old.is_empty() {
        let code = mopp::Code::parse(old).map_err(|e| Error::Invalid(format!("the level's MOPP: {e}")))?;
        let bounds = mopp::bounds::key_bounds(&code)
            .map_err(|e| Error::Invalid(format!("the level's MOPP does not run: {e:?}")))?;
        for (key, b) in bounds {
            if key >> 29 != STRUCTURE_KEY_TYPE {
                inputs.push(Surface {
                    key,
                    vertices: vec![b.min, b.max],
                });
            }
        }
    }
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    Ok(mopp::build(&inputs).to_block())
}

impl Added {
    /// The blocks `edit.collision.stage` sends for this level, in the
    /// bytes a save writes.
    pub fn stage(&self) -> LevelStage {
        self.level.stage()
    }
}

impl Level {
    /// This level's blocks as `edit.collision.stage` takes them.
    pub fn stage(&self) -> LevelStage {
        LevelStage {
            blocks: stage_blocks(&self.bsp),
            leaves: StagedBlock {
                name: "leaves",
                count: (self.leaves.len() / LEAF_SIZE) as u32,
                bytes: self.leaves.clone(),
            },
            mopp: self.mopp.clone(),
        }
    }
}
