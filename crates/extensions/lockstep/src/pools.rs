//! The simulation hash per pool, the co-op probe's hash log, and the diff
//! of two such logs (docs/live/live-coop.md section 9).
//!
//! [`hash_sim`](crate::hash_sim) gives one word over the whole game-state
//! block. Two peers that part need more than "they differ": they need the
//! part of the block that differs first. The block is bump-allocated by the
//! engine and most of what lives in it are datum arrays, each starting with
//! a named header ([`guest_layout::datum::layout`]). [`find_pools`] cuts the block
//! at every such header, so a pool is "the array called `objects` and
//! whatever was allocated after it up to the next array". Bytes before the
//! first header are the pool [`HEAD`].
//!
//! The log is text, one line per tick and pool:
//!
//! ```text
//! # h2 coop-probe hashes v1
//! # role host mode sync-server
//! 120 * 9a0c4f1e22d3b871
//! 120 (head) 01f2e3d4c5b6a798
//! 120 objects 4c5d6e7f8091a2b3
//! # tick 121 checkpoint save
//! ```
//!
//! `*` is the whole block. Lines starting `#` are comments and events; the
//! diff keeps them for context and otherwise ignores them.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// The pool name of the bytes before the first datum array header.
pub const HEAD: &str = "(head)";
/// The pool name of the whole block's hash.
pub const WHOLE: &str = "*";
/// The first line of a hash log.
pub const LOG_HEADER: &str = "# h2 coop-probe hashes v1";

/// One named stretch of the game-state block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pool {
    pub name: String,
    pub offset: u32,
    pub len: u32,
}

/// A datum array header's signature, `'d@t@'`.
const SIGNATURE: u32 = guest_layout::datum::SIGNATURE;
const NAME_LEN: usize = 0x20;
const SIGNATURE_AT: usize = guest_layout::datum::layout::SIGNATURE as usize;

/// The name of a datum array header at the start of `h`, if it is one: the
/// signature in place and a NUL-terminated name of printable ASCII without
/// spaces at the ends.
fn header_name(h: &[u8]) -> Option<String> {
    if h.len() < SIGNATURE_AT + 4 {
        return None;
    }
    let sig = u32::from_le_bytes(h[SIGNATURE_AT..SIGNATURE_AT + 4].try_into().ok()?);
    if sig != SIGNATURE {
        return None;
    }
    let name = &h[..NAME_LEN];
    let end = name.iter().position(|&b| b == 0)?;
    let name = &name[..end];
    if name.is_empty() || !name.iter().all(|b| (0x20..0x7f).contains(b)) {
        return None;
    }
    let s = String::from_utf8(name.to_vec()).ok()?;
    (s.trim() == s).then_some(s)
}

/// The pools of a game-state block: cut at every datum array header (on a
/// 4-byte boundary), named by the array, with [`HEAD`] for anything before
/// the first. A name seen twice gets `#2`, `#3`. Covers every byte of
/// `block` exactly once; an empty block has no pools.
pub fn find_pools(block: &[u8]) -> Vec<Pool> {
    let mut starts: Vec<(usize, String)> = Vec::new();
    let mut at = 0;
    while at + SIGNATURE_AT + 4 <= block.len() {
        if let Some(name) = header_name(&block[at..]) {
            starts.push((at, name));
            at += guest_layout::datum::layout::HEADER_SIZE as usize;
        } else {
            at += 4;
        }
    }
    if block.is_empty() {
        return Vec::new();
    }
    if starts.first().is_none_or(|(o, _)| *o != 0) {
        starts.insert(0, (0, HEAD.to_owned()));
    }
    let mut seen: BTreeMap<String, u32> = BTreeMap::new();
    let mut pools = Vec::with_capacity(starts.len());
    for (i, (offset, name)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(block.len(), |(o, _)| *o);
        let n = seen.entry(name.clone()).or_insert(0);
        *n += 1;
        let name = if *n == 1 {
            name.clone()
        } else {
            format!("{name}#{n}")
        };
        pools.push(Pool {
            name,
            offset: *offset as u32,
            len: (end - offset) as u32,
        });
    }
    pools
}

