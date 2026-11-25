//! Private display pages from one owner-disc lighting split. Filenames are
//! derived from numeric bindings. The manifest supplies no filesystem paths.
use serde::{Deserialize, Serialize};

use crate::Sha256;

pub const DIRECTORY: &str = "relighting";
pub const MANIFEST: &str = "lightmaps.json";
pub const FORMAT: &str = "h2-display-lightmaps-v1";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub map: String,
    pub map_checksum: u32,
    pub bsp: u16,
    pub structure_checksum: u32,
    pub placed_lamp: u32,
    pub levels: Vec<Level>,
}

/// Digests cover decoded, unpadded RGBA8 rows, including the original alpha.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Level {
    pub page: u16,
    pub palette: u8,
    pub mip: u32,
    pub width: u32,
    pub height: u32,
    pub retail: Sha256,
    pub on: Sha256,
    pub off: Sha256,
}

pub fn page_stem(page: u16, palette: u8, mip: u32) -> String {
    format!("g0-page{page}-pal{palette}-mip{mip}")
}
