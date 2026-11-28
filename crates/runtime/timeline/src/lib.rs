//! Timeline events for performance work: frames, guest thread slices,
//! kernel waits, interrupt and DPC delivery, push-buffer drains and GPU
//! submits, written out as Chrome trace-event JSON that Perfetto
//! (<https://ui.perfetto.dev>) and `chrome://tracing` open directly.
//!
//! The crate has no dependencies so the runtime, the kernel and gfx
//! can all call it. Every hook is one relaxed atomic load and a branch
//! while recording is off; nothing allocates, locks or reads the clock
//! until [`start`] has switched a category on. See `docs/perf/profiling.md`.
//!
//! ```
//! use timeline::{Cat, Track};
//! timeline::start(timeline::Config::all());
//! {
//!     let _s = timeline::span(Cat::Guest, Track::Guest(1), "slice");
//! }
//! timeline::frame();
//! let trace = timeline::stop();
//! let mut json = Vec::new();
//! trace.write_chrome(&mut json).unwrap();
//! assert!(String::from_utf8(json).unwrap().contains("\"slice\""));
//! ```

pub mod host;
pub mod phase;
pub mod native;
pub mod setup;

use std::cell::Cell;
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

/// What an event is about. Each is one bit of the recording mask, so a
/// trace can leave out the busy ones (`Hle` fires on every HLE call).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cat {
    /// Swap/Present, one instant per guest frame.
    Frame,
    /// A guest thread's slice on the host CPU (kernel `Machine::run`).
    Guest,
    /// Kernel waits, sleeps and idle clock advances.
    Kernel,
    /// Guest interrupt and DPC delivery.
    Interrupt,
    /// D3D push-buffer drains, KickOff, Swap.
    D3d,
    /// wgpu queue submits and presents.
    Gfx,
    /// Every HLE override call. Off in [`Config::default`]: it is frequent.
    Hle,
    /// The kernel scheduler: one `schedule` span per `Machine::run`
    /// iteration (slices and DPCs nest inside it), pacer waits, and the
    /// logical clock as a counter.
    Sched,
    /// Contended locks: a span for each wait on a lock another thread held
    /// (the guest memory lock). Uncontended acquisitions record nothing.
    Lock,
}

impl Cat {
    pub const ALL: [Cat; 9] = [
        Cat::Frame,
        Cat::Guest,
        Cat::Kernel,
        Cat::Interrupt,
        Cat::D3d,
        Cat::Gfx,
        Cat::Hle,
        Cat::Sched,
        Cat::Lock,
    ];
    const fn bit(self) -> u32 {
        1 << self as u32
    }
    /// The name used in the JSON `cat` field and in [`Config::parse`].
    pub const fn name(self) -> &'static str {
        match self {
            Cat::Frame => "frame",
            Cat::Guest => "guest",
            Cat::Kernel => "kernel",
            Cat::Interrupt => "interrupt",
            Cat::D3d => "d3d",
            Cat::Gfx => "gfx",
            Cat::Hle => "hle",
            Cat::Sched => "sched",
            Cat::Lock => "lock",
        }
    }
}

/// Which row an event is drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Track {
    /// The host thread that records the event.
    Host,
    /// A guest thread, by its kernel thread id (the DPC pseudo-thread too).
    Guest(u32),
}

/// What [`start`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Events kept before further ones are counted in [`Trace::dropped`].
    pub capacity: usize,
    mask: u32,
}

impl Default for Config {
    /// Everything but [`Cat::Hle`], up to four million events.
    fn default() -> Self {
        Self::all().without(Cat::Hle)
    }
}

