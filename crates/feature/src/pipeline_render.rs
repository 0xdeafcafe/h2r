//! Run the end of a world frame on a second executor while simulation
//! advances. Enable with `impl.render.pipelined_frame=on`.
//!
//! The game thread joins the previous frame at the next render or at a
//! device dependency. Before [`Addrs::draw_boundary`] it drives the view
//! loop and publishes worker scene segments and their ordered animation
//! requests. Particle updates and cache residency belong to the game thread.
//! The worker inherits the nested guest stack in a COW snapshot and
//! returns through the original frame-end and present code. The frame
//! wait runs once on the game thread. See docs/graphics/pipelined-render.md.
//!
//! Serial guards cover menu/loading frames, outstanding I/O, quiescence
//! and snapshot coverage. A refused worker call resumes from its current
//! guest CPU and stack on the game thread.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine::points;
use runtime::executor::{CallOutcome, ExecutorError, SecondExecutor};
use runtime::guest::{FaultReason, Permissions, WrittenPage};
use runtime::watch::CONTIGUOUS_BASE;
use runtime::{CpuAbi, Event, ExecCtx, Guest, GuestFault, Runtime, TransferAbi, abi};

use crate::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

#[path = "pipeline_snapshot.rs"]
mod snapshot;
#[path = "pipeline_scene.rs"]
mod scene;
pub use scene::{ResidentCache, SceneTraversal};

use snapshot::Footprint;

/// The native layer handoff, or nothing when the native renderer is not built.
mod world_layers {
    #[cfg(feature = "native-render")]
    pub(super) use crate::native_world_layers::{Layer, current, replace};
    #[cfg(not(feature = "native-render"))]
    pub(super) type Layer = ();
    #[cfg(not(feature = "native-render"))]
    pub(super) fn current() -> Option<Layer> { None }
    #[cfg(not(feature = "native-render"))]
    pub(super) fn replace(_: Option<Layer>) -> Option<Layer> { None }
}

pub const ID: &str = "pipeline-render";
pub const SWITCH: &str = "impl.render.pipelined_frame";
/// Legacy default for the exact implementation switch.
pub const ENV: &str = "H2_PIPELINE_RENDER";
/// `H2_PIPELINE_WATCH=0x485a80,...`: guest words to watch. Each change is
/// printed with the step it showed up after (`pipeline: watch ...`), so a
/// run says whether a merge of ours or the game itself changed a word.
pub const WATCH_ENV: &str = "H2_PIPELINE_WATCH";
/// `0` turns off the frame-counter prediction ([`PipelineRender::predict_frames`]).
pub const PREDICT_FRAMES_ENV: &str = "H2_PIPELINE_PREDICT_FRAMES";
/// `0` turns off the seed replay ([`PipelineRender::replay_seeds`]).
pub const SEED_REPLAY_ENV: &str = "H2_PIPELINE_SEED_REPLAY";
/// Guest functions (hex, comma-separated) the render thread refuses, so a
/// render that reaches one finishes on the game thread
/// ([`PipelineRender::refuse`]): a bisecting aid.
pub const REFUSE_ENV: &str = "H2_PIPELINE_REFUSE";
/// Set (not `0`): every hand-back's conflicts (bytes the sim and the render
/// both changed, the render's value winning) are tallied by word, and the
/// most frequent are printed every [`CONFLICT_REPORT_EVERY`] hand-backs
/// with their virtual addresses ([`PipelineRender::report_conflicts`]).
pub const CONFLICTS_ENV: &str = "H2_PIPELINE_CONFLICTS";
/// `0`: the render's I/O pool calls stop it until the game thread makes
/// them (serviced), as before they were deferred
/// ([`PipelineRender::defer_io`]).
pub const DEFER_IO_ENV: &str = "H2_PIPELINE_DEFER_IO";
/// Hand-backs between two conflict reports.
pub const CONFLICT_REPORT_EVERY: u32 = 300;
/// Words in a conflict report.
const CONFLICT_REPORT_WORDS: usize = 32;

/// The conflicts of the hand-backs since the last report, by physical word.
#[derive(Default)]
struct ConflictTally {
    hand_backs: u32,
    /// Physical word -> (bytes, the last live, view and snapshot bytes).
    words: BTreeMap<u32, (u64, [u8; 3])>,
}

impl ConflictTally {
    fn add(&mut self, found: &[(u32, u8, u8, u8)]) {
        for &(p, live, view, base) in found {
            let e = self.words.entry(p & !3).or_default();
            e.0 += 1;
            e.1 = [live, view, base];
        }
    }

    /// The report lines, most conflicted words first; `virt` names a
    /// physical address's virtual ones.
    fn report(&self, virt: impl Fn(u32) -> Vec<u32>) -> Vec<String> {
        let mut v: Vec<_> = self.words.iter().collect();
        v.sort_by_key(|(p, (n, _))| (std::cmp::Reverse(*n), **p));
        let total: u64 = self.words.values().map(|(n, _)| n).sum();
        let mut out = vec![format!(
            "pipeline: conflicts over {} hand-backs: {total} bytes in {} words; the most frequent:",
            self.hand_backs,
            self.words.len()
        )];
        for (p, (n, [l, w, b])) in v.into_iter().take(CONFLICT_REPORT_WORDS) {
            let va: Vec<String> = virt(*p).iter().map(|a| format!("{a:#010x}")).collect();
            out.push(format!(
                "pipeline:   phys {p:#010x} va [{}] bytes {n} (last: live {l:#04x} render {w:#04x} snapshot {b:#04x})",
                va.join(" ")
            ));
        }
        out
    }
}

/// Whether a switch is on: anything but `0`, `off` or `false`; unset is
/// `default`.
fn switch(value: Option<&str>, default: bool) -> bool {
    match value.map(str::trim) {
        None => default,
        Some(v) => !matches!(v, "0" | "off" | "false"),
    }
}
/// At most this many `pipeline: watch` lines.
const WATCH_LINES: u32 = 200;
/// The render thread's guest stack: as large as the game thread's own
/// (the XBE header's stack size for the main thread, 256 KiB), since the
/// front-end runs there serially and may use as much. 128 KiB was not
/// enough: the first world frame reaches a function whose `_chkstk`
/// (`0x00320560`) probes a frame of over 128 KiB, which faulted just below
/// the render stack (an unmapped read about 128 KiB under its `esp`).
pub const STACK_LEN: u32 = 0x4_0000;

/// Two words: (virtual, physical) or (physical, length).
type Pair = (u32, u32);

/// A byte range of guest memory `[start, end)` (virtual).
pub type Range = (u32, u32);

/// The guest addresses the policy names (docs/graphics/pipelined-render.md,
/// "Every dependency line"); [`Addrs::HALO2`] for the game, anything for a
/// synthetic test.
#[derive(Clone, Debug)]
pub struct Addrs {
    /// The engine frame wait, run on the game thread at the launch and a
    /// no-op on the render thread.
    pub frame_wait: u32,
    /// `[engine_globals]`, `+0x1120` (byte) and `+2` (word): the world-path
    /// test at the top of `0x00223240`.
    pub engine_globals: u32,
    /// `0x00068290`'s globals, in its order: two bytes and a pointer.
    pub ui_state: (u32, u32, u32),
    /// Resource-cache countdowns: `0x00223240` presents only while both are
    /// `<= 0`. Running ones mean loading: serial.
    pub cache_countdowns: (u32, u32),
    /// The 64-bit present counter `rasterizer_present` adds to: the game
    /// thread adds the predicted present at the launch, the join the rest.
    pub present_counter: u32,
    /// Render-frame counter (`render_views` adds 1): handed back as a
    /// delta.
    pub frame_counter: u32,
    /// A byte the render clears (per view) and the sim sets: cleared live
    /// at the launch, never handed back.
    pub launch_clear: u32,
    /// Visit-mark counters (32-bit): the render counts in its view, and the
    /// join adds its count to live RAM's (by delta, as the serial run
    /// counts), since some of the stamps (the lights' at `+0x0c`) are
    /// handed back. At a fallback live is also never behind the view.
    pub mark_counters: &'static [u32],
    /// Visit-mark flags and stamps: render-private; merged at a fallback.
    pub mark_bytes: &'static [Range],
    /// Words the host and the game thread own (the vblank counter and the
    /// swap records the vblank callback writes): never handed back.
    pub host_owned: &'static [Range],
    /// Shared with other guest threads under a kernel lock the render takes
    /// through a serviced call ([`SERVICED`]): around each such call the
    /// render's changes go to live RAM and live RAM's bytes come back into
    /// the view, so the render works on the live copy while it holds the
    /// lock. Left out of the final merge.
    pub live_bytes: &'static [Range],
    /// State only the render changes between its launch and its join (the
    /// D3D device; the sim's D3D calls join first): where the sim's side
    /// changed a word the render changed too, the render's word wins at the
    /// hand-back, not the sim's ([`PipelineRender::render_owned`] adds
    /// ranges known at run time, such as the push buffer).
    pub render_owned: &'static [Range],
    /// The async I/O queue head (0: none): requests queued and not yet
    /// done make the next frame serial (`io_busy`).
    pub io_queue: u32,
    /// The async I/O pool's free list: (head word, link offset in a
    /// request, requests). A frame pipelines only when every request is on
    /// it (the worker idle, nothing queued or being read); otherwise
    /// `io_busy`. `None`: [`io_queue`](Self::io_queue) alone decides.
    pub io_free: Option<(u32, u32, u32)>,
    /// Words holding the handles of the I/O pool's kernel objects (its two
    /// mutexes and the worker's semaphore). The render's waits on and
    /// releases of these are deferred ([`PipelineRender::defer_io`]): the
    /// render carries on, and the game thread makes the calls at the join,
    /// in order. A game-thread wait on one while a render is in flight
    /// joins the render first (`io_joins`), so the pool sees the render's
    /// requests before the tick's, as serially.
    pub io_handles: &'static [u32],
    /// World frames that run serially after a frame that was not (a level
    /// load, a menu): the load burst of cache requests is over first.
    pub warmup: u32,
    /// Frames that run serially after a fallback.
    pub after_fallback: u32,
    /// Random seeds both the sim and the render step: a pointer word and
    /// the seed offsets in the struct it points at, each stepped `x * a +
    /// c` ([`RANDOM_A`], [`RANDOM_C`]). The join counts the render's steps
    /// and applies as many to live RAM's seed, which the sim has stepped
    /// meanwhile, so no draw is replayed and the total stays the serial
    /// run's. Never merged as bytes.
    pub random: Option<(u32, &'static [u32])>,
    /// Record caches both sides allocate in ([`SharedCache`]): a game-thread
    /// call that would allocate joins the render first.
    pub shared_caches: &'static [SharedCache],
    /// Last render-only phase. The original reaches this call after all
    /// game and alternate views, including HUD and UI, have issued their
    /// particle visits and animation requests. A synthetic renderer with
    /// no preparation can start at frame entry.
    pub draw_boundary: Option<DrawBoundary>,
    /// Generated scene scopes whose resource requests can be published before the tick.
    pub scene: Option<SceneTraversal>,
}

/// The call to the draw tail, identified by its callee and caller's return PC.
#[derive(Clone, Copy, Debug)]
pub struct DrawBoundary {
    pub entry: u32,
    pub return_pc: u32,
    /// Position of this direct call below the prepared frame's stack top.
    /// An identically named call in a nested view cannot seal the frame.
    pub depth: u32,
    /// The view context cleared by render_views' epilogue, also needed
    /// on live RAM while the worker finishes that epilogue on its view.
    pub view_context: Option<u32>,
    /// A debug bracket open across this call must close on the same thread.
    pub debug_flag_from_esp: Option<u32>,
}

/// A record cache the render and the sim both allocate in, through one
/// function `entry` (stdcall, one argument: an object handle; returns the
/// record's handle), which hands back the object's record when the object's
/// `field` names a record that `owner` says is the object's, and otherwise
/// takes a free record (or evicts the least recently used) and initializes
/// it. A byte merge of two such allocations is unsound: both sides take the
/// same free record from the same snapshot, the hand-back keeps the render's
/// owner and the sim's bytes wherever the render's equal the snapshot's, and
/// the mixed record is then reused as valid. So a game-thread call that
/// would allocate (judged on live RAM, before it runs) joins the render
/// first, as a sim D3D call does; a call that finds its record only reads.
///
/// Both arrays are Halo data arrays: the data pointer at `+0x44` of the
/// array header the pointer word names, the index in a handle's low 16 bits.
#[derive(Clone, Copy, Debug)]
pub struct SharedCache {
    pub entry: u32,
    /// The pointer word to the object array, its record stride and where
    /// in a record the object's body pointer is.
    pub objects: u32,
    pub object_stride: u32,
    pub body_at: u32,
    /// The body's word holding its cache record handle (`-1`: none).
    pub field: u32,
    /// The pointer word to the cache array, its record length and where in
    /// a record its owner's handle is.
    pub cache: u32,
    pub record_len: u32,
    pub owner: u32,
}

impl SharedCache {
    /// Whether `entry(handle)` would allocate, on `g`: true when anything on
    /// the way does not read.
    pub fn would_allocate(&self, g: &Guest, handle: u32) -> bool {
        let read = || -> Option<bool> {
            let index = |h: u32| h & 0xffff;
            let objects = g.u32(g.u32(self.objects).ok()?.wrapping_add(0x44)).ok()?;
            let body = g
                .u32(
                    objects
                        .wrapping_add(index(handle) * self.object_stride)
                        .wrapping_add(self.body_at),
                )
                .ok()?;
            let record = g.u32(body.wrapping_add(self.field)).ok()?;
            if record == u32::MAX {
                return Some(true);
            }
            let cache = g.u32(g.u32(self.cache).ok()?.wrapping_add(0x44)).ok()?;
            let owner = g
                .u32(
                    cache
                        .wrapping_add(index(record) * self.record_len)
                        .wrapping_add(self.owner),
                )
                .ok()?;
            Some(owner != handle)
        };
        read().unwrap_or(true)
    }
}

/// The game's random step (`0x00027be8`: `imul 0x19660d`, `add
/// 0x3c6ef35f`).
pub const RANDOM_A: u32 = 0x0019_660d;
pub const RANDOM_C: u32 = 0x3c6e_f35f;
/// Steps the join looks for between a seed's snapshot and render values.
const RANDOM_STEPS: u32 = 1 << 16;

/// `n` such that `n` steps take `from` to `to`, if at most
/// [`RANDOM_STEPS`].
fn random_steps(from: u32, to: u32) -> Option<u32> {
    let mut x = from;
    for n in 0..=RANDOM_STEPS {
        if x == to {
            return Some(n);
        }
        x = x.wrapping_mul(RANDOM_A).wrapping_add(RANDOM_C);
    }
    None
}

/// The seeds' addresses, through live RAM's pointer.
fn random_words(a: &Addrs, g: &Guest) -> Vec<u32> {
    let Some((ptr, offsets)) = a.random else {
        return Vec::new();
    };
    match g.u32(ptr) {
        Ok(at) if at != 0 => offsets.iter().map(|&o| at.wrapping_add(o)).collect(),
        _ => Vec::new(),
    }
}

fn random_step(mut x: u32, n: u32) -> u32 {
    for _ in 0..n {
        x = x.wrapping_mul(RANDOM_A).wrapping_add(RANDOM_C);
    }
    x
}

impl Addrs {
    pub const HALO2: Addrs = Addrs {
        frame_wait: 0x0012_b2e0,
        engine_globals: 0x004e_6948,
        ui_state: (0x004c_f770, 0x004c_f772, 0x004c_f77c),
        cache_countdowns: (0x004e_6470, 0x004e_64a0),
        present_counter: 0x0048_5aa0,
        frame_counter: 0x004b_a034,
        launch_clear: 0x004e_6098,
        mark_counters: &[0x004d_e2fc, 0x004e_0308, 0x004e_7414, 0x004e_7c1c],
        mark_bytes: &[
            // flag 0x004de2f8, stamps 0x004de300.. (2048 objects)
            (0x004d_e2f8, 0x004d_e2fc),
            (0x004d_e300, 0x004e_0300),
            // flag 0x004e0304 (counter 0x004e0308)
            (0x004e_0304, 0x004e_0308),
            // flag 0x004e7411, stamps 0x004e7418..0x004e7c18
            (0x004e_7411, 0x004e_7412),
            (0x004e_7418, 0x004e_7c18),
            // flag 0x004e7c18, stamps from 0x004e7c20: their end is not
            // read yet; the census saw lines up to 0x004e7d7f.
            (0x004e_7c18, 0x004e_7c19),
            (0x004e_7c20, 0x004e_8000),
        ],
        host_owned: &[(0x0048_5ab0, 0x0048_5ac0), (0x0055_e6b8, 0x0055_e6c0)],
        // The async I/O pool (`async_io_initialize` `0x00120a90`): the
        // words from `0x004e0354` (the pool mutex `0x004e0358`, the queue
        // mutex `0x004e035c`, the worker's semaphore `0x004e0360`, the
        // sequence `0x004e0364`), the 150 requests from `0x004e0368`, the
        // free-list head `0x004e28e8`, the queue head `0x004e28ec` (the
        // worker `0x00120c40` takes the request there) and the request
        // count `0x004e28f0`.
        live_bytes: &[(0x004e_0354, 0x004e_28f4)],
        // The D3D device (`d3d::GuestLayout::HALO2`: 0x928 words from
        // `0x00404fe0`, the push buffer's put pointer at +0) and the device
        // pointer `0x00407488`.
        render_owned: &[(0x0040_4fe0, 0x0040_7480), (0x0040_7488, 0x0040_748c)],
        io_queue: 0x004e_28ec,
        // The 150 requests from `0x004e0368` (0x40 bytes each), the free
        // list's head `0x004e28e8`, linked at +0x3c (`0x001208b0` pops it,
        // the worker `0x00120c40` pushes a request back when it is done).
        io_free: Some((0x004e_28e8, 0x3c, 150)),
        // The pool mutex `0x004e0358` (`0x001208b0`), the queue mutex
        // `0x004e035c` (`0x001209c0`) and the worker's semaphore
        // `0x004e0360`.
        io_handles: &[0x004e_0358, 0x004e_035c, 0x004e_0360],
        warmup: 150,
        after_fallback: 30,
        // `[0x004e7408]`: +0 the game's seed (object updates, `0x001032a0`,
        // which the render reaches), +4 the effects seed (the render's
        // `0x00027be0` and the particles' `0x00174089`).
        random: Some((0x004e_7408, &[0, 4])),
        // Object lighting records are allocated during the live view pass.
        shared_caches: &[],
        scene: Some(SceneTraversal::HALO2),
        draw_boundary: Some(DrawBoundary {
            entry: engine::render_loop::views::END_A,
            // render_views has exhausted its per-view loop.
            return_pc: 0x0002_b6f7,
            // Frame -> render_game_views -> render_views -> END_A.
            depth: 0x5c,
            view_context: Some(0x004b_a030),
            debug_flag_from_esp: Some(0x13),
        }),
    };
}

/// Kernel calls the game thread makes for the render at the join, after
/// which the render carries on on its own thread against its snapshot: the
/// async I/O pool's lock (`0x001208b0`/`0x001209c0`: wait on the mutex,
/// release it, release the worker's semaphore). Their only memory effects
/// are on the render stack (copied both ways) and in [`Addrs::live_bytes`].
/// Any other refused call is a fallback: the rest of the render on the game
/// thread against live RAM, which has moved on by the sim's ticks since the
/// launch.
pub const SERVICED: &[&str] = &[
    "NtWaitForSingleObjectEx",
    "NtReleaseMutant",
    "NtReleaseSemaphore",
    // A protection change: made live, and the same protections copied to
    // the view's pages, so the render's next access sees what it would
    // serially.
    "MmSetAddressProtect",
];

/// Why a frame ran serially.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Serial {
    /// `0x00223240` would take its fallback path (menus, loading screens,
    /// Bink): only the world path is pipelined.
    NotWorld,
    /// The async I/O queue has requests ([`Addrs::io_queue`]).
    IoBusy,
    /// Within [`Addrs::warmup`] world frames of a frame that was not one.
    Warmup,
    /// Within [`Addrs::after_fallback`] frames of a fallback.
    AfterFallback,
    /// A resource-cache countdown is running (loading).
    Loading,
    /// A snapshot or pause asked for the render to be joined.
    Quiesce,
    /// A page touched by the previous render lost or changed its mapping.
    SnapshotMapping,
    /// The previous render's trace was incomplete.
    SnapshotTrace,
    /// The game thread's memory could not be read for the decision.
    Unreadable,
}

