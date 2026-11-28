//! The frame guard, `impl.sim.frame-guard` (off): the first rule of
//! docs/engine/sim-render-split.md ("The one rule", "The proof") as a test
//! mode. A frame step (`render_frame_and_present`, `0x00223240`) leaves
//! every byte of the game-state arena ([`SimRegion::HALO2`]) as it found
//! it, and the `SimExt` digest the arena hash folds in.
//!
//! At the frame step's entry [`Guard::enter`] drains a write watch over the
//! arena's physical pages (the 4 KiB write tracker, [`runtime::watch`]),
//! copies the arena and arms the access trap ([`runtime::trap`]) for writes
//! on its virtual pages. At the exit [`Guard::leave`] compares each page the
//! watch saw written with the copy. A page whose bytes changed is a
//! [`Finding`]: its address, the pools its changed bytes lie in
//! ([`find_pools`]) and its writer, the first guest `pc` the trap logged on
//! it, else the host. A page written with the bytes it held is not one.
//!
//! A finding whose pools are all in the allowlist (`frame-guard.toml`) is
//! allowed; any other is a failure, printed once per pools and writer as a
//! `frame-guard:` line. At a game's end [`Guard::report`] gives the totals
//! and the allowlist entries no frame step wrote, which go. The list only
//! shrinks: [`MAX_ALLOWED`] bounds it as `tick-ledger.toml` bounds the tick
//! driver's guest phases.
//!
//! The trap is the one `render-census` and the host audit use, so the
//! guard runs alone with it. A write through another virtual alias of an
//! arena page dirties the watch but not the trap: its writer reads
//! `unseen`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, Mutex};

use runtime::{Guest, TrapLog, WatchId};

use crate::SimRegion;
use crate::pools::{Pool, find_pools};

pub const ID: &str = "frame-guard";
/// The trap and watch page.
pub const PAGE: usize = 0x1000;
/// The most entries `frame-guard.toml` may hold. Lower it when an entry
/// goes; it is never raised.
pub const MAX_ALLOWED: usize = 2;
/// Trap hits kept per frame step: two per arena page is the most it logs.
pub const LOG_CAP: usize = 1 << 13;
/// Failure lines printed per process; the counts go on.
pub const PRINT_CAP: usize = 200;
/// The allowlist's text.
pub const ALLOWLIST: &str = include_str!("../frame-guard.toml");

/// Who changed a page first in the frame step.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Writer {
    /// Guest code at this `pc`.
    Guest(u32),
    /// Host code: a Rust port, HLE or the D3D layer; `hle` is the Rust
    /// handler it ran in, or 0.
    Host { hle: u32 },
    /// The trap logged no write on the page (another alias, or a raw
    /// physical writer).
    Unseen,
}

impl fmt::Display for Writer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Writer::Guest(pc) => write!(f, "pc {pc:#010x}"),
            Writer::Host { hle: 0 } => f.write_str("host"),
            Writer::Host { hle } => write!(f, "host (handler {hle:#010x})"),
            Writer::Unseen => f.write_str("unseen"),
        }
    }
}

/// One arena page a frame step changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// The page's guest virtual address.
    pub page: u32,
    /// Its offset from the arena's base.
    pub offset: u32,
    /// The first changed byte's offset in the page, and how many changed.
    pub first: u32,
    pub changed: u32,
    /// The pools the changed bytes lie in, in arena order.
    pub pools: Vec<String>,
    pub writer: Writer,
    /// Every pool is in the allowlist.
    pub allowed: bool,
}

impl Finding {
    /// `page 0x80123000 (arena +0x45000) pool object +0x1cc, 4 bytes, by pc 0x00133790`.
    pub fn line(&self) -> String {
        format!(
            "page {:#010x} (arena +{:#x}) pool {} +{:#x}, {} bytes, by {}{}",
            self.page,
            self.offset,
            self.pools.join(", "),
            self.first,
            self.changed,
            self.writer,
            if self.allowed { " (allowed)" } else { "" },
        )
    }
}

