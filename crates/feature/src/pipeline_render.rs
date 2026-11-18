//! `pipeline-render`: the render front-end one frame behind the simulation,
//! on a second host thread (docs/graphics/pipelined-render.md, "The
//! prototype"). Off unless `H2_PIPELINE_RENDER=1`.
//!
//! At `render_frame_and_present` (`0x00223240`) on the game thread:
//!
//! 1. **Join** the render launched at the previous entry, if any: wait for
//!    the render thread, then hand its writes back to live RAM (the
//!    [`Addrs`] policy: the render stack dropped, the visit marks kept
//!    render-private, the present and render-frame counters by delta, the
//!    host's vblank words left alone, every other changed byte merged).
//!    The render's guest accesses are charged to the game thread's clock.
//! 2. If the render stopped at a call the render thread may not make (a
//!    kernel call, an HLE override nobody audited), it is a **fallback**:
//!    every write is handed back, the stack included, and the game thread
//!    makes that call itself and runs the rest of the render serially,
//!    from where it stopped, before carrying on. Draws already made are
//!    not made again. Fallbacks are counted by the call's name.
//! 3. **Decide**: frames outside gameplay (the world path of `0x00223240`
//!    not taken, resource-cache countdowns running, a quiesce request) run
//!    the original serially.
//! 4. Otherwise run the **frame wait** `0x0012b2e0` here, **snapshot** guest
//!    RAM, apply the launch-time remedies to live RAM (the present counter
//!    ahead by the predicted present, the one-shot flag `0x004e6098`
//!    cleared), **launch** `0x00223240` on the render thread against the
//!    snapshot (its frame wait a no-op there), and return to the game.
//!
//! The sim's own D3D calls on the game thread join the render first, so the
//! device sees them after the render's. Snapshots join first: the policy
//! reports a render in flight and asks for the next frame to be serial.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use engine::points;
use runtime::executor::{CallOutcome, ExecutorError, SecondExecutor};
use runtime::guest::{Permissions, WrittenPage};
use runtime::watch::CONTIGUOUS_BASE;
use runtime::{CpuAbi, Event, ExecCtx, Guest, GuestFault, Runtime, TransferAbi, abi};

use crate::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

pub const ID: &str = "pipeline-render";
/// `H2_PIPELINE_RENDER=1` turns it on.
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
        warmup: 150,
        after_fallback: 30,
        // `[0x004e7408]`: +0 the game's seed (object updates, `0x001032a0`,
        // which the render reaches), +4 the effects seed (the render's
        // `0x00027be0` and the particles' `0x00174089`).
        random: Some((0x004e_7408, &[0, 4])),
        // The object render-state cache (`0x0003ddd0`): the render takes a
        // record per drawn object (its lights, its lighting, stamped with the
        // frame counter); the effect updates in the tick (`0x001794e0`,
        // `0x0017b5d0`) take one for an effect's object to read its
        // lighting. Objects at `[0x004e0300]` (12-byte records, the body at
        // +8), the record handle at body +0xcc, the cache at `[0x00509434]`
        // (0x100-byte records, the owner at +4).
        shared_caches: &[SharedCache {
            entry: 0x0003_ddd0,
            objects: 0x004e_0300,
            object_stride: 12,
            body_at: 8,
            field: 0xcc,
            cache: 0x0050_9434,
            record_len: 0x100,
            owner: 4,
        }],
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
    /// D3D calls from another guest thread while a render was in flight
    /// (not ordered).
    pub d3d_other_thread: u64,
    pub join_wait: Duration,
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
            d3d_other_thread: self.d3d_other_thread - then.d3d_other_thread,
            join_wait: self.join_wait - then.join_wait,
            hand_back: self.hand_back - then.hand_back,
            snapshot: self.snapshot - then.snapshot,
            render: self.render - then.render,
            pages: self.pages - then.pages,
            bytes: self.bytes - then.bytes,
            conflicts: self.conflicts - then.conflicts,
            owned_conflicts: self.owned_conflicts - then.owned_conflicts,
            mispredicts: self.mispredicts - then.mispredicts,
            accesses: self.accesses - then.accesses,
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
            "pipeline: frames pipelined {} completed {} serial [{}] fallbacks {} [{}] serviced {} [{}] d3d_joins {} cache_joins {} d3d_other_thread {} join_wait {:.2}ms hand_back {:.2}ms snapshot {:.2}ms render {:.2}ms pages {:.1} conflicts {} (render-owned {}) mispredicts {}",
            self.pipelined,
            self.completed,
            serial.join(", "),
            fallbacks,
            fb.join(", "),
            serviced,
            sv.join(", "),
            self.d3d_joins,
            self.cache_joins,
            self.d3d_other_thread,
            ms(self.join_wait, joins),
            ms(self.hand_back, joins),
            ms(self.snapshot, joins),
            ms(self.render, joins),
            self.pages as f64 / joins as f64,
            self.conflicts,
            self.owned_conflicts,
            self.mispredicts,
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
    render: Duration,
}

