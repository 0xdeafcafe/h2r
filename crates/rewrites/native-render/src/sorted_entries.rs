//! A layer's sorted entries, from the dispatch boundary `0x0003bc51`
//! (`points::scene_effects::LAYER_BUILT`, docs/graphics/layer-draw.md)
//! to the loop's end: the build `0x0003bab0` has written them, `0x0013da70`
//! has sorted them, and the entry loop `0x0003bc30` dispatches them in
//! order. At the boundary EAX holds the count and the sort buffer is at
//! ESP+8; after each entry the loop reaches `0x0003bc90`
//! (`points::scene_effects::LAYER_ENTRY_DONE`) with ESI at that entry.
//!
//! Each entry is 0x18 bytes. Its half-word `+0` is the item's index in
//! the list, and the loop passes over an entry whose `[[e+0x10]+0x100]`
//! is -1 without a call. A stage that draws an entry's item itself makes
//! the loop pass over it ([`Sorted::pass_over`]), so every other entry
//! keeps its place in the guest's order.

use crate::camera::{GuestRead, GuestWrite};

/// Bytes per entry.
pub const ENTRY_SIZE: u32 = 0x18;
/// The loop's local array holds at most this many (`0xf000 / 0x18`).
pub const MAX_ENTRIES: u32 = 2560;
/// The sort buffer's distance from ESP at the boundary.
pub const BUFFER_FROM_ESP: u32 = 8;
/// The word of a passed-over entry the loop's test reads: the entry's
/// own `+0x14`, set to -1, through `+0x10` pointing 0x100 below it.
const PASS_WORD: u32 = 0x14;

/// The shader draw's cache (`0x00016b90`, docs: `layer_pass::skip_holds`):
/// the cached pass descriptor and implementation under the layer setup's
/// context `0x00485b48`.
pub const CACHED_DESCRIPTOR: u32 = 0x0048_5b48 + 0x20;
pub const CACHED_IMPLEMENTATION: u32 = 0x0048_5b48 + 0x24;
/// The implementation's restore range: a half-word, its low nine bits the
/// first state and the rest the count, into the descriptor's state table
/// at `+0x14` of five-byte records (the state byte, the value word).
const IMPLEMENTATION_RESTORE: u32 = 0x11c;
const DESCRIPTOR_STATES: u32 = 0x14;
const STATE_SIZE: u32 = 5;

/// The sorted entries of the layer being drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sorted {
    pub buffer: u32,
    pub count: u32,
}

/// One entry: the list slot of its item, and whether the loop passes over
/// it without a call (it draws nothing and sets nothing).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub slot: u32,
    pub passed: bool,
}

impl Sorted {
    /// The entries at the boundary from ESP and EAX. `None` for no entry
    /// (the loop does not run) or a count past the array.
    pub fn at(esp: u32, eax: u32) -> Option<Self> {
        let count = eax;
        if count as i32 <= 0 || count > MAX_ENTRIES {
            return None;
        }
        Some(Self { buffer: esp.checked_add(BUFFER_FROM_ESP)?, count })
    }

    fn address(&self, k: u32) -> Option<u32> {
        (k < self.count).then_some(())?;
        self.buffer.checked_add(k.checked_mul(ENTRY_SIZE)?)
    }

    /// The index of the entry at `esi`, `None` outside the buffer.
    pub fn index_of(&self, esi: u32) -> Option<u32> {
        let offset = esi.checked_sub(self.buffer)?;
        (offset % ENTRY_SIZE == 0 && offset / ENTRY_SIZE < self.count).then_some(offset / ENTRY_SIZE)
    }

    /// Entry `k` in draw order.
    pub fn entry(&self, mem: &dyn GuestRead, k: u32) -> Option<Entry> {
        let e = self.address(k)?;
        let slot = mem.read_u32(e)? & 0xffff;
        let target = mem.read_u32(e.checked_add(0x10)?)?;
        let passed = mem.read_u32(target.checked_add(0x100)?)? == u32::MAX;
        Some(Entry { slot, passed })
    }

    /// Makes the loop pass over entry `k`: no handler runs for it, and the
    /// entries around it keep their order. The loop reads nothing else of
    /// an entry it passes over.
    pub fn pass_over(&self, mem: &mut dyn GuestWrite, k: u32) -> Option<()> {
        let e = self.address(k)?;
        let word = e.checked_add(PASS_WORD)?;
        mem.write_u32(word, u32::MAX)?;
        mem.write_u32(e.checked_add(0x10)?, word.checked_sub(0x100)?)
    }
}

