//! Commit a complete game frame to its physical targets exactly once. The
//! producer supplies the scene that was open at this Swap. The worker still
//! owns all proxy mappings and is the only thread that can choose a replay.
//! A resource operation or read that needs earlier draws flushes the retained
//! commands and the legacy renderer completes that frame.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

mod stage;

#[cfg(test)]
mod tests;

use frame_scene::TargetRef;
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

/// The private replay and window draw use the same scene. The latter must
/// render into the supplied targets and issue the present.
pub struct Candidate<B: Backend> {
    pub frame: Arc<ReplayFrame>,
    /// Check device-local target eligibility before the first original
    /// write is consumed. A refusal runs the retained commands normally.
    pub eligible: Box<dyn FnOnce(&B, &HashMap<TargetRef, B::Surface>) -> bool + Send>,
    pub draw: DrawFrame<B>,
}

pub struct FrameOwner<B: Backend> {
    /// Called on the guest thread at Swap, after the scene sink sealed it.
    pub select: Box<dyn FnMut(u64) -> Option<Candidate<B>> + Send>,
    /// Called on the worker when this present used the original commands.
    pub declined: fn(u64),
    /// Called if neither renderer successfully presented this frame.
    pub failed: fn(u64),
    /// Called after the last worker has joined.
    pub stopped: fn(),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnerStats {
    /// In-place native presentations.
    pub presented: u64,
    /// All other presentations on devices with an owner installed.
    pub declined: u64,
    /// Matched GET_REPORT tickets in game views of successfully adopted presents.
    pub flare_queries: u64,
}

static FLARE_QUERIES: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicU64 = AtomicU64::new(0);
static PRESENTED: AtomicU64 = AtomicU64::new(0);
static DECLINED: AtomicU64 = AtomicU64::new(0);

pub fn stats() -> Option<OwnerStats> {
    (ACTIVE.load(Ordering::Relaxed) > 0).then(|| OwnerStats {
        presented: PRESENTED.load(Ordering::Relaxed),
        flare_queries: FLARE_QUERIES.load(Ordering::Relaxed),
        declined: DECLINED.load(Ordering::Relaxed),
    })
}

pub(super) struct Present<B: Backend> {
    pub n: u64,
    pub surface: u32,
    pub at: Instant,
    pub flags: u32,
    pub gamma: Option<Box<[u8; 768]>>,
    pub candidate: Option<Candidate<B>>,
    pub declined: fn(u64),
    pub failed: fn(u64),
}

pub(super) enum RenderOp {
    Stage(Box<crate::SceneStage<u32>>),
    BeginQuery,
    EndQuery { proxy: u64 },
    Clear { color: u32, depth: Option<u32>, clear: Clear },
    ClearRects { color: u32, depth: Option<u32>, clear: RectClear },
    Copy { src: u32, dst: u32 },
    Upload { surface: u32, desc: TextureDesc, bytes: Vec<u8> },
}

impl RenderOp {
    pub fn issue<B: Backend>(self, w: &mut Worker<B>) -> Result<(), String> {
        match self {
            Self::Stage(stage) => stage::issue(*stage, w),
            Self::BeginQuery => w.inner.clear_pixel_count(),
            Self::EndQuery { proxy } => w.issue_report(proxy),
            Self::Clear { color, depth, clear } => {
                let (c, d) = (w.surface(color)?, w.depth(depth)?);
                w.inner.clear(c, d, clear)
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
            Self::Upload { surface, desc, bytes } => {
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
    refused: bool,
}

impl<B: Backend> Transaction<B> {
    fn new() -> Self {
        Self { commands: Vec::new(), refused: false }
    }

    pub fn hold(&self, cmd: &Cmd<B>) -> bool {
        !self.refused && matches!(cmd, Cmd::Draw(_) | Cmd::Render(_))
    }

    pub fn barrier(&mut self) -> Vec<Cmd<B>> {
        if self.commands.is_empty() {
            return Vec::new();
        }
        self.refused = true;
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

fn target_map<B: Backend>(
    frame: &ReplayFrame,
    commands: &[Cmd<B>],
    surface: u32,
) -> Option<HashMap<TargetRef, u32>> {
    if !frame.refusals.is_empty() || !frame.ops.iter().any(|(at, _)| at.slot.in_game_view()) {
        return None;
    }
    let mut targets = HashMap::new();
    bind(&mut targets, frame.presented, Some(surface))?;
    let mut bound = None;
    let mut original = commands.iter();
    for (_, op) in &frame.ops {
        if let ReplayOp::Bind { colour, depth } = op {
            bound = Some((*colour, *depth));
            continue;
        }
        match (op, original.next()?) {
            (ReplayOp::Stage(kept), Cmd::Render(RenderOp::Stage(queued))) => {
                stage::bind_targets(&mut targets, kept, queued)?;
            }
            (ReplayOp::BeginFlareQuery, Cmd::Render(RenderOp::BeginQuery)) => {}
            (ReplayOp::EndFlareQuery(expected), Cmd::Render(RenderOp::EndQuery { proxy })) if expected == proxy => {}
            (ReplayOp::Draw { draw, textures, .. }, Cmd::Draw(slot)) => {
                let packet = slot.as_ref().as_ref()?;
                if packet.expect != DrawOutcome::Drawn || &packet.draw != draw.as_ref() {
                    return None;
                }
                // Sampled targets and deferred texture handles carry queue
                // state beyond the present. Only immutable uploaded textures
                // and the backend's normal unbound stages are independent.
                if !textures.iter().zip(&packet.textures).all(|(t,id)| matches!(
                    (t, id), (None, None) | (Some(ReplayTexture::Upload(_)), Some(_))
                )) {
                    return None;
                }
                let (c, d) = bound?;
                bind(&mut targets, c, Some(packet.color))?;
                bind(&mut targets, d, packet.depth)?;
            }
            (ReplayOp::Clear { colour, depth, clear }, Cmd::Render(RenderOp::Clear { color, depth: d, clear: value }))
                if clear == value => {
                    bind(&mut targets, *colour, Some(*color))?;
                    bind(&mut targets, *depth, *d)?;
                }
            (ReplayOp::ClearRects { colour, depth, clear }, Cmd::Render(RenderOp::ClearRects { color, depth: d, clear: value }))
                if clear == value => {
                    bind(&mut targets, *colour, Some(*color))?;
                    bind(&mut targets, *depth, *d)?;
                }
            (ReplayOp::Copy { from, to }, Cmd::Render(RenderOp::Copy { src, dst })) => {
                bind(&mut targets, *from, Some(*src))?;
                bind(&mut targets, *to, Some(*dst))?;
            }
            (ReplayOp::Upload { to, desc, bytes }, Cmd::Render(RenderOp::Upload { surface, desc: d, bytes: b }))
                if desc == d && bytes.as_ref() == b.as_slice() => {
                    bind(&mut targets, *to, Some(*surface))?;
                }
            _ => return None,
        }
    }
    (original.next().is_none() && !commands.is_empty()).then_some(targets)
}

impl<B: Backend> Worker<B> {
    /// Both renderers spend the same guest proxy only when its backend
    /// ticket has been read, landed asynchronously, or explicitly dropped.
    fn adopt_outputs(&mut self, frame: &ReplayFrame, outputs: Outputs<B::Texture>) -> Result<(), String> {
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
        self.tickets.extend(outputs.tickets);
        let game_queries = expected.filter(|(at, _)| at.slot.in_game_view()).count() as u64;
        FLARE_QUERIES.fetch_add(game_queries, Ordering::Relaxed);
        Ok(())
    }

    pub(super) fn owner_present(&mut self, present: Present<B>) {
        let Present { n, surface, at, flags, gamma, candidate, declined, failed } = present;
        let transaction = self.transaction.replace(Transaction::new()).expect("owner installed");
        let accepted = candidate.and_then(|candidate| {
            let frame = &candidate.frame;
            if transaction.refused || frame.present != n || frame.flags != flags || frame.gamma != gamma {
                return None;
            }
            let proxies = target_map(frame, &transaction.commands, surface)?;
            let surfaces = proxies.into_iter().map(|(key, id)| {
                self.surfaces.get(&id).copied().map(|actual| (key, actual))
            }).collect::<Option<HashMap<_, _>>>()?;
            (candidate.eligible)(&self.inner, &surfaces).then_some((candidate.frame, candidate.draw, surfaces))
        });

        self.inner.stamp_present(at);
        if let Some((frame, draw, targets)) = accepted {
            let result = draw(&mut self.inner, &targets)
                .and_then(|outputs| self.adopt_outputs(&frame, outputs));
            match result {
                Ok(()) => { PRESENTED.fetch_add(1, Ordering::Relaxed); }
                // Encoding may have modified the borrowed targets. The
                // ordinary error latch stops the guest before its next draw.
                Err(err) => {
                    failed(n);
                    self.shared.latch(format!("frame owner: present {n}: {err}"));
                }
            }
        } else {
            for cmd in transaction.commands {
                self.exec_original(cmd);
            }
            match self.surface(surface).and_then(|surface| self.inner.present_with_gamma(surface, flags, gamma.as_deref())) {
                Ok(()) => {
                    DECLINED.fetch_add(1, Ordering::Relaxed);
                    declined(n);
                }
                Err(err) => {
                    failed(n);
                    self.shared.latch(err);
                }
            }
        }
        *self.shared.presented.lock().unwrap() = n;
        self.shared.presented_cv.notify_all();
        self.run(Self::land_pixel_counts);
    }
}
