//! A complete binary encoding of an [`Analysis`], so a build can cache
//! discovery's result instead of rerunning it (crates/apps/boot keys the cache by
//! the image and the analyzer's source hash).
//!
//! The encoding covers every field. The encoder destructures each struct
//! with no `..`, so adding a field anywhere in the result fails to compile
//! here until the codec handles it: a cached analysis can never silently
//! lack something a fresh one has. [`Analysis::decode`] rejects anything it
//! did not write (wrong magic or version, truncation, trailing bytes, an
//! unknown enum tag).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::image::{Byte, ConflictKind};
use crate::{Analysis, Conflict, Function, JumpTable, PointerCensus, SectionCoverage, Source, SwitchKind};

/// Bump on any change to the layout below.
const MAGIC: &[u8; 8] = b"h2anlz\0\x01";

#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend(v.to_le_bytes());
    }
    fn usize(&mut self, v: usize) {
        self.u64(v as u64);
    }
    fn bool(&mut self, v: bool) {
        self.u8(v as u8);
    }
    fn str(&mut self, s: &str) {
        self.usize(s.len());
        self.0.extend(s.as_bytes());
    }
    fn u32s(&mut self, v: &[u32]) {
        self.usize(v.len());
        for &x in v {
            self.u32(x);
        }
    }
    fn opt_u32(&mut self, v: Option<u32>) {
        match v {
            None => self.u8(0),
            Some(x) => {
                self.u8(1);
                self.u32(x);
            }
        }
    }
}

struct R<'a>(&'a [u8]);

type Res<T> = Option<T>;

impl R<'_> {
    fn take(&mut self, n: usize) -> Res<&[u8]> {
        if self.0.len() < n {
            return None;
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Some(a)
    }
    fn u8(&mut self) -> Res<u8> {
        Some(self.take(1)?[0])
    }
    fn u32(&mut self) -> Res<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Res<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
    fn usize(&mut self) -> Res<usize> {
        usize::try_from(self.u64()?).ok()
    }
    /// A count of items at least `min` bytes each: refuses counts the rest
    /// of the input can't hold, so a corrupt length can't allocate wildly.
    fn len(&mut self, min: usize) -> Res<usize> {
        let n = self.usize()?;
        (n.checked_mul(min.max(1))? <= self.0.len()).then_some(n)
    }
    fn bool(&mut self) -> Res<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
    fn str(&mut self) -> Res<String> {
        let n = self.len(1)?;
        String::from_utf8(self.take(n)?.to_vec()).ok()
    }
    fn u32s(&mut self) -> Res<Vec<u32>> {
        let n = self.len(4)?;
        (0..n).map(|_| self.u32()).collect()
    }
    fn opt_u32(&mut self) -> Res<Option<u32>> {
        match self.u8()? {
            0 => Some(None),
            1 => Some(Some(self.u32()?)),
            _ => None,
        }
    }
}

fn source_tag(s: Source) -> u8 {
    match s {
        Source::Entry => 0,
        Source::Call => 1,
        Source::TailCall => 2,
        Source::CodeRef => 3,
        Source::DataPointer => 4,
        Source::GapScan => 5,
    }
}

fn source_of(t: u8) -> Res<Source> {
    Some(match t {
        0 => Source::Entry,
        1 => Source::Call,
        2 => Source::TailCall,
        3 => Source::CodeRef,
        4 => Source::DataPointer,
        5 => Source::GapScan,
        _ => return None,
    })
}

fn switch_tag(s: SwitchKind) -> u8 {
    match s {
        SwitchKind::Bounded => 0,
        SwitchKind::Indexed => 1,
        SwitchKind::Unbounded => 2,
    }
}

fn switch_of(t: u8) -> Res<SwitchKind> {
    Some(match t {
        0 => SwitchKind::Bounded,
        1 => SwitchKind::Indexed,
        2 => SwitchKind::Unbounded,
        _ => return None,
    })
}

fn conflict_tag(k: ConflictKind) -> u8 {
    match k {
        ConflictKind::InvalidOpcode => 0,
        ConflictKind::IntoInsnBody => 1,
        ConflictKind::IntoData => 2,
        ConflictKind::OverlapsClaimed => 3,
        ConflictKind::RunsOffSection => 4,
        ConflictKind::OutsideCode => 5,
        ConflictKind::BadJumpTable => 6,
        ConflictKind::FallsIntoTrap => 7,
        ConflictKind::Implausible => 8,
    }
}

fn conflict_of(t: u8) -> Res<ConflictKind> {
    Some(match t {
        0 => ConflictKind::InvalidOpcode,
        1 => ConflictKind::IntoInsnBody,
        2 => ConflictKind::IntoData,
        3 => ConflictKind::OverlapsClaimed,
        4 => ConflictKind::RunsOffSection,
        5 => ConflictKind::OutsideCode,
        6 => ConflictKind::BadJumpTable,
        7 => ConflictKind::FallsIntoTrap,
        8 => ConflictKind::Implausible,
        _ => return None,
    })
}

