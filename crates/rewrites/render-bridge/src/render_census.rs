//! `render-census`: which guest pages the render front-end writes and reads,
//! and how they meet the simulation's, to decide whether the render
//! front-end can run one frame behind the simulation on its own thread
//! (docs/graphics/pipelined-render.md).
//!
//! Off by default (`H2_FEATURES=render-census`). Observers before and after
//! `render_frame_and_present` (the same pair `render-timing` uses) split
//! the game thread into *render scopes* and the *sim intervals* between
//! them. At each boundary the runtime's access trap (`runtime::trap`) is
//! re-armed on every mapped page for reads and writes, so each scope logs
//! the first read and the first write of every page it touches, with the
//! guest `pc` that made it (0 for the host). From those sets, per frame:
//!
//! - `rw`: pages the render scope wrote, split by who: guest code, our D3D
//!   HLE only (`d3d`: the hit's timeline phase is the D3D layer's), other
//!   host code only (`host`: Rust ports of guest code, the kernel);
//! - `rs`: pages the render scope wrote that the *next* sim interval reads.
//!   For pipelining these are the problem: the sim would read them a frame
//!   early. `rs_core` leaves out the render thread's own stack and pages
//!   only our D3D layer wrote;
//! - `ww`: pages both write (the render's writes would be lost or late);
//! - `sr`: pages the sim interval wrote that the next render scope reads:
//!   what a copy-on-write snapshot has to freeze (expected large).
//!
//! In each region group (`xbe`, `game_state`, `datum`, `world_pool`,
//! `other`) the pages most often in `rs_core` are made *sticky*
//! (`H2_RENDER_CENSUS_LINES` per group, or `all`): the trap keeps their
//! 64-byte line masks per guest thread, so they show whether the sim
//! really reads what the render wrote, before writing it itself
//! (`dep_lines`), or only shares the page.
//!
//! With frame stats on (`H2_FRAME_STATS=1`) hits carry the timeline phase
//! and the running guest thread: that is what tells D3D apart from other
//! host code, and other guest threads (counted per thread, left out of the
//! page sets, `rs_other` when they read what the render wrote) from the
//! game thread in both scopes. Without them every host hit counts as
//! `host` and, in the render scope, a hit in the `cpu` phase as another
//! thread's. The runner's Rust-handler calls (kernel exports, overrides)
//! are counted per scope, and host writes are attributed to the handler
//! they ran in (`runtime::trap`'s HLE census).
//!
//! Every `H2_RENDER_CENSUS_EVERY` frames (default 300) and when a game
//! ends it prints `render-census:` lines and, with `H2_RENDER_CENSUS_OUT`
//! set, rewrites the full per-page table there as TSV.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use engine::points;
use runtime::trap::NO_THREAD;
use runtime::{Guest, HleCall, TrapHit, TrapLog};
use timeline::phase::{self, Phase};

use hooks::{BuildError, Builder, Feature, Manifest};

pub const ID: &str = "render-census";
/// Bytes per line of a sticky page's line masks.
pub const LINE: u32 = 64;
/// How far below the render entry's `esp` counts as the game thread's stack
/// (the XBE's stack commit; docs/lifter/LIFTER.md).
pub const STACK_SPAN: u32 = 0x4_0000;
/// Hits kept per scope before the trap drops (and counts) them.
pub const LOG_CAP: usize = 1 << 21;
/// The D3D library's device and state globals in the XBE
/// (`d3d::layout::GuestLayout::HALO2`: `0x00403cd0`..`0x00408650`).
pub const D3D_GLOBALS: (u32, u32) = (0x0040_3000, 0x0040_9000);
/// Size of the game-state block (docs/engine/engine-map.md, `0x00123b30`).
pub const GAME_STATE_SIZE: u32 = 0x3f_e000;

/// Who made an access.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    /// Generated guest code (the hit has a `pc`).
    Guest,
    /// Our D3D layer (the D3D entries and the push-buffer work under them).
    D3d,
    /// Other host code: Rust ports of guest functions, the kernel, HLE.
    Host,
}

fn phase_of(h: &TrapHit) -> Option<Phase> {
    Phase::ALL.get(h.phase as usize).copied()
}

impl Source {
    pub fn of(h: &TrapHit) -> Source {
        if h.pc != 0 {
            return Source::Guest;
        }
        match phase_of(h) {
            Some(
                Phase::D3d
                | Phase::Decode
                | Phase::Encode
                | Phase::Submit
                | Phase::Wait
                | Phase::RenderWait,
            ) => Source::D3d,
            _ => Source::Host,
        }
    }
}

/// The first access of one kind to a page in a scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct First {
    pub pc: u32,
    pub source: Source,
    /// For the host: the Rust handler it ran in ([`TrapHit::hle`]), or 0.
    pub hle: u32,
}

impl First {
    fn of(h: &TrapHit) -> Self {
        First {
            pc: h.pc,
            source: Source::of(h),
            hle: h.hle,
        }
    }
    /// `0x…` for guest code, else the source.
    pub fn label(self) -> String {
        match self.source {
            Source::Guest => format!("{:#010x}", self.pc),
            Source::D3d => "d3d".into(),
            Source::Host => "host".into(),
        }
    }
}

fn label(f: Option<First>) -> String {
    f.map_or_else(|| "-".into(), First::label)
}

/// Who wrote a page in one scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Writers {
    pub first: Option<First>,
    /// The first guest-code writer, if any.
    pub guest: Option<First>,
    /// The first host (or D3D) writer, if any.
    pub host_first: Option<First>,
    pub d3d: bool,
    pub host: bool,
}

impl Writers {
    pub fn d3d_only(&self) -> bool {
        self.guest.is_none() && !self.host
    }
    pub fn host_only(&self) -> bool {
        self.guest.is_none() && !self.d3d && self.host
    }
}

/// The 64-byte lines of `page` that `[addr, addr + len)` covers.
pub fn line_mask(page: u32, addr: u32, len: u32) -> u64 {
    let lo = (page as u64) << 12;
    let (a, b) = (addr as u64, addr as u64 + len.max(1) as u64);
    let (a, b) = (a.max(lo), b.min(lo + 4096));
    if a >= b {
        return 0;
    }
    let first = (a - lo) / LINE as u64;
    let last = (b - 1 - lo) / LINE as u64;
    (u64::MAX >> (63 - last)) & (u64::MAX << first)
}

/// A sticky page's lines in one scope, for one side (the game thread, or
/// the other guest threads together).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lines {
    pub read: u64,
    pub write: u64,
    /// Read before the same thread wrote them in the scope.
    pub read_first: u64,
}

/// What one scope touched, by virtual page.
#[derive(Debug, Default)]
pub struct ScopeSet {
    pub reads: HashMap<u32, First>,
    pub writes: HashMap<u32, Writers>,
    /// Sticky pages' lines on the game thread.
    pub lines: HashMap<u32, Lines>,
    /// Pages other guest threads read, and their sticky lines.
    pub other_reads: HashMap<u32, First>,
    pub other_lines: HashMap<u32, Lines>,
    /// Hits from other guest threads (left out of the sets above), by
    /// thread ([`NO_THREAD`] when only the phase told).
    pub other_thread: BTreeMap<u32, u64>,
}

/// Whether a hit is another guest thread's than `owner` (the game thread,
/// when known). Without thread ids, in a render scope a hit in the
/// timeline's `cpu` phase is another thread's (the owner is in `render` or
/// a D3D phase); in a sim interval none is.
fn is_other(thread: u32, phase: Option<Phase>, owner: Option<u32>, render: bool) -> bool {
    match owner {
        Some(o) if thread != NO_THREAD => thread != o,
        _ => render && phase == Some(Phase::Guest),
    }
}

