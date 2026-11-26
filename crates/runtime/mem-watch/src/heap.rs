//! Rust heap attribution. A one-word trailer remembers the allocating tag and
//! whether the allocation has an entry in the optional live stack index. The
//! user pointer retains the system allocation's alignment, even for over-aligned
//! types. Counts are requested payload bytes and allocations, excluding trailers
//! and allocator rounding. The OS footprint includes these and foreign heaps.
//!
//! The high bit of each tag's allocation count enables sampling. The previous
//! value returned by the normal counter increment tests that bit, so the
//! sampling-off allocation path needs no extra atomic load or sampling TLS.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::marker::PhantomData;
use std::ptr;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};
use std::sync::{Mutex, Once, OnceLock};

use serde::Serialize;

const SAMPLE_BIT: u64 = 1 << 63;
const TRAILER_SIZE: usize = size_of::<*const Tag>();
const MAX_FRAMES: usize = 24;
const MAX_LIVE_SAMPLES: usize = 32_768;

#[repr(align(8))]
struct Tag {
    name: &'static str,
    bytes: AtomicU64,
    allocations: AtomicU64,
    next: Option<&'static Tag>,
}

impl Tag {
    const fn new(name: &'static str, allocations: u64, next: Option<&'static Tag>) -> Self {
        Self {
            name,
            bytes: AtomicU64::new(0),
            allocations: AtomicU64::new(allocations),
            next,
        }
    }

    fn counts(&self) -> Counts {
        Counts {
            live_bytes: self.bytes.load(Ordering::Relaxed),
            live_allocations: self.allocations.load(Ordering::Relaxed) & !SAMPLE_BIT,
        }
    }
}

static UNSCOPED: Tag = Tag::new("unscoped", 0, None);
static BOOKKEEPING: Tag = Tag::new("watchdog.heap", 0, None);
static REGISTER: Mutex<()> = Mutex::new(());
static BUCKETS: [AtomicPtr<Tag>; 64] = [const { AtomicPtr::new(ptr::null_mut()) }; 64];
static INTERVAL: AtomicU64 = AtomicU64::new(0);

fn tags_in(bucket: &AtomicPtr<Tag>) -> impl Iterator<Item = &'static Tag> {
    // SAFETY: writers publish fully initialized, immutable linked lists of
    // leaked tags; an acquired head and all its successors live forever.
    let head = unsafe { bucket.load(Ordering::Acquire).as_ref() };
    std::iter::successors(head, |tag| tag.next)
}

fn intern(name: &'static str) -> &'static Tag {
    if name == UNSCOPED.name {
        return &UNSCOPED;
    }
    if name == BOOKKEEPING.name {
        return &BOOKKEEPING;
    }
    let hash = name
        .bytes()
        .fold(5381usize, |h, b| h.wrapping_mul(33) ^ usize::from(b));
    let bucket = &BUCKETS[hash % BUCKETS.len()];
    if let Some(tag) = tags_in(bucket).find(|tag| tag.name == name) {
        return tag;
    }
    let _lock = REGISTER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(tag) = tags_in(bucket).find(|tag| tag.name == name) {
        return tag;
    }

    let _internal = enter(&BOOKKEEPING);
    let flag = if INTERVAL.load(Ordering::Relaxed) != 0 {
        SAMPLE_BIT
    } else {
        0
    };
    let tag = Box::leak(Box::new(Tag::new(name, flag, tags_in(bucket).next())));
    bucket.store(ptr::from_ref(tag).cast_mut(), Ordering::Release);
    tag
}

thread_local! {
    // Const, destructor-free TLS can be read inside GlobalAlloc.
    static CURRENT: Cell<&'static Tag> = const { Cell::new(&UNSCOPED) };
    static RANDOM: Cell<Poisson> = const { Cell::new(Poisson { rng: 0, remaining: 0 }) };
}

/// Hold in a local variable for the lifetime of a subsystem operation. Nested
/// guards restore the previous tag in drop order. A guard stays on its thread.
#[must_use]
pub struct Scope {
    previous: &'static Tag,
    _local: PhantomData<*mut ()>,
}

fn enter(tag: &'static Tag) -> Scope {
    let previous = CURRENT
        .try_with(|cur| cur.replace(tag))
        .unwrap_or(&UNSCOPED);
    Scope {
        previous,
        _local: PhantomData,
    }
}

pub fn scope(tag: &'static str) -> Scope {
    enter(intern(tag))
}

impl Drop for Scope {
    fn drop(&mut self) {
        let _ = CURRENT.try_with(|cur| cur.set(self.previous));
    }
}

/// Configure the sampler when a monitored binary starts. Counting also covers
/// allocations made before the watchdog starts, including the main thread's.
pub fn init_from_env() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let Some(raw) = std::env::var_os("H2_MEM_SAMPLE") else {
            return;
        };
        let interval = match crate::parse_size(&raw.to_string_lossy()) {
            Ok(0) => return,
            Ok(n) => n,
            Err(err) => {
                eprintln!("memory watchdog: H2_MEM_SAMPLE: {err}");
                return;
            }
        };
        let _lock = REGISTER.lock().unwrap_or_else(|e| e.into_inner());
        INTERVAL.store(interval, Ordering::Relaxed);
        for tag in std::iter::once(&UNSCOPED).chain(BUCKETS.iter().flat_map(tags_in)) {
            tag.allocations.fetch_or(SAMPLE_BIT, Ordering::Release);
        }
        // BOOKKEEPING is deliberately measured but never sampled. Capturing a
        // stack or growing the address index can itself allocate.
    });
}

