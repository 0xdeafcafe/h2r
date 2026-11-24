//! Index the first word of each record in a named graph block. A write
//! through any alias invalidates the tables sharing this subscription.
use super::Bus;
use runtime::{Guest, WatchId};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

const OWNER: &str = "animation-graph-names";
const MAX_BLOCKS: usize = 128;
const MAX_RECORDS: u32 = 16 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Block {
    records: u32,
    count: u32,
    stride: u32,
}

impl Block {
    fn span(self) -> Option<usize> {
        if self.records == 0 || !(1..=MAX_RECORDS).contains(&self.count) || self.stride < 4 {
            return None;
        }
        let end = (self.count - 1).checked_mul(self.stride)?.checked_add(4)?;
        self.records.checked_add(end - 1)?;
        Some(end as usize)
    }
}

/// Blocks with repeated or unordered IDs retain the guest's binary
/// search. On strictly increasing blocks each reachable hit is unique.
#[derive(Default)]
struct Cache {
    sim_epoch: u64,
    watch: Option<((u64, u64), WatchId)>,
    tables: HashMap<Block, Option<HashMap<u32, u32>>>,
    records: u32,
}

impl Cache {
    fn find(&mut self, ram: &mut Guest, block: Block, len: usize, key: u32) -> Option<u32> {
        let epoch = ram.write_watch_epoch();
        let sim_epoch = sim_core::epoch::SIM.get();
        let full = !self.tables.contains_key(&block)
            && (self.tables.len() >= MAX_BLOCKS || self.records + block.count > MAX_RECORDS);
        if full
            || sim_epoch != self.sim_epoch
            || self
                .watch
                .is_none_or(|(old, id)| old != epoch || ram.has_dirty(id))
        {
            // Watch IDs belong to a Guest. On a change of view, retire this
            // owner's subscription in the incoming Guest only. A fresh epoch
            // always discards all lookup tables.
            for (id, owner) in ram.write_watch_owners() {
                if owner == OWNER {
                    ram.unsubscribe_writes(id);
                }
            }
            self.sim_epoch = sim_epoch;
            self.watch = Some((epoch, ram.subscribe_writes(OWNER)));
            self.tables.clear();
            self.records = 0;
        }
        if let std::collections::hash_map::Entry::Vacant(slot) = self.tables.entry(block) {
            let (_, id) = self.watch?;
            ram.watch_virtual_read(id, block.records, len).ok()?;
            let mut names = HashMap::with_capacity(block.count as usize);
            let mut previous = None;
            for index in 0..block.count {
                let name = ram.u32(block.records + index * block.stride).ok()?;
                if previous.is_some_and(|v| v >= name as i32) {
                    break;
                }
                previous = Some(name as i32);
                names.insert(name, index);
            }
            self.records += block.count;
            slot.insert((names.len() == block.count as usize).then_some(names));
        }
        self.tables.get(&block)?.as_ref()?.get(&key).copied()
    }
}

pub(super) fn find(
    plan: &mut Bus<'_>,
    records: u32,
    count: u32,
    stride: u32,
    key: u32,
) -> Option<u32> {
    let block = Block {
        records,
        count,
        stride,
    };
    let span = block.span()?;
    // Never admit tag bytes supplied by the uncommitted stack journal.
    // The Guest guard excludes concurrent writes while checking/building.
    let ram = plan.unstaged_read(records, span)?;
    static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .find(ram, block, span, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    #[test]
    fn tables_follow_aliased_edits_and_guest_replacement() {
        let block = Block {
            records: 0x10000,
            count: 3,
            stride: 8,
        };
        let len = block.span().unwrap();
        let mut cache = Cache::default();
        let fixture = || {
            let mut g = Guest::new();
            for addr in [block.records, 0x20000] {
                g.map(addr, 0x1000, 0, Permissions::RW).unwrap();
            }
            for (i, value) in [3, 6, 9].into_iter().enumerate() {
                g.set_u32(block.records + i as u32 * block.stride, value)
                    .unwrap();
            }
            g
        };
        let mut a = fixture();
        let mut b = fixture();
        assert_eq!(cache.find(&mut a, block, len, 6), Some(1));
        a.set_u32(0x20008, 5).unwrap();
        assert_eq!(cache.find(&mut a, block, len, 6), None);
        assert_eq!(cache.find(&mut a, block, len, 5), Some(1));
        assert_eq!(cache.find(&mut b, block, len, 6), Some(1));
        assert_eq!(cache.find(&mut a, block, len, 6), None);
        assert_eq!(a.write_watch_owners().len(), 1);
        a.set_u32(0x20008, 3).unwrap();
        assert_eq!(cache.find(&mut a, block, len, 3), None, "duplicate name");
        a.unmap(block.records, 0x1000).unwrap();
        a.map(block.records, 0x1000, 0x4000, Permissions::RW)
            .unwrap();
        for (i, value) in [11, 12, 13].into_iter().enumerate() {
            a.set_u32(block.records + i as u32 * block.stride, value)
                .unwrap();
        }
        assert_eq!(cache.find(&mut a, block, len, 12), Some(1));
    }

    #[test]
    fn a_staged_stack_alias_bypasses_the_warm_table() {
        use crate::sim_anim::manager;
        use runtime::CpuAbi;

        let mut g = Guest::new();
        let text = 0x1dd560;
        g.map(text, 0x63, 0x3000, Permissions::RX).unwrap();
        for va in [0x10000, 0x20000] {
            g.map(va, 0x1000, 0, Permissions::RW).unwrap();
        }
        g.map(0x30000, 0x1000, 0x4000, Permissions::RW).unwrap();
        let header = 0x10100;
        let records = 0x10080;
        g.set_u32(header, 3).unwrap();
        g.set_u32(header + 4, records).unwrap();
        g.set_u32(records, 3).unwrap();
        g.set_u32(records + 8, 6).unwrap();

        // The second stack aliases the names. Its first argument also
        // initializes the third record before the initial index build.
        let warm = 0x30100;
        let alias = 0x2008c;
        for sp in [warm, alias] {
            for (offset, word) in [(0, 0x40000), (4, header), (8, 6), (12, 8)] {
                g.set_u32(sp + offset, word).unwrap();
            }
        }
        for (sp, expected) in [(warm, records + 8), (alias, 0)] {
            let mut cpu = CpuAbi {
                esp: sp,
                ebx: 17,
                esi: 2,
                ..Default::default()
            };
            assert!(manager::run(&mut cpu, &mut g, text).unwrap().is_some());
            assert_eq!(cpu.eax, expected);
        }
        // The pushes of EBX and ESI replace the first two keys.
        assert_eq!(g.u32(records).unwrap(), 2);
        assert_eq!(g.u32(records + 8).unwrap(), 17);
    }
}
