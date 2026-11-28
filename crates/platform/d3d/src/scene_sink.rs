//! The frame step supplies the slot and guest view for each device command.
//! In census mode the sink forwards the calls and keeps their summary for
//! `render.scene`. With [`SceneSink::with_replay`] the sink also captures
//! owned inputs and target operations for the frame owner's pass graph.
//!
//! The replay stream names distinct physical targets, including both halves
//! of a copy swap. At present its final attachment becomes `SceneColour`.
//! The census groups the swap chain separately for statistics. Its slot
//! source answers `None` while the frame step is disabled.

#[cfg(feature = "gpu")]
mod camera;
mod census;
pub mod window;
mod contents;
#[cfg(feature = "gpu")]
pub use contents::TargetSnapshots;
#[cfg(feature = "gpu")]
mod frozen;
#[cfg(feature = "gpu")]
pub use frozen::{FrozenImages, FrozenSample};
mod replay;
#[cfg(feature = "gpu")]
mod retarget;
#[cfg(feature = "gpu")]
pub use retarget::{Retarget, RetargetCounts, Retargeted};
mod seeds;
pub use seeds::TargetSeed;

pub use census::{Census, TargetUse, Uses, census};
/// The newest replay draw a following one can join.
struct OpenRun {
    /// The run's first draw, in the replay's ops.
    head: usize,
    /// The ops' length after the run's last draw.
    end: usize,
    /// The head's pass is the run's own copy.
    owned: bool,
    /// The pass the head held when the run was last extended.
    pass: crate::backend::SharedReplacement,
}

pub use replay::{ReplayFrame, ReplayOp, ReplaySample, ReplayTexture, RetainedExternal, SinkTexture};
use replay::{Origin, Recorder};

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use frame_types::{AuxTarget, Carried, CarriedSummary, GuestOp, Readback, Slot, SlotAt, TargetRef, TargetRole, TargetSample};
use gfx::texture::{DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

use crate::device::ExternalOrigin;

#[cfg(feature = "gpu")]
use crate::backend::ExternalCopies;
use crate::backend::{
    Backend, DrawJudge, DrawOutcome, ExternalPass, ExternalPassRequest, ExternalPlan, ExternalRecording, ExternalSurfaces,
    DisplaySurfacePixels, GuestLayoutKind, QueuedExternalDraw, RectClear, RenderExtents, ResolvedDraw,
    SceneStage, SharedReplacement, StreamBytes, TargetCopy,
};

/// Guest proxy and GPU backend tickets for the reports in a submitted frame,
/// paired in the order that the guest requested them.
pub type FlareTickets = Vec<(u64, u64)>;

/// Where the frame step is, or `None` while the sink records nothing.
pub type SlotSource = fn() -> Option<SlotAt>;

pub(crate) fn retains_draw_stage(stage: timeline::native::Stage) -> bool {
    use timeline::native::Stage;
    matches!(stage, Stage::Flares | Stage::Sky | Stage::Water | Stage::Transparents | Stage::Objects | Stage::Particles | Stage::Ui)
}

/// The native producers whose commit a replaying sink carries
/// ([`Backend::begin_native_carry`]): each issues the packets that are the
/// draws its guest code would (one, or a UI batch's head and siblings), as
/// a pass that records again from those packets and the textures captured
/// with them. A sorted transparent range and an object part are carried the
/// same way: their indexed pass replays the range from the packet. Particles and sky are
/// carried only as the guest's own admitted draws.
pub(crate) fn carries_native_stage(stage: timeline::native::Stage) -> bool {
    use timeline::native::Stage;
    matches!(stage, Stage::Water | Stage::Flares | Stage::Ui | Stage::Transparents | Stage::Objects) && retains_draw_stage(stage)
}

/// Why a replaying sink cannot keep a native pass that carries nothing, for
/// the refusal's report. Post's glow and screen effects are carried as
/// typed stages ([`ReplayOp::Stage`], through
/// [`Backend::prepare_carried_stage`]); a post pass that reaches a plain
/// reservation was not sealed as one.
fn unretained_cause(stage: timeline::native::Stage) -> &'static str {
    use timeline::native::Stage;
    match stage {
        Stage::Post => "a post pass is carried only as a sealed scene stage; this one reached a plain reservation",
        Stage::Ui | Stage::Flares | Stage::Water | Stage::Transparents | Stage::Objects => "the pass has no exact packets the replay can retain (a depth write, a sampled or side target, a pass that cannot record again, or a guest reference request)",
        _ => "the scene recorder cannot retain an unsealed reservation",
    }
}

/// The scene's object shadow the guest is drawing, by index into the
/// sealed scene's `object_shadows`; `None` outside one.
pub type ShadowSource = fn() -> Option<u32>;

/// The source of a sink that never records.
pub fn never() -> Option<SlotAt> {
    None
}

/// Presented surfaces remembered as the scene's colour (a flip chain and
/// the copy swap's source).
const SCENE_COLOURS: usize = 4;

static FRAMES: AtomicU64 = AtomicU64::new(0);
static GAME_FRAMES: AtomicU64 = AtomicU64::new(0);
static DRAWS: AtomicU64 = AtomicU64::new(0);
static OPS: AtomicU64 = AtomicU64::new(0);
static READBACK: [AtomicU64; Readback::COUNT] = [const { AtomicU64::new(0) }; Readback::COUNT];
static SIDE_FRAMES: AtomicU64 = AtomicU64::new(0);
static LAST: Mutex<Option<CarriedSummary>> = Mutex::new(None);

/// The sink's totals since the start (`render.scene`'s counters):
/// `scene_carried_frames` (frames recorded), `scene_carried_draws` and
/// `scene_carried_ops` (of every slot), `scene_readback` and
/// `scene_readback_<kind>` (raw game-view requests, including flare reports)
/// and `scene_side_target_frames` (frames that named a side target in a
/// game view). The completed window matches reports to their native tickets.
pub fn counters() -> Vec<(String, u64)> {
    let mut out = vec![
        ("scene_carried_frames".to_owned(), FRAMES.load(Ordering::Relaxed)),
        ("scene_carried_game_frames".to_owned(), GAME_FRAMES.load(Ordering::Relaxed)),
        ("scene_carried_draws".to_owned(), DRAWS.load(Ordering::Relaxed)),
        ("scene_carried_ops".to_owned(), OPS.load(Ordering::Relaxed)),
        ("scene_side_target_frames".to_owned(), SIDE_FRAMES.load(Ordering::Relaxed)),
    ];
    let each: Vec<u64> = READBACK.iter().map(|c| c.load(Ordering::Relaxed)).collect();
    out.push(("scene_readback".to_owned(), each.iter().sum()));
    for (kind, n) in Readback::ALL.into_iter().zip(each) {
        out.push((format!("scene_readback_{}", kind.name()), n));
    }
    out
}

/// The open frame's carried slots: a bit per [`Slot`] index, and
/// [`UNTRACKED`] for a write made while no slot was declared. Cleared at
/// each present.
static OPEN: AtomicU32 = AtomicU32::new(0);
const UNTRACKED: u32 = 1 << 31;

/// What the open frame (since the last present) filed: the publish's test
/// (docs/graphics/retire-d3d-display.md, "The final display").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenFrame {
    /// No guest op: every slot draws from the scene.
    Scene,
    /// The first slot, in the frame's order, with a guest op.
    Carried(Slot),
    /// A guest write while the frame step declared no slot.
    Untracked,
}

pub fn open_frame() -> OpenFrame {
    decode_open(OPEN.load(Ordering::Relaxed))
}