/// The hash of each pool of `block`, in the order given, seeded by the
/// pool's offset so two pools with equal bytes still differ. A pool that
/// runs past the block hashes what is there.
pub fn hash_pools(block: &[u8], pools: &[Pool]) -> Vec<u64> {
    pools
        .iter()
        .map(|p| {
            let start = (p.offset as usize).min(block.len());
            let end = (start + p.len as usize).min(block.len());
            crate::hash_bytes(u64::from(p.offset), &block[start..end])
        })
        .collect()
}

/// The hash of the whole block, as [`hash_sim`](crate::hash_sim) computes
/// it (seeded by the length).
pub fn hash_block(block: &[u8]) -> u64 {
    crate::hash_bytes(block.len() as u64, block)
}

/// The pools that own the pages of `block` that start at `offsets` (each
/// `page` bytes long), and how many of those pages each overlaps: the
/// pools in block order, a page that spans two pools counted for both. A
/// start hash that does not reproduce names its differing pages by these
/// (docs/game/theater-design.md, "Starting a film in the running game").
pub fn page_owners(block: &[u8], page: usize, offsets: &[usize]) -> Vec<(String, usize)> {
    page_owners_of(&find_pools(block), page, offsets)
}

/// [`page_owners`] over pools already found.
pub fn page_owners_of(pools: &[Pool], page: usize, offsets: &[usize]) -> Vec<(String, usize)> {
    let mut counts = vec![0usize; pools.len()];
    for &at in offsets {
        let end = at.saturating_add(page.max(1));
        for (i, p) in pools.iter().enumerate() {
            let (start, stop) = (p.offset as usize, (p.offset + p.len) as usize);
            if start < end && at < stop {
                counts[i] += 1;
            }
        }
    }
    pools
        .iter()
        .zip(counts)
        .filter(|(_, n)| *n > 0)
        .map(|(p, n)| (p.name.clone(), n))
        .collect()
}

/// [`page_owners`] as one line: `object ×40, (head) ×2`, the pools with the
/// most pages first (block order among equals), at most `shown` of them.
pub fn owners_line(owners: &[(String, usize)], shown: usize) -> String {
    let mut sorted: Vec<&(String, usize)> = owners.iter().collect();
    sorted.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    let mut parts: Vec<String> = sorted
        .iter()
        .take(shown)
        .map(|(name, n)| format!("{name} \u{d7}{n}"))
        .collect();
    let more = sorted.len().saturating_sub(shown);
    if more > 0 {
        parts.push(format!("{more} more"));
    }
    parts.join(", ")
}

/// One hash line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub tick: u32,
    pub pool: String,
    pub hash: u64,
}

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {:016x}", self.tick, self.pool, self.hash)
    }
}

