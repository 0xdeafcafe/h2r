//! The content kit: one tool that builds an item's content (`content/<item>/`:
//! parametric YAML sources, painted atlases and a manifest) into the tags a
//! map needs to hold it. The items are the Pogo Stick (this crate's root
//! modules, three looks, `content/pogo/`), the Flood pod ([`flood_pod`],
//! `content/flood-pod/`), the content stage ([`stage`], `content/stage/`:
//! layout and `kit.yaml`) and the Forge block-out pieces ([`blockout`],
//! `content/blockout/kit.yaml`, over the shapes of `forge-shapes` and the dev
//! tiles of `dev-texture`). Each item's `kit.yaml` is data the builders read
//! (`stage::data`, `blockout::data`, [`atlas`]), with every unknown key
//! refused. The data model every item shares is the manifest
//! ([`atlas`]), the source ([`source`]), the mesher ([`mesh`]) and the atlas
//! writer ([`texture`]); an item adds its rig and its tag layout.
//!
//! The Pogo Stick, three looks of one stick:
//!
//! - [`atlas`]: an item's manifest, `content/<item>/kit.yaml`: the atlas
//!   size, its regions and the item's limits.
//! - [`source`]: the YAML sources and atlases, embedded, and their parser.
//! - [`mesh`]: a source's lathes, sweeps and boxes as triangles.
//! - [`texture`]: an atlas as a `bitm`.
//! - [`kit`]: one look as `bitm`, `shad`, `mode`, `hlmt` and `weap` in a
//!   map's tag set.
//! - [`inject`]: the kit added to a multiplayer map's bytes (what the
//!   `pogo` package's build is compared with).
//! - [`served`]: which weapon tags a map the game was served holds.
//! - [`placed`]: which weapon tags the map being played holds.
//!
//! Every look shares one rig (`rig` in `content/pogo/kit.yaml`, and
//! `pogo::RIG` for the game that poses a rider): the same grips, pegs, foot
//! and piston travel, so the code that poses a rider reads no look.
//!
//! What is an item's own is data in its folder: the manifest
//! ([`atlas::Manifest`]) holds its kit version, tag paths, material, region,
//! bounding sphere, physics sphere, rig, node and marker names and the
//! weapon class it is held as. The Rust here reads them.

pub mod atlas;
pub mod blockout;
pub mod flood_pod;
pub mod inject;
pub mod kit;
pub mod mesh;
pub mod served;
pub mod source;
pub mod stage;
pub mod texture;

/// An invented map for other crates' tests (feature `fixture`).
#[cfg(any(test, feature = "fixture"))]
pub mod fixture;

#[cfg(test)]
mod inject_tests;
#[cfg(test)]
mod tests;

/// The maps never asked for: they are not levels.
pub(crate) const NEVER: [&str; 3] = ["mainmenu", "shared", "single_player_shared"];

pub use pogo::{Style, placed};
