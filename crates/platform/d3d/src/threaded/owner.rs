//! Commit a complete game frame to its physical targets exactly once. The
//! producer supplies the scene that was open at this Swap. The worker still
//! owns all proxy mappings and is the only thread that can choose a replay.
//! A resource operation or read that needs earlier draws flushes the retained
//! commands and the legacy renderer completes that frame.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

mod stage;

#[cfg(test)]
mod tests;

use frame_types::TargetRef;
use gfx::Clear;
use gfx::texture::TextureDesc;

use super::{Cmd, ThreadedBackend, Worker};
use crate::backend::{Backend, DrawOutcome, RectClear};
use crate::scene_sink::{FlareTickets, ReplayFrame, ReplayOp, ReplayTexture};

/// Resources the guest can refer to after the native frame has been submitted.
pub struct Outputs<T> {
    pub tickets: FlareTickets,
    pub samples: HashMap<u32, T>,
}

impl<T> Default for Outputs<T> {
    fn default() -> Self {
        Self { tickets: Vec::new(), samples: HashMap::new() }
    }
}

pub type DrawFrame<B> = Box<
    dyn FnOnce(&mut B, &HashMap<TargetRef, <B as Backend>::Surface>)
        -> Result<Outputs<<B as Backend>::Texture>, String> + Send,
>;

pub type FreezeInputs<B> = fn(
    &mut B, &ReplayFrame, &HashMap<TargetRef, <B as Backend>::Surface>,
) -> Result<(), String>;

/// The private replay and window draw use the same scene. The latter must
/// render into the supplied targets and issue the present.
pub struct Candidate<B: Backend> {
    pub frame: Arc<ReplayFrame>,
    /// Check device-local target eligibility before the first original
    /// write is consumed. A refusal runs the retained commands normally.
    pub eligible: Box<dyn FnOnce(&B, &HashMap<TargetRef, B::Surface>) -> bool + Send>,
    pub draw: DrawFrame<B>,
    /// A matched but ineligible frame may still be captured privately.
    pub before_original: Option<FreezeInputs<B>>,
}

/// The parity reference needs the same input attachments as the private
/// draw, then runs the untouched guest commands and ordinary Swap.
pub enum Submission<B: Backend> {
    Native(Candidate<B>),
    Reference {
        frame: Arc<ReplayFrame>,
        freeze: FreezeInputs<B>,
    },
}

/// The frame owner that installed the worker receives its completions.
/// A present retains its original timeline even across queued map changes.
pub trait OwnerEvents: Send + Sync {
    fn generation(&self) -> u64 { 0 }
    fn declined(&self, _generation: u64, _present: u64) {}
    fn failed(&self, _generation: u64, _present: u64) {}
    fn stopped(&self) {}
}

impl OwnerEvents for () {}

/// Display frames between owned presents (docs/graphics/framescene.md,
/// section 7 and slice 8). The render thread asks whenever nothing is
/// queued, so the guest's commands always go first. A frame drawn here is
/// host-only: it never touches the guest's attachments, tickets or
/// sampled textures.
pub trait DisplayRate<B: Backend>: Send {
    /// Draws a display frame if one is due at `now`. Returns when to ask
    /// again; `None` waits for the next command.
    fn idle(&mut self, backend: &mut B, now: Instant) -> Option<Instant>;
    /// A Swap went out without the graph: the scenes no longer follow on
    /// from what is on screen.
    fn declined(&mut self) {}
}

/// The frame graph's targets on the render thread
/// (`native_render::frame_graph::targets::Store`; docs/graphics/
/// retire-d3d-display.md, "The graph's targets"). Every render target and
/// depth surface the device makes by role comes from it, so the guest's
/// commands and the graph's replay draw on the same surface for a role,
/// whichever per-frame [`TargetRef`] names it.
pub trait TargetStore<B: Backend>: Send {
    /// The role's surface: the one held while its shape holds, else a new
    /// one (the old released).
    fn target(&mut self, backend: &mut B, target: crate::backend::TargetDesc) -> Result<B::Surface, String>;
    /// The device gave its handle for `role` back. The store keeps the
    /// surface for the role's next target.
    fn released(&mut self, backend: &mut B, role: frame_types::TargetRole, surface: B::Surface) -> Result<(), String>;
    /// The surface of each of `frame`'s targets, by its role.
    fn map(&self, frame: &ReplayFrame) -> Result<HashMap<TargetRef, B::Surface>, String>;
}

