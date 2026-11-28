//! The decal callback's cut (docs/graphics/retire-d3d-decals.md): a call of `0x00044040` whose records the native renderer draws
//! returns without the guest body, so none of its texture binds
//! (`0x000144f0`, `0x0001cf50`), tint sets (`D3DDevice_SetVertexData4ub`)
//! or `D3DDevice_DrawVertices` reach D3D.
//!
//! [`walk`] is the body's walk in Rust: the bucket `mode * 512 + list`
//! of `[0x004ea94c]`, each record's `+0x34` link, a record skipped when
//! its owner's marker (`+8`) is the frame word's low half (`0x004ba038`),
//! else marked and drawn. [`turn`] gives each drawn record to the host in
//! order, marking it as the body does; at the first record the host does
//! not draw, the guest body runs and, finding the drawn ones marked, draws
//! only the rest. A call with nothing to draw (an empty bucket, or every
//! record marked) returns: the body would make no write and no D3D entry.
//!
//! The body's caches stay as they were: the bitmap and image the callback
//! last bound (`0x004c878c`, `0x004c8788`), the tint (`0x004c8784`) and
//! the stage textures (`0x0051f3c8`/`0x0051f3d8`) agree with the device,
//! which no cut record touches, and the state wrapper `0x00043a40` resets
//! them for each mode.
//!
//! The arm: every record a guest body draws after a modelled walk is
//! checked at its resolved draw against the walk ([`take_expected`]); a
//! difference ([`mismatch`]) sends every later call to the guest body for
//! the session.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::memory::GuestMemory;
use scene_extract::decals::{BUCKET_CLUSTERS, DECALS, GEOMETRY, GROUPS, RECORD_SIZE, STREAM};

pub use native_render::decal_material::{decal_texture, tint};

/// The switch that turns the cut off (`native-decals.cut=off`).
pub const SWITCH: &str = "native-decals.cut";

/// The frame word whose low half marks a drawn record's owner.
pub const FRAME_WORD: u32 = 0x004b_a038;
/// The mode `0x00043a40` stores for its callbacks (`i16`).
pub const MODE: u32 = 0x004c_8790;
const NONE: u32 = u32::MAX;
/// The longest chain followed: the datum array's own bound.
const LIMIT: usize = 1 << 16;
/// The most vertices one record may draw (4096 quads).
const MAX_VERTICES: u32 = 4 << 12;
/// Bytes of a vertex.
pub const VERTEX_SIZE: u32 = 16;

/// One record the body draws, as it would draw it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// The record's address.
    pub at: u32,
    /// The owner record's address: its `+8` takes the frame's low half.
    pub owner: u32,
    /// What the owner's marker held before the walk.
    pub marker: u16,
    /// The decal tag datum (`+0x2c`) and the image selector (`+0x29`).
    pub decal: u32,
    pub image: i8,
    /// The tint (`+0x24`), `v10` as [`tint`] gives it.
    pub tint: u32,
    /// The first vertex in the stream and the vertex count (4 per quad):
    /// `D3DDevice_DrawVertices(QUADLIST, start, count)` at `0x00044190`.
    pub start: u32,
    pub count: u32,
    /// The texture header the body binds ([`decal_texture`]), or why
    /// Rust cannot say.
    pub texture: Result<u32, &'static str>,
    /// The vertices' bytes (`count` of [`VERTEX_SIZE`]); `None` when they
    /// do not read.
    pub bytes: Option<Arc<[u8]>>,
}

/// A call's walk: the frame's low half and the records drawn, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Walk {
    pub frame: u16,
    pub records: Vec<Record>,
}

fn word(mem: &dyn GuestMemory, addr: u32) -> Result<u32, &'static str> {
    mem.u32(addr).map_err(|_| "unread")
}