#[derive(Clone, Copy)]
struct Poisson {
    rng: u64,
    remaining: u64,
}

impl Poisson {
    fn draw(&mut self, interval: u64) -> u64 {
        // xorshift64*, with one independent nonzero seed per thread. Picking a
        // unit variate in (0, 1] keeps the exponential variate finite.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        let bits = self.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 11;
        let unit = (bits + 1) as f64 / (1u64 << 53) as f64;
        ((-unit.ln() * interval as f64) as u64).saturating_add(1)
    }

    fn select(&mut self, bytes: u64, interval: u64) -> bool {
        if self.rng == 0 {
            static SEED: AtomicU64 = AtomicU64::new(0x517c_c1b7_2722_0a95);
            self.rng = SEED
                .fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed)
                .max(1);
            self.remaining = self.draw(interval);
        }
        if bytes < self.remaining {
            self.remaining -= bytes;
            return false;
        }
        // More than one Poisson event within an allocation selects the same
        // object. By memorylessness the distance from its end is exponential.
        self.remaining = self.draw(interval);
        true
    }
}

#[derive(Clone, Copy, Hash, Eq, PartialEq, Ord, PartialOrd)]
struct RawStack([usize; MAX_FRAMES]);

impl RawStack {
    #[inline(never)]
    fn capture() -> Self {
        let mut ips = [0; MAX_FRAMES];
        let mut i = 0;
        backtrace::trace(|frame| {
            ips[i] = frame.ip() as usize;
            i += 1;
            i < MAX_FRAMES
        });
        Self(ips)
    }
}

#[derive(Clone, Copy)]
struct Sample {
    bytes: usize,
    stack: RawStack,
}

static SAMPLES: OnceLock<Mutex<HashMap<usize, Sample>>> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);

fn insert_sample(addr: usize, sample: Sample) -> bool {
    let _internal = enter(&BOOKKEEPING);
    let mut index = SAMPLES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if index.len() >= MAX_LIVE_SAMPLES || index.try_reserve(1).is_err() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
        return false;
    }
    index.insert(addr, sample);
    true
}

fn take_sample(addr: usize) -> Option<Sample> {
    let _internal = enter(&BOOKKEEPING);
    SAMPLES
        .get()?
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&addr)
}

#[cold]
#[inline(never)]
fn sample_growth(addr: usize, size: usize, added: usize) -> bool {
    let interval = INTERVAL.load(Ordering::Relaxed);
    let select = RANDOM
        .try_with(|random| {
            let mut state = random.get();
            let select = state.select(added as u64, interval);
            random.set(state);
            select
        })
        .unwrap_or(false);
    if !select {
        return false;
    }
    let _internal = enter(&BOOKKEEPING);
    insert_sample(
        addr,
        Sample {
            bytes: size,
            stack: RawStack::capture(),
        },
    )
}

fn extended(layout: Layout) -> Option<Layout> {
    Layout::from_size_align(layout.size().checked_add(TRAILER_SIZE)?, layout.align()).ok()
}

// The trailer is immediately after the last payload byte. It can be unaligned.
// Pointer tagging retains the tag's provenance; bit zero is free on every Tag.
unsafe fn write_trailer(data: *mut u8, size: usize, tag: &'static Tag, sampled: bool) {
    let tagged = ptr::from_ref(tag).map_addr(|addr| addr | usize::from(sampled));
    unsafe {
        data.add(size).cast::<*const Tag>().write_unaligned(tagged);
    }
}