pub struct FrameOwner<B: Backend> {
    /// Guest Swap ordinal and the owner's generation at submission.
    pub select: Box<dyn FnMut(u64, u64) -> Option<Submission<B>> + Send>,
    pub events: Arc<dyn OwnerEvents>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerStats {
    /// In-place native presentations.
    pub presented: u64,
    /// All other presentations on devices with an owner installed.
    pub declined: u64,
    /// Paired game frames checked on a healthy display worker.
    pub candidates: u64,
    pub barriers: u64,
    pub mismatches: u64,
    pub target_refusals: u64,
    /// Matched GET_REPORT tickets in game views of successfully adopted presents.
    pub flare_queries: u64,
}

static FLARE_QUERIES: AtomicU64 = AtomicU64::new(0);
static CANDIDATES: AtomicU64 = AtomicU64::new(0);
static BARRIERS: AtomicU64 = AtomicU64::new(0);
static MISMATCHES: AtomicU64 = AtomicU64::new(0);
static TARGET_REFUSALS: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicU64 = AtomicU64::new(0);
static PRESENTED: AtomicU64 = AtomicU64::new(0);
static DECLINED: AtomicU64 = AtomicU64::new(0);

pub fn stats() -> Option<OwnerStats> {
    (ACTIVE.load(Ordering::Relaxed) > 0).then(|| OwnerStats {
        presented: PRESENTED.load(Ordering::Relaxed),
        flare_queries: FLARE_QUERIES.load(Ordering::Relaxed),
        candidates: CANDIDATES.load(Ordering::Relaxed),
        barriers: BARRIERS.load(Ordering::Relaxed),
        mismatches: MISMATCHES.load(Ordering::Relaxed),
        target_refusals: TARGET_REFUSALS.load(Ordering::Relaxed),
        declined: DECLINED.load(Ordering::Relaxed),
    })
}

pub(super) struct Present<B: Backend> {
    pub n: u64,
    pub surface: u32,
    pub flags: u32,
    pub gamma: Option<Box<[u8; 768]>>,
    pub candidate: Option<Submission<B>>,
    pub generation: u64,
    pub events: Arc<dyn OwnerEvents>,
    /// The present's camera metadata, restored on the worker.
    pub metadata: Option<crate::present_camera::Present>,
}

pub(super) enum RenderOp {
    Stage(Box<crate::SceneStage<u32>>),
    /// Match this as a stage if the graph owns the frame. On fallback
    /// submit its already verified recordings exactly once.
    SealedStage { stage: Box<crate::SceneStage<u32>>, copies: Vec<crate::ExternalRecording> },
    BeginQuery,
    EndQuery { proxy: u64 },
    Clear { color: u32, depth: Option<u32>, clear: Box<Clear> },
    ClearRects { color: u32, depth: Option<u32>, clear: Box<RectClear> },
    Copy { src: u32, dst: u32 },
    Upload { surface: u32, data: Box<(TextureDesc, Vec<u8>)> },
}

impl RenderOp {
    pub fn issue<B: Backend>(self, w: &mut Worker<B>) -> Result<(), String> {
        match self {
            Self::Stage(stage) => stage::issue(*stage, w),
            Self::SealedStage { copies, .. } => {
                for copy in copies { w.inner.commit_external(copy); }
                Ok(())
            }
            Self::BeginQuery => w.inner.clear_pixel_count(),
            Self::EndQuery { proxy } => w.issue_report(proxy),
            Self::Clear { color, depth, clear } => {
                let (c, d) = (w.surface(color)?, w.depth(depth)?);
                w.inner.clear(c, d, *clear)
            }
            Self::ClearRects { color, depth, clear } => {
                let (c, d) = (w.surface(color)?, w.depth(depth)?);
                let result = w.inner.clear_rects(c, d, &clear)?;
                if let Err(e) = super::expect_drawn("clear_rects", result) {
                    w.shared.miss(e);
                }
                Ok(())
            }
            Self::Copy { src, dst } => {
                let (src, dst) = (w.surface(src)?, w.surface(dst)?);
                if !w.inner.copy_surface(src, dst)? {
                    w.shared.miss("copy_surface returned false".into());
                }
                Ok(())
            }
            Self::Upload { surface, data } => {
                let (desc, bytes) = *data;
                let surface = w.surface(surface)?;
                if !w.inner.write_surface(surface, &desc, &bytes)? {
                    w.shared.miss("write_surface returned false".into());
                }
                Ok(())
            }
        }
    }
}

pub(super) struct Transaction<B: Backend> {
    pub commands: Vec<Cmd<B>>,
    pub(super) refused: bool,
}

impl<B: Backend> Transaction<B> {
    fn new() -> Self {
        Self { commands: Vec::new(), refused: false }
    }

