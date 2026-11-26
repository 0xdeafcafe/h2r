//! The Forge monitors' bodies in the world (docs/game/
//! forge-monitor-appearance.md, ML4): each other seat's presentation model
//! at its pose, drawn into every game view over the world's colour and
//! depth by the frame graph (`frame_graph::bodies`).
//!
//! The game publishes the frame's [`Set`] ([`publish`]): the models it
//! holds (read from a campaign map at run time, [`BodyModel`]) and the
//! bodies to place. Nothing here reads guest memory: a body is its model
//! at a pose, in the model's rest pose, so a machine that draws none runs
//! the same game.

use crate::model::{Model, NodeMatrix};

/// One seat's body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Body {
    /// The model's index in [`Set::models`].
    pub model: usize,
    pub position: [f32; 3],
    /// Radians about z, then the pitch about the body's left axis
    /// (positive looks up): the pose's yaw and pitch.
    pub yaw: f32,
    pub pitch: f32,
    /// The colour the lit colour is mixed towards, by its alpha
    /// (`instances::ObjectInput::highlight`); `[0; 4]` draws it as lit.
    pub highlight: [u8; 4],
    /// A sphere around the body, for culling.
    pub sphere: ([f32; 3], f32),
}

/// The node matrix that puts a model at `body`'s pose: its basis is the
/// pose's forward, left and up, its scale one.
pub fn placement(body: &Body) -> NodeMatrix {
    let (sy, cy) = body.yaw.sin_cos();
    let (sp, cp) = body.pitch.sin_cos();
    let forward = [cy * cp, sy * cp, sp];
    let left = [-sy, cy, 0.0];
    // up = forward x left, so a level body's up is +z.
    let up = [
        forward[1] * left[2] - forward[2] * left[1],
        forward[2] * left[0] - forward[0] * left[2],
        forward[0] * left[1] - forward[1] * left[0],
    ];
    NodeMatrix {
        scale: 1.0,
        forward,
        left,
        up,
        position: body.position,
    }
}

/// The inverse of a node matrix (uniform scale, orthonormal axes).
pub fn inverse(m: &NodeMatrix) -> Option<NodeMatrix> {
    if !m.is_finite() || m.scale.abs() < 1e-9 {
        return None;
    }
    let s = 1.0 / m.scale;
    let forward = [m.forward[0], m.left[0], m.up[0]];
    let left = [m.forward[1], m.left[1], m.up[1]];
    let up = [m.forward[2], m.left[2], m.up[2]];
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let t = m.position;
    Some(NodeMatrix {
        scale: s,
        forward,
        left,
        up,
        position: [
            -s * dot(m.forward, t),
            -s * dot(m.left, t),
            -s * dot(m.up, t),
        ],
    })
}

/// The model's nodes with the body at `body`'s pose, every node in its rest
/// frame: the rest frame is the inverse of the model's inverse bind, so a
/// skinned vertex lands at the pose of the model's rest space. `None` when
/// an inverse bind has no inverse.
pub fn nodes(model: &Model, body: &Body) -> Option<Vec<NodeMatrix>> {
    let at = placement(body);
    model
        .inverse_binds
        .iter()
        .map(|bind| inverse(bind).map(|rest| at.compose(&rest)))
        .collect()
}

#[cfg(feature = "gpu")]
pub use published::*;

#[cfg(feature = "gpu")]
mod published {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::Body;
    use crate::gpu::Image;
    use crate::model::Model;

    /// A model the game holds for the bodies: the render model and each
    /// opaque material's base map (a material missing draws white).
    #[derive(Clone)]
    pub struct BodyModel {
        pub model: Model,
        pub bases: Vec<(u16, Image)>,
    }

    /// What the game published for the frame.
    #[derive(Clone, Default)]
    pub struct Set {
        pub models: Vec<Arc<BodyModel>>,
        pub bodies: Vec<Body>,
    }

    static PUBLISHED: Mutex<Option<Arc<Set>>> = Mutex::new(None);
    static HAS_PUBLISHED: AtomicBool = AtomicBool::new(false);

