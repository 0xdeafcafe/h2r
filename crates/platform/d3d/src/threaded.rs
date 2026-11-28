//! A render thread: a [`Backend`] that runs another backend's work on a
//! host thread of its own, the way the NV2A consumes the push buffer while
//! the CPU goes on (docs/graphics/render-thread.md).
//!
//! The device still decodes the push buffer, translates state and resolves
//! every draw on the guest's thread, reading guest memory there. What
//! crosses to the render thread is the backend call: a [`ResolvedDraw`]
//! holds no guest pointers, so shader preparation, encoding, uploads and
//! submission can run later without the guest's memory.
//!
//! Every call whose answer the guest can observe waits for the render
//! thread to reach it, so the guest sees exactly the values a synchronous
//! backend gives:
//!
//! - **Reads** (`read_surface`, `read_depth`, `read_pixel_count`) run on
//!   the render thread after everything queued before them, and the guest
//!   waits for the bytes. These were GPU round trips already. A pixel
//!   count already back is answered without the queue, and
//!   `try_pixel_count` never waits: it answers from the counts the render
//!   thread has landed, or "not back yet" (which the device turns into
//!   `D3DERR_TESTINCOMPLETE` only for a recent test; docs/graphics/d3d.md,
//!   "Visibility tests").
//! - **Handles** (`create_surface`, `upload_texture`, `snapshot_surface`,
//!   `surface_texture`, `report_pixel_count`) are proxies made on the
//!   guest's side at once; the render thread maps them to the inner
//!   backend's.
//! - **Draw outcomes** come from the inner backend's [`DrawJudge`]
//!   ([`Backend::judge`]), which decides on the guest's side, from the
//!   draw and whether each bound texture has a view, what the inner
//!   backend's `draw` will answer: an untranslatable draw is skipped with
//!   the same reason, as with a synchronous backend. Without a judge they
//!   are speculated as drawn.
//! - **Other outcomes** (`clear_rects`, `write_surface`, `copy_surface`,
//!   `snapshot_surface`, `snapshot_surface_into`, `surface_texture`,
//!   `depth_alias_texture`) are
//!   answered as the inner backend answers every valid call: drawn,
//!   `true`, `Some`.
//! - When the inner backend later gives another answer than the one
//!   returned, that is latched as an error, so the next call stops the
//!   guest instead of running on with device state that differs from a
//!   synchronous run. [`ThreadedBackend::misses`] counts them.
//! - **Errors** from the inner backend are latched the same way and
//!   returned by the next call (a failure is a stop either way; the stop
//!   comes at a later entry than with a synchronous backend).
//!
//! The render thread never touches guest memory or the logical clock, and
//! the guest never waits for it except at the reads above and the
//! frame-in-flight limit, which wait on wall time only. So logical time,
//! and everything the guest computes, is the same as with the synchronous
//! backend; only wall-clock overlap changes. The one exception is opt-out:
//! a polled pixel count ([`Backend::try_pixel_count`]) may not be back
//! yet, and a polling device answers that as `D3DERR_TESTINCOMPLETE`.
//!
//! Commands run in the order they were issued, on one queue, so the GPU
//! sees exactly the submissions the synchronous backend makes.
//!
//! The queue is batched: commands (texture drops included) collect in one
//! pending list and cross to the render thread together, at every call
//! that waits for it (before the wait), at each present (before the
//! frame-in-flight wait), every [`BATCH`] commands and when the backend is
//! dropped. A batch runs in order, so what the render thread does, and in
//! what order, is unchanged; it only starts later. Draws handed over with
//! [`Backend::draw_owned`] move to the render thread without a copy, and
//! their stream buffers come back for the device to refill.

mod capacity;
pub mod owner;
mod probe;

use crate::scene_sink::ReplaySample;

