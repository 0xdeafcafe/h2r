//! Read the private, sorted dispatch records of the light's current layer.

use crate::camera::GuestRead;
use crate::skip::{self, GEOMETRY_DRAW, ITEM_SIZE};
use crate::world::Owner;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Item {
    pub part: (Owner, u8),
    pub shader: u32,
    pub layers: u32,
}

/// The entry loop sorts its private 0x18-byte records after the build
/// callbacks. Only this list includes the build refusals, sort tie order,
/// chosen implementation and the dispatcher's geometry exclusion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortedItem {
    pub slot: u32,
    pub handler_item: u32,
    pub geometry: Option<Item>,
    pub implementation: u8,
}

#[derive(Debug)]
pub struct SortedLayer {
    pub entries: Vec<SortedItem>,
    pub counter: u32,
    first_slot: u32,
    remaining: u32,
}

impl SortedLayer {
    /// Skip exactly a prefix of the real dispatches. The loop itself will
    /// advance ESI by one entry and decrement the counter on return.
    pub fn cursor_after(&self, count: usize) -> Option<(u32, u32)> {
        let last = self.entries.get(count.checked_sub(1)?)?.slot;
        let distance = last.checked_sub(self.first_slot)? / ITEM_SIZE;
        Some((last, self.remaining.checked_sub(distance)?))
    }

    /// Consume the confirmed dispatches atomically: the only fallible
    /// guest write precedes both the register update and GPU commit.
    pub fn consume(&self, count: usize, esi: &mut u32, store: impl FnOnce(u32, u32) -> bool) -> bool {
        let Some((last, remaining)) = self.cursor_after(count) else { return false };
        if *esi != self.first_slot || !store(self.counter, remaining) { return false; }
        *esi = last;
        true
    }
}

pub const HANDLER_RETURN: u32 = 0x0003_bc90;

/// At the geometry handler called from the retail layer entry loop.
/// ESI is the slot, and the loop's saved count is at handler ESP + 0x28.
/// Earlier excluded slots are allowed. An earlier actual draw has already
/// claimed this layer and prevents a whole-light takeover.
pub fn sorted(mem: &dyn GuestRead, list: u32, layer: u32, esp: u32, esi: u32) -> Result<SortedLayer, &'static str> {
    let unread = "unreadable sorted light list";
    let word = |p: u32, n: u32| mem.read_u32(p.checked_add(n)?);
    if word(esp, 0) != Some(HANDLER_RETURN) || word(esp, 28) != Some(esi)
        || word(esp, 16) != Some(layer) || !matches!(layer, 12 | 13) {
        return Err("outside the sorted light geometry call");
    }
    let counter = esp.checked_add(40).ok_or(unread)?;
    let base = counter.checked_add(4).ok_or(unread)?;
    let before = esi.checked_sub(base).ok_or(unread)?;
    let remaining = word(counter, 0).ok_or(unread)?;
    let count = (before / ITEM_SIZE).checked_add(remaining).ok_or(unread)?;
    if before % ITEM_SIZE != 0 || remaining == 0 || count > 2560 { return Err("invalid sorted light count"); }
    let first = word(list, 12).ok_or(unread)?;
    let end = word(list, 4).ok_or(unread)?;
    let table = word(list, 20).filter(|&p| p != 0).ok_or(unread)?;
    if first > end || end > word(list, 0).ok_or(unread)? { return Err("invalid gathered light bounds"); }
    let mut entries = Vec::new();
    for n in 0..count {
        let slot = base.checked_add(n * ITEM_SIZE).ok_or(unread)?;
        let target = word(slot, 16).filter(|&p| p != 0).ok_or(unread)?;
        if word(target, 0x100).ok_or(unread)? == u32::MAX { continue; }
        if slot < esi { return Err("a preceding sorted light handler already ran"); }
        let index = word(slot, 0).ok_or(unread)? & 0xffff;
        if !(first..end).contains(&index) { return Err("sorted entry outside the gathered interval"); }
        let item = index.checked_mul(ITEM_SIZE).and_then(|n| table.checked_add(n)).ok_or(unread)?;
        item.checked_add(ITEM_SIZE - 1).ok_or(unread)?;
        let handler = word(item, 4).filter(|&p| p != 0).ok_or(unread)?;
        let shader = word(item, 8).ok_or(unread)?;
        word(item, 12).ok_or(unread)?;
        let selector = word(item, 20).ok_or(unread)?;
        let mask = 1 << layer;
        if word(item, 16).ok_or(unread)? & mask == 0 { return Err("sorted light layer mask changed"); }
        let geometry = if handler == GEOMETRY_DRAW
            && (selector >> 8) as u8 == 0
            && skip::is_structure_item(mem, item).ok_or(unread)? {
            Some(Item { part: skip::structure_part(mem, item).ok_or(unread)?, shader, layers: mask })
        } else { None };
        entries.push(SortedItem { slot, handler_item: item, geometry, implementation: word(slot, 4).ok_or(unread)? as u8 });
    }
    if entries.first().map(|e| e.slot) != Some(esi) { return Err("current sorted light entry was excluded"); }
    Ok(SortedLayer { entries, first_slot: esi, remaining, counter })
}