unsafe fn read_trailer(data: *mut u8, size: usize) -> (&'static Tag, bool) {
    let tagged = unsafe { data.add(size).cast::<*const Tag>().read_unaligned() };
    (
        unsafe { &*tagged.map_addr(|addr| addr & !1) },
        tagged.addr() & 1 != 0,
    )
}

/// System allocator with exact tag accounting. On alloc, the hot path adds a
/// trailer word to the layout, reads CURRENT, adds to two relaxed counters,
/// branches on their sampling bit and writes the trailer. Free reads the
/// trailer and subtracts from the same counters, on whichever thread frees it.
pub struct TaggedAllocator;

impl TaggedAllocator {
    unsafe fn allocate(&self, layout: Layout, zero: bool) -> *mut u8 {
        let Some(larger) = extended(layout) else {
            return ptr::null_mut();
        };
        let data = unsafe {
            if zero {
                System.alloc_zeroed(larger)
            } else {
                System.alloc(larger)
            }
        };
        if !data.is_null() {
            let tag = CURRENT.try_with(Cell::get).unwrap_or(&UNSCOPED);
            tag.bytes.fetch_add(layout.size() as u64, Ordering::Relaxed);
            let old = tag.allocations.fetch_add(1, Ordering::Relaxed);
            let sampled =
                old & SAMPLE_BIT != 0 && sample_growth(data.addr(), layout.size(), layout.size());
            unsafe {
                write_trailer(data, layout.size(), tag, sampled);
            }
        }
        data
    }
}

// SAFETY: every successful allocation uses the same extended layout for System
// allocation and release. The trailer is beyond the caller's payload and holds
// a process-lifetime tag. The original pointer alignment is preserved.
unsafe impl GlobalAlloc for TaggedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        unsafe { self.allocate(layout, false) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        unsafe { self.allocate(layout, true) }
    }

    unsafe fn dealloc(&self, data: *mut u8, layout: Layout) {
        let (tag, sampled) = unsafe { read_trailer(data, layout.size()) };
        if sampled {
            take_sample(data.addr());
        }
        tag.bytes.fetch_sub(layout.size() as u64, Ordering::Relaxed);
        tag.allocations.fetch_sub(1, Ordering::Relaxed);
        // SAFETY: allocate/realloc could succeed only with this exact layout.
        unsafe {
            System.dealloc(data, extended(layout).unwrap_unchecked());
        }
    }

    unsafe fn realloc(&self, data: *mut u8, old: Layout, size: usize) -> *mut u8 {
        let Ok(new) = Layout::from_size_align(size, old.align()) else {
            return ptr::null_mut();
        };
        let Some(larger) = extended(new) else {
            return ptr::null_mut();
        };
        let (tag, sampled) = unsafe { read_trailer(data, old.size()) };
        // Detach before System can move the block and reuse the old address on
        // another thread. On failure restore the original live allocation.
        let sample = if sampled {
            take_sample(data.addr())
        } else {
            None
        };
        let new_data =
            unsafe { System.realloc(data, extended(old).unwrap_unchecked(), larger.size()) };
        if new_data.is_null() {
            if let Some(old_sample) = sample {
                let restored = insert_sample(data.addr(), old_sample);
                unsafe {
                    write_trailer(data, old.size(), tag, restored);
                }
            }
            return new_data;
        }
        tag.bytes.fetch_add(
            (size as u64).wrapping_sub(old.size() as u64),
            Ordering::Relaxed,
        );
        let tracked = if let Some(sample) = sample {
            insert_sample(
                new_data.addr(),
                Sample {
                    bytes: size,
                    ..sample
                },
            )
        } else {
            size > old.size()
                && tag.allocations.load(Ordering::Relaxed) & SAMPLE_BIT != 0
                && sample_growth(new_data.addr(), size, size - old.size())
        };
        unsafe {
            write_trailer(new_data, size, tag, tracked);
        }
        new_data
    }
}

#[derive(Clone, Copy, Default, Serialize)]
pub struct Counts {
    pub live_bytes: u64,
    pub live_allocations: u64,
}

#[derive(Serialize)]
pub struct Frame {
    pub ip: usize,
    pub symbol: String,
}

#[derive(Serialize)]
pub struct Stack {
    #[serde(flatten)]
    pub counts: Counts,
    pub frames: Vec<Frame>,
}

#[derive(Serialize)]
pub struct HeapSnapshot {
    pub sample_interval_bytes: u64,
    pub dropped_samples: u64,
    pub tags: BTreeMap<String, Counts>,
    pub stacks: Vec<Stack>,
}

