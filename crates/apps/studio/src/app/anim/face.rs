//! The face preview (docs/editor/rich-editors.md, "Sounds and facial
//! animation", slice A3): a model posed with its graph's `talk` overlay
//! at the weight a sound's mouth curve gives at one instant.
//!
//! **What the game does (E).** A speaking unit samples the curve by audio
//! time, one aperture byte per 30 Hz tick scaled by 1/255 (`0x218e50`),
//! and blends frame zero of its `talk` overlay by that weight
//! (`0x1cc150`). The overlay is found in the graph's mode tree like any
//! overlay: mode, weapon class and weapon type, each exact first and then
//! `any` (`0x2963f0`), in the type's overlay block (`0x1db170`).
//!
//! **What the preview does (Design).** The model is not in a game, so it
//! has no mode or weapon: the lookup asks for mode `combat`, class `any`
//! and type `any`, which on a converted biped is where the converter
//! puts `talk` (`h1-assets` `jmad::biped`). The base under it is the
//! `idle` action the same lookup finds, at frame zero, else the model's
//! rest pose. The unit's `lip_sync_onset_weight` and
//! `lip_sync_release_weight` are not applied: the weight is the tick's
//! byte as stored. An overlay that lives in an inherited graph is refused
//! by name, not followed.

use map::TagId;
use tags::anim::AnimationGraph;
use tags::anim_sample::{NodeTransform, Pose};
use tags::object::Context;
use tags::sound::MOUTH_HZ;

use super::{AnimError, PoseAnswer, Raw, id_text};
use crate::app::App;
use crate::app::audition::SoundError;

/// The overlay the game blends by the mouth curve.
pub const TALK: &str = "talk";
/// The base action under it.
pub const IDLE: &str = "idle";
/// The mode, weapon class and weapon type the preview looks up from.
pub const LOOKUP: [&str; 3] = ["combat", "any", "any"];

/// One leaf of a graph's mode tree, its names resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeEntry {
    pub mode: String,
    pub class: String,
    pub kind: String,
    pub name: String,
    /// From the type's overlay block; else its action block.
    pub overlay: bool,
    /// -1 for this graph, else a place in its inheritance list.
    pub graph_index: i16,
    pub animation: i16,
}

impl TreeEntry {
    /// `mode:class:type:name`, as the answer names where it was found.
    pub fn path(&self) -> String {
        format!("{}:{}:{}:{}", self.mode, self.class, self.kind, self.name)
    }
}

/// Every action and overlay of `graph`'s mode tree, names resolved
/// through `strings` (a name that does not resolve is empty and matches
/// nothing).
pub fn tree(graph: &AnimationGraph, strings: &[Option<String>]) -> Vec<TreeEntry> {
    let name = |id: tags::StringId| id.resolve(strings).ok().flatten().unwrap_or("").to_owned();
    let mut out = Vec::new();
    for mode in &graph.modes {
        for class in &mode.classes {
            for kind in &class.types {
                for (overlay, list) in [(false, &kind.actions), (true, &kind.overlays)] {
                    for a in list {
                        out.push(TreeEntry {
                            mode: name(mode.name),
                            class: name(class.name),
                            kind: name(kind.name),
                            name: name(a.name),
                            overlay,
                            graph_index: a.graph_index,
                            animation: a.animation_index,
                        });
                    }
                }
            }
        }
    }
    out
}

/// The engine's lookup order over `tree`: `key` in the overlay (or action)
/// block of `mode:class:type`, each tried exact and then `any`, mode
/// outermost.
pub fn find<'t>(
    tree: &'t [TreeEntry],
    [mode, class, kind]: [&str; 3],
    key: &str,
    overlay: bool,
) -> Option<&'t TreeEntry> {
    let (modes, classes, kinds) = (either(mode), either(class), either(kind));
    for m in &modes {
        for c in &classes {
            for t in &kinds {
                if let Some(e) = tree.iter().find(|e| {
                    e.overlay == overlay
                        && e.mode == *m
                        && e.class == *c
                        && e.kind == *t
                        && e.name == key
                }) {
                    return Some(e);
                }
            }
        }
    }
    None
}

/// A lookup word, then `any` unless it is `any`.
fn either(exact: &str) -> Vec<&str> {
    if exact == "any" {
        vec!["any"]
    } else {
        vec![exact, "any"]
    }
}