/// One allowlist entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowed {
    /// The pool's datum array name, as [`find_pools`] names it (no `#n`).
    pub pool: String,
    /// The slice of sim-render-split.md that removes it.
    pub slice: u8,
    pub why: String,
}

/// `frame-guard.toml`, parsed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Allowlist(pub Vec<Allowed>);

/// The slices of sim-render-split.md, "Slices".
const SLICES: std::ops::RangeInclusive<u8> = 1..=12;
/// A datum array's name is at most this long (0x20 bytes with its NUL).
const NAME_MAX: usize = 31;

impl Allowlist {
    /// [`ALLOWLIST`], parsed. It is checked by the crate's tests.
    pub fn builtin() -> Allowlist {
        Allowlist::parse(ALLOWLIST).expect("frame-guard.toml parses")
    }

    /// Lines `"pool" = "slice: why"` under one `[allowed]` table; `#`
    /// comments and blank lines. A pool is named once, at most
    /// [`NAME_MAX`] characters; the slice is one of sim-render-split.md's.
    pub fn parse(text: &str) -> Result<Allowlist, String> {
        let mut out = Vec::new();
        let mut table = false;
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            let at = || format!("frame-guard.toml line {}", n + 1);
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line == "[allowed]" {
                if table {
                    return Err(format!("{}: [allowed] twice", at()));
                }
                table = true;
                continue;
            }
            if !table {
                return Err(format!("{}: an entry before [allowed]", at()));
            }
            let (key, value) = line
                .split_once(" = ")
                .ok_or_else(|| format!("{}: not `\"pool\" = \"slice: why\"`", at()))?;
            let quoted = |s: &str| {
                s.strip_prefix('"')
                    .and_then(|s| s.strip_suffix('"'))
                    .filter(|s| !s.contains('"'))
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{}: {s} is not one quoted string", at()))
            };
            let (pool, value) = (quoted(key.trim())?, quoted(value.trim())?);
            let (slice, why) = value
                .split_once(": ")
                .ok_or_else(|| format!("{}: the value is `slice: why`", at()))?;
            let slice: u8 = slice
                .parse()
                .ok()
                .filter(|s| SLICES.contains(s))
                .ok_or_else(|| {
                    format!("{}: {slice} is not a slice of sim-render-split.md", at())
                })?;
            if pool.is_empty() || pool.len() > NAME_MAX || pool.contains('#') {
                return Err(format!("{}: {pool:?} is not a datum array name", at()));
            }
            if out.iter().any(|a: &Allowed| a.pool == pool) {
                return Err(format!("{}: {pool} twice", at()));
            }
            out.push(Allowed {
                pool,
                slice,
                why: why.to_owned(),
            });
        }
        Ok(Allowlist(out))
    }

    /// Whether the pool `name` (a `#n` repeat counts as its array) is
    /// listed.
    pub fn allows(&self, name: &str) -> bool {
        let base = name.split('#').next().unwrap_or(name);
        self.0.iter().any(|a| a.pool == base)
    }
}

/// The pools the bytes at `offsets` lie in, by index into `pools` (sorted
/// by offset, covering the block), once each, in order.
fn pools_of(pools: &[Pool], offsets: impl Iterator<Item = usize>) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for at in offsets {
        let i = pools
            .partition_point(|p| p.offset as usize <= at)
            .saturating_sub(1);
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out.sort_unstable();
    out
}

/// The findings of one frame step: each page in `pages` (indices into the
/// arena at `base`) whose bytes in `after` differ from `before`, named by
/// `pools` (of `before`) and `writer`.
pub fn compare(
    base: u32,
    before: &[u8],
    after: &[u8],
    pages: &[usize],
    pools: &[Pool],
    writer: impl Fn(usize) -> Writer,
    allow: &Allowlist,
) -> Vec<Finding> {
    let mut out = Vec::new();
    for &page in pages {
        let start = page * PAGE;
        let end = (start + PAGE).min(before.len()).min(after.len());
        if start >= end {
            continue;
        }
        let changed: Vec<usize> = (start..end).filter(|&i| before[i] != after[i]).collect();
        let Some(&first) = changed.first() else {
            continue;
        };
        let names: Vec<String> = pools_of(pools, changed.iter().copied())
            .into_iter()
            .filter_map(|i| pools.get(i).map(|p| p.name.clone()))
            .collect();
        let allowed = !names.is_empty() && names.iter().all(|n| allow.allows(n));
        out.push(Finding {
            page: base.wrapping_add(start as u32),
            offset: start as u32,
            first: (first - start) as u32,
            changed: changed.len() as u32,
            pools: names,
            writer: writer(page),
            allowed,
        });
    }
    out
}