impl Serial {
    fn name(self) -> &'static str {
        match self {
            Serial::NotWorld => "not_world",
            Serial::Loading => "loading",
            Serial::Quiesce => "quiesce",
            Serial::Unreadable => "unreadable",
            Serial::SnapshotMapping => "snapshot_mapping",
            Serial::SnapshotTrace => "snapshot_trace",
            Serial::IoBusy => "io_busy",
            Serial::Warmup => "warmup",
            Serial::AfterFallback => "after_fallback",
        }
    }
}

/// Running totals; [`PipelineRender::stats`] reads them.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Frames launched on the render thread.
    pub pipelined: u64,
    /// Of those, frames that returned on the render thread.
    pub completed: u64,
    pub serial: BTreeMap<&'static str, u64>,
    /// Fallbacks by the call that stopped the render.
    pub fallbacks: BTreeMap<String, u64>,
    /// Calls the game thread made for the render, the render carrying on
    /// on its thread after each.
    pub serviced: BTreeMap<String, u64>,
    /// Joins a sim D3D call made (the render still in flight).
    pub d3d_joins: u64,
    /// Joins a game-thread call that would allocate in a
    /// [`SharedCache`] made.
    pub cache_joins: u64,
    /// Historical telemetry field. There is no tick-entry join.
    pub sim_joins: u64,
    /// D3D calls from another guest thread while a render was in flight
    /// (not ordered).
    pub d3d_other_thread: u64,
    pub join_wait: Duration,
    /// Wall time between launch and entry to the game-thread join.
    pub lead: Duration,
    /// Wall time when the render worker was executing before the game-thread join.
    pub overlap: Duration,
    /// Successful COW accesses to pages absent from the preceding trace.
    pub new_snapshot_pages: u64,
    /// Faults on the worker from unmapped or protected addresses.
    pub snapshot_faults: u64,
    /// Settled animation uses replayed in order from a prepared scene.
    pub animation_uses: u64,
    /// Suspended lookups replayed by the scheduled cache owner.
    pub animation_owner_calls: u64,
    pub scene_retries: u64,
    pub hand_back: Duration,
    pub snapshot: Duration,
    pub render: Duration,
    pub pages: u64,
    pub bytes: u64,
    pub conflicts: u64,
    /// Of `conflicts`, those in render-owned words ([`Addrs::render_owned`]),
    /// where the render's word won.
    pub owned_conflicts: u64,
    /// Joins where the render presented a different number of times than
    /// the launch predicted.
    pub mispredicts: u64,
    /// Guest accesses charged to the game thread at the joins.
    pub accesses: u64,
    /// Frames in which a side waited for the other because of the async
    /// I/O pool: [`io_stalls_render`](Self::io_stalls_render) plus
    /// [`io_stalls_join`](Self::io_stalls_join).
    pub io_stalls: u64,
    /// The time those waits took.
    pub io_stall: Duration,
    /// Frames whose render stopped at a pool call until the game thread
    /// made it (serviced): deferral off, or the sim had used the pool
    /// first that frame ([`io_refused`](Self::io_refused)).
    pub io_stalls_render: u64,
    /// Their time: the render stopped until the game thread made the
    /// call, and the game thread waiting for the rest of the render.
    pub io_stall_render: Duration,
    /// Frames whose game thread waited on a pool handle after the render
    /// had used the pool, and so joined it first.
    pub io_stalls_join: u64,
    /// Their time, the join's.
    pub io_stall_join: Duration,
    /// Pool calls the render made without stopping ([`Addrs::io_handles`]).
    pub io_deferred: u64,
    /// Render pool calls not deferred because the sim had used the pool
    /// first that frame: serviced instead (their order is the sim's, then
    /// the render's).
    pub io_refused: u64,
    /// Game-thread pool waits that joined the render in flight (the render
    /// had used the pool first that frame).
    pub io_joins: u64,
    /// Game-thread pool waits with a render in flight that had not used
    /// the pool: no join, the sim owns the pool for the rest of the frame.
    pub io_sim_first: u64,
    /// Deferred calls whose real result, at the join, differed from the
    /// one the render was given.
    pub io_mismatches: u64,
    /// Pool waits from another guest thread while a render was in flight
    /// (not ordered; none expected, since a frame pipelines only with the
    /// worker idle).
    pub io_other_thread: u64,
}

impl Stats {
    fn since(&self, then: &Stats) -> Stats {
        let sub = |a: &BTreeMap<&'static str, u64>, b: &BTreeMap<&'static str, u64>| {
            a.iter()
                .map(|(k, v)| (*k, v - b.get(k).copied().unwrap_or(0)))
                .filter(|(_, v)| *v != 0)
                .collect()
        };
        Stats {
            pipelined: self.pipelined - then.pipelined,
            completed: self.completed - then.completed,
            serial: sub(&self.serial, &then.serial),
            fallbacks: sub_named(&self.fallbacks, &then.fallbacks),
            serviced: sub_named(&self.serviced, &then.serviced),
            d3d_joins: self.d3d_joins - then.d3d_joins,
            cache_joins: self.cache_joins - then.cache_joins,
            sim_joins: self.sim_joins - then.sim_joins,
            d3d_other_thread: self.d3d_other_thread - then.d3d_other_thread,
            join_wait: self.join_wait - then.join_wait,
            lead: self.lead - then.lead,
            overlap: self.overlap - then.overlap,
            new_snapshot_pages: self.new_snapshot_pages - then.new_snapshot_pages,
            snapshot_faults: self.snapshot_faults - then.snapshot_faults,
            animation_uses: self.animation_uses - then.animation_uses,
            animation_owner_calls: self.animation_owner_calls - then.animation_owner_calls,
            scene_retries: self.scene_retries - then.scene_retries,
            hand_back: self.hand_back - then.hand_back,
            snapshot: self.snapshot - then.snapshot,
            render: self.render - then.render,
            pages: self.pages - then.pages,
            bytes: self.bytes - then.bytes,
            conflicts: self.conflicts - then.conflicts,
            owned_conflicts: self.owned_conflicts - then.owned_conflicts,
            mispredicts: self.mispredicts - then.mispredicts,
            accesses: self.accesses - then.accesses,
            io_stalls: self.io_stalls - then.io_stalls,
            io_stall: self.io_stall - then.io_stall,
            io_stalls_render: self.io_stalls_render - then.io_stalls_render,
            io_stall_render: self.io_stall_render - then.io_stall_render,
            io_stalls_join: self.io_stalls_join - then.io_stalls_join,
            io_stall_join: self.io_stall_join - then.io_stall_join,
            io_deferred: self.io_deferred - then.io_deferred,
            io_refused: self.io_refused - then.io_refused,
            io_joins: self.io_joins - then.io_joins,
            io_sim_first: self.io_sim_first - then.io_sim_first,
            io_mismatches: self.io_mismatches - then.io_mismatches,
            io_other_thread: self.io_other_thread - then.io_other_thread,
        }
    }

