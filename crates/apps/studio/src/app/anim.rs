//! Model poses: a `jmad` tag's animations listed, and one sampled on the
//! model that uses it (docs/editor/rich-editors.md, "Model poses", slice
//! M1). The Animation window ([`crate::ui`]) and the agent API
//! (`anim.list`, `anim.pose`, [`crate::api::anim`]) both go through the
//! `App` methods here, so they are one path.
//!
//! **How a pose is built.** [`tags::anim_sample::AnimationClip::sample`]
//! gives the animation's own channels per node, in the node's local
//! space; a channel it supplies none for is the render model node's own
//! default (`mode` node `+0xc` translation, `+0x18` orientation), as the
//! game composes a model. The graph's nodes and the render model's are
//! the same list, indexed alike (the engine writes an animation's
//! channels by node index into the object's nodes), which is what the
//! graph is refused over when the counts disagree. The merged locals
//! compose up the render model's parent links, Halo 2's rotation rule,
//! which the Halo 1 converter writes in and its disc round-trip checks
//! (`h1-assets/src/models/skin_tests.rs`); the world matrices take the
//! place of the rest pose the native view draws
//! ([`scenario_edit::native_view::posed_nodes`]). An overlay animation
//! applies over the base with the engine's scaled-add rule
//! ([`tags::anim_sample::Pose::add_overlay`]).
//!
//! The graph, its clips and the models are read once per (map, tag) and
//! kept: sampling a frame needs no map access, so scrubbing stays cheap.
//! The window and the API read the map the Tags tool shows
//! ([`App::open_shown_map`]), never writing it.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use map::{RawPointer, TagId};
use scenario_edit::geometry::Camera;
use scenario_edit::native_view::posed_nodes;
use scenario_edit::native_view::preview::{self, Matrix, Piece};
use tags::anim::AnimationGraph;
use tags::anim_sample::{AnimationClip, Codec, NodeDefault};
use tags::object::Context;
use tags::sound::ResourceReader;

use super::App;

/// The renders' background, sRGB-encoded: `model.render`'s own.
pub(crate) const BACKGROUND: [f64; 3] = [0.10, 0.11, 0.13];

/// The orbit a posed model is framed at: the ground view's angles.
const YAW: f32 = 0.8;
const PITCH: f32 = 0.35;

/// Why animations are not listed or a pose not built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnimError {
    /// The graph or model is checked and cannot be shown here: why.
    Refused(String),
    /// Something failed while reading or opening: why.
    Failed(String),
}

impl fmt::Display for AnimError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Refused(s) | Self::Failed(s) => f.write_str(s),
        }
    }
}

/// One animation of the graph, as the list and the picker show it.
#[derive(Clone, Debug)]
pub struct AnimSummary {
    pub index: usize,
    /// The animation's own name, resolved through the map's strings.
    pub name: String,
    /// `base`, `overlay`, `replacement` or `unknown`.
    pub kind: &'static str,
    pub frames: usize,
    /// The static and animated streams' codecs, where present.
    pub codecs: [Option<Codec>; 2],
    /// Codec 8 (blend screen): the "frames" are the cells of an aim grid,
    /// not points in time, and the clip is not one to play.
    pub aim_grid: bool,
    /// Why the clip does not build, when it does not.
    pub error: Option<String>,
}

/// A model of the map whose `hlmt` names the graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelUser {
    pub object: TagId,
    pub name: String,
}

/// A graph read: its animations, and the models that use it.
#[derive(Clone, Debug)]
pub struct AnimInfo {
    pub nodes: usize,
    pub animations: Vec<AnimSummary>,
    pub users: Vec<ModelUser>,
}

/// One node of the pose: the local transform that composes (the
/// animation's channel where it has one, the node's default where it does
/// not), which channels the animation supplied, and the posed world
/// matrix when a model was posed.
#[derive(Clone, Debug)]
pub struct PoseNode {
    /// The graph node's name, resolved through the map's strings.
    pub name: Option<String>,
    pub parent: i16,
    pub rotation: [f32; 4],
    pub translation: [f32; 3],
    pub scale: f32,
    /// `[rotation, translation, scale]`: whether the animation (base or
    /// overlay) supplied the channel; where false, the value is the
    /// node's default.
    pub has: [bool; 3],
    /// The posed model-space matrix, column-major `T·R·S`; `None` when no
    /// model was posed (the graph alone gives no skeleton to compose on).
    pub world: Option<tags::anim_sample::Mat4>,
}

