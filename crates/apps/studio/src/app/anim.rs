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
//! **The timeline.** [`App::anim_read`] decodes a clip to dense frames and
//! reduces them to keys within a tolerance ([`tags::anim_keys`]), listed
//! per node and channel for `anim.read` and the initial editing document.
//! [`Transport`] is the timeline's scrub head,
//! play, loop and speed, one type for the window and its tests.
//!
//! The graph, its clips and the models are read once per (map, tag) and
//! kept: sampling a frame needs no map access, so scrubbing stays cheap.
//! The window and the API read the map the Tags tool shows
//! ([`App::open_shown_map`]), never writing it.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use map::{RawPointer, TagId};
use scenario_edit::geometry::Camera;
use scenario_edit::native_view::posed_nodes;
use scenario_edit::native_view::preview::{self, Matrix, Piece};
use tags::anim::AnimationGraph;
use tags::anim_keys::{self, Keys, Tolerance};
use tags::anim_sample::{AnimationClip, Codec, Mat4, NodeDefault, Pose};
use tags::object::Context;
use tags::sound::ResourceReader;

use super::App;

mod document;
mod face;
pub use document::save::{AnimationPlacement, SavedAnimation};
pub use face::{FacePose, FaceWeight, mouth_weight};
pub use document::transfer::{RetargetOptions, Transfer};

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
    /// Local transforms under the overlay, including model defaults.
    /// The gizmo uses them to turn composed transforms back into delta keys.
    pub base_nodes: Option<Vec<PoseNode>>,
    /// The model posed, when the map has one that uses the graph.
    pub model: Option<PoseModel>,
    pub notes: Vec<String>,
}

/// The animation's rate: the graph's own 30 Hz.
pub const HZ: f32 = 30.0;

/// The timeline's scrub head and play state (docs/editor/studio-redesign.md,
/// "The animation editor", **Timeline**): a frame position over a clip's
/// frames at [`HZ`], times a speed, optionally looping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transport {
    /// Where the head is, in frames (fractional while playing).
    pub frame: f32,
    pub playing: bool,
    pub looping: bool,
    /// Playback speed, a multiple of real time ([`Self::SPEEDS`]).
    pub speed: f32,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            frame: 0.0,
            playing: false,
            looping: false,
            speed: 1.0,
        }
    }
}

impl Transport {
    /// The speeds the timeline offers.
    pub const SPEEDS: [f32; 5] = [0.25, 0.5, 1.0, 2.0, 4.0];

    /// Puts the head on `frame` (held inside the clip) and stops play.
    pub fn scrub(&mut self, frame: f32, frames: usize) {
        let last = frames.saturating_sub(1) as f32;
        self.frame = if frame.is_nan() {
            0.0
        } else {
            frame.clamp(0.0, last)
        };
        self.playing = false;
    }

    /// Plays, or pauses; playing from the end starts again at frame 0.
    pub fn toggle(&mut self, frames: usize) {
        let last = frames.saturating_sub(1) as f32;
        if !self.playing && self.frame >= last {
            self.frame = 0.0;
        }
        self.playing = !self.playing;
    }

    /// One step of the loop (or the stop) of play, one frame at a time.
    pub fn step(&mut self, by: i32, frames: usize) {
        let to = (self.frame.round() + by as f32).clamp(0.0, frames.saturating_sub(1) as f32);
        self.scrub(to, frames);
    }

    /// Moves the head on by `dt` seconds of play. Looping wraps past the
    /// last frame to the start (the last frame is the first again, so the
    /// loop is `frames - 1` long); otherwise play stops on the last.
    pub fn advance(&mut self, dt: f32, frames: usize) {
        if !self.playing {
            return;
        }
        let last = frames.saturating_sub(1) as f32;
        self.frame += dt * HZ * self.speed;
        if self.frame > last {
            if self.looping && last > 0.0 {
                self.frame %= last;
            } else {
                self.frame = last;
                self.playing = false;
            }
        }
    }
}