/// The records the callback draws for `list` (its first stack argument),
/// in its order. `Err` names what does not read.
pub fn walk(mem: &dyn GuestMemory, list: u32) -> Result<Walk, &'static str> {
    let mode = i32::from(mem.u16(MODE).map_err(|_| "unread")? as i16);
    let bucket = (mode << 9).wrapping_add(i32::from(list as u16 as i16));
    if !(0..7 * BUCKET_CLUSTERS as i32).contains(&bucket) {
        return Err("bucket");
    }
    let mut head = word(mem, word(mem, GROUPS)?.wrapping_add(4 * bucket as u32))?;
    let frame = word(mem, FRAME_WORD)? as u16;
    let mut records = Vec::new();
    let mut marked = HashSet::new();
    let mut followed = 0;
    while head != NONE {
        followed += 1;
        if followed > LIMIT {
            return Err("chain");
        }
        let data = word(mem, word(mem, DECALS)?.wrapping_add(0x44))?;
        let at = (head & 0xffff).wrapping_mul(RECORD_SIZE).wrapping_add(data);
        let owner = (word(mem, at.wrapping_add(0x38))? & 0xffff)
            .wrapping_mul(RECORD_SIZE)
            .wrapping_add(data);
        let marker = mem.u16(owner.wrapping_add(8)).map_err(|_| "unread")?;
        let next = word(mem, at.wrapping_add(0x34))?;
        if marker == frame || marked.contains(&owner) {
            head = next;
            continue;
        }
        marked.insert(owner);
        let packed = word(mem, at.wrapping_add(0x28))?;
        let quads = (packed >> 16) as i16;
        if quads < 0 || (quads as u32) * 4 > MAX_VERTICES {
            return Err("quads");
        }
        let count = quads as u32 * 4;
        // `0x00044161`: the allocation's offset shifted by the allocator's
        // shift, in 16-byte vertices.
        let geometry = word(mem, GEOMETRY)?;
        let allocations = word(
            mem,
            word(mem, geometry.wrapping_add(0x64))?.wrapping_add(0x44),
        )?;
        let alloc = word(mem, at.wrapping_add(4))? & 0xffff;
        let offset = word(mem, allocations.wrapping_add(alloc * 24 + 8))?;
        let shift = word(mem, geometry.wrapping_add(0x34))? & 31;
        let offset = offset << shift;
        let base = word(mem, word(mem, STREAM)?.wrapping_add(4))? | 0x8000_0000;
        let bytes = mem
            .bytes(base.wrapping_add(offset), (count * VERTEX_SIZE) as usize)
            .ok()
            .map(Arc::<[u8]>::from);
        let decal = word(mem, at.wrapping_add(0x2c))?;
        let image = (packed >> 8) as u8 as i8;
        records.push(Record {
            at,
            owner,
            marker,
            decal,
            image,
            tint: word(mem, at.wrapping_add(0x24))?,
            start: offset >> 4,
            count,
            texture: decal_texture(&|a| mem.u32(a).ok(), decal, image),
            bytes,
        });
        head = next;
    }
    Ok(Walk { frame, records })
}

/// What draws the cut records: boot's glue over the device.
pub trait Host: Send + Sync {
    /// Draws `record` at this point of the frame as the guest body would
    /// draw it, or names why not, with nothing recorded.
    fn draw(&self, mem: &mut dyn GuestMemory, record: &Record) -> Result<(), &'static str>;
}

static HOST: Mutex<Option<Arc<dyn Host>>> = Mutex::new(None);

/// Installs the host (boot). Without one every call with records runs
/// the guest body.
pub fn set_host(host: Arc<dyn Host>) {
    *HOST.lock().unwrap_or_else(|e| e.into_inner()) = Some(host);
}

fn host() -> Option<Arc<dyn Host>> {
    HOST.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The calls of `0x0002c370`, one per mode of a view: a host's draws
/// take the state of the call they are in.
static MODE_CALLS: AtomicU64 = AtomicU64::new(0);

/// At `0x0002c370`'s entry: the next mode's callbacks begin.
pub fn next_mode_call() {
    MODE_CALLS.fetch_add(1, Ordering::Relaxed);
}

/// The current `0x0002c370` call.
pub fn mode_call() -> u64 {
    MODE_CALLS.load(Ordering::Relaxed)
}

/// What a call did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    /// Every record is drawn and marked: the call returns.
    Return,
    /// The guest body runs (for the records not yet marked).
    Guest,
}

#[derive(Default)]
struct Counts {
    calls: u64,
    returned: u64,
    empty: u64,
    records: u64,
    /// Calls that ran the guest body, by cause.
    kept: BTreeMap<&'static str, u64>,
    /// Arm differences, by field.
    mismatched: BTreeMap<String, u64>,
    checked: u64,
}

static COUNTS: Mutex<Counts> = Mutex::new(Counts {
    calls: 0,
    returned: 0,
    empty: 0,
    records: 0,
    kept: BTreeMap::new(),
    mismatched: BTreeMap::new(),
    checked: 0,
});
static FALLBACK: AtomicBool = AtomicBool::new(false);

fn counts() -> std::sync::MutexGuard<'static, Counts> {
    COUNTS.lock().unwrap_or_else(|e| e.into_inner())
}

fn keep(cause: &'static str) {
    *counts().kept.entry(cause).or_default() += 1;
}

/// Whether a difference has sent the session back to the guest body.
pub fn fallen_back() -> bool {
    FALLBACK.load(Ordering::Relaxed)
}