/// A pool name as it goes in a log: no whitespace (a run of it is `_`).
pub fn log_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut space = false;
    for c in name.chars() {
        if c.is_whitespace() {
            if !space {
                out.push('_');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    if out.is_empty() { "_".into() } else { out }
}

/// Parses a hash line (`tick pool hash`); comments and blank lines are not
/// hash lines.
pub fn parse_line(line: &str) -> Result<Line, String> {
    let mut it = line.split_whitespace();
    let (Some(t), Some(p), Some(h), None) = (it.next(), it.next(), it.next(), it.next()) else {
        return Err(format!("not `tick pool hash`: {line:?}"));
    };
    let tick = t.parse().map_err(|_| format!("bad tick {t:?}"))?;
    if h.len() != 16 {
        return Err(format!("bad hash {h:?} (16 hex digits)"));
    }
    let hash = u64::from_str_radix(h, 16).map_err(|_| format!("bad hash {h:?}"))?;
    Ok(Line {
        tick,
        pool: p.to_owned(),
        hash,
    })
}

/// A parsed hash log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Log {
    /// Every tick's hashes, by pool.
    pub ticks: BTreeMap<u32, BTreeMap<String, u64>>,
    /// The comment lines after the header, in order, without the `# `.
    pub notes: Vec<String>,
}

impl Log {
    /// Parses `text`. The header line is required; a malformed line is an
    /// error naming its line number. A tick and pool given twice keeps the
    /// last (a revert can replay a tick).
    pub fn parse(text: &str) -> Result<Log, String> {
        let mut lines = text.lines().enumerate();
        match lines.next() {
            Some((_, l)) if l.trim_end() == LOG_HEADER => {}
            _ => {
                return Err(format!(
                    "not a coop-probe hash log (first line is not {LOG_HEADER:?})"
                ));
            }
        }
        let mut log = Log::default();
        for (i, l) in lines {
            let l = l.trim();
            if l.is_empty() {
                continue;
            }
            if let Some(note) = l.strip_prefix('#') {
                log.notes.push(note.trim().to_owned());
                continue;
            }
            let line = parse_line(l).map_err(|e| format!("line {}: {e}", i + 1))?;
            log.ticks
                .entry(line.tick)
                .or_default()
                .insert(line.pool, line.hash);
        }
        Ok(log)
    }

    /// The events noted at `tick` (notes of the form `tick N ...`).
    pub fn notes_at(&self, tick: u32) -> Vec<&str> {
        let prefix = format!("tick {tick} ");
        self.notes
            .iter()
            .filter_map(|n| n.strip_prefix(&prefix))
            .collect()
    }
}

/// Where two logs first part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Divergence {
    pub tick: u32,
    /// The pools whose hashes differ at that tick (not counting `*`), in
    /// block order as the first log lists them, then any only one side has.
    pub pools: Vec<String>,
    /// The whole-block hash differs too.
    pub whole: bool,
}

/// The comparison of two hash logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub ticks_a: usize,
    pub ticks_b: usize,
    /// Ticks both logs have.
    pub common: usize,
    /// Common ticks whose hashes all agree.
    pub agreeing: usize,
    pub first_common: Option<u32>,
    pub last_common: Option<u32>,
    /// The first common tick that differs.
    pub first: Option<Divergence>,
    /// Common ticks that differ.
    pub differing: usize,
    /// For each pool, the number of common ticks where it differs, most
    /// first (ties by name).
    pub by_pool: Vec<(String, usize)>,
    /// Notes either log has at the first divergent tick (`a:` or `b:`).
    pub notes: Vec<String>,
}

impl Report {
    /// The logs agree on every tick they share, and share at least one.
    pub fn identical(&self) -> bool {
        self.first.is_none() && self.common > 0
    }
}

fn pool_order(a: &BTreeMap<String, u64>, b: &BTreeMap<String, u64>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for k in a.keys().chain(b.keys()) {
        if k != WHOLE && seen.insert(k.clone()) {
            out.push(k.clone());
        }
    }
    out
}

