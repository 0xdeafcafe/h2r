//! A version for the active structure leaf table. Dirty bits refer to
//! physical pages, including stores through another virtual alias.
use runtime::{Guest, WatchId, guest::Access};
use sim_collision::structure::{LeafTable, TableVersion};
use std::sync::Mutex;

const OWNER: &str = "collision-structure-leaves";

struct Subscription {
    epoch: (u64, u64),
    addr: u32,
    len: u32,
    id: WatchId,
    version: TableVersion,
}

#[derive(Default)]
struct TableWatch {
    active: Option<Subscription>,
}

impl TableWatch {
    fn version(&mut self, g: &mut Guest, addr: u32, len: u32) -> Option<TableVersion> {
        let epoch = g.write_watch_epoch();
        if let Some(s) = &mut self.active
            && (s.epoch, s.addr, s.len) == (epoch, addr, len)
        {
            if g.has_dirty(s.id) {
                g.take_dirty(s.id).ok()?;
                s.version = TableVersion::fresh();
            }
            return Some(s.version);
        }
        // Layout identities are unique across Guests. Subscription IDs
        // are local, so retire our previous subscription in this Guest
        // by its owner when switching layouts or executor views.
        self.active = None;
        for (id, owner) in g.write_watch_owners() {
            if owner == OWNER {
                g.unsubscribe_writes(id);
            }
        }
        g.check(addr, len as usize, Access::Read, 1).ok()?;
        let id = g.subscribe_writes(OWNER);
        if g.watch_virtual_read(id, addr, len as usize).is_err() {
            g.unsubscribe_writes(id);
            return None;
        }
        let version = TableVersion::fresh();
        self.active = Some(Subscription {
            epoch,
            addr,
            len,
            id,
            version,
        });
        Some(version)
    }
}

pub(super) fn table(g: &mut Guest, count_addr: u32, leaves: u32, leaf: u32) -> Option<LeafTable> {
    // A bulk tag read must never introduce a debugger memory trap.
    if g.has_trap() {
        return None;
    }
    let count = g.u32(count_addr).ok()?;
    LeafTable::witnessed(leaves, count, leaf, |len| {
        static WATCH: Mutex<TableWatch> = Mutex::new(TableWatch { active: None });
        WATCH
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .version(g, leaves, len)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    #[test]
    fn cluster_watch_follows_physical_aliases_and_remaps() {
        let mut g = Guest::new();
        let mut w = TableWatch::default();
        const BASE: u32 = 0x10000;
        const ALIAS: u32 = 0x20000;
        g.map(BASE, 0x1000, 0, Permissions::RW).unwrap();
        g.map(ALIAS, 0x1000, 0, Permissions::RW).unwrap();
        let first = w.version(&mut g, BASE, 24).unwrap();
        assert_eq!(w.version(&mut g, BASE, 24), Some(first));
        g.set_u16(ALIAS + 8, 43).unwrap();
        let edited = w.version(&mut g, BASE, 24).unwrap();
        assert_ne!(first, edited);
        assert_eq!(w.version(&mut g, BASE, 24), Some(edited));
        g.unmap(BASE, 0x1000).unwrap();
        assert!(w.version(&mut g, BASE, 24).is_none());
        g.map(BASE, 0x1000, 0x1000, Permissions::RW).unwrap();
        assert_ne!(w.version(&mut g, BASE, 24).unwrap(), edited);
        assert_eq!(g.write_watch_owners().len(), 1);
    }
}
