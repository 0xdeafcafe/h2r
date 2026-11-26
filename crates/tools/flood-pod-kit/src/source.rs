//! The pod's source: `art/flood-pod/flood_pod.yaml`, in the Pogo Stick's
//! primitive YAML (`pogo_kit::source`: nodes, markers, and parts built from
//! lathes, sweeps and boxes), and its atlas, `art/flood-pod/flood_pod.png`.
//!
//! Every position is in model space: world units, +X forward, +Y left, +Z
//! up, the origin the point of the tip ([`crate::RIG`]). A part's vertices
//! move with the one node it names.

use anyhow::{Context, Result, bail, ensure};
use pogo_kit::mesh::{self, Mesh};
pub use pogo_kit::source::Source;
use pogo_kit::texture::{self, Rgba};

use crate::{MARKER_IMPACT, NODE_SHELL, PETALS, RIG, petal_node, squad_exit_marker};

/// The model's name and its atlas's.
pub const NAME: &str = "flood_pod";

/// The atlas size, pixels.
pub const ATLAS: (u16, u16) = (512, 256);

/// The atlas regions a part names, in pixels (x0, y0, x1, y1): the table
/// `scripts/flood-pod-atlas.py` paints and `art/flood-pod/README.md` lists.
pub const REGIONS: [(&str, [u16; 4]); 6] = [
    ("flesh", [0, 0, 256, 128]),
    ("petal", [256, 0, 512, 128]),
    ("sac", [0, 128, 128, 256]),
    ("bone", [128, 128, 256, 256]),
    ("rib", [256, 128, 384, 256]),
    ("tendril", [384, 128, 512, 256]),
];

/// Texels kept clear at a region's edge, so a filtered sample stays in it.
pub const INSET: f32 = 2.0;

/// The most triangles the pod has.
pub const MAX_TRIANGLES: usize = 2000;

/// How near its rig's place a node or marker is.
const PLACE_EPSILON: f32 = 1e-3;

/// A region's rectangle in texture coordinates (u0, v0, u1, v1), inset.
pub fn region(name: &str) -> Option<[f32; 4]> {
    let (_, [x0, y0, x1, y1]) = REGIONS.iter().find(|(n, _)| *n == name)?;
    let (w, h) = (f32::from(ATLAS.0), f32::from(ATLAS.1));
    Some([
        (f32::from(*x0) + INSET) / w,
        (f32::from(*y0) + INSET) / h,
        (f32::from(*x1) - INSET) / w,
        (f32::from(*y1) - INSET) / h,
    ])
}

/// The source text, as built in.
pub fn text() -> &'static str {
    include_str!("../../../../art/flood-pod/flood_pod.yaml")
}

/// The atlas, as built in: the PNG file's bytes (a Git LFS pointer in a
/// checkout without LFS, which `pogo_kit::texture::decode` names).
pub fn atlas_png() -> &'static [u8] {
    include_bytes!("../../../../art/flood-pod/flood_pod.png")
}

/// The built-in atlas's pixels.
pub fn atlas() -> Result<Rgba> {
    texture::decode(atlas_png()).context("flood_pod.png")
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= PLACE_EPSILON)
}

/// Parses and checks a pod's source: the shell first, at the origin, then
/// the four petals, each a child of the shell at its hinge; the impact
/// marker and the four squad exits, on the shell at the rig's places;
/// every part on a node the source has and in one of [`REGIONS`].
pub fn parse(text: &str) -> Result<Source> {
    let s: Source = serde_yaml_ng::from_str(text).context("the pod's source")?;
    ensure!(
        s.nodes.len() == 1 + PETALS,
        "{}: {} nodes; a pod has the shell and {PETALS} petals",
        s.name,
        s.nodes.len()
    );
    let shell = &s.nodes[0];
    ensure!(
        shell.name == NODE_SHELL && shell.parent.is_none() && near(shell.at, [0.0; 3]),
        "{}: node 0 is the root, {NODE_SHELL}, at the origin",
        s.name
    );
    for k in 0..PETALS {
        let (n, name) = (&s.nodes[1 + k], petal_node(k));
        ensure!(
            n.name == name && n.parent.as_deref() == Some(NODE_SHELL),
            "{}: node {} is {name}, a child of {NODE_SHELL}",
            s.name,
            1 + k
        );
        ensure!(
            near(n.at, RIG.hinge(k)),
            "{}: {name} is at {:?}; its hinge is at {:?}",
            s.name,
            n.at,
            RIG.hinge(k)
        );
    }
    let mut want = vec![(MARKER_IMPACT.to_owned(), [0.0; 3])];
    want.extend((0..PETALS).map(|k| (squad_exit_marker(k), RIG.squad_exit(k))));
    for (name, at) in &want {
        let found: Vec<_> = s.markers.iter().filter(|m| m.name == *name).collect();
        let [m] = found.as_slice() else {
            bail!(
                "{}: {} markers named {name}; a pod has one",
                s.name,
                found.len()
            );
        };
        ensure!(
            m.node == NODE_SHELL && near(m.at, *at),
            "{}: marker {name} is on {} at {:?}; it is on {NODE_SHELL} at {at:?}",
            s.name,
            m.node,
            m.at
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

/// The built-in source.
pub fn builtin() -> Result<Source> {
    let s = parse(text())?;
    ensure!(
        s.name == NAME && s.atlas == NAME,
        "the pod's source is {:?} on atlas {:?}, want {NAME:?}",
        s.name,
        s.atlas
    );
    Ok(s)
}

/// A source's triangles on the pod's atlas, within [`MAX_TRIANGLES`].
pub fn build(source: &Source) -> Result<Mesh> {
    let mesh = mesh::build_on(source, &region)?;
    ensure!(
        mesh.triangles() <= MAX_TRIANGLES,
        "{}: {} triangles; a pod has {MAX_TRIANGLES} at most",
        source.name,
        mesh.triangles()
    );
    Ok(mesh)
}
