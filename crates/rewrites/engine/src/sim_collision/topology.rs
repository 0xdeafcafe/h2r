//! A version for the active structure's portal graph. One physical write
//! subscription covers every span the decode read and the projection
//! table, so an edit through any guest mapping retires the version.
use super::layout;
use runtime::{Guest, WatchId};
use sim_collision::{TopologyKey, structure::TableVersion};
use std::sync::Mutex;

const OWNER: &str = "collision-portal-topology";

struct Subscription {
    epoch: (u64, u64),
    key: TopologyKey,
    id: WatchId,
    version: TableVersion,
}

#[derive(Default)]
struct TopologyWatch {
    active: Option<Subscription>,
}

impl TopologyWatch {
    fn version(
        &mut self,
        g: &mut Guest,
        key: TopologyKey,
        sources: Option<&[(u32, u32)]>,
    ) -> Option<TableVersion> {
        // A bulk tag read must never introduce a debugger memory trap.
        if g.has_trap() {
            return None;
        }
        let epoch = g.write_watch_epoch();
        let Some(sources) = sources else {
            let s = self.active.as_ref()?;
            return (s.epoch == epoch && s.key == key && !g.has_dirty(s.id)).then_some(s.version);
        };
        // Watch IDs belong to this Guest; retire every earlier subscription
        // by its owner, including one left in another layout.
        self.active = None;
        for (id, owner) in g.write_watch_owners() {
            if owner == OWNER {
                g.unsubscribe_writes(id);
            }
        }
        let id = g.subscribe_writes(OWNER);
        let projection = [(layout::PROJECTION, 36)];
        for &(at, len) in sources.iter().chain(&projection) {
            if len != 0 && g.watch_virtual_read(id, at, len as usize).is_err() {
                g.unsubscribe_writes(id);
                return None;
            }
        }
        let version = TableVersion::fresh();
        self.active = Some(Subscription {
            epoch,
            key,
            id,
            version,
        });
        Some(version)
    }
}

/// The Guest guard excludes a write between the decode and the watch.
pub(super) fn version(
    g: &mut Guest,
    key: TopologyKey,
    sources: Option<&[(u32, u32)]>,
) -> Option<TableVersion> {
    static WATCH: Mutex<TopologyWatch> = Mutex::new(TopologyWatch { active: None });
    WATCH
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .version(g, key, sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime::guest::Permissions;

    #[test]
    fn an_aliased_edit_or_a_remap_retires_the_graph_version() {
        const BASE: u32 = 0x10000;
        const ALIAS: u32 = 0x20000;
        let key = TopologyKey {
            structure: BASE,
            bsp: BASE + 0x100,
        };
        let mut g = Guest::new();
        let mut w = TopologyWatch::default();
        g.map(BASE, 0x1000, 0, Permissions::RW).unwrap();
        g.map(ALIAS, 0x1000, 0, Permissions::RW).unwrap();
        g.map(layout::PROJECTION, 36, 0x1000, Permissions::RW)
            .unwrap();
        assert_eq!(w.version(&mut g, key, None), None, "nothing armed");
        let sources = [(BASE + 0x18, 4), (BASE + 0x200, 36)];
        let first = w.version(&mut g, key, Some(&sources)).unwrap();
        assert_eq!(w.version(&mut g, key, None), Some(first));
        let other = TopologyKey { bsp: BASE, ..key };
        assert_eq!(w.version(&mut g, other, None), None, "another structure");
        g.set_u32(ALIAS + 0x204, 7).unwrap();
        assert_eq!(w.version(&mut g, key, None), None, "an aliased write");
        let second = w.version(&mut g, key, Some(&sources)).unwrap();
        assert_ne!(first, second);
        g.set_u16(layout::PROJECTION, 1).unwrap();
        assert_eq!(w.version(&mut g, key, None), None, "the projection table");
        let third = w.version(&mut g, key, Some(&sources)).unwrap();
        g.unmap(BASE, 0x1000).unwrap();
        assert_eq!(w.version(&mut g, key, None), None, "a remap");
        assert_eq!(w.version(&mut g, key, Some(&sources)), None, "unreadable");
        assert_ne!(Some(third), w.version(&mut g, key, None));
        assert!(
            g.write_watch_owners()
                .iter()
                .filter(|(_, owner)| *owner == OWNER)
                .count()
                <= 1
        );
    }
}
