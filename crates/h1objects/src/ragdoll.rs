//! A ragdoll for a built biped physics model: which bodies are joined,
//! by what kind of joint, with what frames and limits.
//!
//! Halo 2 makes a dead biped's rigid bodies dynamic and joins them with
//! the physics model's constraints (docs/formats/collision.md,
//! "Constraints" and "Ragdolls"). A built biped has one body per Halo 1
//! collision node with solid cells ([`crate::rebuild::Body`]); this joins
//! each body to the body of its nearest ancestor node that has one, the
//! way retail's ragdolls join theirs (V, the `masterchief` and `elite`
//! physics models: every constraint joins a body to the nearest body up
//! its node chain).
//!
//! The joint kind and limits come from the child node's name: knees and
//! elbows are limited hinges, everything else a ragdoll (cone, twist and
//! plane) joint, as retail's (four hinges at the knees and elbows, five
//! ragdoll joints at the hips, shoulders and head). The limits are our
//! own choices, each inside the range retail's Chief and Elite use for
//! that joint (`tests/ragdoll_disc.rs` holds them to it).
//!
//! Frames follow retail's construction (V, same test): the child's frame
//! sits on the child node's origin; the parent's is the same frame carried
//! into the parent node's space by the child's rest transform. A hinge's
//! child frame is the child node's own axes; a ragdoll joint's turns the
//! twist axis back up the bone (forward `-x`, left `y`, up `-z`).

use crate::physics::Transform;

/// A joint's kind and limits, in degrees as the tag stores them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Joint {
    /// `phmo +0xb8` (constraint type 2): twist about the frame's forward
    /// axis, a cone around it, and the plane limits.
    Ragdoll {
        twist: (f32, f32),
        cone: f32,
        plane: (f32, f32),
    },
    /// `phmo +0xe8` (constraint type 1): rotation about the hinge, as the
    /// tag's `(min, max)` pair (the engine turns it into `[-max, -min]`).
    Hinge { min: f32, max: f32 },
}

/// What a joint is, by the child node's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum JointClass {
    Hip,
    Knee,
    Ankle,
    Spine,
    Neck,
    Clavicle,
    Shoulder,
    Elbow,
    Wrist,
    Other,
}

impl JointClass {
    /// The class of the joint whose child node is named `name` (Halo 1's
    /// node names, e.g. `bip01 l calf`; matched by word so the side and
    /// the `bip01` prefix do not matter).
    pub fn of(name: &str) -> JointClass {
        let n = name.to_ascii_lowercase();
        let has = |w: &str| n.contains(w);
        if has("calf") || has("knee") || has("shin") {
            JointClass::Knee
        } else if has("forearm") || has("elbow") {
            JointClass::Elbow
        } else if has("thigh") || has("hip") {
            JointClass::Hip
        } else if has("upperarm") || has("upper arm") || has("shoulder") {
            JointClass::Shoulder
        } else if has("clavicle") {
            JointClass::Clavicle
        } else if has("foot") || has("toe") || has("ankle") {
            JointClass::Ankle
        } else if has("hand") || has("finger") || has("wrist") {
            JointClass::Wrist
        } else if has("head") || has("neck") {
            JointClass::Neck
        } else if has("spine") || has("pelvis") || has("chest") || has("torso") {
            JointClass::Spine
        } else {
            JointClass::Other
        }
    }