impl Config {
    /// Every category.
    pub fn all() -> Self {
        Config {
            capacity: 4 << 20,
            mask: Cat::ALL.iter().fold(0, |m, c| m | c.bit()),
        }
    }
    /// No category (add some with [`Config::with`]).
    pub fn none() -> Self {
        Config {
            mask: 0,
            ..Self::all()
        }
    }
    pub fn with(mut self, c: Cat) -> Self {
        self.mask |= c.bit();
        self
    }
    pub fn without(mut self, c: Cat) -> Self {
        self.mask &= !c.bit();
        self
    }
    pub fn has(&self, c: Cat) -> bool {
        self.mask & c.bit() != 0
    }
    /// `"all"`, `"default"`, or a comma list of category names, each
    /// optionally prefixed with `-` to remove it from what came before
    /// (`"all,-hle"`, `"frame,d3d,gfx"`).
    pub fn parse(spec: &str) -> Result<Self, String> {
        let mut c = Config::none();
        for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let (remove, name) = match item.strip_prefix('-') {
                Some(n) => (true, n),
                None => (false, item),
            };
            let cats: Vec<Cat> = match name {
                "all" => Cat::ALL.to_vec(),
                "default" => Cat::ALL
                    .iter()
                    .copied()
                    .filter(|&c| c != Cat::Hle)
                    .collect(),
                n => vec![
                    *Cat::ALL
                        .iter()
                        .find(|c| c.name() == n)
                        .ok_or_else(|| format!("unknown timeline category {n:?}"))?,
                ],
            };
            for cat in cats {
                c = if remove { c.without(cat) } else { c.with(cat) };
            }
        }
        Ok(c)
    }
}

static FRAMES: AtomicU64 = AtomicU64::new(0);
static NEXT_HOST: AtomicU32 = AtomicU32::new(1);

thread_local! {
    static HOST_TID: Cell<u32> = const { Cell::new(0) };
}

#[derive(Debug, Clone, Copy)]
enum Phase {
    Complete { dur: u64 },
    Instant,
    AsyncBegin { id: u64 },
    AsyncEnd { id: u64 },
    Counter { value: u64 },
}

#[derive(Debug, Clone, Copy)]
struct Ev {
    ts: u64,
    phase: Phase,
    cat: Cat,
    track: Track,
    host: u32,
    name: &'static str,
    arg: Option<(&'static str, u64)>,
}

struct State {
    events: Vec<Ev>,
    capacity: usize,
    dropped: u64,
    host_names: Vec<(u32, String)>,
    waits: HashMap<u32, (u64, &'static str, u32)>,
    /// The capture events are accepted for; see [`CAPTURE`].
    generation: u32,
    active: bool,
}

/// The capture: its generation in the high half, its category mask in
/// the low half, stored together (under the state lock) by [`start`] and
/// [`stop`], which both bump the generation. A hook makes one load: the
/// mask says whether to record and the generation, taken by the same load
/// before anything else happens, is the capture the event belongs to.
/// [`push`] (and a wait's insertion) keeps an event only if that capture
/// is still the one recording, checked under the lock. So an event begun
/// in one capture never lands in a later one, however long its hook is
/// held up between the load and the push.
static CAPTURE: AtomicU64 = AtomicU64::new(0);
static EVENT_BUFFER_BYTES: AtomicU64 = AtomicU64::new(0);

/// Allocated bytes in the active timeline writer's event buffer.
pub fn event_buffer_bytes() -> u64 {
    EVENT_BUFFER_BYTES.load(Ordering::Relaxed)
}

fn publish_event_capacity(capacity: usize) {
    EVENT_BUFFER_BYTES.store((capacity * size_of::<Ev>()) as u64, Ordering::Relaxed);
}

/// The capture's generation if `cat` is recording. One relaxed load.
#[inline(always)]
fn token(cat: Cat) -> Option<u32> {
    let c = CAPTURE.load(Ordering::Relaxed);
    (c as u32 & cat.bit() != 0).then_some((c >> 32) as u32)
}

fn publish(s: &State, mask: u32) {
    CAPTURE.store(
        ((s.generation as u64) << 32) | mask as u64,
        Ordering::Release,
    );
}

// Test seam: runs on the hook's thread right after it took its token.
#[cfg(test)]
thread_local! {
    static PAUSE: std::cell::RefCell<Option<Box<dyn FnMut()>>> = const { std::cell::RefCell::new(None) };
}

#[inline(always)]
fn after_token() {
    #[cfg(test)]
    if let Some(mut f) = PAUSE.with(|p| p.borrow_mut().take()) {
        f();
    }
}

fn state() -> MutexGuard<'static, State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(State {
            events: Vec::new(),
            capacity: 0,
            dropped: 0,
            host_names: Vec::new(),
            waits: HashMap::new(),
            generation: 0,
            active: false,
        })
    })
    .lock()
    .unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