/// Synchronous light reservations submitted to an actual worker thread.
pub static LIGHT_WORKER_WAITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use gfx::texture::{DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

use crate::backend::{
    Backend, DisplaySurfacePixels, DrawJudge, DrawOutcome, ExternalAttachments, ExternalCommands,
    ExternalCopies, ExternalRecording, GuestLayoutKind, RectClear, RenderExtents, ResolvedDraw,
    StreamBytes, TargetCopy,
    prepare_external_recordings,
};

/// Presents the guest may run ahead of the render thread before `Swap`
/// waits (wall time only). Two, like D3D's default swap throttling.
pub const MAX_FRAMES_IN_FLIGHT: u64 = 2;

/// Commands queued before a batch goes to the render thread without a
/// call that waits (one channel send and wake per batch, not per command).
pub const BATCH: usize = 64;

/// Spent stream buffers kept for the device to refill: at most this many
/// draws' worth, and [`SPARE_BYTES`] of them.
const SPARE_DRAWS: usize = 4 * BATCH;
const SPARE_BYTES: usize = 16 << 20;

type Job<B> = Box<dyn FnOnce(&mut Worker<B>) + Send>;

/// One queued backend command.
enum Cmd<B: Backend> {
    Job(Job<B>),
    /// Work independent of this frame's retained draws: allocate a fresh
    /// resource or retrieve a visibility ticket from an earlier present.
    Independent(Job<B>),
    FrameStats(timeline::phase::WorkerFrame),
    ProbeRead(Box<probe::Read>),
    /// Resolves a shader resource at this exact point in the command list.
    /// The original callback includes the backend's depth-alias fallback.
    Sample(Box<SampleCmd<B>>),
    ReleaseTexture(u64),
    /// The hot path, without a boxed closure. The draw is freed on the
    /// render thread; its stream buffers go back to the guest's side
    /// ([`Shared::spent`]), and so does its emptied box
    /// ([`Shared::spent_boxes`]), for the next draw to fill instead of an
    /// allocation per draw.
    Draw(DrawBox),
    ExternalRun(Vec<ExternalRecording>),
    /// Adjacent queued native producer draws admitted as one pass
    /// ([`Backend::queue_external_draw`], [`QueuedRun`]).
    QueuedRun(Box<QueuedRun>),
    /// Only emitted on the frame-owner path.
    Render(owner::RenderOp),
    OwnerPresent(Box<owner::Present<B>>),
}

/// A queued draw's box: `Some` while queued, emptied by the render thread.
type DrawBox = Box<Option<DrawCmd>>;

struct SampleCmd<B: Backend> {
    surface: u32,
    proxy: u64,
    how: ReplaySample,
    original: Job<B>,
}

struct DrawCmd {
    color: u32,
    depth: Option<u32>,
    textures: [Option<u64>; 4],
    /// The outcome already answered.
    expect: DrawOutcome,
    draw: QueuedDraw,
}

enum QueuedDraw {
    Owned(ResolvedDraw),
    Shared(Arc<ResolvedDraw>),
}

impl QueuedDraw {
    fn into_owned(self) -> Option<ResolvedDraw> {
        match self {
            Self::Owned(draw) => Some(draw),
            Self::Shared(draw) => Arc::into_inner(draw),
        }
    }
}

impl std::ops::Deref for QueuedDraw {
    type Target = ResolvedDraw;

    fn deref(&self) -> &ResolvedDraw {
        match self {
            Self::Owned(draw) => draw,
            Self::Shared(draw) => draw.as_ref(),
        }
    }
}

/// Native producer draws queued as one admission: the first draw's
/// candidate, then each adjacent draw its pass took
/// ([`crate::ExternalPass::queued_run`], [`crate::ExternalPass::extend_run`])
/// on the same attachments, request, textures and stage. The render thread
/// prepares the one pass; if any copy does not record, it draws every
/// draw's fallback, in order, as N single admissions would have.
struct QueuedRun {
    head: crate::backend::QueuedExternalDraw<u32>,
    textures: [Option<u64>; 4],
    /// The joined draws after the head, in order: fallback and completion.
    rest: Vec<(Arc<crate::ResolvedDraw>, QueuedFinished)>,
    /// `head.pass` is the run's own copy, which `extend_run` may change.
    owned: bool,
}

type QueuedFinished = Box<dyn FnOnce(Option<&Result<DrawOutcome, String>>) + Send>;

/// Queued runs and the draws they hold, since start
/// ([`queued_runs`]).
static QUEUED_RUNS: AtomicU64 = AtomicU64::new(0);
static QUEUED_RUN_DRAWS: AtomicU64 = AtomicU64::new(0);

/// `(runs, draws)`: the queued native admissions the render thread was
/// handed and the draws in them. Draws minus runs is the admissions a run
/// saved (docs/graphics/native-particles.md, "One admission a run").
pub fn queued_runs() -> (u64, u64) {
    (QUEUED_RUNS.load(Ordering::Relaxed), QUEUED_RUN_DRAWS.load(Ordering::Relaxed))
}

impl QueuedRun {
    /// Takes `next` into this run when it follows on the same
    /// attachments and its pass can join; otherwise gives it back.
    fn join(&mut self, next: Box<QueuedRun>) -> Result<(), Box<QueuedRun>> {
        let (h, n) = (&self.head, &next.head);
        let plain = |s: &crate::backend::ExternalSurfaces<u32>| {
            s.sampled.is_empty() && s.slots.is_empty() && s.slot_sizes.is_empty()
        };
        if !next.rest.is_empty()
            || h.plan.stage != n.plan.stage || h.plan.view != n.plan.view
            || !h.plan.slots.is_empty() || !n.plan.slots.is_empty()
            || h.surfaces.color != n.surfaces.color || h.surfaces.depth != n.surfaces.depth
            || !plain(&h.surfaces) || !plain(&n.surfaces)
            || h.request != n.request || h.fallback_depth != n.fallback_depth
            || self.textures != next.textures
        {
            return Err(next);
        }
        let joined = {
            let (Ok(mut head), Ok(tail)) = (h.pass.lock(), n.pass.lock()) else { return Err(next) };
            if self.owned {
                head.extend_run(&**tail).then_some(None)
            } else {
                head.queued_run(&**tail).map(Some)
            }
        };
        let Some(fresh) = joined else { return Err(next) };
        if let Some(run) = fresh {
            // The device's pass (a replay may hold it) is never altered.
            self.head.pass = Arc::new(Mutex::new(run));
            self.owned = true;
        }
        let next = *next;
        self.head.plan.ranges += next.head.plan.ranges;
        self.rest.push((next.head.fallback, next.head.finished));
        Ok(())
    }
}

/// The pending batch, shared with the texture proxies so that their drops
/// keep their place in the order.
struct Queue<B: Backend> {
    pending: Mutex<PendingBatch<B>>,
    tx: mpsc::Sender<Vec<Cmd<B>>>,
}

struct PendingBatch<B: Backend> {
    commands: Vec<Cmd<B>>,
    /// Changes at every command outside the pinned external run.
    order: u64,
}

impl<B: Backend> Queue<B> {
    /// Queues `cmd`; hands the batch over once it is [`BATCH`] long.
    fn push(&self, cmd: Cmd<B>) -> Result<(), ()> {
        let mut p = self.pending.lock().unwrap();
        p.commands.push(cmd);
        p.order = p.order.wrapping_add(1);
        if p.commands.len() >= BATCH {
            return self.send(&mut p.commands);
        }
        Ok(())
    }
    fn push_external(&self, recording: ExternalRecording) -> Result<(), ()> {
        let mut pending = self.pending.lock().unwrap();
        if let Some(Cmd::ExternalRun(run)) = pending.commands.last_mut() {
            run.push(recording);
        } else {
            pending.commands.push(Cmd::ExternalRun(vec![recording]));
        }
        if pending.commands.len() >= BATCH { self.send(&mut pending.commands)?; }
        Ok(())
    }

    /// Queues a native producer draw, joined to the run that ends the
    /// pending batch when it can be ([`QueuedRun::join`]): nothing was
    /// queued between them, so the order is the same either way.
    fn push_queued(&self, run: Box<QueuedRun>) -> Result<(), ()> {
        let mut p = self.pending.lock().unwrap();
        p.order = p.order.wrapping_add(1);
        let run = match p.commands.last_mut() {
            Some(Cmd::QueuedRun(open)) => match open.join(run) {
                Ok(()) => return Ok(()),
                Err(run) => run,
            },
            _ => run,
        };
        p.commands.push(Cmd::QueuedRun(run));
        if p.commands.len() >= BATCH {
            return self.send(&mut p.commands);
        }
        Ok(())
    }

    fn order(&self) -> u64 {
        self.pending.lock().unwrap().order
    }

    /// Hands the pending batch to the render thread.
    fn flush(&self) -> Result<(), ()> {
        self.send(&mut self.pending.lock().unwrap().commands)
    }
    /// Sends under the lock, so batches leave in the order they filled.
    fn send(&self, p: &mut Vec<Cmd<B>>) -> Result<(), ()> {
        if p.is_empty() {
            return Ok(());
        }
        let batch = std::mem::replace(p, Vec::with_capacity(BATCH));
        self.tx.send(batch).map_err(|_| ())
    }
}

/// What lives on the render thread.
pub struct Worker<B: Backend> {
    inner: B,
    residency: crate::scene_stage::residency::Frame,
    transaction: Option<owner::Transaction<B>>,
    surfaces: HashMap<u32, B::Surface>,
    textures: HashMap<u64, B::Texture>,
    /// Source of a live immutable scene texture, indexed by its device proxy.
    scene_uploads: HashMap<u64, std::sync::Weak<DecodedTexture>>,
    /// Previous in-place colour samples keep a view into this original
    /// surface. Immutable snapshots never enter this table.
    live_sources: HashMap<u64, u32>,
    /// Pixel-count tickets not yet landed: proxy to the inner backend's,
    /// in report order.
    tickets: BTreeMap<u64, u64>,
    /// The depth buffer read back for depth-alias textures: surface,
    /// device depth generation, guest words.
    depth_words: Option<(u32, u64, Vec<u8>)>,
    shared: Arc<Shared>,
    exit: bool,
    /// [`ThreadedBackend::with_jitter`]: a xorshift state.
    jitter: Option<u64>,
    /// Stream buffers of the draws run in this batch, returned to
    /// [`Shared::spent`] at its end.
    spent: Vec<Vec<StreamBytes>>,
    /// Constant blocks of the draws run in this batch, kept for the
    /// device's next draws at its end ([`crate::shader::recycle_constants`]).
    spent_constants: Vec<crate::shader::Constants>,
    /// Emptied draw boxes of this batch, returned to
    /// [`Shared::spent_boxes`] at its end.
    spent_boxes: Vec<DrawBox>,
    /// Display frames between owned presents
    /// ([`ThreadedBackend::with_display_rate`]).
    display_rate: Option<Box<dyn owner::DisplayRate<B>>>,
    /// The frame graph's targets ([`ThreadedBackend::with_target_store`]):
    /// the game's render targets and depth surfaces, by role.
    targets: Option<Box<dyn owner::TargetStore<B>>>,
}

#[derive(Default)]
struct Shared {
    window: std::sync::OnceLock<Arc<crate::scene_sink::window::PresentWindow>>,
    /// Emptied draw boxes for the guest's side to fill (at most
    /// [`SPARE_DRAWS`]).
    spent_boxes: Mutex<Vec<DrawBox>>,
    /// The first error or wrong speculation, returned by the next call.
    error: Mutex<Option<String>>,
    /// Whether `error` is set (checked without the lock on every call).
    failed: std::sync::atomic::AtomicBool,
    /// Stream buffers of draws the render thread has run, for the device
    /// to refill, and their bytes.
    spent: Mutex<(Vec<Vec<StreamBytes>>, usize)>,
    misses: AtomicU64,
    display_capacity: capacity::DisplayCapacity,
    /// Wall time the render thread spent in the inner backend (ns).
    busy_ns: AtomicU64,
    /// Pixel counts already back from the GPU, by proxy ticket: the render
    /// thread asks the inner backend after each present and each batch
    /// ([`Backend::try_pixel_count`]), so a read of an older report answers
    /// without waiting for the queue.
    landed: Mutex<HashMap<u64, u64>>,
    /// A [`Backend::try_pixel_count`] miss has queued a batch the render
    /// thread has not run yet. An older report can land while the current
    /// frame is retained. A current report has to submit that frame's draws.
    land_kick: [std::sync::atomic::AtomicBool; 2],
    /// The inner backend's [`Backend::runs_scene_stages`], as the render
    /// thread last saw it (after each batch).
    scene_stages: std::sync::atomic::AtomicBool,
}

static READS: AtomicU64 = AtomicU64::new(0);
static READ_NS: AtomicU64 = AtomicU64::new(0);
static THROTTLES: AtomicU64 = AtomicU64::new(0);
static THROTTLE_NS: AtomicU64 = AtomicU64::new(0);
static BUSY_NS: AtomicU64 = AtomicU64::new(0);
static LANDED_COUNTS: AtomicU64 = AtomicU64::new(0);
static PENDING_COUNTS: AtomicU64 = AtomicU64::new(0);

/// [`Waits`] summed over every render thread in the process so far (for
/// a measurement window: take it twice and subtract).
pub fn total_waits() -> Waits {
    let d = |a: &AtomicU64| std::time::Duration::from_nanos(a.load(Ordering::Relaxed));
    Waits {
        reads: READS.load(Ordering::Relaxed),
        read_wait: d(&READ_NS),
        throttles: THROTTLES.load(Ordering::Relaxed),
        throttle_wait: d(&THROTTLE_NS),
        busy: d(&BUSY_NS),
        landed_counts: LANDED_COUNTS.load(Ordering::Relaxed),
        pending_counts: PENDING_COUNTS.load(Ordering::Relaxed),
    }
}

/// Where the guest's thread waited for the render thread.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Waits {
    /// Calls that waited for an answer (reads, `finish`, `with_inner`).
    pub reads: u64,
    /// Wall time those waited.
    pub read_wait: std::time::Duration,
    /// `Swap`s that waited for the frame-in-flight limit.
    pub throttles: u64,
    /// Wall time those waited.
    pub throttle_wait: std::time::Duration,
    /// Wall time the render thread spent running backend work.
    pub busy: std::time::Duration,
    /// Pixel-count reads and polls answered from a report already back,
    /// without a wait (every thread's, in [`total_waits`]).
    pub landed_counts: u64,
    /// Pixel-count polls ([`Backend::try_pixel_count`]) answered "not back
    /// yet" without a wait (every thread's, in [`total_waits`]).
    pub pending_counts: u64,
}

impl Shared {
    fn latch(&self, e: String) {
        let mut g = self.error.lock().unwrap();
        if g.is_none() {
            *g = Some(e);
            self.failed.store(true, Ordering::Release);
        }
    }
    fn miss(&self, e: String) {
        self.misses.fetch_add(1, Ordering::Relaxed);
        self.latch(format!("render thread: wrong speculation: {e}"));
    }
}

impl<B: Backend> Worker<B> {
    fn surface(&self, id: u32) -> Result<B::Surface, String> {
        self.surfaces
            .get(&id)
            .copied()
            .ok_or_else(|| format!("render thread: surface {id} is not live"))
    }
    fn measure_present_raster(&mut self, id: u32) -> crate::backend::RasterMeasurement {
        if self.shared.window.get().is_some() {
            let surface = self.surface(id).ok();
            self.inner.measure_present_raster(surface)
        } else {
            Default::default()
        }
    }