fn decode_open(open: u32) -> OpenFrame {
    if open & UNTRACKED != 0 {
        return OpenFrame::Untracked;
    }
    match Slot::ALL.into_iter().find(|s| open & (1 << s.index()) != 0) {
        Some(slot) => OpenFrame::Carried(slot),
        None => OpenFrame::Scene,
    }
}

/// The last recorded frame's carried part, counted.
pub fn last() -> Option<CarriedSummary> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Whether any sink has recorded a frame.
pub fn recording() -> bool {
    FRAMES.load(Ordering::Relaxed) != 0
}

/// A draw as the sink keeps it: the draw itself when asked
/// ([`SceneSink::keeping_draws`]), else nothing.
pub type KeptDraw = Option<Box<ResolvedDraw>>;

pub struct SceneSink<B: Backend> {
    inner: B,
    source: SlotSource,
    collect_census: bool,
    present_window: Option<Arc<window::PresentWindow>>,
    take_game_view: fn() -> bool,
    keep: bool,
    replay: Option<Recorder<B::Surface>>,
    shadows: Option<ShadowSource>,
    presents: u64,
    guest_reference: Box<dyn Fn() -> bool + Send>,
    /// A native producer's pass the replay will carry
    /// ([`Backend::begin_native_carry`]), until it commits.
    carry: Option<Box<RetainedExternal>>,
    /// The last `prepare_external` was refused by the sink itself.
    refused_reservation: bool,
    /// The carried pass is committing: its recordings are retained, not
    /// refused as unrecorded.
    carrying: bool,
    /// The last draw of the replay that a following adjacent draw of the
    /// same state can join ([`Self::join_run`]).
    run: Option<OpenRun>,
    present_error: Option<Box<dyn FnMut(u64) + Send>>,
    frame: Carried<KeptDraw>,
    last: Carried<KeptDraw>,
    /// The last bind emitted this frame, and where.
    bound: Option<(TargetRef, TargetRef, SlotAt)>,
    /// The scene's colour surfaces, newest last.
    scene_colour: Vec<B::Surface>,
    scene_depth: Option<B::Surface>,
    /// Every live surface, as made.
    surfaces: Vec<(B::Surface, AuxTarget)>,
    /// The roles of the surfaces the device made by role ([`Backend::create_target`]).
    roles: Vec<(B::Surface, TargetRole)>,
    /// This frame's side targets, by number.
    aux: Vec<B::Surface>,
}

impl<B: Backend> SceneSink<B> {
    pub fn new(inner: B, source: SlotSource) -> Self {
        Self {
            inner,
            source,
            collect_census: true,
            present_window: None,
            take_game_view: || false,
            keep: false,
            replay: None,
            shadows: None,
            presents: 0,
            guest_reference: Box::new(|| false),
            carry: None,
            refused_reservation: false,
            carrying: false,
            run: None,
            present_error: None,
            frame: Carried::default(),
            last: Carried::default(),
            bound: None,
            scene_colour: Vec::new(),
            scene_depth: None,
            surfaces: Vec::new(),
            roles: Vec::new(),
            aux: Vec::new(),
        }
    }

    /// Enable the full cross-frame census of transient targets and reads.
    /// A rendering worker can retain its ordinary counters without making
    /// a second pass over every draw to collect the census.
    pub fn with_census(mut self, enabled: bool) -> Self {
        self.collect_census = enabled;
        self
    }

    /// Joins the original guest Swap to its display worker's result.
    pub fn with_present_window(mut self, window: Option<Arc<window::PresentWindow>>, game_view: fn() -> bool) -> Self {
        self.present_window = window;
        self.take_game_view = game_view;
        self
    }

    /// Keeps each draw in the carried part (tests; a census keeps none).
    pub fn keeping_draws(mut self) -> Self {
        self.keep = true;
        self
    }

    /// Retain complete guest draws and their inputs for native replay. The
    /// receiver gets ownership at the guest present. Only repeatable typed
    /// stages can replace guest draws while recording.
    pub fn with_replay(mut self, on_present: impl FnMut(ReplayFrame) + Send + 'static) -> Self {
        let mut recorder = Recorder::new(on_present);
        recorder.shadows = self.shadows;
        self.replay = Some(recorder);
        self
    }

    /// A parity request needs a frame that reaches the reference backend
    /// entirely as guest draws. The device asks before each guest skip.
    pub fn with_guest_reference_request(mut self, requested: impl Fn() -> bool + Send + 'static) -> Self {
        self.guest_reference = Box::new(requested);
        self
    }

    /// Cancel the pending scene/reference join if this guest swap fails.
    pub fn on_present_error(mut self, cancel: impl FnMut(u64) + Send + 'static) -> Self {
        self.present_error = Some(Box::new(cancel));
        self
    }

    fn replaying(&self) -> bool {
        self.replay.is_some() && self.at().is_some()
    }

    fn replay_op(&mut self, at: SlotAt, op: ReplayOp) {
        if let Some(replay) = &mut self.replay {
            replay.push(at, op);
        }
    }

    /// Marks each object shadow's ops in the replay
    /// ([`ReplayOp::ObjectShadow`]). The marks draw nothing.
    pub fn with_object_shadows(mut self, shadow_at: ShadowSource) -> Self {
        self.shadows = Some(shadow_at);
        if let Some(replay) = &mut self.replay {
            replay.shadows = Some(shadow_at);
        }
        self
    }

    /// Replay target identity remains physical until present identifies
    /// the final surface. The census' swap-chain alias heuristic is only
    /// for describing counts; a copy swap still has two distinct targets.
    fn replay_target(&mut self, surface: B::Surface) -> TargetRef {
        self.replay.as_mut().expect("recording replay").target(surface, &self.surfaces, &self.roles)
    }

    fn replay_attachments(&mut self, colour: B::Surface, depth: Option<B::Surface>) -> (TargetRef, TargetRef) {
        (self.replay_target(colour), depth.map_or(TargetRef::None, |d| self.replay_target(d)))
    }

    /// The one path of a shared or admitted draw: the backend gets it and
    /// answers as for any guest draw, and a replay under record carries it
    /// (with `admission`, when exact) only if it was drawn.
    fn draw_carried(
        &mut self, color: B::Surface, depth: Option<B::Surface>, draw: Arc<ResolvedDraw>, textures: [Option<&SinkTexture<B::Texture>>; 4],
        admission: Option<crate::backend::RetainedAdmission<B::Surface>>,
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        self.record_draw(color, depth, &draw);
        let inputs = textures.map(|t| t.map(|t| &t.inner));
        if !self.replaying() {
            return self.inner.draw_shared(color, depth, draw, inputs);
        }
        let result = self.inner.draw_shared(color, depth, Arc::clone(&draw), inputs);
        if matches!(result.0, Ok(DrawOutcome::Drawn)) {
            let external = admission.and_then(|a| self.retain_admission(a, std::slice::from_ref(&draw), color, depth));
            self.replay_draw(color, depth, draw, textures, external);
            self.join_run();
        }
        result
    }