/// The weight the face takes at one instant of a voiced permutation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceWeight {
    /// The 30 Hz tick the instant falls in.
    pub tick: usize,
    /// The curve's length in ticks.
    pub ticks: usize,
    /// That tick's aperture byte; `None` past the curve's end.
    pub byte: Option<u8>,
    /// `byte / 255`, 0 past the end (the mouth closed).
    pub weight: f32,
}

/// The weight at `ms` into a sound whose curve is `curve`: the byte of
/// the tick `ms` falls in, scaled by 1/255, as the game samples it.
pub fn mouth_weight(curve: &[u8], ms: u64) -> FaceWeight {
    let tick = usize::try_from(ms * u64::from(MOUTH_HZ) / 1000).unwrap_or(usize::MAX);
    let byte = curve.get(tick).copied();
    FaceWeight {
        tick,
        ticks: curve.len(),
        byte,
        weight: byte.map_or(0.0, |b| f32::from(b) / 255.0),
    }
}

/// A face posed: what was found and the pose.
#[derive(Clone)]
pub struct FacePose {
    /// The overlay posed: where the lookup found it (`mode:class:type:talk`),
    /// or the animation named in its place.
    pub talk: String,
    /// The base action's path, `None` for the model's rest pose.
    pub base: Option<String>,
    pub weight: f32,
    pub pose: PoseAnswer,
}

/// A graph's identity for the face cache, by the object posed.
type ObjectKey = super::GraphKey;

impl App {
    /// The objects the face preview offers: the shown map's bipeds, by
    /// name.
    pub fn face_objects(&self) -> Vec<(TagId, String)> {
        let mut out: Vec<_> = self
            .shown_tags()
            .tags()
            .iter()
            .filter(|t| t.group.0 == *b"bipd")
            .map(|t| (t.id, t.name.clone()))
            .collect();
        out.sort_by(|a, b| a.1.cmp(&b.1));
        out
    }

    /// The animation graph `object`'s model names, read once per object.
    pub fn face_graph(&mut self, object: TagId) -> Result<TagId, AnimError> {
        let key: ObjectKey = self.anim_graph_key(object);
        if let Some(found) = self.anim.face_graphs.get(&key) {
            return found.clone();
        }
        let found = (|| {
            let mut map = self
                .open_shown_map()
                .map_err(|e| AnimError::Failed(format!("the map: {e}")))?;
            let ctx = Context::read(&mut map)
                .map_err(|e| AnimError::Failed(format!("the map's tags: {e}")))?;
            let (_, model) = scenario_edit::object_collision::resolve(&ctx, object)
                .map_err(|e| AnimError::Refused(format!("{}: {e}", id_text(object))))?;
            model
                .and_then(|m| m.animation.map(|a| a.id))
                .filter(|id| *id != TagId::NULL)
                .ok_or_else(|| {
                    AnimError::Refused(format!(
                        "{}: its model names no animation graph",
                        id_text(object)
                    ))
                })
        })();
        // A failure to read is asked again next time; an answer is kept.
        if !matches!(found, Err(AnimError::Failed(_))) {
            self.anim.face_graphs.insert(key, found.clone());
        }
        found
    }