thread_local! {
    /// The records the running guest body is expected to draw, in order;
    /// `None` when its walk was not modelled.
    static EXPECTED: RefCell<Option<VecDeque<Record>>> = const { RefCell::new(None) };
}

/// What the arm expects of the guest body's next decal draw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expected {
    /// The call's walk was not modelled: nothing to check.
    Unmodelled,
    /// This record.
    Next(Record),
    /// The walk drew no more records.
    Missing,
}

fn expect(records: impl IntoIterator<Item = Record>) {
    EXPECTED.with(|e| *e.borrow_mut() = Some(records.into_iter().collect()));
}

/// The record the guest body's next decal draw should be (the admission
/// takes one per `D3DDevice_DrawVertices` in a callback).
pub fn take_expected() -> Expected {
    EXPECTED.with(|e| match e.borrow_mut().as_mut() {
        None => Expected::Unmodelled,
        Some(q) => q.pop_front().map_or(Expected::Missing, Expected::Next),
    })
}

/// At the callback's return: a record the walk expected that no draw
/// took is the difference `unseen`.
pub fn end_call() {
    let left = EXPECTED
        .with(|e| e.borrow_mut().take())
        .map_or(0, |q| q.len());
    if left != 0 {
        mismatch(&["unseen".to_owned()]);
    }
}

/// One checked draw and its differing fields: any difference sends every
/// later call to the guest body for the session.
pub fn mismatch(fields: &[String]) {
    let mut c = counts();
    c.checked += 1;
    if fields.is_empty() {
        return;
    }
    for f in fields {
        *c.mismatched.entry(f.clone()).or_default() += 1;
    }
    drop(c);
    if !FALLBACK.swap(true, Ordering::Relaxed) {
        eprintln!(
            "native-decals: a decal draw differs from the cut's walk ({fields:?}); the guest body draws every decal from here"
        );
    }
}

/// The call's own turn at the callback's entry, `list` its first stack
/// argument. On [`Turn::Return`] the records are drawn and marked.
pub fn turn(mem: &mut dyn GuestMemory, list: u32) -> Turn {
    turn_with(mem, list, host().as_deref())
}

fn turn_with(mem: &mut dyn GuestMemory, list: u32, host: Option<&dyn Host>) -> Turn {
    counts().calls += 1;
    EXPECTED.with(|e| *e.borrow_mut() = None);
    let walk = match walk(&*mem, list) {
        Ok(w) => w,
        Err(cause) => {
            keep(cause);
            return Turn::Guest;
        }
    };
    if walk.records.is_empty() {
        let mut c = counts();
        c.empty += 1;
        c.returned += 1;
        return Turn::Return;
    }
    let stop = |cause: &'static str, rest: &[Record]| {
        keep(cause);
        expect(rest.iter().cloned());
        Turn::Guest
    };
    if fallen_back() {
        return stop("fallback", &walk.records);
    }
    let Some(host) = host else {
        return stop("host", &walk.records);
    };
    for (n, r) in walk.records.iter().enumerate() {
        // Marked first, as the body marks before it draws; put back when
        // the host does not draw it, so the guest body does.
        let marker = r.owner.wrapping_add(8);
        if mem.set_u16(marker, walk.frame).is_err() {
            return stop("marker", &walk.records[n..]);
        }
        if let Err(cause) = host.draw(mem, r) {
            if mem.set_u16(marker, r.marker).is_err() {
                // The record stays marked: the guest body would skip it.
                keep("marker");
                mismatch(&["marker".to_owned()]);
                return Turn::Guest;
            }
            return stop(cause, &walk.records[n..]);
        }
        counts().records += 1;
    }
    counts().returned += 1;
    Turn::Return
}

/// `render.scene` rows: `decal_call_{calls,returned,empty,checked}`,
/// `decal_cut_records`, `decal_cut_fallback`, the calls that ran the guest
/// body by cause (`scene_kept_decal_call_<cause>`) and the arm's
/// differences by field (`decal_cut_mismatch_<field>`).
pub fn counters() -> Vec<(String, u64)> {
    let c = counts();
    let mut out = vec![
        ("decal_call_calls".to_owned(), c.calls),
        ("decal_call_returned".to_owned(), c.returned),
        ("decal_call_empty".to_owned(), c.empty),
        ("decal_call_checked".to_owned(), c.checked),
        ("decal_cut_records".to_owned(), c.records),
        ("decal_cut_fallback".to_owned(), u64::from(fallen_back())),
    ];
    out.extend(
        c.kept
            .iter()
            .map(|(k, n)| (format!("scene_kept_decal_call_{k}"), *n)),
    );
    out.extend(
        c.mismatched
            .iter()
            .map(|(f, n)| (format!("decal_cut_mismatch_{f}"), *n)),
    );
    out
}