    pub fn hold(&self, cmd: &Cmd<B>) -> bool {
        !self.refused && matches!(cmd, Cmd::Draw(_) | Cmd::Render(_) | Cmd::Sample(_) | Cmd::ReleaseTexture(_) | Cmd::FrameStats(_) | Cmd::ProbeRead(_))
    }

    pub fn barrier(&mut self) -> Vec<Cmd<B>> {
        if self.commands.is_empty() {
            return Vec::new();
        }
        self.refused |= self.commands.iter().any(|cmd| !matches!(cmd, Cmd::ProbeRead(_)));
        std::mem::take(&mut self.commands)
    }
}

impl<B: Backend + 'static> ThreadedBackend<B> {
    /// Permit the host to replace a complete game frame at Swap. Jobs with
    /// observable results (including mid-frame queries) still run in order.
    pub fn with_frame_owner(mut self, owner: FrameOwner<B>) -> Self {
        if self.frame_owner.is_none() {
            ACTIVE.fetch_add(1, Ordering::Relaxed);
            self.post(|w| {
                w.transaction = Some(Transaction::new());
                Ok(())
            });
        }
        self.frame_owner = Some(owner);
        self
    }

    /// Make the game's targets through the frame graph's store
    /// ([`TargetStore`]). Posted before any surface is made.
    pub fn with_target_store(self, store: Box<dyn TargetStore<B>>) -> Self {
        self.post(move |w| {
            w.targets = Some(store);
            Ok(())
        });
        self
    }

    /// Draw display frames between owned presents on the render thread
    /// ([`DisplayRate`]).
    pub fn with_display_rate(self, rate: Box<dyn DisplayRate<B>>) -> Self {
        self.post(move |w| {
            w.display_rate = Some(rate);
            Ok(())
        });
        self
    }
}

pub(super) fn uninstall() {
    ACTIVE.fetch_sub(1, Ordering::Relaxed);
}

/// Each frame-local name must identify one physical surface for its full
/// lifetime, and two different names must never refer to the same surface.
fn bind(map: &mut HashMap<TargetRef, u32>, target: TargetRef, id: Option<u32>) -> Option<()> {
    let Some(id) = id else { return (target == TargetRef::None).then_some(()) };
    if target == TargetRef::None {
        return None;
    }
    if let Some(existing) = map.get(&target) {
        return (*existing == id).then_some(());
    }
    if map.values().any(|&other| other == id) {
        return None;
    }
    map.insert(target, id);
    Some(())
}

struct Proxies {
    targets: HashMap<TargetRef, u32>,
    // The graph names samples locally. The device can keep the texture
    // after this frame, so each needs its exact original proxy handle.
    samples: HashMap<u32, u64>,
}