    /// One `pipeline:` line.
    pub fn line(&self) -> String {
        let ms = |d: Duration, n: u64| {
            if n == 0 {
                0.0
            } else {
                d.as_secs_f64() * 1e3 / n as f64
            }
        };
        let joins = self.pipelined.max(1);
        let serial: Vec<String> = self
            .serial
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
        let mut fb: Vec<(&String, &u64)> = self.fallbacks.iter().collect();
        fb.sort_by_key(|(_, v)| std::cmp::Reverse(**v));
        let fallbacks: u64 = self.fallbacks.values().sum();
        let fb: Vec<String> = fb.iter().map(|(k, v)| format!("{k} {v}")).collect();
        let serviced: u64 = self.serviced.values().sum();
        let sv: Vec<String> = self
            .serviced
            .iter()
            .map(|(k, v)| format!("{k} {v}"))
            .collect();
        format!(
            "pipeline: frames pipelined {} completed {} serial [{}] fallbacks {} [{}] serviced {} [{}] d3d_joins {} cache_joins {} sim_joins {} d3d_other_thread {} lead {:.2}ms overlap {:.2}ms new_snapshot_pages {} snapshot_faults {} join_wait {:.2}ms hand_back {:.2}ms snapshot {:.2}ms render {:.2}ms pages {:.1} conflicts {} (render-owned {}) mispredicts {} io_stalls {} ({:.2}ms) [render {} ({:.2}ms), sim_join {} ({:.2}ms)] io_deferred {} io_refused {} io_joins {} io_sim_first {} io_mismatches {} io_other_thread {}",
            self.pipelined,
            self.completed,
            serial.join(", "),
            fallbacks,
            fb.join(", "),
            serviced,
            sv.join(", "),
            self.d3d_joins,
            self.cache_joins,
            self.sim_joins,
            self.d3d_other_thread,
            ms(self.lead, joins),
            ms(self.overlap, joins),
            self.new_snapshot_pages,
            self.snapshot_faults,
            ms(self.join_wait, joins),
            ms(self.hand_back, joins),
            ms(self.snapshot, joins),
            ms(self.render, joins),
            self.pages as f64 / joins as f64,
            self.conflicts,
            self.owned_conflicts,
            self.mispredicts,
            self.io_stalls,
            self.io_stall.as_secs_f64() * 1e3,
            self.io_stalls_render,
            self.io_stall_render.as_secs_f64() * 1e3,
            self.io_stalls_join,
            self.io_stall_join.as_secs_f64() * 1e3,
            self.io_deferred,
            self.io_refused,
            self.io_joins,
            self.io_sim_first,
            self.io_mismatches,
            self.io_other_thread,
        )
    }
}

fn sub_named(a: &BTreeMap<String, u64>, b: &BTreeMap<String, u64>) -> BTreeMap<String, u64> {
    a.iter()
        .map(|(k, v)| (k.clone(), v - b.get(k).copied().unwrap_or(0)))
        .filter(|(_, v)| *v != 0)
        .collect()
}

/// What a render run sends back.
struct Done {
    exec: Box<SecondExecutor>,
    result: Result<CallOutcome, ExecutorError>,
    started: Instant,
    elapsed: Duration,
    footprint: Option<Footprint>,
}

enum Work {
    Draw(Job),
    Scene(scene::Job),
}

struct Job {
    exec: Box<SecondExecutor>,
    cpu: CpuAbi,
    start: JobStart,
    reply: SyncSender<Done>,
}

enum JobStart {
    Frame(u32),
    Prepared,
    Resume,
}

/// The live render is running on the dedicated render stack. The outer
/// token unwinds to the game caller. Only guest frames may cross the cut.
struct Preparing {
    token: u32,
    continuations: usize,
    scene_retry: bool,
    tail: Option<CpuAbi>,
}

/// A render launched and not yet joined.
struct Flight {
    rx: Receiver<Done>,
    /// Presents added to live at the launch.
    predicted: u64,
    launched: Instant,
}

/// Built once, at the first frame that pipelines.
struct Setup {
    jobs: SyncSender<Work>,
    scene: Option<Box<SecondExecutor>>,
    scene_dispatch_generation: u64,
    original: Arc<runtime::exec::OriginalHandler>,
    /// Physical pages of the render stack, sorted.
    stack_pages: Vec<u32>,
}

struct Inner {
    conflicts: ConflictTally,
    exec: Option<Box<SecondExecutor>>,
    setup: Option<Setup>,
    flight: Option<Flight>,
    preparing: Option<Preparing>,
    stats: Stats,
    coverage: snapshot::Coverage,
    printed: Stats,
    last_print: Option<Instant>,
    /// The last decision pipelined (a switch either way prints a line).
    pipelining: bool,
    /// [`Shared::watch`]'s words as last seen (`None`: not yet), and lines
    /// printed.
    watched: Vec<Option<Watched>>,
    watch_lines: u32,
    /// The watched words' write trap is set on live memory.
    focused: bool,
    /// The pool calls the render in flight deferred, in its order.
    deferred: Vec<Deferred>,
    /// When the render thread last stopped (a serviced call's wait starts
    /// there).
    stopped: Option<Instant>,
    render_layer: Option<world_layers::Layer>,
    /// The frame in flight's render stopped at a pool call
    /// ([`Stats::io_stalls_render`]).
    io_stalled_render: bool,
    /// The frame in flight was joined by a sim pool wait
    /// ([`Stats::io_stalls_join`]).
    io_stalled_join: bool,
    /// Which side used the I/O pool first in the frame in flight.
    pool: PoolUse,
}

/// Which side used the async I/O pool first since the launch. The first
/// one owns it for the rest of the frame: a render pool call after the
/// sim's is serviced at the join (the sim's, then the render's), and a sim
/// pool wait after the render's deferred calls joins the render (the
/// render's, then the sim's). Either way each side's calls see the pool as
/// the other left it; a frame where only one side uses the pool never
/// waits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum PoolUse {
    #[default]
    Free,
    Render,
    Sim,
}

/// A kernel call the render made on the I/O pool's objects and carried on
/// from ([`Addrs::io_handles`]): the game thread makes it at the join.
#[derive(Clone, Debug)]
struct Deferred {
    ordinal: u32,
    args: Vec<u32>,
    /// What the render was given.
    eax: u32,
    /// The out parameter's index and the value the render was given there.
    out: Option<(usize, u32)>,
}

/// The pool calls the render may defer: (ordinal, argument words, the
/// argument that must be null, the out parameter written).
/// `NtWaitForSingleObjectEx(handle, mode, alertable, timeout)` with no
/// timeout acquires the mutex; `NtReleaseMutant(handle, previous)` with no
/// previous count; `NtReleaseSemaphore(handle, count, previous)`, the
/// previous count (the pool's own `0x001209c0` passes a local it never
/// reads) given as 0.
const IO_CALLS: &[(u32, u32, Option<usize>, Option<usize>)] = &[
    (234, 4, Some(3), None),
    (221, 2, Some(1), None),
    (222, 3, None, Some(2)),
];

fn io_call(token: u32) -> Option<(u32, u32, Option<usize>, Option<usize>)> {
    IO_CALLS
        .iter()
        .copied()
        .find(|&(ordinal, ..)| runtime::kernel::function_token(ordinal) == token)
}

/// Overrides the render thread may run besides the D3D layer's.
pub type AllowFn = dyn Fn(u32, &str) -> bool + Send + Sync;

struct Shared {
    addrs: Addrs,
    allow: Arc<AllowFn>,
    d3d_entries: Vec<u32>,
    shared_watches: Vec<&'static str>,
    serviced: Vec<&'static str>,
    print: bool,
    stack_top: AtomicU32,
    /// The guest thread that runs `0x00223240` (0 before the first frame).
    game_thread: AtomicU32,
    in_flight: AtomicBool,
    drain: AtomicBool,
    /// A fallback's rest of the render is running on the game thread: its
    /// frame wait has run at the launch already.
    resuming: AtomicBool,
    setup_failed: AtomicBool,
    /// World frames in a row so far (for [`Addrs::warmup`]).
    world_run: AtomicU32,
    /// Frames still to run serially after a fallback.
    serial_left: AtomicU32,
    /// Views (`frame_counter` steps) the next launch adds for the render
    /// (the last completed frame's), and what the render in flight was
    /// credited with at its launch.
    frames_next: AtomicU32,
    frames_predicted: AtomicU32,
    /// Guest words whose changes are printed ([`WATCH_ENV`]).
    watch: Vec<u32>,
    /// The launch credits the live frame counter with the views the render
    /// will count ([`PipelineRender::predict_frames`]).
    predict_frames: bool,
    /// Guest functions the render thread refuses ([`REFUSE_ENV`]).
    refuse: Vec<u32>,
    /// Tally and print the hand-backs' conflicts ([`CONFLICTS_ENV`]).
    report_conflicts: bool,
    check_snapshot: bool,
    /// Defer the render's I/O pool calls ([`DEFER_IO_ENV`]).
    defer_io: bool,
    /// [`Addrs::render_owned`] and the ranges
    /// [`PipelineRender::render_owned`] added.
    render_owned: Vec<Range>,
    inner: Mutex<Inner>,
}

/// The feature. See the module docs.
#[derive(Clone)]
pub struct PipelineRender {
    on: bool,
    off_reason: Option<String>,
    shared: Arc<Shared>,
}

impl Default for PipelineRender {
    fn default() -> Self {
        Self::new()
    }
}

impl PipelineRender {
    /// Configures the runtime feature from the environment.
    pub fn new() -> Self {
        let on = std::env::var(ENV).is_ok_and(|v| v == "1");
        let watch = parse_watch(std::env::var(WATCH_ENV).ok().as_deref());
        let env = |name| std::env::var(name).ok();
        let refuse = parse_watch(env(REFUSE_ENV).as_deref());
        Self::with(on, Addrs::HALO2)
            .check_snapshot(true)
            .print(true)
            .watch(&watch)
            .predict_frames(switch(env(PREDICT_FRAMES_ENV).as_deref(), true))
            .replay_seeds(switch(env(SEED_REPLAY_ENV).as_deref(), true))
            .refuse(&refuse)
            .report_conflicts(switch(env(CONFLICTS_ENV).as_deref(), false))
            .defer_io(switch(env(DEFER_IO_ENV).as_deref(), true))
    }

    /// Explicitly on or off, with `addrs`. Prints no `pipeline:` lines
    /// until [`print`](Self::print).
    pub fn with(on: bool, addrs: Addrs) -> Self {
        let render_owned = addrs.render_owned.to_vec();
        PipelineRender {
            on,
            off_reason: None,
            shared: Arc::new(Shared {
                addrs,
                allow: Arc::new(|_, _| false),
                d3d_entries: Vec::new(),
                shared_watches: Vec::new(),
                serviced: SERVICED.to_vec(),
                print: on,
                stack_top: AtomicU32::new(0),
                game_thread: AtomicU32::new(0),
                in_flight: AtomicBool::new(false),
                drain: AtomicBool::new(false),
                resuming: AtomicBool::new(false),
                setup_failed: AtomicBool::new(false),
                world_run: AtomicU32::new(0),
                serial_left: AtomicU32::new(0),
                frames_next: AtomicU32::new(1),
                frames_predicted: AtomicU32::new(0),
                watch: Vec::new(),
                predict_frames: true,
                refuse: Vec::new(),
                report_conflicts: false,
                check_snapshot: false,
                defer_io: true,
                render_owned,
                inner: Mutex::new(Inner {
                    conflicts: ConflictTally::default(),
                    exec: None,
                    setup: None,
                    flight: None,
                    preparing: None,
                    stats: Stats::default(),
                    coverage: snapshot::Coverage::default(),
                    printed: Stats::default(),
                    last_print: None,
                    pipelining: false,
                    watched: Vec::new(),
                    watch_lines: 0,
                    focused: false,
                    deferred: Vec::new(),
                    stopped: None,
                    render_layer: None,
                    io_stalled_render: false,
                    io_stalled_join: false,
                    pool: PoolUse::Free,
                }),
            }),
        }
    }

    fn shared_mut(&mut self) -> &mut Shared {
        Arc::get_mut(&mut self.shared).expect("configured before it is built")
    }

