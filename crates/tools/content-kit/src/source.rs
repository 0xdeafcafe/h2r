//! The models' source: `content/pogo/models/<style>.yaml`, a parametric description
//! (nodes, markers, and parts built from lathes, sweeps and boxes), read
//! with every unknown key refused. [`crate::mesh`] turns it into triangles.
//!
//! Every position in a source is in model space: the frame node's, world
//! units, +X forward, +Y left, +Z up, the origin the right grip
//! (the `rig` of the manifest). A part's vertices move with the one node it names.

use anyhow::{Context, bail, ensure};
use serde::Deserialize;

use std::sync::OnceLock;

use crate::Style;
use crate::atlas::Manifest;

/// The item manifest, `content/pogo/kit.yaml`: the atlas size and the
/// regions a part names ([`crate::atlas`]).
pub fn manifest() -> &'static Manifest {
    static M: OnceLock<Manifest> = OnceLock::new();
    M.get_or_init(|| {
        Manifest::parse(include_str!("../../../../content/pogo/kit.yaml"))
            .expect("content/pogo/kit.yaml is a manifest")
    })
}

/// The atlas size, pixels.
pub fn atlas_size() -> (u16, u16) {
    manifest().atlas.size()
}

/// The atlas regions a part names, in pixels (x0, y0, x1, y1): the table
/// `scripts/pogo-atlas.py` paints and `content/pogo/README.md` lists.
pub fn regions() -> Vec<(&'static str, [u16; 4])> {
    manifest().atlas.regions()
}

/// A region's rectangle in texture coordinates (u0, v0, u1, v1), inset.
pub fn region(name: &str) -> Option<[f32; 4]> {
    manifest().atlas.region(name)
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// The model's name (the `mode` tag's string ID).
    pub name: String,
    /// The atlas the parts' regions are in: the style's id.
    pub atlas: String,
    pub nodes: Vec<Node>,
    pub markers: Vec<Marker>,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub name: String,
    /// The parent node's name; none for the root.
    #[serde(default)]
    pub parent: Option<String>,
    /// Where the node is, model space.
    pub at: [f32; 3],
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub name: String,
    pub node: String,
    /// Where the marker is, model space.
    pub at: [f32; 3],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shade {
    /// Neighbouring faces share normals unless they meet at a crease.
    #[default]
    Smooth,
    /// Every face has its own normal.
    Flat,
}

/// One part: in the file, a map with exactly one of the keys `lathe`,
/// `sweep` and `box`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "PartEntry")]
pub enum Part {
    Lathe(Lathe),
    Sweep(Sweep),
    Box(BoxPart),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PartEntry {
    #[serde(default)]
    lathe: Option<Lathe>,
    #[serde(default)]
    sweep: Option<Sweep>,
    #[serde(default, rename = "box")]
    boxed: Option<BoxPart>,
}

impl TryFrom<PartEntry> for Part {
    type Error = String;
    fn try_from(e: PartEntry) -> Result<Self, String> {
        match (e.lathe, e.sweep, e.boxed) {
            (Some(p), None, None) => Ok(Part::Lathe(p)),
            (None, Some(p), None) => Ok(Part::Sweep(p)),
            (None, None, Some(p)) => Ok(Part::Box(p)),
            _ => Err("a part is exactly one of lathe, sweep and box".into()),
        }
    }
}

/// A profile of (radius, distance along the axis) points turned about an
/// axis line.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lathe {
    /// What the part is, for the reader.
    #[serde(default)]
    pub what: String,
    pub node: String,
    pub region: String,
    /// A point on the axis.
    pub origin: [f32; 3],
    /// The axis direction.
    #[serde(default = "up")]
    pub axis: [f32; 3],
    pub segments: u32,
    pub profile: Vec<[f32; 2]>,
    /// The profile is a loop (a ring's section): its last point joins its
    /// first.
    #[serde(default)]
    pub closed: bool,
    /// Close an open profile's ends onto the axis.
    #[serde(default)]
    pub caps: bool,
    #[serde(default)]
    pub shade: Shade,
}

/// A section drawn in the plane across a path and carried along it.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sweep {
    #[serde(default)]
    pub what: String,
    pub node: String,
    pub region: String,
    pub section: Section,
    pub path: Vec<[f32; 3]>,
    /// The section's scale at each path point; 1 everywhere when empty.
    #[serde(default)]
    pub scales: Vec<f32>,
    /// Close both ends.
    #[serde(default = "yes")]
    pub caps: bool,
    #[serde(default)]
    pub shade: Shade,
}