/// [`counters`] with static names (each distinct name is leaked once).
pub fn rows() -> Vec<(&'static str, u64)> {
    static NAMES: Mutex<BTreeMap<String, &'static str>> = Mutex::new(BTreeMap::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    counters()
        .into_iter()
        .map(|(n, v)| {
            (
                *names
                    .entry(n.clone())
                    .or_insert_with(|| Box::leak(n.into_boxed_str())),
                v,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;

    const BASE: u32 = 0x004b_0000;
    const TABLE: u32 = 0x0056_0000;
    const HEADER: u32 = 0x0057_0000;
    const DATA: u32 = 0x0057_1000;
    const GEO: u32 = 0x0058_0000;
    const GEO_ARRAY: u32 = 0x0058_0100;
    const ALLOCS: u32 = 0x0058_1000;
    const STREAM_AT: u32 = 0x0058_2000;
    /// The stream's base, `| 0x80000000` as the body forms it; the flat
    /// memory is below, so the vertices do not read.
    const VERTICES: u32 = 0x0059_0000;

    fn mem() -> FlatMemory {
        let mut m = FlatMemory::new(BASE, 0x0010_0000);
        m.set_u32(GROUPS, TABLE).unwrap();
        m.set_u32(DECALS, HEADER).unwrap();
        m.set_u32(HEADER + 0x44, DATA).unwrap();
        m.set_u32(GEOMETRY, GEO).unwrap();
        m.set_u32(GEO + 0x64, GEO_ARRAY).unwrap();
        m.set_u32(GEO + 0x34, 4).unwrap();
        m.set_u32(GEO_ARRAY + 0x44, ALLOCS).unwrap();
        m.set_u32(STREAM, STREAM_AT).unwrap();
        m.set_u32(STREAM_AT + 4, VERTICES).unwrap();
        m.set_u32(FRAME_WORD, 0x0001_0007).unwrap();
        m.set_u16(MODE, 2).unwrap();
        for i in 0..7 * BUCKET_CLUSTERS {
            m.set_u32(TABLE + 4 * i, NONE).unwrap();
        }
        m
    }

    /// Record `index`: owner `owner`, `quads` quads in allocation
    /// `index` at offset `offset` (before the shift), link `next`.
    fn record(m: &mut FlatMemory, index: u32, owner: u32, quads: u32, offset: u32, next: u32) {
        let at = DATA + index * RECORD_SIZE;
        m.set_u32(at + 4, index).unwrap();
        m.set_u32(at + 0x24, 0xff00_0000 | index).unwrap();
        m.set_u32(at + 0x28, quads << 16 | 0x03 << 8).unwrap();
        m.set_u32(at + 0x2c, 0xe600_0000 | index).unwrap();
        m.set_u32(at + 0x34, next).unwrap();
        m.set_u32(at + 0x38, owner).unwrap();
        m.set_u32(ALLOCS + index * 24 + 8, offset).unwrap();
    }

    fn head(m: &mut FlatMemory, mode: u32, list: u32, index: u32) {
        m.set_u32(TABLE + 4 * (mode * BUCKET_CLUSTERS + list), index)
            .unwrap();
    }

    fn marker(m: &FlatMemory, index: u32) -> u16 {
        m.u16(DATA + index * RECORD_SIZE + 8).unwrap()
    }

    /// Draws every record, or refuses from the `refuse`-th on.
    #[derive(Default)]
    struct Draws {
        drawn: Mutex<Vec<u32>>,
        refuse: Option<usize>,
    }

    impl Host for Draws {
        fn draw(&self, _mem: &mut dyn GuestMemory, record: &Record) -> Result<(), &'static str> {
            let mut drawn = self.drawn.lock().unwrap();
            if self.refuse.is_some_and(|n| drawn.len() >= n) {
                return Err("texture_streaming");
            }
            drawn.push(record.at);
            Ok(())
        }
    }

    #[test]
    fn the_walk_follows_the_chain_and_skips_marked_owners() {
        let mut m = mem();
        // Bucket (2, 5): records 0, 1, 2, 3. Record 1's owner is marked
        // this frame; record 3 shares record 0's owner.
        record(&mut m, 0, 0, 2, 0x10, 1);
        record(&mut m, 1, 1, 1, 0x20, 2);
        record(&mut m, 2, 2, 1, 0x30, 3);
        record(&mut m, 3, 0, 1, 0x40, NONE);
        m.set_u16(DATA + RECORD_SIZE + 8, 7).unwrap();
        head(&mut m, 2, 5, 0);
        let w = walk(&m, 0xffff_0005).unwrap();
        assert_eq!(w.frame, 7);
        let got: Vec<_> = w
            .records
            .iter()
            .map(|r| (r.at, r.owner, r.start, r.count, r.image, r.tint))
            .collect();
        assert_eq!(
            got,
            [
                (DATA, DATA, 0x10, 8, 3, 0xff00_0000),
                (
                    DATA + 2 * RECORD_SIZE,
                    DATA + 2 * RECORD_SIZE,
                    0x30,
                    4,
                    3,
                    0xff00_0002
                ),
            ]
        );
        // The walk only reads.
        assert_eq!((marker(&m, 0), marker(&m, 2)), (0, 0));
        // The decal tags do not read here.
        assert_eq!(w.records[0].texture, Err("texture_unread"));
    }

    #[test]
    fn a_cut_call_marks_its_records_draws_them_natively_and_returns_without_the_body() {
        let mut m = mem();
        record(&mut m, 0, 0, 2, 0x10, 1);
        record(&mut m, 1, 1, 1, 0x20, NONE);
        head(&mut m, 2, 5, 0);
        let before = m.bytes(BASE, 0x0010_0000).unwrap();
        let host = Draws::default();
        assert_eq!(turn_with(&mut m, 5, Some(&host)), Turn::Return);
        // The native draws, in the body's order.
        assert_eq!(*host.drawn.lock().unwrap(), [DATA, DATA + RECORD_SIZE]);
        // No guest body: the only writes are the two markers the body
        // makes, so nothing reached D3D through the guest.
        let after = m.bytes(BASE, 0x0010_0000).unwrap();
        let changed: Vec<u32> = (0..before.len())
            .filter(|&i| before[i] != after[i])
            .map(|i| BASE + i as u32)
            .collect();
        assert_eq!(changed, [DATA + 8, DATA + RECORD_SIZE + 8]);
        assert_eq!((marker(&m, 0), marker(&m, 1)), (7, 7));
        // A second call this frame finds them marked: nothing to draw.
        assert_eq!(turn_with(&mut m, 5, Some(&host)), Turn::Return);
        assert_eq!(host.drawn.lock().unwrap().len(), 2);
        assert_eq!(take_expected(), Expected::Unmodelled);
    }

    #[test]
    fn an_empty_bucket_returns_with_no_host() {
        let mut m = mem();
        assert_eq!(turn_with(&mut m, 9, None), Turn::Return);
    }

    #[test]
    fn a_record_the_host_refuses_runs_the_body_for_it_and_the_rest() {
        let mut m = mem();
        record(&mut m, 0, 0, 1, 0x10, 1);
        record(&mut m, 1, 1, 1, 0x20, 2);
        record(&mut m, 2, 2, 1, 0x30, NONE);
        head(&mut m, 2, 5, 0);
        let host = Draws {
            refuse: Some(1),
            ..Draws::default()
        };
        assert_eq!(turn_with(&mut m, 5, Some(&host)), Turn::Guest);
        assert_eq!(*host.drawn.lock().unwrap(), [DATA]);
        // The drawn record is marked, so the body skips it; the refused
        // one is not.
        assert_eq!((marker(&m, 0), marker(&m, 1), marker(&m, 2)), (7, 0, 0));
        // The body's draws are checked against the rest of the walk.
        let next = |e| match e {
            Expected::Next(r) => r.at,
            other => panic!("{other:?}"),
        };
        assert_eq!(next(take_expected()), DATA + RECORD_SIZE);
        assert_eq!(next(take_expected()), DATA + 2 * RECORD_SIZE);
        assert_eq!(take_expected(), Expected::Missing);
        assert!(
            counters()
                .iter()
                .any(|(n, v)| n == "scene_kept_decal_call_texture_streaming" && *v >= 1)
        );
    }

    #[test]
    fn no_host_runs_the_body_and_a_bad_bucket_is_not_modelled() {
        let mut m = mem();
        record(&mut m, 0, 0, 1, 0x10, NONE);
        head(&mut m, 2, 5, 0);
        assert_eq!(turn_with(&mut m, 5, None), Turn::Guest);
        assert_eq!(marker(&m, 0), 0);
        assert!(matches!(take_expected(), Expected::Next(_)));
        m.set_u16(MODE, 9).unwrap();
        assert_eq!(turn_with(&mut m, 5, None), Turn::Guest);
        assert_eq!(take_expected(), Expected::Unmodelled);
    }
}