impl ScopeSet {
    /// `owner`: the game thread, when frame stats tell; `render`: a render
    /// scope (see [`is_other`]).
    pub fn from_log(log: &TrapLog, owner: Option<u32>, render: bool) -> Self {
        let mut s = ScopeSet::default();
        for h in &log.hits {
            let other = is_other(h.thread, phase_of(h), owner, render);
            if other {
                *s.other_thread.entry(h.thread).or_default() += 1;
            }
            let first_page = h.addr >> 12;
            let last_page = ((h.addr as u64 + h.len.max(1) as u64 - 1) >> 12) as u32;
            for p in first_page..=last_page {
                let f = First::of(h);
                if other {
                    if !h.write {
                        s.other_reads.entry(p).or_insert(f);
                    }
                } else if h.write {
                    let w = s.writes.entry(p).or_default();
                    w.first.get_or_insert(f);
                    match f.source {
                        Source::Guest => {
                            w.guest.get_or_insert(f);
                        }
                        Source::D3d => w.d3d = true,
                        Source::Host => w.host = true,
                    }
                    if f.source != Source::Guest {
                        w.host_first.get_or_insert(f);
                    }
                } else {
                    s.reads.entry(p).or_insert(f);
                }
            }
        }
        for l in &log.lines {
            // A line record has no phase: without thread ids it is the
            // game thread's.
            let other = is_other(l.thread, None, owner, false);
            let e = if other {
                s.other_lines.entry(l.page).or_default()
            } else {
                s.lines.entry(l.page).or_default()
            };
            e.read |= l.read;
            e.write |= l.write;
            e.read_first |= l.read_first;
        }
        s
    }
    pub fn other_hits(&self) -> u64 {
        self.other_thread.values().sum()
    }
}

/// Everything counted for one page.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageStat {
    /// Render scopes that wrote it, and of those: by guest code, by our D3D
    /// layer only, by other host code only.
    pub rw: u64,
    pub rw_guest: u64,
    pub rw_d3d_only: u64,
    pub rw_host_only: u64,
    /// The first render writer seen (guest code first if any wrote it).
    pub render_writer: Option<First>,
    /// The first host or D3D render writer seen, with its Rust handler.
    pub host_writer: Option<First>,
    pub render_read: u64,
    pub render_reader: Option<First>,
    /// Render-written, then read by the next sim interval (the game
    /// thread); of those, outside the stack and not only our D3D layer's.
    pub rs: u64,
    pub rs_core: u64,
    pub rs_reader: Option<First>,
    /// Render-written, then read by another guest thread in the next sim
    /// interval.
    pub rs_other: u64,
    /// Render-written, then written by the next sim interval.
    pub ww: u64,
    pub ww_writer: Option<First>,
    /// Written by a sim interval, then read by the next render scope.
    pub sr: u64,
    pub sr_writer: Option<First>,
    pub sr_reader: Option<First>,
    /// Sticky pages, all on the game thread: lines the render wrote; lines
    /// the sim read before writing them itself (`sr_lines`) and at all
    /// (`sr_lines_any`); render-written lines the next sim read first
    /// (`dep_lines`, the dependency) and at all (`dep_lines_any`).
    pub rw_lines: u64,
    pub sr_lines: u64,
    pub sr_lines_any: u64,
    pub dep_lines: u64,
    pub dep_lines_any: u64,
    pub dep_line_frames: u64,
    /// Lines other guest threads read in sim intervals, and those of them
    /// the render had written.
    pub other_read_lines: u64,
    pub dep_lines_other: u64,
    /// Render scopes the page was sticky in.
    pub sticky_frames: u64,
}

/// Page counts summed over frames (divide by `frames`/`pairs`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Render scopes counted.
    pub frames: u64,
    /// Render scopes followed by a counted sim interval.
    pub pairs: u64,
    pub rw: u64,
    pub rw_guest: u64,
    pub rw_d3d_only: u64,
    pub rw_host_only: u64,
    pub render_read: u64,
    pub sim_written: u64,
    pub sim_read: u64,
    pub rs: u64,
    /// `rs` without stack pages and pages only our D3D layer wrote.
    pub rs_core: u64,
    /// Of `rs_core`, pages that were sticky (have line data).
    pub rs_core_lined: u64,
    /// Render-written pages another guest thread read in the next interval.
    pub rs_other: u64,
    pub ww: u64,
    pub ww_core: u64,
    pub sr: u64,
    /// Hits from other guest threads in render scopes / sim intervals.
    pub other_thread: u64,
    pub sim_other_thread: u64,
    /// Hits by our D3D layer in sim intervals on the game thread: D3D calls
    /// the sim makes, which a render thread would have to order against its
    /// own.
    pub sim_d3d: u64,
    /// Dependency lines on sticky pages: read first, and read at all.
    pub dep_lines: u64,
    pub dep_lines_any: u64,
    pub dropped: u64,
}

impl Totals {
    fn add(&mut self, o: &Totals) {
        self.frames += o.frames;
        self.pairs += o.pairs;
        self.rw += o.rw;
        self.rw_guest += o.rw_guest;
        self.rw_d3d_only += o.rw_d3d_only;
        self.rw_host_only += o.rw_host_only;
        self.render_read += o.render_read;
        self.sim_written += o.sim_written;
        self.sim_read += o.sim_read;
        self.rs += o.rs;
        self.rs_core += o.rs_core;
        self.rs_core_lined += o.rs_core_lined;
        self.rs_other += o.rs_other;
        self.ww += o.ww;
        self.ww_core += o.ww_core;
        self.sr += o.sr;
        self.other_thread += o.other_thread;
        self.sim_other_thread += o.sim_other_thread;
        self.sim_d3d += o.sim_d3d;
        self.dep_lines += o.dep_lines;
        self.dep_lines_any += o.dep_lines_any;
        self.dropped += o.dropped;
    }

    /// The summary line: per-frame page averages.
    pub fn line(&self) -> String {
        let f = |n: u64, d: u64| if d == 0 { 0.0 } else { n as f64 / d as f64 };
        let (fr, pr) = (self.frames, self.pairs);
        format!(
            "frames={} pairs={} per-frame pages: rw={:.1} (guest {:.1} d3d_only {:.1} host_only {:.1}) render_read={:.1} sim_written={:.1} sim_read={:.1} | rs={:.1} rs_core={:.1} ww={:.1} ww_core={:.1} sr={:.1} | other_thread_hits={} sim_d3d_hits={} dropped={} | rs_core_lined={:.1} rs_other={:.1} sim_other_thread_hits={} dep_lines={:.1} dep_lines_any={:.1}",
            fr,
            pr,
            f(self.rw, fr),
            f(self.rw_guest, fr),
            f(self.rw_d3d_only, fr),
            f(self.rw_host_only, fr),
            f(self.render_read, fr),
            f(self.sim_written, pr),
            f(self.sim_read, pr),
            f(self.rs, pr),
            f(self.rs_core, pr),
            f(self.ww, pr),
            f(self.ww_core, pr),
            f(self.sr, fr),
            self.other_thread,
            self.sim_d3d,
            self.dropped,
            f(self.rs_core_lined, pr),
            f(self.rs_other, pr),
            self.sim_other_thread,
            f(self.dep_lines, pr),
            f(self.dep_lines_any, pr),
        )
    }
}

/// Labels for guest addresses. Earlier ranges win; the game thread's stack
/// wins over all.
#[derive(Clone, Debug, Default)]
pub struct Regions {
    ranges: Vec<(u32, u32, String)>,
    stack: Option<(u32, u32)>,
}

