use tags::anim_keys::{Kind, distance};
use tags::anim_sample::{NodeDefault, NodeTransform, Pose};
use tags::anim_write::q16;

use crate::{AnimDoc, normalize};

#[derive(Debug, PartialEq)]
pub struct NodeCheck {
    pub node: usize,
    /// Radians.
    pub rotation: f32,
    pub translation: f32,
    pub scale: f32,
}

impl NodeCheck {
    fn new(node: usize) -> Self {
        Self {
            node,
            rotation: 0.0,
            translation: 0.0,
            scale: 0.0,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct FootContact {
    pub node: usize,
    pub frame: usize,
    pub z: f32,
}

#[derive(Debug, PartialEq)]
pub struct Check {
    /// Worst error over every frame, per node, using the AN2 encoder's
    /// quantisation. Both codecs store translation and scale as floats.
    pub nodes: Vec<NodeCheck>,
    pub feet_checked: bool,
    pub feet_below_ground: Vec<FootContact>,
    /// The first-to-last local transform differences by node.
    pub loop_seam: Vec<NodeCheck>,
}

fn quantized(q: [f32; 4]) -> [f32; 4] {
    let bytes = q16(q);
    let encoded =
        std::array::from_fn(|i| f32::from(i16::from_le_bytes([bytes[2 * i], bytes[2 * i + 1]])));
    normalize(encoded).expect("encoding a unit rotation retains a nonzero component")
}

fn translation(n: &NodeTransform) -> [f32; 4] {
    let [x, y, z] = n.translation;
    [x, y, z, 0.0]
}

impl AnimDoc {
    /// Check the codec's quantisation, the seam, and named foot nodes on
    /// the model skeleton when it is available. An overlay is composed at
    /// full weight over rest for the ground check.
    pub fn check(&self, defaults: Option<&[NodeDefault]>, overlay: bool) -> Result<Check, String> {
        let feet: Vec<_> = self
            .nodes()
            .iter()
            .enumerate()
            .filter_map(|(i, n)| {
                n.name.as_ref().and_then(|name| {
                    let name = name.to_ascii_lowercase();
                    (name.contains("foot") || name.contains("feet")).then_some(i)
                })
            })
            .collect();
        let parents: Vec<_> = defaults
            .map(|d| d.iter().map(|n| n.parent).collect())
            .unwrap_or_default();
        let mut check = Check {
            nodes: (0..self.nodes().len()).map(NodeCheck::new).collect(),
            feet_checked: defaults.is_some() && !feet.is_empty(),
            feet_below_ground: Vec::new(),
            loop_seam: Vec::with_capacity(self.nodes().len()),
        };
        let rest = match (overlay, defaults) {
            (true, Some(defaults)) => Some(
                Pose {
                    nodes: vec![NodeTransform::default(); self.nodes().len()],
                }
                .merged(defaults)
                .map_err(|err| err.to_string())?,
            ),
            _ => None,
        };
        let compose = |sample: &Pose, defaults: &[NodeDefault]| -> Result<Pose, String> {
            if let Some(rest) = &rest {
                let mut composed = rest.clone();
                composed
                    .add_overlay(sample, 1.0)
                    .map_err(|err| err.to_string())?;
                Ok(composed)
            } else {
                sample.merged(defaults).map_err(|err| err.to_string())
            }
        };
        for frame in 0..self.frames() {
            let sample = self.sample(frame as f32);
            for (check, node) in check.nodes.iter_mut().zip(&sample.nodes) {
                if node.has_rotation {
                    check.rotation = check.rotation.max(distance(
                        Kind::Rotation,
                        node.rotation,
                        quantized(node.rotation),
                    ));
                }
            }
            if let Some(defaults) = defaults.filter(|_| check.feet_checked) {
                let worlds = compose(&sample, defaults)?
                    .world_matrices(&parents)
                    .map_err(|err| err.to_string())?;
                for &node in &feet {
                    let z = worlds[node][3][2];
                    if z < 0.0 {
                        check.feet_below_ground.push(FootContact { node, frame, z });
                    }
                }
            }
        }
        let seam_pose = |frame| {
            let sample = self.sample(frame);
            match defaults {
                Some(defaults) => compose(&sample, defaults),
                None => Ok(sample),
            }
        };
        let first = seam_pose(0.0)?;
        let last = seam_pose((self.frames() - 1) as f32)?;
        check.loop_seam = first
            .nodes
            .iter()
            .zip(&last.nodes)
            .enumerate()
            .map(|(node, (a, b))| NodeCheck {
                node,
                rotation: distance(Kind::Rotation, a.rotation, b.rotation),
                translation: distance(Kind::Translation, translation(a), translation(b)),
                scale: (a.scale - b.scale).abs(),
            })
            .collect();
        Ok(check)
    }
}