    /// Joins the replay's newest draw to the run of adjacent draws before
    /// it when their passes can be one ([`ExternalPass::run_with`]) on the
    /// same attachments with nothing recorded between them: the run's
    /// head then holds the one pass, which records every packet, and the
    /// draws after it follow ([`RetainedExternal::follows`]). The pass the
    /// guest's own queue holds is never altered; the run's pass is a copy
    /// made at its first join.
    fn join_run(&mut self) {
        let Some(replay) = self.replay.as_mut() else {
            self.run = None;
            return;
        };
        let ops = &mut replay.frame.ops;
        let end = ops.len();
        let Some(last) = end.checked_sub(1) else {
            self.run = None;
            return;
        };
        let (before, tail) = ops.split_at_mut(last);
        let ReplayOp::Draw { external: Some(next), .. } = &mut tail[0].1 else {
            self.run = None;
            return;
        };
        if next.at != 0 || next.packets.len() != 1 {
            self.run = None;
            return;
        }
        let fresh = OpenRun { head: last, end, owned: false, pass: Arc::clone(&next.pass) };
        let Some(run) = self.run.take().filter(|r| r.end + 1 == end && r.head < last) else {
            self.run = Some(fresh);
            return;
        };
        let ReplayOp::Draw { external: Some(head), .. } = &mut before[run.head].1 else {
            self.run = Some(fresh);
            return;
        };
        let same = Arc::ptr_eq(&run.pass, &head.pass)
            && head.plan.stage == next.plan.stage
            && head.surfaces.color == next.surfaces.color
            && head.surfaces.depth == next.surfaces.depth
            && head.request.surface_size == next.request.surface_size;
        let merged = same
            .then(|| {
                let (h, n) = (head.pass.lock().ok()?, next.pass.lock().ok()?);
                h.run_with(&**n)
            })
            .flatten();
        let Some(merged) = merged else {
            self.run = Some(fresh);
            return;
        };
        if run.owned {
            if let Ok(mut pass) = head.pass.lock() {
                *pass = merged;
            }
        } else {
            head.pass = Arc::new(std::sync::Mutex::new(merged));
        }
        let mut packets = head.packets.to_vec();
        packets.push(Arc::clone(&next.packets[0]));
        let packets: Arc<[Arc<ResolvedDraw>]> = packets.into();
        head.packets = Arc::clone(&packets);
        head.plan.ranges = packets.len();
        next.at = packets.len() - 1;
        next.packets = packets;
        next.pass = Arc::clone(&head.pass);
        next.plan = head.plan.clone();
        self.run = Some(OpenRun { head: run.head, end, owned: true, pass: Arc::clone(&head.pass) });
    }

    /// What a replay keeps of a typed admission offered for `draws` (the
    /// guest's one draw, or a native producer's packets in order), or
    /// `None`: the admission is not exact for them and replay draws the
    /// packets alone. Checked here, at capture; replay checks again against
    /// what it resolves. The result is the first draw's (`at` 0).
    fn retain_admission(
        &mut self, admission: crate::backend::RetainedAdmission<B::Surface>, draws: &[Arc<ResolvedDraw>],
        colour: B::Surface, depth: Option<B::Surface>,
    ) -> Option<Box<RetainedExternal>> {
        use timeline::native::Stage;
        let s = &admission.surfaces;
        let known = |surface: &B::Surface| self.surfaces.iter().any(|&(id, _)| id == *surface);
        let attachments = s.color == colour && s.depth == depth.unwrap_or(colour);
        let room = self.replay.as_ref()?.frame.aux.len() + self.surfaces.len() <= usize::from(u16::MAX) + 1;
        if (self.guest_reference)()
            || !attachments
            || !s.sampled.is_empty() || !s.slots.is_empty() || !s.slot_sizes.is_empty()
            || !known(&s.color) || !known(&s.depth) || !room
            || !retains_draw_stage(admission.plan.stage)
            || draws.is_empty()
            || (admission.plan.stage == Stage::Flares && draws.iter().any(|d| d.writes_depth_stencil()))
            || admission.packets.len() != draws.len()
            || !admission.packets.iter().zip(draws).all(|(p, d)| Arc::ptr_eq(p, d) || **p == **d)
            || !admission.pass.lock().is_ok_and(|p| p.replays_scene())
        {
            return None;
        }
        let surfaces = s.try_map(|surface| Some(self.replay_target(*surface)))?;
        Some(Box::new(RetainedExternal {
            packets: draws.iter().cloned().collect(),
            at: 0,
            surfaces,
            request: admission.request,
            plan: admission.plan,
            pass: admission.pass,
        }))
    }

    fn replay_stage(&mut self, stage: &SceneStage<B::Surface>) -> Option<(SlotAt, ReplayOp)> {
        let at = self.write_slot()?;
        self.replay.as_ref()?;
        if !stage.pass.lock().is_ok_and(|pass| pass.replays_scene()) {
            self.refuse_replay("nonrepeatable_scene_stage");
            return None;
        }
        let surfaces = stage.surfaces.try_map(|s| Some(self.replay_target(*s)))?;
        Some((at, ReplayOp::Stage(SceneStage {
            surfaces,
            request: stage.request,
            plan: stage.plan.clone(),
            pass: Arc::clone(&stage.pass),
        })))
    }

    /// A write that bypasses recording invalidates mutable attachment history.
    fn untracked_write(&mut self) {
        if let Some(replay) = &mut self.replay {
            replay.untracked_write();
        }
    }

    fn write_slot(&mut self) -> Option<SlotAt> {
        let at = self.at();
        if at.is_none() {
            OPEN.fetch_or(UNTRACKED, Ordering::Relaxed);
            self.untracked_write();
        }
        at
    }

    /// Files a guest op at `at` in the frame's carried part, and marks its
    /// slot carried in the open frame ([`open_frame`]).
    fn file(&mut self, at: SlotAt, op: GuestOp<KeptDraw>) {
        OPEN.fetch_or(1 << at.slot.index(), Ordering::Relaxed);
        self.frame.push(at, op);
    }

    fn refuse_replay(&mut self, cause: &'static str) {
        if self.write_slot().is_some() && let Some(replay) = &mut self.replay {
            replay.frame.refuse(cause);
        }
    }