    /// The frame's bodies, or none. An empty set is none.
    pub fn publish(set: Option<Set>) {
        let set = set.filter(|s| !s.bodies.is_empty()).map(Arc::new);
        if set.is_none() && !HAS_PUBLISHED.swap(false, Ordering::AcqRel) {
            return;
        }
        HAS_PUBLISHED.store(set.is_some(), Ordering::Release);
        *PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()) = set;
    }

    pub fn published() -> Option<Arc<Set>> {
        if !HAS_PUBLISHED.load(Ordering::Acquire) {
            return None;
        }
        PUBLISHED.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ModelInput, ModelPartInput, ModelSectionInput};

    fn body(yaw: f32, pitch: f32, position: [f32; 3]) -> Body {
        Body {
            model: 0,
            position,
            yaw,
            pitch,
            highlight: [0; 4],
            sphere: (position, 1.0),
        }
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn a_level_body_at_zero_yaw_is_the_identity_moved_to_its_position() {
        let m = placement(&body(0.0, 0.0, [1.0, 2.0, 3.0]));
        assert!(near(m.forward, [1.0, 0.0, 0.0]));
        assert!(near(m.left, [0.0, 1.0, 0.0]));
        assert!(near(m.up, [0.0, 0.0, 1.0]));
        assert_eq!((m.scale, m.position), (1.0, [1.0, 2.0, 3.0]));
    }

    #[test]
    fn yaw_turns_it_about_z_and_pitch_lifts_its_nose() {
        let quarter = std::f32::consts::FRAC_PI_2;
        let m = placement(&body(quarter, 0.0, [0.0; 3]));
        assert!(near(m.forward, [0.0, 1.0, 0.0]));
        assert!(near(m.left, [-1.0, 0.0, 0.0]));
        assert!(near(m.up, [0.0, 0.0, 1.0]));
        let up = placement(&body(0.0, quarter, [0.0; 3]));
        assert!(near(up.forward, [0.0, 0.0, 1.0]), "looks up");
        assert!(near(up.up, [-1.0, 0.0, 0.0]), "its top turns back");
        // Orthonormal at any pose.
        let any = placement(&body(0.7, -0.4, [0.0; 3]));
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        assert!(dot(any.forward, any.left).abs() < 1e-5);
        assert!(dot(any.forward, any.up).abs() < 1e-5);
        assert!((dot(any.up, any.up) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn the_inverse_undoes_the_node() {
        let m = placement(&body(0.9, 0.3, [4.0, -2.0, 1.5]));
        let back = inverse(&m).expect("a rigid node inverts");
        let p = [0.3, -1.2, 2.5];
        assert!(near(back.point(m.point(p)), p));
        assert!(near(m.point(back.point(p)), p));
        let mut flat = m;
        flat.scale = 0.0;
        assert_eq!(inverse(&flat), None);
    }

    fn model_with_binds(binds: Vec<NodeMatrix>) -> Model {
        Model::build(&ModelInput {
            inverse_binds: binds,
            sections: vec![ModelSectionInput {
                positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                uvs: vec![[0.0; 2]; 3],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                parts: vec![ModelPartInput {
                    material: 0,
                    transparent: false,
                    triangles: vec![[0, 1, 2]],
                }],
                ..Default::default()
            }],
        })
    }

    #[test]
    fn a_nodes_rest_frame_under_the_pose_skins_the_rest_model_to_the_pose() {
        // A second node whose rest frame is 2 units up the model's z: its
        // inverse bind moves the other way.
        let rest = NodeMatrix {
            position: [0.0, 0.0, 2.0],
            ..NodeMatrix::IDENTITY
        };
        let model = model_with_binds(vec![NodeMatrix::IDENTITY, inverse(&rest).unwrap()]);
        let b = body(0.5, 0.0, [10.0, 0.0, 0.0]);
        let world = nodes(&model, &b).expect("both invert");
        assert_eq!(world.len(), 2);
        // Skinning is node * inverse bind: a rest point lands where the
        // pose puts the model's rest space, for every node.
        let at = placement(&b);
        for (node, bind) in world.iter().zip(&model.inverse_binds) {
            let p = [0.4, 0.2, 2.1];
            assert!(near(node.point(bind.point(p)), at.point(p)));
        }
    }

    #[test]
    fn a_model_whose_bind_has_no_inverse_places_no_nodes() {
        let flat = NodeMatrix {
            scale: 0.0,
            ..NodeMatrix::IDENTITY
        };
        let model = model_with_binds(vec![flat]);
        assert_eq!(nodes(&model, &body(0.0, 0.0, [0.0; 3])), None);
    }
}