fn target_map<B: Backend>(
    frame: &ReplayFrame,
    commands: &[Cmd<B>],
    uploads: &HashMap<u64, std::sync::Weak<gfx::texture::DecodedTexture>>,
    live_sources: &HashMap<u64, u32>,
    surface: u32,
) -> Option<Proxies> {
    if !frame.refusals.is_empty() || !frame.has_game_view() {
        return None;
    }
    let mut targets = HashMap::new();
    bind(&mut targets, frame.presented, Some(surface))?;
    let mut bound = None;
    let mut samples = HashMap::new();
    // A released proxy has no further uses from the guest. On success
    // it is removed after recording, once the submitted GPU work owns
    // its references. The legacy branch executes releases in order.
    // Only a trailing stats marker can be joined to the work of this
    // whole graph. A split Swap can place one between two sets of draws.
    if let Some(i) = commands.iter().position(|cmd| matches!(cmd, Cmd::FrameStats(_)))
        && commands[i + 1..].iter().any(|cmd| !matches!(cmd, Cmd::ReleaseTexture(_) | Cmd::ProbeRead(_)))
    {
        return None;
    }
    // The worker checks all speculative draw outcomes. A skipped draw
    // omitted by the scene sink must still execute and verify its result.
    let mut original = commands.iter().filter(|cmd| !matches!(cmd, Cmd::FrameStats(_) | Cmd::ProbeRead(_)));
    let mut released = HashSet::new();
    for (_, op) in &frame.ops {
        if let ReplayOp::Bind { colour, depth } = op {
            bound = Some((*colour, *depth));
            continue;
        }
        // A marker has no command of its own.
        if op.is_marker() {
            continue;
        }
        let command = loop {
            match original.next()? {
                Cmd::ReleaseTexture(proxy) => { released.insert(*proxy); }
                cmd => break cmd,
            }
        };
        match (op, command) {
            (ReplayOp::Sample { of, id, how, into }, Cmd::Sample(sample)) if how == &sample.how => {
                bind(&mut targets, *of, Some(sample.surface))?;
                // A refill names the sample whose texture it reuses: the
                // same proxy, refilled in place on the device's side.
                if into.is_some_and(|prev| samples.get(&prev) != Some(&sample.proxy)) { return None; }
                if samples.insert(*id, sample.proxy).is_some() { return None; }
            }
            (ReplayOp::Stage(kept), Cmd::Render(
                RenderOp::Stage(queued) | RenderOp::SealedStage { stage: queued, .. }
            )) => {
                stage::bind_targets(&mut targets, kept, queued)?;
            }
            (ReplayOp::BeginFlareQuery, Cmd::Render(RenderOp::BeginQuery)) => {}
            (ReplayOp::EndFlareQuery(expected), Cmd::Render(RenderOp::EndQuery { proxy })) if expected == proxy => {}
            (ReplayOp::Draw { draw, textures, .. }, Cmd::Draw(slot)) => {
                let packet = slot.as_ref().as_ref()?;
                if packet.expect != DrawOutcome::Drawn || !std::ptr::eq(&*packet.draw, draw.as_ref()) {
                    return None;
                }
                if packet.textures.iter().flatten().any(|proxy| released.contains(proxy)) {
                    return None;
                }
                if !textures.iter().zip(&packet.textures).all(|(binding, &proxy)| match (binding, proxy) {
                    (None, None) => true,
                    (Some(ReplayTexture::Sample(id)), Some(proxy)) => samples.get(id) == Some(&proxy),
                    (Some(ReplayTexture::Frozen(sample)), Some(proxy)) => sample.proxy == Some(proxy),
                    (Some(ReplayTexture::Live { target, binding }), Some(proxy)) => {
                        binding.proxy == Some(proxy) && live_sources.get(&proxy).is_some_and(|&source| {
                            bind(&mut targets, *target, Some(source)).is_some()
                        })
                    }
                    (Some(ReplayTexture::Upload(asset)), Some(proxy)) => uploads
                        .get(&proxy).is_some_and(|source| source.as_ptr() == Arc::as_ptr(asset)),
                    _ => false,
                }) {
                    return None;
                }
                let (c, d) = bound?;
                bind(&mut targets, c, Some(packet.color))?;
                bind(&mut targets, d, packet.depth)?;
            }
            (ReplayOp::Clear { colour, depth, clear }, Cmd::Render(RenderOp::Clear { color, depth: d, clear: value }))
                if clear == value.as_ref() => {
                    bind(&mut targets, *colour, Some(*color))?;
                    bind(&mut targets, *depth, *d)?;
                }
            (ReplayOp::ClearRects { colour, depth, clear }, Cmd::Render(RenderOp::ClearRects { color, depth: d, clear: value }))
                if clear == value.as_ref() => {
                    bind(&mut targets, *colour, Some(*color))?;
                    bind(&mut targets, *depth, *d)?;
                }
            (ReplayOp::Copy { from, to }, Cmd::Render(RenderOp::Copy { src, dst })) => {
                bind(&mut targets, *from, Some(*src))?;
                bind(&mut targets, *to, Some(*dst))?;
            }
            (ReplayOp::Upload { to, desc, bytes }, Cmd::Render(RenderOp::Upload { surface, data }))
                if desc == &data.0 && bytes.as_ref() == data.1.as_slice() => {
                    bind(&mut targets, *to, Some(*surface))?;
                }
            _ => return None,
        }
    }
    original.all(|cmd| matches!(cmd, Cmd::ReleaseTexture(_))).then_some(Proxies { targets, samples })
}