    /// `object` posed with its graph's `talk` overlay at `weight` (0 to 1)
    /// over its `idle` (module docs). `overlay` names another of the
    /// graph's animations to blend in its place.
    pub fn face_pose(
        &mut self,
        object: TagId,
        weight: f32,
        overlay: Option<usize>,
    ) -> Result<FacePose, AnimError> {
        let graph = self.face_graph(object)?;
        self.ensure_anim(graph)?;
        let mut files = self
            .shown_raw_files()
            .map_err(|e| AnimError::Failed(format!("the map's resource files: {e}")))?;
        let mut reader = Raw(&mut files);
        let loaded = self.anim.loaded.as_mut().expect("loaded");
        let own = |e: &TreeEntry, what: &str| -> Result<usize, AnimError> {
            if e.graph_index != -1 {
                return Err(AnimError::Refused(format!(
                    "{}: its {what} ({}) is in inherited graph {}, which the preview does not follow",
                    loaded.name,
                    e.path(),
                    e.graph_index
                )));
            }
            usize::try_from(e.animation)
                .ok()
                .filter(|&i| i < loaded.graph.animations.len())
                .ok_or_else(|| {
                    AnimError::Failed(format!(
                        "{}: {} names animation {}, past the graph's {}",
                        loaded.name,
                        e.path(),
                        e.animation,
                        loaded.graph.animations.len()
                    ))
                })
        };
        let (talk_index, talk) = match overlay {
            Some(i) => {
                let name = loaded.animation_names.get(i).cloned().ok_or_else(|| {
                    AnimError::Refused(format!(
                        "no animation {i} of {}",
                        loaded.graph.animations.len()
                    ))
                })?;
                (i, name)
            }
            None => {
                let e = find(&loaded.tree, LOOKUP, TALK, true).ok_or_else(|| {
                    AnimError::Refused(format!(
                        "{}: no {TALK} overlay at {m}:{c}:{t} or any:any:any",
                        loaded.name,
                        m = LOOKUP[0],
                        c = LOOKUP[1],
                        t = LOOKUP[2],
                    ))
                })?;
                (own(e, "talk overlay")?, e.path())
            }
        };
        let base = match find(&loaded.tree, LOOKUP, IDLE, false) {
            Some(e) => Some((own(e, "idle")?, e.path())),
            None => None,
        };
        // Frame zero of the overlay, as the game blends it.
        let sample = loaded.sample_clip(talk_index, 0.0, &mut reader)?;
        let base_pose = match &base {
            Some((i, _)) => loaded.sample_clip(*i, 0.0, &mut reader)?,
            None => Pose {
                nodes: vec![NodeTransform::default(); sample.nodes.len()],
            },
        };
        let weight = if weight.is_finite() {
            weight.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut pose = loaded.pose(
            loaded.summary(talk_index),
            0.0,
            base_pose,
            Some((&sample, weight)),
            Some(&id_text(object)),
        )?;
        if base.is_none() {
            pose.notes.push(format!(
                "no {IDLE} action: the overlay is on the model's rest pose"
            ));
        }
        Ok(FacePose {
            talk,
            base: base.map(|(_, path)| path),
            weight,
            pose,
        })
    }

    /// The face's weight `ms` into permutation `permutation` of pitch range
    /// `range` of the sound `id`; `None` when the permutation is unvoiced.
    pub fn face_weight(
        &mut self,
        id: TagId,
        range: usize,
        permutation: usize,
        ms: u64,
    ) -> Result<Option<FaceWeight>, SoundError> {
        Ok(self
            .sound_mouth(id, range, permutation)?
            .map(|curve| mouth_weight(&curve, ms)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, overlay: bool, animation: i16) -> TreeEntry {
        let p: Vec<&str> = path.split(':').collect();
        TreeEntry {
            mode: p[0].into(),
            class: p[1].into(),
            kind: p[2].into(),
            name: p[3].into(),
            overlay,
            graph_index: -1,
            animation,
        }
    }

    /// The preview at a tick uses that tick's byte: 33 ms is tick 0, 34 ms
    /// tick 1, and past the curve the mouth is closed.
    #[test]
    fn the_weight_at_an_instant_is_its_ticks_byte() {
        let curve = [0u8, 51, 255];
        let at = |ms| mouth_weight(&curve, ms);
        assert_eq!((at(0).tick, at(0).byte, at(0).weight), (0, Some(0), 0.0));
        assert_eq!(at(33).tick, 0);
        assert_eq!(
            (at(34).tick, at(34).byte, at(34).weight),
            (1, Some(51), 0.2)
        );
        assert_eq!((at(67).tick, at(67).weight), (2, 1.0));
        let past = at(100);
        assert_eq!(
            (past.tick, past.ticks, past.byte, past.weight),
            (3, 3, None, 0.0)
        );
    }

    /// `talk` is found in the overlay block, exact mode first, then `any`;
    /// an action of the same name is not an overlay.
    #[test]
    fn talk_follows_the_engines_lookup_order() {
        let tree = vec![
            entry("combat:any:any:talk", false, 9),
            entry("any:any:any:talk", true, 2),
            entry("combat:any:any:idle", false, 0),
        ];
        let talk = find(&tree, LOOKUP, TALK, true).unwrap();
        assert_eq!(
            (talk.path(), talk.animation),
            ("any:any:any:talk".to_owned(), 2)
        );
        let mut tree = tree;
        tree.push(entry("combat:any:any:talk", true, 3));
        assert_eq!(find(&tree, LOOKUP, TALK, true).unwrap().animation, 3);
        assert_eq!(find(&tree, LOOKUP, IDLE, false).unwrap().animation, 0);
        // Another mode's or class's talk is not this lookup's.
        let other = vec![
            entry("patrol:any:any:talk", true, 4),
            entry("combat:rifle:any:talk", true, 5),
        ];
        assert_eq!(find(&other, LOOKUP, TALK, true), None);
    }
}