/// One channel of a node on the timeline: a rotation, translation or scale
/// as keys.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadChannel {
    pub kind: anim_keys::Kind,
    pub keys: Vec<anim_keys::Key>,
    /// More than one key: the channel changes over the clip.
    pub moves: bool,
}

/// One row of the timeline: a node with the channels the clip drives.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadNode {
    pub index: usize,
    /// The graph node's name, resolved through the map's strings.
    pub name: Option<String>,
    pub parent: i16,
    pub channels: Vec<ReadChannel>,
}

/// One animation read as the timeline shows it.
#[derive(Clone, Debug)]
pub struct AnimRead {
    pub animation: AnimSummary,
    pub frames: usize,
    /// One row per node of the clip, in node order.
    pub nodes: Vec<ReadNode>,
    /// The keys the rows list, within [`Keys::tolerance`] of the dense
    /// frames at every frame.
    pub keys: Keys,
    /// How many dense channel values the clip decodes to (channels times
    /// frames): what the keys replace.
    pub dense_values: usize,
    /// The largest error of the keys against the dense frames, per kind
    /// `[rotation (radians), translation, scale]`.
    pub max_error: [f32; 3],
    /// Events the animation declares: frame, sound and effect blocks,
    /// counted but not decoded.
    pub events: [usize; 3],
    pub notes: Vec<String>,
}

/// A graph's identity even while the Tags tool shows another map.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct GraphKey {
    source: Option<PathBuf>,
    map: Option<String>,
    tag: TagId,
}

/// What was read for one graph, kept between calls.
struct Loaded {
    key: GraphKey,
    /// The graph's tag name, for messages.
    name: String,
    graph: AnimationGraph,
    /// The graph's mode tree, names resolved, for the face preview's
    /// lookups ([`face`]).
    tree: Vec<face::TreeEntry>,
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

    fn sample_clip(
        &mut self,
        index: usize,
        frame: f32,
        reader: &mut dyn ResourceReader,
    ) -> Result<Pose, AnimError> {
        self.clip(index, reader)
            .map_err(AnimError::Refused)?
            .sample(frame)
            .map_err(|e| AnimError::Failed(format!("sampling: {e}")))
    }

