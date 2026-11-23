//! The Pogo Stick's pose (docs/game/pogo-stick.md, "The pose"): a rider's
//! and a stick's node rows from the rider's [`Sample`].
//!
//! Pure: [`pose`] takes a [`Skeleton`] (a model's nodes at rest, with the
//! roles the keys name found in it), the rows the game has for the biped
//! and the stick, the sample, the stick's [`Rig`] and the [`Keys`]
//! (`art/pogo/poses.yaml`), and gives the rows to draw.
//! [`crate::pogo_pose_glue`] reads and writes the game around it.
//!
//! **The body frame.** Its origin is the pelvis joint where the game has
//! it; its facing is the yaw of the game's pelvis row, upright. Everything
//! is laid out in that frame and turned into the world at the end.
//!
//! **Mounted.** The stick stands upright ahead of the pelvis, handlebar
//! across the body. The pelvis, spine, neck and head take keyed pitches by
//! forward kinematics. Each leg and arm is a two-bone solve
//! ([`two_bone`]): the foot's sole point on its peg, the hand's palm point
//! on its grip. A target out of reach leaves the limb straight toward it.
//!
//! **A row** is a rigid turn of the node's rest frame: the row's axes are
//! the turn times the rest axes, its position the joint. The rest frames
//! come from the model's inverse rest frames, so no quaternion convention
//! is read.
//!
//! **Compress** brings the frame, the pegs and the rider down by the
//! piston's travel and slides the piston up the frame by the same, so the
//! foot stays where it is. **Tuck** brings the pegs up toward the pelvis.
//!
//! **Tricks** change the mounted layout by a weight that eases in over
//! the first [`TRICK_EASE`] of the trick and out over the last, so each
//! starts and ends on the mounted pose. The flips turn the whole of it
//! about the left axis through the pelvis joint, a whole number of turns.
//! In the Bar Spin the frame turns once about the shaft while the hands
//! lift clear and the feet hop off the pegs.
//!
//! **Mount and dismount** blend every row from the game's to the mounted
//! one by the sample's `mounted`, eased.

use std::sync::OnceLock;

use frame_scene::NodeMatrix;
use pogo::Rig;
use serde::Deserialize;

use crate::pogo::{Sample, Trick};

type V3 = [f32; 3];
/// A rotation, `m[row][column]`.
type M3 = [[f32; 3]; 3];

/// The left side's index in a pair.
pub const LEFT: usize = 0;
/// The right side's.
pub const RIGHT: usize = 1;
/// The share of a trick its weight eases in over, and out over.
pub const TRICK_EASE: f32 = 0.15;
/// The node rows a stick has: the frame and the piston.
pub const STICK_NODES: usize = 2;

const IDENT: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

// ---- vectors ----

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}

fn unit(a: V3) -> Option<V3> {
    let l = len(a);
    (l.is_finite() && l > 1e-6).then(|| scale(a, 1.0 / l))
}

