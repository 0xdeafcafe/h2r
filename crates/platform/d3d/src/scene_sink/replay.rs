//! Owned work for drawing a sealed frame. Texture inputs are either immutable
//! uploads or the result of a sample at a particular point in this stream.

use std::sync::Arc;

use frame_types::{AuxTarget, SlotAt, TargetRef, TargetRole};
use frame_types::carried::CameraUse;
use gfx::texture::{DecodeOptions, DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

use crate::backend::{
    ExternalPassRequest, ExternalPlan, ExternalSurfaces, RectClear, ResolvedDraw, SceneStage, SharedReplacement,
};

#[derive(Clone, Debug)]
pub enum ReplayTexture {
    Upload(Arc<DecodedTexture>),
    Placeholder(TextureDesc),
    Sample(u32),
    #[cfg(feature = "gpu")]
    Frozen(super::FrozenSample),
    /// A previous frame created a view of this still-live attachment. The
    /// original binding establishes identity only. Replay reads its own
    /// attachment at this draw's position in the command stream.
    #[cfg(feature = "gpu")]
    Live { target: TargetRef, binding: super::FrozenSample },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplaySample {
    Snapshot { opaque: bool },
    InPlace,
    Depth,
    Alias {
        format: D3DFormat,
        width: u32,
        height: u32,
        pitch: u32,
        generation: u64,
        offset: usize,
        desc: TextureDesc,
        options: DecodeOptions,
    },
}

/// A typed replacement retained beside the packet in [`ReplayOp::Draw`].
/// The pass records repeatedly
/// ([`crate::backend::ExternalPass::replays_scene`]) and reads only the
/// draw's own texture inputs, so a replay's live and frozen bindings
/// reach it unchanged.
///
/// One admission may stand in for several guest draws (a UI batch's head
/// and its siblings): the replay holds one draw for each packet, in order,
/// each with this admission and its place `at`. The first prepares the
/// pass, which records them all; a later one draws nothing after the
/// first did, and its own packet after the first drew its packet.
#[derive(Clone)]
pub struct RetainedExternal {
    /// The packets the admission was made for, in the guest's issue order.
    /// Replay uses the admission only for draws that still equal them.
    pub packets: Arc<[Arc<ResolvedDraw>]>,
    /// This draw's place in `packets`.
    pub at: usize,
    pub surfaces: ExternalSurfaces<TargetRef>,
    pub request: ExternalPassRequest,
    pub plan: ExternalPlan,
    pub pass: SharedReplacement,
}

impl RetainedExternal {
    /// The packet of this draw.
    pub fn packet(&self) -> &Arc<ResolvedDraw> {
        &self.packets[self.at]
    }

    /// A later draw of a run whose first draw records the pass for all.
    pub fn follows(&self) -> bool {
        self.at > 0
    }

    /// The admission's attachments on this replay's targets, only when it
    /// still describes `draw`: the draw equals the packet it was made for,
    /// its surfaces are the attachments `draw` is bound to (a colour-only
    /// draw has the colour as its depth), and a private capture is at 1x (a
    /// live display's backend answers for its own scale and copies). `None`
    /// means draw the packet.
    pub fn resolve<S: Copy + Eq>(
        &self,
        draw: &Arc<ResolvedDraw>,
        targets: &std::collections::HashMap<TargetRef, S>,
        bound: (S, Option<S>),
        display: bool,
        scale: impl Into<gfx::raster::RenderScale>,
    ) -> Option<ExternalSurfaces<S>> {
        let packet = self.packets.get(self.at)?;
        if !(display || scale.into() == gfx::raster::RenderScale::ONE) || !(Arc::ptr_eq(packet, draw) || **packet == **draw) {
            return None;
        }
        let surfaces = self.surfaces.try_map(|r| targets.get(r).copied())?;
        (surfaces.color == bound.0 && surfaces.depth == bound.1.unwrap_or(bound.0)).then_some(surfaces)
    }
}

impl std::fmt::Debug for RetainedExternal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetainedExternal")
            .field("plan", &self.plan)
            .field("at", &(self.at, self.packets.len()))
            .field("colour", &self.surfaces.color)
            .field("depth", &self.surfaces.depth)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for SceneStage<TargetRef> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneStage")
            .field("plan", &self.plan)
            .field("colour", &self.surfaces.color)
            .field("depth", &self.surfaces.depth)
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum ReplayOp {
    /// The typed stage at the exact point where the guest released its draws.
    Stage(SceneStage<TargetRef>),
    BeginFlareQuery,
    /// The original ticket issued to the guest. Private playback skips it.
    EndFlareQuery(u64),
    Bind { colour: TargetRef, depth: TargetRef },
    /// A guest draw. `external`, when set, is the typed replacement the
    /// guest's draw was admitted as: replay prepares it on its own
    /// attachments with this op's own texture inputs, and draws `draw`
    /// (the exact D3D packet) when it cannot. A draw that follows in its
    /// admission's run ([`RetainedExternal::follows`]) draws nothing when
    /// the run's first draw recorded the pass.
    Draw {
        draw: Arc<ResolvedDraw>,
        textures: [Option<ReplayTexture>; 4],
        camera: CameraUse,
        external: Option<Box<RetainedExternal>>,
    },
    Clear { colour: TargetRef, depth: TargetRef, clear: Clear },
    ClearRects { colour: TargetRef, depth: TargetRef, clear: RectClear },
    Copy { from: TargetRef, to: TargetRef },
    Upload { to: TargetRef, desc: TextureDesc, bytes: Arc<[u8]> },
    /// Sample `id` of `of`. `into`: the earlier sample of this frame whose
    /// texture this copy refills (a pooled frame-buffer copy,
    /// [`crate::Backend::snapshot_surface_into`]): replay copies into that
    /// sample's texture, which holds this sample's contents from here on,
    /// so the ops are applied in their recorded order. `None`: a texture
    /// of its own.
    Sample { of: TargetRef, id: u32, how: ReplaySample, into: Option<u32> },
    /// The guest's object shadow (`0x00032350`) the following ops draw,
    /// by its index in the scene's `object_shadows`; `None` after it.
    /// Recorded where the shadow the hooks declare changes
    /// ([`super::SceneSink::with_object_shadows`]). It draws nothing; E3
    /// lighting reads it to put a sun tile in place of a shadow's mask.
    ObjectShadow(Option<u32>),
}

impl ReplayOp {
    /// Ops that only mark a place in the frame: replay applies nothing.
    pub fn is_marker(&self) -> bool {
        matches!(self, Self::ObjectShadow(_))
    }

    fn map_targets(&mut self, map: impl Fn(TargetRef) -> TargetRef) {
        match self {
            Self::Stage(stage) => {
                stage.surfaces.color = map(stage.surfaces.color);
                stage.surfaces.depth = map(stage.surfaces.depth);
                for s in &mut stage.surfaces.sampled { *s = map(*s); }
                for s in stage.surfaces.slots.iter_mut().flatten() { *s = map(*s); }
            }
            Self::Bind { colour, depth } | Self::Clear { colour, depth, .. } | Self::ClearRects { colour, depth, .. } => {
                *colour = map(*colour);
                *depth = map(*depth);
            }
            Self::Copy { from, to } => {
                *from = map(*from);
                *to = map(*to);
            }
            Self::Upload { to, .. } => *to = map(*to),
            Self::Sample { of, .. } => *of = map(*of),
            Self::Draw { textures, external, .. } => {
                if let Some(e) = external {
                    let surfaces = e.surfaces.try_map(|s| Some(map(*s))).expect("total map");
                    e.surfaces = surfaces;
                }
                #[cfg(feature = "gpu")]
                for texture in textures.iter_mut().flatten() {
                    if let ReplayTexture::Live { target, .. } = texture { *target = map(*target); }
                }
                #[cfg(not(feature = "gpu"))]
                let _ = textures;
            }
            Self::BeginFlareQuery | Self::EndFlareQuery(_) | Self::ObjectShadow(_) => {}
        }
    }
}

/// A present's resources and commands. A frame with a refusal cannot be
/// redrawn independently. Frozen texture inputs require the producing GPU device.
#[derive(Debug)]
pub struct ReplayFrame {
    /// Guest presents, including those without captured ops; first present is 1.
    pub present: u64,
    /// The frame step entered a game view before this Swap. A view may
    /// contribute no packets of its own and be followed only by HUD work.
    pub entered_game_view: bool,
    pub flags: u32,
    pub gamma: Option<Box<[u8; 768]>>,
    /// Exact contents last established by an earlier clear or whole upload.
    /// Existing display attachments already hold these pixels.
    pub initial: Vec<(TargetRef, super::TargetSeed)>,
    /// Private target contents retained by the display worker before the
    /// first write of this present. The cell belongs to this sealed frame.
    #[cfg(feature = "gpu")]
    pub target_snapshots: super::TargetSnapshots,
    /// Set by the display worker after it validates every inherited live
    /// view against this transaction's actual surface and texture.
    #[cfg(feature = "gpu")]
    pub checked_live_inputs: LiveInputValidation,
    pub ops: Vec<(SlotAt, ReplayOp)>,
    pub aux: Vec<AuxTarget>,
    pub colour: Option<AuxTarget>,
    pub depth: Option<AuxTarget>,
    pub presented: TargetRef,
    /// Each target's role, which the frame graph owns its surface by
    /// (`native_render::frame_graph::targets`). A target the device made
    /// with no role (a device without a role table) has none.
    pub roles: Vec<(TargetRef, TargetRole)>,
    pub refusals: Vec<&'static str>,
}

impl ReplayFrame {
    /// The role of `target`; `None` for [`TargetRef::None`] and a target
    /// made without one.
    pub fn role(&self, target: TargetRef) -> Option<TargetRole> {
        self.roles.iter().find(|(t, _)| *t == target).map(|(_, r)| *r)
    }
}

impl Default for ReplayFrame {
    fn default() -> Self {
        Self {
            present: 0,
            entered_game_view: false,
            flags: 0,
            gamma: None,
            initial: Vec::new(),
            #[cfg(feature = "gpu")]
            target_snapshots: Default::default(),
            #[cfg(feature = "gpu")]
            checked_live_inputs: Default::default(),
            ops: Vec::new(),
            aux: Vec::new(),
            colour: None,
            depth: None,
            presented: TargetRef::None,
            roles: Vec::new(),
            refusals: Vec::new(),
        }
    }
}

#[cfg(feature = "gpu")]
#[derive(Debug, Default)]
pub struct LiveInputValidation(std::sync::atomic::AtomicBool);

impl ReplayFrame {
    #[cfg(feature = "gpu")]
    pub fn live_bindings(&self) -> impl Iterator<Item = (TargetRef, &super::FrozenSample)> {
        self.ops.iter().filter_map(|(_, op)| match op {
            ReplayOp::Draw { textures, .. } => Some(textures.iter().flatten()),
            _ => None,
        }).flatten().filter_map(|texture| match texture {
            ReplayTexture::Live { target, binding } => Some((*target, binding)),
            _ => None,
        })
    }

    /// The display worker confirmed all guest bindings still alias the
    /// selected physical images before it submitted this frame's writes.
    #[cfg(feature = "gpu")]
    pub(crate) fn validate_live_bindings(&self) {
        self.checked_live_inputs.0.store(true, std::sync::atomic::Ordering::Release);
    }

    #[cfg(feature = "gpu")]
    pub fn live_bindings_validated(&self) -> bool {
        self.checked_live_inputs.0.load(std::sync::atomic::Ordering::Acquire) || self.live_bindings().next().is_none()
    }

    pub fn has_game_view(&self) -> bool {
        self.entered_game_view || self.ops.iter().any(|(at, _)| at.slot.in_game_view())
    }

    /// True when the reference backend received the guest's original draws
    /// throughout this captured frame. A typed pass may already have been
    /// accepted before a parity request arrived in the middle of a frame.
    pub fn guest_reference_complete(&self) -> bool {
        self.refusals.is_empty() && !self.ops.iter().any(|(_, op)| matches!(op, ReplayOp::Stage(_)))
    }

    pub(super) fn refuse(&mut self, cause: &'static str) {
        if !self.refusals.contains(&cause) {
            self.refusals.push(cause);
        }
    }


}

pub(super) struct Recorder<S> {
    pub frame: ReplayFrame,
    // Physical handles can be destroyed and recycled before present. Ops
    // retain the number of the resource, independently of its live handle.
    live: Vec<(S, u16)>,
    /// The roles of this frame's physical ids, as `live` numbers them.
    roles: Vec<(u16, TargetRole)>,
    // Allocation identities only for surfaces with an in-place view. Guest
    // texture handles may outlive the surface handle or cross many presents.
    views: Vec<(S, Arc<()>)>,
    seeds: Vec<(S, super::TargetSeed)>,
    bound: Option<(SlotAt, TargetRef, TargetRef)>,
    pub seq: u64,
    next_sample: u32,
    output: Box<dyn FnMut(ReplayFrame) + Send>,
    /// Where the hooks declare the object shadow being drawn.
    pub shadows: Option<super::ShadowSource>,
    /// The object shadow the last marker declared this frame.
    shadow: Option<u32>,
}

impl<S: Copy + Eq> Recorder<S> {
    pub fn new(output: impl FnMut(ReplayFrame) + Send + 'static) -> Self {
        Self {
            frame: ReplayFrame::default(),
            live: Vec::new(),
            roles: Vec::new(),
            views: Vec::new(),
            seeds: Vec::new(),
            bound: None,
            seq: 0,
            next_sample: 0,
            output: Box::new(output),
            shadows: None,
            shadow: None,
        }
    }

    /// Appends `op`, after an [`ReplayOp::ObjectShadow`] marker when the
    /// declared object shadow changed since the last op. Without a source
    /// no marker is ever written.
    pub fn push(&mut self, at: SlotAt, op: ReplayOp) {
        if let Some(source) = self.shadows {
            let now = source();
            if now != self.shadow {
                self.frame.ops.push((at, ReplayOp::ObjectShadow(now)));
                self.shadow = now;
            }
        }
        self.frame.ops.push((at, op));
    }

    pub fn target(&mut self, surface: S, surfaces: &[(S, AuxTarget)], roles: &[(S, TargetRole)]) -> TargetRef {
        if let Some((_, id)) = self.live.iter().find(|(s, _)| *s == surface) {
            return TargetRef::Aux(*id);
        }
        let Some((_, desc)) = surfaces.iter().find(|(s, _)| *s == surface) else {
            self.frame.refuse("unknown_surface");
            return TargetRef::None;
        };
        let Ok(id) = u16::try_from(self.frame.aux.len()) else {
            self.frame.refuse("too_many_surfaces");
            return TargetRef::None;
        };
        if let Some((_, seed)) = self.seeds.iter().find(|(s, _)| *s == surface) {
            self.frame.initial.push((TargetRef::Aux(id), seed.clone()));
        }
        self.frame.aux.push(*desc);
        self.live.push((surface, id));
        if let Some((_, role)) = roles.iter().find(|(s, _)| *s == surface) {
            self.roles.push((id, *role));
        }
        TargetRef::Aux(id)
    }

    pub fn view_identity(&mut self, surface: S) -> Arc<()> {
        if let Some((_, identity)) = self.views.iter().find(|(s, _)| *s == surface) {
            return Arc::clone(identity);
        }
        let identity = Arc::new(());
        self.views.push((surface, Arc::clone(&identity)));
        identity
    }

    pub fn view_surface(&self, identity: &Arc<()>) -> Option<S> {
        self.views.iter().find(|(_, id)| Arc::ptr_eq(id, identity)).map(|(s, _)| *s)
    }

    pub fn forget_view_identities(&mut self) {
        self.views.clear();
    }

    pub fn destroy(&mut self, surface: S) {
        self.views.retain(|(s, _)| *s != surface);
        self.seeds.retain(|(s, _)| *s != surface);
        self.live.retain(|(s, _)| *s != surface);
    }

    pub fn untracked_write(&mut self) {
        self.seeds.clear();
        self.frame.initial.clear();
        if !self.frame.ops.is_empty() {
            self.frame.refuse("write_outside_scene");
        }
    }

    pub fn bind(&mut self, at: SlotAt, colour: TargetRef, depth: TargetRef) {
        if self.bound != Some((at, colour, depth)) {
            self.push(at, ReplayOp::Bind { colour, depth });
            self.bound = Some((at, colour, depth));
        }
    }

    pub fn sample(
        &mut self, at: SlotAt, of: TargetRef, how: ReplaySample, view: Option<Arc<()>>, into: Option<u32>,
    ) -> Origin {
        let id = self.next_sample;
        self.next_sample += 1;
        let origin = Origin::sample(self.seq, Some(id), &how, view);
        self.push(at, ReplayOp::Sample { of, id, how, into });
        origin
    }

    pub fn finish(&mut self, present: TargetRef) {
        self.shadow = None;
        let mut frame = std::mem::take(&mut self.frame);
        if frame.refusals.is_empty() {
            let mut end = super::seeds::final_contents(&frame);
            for &(surface, id) in &self.live {
                self.seeds.retain(|(s, _)| *s != surface);
                if let Some(seed) = end[usize::from(id)].take() {
                    self.seeds.push((surface, seed));
                }
            }
        } else {
            // An external/unrecorded write could have touched any surface.
            self.seeds.clear();
        }
        // Only the actual present surface is the final colour. In particular
        // the source of a copy swap must remain distinct from its destination.
        let depth = frame.ops.iter().find_map(|(_, op)| match op {
            ReplayOp::Stage(stage) if stage.surfaces.color == present && stage.surfaces.depth != present => Some(stage.surfaces.depth),
            ReplayOp::Bind { colour, depth } | ReplayOp::Clear { colour, depth, .. }
            | ReplayOp::ClearRects { colour, depth, .. } if *colour == present && *depth != TargetRef::None => Some(*depth),
            _ => None,
        });
        let mut names = Vec::with_capacity(frame.aux.len());
        for (i, desc) in std::mem::take(&mut frame.aux).into_iter().enumerate() {
            let physical = TargetRef::Aux(i as u16);
            names.push(if physical == present {
                frame.colour = Some(desc);
                TargetRef::SceneColour
            } else if Some(physical) == depth {
                frame.depth = Some(desc);
                TargetRef::SceneDepth
            } else {
                let next = frame.aux.len() as u16;
                frame.aux.push(desc);
                TargetRef::Aux(next)
            });
        }
        let map = |r| match r { TargetRef::Aux(i) => names[usize::from(i)], other => other };
        frame.presented = map(present);
        frame.roles = self.roles.drain(..).map(|(id, role)| (map(TargetRef::Aux(id)), role)).collect();
        for (target, _) in &mut frame.initial {
            *target = map(*target);
        }
        for (_, op) in &mut frame.ops {
            op.map_targets(map);
        }
        self.live.clear();
        self.bound = None;
        self.next_sample = 0;
        self.seq = self.seq.wrapping_add(1);
        (self.output)(frame);
    }
}

pub(super) enum Origin {
    Upload(Arc<DecodedTexture>),
    Placeholder(TextureDesc),
    Sample {
        seq: u64,
        id: Option<u32>,
        immutable: bool,
        view: Option<Arc<()>>,
        #[cfg(feature = "gpu")]
        frozen: std::sync::OnceLock<Option<super::FrozenSample>>,
    },
}

impl Origin {
    pub fn sample(seq: u64, id: Option<u32>, how: &ReplaySample, view: Option<Arc<()>>) -> Self {
        Self::Sample {
            seq, id, view,
            immutable: !matches!(how, ReplaySample::InPlace),
            #[cfg(feature = "gpu")]
            frozen: Default::default(),
        }
    }
}

/// The original backend's texture plus the information needed to bind
/// it to a private draw. The census path does not retain texture bytes.
pub struct SinkTexture<T> {
    pub(super) inner: T,
    pub(super) origin: Option<Origin>,
}

#[cfg(test)]
mod allocation_tests {
    use super::Recorder;

    #[test]
    fn recycled_surface_handle_cannot_reidentify_an_older_view() {
        let mut recorder = Recorder::new(|_| {});
        let first = recorder.view_identity(3);
        assert_eq!(recorder.view_surface(&first), Some(3));
        recorder.destroy(3);
        let second = recorder.view_identity(3);
        assert_eq!(recorder.view_surface(&first), None);
        assert_eq!(recorder.view_surface(&second), Some(3));
        recorder.forget_view_identities();
        assert_eq!(recorder.view_surface(&second), None);
    }
}

#[cfg(test)]
mod role_tests {
    use std::sync::{Arc, Mutex};

    use frame_types::{Slot, SlotAt, TargetRef, TargetRole};
    use gfx::{Clear, D3DFormat};

    use crate::backend::TargetDesc;
    use crate::{Backend, RecordingBackend, SceneSink};

    /// A target's number is its first use in the frame, so the freeze
    /// frame's target is `Aux(0)` in one frame and the post colour's in
    /// the next; each frame says which role each number is.
    #[test]
    fn each_frame_names_its_targets_roles() {
        let frames = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&frames);
        let mut sink = SceneSink::new(RecordingBackend::new(), || Some(SlotAt { slot: Slot::AfterPost, view: 0 }))
            .with_replay(move |f| output.lock().unwrap().push(f));
        let desc = |role| TargetDesc { role, format: D3DFormat::LinA8R8G8B8, width: 4, height: 4, depth: false, texture: false };
        let (freeze, post) = (TargetRole::Game(0x12), TargetRole::Game(1));
        let main = sink.create_target(desc(TargetRole::Display(0))).unwrap();
        let freeze_s = sink.create_target(desc(freeze)).unwrap();
        let post_s = sink.create_target(desc(post)).unwrap();
        sink.clear(freeze_s, None, Clear::all([0.; 4])).unwrap();
        sink.copy_surface(post_s, main).unwrap();
        sink.present(main, 0).unwrap();
        sink.clear(post_s, None, Clear::all([0.; 4])).unwrap();
        sink.copy_surface(freeze_s, main).unwrap();
        sink.present(main, 0).unwrap();
        let taken = std::mem::take(&mut *frames.lock().unwrap());
        let [first, second] = &taken[..] else { panic!("two frames") };
        for f in [first, second] {
            assert_eq!(f.presented, TargetRef::SceneColour);
            assert_eq!(f.role(TargetRef::SceneColour), Some(TargetRole::Display(0)));
            assert_eq!(f.role(TargetRef::None), None);
        }
        assert_eq!((first.role(TargetRef::Aux(0)), first.role(TargetRef::Aux(1))), (Some(freeze), Some(post)));
        assert_eq!((second.role(TargetRef::Aux(0)), second.role(TargetRef::Aux(1))), (Some(post), Some(freeze)));
        sink.release_target(post, post_s).unwrap();
        let plain = sink.create_surface(D3DFormat::LinA8R8G8B8, 4, 4, false).unwrap();
        sink.clear(plain, None, Clear::all([0.; 4])).unwrap();
        sink.present(main, 0).unwrap();
        let third = frames.lock().unwrap().pop().unwrap();
        assert_eq!(third.role(TargetRef::Aux(0)), None, "a surface made with no role has none");
        assert_eq!(third.role(TargetRef::SceneColour), Some(TargetRole::Display(0)));
    }
}