fn tag_counts() -> BTreeMap<String, Counts> {
    [&UNSCOPED, &BOOKKEEPING]
        .into_iter()
        .chain(BUCKETS.iter().flat_map(tags_in))
        .map(|tag| (tag.name.to_owned(), tag.counts()))
        .collect()
}

/// Copy counts per stack under the address-index lock. Symbolization and sorting
/// always run after releasing it. Only observed bytes are summed; these totals
/// are useful for call-site attribution, not estimates of the entire Rust heap.
fn ranked_stacks() -> Vec<(RawStack, Counts)> {
    let Some(index) = SAMPLES.get() else {
        return Vec::new();
    };
    let mut totals = HashMap::<RawStack, Counts>::new();
    {
        let index = index.lock().unwrap_or_else(|e| e.into_inner());
        for sample in index.values() {
            let counts = totals.entry(sample.stack).or_default();
            counts.live_allocations += 1;
            counts.live_bytes += sample.bytes as u64;
        }
    }
    let mut entries: Vec<_> = totals.into_iter().collect();
    entries.sort_unstable_by(|(a, av), (b, bv)| {
        bv.live_bytes.cmp(&av.live_bytes).then_with(|| a.cmp(b))
    });
    entries
}

fn frame(ip: usize) -> Frame {
    let mut symbol = None;
    backtrace::resolve(ip as *mut _, |s| {
        if symbol.is_none() {
            symbol = s.name().map(|n| n.to_string());
        }
    });
    Frame {
        ip,
        symbol: symbol.unwrap_or_else(|| format!("0x{ip:x}")),
    }
}

impl RawStack {
    fn frames(&self) -> impl Iterator<Item = Frame> + '_ {
        self.0.iter().copied().take_while(|ip| *ip != 0).map(frame)
    }

    fn label(&self) -> String {
        let mut hash = DefaultHasher::new();
        self.hash(&mut hash);
        // Prefix with a user frame when symbols are present. Preserve a key for
        // stripped binaries and distinguish paths through the same user frame.
        let name = self
            .frames()
            .find(|f| {
                ![
                    "mem_watch::",
                    "backtrace::",
                    "::__rust_",
                    "__rust_alloc",
                    "__rust_realloc",
                    "alloc::",
                    "core::alloc::",
                    "0x",
                ]
                .iter()
                .any(|s| f.symbol.contains(s))
            })
            .map(|f| f.symbol)
            .unwrap_or_else(|| "unsymbolized".into());
        format!(
            "{} [{:016x}]",
            name.chars().take(120).collect::<String>(),
            hash.finish()
        )
    }
}

/// On-demand heap inspection. The recorder still gets all tag totals when stack
/// sampling is off. A stack is symbolized in the process whose IPs it contains.
pub fn snapshot(top: usize) -> Result<HeapSnapshot, &'static str> {
    let interval = INTERVAL.load(Ordering::Relaxed);
    if interval == 0 {
        return Err(
            "allocation stack sampling is off; set H2_MEM_SAMPLE to a byte interval (for example 1M) before starting the process",
        );
    }
    let _internal = enter(&BOOKKEEPING);
    let tags = tag_counts();
    let stacks = ranked_stacks()
        .into_iter()
        .take(top.min(128))
        .map(|(stack, counts)| Stack {
            counts,
            frames: stack.frames().collect(),
        })
        .collect();
    Ok(HeapSnapshot {
        sample_interval_bytes: interval,
        dropped_samples: DROPPED.load(Ordering::Relaxed),
        tags,
        stacks,
    })
}

