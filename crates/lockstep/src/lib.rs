//! The determinism harness for the high-frame-rate work
//! (docs/high-framerate.md, phase 1): a hash of guest RAM at every guest
//! tick and present, a trace of those hashes, and a comparison of two
//! traces that names the first record and the RAM megabytes where they part.
//!
//! The rule it checks: whatever the host does to presentation (60, 120 or
//! uncapped display rates, replayed frames), the guest runs the same
//! instructions on the same schedule, so two runs with the same input
//! produce the same trace. Input comes from `xapi::tap` (record the packets
//! the guest read, replay them by read index).
//!
//! Hashing is opt-in and reads RAM from Rust (uncharged, no guest-visible
//! write). The per-tick observer is a post-hook on the guest's tick
//! function, so it adds dispatch transfers: compare runs that all have it on
//! (or all off), never one of each. Per-present records taken between
//! scheduler slices add nothing to the guest's run.

pub mod synthetic;

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use runtime::exec::OriginalHandler;
use runtime::{Guest, Runtime, TransferAbi, abi, guest::Permissions};

/// RAM is hashed in chunks of this many bytes, so a divergence can name
/// where in physical memory it is.
pub const CHUNK: usize = 1 << 20;
/// Chunks in the Xbox's 64 MiB.
pub const CHUNKS: usize = runtime::guest::RAM_SIZE / CHUNK;
const PAGE: usize = runtime::guest::PAGE_SIZE as usize;

/// A hash of guest physical RAM: one word per [`CHUNK`] and one over all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RamHash {
    pub total: u64,
    /// Per-chunk hashes, [`CHUNKS`] long. A chunk with nothing mapped or
    /// reserved hashes as [`EMPTY`].
    pub chunks: Vec<u64>,
}

/// The hash of a chunk with no page in use.
pub const EMPTY: u64 = 0;

fn fmix(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^ (h >> 33)
}