    pub fn inner(&self) -> &B {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut B {
        // The caller can destroy and recreate a surface behind the sink.
        if let Some(replay) = &mut self.replay { replay.forget_view_identities(); }
        self.untracked_write();
        &mut self.inner
    }

    /// The carried part of the last frame presented while recording.
    pub fn last_frame(&self) -> &Carried<KeptDraw> {
        &self.last
    }

    fn at(&self) -> Option<SlotAt> {
        (self.source)()
    }

    fn made(&mut self, surface: B::Surface, target: AuxTarget) {
        if let Some(replay) = &mut self.replay { replay.destroy(surface); }
        self.surfaces.retain(|(s, _)| *s != surface);
        self.roles.retain(|(s, _)| *s != surface);
        self.surfaces.push((surface, target));
    }

    fn forget(&mut self, surface: B::Surface) {
        self.surfaces.retain(|(s, _)| *s != surface);
        self.roles.retain(|(s, _)| *s != surface);
        self.scene_colour.retain(|s| *s != surface);
        if self.scene_depth == Some(surface) {
            self.scene_depth = None;
        }
    }

    fn target(&mut self, surface: B::Surface) -> TargetRef {
        if self.scene_colour.contains(&surface) {
            return TargetRef::SceneColour;
        }
        if self.scene_depth == Some(surface) {
            return TargetRef::SceneDepth;
        }
        let n = match self.aux.iter().position(|s| *s == surface) {
            Some(n) => n,
            None => {
                let target = self.surfaces.iter().find(|(s, _)| *s == surface).map_or_else(AuxTarget::default, |(_, t)| *t);
                self.aux.push(surface);
                self.frame.aux.push(target);
                self.aux.len() - 1
            }
        };
        TargetRef::Aux(u16::try_from(n).unwrap_or(u16::MAX))
    }

    /// The colour and depth of a draw or clear; a depth bound with the
    /// scene's colour is the scene's depth.
    fn attachments(&mut self, colour: B::Surface, depth: Option<B::Surface>) -> (TargetRef, TargetRef) {
        let c = self.target(colour);
        if c == TargetRef::SceneColour
            && let Some(d) = depth
        {
            self.scene_depth = Some(d);
        }
        (c, depth.map_or(TargetRef::None, |d| self.target(d)))
    }

    fn bind(&mut self, at: SlotAt, colour: B::Surface, depth: Option<B::Surface>) {
        let (c, d) = self.attachments(colour, depth);
        if self.bound != Some((c, d, at)) {
            self.file(at, GuestOp::Bind { colour: c, depth: d });
            self.bound = Some((c, d, at));
        }
    }

    fn record_draw(&mut self, colour: B::Surface, depth: Option<B::Surface>, draw: &ResolvedDraw) {
        let Some(at) = self.write_slot() else { return };
        self.bind(at, colour, depth);
        let kept = self.keep.then(|| Box::new(draw.clone()));
        self.file(at, GuestOp::Draw(kept));
    }

    fn replay_draw(
        &mut self, colour: B::Surface, depth: Option<B::Surface>, draw: Arc<ResolvedDraw>, textures: [Option<&SinkTexture<B::Texture>>; 4],
        external: Option<Box<RetainedExternal>>,
    ) {
        let Some(at) = self.at() else { return };
        let (c, d) = self.replay_attachments(colour, depth);
        let replay = self.replay.as_mut().expect("recording replay");
        replay.bind(at, c, d);
        let inputs = textures.map(|t| {
            let texture = t?;
            match &texture.origin {
                Some(Origin::Upload(data)) => Some(ReplayTexture::Upload(data.clone())),
                Some(Origin::Placeholder(desc)) => Some(ReplayTexture::Placeholder(desc.clone())),
                Some(Origin::Sample { seq, id: Some(id), .. }) if *seq == replay.seq => {
                    Some(ReplayTexture::Sample(*id))
                }
                Some(Origin::Sample { seq, immutable: true, .. }) if *seq != replay.seq => {
                    #[cfg(feature = "gpu")]
                    if let Some(Origin::Sample { frozen, .. }) = &texture.origin
                        && let Some(sample) = frozen.get_or_init(|| self.inner.retain_scene_sample(&texture.inner))
                    {
                        return Some(ReplayTexture::Frozen(sample.clone()));
                    }
                    replay.frame.refuse("unavailable_frozen_sample");
                    None
                }
                Some(Origin::Sample { seq, immutable: false, view, .. }) if *seq != replay.seq => {
                    let Some(surface) = view.as_ref().and_then(|id| replay.view_surface(id)) else {
                        replay.frame.refuse("released_in_place_source");
                        return None;
                    };
                    #[cfg(feature = "gpu")]
                    if let Some(Origin::Sample { frozen, .. }) = &texture.origin
                        && let Some(binding) = frozen.get_or_init(|| self.inner.retain_scene_sample(&texture.inner))
                    {
                        let target = replay.target(surface, &self.surfaces, &self.roles);
                        return Some(ReplayTexture::Live { target, binding: binding.clone() });
                    }
                    #[cfg(not(feature = "gpu"))]
                    let _ = surface;
                    replay.frame.refuse("unavailable_in_place_binding");
                    None
                }
                _ => {
                    replay.frame.refuse("uncaptured_texture");
                    None
                }
            }
        });
        let camera = if draw.layout.pretransformed {
            frame_types::carried::CameraUse::Screen
        } else {
            frame_types::carried::CameraUse::Unknown
        };
        replay.push(at, ReplayOp::Draw { draw, textures: inputs, camera, external });
    }

    fn record_clear(&mut self, colour: B::Surface, depth: Option<B::Surface>, rects: bool) {
        let Some(at) = self.write_slot() else { return };
        let (c, d) = self.attachments(colour, depth);
        self.file(at, GuestOp::Clear { colour: c, depth: d, rects });
    }

    fn record_sample(&mut self, of: B::Surface, how: TargetSample) {
        let Some(at) = self.at() else { return };
        let of = self.target(of);
        self.file(at, GuestOp::Sample { of, how });
    }

    fn sample_texture(&mut self, inner: B::Texture, surface: B::Surface, how: TargetSample, kind: impl FnOnce() -> ReplaySample) -> SinkTexture<B::Texture> {
        let origin = self.sample_origin(surface, how, kind, None);
        SinkTexture { inner, origin }
    }

    /// Records a sample of `surface` and gives the origin of the texture
    /// that holds it. `into`: the earlier sample of this frame whose
    /// texture the sample refills.
    fn sample_origin(&mut self, surface: B::Surface, how: TargetSample, kind: impl FnOnce() -> ReplaySample, into: Option<u32>) -> Option<Origin> {
        self.record_sample(surface, how);
        let at = self.at();
        self.replay.as_mut().map(|replay| {
            let how = kind();
            let view = (matches!(how, ReplaySample::InPlace)
                && self.surfaces.iter().any(|(s, desc)| *s == surface && !desc.depth))
                .then(|| replay.view_identity(surface));
            if let Some(at) = at {
                let of = replay.target(surface, &self.surfaces, &self.roles);
                replay.sample(at, of, how, view, into)
            } else {
                Origin::sample(replay.seq, None, &how, view)
            }
        })
    }

    fn record_readback(&mut self, kind: Readback) {
        if let Some(at) = self.at() {
            self.frame.readback(at, kind);
        }
    }

    fn presented(&mut self, surface: B::Surface, flags: u32, gamma: Option<&[u8; 768]>) {
        self.presents += 1;
        OPEN.store(0, Ordering::Relaxed);
        let entered_game_view = (self.take_game_view)();
        if self.replay.is_some() && (entered_game_view || self.at().is_some() || !self.frame.ops.is_empty()) {
            let shown = self.replay_target(surface);
            let replay = self.replay.as_mut().expect("recording replay");
            replay.frame.present = self.presents;
            replay.frame.entered_game_view = entered_game_view;
            replay.frame.flags = flags;
            replay.frame.gamma = gamma.copied().map(Box::new);
            replay.finish(shown);
        }
        self.scene_colour.retain(|s| *s != surface);
        self.scene_colour.push(surface);
        if self.scene_colour.len() > SCENE_COLOURS {
            self.scene_colour.remove(0);
        }
        if !entered_game_view && self.at().is_none() && self.frame.ops.is_empty() && self.frame.readbacks.is_empty() {
            if let Some(window) = &self.present_window {
                window.submit(self.presents, window::Input {
                    drops: crate::scene_stage::take_local_drops(), ..Default::default()
                });
            }
            return;
        }
        let summary = self.frame.summary();
        if let Some(window) = &self.present_window {
            window.submit(self.presents, window::Input {
                game_view: entered_game_view || !summary.views.is_empty() || summary.readback_total() != 0,
                reads: summary.readbacks,
                drops: crate::scene_stage::take_local_drops(),
            });
        }
        if self.collect_census {
            census::record(&self.frame, self.presents, self.present_window.as_deref());
        } else {
            census::skip();
        }
        if entered_game_view || !summary.views.is_empty() {
            GAME_FRAMES.fetch_add(1, Ordering::Relaxed);
        }
        FRAMES.fetch_add(1, Ordering::Relaxed);
        DRAWS.fetch_add(summary.draws(), Ordering::Relaxed);
        OPS.fetch_add(summary.slots.iter().map(|c| u64::from(c.total())).sum(), Ordering::Relaxed);
        for (c, &n) in READBACK.iter().zip(&summary.readbacks) {
            c.fetch_add(u64::from(n), Ordering::Relaxed);
        }
        if !summary.side_targets.is_empty() {
            SIDE_FRAMES.fetch_add(1, Ordering::Relaxed);
        }
        *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(summary);
        std::mem::swap(&mut self.frame, &mut self.last);
        self.frame.clear();
        self.aux.clear();
        self.bound = None;
    }
}

// Every method is forwarded: a provided method left out would silently
// replace the inner backend's with the trait's default
// (`forwards_every_backend_method` below).
impl<B: Backend> Backend for SceneSink<B> {
    type Surface = B::Surface;
    type Texture = SinkTexture<B::Texture>;