    /// The D3D layer: `entries` are the addresses the game calls (the sim's
    /// calls there join the render first); `allow` names every override the
    /// render thread may run (the entries, their shadows); `watches` the
    /// write-watch owners those overrides drain by id.
    pub fn with_d3d(
        mut self,
        entries: Vec<u32>,
        allow: impl Fn(u32, &str) -> bool + Send + Sync + 'static,
        watches: &[&'static str],
    ) -> Self {
        let s = self.shared_mut();
        s.d3d_entries = entries;
        s.allow = Arc::new(allow);
        s.shared_watches = watches.to_vec();
        self
    }

    /// The kernel calls the game thread makes for the render (default
    /// [`SERVICED`]); every other refused call is a fallback.
    pub fn serviced(mut self, names: &[&'static str]) -> Self {
        self.shared_mut().serviced = names.to_vec();
        self
    }

    /// Whether the launch credits the live frame counter with the views
    /// the render will count, so the sim's next tick reads the serial
    /// run's value (on by default; [`PREDICT_FRAMES_ENV`]). Off, the join
    /// adds them, after the tick read it one short.
    pub fn predict_frames(mut self, on: bool) -> Self {
        self.shared_mut().predict_frames = on;
        self
    }

    /// Whether the join applies the render's random steps on top of the
    /// sim's (on by default; [`SEED_REPLAY_ENV`]). Off, the seeds are merged
    /// as bytes, the render's value winning.
    pub fn replay_seeds(mut self, on: bool) -> Self {
        if !on {
            self.shared_mut().addrs.random = None;
        }
        self
    }

    /// Guest functions the render thread refuses ([`REFUSE_ENV`]): a render
    /// reaching one falls back there and finishes on the game thread.
    pub fn refuse(mut self, addrs: &[u32]) -> Self {
        self.shared_mut().refuse = addrs.to_vec();
        self
    }

    /// Whether the render's I/O pool calls are deferred to the join
    /// ([`Addrs::io_handles`]; on by default, [`DEFER_IO_ENV`]). Off, each
    /// stops the render until the game thread makes it (serviced).
    pub fn defer_io(mut self, on: bool) -> Self {
        self.shared_mut().defer_io = on;
        self
    }

    /// Tally every hand-back's conflicts by word and print the most
    /// frequent now and then ([`CONFLICTS_ENV`]).
    pub fn report_conflicts(mut self, on: bool) -> Self {
        self.shared_mut().report_conflicts = on;
        self
    }

    /// More guest ranges the render owns between its launch and its join
    /// ([`Addrs::render_owned`]): the D3D push buffer, wherever the host
    /// put it.
    pub fn render_owned(mut self, ranges: &[Range]) -> Self {
        self.shared_mut().render_owned.extend_from_slice(ranges);
        self
    }

    /// Use the render census access trap to check snapshot layout across frames.
    pub fn check_snapshot(mut self, on: bool) -> Self {
        self.shared_mut().check_snapshot = on;
        self
    }

    /// Resolve the exact switch with the old variable as its fallback.
    pub fn requested(switches: &crate::Switches, legacy: Option<&str>) -> bool {
        switches.feature(SWITCH, legacy == Some("1"))
    }

    /// The conflict report so far (tests): the lines
    /// [`CONFLICTS_ENV`] prints.
    pub fn conflict_report(&self) -> Vec<String> {
        let inner = self.shared.lock();
        let g = inner.exec.as_ref().map(|e| e.runtime().clone());
        match g {
            Some(rt) => inner.conflicts.report(|p| rt.memory().virtual_of(p)),
            None => inner.conflicts.report(|_| Vec::new()),
        }
    }

    /// Off, saying why, whatever the switch says (lockstep).
    pub fn disabled(mut self, why: impl Into<String>) -> Self {
        self.off_reason = Some(why.into());
        self
    }

    /// Watch guest words ([`WATCH_ENV`]): every change is printed with the
    /// step after which it was seen.
    pub fn watch(mut self, words: &[u32]) -> Self {
        let s = self.shared_mut();
        s.watch = words.to_vec();
        // Printed with the pipeline off too (`print` after this overrides).
        s.print |= !words.is_empty();
        s.inner.get_mut().unwrap_or_else(|e| e.into_inner()).watched = vec![None; words.len()];
        self
    }

    /// The `pipeline: watch` lines so far (tests).
    pub fn watch_lines(&self) -> u32 {
        self.shared.lock().watch_lines
    }

    /// Print `pipeline:` lines once a second or not.
    pub fn print(mut self, on: bool) -> Self {
        self.shared_mut().print = on;
        self
    }

    pub fn stats(&self) -> Stats {
        self.shared.lock().stats.clone()
    }

    /// True while a launched render has not been joined.
    pub fn in_flight(&self) -> bool {
        self.shared.in_flight.load(Ordering::Acquire)
    }

    /// Ask for the next frame to join and run serially (a snapshot, an
    /// agent pause).
    pub fn request_quiesce(&self) {
        self.shared.drain.store(true, Ordering::Release);
    }
}

impl Feature for PipelineRender {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Pipelined frame-end rendering").doc("docs/graphics/pipelined-render.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        let off = if !b.switches().feature(SWITCH, self.on) {
            true
        } else if let Some(why) = &self.off_reason {
            eprintln!("pipeline: off: {why}");
            true
        } else {
            false
        };
        if off {
            // The watch alone, at each frame, for the switched-off run to
            // compare with.
            if !self.shared.watch.is_empty() {
                let me = Arc::clone(&self.shared);
                b.raw()
                    .observe_before(&points::RENDER_FRAME, "watch (pipelining off)", move |o| {
                        watch_now(&me, o.runtime, "the game thread ran (pipelining off)")
                    });
            }
            return Ok(());
        }
        if b.switches().feature("render-census", false) {
            eprintln!("pipeline: off: render-census watches the render scope on the game thread");
            return Ok(());
        }
        let stack = b.guest_alloc(STACK_LEN, Permissions::RW)?;
        self.shared
            .stack_top
            .store(stack + STACK_LEN, Ordering::Relaxed);
        let me = Arc::clone(&self.shared);
        b.raw().implement(
            &points::RENDER_FRAME,
            "pipelined frame",
            move |cpu, ctx, addr| frame(&me, cpu, ctx, addr),
        );
        let me = Arc::clone(&self.shared);
        b.snapshot(SnapshotPolicy::forbid_while(move || {
            me.in_flight.load(Ordering::Acquire).then(|| {
                me.drain.store(true, Ordering::Release);
                "a pipelined render is in flight (the next frame joins it)".to_string()
            })
        }));
        Ok(())
    }
}

impl Shared {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

type Cont = Box<dyn FnOnce(&mut CpuAbi, &mut ExecCtx) -> Result<TransferAbi, GuestFault> + Send>;

/// The override at `render_frame_and_present`.
fn frame(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    addr: u32,
) -> Result<TransferAbi, GuestFault> {
    let nested = {
        let inner = me.lock();
        (inner.preparing.is_some() || me.resuming.load(Ordering::Acquire))
            .then(|| Arc::clone(&inner.setup.as_ref().expect("active frame").original))
    };
    if let Some(original) = nested {
        return Ok(original.invoke(cpu, ctx, addr));
    }
    me.game_thread.store(ctx.thread_id, Ordering::Relaxed);
    if let Err(e) = setup(me, ctx, addr) {
        if !me.setup_failed.swap(true, Ordering::Relaxed) {
            eprintln!("pipeline: off: {e}");
        }
        let original = ctx
            .runtime()
            .resolve_original(addr)
            .expect("the plan installs this point over a generated original");
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let me2 = Arc::clone(me);
    join_then(
        me,
        cpu,
        ctx,
        Box::new(move |cpu, ctx| decide(&me2, cpu, ctx, addr)),
    )
}

/// Makes the executor, the render thread and the game-thread wrappers, once.
fn setup(me: &Arc<Shared>, ctx: &mut ExecCtx, addr: u32) -> Result<(), String> {
    let mut inner = me.lock();
    if inner.setup.is_some() {
        return Ok(());
    }
    let rt = Arc::clone(ctx.runtime());
    let original = rt
        .resolve_original(addr)
        .ok_or_else(|| format!("{addr:#010x} has no generated original"))?;
    let allow = Arc::clone(&me.allow);
    let a = me.addrs.clone();
    // Functions to refuse on the render thread (a bisecting aid): wrapped
    // in an override of their own on the game thread, which the fork then
    // refuses (a new override carries no context-only mark).
    let refused: Vec<_> = me
        .refuse
        .iter()
        .map(|&at| (at, rt.resolve_original(at)))
        .collect();
    rt.edit_dispatch(|d| {
        for (at, original) in refused {
            let Some(prev) = d.override_handler(at).or(original) else {
                continue;
            };
            let prev = Arc::new(prev);
            d.register_override(
                at,
                format!("pipeline: refused on the render thread ({REFUSE_ENV})"),
                move |cpu, ctx, sel| Ok(prev.invoke(cpu, ctx, sel)),
            );
        }
    });
    let refuse = me.refuse.clone();
    let mut exec = SecondExecutor::new(&rt, |at, name| {
        // Plan chains (render-loop's ports, render-timing's observers) are
        // refused: they keep Rust continuations on the executor's context,
        // which a fallback could not carry over to the game thread.
        !refuse.contains(&at) && (allow(at, name) || at == addr || at == a.frame_wait)
    });
    exec.max_transfers = u64::MAX;
    for w in &me.shared_watches {
        exec.share_watch(w);
    }
    // On the render thread: `0x00223240` is the original, and the frame
    // wait already ran on the game thread.
    let render_original = rt.resolve_original(addr).expect("checked above");
    exec.runtime().edit_dispatch(|d| {
        d.register_override(
            addr,
            "pipeline: render_frame_and_present",
            move |cpu, ctx, sel| Ok(render_original.invoke(cpu, ctx, sel)),
        );
        d.register_override(
            a.frame_wait,
            "pipeline: frame wait (done at the launch)",
            |cpu, ctx, _| {
                cpu.eax = 0;
                ctx.memory().ret(cpu, 0)
            },
        );
        // The I/O pool's waits and releases: the render carries on as if
        // each succeeded at once, and the game thread makes them at the
        // join, in order (`replay`). Any other call of these is refused as
        // before (a service).
        if me.defer_io && !a.io_handles.is_empty() {
            for &(ordinal, words, null, out) in IO_CALLS {
                let token = runtime::kernel::function_token(ordinal);
                let Some(refuse) = d.override_handler(token) else {
                    continue;
                };
                let me = Arc::clone(me);
                let handles = a.io_handles;
                d.register_override(
                    token,
                    format!("pipeline: an I/O pool call deferred to the join ({ordinal})"),
                    move |cpu, ctx, sel| {
                        let mut m = ctx.memory();
                        let args: Option<Vec<u32>> = (0..words)
                            .map(|i| m.u32(cpu.esp.wrapping_add(4 + 4 * i)).ok())
                            .collect();
                        let pool = args.as_ref().is_some_and(|args| {
                            args[0] != 0
                                && handles.iter().any(|&h| m.u32(h).ok() == Some(args[0]))
                                && null.is_none_or(|i| args[i] == 0)
                        });
                        let Some(args) = args.filter(|_| pool) else {
                            drop(m);
                            return Ok(refuse.invoke(cpu, ctx, sel));
                        };
                        {
                            let mut inner = me.lock();
                            if inner.pool == PoolUse::Sim {
                                // The sim used the pool first: this call is
                                // made at the join against what it left.
                                inner.stats.io_refused += 1;
                                drop(inner);
                                drop(m);
                                return Ok(refuse.invoke(cpu, ctx, sel));
                            }
                            inner.pool = PoolUse::Render;
                        }
                        let out = match out {
                            Some(i) if args[i] != 0 => {
                                m.set_u32(args[i], 0)?;
                                Some((i, 0))
                            }
                            _ => None,
                        };
                        cpu.eax = 0;
                        {
                            let mut inner = me.lock();
                            inner.stats.io_deferred += 1;
                            inner.deferred.push(Deferred {
                                ordinal,
                                args,
                                eax: 0,
                                out,
                            });
                        }
                        m.ret(cpu, 4 * words)
                    },
                );
            }
        }
    });
    // On the game thread: the sim's D3D calls join the render first; the
    // frame wait is skipped while a fallback finishes a render here.
    let generated_wait = rt.resolve_original(a.frame_wait);
    let generated_caches: Vec<_> = me
        .addrs
        .shared_caches
        .iter()
        .map(|c| rt.resolve_original(c.entry))
        .collect();
    let boundary = a.draw_boundary.map(|b| (b, rt.resolve_original(b.entry)));
    let scene_exec = a.scene.map(|layout| scene::executor(&rt, layout));
    let scene_entries: Vec<_> = a
        .scene
        .into_iter()
        .flat_map(|s| s.entries)
        .filter_map(|&entry| rt.resolve_original(entry).map(|original| (entry, original)))
        .collect();
    rt.edit_dispatch(|d| {
        // A pool wait on the game thread while a render that has used the
        // pool is in flight joins it first: the render's deferred pool
        // calls (and its requests) come before the tick's. A render that
        // has not used the pool is left running: the sim owns the pool for
        // the rest of the frame (`PoolUse`).
        let wait = runtime::kernel::function_token(IO_CALLS[0].0);
        if me.defer_io
            && !a.io_handles.is_empty()
            && let Some(prev) = d.override_handler(wait)
        {
            let name = format!(
                "{} (pipeline: an I/O pool wait joins first)",
                d.override_name(wait).unwrap_or("NtWaitForSingleObjectEx")
            );
            let me = Arc::clone(me);
            let prev = Arc::new(prev);
            d.register_override(wait, name, move |cpu, ctx, sel| {
                if !me.in_flight.load(Ordering::Acquire) {
                    return Ok(prev.invoke(cpu, ctx, sel));
                }
                let pool = {
                    let g = ctx.memory();
                    let h = g.u32(cpu.esp.wrapping_add(4)).unwrap_or(0);
                    h != 0
                        && me
                            .addrs
                            .io_handles
                            .iter()
                            .any(|&w| g.u32(w).ok() == Some(h))
                };
                if !pool {
                    return Ok(prev.invoke(cpu, ctx, sel));
                }
                let game = ctx.thread_id == me.game_thread.load(Ordering::Relaxed);
                {
                    let mut inner = me.lock();
                    match inner.pool {
                        PoolUse::Render if game => inner.io_stalled_join = true,
                        PoolUse::Render => {
                            inner.stats.io_other_thread += 1;
                            drop(inner);
                            return Ok(prev.invoke(cpu, ctx, sel));
                        }
                        PoolUse::Free | PoolUse::Sim => {
                            if inner.pool == PoolUse::Free {
                                inner.stats.io_sim_first += 1;
                            }
                            inner.pool = PoolUse::Sim;
                            drop(inner);
                            return Ok(prev.invoke(cpu, ctx, sel));
                        }
                    }
                }
                let t = Instant::now();
                let (me2, prev) = (Arc::clone(&me), Arc::clone(&prev));
                join_then(
                    &me,
                    cpu,
                    ctx,
                    Box::new(move |cpu, ctx| {
                        {
                            let mut inner = me2.lock();
                            inner.stats.io_joins += 1;
                            inner.stats.io_stall += t.elapsed();
                            inner.stats.io_stall_join += t.elapsed();
                        }
                        Ok(prev.invoke(cpu, ctx, sel))
                    }),
                )
            });
        }
        for &entry in &me.d3d_entries {
            let Some(prev) = d.override_handler(entry) else {
                continue;
            };
            let name = format!(
                "{} (pipeline: joins first)",
                d.override_name(entry).unwrap_or("d3d")
            );
            let me = Arc::clone(me);
            let prev = Arc::new(prev);
            d.register_override(entry, name, move |cpu, ctx, sel| {
                if !me.in_flight.load(Ordering::Acquire) {
                    return Ok(prev.invoke(cpu, ctx, sel));
                }
                if ctx.thread_id != me.game_thread.load(Ordering::Relaxed) {
                    me.lock().stats.d3d_other_thread += 1;
                    return Ok(prev.invoke(cpu, ctx, sel));
                }
                me.lock().stats.d3d_joins += 1;
                let prev = Arc::clone(&prev);
                join_then(
                    &me,
                    cpu,
                    ctx,
                    Box::new(move |cpu, ctx| Ok(prev.invoke(cpu, ctx, sel))),
                )
            });
        }
        for (c, generated) in me.addrs.shared_caches.iter().zip(generated_caches) {
            let Some(prev) = d.override_handler(c.entry).or(generated) else {
                continue;
            };
            let name = format!(
                "{} (pipeline: an allocation joins first)",
                d.override_name(c.entry).unwrap_or("shared cache")
            );
            let (me, c) = (Arc::clone(me), *c);
            let prev = Arc::new(prev);
            d.register_override(c.entry, name, move |cpu, ctx, sel| {
                if !me.in_flight.load(Ordering::Acquire)
                    || ctx.thread_id != me.game_thread.load(Ordering::Relaxed)
                {
                    return Ok(prev.invoke(cpu, ctx, sel));
                }
                let allocates = {
                    let g = ctx.memory();
                    let handle = g.u32(cpu.esp.wrapping_add(4)).unwrap_or(u32::MAX);
                    c.would_allocate(&g, handle)
                };
                if !allocates {
                    return Ok(prev.invoke(cpu, ctx, sel));
                }
                me.lock().stats.cache_joins += 1;
                let prev = Arc::clone(&prev);
                join_then(
                    &me,
                    cpu,
                    ctx,
                    Box::new(move |cpu, ctx| Ok(prev.invoke(cpu, ctx, sel))),
                )
            });
        }
        if let Some(layout) = a.scene {
            scene::install(d, me, scene_entries, layout);
        }
        if let Some((b, generated)) = boundary
            && let Some(original) = d.override_handler(b.entry).or(generated)
        {
            let me = Arc::clone(me);
            d.register_override(
                b.entry,
                "pipeline: prepared draw tail",
                move |cpu, ctx, sel| {
                    let at_boundary = ctx.thread_id == me.game_thread.load(Ordering::Relaxed)
                        && cpu.esp.checked_add(b.depth)
                            == Some(me.stack_top.load(Ordering::Relaxed))
                        && {
                            let m = ctx.memory();
                            m.u32(cpu.esp)? == b.return_pc
                                && b.debug_flag_from_esp.is_none_or(|offset| {
                                    m.u8(cpu.esp.wrapping_add(offset)).ok() == Some(0)
                                })
                        };
                    if at_boundary && let Some(ret) = prepared_tail(&me, cpu, ctx, b.entry) {
                        return Ok(ret);
                    }
                    Ok(original.invoke(cpu, ctx, sel))
                },
            );
        }
        if let Some(prev) = d.override_handler(a.frame_wait) {
            let name = format!(
                "{} (pipeline: skipped in a fallback)",
                d.override_name(a.frame_wait).unwrap_or("frame wait")
            );
            let me = Arc::clone(me);
            d.register_override(a.frame_wait, name, move |cpu, ctx, sel| {
                if me.resuming.load(Ordering::Acquire) {
                    cpu.eax = 0;
                    return ctx.memory().ret(cpu, 0);
                }
                Ok(prev.invoke(cpu, ctx, sel))
            });
        } else if let Some(prev) = generated_wait {
            // A generated frame wait: wrap it the same way.
            let me = Arc::clone(me);
            {
                d.register_override(
                    a.frame_wait,
                    "pipeline: frame wait",
                    move |cpu, ctx, sel| {
                        if me.resuming.load(Ordering::Acquire) {
                            cpu.eax = 0;
                            return ctx.memory().ret(cpu, 0);
                        }
                        Ok(prev.invoke(cpu, ctx, sel))
                    },
                );
            }
        }
    });
    let top = me.stack_top.load(Ordering::Relaxed);
    let stack_pages = {
        let g = rt.memory();
        let mut v: Vec<u32> = (top - STACK_LEN..top)
            .step_by(0x1000)
            .filter_map(|va| g.fold_address(va).map(|p| p & !CONTIGUOUS_BASE))
            .collect();
        v.sort_unstable();
        v
    };
    let (jobs, rx) = sync_channel::<Work>(1);
    let me_t = Arc::clone(me);
    std::thread::Builder::new()
        .name("h2-pipeline-render".into())
        // Generated code nests host frames for guest direct calls; the game
        // thread is the process's main thread (8 MB). Reserved, not touched.
        .stack_size(64 << 20)
        .spawn(move || {
            while let Ok(work) = rx.recv() {
                let mut job = match work {
                    Work::Draw(job) => job,
                    Work::Scene(job) => {
                        scene::run(job);
                        continue;
                    }
                };
                if me_t.check_snapshot {
                    snapshot::arm(&mut job.exec.runtime().memory());
                }
                if !matches!(job.start, JobStart::Resume) {
                    world_layers::replace(None);
                }
                let started = Instant::now();
                let result = match job.start {
                    JobStart::Frame(entry) => job.exec.call(&mut job.cpu, entry, &[]),
                    JobStart::Prepared => job.exec.enter(&mut job.cpu),
                    JobStart::Resume => job.exec.resume(&mut job.cpu),
                };
                let stopped = Instant::now();
                {
                    let mut inner = me_t.lock();
                    inner.stopped = Some(stopped);
                    inner.render_layer = world_layers::current();
                }
                let footprint = me_t
                    .check_snapshot
                    .then(|| Footprint::take(&mut job.exec.runtime().memory()));
                let _ = job.reply.send(Done {
                    exec: job.exec,
                    result,
                    started,
                    elapsed: stopped - started,
                    footprint,
                });
            }
        })
        .map_err(|e| format!("starting the render thread: {e}"))?;
    inner.exec = Some(Box::new(exec));
    inner.setup = Some(Setup {
        jobs,
        scene: scene_exec,
        scene_dispatch_generation: rt.dispatch_generation(),
        original: Arc::new(original),
        stack_pages,
    });
    Ok(())
}

/// Joins the render in flight, if any, then runs `k`. A fallback runs the
/// rest of the render on this thread first.
fn join_then(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    k: Cont,
) -> Result<TransferAbi, GuestFault> {
    let flight = me.lock().flight.take();
    let Some(flight) = flight else {
        return k(cpu, ctx);
    };
    let t = Instant::now();
    let lead = t - flight.launched;
    let done = flight.rx.recv();
    let wait = t.elapsed();
    me.in_flight.store(false, Ordering::Release);
    let Ok(done) = done else {
        return Ok(TransferAbi::fault(ctx.emit(Event::Unsupported {
            pc: cpu.eip,
            mnemonic: "pipeline".into(),
            detail: "the render thread died".into(),
            backtrace: Vec::new(),
        })));
    };
    {
        let mut inner = me.lock();
        inner.stats.join_wait += wait;
        inner.stats.lead += lead;
        inner.stats.overlap += t.saturating_duration_since(done.started).min(done.elapsed);
    }
    let (exec, result) = accept_done(me, done);
    settle(me, cpu, ctx, exec, result, flight.predicted, None, k)
}

fn accept_done(me: &Shared, done: Done) -> (Box<SecondExecutor>, Result<CallOutcome, ExecutorError>) {
    let mut inner = me.lock();
    inner.stats.render += done.elapsed;
    if let Some(footprint) = done.footprint {
        inner.coverage.add(footprint);
    }
    if let Err(ExecutorError::Fault {
        event: Some(Event::MemoryFault { fault, .. }),
        ..
    }) = &done.result
        && matches!(fault.reason, FaultReason::Unmapped | FaultReason::Protection)
    {
        inner.stats.snapshot_faults += 1;
        eprintln!(
            "pipeline: snapshot fault page={:#010x}: {fault}",
            fault.addr & !0xfff,
        );
    }
    (done.exec, done.result)
}

/// A render run ended: hand it back and run `k` (completed); make its
/// kernel call here and carry it on on the render thread (serviced); or
/// hand everything back and run its rest here (fallback).
#[allow(clippy::too_many_arguments)]
fn settle(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut exec: Box<SecondExecutor>,
    result: Result<CallOutcome, ExecutorError>,
    predicted: u64,
    live: Option<LiveSync>,
    k: Cont,
) -> Result<TransferAbi, GuestFault> {
    let calls = std::mem::take(&mut me.lock().deferred);
    if !calls.is_empty() {
        return replay(me, cpu, ctx, exec, result, predicted, live, k, calls);
    }
    let rt = Arc::clone(ctx.runtime());
    if let Err(ExecutorError::Refused(r)) = &result
        && r.ordinal.is_some()
        && me.serviced.contains(&r.name.as_str())
        && let (Some(at), Some(ret)) = (exec.refused_cpu(), r.return_pc)
    {
        let accesses = exec.last_accesses();
        ctx.charge_accesses(accesses);
        let mut inner = me.lock();
        inner.stats.accesses += accesses;
        *inner
            .stats
            .serviced
            .entry(format!("kernel:{}", r.name))
            .or_default() += 1;
        drop(inner);
        return service(me, cpu, ctx, exec, at, ret, predicted, live, k);
    }
    // The live ranges went to live RAM at each serviced call: their last
    // changes too, and none through the merge.
    let mut live = live;
    if let Some(live) = &mut live {
        live.push(&exec, &rt);
        // One last flush, against what the earlier ones gave live RAM: a
        // byte the render changed and put back after a flush (a view depth
        // pushed across a serviced call and popped after it) holds its
        // snapshot value again, which the hand-back below (against the
        // snapshot) would not see, leaving live RAM with the flushed,
        // pushed value. Everything the render wrote since its first service
        // goes to live RAM here; the hand-back is left the bytes the
        // flushes skip (counters by delta, marks, host-owned bytes). The
        // render stack stays out of both: a fallback copies it whole from
        // the view, which must keep the render's bytes there.
        let mut skip = live.skip.clone();
        {
            let mut inner = me.lock();
            if let Some(s) = &inner.setup {
                skip.extend(s.stack_pages.iter().map(|&p| (p, 0x1000)));
            }
            let h = exec.flush(&rt, &mut live.flushed, &skip);
            inner.stats.bytes += h.bytes as u64;
            inner.stats.conflicts += h.conflicts as u64;
        }
        exec.unflush(&live.flushed, &skip);
        for p in &live.pieces {
            exec.revert(p.physical, p.last.len() as u32);
        }
    }
    let flushed_pages: Vec<u32> = live
        .as_ref()
        .map(|l| l.flushed.keys().copied().collect())
        .unwrap_or_default();
    let t = Instant::now();
    let mut inner = me.lock();
    let stack_pages = inner
        .setup
        .as_ref()
        .map(|s| s.stack_pages.clone())
        .unwrap_or_default();
    let fallback_context = if matches!(&result, Err(ExecutorError::Refused(_))) {
        me.addrs
            .draw_boundary
            .and_then(|b| b.view_context)
            .map(|va| exec.runtime().memory().u32(va).map(|value| (va, value)))
            .transpose()?
    } else {
        None
    };
    let mut portable = false;
    let outcome = match result {
        Ok(out) => {
            inner.stats.completed += 1;
            let (new_pages, first) = inner.coverage.complete();
            inner.stats.new_snapshot_pages += new_pages as u64;
            if let Some(page) = first
                && me.print
            {
                eprintln!("pipeline: snapshot footprint added {new_pages} pages, first={page:#010x}");
            }
            hand_back(
                me,
                &mut inner,
                &mut exec,
                &rt,
                predicted,
                false,
                &stack_pages,
                &flushed_pages,
            );
            ctx.charge_accesses(out.accesses);
            inner.stats.accesses += out.accesses;
            None
        }
        Err(ExecutorError::Refused(r)) => {
            let cause = match r.ordinal {
                Some(_) => format!("kernel:{}", r.name),
                None => r.name.clone(),
            };
            // A Rust continuation on the executor's context (a D3D push
            // callback or a destroy the D3D layer is delivering) holds part
            // of the render's state that the game thread's context cannot
            // take over: the rest of the render cannot run there.
            let live: Vec<&'static str> = exec
                .context()
                .live_continuations()
                .iter()
                .map(|c| c.origin)
                .collect();
            portable = !live.is_empty() && live.iter().all(|&origin| origin == crate::entry::RENDER_ENTRY);
            let unresumable = !live.is_empty() && !portable;
            let cause = if unresumable {
                format!("unresumable:{cause}")
            } else {
                cause
            };
            *inner.stats.fallbacks.entry(cause.clone()).or_default() += 1;
            me.serial_left
                .store(me.addrs.after_fallback, Ordering::Relaxed);
            hand_back(
                me,
                &mut inner,
                &mut exec,
                &rt,
                predicted,
                true,
                &stack_pages,
                &flushed_pages,
            );
            ctx.charge_accesses(exec.last_accesses());
            inner.stats.accesses += exec.last_accesses();
            if unresumable {
                exec.context().clear_continuations();
                Some(Err(format!(
                    "{cause} inside {live:?} on the render thread (set {SWITCH}=off)"
                )))
            } else {
                Some(exec.refused_cpu().ok_or("refused with no CPU".to_string()))
            }
        }
        Err(e) => {
            *inner
                .stats
                .fallbacks
                .entry(format!("error:{}", short(&e)))
                .or_default() += 1;
            hand_back(
                me,
                &mut inner,
                &mut exec,
                &rt,
                predicted,
                true,
                &stack_pages,
                &flushed_pages,
            );
            exec.context().clear_continuations();
            Some(Err(e.to_string()))
        }
    };
    // The rest of the render runs on its stack in live RAM: the view's
    // stack exactly. A byte merge against the snapshot is wrong there once
    // a serviced call has copied the stack to live RAM (a return address
    // equal to the snapshot's would keep live RAM's stale bytes).
    let mut restore_context = None;
    if let Some(Ok(at)) = &outcome {
        if let Some((va, render_context)) = fallback_context {
            let mut live = rt.memory();
            let saved = live.u32(va)?;
            // hand_back excluded the epilogue's clear. At a refusal the
            // callee may still need the pre-clear context from the view.
            live.set_u32(va, render_context)?;
            restore_context = Some((va, saved));
        }
        let top = me.stack_top.load(Ordering::Relaxed);
        copy_stack(&exec.runtime().memory(), &mut rt.memory(), at.esp, top);
    }
    inner.stats.hand_back += t.elapsed();
    let render_layer = inner.render_layer;
    inner.exec = Some(exec);
    drop(inner);
    match outcome {
        None => k(cpu, ctx),
        Some(Ok(at)) => resume(
            me,
            cpu,
            ctx,
            at,
            portable,
            render_layer,
            Box::new(move |cpu, ctx| {
                if let Some((va, value)) = restore_context {
                    ctx.memory().set_u32(va, value)?;
                }
                k(cpu, ctx)
            }),
        ),
        Some(Err(detail)) => Ok(TransferAbi::fault(ctx.emit(Event::Unsupported {
            pc: cpu.eip,
            mnemonic: "pipeline".into(),
            detail: format!("the pipelined render stopped: {detail}"),
            backtrace: Vec::new(),
        }))),
    }
}

/// One page-bounded piece of an [`Addrs::live_bytes`] range, and its bytes
/// as the view last had them in step with live RAM.
struct LivePiece {
    va: u32,
    physical: u32,
    last: Vec<u8>,
}

/// The live ranges of a render that has made serviced calls, and what it
/// has flushed to live RAM.
struct LiveSync {
    pieces: Vec<LivePiece>,
    /// The view's bytes of each page at its last flush.
    flushed: BTreeMap<u32, Box<[u8]>>,
    /// Physical ranges a flush leaves alone: render-private (the marks),
    /// handed back by delta (the counters), never handed back (host-owned,
    /// the launch clear) or synced on their own (the live ranges).
    skip: Vec<(u32, u32)>,
}

impl LiveSync {
    /// Starts from the snapshot's bytes.
    fn new(a: &Addrs, exec: &SecondExecutor, rt: &Runtime) -> LiveSync {
        let g = rt.memory();
        let base = exec.base().map(|b| b.bytes());
        let mut live = Vec::new();
        for &range in a.live_bytes {
            for (va, physical, len) in va_pieces(&g, range) {
                let last = base
                    .and_then(|b| b.get(physical as usize..(physical + len) as usize))
                    .map(<[u8]>::to_vec);
                if let Some(last) = last {
                    live.push(LivePiece { va, physical, last });
                }
            }
        }
        let mut skip = Vec::new();
        for &r in a.mark_bytes.iter().chain(a.host_owned).chain(a.live_bytes) {
            skip.extend(pieces(&g, r));
        }
        for &va in a.mark_counters {
            skip.extend(pieces(&g, (va, va + 4)));
        }
        skip.extend(pieces(&g, (a.present_counter, a.present_counter + 8)));
        skip.extend(pieces(&g, (a.frame_counter, a.frame_counter + 4)));
        skip.extend(pieces(&g, (a.launch_clear, a.launch_clear + 1)));
        if let Some(va) = a.draw_boundary.and_then(|b| b.view_context) {
            skip.extend(pieces(&g, (va, va + 4)));
        }
        for va in random_words(a, &g) {
            skip.extend(pieces(&g, (va, va + 4)));
        }
        LiveSync {
            pieces: live,
            flushed: BTreeMap::new(),
            skip,
        }
    }

    /// The bytes the render changed since the last sync, to live RAM.
    fn push(&self, exec: &SecondExecutor, rt: &Runtime) {
        let view = exec.runtime().memory();
        let mut live = rt.memory();
        for p in &self.pieces {
            let mut v = vec![0u8; p.last.len()];
            let mut l = vec![0u8; p.last.len()];
            if view.read_bytes(p.va, &mut v, 1).is_err()
                || live.read_bytes(p.va, &mut l, 1).is_err()
            {
                continue;
            }
            let mut changed = false;
            for (i, (&v, &last)) in v.iter().zip(&p.last).enumerate() {
                if v != last {
                    l[i] = v;
                    changed = true;
                }
            }
            if changed {
                let _ = live.write_bytes(p.va, &l, 1);
            }
        }
    }

    /// Live RAM's bytes into the view.
    fn pull(&mut self, exec: &SecondExecutor, rt: &Runtime) {
        let live = rt.memory();
        let mut view = exec.runtime().memory();
        for p in &mut self.pieces {
            let mut l = vec![0u8; p.last.len()];
            if live.read_bytes(p.va, &mut l, 1).is_ok() && view.write_bytes(p.va, &l, 1).is_ok() {
                p.last = l;
            }
        }
    }
}

/// Each page-bounded piece of `[start, end)` as (virtual, physical, len).
fn va_pieces(g: &Guest, (start, end): Range) -> Vec<(u32, u32, u32)> {
    let mut out = Vec::new();
    let mut va = start;
    while va < end {
        let next = ((va & !0xfff) + 0x1000).min(end);
        if let Some(p) = physical(g, va) {
            out.push((va, p, next - va));
        }
        va = next;
    }
    out
}

/// `MmSetAddressProtect(base, size, ...)`'s ordinal.
const PROTECT_ORDINAL: u32 = 182;

/// The permissions of each page of `[base, base + size)` in `from` (live
/// RAM after a protection change) onto `to` (the view).
fn copy_protection(from: &Guest, to: &mut Guest, base: u32, size: u32) {
    let first = base & !0xfff;
    let end = (u64::from(base) + u64::from(size)).next_multiple_of(0x1000);
    let mut page = u64::from(first);
    while page < end {
        let va = page as u32;
        if let Some(m) = from.mappings().iter().find(|m| {
            va >= m.virtual_start && u64::from(va) < u64::from(m.virtual_start) + u64::from(m.len)
        }) {
            let _ = to.protect_range(va, 0x1000, m.permissions);
        }
        page += 0x1000;
    }
}

/// Copies the render stack from `esp`'s page to its top, `from` to `to`.
fn copy_stack(from: &Guest, to: &mut Guest, esp: u32, top: u32) {
    let start = (esp & !0xfff).max(top.saturating_sub(STACK_LEN));
    if start >= top {
        return;
    }
    let mut b = vec![0u8; (top - start) as usize];
    if from.read_bytes(start, &mut b, 1).is_ok() {
        let _ = to.write_bytes(start, &b, 1);
    }
}

/// The pool calls the render deferred ([`Addrs::io_handles`]), made here on
/// the game thread in the render's order, then [`settle`] again. What the
/// render wrote so far goes to live RAM first, as before a service: the
/// worker a semaphore release wakes reads the request the render wrote.
#[allow(clippy::too_many_arguments)]
fn replay(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut exec: Box<SecondExecutor>,
    result: Result<CallOutcome, ExecutorError>,
    predicted: u64,
    live: Option<LiveSync>,
    k: Cont,
    mut calls: Vec<Deferred>,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let mut live = live.unwrap_or_else(|| LiveSync::new(&me.addrs, &exec, &rt));
    live.push(&exec, &rt);
    let h = exec.flush(&rt, &mut live.flushed, &live.skip);
    {
        let mut inner = me.lock();
        inner.stats.bytes += h.bytes as u64;
        inner.stats.conflicts += h.conflicts as u64;
    }
    calls.reverse();
    replay_next(me, cpu, ctx, exec, result, predicted, live, k, calls)
}

/// The next deferred call (the last of `calls`), or [`settle`] when none
/// is left. The arguments go below the game thread's stack pointer, an out
/// parameter to a scratch word there; a real result other than the one the
/// render was given is counted ([`Stats::io_mismatches`]).
#[allow(clippy::too_many_arguments)]
fn replay_next(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    exec: Box<SecondExecutor>,
    result: Result<CallOutcome, ExecutorError>,
    predicted: u64,
    mut live: LiveSync,
    k: Cont,
    mut calls: Vec<Deferred>,
) -> Result<TransferAbi, GuestFault> {
    let Some(call) = calls.pop() else {
        // What the calls did to the live ranges (the worker took the
        // request and put it back) is the view's now too, as after a
        // service: the final sync must not push the render's older bytes
        // over it.
        let rt = Arc::clone(ctx.runtime());
        live.pull(&exec, &rt);
        return settle(me, cpu, ctx, exec, result, predicted, Some(live), k);
    };
    let rt = Arc::clone(ctx.runtime());
    let base = cpu.esp.wrapping_sub(0x400) & !0xf;
    let scratch = base + 0x40;
    {
        let mut m = rt.memory();
        for (i, &arg) in call.args.iter().enumerate() {
            let arg = match call.out {
                Some((o, _)) if o == i => scratch,
                _ => arg,
            };
            m.set_u32(base + 4 * i as u32, arg)?;
        }
        m.set_u32(scratch, 0)?;
    }
    let saved = *cpu;
    let mut c = *cpu;
    c.esp = base;
    let me2 = Arc::clone(me);
    let token = runtime::kernel::function_token(call.ordinal);
    let t = {
        let mut mem = rt.memory();
        ctx.call_guest_named(
            &mut c,
            &mut mem,
            token,
            "pipeline: an I/O pool call the render deferred",
            move |cpu, ctx| {
                let done = *cpu;
                *cpu = saved;
                let out_differs = call
                    .out
                    .is_some_and(|(_, given)| ctx.memory().u32(scratch).ok() != Some(given));
                if done.eax != call.eax || out_differs {
                    me2.lock().stats.io_mismatches += 1;
                }
                replay_next(&me2, cpu, ctx, exec, result, predicted, live, k, calls)
            },
        )
    };
    match t {
        Ok(t) if t.kind == abi::CALL => {
            *cpu = c;
            Ok(t)
        }
        other => other,
    }
}

/// A serviced call: the render stopped at kernel call `at` (returning to
/// `ret`). Make the call here, on the game thread's context (the kernel
/// sees the thread the render runs for serially) with the render's stack
/// and live ranges brought over, then carry the render on on its own
/// thread against its snapshot, and settle again. The render never reads
/// what the sim did since the launch.
#[allow(clippy::too_many_arguments)]
fn service(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    mut exec: Box<SecondExecutor>,
    at: CpuAbi,
    ret: u32,
    predicted: u64,
    live: Option<LiveSync>,
    k: Cont,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    if io_call(at.eip).is_some() {
        // The render stopped at a pool call: it has waited since then.
        let mut inner = me.lock();
        inner.io_stalled_render = true;
        if let Some(t) = inner.stopped {
            inner.stats.io_stall += t.elapsed();
            inner.stats.io_stall_render += t.elapsed();
        }
    }
    let top = me.stack_top.load(Ordering::Relaxed);
    let mut live = live.unwrap_or_else(|| LiveSync::new(&me.addrs, &exec, &rt));
    copy_stack(&exec.runtime().memory(), &mut rt.memory(), at.esp, top);
    live.push(&exec, &rt);
    // Everything else the render wrote so far, so another guest thread the
    // call wakes (the I/O worker reading a request's fields and what they
    // point at) sees it.
    if !me.watch.is_empty() {
        let mut inner = me.lock();
        check_watch(
            me,
            &mut inner,
            &rt.memory(),
            "the game thread ran (before a service)",
        );
    }
    let h = exec.flush(&rt, &mut live.flushed, &live.skip);
    {
        let mut inner = me.lock();
        inner.stats.bytes += h.bytes as u64;
        inner.stats.conflicts += h.conflicts as u64;
        if !me.watch.is_empty() {
            check_watch(
                me,
                &mut inner,
                &rt.memory(),
                "a service's stack copy, push and flush",
            );
        }
    }
    // A protection change: its range, to copy to the view after.
    let protect = (at.eip == runtime::kernel::function_token(PROTECT_ORDINAL))
        .then(|| {
            let v = exec.runtime().memory();
            Some((v.u32(at.esp + 4).ok()?, v.u32(at.esp + 8).ok()?))
        })
        .flatten();
    let saved = *cpu;
    // The call as the render made it, its return address replaced by ours.
    let mut c = at;
    c.esp = at.esp.wrapping_add(4);
    let me2 = Arc::clone(me);
    let t = {
        let mut mem = rt.memory();
        ctx.call_guest_named(
            &mut c,
            &mut mem,
            at.eip,
            "pipeline: a kernel call for the render",
            move |cpu, ctx| {
                let done = *cpu;
                *cpu = saved;
                carry_on(
                    &me2, cpu, ctx, exec, at, ret, done, predicted, live, protect, k,
                )
            },
        )
    };
    match t {
        Ok(t) if t.kind == abi::CALL => {
            *cpu = c;
            Ok(t)
        }
        other => other,
    }
}

/// After a serviced call: its results into the view, the render on.
#[allow(clippy::too_many_arguments)]
fn carry_on(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    exec: Box<SecondExecutor>,
    at: CpuAbi,
    ret: u32,
    done: CpuAbi,
    predicted: u64,
    mut live: LiveSync,
    protect: Option<(u32, u32)>,
    k: Cont,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    watch_now(me, &rt, "a serviced kernel call");
    let top = me.stack_top.load(Ordering::Relaxed);
    copy_stack(&rt.memory(), &mut exec.runtime().memory(), at.esp, top);
    live.pull(&exec, &rt);
    if let Some((base, size)) = protect {
        copy_protection(&rt.memory(), &mut exec.runtime().memory(), base, size);
    }
    let mut r = at;
    r.eax = done.eax;
    r.ecx = done.ecx;
    r.edx = done.edx;
    r.esp = done.esp;
    r.eip = ret;
    let jobs = me.lock().setup.as_ref().map(|s| s.jobs.clone());
    let (tx, rx) = sync_channel(1);
    let sent = jobs.is_some_and(|j| {
        j.send(Work::Draw(Job {
            exec,
            cpu: r,
            start: JobStart::Resume,
            reply: tx,
        }))
        .is_ok()
    });
    let t = Instant::now();
    let finished = if sent { rx.recv().ok() } else { None };
    if io_call(at.eip).is_some() {
        // The game thread waited for the rest of the render.
        let mut inner = me.lock();
        inner.stats.io_stall += t.elapsed();
        inner.stats.io_stall_render += t.elapsed();
    }
    let Some(done) = finished else {
        return Ok(TransferAbi::fault(ctx.emit(Event::Unsupported {
            pc: cpu.eip,
            mnemonic: "pipeline".into(),
            detail: "the render thread died".into(),
            backtrace: Vec::new(),
        })));
    };
    me.lock().stats.join_wait += t.elapsed();
    let (exec, result) = accept_done(me, done);
    settle(me, cpu, ctx, exec, result, predicted, Some(live), k)
}

fn short(e: &ExecutorError) -> &'static str {
    match e {
        ExecutorError::Refused(_) => "refused",
        ExecutorError::Fault { .. } => "fault",
        ExecutorError::Cancelled => "cancelled",
        ExecutorError::Stack(_) => "stack",
        ExecutorError::NoSnapshot => "no_snapshot",
    }
}

/// A fallback: the render's writes are live (stack included); make the
/// refused call here with the render's CPU, and when the render returns
/// (to the slot its launch pushed `HARNESS_EXIT` into) restore this
/// thread's CPU and run `k`.
fn resume(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    at: CpuAbi,
    portable: bool,
    layer: Option<world_layers::Layer>,
    k: Cont,
) -> Result<TransferAbi, GuestFault> {
    let saved = *cpu;
    let rt = Arc::clone(ctx.runtime());
    let mut c = *cpu;
    c.esp = me.stack_top.load(Ordering::Relaxed);
    me.resuming.store(true, Ordering::Release);
    let previous_layer = world_layers::replace(layer);
    // Both executors may have allocated exactly the same token for
    // different callbacks. Pivot the callback pool with the stack instead
    // of inserting its closures into the live pool or rewriting addresses.
    let was_masked = ctx.interrupts_masked();
    let in_interrupt = portable && ctx.in_interrupt();
    if portable {
        exchange_render_callbacks(me, ctx);
        if in_interrupt {
            ctx.set_interrupts_masked(true);
        }
    }
    let me2 = Arc::clone(me);
    let t = {
        let mut mem = rt.memory();
        ctx.call_guest_named(
            &mut c,
            &mut mem,
            at.eip,
            "pipeline: the rest of a render, serially",
            move |cpu, ctx| {
                if portable {
                    exchange_render_callbacks(&me2, ctx);
                    if in_interrupt {
                        ctx.set_interrupts_masked(was_masked);
                    }
                }
                world_layers::replace(previous_layer);
                me2.resuming.store(false, Ordering::Release);
                *cpu = saved;
                k(cpu, ctx)
            },
        )
    };
    match t {
        Ok(t) if t.kind == abi::CALL => {
            *cpu = at;
            Ok(TransferAbi::jump(at.eip))
        }
        other => {
            if portable {
                exchange_render_callbacks(me, ctx);
                if in_interrupt {
                    ctx.set_interrupts_masked(was_masked);
                }
            }
            world_layers::replace(previous_layer);
            me.resuming.store(false, Ordering::Release);
            *cpu = saved;
            other
        }
    }
}

/// The render executor is idle for the entire serial continuation. Its
/// suspended stack owns this pool until the outer render return fires.
fn exchange_render_callbacks(me: &Shared, ctx: &mut ExecCtx) {
    let mut inner = me.lock();
    let exec = inner.exec.as_mut().expect("joined render executor");
    ctx.swap_callback_stack(exec.context());
}

fn physical(g: &Guest, va: u32) -> Option<u32> {
    g.fold_address(va).map(|p| p & !CONTIGUOUS_BASE)
}

/// Each page-bounded piece of `[start, end)` as (physical, len).
fn pieces(g: &Guest, (start, end): Range) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut va = start;
    while va < end {
        let next = ((va & !0xfff) + 0x1000).min(end);
        if let Some(p) = physical(g, va) {
            out.push((p, next - va));
        }
        va = next;
    }
    out
}

fn read_phys(bytes: &[u8], p: u32, n: usize) -> u64 {
    let mut b = [0u8; 8];
    let p = p as usize;
    if p + n <= bytes.len() {
        b[..n].copy_from_slice(&bytes[p..p + n]);
    }
    u64::from_le_bytes(b)
}

/// Hands the render's writes back to live RAM. `fallback`: the rest of the
/// render will run live, so the stack and the marks go back too.
#[allow(clippy::too_many_arguments)]
fn hand_back(
    me: &Shared,
    inner: &mut Inner,
    exec: &mut SecondExecutor,
    rt: &Runtime,
    predicted: u64,
    fallback: bool,
    stack_pages: &[u32],
    flushed: &[u32],
) {
    let a = &me.addrs;
    let render = std::mem::take(&mut inner.io_stalled_render);
    let join = std::mem::take(&mut inner.io_stalled_join);
    inner.stats.io_stalls_render += u64::from(render);
    inner.stats.io_stalls_join += u64::from(join);
    inner.stats.io_stalls += u64::from(render || join);
    if !me.watch.is_empty() {
        check_watch(
            me,
            inner,
            &rt.memory(),
            "the game thread ran (before the hand-back)",
        );
    }
    // Counters by delta, read before anything is reverted.
    let (present, frame) = {
        let g = rt.memory();
        (
            physical(&g, a.present_counter),
            physical(&g, a.frame_counter),
        )
    };
    let (marks, excluded): (Vec<Pair>, Vec<Pair>) = {
        let g = rt.memory();
        let marks = a
            .mark_counters
            .iter()
            .filter_map(|&va| physical(&g, va).map(|p| (va, p)))
            .collect();
        let mut ex: Vec<(u32, u32)> = Vec::new();
        for r in a.host_owned {
            ex.extend(pieces(&g, *r));
        }
        ex.extend(pieces(&g, (a.launch_clear, a.launch_clear + 1)));
        if let Some(va) = a.draw_boundary.and_then(|b| b.view_context) {
            ex.extend(pieces(&g, (va, va + 4)));
        }
        if !fallback {
            for r in a.mark_bytes {
                ex.extend(pieces(&g, *r));
            }
        }
        (marks, ex)
    };
    // (view, base) of each counter.
    let (present_v, frame_v, mark_values) = {
        let view = exec.runtime().memory();
        let view = view.physical_ram();
        let base = exec.base().map(|b| b.bytes());
        let read = |p: Option<u32>, n: usize| -> (u64, u64) {
            match (p, base) {
                (Some(p), Some(base)) => (read_phys(view, p, n), read_phys(base, p, n)),
                _ => (0, 0),
            }
        };
        let marks: Vec<(u32, (u64, u64))> = marks
            .iter()
            .map(|&(va, p)| (va, read(Some(p), 4)))
            .collect();
        (read(present, 8), read(frame, 4), marks)
    };
    // The random seeds: (va, physical, view, snapshot).
    let seeds: Vec<(u32, u32, u32, u32)> = {
        let g = rt.memory();
        let view = exec.runtime().memory();
        let base = exec.base().map(|b| b.bytes());
        random_words(a, &g)
            .into_iter()
            .filter_map(|va| {
                let p = physical(&g, va)?;
                let base = read_phys(base?, p, 4) as u32;
                Some((va, p, read_phys(view.physical_ram(), p, 4) as u32, base))
            })
            .collect()
    };
    let present_delta = present_v.0.wrapping_sub(present_v.1);
    let frame_delta = (frame_v.0 as u32).wrapping_sub(frame_v.1 as u32);
    let frames_predicted = me.frames_predicted.load(Ordering::Relaxed);
    let frames_wrong = me.predict_frames && frame_delta != frames_predicted;
    if !fallback && (present_delta != predicted || frames_wrong) {
        inner.stats.mispredicts += 1;
    }
    if !fallback {
        // What the next launch predicts: as many views as this frame.
        me.frames_next.store(frame_delta, Ordering::Relaxed);
    }
    for (p, n) in excluded {
        exec.revert(p, n);
    }
    for &(_, p, _, _) in &seeds {
        exec.revert(p, 4);
    }
    for r in [present.map(|p| (p, 8)), frame.map(|p| (p, 4))]
        .into_iter()
        .flatten()
        .chain(marks.iter().map(|&(_, p)| (p, 4)))
    {
        exec.revert(r.0, r.1);
    }
    let mut written: Vec<WrittenPage> = exec.written();
    // Pages flushed earlier and not stored to since: what the flushes left
    // out (the marks, the counters) is still to come back.
    if !flushed.is_empty()
        && let Some(base) = exec.base()
    {
        let view = exec.runtime().memory();
        for &p in flushed {
            if !written.iter().any(|w| w.physical == p) {
                written.push(WrittenPage {
                    physical: p,
                    lines: view.changed_lines(base, p),
                });
            }
        }
    }
    if !fallback {
        written.retain(|w| stack_pages.binary_search(&w.physical).is_err());
    }
    if me.report_conflicts {
        let found = exec.conflicts(rt, &written, 1 << 16);
        let t = &mut inner.conflicts;
        t.add(&found);
        t.hand_backs += 1;
        if t.hand_backs.is_multiple_of(CONFLICT_REPORT_EVERY) {
            let g = rt.memory();
            for line in t.report(|p| g.virtual_of(p)) {
                eprintln!("{line}");
            }
            drop(g);
            *t = ConflictTally::default();
        }
    }
    let owned: Vec<(u32, u32)> = {
        let g = rt.memory();
        me.render_owned
            .iter()
            .flat_map(|&r| pieces(&g, r))
            .map(|(p, n)| (p, p + n))
            .collect()
    };
    let h = exec.hand_back_owned(rt, &written, &owned);
    inner.stats.owned_conflicts += h.owned_conflicts as u64;
    inner.stats.pages += written.iter().filter(|w| w.lines != 0).count() as u64;
    inner.stats.bytes += h.bytes as u64;
    inner.stats.conflicts += h.conflicts as u64;
    let mut g = rt.memory();
    if let Ok(v) = g.u64(a.present_counter) {
        let _ = g.set_u64(
            a.present_counter,
            v.wrapping_add(present_delta).wrapping_sub(predicted),
        );
    }
    // The launch added the predicted views already, so the sim's next tick
    // read the count a serial run would have.
    if frame_delta != frames_predicted
        && let Ok(v) = g.u32(a.frame_counter)
    {
        let _ = g.set_u32(
            a.frame_counter,
            v.wrapping_add(frame_delta).wrapping_sub(frames_predicted),
        );
    }
    me.frames_predicted.store(0, Ordering::Relaxed);
    // The render's random steps on top of the sim's: no draw replayed, and
    // the steps add up to the serial run's.
    for (va, _, view, base) in seeds {
        if a.draw_boundary.is_some() || view == base {
            continue;
        }
        let Ok(live) = g.u32(va) else { continue };
        let next = match random_steps(base, view) {
            Some(n) => random_step(live, n),
            None => view,
        };
        let _ = g.set_u32(va, next);
    }
    // The visit counters by delta, as the serial run counts: live RAM must
    // never be behind a stamp the render handed back. Some stamps live
    // outside the render-private ranges (each light record's at `+0x0c`,
    // counted by `0x004e0308`): with the counter left at the sim's value,
    // the next render counted to the stamps the last one left, found every
    // light visited and drew no dynamic light at all. At a fallback the
    // rest of the render runs live, so it is also never behind the view.
    for (va, (view, base)) in mark_values {
        let Ok(live) = g.u32(va) else { continue };
        let mut next = live.wrapping_add((view as u32).wrapping_sub(base as u32));
        if fallback && (view as u32).wrapping_sub(next) as i32 > 0 {
            next = view as u32;
        }
        let _ = g.set_u32(va, next);
    }
    if !me.watch.is_empty() {
        let stage = if fallback {
            "the hand-back before a fallback"
        } else {
            "the hand-back"
        };
        check_watch(me, inner, &g, stage);
    }
}

fn coverage_reason(me: &Shared, guest: &Guest) -> Option<Serial> {
    match me.lock().coverage.check(guest) {
        Err(snapshot::Gap::Mapping(page)) => {
            eprintln!("pipeline: serial snapshot_mapping page={page:#010x}");
            Some(Serial::SnapshotMapping)
        }
        Err(snapshot::Gap::Trace) => {
            eprintln!("pipeline: serial snapshot_trace (access log overflow)");
            Some(Serial::SnapshotTrace)
        }
        Ok(()) => None,
    }
}

/// After the join: serial, or frame wait, snapshot and launch.
fn decide(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    addr: u32,
) -> Result<TransferAbi, GuestFault> {
    let rt = Arc::clone(ctx.runtime());
    let reason = if me.drain.swap(false, Ordering::AcqRel) {
        Some(Serial::Quiesce)
    } else if me.resuming.load(Ordering::Acquire) {
        Some(Serial::AfterFallback)
    } else {
        serial_reason(&me.addrs, &rt.memory())
    };
    let reason = guard(me, reason, &rt.memory());
    let reason = reason.or_else(|| coverage_reason(me, &rt.memory()));
    watch_now(me, &rt, "the game thread ran (at a frame)");
    if me.print {
        // A line at each switch between serial and pipelined frames, so the
        // last line before a stop says which kind ran.
        let mut inner = me.lock();
        if inner.pipelining != reason.is_none() {
            inner.pipelining = reason.is_none();
            eprintln!(
                "pipeline: {} from here ({} pipelined, {} serial so far)",
                if reason.is_none() {
                    "pipelining"
                } else {
                    "serial"
                },
                inner.stats.pipelined,
                inner.stats.serial.values().sum::<u64>(),
            );
        }
    }
    maybe_print(me);
    if let Some(r) = reason {
        let original = {
            let mut inner = me.lock();
            inner.coverage.reset();
            *inner.stats.serial.entry(r.name()).or_default() += 1;
            inner.setup.as_ref().map(|s| Arc::clone(&s.original))
        };
        let original = original.expect("set up before the first decision");
        return Ok(original.invoke(cpu, ctx, addr));
    }
    if me.addrs.draw_boundary.is_some() {
        return prepare_frame(me, cpu, ctx, addr);
    }
    wait_and_launch(me, cpu, ctx, addr, None)
}

/// Run the actual view traversal in order, on live memory. The only
/// difference in the render stack is its outer return token. This stack
/// stays at the same guest addresses across the handoff, including any
/// pointers to locals carried through registers or guest memory.
fn prepare_frame(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    addr: u32,
) -> Result<TransferAbi, GuestFault> {
    let game_cpu = *cpu;
    let rt = Arc::clone(ctx.runtime());
    let original = Arc::clone(&me.lock().setup.as_ref().expect("set up").original);
    let top = me.stack_top.load(Ordering::Relaxed);
    cpu.esp = top;
    let me2 = Arc::clone(me);
    let call = ctx.call_guest_named(
        cpu,
        &mut rt.memory(),
        addr,
        "pipeline: serial frame preparation",
        move |cpu, ctx| {
            // Keep the dedicated stack reserved if the frame wait yields.
            let tail = me2
                .lock()
                .preparing
                .as_ref()
                .expect("started frame preparation")
                .tail;
            if let Some(tail) = tail {
                *cpu = game_cpu;
                wait_and_launch(&me2, cpu, ctx, addr, Some(tail))
            } else {
                // A different frame path, or a Rust continuation across
                // the boundary. The original finished, including its wait.
                let mut inner = me2.lock();
                inner.preparing = None;
                *inner.stats.serial.entry("unsplit_frame").or_default() += 1;
                drop(inner);
                cpu.esp = game_cpu.esp;
                ctx.memory().ret(cpu, 0)
            }
        },
    );
    let call = match call {
        Ok(call) if call.kind == abi::CALL => call,
        other => {
            *cpu = game_cpu;
            return other;
        }
    };
    me.lock().preparing = Some(Preparing {
        token: call.resume_pc,
        continuations: ctx.live_continuations().len(),
        scene_retry: false,
        tail: None,
    });
    Ok(original.invoke(cpu, ctx, addr))
}

fn prepared_tail(
    me: &Shared,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    entry: u32,
) -> Option<TransferAbi> {
    let mut inner = me.lock();
    let preparing = inner.preparing.as_mut()?;
    if preparing.scene_retry
        || preparing.tail.is_some()
        || preparing.continuations != ctx.live_continuations().len()
    {
        return None;
    }
    preparing.tail = Some(CpuAbi { eip: entry, ..*cpu });
    cpu.esp = me.stack_top.load(Ordering::Relaxed);
    Some(TransferAbi::ret(preparing.token))
}

fn wait_and_launch(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    addr: u32,
    tail: Option<CpuAbi>,
) -> Result<TransferAbi, GuestFault> {
    let saved = *cpu;
    let rt = Arc::clone(ctx.runtime());
    let mut mem = rt.memory();
    let ret_pc = mem.u32(cpu.esp)?;
    let me2 = Arc::clone(me);
    ctx.call_guest_named(
        cpu,
        &mut mem,
        me.addrs.frame_wait,
        "pipeline: frame wait before the launch",
        move |cpu, ctx| launch(&me2, cpu, ctx, saved, ret_pc, addr, tail),
    )
}

/// The guards on top of [`serial_reason`]: warm-up after a frame that was
/// not a world frame, a busy I/O queue, frames after a fallback.
fn guard(me: &Shared, reason: Option<Serial>, g: &Guest) -> Option<Serial> {
    let a = &me.addrs;
    if matches!(
        reason,
        Some(Serial::NotWorld | Serial::Loading | Serial::Unreadable)
    ) {
        me.world_run.store(0, Ordering::Relaxed);
        return reason;
    }
    let run = me.world_run.fetch_add(1, Ordering::Relaxed);
    if reason.is_some() {
        return reason;
    }
    if run < a.warmup {
        return Some(Serial::Warmup);
    }
    let left = me.serial_left.load(Ordering::Relaxed);
    if left > 0 {
        me.serial_left.store(left - 1, Ordering::Relaxed);
        return Some(Serial::AfterFallback);
    }
    pool_reason(me, g)
}

fn pool_reason(me: &Shared, g: &Guest) -> Option<Serial> {
    let a = &me.addrs;
    if a.io_queue != 0 && !matches!(g.u32(a.io_queue), Ok(0)) {
        return Some(Serial::IoBusy);
    }
    // With the render's pool calls deferred, the worker must be idle at the
    // launch (every request free): its own pool calls, made while the game
    // thread waits, are not ordered against the render's.
    if me.defer_io
        && let Some((head, link, requests)) = a.io_free
        && free_requests(g, head, link, requests) != Some(requests)
    {
        return Some(Serial::IoBusy);
    }
    None
}

/// The requests on the I/O pool's free list (`None`: unreadable, or longer
/// than the pool).
fn free_requests(g: &Guest, head: u32, link: u32, requests: u32) -> Option<u32> {
    let mut at = g.u32(head).ok()?;
    let mut n = 0;
    while at != 0 {
        n += 1;
        if n > requests {
            return None;
        }
        at = g.u32(at.wrapping_add(link)).ok()?;
    }
    Some(n)
}

/// Why this frame must run serially, read from live memory.
fn serial_reason(a: &Addrs, g: &Guest) -> Option<Serial> {
    let world = (|| -> Result<bool, GuestFault> {
        let w = g.u32(a.engine_globals)?;
        if w == 0 || g.u8(w.wrapping_add(0x1120))? == 0 || g.u16(w.wrapping_add(2))? == 0xffff {
            return Ok(false);
        }
        // 0x00068290: true (skip the world) when its first byte is set, its
        // second clear, and the record it points at is active and not in
        // state 4.
        let (on, off, rec) = a.ui_state;
        if g.u8(on)? != 0 && g.u8(off)? == 0 {
            let r = g.u32(rec)?;
            if g.u32(r.wrapping_add(8))? != 0 && g.u32(r.wrapping_add(0x18))? != 4 {
                return Ok(false);
            }
        }
        Ok(true)
    })();
    match world {
        Err(_) => return Some(Serial::Unreadable),
        Ok(false) => return Some(Serial::NotWorld),
        Ok(true) => {}
    }
    let (c0, c1) = a.cache_countdowns;
    match (g.u32(c0), g.u32(c1)) {
        (Ok(x), Ok(y)) if (x as i32) <= 0 && (y as i32) <= 0 => None,
        (Ok(_), Ok(_)) => Some(Serial::Loading),
        _ => Some(Serial::Unreadable),
    }
}

/// After the frame wait: snapshot, the launch-time remedies, launch, and
/// return to the game.
fn launch(
    me: &Arc<Shared>,
    cpu: &mut CpuAbi,
    ctx: &mut ExecCtx,
    saved: CpuAbi,
    ret_pc: u32,
    addr: u32,
    tail: Option<CpuAbi>,
) -> Result<TransferAbi, GuestFault> {
    let eax = cpu.eax;
    *cpu = saved;
    cpu.eax = eax;
    let rt = Arc::clone(ctx.runtime());
    if let Some(at) = tail {
        me.lock().preparing = None;
        // Cache misses in the live traversal can start I/O or remap a
        // page. Check at the actual snapshot, without repeating the
        // visits, loads, or frame wait if this candidate became serial.
        let reason = if me.drain.swap(false, Ordering::AcqRel) {
            Some(Serial::Quiesce)
        } else {
            let g = rt.memory();
            pool_reason(me, &g)
                .or_else(|| coverage_reason(me, &g))
                .or_else(|| serial_reason(&me.addrs, &g))
        };
        if let Some(reason) = reason {
            let mut inner = me.lock();
            inner.coverage.reset();
            *inner.stats.serial.entry(reason.name()).or_default() += 1;
            drop(inner);
            return resume(
                me,
                cpu,
                ctx,
                at,
                false,
                world_layers::current(),
                Box::new(|cpu, ctx| ctx.memory().ret(cpu, 0)),
            );
        }
    }
    let mut inner = me.lock();
    let Some(mut exec) = inner.exec.take() else {
        drop(inner);
        return Ok(TransferAbi::fault(ctx.emit(Event::Unsupported {
            pc: addr,
            mnemonic: "pipeline".into(),
            detail: "no idle executor at the launch".into(),
            backtrace: Vec::new(),
        })));
    };
    let t = Instant::now();
    drop(inner);
    exec.snapshot(&rt);
    let snap = t.elapsed();
    let predicted = 1;
    {
        let mut g = rt.memory();
        let a = &me.addrs;
        if let Ok(v) = g.u64(a.present_counter) {
            g.set_u64(a.present_counter, v.wrapping_add(predicted))?;
        }
        // The views the render will count, as the present: the sim's next
        // tick reads the frame counter (the particles, `0x0017469d`) and
        // must see the serial run's value.
        let frames = me.frames_next.load(Ordering::Relaxed);
        if tail.is_none()
            && me.predict_frames
            && let Ok(v) = g.u32(a.frame_counter)
        {
            g.set_u32(a.frame_counter, v.wrapping_add(frames))?;
            me.frames_predicted.store(frames, Ordering::Relaxed);
        }
        if tail.is_none() {
            g.set_u8(a.launch_clear, 0)?;
        } else if let Some(context) = a.draw_boundary.and_then(|b| b.view_context) {
            g.set_u32(context, 0)?;
        }
    }
    watch_now(me, &rt, "the frame wait and the launch");
    let (tx, rx) = sync_channel(1);
    let top = me.stack_top.load(Ordering::Relaxed);
    let (render_cpu, start) = if let Some(at) = tail {
        // The game context consumed this continuation. The copied stack
        // returns to the executor harness at exactly the same depth.
        exec.runtime()
            .memory()
            .set_u32(top - 4, runtime::exec::HARNESS_EXIT)?;
        (at, JobStart::Prepared)
    } else {
        (CpuAbi { esp: top, ..saved }, JobStart::Frame(addr))
    };
    let mut inner = me.lock();
    inner.coverage.begin();
    inner.stats.snapshot += snap;
    inner.stats.pipelined += 1;
    let jobs = inner.setup.as_ref().expect("set up").jobs.clone();
    inner.flight = Some(Flight {
        rx,
        predicted,
        launched: Instant::now(),
    });
    inner.pool = PoolUse::Free;
    me.in_flight.store(true, Ordering::Release);
    drop(inner);
    if jobs
        .send(Work::Draw(Job {
            exec,
            cpu: render_cpu,
            start,
            reply: tx,
        }))
        .is_err()
    {
        me.in_flight.store(false, Ordering::Release);
        me.lock().flight = None;
        return Ok(TransferAbi::fault(ctx.emit(Event::Unsupported {
            pc: addr,
            mnemonic: "pipeline".into(),
            detail: "the render thread is gone".into(),
            backtrace: Vec::new(),
        })));
    }
    cpu.esp = saved.esp.wrapping_add(4);
    Ok(TransferAbi::ret(ret_pc))
}

/// `0x485a80,0x4e034c` (hex, `0x` optional; commas or spaces): the words
/// to watch. Unreadable entries are dropped.
fn parse_watch(v: Option<&str>) -> Vec<u32> {
    v.unwrap_or("")
        .split([',', ' '])
        .filter_map(|s| {
            let s = s.trim();
            let s = s.strip_prefix("0x").unwrap_or(s);
            (!s.is_empty())
                .then(|| u32::from_str_radix(s, 16).ok())
                .flatten()
        })
        .collect()
}

/// A watched word as seen: its value (`None`: unmapped) and whether the
/// page that value points at is mapped (readable) in live memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Watched {
    value: Option<u32>,
    target_mapped: bool,
}