/// Take a light only when the entire set of relevant sorted entries is
/// available before any replacement. The other layer has its own build.
pub fn single_layer(or: u32, flags: u32, definition: u32) -> Result<u32, &'static str> {
    if flags & 4 != 0 || (or & (1 << 11) != 0 && (flags & 2 != 0 || definition & 32 != 0))
        || or & (1 << 14) != 0 {
        return Err("light has shadow, stencil or layer 14 work");
    }
    const DIFFUSE: u32 = 1 << 12;
    const SPECULAR: u32 = 1 << 13;
    match or & (DIFFUSE | if flags & 3 == 3 { SPECULAR } else { 0 }) {
        DIFFUSE => Ok(12),
        SPECULAR => Ok(13),
        0 => Err("no diffuse or specular light entries"),
        _ => Err("both sorted light layers are required"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::tests::Mem;
    #[test]
    fn cursor_consumes_sorted_duplicates_and_skips_excluded_slots() {
        let mut m = Mem::default();
        let (esp, list, table, target) = (0x1000, 0x2000, 0x3000, 0x4000);
        let base = esp + 44;
        for (at, v) in [(esp, 0x3bc90), (esp+16, 12), (esp+28, base), (esp+40, 4),
            (list, 5), (list+4, 5), (list+12, 1), (list+20, table), (target+256, 1), (target+512, u32::MAX)] {
            m.w(at, v);
        }
        for (i, id) in [4, 3, 4, 1].into_iter().enumerate() {
            let slot = base + i as u32 * ITEM_SIZE;
            m.w(slot, id);
            m.w(slot+4, 2);
            m.w(slot+16, if i==1 { target+256 } else { target });
            let item = table + id*ITEM_SIZE;
            m.w(item+4, 123);
            m.w(item+12, 0);
            m.w(item+20, 0);
            m.w(item+8, 7);
            m.w(item+16, 1<<12);
        }
        let sorted = sorted(&m, list, 12, esp, base).unwrap();
        assert_eq!(sorted.entries.iter().map(|s| s.handler_item).collect::<Vec<_>>(),
            [table+4*ITEM_SIZE, table+4*ITEM_SIZE, table+ITEM_SIZE]);
        assert_eq!(sorted.cursor_after(2), Some((base+2*ITEM_SIZE, 2)));
        assert_eq!(sorted.cursor_after(4), None);
        m.0.remove(&(base+3*ITEM_SIZE+16));
        assert!(super::sorted(&m, list, 12, esp, base).is_err());
    }
    #[test]
    fn second_active_layer_prevents_early_ownership() {
        assert_eq!(single_layer(3 << 12, 1, 0), Ok(12));
        assert!(single_layer(3 << 12, 3, 0).is_err());
        assert_eq!(single_layer(1 << 13, 3, 0), Ok(13));
    }
}