    fn depth(&self, id: Option<u32>) -> Result<Option<B::Surface>, String> {
        id.map(|d| self.surface(d)).transpose()
    }
    fn issue_report(&mut self, proxy: u64) -> Result<(), String> {
        match self.inner.report_pixel_count()? {
            Some(ticket) => { self.tickets.insert(proxy, ticket); }
            None => self.shared.miss("report_pixel_count returned None".into()),
        }
        Ok(())
    }

    /// Moves the pixel counts the GPU has written into
    /// [`Shared::landed`]. Same values as a read: only the wait goes.
    /// Reports finish in order, so it stops at the first not back.
    fn land_pixel_counts(&mut self) -> Result<(), String> {
        self.land_counts(true)
    }
    /// [`land_pixel_counts`](Self::land_pixel_counts); without `submit`, a
    /// report still in the backend's open encoder stops it instead of
    /// being submitted early (after each batch: it lands after the
    /// submission that takes it, at the latest the present).
    fn land_counts(&mut self, submit: bool) -> Result<(), String> {
        let mut landed = Vec::new();
        for (&proxy, &t) in &self.tickets {
            if !submit && self.inner.pixel_count_unsubmitted(t) {
                break;
            }
            match self.inner.try_pixel_count(t)? {
                Some(c) => landed.push((proxy, c)),
                None => break,
            }
        }
        if landed.is_empty() {
            return Ok(());
        }
        let mut l = self.shared.landed.lock().unwrap();
        for (proxy, c) in landed {
            self.tickets.remove(&proxy);
            l.insert(proxy, c);
        }
        Ok(())
    }
    /// Runs `f`, latching an error.
    fn run(&mut self, f: impl FnOnce(&mut Self) -> Result<(), String>) {
        if let Err(e) = f(self) {
            self.shared.latch(e);
        }
    }

    fn exec(&mut self, cmd: Cmd<B>) {
        if let Cmd::Independent(job) = cmd {
            job(self);
            return;
        }
        if let Cmd::OwnerPresent(present) = cmd {
            let metadata = present.metadata.clone();
            crate::present_camera::with(metadata, || self.owner_present(*present));
            return;
        }
        if let Some(pending) = self.transaction.as_mut() {
            if pending.hold(&cmd) {
                pending.commands.push(cmd);
                return;
            }
            // A read, query, resource lifetime change or external command
            // needs the original GPU state in queue order.
            let commands = pending.barrier();
            if !pending.refused && !commands.is_empty() {
                // Only probe reads preceded this barrier. Preserve the
                // bytes before it runs, and let the eventual presenter
                // decide whether the diagnostic receives those bytes.
                let mut saved = commands;
                for cmd in &mut saved {
                    if let Cmd::ProbeRead(read) = cmd {
                        read.preserve(self);
                    }
                }
                self.transaction.as_mut().expect("owner installed").commands = saved;
            } else {
                for saved in commands {
                    self.exec_original(saved);
                }
            }
        }
        self.exec_original(cmd);
    }

    fn release_texture(&mut self, proxy: u64) {
        self.textures.remove(&proxy);
        self.scene_uploads.remove(&proxy);
        self.live_sources.remove(&proxy);
    }

    fn exec_original(&mut self, cmd: Cmd<B>) {
        match cmd {
            Cmd::Render(op) => self.run(|worker| op.issue(worker)),
            Cmd::OwnerPresent(_) => unreachable!("a present terminates the owner transaction"),
            Cmd::Sample(sample) => (sample.original)(self),
            Cmd::Job(job) | Cmd::Independent(job) => job(self),
            Cmd::FrameStats(frame) => frame.finish(),
            Cmd::ProbeRead(read) => read.deliver(self),
            Cmd::ReleaseTexture(proxy) => self.release_texture(proxy),
            Cmd::ExternalRun(run) => {
                for recording in run { self.inner.commit_external(recording); }
            }
            Cmd::QueuedRun(run) => {
                let run = *run;
                self.run(|w| w.queued_run(run));
            }
            Cmd::Draw(slot) => {
                if let Err(e) = self.draw(slot.as_ref().as_ref().expect("a queued draw")) {
                    self.shared.latch(e);
                }
                self.recycle_draw(slot);
            }
        }
    }

    /// A queued run ([`QueuedRun`]): its one pass recorded on every copy,
    /// or each draw's fallback in order.
    fn queued_run(&mut self, run: QueuedRun) -> Result<(), String> {
        let QueuedRun { head, textures: ids, rest, .. } = run;
        QUEUED_RUNS.fetch_add(1, Ordering::Relaxed);
        QUEUED_RUN_DRAWS.fetch_add(1 + rest.len() as u64, Ordering::Relaxed);
        timeline::phase::count(timeline::phase::Counter::QueuedRuns, 1);
        timeline::phase::count(timeline::phase::Counter::QueuedRunDraws, 1 + rest.len() as u64);
        let color = self.surface(head.surfaces.color)?;
        let depth = self.depth(head.fallback_depth)?;
        let mut textures = [None; 4];
        for (slot, id) in textures.iter_mut().zip(ids) {
            if let Some(id) = id {
                *slot = Some(self.textures.get(&id).ok_or_else(|| format!("render thread: texture {id} is not live"))?);
            }
        }
        if let Some(surfaces) = head.surfaces.try_map(|s| self.surfaces.get(s).copied())
            && let Ok(Some(recordings)) = self.inner.prepare_external(
                surfaces, head.request, textures, &head.plan, crate::TargetCopy::Reference, head.pass,
            )
            && crate::device::complete_recordings(&head.plan, &recordings)
        {
            for recording in recordings { self.inner.commit_external(recording); }
            (head.finished)(None);
            for (_, finished) in rest { finished(None); }
            return Ok(());
        }
        crate::backend::note_declined(head.plan.stage, "threaded.rs", line!(), "the render thread did not record every copy");
        for (draw, finished) in std::iter::once((head.fallback, head.finished)).chain(rest) {
            let result = self.inner.draw(color, depth, &draw, textures);
            finished(Some(&result));
            let outcome = result?;
            self.residency.guest_draw(&outcome);
            if outcome != DrawOutcome::Drawn {
                self.shared.miss(format!("{}: answered Drawn, the backend gave {outcome:?}", draw.entry));
            }
        }
        Ok(())
    }

    fn recycle_draw(&mut self, mut slot: DrawBox) {
        let DrawCmd { draw, .. } = slot.take().expect("a queued draw");
        self.spent_boxes.push(slot);
        let Some(draw) = draw.into_owned() else { return };
        let mut streams = draw.streams;
        // Vertex cache snapshots are released here; only copies go back
        // to be refilled on the next frame.
        streams.retain(|s| s.data.cache_id().is_none());
        self.spent.push(streams);
        self.spent_constants.push(draw.shader.constants);
    }

    fn draw(&mut self, d: &DrawCmd) -> Result<(), String> {
        let (c, depth) = (self.surface(d.color)?, self.depth(d.depth)?);
        let mut tex: [Option<&B::Texture>; 4] = [None; 4];
        for (slot, id) in tex.iter_mut().zip(d.textures) {
            if let Some(id) = id {
                *slot = Some(
                    self.textures
                        .get(&id)
                        .ok_or_else(|| format!("render thread: texture {id} is not live"))?,
                );
            }
        }
        let r = self.inner.draw(c, depth, &d.draw, tex)?;
        self.residency.guest_draw(&r);
        if r != d.expect {
            self.shared.miss(format!(
                "{}: answered {:?}, the backend gave {r:?}",
                d.draw.entry, d.expect
            ));
        }
        Ok(())
    }

    /// Returns this batch's stream buffers to the guest's side (one lock
    /// per batch), within the pool's limits; the rest are freed here.
    fn return_spent(&mut self) {
        if !self.spent_constants.is_empty() {
            crate::shader::recycle_constants(&mut self.spent_constants);
        }
        if !self.spent_boxes.is_empty() {
            let mut g = self.shared.spent_boxes.lock().unwrap();
            let room = SPARE_DRAWS.saturating_sub(g.len());
            g.extend(self.spent_boxes.drain(..).take(room));
            self.spent_boxes.clear();
        }
        if self.spent.is_empty() {
            return;
        }
        let mut g = self.shared.spent.lock().unwrap();
        let (pool, bytes) = &mut *g;
        for streams in self.spent.drain(..) {
            let n: usize = streams.iter().map(|s| s.data.capacity()).sum();
            if pool.len() < SPARE_DRAWS && *bytes + n <= SPARE_BYTES {
                *bytes += n;
                pool.push(streams);
            }
        }
    }
}

/// A texture handle: the render thread's texture of the same id. Dropping
/// it frees that texture after everything queued before the drop.
pub struct ProxyTexture<B: Backend + 'static> {
    id: u64,
    /// Whether the inner backend's texture has a view the shaders sample
    /// (the judge's answer; true without one).
    view: bool,
    queue: Arc<Queue<B>>,
}

impl<B: Backend + 'static> Drop for ProxyTexture<B> {
    fn drop(&mut self) {
        let id = self.id;
        // After the render thread exits there is nothing left to free.
        let _ = self.queue.push(Cmd::ReleaseTexture(id));
    }
}

impl<B: Backend + 'static> std::fmt::Debug for ProxyTexture<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProxyTexture({})", self.id)
    }
}

/// Called first thing on each render thread a [`ThreadedBackend`] starts,
/// e.g. to register it with the sampling profiler.
pub static RENDER_THREAD_START: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

