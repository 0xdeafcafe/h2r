//! The map cache status `0x213f10` (`points::MAP_STATUS`), answered from
//! its last result while nothing it reads has changed
//! (docs/runtime/cache-slots.md, "The status poll").
//!
//! The status is a pure function of guest memory. It normalises its path
//! argument with `sprintf` (`0x214db0`) and compares it with the six slot
//! paths (`0x214a60`, case-insensitive), the eight copy requests
//! (`0x214060`, exact) and the two queued paths (`0x213e30`, `0x213e90`,
//! each normalised again). Up to seven `sprintf` calls run through the CRT
//! for one answer, and the frame poll `0x163680` asks three to five times a
//! frame with the same paths.
//!
//! [`Cache`] keys an answer on everything the original can read:
//!
//! - the entry ESP and the path pointer;
//! - the path's bytes through its NUL, at most [`PATH_MAX`] of them (the
//!   normalised path is then at most `PATH_MAX + 12` bytes, so every
//!   compare stops inside the blocks below);
//! - [`State`]: the six slot paths, the copy request block, the queue
//!   block through `0x55c02b`, and the CRT locale handle `0x480ce8`.
//!
//! The answer is the registers the original leaves that are not its
//! callee-saved ones: EAX, ECX, EDX and the arithmetic flags (their last
//! writer is `add esp, 100h`, so they depend on ESP, which the key holds).
//! The original's other writes are to its own stack below the entry ESP,
//! the dead stack a replacement does not reproduce
//! (docs/engine/README.md, "How the contracts were checked").
//!
//! A path with no NUL in reach, a queued path with no NUL in its block,
//! or a locale other than C (`tolower` at `0x321846` then reads tables)
//! is not cached: the original runs ([`Lookup::Bypass`]).

use guest_layout::memory::GuestMemory;

/// The slot records' resident paths: header `+0x20` of the records at
/// `0x557c90` (stride `0x804`), which `0x214a60` walks.
pub const SLOT_PATHS: u32 = 0x0055_7cb4;
pub const SLOT_STRIDE: u32 = 0x804;
pub const SLOTS: u32 = 6;
pub const SLOT_PATH_BYTES: usize = 0x100;
/// The copy requests `0x214060` walks: eight of `0x108` bytes, a path and
/// at `+0x104` the state it tests (`>= 3`: status 4).
pub const REQUESTS: u32 = 0x0055_acac;
pub const REQUEST_BYTES: usize = 8 * 0x108;
/// The queued path `0x213e30` normalises (`0x55bd21`), the value it returns
/// (`0x55bf24`), the second queued path `0x213e90` normalises (`0x55bf28`)
/// and its value (`0x55c028`): one block through `0x55c02b`.
pub const QUEUE: u32 = 0x0055_bd21;
pub const QUEUE_BYTES: usize = 0x0055_c02c - 0x0055_bd21;
/// The second queued path's offset in the queue block.
pub const QUEUE_SECOND: usize = 0x0055_bf28 - 0x0055_bd21;
/// The CRT's locale handle: 0 is the C locale, in which `tolower` reads
/// nothing else.
pub const LOCALE: u32 = 0x0048_0ce8;
/// The longest path argument cached, without its NUL.
pub const PATH_MAX: usize = 0xf0;
/// The flags the original writes: CF, PF, AF, ZF, SF, OF.
pub const STATUS_FLAGS: u32 = 0x8d5;

const STATE_BYTES: usize = SLOTS as usize * SLOT_PATH_BYTES + REQUEST_BYTES + QUEUE_BYTES + 4;
const QUEUE_AT: usize = SLOTS as usize * SLOT_PATH_BYTES + REQUEST_BYTES;
const ENTRIES_MAX: usize = 16;
const PENDING_MAX: usize = 8;
const PAGE: u32 = 0x1000;

/// What the original leaves in the registers it does not preserve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Answer {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
    /// `eflags & STATUS_FLAGS`.
    pub flags: u32,
    /// `eflags_defined & STATUS_FLAGS`.
    pub flags_defined: u32,
}