impl Watched {
    fn read(g: &Guest, va: u32) -> Watched {
        let value = g.u32(va).ok();
        Watched {
            value,
            target_mapped: value.is_some_and(|v| g.u8(v).is_ok()),
        }
    }

    fn show(self) -> String {
        match self.value {
            None => "unmapped".into(),
            Some(v) => format!(
                "{v:#010x} (its page {})",
                if self.target_mapped {
                    "mapped"
                } else {
                    "unmapped"
                }
            ),
        }
    }
}

/// Compare the watched words with their last values and print the first
/// sight of each, and every change of its value or of whether the page it
/// points at is mapped, as seen after `stage` (a change "before the join"
/// was the game's own, one "after the hand-back" or "after the service" a
/// merge of ours).
fn check_watch(me: &Shared, inner: &mut Inner, g: &Guest, stage: &str) {
    for (i, &va) in me.watch.iter().enumerate() {
        let now = Watched::read(g, va);
        let then = inner.watched[i].replace(now);
        if then == Some(now) {
            continue;
        }
        inner.watch_lines += 1;
        if inner.watch_lines > WATCH_LINES || !me.print {
            continue;
        }
        let pipelined = inner.stats.pipelined;
        let serial = inner.stats.serial.values().sum::<u64>();
        match then {
            None => eprintln!(
                "pipeline: watch {va:#010x} = {} first seen after {stage} (pipelined {pipelined}, serial {serial})",
                now.show()
            ),
            Some(then) => eprintln!(
                "pipeline: watch {va:#010x} {} -> {} after {stage} (pipelined {pipelined}, serial {serial})",
                then.show(),
                now.show()
            ),
        }
    }
}