impl<B: Backend> Worker<B> {
    /// Both renderers spend the same guest proxy only when its backend
    /// ticket has been read, landed asynchronously, or explicitly dropped.
    fn adopt_outputs(&mut self, frame: &ReplayFrame, samples: &HashMap<u32, u64>, outputs: Outputs<B::Texture>) -> Result<u64, String> {
        let expected = frame.ops.iter().filter_map(|(at, op)| match op {
            ReplayOp::EndFlareQuery(proxy) => Some((at, *proxy)),
            _ => None,
        });
        if !expected.clone().map(|(_, proxy)| proxy).eq(outputs.tickets.iter().map(|&(proxy, _)| proxy)) {
            for (_, ticket) in outputs.tickets {
                self.inner.drop_pixel_count(ticket);
            }
            return Err("native flare reports do not match the submitted queries".into());
        }
        if samples.len() != outputs.samples.len() || outputs.samples.keys().any(|k| !samples.contains_key(k)) {
            for (_, ticket) in outputs.tickets { self.inner.drop_pixel_count(ticket); }
            return Err("native sampled targets do not match the submitted textures".into());
        }
        // A pooled frame-buffer copy is one proxy sampled several times
        // (`Backend::snapshot_surface_into`): it keeps its last sample.
        let mut outputs_by_id: Vec<_> = outputs.samples.into_iter().collect();
        outputs_by_id.sort_unstable_by_key(|&(id, _)| id);
        for (id, handle) in outputs_by_id {
            self.textures.insert(samples[&id], handle);
        }
        self.tickets.extend(outputs.tickets);
        let game_queries = expected.filter(|(at, _)| at.slot.in_game_view()).count() as u64;
        FLARE_QUERIES.fetch_add(game_queries, Ordering::Relaxed);
        Ok(game_queries)
    }

    /// The display-rate loop's turn while nothing is queued. A refused
    /// transaction runs the guest's commands on the device as they come,
    /// so no display frame is drawn in the middle of one.
    pub(super) fn display_idle(&mut self) -> Option<Instant> {
        if self.transaction.as_ref().is_some_and(|t| t.refused) {
            return None;
        }
        let rate = self.display_rate.as_mut()?;
        rate.idle(&mut self.inner, Instant::now())
    }