/// Compares two logs tick by tick over the ticks both have. A pool one side
/// has and the other lacks at a tick counts as differing.
pub fn diff(a: &Log, b: &Log) -> Report {
    let mut r = Report {
        ticks_a: a.ticks.len(),
        ticks_b: b.ticks.len(),
        common: 0,
        agreeing: 0,
        first_common: None,
        last_common: None,
        first: None,
        differing: 0,
        by_pool: Vec::new(),
        notes: Vec::new(),
    };
    let mut by_pool: BTreeMap<String, usize> = BTreeMap::new();
    for (tick, ha) in &a.ticks {
        let Some(hb) = b.ticks.get(tick) else {
            continue;
        };
        r.common += 1;
        r.first_common.get_or_insert(*tick);
        r.last_common = Some(*tick);
        let pools: Vec<String> = pool_order(ha, hb)
            .into_iter()
            .filter(|p| ha.get(p) != hb.get(p))
            .collect();
        let whole = ha.get(WHOLE) != hb.get(WHOLE);
        if pools.is_empty() && !whole {
            r.agreeing += 1;
            continue;
        }
        r.differing += 1;
        for p in &pools {
            *by_pool.entry(p.clone()).or_insert(0) += 1;
        }
        if r.first.is_none() {
            r.notes = a
                .notes_at(*tick)
                .into_iter()
                .map(|n| format!("a: {n}"))
                .chain(b.notes_at(*tick).into_iter().map(|n| format!("b: {n}")))
                .collect();
            r.first = Some(Divergence {
                tick: *tick,
                pools,
                whole,
            });
        }
    }
    let mut by_pool: Vec<(String, usize)> = by_pool.into_iter().collect();
    by_pool.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
    r.by_pool = by_pool;
    r
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "ticks: {} in a, {} in b, {} in both",
            self.ticks_a, self.ticks_b, self.common
        )?;
        if let (Some(lo), Some(hi)) = (self.first_common, self.last_common) {
            writeln!(f, "common ticks: {lo}..={hi}")?;
        }
        if self.common == 0 {
            return writeln!(f, "no tick in common: nothing to compare");
        }
        match &self.first {
            None => writeln!(f, "identical: all {} common ticks agree", self.common),
            Some(d) => {
                writeln!(
                    f,
                    "first divergence: tick {} ({} pool{} differ{})",
                    d.tick,
                    d.pools.len(),
                    if d.pools.len() == 1 { "" } else { "s" },
                    if d.whole { ", whole block differs" } else { "" }
                )?;
                for p in &d.pools {
                    writeln!(f, "  pool {p}")?;
                }
                for n in &self.notes {
                    writeln!(f, "  note {n}")?;
                }
                writeln!(
                    f,
                    "agreeing ticks: {}; differing ticks: {}",
                    self.agreeing, self.differing
                )?;
                writeln!(f, "pools by differing ticks:")?;
                for (p, n) in self.by_pool.iter().take(20) {
                    writeln!(f, "  {n:>8}  {p}")?;
                }
                if self.by_pool.len() > 20 {
                    writeln!(f, "  ... {} more", self.by_pool.len() - 20)?;
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A datum array header named `name` at `at` in `block`.
    fn header(block: &mut [u8], at: usize, name: &str) {
        block[at..at + name.len()].copy_from_slice(name.as_bytes());
        block[at + name.len()] = 0;
        block[at + SIGNATURE_AT..at + SIGNATURE_AT + 4].copy_from_slice(&SIGNATURE.to_le_bytes());
    }

    #[test]
    fn pools_cut_at_every_named_header_and_cover_the_block() {
        let mut b = vec![0u8; 0x1000];
        header(&mut b, 0x100, "players");
        header(&mut b, 0x400, "objects");
        header(&mut b, 0x800, "objects");
        let pools = find_pools(&b);
        let names: Vec<&str> = pools.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, [HEAD, "players", "objects", "objects#2"]);
        assert_eq!(
            pools[0],
            Pool {
                name: HEAD.into(),
                offset: 0,
                len: 0x100
            }
        );
        assert_eq!(pools[3].offset, 0x800);
        assert_eq!(
            pools.iter().map(|p| p.len).sum::<u32>(),
            0x1000,
            "every byte once"
        );
        for w in pools.windows(2) {
            assert_eq!(w[0].offset + w[0].len, w[1].offset);
        }
    }

    #[test]
    fn differing_pages_are_named_by_the_pools_they_overlap() {
        let mut b = vec![0u8; 0x4000];
        header(&mut b, 0x800, "players");
        header(&mut b, 0x1800, "object");
        // Pages at 0x0 (head only), 0x1000 (players then object at 0x1800),
        // 0x2000 and 0x3000 (object).
        let owners = page_owners(&b, 0x1000, &[0, 0x1000, 0x2000, 0x3000]);
        assert_eq!(
            owners,
            vec![
                (HEAD.to_string(), 1),
                ("players".to_string(), 2),
                ("object".to_string(), 3),
            ],
            "the page at 0x0 is head and players' start; 0x1000 spans players and object"
        );
        let first = page_owners(&b, 0x1000, &[0x2000]);
        assert_eq!(first, vec![("object".to_string(), 1)]);
        assert!(page_owners(&b, 0x1000, &[]).is_empty());
        assert_eq!(owners_line(&owners, 2), "object \u{d7}3, players \u{d7}2, 1 more");
        assert_eq!(
            owners_line(&owners, 8),
            "object \u{d7}3, players \u{d7}2, (head) \u{d7}1"
        );
    }

    #[test]
    fn a_header_at_zero_has_no_head_and_junk_is_not_a_header() {
        let mut b = vec![0u8; 0x200];
        header(&mut b, 0, "first");
        // The signature with an unterminated, an empty and a non-printable name.
        b[0x80..0xa0].fill(b'x');
        b[0x80 + SIGNATURE_AT..0x80 + SIGNATURE_AT + 4].copy_from_slice(&SIGNATURE.to_le_bytes());
        b[0x100 + SIGNATURE_AT..0x100 + SIGNATURE_AT + 4].copy_from_slice(&SIGNATURE.to_le_bytes());
        b[0x180] = 0x07;
        b[0x180 + SIGNATURE_AT..0x180 + SIGNATURE_AT + 4].copy_from_slice(&SIGNATURE.to_le_bytes());
        // Not on a 4-byte boundary.
        header(&mut b, 0x1c2 - SIGNATURE_AT, "odd");
        let pools = find_pools(&b);
        assert_eq!(pools.len(), 1, "{pools:?}");
        assert_eq!(pools[0].name, "first");
        assert!(find_pools(&[]).is_empty());
    }

    #[test]
    fn a_changed_byte_changes_only_its_pool() {
        let mut b = vec![0u8; 0x1000];
        header(&mut b, 0x100, "players");
        header(&mut b, 0x400, "objects");
        let pools = find_pools(&b);
        let before = hash_pools(&b, &pools);
        let whole = hash_block(&b);
        b[0x500] ^= 1;
        let after = hash_pools(&b, &pools);
        assert_eq!(before[0], after[0]);
        assert_eq!(before[1], after[1]);
        assert_ne!(before[2], after[2], "objects");
        assert_ne!(whole, hash_block(&b));
        // Equal bytes in two pools still hash apart (seeded by offset).
        let z = vec![0u8; 0x200];
        let two = [
            Pool {
                name: "a".into(),
                offset: 0,
                len: 0x100,
            },
            Pool {
                name: "b".into(),
                offset: 0x100,
                len: 0x100,
            },
        ];
        let h = hash_pools(&z, &two);
        assert_ne!(h[0], h[1]);
        // A pool past the end hashes what is there instead of panicking.
        let past = [Pool {
            name: "p".into(),
            offset: 0x180,
            len: 0x1000,
        }];
        assert_eq!(hash_pools(&z, &past).len(), 1);
    }

    #[test]
    fn hash_sim_and_hash_block_agree() {
        let b = vec![7u8; 0x300];
        assert_eq!(hash_block(&b), crate::hash_bytes(0x300, &b));
    }

    #[test]
    fn lines_round_trip() {
        let l = Line {
            tick: 42,
            pool: "objects".into(),
            hash: 0x0123_4567_89ab_cdef,
        };
        let s = l.to_string();
        assert_eq!(s, "42 objects 0123456789abcdef");
        assert_eq!(parse_line(&s), Ok(l));
        assert!(parse_line("42 objects").is_err());
        assert!(parse_line("42 objects 0123 extra").is_err());
        assert!(parse_line("x objects 0123456789abcdef").is_err());
        assert!(parse_line("1 objects 0123").is_err(), "short hash");
        assert!(parse_line("1 objects 0123456789abcdeg").is_err());
        assert_eq!(
            log_name("cached object render states"),
            "cached_object_render_states"
        );
        assert_eq!(log_name("  a  b "), "_a_b_");
        assert_eq!(log_name(""), "_");
    }

    fn log(lines: &[&str]) -> Log {
        let mut t = String::from(LOG_HEADER);
        for l in lines {
            t.push('\n');
            t.push_str(l);
        }
        Log::parse(&t).unwrap()
    }

    #[test]
    fn logs_parse_with_notes_and_refuse_garbage() {
        let l = log(&[
            "# role host",
            "1 * 0000000000000001",
            "1 objects 0000000000000002",
            "",
            "# tick 1 checkpoint save",
            "2 * 0000000000000003",
        ]);
        assert_eq!(l.ticks.len(), 2);
        assert_eq!(l.ticks[&1]["objects"], 2);
        assert_eq!(l.notes, ["role host", "tick 1 checkpoint save"]);
        assert_eq!(l.notes_at(1), ["checkpoint save"]);
        assert!(
            Log::parse("1 * 0000000000000001")
                .unwrap_err()
                .contains("not a coop-probe")
        );
        let e = Log::parse(&format!("{LOG_HEADER}\n1 * 01\n")).unwrap_err();
        assert!(e.starts_with("line 2:"), "{e}");
    }

    #[test]
    fn the_diff_names_the_first_tick_and_pool() {
        let a = log(&[
            "10 * 00000000000000a0",
            "10 players 0000000000000001",
            "10 objects 0000000000000002",
            "11 * 00000000000000a1",
            "11 players 0000000000000001",
            "11 objects 0000000000000003",
            "# tick 12 checkpoint save",
            "12 * 00000000000000a2",
            "12 players 0000000000000001",
            "12 objects 0000000000000004",
            "13 * 00000000000000a3",
        ]);
        let b = log(&[
            "11 * 00000000000000a1",
            "11 players 0000000000000001",
            "11 objects 0000000000000003",
            "12 * 00000000000000ff",
            "12 players 0000000000000001",
            "12 objects 0000000000000005",
            "13 * 00000000000000ff",
            "13 players 0000000000000009",
            "14 * 00000000000000a4",
        ]);
        let r = diff(&a, &b);
        assert_eq!((r.ticks_a, r.ticks_b, r.common), (4, 4, 3));
        assert_eq!((r.first_common, r.last_common), (Some(11), Some(13)));
        assert_eq!(
            r.first,
            Some(Divergence {
                tick: 12,
                pools: vec!["objects".into()],
                whole: true
            })
        );
        assert_eq!((r.agreeing, r.differing), (1, 2));
        // Tick 13: a has no `players`, b does: that differs too.
        assert_eq!(r.by_pool, [("objects".into(), 1), ("players".into(), 1)]);
        assert_eq!(r.notes, ["a: checkpoint save"]);
        assert!(!r.identical());
        let text = r.to_string();
        assert!(text.contains("first divergence: tick 12"), "{text}");
        assert!(text.contains("pool objects"), "{text}");
    }

    #[test]
    fn identical_logs_and_logs_with_nothing_in_common() {
        let a = log(&["1 * 0000000000000001", "2 * 0000000000000002"]);
        let r = diff(&a, &a);
        assert!(r.identical());
        assert!(
            r.to_string()
                .contains("identical: all 2 common ticks agree")
        );
        let b = log(&["5 * 0000000000000001"]);
        let r = diff(&a, &b);
        assert!(!r.identical());
        assert_eq!(r.common, 0);
        assert!(r.to_string().contains("no tick in common"));
    }
}