/// Compact category totals for each flight-recorder row. The key cache keeps
/// stable stacks cheap to record repeatedly. Limit both on-disk cardinality
/// and symbol cache residency when a long session goes through many stacks.
pub fn totals() -> (BTreeMap<String, u64>, BTreeMap<String, u64>) {
    let _internal = enter(&BOOKKEEPING);
    let tags = tag_counts()
        .into_iter()
        .map(|(k, v)| (k, v.live_bytes))
        .collect();
    let mut stacks = BTreeMap::new();
    if INTERVAL.load(Ordering::Relaxed) != 0 {
        static LABELS: OnceLock<Mutex<HashMap<RawStack, String>>> = OnceLock::new();
        let entries = ranked_stacks();
        let mut labels = LABELS
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if labels.len() > 256 {
            labels.clear();
        }
        let mut other = 0;
        for (i, (stack, counts)) in entries.into_iter().enumerate() {
            if i < 32 {
                let name = labels.entry(stack).or_insert_with(|| stack.label());
                stacks.insert(name.clone(), counts.live_bytes);
            } else {
                other += counts.live_bytes;
            }
        }
        if other > 0 {
            stacks.insert("other sampled stacks".into(), other);
        }
    }
    (tags, stacks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nesting_reallocation_and_cross_thread_free_keep_the_owner() {
        let outer = intern("test.heap.outer");
        let inner = intern("test.heap.inner");
        let small = Layout::from_size_align(17, 256).unwrap();
        let big = Layout::from_size_align(97, 256).unwrap();
        let _outer = scope(outer.name);
        let a = unsafe { TaggedAllocator.alloc_zeroed(small) };
        assert!(!a.is_null());
        assert_eq!(a.addr() % 256, 0);
        assert!(
            unsafe { std::slice::from_raw_parts(a, 17) }
                .iter()
                .all(|b| *b == 0)
        );
        unsafe {
            a.write(13);
        }

        let a = {
            let _inner = scope(inner.name);
            let b = unsafe { TaggedAllocator.alloc(small) };
            let a = unsafe { TaggedAllocator.realloc(a, small, big.size()) };
            assert!(!a.is_null());
            assert_eq!(unsafe { a.read() }, 13);
            assert_eq!(outer.counts().live_bytes, 97);
            assert_eq!(inner.counts().live_allocations, 1);
            unsafe {
                TaggedAllocator.dealloc(b, small);
            }
            a
        };
        let b = unsafe { TaggedAllocator.alloc(small) };
        assert_eq!(outer.counts().live_allocations, 2);
        unsafe {
            TaggedAllocator.dealloc(b, small);
        }
        // Carry an exposed address to the worker; the allocation remains live.
        let addr = a.expose_provenance();
        std::thread::spawn(move || unsafe {
            TaggedAllocator.dealloc(ptr::with_exposed_provenance_mut(addr), big);
        })
        .join()
        .unwrap();
        assert_eq!(outer.counts().live_bytes, 0);
        assert_eq!(outer.counts().live_allocations, 0);
        assert_eq!(inner.counts().live_bytes, 0);
    }

    #[test]
    fn sampled_allocation_moves_then_disappears_on_free() {
        let tag = intern("test.heap.sample");
        let _guard = scope(tag.name);
        let old = Layout::from_size_align(5, 16).unwrap();
        let new = Layout::from_size_align(4097, 16).unwrap();
        let a = unsafe { TaggedAllocator.alloc(old) };
        assert!(!a.is_null());
        // Inject a sampled IP independent of platform unwinding and RNG state.
        let mut stack = RawStack([0; MAX_FRAMES]);
        stack.0[0] = 0xcafe;
        assert!(insert_sample(
            a.addr(),
            Sample {
                bytes: old.size(),
                stack
            }
        ));
        unsafe {
            write_trailer(a, old.size(), tag, true);
        }
        let a = unsafe { TaggedAllocator.realloc(a, old, new.size()) };
        assert!(!a.is_null());
        let totals = ranked_stacks();
        let counts = totals.iter().find(|(s, _)| *s == stack).unwrap().1;
        assert_eq!(counts.live_bytes, 4097);
        assert_eq!(counts.live_allocations, 1);
        unsafe {
            TaggedAllocator.dealloc(a, new);
        }
        assert!(!ranked_stacks().iter().any(|(s, _)| *s == stack));
    }

    #[test]
    fn poisson_sampler_consumes_the_stream_across_small_allocations() {
        let mut p = Poisson {
            rng: 42,
            remaining: 10,
        };
        assert!(!p.select(4, 1000));
        assert_eq!(p.remaining, 6);
        assert!(p.select(6, 1000));
        assert!(p.remaining > 0);
    }

    #[test]
    fn trailer_overflow_leaves_the_original_allocation_intact() {
        let tag = intern("test.heap.fail");
        let _guard = scope(tag.name);
        let old = Layout::from_size_align(3, 1).unwrap();
        let a = unsafe { TaggedAllocator.alloc(old) };
        assert!(!a.is_null());
        assert!(unsafe { TaggedAllocator.realloc(a, old, isize::MAX as usize) }.is_null());
        assert_eq!(tag.counts().live_bytes, 3);
        assert_eq!(tag.counts().live_allocations, 1);
        unsafe {
            TaggedAllocator.dealloc(a, old);
        }
    }
}