fn byte_tag(b: Byte) -> u8 {
    match b {
        Byte::Unknown => 0,
        Byte::InsnStart => 1,
        Byte::InsnBody => 2,
        Byte::Data => 3,
    }
}

fn byte_of(t: u8) -> Res<Byte> {
    Some(match t {
        0 => Byte::Unknown,
        1 => Byte::InsnStart,
        2 => Byte::InsnBody,
        3 => Byte::Data,
        _ => return None,
    })
}

/// Run-length encode `v` as (count, item) pairs.
fn runs<T: Copy + PartialEq>(w: &mut W, v: &[T], put: impl Fn(&mut W, T)) {
    let mut out: Vec<(usize, T)> = Vec::new();
    for &x in v {
        match out.last_mut() {
            Some((n, y)) if *y == x => *n += 1,
            _ => out.push((1, x)),
        }
    }
    w.usize(v.len());
    w.usize(out.len());
    for (n, x) in out {
        w.usize(n);
        put(w, x);
    }
}

fn unruns<T: Copy>(r: &mut R, get: impl Fn(&mut R) -> Res<T>) -> Res<Vec<T>> {
    let total = r.usize()?;
    let pairs = r.len(9)?;
    // Each run holds at least one item, so the total is bounded by what
    // the runs could describe only after reading them: cap the reservation.
    let mut v = Vec::with_capacity(total.min(1 << 28));
    for _ in 0..pairs {
        let n = r.usize()?;
        let x = get(r)?;
        if n == 0 || v.len().checked_add(n)? > total {
            return None;
        }
        v.extend(std::iter::repeat_n(x, n));
    }
    (v.len() == total).then_some(v)
}

fn put_jump_table(w: &mut W, t: &JumpTable) {
    let JumpTable { site, table, kind, count, bound, index_table, targets, holes } = t;
    w.u32(*site);
    w.u32(*table);
    w.u8(switch_tag(*kind));
    w.u32(*count);
    w.opt_u32(*bound);
    match index_table {
        None => w.u8(0),
        Some((a, n)) => {
            w.u8(1);
            w.u32(*a);
            w.u32(*n);
        }
    }
    w.u32s(targets);
    w.u32s(holes);
}

fn get_jump_table(r: &mut R) -> Res<JumpTable> {
    Some(JumpTable {
        site: r.u32()?,
        table: r.u32()?,
        kind: switch_of(r.u8()?)?,
        count: r.u32()?,
        bound: r.opt_u32()?,
        index_table: match r.u8()? {
            0 => None,
            1 => Some((r.u32()?, r.u32()?)),
            _ => return None,
        },
        targets: r.u32s()?,
        holes: r.u32s()?,
    })
}

impl Analysis {
    /// The whole analysis as bytes [`Analysis::decode`] reads back.
    /// Deterministic: equal analyses encode to equal bytes (hash sets are
    /// written sorted).
    pub fn encode(&self) -> Vec<u8> {
        let Analysis {
            functions,
            sections,
            switches,
            jump_tables,
            conflicts,
            rejected,
            labels,
            kernel_calls,
            kernel_imports,
            gaps,
            ranges,
            blacklisted,
            rounds,
            converged,
            claims,
            losers,
            tail_sites,
            pointers,
        } = self;
        let mut w = W::default();
        w.0.extend(MAGIC);
        w.usize(functions.len());
        for (va, f) in functions {
            let Function { start, source, section, returns } = f;
            w.u32(*va);
            w.u32(*start);
            w.u8(source_tag(*source));
            w.str(section);
            w.bool(*returns);
        }
        w.usize(sections.len());
        for s in sections {
            let SectionCoverage { name, size, code, data, padding, unknown, functions } = s;
            w.str(name);
            for v in [size, code, data, padding, unknown, functions] {
                w.usize(*v);
            }
        }
        w.usize(switches.len());
        for (va, k) in switches {
            w.u32(*va);
            w.u8(switch_tag(*k));
        }
        w.usize(jump_tables.len());
        for (va, t) in jump_tables {
            w.u32(*va);
            put_jump_table(&mut w, t);
        }
        w.usize(conflicts.len());
        for c in conflicts {
            let Conflict { va, kind, function, claimed_by } = c;
            w.u32(*va);
            w.u8(conflict_tag(*kind));
            w.u32(*function);
            w.opt_u32(*claimed_by);
        }
        w.usize(rejected.len());
        for ((s, k), n) in rejected {
            w.u8(source_tag(*s));
            w.u8(conflict_tag(*k));
            w.usize(*n);
        }
        w.u32s(&labels.iter().copied().collect::<Vec<_>>());
        w.usize(kernel_calls.len());
        for (o, n) in kernel_calls {
            w.u32(*o);
            w.usize(*n);
        }
        w.usize(*kernel_imports);
        w.usize(gaps.len());
        for (va, n) in gaps {
            w.u32(*va);
            w.usize(*n);
        }
        w.usize(ranges.len());
        for (va, n, name) in ranges {
            w.u32(*va);
            w.u32(*n);
            w.str(name);
        }
        w.usize(*blacklisted);
        w.usize(*rounds);
        w.bool(*converged);
        w.usize(claims.len());
        for (va, state, owner) in claims {
            w.u32(*va);
            runs(&mut w, state, |w, b| w.u8(byte_tag(b)));
            runs(&mut w, owner, |w, o| w.u32(o));
        }
        w.u32s(losers);
        let mut tails: Vec<u32> = tail_sites.iter().copied().collect();
        tails.sort_unstable();
        w.u32s(&tails);
        let PointerCensus { entries, interior, bodies, data, padding, unknown } = pointers;
        for v in [entries, interior, bodies, data, padding, unknown] {
            w.usize(*v);
        }
        w.0
    }