    pub(super) fn owner_present(&mut self, present: Present<B>) {
        // The metadata is this thread's present around this call
        // (`Worker::exec`): the draw and a display-rate loop read the
        // guest's image from it (`present_camera::guest_image`) to place
        // the marks of the display frames they show.
        let Present { n, surface, flags, gamma, candidate, generation, events, metadata: _ } = present;
        let transaction = self.transaction.replace(Transaction::new()).expect("owner installed");
        let accepted = candidate.and_then(|submission| {
            if self.shared.failed.load(Ordering::Acquire) {
                return None;
            }
            let (frame, native) = match &submission {
                Submission::Native(candidate) => {
                    CANDIDATES.fetch_add(1, Ordering::Relaxed);
                    (&candidate.frame, true)
                }
                Submission::Reference { frame, .. } => (frame, false),
            };
            if transaction.refused {
                if native { BARRIERS.fetch_add(1, Ordering::Relaxed); }
                return None;
            }
            let matching_swap = frame.present == n && frame.flags == flags && frame.gamma == gamma;
            let Some(proxies) = matching_swap.then(|| target_map(frame, &transaction.commands, &self.scene_uploads, &self.live_sources, surface)).flatten() else {
                if native { MISMATCHES.fetch_add(1, Ordering::Relaxed); }
                return None;
            };
            // With the graph's store the replay draws on the graph's
            // targets, by role; the guest's commands drew on the same ones.
            let surfaces = match &self.targets {
                Some(store) => Some(store.map(frame).unwrap_or_else(|e| unimplemented!("graph targets: {e}"))),
                None => proxies.targets.into_iter().map(|(key, id)| {
                    self.surfaces.get(&id).copied().map(|actual| (key, actual))
                }).collect::<Option<HashMap<_, _>>>(),
            };
            match submission {
                Submission::Native(candidate) => {
                    let Some(surfaces) = surfaces else {
                        TARGET_REFUSALS.fetch_add(1, Ordering::Relaxed);
                        return None;
                    };
                    if !(candidate.eligible)(&self.inner, &surfaces) {
                        TARGET_REFUSALS.fetch_add(1, Ordering::Relaxed);
                        if let Some(freeze) = candidate.before_original {
                            let _ = freeze(&mut self.inner, &candidate.frame, &surfaces);
                        }
                        return None;
                    }
                    Some((candidate.frame, candidate.draw, surfaces, proxies.samples))
                }
                Submission::Reference { frame, freeze } => {
                    if let Some(targets) = surfaces {
                        // A failed input copy disqualifies the private draw.
                        // It never suppresses the original guest writes.
                        let _ = freeze(&mut self.inner, &frame, &targets);
                    }
                    None
                }
            }
        });

        use crate::scene_sink::window::DisplayResult;
        let mut outcome = DisplayResult::Failed;
        let raster = self.measure_present_raster(surface);
        if let Some((frame, draw, targets, samples)) = accepted {
            // The worker owns this scene and all of its original commands.
            // A locked producer can queue the next scene while this draws.
            self.shared.display_capacity.release(n);
            let result = self.residency.record(|| draw(&mut self.inner, &targets), Result::is_err)
                .and_then(|outputs| self.adopt_outputs(&frame, &samples, outputs));
            for cmd in transaction.commands {
                match cmd {
                    Cmd::ReleaseTexture(proxy) => self.release_texture(proxy),
                    Cmd::Sample(sample) if result.is_ok() && matches!(sample.how, crate::scene_sink::ReplaySample::InPlace) => {
                        self.live_sources.insert(sample.proxy, sample.surface);
                    }
                    Cmd::FrameStats(frame) => frame.finish(),
                    Cmd::Draw(slot) => self.recycle_draw(slot),
                    _ => {}
                }
            }
            match result {
                Ok(game_queries) => {
                    outcome = DisplayResult::Owned { game_queries };
                    PRESENTED.fetch_add(1, Ordering::Relaxed);
                }
                // Encoding may have modified the borrowed targets. The
                // ordinary error latch stops the guest before its next draw.
                Err(err) => {
                    events.failed(generation, n);
                    self.shared.latch(format!("frame owner: present {n}: {err}"));
                }
            }
        } else {
            if let Some(rate) = self.display_rate.as_mut() {
                rate.declined();
            }
            for cmd in transaction.commands {
                self.exec_original(cmd);
            }
            match self.surface(surface).and_then(|surface| self.inner.present_with_gamma(surface, flags, gamma.as_deref())) {
                Ok(()) if self.inner.last_present_unshown() || self.shared.failed.load(Ordering::Acquire) => events.failed(generation, n),
                Ok(()) => {
                    outcome = DisplayResult::Ordinary;
                    DECLINED.fetch_add(1, Ordering::Relaxed);
                    events.declined(generation, n);
                }
                Err(err) => {
                    events.failed(generation, n);
                    self.shared.latch(err);
                }
            }
            self.shared.display_capacity.release(n);
        }
        let residency = self.residency.present(!matches!(&outcome, DisplayResult::Failed));
        if let Some(window) = self.shared.window.get() {
            let extent = self.inner.take_presented_extent().filter(|_| !matches!(&outcome, DisplayResult::Failed));
            window.complete_at_scale(n, outcome, residency, crate::scene_stage::take_local_drops(), raster.finish(extent));
        }
        self.run(Self::land_pixel_counts);
    }
}
