//! Enter the real sort or the first unported build callback with the
//! layer builder's own frame. The guest can change the list in that callback.

use guest_layout::memory::GuestMemory;
use native_render::skip;
use runtime::{CpuAbi, TransferAbi};

use super::scene_build;

mod parity;
pub(super) use parity::{Decision, complete, finish, verifying};
pub(crate) use parity::counters;

const CALLBACK_RETURN: u32 = 0x0003_bb2e;
const SORT_RETURN: u32 = 0x0003_bb73;
const DEFAULT_COMPARE: u32 = 0x0003_bb80;
const DISTANCE_COMPARE: u32 = 0x0003_bbd0;
const BUILD_CALLER: u32 = guest_layout::points::scene_effects::LAYER_BUILD_RETURN;
const SLOT: usize = 24;

#[derive(Clone, Copy)]
struct Callback {
    target: u32, i: u32, item: u32, slot: u32, shader: u32, packed: u32, selector: u8, lod: u8,
}

struct Build {
    list: u32,
    arg: u32,
    layer: u32,
    buffer: u32,
    end: u32,
    entries: Vec<[u8; SLOT]>,
    count: usize,
    builds: u32,
    callback: Option<Callback>,
    models: bool,
}

fn plan(m: &dyn GuestMemory, sp: u32, models: bool) -> Option<Build> {
    if m.u32(sp).ok()? != BUILD_CALLER { return None; }
    let list = m.u32(sp.checked_add(4)?).ok()?;
    let arg = m.u32(sp.checked_add(8)?).ok()?;
    let layer = m.u32(sp.checked_add(12)?).ok()?;
    let buffer = m.u32(sp.checked_add(16)?).ok()?;
    if !matches!(layer, 1 | 3 | 5 | 6 | 7 | 8 | 16) || list == 0 { return None; }
    let end = m.u32(list.checked_add(4)?).ok()?;
    let first = m.u32(list.checked_add(12)?).ok()?;
    if first >= end || end > u32::from(u16::MAX) { return None; }
    let items = m.u32(list.checked_add(20)?).ok()?;
    items.checked_add(end * skip::ITEM_SIZE)?;
    let bit = 1 << layer;
    let mut entries = Vec::new();
    let mut count = 0usize;
    let mut builds = 0;
    let mut callback = None;
    let mut used_model = false;
    for i in first..end {
        let item = items + i * skip::ITEM_SIZE;
        if m.u32(item + 16).ok()? & bit == 0 { continue; }
        if count >= 2560 { return None; }
        let shader = m.u32(item + 8).ok()?;
        let packed = m.u32(item + 12).ok()?;
        let selector = m.u8(item + 20).ok()?;
        let lod = m.u8(item + 21).ok()?;
        let target = m.u32(item).ok()?;
        let slot_at = buffer.checked_add((count * SLOT) as u32)?;
        slot_at.checked_add(skip::ITEM_SIZE)?;
        let mut slot = [0; SLOT];
        if let Some(old) = entries.get(count) {
            slot = *old;
        } else {
            m.read(slot_at, &mut slot).ok()?;
        }
        slot[..2].copy_from_slice(&(i as u16).to_le_bytes());
        slot[2..4].copy_from_slice(&(shader as u16).to_le_bytes());
        let built = (target == skip::GEOMETRY_BUILD)
            .then(|| scene_build::build(m, shader, lod, layer, selector, packed, slot, models)).flatten();
        if let Some(entry) = built {
            used_model |= entry.model;
            entries.truncate(count);
            entries.push(entry.bytes);
            count += usize::from(entry.accepted);
            builds += 1;
        } else {
            if builds == 0 && (i == first || !super::claimed_entries(layer)) { return None; }
            entries.truncate(count);
            entries.push(slot);
            callback = Some(Callback { target, i, item, slot: slot_at, shader, packed, selector, lod });
            break;
        }
    }
    if builds == 0 && callback.is_none() { return None; }

    Some(Build { list, arg, layer, buffer, end, entries, count, builds, callback, models: used_model })
}

pub(super) fn prefix(m: &mut dyn GuestMemory, c: &mut CpuAbi) -> Option<(TransferAbi, u32)> {
    let build = plan(m, c.esp, false)?;
    resume(m, c, build)
}