/// Four independent multiply-rotate lanes over 32-byte blocks. Each step is
/// a bijection of the lane given the input word, and the lanes fold in one
/// at a time through a bijective mix, so any change confined to one lane
/// (in particular any single changed word) always changes the result.
fn hash_bytes(seed: u64, bytes: &[u8]) -> u64 {
    const P: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut lanes = [
        seed ^ 0x243f_6a88_85a3_08d3,
        seed ^ 0x1319_8a2e_0370_7344,
        seed ^ 0xa409_3822_299f_31d0,
        seed ^ 0x082e_fa98_ec4e_6c89,
    ];
    let (blocks, rest) = bytes.as_chunks::<32>();
    for b in blocks {
        for (i, lane) in lanes.iter_mut().enumerate() {
            let w = u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
            *lane = (*lane ^ w).wrapping_mul(P | 1).rotate_left(29);
        }
    }
    let mut tail = [0u8; 32];
    tail[..rest.len()].copy_from_slice(rest);
    for (i, lane) in lanes.iter_mut().enumerate() {
        let w = u64::from_le_bytes(tail[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
        *lane = (*lane ^ w).wrapping_mul(P | 1).rotate_left(29);
    }
    let mut h = fmix(bytes.len() as u64 ^ seed);
    for lane in lanes {
        h = fmix(h ^ lane);
    }
    h
}

/// Which physical pages are in use: mapped by any virtual range or held by
/// a physical reservation. Unused pages are never hashed (they are zero or
/// stale, and the guest cannot reach them).
fn pages_in_use(guest: &Guest) -> Vec<bool> {
    let mut used = vec![false; runtime::guest::RAM_SIZE / PAGE];
    let mut mark = |start: u32, len: u32| {
        let first = start as usize / PAGE;
        let end = (start as usize + len as usize)
            .div_ceil(PAGE)
            .min(used.len());
        for p in used.iter_mut().take(end).skip(first) {
            *p = true;
        }
    };
    for m in guest.mappings() {
        mark(m.physical_start, m.len);
    }
    for r in guest.physical_reservations() {
        mark(r.physical_start, r.len);
    }
    used
}

/// Hashes the physical RAM in use. Holds no lock of its own; the caller
/// holds the memory guard.
pub fn hash_guest(guest: &Guest) -> RamHash {
    let ram = guest.physical_ram();
    let used = pages_in_use(guest);
    let per_chunk = CHUNK / PAGE;
    let mut chunks = Vec::with_capacity(CHUNKS);
    let mut total = fmix(0x6c6f_636b_7374_6570); // "lockstep"
    for (c, pages) in used.chunks(per_chunk).enumerate() {
        let mut h = EMPTY;
        let mut any = false;
        let mut p = 0;
        while p < pages.len() {
            if !pages[p] {
                p += 1;
                continue;
            }
            // A run of used pages, hashed as one span seeded by its place.
            let start = p;
            while p < pages.len() && pages[p] {
                p += 1;
            }
            let at = c * CHUNK + start * PAGE;
            let span = &ram[at..c * CHUNK + p * PAGE];
            h = fmix(h ^ hash_bytes(at as u64, span));
            any = true;
        }
        let h = if any { h | 1 } else { EMPTY };
        total = fmix(total ^ h ^ (c as u64) << 56);
        chunks.push(h);
    }
    RamHash { total, chunks }
}

/// Where a guest keeps its simulation state: the block a hash of "the
/// game" should cover, as opposed to all of RAM ([`hash_guest`]).
///
/// The whole-RAM hash is the strict test (two machines doing the same work
/// have the same RAM). This one is the narrow test: it stays equal when
/// only presentation differs, and it is small enough to send every tick
/// (docs/live-coop.md, docs/theater.md "What the recorder adds").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimRegion {
    /// `len` bytes at a fixed guest address.
    Fixed { base: u32, len: u32 },
    /// `len` bytes at the address held in the guest word `pointer`.
    Indirect { pointer: u32, len: u32 },
}

impl SimRegion {
    /// Halo 2's game-state arena: `0x3fe000` bytes, the block checkpoints
    /// save, behind the base word at `0x004e6080` (docs/engine-map.md,
    /// `game_state_arena_initialize` `0x00123b30`; docs/checkpoints.md).
    /// The base word is from a static reading and has not been checked in
    /// a running game.
    pub const HALO2: SimRegion = SimRegion::Indirect {
        pointer: 0x004e_6080,
        len: 0x003f_e000,
    };
}

/// The hash of `region`, or `None` while it is not there to read (before
/// the guest has set the pointer, or with the range not mapped). Reads
/// guest memory from Rust: uncharged, and it writes nothing.
pub fn hash_sim(guest: &Guest, region: SimRegion) -> Option<u64> {
    let (base, len) = match region {
        SimRegion::Fixed { base, len } => (base, len),
        SimRegion::Indirect { pointer, len } => (guest.u32(pointer).ok()?, len),
    };
    if base == 0 {
        return None;
    }
    let mut bytes = vec![0u8; len as usize];
    guest.read_bytes(base, &mut bytes, 1).ok()?;
    Some(hash_bytes(u64::from(len), &bytes))
}

/// What a record marks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The guest's tick function returned (Halo 2: `game_tick` `0x00137fe0`).
    Tick,
    /// A guest present (Swap) went out, taken at the next slice boundary.
    Present,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Tick => "tick",
            Kind::Present => "present",
        }
    }
}

/// One observation of the guest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub kind: Kind,
    /// The how-manyth record of this kind, from 0.
    pub seq: u64,
    /// Logical guest time (100 ns ticks), when the runtime has a clock.
    pub clock: Option<u64>,
    /// The guest's own tick counter (Halo 2: `game_time + 8`), when read.
    pub game_tick: Option<u32>,
    pub hash: RamHash,
}

/// A run's records in the order they were taken.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Trace {
    pub records: Vec<Record>,
}

/// The first line of a trace file.
pub const HEADER: &str = "# h2 lockstep trace v1";