    fn pose(
        &self,
        summary: AnimSummary,
        frame: f32,
        mut pose: Pose,
        overlay: Option<(&Pose, f32)>,
        model: Option<&str>,
    ) -> Result<PoseAnswer, AnimError> {
        let nodes_of = |pose: &Pose, parents: &[i16], worlds: Option<&[Mat4]>| {
            pose.nodes
                .iter()
                .enumerate()
                .map(|(i, n)| PoseNode {
                    name: self.node_names.get(i).cloned().flatten(),
                    parent: parents.get(i).copied().unwrap_or(-1),
                    rotation: n.rotation,
                    translation: n.translation,
                    scale: n.scale,
                    has: [n.has_rotation, n.has_translation, n.has_scale],
                    world: worlds.map(|w| w[i]),
                })
                .collect::<Vec<_>>()
        };
        let apply_overlay = |pose: &mut Pose, parents: &[i16]| -> Result<_, AnimError> {
            let Some((sample, weight)) = overlay else {
                return Ok(None);
            };
            let base_nodes = nodes_of(pose, parents, None);
            pose.add_overlay(sample, weight)
                .map_err(|e| AnimError::Failed(format!("the overlay: {e}")))?;
            Ok(Some(base_nodes))
        };

        let Some(picked) = self.pick_model(model)? else {
            let parents: Vec<_> = self.graph.nodes.iter().map(|n| n.parent).collect();
            let base_nodes = apply_overlay(&mut pose, &parents)?;
            return Ok(PoseAnswer {
                animation: summary,
                frame,
                nodes: nodes_of(&pose, &parents, None),
                base_nodes,
                model: None,
                notes: vec![
                    "no model of this map uses this graph: the transforms are the animation's own channels"
                        .to_owned(),
                ],
            });
        };
        let (user, piece) = &self.models[picked];
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
        // Merge the base's channels with the model's defaults, then add
        // the overlay before composing the local transforms up the tree.
        let defaults = model_defaults(&piece);
        let parents: Vec<_> = defaults.iter().map(|n| n.parent).collect();
        let mut composed = pose
            .merged(&defaults)
            .map_err(|e| AnimError::Failed(e.to_string()))?;
        let base_nodes = apply_overlay(&mut composed, &parents)?;
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
            base_nodes,
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

    fn pick_model(&self, model: Option<&str>) -> Result<Option<usize>, AnimError> {
        pick_model(&self.models, model)
    }
}

/// Which model to use: the named one, the only one when none is named,
/// or no selection if the graph has no model. Poses and retargeting
/// interpret names and tag IDs the same way.
fn pick_model<T>(
    models: &[(ModelUser, T)],
    model: Option<&str>,
) -> Result<Option<usize>, AnimError> {
    let names = || {
        models
            .iter()
            .map(|(user, _)| user.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match model {
        None => match models.len() {
            0 => Ok(None),
            1 => Ok(Some(0)),
            n => Err(AnimError::Refused(format!(
                "{n} models use this graph: {}", names()
            ))),
        },
        Some(want) => models
            .iter()
            .position(|(user, _)| {
                user.name.eq_ignore_ascii_case(want)
                    || id_text(user.object).eq_ignore_ascii_case(want)
            })
            .map(Some)
            .ok_or_else(|| AnimError::Refused(format!(
                "{want:?} is not one of the models using this graph ({})", names()
            ))),
    }
}

/// Retain the rest transforms even after a different graph takes the
/// model preview cache. Open documents can then serve as retarget sources
/// from a different map without reopening it or retaining its mesh.
struct GraphRest {
    name: String,
    models: Vec<(ModelUser, Result<Vec<NodeDefault>, String>)>,
}

impl GraphRest {
    fn model(&self, model: Option<&str>) -> Result<&[NodeDefault], AnimError> {
        let index = pick_model(&self.models, model)?.ok_or_else(|| {
            AnimError::Refused(format!("{} has no model with a rest skeleton", self.name))
        })?;
        let (user, rest) = &self.models[index];
        rest.as_deref().map_err(|why| AnimError::Refused(format!("{}: {why}", user.name)))
    }
}

/// The app's animation documents and the graph currently loaded for posing.
#[derive(Default)]
pub struct Anim {
    loaded: Option<Loaded>,
    docs: Vec<document::AnimationDocument>,
    graph_rests: HashMap<GraphKey, GraphRest>,
    /// The graph each object posed by the face preview uses, by object.
    face_graphs: HashMap<GraphKey, Result<TagId, AnimError>>,
}

fn model_defaults(piece: &Piece) -> Vec<NodeDefault> {
    piece
        .skeleton
        .iter()
        .map(|n| NodeDefault {
            parent: n.parent,
            rotation: n.rotation,
            translation: n.translation,
            scale: n.scale,
        })
        .collect()
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

/// The Animation window's GPU-only picture from the native object renderer.
pub(crate) fn texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    model: &PoseModel,
    size: [u32; 2],
) -> wgpu::Texture {
    preview::render_posed(
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
    )
}

/// The same picture as RGBA bytes for `anim.pose` to write to a PNG.
pub(crate) fn draw(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    model: &PoseModel,
    size: [u32; 2],
) -> Result<(Vec<u8>, u32, u32), String> {
    let texture = texture(device, queue, model, size);
    let rgba =
        crate::headless::read_texture(device, queue, &texture).map_err(|e| format!("{e:#}"))?;
    Ok((rgba, texture.width(), texture.height()))
}

impl App {
    pub(crate) fn anim_graph_key(&self, id: TagId) -> GraphKey {
        let source = self
            .source
            .as_ref()
            .map(|s| s.path().to_path_buf())
            .or_else(|| self.session.source().map(|s| s.path.clone()));
        GraphKey {
            source,
            map: self.shown_map_name().map(str::to_owned),
            tag: id,
        }
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

    /// Animation `animation` of the graph `id` as the timeline shows it:
    /// the clip decoded to dense frames and reduced to keys within
    /// `tolerance` ([`tags::anim_keys::reduce`]), listed per node. An aim
    /// grid (codec 8) is not a time series: its nodes list without keys,
    /// and a note says so.
    pub fn anim_read(
        &mut self,
        id: TagId,
        animation: usize,
        tolerance: Tolerance,
    ) -> Result<AnimRead, AnimError> {
        self.ensure_anim(id)?;
        let mut files = self
            .shown_raw_files()
            .map_err(|e| AnimError::Failed(format!("the map's resource files: {e}")))?;
        let mut reader = Raw(&mut files);
        let loaded = self.anim.loaded.as_mut().expect("loaded");
        let clip = loaded
            .clip(animation, &mut reader)
            .map_err(AnimError::Refused)?;
        let summary = loaded.summary(animation);
        let record = &loaded.graph.animations[animation];
        let events = [
            record.frame_events.len(),
            record.sound_events.len(),
            record.effect_events.len(),
        ];
        let mut notes = Vec::new();
        if events.iter().any(|&n| n > 0) {
            notes.push(
                "events are counted, not decoded (docs/formats/animation.md, \"Open items\")"
                    .to_owned(),
            );
        }
        if record.frame_info_type != 0 {
            notes.push("root movement (frame info) is not decoded".to_owned());
        }
        let frames = clip.frame_count();
        let poses = if clip.is_aim_grid() {
            notes.push(
                "an aim grid: its frames are cells selected by aim, not points in time, so no keys are listed"
                    .to_owned(),
            );
            Vec::new()
        } else {
            (0..frames)
                .map(|f| clip.sample_frame(f))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| AnimError::Failed(format!("sampling: {e}")))?
        };
        let keys = anim_keys::reduce(&poses, tolerance);
        let max_error = keys.max_error(&poses);
        let nodes = (0..clip.node_count())
            .map(|index| ReadNode {
                index,
                name: loaded.node_names.get(index).cloned().flatten(),
                parent: loaded.graph.nodes.get(index).map_or(-1, |n| n.parent),
                channels: keys
                    .channels
                    .iter()
                    .filter(|c| c.node == index)
                    .map(|c| ReadChannel {
                        kind: c.kind,
                        keys: c.keys.clone(),
                        moves: c.moves(),
                    })
                    .collect(),
            })
            .collect();
        Ok(AnimRead {
            animation: summary,
            frames,
            nodes,
            dense_values: keys.channels.len() * poses.len(),
            keys,
            max_error,
            events,
            notes,
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
        let pose = loaded.sample_clip(animation, frame, &mut reader)?;
        let overlay = overlay
            .map(|(index, weight)| {
                let pose = loaded.sample_clip(index, frame, &mut reader)?;
                Ok::<_, AnimError>((pose, weight))
            })
            .transpose()?;
        loaded.pose(
            loaded.summary(animation),
            frame,
            pose,
            overlay.as_ref().map(|(pose, weight)| (pose, *weight)),
            model,
        )
    }

    fn ensure_anim(&mut self, id: TagId) -> Result<(), AnimError> {
        let key = self.anim_graph_key(id);
        if self.anim.loaded.as_ref().is_some_and(|l| l.key == key) {
            return Ok(());
        }
        let mut map = self
            .open_shown_map()
            .map_err(|e| AnimError::Failed(format!("the map: {e}")))?;
        let entry = map.tag(id).map_err(|e| AnimError::Failed(e.to_string()))?;
        if entry.group.0 != *b"jmad" {
            return Err(AnimError::Refused(format!(
                "{} is not an animation graph (jmad)",
                tags::defs::fourcc_text(entry.group)
            )));
        }
        let graph_name = entry.name.clone().unwrap_or_else(|| id_text(id));
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
        let tree = face::tree(&graph, map.strings());
        let users = users_of(&ctx, id);
        let mut files = self
            .shown_raw_files()
            .map_err(|e| AnimError::Failed(format!("the map's resource files: {e}")))?;
        let models: Vec<_> = users
            .iter()
            .map(|u| {
                let piece = preview::object(&mut map, &ctx, &mut files, u.object, None)
                    .map(Arc::new)
                    .map_err(|e| e.to_string());
                (u.clone(), piece)
            })
            .collect();
        self.anim.graph_rests.insert(
            key.clone(),
            GraphRest {
                name: graph_name.clone(),
                models: models
                    .iter()
                    .map(|(user, piece)| {
                        let rest = piece.as_ref().map(|piece| model_defaults(piece)).map_err(Clone::clone);
                        (user.clone(), rest)
                    })
                    .collect(),
            },
        );
        self.anim.loaded = Some(Loaded {
            key,
            name: graph_name,
            graph,
            tree,
            node_names,
            animation_names,
            resources: HashMap::new(),
            clips: HashMap::new(),
            models,
        });
        Ok(())
    }
}

#[cfg(test)]
mod transport_tests {
    use super::{HZ, Transport};

    #[test]
    fn scrub_holds_the_head_inside_the_clip_and_stops_play() {
        let mut t = Transport {
            playing: true,
            ..Transport::default()
        };
        t.scrub(3.0, 10);
        assert_eq!((t.frame, t.playing), (3.0, false));
        t.scrub(99.0, 10);
        assert_eq!(t.frame, 9.0);
        t.scrub(-4.0, 10);
        assert_eq!(t.frame, 0.0);
        t.scrub(f32::NAN, 10);
        assert_eq!(t.frame, 0.0);
        t.scrub(5.0, 0);
        assert_eq!(t.frame, 0.0);
    }

    #[test]
    fn play_runs_at_thirty_hertz_times_the_speed_and_stops_on_the_last_frame() {
        let mut t = Transport::default();
        t.advance(1.0, 100);
        assert_eq!(t.frame, 0.0, "a paused head does not move");
        t.toggle(100);
        assert!(t.playing);
        t.advance(0.5, 100);
        assert!((t.frame - 0.5 * HZ).abs() < 1e-4);
        t.speed = 2.0;
        t.advance(0.25, 100);
        assert!((t.frame - (0.5 * HZ + 0.25 * HZ * 2.0)).abs() < 1e-4);
        t.advance(10.0, 100);
        assert_eq!((t.frame, t.playing), (99.0, false));
        // Playing from the end starts again.
        t.toggle(100);
        assert_eq!((t.frame, t.playing), (0.0, true));
    }

    #[test]
    fn loop_wraps_past_the_last_frame_and_keeps_playing() {
        let mut t = Transport {
            looping: true,
            ..Transport::default()
        };
        t.toggle(31);
        // 31 frames: the loop is 30 long. 1.1 s is 33 frames: 3 past.
        t.advance(1.1, 31);
        assert!(t.playing);
        assert!((t.frame - 3.0).abs() < 1e-3, "{}", t.frame);
        // A one-frame clip has no loop to run: it stops.
        let mut one = Transport {
            looping: true,
            ..Transport::default()
        };
        one.toggle(1);
        one.advance(1.0, 1);
        assert_eq!((one.frame, one.playing), (0.0, false));
    }

    #[test]
    fn step_moves_one_frame_and_pauses() {
        let mut t = Transport::default();
        t.toggle(5);
        t.step(1, 5);
        assert_eq!((t.frame, t.playing), (1.0, false));
        t.step(-9, 5);
        assert_eq!(t.frame, 0.0);
        t.step(9, 5);
        assert_eq!(t.frame, 4.0);
    }
}