    #[cfg(feature = "gpu")]
    fn frozen_images(&self, texture: &Self::Texture) -> Option<FrozenImages> {
        self.inner.frozen_images(&texture.inner)
    }

    #[cfg(feature = "gpu")]
    fn retain_scene_sample(&self, texture: &Self::Texture) -> Option<FrozenSample> {
        self.inner.retain_scene_sample(&texture.inner)
    }

    fn create_surface(&mut self, format: D3DFormat, width: u32, height: u32, depth: bool) -> Result<Self::Surface, String> {
        let s = self.inner.create_surface(format, width, height, depth)?;
        self.made(s, AuxTarget { width, height, format: u32::from(format as u8), depth });
        Ok(s)
    }
    fn destroy_surface(&mut self, surface: Self::Surface) -> Result<(), String> {
        if let Some(replay) = &mut self.replay {
            replay.destroy(surface);
        }
        self.forget(surface);
        self.inner.destroy_surface(surface)
    }
    fn create_target(&mut self, target: crate::backend::TargetDesc) -> Result<Self::Surface, String> {
        let s = self.inner.create_target(target)?;
        let crate::backend::TargetDesc { role, format, width, height, depth, .. } = target;
        self.made(s, AuxTarget { width, height, format: u32::from(format as u8), depth });
        self.roles.push((s, role));
        Ok(s)
    }
    fn release_target(&mut self, role: TargetRole, surface: Self::Surface) -> Result<(), String> {
        if let Some(replay) = &mut self.replay {
            replay.destroy(surface);
        }
        self.forget(surface);
        self.inner.release_target(role, surface)
    }
    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<Self::Texture, String> {
        if self.replay.is_some() {
            return self.upload_shared(Arc::new(texture.clone()));
        }
        Ok(SinkTexture { inner: self.inner.upload_texture(texture)?, origin: None })
    }
    fn upload_shared(&mut self, texture: Arc<DecodedTexture>) -> Result<Self::Texture, String> {
        let origin = self.replay.as_ref().map(|_| Origin::Upload(Arc::clone(&texture)));
        let inner = self.inner.upload_shared(texture)?;
        Ok(SinkTexture { inner, origin })
    }
    fn uses_texture_placeholders(&self) -> bool {
        self.inner.uses_texture_placeholders()
    }
    fn texture_placeholder(&mut self, desc: &TextureDesc) -> Result<Self::Texture, String> {
        let inner = self.inner.texture_placeholder(desc)?;
        let origin = self.replay.as_ref().map(|_| Origin::Placeholder(desc.clone()));
        Ok(SinkTexture { inner, origin })
    }
    fn bc_supported(&self) -> bool {
        self.inner.bc_supported()
    }
    fn clear(&mut self, color: Self::Surface, depth: Option<Self::Surface>, clear: Clear) -> Result<(), String> {
        self.inner.clear(color, depth, clear)?;
        self.record_clear(color, depth, false);
        if let Some(at) = self.at() && self.replay.is_some() {
            let (colour, depth) = self.replay_attachments(color, depth);
            self.replay_op(at, ReplayOp::Clear { colour, depth, clear });
        }
        Ok(())
    }
    fn draw(
        &mut self, color: Self::Surface, depth: Option<Self::Surface>, draw: &ResolvedDraw, textures: [Option<&Self::Texture>; 4],
    ) -> Result<DrawOutcome, String> {
        if self.replaying() {
            return self.draw_shared(color, depth, Arc::new(draw.clone()), textures).0;
        }
        self.record_draw(color, depth, draw);
        self.inner.draw(color, depth, draw, textures.map(|t| t.map(|t| &t.inner)))
    }
    fn draw_owned(
        &mut self, color: Self::Surface, depth: Option<Self::Surface>, draw: ResolvedDraw, textures: [Option<&Self::Texture>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        if self.replaying() {
            return self.draw_shared(color, depth, Arc::new(draw), textures);
        }
        self.record_draw(color, depth, &draw);
        self.inner.draw_owned(color, depth, draw, textures.map(|t| t.map(|t| &t.inner)))
    }
    fn draw_shared(
        &mut self, color: Self::Surface, depth: Option<Self::Surface>, draw: Arc<ResolvedDraw>, textures: [Option<&Self::Texture>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        self.draw_carried(color, depth, draw, textures, None)
    }
    fn draw_admitted(
        &mut self, color: Self::Surface, depth: Option<Self::Surface>, draw: ResolvedDraw, textures: [Option<&Self::Texture>; 4],
        admission: crate::backend::RetainedAdmission<Self::Surface>,
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        if !self.replaying() {
            return self.draw_owned(color, depth, draw, textures);
        }
        self.draw_carried(color, depth, Arc::new(draw), textures, Some(admission))
    }
    fn frame_stats(&mut self) {
        self.inner.frame_stats()
    }
    fn present_replay(&mut self, surface: Self::Surface, gamma: Option<&[u8; 768]>) -> Result<(), String> {
        self.inner.present_replay(surface, gamma)
    }
    fn queue_frame_probe_read(
        &mut self, surface: Self::Surface, width: u32, height: u32,
        done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
    ) {
        self.inner.queue_frame_probe_read(surface, width, height, done)
    }

    fn measure_present_raster(&mut self, surface: Option<Self::Surface>) -> crate::backend::RasterMeasurement {
        self.inner.measure_present_raster(surface)
    }
    fn last_present_unshown(&self) -> bool {
        self.inner.last_present_unshown()
    }

    fn take_presented_extent(&mut self) -> Option<(u32, u32)> {
        self.inner.take_presented_extent()
    }
    fn surface_extent(&self, surface: Self::Surface) -> Option<(u32, u32)> {
        self.inner.surface_extent(surface)
    }
    fn render_extents(
        &mut self,
        color: Self::Surface,
        depth: Option<Self::Surface>,
        textures: [Option<&Self::Texture>; 4],
    ) -> RenderExtents {
        self.inner.render_extents(color, depth, textures.map(|t| t.map(|t| &t.inner)))
    }
    fn read_display_surface(
        &mut self,
        surface: Self::Surface,
    ) -> Result<Option<DisplaySurfacePixels>, String> {
        self.inner.read_display_surface(surface)
    }
    fn reset_present_metadata(&mut self) -> Result<(), String> {
        self.inner.reset_present_metadata()
    }

    fn drain(&mut self) -> Result<(), String> {
        self.inner.drain()
    }
    fn present(&mut self, surface: Self::Surface, flags: u32) -> Result<(), String> {
        self.presented(surface, flags, None);
        let result = self.inner.present(surface, flags);
        if result.is_err() && let Some(cancel) = self.present_error.as_mut() { cancel(self.presents); }
        result
    }
    fn present_with_gamma(&mut self, surface: Self::Surface, flags: u32, gamma: Option<&[u8; 768]>) -> Result<(), String> {
        self.presented(surface, flags, gamma);
        let result = self.inner.present_with_gamma(surface, flags, gamma);
        if result.is_err() && let Some(cancel) = self.present_error.as_mut() { cancel(self.presents); }
        result
    }
    fn clear_rects(&mut self, color: Self::Surface, depth: Option<Self::Surface>, clear: &RectClear) -> Result<DrawOutcome, String> {
        let outcome = self.inner.clear_rects(color, depth, clear)?;
        self.record_clear(color, depth, true);
        if outcome == DrawOutcome::Drawn && let Some(at) = self.at() && self.replay.is_some() {
            let (colour, depth) = self.replay_attachments(color, depth);
            self.replay_op(at, ReplayOp::ClearRects { colour, depth, clear: clear.clone() });
        }
        Ok(outcome)
    }
    fn read_surface(
        &mut self, surface: Self::Surface, format: D3DFormat, width: u32, height: u32, layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        self.record_readback(Readback::Surface);
        self.inner.read_surface(surface, format, width, height, layout)
    }
    fn read_depth(
        &mut self, surface: Self::Surface, format: D3DFormat, width: u32, height: u32, layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        self.record_readback(Readback::Depth);
        self.inner.read_depth(surface, format, width, height, layout)
    }
    fn copy_surface(&mut self, src: Self::Surface, dst: Self::Surface) -> Result<bool, String> {
        // A copy swap shows a copy of the surface the guest drew into.
        if self.scene_colour.contains(&dst) && !self.scene_colour.contains(&src) {
            self.scene_colour.push(src);
            if self.scene_colour.len() > SCENE_COLOURS {
                self.scene_colour.remove(0);
            }
        }
        let copied = self.inner.copy_surface(src, dst)?;
        if copied && let Some(at) = self.write_slot() {
            let (from, to) = (self.target(src), self.target(dst));
            self.file(at, GuestOp::Copy { from, to });
            if self.replay.is_some() {
                let (from, to) = (self.replay_target(src), self.replay_target(dst));
                self.replay_op(at, ReplayOp::Copy { from, to });
            }
        }
        Ok(copied)
    }
    fn write_surface(&mut self, surface: Self::Surface, desc: &TextureDesc, bytes: &[u8]) -> Result<bool, String> {
        let written = self.inner.write_surface(surface, desc, bytes)?;
        if written && let Some(at) = self.write_slot() {
            let to = self.target(surface);
            self.file(at, GuestOp::Upload { to });
            if self.replay.is_some() {
                let to = self.replay_target(surface);
                self.replay_op(at, ReplayOp::Upload { to, desc: desc.clone(), bytes: Arc::from(bytes) });
            }
        }
        Ok(written)
    }
    fn snapshot_surface(&mut self, surface: Self::Surface, opaque: bool) -> Result<Option<Self::Texture>, String> {
        let r = self.inner.snapshot_surface(surface, opaque)?;
        Ok(r.map(|inner| self.sample_texture(inner, surface, TargetSample::Snapshot, || ReplaySample::Snapshot { opaque })))
    }
    /// A refill is a new sample of `surface` in `into`'s texture: draws
    /// after it bind the new sample, and when the texture's last sample was
    /// in this frame, replay refills that sample's texture
    /// ([`ReplayOp::Sample`]'s `into`) rather than make one.
    fn snapshot_surface_into(&mut self, surface: Self::Surface, opaque: bool, into: &mut Self::Texture) -> Result<bool, String> {
        if !self.inner.snapshot_surface_into(surface, opaque, &mut into.inner)? {
            return Ok(false);
        }
        let previous = self.replay.as_ref().and_then(|replay| match &into.origin {
            Some(Origin::Sample { seq, id: Some(id), immutable: true, .. }) if *seq == replay.seq => Some(*id),
            _ => None,
        });
        into.origin = self.sample_origin(surface, TargetSample::Snapshot, || ReplaySample::Snapshot { opaque }, previous);
        Ok(true)
    }
    fn clear_pixel_count(&mut self) -> Result<(), String> {
        self.inner.clear_pixel_count()?;
        if let Some(at) = self.at() {
            self.replay_op(at, ReplayOp::BeginFlareQuery);
        }
        Ok(())
    }
    #[cfg(feature = "gpu")]
    fn external_attachments(
        &mut self, surfaces: ExternalSurfaces<Self::Surface>, textures: [Option<&Self::Texture>; 4], copy: TargetCopy,
    ) -> Result<ExternalCopies, String> {
        if self.replaying() { return Ok(ExternalCopies::Missing(copy)); }
        self.inner.external_attachments(surfaces, textures.map(|t| t.map(|t| &t.inner)), copy)
    }
    fn prepare_external(
        &mut self, surfaces: ExternalSurfaces<Self::Surface>, request: ExternalPassRequest, textures: [Option<&Self::Texture>; 4],
        plan: &ExternalPlan, copy: TargetCopy, pass: SharedReplacement,
    ) -> Result<Option<Vec<ExternalRecording>>, String> {
        if self.replaying() && self.carry.is_none() && !self.carrying {
            let pass = pass.lock().unwrap_or_else(|err| err.into_inner());
            ExternalOrigin::new(&**pass, "scene capture reservation", Some(plan.stage))
                .refused(unretained_cause(plan.stage), None);
            self.refused_reservation = true;
            return Ok(None);
        }
        self.inner.prepare_external(surfaces, request, textures.map(|t| t.map(|t| &t.inner)), plan, copy, pass)
    }
    fn take_scene_refusal(&mut self) -> bool {
        std::mem::take(&mut self.refused_reservation)
    }
    fn can_commit_carried(&self) -> bool {
        self.inner.can_commit_carried()
            && (self.replay.is_none() || (self.at().is_some() && !(self.guest_reference)()))
    }
    fn prepare_carried_stage(
        &mut self, stage: &SceneStage<Self::Surface>,
    ) -> Result<Option<Vec<ExternalRecording>>, String> {
        if !self.can_commit_carried() { return Ok(None); }
        {
            let pass = stage.pass.lock().map_err(|_| "poisoned carried stage")?;
            if !self.inner.accepts_scene_pass(&**pass) || (self.replay.is_some() && !pass.replays_scene()) {
                return Ok(None);
            }
        }
        if let Some(replay) = &self.replay {
            // All stages are prepared before the first is committed. Leave
            // space to name every live surface in the committed sequence.
            let s = &stage.surfaces;
            let known = [s.color, s.depth].into_iter().chain(s.sampled.iter().copied())
                .chain(s.slots.iter().flatten().copied())
                .all(|surface| self.surfaces.iter().any(|&(id, _)| id == surface));
            if !known || replay.frame.aux.len() + self.surfaces.len() > usize::from(u16::MAX) + 1 {
                return Ok(None);
            }
        }
        self.inner.prepare_carried_stage(stage)
    }
    fn commit_carried_stage(
        &mut self, stage: SceneStage<Self::Surface>, copies: Vec<ExternalRecording>,
    ) {
        let retained = self.replay_stage(&stage);
        self.inner.commit_carried_stage(stage, copies);
        if let Some((at, op)) = retained { self.replay_op(at, op); }
    }
    // A one-shot replacement cannot draw again into a private target.
    // Declining before commit leaves its original packet in this slot; the
    // draw's typed admission, when it has one, travels with that packet
    // ([`Backend::draw_admitted`]). A native producer whose carry has begun
    // ([`Backend::begin_native_carry`]) is queued: the device records its
    // carried packets in this slot once it is.
    fn queue_external_draw(&mut self, candidate: QueuedExternalDraw<Self::Surface>, textures: [Option<&Self::Texture>; 4]) -> bool {
        if self.replaying() && self.carry.is_none() { return false; }
        self.untracked_write();
        self.inner.queue_external_draw(candidate, textures.map(|t| t.map(|t| &t.inner)))
    }
    fn reusable_external_attachments(&self) -> bool {
        self.inner.reusable_external_attachments()
    }
    fn accepts_scene_pass(&self, pass: &dyn ExternalPass) -> bool {
        self.inner.accepts_scene_pass(pass) && (!self.replaying() || (!(self.guest_reference)() && pass.replays_scene()))
    }
    fn runs_scene_stages(&self) -> bool {
        self.inner.runs_scene_stages()
    }
    fn run_scene_stage(&mut self, stage: SceneStage<Self::Surface>) -> Result<bool, String> {
        let retained = self.replay_stage(&stage);
        let taken = self.inner.run_scene_stage(stage)?;
        if taken && let Some((at, op)) = retained {
            self.replay_op(at, op);
        }
        Ok(taken)
    }
    fn queues_scene_stages(&self) -> bool {
        self.inner.queues_scene_stages()
    }
    fn queue_scene_stage(&mut self, stage: SceneStage<Self::Surface>) -> bool {
        let retained = self.replay_stage(&stage);
        let queued = self.inner.queue_scene_stage(stage);
        if queued && let Some((at, op)) = retained {
            self.replay_op(at, op);
        }
        queued
    }
    fn begin_native_carry(
        &mut self, color: Self::Surface, depth: Option<Self::Surface>, admission: &crate::backend::RetainedAdmission<Self::Surface>,
    ) -> bool {
        // Only the producers [`carries_native_stage`] names: passes the
        // replay can record again from their packet and captured textures.
        self.carry = None;
        if !self.replaying() || !carries_native_stage(admission.plan.stage) {
            return false;
        }
        let packets = Arc::clone(&admission.packets);
        let again = crate::backend::RetainedAdmission {
            packets: Arc::clone(&packets),
            surfaces: admission.surfaces.clone(),
            request: admission.request,
            plan: admission.plan.clone(),
            pass: Arc::clone(&admission.pass),
        };
        self.carry = self.retain_admission(again, &packets, color, depth);
        self.carry.is_some()
    }
    fn record_native_carry(
        &mut self, color: Self::Surface, depth: Option<Self::Surface>, textures: [Option<&Self::Texture>; 4],
    ) {
        // One replay draw per packet, in the guest's order, each holding
        // the admission and its place in it. A batch's siblings were
        // resolved against its head's textures.
        let Some(external) = self.carry.take() else { return };
        for at in 0..external.packets.len() {
            let draw = Arc::clone(&external.packets[at]);
            self.record_draw(color, depth, &draw);
            let held = Box::new(RetainedExternal { at, ..(*external).clone() });
            self.replay_draw(color, depth, draw, textures, Some(held));
        }
        self.carrying = true;
    }
    fn end_native_carry(&mut self) {
        self.carry = None;
        self.carrying = false;
    }
    fn commit_external(&mut self, recording: ExternalRecording) {
        if !self.carrying {
            self.refuse_replay("unrecorded_external_pass");
        }
        self.inner.commit_external(recording)
    }
    #[cfg(feature = "gpu")]
    fn upload_scene_assets(&mut self, upload: crate::backend::AssetUpload) {
        self.inner.upload_scene_assets(upload);
    }

    #[cfg(feature = "gpu")]
    fn upload_device(&self) -> Option<(wgpu::Device, wgpu::Queue)> {
        self.inner.upload_device()
    }

    fn supports_external_pass(&self) -> bool {
        self.inner.supports_external_pass()
    }
    fn external_pass(
        &mut self, color: Self::Surface, depth: Self::Surface, sampled: Vec<Option<Self::Surface>>, request: ExternalPassRequest,
        pass: Box<dyn ExternalPass>,
    ) -> Result<bool, String> {
        if self.replaying() {
            ExternalOrigin::new(&*pass, "scene capture issue", None)
                .refused("the scene recorder cannot retain an untyped pass", None);
            return Ok(false);
        }
        self.untracked_write();
        self.inner.external_pass(color, depth, sampled, request, pass)
    }
    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        self.record_readback(Readback::PixelCount);
        let ticket = self.inner.report_pixel_count()?;
        if let Some(proxy) = ticket && let Some(at) = self.at() {
            self.replay_op(at, ReplayOp::EndFlareQuery(proxy));
        }
        Ok(ticket)
    }
    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        self.inner.read_pixel_count(ticket)
    }
    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        self.inner.try_pixel_count(ticket)
    }
    fn drop_pixel_count(&mut self, ticket: u64) {
        self.inner.drop_pixel_count(ticket)
    }
    fn pixel_count_unsubmitted(&self, ticket: u64) -> bool {
        self.inner.pixel_count_unsubmitted(ticket)
    }
    fn create_texture_target(&mut self, format: D3DFormat, width: u32, height: u32) -> Result<Self::Surface, String> {
        let s = self.inner.create_texture_target(format, width, height)?;
        self.made(s, AuxTarget { width, height, format: u32::from(format as u8), depth: false });
        Ok(s)
    }
    fn can_sample_surfaces(&self) -> bool {
        self.inner.can_sample_surfaces()
    }
    fn surface_texture(&mut self, surface: Self::Surface) -> Result<Option<Self::Texture>, String> {
        let r = self.inner.surface_texture(surface)?;
        Ok(r.map(|inner| self.sample_texture(inner, surface, TargetSample::InPlace, || ReplaySample::InPlace)))
    }
    fn can_sample_depth_surfaces(&self) -> bool {
        self.inner.can_sample_depth_surfaces()
    }
    fn depth_level_texture(&mut self, surface: Self::Surface) -> Result<Option<Self::Texture>, String> {
        let r = self.inner.depth_level_texture(surface)?;
        Ok(r.map(|inner| self.sample_texture(inner, surface, TargetSample::Depth, || ReplaySample::Depth)))
    }
    fn judge(&self) -> Option<Box<dyn DrawJudge>> {
        self.inner.judge()
    }
    fn handles_surface_aliases(&self) -> bool {
        self.inner.handles_surface_aliases()
    }
    fn defers_depth_aliases(&self) -> bool {
        self.inner.defers_depth_aliases()
    }
    fn depth_alias_texture(
        &mut self, depth: Self::Surface, format: D3DFormat, width: u32, height: u32, pitch: u32, generation: u64, offset: usize,
        desc: &TextureDesc, options: gfx::texture::DecodeOptions,
    ) -> Result<Option<Self::Texture>, String> {
        let r = self.inner.depth_alias_texture(depth, format, width, height, pitch, generation, offset, desc, options)?;
        Ok(r.map(|inner| self.sample_texture(inner, depth, TargetSample::Depth, || ReplaySample::Alias { format, width, height, pitch, generation, offset, desc: desc.clone(), options })))
    }
    fn can_mirror(&self) -> bool {
        self.inner.can_mirror()
    }
    fn mirror_reserve(&mut self, len: u32) -> Result<(), String> {
        self.inner.mirror_reserve(len)
    }
    fn mirror_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), String> {
        self.inner.mirror_write(offset, bytes)
    }
    fn mirror_store(
        &mut self, surface: Self::Surface, format: D3DFormat, width: u32, height: u32, layout: GuestLayoutKind, offset: u32,
    ) -> Result<(), String> {
        self.inner.mirror_store(surface, format, width, height, layout, offset)
    }
    fn mirror_load(
        &mut self, surface: Self::Surface, format: D3DFormat, width: u32, height: u32, layout: GuestLayoutKind, offset: u32,
    ) -> Result<(), String> {
        self.inner.mirror_load(surface, format, width, height, layout, offset)?;
        self.refuse_replay("mirror_load");
        Ok(())
    }
    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        self.record_readback(Readback::Mirror);
        self.inner.mirror_read(offset, len)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU8;

    use frame_types::{Slot, TargetRef};
    use gfx::Clear;

    use super::*;
    use crate::backend::{Recorded, RecordingBackend};

    /// Every `Backend` method must be forwarded; a missed provided method
    /// silently falls back to the trait's default.
    #[test]
    fn forwards_every_backend_method() {
        fn methods(src: &str, start: &str) -> std::collections::BTreeSet<String> {
            let body = &src[src.find(start).expect(start)..];
            let body = &body[..body.find("\n}\n").unwrap()];
            body.lines()
                .filter_map(|l| l.trim().strip_prefix("fn ").map(|r| r.split(['(', '<']).next().unwrap().to_string()))
                .collect()
        }
        let tr = methods(include_str!("backend.rs"), "pub trait Backend");
        let ours = methods(include_str!("scene_sink.rs"), "impl<B: Backend> Backend for SceneSink<B>");
        let missing: Vec<_> = tr.difference(&ours).collect();
        assert!(missing.is_empty(), "SceneSink does not forward {missing:?}");
    }

    /// A test's slot: 0xff is off, else `slot << 4 | view`.
    static SLOT: AtomicU8 = AtomicU8::new(0xff);

    fn source() -> Option<SlotAt> {
        let v = SLOT.load(Ordering::Relaxed);
        (v != 0xff).then(|| SlotAt { slot: Slot::ALL[usize::from(v >> 4)], view: v & 0xf })
    }

    fn set(slot: Option<Slot>, view: u8) {
        SLOT.store(slot.map_or(0xff, |s| ((s.index() as u8) << 4) | view), Ordering::Relaxed);
    }

    // The tests below share `SLOT`; one test drives every case in turn.
    #[test]
    fn the_sink_names_targets_slots_and_readbacks_and_passes_every_call_on() {
        let mut s = SceneSink::new(RecordingBackend::new(), source).keeping_draws();
        let fmt = D3DFormat::A8R8G8B8;
        let back = s.create_surface(fmt, 64, 64, false).unwrap();
        let depth = s.create_surface(fmt, 64, 64, true).unwrap();
        let side = s.create_texture_target(fmt, 16, 16).unwrap();
        s.inner_mut().sample_surfaces = true;

        // Off: nothing recorded, everything passed on.
        set(None, 0);
        s.clear(back, Some(depth), Clear::NONE).unwrap();
        s.present(back, 0).unwrap();
        assert!(s.last_frame().ops.is_empty());
        assert_eq!(s.inner().calls.iter().filter(|c| matches!(c, Recorded::Present { .. })).count(), 1);

        // On: the back buffer is presented, so it is the scene's colour.
        set(Some(Slot::Opaque), 2);
        s.clear(back, Some(depth), Clear::NONE).unwrap();
        set(Some(Slot::AfterLights), 2);
        s.clear(side, None, Clear::NONE).unwrap();
        s.surface_texture(side).unwrap();
        s.report_pixel_count().unwrap();
        set(Some(Slot::Overlay), 2);
        s.read_surface(side, fmt, 16, 16, GuestLayoutKind::Swizzled).unwrap();
        s.present(back, 0).unwrap();
        set(None, 0);
        let f = s.last_frame();
        let ops: Vec<(Slot, GuestOp<()>)> = f
            .ops
            .iter()
            .map(|o| (o.at.slot, match &o.op {
                GuestOp::Draw(_) => GuestOp::Draw(()),
                GuestOp::Bind { colour, depth } => GuestOp::Bind { colour: *colour, depth: *depth },
                GuestOp::Clear { colour, depth, rects } => GuestOp::Clear { colour: *colour, depth: *depth, rects: *rects },
                GuestOp::Copy { from, to } => GuestOp::Copy { from: *from, to: *to },
                GuestOp::Upload { to } => GuestOp::Upload { to: *to },
                GuestOp::Sample { of, how } => GuestOp::Sample { of: *of, how: *how },
            }))
            .collect();
        assert_eq!(ops, [
            (Slot::Opaque, GuestOp::Clear { colour: TargetRef::SceneColour, depth: TargetRef::SceneDepth, rects: false }),
            (Slot::AfterLights, GuestOp::Clear { colour: TargetRef::Aux(0), depth: TargetRef::None, rects: false }),
            (Slot::AfterLights, GuestOp::Sample { of: TargetRef::Aux(0), how: TargetSample::InPlace }),
        ]);
        assert!(f.ops.iter().all(|o| o.at.view == 2));
        assert_eq!(f.aux, [AuxTarget { width: 16, height: 16, format: u32::from(fmt as u8), depth: false }]);
        let summary = last().expect("a frame was recorded");
        assert_eq!(summary.readbacks[Readback::PixelCount.index()], 1);
        assert_eq!(summary.readbacks_outside[Readback::Surface.index()], 1);
        assert_eq!(summary.side_targets.len(), 1);
        // The recorder saw every call: three clears, two presents.
        let clears = s.inner().calls.iter().filter(|c| matches!(c, Recorded::Clear { .. })).count();
        assert_eq!(clears, 3);

        // A copy swap: the surface copied into the presented one is the
        // scene's colour too.
        set(Some(Slot::Transparents), 0);
        let drawn = s.create_surface(fmt, 64, 64, false).unwrap();
        s.copy_surface(drawn, back).unwrap();
        s.clear(drawn, None, Clear::NONE).unwrap();
        s.present(back, 0).unwrap();
        set(None, 0);
        let f = s.last_frame();
        assert!(matches!(f.ops[0].op, GuestOp::Copy { from: TargetRef::SceneColour, to: TargetRef::SceneColour }));
        assert!(matches!(f.ops[1].op, GuestOp::Clear { colour: TargetRef::SceneColour, .. }));
        assert!(f.aux.is_empty());

        // A destroyed surface is forgotten.
        s.destroy_surface(drawn).unwrap();
        s.destroy_surface(back).unwrap();
        set(Some(Slot::Opaque), 0);
        let again = s.create_surface(fmt, 32, 32, false).unwrap();
        s.clear(again, None, Clear::NONE).unwrap();
        s.present(again, 0).unwrap();
        set(None, 0);
        assert_eq!(s.last_frame().ops.len(), 1);
        assert!(matches!(s.last_frame().ops[0].op, GuestOp::Clear { colour: TargetRef::Aux(0), .. }));
        assert_eq!(s.last_frame().aux[0].width, 32);
    }

    #[test]
    fn the_open_frame_names_its_first_carried_slot_and_an_untracked_write_wins() {
        assert_eq!(decode_open(0), OpenFrame::Scene);
        let bit = |s: Slot| 1u32 << s.index();
        assert_eq!(decode_open(bit(Slot::Overlay)), OpenFrame::Carried(Slot::Overlay));
        assert_eq!(decode_open(bit(Slot::Overlay) | bit(Slot::AfterLights)), OpenFrame::Carried(Slot::AfterLights), "the frame's order");
        assert_eq!(decode_open(bit(Slot::Opaque) | UNTRACKED), OpenFrame::Untracked);
    }
}
