//! The Flood pod's kit: the pod a Firefight Flood squad arrives in
//! (docs/game/firefight.md, "The Flood pod"), built from a parametric
//! source and a painted atlas under `content/flood-pod/` into the tags a map
//! holds it by.
//!
//! - [`source`]: the YAML source and the atlas, embedded, and the checks a
//!   pod's source passes.
//! - [`kit`]: the pod as `bitm`, `shad`, `mode`, `phmo`, `hlmt` and `vehi`
//!   in a map's tag set.
//! - [`inject`]: the kit added to a multiplayer map's bytes.
//!
//! The primitives (lathes, sweeps, boxes), the mesher and the atlas's
//! `bitm` writer are the Pogo Stick's (`crate::{source, mesh, texture}`).
//!
//! The pod has one rig ([`rig`]): a `shell` node and four `petal` nodes
//! hinged on it, which the game turns to open the pod; the model is drawn
//! shut. Its values, tag path, version, material and the rest are the
//! manifest's (`content/flood-pod/kit.yaml`, [`manifest`]).

use crate::atlas::Manifest;

pub mod inject;
pub mod kit;
pub mod source;

#[cfg(test)]
mod tests;

/// The item manifest the pod is built from, `content/flood-pod/kit.yaml`.
pub fn manifest() -> &'static Manifest {
    source::manifest()
}

/// The pod's tags' path, the same for each group.
pub fn tag_path() -> &'static str {
    manifest().tag_path()
}

/// The root node: the root bulb, the collar, the sac, the ribs and the
/// tendrils.
pub fn node_shell() -> &'static str {
    &manifest().nodes[0].name
}

/// How many petals the shell has, and how many squad exits: the manifest's
/// nodes after the shell.
pub fn petals() -> usize {
    manifest().nodes.len() - 1
}

/// Petal `k`'s node: `petal_0` on +X, then anticlockwise seen from above
/// (+Y, -X, -Y).
pub fn petal_node(k: usize) -> String {
    manifest().nodes[1 + k].name.clone()
}

/// The marker at the point of the tip, where the pod meets the ground:
/// the model's origin.
pub fn marker_impact() -> &'static str {
    &manifest().markers[0].name
}

/// Squad exit `k`'s marker: a place on the ground beside the pod, between
/// petals `k` and `k + 1`, where a member of the squad is put.
pub fn squad_exit_marker(k: usize) -> String {
    manifest().markers[1 + k].name.clone()
}

/// Where the pod's moving parts are, model space, world units: +X forward,
/// +Y left, +Z up, the origin the point of the tip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rig {
    /// How far a petal's hinge is from the pod's axis, and how high.
    pub hinge_radius: f32,
    pub hinge_height: f32,
    /// How far a squad exit is from the pod's axis, on the ground.
    pub exit_radius: f32,
    /// The angle a petal turns about its hinge from shut to open, radians.
    pub open: f32,
}

/// The pod's rig: the manifest's `rig.values`.
pub fn rig() -> Rig {
    let m = manifest();
    Rig {
        hinge_radius: m.value("hinge_radius"),
        hinge_height: m.value("hinge_height"),
        exit_radius: m.value("exit_radius"),
        open: m.value("open"),
    }
}

impl Rig {
    /// The way petal `k` is from the axis: a unit vector on the ground.
    pub fn petal_direction(&self, k: usize) -> [f32; 3] {
        quarter(k as f32)
    }

    /// Petal `k`'s node, in the shell.
    pub fn hinge(&self, k: usize) -> [f32; 3] {
        let d = self.petal_direction(k);
        [
            d[0] * self.hinge_radius,
            d[1] * self.hinge_radius,
            self.hinge_height,
        ]
    }

    /// The axis petal `k` turns about, through its node: a right-handed
    /// turn of [`Rig::open`] about it lays the petal outward and down.
    pub fn hinge_axis(&self, k: usize) -> [f32; 3] {
        quarter(k as f32 + 1.0)
    }

    /// Squad exit `k`, on the ground between petals `k` and `k + 1`.
    pub fn squad_exit(&self, k: usize) -> [f32; 3] {
        let d = quarter(k as f32 + 0.5);
        [d[0] * self.exit_radius, d[1] * self.exit_radius, 0.0]
    }
}

/// The unit vector `quarters` quarter turns anticlockwise from +X.
fn quarter(quarters: f32) -> [f32; 3] {
    let (sin, cos) = (quarters * std::f32::consts::FRAC_PI_2).sin_cos();
    [cos, sin, 0.0]
}