/// The writer of each virtual page the trap logged a write on: the first
/// guest `pc`, else the first host hit.
pub fn writers(log: &TrapLog) -> HashMap<u32, Writer> {
    let mut out: HashMap<u32, Writer> = HashMap::new();
    for h in log.hits.iter().filter(|h| h.write) {
        let first = h.addr >> 12;
        let last = ((h.addr as u64 + h.len.max(1) as u64 - 1) >> 12) as u32;
        let w = if h.pc != 0 {
            Writer::Guest(h.pc)
        } else {
            Writer::Host { hle: h.hle }
        };
        for page in first..=last {
            let slot = out.entry(page).or_insert(w);
            if matches!(*slot, Writer::Host { .. }) && matches!(w, Writer::Guest(_)) {
                *slot = w;
            }
        }
    }
    out
}

/// Totals since the process started.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Frame steps guarded (the arena there at the entry).
    pub frames: u64,
    /// Arena pages the watch saw written, and of those, changed.
    pub pages_written: u64,
    pub pages_changed: u64,
    /// Changed pages the allowlist covers, and the others.
    pub allowed: u64,
    pub failures: u64,
    /// Frame steps that changed the `SimExt` digest.
    pub ext_changed: u64,
    /// Changed pages by pool (allowed or not).
    pub by_pool: BTreeMap<String, u64>,
}

/// What one frame step left.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub findings: Vec<Finding>,
    /// The `SimExt` digest differs from the entry's.
    pub ext_changed: bool,
}

impl Frame {
    /// No failure: every changed page allowed, the digest the same.
    pub fn clean(&self) -> bool {
        !self.ext_changed && self.findings.iter().all(|f| f.allowed)
    }
}

/// The guard's state across frame steps.
pub struct Guard {
    region: SimRegion,
    allow: Allowlist,
    watch: Option<WatchId>,
    /// The arena base the watch covers, and its folded pages' indices.
    base: u32,
    folded: HashMap<u32, usize>,
    before: Vec<u8>,
    after: Vec<u8>,
    ext: Option<u64>,
    open: bool,
    stats: Stats,
    printed: HashSet<(Vec<String>, Writer)>,
}

