//! Studio's in-memory sparse animation document. See the animation editor
//! in `docs/editor/studio-redesign.md`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tags::anim_keys::{self, Keys, Kind};
use tags::anim_sample::{NodeTransform, Pose};

mod angles;
mod check;
mod gltf;
mod retarget;
mod source;

pub use angles::{
    angles_near, angles_to_rotation, rotation_to_angles, tangent_from_angles, tangent_to_angles,
};
pub use check::{Check, FootContact, NodeCheck};
pub use gltf::{GltfClip, gltf_clips, read_gltf};
pub use retarget::{DroppedChannel, NodeMapping, RetargetReport, retarget, retarget_report};
pub use source::{
    ClipKind, Encoded, Operation, Source, SourceChannel, SourceKey, SourceKind, SourceNode, encode,
};

/// A graph node, indexed by its position in the graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub name: Option<String>,
    pub parent: i16,
}

/// Interpolation from a key to the next key. Outside the keyed interval
/// the channel holds the nearest key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Interp {
    Step,
    #[default]
    Linear,
    Smooth,
}

impl Interp {
    pub const ALL: [Self; 3] = [Self::Step, Self::Linear, Self::Smooth];

    pub fn name(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::Linear => "linear",
            Self::Smooth => "smooth",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub frame: usize,
    /// Quaternion xyzw, translation xyz, or scale in the first component.
    pub value: [f32; 4],
    pub interp: Interp,
    /// Cubic slope per frame, in the channel's stored coordinates.
    pub tangent: [f32; 4],
}

type Track = (usize, Kind);
type Tracks = BTreeMap<Track, Arc<Vec<Key>>>;

/// Consecutive writes from a single pointer gesture share an undo entry.
/// An intervening ordinary write, undo, or redo starts a new entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gesture(u64);

#[derive(Debug)]
struct Undo {
    tracks: Tracks,
    gesture: Option<Gesture>,
    /// The step's number ([`AnimDoc::share_steps`]).
    serial: u64,
}

/// A clip, its skeleton, and its edit history. Track storage is shared
/// with undo entries until an edit writes that track.
#[derive(Debug)]
pub struct AnimDoc {
    name: String,
    frames: usize,
    nodes: Vec<Node>,
    tracks: Tracks,
    undo: Vec<Undo>,
    /// Each undone step: the tracks it gives back, and its number.
    redo: Vec<(Tracks, u64)>,
    revision: u64,
    next_gesture: u64,
    /// Numbers the steps: this document's own counter until the host
    /// shares its ([`Self::share_steps`]).
    steps: Arc<AtomicU64>,
}

impl AnimDoc {
    pub fn new(name: String, frames: usize, nodes: Vec<Node>) -> Result<Self, String> {
        if name.trim().is_empty() {
            return Err("the animation needs a name".to_owned());
        }
        if !(1..=i16::MAX as usize).contains(&frames) {
            return Err(format!("frame count must be 1 to {}", i16::MAX));
        }
        Ok(Self {
            name,
            frames,
            nodes,
            tracks: Tracks::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            next_gesture: 0,
            steps: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Import AN1's reduction, keeping its linear interpolation.
    pub fn from_reduced(name: String, nodes: Vec<Node>, keys: Keys) -> Result<Self, String> {
        if nodes.len() != keys.nodes {
            return Err("the reduction and graph have different node counts".to_owned());
        }
        let mut doc = Self::new(name, keys.frames, nodes)?;
        for channel in keys.channels {
            let track = (channel.node, channel.kind);
            let keys = channel
                .keys
                .into_iter()
                .map(|key| Key {
                    frame: key.frame,
                    value: key.value,
                    interp: Interp::Linear,
                    tangent: [0.0; 4],
                })
                .collect::<Vec<_>>();
            doc.validate_keys(track, &keys)?;
            if !keys.is_empty() {
                doc.tracks.insert(track, Arc::new(keys));
            }
        }
        Ok(doc)
    }

    /// Copy the active tracks into a separately editable document.
    pub fn duplicate(&self, name: String) -> Result<Self, String> {
        let mut doc = Self::new(name, self.frames, self.nodes.clone())?;
        doc.tracks = self.tracks.clone();
        Ok(doc)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn frames(&self) -> usize {
        self.frames
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn channels(&self) -> impl Iterator<Item = (usize, Kind, &[Key])> {
        self.tracks
            .iter()
            .map(|(&(node, kind), keys)| (node, kind, keys.as_slice()))
    }

    pub fn keys(&self, node: usize, kind: Kind) -> &[Key] {
        self.tracks.get(&(node, kind)).map_or(&[], |k| k.as_slice())
    }

    pub fn key(&self, node: usize, kind: Kind, frame: usize) -> Option<Key> {
        let keys = self.keys(node, kind);
        keys.binary_search_by_key(&frame, |k| k.frame)
            .ok()
            .map(|i| keys[i])
    }

    pub fn begin_gesture(&mut self) -> Gesture {
        self.next_gesture += 1;
        Gesture(self.next_gesture)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Number this document's steps on `steps`, the counter its host
    /// numbers its other undo stacks on. A document whose steps come from
    /// several places (a graph's tab holds its tags' writes and each of
    /// its animation documents) undoes them in the order they were made:
    /// [`Self::step_serial`] and [`Self::redo_serial`] give the numbers to
    /// compare by. A document numbers its steps on its own counter until
    /// the host shares one.
    pub fn share_steps(&mut self, steps: Arc<AtomicU64>) {
        self.steps = steps;
    }

    /// The number of the newest step not undone ([`Self::share_steps`]),
    /// or `None` when every step is undone.
    pub fn step_serial(&self) -> Option<u64> {
        self.undo.last().map(|u| u.serial)
    }

    /// The number of the oldest undone step, the next a redo takes: the
    /// steps of one document are undone newest first.
    pub fn redo_serial(&self) -> Option<u64> {
        self.redo.last().map(|(_, serial)| *serial)
    }

    pub fn undo(&mut self) -> bool {
        let Some(entry) = self.undo.pop() else {
            return false;
        };
        self.redo
            .push((std::mem::replace(&mut self.tracks, entry.tracks), entry.serial));
        if let Some(previous) = self.undo.last_mut() {
            previous.gesture = None;
        }
        self.revision += 1;
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some((tracks, serial)) = self.redo.pop() else {
            return false;
        };
        self.undo.push(Undo {
            tracks: std::mem::replace(&mut self.tracks, tracks),
            gesture: None,
            serial,
        });
        self.revision += 1;
        true
    }

    /// Insert a key or update its value. Keep the old interpolation and
    /// tangent when updating unless interpolation is supplied.
    pub fn set_key(
        &mut self,
        node: usize,
        kind: Kind,
        frame: usize,
        value: [f32; 4],
        interp: Option<Interp>,
    ) -> Result<bool, String> {
        self.set_key_during_gesture(node, kind, frame, value, interp, None)
    }

    pub fn set_key_during_gesture(
        &mut self,
        node: usize,
        kind: Kind,
        frame: usize,
        value: [f32; 4],
        interp: Option<Interp>,
        gesture: Option<Gesture>,
    ) -> Result<bool, String> {
        let mut keys = self.keys(node, kind).to_vec();
        let index = keys.binary_search_by_key(&frame, |key| key.frame);
        let mut value = if index.is_ok_and(|i| keys[i].value == value) {
            value
        } else {
            clean_value(kind, value)?
        };
        let mut key = Key {
            frame,
            value,
            interp: interp.unwrap_or_default(),
            tangent: [0.0; 4],
        };
        if let Ok(i) = index {
            let old = keys[i];
            if kind == Kind::Rotation && dot(old.value, value) < 0.0 {
                value = value.map(|v| -v);
            }
            key.value = value;
            key.interp = interp.unwrap_or(old.interp);
            key.tangent = old.tangent;
            keys[i] = key;
        } else if let Err(i) = index {
            keys.insert(i, key);
        }
        self.replace_keys(node, kind, keys, gesture)
    }

    /// Set the slope at a key. Both sides of a smooth segment use the same
    /// key tangent, including when the key's own outgoing segment is linear.
    pub fn set_tangent(
        &mut self,
        node: usize,
        kind: Kind,
        frame: usize,
        tangent: [f32; 4],
        gesture: Option<Gesture>,
    ) -> Result<bool, String> {
        let mut keys = self.keys(node, kind).to_vec();
        let i = keys
            .binary_search_by_key(&frame, |key| key.frame)
            .map_err(|_| format!("no {} key at frame {frame}", kind.name()))?;
        keys[i].tangent = tangent;
        keys[i].tangent[kind.width()..].fill(0.0);
        self.replace_keys(node, kind, keys, gesture)
    }

    pub fn remove_key(&mut self, node: usize, kind: Kind, frame: usize) -> Result<bool, String> {
        self.validate_position(node, frame)?;
        let mut keys = self.keys(node, kind).to_vec();
        if let Ok(i) = keys.binary_search_by_key(&frame, |key| key.frame) {
            keys.remove(i);
            return self.replace_keys(node, kind, keys, None);
        }
        Ok(false)
    }

    /// Replace one track atomically, for timeline edits spanning several
    /// keys. A collision or out-of-range frame leaves the track untouched.
    pub fn replace_keys(
        &mut self,
        node: usize,
        kind: Kind,
        keys: Vec<Key>,
        gesture: Option<Gesture>,
    ) -> Result<bool, String> {
        let track = (node, kind);
        self.validate_keys(track, &keys)?;
        if self.keys(node, kind) == keys {
            return Ok(false);
        }
        let merge = gesture.is_some() && self.undo.last().is_some_and(|u| u.gesture == gesture);
        if !merge {
            self.undo.push(Undo {
                tracks: self.tracks.clone(),
                gesture,
                serial: self.steps.fetch_add(1, Ordering::Relaxed) + 1,
            });
            self.redo.clear();
        }
        if keys.is_empty() {
            self.tracks.remove(&track);
        } else {
            self.tracks.insert(track, Arc::new(keys));
        }
        if merge && self.undo.last().is_some_and(|u| u.tracks == self.tracks) {
            self.undo.pop();
        }
        self.revision += 1;
        Ok(true)
    }

    fn validate_position(&self, node: usize, frame: usize) -> Result<(), String> {
        if node >= self.nodes.len() {
            return Err(format!("no node {node} of {}", self.nodes.len()));
        }
        if frame >= self.frames {
            return Err(format!("no frame {frame} of {}", self.frames));
        }
        Ok(())
    }

    fn validate_keys(&self, (node, kind): Track, keys: &[Key]) -> Result<(), String> {
        self.validate_position(node, 0)?;
        if keys.windows(2).any(|pair| pair[0].frame >= pair[1].frame) {
            return Err("keys must be in frame order with one key per frame".to_owned());
        }
        for key in keys {
            self.validate_position(node, key.frame)?;
            if !key.value.into_iter().chain(key.tangent).all(f32::is_finite) {
                return Err("key values and tangents must be finite".to_owned());
            }
            if kind == Kind::Rotation && (dot(key.value, key.value) - 1.0).abs() > 1e-5 {
                return Err("rotation keys must be unit quaternions".to_owned());
            }
        }
        Ok(())
    }

    pub fn value_at(&self, node: usize, kind: Kind, frame: f32) -> Option<[f32; 4]> {
        value_at(kind, self.keys(node, kind), frame)
    }

    /// Sample local channels, leaving unkeyed channels at identity with
    /// their flags clear so the pose pipeline can merge skeleton defaults.
    pub fn sample(&self, frame: f32) -> Pose {
        let mut nodes = vec![NodeTransform::default(); self.nodes.len()];
        for (node, kind, keys) in self.channels() {
            let Some(value) = value_at(kind, keys, frame) else {
                continue;
            };
            let n = &mut nodes[node];
            match kind {
                Kind::Rotation => {
                    n.rotation = value;
                    n.has_rotation = true;
                }
                Kind::Translation => {
                    n.translation = [value[0], value[1], value[2]];
                    n.has_translation = true;
                }
                Kind::Scale => {
                    n.scale = value[0];
                    n.has_scale = true;
                }
            }
        }
        Pose { nodes }
    }

    pub fn bake(&self) -> Vec<Pose> {
        (0..self.frames).map(|f| self.sample(f as f32)).collect()
    }
}

fn dot(a: [f32; 4], b: [f32; 4]) -> f32 {
    a.into_iter().zip(b).map(|(x, y)| x * y).sum()
}

fn normalize(q: [f32; 4]) -> Option<[f32; 4]> {
    let length = q.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>().sqrt();
    (length > 0.0 && length.is_finite()).then(|| q.map(|v| (f64::from(v) / length) as f32))
}

fn clean_value(kind: Kind, mut value: [f32; 4]) -> Result<[f32; 4], String> {
    value[kind.width()..].fill(0.0);
    if !value.into_iter().all(f32::is_finite) {
        return Err("the key value must be finite".to_owned());
    }
    // Preserve a unit key's stored value when only interpolation changes.
    if kind == Kind::Rotation && (dot(value, value) - 1.0).abs() > f32::EPSILON * 2.0 {
        return normalize(value).ok_or_else(|| "rotation needs a nonzero quaternion".to_owned());
    }
    Ok(value)
}

fn value_at(kind: Kind, keys: &[Key], frame: f32) -> Option<[f32; 4]> {
    let first = keys.first()?;
    let last = keys.last()?;
    if frame.is_nan() || frame <= first.frame as f32 {
        return Some(first.value);
    }
    if frame >= last.frame as f32 {
        return Some(last.value);
    }
    let after = keys.partition_point(|k| (k.frame as f32) <= frame);
    let (a, b) = (keys[after - 1], keys[after]);
    if frame == a.frame as f32 || a.interp == Interp::Step {
        return Some(a.value);
    }
    let span = (b.frame - a.frame) as f32;
    let t = (frame - a.frame as f32) / span;
    Some(match a.interp {
        Interp::Step => a.value,
        Interp::Linear => {
            let value = anim_keys::lerp(kind, a.value, b.value, t);
            if value.into_iter().all(f32::is_finite) {
                value
            } else {
                // Opposite finite endpoints can overflow the subtraction.
                let t = f64::from(t);
                std::array::from_fn(|i| {
                    (f64::from(a.value[i]) * (1.0 - t) + f64::from(b.value[i]) * t) as f32
                })
            }
        }
        Interp::Smooth => smooth(kind, a, b, span, t),
    })
}

/// Hermite interpolation. For a rotation the endpoints and derivatives
/// have to share a hemisphere, and every sample goes back onto the sphere.
fn smooth(kind: Kind, a: Key, b: Key, span: f32, t: f32) -> [f32; 4] {
    let s = if kind == Kind::Rotation && dot(a.value, b.value) < 0.0 {
        -1.0
    } else {
        1.0
    };
    let (t, span) = (f64::from(t), f64::from(span));
    let t2 = t * t;
    let t3 = t2 * t;
    let out = std::array::from_fn(|i| {
        let value = (2.0 * t3 - 3.0 * t2 + 1.0) * f64::from(a.value[i])
            + (t3 - 2.0 * t2 + t) * span * f64::from(a.tangent[i])
            + (-2.0 * t3 + 3.0 * t2) * f64::from(s * b.value[i])
            + (t3 - t2) * span * f64::from(s * b.tangent[i]);
        value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
    });
    if kind == Kind::Rotation {
        normalize(out).unwrap_or_else(|| anim_keys::lerp(kind, a.value, b.value, t as f32))
    } else {
        out
    }
}

#[cfg(test)]
mod tests;