    /// The joint this class gets. Each limit lies inside the range
    /// retail's Chief and Elite use for the same joint (degrees):
    /// knees hinge over `(-80, 7.5)`, elbows `(-70, 25)`; hips twist 5,
    /// cone 30; shoulders twist 7.5, cone 20 to 60; the head twists 45,
    /// cone 35. Plane limits are kept symmetric (retail's are mirrored by
    /// side, which depends on each skeleton's axes).
    pub fn joint(self) -> Joint {
        let rag = |twist: f32, cone: f32, plane: f32| Joint::Ragdoll {
            twist: (-twist, twist),
            cone,
            plane: (-plane, plane),
        };
        match self {
            JointClass::Knee => Joint::Hinge { min: -75.0, max: 5.0 },
            JointClass::Elbow => Joint::Hinge { min: -65.0, max: 10.0 },
            JointClass::Hip => rag(5.0, 30.0, 45.0),
            JointClass::Shoulder => rag(7.5, 45.0, 45.0),
            JointClass::Neck => rag(30.0, 30.0, 20.0),
            JointClass::Spine => rag(10.0, 15.0, 15.0),
            JointClass::Clavicle => rag(5.0, 10.0, 10.0),
            JointClass::Ankle | JointClass::Wrist => rag(5.0, 20.0, 20.0),
            JointClass::Other => rag(5.0, 15.0, 15.0),
        }
    }
}

/// A frame in one body's node space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub forward: [f32; 3],
    pub left: [f32; 3],
    pub up: [f32; 3],
    pub position: [f32; 3],
}

impl Frame {
    /// The node's own axes on its origin.
    pub const IDENTITY: Frame = Frame {
        forward: [1.0, 0.0, 0.0],
        left: [0.0, 1.0, 0.0],
        up: [0.0, 0.0, 1.0],
        position: [0.0; 3],
    };
    /// A ragdoll joint's child frame: the twist axis back up the bone.
    pub const TWIST_BACK: Frame = Frame {
        forward: [-1.0, 0.0, 0.0],
        left: [0.0, 1.0, 0.0],
        up: [0.0, 0.0, -1.0],
        position: [0.0; 3],
    };

    /// This frame carried by `m` (axes rotated, position moved).
    pub fn carried(&self, m: &Transform) -> Frame {
        Frame {
            forward: m.vector(self.forward),
            left: m.vector(self.left),
            up: m.vector(self.up),
            position: m.point(self.position),
        }
    }
}

/// One constraint of a built ragdoll, as the tag holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Constraint {
    pub joint: Joint,
    pub class: JointClass,
    /// Tag `node_a` and `node_b` (model nodes): the parent then the child
    /// for a hinge, the child then the parent for a ragdoll joint (V,
    /// retail's order).
    pub node_a: i16,
    pub node_b: i16,
    pub frame_a: Frame,
    pub frame_b: Frame,
}

/// A joint between two bodies: indices into the bodies, the child's
/// class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Link {
    pub parent: usize,
    pub child: usize,
    pub class: JointClass,
}

/// Joins `body_nodes` (each body's model node) over the model's node tree
/// (`(name, parent)` per node): each body to the body of its nearest
/// ancestor node that has one. `None` when the skeleton is not one this
/// ragdoll fits: fewer than three bodies, a body on no node, more than one
/// body with no ancestor body (the pieces would fall apart), or no leg
/// (no hip and knee) to stand the classification on.
pub fn links(nodes: &[(&str, i16)], body_nodes: &[i16]) -> Option<Vec<Link>> {
    if body_nodes.len() < 3 {
        return None;
    }
    let body_of = |node: i16| body_nodes.iter().position(|&b| b == node);
    let mut out = Vec::new();
    let mut roots = 0;
    for (child, &node) in body_nodes.iter().enumerate() {
        if node < 0 || node as usize >= nodes.len() {
            return None;
        }
        let mut up = nodes[node as usize].1;
        let mut steps = 0;
        let parent = loop {
            if up < 0 || up as usize >= nodes.len() || steps > nodes.len() {
                break None;
            }
            if let Some(b) = body_of(up) {
                break Some(b);
            }
            up = nodes[up as usize].1;
            steps += 1;
        };
        match parent {
            Some(parent) => out.push(Link {
                parent,
                child,
                class: JointClass::of(nodes[node as usize].0),
            }),
            None => roots += 1,
        }
    }
    let has = |c: JointClass| out.iter().any(|l| l.class == c);
    (roots == 1 && has(JointClass::Hip) && has(JointClass::Knee)).then_some(out)
}