impl Trace {
    /// One line per record:
    /// `tick 3 clock=1000000 game=11 hash=<16 hex> chunks=<64 x 16 hex, comma separated>`.
    /// `-` stands for a clock or game tick that was not read.
    pub fn to_text(&self) -> String {
        let mut s = String::with_capacity(64 + self.records.len() * (CHUNKS * 17 + 80));
        s.push_str(HEADER);
        s.push('\n');
        let opt = |v: Option<u64>| v.map_or("-".to_owned(), |v| v.to_string());
        for r in &self.records {
            let _ = write!(
                s,
                "{} {} clock={} game={} hash={:016x} chunks=",
                r.kind.name(),
                r.seq,
                opt(r.clock),
                opt(r.game_tick.map(u64::from)),
                r.hash.total
            );
            for (i, c) in r.hash.chunks.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                let _ = write!(s, "{c:016x}");
            }
            s.push('\n');
        }
        s
    }

    pub fn parse(text: &str) -> Result<Trace, String> {
        let mut lines = text.lines().enumerate();
        match lines.next() {
            Some((_, h)) if h.trim() == HEADER => {}
            _ => {
                return Err(format!(
                    "not a lockstep trace (first line is not {HEADER:?})"
                ));
            }
        }
        let mut records = Vec::new();
        for (n, line) in lines {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let bad = |what: &str| format!("line {}: {what}: {line:?}", n + 1);
            let mut words = line.split_whitespace();
            let kind = match words.next() {
                Some("tick") => Kind::Tick,
                Some("present") => Kind::Present,
                _ => return Err(bad("unknown record kind")),
            };
            let seq = words
                .next()
                .and_then(|w| w.parse().ok())
                .ok_or_else(|| bad("sequence number"))?;
            let mut field = |key: &str| -> Result<&str, String> {
                words
                    .next()
                    .and_then(|w| w.strip_prefix(key))
                    .and_then(|w| w.strip_prefix('='))
                    .ok_or_else(|| bad(key))
            };
            let opt = |v: &str| -> Result<Option<u64>, String> {
                if v == "-" {
                    Ok(None)
                } else {
                    v.parse().map(Some).map_err(|_| bad("number"))
                }
            };
            let clock = opt(field("clock")?)?;
            let game_tick = opt(field("game")?)?
                .map(u32::try_from)
                .transpose()
                .map_err(|_| bad("game tick"))?;
            let total = u64::from_str_radix(field("hash")?, 16).map_err(|_| bad("hash"))?;
            let chunks = field("chunks")?
                .split(',')
                .map(|c| u64::from_str_radix(c, 16))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| bad("chunk hash"))?;
            if chunks.len() != CHUNKS {
                return Err(bad("chunk count"));
            }
            records.push(Record {
                kind,
                seq,
                clock,
                game_tick,
                hash: RamHash { total, chunks },
            });
        }
        Ok(Trace { records })
    }

    pub fn ticks(&self) -> usize {
        self.records.iter().filter(|r| r.kind == Kind::Tick).count()
    }
}

/// How two traces compare.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Same records, all of them.
    Identical { records: usize },
    /// Record `index` is the first that differs.
    Diverged {
        index: usize,
        a: Box<Record>,
        b: Box<Record>,
        /// Chunks (MiB of physical RAM) whose hashes differ.
        chunks: Vec<usize>,
    },
    /// One trace is a prefix of the other.
    Truncated { common: usize, a: usize, b: usize },
}

impl Verdict {
    pub fn identical(&self) -> bool {
        matches!(self, Verdict::Identical { .. })
    }
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let opt = |v: Option<u64>| v.map_or("-".to_owned(), |v| v.to_string());
        match self {
            Verdict::Identical { records } => write!(f, "identical: {records} records"),
            Verdict::Truncated { common, a, b } => {
                write!(
                    f,
                    "identical for {common} records, then one run ends ({a} against {b} records)"
                )
            }
            Verdict::Diverged {
                index,
                a,
                b,
                chunks,
            } => {
                write!(
                    f,
                    "diverged at record {index}: {} {} (clock {} / {}, game tick {} / {})",
                    a.kind.name(),
                    a.seq,
                    opt(a.clock),
                    opt(b.clock),
                    opt(a.game_tick.map(u64::from)),
                    opt(b.game_tick.map(u64::from))
                )?;
                if a.kind != b.kind || a.seq != b.seq {
                    write!(f, "; the other run has {} {} there", b.kind.name(), b.seq)?;
                }
                if !chunks.is_empty() {
                    let mib: Vec<String> = chunks
                        .iter()
                        .map(|c| format!("{:#010x}", c * CHUNK))
                        .collect();
                    write!(f, "; RAM differs in the MiB at {}", mib.join(", "))?;
                }
                Ok(())
            }
        }
    }
}

/// Compares two runs record by record.
pub fn compare(a: &Trace, b: &Trace) -> Verdict {
    for (index, (ra, rb)) in a.records.iter().zip(&b.records).enumerate() {
        if ra != rb {
            let chunks = ra
                .hash
                .chunks
                .iter()
                .zip(&rb.hash.chunks)
                .enumerate()
                .filter(|(_, (x, y))| x != y)
                .map(|(i, _)| i)
                .collect();
            return Verdict::Diverged {
                index,
                a: Box::new(ra.clone()),
                b: Box::new(rb.clone()),
                chunks,
            };
        }
    }
    let (la, lb) = (a.records.len(), b.records.len());
    if la == lb {
        Verdict::Identical { records: la }
    } else {
        Verdict::Truncated {
            common: la.min(lb),
            a: la,
            b: lb,
        }
    }
}

