//! The collision compiler (docs/editor/modeling.md, slice 8, and
//! docs/editor/modeling-targets.md, "Structure collision", step 1).
//!
//! [`build`] turns convex, planar polygons into a Halo 2 collision BSP
//! (`tags::collision::Bsp`): the eight blocks a structure BSP holds at
//! `sbsp+0x14` and a `coll` permutation holds per BSP.
//!
//! - **Prepare** (`prep.rs`): weld vertices within [`Options::weld`]; drop
//!   polygons left with under three corners or no area; triangulate a
//!   polygon that is not planar or not convex; split a ring of more than
//!   eight vertices into fans of at most eight (retail rings have at most
//!   eight, and Havok's surface shape copies a ring with no count check).
//! - **Planes**: one unit plane per distinct plane; a surface names its
//!   plane with bit 15 set when it faces the plane's back.
//! - **Edges** (`edges.rs`): the winged-edge table, so `Bsp::surface_ring`
//!   gives each surface's ring in its input winding.
//! - **3D tree** (`tree.rs`): recursive partition on a surface's plane
//!   (fewest splits, then best balance, ties by lowest plane index),
//!   splitting straddling fragments. Fragments on the plane stay at the
//!   node. An empty side is solid when a one-sided fragment at the node
//!   faces away from it and none faces it, else an open leaf.
//! - **References and 2D trees** (`tree.rs`, `tree2d.rs`): each open leaf
//!   takes one reference per plane on its path where it borders surfaces
//!   seen from its side, its flip bit set when the leaf is on the plane's
//!   back, and a 2D tree over those surfaces in the plane's projection
//!   (`query::project`). Leaf flag bit 0 marks a leaf whose references
//!   reach a two-sided surface.
//!
//! Polygons wind counter-clockwise seen from their open side: the
//! right-hand normal points out of the solid. A two-sided polygon
//! ([`TWO_SIDED`]) is open on both sides.
//!
//! What the packed fields cannot hold is refused ([`Limits::HALO2`]).
//! [`verify::check`] is the acceptance: `Bsp::validate`, the centroid
//! walk, the reference side and the leaf flag rule; [`query`] is the
//! engine's walks, which tests run random segments through.
//!
//! [`coll`] opens a `coll` BSP as a `mesh_edit::Mesh`, compiles it back,
//! rebuilds the permutation's BSP physics slots and stages the eight
//! blocks for `edit.collision.stage` (docs/editor/realtime-edits.md,
//! "Collision"). [`from_render`] makes collision polygons from a render
//! section's triangles.
//!
//! [`merge`] appends a compiled solid to a level's BSP keeping every
//! existing index, and [`level`] adds a solid to a structure's level
//! collision with its per-leaf block and MOPP (docs/editor/modeling.md,
//! slice 11).

pub mod coll;
mod edges;
pub mod from_render;
mod geom;
pub mod level;
pub mod merge;
mod prep;
pub mod query;
mod tree;
mod tree2d;
pub mod verify;

use tags::collision::{Bsp, Plane, Surface, Vertex};

/// Surface flag bit 0: the surface is open on both sides (the Halo 1
/// meaning, which `h1convert::collision` copies unchanged).
pub const TWO_SIDED: u8 = 0x1;
/// A surface's breakable-surface byte when it has none.
pub const NO_BREAKABLE: u8 = 0xff;
/// Most vertices in one ring.
pub const MAX_RING: usize = 8;

/// One input polygon: convex and planar, wound counter-clockwise seen from
/// its open side.
#[derive(Clone, Debug, PartialEq)]
pub struct Polygon {
    pub points: Vec<[f32; 3]>,
    /// Index into the owning tag's material list.
    pub material: u16,
    /// Surface flags ([`TWO_SIDED`] and the bits the engine tests).
    pub flags: u8,
    /// The breakable-surface index, [`NO_BREAKABLE`] for none.
    pub breakable_surface: u8,
}

impl Polygon {
    /// A one-sided polygon with no breakable surface.
    pub fn new(points: Vec<[f32; 3]>, material: u16) -> Self {
        Self {
            points,
            material,
            flags: 0,
            breakable_surface: NO_BREAKABLE,
        }
    }

    pub fn two_sided(mut self) -> Self {
        self.flags |= TWO_SIDED;
        self
    }
}

/// The counts a collision BSP's packed fields can hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Surfaces: a 2D child holds 15 bits and `0x7fff` is its "none".
    pub surfaces: usize,
    /// Edges: `u16` indices.
    pub edges: usize,
    /// Vertices: `u16` indices.
    pub vertices: usize,
    /// Planes: 15 bits beside a surface's or reference's flip bit.
    pub planes: usize,
    /// 2D nodes: 15 bits beside the surface bit.
    pub nodes_2d: usize,
    /// 2D references: a leaf's `u16` first reference, `0xffff` none.
    pub refs: usize,
    /// 3D nodes: 23 bits of a 24-bit child.
    pub nodes: usize,
    /// Leaves: 23 bits of a 24-bit child; leaf `0x7fffff` would encode as
    /// the child "none" (`0xffffff`).
    pub leaves: usize,
    /// References of one leaf: a `u8` count.
    pub leaf_refs: usize,
}