/// The constraints for `links`, with `rest[i]` node `i`'s rest transform
/// relative to its parent node (`p_parent = r p_node + t`) and `parents`
/// each node's parent. The child's rest transform relative to the parent
/// body's node is the product along the chain between them.
pub fn constraints(
    links: &[Link],
    body_nodes: &[i16],
    parents: &[i16],
    rest: &[Transform],
) -> Vec<Constraint> {
    links
        .iter()
        .map(|l| {
            let (pn, cn) = (body_nodes[l.parent], body_nodes[l.child]);
            let rel = relative(parents, rest, pn, cn);
            let joint = l.class.joint();
            match joint {
                Joint::Hinge { .. } => Constraint {
                    joint,
                    class: l.class,
                    node_a: pn,
                    node_b: cn,
                    frame_a: Frame::IDENTITY.carried(&rel),
                    frame_b: Frame::IDENTITY,
                },
                Joint::Ragdoll { .. } => Constraint {
                    joint,
                    class: l.class,
                    node_a: cn,
                    node_b: pn,
                    frame_a: Frame::TWIST_BACK,
                    frame_b: Frame::TWIST_BACK.carried(&rel),
                },
            }
        })
        .collect()
}

/// Node `child`'s rest transform in node `ancestor`'s space: the local
/// transforms from `child` up to (not including) `ancestor`, composed.
pub fn relative(parents: &[i16], rest: &[Transform], ancestor: i16, child: i16) -> Transform {
    let mut m = Transform::IDENTITY;
    let mut n = child;
    let mut steps = 0;
    while n >= 0 && n != ancestor && (n as usize) < rest.len() && steps <= rest.len() {
        m = rest[n as usize].then(&m);
        n = parents[n as usize];
        steps += 1;
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Halo 1 style `bip01` skeleton: (name, parent, translation).
    fn skeleton() -> Vec<(&'static str, i16, [f32; 3])> {
        vec![
            ("bip01 pelvis", -1, [0.0, 0.0, 0.0]),
            ("bip01 l thigh", 0, [0.0, 0.1, 0.0]),
            ("bip01 r thigh", 0, [0.0, -0.1, 0.0]),
            ("bip01 spine", 0, [0.05, 0.0, 0.0]),
            ("bip01 l calf", 1, [0.2, 0.0, 0.0]),
            ("bip01 r calf", 2, [0.2, 0.0, 0.0]),
            ("bip01 spine1", 3, [0.1, 0.0, 0.0]),
            ("bip01 l foot", 4, [0.2, 0.0, 0.0]),
            ("bip01 r foot", 5, [0.2, 0.0, 0.0]),
            ("bip01 neck", 6, [0.15, 0.0, 0.0]),
            ("bip01 l clavicle", 6, [0.1, 0.05, 0.0]),
            ("bip01 r clavicle", 6, [0.1, -0.05, 0.0]),
            ("bip01 head", 9, [0.05, 0.0, 0.0]),
            ("bip01 l upperarm", 10, [0.08, 0.0, 0.0]),
            ("bip01 r upperarm", 11, [0.08, 0.0, 0.0]),
            ("bip01 l forearm", 13, [0.12, 0.0, 0.0]),
            ("bip01 r forearm", 14, [0.12, 0.0, 0.0]),
        ]
    }

    #[test]
    fn a_biped_skeleton_maps_to_a_tree_of_joints() {
        let s = skeleton();
        let nodes: Vec<(&str, i16)> = s.iter().map(|n| (n.0, n.1)).collect();
        // Bodies as a converted cyborg's: no spine, neck or clavicle body.
        let bodies = [0i16, 1, 2, 4, 5, 7, 8, 12, 13, 14, 15, 16];
        let links = links(&nodes, &bodies).expect("a humanoid");
        assert_eq!(links.len(), bodies.len() - 1, "every body but the root");
        let by_child = |node: i16| {
            let l = links
                .iter()
                .find(|l| bodies[l.child] == node)
                .unwrap_or_else(|| panic!("no link for node {node}"));
            (bodies[l.parent], l.class)
        };
        assert_eq!(by_child(1), (0, JointClass::Hip));
        assert_eq!(by_child(4), (1, JointClass::Knee));
        assert_eq!(by_child(7), (4, JointClass::Ankle));
        // Bodyless nodes in between are skipped up to the nearest body.
        assert_eq!(by_child(12), (0, JointClass::Neck));
        assert_eq!(by_child(13), (0, JointClass::Shoulder));
        assert_eq!(by_child(15), (13, JointClass::Elbow));
        // Kinds as retail's: hinges at knees and elbows, ragdoll joints
        // elsewhere.
        for l in &links {
            let hinge = matches!(l.class.joint(), Joint::Hinge { .. });
            assert_eq!(hinge, matches!(l.class, JointClass::Knee | JointClass::Elbow));
        }

        let parents: Vec<i16> = s.iter().map(|n| n.1).collect();
        // A quarter turn about z on every node, so the frames are not
        // trivially the identity.
        let q = std::f32::consts::FRAC_PI_4;
        let r = [[q.cos() * 1.0, -q.sin(), 0.0], [q.sin(), q.cos(), 0.0], [0.0, 0.0, 1.0]];
        let rest: Vec<Transform> = s.iter().map(|n| Transform { r, t: n.2 }).collect();
        let cs = constraints(&links, &bodies, &parents, &rest);
        let world = |node: i16| relative(&parents, &rest, -1, node);
        for c in &cs {
            // Parent and child as retail orders them.
            let (parent, child) = match c.joint {
                Joint::Hinge { .. } => (c.node_a, c.node_b),
                Joint::Ragdoll { .. } => (c.node_b, c.node_a),
            };
            assert!(bodies.contains(&parent) && bodies.contains(&child));
            let mut up = parents[child as usize];
            while up != parent {
                assert!(up >= 0, "{parent} is not an ancestor of {child}");
                up = parents[up as usize];
            }
            // At rest the two frames are one frame in the world.
            let fa = c.frame_a.carried(&world(c.node_a));
            let fb = c.frame_b.carried(&world(c.node_b));
            for (x, y) in [
                (fa.forward, fb.forward),
                (fa.left, fb.left),
                (fa.up, fb.up),
                (fa.position, fb.position),
            ] {
                for k in 0..3 {
                    assert!((x[k] - y[k]).abs() < 1e-5, "{c:?}: {x:?} against {y:?}");
                }
            }
            // The child's frame sits on the child's origin.
            let own = if child == c.node_a { c.frame_a } else { c.frame_b };
            assert_eq!(own.position, [0.0; 3]);
        }
        let knee = cs.iter().find(|c| c.class == JointClass::Knee && c.node_b == 4).unwrap();
        let p = knee.frame_a.position;
        assert!((p[0] - 0.2).abs() < 1e-6 && p[1].abs() < 1e-6, "{p:?}");
    }

    #[test]
    fn skeletons_that_do_not_fit_get_no_ragdoll() {
        let s = skeleton();
        let nodes: Vec<(&str, i16)> = s.iter().map(|n| (n.0, n.1)).collect();
        // Two bodies only.
        assert_eq!(links(&nodes, &[0, 1]), None);
        // No pelvis body: both legs and the torso are roots.
        assert_eq!(links(&nodes, &[1, 4, 2, 5, 12]), None);
        // No legs.
        assert_eq!(links(&nodes, &[0, 12, 13, 15]), None);
        // A body on no node.
        assert_eq!(links(&nodes, &[-1, 1, 4]), None);
        // Not a humanoid: a sentinel's names.
        let drone = [("frame", -1), ("gun", 0), ("wing", 0), ("tail", 0)];
        assert_eq!(links(&drone, &[0, 1, 2, 3]), None);
    }

    #[test]
    fn joint_classes_by_name() {
        for (name, class) in [
            ("bip01 l calf", JointClass::Knee),
            ("bip01 r forearm", JointClass::Elbow),
            ("bip01 r thigh", JointClass::Hip),
            ("bip01 l upperarm", JointClass::Shoulder),
            ("bip01 head", JointClass::Neck),
            ("bip01 spine1", JointClass::Spine),
            ("bip01 l foot", JointClass::Ankle),
            ("bip01 r hand", JointClass::Wrist),
            ("bip01 l clavicle", JointClass::Clavicle),
            ("bip01 tail", JointClass::Other),
        ] {
            assert_eq!(JointClass::of(name), class, "{name}");
        }
    }

    /// The built physics model holds the constraints in the typed blocks,
    /// one node edge per constraint naming it, and reads back.
    #[test]
    fn a_built_physics_model_carries_the_ragdoll() {
        use tags::defs::{self, Defs, Val};
        let cube = |h: f64| {
            let mut planes = Vec::new();
            for a in 0..3 {
                let mut n = [0.0; 3];
                n[a] = 1.0;
                planes.push((n, -h));
                n[a] = -1.0;
                planes.push((n, -h));
            }
            crate::physics::hull(&planes).unwrap()
        };
        let s = skeleton();
        let nodes: Vec<(&str, i16)> = s.iter().map(|n| (n.0, n.1)).collect();
        let body_nodes = [0i16, 1, 4, 2, 5, 12];
        let bodies: Vec<crate::rebuild::Body> = body_nodes
            .iter()
            .map(|&node| crate::rebuild::Body { node, pieces: vec![cube(0.05)], mass: 5.0 })
            .collect();
        let pnodes: Vec<crate::rebuild::PhysNode> = s
            .iter()
            .map(|n| crate::rebuild::PhysNode {
                name: n.0.into(),
                parent: n.1,
                next_sibling: -1,
                first_child: -1,
            })
            .collect();
        let parents: Vec<i16> = s.iter().map(|n| n.1).collect();
        let rest: Vec<Transform> = s
            .iter()
            .map(|n| Transform { r: Transform::IDENTITY.r, t: n.2 })
            .collect();
        let ls = links(&nodes, &body_nodes).unwrap();
        let cs = constraints(&ls, &body_nodes, &parents, &rest);
        let root = crate::rebuild::physics_model(&bodies, &pnodes, crate::rebuild::BIPED_BODY, &cs)
            .unwrap();
        let def = Defs::builtin().group(map::FourCC(*b"phmo")).unwrap();
        let got = defs::from_built(def, &root).unwrap();
        assert_eq!(defs::to_built(def, &got, &mut |_| None).unwrap().bytes, root.bytes);
        let block = |r: &defs::Record, p: &str| match r.at(p) {
            Some(Val::Block(b)) => b.clone(),
            other => panic!("{p}: {other:?}"),
        };
        let int = |r: &defs::Record, p: &str| match r.at(p) {
            Some(Val::Int(i)) => *i,
            other => panic!("{p}: {other:?}"),
        };
        let rags = block(&got, "ragdoll_constraints");
        let hinges = block(&got, "limited_hinge_constraints");
        let edges = block(&got, "node_edges");
        // Hips and the head are ragdoll joints, the knees hinges.
        assert_eq!((rags.len(), hinges.len(), edges.len()), (3, 2, 5));
        for (kind, list) in [(2, &rags), (1, &hinges)] {
            for (i, c) in list.iter().enumerate() {
                let e = &edges[int(c, "header.edge") as usize];
                assert_eq!(int(e, "node_a"), int(c, "header.node_a"));
                assert_eq!(int(e, "node_b"), int(c, "header.node_b"));
                let r = &block(e, "constraints")[0];
                assert_eq!((int(r, "constraint_type"), int(r, "constraint_index")), (kind, i as i64));
                assert_eq!(r.at("friction"), Some(&Val::Float(crate::rebuild::JOINT_FRICTION)));
            }
        }
        let knee = &hinges[0];
        assert_eq!(knee.at("min_angle"), Some(&Val::Float(-75.0)));
        assert_eq!((int(knee, "header.node_a"), int(knee, "header.node_b")), (1, 4));
        let phmo_nodes = block(&got, "nodes");
        assert_eq!(int(&phmo_nodes[4], "parent"), 1);
        assert_eq!(int(&phmo_nodes[4], "flags"), 0);
    }
}