    /// An analysis [`Analysis::encode`] wrote, or `None` for anything else.
    pub fn decode(bytes: &[u8]) -> Option<Analysis> {
        let mut r = R(bytes.strip_prefix(MAGIC.as_slice())?);
        let r = &mut r;
        let mut functions = BTreeMap::new();
        for _ in 0..r.len(10)? {
            let va = r.u32()?;
            let f = Function { start: r.u32()?, source: source_of(r.u8()?)?, section: r.str()?, returns: r.bool()? };
            functions.insert(va, f);
        }
        let mut sections = Vec::new();
        for _ in 0..r.len(8)? {
            sections.push(SectionCoverage {
                name: r.str()?,
                size: r.usize()?,
                code: r.usize()?,
                data: r.usize()?,
                padding: r.usize()?,
                unknown: r.usize()?,
                functions: r.usize()?,
            });
        }
        let mut switches = BTreeMap::new();
        for _ in 0..r.len(5)? {
            switches.insert(r.u32()?, switch_of(r.u8()?)?);
        }
        let mut jump_tables = BTreeMap::new();
        for _ in 0..r.len(4)? {
            jump_tables.insert(r.u32()?, get_jump_table(r)?);
        }
        let mut conflicts = Vec::new();
        for _ in 0..r.len(10)? {
            conflicts.push(Conflict {
                va: r.u32()?,
                kind: conflict_of(r.u8()?)?,
                function: r.u32()?,
                claimed_by: r.opt_u32()?,
            });
        }
        let mut rejected = BTreeMap::new();
        for _ in 0..r.len(10)? {
            rejected.insert((source_of(r.u8()?)?, conflict_of(r.u8()?)?), r.usize()?);
        }
        let labels: BTreeSet<u32> = r.u32s()?.into_iter().collect();
        let mut kernel_calls = BTreeMap::new();
        for _ in 0..r.len(12)? {
            kernel_calls.insert(r.u32()?, r.usize()?);
        }
        let kernel_imports = r.usize()?;
        let mut gaps = Vec::new();
        for _ in 0..r.len(12)? {
            gaps.push((r.u32()?, r.usize()?));
        }
        let mut ranges = Vec::new();
        for _ in 0..r.len(16)? {
            ranges.push((r.u32()?, r.u32()?, r.str()?));
        }
        let blacklisted = r.usize()?;
        let rounds = r.usize()?;
        let converged = r.bool()?;
        let mut claims = Vec::new();
        for _ in 0..r.len(20)? {
            let va = r.u32()?;
            let state = unruns(r, |r| byte_of(r.u8()?))?;
            let owner = unruns(r, |r| r.u32())?;
            if state.len() != owner.len() {
                return None;
            }
            claims.push((va, state, owner));
        }
        let losers = r.u32s()?;
        let tail_sites: HashSet<u32> = r.u32s()?.into_iter().collect();
        let pointers = PointerCensus {
            entries: r.usize()?,
            interior: r.usize()?,
            bodies: r.usize()?,
            data: r.usize()?,
            padding: r.usize()?,
            unknown: r.usize()?,
        };
        if !r.0.is_empty() {
            return None;
        }
        Some(Analysis {
            functions,
            sections,
            switches,
            jump_tables,
            conflicts,
            rejected,
            labels,
            kernel_calls,
            kernel_imports,
            gaps,
            ranges,
            blacklisted,
            rounds,
            converged,
            claims,
            losers,
            tail_sites,
            pointers,
        })
    }
}