impl Guard {
    pub fn new(region: SimRegion, allow: Allowlist) -> Guard {
        Guard {
            region,
            allow,
            watch: None,
            base: 0,
            folded: HashMap::new(),
            before: Vec::new(),
            after: Vec::new(),
            ext: None,
            open: false,
            stats: Stats::default(),
            printed: HashSet::new(),
        }
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    fn arena(&self, g: &Guest) -> Option<(u32, u32)> {
        match self.region {
            SimRegion::Fixed { base, len } => Some((base, len)),
            SimRegion::Indirect { pointer, len } => {
                let base = g.u32(pointer).ok()?;
                (base != 0).then_some((base, len))
            }
        }
    }

    /// Watches the physical pages of the arena at `base`.
    fn subscribe(&mut self, g: &mut Guest, base: u32, len: u32) -> Result<WatchId, String> {
        if let Some(id) = self.watch.take() {
            g.unsubscribe_writes(id);
        }
        self.folded.clear();
        self.base = 0;
        let id = g.subscribe_writes("frame guard");
        self.watch = Some(id);
        for i in 0..(len as usize).div_ceil(PAGE) {
            let va = base.wrapping_add((i * PAGE) as u32);
            let pa = g
                .fold_address(va)
                .ok_or_else(|| format!("arena page {va:#010x} is not mapped"))?;
            g.watch_writes(id, pa & !(PAGE as u32 - 1), PAGE as u32)
                .map_err(|e| format!("arena page {va:#010x}: {e}"))?;
            self.folded.insert(pa & !(PAGE as u32 - 1), i);
        }
        self.base = base;
        Ok(id)
    }

    /// The frame step's entry: drain the watch, copy the arena, arm the
    /// trap on it. Nothing is armed while the arena is not there.
    pub fn enter(&mut self, g: &mut Guest) -> Result<(), String> {
        self.open = false;
        let Some((base, len)) = self.arena(g) else {
            return Ok(());
        };
        let id = match self.watch {
            Some(id) if self.base == base => id,
            _ => self.subscribe(g, base, len)?,
        };
        g.take_dirty(id).map_err(|e| e.to_string())?;
        self.before.resize(len as usize, 0);
        g.read_bytes(base, &mut self.before, 1)
            .map_err(|e| format!("arena read: {e:?}"))?;
        self.ext = sim_world::ext::digest();
        g.install_trap(LOG_CAP);
        let _ = g.take_trap_hits();
        g.arm_writes(base, len);
        self.open = true;
        Ok(())
    }

    /// The frame step's exit: the changed pages since [`enter`](Self::enter),
    /// the trap disarmed. A frame with no entry (the arena missing there)
    /// finds nothing.
    pub fn leave(&mut self, g: &mut Guest) -> Result<Frame, String> {
        if !std::mem::take(&mut self.open) {
            return Ok(Frame::default());
        }
        let log = g.take_trap_hits();
        g.disarm_all();
        let id = self.watch.ok_or("no watch")?;
        let dirty = g.take_dirty(id).map_err(|e| e.to_string())?;
        let mut pages: Vec<usize> = dirty
            .iter()
            .flat_map(|r| (r.addr..r.addr + r.len).step_by(PAGE))
            .filter_map(|pa| self.folded.get(&pa).copied())
            .collect();
        pages.sort_unstable();
        pages.dedup();
        let base = self.base;
        self.after.resize(self.before.len(), 0);
        g.read_bytes(base, &mut self.after, 1)
            .map_err(|e| format!("arena read: {e:?}"))?;
        let pools = find_pools(&self.before);
        let by_page = writers(&log);
        let writer = |i: usize| {
            let va = base.wrapping_add((i * PAGE) as u32);
            by_page.get(&(va >> 12)).copied().unwrap_or(Writer::Unseen)
        };
        let findings = compare(
            base,
            &self.before,
            &self.after,
            &pages,
            &pools,
            writer,
            &self.allow,
        );
        let frame = Frame {
            findings,
            ext_changed: sim_world::ext::digest() != self.ext,
        };
        let s = &mut self.stats;
        s.frames += 1;
        s.pages_written += pages.len() as u64;
        s.pages_changed += frame.findings.len() as u64;
        s.ext_changed += u64::from(frame.ext_changed);
        for f in &frame.findings {
            if f.allowed {
                s.allowed += 1;
            } else {
                s.failures += 1;
            }
            for p in &f.pools {
                *s.by_pool
                    .entry(p.split('#').next().unwrap_or(p).to_owned())
                    .or_default() += 1;
            }
        }
        Ok(frame)
    }

    /// The failure lines of `frame` not printed before: one per pools and
    /// writer, at most [`PRINT_CAP`] in all.
    pub fn new_failures(&mut self, frame: &Frame) -> Vec<String> {
        let mut out = Vec::new();
        if frame.ext_changed
            && self
                .printed
                .insert((vec!["sim_ext".into()], Writer::Unseen))
        {
            out.push("the SimExt digest changed in a frame step".to_owned());
        }
        for f in frame.findings.iter().filter(|f| !f.allowed) {
            if self.printed.len() >= PRINT_CAP {
                break;
            }
            if self.printed.insert((f.pools.clone(), f.writer)) {
                out.push(f.line());
            }
        }
        out
    }

    /// The totals and the allowlist entries no frame step changed: the
    /// game-end line.
    pub fn report(&self) -> String {
        let s = &self.stats;
        let unused: Vec<&str> = self
            .allow
            .0
            .iter()
            .filter(|a| !s.by_pool.contains_key(&a.pool))
            .map(|a| a.pool.as_str())
            .collect();
        let failing: BTreeSet<&str> = s
            .by_pool
            .keys()
            .map(String::as_str)
            .filter(|p| !self.allow.allows(p))
            .collect();
        let mut line = format!(
            "{} frames, {} pages written, {} changed: {} allowed, {} failures, SimExt changed in {}",
            s.frames, s.pages_written, s.pages_changed, s.allowed, s.failures, s.ext_changed
        );
        if !failing.is_empty() {
            line.push_str(&format!(
                "; failing pools: {}",
                failing.into_iter().collect::<Vec<_>>().join(", ")
            ));
        }
        if s.frames != 0 && !unused.is_empty() {
            line.push_str(&format!(
                "; allowlist entries not written (remove them): {}",
                unused.join(", ")
            ));
        }
        line
    }
}

/// The feature: [`Guard`] around `render_frame_and_present`, under
/// `impl.sim.frame-guard` (off).
#[derive(Clone)]
pub struct FrameGuard(Arc<Mutex<Guard>>);

impl Default for FrameGuard {
    fn default() -> Self {
        FrameGuard(Arc::new(Mutex::new(Guard::new(
            SimRegion::HALO2,
            Allowlist::builtin(),
        ))))
    }
}

impl FrameGuard {
    /// The totals so far (`sim.render_writes` reads them).
    pub fn stats(&self) -> Stats {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .stats
            .clone()
    }
}

/// `frame-guard: ` and the line, on stderr.
fn say(line: &str) {
    eprintln!("frame-guard: {line}");
}

impl hooks::Feature for FrameGuard {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            ID,
            "The frame step leaves the game-state arena as it found it",
        )
        .doc("docs/engine/sim-render-split.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        if !b.switches().implementation("sim.frame-guard", false) {
            return Ok(());
        }
        if b.switches().feature("render-census", false) {
            return Err(hooks::BuildError(
                "impl.sim.frame-guard and render-census both use the guest access trap".into(),
            ));
        }
        // The watch and the trap are host state, armed only inside a frame
        // step, and the copy is the frame's.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let state = Arc::clone(&self.0);
        b.raw().observe_before(
            &guest_layout::points::RENDER_FRAME,
            "frame guard: enter",
            move |o| {
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                if let Err(e) = s.enter(&mut o.guest()) {
                    say(&e);
                }
            },
        );
        let state = Arc::clone(&self.0);
        b.raw().observe_after(
            &guest_layout::points::RENDER_FRAME,
            "frame guard: leave",
            move |o| {
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                match s.leave(&mut o.guest()) {
                    Ok(frame) => s.new_failures(&frame).iter().for_each(|l| say(l)),
                    Err(e) => say(&e),
                }
            },
        );
        let state = Arc::clone(&self.0);
        b.game()
            .on_end("frame guard: report", move |_| {
                say(&state.lock().unwrap_or_else(|e| e.into_inner()).report());
            })
            .optional();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    const POINTER: u32 = 0x0001_0000;
    const ARENA: u32 = 0x0010_0000;
    const LEN: u32 = 4 * PAGE as u32;