/// A posed model, ready to draw.
#[derive(Clone, Debug)]
pub struct PoseModel {
    /// The object whose model was posed.
    pub object: TagId,
    pub name: String,
    pub piece: Arc<Piece>,
    /// One node matrix per model node, in place of the rest pose.
    pub nodes: Vec<Matrix>,
    /// The sphere the posed vertices lie in, for culling.
    pub sphere: ([f32; 3], f32),
    /// The camera that frames the posed model.
    pub camera: Camera,
}

/// One animation, one frame, answered.
#[derive(Clone, Debug)]
pub struct PoseAnswer {
    pub animation: AnimSummary,
    /// The frame sampled (fractional frames blend the neighbours).
    pub frame: f32,
    pub nodes: Vec<PoseNode>,
    /// The model posed, when the map has one that uses the graph.
    pub model: Option<PoseModel>,
    pub notes: Vec<String>,
}

/// What was read for one (map, tag), kept between calls.
struct Loaded {
    key: (String, TagId),
    graph: AnimationGraph,
    /// The graph's node and animation names, resolved through the map's
    /// strings (a name that does not resolve is `animation <index>`).
    node_names: Vec<Option<String>>,
    animation_names: Vec<String>,
    /// Resource blobs by the graph's resource index, read on demand and
    /// shared by every clip built from one.
    resources: HashMap<u32, Arc<[u8]>>,
    /// Clips by animation index: what built, or why it did not.
    clips: HashMap<usize, Result<AnimationClip, String>>,
    /// The models that use the graph, each with the piece to pose.
    models: Vec<(ModelUser, Result<Arc<Piece>, String>)>,
}

impl Loaded {
    /// The clip of animation `index`, building it (and reading its
    /// resource) the first time. The error is kept: a damaged animation
    /// is named every time it is asked for, not re-read.
    fn clip(
        &mut self,
        index: usize,
        reader: &mut dyn ResourceReader,
    ) -> Result<AnimationClip, String> {
        if let Some(r) = self.clips.get(&index) {
            return r.clone();
        }
        let anim =
            self.graph.animations.get(index).ok_or_else(|| {
                format!("no animation {index} of {}", self.graph.animations.len())
            })?;
        let built = if !anim.in_resource() {
            Err("the animation's data is not in a resource (inline data is unsupported)".to_owned())
        } else {
            let resource = self.resource(anim.resource_index, reader)?;
            self.graph
                .clip_from_shared(index, &resource)
                .map_err(|e| e.to_string())
        };
        self.clips.insert(index, built.clone());
        built
    }

    fn resource(
        &mut self,
        index: u32,
        reader: &mut dyn ResourceReader,
    ) -> Result<Arc<[u8]>, String> {
        if let Some(r) = self.resources.get(&index) {
            return Ok(Arc::clone(r));
        }
        let r = self
            .graph
            .read_resource(index as usize, reader)
            .map_err(|e| e.to_string())?;
        self.resources.insert(index, Arc::clone(&r));
        Ok(r)
    }

    /// The row the list shows for animation `index` (its clip built
    /// first).
    fn summary(&self, index: usize) -> AnimSummary {
        let a = &self.graph.animations[index];
        let clip = self.clips.get(&index).and_then(|c| c.as_ref().ok());
        AnimSummary {
            index,
            name: self.animation_names[index].clone(),
            kind: match a.anim_type {
                tags::anim::AnimationType::Base => "base",
                tags::anim::AnimationType::Overlay => "overlay",
                tags::anim::AnimationType::Replacement => "replacement",
                tags::anim::AnimationType::Unknown(_) => "unknown",
            },
            frames: usize::try_from(a.frame_count.max(0)).unwrap_or(0),
            codecs: clip.map(|c| c.codecs()).unwrap_or([None, None]),
            aim_grid: clip.is_some_and(|c| c.is_aim_grid()),
            error: self
                .clips
                .get(&index)
                .and_then(|c| c.as_ref().err().cloned()),
        }
    }