impl Limits {
    /// What Halo 2's packed fields hold.
    pub const HALO2: Limits = Limits {
        surfaces: 0x7fff,
        edges: 0xffff,
        vertices: 0xffff,
        planes: 0x8000,
        nodes_2d: 0x8000,
        refs: 0xffff,
        nodes: 0x80_0000,
        leaves: 0x7f_ffff,
        leaf_refs: 0xff,
    };

    pub(crate) fn check(&self, what: &'static str, value: usize) -> Result<()> {
        let max = match what {
            "surface" => self.surfaces,
            "edge" => self.edges,
            "vertex" => self.vertices,
            "plane" => self.planes,
            "2D node" => self.nodes_2d,
            "2D reference" => self.refs,
            "3D node" => self.nodes,
            "leaf" => self.leaves,
            "leaf 2D reference" => self.leaf_refs,
            _ => usize::MAX,
        };
        if value > max {
            return Err(Error::TooBig { what, value, max });
        }
        Ok(())
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self::HALO2
    }
}

/// How [`build_with`] prepares its input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    /// Vertices closer than this (world units) become one.
    pub weld: f32,
    pub limits: Limits,
}

/// Welds within a millimetre-scale distance: Halo 2's world unit is about
/// three metres, so this is a few millimetres.
pub const DEFAULT_WELD: f32 = 1e-3;

impl Default for Options {
    fn default() -> Self {
        Self {
            weld: DEFAULT_WELD,
            limits: Limits::HALO2,
        }
    }
}

/// What [`build_with`] did to its input.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Input vertices merged into an earlier one.
    pub welded: usize,
    /// Input polygons dropped: under three distinct corners or no area.
    pub dropped: Vec<usize>,
    /// Input polygons triangulated (not planar or not convex).
    pub triangulated: Vec<usize>,
    /// Input polygons split into rings of at most eight.
    pub split: Vec<usize>,
}

/// A compiled BSP and where each surface came from.
#[derive(Clone, Debug, PartialEq)]
pub struct Compiled {
    pub bsp: Bsp,
    /// Per surface, the input polygon it is (or a piece of).
    pub surface_source: Vec<usize>,
    pub report: Report,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum Error {
    #[error("no polygon is left to compile")]
    Empty,
    #[error("polygon {0} has a coordinate that is not finite")]
    NonFinite(usize),
    #[error("{what} count {value} is over the limit of {max}")]
    TooBig {
        what: &'static str,
        value: usize,
        max: usize,
    },
    #[error("polygons {a} and {b} overlap on one plane")]
    Overlap { a: usize, b: usize },
    #[error("surface {surface} uses material {material}, past the tag's {materials}")]
    Material {
        surface: usize,
        material: u16,
        materials: usize,
    },
    #[error("{0}")]
    Invalid(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Compiles `polygons` with the default [`Options`].
pub fn build(polygons: &[Polygon]) -> Result<Compiled> {
    build_with(polygons, &Options::default())
}

/// Compiles `polygons` to a collision BSP. Nothing is returned but a
/// whole BSP or an error naming what failed.
pub fn build_with(polygons: &[Polygon], opts: &Options) -> Result<Compiled> {
    let prepared = prep::prepare(polygons, opts.weld)?;
    if prepared.surfaces.is_empty() {
        return Err(Error::Empty);
    }
    let limits = &opts.limits;
    limits.check("surface", prepared.surfaces.len())?;
    limits.check("vertex", prepared.points.len())?;
    limits.check("plane", prepared.planes.len())?;

    let edges = edges::build(&prepared)?;
    limits.check("edge", edges.edges.len())?;

    let source = |s: usize| prepared.surfaces[s].source;
    let tree = tree::build(&prepared, limits).map_err(|e| match e {
        Error::Overlap { a, b } => Error::Overlap {
            a: source(a),
            b: source(b),
        },
        e => e,
    })?;

    let bsp = Bsp {
        unknown_00: 0,
        nodes: tree.nodes,
        planes: prepared.planes.iter().map(|p| Plane(*p)).collect(),
        leaves: tree.leaves,
        refs: tree.refs,
        nodes_2d: tree.nodes_2d,
        surfaces: prepared
            .surfaces
            .iter()
            .enumerate()
            .map(|(i, s)| Surface {
                plane: s.plane as u16 | if s.flip { 0x8000 } else { 0 },
                first_edge: edges.first_edge[i],
                flags: s.flags,
                breakable_surface: s.breakable_surface,
                material: s.material,
            })
            .collect(),
        edges: edges.edges,
        vertices: prepared
            .points
            .iter()
            .zip(&edges.vertex_edge)
            .map(|(p, &e)| Vertex { point: *p, edge: e })
            .collect(),
    };
    Ok(Compiled {
        bsp,
        surface_source: prepared.surfaces.iter().map(|s| s.source).collect(),
        report: prepared.report,
    })
}

#[cfg(test)]
mod fixtures;
#[cfg(test)]
#[path = "build_tests.rs"]
mod build_tests;
#[cfg(test)]
#[path = "coll_tests.rs"]
mod coll_tests;
#[cfg(test)]
#[path = "merge_tests.rs"]
mod merge_tests;
