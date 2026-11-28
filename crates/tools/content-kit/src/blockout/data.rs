//! The block-out pieces' manifest, `content/blockout/kit.yaml`: the node,
//! region, crate type, collision material, dev folder, fixed body and
//! palette words a definition's tags are made with. Parsed once; an unknown
//! or missing key is refused. What is structure (tag field offsets, the
//! dev file's magic) stays in the builders.

use std::sync::OnceLock;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

/// The shipped manifest.
pub const SOURCE: &str = include_str!("../../../../../content/blockout/kit.yaml");

/// The block-out manifest.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockoutData {
    /// The item's id (its folder under `content/`).
    pub item: String,
    /// The one node of every piece.
    pub node: String,
    /// The one region and permutation of a piece's models.
    pub region: String,
    /// `obje.object_type` of a crate.
    pub crate_type: u32,
    /// The one collision material's name.
    pub collision_material: String,
    /// The tag folder of the dev set.
    pub dev_folder: String,
    pub physics: PhysicsData,
    pub palette: PaletteData,
}

/// A piece's fixed rigid body.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicsData {
    pub flags: u16,
    pub motion: u16,
    /// Kilograms.
    pub mass: f32,
}

/// The palette survey's words and bound.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaletteData {
    pub max_tile: u16,
    pub rock_and_ground: Vec<String>,
}

impl BlockoutData {
    /// Parses and checks a manifest.
    pub fn parse(text: &str) -> Result<BlockoutData> {
        let d: BlockoutData = serde_yaml_ng::from_str(text).context("the block-out manifest")?;
        ensure!(d.item == "blockout", "the block-out manifest is for item {}", d.item);
        ensure!(!d.node.is_empty() && !d.region.is_empty(), "a piece needs a node and a region");
        ensure!(!d.collision_material.is_empty() && !d.dev_folder.is_empty(), "an empty name");
        ensure!(d.physics.mass > 0.0, "a fixed body's mass must be positive");
        ensure!(d.palette.max_tile > 0, "the largest tile repeat is zero");
        Ok(d)
    }
}

/// The shipped manifest, parsed once.
pub fn data() -> &'static BlockoutData {
    static DATA: OnceLock<BlockoutData> = OnceLock::new();
    DATA.get_or_init(|| BlockoutData::parse(SOURCE).expect("content/blockout/kit.yaml parses"))
}