impl Regions {
    pub fn add(&mut self, start: u32, end: u32, label: impl Into<String>) {
        if start < end {
            self.ranges.push((start, end, label.into()));
        }
    }
    /// The game thread's stack, from `esp` at the render entry.
    pub fn set_stack(&mut self, esp: u32) {
        let top = (esp & !0xfff).saturating_add(0x2000);
        self.stack = Some(((esp & !0xfff).saturating_sub(STACK_SPAN), top));
    }
    pub fn is_stack(&self, va: u32) -> bool {
        self.stack.is_some_and(|(a, b)| (a..b).contains(&va))
    }
    pub fn classify(&self, va: u32) -> &str {
        if self.is_stack(va) {
            return "stack";
        }
        if let Some((_, _, l)) = self.ranges.iter().find(|(a, b, _)| (*a..*b).contains(&va)) {
            return l;
        }
        match va {
            0x8000_0000..0x8400_0000 => "contiguous",
            0xf000_0000..0xf400_0000 => "write_combined",
            _ => "other",
        }
    }

    /// What the repo knows about Halo 2's memory, read from `g`: the
    /// `extra` ranges first (the push buffer, from the boot), the D3D
    /// globals, the game-state block and the datum arrays in it, the world
    /// pool (tag cache, BSPs, buffers carved from it), the XBE image.
    pub fn scan(g: &Guest, extra: &[(u32, u32, &'static str)]) -> Regions {
        let mut r = Regions::default();
        for &(a, b, l) in extra {
            r.add(a, b, l);
        }
        r.add(D3D_GLOBALS.0, D3D_GLOBALS.1, "d3d_globals");
        if let Ok(base) = g.u32(points::GAME_STATE_BASE.base)
            && base != 0
        {
            let mut block = Vec::new();
            if g.read_append(base, GAME_STATE_SIZE as usize, &mut block, 1)
                .is_ok()
            {
                for (at, name, data, len) in datum_arrays(&block) {
                    let label = format!("datum:{name}");
                    r.add(
                        base + at,
                        base + at + engine::datum::layout::HEADER_SIZE,
                        &label,
                    );
                    r.add(data, data.saturating_add(len), label);
                }
            }
            r.add(base, base.saturating_add(GAME_STATE_SIZE), "game_state");
        }
        let pool = runtime::loader::HALO2_PHYSICAL_ARENA;
        r.add(
            0x8000_0000 | pool.start,
            0x8000_0000 | pool.end,
            "world_pool",
        );
        r.add(
            0xf000_0000 | pool.start,
            0xf000_0000 | pool.end,
            "world_pool",
        );
        let image = runtime::loader::HEADER_BASE;
        if let Ok(size) = g.u32(image + 0x10c) {
            r.add(image, image.saturating_add(size), "xbe_image");
        }
        r
    }
}

/// Datum arrays in a game-state block: (offset, name, data, data length).
pub fn datum_arrays(block: &[u8]) -> Vec<(u32, String, u32, u32)> {
    use engine::datum::{SIGNATURE, layout as l};
    let word = |at: usize| -> Option<u32> {
        block
            .get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let mut out = Vec::new();
    let mut at = 0;
    while at + l::HEADER_SIZE as usize <= block.len() {
        if word(at + l::SIGNATURE as usize) != Some(SIGNATURE) {
            at += 4;
            continue;
        }
        let (max, size, data) = (
            word(at + l::MAXIMUM_COUNT as usize).unwrap_or(0),
            word(at + l::ELEMENT_SIZE as usize).unwrap_or(0),
            word(at + l::DATA as usize).unwrap_or(0),
        );
        let name: String = block[at..at + 32]
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect();
        let len = (max as u64) * (size as u64);
        if max != 0
            && size != 0
            && len <= 0x0400_0000
            && data != 0
            && !name.is_empty()
            && name.chars().all(|c| c.is_ascii_graphic() || c == ' ')
        {
            out.push((at as u32, name, data, len as u32));
        }
        at += l::HEADER_SIZE as usize;
    }
    out
}

/// The sticky-page group a region belongs to, or `None` for pages never
/// made sticky (the stack, our D3D layer's own memory).
pub fn region_group(region: &str) -> Option<&'static str> {
    Some(match region {
        "stack" | "d3d_globals" | "push_buffer" | "d3d_scratch" => return None,
        "xbe_image" => "xbe",
        "game_state" => "game_state",
        r if r.starts_with("datum:") => "datum",
        "world_pool" => "world_pool",
        _ => "other",
    })
}

/// Whether an HLE name is a wait: the kernel's waits, delays and yields.
pub fn is_wait(name: &str) -> bool {
    ["Wait", "Delay", "Yield", "Sleep"]
        .iter()
        .any(|w| name.contains(w))
}

/// Rust-handler calls to one target in one kind of scope.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HleStat {
    /// On the game thread (or with no thread known) / on other threads.
    pub calls: u64,
    pub other_calls: u64,
    /// Calls that ended in a STOP (blocked, yielded or preempted).
    pub stops: u64,
    pub other_stops: u64,
}

/// The census proper: pure, fed the trap's logs at each boundary.
#[derive(Debug, Default)]
pub struct Census {
    pub regions: Regions,
    sticky: Vec<u32>,
    /// The game thread, from the frame stats at the render entry.
    owner: Option<u32>,
    prev_render: Option<(HashMap<u32, Writers>, HashMap<u32, u64>)>,
    last_sim_writes: Option<HashMap<u32, First>>,
    pages: BTreeMap<u32, PageStat>,
    window: Totals,
    run: Totals,
    /// rs_core page-frames and those with line data, by sticky group.
    coverage: BTreeMap<&'static str, (u64, u64)>,
    /// Other-thread hits by (render scope, thread).
    threads: BTreeMap<(bool, u32), u64>,
    /// Rust-handler calls by (render scope, target).
    hle: BTreeMap<(bool, u32), HleStat>,
    /// Render-scope page writes by the host, by (source, Rust handler).
    host_writes: BTreeMap<(Source, u32), u64>,
    names: HashMap<u32, String>,
}

impl Census {
    pub fn sticky(&self) -> &[u32] {
        &self.sticky
    }
    pub fn pages(&self) -> &BTreeMap<u32, PageStat> {
        &self.pages
    }
    /// Since the census started, the open window included.
    pub fn totals(&self) -> Totals {
        let mut t = self.run.clone();
        t.add(&self.window);
        t
    }
    /// The game thread's id (frame stats), for telling other threads apart.
    pub fn set_owner(&mut self, owner: Option<u32>) {
        self.owner = owner;
    }
    pub fn hle(&self) -> &BTreeMap<(bool, u32), HleStat> {
        &self.hle
    }
    /// Rust-handler targets without a name yet.
    pub fn unnamed(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .hle
            .keys()
            .map(|&(_, t)| t)
            .chain(self.host_writes.keys().map(|&(_, t)| t))
            .filter(|t| *t != 0 && !self.names.contains_key(t))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
    pub fn set_name(&mut self, target: u32, name: String) {
        self.names.insert(target, name);
    }
    fn name(&self, target: u32) -> String {
        self.names
            .get(&target)
            .cloned()
            .unwrap_or_else(|| format!("{target:#010x}"))
    }
    fn host_label(&self, f: Option<First>) -> String {
        match f {
            None => "-".into(),
            Some(f) if f.hle != 0 => self.name(f.hle),
            Some(f) => format!("{}:no-handler", f.label()),
        }
    }

    /// Rust-handler calls since the last boundary, made in a render scope
    /// or a sim interval.
    pub fn note_hle(&mut self, render: bool, calls: &[HleCall]) {
        for c in calls {
            let e = self.hle.entry((render, c.target)).or_default();
            let game = match self.owner {
                Some(o) => c.thread == o || c.thread == NO_THREAD,
                None => true,
            };
            if game {
                e.calls += c.calls;
                e.stops += c.stops;
            } else {
                e.other_calls += c.calls;
                e.other_stops += c.stops;
            }
        }
    }

    /// A sim interval ended (the render entry): its log. Pairs it with the
    /// render scope before it.
    pub fn sim_done(&mut self, log: &TrapLog) {
        let s = ScopeSet::from_log(log, self.owner, false);
        let w = &mut self.window;
        w.dropped += log.dropped;
        w.sim_d3d += log
            .hits
            .iter()
            .filter(|h| {
                Source::of(h) == Source::D3d && !is_other(h.thread, phase_of(h), self.owner, false)
            })
            .count() as u64;
        w.sim_other_thread += s.other_hits();
        for (&t, &n) in &s.other_thread {
            *self.threads.entry((false, t)).or_default() += n;
        }
        if let Some((writes, lines)) = self.prev_render.take() {
            w.pairs += 1;
            w.sim_written += s.writes.len() as u64;
            w.sim_read += s.reads.len() as u64;
            for (&p, rw) in &writes {
                let core = !rw.d3d_only() && !self.regions.is_stack(p << 12);
                let stat = self.pages.entry(p).or_default();
                if let Some(&r) = s.reads.get(&p) {
                    w.rs += 1;
                    stat.rs += 1;
                    stat.rs_reader.get_or_insert(r);
                    if core {
                        w.rs_core += 1;
                        stat.rs_core += 1;
                        let lined = self.sticky.contains(&p);
                        w.rs_core_lined += lined as u64;
                        if let Some(g) = region_group(self.regions.classify(p << 12)) {
                            let c = self.coverage.entry(g).or_default();
                            c.0 += 1;
                            c.1 += lined as u64;
                        }
                    }
                }
                if s.other_reads.contains_key(&p) {
                    w.rs_other += 1;
                    stat.rs_other += 1;
                }
                if let Some(sw) = s.writes.get(&p) {
                    w.ww += 1;
                    w.ww_core += core as u64;
                    stat.ww += 1;
                    if let Some(f) = sw.first {
                        stat.ww_writer.get_or_insert(f);
                    }
                }
            }
            for (&p, &rw_lines) in &lines {
                let l = s.lines.get(&p).copied().unwrap_or_default();
                let o = s.other_lines.get(&p).copied().unwrap_or_default();
                let stat = self.pages.entry(p).or_default();
                stat.sr_lines |= l.read_first;
                stat.sr_lines_any |= l.read;
                let dep = rw_lines & l.read_first;
                let dep_any = rw_lines & l.read;
                stat.dep_lines |= dep;
                stat.dep_lines_any |= dep_any;
                stat.dep_line_frames += (dep != 0) as u64;
                stat.other_read_lines |= o.read;
                stat.dep_lines_other |= rw_lines & o.read;
                w.dep_lines += dep.count_ones() as u64;
                w.dep_lines_any += dep_any.count_ones() as u64;
            }
        }
        self.last_sim_writes = Some(
            s.writes
                .iter()
                .filter_map(|(&p, w)| Some((p, w.first?)))
                .collect(),
        );
    }

    /// A render scope ended: its log. Pairs it with the sim interval
    /// before it.
    pub fn render_done(&mut self, log: &TrapLog) {
        let r = ScopeSet::from_log(log, self.owner, true);
        let w = &mut self.window;
        w.frames += 1;
        w.dropped += log.dropped;
        w.other_thread += r.other_hits();
        for (&t, &n) in &r.other_thread {
            *self.threads.entry((true, t)).or_default() += n;
        }
        w.rw += r.writes.len() as u64;
        w.render_read += r.reads.len() as u64;
        for (&p, wr) in &r.writes {
            let stat = self.pages.entry(p).or_default();
            stat.rw += 1;
            if let Some(g) = wr.guest {
                w.rw_guest += 1;
                stat.rw_guest += 1;
                if stat.render_writer.is_none_or(|f| f.source != Source::Guest) {
                    stat.render_writer = Some(g);
                }
            } else if wr.d3d_only() {
                w.rw_d3d_only += 1;
                stat.rw_d3d_only += 1;
            } else if wr.host_only() {
                w.rw_host_only += 1;
                stat.rw_host_only += 1;
            }
            if let Some(f) = wr.first {
                stat.render_writer.get_or_insert(f);
            }
            if let Some(h) = wr.host_first {
                stat.host_writer.get_or_insert(h);
                *self.host_writes.entry((h.source, h.hle)).or_default() += 1;
            }
        }
        for &p in &self.sticky {
            self.pages.entry(p).or_default().sticky_frames += 1;
        }
        let sim = self.last_sim_writes.take();
        for (&p, &f) in &r.reads {
            let stat = self.pages.entry(p).or_default();
            stat.render_read += 1;
            stat.render_reader.get_or_insert(f);
            if let Some(sw) = sim.as_ref().and_then(|s| s.get(&p)) {
                w.sr += 1;
                stat.sr += 1;
                stat.sr_writer.get_or_insert(*sw);
                stat.sr_reader.get_or_insert(f);
            }
        }
        let lines: HashMap<u32, u64> = r.lines.iter().map(|(&p, l)| (p, l.write)).collect();
        for (&p, &l) in &lines {
            self.pages.entry(p).or_default().rw_lines |= l;
        }
        self.prev_render = Some((r.writes, lines));
    }

    /// Makes sticky, in each region group ([`region_group`]), the pages
    /// most often in `rs_core` (render-written, then read by the game
    /// thread, outside the stack and our D3D layer's pages): up to
    /// `per_region` of them, or all with `None`. Returns them.
    pub fn pick_sticky(&mut self, per_region: Option<usize>) -> Vec<u32> {
        let mut by: BTreeMap<&'static str, Vec<(u64, u32)>> = BTreeMap::new();
        for (&p, s) in &self.pages {
            if s.rs_core == 0 {
                continue;
            }
            if let Some(g) = region_group(self.regions.classify(p << 12)) {
                by.entry(g).or_default().push((s.rs_core, p));
            }
        }
        let mut out = Vec::new();
        for (_, mut v) in by {
            v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            out.extend(
                v.into_iter()
                    .take(per_region.unwrap_or(usize::MAX))
                    .map(|(_, p)| p),
            );
        }
        out.sort_unstable();
        self.sticky = out.clone();
        out
    }

    fn top(&self, key: impl Fn(&PageStat) -> u64, n: usize) -> Vec<(u32, &PageStat)> {
        let mut v: Vec<(u32, &PageStat)> = self
            .pages
            .iter()
            .filter(|(_, s)| key(s) != 0)
            .map(|(&p, s)| (p, s))
            .collect();
        v.sort_by(|a, b| key(b.1).cmp(&key(a.1)).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }

    /// The report: the window's summary, the run's, region counts, line
    /// coverage, other threads, Rust-handler calls, host writers and the
    /// top pages. Starts a new window.
    pub fn report(&mut self, top: usize) -> Vec<String> {
        let w = std::mem::take(&mut self.window);
        self.run.add(&w);
        let mut out = vec![
            format!("window {}", w.line()),
            format!("run {}", self.run.line()),
        ];
        let (frames, pairs) = (self.run.frames, self.run.pairs);
        let per = |n: u64, d: u64| if d == 0 { 0.0 } else { n as f64 / d as f64 };
        // Pages in each relation by region, over the run.
        let mut by: BTreeMap<&str, [u64; 4]> = BTreeMap::new();
        for (&p, s) in &self.pages {
            let e = by.entry(self.regions.classify(p << 12)).or_default();
            e[0] += (s.rw != 0) as u64;
            e[1] += (s.rs != 0) as u64;
            e[2] += (s.ww != 0) as u64;
            e[3] += (s.sr != 0) as u64;
        }
        for (region, c) in by {
            out.push(format!(
                "  region {region}: pages ever rw={} rs={} ww={} sr={}",
                c[0], c[1], c[2], c[3]
            ));
        }
        for (g, &(n, lined)) in &self.coverage {
            out.push(format!(
                "  coverage group={g} rs_core/pair={:.1} lined/pair={:.1} ({:.0}%)",
                per(n, pairs),
                per(lined, pairs),
                100.0 * per(lined, n)
            ));
        }
        let mut sticky_by: BTreeMap<&str, usize> = BTreeMap::new();
        for &p in &self.sticky {
            if let Some(g) = region_group(self.regions.classify(p << 12)) {
                *sticky_by.entry(g).or_default() += 1;
            }
        }
        for (g, n) in sticky_by {
            out.push(format!("  sticky group={g} pages={n}"));
        }
        let thread = |t: u32| {
            if t == NO_THREAD {
                "?".to_string()
            } else {
                t.to_string()
            }
        };
        for (&(render, t), &n) in &self.threads {
            let (scope, d) = if render {
                ("render", frames)
            } else {
                ("sim", pairs)
            };
            out.push(format!(
                "  other_thread scope={scope} thread={} hits/frame={:.1}",
                thread(t),
                per(n, d)
            ));
        }
        let mut calls: Vec<(&(bool, u32), &HleStat)> = self.hle.iter().collect();
        calls.sort_by(|a, b| {
            (b.0.0, b.1.calls + b.1.other_calls).cmp(&(a.0.0, a.1.calls + a.1.other_calls))
        });
        let mut shown: BTreeMap<bool, usize> = BTreeMap::new();
        for (&(render, target), st) in calls {
            let name = self.name(target);
            let n = shown.entry(render).or_default();
            if *n >= top && !is_wait(&name) {
                continue;
            }
            *n += 1;
            let (scope, d) = if render {
                ("render", frames)
            } else {
                ("sim", pairs)
            };
            out.push(format!(
                "  hle scope={scope} name={name} target={target:#010x} calls/frame game={:.2} other={:.2} stops game={} other={}{}",
                per(st.calls, d),
                per(st.other_calls, d),
                st.stops,
                st.other_stops,
                if is_wait(&name) { " wait" } else { "" }
            ));
        }
        for (&(source, hle), &n) in &self.host_writes {
            let by = if hle != 0 {
                self.name(hle)
            } else {
                format!("{}:no-handler", First { pc: 0, source, hle }.label())
            };
            out.push(format!(
                "  host_writes scope=render by={by} pages/frame={:.2}",
                per(n, frames)
            ));
        }
        let line = |kind: &str, p: u32, n: u64, a: String, b: String| {
            format!(
                "  {kind} page={:#010x} region={} frames={n}/{frames} {a} {b}",
                p << 12,
                self.regions.classify(p << 12)
            )
        };
        for (p, s) in self.top(|s| s.rs, top) {
            out.push(line(
                "rs",
                p,
                s.rs,
                format!("render_writer={}", label(s.render_writer)),
                format!("sim_reader={}", label(s.rs_reader)),
            ));
        }
        for (p, s) in self.top(|s| s.ww, top) {
            out.push(line(
                "ww",
                p,
                s.ww,
                format!("render_writer={}", label(s.render_writer)),
                format!("sim_writer={}", label(s.ww_writer)),
            ));
        }
        for (p, s) in self.top(|s| s.sr, top) {
            out.push(line(
                "sr",
                p,
                s.sr,
                format!("sim_writer={}", label(s.sr_writer)),
                format!("render_reader={}", label(s.sr_reader)),
            ));
        }
        for (&p, s) in &self.pages {
            if s.dep_lines | s.dep_lines_any | s.dep_lines_other == 0 {
                continue;
            }
            out.push(format!(
                "  lines page={:#010x} region={} rw_lines={:#018x} sr_lines={:#018x} dep_lines={:#018x} ({} of 64) dep_frames={} dep_lines_any={:#018x} dep_lines_other={:#018x}",
                p << 12,
                self.regions.classify(p << 12),
                s.rw_lines,
                s.sr_lines,
                s.dep_lines,
                s.dep_lines.count_ones(),
                s.dep_line_frames,
                s.dep_lines_any,
                s.dep_lines_other,
            ));
        }
        out
    }

    /// The full per-page table. The columns after `dep_line_frames` are
    /// census v2's.
    pub fn tsv(&self) -> String {
        let t = self.totals();
        let mut s = format!(
            "# render-census frames={} pairs={}\npage\tregion\trw\trw_guest\trw_d3d_only\trw_host_only\trender_writer\trender_read\trender_reader\trs\trs_sim_reader\tww\tww_sim_writer\tsr\tsr_sim_writer\tsr_render_reader\trw_lines\tsr_lines\tdep_lines\tdep_line_frames\trs_core\trs_other\thost_writer\tsr_lines_any\tdep_lines_any\tother_read_lines\tdep_lines_other\tsticky_frames\n",
            t.frames, t.pairs
        );
        for (&p, st) in &self.pages {
            s += &format!(
                "{:#010x}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:#018x}\t{:#018x}\t{:#018x}\t{}\t{}\t{}\t{}\t{:#018x}\t{:#018x}\t{:#018x}\t{:#018x}\t{}\n",
                p << 12,
                self.regions.classify(p << 12),
                st.rw,
                st.rw_guest,
                st.rw_d3d_only,
                st.rw_host_only,
                label(st.render_writer),
                st.render_read,
                label(st.render_reader),
                st.rs,
                label(st.rs_reader),
                st.ww,
                label(st.ww_writer),
                st.sr,
                label(st.sr_writer),
                label(st.sr_reader),
                st.rw_lines,
                st.sr_lines,
                st.dep_lines,
                st.dep_line_frames,
                st.rs_core,
                st.rs_other,
                self.host_label(st.host_writer),
                st.sr_lines_any,
                st.dep_lines_any,
                st.other_read_lines,
                st.dep_lines_other,
                st.sticky_frames,
            );
        }
        s
    }
}

/// Feature options, from the environment by [`RenderCensus::new`].
#[derive(Clone, Debug)]
pub struct Options {
    /// Report every this many render frames (`H2_RENDER_CENSUS_EVERY`).
    pub every: u64,
    /// Pages per top list (`H2_RENDER_CENSUS_TOP`).
    pub top: usize,
    /// Sticky pages for line detail per region group
    /// (`H2_RENDER_CENSUS_LINES`: a number, 0 for none, or `all`; default
    /// 64).
    pub lines: Option<usize>,
    /// The TSV (`H2_RENDER_CENSUS_OUT`).
    pub out: Option<std::path::PathBuf>,
    /// Print the report to stderr.
    pub print: bool,
}

impl Options {
    pub fn from_env() -> Self {
        let num = |k: &str, d: u64| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(d)
        };
        Options {
            every: num("H2_RENDER_CENSUS_EVERY", 300).max(1),
            top: num("H2_RENDER_CENSUS_TOP", 20) as usize,
            lines: parse_lines(std::env::var("H2_RENDER_CENSUS_LINES").ok().as_deref()),
            out: std::env::var_os("H2_RENDER_CENSUS_OUT").map(Into::into),
            print: true,
        }
    }
}

/// `H2_RENDER_CENSUS_LINES`: `all` is no budget; a number is the budget per
/// region group; unset or unparsable is 64.
pub fn parse_lines(v: Option<&str>) -> Option<usize> {
    match v.map(str::trim) {
        Some("all") => None,
        Some(n) => Some(n.parse().unwrap_or(64)),
        None => Some(64),
    }
}

/// A Rust handler's name: the kernel export for a kernel token, else the
/// override's registered name.
fn hle_name(rt: &runtime::Runtime, target: u32) -> Option<String> {
    if let Some(ord) = target
        .checked_sub(runtime::kernel::KERNEL_TOKEN_BASE)
        .filter(|v| v % 4 == 0)
        .map(|v| v / 4)
        && let Some(f) = runtime::kernel::function(ord)
    {
        return Some(format!("kernel:{}", f.name));
    }
    rt.read_dispatch(|d| d.override_name(target).map(str::to_owned))
}

/// Names the census's new Rust-handler targets.
fn name_targets(census: &mut Census, rt: &runtime::Runtime) {
    for t in census.unnamed() {
        let name = hle_name(rt, t).unwrap_or_else(|| format!("{t:#010x}"));
        census.set_name(t, name);
    }
}

struct State {
    census: Census,
    armed: bool,
    since_report: u64,
}

/// The feature. See the module docs.
pub struct RenderCensus {
    opts: Options,
    extra: Vec<(u32, u32, &'static str)>,
    state: Arc<Mutex<State>>,
}

impl Default for RenderCensus {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderCensus {
    pub fn new() -> Self {
        Self::with_options(Options::from_env())
    }
    pub fn with_options(opts: Options) -> Self {
        RenderCensus {
            opts,
            extra: Vec::new(),
            state: Arc::new(Mutex::new(State {
                census: Census::default(),
                armed: false,
                since_report: 0,
            })),
        }
    }
    /// Labels `[start, end)` (the boot's push buffer, say). Wins over the
    /// ranges the census finds itself.
    pub fn with_region(mut self, start: u32, end: u32, label: &'static str) -> Self {
        self.extra.push((start, end, label));
        self
    }
    /// Runs `f` on the census (tests, reports).
    pub fn with_census<R>(&self, f: impl FnOnce(&mut Census) -> R) -> R {
        f(&mut self.state.lock().unwrap_or_else(|e| e.into_inner()).census)
    }
    /// Why a machine snapshot must wait (docs/runtime/snapshots.md): from
    /// the first render frame on, the access trap is armed on every mapped
    /// page and its log, sticky pages and the census are host state a
    /// snapshot does not carry. Nothing disarms it, so a run with the
    /// census on cannot be snapshotted after its first frame.
    pub fn snapshot_live(&self) -> Option<String> {
        trap_live(&self.state)
    }
}

/// [`RenderCensus::snapshot_live`] over the shared state.
fn trap_live(state: &Mutex<State>) -> Option<String> {
    let armed = state.lock().unwrap_or_else(|e| e.into_inner()).armed;
    armed.then(|| "the render census's access trap is armed".into())
}

fn report(s: &mut State, opts: &Options) {
    let lines = s.census.report(opts.top);
    if opts.print {
        for l in lines {
            eprintln!("render-census: {l}");
        }
    }
    if let Some(path) = &opts.out {
        let tmp = path.with_extension("tsv.tmp");
        let r = std::fs::write(&tmp, s.census.tsv()).and_then(|_| std::fs::rename(&tmp, path));
        if let Err(e) = r {
            eprintln!("render-census: cannot write {}: {e}", path.display());
        }
    }
}

impl Feature for RenderCensus {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Render front-end memory census")
            .off_by_default()
            .doc("docs/graphics/pipelined-render.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // render-timing opens the frame stats' render scope itself.
        let scope = !b.switches().feature(crate::render_timing::ID, false);
        let (state, opts, extra) = (
            Arc::clone(&self.state),
            self.opts.clone(),
            self.extra.clone(),
        );
        let enter = move |o: &hooks::Obs| {
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            let calls = runtime::trap::take_hle_calls();
            if s.armed {
                s.census.note_hle(false, &calls);
            }
            if s.since_report >= opts.every {
                // Before the memory guard: names come from the dispatch.
                name_targets(&mut s.census, o.runtime);
            }
            let mut g = o.guest();
            if s.armed {
                let log = g.take_trap_hits();
                s.census.sim_done(&log);
            } else {
                g.install_trap(LOG_CAP);
                runtime::trap::set_hle_census(true);
                s.armed = true;
                if !phase::enabled() && opts.print {
                    eprintln!(
                        "render-census: frame stats are off: D3D and other threads are not told apart (set H2_FRAME_STATS=1)"
                    );
                }
            }
            if s.since_report == 0 {
                s.census.regions = Regions::scan(&g, &extra);
            }
            s.census.regions.set_stack(o.cpu.esp);
            // The game thread: the one running the render entry.
            s.census.set_owner(phase::running_guest_thread());
            if s.since_report >= opts.every {
                s.since_report = 0;
                report(&mut s, &opts);
                let sticky = s.census.pick_sticky(opts.lines);
                g.set_sticky_pages(&sticky);
                s.census.regions = Regions::scan(&g, &extra);
                s.census.regions.set_stack(o.cpu.esp);
            }
            // The scan's own reads, and anything else before the scope.
            let _ = g.take_trap_hits();
            g.arm_all(true, true);
            drop(g);
            drop(s);
            let _ = runtime::trap::take_hle_calls();
            if scope {
                phase::scope_begin(Phase::Render);
            }
        };
        let state = Arc::clone(&self.state);
        let leave = move |o: &hooks::Obs| {
            if scope {
                phase::scope_end();
            }
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            if !s.armed {
                return;
            }
            let calls = runtime::trap::take_hle_calls();
            s.census.note_hle(true, &calls);
            let mut g = o.guest();
            let log = g.take_trap_hits();
            s.census.render_done(&log);
            s.since_report += 1;
            g.arm_all(true, true);
        };
        b.raw()
            .observe_before(&points::RENDER_FRAME, "census enter", enter);
        b.raw()
            .observe_after(&points::RENDER_FRAME, "census leave", leave);
        let (state, opts) = (Arc::clone(&self.state), self.opts.clone());
        b.game()
            .on_end("census report", move |_| {
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                if s.census.totals().frames != 0 {
                    report(&mut s, &opts);
                }
            })
            .optional();
        let state = Arc::clone(&self.state);
        b.snapshot(hooks::SnapshotPolicy::forbid_while(move || {
            trap_live(&state)
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::LineRec;

    #[test]
    fn a_snapshot_waits_once_the_trap_is_armed() {
        let c = RenderCensus::with_options(Options {
            every: 300,
            top: 20,
            lines: None,
            out: None,
            print: false,
        });
        assert_eq!(c.snapshot_live(), None);
        c.state.lock().unwrap().armed = true;
        assert_eq!(
            c.snapshot_live().as_deref(),
            Some("the render census's access trap is armed")
        );
    }

    const RENDER: u8 = Phase::Render as u8;
    const D3D: u8 = Phase::D3d as u8;
    const CPU: u8 = Phase::Guest as u8;

    fn hit(addr: u32, write: bool, pc: u32, phase: u8) -> TrapHit {
        TrapHit {
            addr,
            len: 4,
            write,
            pc,
            phase,
            thread: NO_THREAD,
            hle: 0,
        }
    }
    /// A hit on guest thread `thread`.
    fn on(thread: u32, h: TrapHit) -> TrapHit {
        TrapHit { thread, ..h }
    }
    fn log(hits: Vec<TrapHit>) -> TrapLog {
        TrapLog {
            hits,
            dropped: 0,
            lines: Vec::new(),
        }
    }
    /// A sticky page's lines on `thread`.
    fn lines(page_va: u32, thread: u32, read: u64, write: u64, read_first: u64) -> LineRec {
        LineRec {
            page: page_va >> 12,
            thread,
            read,
            write,
            read_first,
            accesses: 1,
        }
    }
    fn with_lines(mut l: TrapLog, recs: Vec<LineRec>) -> TrapLog {
        l.lines = recs;
        l
    }

    const A: u32 = 0x8010_0000;
    const B: u32 = 0x8010_1000;
    const C: u32 = 0x8010_2000;
    const D: u32 = 0x8010_3000;
    const E: u32 = 0x8010_4000;
    const ESP: u32 = 0x7000_0000 - 0x100;

    /// One frame of a synthetic game: the render writes A (guest code), B
    /// (our D3D only), C (host code only) and its stack, reads D; another
    /// thread writes E inside the render scope. The sim then reads A, B and
    /// the stack, writes A and D.
    fn render(c: &mut Census) {
        c.render_done(&log(vec![
            hit(A + 8, true, 0x0001_0000, RENDER),
            hit(B, true, 0, D3D),
            hit(C, true, 0, RENDER),
            hit(ESP - 0x40, true, 0x0001_0004, RENDER),
            hit(D, false, 0x0001_0008, RENDER),
            hit(E, true, 0x0009_0000, CPU),
        ]));
    }
    fn sim(c: &mut Census) {
        c.sim_done(&log(vec![
            hit(A + 8, false, 0x0002_0000, CPU),
            hit(A + 12, true, 0x0002_0004, CPU),
            hit(B, false, 0x0002_0008, CPU),
            hit(ESP - 0x40, false, 0x0002_000c, CPU),
            hit(D, true, 0x0002_0010, CPU),
        ]));
    }

    fn census() -> Census {
        let mut c = Census::default();
        c.regions.set_stack(ESP);
        c
    }

    #[test]
    fn writes_and_reads_are_paired_across_the_boundaries() {
        let mut c = census();
        // A sim interval before the first render scope has nothing to pair.
        sim(&mut c);
        for _ in 0..3 {
            render(&mut c);
            sim(&mut c);
        }
        let t = c.totals();
        assert_eq!((t.frames, t.pairs), (3, 3));
        assert_eq!(t.rw, 3 * 4, "A, B, C and the stack");
        assert_eq!((t.rw_guest, t.rw_d3d_only, t.rw_host_only), (6, 3, 3));
        assert_eq!(t.rs, 3 * 3, "A, B and the stack");
        assert_eq!(
            t.rs_core, 3,
            "only A: B is our D3D's, the stack the thread's"
        );
        assert_eq!((t.ww, t.ww_core), (3, 3));
        assert_eq!(
            t.sr, 3,
            "D, every frame: the sim before the first render wrote it too"
        );
        assert_eq!(t.other_thread, 3);
        let a = &c.pages()[&(A >> 12)];
        assert_eq!((a.rw, a.rw_guest, a.rs, a.ww), (3, 3, 3, 3));
        assert_eq!(a.render_writer.unwrap().label(), "0x00010000");
        assert_eq!(a.rs_reader.unwrap().label(), "0x00020000");
        assert_eq!(a.ww_writer.unwrap().label(), "0x00020004");
        let b = &c.pages()[&(B >> 12)];
        assert_eq!(
            (b.rw_d3d_only, b.render_writer.unwrap().label()),
            (3, "d3d".into())
        );
        assert_eq!(c.pages()[&(C >> 12)].render_writer.unwrap().label(), "host");
        let d = &c.pages()[&(D >> 12)];
        assert_eq!((d.sr, d.rw), (3, 0));
        assert_eq!(d.sr_writer.unwrap().pc, 0x0002_0010);
        assert_eq!(d.sr_reader.unwrap().pc, 0x0001_0008);
        assert!(
            !c.pages().contains_key(&(E >> 12)),
            "another thread's write is not the render's"
        );
    }

    #[test]
    fn without_phases_every_render_hit_counts_and_the_host_is_host() {
        let mut c = census();
        c.render_done(&log(vec![
            hit(B, true, 0, u8::MAX),
            hit(E, true, 0x10, u8::MAX),
        ]));
        c.sim_done(&log(vec![hit(B, true, 0, D3D), hit(C, false, 0, CPU)]));
        let t = c.totals();
        assert_eq!((t.rw, t.rw_host_only, t.other_thread), (2, 1, 0));
        assert_eq!(t.sim_d3d, 1, "the sim's D3D call");
    }

    #[test]
    fn a_guest_write_wins_the_writer_over_a_host_one_first() {
        let mut c = census();
        c.render_done(&log(vec![
            hit(A, true, 0, D3D),
            hit(A + 4, true, 0x77, RENDER),
        ]));
        let a = &c.pages()[&(A >> 12)];
        assert_eq!((a.rw_guest, a.rw_d3d_only), (1, 0));
        assert_eq!(a.render_writer.unwrap().pc, 0x77);
    }

    #[test]
    fn a_dependency_line_is_one_the_sim_read_before_writing_it() {
        let mut c = census();
        render(&mut c);
        sim(&mut c);
        assert_eq!(
            c.pick_sticky(Some(4)),
            vec![A >> 12],
            "rs_core only: not B (our D3D's) nor the stack"
        );
        // The render writes lines 0-2. The sim reads line 5; reads line 1
        // first; writes line 2 and only then reads it.
        c.render_done(&with_lines(
            log(vec![hit(A, true, 1, RENDER)]),
            vec![lines(A, NO_THREAD, 0, 0b111, 0)],
        ));
        c.sim_done(&with_lines(
            log(vec![hit(A + 64, false, 2, CPU)]),
            vec![lines(A, NO_THREAD, 1 << 5 | 0b110, 0b100, 1 << 5 | 0b10)],
        ));
        let a = &c.pages()[&(A >> 12)];
        assert_eq!(a.rw_lines, 0b111);
        assert_eq!(
            (a.sr_lines, a.sr_lines_any),
            (1 << 5 | 0b10, 1 << 5 | 0b110)
        );
        assert_eq!(
            (a.dep_lines, a.dep_lines_any, a.dep_line_frames),
            (0b10, 0b110, 1),
            "line 2 was the sim's own value when it read it"
        );
        let t = c.totals();
        assert_eq!((t.dep_lines, t.dep_lines_any), (1, 2));
        assert_eq!(t.rs_core_lined, 1, "A was sticky when it was rs_core");
        let r = c.report(5);
        assert!(
            r.iter()
                .any(|l| l.contains("lines page=0x80100000") && l.contains("(1 of 64)")),
            "{r:#?}"
        );
        assert!(
            r.iter()
                .any(|l| l.contains("coverage group=other rs_core/pair=1.0 lined/pair=0.5 (50%)")),
            "{r:#?}"
        );
    }

    #[test]
    fn sticky_pages_are_picked_per_region_group_within_the_budget() {
        let mut c = census();
        c.regions.add(0x0001_0000, 0x0010_0000, "xbe_image");
        c.regions.add(0x8020_0000, 0x8020_2000, "datum:objects");
        c.regions.add(0x8020_0000, 0x8030_0000, "game_state");
        c.regions.add(0x8000_0000, 0x8010_0000, "world_pool");
        c.regions.add(0x0040_3000, 0x0040_9000, "d3d_globals");
        let pages = [
            0x0001_1000u32,
            0x0001_2000,
            0x0001_3000,
            0x8020_0000,
            0x8020_4000,
            0x8000_5000,
            0x0040_4000,
        ];
        // Each page rs_core once; the second XBE page twice.
        for round in 0..2 {
            let hits: Vec<_> = pages
                .iter()
                .filter(|&&p| round == 0 || p == 0x0001_2000)
                .map(|&p| hit(p, true, 1, RENDER))
                .collect();
            let reads: Vec<_> = hits.iter().map(|h| hit(h.addr, false, 2, CPU)).collect();
            c.render_done(&log(hits));
            c.sim_done(&log(reads));
        }
        assert_eq!(
            c.pick_sticky(Some(1)),
            vec![
                0x0001_2000 >> 12,
                0x8000_5000 >> 12,
                0x8020_0000 >> 12,
                0x8020_4000 >> 12
            ],
            "one per group (the most frequent XBE page), never the D3D globals"
        );
        assert_eq!(c.pick_sticky(None).len(), 6, "all rs_core pages");
        assert_eq!(c.pick_sticky(Some(0)), Vec::<u32>::new());
        c.pick_sticky(Some(2));
        let r = c.report(5);
        for want in [
            "sticky group=xbe pages=2",
            "sticky group=datum pages=1",
            "coverage group=game_state rs_core/pair=0.5",
        ] {
            assert!(r.iter().any(|l| l.contains(want)), "{want}: {r:#?}");
        }
        assert_eq!(region_group("datum:lights"), Some("datum"));
        assert_eq!(region_group("stack"), None);
        assert_eq!(region_group("contiguous"), Some("other"));
    }

    #[test]
    fn other_guest_threads_are_told_apart_by_thread_id_in_both_scopes() {
        let mut c = census();
        c.set_owner(Some(7));
        // The render (thread 7) writes A; thread 9 writes E inside the scope,
        // in a phase that would have counted as the render's.
        c.render_done(&with_lines(
            log(vec![
                on(7, hit(A, true, 1, RENDER)),
                on(9, hit(E, true, 3, RENDER)),
            ]),
            vec![lines(A, 7, 0, 0b1, 0), lines(A, 9, 0, 0b10, 0)],
        ));
        // Thread 9 reads A (and its line 0) next; the game thread reads
        // nothing of it.
        c.sim_done(&with_lines(
            log(vec![
                on(9, hit(A, false, 4, CPU)),
                on(7, hit(D, false, 5, CPU)),
            ]),
            vec![lines(A, 9, 0b1, 0, 0b1)],
        ));
        let t = c.totals();
        assert_eq!((t.rw, t.other_thread, t.sim_other_thread), (1, 1, 1));
        assert_eq!((t.rs, t.rs_other), (0, 1));
        assert!(!c.pages().contains_key(&(E >> 12)));
        let a = &c.pages()[&(A >> 12)];
        assert_eq!((a.rw_lines, a.dep_lines, a.dep_lines_other), (0b1, 0, 0b1));
        let r = c.report(5);
        for want in [
            "other_thread scope=render thread=9 hits/frame=1.0",
            "other_thread scope=sim thread=9 hits/frame=1.0",
        ] {
            assert!(r.iter().any(|l| l.contains(want)), "{want}: {r:#?}");
        }
    }

    #[test]
    fn handler_calls_and_host_writes_are_named_per_scope() {
        const WAIT: u32 = 0xfff0_0000 + 4 * 189;
        const D3D_ENTRY: u32 = 0x0030_0000;
        let mut c = census();
        c.set_owner(Some(7));
        c.note_hle(
            true,
            &[
                HleCall {
                    target: WAIT,
                    thread: 7,
                    calls: 2,
                    stops: 1,
                },
                HleCall {
                    target: D3D_ENTRY,
                    thread: 9,
                    calls: 3,
                    stops: 0,
                },
            ],
        );
        c.render_done(&log(vec![
            TrapHit {
                hle: D3D_ENTRY,
                ..on(7, hit(B, true, 0, D3D))
            },
            on(7, hit(C, true, 0, RENDER)),
        ]));
        assert_eq!(c.unnamed(), vec![D3D_ENTRY, WAIT]);
        c.set_name(WAIT, "kernel:NtWaitForSingleObjectEx".into());
        c.set_name(D3D_ENTRY, "D3DDevice_Swap".into());
        assert_eq!(
            c.hle()[&(true, WAIT)],
            HleStat {
                calls: 2,
                stops: 1,
                ..HleStat::default()
            }
        );
        let r = c.report(0);
        for want in [
            "hle scope=render name=kernel:NtWaitForSingleObjectEx target=0xfff002f4 calls/frame game=2.00 other=0.00 stops game=1 other=0 wait",
            "host_writes scope=render by=D3DDevice_Swap pages/frame=1.00",
            "host_writes scope=render by=host:no-handler pages/frame=1.00",
        ] {
            assert!(r.iter().any(|l| l.contains(want)), "{want}: {r:#?}");
        }
        assert!(
            !r.iter().any(|l| l.contains("name=D3DDevice_Swap")),
            "top 0 shows only waits: {r:#?}"
        );
        let tsv = c.tsv();
        let row = tsv.lines().find(|l| l.starts_with("0x80101000")).unwrap();
        assert!(row.contains("\tD3DDevice_Swap\t"), "{row}");
        assert!(is_wait("kernel:KeDelayExecutionThread"));
        assert!(!is_wait("kernel:KeQueryPerformanceCounter"));
    }

    #[test]
    fn the_line_budget_reads_all_or_a_number() {
        assert_eq!(parse_lines(None), Some(64));
        assert_eq!(parse_lines(Some("all")), None);
        assert_eq!(parse_lines(Some("8")), Some(8));
        assert_eq!(parse_lines(Some("0")), Some(0));
    }

    #[test]
    fn line_masks_clip_to_the_page() {
        assert_eq!(line_mask(1, 0x1000, 1), 1);
        assert_eq!(line_mask(1, 0x103f, 2), 0b11);
        assert_eq!(line_mask(1, 0x0ffe, 4), 1, "a span from the page before");
        assert_eq!(line_mask(1, 0x1ffe, 4), 1 << 63);
        assert_eq!(line_mask(1, 0x0000, 0x3000), u64::MAX);
        assert_eq!(line_mask(1, 0x2000, 4), 0);
    }

    #[test]
    fn regions_put_the_stack_first_then_the_order_added() {
        let mut r = Regions::default();
        r.add(0x1000, 0x3000, "first");
        r.add(0x2000, 0x5_6000, "second");
        r.set_stack(0x0005_3010);
        assert_eq!(r.classify(0x1800), "first");
        assert_eq!(r.classify(0x2800), "first");
        assert_eq!(r.classify(0x1_0000), "second");
        assert_eq!(r.classify(0x1_3000), "stack", "the stack's span below esp");
        assert_eq!(r.classify(0x0005_4fff), "stack", "a page above esp too");
        assert_eq!(r.classify(0x0005_5000), "second");
        assert_eq!(r.classify(0x0005_6000), "other");
        assert_eq!(r.classify(0x8123_4000), "contiguous");
    }

    #[test]
    fn datum_arrays_are_found_by_signature() {
        use engine::datum::{SIGNATURE, layout as l};
        let mut block = vec![0u8; 0x400];
        let mut put = |at: usize, v: u32| block[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let h = 0x100;
        put(h + l::SIGNATURE as usize, SIGNATURE);
        put(h + l::MAXIMUM_COUNT as usize, 16);
        put(h + l::ELEMENT_SIZE as usize, 0x20);
        put(h + l::DATA as usize, 0x8020_0000);
        // A signature without a name or data is not an array.
        put(0x200 + l::SIGNATURE as usize, SIGNATURE);
        block[h..h + 5].copy_from_slice(b"thing");
        assert_eq!(
            datum_arrays(&block),
            vec![(0x100, "thing".to_string(), 0x8020_0000, 0x200)]
        );
    }

    #[test]
    fn the_report_and_table_name_every_relation() {
        let mut c = census();
        c.regions.add(A, A + 0x1000, "game_state");
        render(&mut c);
        sim(&mut c);
        render(&mut c);
        let r = c.report(3);
        assert!(r[0].starts_with("window frames=2 pairs=1"), "{}", r[0]);
        assert!(r[0].contains("rs=3.0 rs_core=1.0"), "{}", r[0]);
        for want in [
            "rs page=0x80100000 region=game_state frames=1/2 render_writer=0x00010000 sim_reader=0x00020000",
            "ww page=0x80100000",
            "sr page=0x80103000 region=contiguous frames=1/2 sim_writer=0x00020010 render_reader=0x00010008",
            "region stack: pages ever rw=1 rs=1",
        ] {
            assert!(r.iter().any(|l| l.contains(want)), "{want}: {r:#?}");
        }
        let tsv = c.tsv();
        assert!(tsv.starts_with("# render-census frames=2 pairs=1\npage\tregion\trw\t"));
        let row = tsv.lines().find(|l| l.starts_with("0x80101000")).unwrap();
        assert!(
            row.starts_with("0x80101000\tcontiguous\t2\t0\t2\t0\td3d\t0\t-\t1\t0x00020008"),
            "{row}"
        );
        assert_eq!(tsv.lines().count(), 2 + 5, "A, B, C, D and the stack page");
    }
}