fn watch_now(me: &Shared, rt: &Runtime, stage: &str) {
    if me.watch.is_empty() {
        return;
    }
    let mut inner = me.lock();
    let mut g = rt.memory();
    if !inner.focused {
        // From here every write over a watched word is printed with its
        // pc and thread as it happens (`trap: focus ...`).
        inner.focused = true;
        g.set_trap_focus(&me.watch);
    }
    check_watch(me, &mut inner, &g, stage);
}

fn maybe_print(me: &Shared) {
    if !me.print {
        return;
    }
    let mut inner = me.lock();
    let now = Instant::now();
    let Some(last) = inner.last_print else {
        inner.last_print = Some(now);
        return;
    };
    if now.duration_since(last) < Duration::from_secs(1) {
        return;
    }
    inner.last_print = Some(now);
    let delta = inner.stats.since(&inner.printed);
    inner.printed = inner.stats.clone();
    drop(inner);
    eprintln!("{}", delta.line());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_pipeline_uses_its_exact_switch() {
        use crate::{Base, Registry, Switches};

        for (spec, legacy, expected) in [
            ("", false, false),
            ("impl.render.pipelined_frame=on", false, true),
            ("impl.render.native=off,impl.render.pipelined_frame=on", false, true),
            ("impl.render.native=on", false, false),
            ("impl.render.pipelined_frame=off", true, false),
            ("impl.render=off", true, true),
        ] {
            let switches = Switches::parse(spec).unwrap();
            assert_eq!(
                PipelineRender::requested(&switches, legacy.then_some("1")),
                expected,
                "{spec}"
            );
            let mut registry = Registry::new(switches);
            registry
                .add(&PipelineRender::with(legacy, Addrs::HALO2))
                .unwrap();
            let plan = registry.build(&Base).unwrap();
            assert_eq!(
                plan.chains.contains_key(&points::RENDER_FRAME.base),
                expected,
                "{spec}"
            );
        }
    }

    #[test]
    fn watch_words_parse_as_hex_with_or_without_0x() {
        assert_eq!(
            parse_watch(Some("0x485a80, 4e034c,zz,,0x10")),
            vec![0x0048_5a80, 0x004e_034c, 0x10]
        );
        assert!(parse_watch(None).is_empty());
    }

    #[test]
    fn a_line_averages_per_pipelined_frame() {
        let s = Stats {
            pipelined: 2,
            completed: 1,
            join_wait: Duration::from_millis(4),
            fallbacks: [("kernel:NtWaitForSingleObject".to_string(), 1)].into(),
            serial: [("loading", 3)].into(),
            ..Stats::default()
        };
        let l = s.line();
        assert!(l.starts_with("pipeline: frames pipelined 2 completed 1 serial [loading 3] fallbacks 1 [kernel:NtWaitForSingleObject 1]"), "{l}");
        assert!(l.contains("join_wait 2.00ms"), "{l}");
    }
}
