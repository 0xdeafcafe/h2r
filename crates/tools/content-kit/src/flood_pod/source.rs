//! The pod's source: `content/flood-pod/flood_pod.yaml`, in the Pogo Stick's
//! primitive YAML (`crate::source`: nodes, markers, and parts built from
//! lathes, sweeps and boxes), and its atlas, `content/flood-pod/flood_pod.png`.
//!
//! Every position is in model space: world units, +X forward, +Y left, +Z
//! up, the origin the point of the tip ([`super::RIG`]). A part's vertices
//! move with the one node it names.

use anyhow::{Context, Result, bail, ensure};
use crate::mesh::{self, Mesh};
pub use crate::source::Source;
use crate::texture::{self, Rgba};
use crate::atlas::Manifest;
use std::sync::OnceLock;

use super::{MARKER_IMPACT, NODE_SHELL, PETALS, RIG, petal_node, squad_exit_marker};

/// The model's name and its atlas's.
pub const NAME: &str = "flood_pod";

/// The item manifest, `content/flood-pod/kit.yaml`: the atlas size, the
/// regions a part names and the most triangles the pod has
/// ([`crate::atlas`]).
pub fn manifest() -> &'static Manifest {
    static M: OnceLock<Manifest> = OnceLock::new();
    M.get_or_init(|| {
        Manifest::parse(include_str!("../../../../../content/flood-pod/kit.yaml"))
            .expect("content/flood-pod/kit.yaml is a manifest")
    })
}

/// The atlas size, pixels.
pub fn atlas_size() -> (u16, u16) {
    manifest().atlas.size()
}

/// The atlas regions a part names, in pixels (x0, y0, x1, y1): the table
/// `scripts/flood-pod-atlas.py` paints and `content/flood-pod/README.md`
/// lists.
pub fn regions() -> Vec<(&'static str, [u16; 4])> {
    manifest().atlas.regions()
}

/// The most triangles the pod has.
pub fn max_triangles() -> usize {
    manifest().max_triangles.expect("the pod's manifest bounds its triangles")
}

/// How near its rig's place a node or marker is.
const PLACE_EPSILON: f32 = 1e-3;

/// A region's rectangle in texture coordinates (u0, v0, u1, v1), inset.
pub fn region(name: &str) -> Option<[f32; 4]> {
    manifest().atlas.region(name)
}

/// The source text, as built in.
pub fn text() -> &'static str {
    include_str!("../../../../../content/flood-pod/flood_pod.yaml")
}

/// The atlas, as built in: the PNG file's bytes (a Git LFS pointer in a
/// checkout without LFS, which `crate::texture::decode` names).
pub fn atlas_png() -> &'static [u8] {
    include_bytes!("../../../../../content/flood-pod/flood_pod.png")
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
/// every part on a node the source has and in one of [`regions`].
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

/// A source's triangles on the pod's atlas, within [`max_triangles`].
pub fn build(source: &Source) -> Result<Mesh> {
    let mesh = mesh::build_on(source, &region)?;
    ensure!(
        mesh.triangles() <= max_triangles(),
        "{}: {} triangles; a pod has {} at most",
        source.name,
        mesh.triangles(),
        max_triangles()
    );
    Ok(mesh)
}