/// Where the guest's tick is and how to read its tick counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TickHook {
    /// The tick function (called once per simulated tick).
    pub entry: u32,
    /// A private, otherwise unused page address for the observer's call back
    /// into the original (mapped read-execute by [`install_tick_observer`]).
    pub shadow: u32,
    /// The guest word holding a pointer to the game-time record, whose
    /// `+8` is the tick count. `None`: no game tick in the records.
    pub game_time: Option<u32>,
}

impl TickHook {
    /// Halo 2: `game_tick` `0x00137fe0`, the game-time pointer at
    /// `0x00510c54` (docs/high-framerate.md section 1). Shadow page
    /// `0xa0ff9000` (boot's shadow list is in crates/boot/src/lib.rs).
    pub const HALO2: TickHook = TickHook {
        entry: 0x0013_7fe0,
        shadow: 0xa0ff_9000,
        game_time: Some(0x0051_0c54),
    };
}

/// Collects a run's records. Shared between the tick observer (guest
/// thread), the slice loop's present records and whoever writes the trace.
#[derive(Debug, Default)]
pub struct Recorder {
    trace: Mutex<Trace>,
    ticks: Arc<AtomicU64>,
    presents: AtomicU64,
    game_time: Option<u32>,
}

impl Recorder {
    pub fn new(game_time: Option<u32>) -> Arc<Self> {
        Arc::new(Self {
            game_time,
            ..Default::default()
        })
    }

