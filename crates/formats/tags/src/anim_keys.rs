//! Sparse keys for a decoded clip: the key reduction of the animation
//! editor (docs/editor/studio-redesign.md, "The animation editor").
//!
//! A retail animation decodes to dense frames ([`crate::anim_sample`]).
//! [`reduce`] turns those into the fewest keys per channel that
//! [`Keys::bake`] plays back within a tolerance at every frame: a channel
//! is one node's rotation, translation or scale, a key a frame and a
//! value, and the frames between two keys are linear (a rotation along
//! the shorter arc, re-normalised, as [`crate::anim_sample::AnimationClip::sample`]
//! blends). Frames before the first key and after the last hold it.
//!
//! The reduction is Ramer-Douglas-Peucker over the frames: the ends are
//! kept, and a range splits at the frame farthest from the line between
//! its ends while that frame is out of tolerance. A channel whose every
//! frame is within tolerance of its first is one key. The error of a
//! rotation is the angle of the turn between two orientations, so one
//! tolerance means the same on every node; a translation's is a distance
//! and a scale's a difference.
//!
//! Only channels a frame supplies (`has_*` of the sample) get keys, so a
//! baked pose keeps which channels the animation drove.

use crate::anim_sample::{NodeTransform, Pose};

/// Which part of a node's transform a channel carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Rotation,
    Translation,
    Scale,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Rotation, Kind::Translation, Kind::Scale];

    pub fn name(self) -> &'static str {
        match self {
            Self::Rotation => "rotation",
            Self::Translation => "translation",
            Self::Scale => "scale",
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Rotation => 0,
            Self::Translation => 1,
            Self::Scale => 2,
        }
    }

    /// How many of a key's four numbers the channel uses.
    pub fn width(self) -> usize {
        match self {
            Self::Rotation => 4,
            Self::Translation => 3,
            Self::Scale => 1,
        }
    }
}

/// The error a reduction may leave at any frame, per kind of channel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerance {
    /// The angle between the original and the baked orientation, radians.
    pub rotation: f32,
    /// The distance between the positions, in the animation's units.
    pub translation: f32,
    /// The difference between the scales.
    pub scale: f32,
}

impl Tolerance {
    /// A tenth of a degree, a thousandth of a unit, a thousandth of a
    /// scale: below what the game shows.
    pub const DEFAULT: Tolerance = Tolerance {
        rotation: 0.1 * std::f32::consts::PI / 180.0,
        translation: 0.001,
        scale: 0.001,
    };

    fn of(&self, kind: Kind) -> f32 {
        [self.rotation, self.translation, self.scale][kind.index()]
    }
}

impl Default for Tolerance {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// One key: a frame and the channel's value there. A rotation is a unit
/// quaternion `[x, y, z, w]`, a translation `[x, y, z, 0]`, a scale
/// `[s, 0, 0, 0]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub frame: usize,
    pub value: [f32; 4],
}

/// One node's rotation, translation or scale as keys in frame order.
#[derive(Clone, Debug, PartialEq)]
pub struct Channel {
    pub node: usize,
    pub kind: Kind,
    pub keys: Vec<Key>,
}

impl Channel {
    /// More than one key: the channel changes over the clip.
    pub fn moves(&self) -> bool {
        self.keys.len() > 1
    }

    /// The channel's value at `frame`: held before the first key and
    /// after the last, linear between.
    pub fn value_at(&self, frame: f32) -> [f32; 4] {
        let keys = &self.keys;
        let (Some(first), Some(last)) = (keys.first(), keys.last()) else {
            return [0.0; 4];
        };
        if frame <= first.frame as f32 {
            return first.value;
        }
        if frame >= last.frame as f32 {
            return last.value;
        }
        let after = keys.partition_point(|k| (k.frame as f32) <= frame);
        let (a, b) = (keys[after - 1], keys[after]);
        let t = (frame - a.frame as f32) / (b.frame - a.frame) as f32;
        lerp(self.kind, a.value, b.value, t)
    }
}

/// A clip as keys: [`reduce`]'s result.
#[derive(Clone, Debug, PartialEq)]
pub struct Keys {
    /// The clip's frame count.
    pub frames: usize,
    /// The clip's node count.
    pub nodes: usize,
    pub tolerance: Tolerance,
    /// By node, then rotation, translation, scale; only channels the
    /// clip drives.
    pub channels: Vec<Channel>,
}

impl Keys {
    /// Keys over every channel.
    pub fn key_count(&self) -> usize {
        self.channels.iter().map(|c| c.keys.len()).sum()
    }

    /// The clip played back at every frame: each channel at its frame,
    /// the channels no key covers left at the identity with their `has_*`
    /// false.
    pub fn bake(&self) -> Vec<Pose> {
        (0..self.frames)
            .map(|f| {
                let mut pose = Pose {
                    nodes: vec![NodeTransform::default(); self.nodes],
                };
                for c in &self.channels {
                    let v = c.value_at(f as f32);
                    let n = &mut pose.nodes[c.node];
                    match c.kind {
                        Kind::Rotation => {
                            n.rotation = v;
                            n.has_rotation = true;
                        }
                        Kind::Translation => {
                            n.translation = [v[0], v[1], v[2]];
                            n.has_translation = true;
                        }
                        Kind::Scale => {
                            n.scale = v[0];
                            n.has_scale = true;
                        }
                    }
                }
                pose
            })
            .collect()
    }