fn resume(m: &mut dyn GuestMemory, c: &mut CpuAbi, build: Build) -> Option<(TransferAbi, u32)> {
    let sp = c.esp;
    let Build { list, arg, layer, buffer, end, entries, count, builds, callback, .. } = build;
    let bit = 1 << layer;
    // Keep the two writes transactional. A refusal must leave the initial
    // sort slot intact, including words inherited from a previous layer.
    let stack = sp.checked_sub(60)?;
    let mut frame = [0; 60];
    m.read(stack, &mut frame).ok()?;
    let old_frame = frame;
    let mut write = |offset: u32, word: u32| {
        let i = (60 - offset) as usize;
        frame[i..i+4].copy_from_slice(&word.to_le_bytes());
    };
    write(16, c.ebp);
    write(20, c.esi);
    let count = count as u32;
    let transfer = if let Some(Callback { target, i, slot, shader, packed, selector, lod, .. }) = callback {
        write(24, c.ebx);
        write(28, c.edi);
        write(4, bit);
        write(8, i * skip::ITEM_SIZE);
        write(12, count + 1);
        for (n, word) in [slot, packed, u32::from(selector), layer, u32::from(lod), arg, shader, CALLBACK_RETURN].into_iter().enumerate() {
            write(32 + 4*n as u32, word);
        }
        TransferAbi::call(target, CALLBACK_RETURN)
    } else {
        write(24, list);
        write(28, if layer == 5 { DISTANCE_COMPARE } else { DEFAULT_COMPARE });
        write(32, skip::ITEM_SIZE);
        write(36, SORT_RETURN);
        TransferAbi::call(guest_layout::points::LAYER_SORT.base, SORT_RETURN)
    };
    // Buffer and private frame are disjoint at LAYER_ENTRIES' call site.
    if buffer < sp.checked_add(20)? { return None; }
    m.write(stack, &frame).ok()?;
    if m.write(buffer, entries.as_flattened()).is_err() {
        m.write(stack, &old_frame).expect("restore layer build frame before fallback");
        return None;
    }
    if let Some(Callback { i, item, slot, shader, lod, .. }) = callback {
        c.esp = stack;
        c.eax = item;
        c.ecx = arg;
        c.edx = shader;
        c.esi = i;
        c.edi = slot + skip::ITEM_SIZE;
        c.ebp = (count + 1) & !0xffff | shader & 0xffff;
        c.ebx = u32::from(lod);
    } else {
        c.esp = sp - 36;
        c.eax = buffer;
        c.ecx = count;
        c.edx = list;
        c.esi = end;
        c.ebp = count;
    }
    Some((transfer, builds))
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::FlatMemory;


    pub(super) const SP: u32 = 0x510000;
    pub(super) const BUF: u32 = SP+0x1c;
    pub(super) const LIST: u32 = SP+0x200;
    pub(super) const ITEMS: u32 = LIST+0x100;
    pub(super) const BSP: u32 = ITEMS+0x100;
    pub(super) const SECTION: u32 = BSP+0x100;
    pub(super) const HEADER: u32 = BSP+0x300;
    pub(super) const TABLE: u32 = HEADER+0x500;

    pub(super) fn fixture() -> FlatMemory {
        let mut m = FlatMemory::new(0x400000, 0x380000);
        for (at, v) in [(SP, BUILD_CALLER), (SP+4, LIST), (SP+8, 99), (SP+12, 1), (SP+16, BUF),
            (LIST, 32), (LIST+4, 1), (LIST+20, ITEMS), (ITEMS, skip::GEOMETRY_BUILD), (ITEMS+8, 1), (ITEMS+16, 2),
            (skip::RENDER_RECORDS+4, u32::MAX), (crate::native_world::ACTIVE_BSP, BSP), (BSP+0xa0, SECTION),
            (SECTION+0x50, SECTION+0x80), (SECTION+0x80, 1),
            (guest_layout::tags::TAG_ENTRIES_GLOBAL, TABLE), (TABLE+16, u32::from_be_bytes(*b"shad")),
            (TABLE+24, HEADER+0x40), (HEADER+0x64, HEADER+0x80), (HEADER+0x80, 2),
            (TABLE+40, HEADER+0xc0), (HEADER+0x11c, HEADER), (HEADER+4, HEADER+0x140),
            (HEADER+12, HEADER+0x170), (HEADER+20, HEADER+0x190), (HEADER+0x194, 3),
            (TABLE+56, HEADER+0x1c0), (HEADER+0x1e0, HEADER+0x200), (HEADER+0x204, HEADER+0x220),
            (HEADER+0x320, 4), (TABLE+72, HEADER+0x350), (HEADER+0x354, 1), (HEADER+0x358, HEADER+0x380),
            (HEADER+0x38c, 16)] { m.set_u32(at, v).unwrap(); }
        m.set_u16(HEADER+0x172, 1<<9).unwrap();
        m
    }

    #[test]
    fn native_prefix_enters_the_real_sort_or_the_first_foreign_callback_in_order() {
        let mut m = fixture();
        let mut c = CpuAbi { esp: SP, ebp: 123, esi: 456, ..Default::default() };
        let (t, n) = prefix(&mut m, &mut c).unwrap();
        assert_eq!((t.target_pc, t.resume_pc, n), (guest_layout::points::LAYER_SORT.base, SORT_RETURN, 1));
        assert_eq!((c.ecx,c.eax,c.esp), (1,BUF,SP-36));
        assert_eq!(m.u32(BUF+16).unwrap(), HEADER+0x220);
        assert_eq!(m.u32(SP-16).unwrap(), 123);
        assert_eq!(m.u32(SP-20).unwrap(), 456);

        m.set_u32(LIST+4, 2).unwrap();
        m.set_u32(ITEMS+24, 0x123456).unwrap();
        m.set_u32(ITEMS+24+8, 7).unwrap();
        m.set_u32(ITEMS+24+16, 2).unwrap();
        let mut c = CpuAbi { esp: SP, ..Default::default() };
        let (t, n) = prefix(&mut m, &mut c).unwrap();
        assert_eq!((t.target_pc,t.resume_pc,n), (0x123456,CALLBACK_RETURN,1));
        assert_eq!(c.esi, 1);
        assert_eq!(m.u32(c.esp+28).unwrap(), BUF+24);
        assert_eq!(m.u32(c.esp+8).unwrap(), 99);
        assert_eq!(m.u16(BUF+24).unwrap(), 1);
        assert_eq!(m.u16(BUF+26).unwrap(), 7);
    }
}