    /// Which of the graph's models to pose: the one named, the only one
    /// when none is, or `None` when the map has none. Several without a
    /// name, or a name that is no user, are refused by name.
    fn pick_model(&self, model: Option<&str>) -> Result<Option<usize>, AnimError> {
        match model {
            None => match self.models.len() {
                0 => Ok(None),
                1 => Ok(Some(0)),
                n => Err(AnimError::Refused(format!(
                    "{n} models use this graph: {}",
                    self.models
                        .iter()
                        .map(|(u, _)| u.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))),
            },
            Some(want) => {
                let found = self.models.iter().position(|(u, _)| {
                    u.name.eq_ignore_ascii_case(want)
                        || id_text(u.object).eq_ignore_ascii_case(want)
                });
                match found {
                    Some(i) => Ok(Some(i)),
                    None => Err(AnimError::Refused(format!(
                        "{want:?} is not one of the models using this graph ({})",
                        self.models
                            .iter()
                            .map(|(u, _)| u.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))),
                }
            }
        }
    }
}

/// The app's model-pose state: what was read for which (map, tag).
#[derive(Default)]
pub struct Anim {
    loaded: Option<Loaded>,
}

/// [`ResourceReader`] over the shown map's raw-resource files.
struct Raw<'a>(&'a mut assets::ResourceFiles);

impl ResourceReader for Raw<'_> {
    fn check(&self, p: RawPointer, size: u32) -> tags::Result<()> {
        let Some(length) = self.0.length(p.file) else {
            return Err(tags::Error::MissingResource(p.file));
        };
        if u64::from(p.offset) + u64::from(size) > length {
            return Err(tags::Error::ResourceBounds {
                file: p.file,
                offset: p.offset,
                size,
                length,
            });
        }
        Ok(())
    }

    fn read_raw(&mut self, p: RawPointer, size: u32) -> tags::Result<Vec<u8>> {
        self.check(p, size)?;
        self.0
            .read(p.file, p.offset, size)
            .map_err(|_| tags::Error::Invalid("a raw resource file did not read"))
    }
}

fn id_text(id: TagId) -> String {
    format!("{:#010x}", id.0)
}