struct Job {
    exec: Box<SecondExecutor>,
    cpu: CpuAbi,
    /// `Some(entry)`: call it; `None`: carry on from `cpu.eip` after a
    /// serviced call.
    entry: Option<u32>,
    reply: SyncSender<Done>,
}

/// A render launched and not yet joined.
struct Flight {
    rx: Receiver<Done>,
    /// Presents added to live at the launch.
    predicted: u64,
}

/// Built once, at the first frame that pipelines.
struct Setup {
    jobs: SyncSender<Job>,
    original: Arc<runtime::exec::OriginalHandler>,
    /// Physical pages of the render stack, sorted.
    stack_pages: Vec<u32>,
}

struct Inner {
    conflicts: ConflictTally,
    exec: Option<Box<SecondExecutor>>,
    setup: Option<Setup>,
    flight: Option<Flight>,
    stats: Stats,
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
    /// On when `H2_PIPELINE_RENDER=1`.
    pub fn new() -> Self {
        let on = std::env::var(ENV).is_ok_and(|v| v == "1");
        let watch = parse_watch(std::env::var(WATCH_ENV).ok().as_deref());
        let env = |name| std::env::var(name).ok();
        let refuse = parse_watch(env(REFUSE_ENV).as_deref());
        Self::with(on, Addrs::HALO2)
            .watch(&watch)
            .predict_frames(switch(env(PREDICT_FRAMES_ENV).as_deref(), true))
            .replay_seeds(switch(env(SEED_REPLAY_ENV).as_deref(), true))
            .refuse(&refuse)
            .report_conflicts(switch(env(CONFLICTS_ENV).as_deref(), false))
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
                render_owned,
                inner: Mutex::new(Inner {
                    conflicts: ConflictTally::default(),
                    exec: None,
                    setup: None,
                    flight: None,
                    stats: Stats::default(),
                    printed: Stats::default(),
                    last_print: None,
                    pipelining: false,
                    watched: Vec::new(),
                    watch_lines: 0,
                    focused: false,
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
        Manifest::new(ID, "Pipelined render front-end (prototype)")
            .doc("docs/graphics/pipelined-render.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.snapshot(SnapshotPolicy::Stateless);
        let off = if !self.on {
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
    rt.edit_dispatch(|d| {
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
    let (jobs, rx) = sync_channel::<Job>(1);
    std::thread::Builder::new()
        .name("h2-pipeline-render".into())
        // Generated code nests host frames for guest direct calls; the game
        // thread is the process's main thread (8 MB). Reserved, not touched.
        .stack_size(64 << 20)
        .spawn(move || {
            while let Ok(mut job) = rx.recv() {
                let t = Instant::now();
                let result = match job.entry {
                    Some(entry) => job.exec.call(&mut job.cpu, entry, &[]),
                    None => job.exec.resume(&mut job.cpu),
                };
                let _ = job.reply.send(Done {
                    exec: job.exec,
                    result,
                    render: t.elapsed(),
                });
            }
        })
        .map_err(|e| format!("starting the render thread: {e}"))?;
    inner.exec = Some(Box::new(exec));
    inner.setup = Some(Setup {
        jobs,
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
    let done = flight.rx.recv();
    let wait = t.elapsed();
    me.in_flight.store(false, Ordering::Release);
    let Ok(Done {
        exec,
        result,
        render,
    }) = done
    else {
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
        inner.stats.render += render;
    }
    settle(me, cpu, ctx, exec, result, flight.predicted, None, k)
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
    let outcome = match result {
        Ok(out) => {
            inner.stats.completed += 1;
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
            let unresumable = !live.is_empty();
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
                    "{cause} inside {live:?} on the render thread (run without H2_PIPELINE_RENDER)"
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
    if let Some(Ok(at)) = &outcome {
        let top = me.stack_top.load(Ordering::Relaxed);
        copy_stack(&exec.runtime().memory(), &mut rt.memory(), at.esp, top);
    }
    inner.stats.hand_back += t.elapsed();
    inner.exec = Some(exec);
    drop(inner);
    match outcome {
        None => k(cpu, ctx),
        Some(Ok(at)) => resume(me, cpu, ctx, at, k),
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
        j.send(Job {
            exec,
            cpu: r,
            entry: None,
            reply: tx,
        })
        .is_ok()
    });
    let t = Instant::now();
    let finished = if sent { rx.recv().ok() } else { None };
    let Some(Done {
        exec,
        result,
        render,
    }) = finished
    else {
        return Ok(TransferAbi::fault(ctx.emit(Event::Unsupported {
            pc: cpu.eip,
            mnemonic: "pipeline".into(),
            detail: "the render thread died".into(),
            backtrace: Vec::new(),
        })));
    };
    {
        let mut inner = me.lock();
        inner.stats.join_wait += t.elapsed();
        inner.stats.render += render;
    }
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
    k: Cont,
) -> Result<TransferAbi, GuestFault> {
    let saved = *cpu;
    let rt = Arc::clone(ctx.runtime());
    let mut c = *cpu;
    c.esp = me.stack_top.load(Ordering::Relaxed);
    me.resuming.store(true, Ordering::Release);
    let me2 = Arc::clone(me);
    let t = {
        let mut mem = rt.memory();
        ctx.call_guest_named(
            &mut c,
            &mut mem,
            at.eip,
            "pipeline: the rest of a render, serially",
            move |cpu, ctx| {
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
            me.resuming.store(false, Ordering::Release);
            *cpu = saved;
            other
        }
    }
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
        if view == base {
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
    } else {
        serial_reason(&me.addrs, &rt.memory())
    };
    let reason = guard(me, reason, &rt.memory());
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
            *inner.stats.serial.entry(r.name()).or_default() += 1;
            inner.setup.as_ref().map(|s| Arc::clone(&s.original))
        };
        let original = original.expect("set up before the first decision");
        return Ok(original.invoke(cpu, ctx, addr));
    }
    let saved = *cpu;
    let mut mem = rt.memory();
    let ret_pc = mem.u32(cpu.esp)?;
    let me2 = Arc::clone(me);
    ctx.call_guest_named(
        cpu,
        &mut mem,
        me.addrs.frame_wait,
        "pipeline: frame wait before the launch",
        move |cpu, ctx| launch(&me2, cpu, ctx, saved, ret_pc, addr),
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
    if a.io_queue != 0 && !matches!(g.u32(a.io_queue), Ok(0)) {
        return Some(Serial::IoBusy);
    }
    None
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
) -> Result<TransferAbi, GuestFault> {
    let eax = cpu.eax;
    *cpu = saved;
    cpu.eax = eax;
    let rt = Arc::clone(ctx.runtime());
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
        if me.predict_frames
            && let Ok(v) = g.u32(a.frame_counter)
        {
            g.set_u32(a.frame_counter, v.wrapping_add(frames))?;
            me.frames_predicted.store(frames, Ordering::Relaxed);
        }
        g.set_u8(a.launch_clear, 0)?;
    }
    watch_now(me, &rt, "the frame wait and the launch");
    let (tx, rx) = sync_channel(1);
    let mut render_cpu = saved;
    render_cpu.esp = me.stack_top.load(Ordering::Relaxed);
    let mut inner = me.lock();
    inner.stats.snapshot += snap;
    inner.stats.pipelined += 1;
    let jobs = inner.setup.as_ref().expect("set up").jobs.clone();
    inner.flight = Some(Flight { rx, predicted });
    me.in_flight.store(true, Ordering::Release);
    drop(inner);
    if jobs
        .send(Job {
            exec,
            cpu: render_cpu,
            entry: Some(addr),
            reply: tx,
        })
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
