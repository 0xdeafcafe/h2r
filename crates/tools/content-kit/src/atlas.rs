//! The item manifest: `content/<item>/kit.yaml`, the data every item kit
//! shares. An item is a parametric source (nodes, markers, lathes, sweeps
//! and boxes, [`crate::source`]), a painted atlas (PNG) and this manifest,
//! which names the atlas's size, the texels kept clear at a region's edge
//! and the regions a part may name, with every unknown key refused.
//!
//! The Pogo Stick (`content/pogo/kit.yaml`) and the Flood pod
//! (`content/flood-pod/kit.yaml`) are items; the builders that turn an item
//! into tags (`kit`, `inject`) read the manifest, never a table of their own.

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

/// One item's manifest.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// The item's id (its folder under `content/`).
    pub item: String,
    pub atlas: Atlas,
    /// The most triangles the item's mesh has; none for no limit.
    #[serde(default)]
    pub max_triangles: Option<usize>,
}

/// An atlas: its size in pixels and its named rectangles.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Atlas {
    pub width: u16,
    pub height: u16,
    /// Texels kept clear at a region's edge, so a filtered sample stays in it.
    pub inset: f32,
    pub regions: Vec<Region>,
}

/// A named rectangle of the atlas, in pixels.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub name: String,
    /// x0, y0, x1, y1.
    pub rect: [u16; 4],
}

impl Manifest {
    /// Parses and checks a manifest: every region inside the atlas, with
    /// area, named once, and none overlapping another.
    pub fn parse(text: &str) -> Result<Manifest> {
        let m: Manifest = serde_yaml_ng::from_str(text).context("the item manifest")?;
        let a = &m.atlas;
        ensure!(a.width > 0 && a.height > 0, "{}: an empty atlas", m.item);
        for (i, r) in a.regions.iter().enumerate() {
            let [x0, y0, x1, y1] = r.rect;
            ensure!(
                x0 < x1 && y0 < y1 && x1 <= a.width && y1 <= a.height,
                "{}: region {} is outside the atlas or empty",
                m.item,
                r.name
            );
            for o in &a.regions[i + 1..] {
                if o.name == r.name {
                    bail!("{}: region {} twice", m.item, r.name);
                }
                let [a0, b0, a1, b1] = o.rect;
                ensure!(
                    x1 <= a0 || a1 <= x0 || y1 <= b0 || b1 <= y0,
                    "{}: regions {} and {} overlap",
                    m.item,
                    r.name,
                    o.name
                );
            }
        }
        Ok(m)
    }
}

impl Atlas {
    /// The atlas size, pixels.
    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// The regions, in the order the manifest lists them.
    pub fn regions(&self) -> Vec<(&str, [u16; 4])> {
        self.regions.iter().map(|r| (r.name.as_str(), r.rect)).collect()
    }

    /// A region's rectangle in texture coordinates (u0, v0, u1, v1), inset.
    pub fn region(&self, name: &str) -> Option<[f32; 4]> {
        let r = self.regions.iter().find(|r| r.name == name)?;
        let [x0, y0, x1, y1] = r.rect;
        let (w, h) = (f32::from(self.width), f32::from(self.height));
        Some([
            (f32::from(x0) + self.inset) / w,
            (f32::from(y0) + self.inset) / h,
            (f32::from(x1) - self.inset) / w,
            (f32::from(y1) - self.inset) / h,
        ])
    }
}