fn lerp(a: V3, b: V3, t: f32) -> V3 {
    add(a, scale(sub(b, a), t))
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn clamp01(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// 0 to 1 over 0 to 1, still at both ends.
fn ease(t: f32) -> f32 {
    let t = clamp01(t);
    t * t * (3.0 - 2.0 * t)
}

/// A side's copy of a vector given for the left: the right's is mirrored.
pub fn mirror(v: V3, side: usize) -> V3 {
    if side == RIGHT {
        [v[0], -v[1], v[2]]
    } else {
        v
    }
}

// ---- rotations ----

fn m_mul(a: M3, b: M3) -> M3 {
    let mut out = [[0.0; 3]; 3];
    for (r, row) in out.iter_mut().enumerate() {
        for (c, v) in row.iter_mut().enumerate() {
            *v = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
    }
    out
}

fn m_vec(m: M3, v: V3) -> V3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

fn m_t(m: M3) -> M3 {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}

/// The rotation whose columns are `x`, `y`, `z`.
fn cols(x: V3, y: V3, z: V3) -> M3 {
    [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]]
}

/// A pitch: about the left axis (+Y), positive leaning forward (+X goes
/// toward -Z, +Z toward +X).
fn ry(a: f32) -> M3 {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// A yaw: about the up axis, +X toward +Y.
fn rz(a: f32) -> M3 {
    let (s, c) = a.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// A row's axes as a rotation.
fn axes(row: &NodeMatrix) -> M3 {
    cols(row.forward, row.left, row.up)
}

/// The unit vector nearest `pole` that is square to the unit `dir`; when
/// `pole` runs along `dir`, the nearest to up, forward or left.
fn square_to(dir: V3, pole: V3) -> V3 {
    for p in [pole, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
        let across = sub(p, scale(dir, dot(p, dir)));
        if len(across) > 1e-3
            && let Some(u) = unit(across)
        {
            return u;
        }
    }
    [0.0, 0.0, 1.0]
}

/// The frame of a bone that runs along `dir` and faces `pole`: +X along
/// the bone, +Y toward the pole.
fn frame(dir: V3, pole: V3) -> M3 {
    let x = unit(dir).unwrap_or([0.0, 0.0, -1.0]);
    let y = square_to(x, pole);
    cols(x, y, cross(x, y))
}

/// The turn that takes a bone at rest (running `rest_dir`, facing
/// `rest_pole`) to one running `dir` and facing `pole`.
fn bone_turn(dir: V3, pole: V3, rest_dir: V3, rest_pole: V3) -> M3 {
    m_mul(frame(dir, pole), m_t(frame(rest_dir, rest_pole)))
}

/// A two-bone limb from `root` toward `target`, its bones `l1` and `l2`
/// long, bending toward `pole`: where the middle joint and the end joint
/// go. The end is on the target when it is in reach; a target too far
/// leaves the limb straight toward it, one too near folds it as far as it
/// goes.
pub fn two_bone(root: V3, target: V3, l1: f32, l2: f32, pole: V3) -> (V3, V3) {
    let to = sub(target, root);
    let dir = unit(to).unwrap_or([0.0, 0.0, -1.0]);
    let near = (l1 - l2).abs() + 1e-5;
    let far = (l1 + l2 - 1e-5).max(near);
    let dist = len(to);
    let reach = if dist.is_finite() {
        dist.clamp(near, far)
    } else {
        far
    };
    let along = (l1 * l1 - l2 * l2 + reach * reach) / (2.0 * reach);
    let out = (l1 * l1 - along * along).max(0.0).sqrt();
    let bend = square_to(dir, pole);
    (
        add(root, add(scale(dir, along), scale(bend, out))),
        add(root, scale(dir, reach)),
    )
}

// ---- the keys ----

/// `art/pogo/poses.yaml`; the file's header says what each field is.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Keys {
    pub bones: BoneNames,
    pub mounted: Mounted,
    pub tricks: Tricks,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BoneNames {
    pub pelvis: Vec<String>,
    #[serde(default)]
    pub spine: Vec<Vec<String>>,
    #[serde(default)]
    pub neck: Vec<String>,
    #[serde(default)]
    pub head: Vec<String>,
    pub left: SideNames,
    pub right: SideNames,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SideNames {
    pub thigh: Vec<String>,
    pub calf: Vec<String>,
    pub foot: Vec<String>,
    pub upper_arm: Vec<String>,
    pub forearm: Vec<String>,
    pub hand: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mounted {
    pub pelvis_over_pegs: f32,
    pub stick_forward: f32,
    pub tuck_lift: f32,
    pub pelvis_pitch: f32,
    pub spine_pitch: f32,
    pub neck_pitch: f32,
    pub head_pitch: f32,
    pub sole: V3,
    pub palm: V3,
    pub hand_yaw: f32,
    pub knee: V3,
    pub elbow: V3,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Tricks {
    pub no_hander: NoHander,
    pub can_can: CanCan,
    pub bar_spin: BarSpin,
    pub superman: Superman,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NoHander {
    pub hand: V3,
    pub spine_pitch: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CanCan {
    pub foot: V3,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BarSpin {
    pub hand_lift: V3,
    pub hop: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Superman {
    pub pelvis: V3,
    pub pelvis_pitch: f32,
    pub spine_pitch: f32,
    pub head_pitch: f32,
    pub foot_pitch: f32,
    pub foot: V3,
}

/// The keys' source, embedded.
pub const KEYS_YAML: &str = include_str!("../../../../art/pogo/poses.yaml");

impl Keys {
    pub fn parse(yaml: &str) -> Result<Keys, String> {
        serde_yaml_ng::from_str(yaml).map_err(|e| e.to_string())
    }
}

/// The embedded keys, parsed once; `Err` says why they do not parse (a
/// test keeps that from shipping).
pub fn keys() -> Result<&'static Keys, &'static str> {
    static KEYS: OnceLock<Result<Keys, String>> = OnceLock::new();
    KEYS.get_or_init(|| Keys::parse(KEYS_YAML))
        .as_ref()
        .map_err(String::as_str)
}

// ---- the skeleton ----

/// A node at rest.
#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    pub name: String,
    pub parent: Option<usize>,
    /// The node's frame in model space.
    pub rest: NodeMatrix,
}

/// A node's rest frame from the model's inverse rest frame (`mode` node
/// `+0x28`, thirteen floats in a row's layout): the transpose of its
/// axes, and the position they take back. `None` when it is not finite.
pub fn rest_of(inverse: &[f32]) -> Option<NodeMatrix> {
    let w: &[f32; 13] = inverse.get(..13)?.try_into().ok()?;
    let inv = NodeMatrix::from_words(w);
    if !inv.is_finite() {
        return None;
    }
    let r = m_t(axes(&inv));
    let col = |c: usize| [r[0][c], r[1][c], r[2][c]];
    Some(NodeMatrix {
        scale: 1.0,
        forward: col(0),
        left: col(1),
        up: col(2),
        position: scale(m_vec(r, inv.position), -1.0),
    })
}

/// A two-bone limb: its three nodes and its two lengths at rest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limb {
    pub root: usize,
    pub mid: usize,
    pub end: usize,
    pub l1: f32,
    pub l2: f32,
}

/// The nodes the pose moves.
#[derive(Clone, Debug, PartialEq)]
pub struct Roles {
    pub pelvis: usize,
    pub spine: Vec<usize>,
    pub neck: Option<usize>,
    pub head: Option<usize>,
    /// By [`LEFT`] and [`RIGHT`].
    pub legs: [Limb; 2],
    pub arms: [Limb; 2],
}

/// A model's nodes at rest, with the keys' roles found among them.
#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    bones: Vec<Bone>,
    /// Every node, parents before children.
    order: Vec<usize>,
    roles: Result<Roles, String>,
}

impl Skeleton {
    /// `bones` in the model's node order. A parent that is out of range,
    /// or the node itself, is no parent.
    pub fn new(mut bones: Vec<Bone>, keys: &Keys) -> Skeleton {
        let n = bones.len();
        for (i, b) in bones.iter_mut().enumerate() {
            b.name = b.name.to_ascii_lowercase();
            b.parent = b.parent.filter(|&p| p < n && p != i);
        }
        let mut order = Vec::with_capacity(n);
        let mut placed = vec![false; n];
        for _ in 0..n {
            let before = order.len();
            for (i, b) in bones.iter().enumerate() {
                if !placed[i] && b.parent.is_none_or(|p| placed[p]) {
                    placed[i] = true;
                    order.push(i);
                }
            }
            if order.len() == before {
                break;
            }
        }
        let mut s = Skeleton {
            bones,
            order,
            roles: Err(String::new()),
        };
        s.roles = if s.order.len() != n {
            Err("its nodes' parents loop".into())
        } else if s.bones.iter().any(|b| !b.rest.is_finite()) {
            Err("a node's rest frame is not finite".into())
        } else {
            s.find_roles(&keys.bones)
        };
        s
    }

    pub fn len(&self) -> usize {
        self.bones.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }

    pub fn bones(&self) -> &[Bone] {
        &self.bones
    }

    /// The node named by the first of `names` the model has, lower case.
    pub fn find(&self, names: &[String]) -> Option<usize> {
        names.iter().find_map(|n| {
            let n = n.to_ascii_lowercase();
            self.bones.iter().position(|b| b.name == n)
        })
    }

    /// The roles, or why the model is not posed.
    pub fn roles(&self) -> Result<&Roles, &str> {
        self.roles.as_ref().map_err(String::as_str)
    }

    fn descends(&self, node: usize, from: usize) -> bool {
        let mut at = self.bones[node].parent;
        // The order holds every node, so the parents do not loop.
        while let Some(p) = at {
            if p == from {
                return true;
            }
            at = self.bones[p].parent;
        }
        false
    }

    fn limb(&self, what: &str, names: [&[String]; 3]) -> Result<Limb, String> {
        let mut nodes = [0usize; 3];
        for (slot, list) in nodes.iter_mut().zip(names) {
            *slot = self.find(list).ok_or_else(|| {
                format!(
                    "no {what} node ({})",
                    list.first().map_or("unnamed", String::as_str)
                )
            })?;
        }
        let [root, mid, end] = nodes;
        if !self.descends(mid, root) || !self.descends(end, mid) {
            return Err(format!("the {what}'s nodes are not one chain"));
        }
        let at = |i: usize| self.bones[i].rest.position;
        let (l1, l2) = (len(sub(at(mid), at(root))), len(sub(at(end), at(mid))));
        if !(l1 > 1e-4 && l2 > 1e-4) {
            return Err(format!("the {what} has a bone of no length"));
        }
        Ok(Limb {
            root,
            mid,
            end,
            l1,
            l2,
        })
    }

    fn find_roles(&self, names: &BoneNames) -> Result<Roles, String> {
        let pelvis = self.find(&names.pelvis).ok_or("no pelvis node")?;
        let side = |s: &SideNames, name: &str| -> Result<(Limb, Limb), String> {
            let leg = self.limb(&format!("{name} leg"), [&s.thigh, &s.calf, &s.foot])?;
            let arm = self.limb(&format!("{name} arm"), [&s.upper_arm, &s.forearm, &s.hand])?;
            for limb in [leg, arm] {
                if !self.descends(limb.root, pelvis) {
                    return Err(format!("the {name} side does not hang off the pelvis"));
                }
            }
            Ok((leg, arm))
        };
        let (left_leg, left_arm) = side(&names.left, "left")?;
        let (right_leg, right_arm) = side(&names.right, "right")?;
        Ok(Roles {
            pelvis,
            spine: names.spine.iter().filter_map(|s| self.find(s)).collect(),
            neck: self.find(&names.neck),
            head: self.find(&names.head),
            legs: [left_leg, right_leg],
            arms: [left_arm, right_arm],
        })
    }
}

// ---- the layout ----

/// Where the knee points at rest, model space: forward.
const REST_KNEE: V3 = [1.0, 0.0, 0.0];
/// Where the elbow points at rest: back.
const REST_ELBOW: V3 = [-1.0, 0.0, 0.0];

/// The rider and the stick laid out in the body frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Plan {
    /// The pelvis joint.
    pelvis: V3,
    /// Radians.
    pelvis_pitch: f32,
    spine_pitch: f32,
    neck_pitch: f32,
    head_pitch: f32,
    foot_pitch: f32,
    /// The stick's origin with no spin.
    stick: V3,
    /// The frame's turn about the shaft, radians.
    spin: f32,
    /// The feet's joints, by side.
    ankle: [V3; 2],
    /// The hands' joints.
    wrist: [V3; 2],
    /// The whole pose's pitch about the pelvis joint, radians.
    flip: f32,
}

/// The shaft's line in the stick's space: its Y.
fn shaft_y(rig: &Rig) -> f32 {
    (rig.peg_l[1] + rig.peg_r[1]) * 0.5
}

/// The hand's turn from its rest pose, body frame.
fn hand_turn(keys: &Keys, side: usize) -> M3 {
    let yaw = keys.mounted.hand_yaw.to_radians();
    rz(if side == RIGHT { -yaw } else { yaw })
}

/// A trick's weight at `progress`: in over the first [`TRICK_EASE`], out
/// over the last.
pub fn trick_weight(progress: f32) -> f32 {
    let p = clamp01(progress);
    ease(p / TRICK_EASE) * ease((1.0 - p) / TRICK_EASE)
}

fn plan(keys: &Keys, rig: &Rig, sample: &Sample) -> Plan {
    let k = &keys.mounted;
    let pelvis = [0.0, 0.0, -rig.travel * clamp01(sample.compress)];
    let over = k.pelvis_over_pegs - clamp01(sample.tuck) * k.tuck_lift;
    let peg_z = (rig.peg_l[2] + rig.peg_r[2]) * 0.5;
    let stick = add(pelvis, [k.stick_forward, -shaft_y(rig), -over - peg_z]);
    let pegs = [rig.peg_l, rig.peg_r];
    let grips = [rig.grip_l, rig.grip_r];
    let mut p = Plan {
        pelvis,
        pelvis_pitch: k.pelvis_pitch.to_radians(),
        spine_pitch: k.spine_pitch.to_radians(),
        neck_pitch: k.neck_pitch.to_radians(),
        head_pitch: k.head_pitch.to_radians(),
        foot_pitch: 0.0,
        stick,
        spin: 0.0,
        ankle: std::array::from_fn(|s| sub(add(stick, pegs[s]), mirror(k.sole, s))),
        wrist: std::array::from_fn(|s| {
            sub(
                add(stick, grips[s]),
                m_vec(hand_turn(keys, s), mirror(k.palm, s)),
            )
        }),
        flip: 0.0,
    };
    let Some((trick, progress)) = sample.trick else {
        return p;
    };
    let w = trick_weight(progress);
    let t = &keys.tricks;
    match trick {
        Trick::NoHander => {
            for s in [LEFT, RIGHT] {
                p.wrist[s] = lerp(p.wrist[s], add(p.pelvis, mirror(t.no_hander.hand, s)), w);
            }
            p.spine_pitch += w * t.no_hander.spine_pitch.to_radians();
        }
        Trick::CanCan => {
            p.ankle[RIGHT] = lerp(p.ankle[RIGHT], add(p.pelvis, t.can_can.foot), w);
        }
        Trick::BarSpin => {
            p.spin = std::f32::consts::TAU * ease(progress);
            for s in [LEFT, RIGHT] {
                p.wrist[s] = add(p.wrist[s], scale(mirror(t.bar_spin.hand_lift, s), w));
                p.ankle[s][2] += w * t.bar_spin.hop;
            }
        }
        Trick::Superman => {
            let s = &t.superman;
            p.pelvis = add(p.pelvis, scale(s.pelvis, w));
            p.pelvis_pitch = mix(p.pelvis_pitch, s.pelvis_pitch.to_radians(), w);
            p.spine_pitch = mix(p.spine_pitch, s.spine_pitch.to_radians(), w);
            p.head_pitch = mix(p.head_pitch, s.head_pitch.to_radians(), w);
            p.foot_pitch = w * s.foot_pitch.to_radians();
            for side in [LEFT, RIGHT] {
                p.ankle[side] = lerp(p.ankle[side], add(p.pelvis, mirror(s.foot, side)), w);
            }
        }
        Trick::Backflip | Trick::DoubleBackflip | Trick::FrontFlip => {
            // Backward is the head going back: a negative pitch.
            p.flip = -std::f32::consts::TAU * trick.flips() * ease(progress);
        }
    }
    p
}

/// Every node's turn and joint in the body frame, before the flip.
fn solve(sk: &Skeleton, roles: &Roles, keys: &Keys, p: &Plan) -> Vec<(M3, V3)> {
    let bones = &sk.bones;
    let at = |i: usize| bones[i].rest.position;
    let pelvis_rest = at(roles.pelvis);
    let mut out = vec![(IDENT, [0.0; 3]); bones.len()];
    // Turns a limb's root set for its other two nodes.
    let mut set: Vec<Option<M3>> = vec![None; bones.len()];
    for &b in &sk.order {
        let (mut q, mut j) = match bones[b].parent {
            Some(parent) => {
                let (pq, pj) = out[parent];
                (pq, add(pj, m_vec(pq, sub(at(b), at(parent)))))
            }
            // A node over the pelvis, or beside it: carried with it,
            // upright.
            None => (IDENT, add(p.pelvis, sub(at(b), pelvis_rest))),
        };
        if b == roles.pelvis {
            q = ry(p.pelvis_pitch);
            j = p.pelvis;
        } else if let Some(turn) = set[b] {
            q = turn;
        } else if roles.spine.contains(&b) {
            q = m_mul(ry(p.spine_pitch), q);
        } else if roles.neck == Some(b) {
            q = m_mul(ry(p.neck_pitch), q);
        } else if roles.head == Some(b) {
            q = m_mul(ry(p.head_pitch), q);
        } else {
            for side in [LEFT, RIGHT] {
                let (limb, target, pole, rest_pole, end) = if roles.legs[side].root == b {
                    (
                        roles.legs[side],
                        p.ankle[side],
                        mirror(keys.mounted.knee, side),
                        REST_KNEE,
                        ry(p.foot_pitch),
                    )
                } else if roles.arms[side].root == b {
                    (
                        roles.arms[side],
                        p.wrist[side],
                        mirror(keys.mounted.elbow, side),
                        REST_ELBOW,
                        hand_turn(keys, side),
                    )
                } else {
                    continue;
                };
                let (mid, tip) = two_bone(j, target, limb.l1, limb.l2, pole);
                q = bone_turn(
                    sub(mid, j),
                    pole,
                    sub(at(limb.mid), at(limb.root)),
                    rest_pole,
                );
                set[limb.mid] = Some(bone_turn(
                    sub(tip, mid),
                    pole,
                    sub(at(limb.end), at(limb.mid)),
                    rest_pole,
                ));
                set[limb.end] = Some(end);
                break;
            }
        }
        out[b] = (q, j);
    }
    out
}

/// The body's yaw from the game's pelvis row: where the model's forward
/// points, flattened; its left when the body is pitched on end.
fn body_yaw(row: &NodeMatrix, rest: &NodeMatrix) -> Option<f32> {
    let turn = m_mul(axes(row), m_t(axes(rest)));
    let forward = m_vec(turn, [1.0, 0.0, 0.0]);
    if forward[0].hypot(forward[1]) > 0.2 {
        return Some(forward[1].atan2(forward[0]));
    }
    let left = m_vec(turn, [0.0, 1.0, 0.0]);
    (left[0].hypot(left[1]) > 0.2).then(|| left[1].atan2(left[0]) - std::f32::consts::FRAC_PI_2)
}

/// `a` to `b` by `t`: the position straight, the axes blended and made
/// square again, `a`'s scale.
fn blend(a: &NodeMatrix, b: &NodeMatrix, t: f32) -> NodeMatrix {
    if t >= 1.0 {
        return NodeMatrix {
            scale: a.scale,
            ..*b
        };
    }
    let forward = unit(lerp(a.forward, b.forward, t));
    let left = forward.and_then(|f| {
        let l = lerp(a.left, b.left, t);
        unit(sub(l, scale(f, dot(l, f))))
    });
    let (forward, left) = match (forward, left) {
        (Some(f), Some(l)) => (f, l),
        // Opposed axes have no half way: the far side's.
        _ => (b.forward, b.left),
    };
    NodeMatrix {
        scale: a.scale,
        forward,
        left,
        up: cross(forward, left),
        position: lerp(a.position, b.position, t),
    }
}

/// The piston's row for a frame row: the frame's child at the rig's
/// place, slid up the frame by the travel times `compress`.
pub fn piston_row(frame: &NodeMatrix, rig: &Rig, compress: f32) -> NodeMatrix {
    NodeMatrix {
        position: frame.point(add(rig.piston, [0.0, 0.0, rig.travel * clamp01(compress)])),
        ..*frame
    }
}

/// Where a point fixed to a node is: `offset` from the node's joint in
/// model space, carried by the row's turn from the rest frame.
pub fn attach(row: &NodeMatrix, rest: &NodeMatrix, offset: V3) -> V3 {
    add(
        row.position,
        m_vec(m_mul(axes(row), m_t(axes(rest))), offset),
    )
}

/// The rows to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Posed {
    /// One a node, in the skeleton's order.
    pub biped: Vec<NodeMatrix>,
    /// The frame and the piston.
    pub stick: [NodeMatrix; STICK_NODES],
}

/// The rider's and the stick's rows for `sample`, world space, from the
/// rows the game has (`biped`, one a node of `sk`; `stick`, the frame and
/// the piston).
///
/// `None` leaves the game's rows as they are: the stick carried with the
/// piston out, a model the keys' roles are not all found in
/// ([`Skeleton::roles`] says which), a row count that is not the
/// skeleton's, or rows that are not finite.
pub fn pose(
    sk: &Skeleton,
    keys: &Keys,
    rig: &Rig,
    biped: &[NodeMatrix],
    stick: &[NodeMatrix; STICK_NODES],
    sample: &Sample,
) -> Option<Posed> {
    if !sample.posed() {
        return None;
    }
    let roles = sk.roles().ok()?;
    if biped.len() != sk.len() || !biped.iter().chain(stick).all(NodeMatrix::is_finite) {
        return None;
    }
    let weight = ease(sample.mounted);
    let mut out = Posed {
        biped: biped.to_vec(),
        stick: *stick,
    };
    if weight > 0.0 {
        let pelvis = &biped[roles.pelvis];
        let yaw = rz(body_yaw(pelvis, &sk.bones[roles.pelvis].rest)?);
        let plan = plan(keys, rig, sample);
        let flip = ry(plan.flip);
        let turn = m_mul(yaw, flip);
        // A body-frame point in the world: flipped about the pelvis
        // joint, turned to the body's yaw, at the game's pelvis.
        let place = |v: V3| {
            add(
                pelvis.position,
                m_vec(yaw, add(plan.pelvis, m_vec(flip, sub(v, plan.pelvis)))),
            )
        };
        for (b, (q, j)) in solve(sk, roles, keys, &plan).into_iter().enumerate() {
            let r = m_mul(m_mul(turn, q), axes(&sk.bones[b].rest));
            let mounted = NodeMatrix {
                scale: biped[b].scale,
                forward: [r[0][0], r[1][0], r[2][0]],
                left: [r[0][1], r[1][1], r[2][1]],
                up: [r[0][2], r[1][2], r[2][2]],
                position: place(j),
            };
            out.biped[b] = blend(&biped[b], &mounted, weight);
        }
        // The frame: turned about the shaft by the spin.
        let spin = rz(plan.spin);
        let shaft = [0.0, shaft_y(rig), 0.0];
        let r = m_mul(turn, spin);
        let frame = NodeMatrix {
            scale: stick[0].scale,
            forward: [r[0][0], r[1][0], r[2][0]],
            left: [r[0][1], r[1][1], r[2][1]],
            up: [r[0][2], r[1][2], r[2][2]],
            position: place(add(plan.stick, sub(shaft, m_vec(spin, shaft)))),
        };
        out.stick[0] = blend(&stick[0], &frame, weight);
    }
    out.stick[1] = piston_row(&out.stick[0], rig, sample.compress);
    out.biped
        .iter()
        .chain(&out.stick)
        .all(NodeMatrix::is_finite)
        .then_some(out)
}

#[cfg(test)]
#[path = "pogo_pose_tests.rs"]
pub(crate) mod tests;
