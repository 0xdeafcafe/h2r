//! The active structure's leaf-to-cluster table. A source version identifies
//! the contents of an entire readable table under a single guest memory guard.
use crate::{Bail, GuestRead, bsp::MAX_COUNT};
use std::sync::atomic::{AtomicU64, Ordering};

/// A producer keeps this token only while the table has the same bytes.
/// Different guest memories and unreported RAM replacements need distinct tokens.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableVersion(u64);

impl TableVersion {
    pub fn fresh() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeafTable {
    addr: u32,
    len: u32,
    version: TableVersion,
}

impl LeafTable {
    pub(crate) fn footprint(self) -> u64 {
        u64::from(self.len / 8) * std::mem::size_of::<i16>() as u64
    }

    /// Only cache a complete bounded guest span that contains the requested
    /// leaf. The caller uses the ordinary two-byte read if this cannot be proved.
    pub fn witnessed(
        addr: u32,
        count: u32,
        leaf: u32,
        version: impl FnOnce(u32) -> Option<TableVersion>,
    ) -> Option<Self> {
        if leaf >= count || count > MAX_COUNT {
            return None;
        }
        let len = count * 8;
        if u64::from(addr) + u64::from(len) > 1 << 32 {
            return None;
        }
        Some(Self {
            addr,
            len,
            version: version(len)?,
        })
    }
}

/// The structure data used by the location query. Indexes retain the
/// collision BSP's leaf numbering and signed cluster sentinel.
pub struct Structure {
    pub(crate) source: LeafTable,
    leaf_cluster: Box<[i16]>,
}

impl Structure {
    pub(crate) fn decode(g: &impl GuestRead, source: LeafTable) -> Result<Self, Bail> {
        let raw = g.bytes(source.addr, source.len as usize)?;
        let leaf_cluster = raw
            .chunks_exact(8)
            .map(|row| i16::from_le_bytes([row[0], row[1]]))
            .collect();
        Ok(Self {
            source,
            leaf_cluster,
        })
    }

    pub fn leaf_cluster(&self, leaf: u32) -> Option<i16> {
        self.leaf_cluster.get(leaf as usize).copied()
    }

    pub(crate) fn footprint(&self) -> usize {
        std::mem::size_of_val(&*self.leaf_cluster)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Image;
    use crate::{CollisionWorld, Fault};
    use sim_core::Epoch;
    use std::cell::Cell;

    struct Rows {
        image: Image,
        reads: Cell<usize>,
    }

    impl GuestRead for Rows {
        fn read(&self, addr: u32, dst: &mut [u8]) -> Result<(), Fault> {
            self.reads.set(self.reads.get() + 1);
            self.image.read(addr, dst)
        }
    }

    #[test]
    fn cached_clusters_redecode_after_a_write_or_epoch_change() {
        const ADDR: u32 = 0x2000;
        static EPOCH: Epoch = Epoch::new();
        let mut rows = Rows {
            image: Image::new(ADDR, 24),
            reads: Cell::new(0),
        };
        rows.image.put(ADDR, &42i16.to_le_bytes());
        rows.image.put(ADDR + 8, &(-1i16).to_le_bytes());
        rows.image.put(ADDR + 16, &6i16.to_le_bytes());
        let world = CollisionWorld::new(&EPOCH);
        let table = || LeafTable::witnessed(ADDR, 3, 0, |_| Some(TableVersion::fresh())).unwrap();
        let source = table();
        assert_eq!(world.leaf_cluster(&rows, source, 0), Some(42));
        assert_eq!(world.leaf_cluster(&rows, source, 1), Some(-1));
        assert_eq!(world.leaf_cluster(&rows, source, 2), Some(6));
        assert_eq!(rows.reads.get(), 1);
        assert_eq!(world.stats().bytes, 6);
        rows.image.put(ADDR + 8, &15i16.to_le_bytes());
        let edited = table();
        assert_eq!(world.leaf_cluster(&rows, edited, 1), Some(15));
        assert_eq!(rows.reads.get(), 2);
        EPOCH.bump();
        assert_eq!(world.leaf_cluster(&rows, edited, 1), Some(15));
        assert_eq!(rows.reads.get(), 3);
        assert_eq!(world.stats().bytes, 6);
        world.clear();
        assert_eq!(world.stats().bytes, 0);
    }

    #[test]
    fn budget_uses_the_live_word_and_releases_the_superseded_table() {
        static EPOCH: Epoch = Epoch::new();
        let rows = Rows {
            image: Image::new(0, 16),
            reads: Cell::new(0),
        };
        let world = CollisionWorld::with_budget(&EPOCH, 2);
        let table = |n| LeafTable::witnessed(0, n, 0, |_| Some(TableVersion::fresh())).unwrap();
        assert_eq!(world.leaf_cluster(&rows, table(1), 0), Some(0));
        assert_eq!(world.stats().bytes, 2);
        assert_eq!(world.leaf_cluster(&rows, table(2), 0), None);
        assert_eq!(rows.reads.get(), 1, "the oversized table was never read");
        let stats = world.stats();
        assert_eq!(stats.cluster_budget_reads, 1);
        assert_eq!(stats.bytes, 0, "the old source was released");
    }

    #[test]
    fn unbounded_or_untracked_leaf_tables_remain_live() {
        let unseen =
            |_: u32| -> Option<TableVersion> { panic!("unused range must not install a watch") };
        assert!(LeafTable::witnessed(0, 0, 0, unseen).is_none());
        assert!(LeafTable::witnessed(0, 1, 1, unseen).is_none());
        assert!(LeafTable::witnessed(0, MAX_COUNT + 1, 0, unseen).is_none());
        assert!(LeafTable::witnessed(u32::MAX - 6, 1, 0, unseen).is_none());
        assert!(LeafTable::witnessed(0, 1, 0, |_| None).is_none());
    }
}