/// The map's objects whose `hlmt` names `graph`, by name then datum.
fn users_of(ctx: &Context, graph: TagId) -> Vec<ModelUser> {
    let mut out: Vec<ModelUser> = ctx
        .catalog
        .tags
        .values()
        .filter(|t| tags::object::is_object_group(t.group))
        .filter_map(|t| {
            let (_, resolved) = scenario_edit::object_collision::resolve(ctx, t.id).ok()?;
            let uses = resolved
                .as_ref()
                .and_then(|r| r.animation.as_ref())
                .is_some_and(|a| a.id == graph);
            uses.then(|| ModelUser {
                object: t.id,
                name: t.name.clone().unwrap_or_else(|| id_text(t.id)),
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.name, a.object.0).cmp(&(&b.name, b.object.0)));
    out
}

/// The sphere covering `bounds`.
fn sphere_of(bounds: Option<([f32; 3], [f32; 3])>) -> ([f32; 3], f32) {
    bounds.map_or(([0.0; 3], 0.0), |(lo, hi)| {
        let c = std::array::from_fn(|k| (lo[k] + hi[k]) * 0.5);
        let r = (0..3).map(|k| (hi[k] - c[k]).powi(2)).sum::<f32>().sqrt();
        (c, r)
    })
}

/// The posed model drawn offscreen through the viewport's native object
/// renderer (the picture `anim.pose` writes and the Animation window
/// shows): RGBA bytes and the texture's size.
pub(crate) fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    model: &PoseModel,
    size: [u32; 2],
) -> Result<(Vec<u8>, u32, u32), String> {
    let texture = preview::render_posed(
        device,
        queue,
        &[preview::Posed {
            piece: &model.piece,
            nodes: &model.nodes,
            sphere: model.sphere,
        }],
        &model.camera,
        size,
        BACKGROUND,
    );
    let rgba =
        crate::headless::read_texture(device, queue, &texture).map_err(|e| format!("{e:#}"))?;
    Ok((rgba, texture.width(), texture.height()))
}

impl App {
    fn anim_key(&self, id: TagId) -> (String, TagId) {
        let source = self
            .source
            .as_ref()
            .map(|s| s.path().display().to_string())
            .unwrap_or_default();
        (
            format!("{source}|{}", self.shown_map_name().unwrap_or_default()),
            id,
        )
    }

    /// Reads the graph of `id` from the map the Tags tool shows, unless it
    /// is the one loaded: its animations and the models that use it.
    pub fn anim_info(&mut self, id: TagId) -> Result<AnimInfo, AnimError> {
        self.ensure_anim(id)?;
        let mut files = self
            .shown_raw_files()
            .map_err(|e| AnimError::Failed(format!("the map's resource files: {e}")))?;
        let mut reader = Raw(&mut files);
        let loaded = self.anim.loaded.as_mut().expect("loaded");
        // Build every clip once: the list names each animation's codecs,
        // and why any does not build.
        for index in 0..loaded.graph.animations.len() {
            let _ = loaded.clip(index, &mut reader);
        }
        let animations = (0..loaded.graph.animations.len())
            .map(|i| loaded.summary(i))
            .collect();
        let users = loaded.models.iter().map(|(u, _)| u.clone()).collect();
        Ok(AnimInfo {
            nodes: loaded.graph.nodes.len(),
            animations,
            users,
        })
    }

    /// Samples animation `animation` of the graph `id` at `frame`
    /// (fractional frames blend the neighbours), with `overlay`'s sample
    /// applied at `weight` over it, on the model that uses the graph.
    /// `model` names which, when several do (a user's tag name or
    /// `0x...` id); without it the only user is posed, or none when the
    /// map has none. The answer carries the node transforms and the posed
    /// model ready to draw.
    pub fn anim_pose(
        &mut self,
        id: TagId,
        animation: usize,
        frame: f32,
        overlay: Option<(usize, f32)>,
        model: Option<&str>,
    ) -> Result<PoseAnswer, AnimError> {
        self.ensure_anim(id)?;
        let mut files = self
            .shown_raw_files()
            .map_err(|e| AnimError::Failed(format!("the map's resource files: {e}")))?;
        let mut reader = Raw(&mut files);
        let loaded = self.anim.loaded.as_mut().expect("loaded");
        let clip = loaded
            .clip(animation, &mut reader)
            .map_err(AnimError::Refused)?;
        let over = match overlay {
            Some((index, _)) => Some(
                loaded
                    .clip(index, &mut reader)
                    .map_err(AnimError::Refused)?,
            ),
            None => None,
        };
        let summary = loaded.summary(animation);
        let picked = loaded.pick_model(model)?;
        let pose = clip
            .sample(frame)
            .map_err(|e| AnimError::Failed(format!("sampling: {e}")))?;
        let node_names = &loaded.node_names;
        let nodes_of = |pose: &tags::anim_sample::Pose,
                        parents: &[i16],
                        worlds: Option<&[tags::anim_sample::Mat4]>|
         -> Vec<PoseNode> {
            pose.nodes
                .iter()
                .enumerate()
                .map(|(i, n)| PoseNode {
                    name: node_names.get(i).cloned().flatten(),
                    parent: parents.get(i).copied().unwrap_or(-1),
                    rotation: n.rotation,
                    translation: n.translation,
                    scale: n.scale,
                    has: [n.has_rotation, n.has_translation, n.has_scale],
                    world: worlds.map(|w| w[i]),
                })
                .collect()
        };
        let Some(picked) = picked else {
            // No model to compose on: the channels are the animation's
            // own, the overlay applied over them as they are.
            let mut pose = pose;
            if let (Some(over), Some((_, weight))) = (over.as_ref(), overlay) {
                let sample = over
                    .sample(frame)
                    .map_err(|e| AnimError::Failed(format!("sampling the overlay: {e}")))?;
                pose.add_overlay(&sample, weight)
                    .map_err(|e| AnimError::Failed(format!("the overlay: {e}")))?;
            }
            let parents: Vec<i16> = loaded.graph.nodes.iter().map(|n| n.parent).collect();
            return Ok(PoseAnswer {
                animation: summary,
                frame,
                nodes: nodes_of(&pose, &parents, None),
                model: None,
                notes: vec![
                    "no model of this map uses this graph: the transforms are the animation's own channels"
                        .to_owned(),
                ],
            });
        };
        let (user, piece) = &loaded.models[picked];
        let piece = piece
            .clone()
            .map_err(|e| AnimError::Refused(format!("{}: {e}", user.name)))?;
        if pose.nodes.len() != piece.skeleton.len() {
            return Err(AnimError::Refused(format!(
                "{}: the animation has {} nodes, the render model {}",
                summary.name,
                pose.nodes.len(),
                piece.skeleton.len()
            )));
        }
        // The engine's order: the node defaults under the base's channels,
        // the overlay scaled onto what that left.
        let defaults: Vec<NodeDefault> = piece
            .skeleton
            .iter()
            .map(|n| NodeDefault {
                parent: n.parent,
                rotation: n.rotation,
                translation: n.translation,
                scale: n.scale,
            })
            .collect();
        let mut composed = pose
            .merged(&defaults)
            .map_err(|e| AnimError::Failed(e.to_string()))?;
        if let (Some(over), Some((_, weight))) = (over.as_ref(), overlay) {
            let sample = over
                .sample(frame)
                .map_err(|e| AnimError::Failed(format!("sampling the overlay: {e}")))?;
            composed
                .add_overlay(&sample, weight)
                .map_err(|e| AnimError::Failed(format!("the overlay: {e}")))?;
        }
        let parents: Vec<i16> = piece.skeleton.iter().map(|n| n.parent).collect();
        let worlds = composed
            .world_matrices(&parents)
            .map_err(|e| AnimError::Failed(e.to_string()))?;
        let nodes = posed_nodes(&piece.skeleton, &composed, &Matrix::IDENTITY)
            .map_err(AnimError::Failed)?;
        let bounds = preview::posed_bounds(&piece, &nodes);
        let camera = preview::framing(bounds.unwrap_or(([-0.5; 3], [0.5; 3])), YAW, PITCH);
        Ok(PoseAnswer {
            animation: summary,
            frame,
            nodes: nodes_of(&composed, &parents, Some(&worlds)),
            model: Some(PoseModel {
                object: user.object,
                name: user.name.clone(),
                piece,
                nodes,
                sphere: sphere_of(bounds),
                camera,
            }),
            notes: Vec::new(),
        })
    }

    fn ensure_anim(&mut self, id: TagId) -> Result<(), AnimError> {
        let key = self.anim_key(id);
        if self.anim.loaded.as_ref().is_some_and(|l| l.key == key) {
            return Ok(());
        }
        let mut map = self
            .open_shown_map()
            .map_err(|e| AnimError::Failed(format!("the map: {e}")))?;
        let group = map
            .tag(id)
            .map_err(|e| AnimError::Failed(e.to_string()))?
            .group;
        if group.0 != *b"jmad" {
            return Err(AnimError::Refused(format!(
                "{} is not an animation graph (jmad)",
                tags::defs::fourcc_text(group)
            )));
        }
        let ctx = Context::read(&mut map)
            .map_err(|e| AnimError::Failed(format!("the map's tags: {e}")))?;
        let graph =
            AnimationGraph::read(&mut map, id).map_err(|e| AnimError::Failed(e.to_string()))?;
        let node_names = graph
            .nodes
            .iter()
            .map(|n| {
                n.name
                    .resolve(map.strings())
                    .ok()
                    .flatten()
                    .map(str::to_owned)
            })
            .collect();
        let animation_names = graph
            .animations
            .iter()
            .enumerate()
            .map(|(i, a)| {
                a.name
                    .resolve(map.strings())
                    .ok()
                    .flatten()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("animation {i}"))
            })
            .collect();
        let users = users_of(&ctx, id);
        let mut files = self
            .shown_raw_files()
            .map_err(|e| AnimError::Failed(format!("the map's resource files: {e}")))?;
        let models = users
            .iter()
            .map(|u| {
                let piece = preview::object(&mut map, &ctx, &mut files, u.object, None)
                    .map(Arc::new)
                    .map_err(|e| e.to_string());
                (u.clone(), piece)
            })
            .collect();
        self.anim.loaded = Some(Loaded {
            key,
            graph,
            node_names,
            animation_names,
            resources: HashMap::new(),
            clips: HashMap::new(),
            models,
        });
        Ok(())
    }
}