thread_local! {
    static TEST_NOW_NS: Cell<Option<u64>> = const { Cell::new(None) };
}

/// Nanoseconds since the first use of the timeline in this process.
///
/// Frame stats read it at every phase switch, several times a draw, so it
/// is the bare clock: on Apple platforms `mach_absolute_time` (the counter
/// `Instant` reads, minus `clock_gettime`'s layers: about 6 ns a read
/// against 25 ns for `Instant::elapsed` on an M-series core), scaled to
/// nanoseconds by a fixed-point factor; elsewhere `Instant`.
#[inline]
pub fn now_ns() -> u64 {
    #[cfg(test)]
    if let Some(now) = TEST_NOW_NS.get() {
        return now;
    }
    clock::now_ns()
}

#[cfg(target_vendor = "apple")]
mod clock {
    use std::sync::OnceLock;

    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }

    /// The first reading, and ticks to nanoseconds as a 32.32 fixed-point
    /// factor (exact when the timebase is 1/1; with Apple silicon's 125/3
    /// it is off by under 4e-12, a nanosecond in four minutes).
    struct Epoch {
        start: u64,
        scale: u128,
    }

    fn epoch() -> &'static Epoch {
        static E: OnceLock<Epoch> = OnceLock::new();
        E.get_or_init(|| {
            let mut tb = Timebase { numer: 1, denom: 1 };
            // SAFETY: `tb` is a valid out-pointer for the call.
            if unsafe { mach_timebase_info(&mut tb) } != 0 || tb.denom == 0 {
                tb = Timebase { numer: 1, denom: 1 };
            }
            Epoch {
                // SAFETY: no preconditions.
                start: unsafe { mach_absolute_time() },
                scale: ((tb.numer as u128) << 32) / tb.denom as u128,
            }
        })
    }

    #[inline]
    pub(super) fn now_ns() -> u64 {
        let e = epoch();
        // SAFETY: no preconditions.
        let ticks = unsafe { mach_absolute_time() }.saturating_sub(e.start);
        ((ticks as u128 * e.scale) >> 32) as u64
    }
}

#[cfg(not(target_vendor = "apple"))]
mod clock {
    use std::sync::OnceLock;
    use std::time::Instant;

    #[inline]
    pub(super) fn now_ns() -> u64 {
        static E: OnceLock<Instant> = OnceLock::new();
        E.get_or_init(Instant::now).elapsed().as_nanos() as u64
    }
}

/// True while `cat` is being recorded. One relaxed load.
#[inline(always)]
pub fn enabled(cat: Cat) -> bool {
    token(cat).is_some()
}

/// True while anything is being recorded.
#[inline(always)]
pub fn recording() -> bool {
    CAPTURE.load(Ordering::Relaxed) as u32 != 0
}

fn host_tid() -> u32 {
    HOST_TID.with(|t| {
        if t.get() == 0 {
            let id = NEXT_HOST.fetch_add(1, Ordering::Relaxed);
            t.set(id);
            let name = std::thread::current()
                .name()
                .map_or_else(|| format!("host thread {id}"), str::to_owned);
            state().host_names.push((id, name));
        }
        t.get()
    })
}

#[cold]
fn push(ev: Ev, generation: u32) {
    let mut s = state();
    if !s.active || s.generation != generation {
        return;
    }
    if s.events.len() < s.capacity {
        let capacity = s.events.capacity();
        s.events.push(ev);
        if s.events.capacity() != capacity {
            publish_event_capacity(s.events.capacity());
        }
    } else {
        s.dropped += 1;
    }
}

