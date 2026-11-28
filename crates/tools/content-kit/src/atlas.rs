//! The item manifest: `content/<item>/kit.yaml`, the data every item kit
//! shares. An item is a parametric source (nodes, markers, lathes, sweeps
//! and boxes, [`crate::source`]), a painted atlas (PNG) and this manifest,
//! which names the atlas's size, the texels kept clear at a region's edge
//! and the regions a part may name, with every unknown key refused.
//!
//! The Pogo Stick (`content/pogo/kit.yaml`) and the Flood pod
//! (`content/flood-pod/kit.yaml`) are items; the builders that turn an item
//! into tags (`kit`, `inject`) read the manifest, never a table of their own.
//! Beside the atlas the manifest holds the item's kit version, tag paths,
//! material, region, bounding sphere, physics, rig, node and marker names
//! and the weapon class it is held as; every field but `item` and `atlas`
//! is optional, and an accessor names the field a builder needs when the
//! item has none.

use std::collections::BTreeMap;

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
    /// The kit's version: part of the string (`<item>_kit_v<n>`) a map
    /// carrying the kit interns.
    #[serde(default)]
    pub kit_version: Option<u32>,
    /// The tag path the item's tags are named by (the same for each group).
    #[serde(default)]
    pub tag_path: Option<String>,
    /// The folder under which an item with several looks names its tags.
    #[serde(default)]
    pub folder: Option<String>,
    /// The global material the item is made of, where the map has it.
    #[serde(default)]
    pub material: Option<String>,
    /// The one region and permutation of the item's model.
    #[serde(default)]
    pub region: Option<String>,
    /// The object's bounding sphere, model space.
    #[serde(default)]
    pub bounding: Option<Bounding>,
    /// The physics model's one sphere.
    #[serde(default)]
    pub physics: Option<Physics>,
    /// The unit's field of view, radians.
    #[serde(default)]
    pub field_of_view: Option<f32>,
    /// Where the item's moving parts and contact points are.
    #[serde(default)]
    pub rig: RigData,
    /// The nodes a source has, in order, each with its parent.
    #[serde(default)]
    pub nodes: Vec<NodeSpec>,
    /// The markers a source has, each on its node.
    #[serde(default)]
    pub markers: Vec<MarkerSpec>,
    /// The weapon class and type the item is held as.
    #[serde(default)]
    pub hold: Option<Hold>,
}

/// A bounding sphere, model space: its middle and radius.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bounding {
    pub offset: [f32; 3],
    pub radius: f32,
}

/// A physics model's one sphere.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Physics {
    pub centre: [f32; 3],
    pub radius: f32,
    /// Kilograms.
    pub mass: f32,
}

/// An item's rig: named points (model space, world units) and named
/// numbers (lengths, angles, counts).
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RigData {
    #[serde(default)]
    pub points: BTreeMap<String, [f32; 3]>,
    #[serde(default)]
    pub values: BTreeMap<String, f32>,
}

/// A node a source has: its name and its parent's, none for the root.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeSpec {
    pub name: String,
    #[serde(default)]
    pub parent: Option<String>,
}

/// A marker a source has: its name and its node's. Its place is the rig
/// point of the same name when the rig has one.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerSpec {
    pub name: String,
    pub node: String,
}

/// The weapon an item is held as: the player bipeds whose animation
/// graphs must hold it, the class and type preferred, and the animation
/// mode a unit on foot holds a weapon in.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hold {
    pub bipeds: Vec<String>,
    pub class: [String; 2],
    pub mode: String,
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
    fn need<'a, T>(&self, field: &str, v: &'a Option<T>) -> &'a T {
        match v {
            Some(v) => v,
            None => panic!("content/{}/kit.yaml has no `{field}`", self.item),
        }
    }

    /// The kit's version.
    pub fn kit_version(&self) -> u32 {
        *self.need("kit_version", &self.kit_version)
    }

    /// The string a map carrying this kit interns: `<item>_kit_v<n>`.
    pub fn marker_string(&self) -> String {
        format!("{}_kit_v{}", self.item, self.kit_version())
    }

    /// The tag path the item's tags are named by.
    pub fn tag_path(&self) -> &str {
        self.need("tag_path", &self.tag_path)
    }

    /// The folder an item's looks are named under.
    pub fn folder(&self) -> &str {
        self.need("folder", &self.folder)
    }

    /// The global material the item is made of.
    pub fn material(&self) -> &str {
        self.need("material", &self.material)
    }

    /// The one region and permutation of the item's model.
    pub fn region(&self) -> &str {
        self.need("region", &self.region)
    }

    /// The bounding sphere.
    pub fn bounding(&self) -> Bounding {
        *self.need("bounding", &self.bounding)
    }

    /// The physics sphere.
    pub fn physics(&self) -> Physics {
        *self.need("physics", &self.physics)
    }

    /// The unit's field of view, radians.
    pub fn field_of_view(&self) -> f32 {
        *self.need("field_of_view", &self.field_of_view)
    }

    /// The weapon class the item is held as.
    pub fn hold(&self) -> &Hold {
        self.need("hold", &self.hold)
    }

    /// A rig point by name.
    pub fn point(&self, name: &str) -> [f32; 3] {
        *self
            .rig
            .points
            .get(name)
            .unwrap_or_else(|| panic!("content/{}/kit.yaml: no rig point {name}", self.item))
    }

    /// A rig number by name.
    pub fn value(&self, name: &str) -> f32 {
        *self
            .rig
            .values
            .get(name)
            .unwrap_or_else(|| panic!("content/{}/kit.yaml: no rig value {name}", self.item))
    }

    /// The node names, in order.
    pub fn node_names(&self) -> Vec<&str> {
        self.nodes.iter().map(|n| n.name.as_str()).collect()
    }

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
        for (i, n) in m.nodes.iter().enumerate() {
            ensure!(
                m.nodes[..i].iter().all(|o| o.name != n.name),
                "{}: node {} twice",
                m.item,
                n.name
            );
            if let Some(p) = &n.parent {
                ensure!(
                    m.nodes[..i].iter().any(|o| &o.name == p),
                    "{}: node {} has parent {p}, which does not come before it",
                    m.item,
                    n.name
                );
            }
        }
        for k in &m.markers {
            ensure!(
                m.nodes.iter().any(|n| n.name == k.node),
                "{}: marker {} on no node of the manifest",
                m.item,
                k.name
            );
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