struct PinnedExternalRun {
    key: (u64, u32, u32, TargetCopy),
    order: u64,
    attachments: Arc<[ExternalAttachments]>,
}

/// [`Backend`] that queues `B`'s work to a render thread.
pub struct ThreadedBackend<B: Backend + 'static> {
    queue: Arc<Queue<B>>,
    frame_owner: Option<owner::FrameOwner<B>>,
    /// Stream buffers taken from [`Shared::spent`], handed back one draw's
    /// worth per draw.
    spare: Vec<Vec<StreamBytes>>,
    /// Draw boxes taken from [`Shared::spent_boxes`].
    spare_boxes: Vec<DrawBox>,
    join: Option<JoinHandle<()>>,
    shared: Arc<Shared>,
    bc: bool,
    sample_surfaces: bool,
    sample_depth: bool,
    surface_aliases: bool,
    external_pass: bool,
    upload_device: Option<(wgpu::Device, wgpu::Queue)>,
    reusable_external_attachments: bool,
    pinned_external_run: Option<PinnedExternalRun>,
    /// `inner.can_mirror()`.
    mirror: bool,
    next_surface: u32,
    next_texture: u64,
    next_ticket: u64,
    /// First visibility ticket that can depend on this present's commands.
    first_frame_ticket: u64,
    presents: u64,
    synchronous_presents: bool,
    waits: Waits,
    /// The inner backend's draw outcomes, answered here (`None`: they are
    /// speculated as drawn).
    judge: Option<Box<dyn DrawJudge>>,
    /// Each live surface's format and kind (for the judge's views).
    surface_formats: HashMap<u32, (D3DFormat, bool)>,
    /// Depth-alias decodes seen: description, signs, whether it has a view.
    alias_views: Vec<(TextureDesc, [bool; 4], bool)>,
}

impl<B: Backend + 'static> ThreadedBackend<B> {
    /// Installs the counter window shared with this device's guest sink.
    pub fn with_present_window(self, window: Option<Arc<crate::scene_sink::window::PresentWindow>>) -> Self {
        if let Some(window) = window { let _ = self.shared.window.set(window); }
        self
    }

    /// Offline capture must finish every swap before the replay clock advances.
    pub fn with_synchronous_presents(mut self, enabled: bool) -> Self {
        self.synchronous_presents = enabled;
        self
    }

    /// Moves `inner` to a new thread named `d3d-render`.
    pub fn new(inner: B) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel::<Vec<Cmd<B>>>();
        let shared = Arc::new(Shared::default());
        let bc = inner.bc_supported();
        let judge = inner.judge();
        let sample_surfaces = inner.can_sample_surfaces();
        let sample_depth = inner.can_sample_depth_surfaces();
        let surface_aliases = inner.handles_surface_aliases();
        let external_pass = inner.supports_external_pass();
        let upload_device = inner.upload_device();
        let reusable_external_attachments = inner.reusable_external_attachments();
        shared.scene_stages.store(inner.runs_scene_stages(), Ordering::Relaxed);
        let mirror = inner.can_mirror();
        let s = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("d3d-render".into())
            .spawn(move || {
                let _worker_lifetime = capacity::WorkerLifetime(Arc::clone(&s));
                if let Some(hook) = RENDER_THREAD_START.get() {
                    hook();
                }
                timeline::native::mark_render_thread();
                let mut w = Worker {
                    inner,
                    residency: Default::default(),
                    transaction: None,
                    surfaces: HashMap::new(),
                    textures: HashMap::new(),
                    scene_uploads: HashMap::new(),
                    live_sources: HashMap::new(),
                    tickets: BTreeMap::new(),
                    depth_words: None,
                    shared: s,
                    exit: false,
                    jitter: None,
                    spent: Vec::new(),
                    spent_constants: Vec::new(),
                    spent_boxes: Vec::new(),
                    display_rate: None,
                    targets: None,
                };
                // When the display-rate loop wants its next frame
                // (`owner::DisplayRate`: display frames between owned
                // presents). It runs only with nothing queued, so the
                // guest's commands always go first.
                let mut wake: Option<std::time::Instant> = None;
                'batches: loop {
                    let batch = match wake {
                        None => match rx.recv() {
                            Ok(b) => b,
                            Err(_) => break,
                        },
                        Some(at) => {
                            match rx.recv_timeout(
                                at.saturating_duration_since(std::time::Instant::now()),
                            ) {
                                Ok(b) => b,
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    let t = std::time::Instant::now();
                                    wake = w.display_idle();
                                    let ns = t.elapsed().as_nanos() as u64;
                                    w.shared.busy_ns.fetch_add(ns, Ordering::Relaxed);
                                    BUSY_NS.fetch_add(ns, Ordering::Relaxed);
                                    continue;
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                        }
                    };
                    let t = std::time::Instant::now();
                    // The jitter's delays are not backend work.
                    let mut slept = std::time::Duration::ZERO;
                    for cmd in batch {
                        if let Some(x) = w.jitter.as_mut() {
                            *x ^= *x << 13;
                            *x ^= *x >> 7;
                            *x ^= *x << 17;
                            let s = std::time::Instant::now();
                            std::thread::sleep(std::time::Duration::from_micros(*x % 300));
                            slept += s.elapsed();
                        }
                        w.exec(cmd);
                        if w.exit {
                            break 'batches;
                        }
                    }
                    // Counts back since the last present are there for
                    // the guest's next poll, not one present later.
                    if !w.tickets.is_empty() {
                        w.run(|w| w.land_counts(false));
                    }
                    let ns = t.elapsed().saturating_sub(slept).as_nanos() as u64;
                    w.shared.busy_ns.fetch_add(ns, Ordering::Relaxed);
                    BUSY_NS.fetch_add(ns, Ordering::Relaxed);
                    w.return_spent();
                    w.shared.scene_stages.store(w.inner.runs_scene_stages(), Ordering::Relaxed);
                    // Ask once nothing is queued: a batch already waiting
                    // is taken first (a zero timeout). Without a display
                    // frame due the answer is `None` and the loop blocks
                    // again.
                    wake = Some(std::time::Instant::now());
                }
            })?;
        Ok(Self {
            queue: Arc::new(Queue {
                pending: Mutex::new(PendingBatch { commands: Vec::with_capacity(BATCH), order: 0 }),
                tx,
            }),
            frame_owner: None,
            spare: Vec::new(),
            spare_boxes: Vec::new(),
            join: Some(join),
            shared,
            bc,
            sample_surfaces,
            sample_depth,
            surface_aliases,
            external_pass,
            upload_device,
            reusable_external_attachments,
            pinned_external_run: None,
            mirror,
            next_surface: 1,
            next_texture: 1,
            next_ticket: 1,
            first_frame_ticket: 1,
            presents: 0,
            synchronous_presents: false,
            waits: Waits::default(),
            judge,
            surface_formats: HashMap::new(),
            alias_views: Vec::new(),
        })
    }

    /// Wrong speculations so far (each also stops the guest).
    pub fn misses(&self) -> u64 {
        self.shared.misses.load(Ordering::Relaxed)
    }

    /// Where the guest waited so far, and the render thread's busy time.
    pub fn waits(&self) -> Waits {
        Waits {
            busy: std::time::Duration::from_nanos(self.shared.busy_ns.load(Ordering::Relaxed)),
            ..self.waits
        }
    }

    /// Delays the render thread by a pseudo-random 0-300 us before every
    /// command from here on (seeded), so tests can show that its timing
    /// changes nothing the guest sees.
    pub fn with_jitter(self, seed: u64) -> Self {
        let seed = seed | 1;
        self.post(move |w| {
            w.jitter = Some(seed);
            Ok(())
        });
        self
    }

    /// Waits until the render thread has run everything queued so far,
    /// and has landed the pixel counts back by then.
    pub fn finish(&mut self) -> Result<(), String> {
        self.call(|w| w.land_pixel_counts())
    }

    /// Runs `f` on the inner backend after everything queued, and waits for
    /// its answer (tests and reports: `GfxBackend::batch_stats`, say).
    pub fn with_inner<R: Send + 'static>(
        &mut self,
        f: impl FnOnce(&mut B) -> R + Send + 'static,
    ) -> Result<R, String> {
        self.call(move |w| Ok(f(&mut w.inner)))
    }

    /// [`with_inner`](Self::with_inner) with the inner backend's handle of
    /// `surface` (`GfxBackend::read_color` of the back buffer, say).
    pub fn with_surface<R: Send + 'static>(
        &mut self,
        surface: u32,
        f: impl FnOnce(&mut B, B::Surface) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        self.call(move |w| {
            let s = w.surface(surface)?;
            f(&mut w.inner, s)
        })
    }

    fn check(&self) -> Result<(), String> {
        if !self.shared.failed.load(Ordering::Acquire) {
            return Ok(());
        }
        match &*self.shared.error.lock().unwrap() {
            Some(e) => Err(e.clone()),
            None => Ok(()),
        }
    }

    fn post(&self, f: impl FnOnce(&mut Worker<B>) -> Result<(), String> + Send + 'static) {
        self.push(Cmd::Job(Box::new(move |w: &mut Worker<B>| w.run(f))));
    }

    fn post_create(&self, f: impl FnOnce(&mut Worker<B>) -> Result<(), String> + Send + 'static) {
        let create: Job<B> = Box::new(move |w| w.run(f));
        self.push(if self.frame_owner.is_some() { Cmd::Independent(create) } else { Cmd::Job(create) });
    }

    fn post_sample(
        &self, surface: u32, proxy: u64, how: Option<ReplaySample>,
        issue: impl FnOnce(&mut Worker<B>) -> Result<(), String> + Send + 'static,
    ) {
        let original: Job<B> = Box::new(move |w| w.run(issue));
        self.push(match how {
            Some(how) => Cmd::Sample(Box::new(SampleCmd { surface, proxy, how, original })),
            None => Cmd::Job(original),
        });
    }

    fn push(&self, cmd: Cmd<B>) {
        if self.queue.push(cmd).is_err() {
            self.shared.latch("render thread: gone".into());
        }
    }

    /// Hands everything queued to the render thread.
    fn flush(&self) {
        if self.queue.flush().is_err() {
            self.shared.latch("render thread: gone".into());
        }
    }

    /// A spent draw's stream buffers (empty if none has come back).
    fn spare_streams(&mut self) -> Vec<StreamBytes> {
        if self.spare.is_empty() {
            let mut g = self.shared.spent.lock().unwrap();
            std::mem::swap(&mut self.spare, &mut g.0);
            g.1 = 0;
        }
        self.spare.pop().unwrap_or_default()
    }

    /// `cmd` in a draw box the render thread has emptied, or a new one.
    fn queue_draw(
        &mut self,
        color: u32,
        depth: Option<u32>,
        draw: QueuedDraw,
        textures: [Option<&ProxyTexture<B>>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        if let Err(e) = self.check() {
            return (Err(e), draw.into_owned().map(|d| d.streams).unwrap_or_default());
        }
        // A skipped draw still goes to the render thread: the inner
        // backend submits a pass for another target before it skips.
        let judged = self.judge_draw(&draw, textures);
        let ids = textures.map(|t| t.map(|t| t.id));
        let slot = self.draw_box(DrawCmd {
            color,
            depth,
            textures: ids,
            expect: judged.clone(),
            draw,
        });
        self.push(Cmd::Draw(slot));
        (Ok(judged), self.spare_streams())
    }

    fn draw_box(&mut self, cmd: DrawCmd) -> DrawBox {
        if self.spare_boxes.is_empty() {
            std::mem::swap(
                &mut self.spare_boxes,
                &mut self.shared.spent_boxes.lock().unwrap(),
            );
        }
        match self.spare_boxes.pop() {
            Some(mut slot) => {
                *slot = Some(cmd);
                slot
            }
            None => Box::new(Some(cmd)),
        }
    }

    /// The outcome the inner backend will give `draw`, decided now from the
    /// draw and the textures' views; speculated as drawn without a judge.
    fn judge_draw(
        &mut self,
        draw: &ResolvedDraw,
        textures: [Option<&ProxyTexture<B>>; 4],
    ) -> DrawOutcome {
        match self.judge.as_mut() {
            Some(j) => j.draw(draw, textures.map(|t| t.map(|t| t.view))),
            None => DrawOutcome::Drawn,
        }
    }

    /// Classify before enqueueing: tickets older than the last Swap's fence
    /// depend only on backend submissions preceding this frame.
    fn previous_report(&self, ticket: u64) -> bool {
        self.frame_owner.is_some() && ticket < self.first_frame_ticket
    }

    /// Runs `f` after everything queued, and waits for its answer.
    fn call<R: Send + 'static>(
        &mut self,
        f: impl FnOnce(&mut Worker<B>) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        self.call_job(Cmd::Job, f)
    }

    fn call_job<R: Send + 'static>(
        &mut self,
        command: fn(Job<B>) -> Cmd<B>,
        f: impl FnOnce(&mut Worker<B>) -> Result<R, String> + Send + 'static,
    ) -> Result<R, String> {
        self.check()?;
        let _phase = timeline::phase::enter(timeline::phase::Phase::RenderWait);
        let t = std::time::Instant::now();
        let (rtx, rrx) = mpsc::sync_channel(1);
        let gone = |_| "render thread: gone".to_string();
        self.queue
            .push(command(Box::new(move |w: &mut Worker<B>| {
                let _ = rtx.send(f(w));
            })))
            .map_err(gone)?;
        self.queue.flush().map_err(gone)?;
        let r = rrx.recv().map_err(|_| "render thread: gone".to_string())?;
        self.waits.reads += 1;
        self.waits.read_wait += t.elapsed();
        READS.fetch_add(1, Ordering::Relaxed);
        READ_NS.fetch_add(t.elapsed().as_nanos() as u64, Ordering::Relaxed);
        // An error queued before this call comes first.
        self.check()?;
        r
    }

    fn pin_external_run(
        &mut self, run: u64, surfaces: crate::ExternalSurfaces<u32>, copy: TargetCopy,
        stage: timeline::native::Stage,
    ) -> Result<ExternalCopies<Arc<[ExternalAttachments]>>, String> {
        self.check()?;
        let key = (run, surfaces.color, surfaces.depth, copy);
        let order = self.queue.order();
        if let Some(pinned) = self.pinned_external_run.as_ref()
            && (pinned.key, pinned.order) == (key, order)
        {
            return Ok(ExternalCopies::Ready(Arc::clone(&pinned.attachments)));
        }
        self.pinned_external_run = None;
        count_external_wait(stage);
        let copies = self.external_attachments(surfaces, [None; 4], copy)?;
        Ok(match copies {
            ExternalCopies::Ready(attachments) => {
                let attachments: Arc<[_]> = attachments.into();
                self.pinned_external_run = Some(PinnedExternalRun {
                    key, order: self.queue.order(),
                    attachments: Arc::clone(&attachments),
                });
                ExternalCopies::Ready(attachments)
            }
            ExternalCopies::Missing(copy) => ExternalCopies::Missing(copy),
        })
    }

    fn new_surface(&mut self) -> u32 {
        let id = self.next_surface;
        self.next_surface += 1;
        id
    }

    fn new_texture(&mut self, view: bool) -> ProxyTexture<B> {
        let id = self.next_texture;
        self.next_texture += 1;
        ProxyTexture {
            id,
            view,
            queue: Arc::clone(&self.queue),
        }
    }

    /// The judge's view answer for a texture over surface `surface`.
    fn surface_view(&self, surface: u32) -> bool {
        match (&self.judge, self.surface_formats.get(&surface)) {
            (Some(j), Some(&(format, false))) => j.surface_view(format),
            (Some(_), _) => false,
            (None, _) => true,
        }
    }
}