    /// The largest error of the keys against the dense `poses` they came
    /// from, per kind `[rotation, translation, scale]`, over every frame.
    pub fn max_error(&self, poses: &[Pose]) -> [f32; 3] {
        let mut worst = [0.0f32; 3];
        for c in &self.channels {
            for (f, pose) in poses.iter().enumerate().take(self.frames) {
                let Some(n) = pose.nodes.get(c.node) else {
                    continue;
                };
                let e = distance(c.kind, c.value_at(f as f32), value(c.kind, n));
                let w = &mut worst[c.kind.index()];
                *w = w.max(e);
            }
        }
        worst
    }
}

fn value(kind: Kind, n: &NodeTransform) -> [f32; 4] {
    match kind {
        Kind::Rotation => n.rotation,
        Kind::Translation => [n.translation[0], n.translation[1], n.translation[2], 0.0],
        Kind::Scale => [n.scale, 0.0, 0.0, 0.0],
    }
}

fn supplied(kind: Kind, n: &NodeTransform) -> bool {
    match kind {
        Kind::Rotation => n.has_rotation,
        Kind::Translation => n.has_translation,
        Kind::Scale => n.has_scale,
    }
}

/// The sign that puts `b` in `a`'s hemisphere (`q` and `-q` are one turn).
fn hemisphere(a: [f32; 4], b: [f32; 4]) -> f32 {
    let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    if dot < 0.0 { -1.0 } else { 1.0 }
}

/// How far apart two values of a channel are: for a rotation the angle of
/// the turn between them (`4 asin(|a - b| / 2)` of the quaternions in one
/// hemisphere, which stays exact for small angles where `acos` of a dot
/// does not).
fn distance(kind: Kind, a: [f32; 4], b: [f32; 4]) -> f32 {
    match kind {
        Kind::Rotation => {
            let s = hemisphere(a, b);
            let d: f32 = (0..4)
                .map(|k| (a[k] - s * b[k]).powi(2))
                .sum::<f32>()
                .sqrt();
            4.0 * (d * 0.5).min(1.0).asin()
        }
        Kind::Translation => (0..3).map(|k| (a[k] - b[k]).powi(2)).sum::<f32>().sqrt(),
        Kind::Scale => (a[0] - b[0]).abs(),
    }
}

fn lerp(kind: Kind, a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    let s = if kind == Kind::Rotation {
        hemisphere(a, b)
    } else {
        1.0
    };
    let mut out: [f32; 4] = std::array::from_fn(|k| a[k] + (s * b[k] - a[k]) * t);
    if kind == Kind::Rotation {
        let l = out.iter().map(|x| x * x).sum::<f32>().sqrt();
        if l > 0.0 && l.is_finite() {
            out = out.map(|x| x / l);
        } else {
            out = a;
        }
    }
    out
}

/// The keys of one dense series, as frames: the ends, every frame a
/// split keeps, in order.
fn reduce_series(kind: Kind, series: &[[f32; 4]], tolerance: f32) -> Vec<usize> {
    let last = series.len() - 1;
    if series
        .iter()
        .all(|v| distance(kind, *v, series[0]) <= tolerance)
    {
        return vec![0];
    }
    let mut keep = vec![false; series.len()];
    keep[0] = true;
    keep[last] = true;
    let mut ranges = vec![(0, last)];
    while let Some((a, b)) = ranges.pop() {
        let mut worst = (0.0f32, a);
        for i in a + 1..b {
            let t = (i - a) as f32 / (b - a) as f32;
            let e = distance(kind, series[i], lerp(kind, series[a], series[b], t));
            if e > worst.0 {
                worst = (e, i);
            }
        }
        if worst.0 > tolerance {
            keep[worst.1] = true;
            ranges.push((a, worst.1));
            ranges.push((worst.1, b));
        }
    }
    (0..series.len()).filter(|&i| keep[i]).collect()
}

/// The dense `poses` of a clip (one per frame, from
/// [`crate::anim_sample::AnimationClip::sample_frame`]) as keys that bake
/// back within `tolerance` at every frame. Every pose has the first one's
/// node count.
pub fn reduce(poses: &[Pose], tolerance: Tolerance) -> Keys {
    let nodes = poses.first().map_or(0, |p| p.nodes.len());
    let mut channels = Vec::new();
    for node in 0..nodes {
        for kind in Kind::ALL {
            let driven = poses
                .iter()
                .any(|p| p.nodes.get(node).is_some_and(|n| supplied(kind, n)));
            if !driven {
                continue;
            }
            let series: Vec<[f32; 4]> = poses
                .iter()
                .map(|p| p.nodes.get(node).map_or([0.0; 4], |n| value(kind, n)))
                .collect();
            let keys = reduce_series(kind, &series, tolerance.of(kind))
                .into_iter()
                .map(|frame| Key {
                    frame,
                    value: series[frame],
                })
                .collect();
            channels.push(Channel { node, kind, keys });
        }
    }
    Keys {
        frames: poses.len(),
        nodes,
        tolerance,
        channels,
    }
}