impl Answer {
    /// The answer in registers as the original returned them.
    pub fn of(eax: u32, ecx: u32, edx: u32, eflags: u32, eflags_defined: u32) -> Self {
        Answer {
            eax,
            ecx,
            edx,
            flags: eflags & STATUS_FLAGS,
            flags_defined: eflags_defined & STATUS_FLAGS,
        }
    }
    /// `eflags` and `eflags_defined` with the status flags replaced.
    pub fn merge_flags(&self, eflags: u32, eflags_defined: u32) -> (u32, u32) {
        (
            (eflags & !STATUS_FLAGS) | self.flags,
            (eflags_defined & !STATUS_FLAGS) | self.flags_defined,
        )
    }
}

/// What to do at an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// Return this; the original does not run.
    Hit(Answer),
    /// Run the original, then [`Cache::record`] its answer.
    Miss,
    /// Run the original; its answer is not kept.
    Bypass(&'static str),
}

/// Counts since the cache was made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub hits: u64,
    pub misses: u64,
    pub bypasses: u64,
    /// Misses whose answer was dropped: what the original read changed
    /// while it ran (another guest thread between slices).
    pub dropped: u64,
}

struct Entry {
    esp: u32,
    ptr: u32,
    path: Vec<u8>,
    answer: Answer,
}

struct Pending {
    esp: u32,
    ptr: u32,
    path: Vec<u8>,
    state: Box<[u8]>,
}

/// The answers, all for one [`State`]. See the module docs.
#[derive(Default)]
pub struct Cache {
    state: Option<Box<[u8]>>,
    entries: Vec<Entry>,
    pending: Vec<Pending>,
    scratch: Vec<u8>,
    pub stats: Stats,
}

impl Cache {
    pub fn new() -> Self {
        Self::default()
    }

    /// At the entry, with the entry ESP and the path pointer (EAX).
    pub fn lookup(&mut self, mem: &dyn GuestMemory, esp: u32, ptr: u32) -> Lookup {
        let path = match read_path(mem, ptr) {
            Ok(path) => path,
            Err(why) => {
                self.stats.bypasses += 1;
                return Lookup::Bypass(why);
            }
        };
        let mut scratch = std::mem::take(&mut self.scratch);
        if let Err(why) = read_state(mem, &mut scratch) {
            self.scratch = scratch;
            self.stats.bypasses += 1;
            return Lookup::Bypass(why);
        }
        if self.state.as_deref() == Some(&scratch[..])
            && let Some(e) = self
                .entries
                .iter()
                .find(|e| e.esp == esp && e.ptr == ptr && e.path == path)
        {
            let answer = e.answer;
            self.scratch = scratch;
            self.stats.hits += 1;
            return Lookup::Hit(answer);
        }
        self.pending.retain(|p| p.esp != esp);
        if self.pending.len() >= PENDING_MAX {
            // A call that faulted or never returned left its entry here.
            self.pending.remove(0);
        }
        self.pending.push(Pending {
            esp,
            ptr,
            path,
            state: scratch[..].into(),
        });
        self.scratch = scratch;
        self.stats.misses += 1;
        Lookup::Miss
    }

    /// After the original returned to the call that missed at `esp` (the
    /// entry ESP), with the registers it left. The answer is kept only if
    /// the path and the state still read as they did at the entry.
    pub fn record(&mut self, mem: &dyn GuestMemory, esp: u32, answer: Answer) {
        let Some(i) = self.pending.iter().position(|p| p.esp == esp) else {
            return;
        };
        let p = self.pending.swap_remove(i);
        let mut scratch = std::mem::take(&mut self.scratch);
        let same = read_path(mem, p.ptr).is_ok_and(|path| path == p.path)
            && read_state(mem, &mut scratch).is_ok()
            && scratch[..] == p.state[..];
        self.scratch = scratch;
        if !same {
            self.stats.dropped += 1;
            return;
        }
        if self.state.as_deref() != Some(&p.state[..]) {
            self.entries.clear();
            self.state = Some(p.state);
        }
        self.entries
            .retain(|e| !(e.esp == p.esp && e.ptr == p.ptr && e.path == p.path));
        if self.entries.len() >= ENTRIES_MAX {
            self.entries.remove(0);
        }
        self.entries.push(Entry {
            esp: p.esp,
            ptr: p.ptr,
            path: p.path,
            answer,
        });
    }