impl<B: Backend + 'static> Drop for ThreadedBackend<B> {
    fn drop(&mut self) {
        if std::env::var_os("H2_RENDER_THREAD_STATS").is_some() {
            eprintln!("render thread: {:?}", self.waits());
        }
        let _ = self
            .queue
            .push(Cmd::Job(Box::new(|w: &mut Worker<B>| w.exit = true)));
        let _ = self.queue.flush();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        if let Some(owner) = self.frame_owner.take() {
            owner::uninstall();
            owner.events.stopped();
        }
    }
}

fn expect_drawn(what: &str, r: DrawOutcome) -> Result<(), String> {
    match r {
        DrawOutcome::Drawn => Ok(()),
        other => Err(format!("{what}: {other:?}")),
    }
}

fn count_external_wait(stage: timeline::native::Stage) {
    stage.count(timeline::native::Metric::WorkerRoundTrips, 1);
    if stage == timeline::native::Stage::Lights {
        LIGHT_WORKER_WAITS.fetch_add(1, Ordering::Relaxed);
    }
}

impl<B: Backend + 'static> Backend for ThreadedBackend<B> {
    type Surface = u32;
    type Texture = ProxyTexture<B>;

    fn retain_scene_sample(&self, texture: &Self::Texture) -> Option<crate::scene_sink::FrozenSample> {
        if !Arc::ptr_eq(&self.queue, &texture.queue) { return None; }
        let id = texture.id;
        let (sample, send) = crate::scene_sink::FrozenSample::pending(id);
        // The producing present precedes this job and any guest release
        // follows it. For an in-place view the result identifies its GPU
        // image; the seal reads the corresponding attachment at the draw.
        self.push(Cmd::Independent(Box::new(move |worker| {
            let images = worker.textures.get(&id).and_then(|t| worker.inner.frozen_images(t));
            let _ = send.send(images);
        })));
        // The sim may pause at the sealed-frame boundary before its next
        // Swap. Dispatch the job now without waiting for the worker.
        self.flush();
        Some(sample)
    }

    fn drain(&mut self) -> Result<(), String> {
        self.finish()
    }

    fn create_surface(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
        depth: bool,
    ) -> Result<u32, String> {
        self.check()?;
        let id = self.new_surface();
        self.surface_formats.insert(id, (format, depth));
        self.post_create(move |w| {
            let s = w.inner.create_surface(format, width, height, depth)?;
            w.surfaces.insert(id, s);
            Ok(())
        });
        Ok(id)
    }

    fn create_texture_target(
        &mut self,
        format: D3DFormat,
        width: u32,
        height: u32,
    ) -> Result<u32, String> {
        self.check()?;
        let id = self.new_surface();
        self.surface_formats.insert(id, (format, false));
        self.post_create(move |w| {
            let s = w.inner.create_texture_target(format, width, height)?;
            w.surfaces.insert(id, s);
            Ok(())
        });
        Ok(id)
    }

    fn create_target(&mut self, target: crate::backend::TargetDesc) -> Result<u32, String> {
        self.check()?;
        let id = self.new_surface();
        self.surface_formats.insert(id, (target.format, target.depth));
        self.post_create(move |w| {
            let s = match &mut w.targets {
                Some(store) => store.target(&mut w.inner, target)?,
                None => w.inner.create_target(target)?,
            };
            w.surfaces.insert(id, s);
            Ok(())
        });
        Ok(id)
    }

    fn release_target(&mut self, role: frame_types::TargetRole, surface: u32) -> Result<(), String> {
        self.check()?;
        self.surface_formats.remove(&surface);
        self.post(move |w| {
            let s = w.surface(surface)?;
            w.surfaces.remove(&surface);
            w.live_sources.retain(|_, source| *source != surface);
            match &mut w.targets {
                Some(store) => store.released(&mut w.inner, role, s),
                None => w.inner.release_target(role, s),
            }
        });
        Ok(())
    }

    fn destroy_surface(&mut self, surface: u32) -> Result<(), String> {
        self.check()?;
        self.surface_formats.remove(&surface);
        self.post(move |w| {
            let s = w.surface(surface)?;
            w.surfaces.remove(&surface);
            w.live_sources.retain(|_, source| *source != surface);
            w.inner.destroy_surface(s)
        });
        Ok(())
    }

    fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<ProxyTexture<B>, String> {
        self.check()?;
        let view = self.judge.as_ref().is_none_or(|j| j.upload_view(texture));
        let t = self.new_texture(view);
        let id = t.id;
        let texture = texture.clone();
        self.post_create(move |w| {
            let h = w.inner.upload_texture(&texture)?;
            w.textures.insert(id, h);
            Ok(())
        });
        Ok(t)
    }

    fn upload_shared(&mut self, texture: Arc<DecodedTexture>) -> Result<ProxyTexture<B>, String> {
        self.check()?;
        let view = self.judge.as_ref().is_none_or(|j| j.upload_view(&texture));
        let handle = self.new_texture(view);
        let id = handle.id;
        self.post_create(move |w| {
            let gpu_texture = w.inner.upload_texture(&texture)?;
            w.scene_uploads.insert(id, Arc::downgrade(&texture));
            w.textures.insert(id, gpu_texture);
            Ok(())
        });
        Ok(handle)
    }

    fn bc_supported(&self) -> bool {
        self.bc
    }

    fn clear(&mut self, color: u32, depth: Option<u32>, clear: Clear) -> Result<(), String> {
        self.check()?;
        if self.frame_owner.is_some() {
            self.push(Cmd::Render(owner::RenderOp::Clear { color, depth, clear: Box::new(clear) }));
            return Ok(());
        }
        self.post(move |w| {
            let (c, d) = (w.surface(color)?, w.depth(depth)?);
            w.inner.clear(c, d, clear)
        });
        Ok(())
    }

    fn draw(
        &mut self,
        color: u32,
        depth: Option<u32>,
        draw: &ResolvedDraw,
        textures: [Option<&ProxyTexture<B>>; 4],
    ) -> Result<DrawOutcome, String> {
        // A copy: the caller keeps `draw` (the device hands its draws
        // over with `draw_owned`).
        let (r, _) = self.draw_owned(color, depth, draw.clone(), textures);
        r
    }

    fn draw_owned(
        &mut self,
        color: u32,
        depth: Option<u32>,
        draw: ResolvedDraw,
        textures: [Option<&ProxyTexture<B>>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        self.queue_draw(color, depth, QueuedDraw::Owned(draw), textures)
    }

    fn draw_shared(
        &mut self,
        color: u32,
        depth: Option<u32>,
        draw: Arc<ResolvedDraw>,
        textures: [Option<&ProxyTexture<B>>; 4],
    ) -> (Result<DrawOutcome, String>, Vec<StreamBytes>) {
        self.queue_draw(color, depth, QueuedDraw::Shared(draw), textures)
    }

    fn frame_stats(&mut self) {
        if let Some(frame) = timeline::phase::forward_frame() {
            self.push(Cmd::FrameStats(frame));
        }
    }

    fn reset_present_metadata(&mut self) -> Result<(), String> {
        self.call(|w| w.inner.reset_present_metadata())
    }

    fn present(&mut self, surface: u32, flags: u32) -> Result<(), String> {
        self.present_with_gamma(surface, flags, None)
    }

    fn present_with_gamma(
        &mut self,
        surface: u32,
        flags: u32,
        gamma: Option<&[u8; 768]>,
    ) -> Result<(), String> {
        self.check()?;
        let gamma = gamma.map(|g| Box::new(*g));
        self.presents += 1;
        self.first_frame_ticket = self.next_ticket;
        let n = self.presents;
        let metadata = crate::present_camera::current();
        let owned = self.frame_owner.is_some();
        if owned {
            // At most two sealed transactions wait to be taken. Refused
            // transactions occupy their slot through the original Swap.
            self.wait_for_display_capacity(n)?;
        }
        if let Some(owner) = self.frame_owner.as_mut() {
            let events = Arc::clone(&owner.events);
            let generation = events.generation();
            let candidate = (owner.select)(n, generation);
            self.push(Cmd::OwnerPresent(Box::new(owner::Present {
                n, surface, flags, gamma, candidate, generation, events, metadata,
            })));
        } else {
            self.post(move |w| {
                let raster = w.measure_present_raster(surface);
                let r = w.surface(surface).and_then(|s| crate::present_camera::with(metadata, || {
                    w.inner.present_with_gamma(s, flags, gamma.as_deref())
                }));
                let complete = r.is_ok() && !w.inner.last_present_unshown() && !w.shared.failed.load(Ordering::Relaxed);
                let residency = w.residency.present(complete);
                if let Some(window) = w.shared.window.get() {
                    use crate::scene_sink::window::DisplayResult;
                    let outcome = if complete { DisplayResult::Ordinary } else { DisplayResult::Failed };
                    let extent = w.inner.take_presented_extent().filter(|_| complete);
                    window.complete_at_scale(n, outcome, residency, crate::scene_stage::take_local_drops(), raster.finish(extent));
                }
                w.shared.display_capacity.release(n);
                r?;
                w.land_pixel_counts()
            });
        }
        self.flush();
        if self.synchronous_presents {
            return self.call(|_| Ok(()));
        }
        if !owned {
            self.wait_for_display_capacity(n)?;
        }
        Ok(())
    }

    fn clear_rects(
        &mut self,
        color: u32,
        depth: Option<u32>,
        clear: &RectClear,
    ) -> Result<DrawOutcome, String> {
        self.check()?;
        let clear = clear.clone();
        if self.frame_owner.is_some() {
            self.push(Cmd::Render(owner::RenderOp::ClearRects { color, depth, clear: Box::new(clear) }));
            return Ok(DrawOutcome::Drawn);
        }
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            let (c, d) = (w.surface(color)?, w.depth(depth)?);
            let r = w.inner.clear_rects(c, d, &clear)?;
            if let Err(e) = expect_drawn("clear_rects", r) {
                shared.miss(e);
            }
            Ok(())
        });
        Ok(DrawOutcome::Drawn)
    }

    fn queue_frame_probe_read(
        &mut self, surface: u32, width: u32, height: u32,
        done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>,
    ) {
        if let Err(error) = self.check() {
            done(Err(error));
            return;
        }
        self.push(Cmd::ProbeRead(Box::new(probe::Read::new(surface, width, height, done))));
    }

    fn render_extents(
        &mut self,
        color: u32,
        depth: Option<u32>,
        textures: [Option<&ProxyTexture<B>>; 4],
    ) -> RenderExtents {
        let texture_ids = textures.map(|texture| texture.map(|t| t.id));
        self.call(move |worker| {
            let color = worker.surface(color)?;
            let depth = worker.depth(depth)?;
            let textures = texture_ids.map(|id| id.and_then(|id| worker.textures.get(&id)));
            Ok(worker.inner.render_extents(color, depth, textures))
        }).unwrap_or_default()
    }

    fn read_display_surface(&mut self, surface: u32) -> Result<Option<DisplaySurfacePixels>, String> {
        self.call(move |worker| {
            let surface = worker.surface(surface)?;
            worker.inner.read_display_surface(surface)
        })
    }

    fn read_surface(
        &mut self,
        surface: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        self.call(move |w| {
            let s = w.surface(surface)?;
            w.inner.read_surface(s, format, width, height, layout)
        })
    }

    fn read_depth(
        &mut self,
        surface: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        self.call(move |w| {
            let s = w.surface(surface)?;
            w.inner.read_depth(s, format, width, height, layout)
        })
    }

    fn copy_surface(&mut self, src: u32, dst: u32) -> Result<bool, String> {
        self.check()?;
        if self.frame_owner.is_some() {
            self.push(Cmd::Render(owner::RenderOp::Copy { src, dst }));
            return Ok(true);
        }
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            let (s, d) = (w.surface(src)?, w.surface(dst)?);
            if !w.inner.copy_surface(s, d)? {
                shared.miss("copy_surface returned false".into());
            }
            Ok(())
        });
        Ok(true)
    }

    fn write_surface(
        &mut self,
        surface: u32,
        desc: &TextureDesc,
        bytes: &[u8],
    ) -> Result<bool, String> {
        self.check()?;
        let desc = desc.clone();
        let bytes = bytes.to_vec();
        if self.frame_owner.is_some() {
            self.push(Cmd::Render(owner::RenderOp::Upload { surface, data: Box::new((desc, bytes)) }));
            return Ok(true);
        }
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            let s = w.surface(surface)?;
            if !w.inner.write_surface(s, &desc, &bytes)? {
                shared.miss("write_surface returned false".into());
            }
            Ok(())
        });
        Ok(true)
    }

    fn snapshot_surface(
        &mut self,
        surface: u32,
        opaque: bool,
    ) -> Result<Option<ProxyTexture<B>>, String> {
        self.check()?;
        let t = self.new_texture(self.surface_view(surface));
        let id = t.id;
        let shared = Arc::clone(&self.shared);
        let sample = self.frame_owner.as_ref().map(|_| ReplaySample::Snapshot { opaque });
        self.post_sample(surface, id, sample, move |w| {
            let s = w.surface(surface)?;
            match w.inner.snapshot_surface(s, opaque)? {
                Some(h) => {
                    w.textures.insert(id, h);
                }
                None => shared.miss("snapshot_surface returned None".into()),
            }
            Ok(())
        });
        Ok(Some(t))
    }

    /// Queued as a sample of the same proxy: the render thread refills the
    /// texture it holds for `into` in queue order, after the draws queued
    /// before that read it. Where the inner backend cannot refill it (or
    /// holds none, as after a frame owner's replay kept another), the
    /// proxy gets a new copy instead, so the answer is always `true`.
    fn snapshot_surface_into(
        &mut self,
        surface: u32,
        opaque: bool,
        into: &mut ProxyTexture<B>,
    ) -> Result<bool, String> {
        self.check()?;
        let id = into.id;
        let shared = Arc::clone(&self.shared);
        let sample = self.frame_owner.as_ref().map(|_| ReplaySample::Snapshot { opaque });
        self.post_sample(surface, id, sample, move |w| {
            let s = w.surface(surface)?;
            if let Some(t) = w.textures.get_mut(&id)
                && w.inner.snapshot_surface_into(s, opaque, t)?
            {
                return Ok(());
            }
            match w.inner.snapshot_surface(s, opaque)? {
                Some(h) => {
                    w.textures.insert(id, h);
                }
                None => shared.miss("snapshot_surface returned None".into()),
            }
            Ok(())
        });
        Ok(true)
    }

    fn clear_pixel_count(&mut self) -> Result<(), String> {
        self.check()?;
        if self.frame_owner.is_some() {
            self.push(Cmd::Render(owner::RenderOp::BeginQuery));
        } else {
            self.post(|w| w.inner.clear_pixel_count());
        }
        Ok(())
    }

    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        self.check()?;
        let proxy = self.next_ticket;
        self.next_ticket += 1;
        if self.frame_owner.is_some() {
            self.push(Cmd::Render(owner::RenderOp::EndQuery { proxy }));
        } else {
            self.post(move |w| w.issue_report(proxy));
        }
        Ok(Some(proxy))
    }

    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        // Already back: the count the inner backend's read gives, with no
        // wait for the work queued since.
        self.check()?;
        if let Some(c) = self.shared.landed.lock().unwrap().remove(&ticket) {
            LANDED_COUNTS.fetch_add(1, Ordering::Relaxed);
            self.waits.landed_counts += 1;
            return Ok(c);
        }
        let command: fn(Job<B>) -> Cmd<B> = if self.previous_report(ticket) { Cmd::Independent } else { Cmd::Job };
        self.call_job(command, move |w| {
            // It may have landed after the check above.
            if let Some(c) = w.shared.landed.lock().unwrap().remove(&ticket) {
                return Ok(c);
            }
            let t = w
                .tickets
                .remove(&ticket)
                .ok_or_else(|| format!("pixel-count ticket {ticket} is not live"))?;
            w.inner.read_pixel_count(t)
        })
    }

    /// From [`Shared::landed`], without waiting for the queue. A count not
    /// there yet answers `None` at once and kicks the render thread: the
    /// pending batch goes over, and the counts back at its end land.
    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        self.check()?;
        if let Some(c) = self.shared.landed.lock().unwrap().remove(&ticket) {
            LANDED_COUNTS.fetch_add(1, Ordering::Relaxed);
            self.waits.landed_counts += 1;
            return Ok(Some(c));
        }
        PENDING_COUNTS.fetch_add(1, Ordering::Relaxed);
        self.waits.pending_counts += 1;
        let previous = self.previous_report(ticket);
        let index = usize::from(previous);
        if !self.shared.land_kick[index].swap(true, Ordering::AcqRel) {
            let kick: Job<B> = Box::new(move |w| {
                w.shared.land_kick[index].store(false, Ordering::Release);
                w.run(Worker::land_pixel_counts);
            });
            self.push(if previous { Cmd::Independent(kick) } else { Cmd::Job(kick) });
        }
        self.flush();
        Ok(None)
    }

    fn drop_pixel_count(&mut self, ticket: u64) {
        self.shared.landed.lock().unwrap().remove(&ticket);
        let drop_ticket: Job<B> = Box::new(move |w| {
            w.shared.landed.lock().unwrap().remove(&ticket);
            if let Some(t) = w.tickets.remove(&ticket) {
                w.inner.drop_pixel_count(t);
            }
        });
        self.push(if self.previous_report(ticket) { Cmd::Independent(drop_ticket) } else { Cmd::Job(drop_ticket) });
    }

    fn can_sample_surfaces(&self) -> bool {
        self.sample_surfaces
    }

    fn can_mirror(&self) -> bool {
        self.mirror
    }

    fn mirror_reserve(&mut self, len: u32) -> Result<(), String> {
        self.check()?;
        self.post(move |w| w.inner.mirror_reserve(len));
        Ok(())
    }

    fn mirror_write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), String> {
        self.check()?;
        let bytes = bytes.to_vec();
        self.post(move |w| w.inner.mirror_write(offset, &bytes));
        Ok(())
    }

    fn mirror_store(
        &mut self,
        surface: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        self.check()?;
        self.post(move |w| {
            let s = w.surface(surface)?;
            w.inner
                .mirror_store(s, format, width, height, layout, offset)
        });
        Ok(())
    }

    fn mirror_load(
        &mut self,
        surface: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
        offset: u32,
    ) -> Result<(), String> {
        self.check()?;
        self.post(move |w| {
            let s = w.surface(surface)?;
            w.inner
                .mirror_load(s, format, width, height, layout, offset)
        });
        Ok(())
    }

    fn mirror_read(&mut self, offset: u32, len: u32) -> Result<Vec<u8>, String> {
        self.call(move |w| w.inner.mirror_read(offset, len))
    }

    fn can_sample_depth_surfaces(&self) -> bool {
        self.sample_depth
    }

    fn external_attachments(
        &mut self, surfaces: crate::ExternalSurfaces<Self::Surface>, textures: [Option<&Self::Texture>; 4],
        copy: crate::TargetCopy,
    ) -> Result<crate::ExternalCopies, String> {
        let ids = textures.map(|t| t.map(|t| t.id));
        self.call(move |w| {
            let Some(surfaces) = surfaces.try_map(|s| w.surfaces.get(s).copied()) else { return Ok(crate::ExternalCopies::Missing(copy)) };
            let textures = ids.map(|id| id.and_then(|i| w.textures.get(&i)));
            if (0..4).any(|i| ids[i].is_some() && textures[i].is_none()) { return Ok(crate::ExternalCopies::Missing(copy)); }
            w.inner.external_attachments(surfaces, textures, copy)
        })
    }

    fn prepare_external(
        &mut self, surfaces: crate::ExternalSurfaces<Self::Surface>, request: crate::ExternalPassRequest,
        textures: [Option<&Self::Texture>; 4], plan: &crate::ExternalPlan, copy: crate::TargetCopy, pass: crate::SharedReplacement,
    ) -> Result<Option<Vec<crate::ExternalRecording>>, String> {
        if matches!(plan.stage, timeline::native::Stage::World | timeline::native::Stage::Objects | timeline::native::Stage::Lights | timeline::native::Stage::Sky)
            && pass.lock().map_err(|_| "native pass poisoned")?.resolved_fallback().is_some()
        {
            // This is a resolved admission the queue declined. Keep its
            // ordinary D3D path instead of waiting for a worker decision.
            return Ok(None);
        }
        if self.reusable_external_attachments && textures.iter().all(Option::is_none)
            && surfaces.sampled.is_empty() && surfaces.slots.is_empty() && surfaces.slot_sizes.is_empty()
        {
            let batch = pass.lock().map_err(|_| "native pass poisoned")?.batch_key();
            if let Some(run) = batch {
                return prepare_external_recordings(request, plan, copy, pass, true, || {
                    self.pin_external_run(run, surfaces, copy, plan.stage)
                });
            }
        }
        self.pinned_external_run = None;
        count_external_wait(plan.stage);
        let plan = plan.clone();
        let ids = textures.map(|t| t.map(|t| t.id));
        self.call(move |w| {
            let Some(surfaces) = surfaces.try_map(|s| w.surfaces.get(s).copied()) else { return Ok(None) };
            let textures = ids.map(|id| id.and_then(|i| w.textures.get(&i)));
            if (0..4).any(|i| ids[i].is_some() && textures[i].is_none()) { return Ok(None); }
            w.inner.prepare_external(surfaces, request, textures, &plan, copy, pass)
        })
    }

    fn can_commit_carried(&self) -> bool {
        self.queues_scene_stages()
    }

    fn prepare_carried_stage(
        &mut self, stage: &crate::SceneStage<u32>,
    ) -> Result<Option<Vec<ExternalRecording>>, String> {
        if !self.can_commit_carried() { return Ok(None); }
        if self.frame_owner.is_none() {
            return self.prepare_external(stage.surfaces.clone(), stage.request, [None; 4],
                &stage.plan, TargetCopy::Reference, Arc::clone(&stage.pass));
        }

        // These lights compile draws from owned geometry and static textures.
        // Pinning attachments records handles and commands, without consuming
        // the results of earlier draws. A general worker query would flush the
        // held commands and disqualify the very frame the light belongs to.
        // Post reads the frame it is sealed into; its surfaces map on the worker.
        let lights = stage.plan.stage == timeline::native::Stage::Lights;
        if !(lights || stage.plan.stage == timeline::native::Stage::Post)
            || (lights && (!stage.surfaces.sampled.is_empty() || !stage.surfaces.slots.is_empty()))
            || !stage.pass.lock().map_err(|_| "native pass poisoned")?.replays_scene()
        {
            return Ok(None);
        }
        let surfaces = stage.surfaces.clone();
        let plan = stage.plan.clone();
        let request = stage.request;
        let pass = Arc::clone(&stage.pass);
        count_external_wait(plan.stage);
        self.call_job(Cmd::Independent, move |worker| {
            if !worker.inner.runs_scene_stages() { return Ok(None); }
            let Some(surfaces) = surfaces.try_map(|s| worker.surfaces.get(s).copied()) else {
                return Ok(None);
            };
            worker.inner.prepare_carried_stage(&crate::SceneStage { surfaces, request, plan, pass })
        })
    }

    fn commit_carried_stage(&mut self, stage: crate::SceneStage<u32>, copies: Vec<ExternalRecording>) {
        self.pinned_external_run = None;
        self.push(Cmd::Render(owner::RenderOp::SealedStage { stage: Box::new(stage), copies }));
    }

    fn queue_external_draw(
        &mut self, candidate: crate::backend::QueuedExternalDraw<u32>,
        textures: [Option<&Self::Texture>; 4],
    ) -> bool {
        if !self.external_pass
            || self.check().is_err() || self.judge.is_none()
            || self.judge_draw(&candidate.fallback, textures) != DrawOutcome::Drawn
        {
            return false;
        }
        let textures = textures.map(|t| t.map(|t| t.id));
        let run = Box::new(QueuedRun { head: candidate, textures, rest: Vec::new(), owned: false });
        if self.queue.push_queued(run).is_err() {
            self.shared.latch("render thread: gone".into());
        }
        true
    }

    /// Taken while the inner backend runs scene stages (as the render
    /// thread last saw it) and no error is latched.
    fn queues_scene_stages(&self) -> bool {
        self.external_pass && self.check().is_ok() && self.shared.scene_stages.load(Ordering::Relaxed)
    }

    /// One command in the queue: the render thread maps the attachments,
    /// prepares and records the stage when it reaches it, in order with
    /// the draws around it. The game thread does none of that work and
    /// does not wait for it. A stage the render thread cannot record is
    /// dropped and counted ([`crate::scene_stage::dropped`]).
    fn queue_scene_stage(&mut self, stage: crate::SceneStage<u32>) -> bool {
        if !self.queues_scene_stages() {
            return false;
        }
        let kind = stage.plan.stage;
        crate::scene_stage::queued(kind);
        // A stage changes what a pinned run's attachments hold.
        self.pinned_external_run = None;
        self.push(Cmd::Render(owner::RenderOp::Stage(Box::new(stage))));
        true
    }

    fn commit_external(&mut self, recording: crate::ExternalRecording) {
        // Appending packets, bundles or encoded commands to the worker's
        // pass or encoder changes no surface binding, so it leaves a pinned run's attachments valid:
        // the next reservation of the run (an adjacent object run) skips
        // the worker round trip. A standalone command buffer stays an
        // ordinary command.
        if matches!(&recording.commands,
            ExternalCommands::Bundles { .. } | ExternalCommands::Draws { .. } | ExternalCommands::Encoded(_))
        {
            if self.queue.push_external(recording).is_err() {
                self.shared.latch("render thread: gone".into());
            }
            return;
        }
        self.post(move |w| {
            w.inner.commit_external(recording);
            Ok(())
        });
    }

    #[cfg(feature = "gpu")]
    fn upload_scene_assets(&mut self, upload: crate::backend::AssetUpload) {
        self.post_create(move |w| {
            w.inner.upload_scene_assets(upload);
            Ok(())
        });
        // Asset prefetch can start before the first guest frame fills a batch.
        self.flush();
    }

    fn upload_device(&self) -> Option<(wgpu::Device, wgpu::Queue)> {
        self.upload_device.clone()
    }

    fn supports_external_pass(&self) -> bool {
        self.external_pass
    }

    fn external_pass(
        &mut self,
        color: u32,
        depth: u32,
        sampled: Vec<Option<u32>>,
        request: crate::backend::ExternalPassRequest,
        pass: Box<dyn crate::backend::ExternalPass>,
    ) -> Result<bool, String> {
        self.check()?;
        if !self.external_pass {
            return Ok(false);
        }
        let confirm = pass.confirm_issue();
        let producer = pass.diagnostic_name();
        let issue = move |w: &mut Worker<B>| {
            let (c, d) = (w.surface(color)?, w.surface(depth)?);
            let sampled = sampled.into_iter().map(|s| s.and_then(|s| w.surface(s).ok())).collect();
            w.inner.external_pass(c, d, sampled, request, pass)
        };
        if confirm {
            return self.call(issue);
        }
        let shared = Arc::clone(&self.shared);
        self.post(move |w| {
            if !issue(w)? {
                shared.miss(format!("{producer}: the worker declined an untyped external pass"));
            }
            Ok(())
        });
        Ok(true)
    }

    fn depth_level_texture(&mut self, surface: u32) -> Result<Option<ProxyTexture<B>>, String> {
        self.check()?;
        // An RGBA8 texture: it has a view.
        let t = self.new_texture(true);
        let id = t.id;
        let shared = Arc::clone(&self.shared);
        let sample = self.frame_owner.as_ref().map(|_| ReplaySample::Depth);
        self.post_sample(surface, id, sample, move |w| {
            let s = w.surface(surface)?;
            match w.inner.depth_level_texture(s)? {
                Some(h) => {
                    w.textures.insert(id, h);
                }
                None => shared.miss("depth_level_texture returned None".into()),
            }
            Ok(())
        });
        Ok(Some(t))
    }

    fn handles_surface_aliases(&self) -> bool { self.surface_aliases }

    fn defers_depth_aliases(&self) -> bool {
        true
    }

    fn depth_alias_texture(
        &mut self,
        depth: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        pitch: u32,
        generation: u64,
        offset: usize,
        desc: &TextureDesc,
        options: gfx::texture::DecodeOptions,
    ) -> Result<Option<ProxyTexture<B>>, String> {
        self.check()?;
        let view = match &self.judge {
            None => true,
            Some(j) => match self
                .alias_views
                .iter()
                .find(|(d, s, _)| d == desc && *s == options.signed)
            {
                Some(&(_, _, v)) => v,
                None => {
                    // The host format depends on the description only:
                    // decode zeros once to learn it.
                    let len = desc.total_size().map_err(|e| e.to_string())?;
                    let v = gfx::texture::decode_texture(desc, &vec![0; len], None, options)
                        .is_ok_and(|t| j.upload_view(&t));
                    self.alias_views.push((desc.clone(), options.signed, v));
                    v
                }
            },
        };
        let t = self.new_texture(view);
        let id = t.id;
        let desc = desc.clone();
        let shared = Arc::clone(&self.shared);
        let sample = self.frame_owner.as_ref().map(|_| ReplaySample::Alias { format, width, height, pitch, generation, offset, desc: desc.clone(), options });
        self.post_sample(depth, id, sample, move |w| {
            // A backend that makes the texture itself (`GfxBackend` decodes
            // the words on the GPU) does, with no read-back here.
            if w.inner.defers_depth_aliases() || w.inner.handles_surface_aliases() {
                let s = w.surface(depth)?;
                if let Some(h) = w.inner.depth_alias_texture(
                    s, format, width, height, pitch, generation, offset, &desc, options,
                )? {
                    w.textures.insert(id, h);
                    return Ok(());
                }
            }
            // One read-back per depth generation, as the device does it.
            if w.depth_words
                .as_ref()
                .is_none_or(|(s, g, _)| (*s, *g) != (depth, generation))
            {
                let s = w.surface(depth)?;
                let layout = GuestLayoutKind::Linear { pitch };
                let Some(bytes) = w.inner.read_depth(s, format, width, height, layout)? else {
                    shared.miss("read_depth returned None for a depth-alias texture".into());
                    return Ok(());
                };
                w.depth_words = Some((depth, generation, bytes));
            }
            let bytes = &w.depth_words.as_ref().expect("read above").2;
            let decoded = match bytes
                .get(offset..)
                .ok_or_else(|| {
                    format!("read_depth returned {} bytes, offset {offset}", bytes.len())
                })
                .and_then(|b| {
                    gfx::texture::decode_texture(&desc, b, None, options).map_err(|e| e.to_string())
                }) {
                Ok(d) => d,
                Err(e) => {
                    shared.miss(format!("depth-alias texture: {e}"));
                    return Ok(());
                }
            };
            let h = w.inner.upload_texture(&decoded)?;
            w.textures.insert(id, h);
            Ok(())
        });
        Ok(Some(t))
    }

    fn surface_texture(&mut self, surface: u32) -> Result<Option<ProxyTexture<B>>, String> {
        self.check()?;
        if !self.sample_surfaces {
            return Ok(None);
        }
        let t = self.new_texture(self.surface_view(surface));
        let id = t.id;
        let shared = Arc::clone(&self.shared);
        let track_source = self.frame_owner.is_some();
        let sample = track_source.then_some(ReplaySample::InPlace);
        self.post_sample(surface, id, sample, move |w| {
            let s = w.surface(surface)?;
            match w.inner.surface_texture(s)? {
                Some(h) => {
                    w.textures.insert(id, h);
                    if track_source {
                        w.live_sources.insert(id, surface);
                    }
                }
                None => shared.miss("surface_texture returned None".into()),
            }
            Ok(())
        });
        Ok(Some(t))
    }
}
