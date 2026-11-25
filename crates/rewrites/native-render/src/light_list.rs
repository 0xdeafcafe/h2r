//! Read the private, sorted dispatch records of the light's current layer.

use crate::camera::GuestRead;
use crate::skip::ITEM_SIZE;
use scene_extract::light_groups::Sorted;

/// The entry loop sorts its private 0x18-byte records after the build
/// callbacks. Only this list includes the build refusals, sort tie order,
/// chosen implementation and the dispatcher's geometry exclusion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortedItem {
    pub slot: u32,
    pub handler_item: u32,
    sorted: Sorted,
}

impl SortedItem {
    /// The standard handler's stream, pose, binding and guest index address.
    pub fn scene_entry(
        &self,
        mem: &dyn GuestRead,
    ) -> Result<(frame_scene::LightEntry, Vec<frame_scene::Row>, u32), &'static str> {
        use scene_extract::light_groups::{Cause, dispatched_entry};
        dispatched_entry(mem, self.handler_item, self.sorted).map_err(|cause| match cause {
            Cause::Memory => "unreadable gathered light entry",
            Cause::Callback => "nonstandard gathered light geometry callback",
            Cause::Owner => "gathered light render record has no live owner",
            Cause::Section => "gathered light section or part unavailable",
            Cause::Range => "gathered light index span outside its section",
            Cause::State => "gathered light handler toggles per-part state",
            Cause::Subparts => "gathered light handler selects visible subparts",
            Cause::Material => "gathered light shader needs handler state or is unreadable",
            Cause::Pose => "gathered light model node palette unavailable",
        })
    }
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
    sorted_at(mem, list, layer, esp, esi, HANDLER_RETURN)
}

/// The same ABI when the layer-loop port pushed its authenticated continuation token.
pub fn sorted_at(mem: &dyn GuestRead, list: u32, layer: u32, esp: u32, esi: u32, caller: u32) -> Result<SortedLayer, &'static str> {
    SortedCall { list, layer, esp, esi, caller }.read(mem, |_| Ok(()))
}

/// The current handler's sorted list, before the host walks its entries.
pub struct SortedCall {
    pub list: u32,
    pub layer: u32,
    pub esp: u32,
    pub esi: u32,
    pub caller: u32,
}

impl SortedCall {
    /// Check residency as each entry is decoded so a refused entry can
    /// leave the rest of the list unread. No guest state is written.
    pub fn read(
        &self,
        mem: &dyn GuestRead,
        mut check: impl FnMut(&SortedItem) -> Result<(), &'static str>,
    ) -> Result<SortedLayer, &'static str> {
        let Self { list, layer, esp, esi, caller } = *self;
        let unread = "unreadable sorted light list";
        let word = |p: u32, n: u32| mem.read_u32(p.checked_add(n)?);
        if word(esp, 0) != Some(caller) || word(esp, 28) != Some(esi)
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
            let sorted = Sorted::read(mem, slot).map_err(|_| unread)?;
            let index = u32::from(sorted.index);
            if !(first..end).contains(&index) { return Err("sorted entry outside the gathered interval"); }
            let item = index.checked_mul(ITEM_SIZE).and_then(|n| table.checked_add(n)).ok_or(unread)?;
            item.checked_add(ITEM_SIZE - 1).ok_or(unread)?;
            word(item, 4).filter(|&p| p != 0).ok_or(unread)?;
            if word(item, 16).ok_or(unread)? & (1 << layer) == 0 { return Err("sorted light layer mask changed"); }
            let entry = SortedItem { slot, handler_item: item, sorted };
            check(&entry)?;
            entries.push(entry);
        }
        if entries.first().map(|e| e.slot) != Some(esi) { return Err("current sorted light entry was excluded"); }
        Ok(SortedLayer { entries, first_slot: esi, remaining, counter })
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
            m.w(slot+8, 1);
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

        let call = SortedCall { list, layer: 12, esp, esi: base, caller: HANDLER_RETURN };
        let mut checked = 0;
        let result = call.read(&m, |entry| {
            checked += 1;
            assert_eq!(entry.slot, base);
            Err("no resident geometry")
        });
        assert_eq!(result.unwrap_err(), "no resident geometry");
        assert_eq!(checked, 1, "the unreadable suffix is never inspected");
    }
}