    /// Answers kept for the current state.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The path's bytes through its NUL. Reads a page at a time, never past
/// the page holding the NUL: the original's `strrchr` reads to the NUL, so
/// no page is read that it would not read.
pub fn read_path(mem: &dyn GuestMemory, ptr: u32) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::with_capacity(32);
    let mut buf = [0u8; PATH_MAX + 1];
    let mut at = ptr;
    while out.len() <= PATH_MAX {
        let room = (PAGE - (at & (PAGE - 1))) as usize;
        let n = room.min(PATH_MAX + 1 - out.len());
        mem.read(at, &mut buf[..n])
            .map_err(|_| "the path is not readable")?;
        if let Some(z) = buf[..n].iter().position(|&b| b == 0) {
            out.extend_from_slice(&buf[..=z]);
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n]);
        at = at.wrapping_add(n as u32);
    }
    Err("the path is longer than 240 bytes")
}

/// Everything the original reads besides the path, into `out`.
pub fn read_state(mem: &dyn GuestMemory, out: &mut Vec<u8>) -> Result<(), &'static str> {
    out.clear();
    out.resize(STATE_BYTES, 0);
    let unreadable = |_| "the cache slot globals are not readable";
    for slot in 0..SLOTS as usize {
        let at = SLOT_PATHS + slot as u32 * SLOT_STRIDE;
        let range = slot * SLOT_PATH_BYTES..(slot + 1) * SLOT_PATH_BYTES;
        mem.read(at, &mut out[range]).map_err(unreadable)?;
    }
    let requests = SLOTS as usize * SLOT_PATH_BYTES;
    mem.read(REQUESTS, &mut out[requests..requests + REQUEST_BYTES])
        .map_err(unreadable)?;
    mem.read(QUEUE, &mut out[QUEUE_AT..QUEUE_AT + QUEUE_BYTES])
        .map_err(unreadable)?;
    // The second queued path ends inside the block, and so the first does:
    // it ends at its own NUL or at the second's.
    if !out[QUEUE_AT + QUEUE_SECOND..QUEUE_AT + QUEUE_BYTES].contains(&0) {
        return Err("a queued path has no NUL in its block");
    }
    let locale = QUEUE_AT + QUEUE_BYTES;
    mem.read(LOCALE, &mut out[locale..locale + 4])
        .map_err(unreadable)?;
    if out[locale..] != [0u8; 4] {
        return Err("the CRT locale is not C");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const ESP: u32 = 0x7f_f000;
    const PATH: u32 = 0x46_0000;

    fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        for slot in 0..SLOTS {
            g.map(SLOT_PATHS + slot * SLOT_STRIDE, SLOT_PATH_BYTES as u32);
        }
        g.map(REQUESTS, REQUEST_BYTES as u32)
            .map(QUEUE, QUEUE_BYTES as u32)
            .map(LOCALE, 4)
            .put(PATH, b"d:\\maps\\mainmenu.map\0");
        g
    }

    fn answer(eax: u32) -> Answer {
        Answer::of(eax, 0x11, 0x22, 0x8d5 | 0x400, 0xfff)
    }

    #[test]
    fn a_recorded_answer_is_returned_for_the_same_key_and_state() {
        let g = guest();
        let mut c = Cache::new();
        assert_eq!(c.lookup(&g, ESP, PATH), Lookup::Miss);
        c.record(&g, ESP, answer(3));
        assert_eq!(c.lookup(&g, ESP, PATH), Lookup::Hit(answer(3)));
        assert_eq!(c.stats.hits, 1);
        assert_eq!(c.stats.misses, 1);
        // The direction flag and the rest are not the answer's.
        assert_eq!(answer(3).flags, 0x8d5);
        assert_eq!(answer(3).merge_flags(0x202, 0), (0x202 | 0x8d5, 0x8d5));
    }

    #[test]
    fn another_esp_pointer_or_path_misses() {
        let mut g = guest();
        let mut c = Cache::new();
        assert_eq!(c.lookup(&g, ESP, PATH), Lookup::Miss);
        c.record(&g, ESP, answer(3));
        assert_eq!(c.lookup(&g, ESP - 0x10, PATH), Lookup::Miss);
        g.put(PATH + 0x100, b"d:\\maps\\mainmenu.map\0");
        assert_eq!(c.lookup(&g, ESP, PATH + 0x100), Lookup::Miss);
        g.put(PATH + 9, b"M");
        assert_eq!(c.lookup(&g, ESP, PATH), Lookup::Miss);
    }

    #[test]
    fn any_change_to_what_the_original_reads_misses() {
        let edits: [(u32, u8); 7] = [
            (SLOT_PATHS, b'd'),
            (SLOT_PATHS + 5 * SLOT_STRIDE + 0xff, 1),
            (REQUESTS + 7 * 0x108 + 0x104, 3),
            (QUEUE, b'x'),
            (0x0055_bf24, 1),
            (0x0055_c02b, 1),
            (LOCALE + 3, 1),
        ];
        for (at, byte) in edits {
            let mut g = guest();
            let mut c = Cache::new();
            assert_eq!(c.lookup(&g, ESP, PATH), Lookup::Miss);
            c.record(&g, ESP, answer(0));
            g.put_u8(at, byte);
            assert_ne!(c.lookup(&g, ESP, PATH), Lookup::Hit(answer(0)), "{at:#x}");
        }
    }

    #[test]
    fn a_state_change_during_the_call_drops_the_answer() {
        let mut g = guest();
        let mut c = Cache::new();
        assert_eq!(c.lookup(&g, ESP, PATH), Lookup::Miss);
        g.put_u8(REQUESTS, b'd');
        c.record(&g, ESP, answer(4));
        assert_eq!(c.stats.dropped, 1);
        assert!(c.is_empty());
        assert_eq!(c.lookup(&g, ESP, PATH), Lookup::Miss);
    }

    #[test]
    fn a_new_state_forgets_the_old_answers() {
        let mut g = guest();
        let mut c = Cache::new();
        assert_eq!(c.lookup(&g, ESP, PATH), Lookup::Miss);
        c.record(&g, ESP, answer(0));
        g.put(SLOT_PATHS, b"d:\\maps\\mainmenu.map\0");
        assert_eq!(c.lookup(&g, ESP, PATH), Lookup::Miss);
        c.record(&g, ESP, answer(3));
        assert_eq!(c.len(), 1);
        assert_eq!(c.lookup(&g, ESP, PATH), Lookup::Hit(answer(3)));
    }

    #[test]
    fn uncacheable_inputs_bypass() {
        let mut g = guest();
        let mut c = Cache::new();
        g.put(PATH, &[b'a'; PATH_MAX + 1]);
        assert!(matches!(c.lookup(&g, ESP, PATH), Lookup::Bypass(_)));
        g.put(PATH + PATH_MAX as u32, &[0]);
        assert_eq!(c.lookup(&g, ESP, PATH), Lookup::Miss);
        assert!(matches!(c.lookup(&g, ESP, 0x10), Lookup::Bypass(_)));

        let mut g = guest();
        g.put(
            QUEUE + QUEUE_SECOND as u32,
            &[b'q'; QUEUE_BYTES - QUEUE_SECOND],
        );
        assert!(matches!(c.lookup(&g, ESP, PATH), Lookup::Bypass(_)));
        let mut g = guest();
        g.put_u32(LOCALE, 1);
        assert!(matches!(c.lookup(&g, ESP, PATH), Lookup::Bypass(_)));
        assert_eq!(c.stats.bypasses, 4);
    }

    #[test]
    fn a_path_ending_at_a_page_edge_reads_no_further() {
        let mut g = FakeGuest::new();
        let end = 0x46_1000;
        g.put(end - 4, b"abc\0");
        assert_eq!(read_path(&g, end - 4).unwrap(), b"abc\0");
        g.put(end - 3, b"bcd");
        assert!(read_path(&g, end - 4).is_err());
        g.put(end, b"e\0");
        assert_eq!(read_path(&g, end - 4).unwrap(), b"abcde\0");
    }

    #[test]
    fn calls_that_never_returned_do_not_grow_the_pending_list() {
        let g = guest();
        let mut c = Cache::new();
        for i in 0..32 {
            assert_eq!(c.lookup(&g, ESP - 0x100 * i, PATH), Lookup::Miss);
        }
        assert!(c.pending.len() <= PENDING_MAX);
        c.record(&g, ESP - 0x100 * 31, answer(1));
        assert_eq!(c.lookup(&g, ESP - 0x100 * 31, PATH), Lookup::Hit(answer(1)));
    }
}