/// The restore list of the implementation the shader draw has cached,
/// as (state, value) by guest state index: what the next implementation
/// switch writes before it sets its own list. Empty with nothing cached
/// (the layer setup clears the cache before the first entry). `None` when
/// a word does not read.
pub fn cached_restore(mem: &dyn GuestRead) -> Option<Vec<(u32, u32)>> {
    let implementation = mem.read_u32(CACHED_IMPLEMENTATION)?;
    let descriptor = mem.read_u32(CACHED_DESCRIPTOR)?;
    if implementation == 0 || descriptor == 0 {
        return Some(Vec::new());
    }
    let range = mem.read_u32(implementation.checked_add(IMPLEMENTATION_RESTORE)?)? & 0xffff;
    let table = mem.read_u32(descriptor.checked_add(DESCRIPTOR_STATES)?)?;
    let start = range & 0x1ff;
    (start..start + (range >> 9))
        .map(|index| {
            let at = table.checked_add(STATE_SIZE.checked_mul(index)?)?;
            Some((mem.read_u32(at)? & 0xff, mem.read_u32(at.checked_add(1)?)?))
        })
        .collect()
}

/// The device as the next implementation switch leaves it: `restore`
/// ([`cached_restore`]) over `device`.
pub fn after_restore<'a>(restore: &'a [(u32, u32)], device: &'a dyn Fn(u32) -> Option<u32>) -> impl Fn(u32) -> Option<u32> + 'a {
    move |state| restore.iter().rev().find(|(s, _)| *s == state).map(|&(_, v)| v).or_else(|| device(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;

    const ESP: u32 = 0x0010_0000;
    const DRAWS: u32 = 0x0020_0000;
    const PASSES: u32 = 0x0020_1000;

    /// Entries for `slots` at the boundary; a slot in `passed` points at a
    /// target the loop passes over.
    fn sorted(m: &mut Mem, slots: &[u32], passed: &[u32]) -> Sorted {
        m.w(DRAWS + 0x100, 0);
        m.w(PASSES + 0x100, u32::MAX);
        let s = Sorted::at(ESP, slots.len() as u32).unwrap();
        for (k, &slot) in slots.iter().enumerate() {
            let e = s.buffer + ENTRY_SIZE * k as u32;
            for w in 0..6 {
                m.w(e + 4 * w, 0x1000 * k as u32 + w);
            }
            m.w(e, 0xab00_0000 | slot);
            m.w(e + 0x10, if passed.contains(&slot) { PASSES } else { DRAWS });
        }
        s
    }

    #[test]
    fn the_entries_read_in_draw_order_with_their_item_slots() {
        let mut m = Mem::default();
        let s = sorted(&mut m, &[4, 1, 7], &[1]);
        assert_eq!(s.buffer, ESP + 8);
        let read: Vec<Entry> = (0..3).map(|k| s.entry(&m, k).unwrap()).collect();
        assert_eq!(
            read,
            vec![Entry { slot: 4, passed: false }, Entry { slot: 1, passed: true }, Entry { slot: 7, passed: false }]
        );
        assert_eq!(s.entry(&m, 3), None);
        assert_eq!(s.index_of(s.buffer + 2 * ENTRY_SIZE), Some(2));
        assert_eq!(s.index_of(s.buffer + 3 * ENTRY_SIZE), None);
        assert_eq!(s.index_of(s.buffer + 4), None);
        assert_eq!(Sorted::at(ESP, 0), None, "no entry: the loop does not run");
        assert_eq!(Sorted::at(ESP, u32::MAX), None);
        assert_eq!(Sorted::at(ESP, MAX_ENTRIES + 1), None);
    }

    #[test]
    fn a_passed_over_entry_reads_as_passed_and_keeps_its_slot_and_neighbours() {
        let mut m = Mem::default();
        let s = sorted(&mut m, &[4, 1, 7], &[]);
        s.pass_over(&mut m, 1).unwrap();
        assert_eq!(s.entry(&m, 1), Some(Entry { slot: 1, passed: true }));
        assert_eq!(s.entry(&m, 0), Some(Entry { slot: 4, passed: false }));
        assert_eq!(s.entry(&m, 2), Some(Entry { slot: 7, passed: false }));
        assert_eq!(m.read_u32(s.buffer + ENTRY_SIZE).unwrap() & 0xffff, 1, "word 0 untouched");
        assert_eq!(s.pass_over(&mut m, 3), None);
    }

    #[test]
    fn the_cached_restore_list_reads_from_the_descriptor_table() {
        let mut m = Mem::default();
        let (imp, desc, table) = (0x0030_0000, 0x0030_1000, 0x0030_2000);
        m.w(CACHED_IMPLEMENTATION, 0);
        m.w(CACHED_DESCRIPTOR, desc);
        assert_eq!(cached_restore(&m), Some(Vec::new()), "nothing cached");
        m.w(CACHED_IMPLEMENTATION, imp);
        m.w(imp + IMPLEMENTATION_RESTORE, (2 << 9) | 1);
        m.w(desc + DESCRIPTOR_STATES, table);
        // Record 1 at table+5 and record 2 at table+10, read as words.
        m.w(table + 5, 0x0000_2a07);
        m.w(table + 6, 0x0000_002a);
        m.w(table + 10, 0x0000_0109);
        m.w(table + 11, 0x0000_0001);
        assert_eq!(cached_restore(&m), Some(vec![(7, 0x2a), (9, 1)]));
        let device = |s: u32| Some(s * 100);
        let after = after_restore(&[(7, 0x2a)], &device);
        assert_eq!((after(7), after(8)), (Some(0x2a), Some(800)));
    }
}