    /// The count of ticks recorded so far, for stamping input reads with
    /// the tick they fall in (`xapi::tap::Recorder::with_ticks`).
    pub fn tick_counter(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.ticks)
    }

    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }

    fn game_tick(&self, guest: &Guest) -> Option<u32> {
        let ptr = guest.u32(self.game_time?).ok()?;
        guest.u32(ptr.wrapping_add(8)).ok()
    }

    /// Hashes `guest` and appends a record.
    pub fn record(&self, kind: Kind, guest: &Guest, clock: Option<u64>) {
        let seq = match kind {
            Kind::Tick => self.ticks.fetch_add(1, Ordering::Relaxed),
            Kind::Present => self.presents.fetch_add(1, Ordering::Relaxed),
        };
        let record = Record {
            kind,
            seq,
            clock,
            game_tick: self.game_tick(guest),
            hash: hash_guest(guest),
        };
        self.trace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .records
            .push(record);
    }

    /// Records presents up to `presents` (the count of guest presents so
    /// far), one record per present not yet recorded. Call between slices.
    pub fn record_presents(&self, presents: u64, guest: &Guest, clock: Option<u64>) {
        while self.presents.load(Ordering::Relaxed) < presents {
            self.record(Kind::Present, guest, clock);
        }
    }

    /// The records so far.
    pub fn trace(&self) -> Trace {
        self.trace.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// Wraps the guest's tick function: the original runs unchanged, and when it
/// returns `recorder` hashes RAM before the caller continues.
///
/// The original return address is popped and the call goes through
/// `hook.shadow`, so during the tick the guest stack slot holds the
/// continuation token instead of the caller's address, and the observer's
/// own transfers count against the slice budget. Both are the same in every
/// run that has the observer on. Fails when the tick has no generated
/// original or something else already overrides it (Live telemetry wraps
/// the same function).
pub fn install_tick_observer(
    rt: &Runtime,
    hook: TickHook,
    recorder: Arc<Recorder>,
) -> Result<(), String> {
    // resolve_original takes the dispatch read lock: not under edit_dispatch.
    let original = || {
        rt.resolve_original(hook.entry).ok_or_else(|| {
            format!(
                "lockstep: the tick function {:#x} has no generated original",
                hook.entry
            )
        })
    };
    let (via_shadow, fallback): (OriginalHandler, OriginalHandler) = (original()?, original()?);
    rt.memory()
        .map_zeroed(hook.shadow, runtime::guest::PAGE_SIZE, Permissions::RX)
        .map_err(|e| {
            format!(
                "lockstep: mapping the shadow page {:#x}: {e:?}",
                hook.shadow
            )
        })?;
    rt.edit_dispatch(|d| {
        for a in [hook.entry, hook.shadow] {
            if let Some(name) = d.override_name(a) {
                return Err(format!("lockstep: {a:#x} is already overridden ({name})"));
            }
        }
        let entry = hook.entry;
        let _ = d.register_override(hook.shadow, "lockstep tick original", move |cpu, ctx, _| {
            Ok(via_shadow.invoke(cpu, ctx, entry))
        });
        let _ = d.register_override(hook.entry, "lockstep tick observer", move |cpu, ctx, _| {
            let Ok(return_pc) = ctx.memory().u32(cpu.esp) else {
                return Ok(fallback.invoke(cpu, ctx, entry));
            };
            cpu.esp = cpu.esp.wrapping_add(4);
            let recorder = Arc::clone(&recorder);
            let transfer = {
                let runtime = Arc::clone(ctx.runtime());
                let mut memory = runtime.memory();
                ctx.call_guest(cpu, &mut memory, hook.shadow, move |_cpu, ctx| {
                    let clock = ctx.runtime().guest_time();
                    let runtime = Arc::clone(ctx.runtime());
                    recorder.record(Kind::Tick, &runtime.memory(), clock);
                    Ok(TransferAbi::ret(return_pc))
                })
            };
            match transfer {
                Ok(t) if t.kind != abi::FAULT => Ok(t),
                _ => {
                    cpu.esp = cpu.esp.wrapping_sub(4);
                    Ok(fallback.invoke(cpu, ctx, entry))
                }
            }
        });
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guest_with(pages: &[(u32, u32)]) -> Guest {
        let mut g = Guest::new();
        for &(va, len) in pages {
            g.map_zeroed(va, len, Permissions::RW).unwrap();
        }
        g
    }

    #[test]
    fn equal_memory_hashes_equal_and_any_word_changes_it() {
        let mut a = guest_with(&[(0x10000, 0x20000)]);
        let b = guest_with(&[(0x10000, 0x20000)]);
        assert_eq!(hash_guest(&a), hash_guest(&b));
        let base = hash_guest(&a);
        for addr in [0x10000, 0x10004, 0x1fffc, 0x2fffc] {
            a.set_u32(addr, 1).unwrap();
            let h = hash_guest(&a);
            assert_ne!(h.total, base.total, "{addr:#x}");
            a.set_u32(addr, 0).unwrap();
        }
        assert_eq!(hash_guest(&a), base);
    }

    #[test]
    fn swapping_two_words_is_seen() {
        let mut a = guest_with(&[(0x10000, 0x1000)]);
        let mut b = guest_with(&[(0x10000, 0x1000)]);
        a.set_u32(0x10000, 7).unwrap();
        a.set_u32(0x10008, 9).unwrap();
        b.set_u32(0x10000, 9).unwrap();
        b.set_u32(0x10008, 7).unwrap();
        assert_ne!(hash_guest(&a), hash_guest(&b));
    }

    #[test]
    fn the_changed_chunk_is_named() {
        let mut a = guest_with(&[(0x10000, 0x1000)]);
        let b = guest_with(&[(0x10000, 0x1000)]);
        // The first mapping's physical page is the allocator's choice: find it.
        let phys = a.mappings()[0].physical_start as usize;
        a.set_u32(0x10010, 5).unwrap();
        let (ha, hb) = (hash_guest(&a), hash_guest(&b));
        let differ: Vec<usize> = (0..CHUNKS)
            .filter(|&c| ha.chunks[c] != hb.chunks[c])
            .collect();
        assert_eq!(differ, vec![phys / CHUNK]);
        assert_eq!(ha.chunks.len(), CHUNKS);
        assert!(hb.chunks.iter().filter(|&&c| c != EMPTY).count() >= 1);
    }

    fn record(kind: Kind, seq: u64, total: u64) -> Record {
        let mut chunks = vec![EMPTY; CHUNKS];
        chunks[3] = total;
        Record {
            kind,
            seq,
            clock: Some(seq * 333_333),
            game_tick: Some(seq as u32),
            hash: RamHash { total, chunks },
        }
    }

    #[test]
    fn traces_round_trip_through_text() {
        let mut t = Trace {
            records: vec![record(Kind::Tick, 0, 1), record(Kind::Present, 0, 2)],
        };
        t.records[1].clock = None;
        t.records[1].game_tick = None;
        let text = t.to_text();
        assert!(text.starts_with(HEADER));
        assert_eq!(Trace::parse(&text).unwrap(), t);
        assert!(Trace::parse("tick 0").is_err());
        assert!(Trace::parse(&format!("{HEADER}\ntick x clock=- game=- hash=0 chunks=0")).is_err());
        assert!(
            Trace::parse(&format!("{HEADER}\ntick 0 clock=- game=- hash=0 chunks=0")).is_err(),
            "chunk count"
        );
    }

    #[test]
    fn compare_names_the_first_divergence() {
        let a = Trace {
            records: (0..5).map(|i| record(Kind::Tick, i, 100 + i)).collect(),
        };
        assert_eq!(compare(&a, &a.clone()), Verdict::Identical { records: 5 });
        let mut b = a.clone();
        b.records[3].hash.total ^= 1;
        b.records[3].hash.chunks[3] ^= 1;
        b.records[4].hash.total ^= 1;
        match compare(&a, &b) {
            Verdict::Diverged { index, chunks, .. } => {
                assert_eq!(index, 3);
                assert_eq!(chunks, vec![3]);
            }
            v => panic!("{v:?}"),
        }
        let text = compare(&a, &b).to_string();
        assert!(
            text.contains("record 3") && text.contains("0x00300000"),
            "{text}"
        );
        let mut c = a.clone();
        c.records.truncate(2);
        assert_eq!(
            compare(&a, &c),
            Verdict::Truncated {
                common: 2,
                a: 5,
                b: 2
            }
        );
        let mut d = a.clone();
        d.records[1].clock = Some(1);
        assert!(
            matches!(compare(&a, &d), Verdict::Diverged { index: 1, .. }),
            "the clock is guest-visible"
        );
    }

    #[test]
    fn presents_are_recorded_once_each() {
        let g = guest_with(&[(0x10000, 0x1000)]);
        let r = Recorder::new(None);
        r.record_presents(2, &g, Some(5));
        r.record_presents(2, &g, Some(6));
        r.record_presents(3, &g, None);
        let t = r.trace();
        assert_eq!(
            t.records
                .iter()
                .map(|r| (r.kind, r.seq))
                .collect::<Vec<_>>(),
            [(Kind::Present, 0), (Kind::Present, 1), (Kind::Present, 2)]
        );
        assert_eq!(t.records[0].game_tick, None);
    }

    #[test]
    fn the_simulation_hash_covers_its_region_and_nothing_else() {
        let mut g = guest_with(&[(0x10000, 0x4000), (0x40000, 0x1000)]);
        let fixed = SimRegion::Fixed {
            base: 0x11000,
            len: 0x2000,
        };
        let h = hash_sim(&g, fixed).unwrap();
        // Outside the region, before and after: no change.
        g.set_u32(0x10ffc, 1).unwrap();
        g.set_u32(0x13000, 1).unwrap();
        assert_eq!(hash_sim(&g, fixed), Some(h));
        // The first and the last word of it: each changes the hash.
        g.set_u32(0x11000, 1).unwrap();
        let first = hash_sim(&g, fixed).unwrap();
        assert_ne!(first, h);
        g.set_u32(0x12ffc, 1).unwrap();
        assert_ne!(hash_sim(&g, fixed), Some(first));
        // The whole-RAM hash saw every one of those writes.
        assert_ne!(
            hash_guest(&g),
            hash_guest(&guest_with(&[(0x10000, 0x4000), (0x40000, 0x1000)]))
        );
    }

    #[test]
    fn an_indirect_region_follows_its_pointer_and_waits_for_it() {
        let mut g = guest_with(&[(0x10000, 0x4000), (0x40000, 0x1000)]);
        let region = SimRegion::Indirect {
            pointer: 0x40000,
            len: 0x1000,
        };
        assert_eq!(hash_sim(&g, region), None, "the pointer is not set yet");
        g.set_u32(0x40000, 0x12000).unwrap();
        let h = hash_sim(&g, region).unwrap();
        assert_eq!(
            Some(h),
            hash_sim(
                &g,
                SimRegion::Fixed {
                    base: 0x12000,
                    len: 0x1000
                }
            )
        );
        g.set_u32(0x12800, 7).unwrap();
        assert_ne!(hash_sim(&g, region), Some(h));
        // Pointing outside mapped memory, or running off its end: no hash.
        g.set_u32(0x40000, 0x9000_0000).unwrap();
        assert_eq!(hash_sim(&g, region), None);
        g.set_u32(0x40000, 0x13800).unwrap();
        assert_eq!(hash_sim(&g, region), None);
    }
}