/// Build and push an event for capture `generation`.
#[cold]
#[inline(never)]
fn record(
    generation: u32,
    cat: Cat,
    track: Track,
    name: &'static str,
    phase: Phase,
    arg: Option<(&'static str, u64)>,
) {
    after_token();
    let mut e = ev(cat, track, name, phase, now_ns());
    e.arg = arg;
    push(e, generation);
}

fn ev(cat: Cat, track: Track, name: &'static str, phase: Phase, ts: u64) -> Ev {
    Ev {
        ts,
        phase,
        cat,
        track,
        host: host_tid(),
        name,
        arg: None,
    }
}

/// Start recording the categories in `config`, discarding anything
/// recorded before. The frame counter is not reset.
pub fn start(config: Config) {
    now_ns();
    {
        let mut s = state();
        s.events = Vec::with_capacity(config.capacity.min(1 << 16));
        publish_event_capacity(s.events.capacity());
        s.capacity = config.capacity;
        s.dropped = 0;
        s.waits.clear();
        s.generation = s.generation.wrapping_add(1);
        s.active = true;
        publish(&s, config.mask);
    }
}

/// Stop recording and take what was recorded.
pub fn stop() -> Trace {
    let mut s = state();
    s.generation = s.generation.wrapping_add(1);
    s.active = false;
    publish(&s, 0);
    let mut events = std::mem::take(&mut s.events);
    publish_event_capacity(s.events.capacity());
    events.sort_by_key(|e| e.ts);
    s.waits.clear();
    Trace {
        events,
        dropped: std::mem::take(&mut s.dropped),
        host_names: s.host_names.clone(),
    }
}

struct Open {
    ev: Ev,
    start: u64,
    generation: u32,
}

/// An open span; records a complete event when dropped. One pointer: an
/// inert span (category off) is a null check to make and to drop.
#[must_use = "a span ends when this is dropped"]
pub struct Span {
    open: Option<Box<Open>>,
}

impl Span {
    /// Attach one numeric argument (shown in the event's details).
    #[inline(always)]
    pub fn arg(mut self, key: &'static str, value: u64) -> Self {
        self.set_arg(key, value);
        self
    }
    /// Set or replace the argument after the span started.
    #[inline(always)]
    pub fn set_arg(&mut self, key: &'static str, value: u64) {
        if let Some(o) = &mut self.open {
            o.ev.arg = Some((key, value));
        }
    }
}

impl Drop for Span {
    #[inline(always)]
    fn drop(&mut self) {
        if let Some(o) = self.open.take() {
            finish(&o);
        }
    }
}

#[cold]
#[inline(never)]
fn finish(o: &Open) {
    let mut e = o.ev;
    e.phase = Phase::Complete {
        dur: now_ns().saturating_sub(o.start),
    };
    push(e, o.generation);
}

#[cold]
#[inline(never)]
fn open(generation: u32, cat: Cat, track: Track, name: &'static str) -> Span {
    after_token();
    let ts = now_ns();
    Span {
        open: Some(Box::new(Open {
            ev: ev(cat, track, name, Phase::Instant, ts),
            start: ts,
            generation,
        })),
    }
}

/// A span from now until the returned guard drops. Inert (one load and a
/// branch, no clock read, nothing to drop) unless `cat` is recording.
#[inline(always)]
pub fn span(cat: Cat, track: Track, name: &'static str) -> Span {
    match token(cat) {
        Some(g) => open(g, cat, track, name),
        None => Span { open: None },
    }
}

/// A zero-length event.
#[inline]
pub fn instant(cat: Cat, track: Track, name: &'static str) {
    if let Some(g) = token(cat) {
        record(g, cat, track, name, Phase::Instant, None);
    }
}

/// A zero-length event with one argument.
#[inline]
pub fn instant_arg(cat: Cat, track: Track, name: &'static str, key: &'static str, value: u64) {
    if let Some(g) = token(cat) {
        record(g, cat, track, name, Phase::Instant, Some((key, value)));
    }
}

/// Start an asynchronous span: one that may outlive the host call that
/// began it or overlap others on its track (an interrupt that a
/// preemption interrupts). End it with the same `name` and `id`.
#[inline]
pub fn async_begin(cat: Cat, track: Track, name: &'static str, id: u64) {
    if let Some(g) = token(cat) {
        record(g, cat, track, name, Phase::AsyncBegin { id }, None);
    }
}

/// End an [`async_begin`] span.
#[inline]
pub fn async_end(cat: Cat, track: Track, name: &'static str, id: u64) {
    if let Some(g) = token(cat) {
        record(g, cat, track, name, Phase::AsyncEnd { id }, None);
    }
}

/// A counter sample (drawn as its own track).
#[inline]
pub fn counter(cat: Cat, name: &'static str, value: u64) {
    if let Some(g) = token(cat) {
        record(g, cat, Track::Host, name, Phase::Counter { value }, None);
    }
}

/// Guest thread `thread` started waiting (`name`: what on). Ended by the
/// next [`wait_end`] for the thread; a second begin replaces the first.
#[inline]
pub fn wait_begin(thread: u32, name: &'static str) {
    if let Some(g) = token(Cat::Kernel) {
        wait_begin_slow(g, thread, name);
    }
}

#[cold]
#[inline(never)]
fn wait_begin_slow(generation: u32, thread: u32, name: &'static str) {
    after_token();
    let ts = now_ns();
    let mut s = state();
    // A begin from a capture that has since ended must not replace a
    // wait the current capture recorded for the same thread.
    if s.active && s.generation == generation {
        s.waits.insert(thread, (ts, name, generation));
    }
}

/// Guest thread `thread` runs again: closes its open wait, if any, as a
/// span on the thread's track.
#[inline]
pub fn wait_end(thread: u32) {
    if let Some(g) = token(Cat::Kernel) {
        wait_end_slow(g, thread);
    }
}

#[cold]
fn wait_end_slow(generation: u32, thread: u32) {
    after_token();
    let (start, name) = {
        let mut s = state();
        // An end from a capture that has since ended must not take a wait
        // the current capture recorded for the same thread.
        if !s.active || s.generation != generation {
            return;
        }
        match s.waits.remove(&thread) {
            Some((start, name, _)) => (start, name),
            None => return,
        }
    };
    let mut e = ev(
        Cat::Kernel,
        Track::Guest(thread),
        name,
        Phase::Complete { dur: 0 },
        start,
    );
    e.phase = Phase::Complete {
        dur: now_ns().saturating_sub(start),
    };
    push(e, generation);
}

/// One guest frame was presented (Swap). Always counted, so [`frames`]
/// gives an fps without recording; also an instant when [`Cat::Frame`]
/// is on, and the end of a frame for [`phase`] stats when they are on.
#[inline]
pub fn frame() {
    let n = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    if phase::enabled() {
        phase::frame_end(n);
    }
    if let Some(g) = token(Cat::Frame) {
        record(
            g,
            Cat::Frame,
            Track::Host,
            "frame",
            Phase::Instant,
            Some(("n", n)),
        );
    }
}

/// Frames counted by [`frame`] since the process started.
pub fn frames() -> u64 {
    FRAMES.load(Ordering::Relaxed)
}

/// Recorded events, ready to write.
#[derive(Debug, Clone)]
pub struct Trace {
    events: Vec<Ev>,
    /// Events lost because the buffer was full.
    pub dropped: u64,
    host_names: Vec<(u32, String)>,
}

const HOST_PID: u32 = 1;
const GUEST_PID: u32 = 2;

fn json_str(out: &mut impl Write, s: &str) -> io::Result<()> {
    out.write_all(b"\"")?;
    for c in s.chars() {
        match c {
            '"' => out.write_all(b"\\\"")?,
            '\\' => out.write_all(b"\\\\")?,
            c if (c as u32) < 0x20 => write!(out, "\\u{:04x}", c as u32)?,
            c => write!(out, "{c}")?,
        }
    }
    out.write_all(b"\"")
}

/// Microseconds with nanosecond precision, as trace-event `ts` expects.
fn us(ns: u64) -> String {
    format!("{}.{:03}", ns / 1000, ns % 1000)
}

impl Trace {
    /// Number of events.
    pub fn len(&self) -> usize {
        self.events.len()
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
    /// Events named `name` (for tests and summaries).
    pub fn count(&self, name: &str) -> usize {
        self.events.iter().filter(|e| e.name == name).count()
    }
    /// Write Chrome trace-event JSON (the "JSON object" form).
    pub fn write_chrome(&self, out: &mut impl Write) -> io::Result<()> {
        out.write_all(b"{\"displayTimeUnit\":\"ns\",\"traceEvents\":[\n")?;
        let mut first = true;
        let mut sep = |out: &mut dyn Write| -> io::Result<()> {
            if !std::mem::take(&mut first) {
                out.write_all(b",\n")?;
            }
            Ok(())
        };
        for (pid, name) in [(HOST_PID, "host"), (GUEST_PID, "guest threads")] {
            sep(out)?;
            write!(
                out,
                "{{\"ph\":\"M\",\"name\":\"process_name\",\"pid\":{pid},\"tid\":0,\"args\":{{\"name\":"
            )?;
            json_str(out, name)?;
            out.write_all(b"}}")?;
        }
        for (tid, name) in &self.host_names {
            sep(out)?;
            write!(
                out,
                "{{\"ph\":\"M\",\"name\":\"thread_name\",\"pid\":{HOST_PID},\"tid\":{tid},\"args\":{{\"name\":"
            )?;
            json_str(out, name)?;
            out.write_all(b"}}")?;
        }
        let mut guests: Vec<u32> = self
            .events
            .iter()
            .filter_map(|e| match e.track {
                Track::Guest(t) => Some(t),
                Track::Host => None,
            })
            .collect();
        guests.sort_unstable();
        guests.dedup();
        for t in guests {
            sep(out)?;
            write!(
                out,
                "{{\"ph\":\"M\",\"name\":\"thread_name\",\"pid\":{GUEST_PID},\"tid\":{t},\"args\":{{\"name\":\"guest thread {t:#x}\"}}}}"
            )?;
        }
        for e in &self.events {
            sep(out)?;
            let (pid, tid) = match e.track {
                Track::Host => (HOST_PID, e.host),
                Track::Guest(t) => (GUEST_PID, t),
            };
            out.write_all(b"{\"name\":")?;
            json_str(out, e.name)?;
            write!(
                out,
                ",\"cat\":\"{}\",\"pid\":{pid},\"tid\":{tid},\"ts\":{}",
                e.cat.name(),
                us(e.ts)
            )?;
            match e.phase {
                Phase::Complete { dur } => write!(out, ",\"ph\":\"X\",\"dur\":{}", us(dur))?,
                Phase::Instant => out.write_all(b",\"ph\":\"i\",\"s\":\"t\"")?,
                Phase::AsyncBegin { id } => write!(out, ",\"ph\":\"b\",\"id\":\"{id:#x}\"")?,
                Phase::AsyncEnd { id } => write!(out, ",\"ph\":\"e\",\"id\":\"{id:#x}\"")?,
                Phase::Counter { value } => {
                    out.write_all(b",\"ph\":\"C\",\"args\":{")?;
                    json_str(out, e.name)?;
                    write!(out, ":{value}}}}}")?;
                    continue;
                }
            }
            if let Some((k, v)) = e.arg {
                out.write_all(b",\"args\":{")?;
                json_str(out, k)?;
                write!(out, ":{v}}}")?;
            }
            out.write_all(b"}")?;
        }
        out.write_all(b"\n]}\n")
    }
    /// Write the JSON to `path`.
    pub fn save(&self, path: &std::path::Path) -> io::Result<()> {
        let mut f = io::BufWriter::new(std::fs::File::create(path)?);
        self.write_chrome(&mut f)?;
        f.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recording is process-global; tests that start it take this.
    pub(super) static SERIAL: Mutex<()> = Mutex::new(());

    #[test]
    fn nothing_is_recorded_while_off() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let _ = stop();
        assert!(!recording());
        let before = frames();
        {
            let _s = span(Cat::Guest, Track::Guest(1), "slice");
        }
        instant(Cat::Kernel, Track::Host, "idle");
        wait_begin(3, "wait");
        wait_end(3);
        frame();
        assert_eq!(frames(), before + 1, "frames count even while off");
        assert!(stop().is_empty());
    }

    #[test]
    fn spans_waits_and_frames_reach_the_json() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        start(Config::all());
        {
            let _s = span(Cat::Guest, Track::Guest(7), "slice").arg("transfers", 12);
            wait_begin(7, "KeWaitForSingleObject");
        }
        wait_end(7);
        wait_end(7); // already closed: nothing
        async_begin(Cat::Interrupt, Track::Guest(7), "vblank", 1);
        async_end(Cat::Interrupt, Track::Guest(7), "vblank", 1);
        counter(Cat::Gfx, "draws", 3);
        frame();
        let t = stop();
        assert_eq!(t.count("slice"), 1);
        assert_eq!(t.count("KeWaitForSingleObject"), 1);
        assert_eq!(t.count("vblank"), 2);
        assert_eq!(t.count("frame"), 1);
        let mut out = Vec::new();
        t.write_chrome(&mut out).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("{\"displayTimeUnit\""));
        assert!(s.contains("\"ph\":\"X\""));
        assert!(s.contains("\"transfers\":12"));
        assert!(s.contains("guest thread 0x7"));
        assert!(s.contains("\"ph\":\"C\",\"args\":{\"draws\":3}"));
        assert!(s.trim_end().ends_with("]}"));
    }

    #[test]
    fn categories_filter_and_capacity_drops() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        start(Config {
            capacity: 2,
            ..Config::none().with(Cat::D3d)
        });
        instant(Cat::Gfx, Track::Host, "submit");
        for _ in 0..5 {
            instant(Cat::D3d, Track::Host, "drain");
        }
        assert_eq!(event_buffer_bytes(), 2 * size_of::<Ev>() as u64);
        let t = stop();
        assert_eq!(event_buffer_bytes(), 0);
        assert_eq!(t.count("submit"), 0);
        assert_eq!(t.len(), 2);
        assert_eq!(t.dropped, 3);
    }

    #[test]
    fn a_span_from_an_earlier_capture_is_not_written_into_a_later_one() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        start(Config::all());
        let old = span(Cat::Guest, Track::Guest(1), "old");
        wait_begin(2, "old wait");
        assert!(stop().is_empty());
        start(Config::all());
        drop(old);
        wait_end(2);
        {
            let _new = span(Cat::Guest, Track::Guest(1), "new");
        }
        let t = stop();
        assert_eq!(t.count("old"), 0);
        assert_eq!(t.count("old wait"), 0);
        assert_eq!(t.count("new"), 1);
        // Closed after recording stopped: dropped, not buffered.
        start(Config::all());
        let late = span(Cat::Guest, Track::Guest(1), "late");
        let _ = stop();
        drop(late);
        start(Config::all());
        assert_eq!(stop().count("late"), 0);
    }

    /// Run `hook` on a worker, held up right after it took its token while
    /// the main thread does `between`; returns the trace of the capture
    /// running once the worker finished.
    fn interleave(hook: fn(), between: impl FnOnce()) {
        use std::sync::mpsc;
        let (at_token, paused) = mpsc::channel();
        let (go, resume) = mpsc::channel::<()>();
        let w = std::thread::Builder::new()
            .name("held-up hook".into())
            .spawn(move || {
                PAUSE.with(|p| {
                    *p.borrow_mut() = Some(Box::new(move || {
                        at_token.send(()).unwrap();
                        resume.recv().unwrap();
                    }))
                });
                hook();
            })
            .unwrap();
        paused.recv().unwrap();
        between();
        go.send(()).unwrap();
        w.join().unwrap();
    }

    #[test]
    fn an_event_held_up_across_a_restart_stays_out_of_the_new_capture() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        start(Config::all());
        interleave(
            || instant(Cat::Guest, Track::Host, "old instant"),
            || {
                assert!(stop().is_empty());
                start(Config::none().with(Cat::Hle));
            },
        );
        let t = stop();
        assert_eq!(t.count("old instant"), 0, "B records only hle");
        assert!(t.is_empty());
    }

    #[test]
    fn a_wait_begin_held_up_across_a_restart_keeps_the_new_wait() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        start(Config::all());
        interleave(
            || wait_begin(5, "old wait"),
            || {
                let _ = stop();
                start(Config::all());
                wait_begin(5, "new wait");
            },
        );
        wait_end(5);
        let t = stop();
        assert_eq!(t.count("new wait"), 1);
        assert_eq!(t.count("old wait"), 0);
    }

    #[test]
    fn a_wait_end_held_up_across_a_restart_leaves_the_new_wait() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        start(Config::all());
        wait_begin(6, "old wait");
        interleave(
            || wait_end(6),
            || {
                let _ = stop();
                start(Config::all());
                wait_begin(6, "new wait");
            },
        );
        wait_end(6);
        let t = stop();
        assert_eq!(
            t.count("new wait"),
            1,
            "the late end must not take B's wait"
        );
        assert_eq!(t.count("old wait"), 0);
    }

    #[test]
    fn an_inert_span_is_one_pointer() {
        assert_eq!(size_of::<Span>(), size_of::<usize>());
    }

    #[test]
    fn config_parses_lists_and_removals() {
        let c = Config::parse("all,-hle").unwrap();
        assert_eq!(c, Config::default());
        let c = Config::parse("frame, d3d").unwrap();
        assert!(c.has(Cat::Frame) && c.has(Cat::D3d) && !c.has(Cat::Guest));
        assert!(Config::parse("bogus").is_err());
    }

    #[test]
    fn sched_and_lock_are_categories_of_their_own() {
        let c = Config::parse("sched").unwrap();
        assert!(c.has(Cat::Sched) && !c.has(Cat::Lock) && !c.has(Cat::Kernel));
        let d = Config::default();
        assert!(d.has(Cat::Sched) && d.has(Cat::Lock), "on by default");
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        start(Config::none().with(Cat::Lock));
        {
            let _s = span(Cat::Sched, Track::Host, "schedule");
            let _l = span(Cat::Lock, Track::Host, "memory lock wait");
        }
        let t = stop();
        assert_eq!(t.count("schedule"), 0);
        assert_eq!(t.count("memory lock wait"), 1);
        let mut out = Vec::new();
        t.write_chrome(&mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("\"cat\":\"lock\""));
    }

    #[test]
    fn names_are_escaped() {
        let mut out = Vec::new();
        json_str(&mut out, "a\"b\\c\n").unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "\"a\\\"b\\\\c\\u000a\"");
    }

    /// The bare clock never goes backwards and keeps time with `Instant`:
    /// over an interval measured both ways they agree to well under a
    /// percent (the slack is the reads' own spacing, not drift).
    #[test]
    fn now_ns_is_monotonic_and_keeps_time_with_instant() {
        let mut last = now_ns();
        for _ in 0..100_000 {
            let t = now_ns();
            assert!(t >= last, "{t} after {last}");
            last = t;
        }
        let (i0, n0) = (std::time::Instant::now(), now_ns());
        std::thread::sleep(std::time::Duration::from_millis(50));
        let (n1, i1) = (now_ns(), std::time::Instant::now());
        let clock = (n1 - n0) as f64;
        let instant = (i1 - i0).as_nanos() as f64;
        assert!(clock <= instant + 100.0, "{clock} ns inside {instant} ns");
        assert!(clock > instant * 0.99, "{clock} ns against {instant} ns");
    }

    /// `cargo test -p timeline --release now_ns_cost -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark"]
    fn now_ns_cost() {
        let n = 10_000_000u64;
        let e = std::time::Instant::now();
        let time = |f: &dyn Fn() -> u64| {
            (0..5)
                .map(|_| {
                    let s = std::time::Instant::now();
                    let mut acc = 0u64;
                    for _ in 0..n {
                        acc = acc.wrapping_add(f());
                    }
                    std::hint::black_box(acc);
                    s.elapsed().as_secs_f64() * 1e9 / n as f64
                })
                .fold(f64::MAX, f64::min)
        };
        println!("Instant::elapsed: {:.2} ns/read", time(&|| e.elapsed().as_nanos() as u64));
        println!("now_ns:           {:.2} ns/read", time(&now_ns));
    }
}