/// A sweep's section: in the file, a map with exactly one of the keys
/// `circle` and `polygon`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(try_from = "SectionEntry")]
pub enum Section {
    Circle(Circle),
    /// Points going anticlockwise.
    Polygon(Vec<[f32; 2]>),
}

#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Circle {
    pub radius: f32,
    pub sides: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SectionEntry {
    #[serde(default)]
    circle: Option<Circle>,
    #[serde(default)]
    polygon: Option<Vec<[f32; 2]>>,
}

impl TryFrom<SectionEntry> for Section {
    type Error = String;
    fn try_from(e: SectionEntry) -> Result<Self, String> {
        match (e.circle, e.polygon) {
            (Some(c), None) => Ok(Section::Circle(c)),
            (None, Some(p)) => Ok(Section::Polygon(p)),
            _ => Err("a section is exactly one of circle and polygon".into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoxPart {
    #[serde(default)]
    pub what: String,
    pub node: String,
    pub region: String,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// The width cut off every edge; 0 leaves them sharp.
    #[serde(default)]
    pub bevel: f32,
}

fn up() -> [f32; 3] {
    [0.0, 0.0, 1.0]
}

fn yes() -> bool {
    true
}

impl Part {
    pub fn node(&self) -> &str {
        match self {
            Part::Lathe(p) => &p.node,
            Part::Sweep(p) => &p.node,
            Part::Box(p) => &p.node,
        }
    }

    pub fn region(&self) -> &str {
        match self {
            Part::Lathe(p) => &p.region,
            Part::Sweep(p) => &p.region,
            Part::Box(p) => &p.region,
        }
    }
}

/// A style's source text, as built in.
pub fn text(style: Style) -> &'static str {
    match style {
        Style::Unsc => include_str!("../../../../content/pogo/models/unsc.yaml"),
        Style::Covenant => include_str!("../../../../content/pogo/models/covenant.yaml"),
        Style::Forerunner => include_str!("../../../../content/pogo/models/forerunner.yaml"),
    }
}

/// A style's atlas, as built in: the PNG file's bytes (a Git LFS pointer
/// in a checkout without LFS, which [`crate::texture::decode`] names).
pub fn atlas(style: Style) -> &'static [u8] {
    match style {
        Style::Unsc => include_bytes!("../../../../content/pogo/models/unsc.png"),
        Style::Covenant => include_bytes!("../../../../content/pogo/models/covenant.png"),
        Style::Forerunner => include_bytes!("../../../../content/pogo/models/forerunner.png"),
    }
}

/// Parses and checks a source: the manifest's nodes in order, at most
/// (the root first, each next one the child the manifest names), every part
/// and marker on a node the source has, every region one of [`regions`].
pub fn parse(text: &str) -> anyhow::Result<Source> {
    let s: Source = serde_yaml_ng::from_str(text).context("the model source")?;
    let item = manifest();
    ensure!(
        !s.nodes.is_empty() && s.nodes.len() <= item.nodes.len(),
        "{}: {} nodes; the manifest has {} ({})",
        s.name,
        s.nodes.len(),
        item.nodes.len(),
        item.node_names().join(", ")
    );
    for (i, (n, want)) in s.nodes.iter().zip(&item.nodes).enumerate() {
        ensure!(
            n.name == want.name && n.parent == want.parent,
            "{}: node {i} is {} (parent {:?}), want {} (parent {:?})",
            s.name,
            n.name,
            n.parent,
            want.name,
            want.parent
        );
    }
    for m in &s.markers {
        ensure!(
            s.node(&m.node).is_some(),
            "{}: marker {} on no node",
            s.name,
            m.name
        );
    }
    for (i, p) in s.parts.iter().enumerate() {
        if s.node(p.node()).is_none() {
            bail!(
                "{}: part {i} on node {:?}, which the source lacks",
                s.name,
                p.node()
            );
        }
        if region(p.region()).is_none() {
            bail!(
                "{}: part {i} in region {:?}, which the atlas lacks",
                s.name,
                p.region()
            );
        }
    }
    Ok(s)
}

impl Source {
    /// The built-in source of `style`.
    pub fn builtin(style: Style) -> anyhow::Result<Source> {
        let s = parse(text(style))?;
        ensure!(
            s.atlas == style.id(),
            "{}: atlas {:?}, want {:?}",
            s.name,
            s.atlas,
            style.id()
        );
        Ok(s)
    }

    /// A node's index by name.
    pub fn node(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name == name)
    }

    /// A marker's place in model space.
    pub fn marker(&self, name: &str) -> Option<[f32; 3]> {
        self.markers.iter().find(|m| m.name == name).map(|m| m.at)
    }
}