    /// A datum array header named `name` at `at` (signature `'d@t@'`).
    fn header(block: &mut [u8], at: usize, name: &str) {
        block[at..at + name.len()].copy_from_slice(name.as_bytes());
        let sig = at + guest_layout::datum::layout::SIGNATURE as usize;
        block[sig..sig + 4].copy_from_slice(&guest_layout::datum::SIGNATURE.to_le_bytes());
    }

    /// Four arena pages: `(head)` on page 0, `object` from page 1, `lights`
    /// from page 3.
    fn guest() -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(POINTER, PAGE as u32, Permissions::RW).unwrap();
        g.map_zeroed(ARENA, LEN, Permissions::RW).unwrap();
        g.prepare_fast_pages();
        let mut block = vec![0u8; LEN as usize];
        header(&mut block, PAGE, "object");
        header(&mut block, 3 * PAGE, "lights");
        g.write_bytes(ARENA, &block, 1).unwrap();
        g.set_u32(POINTER, ARENA).unwrap();
        g
    }

    fn allow(pools: &[&str]) -> Allowlist {
        Allowlist(
            pools
                .iter()
                .map(|p| Allowed {
                    pool: (*p).into(),
                    slice: 8,
                    why: "test".into(),
                })
                .collect(),
        )
    }

    fn guard(pools: &[&str]) -> Guard {
        Guard::new(
            SimRegion::Indirect {
                pointer: POINTER,
                len: LEN,
            },
            allow(pools),
        )
    }

    #[test]
    fn the_allowlist_parses_and_only_shrinks() {
        let a = Allowlist::builtin();
        assert!(
            a.0.len() <= MAX_ALLOWED,
            "{} allowlist entries, at most {MAX_ALLOWED}: the list only shrinks",
            a.0.len()
        );
        assert!(
            a.0.iter()
                .all(|e| SLICES.contains(&e.slice) && !e.why.is_empty())
        );
        assert!(a.allows("particles#2") && !a.allows("object"));
        // Slice 8 moved the light render fields and the render-state cache
        // off the arena (game::render_scratch).
        assert!(!a.allows("lights") && !a.allows("cached object render states"));
    }

    #[test]
    fn a_bad_allowlist_line_is_named() {
        for (text, want) in [
            ("\"lights\" = \"8: x\"", "before [allowed]"),
            ("[allowed]\n[allowed]", "twice"),
            ("[allowed]\nlights = \"8: x\"", "quoted"),
            ("[allowed]\n\"lights\" = \"13: x\"", "not a slice"),
            ("[allowed]\n\"lights\" = \"eight\"", "slice: why"),
            (
                "[allowed]\n\"lights\" = \"8: x\"\n\"lights\" = \"9: y\"",
                "lights twice",
            ),
            ("[allowed]\n\"objects#2\" = \"8: x\"", "datum array name"),
        ] {
            let e = Allowlist::parse(text).unwrap_err();
            assert!(e.contains(want), "{text:?}: {e}");
        }
        assert_eq!(
            Allowlist::parse("# only\n\n[allowed]\n").unwrap(),
            Allowlist::default()
        );
    }

    #[test]
    fn a_frame_that_writes_nothing_or_the_same_bytes_is_clean() {
        let mut g = guest();
        let mut s = guard(&[]);
        s.enter(&mut g).unwrap();
        g.write_bytes_from(0x1234, ARENA + PAGE as u32 + 0x40, &[0, 0], 1)
            .unwrap();
        let f = s.leave(&mut g).unwrap();
        assert!(f.findings.is_empty() && f.clean(), "{f:?}");
        assert_eq!(
            (
                s.stats().frames,
                s.stats().pages_written,
                s.stats().pages_changed
            ),
            (1, 1, 0)
        );
    }

    /// A change is named by page, pool and the guest's `pc`; the tick's
    /// writes between frames are not the frame's.
    #[test]
    fn a_changed_page_names_its_pool_and_writer() {
        let mut g = guest();
        let mut s = guard(&[]);
        s.enter(&mut g).unwrap();
        g.write_bytes_from(0x0013_3790, ARENA + PAGE as u32 + 0xcc, &[1, 2, 3, 4], 1)
            .unwrap();
        g.write_bytes_from(0x0013_37a0, ARENA + PAGE as u32 + 0xd0, &[5], 1)
            .unwrap();
        let f = s.leave(&mut g).unwrap();
        assert!(!f.clean());
        assert_eq!(
            f.findings,
            [Finding {
                page: ARENA + PAGE as u32,
                offset: PAGE as u32,
                first: 0xcc,
                changed: 5,
                pools: vec!["object".into()],
                writer: Writer::Guest(0x0013_3790),
                allowed: false,
            }]
        );
        assert_eq!(
            f.findings[0].line(),
            "page 0x00101000 (arena +0x1000) pool object +0xcc, 5 bytes, by pc 0x00133790"
        );
        assert_eq!(s.new_failures(&f).len(), 1);
        assert!(s.new_failures(&f).is_empty(), "printed once");

        // The tick: writes outside a frame step are not findings.
        g.write_bytes_from(0x0020_0000, ARENA + 0x10, &[9], 1)
            .unwrap();
        s.enter(&mut g).unwrap();
        let f = s.leave(&mut g).unwrap();
        assert!(f.findings.is_empty(), "{f:?}");
    }

    #[test]
    fn allowed_pools_pass_and_a_page_across_two_pools_needs_both() {
        let mut g = guest();
        let mut s = guard(&["lights"]);
        s.enter(&mut g).unwrap();
        g.set_u32(ARENA + 3 * PAGE as u32 + 0x100, 7).unwrap();
        let f = s.leave(&mut g).unwrap();
        assert!(f.clean(), "{f:?}");
        assert_eq!(f.findings[0].writer, Writer::Host { hle: 0 });
        assert!(f.findings[0].allowed);

        // Page 1 holds `object` only; cut a `lights` array into page 2's
        // tail so one page spans two pools.
        let mut g = guest();
        let mut block = vec![0u8; LEN as usize];
        g.read_bytes(ARENA, &mut block, 1).unwrap();
        header(&mut block, 2 * PAGE + 0x800, "lights");
        g.write_bytes(ARENA, &block, 1).unwrap();
        let mut s = guard(&["lights"]);
        s.enter(&mut g).unwrap();
        g.write_bytes_from(0x40, ARENA + 2 * PAGE as u32 + 0x10, &[1], 1)
            .unwrap();
        g.write_bytes_from(0x44, ARENA + 2 * PAGE as u32 + 0x900, &[1], 1)
            .unwrap();
        let f = s.leave(&mut g).unwrap();
        assert_eq!(f.findings.len(), 1);
        assert_eq!(f.findings[0].pools, ["object", "lights"]);
        assert!(!f.findings[0].allowed, "object is not allowed");
        assert!(
            s.report().contains("failing pools: object"),
            "{}",
            s.report()
        );
    }

    #[test]
    fn the_report_names_entries_no_frame_wrote() {
        let mut g = guest();
        let mut s = guard(&["lights", "particles"]);
        s.enter(&mut g).unwrap();
        g.set_u8(ARENA + 3 * PAGE as u32 + 0x200, 1).unwrap();
        s.leave(&mut g).unwrap();
        let r = s.report();
        assert!(
            r.starts_with("1 frames, 1 pages written, 1 changed: 1 allowed, 0 failures"),
            "{r}"
        );
        assert!(
            r.ends_with("allowlist entries not written (remove them): particles"),
            "{r}"
        );
    }

    #[test]
    fn no_arena_no_guard_and_the_trap_is_disarmed_after() {
        let mut g = guest();
        g.set_u32(POINTER, 0).unwrap();
        let mut s = guard(&[]);
        s.enter(&mut g).unwrap();
        g.set_u8(ARENA, 1).unwrap();
        assert_eq!(s.leave(&mut g).unwrap(), Frame::default());
        assert_eq!(s.stats().frames, 0);

        g.set_u32(POINTER, ARENA).unwrap();
        s.enter(&mut g).unwrap();
        s.leave(&mut g).unwrap();
        g.write_bytes_from(0x50, ARENA + 0x20, &[3], 1).unwrap();
        assert!(
            g.take_trap_hits().hits.is_empty(),
            "nothing armed between frame steps"
        );
    }

    #[test]
    fn writers_prefer_the_guest_and_span_pages() {
        let hit = |addr, len, pc| runtime::TrapHit {
            addr,
            len,
            write: true,
            pc,
            phase: u8::MAX,
            thread: runtime::trap::NO_THREAD,
            hle: 0,
        };
        let log = TrapLog {
            hits: vec![hit(0x1ffe, 4, 0), hit(0x2010, 1, 0x99), hit(0x3000, 4, 0)],
            ..TrapLog::default()
        };
        let w = writers(&log);
        assert_eq!(w[&1], Writer::Host { hle: 0 });
        assert_eq!(
            w[&2],
            Writer::Guest(0x99),
            "the guest's pc wins over the host's first write"
        );
        assert_eq!(w[&3], Writer::Host { hle: 0 });
    }
}
