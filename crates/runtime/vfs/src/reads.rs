//! What the guest read, by path: the report's `wall.reads`
//! (docs/runtime/sim.md §6). A small map keyed by the object's path, bumped
//! on each read the VFS serves, so a run says which files its bytes came from
//! and why a map read was copied rather than mapped.

use std::collections::{BTreeMap, HashMap};

/// Paths kept. A title opens hundreds; the bound stops a runaway.
const MAX_PATHS: usize = 4096;

/// One path's reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PathReads {
    /// Bytes supplied as private file mappings.
    pub mapped_bytes: u64,
    /// Bytes copied into guest RAM.
    pub copied_bytes: u64,
    /// Reads served: one per mapped read, one per backend read on the copy
    /// path (a bounded-chunk transfer is several).
    pub reads: u64,
    /// Why the first mapped read of the path was refused, if one was
    /// ([`crate::Vfs::try_map_read`]).
    pub refusal: Option<&'static str>,
}

#[derive(Default)]
pub(crate) struct ReadLog {
    by_path: HashMap<String, PathReads>,
}

impl ReadLog {
    fn entry(&mut self, path: &str) -> Option<&mut PathReads> {
        if !self.by_path.contains_key(path) {
            if self.by_path.len() >= MAX_PATHS {
                return None;
            }
            self.by_path.insert(path.to_owned(), PathReads::default());
        }
        self.by_path.get_mut(path)
    }

    /// A mapped read of `len` bytes, `mapped` of them mapped, the rest copied.
    pub(crate) fn mapped(&mut self, path: &str, len: usize, mapped: usize) {
        if let Some(e) = self.entry(path) {
            e.reads += 1;
            e.mapped_bytes += mapped as u64;
            e.copied_bytes += len.saturating_sub(mapped) as u64;
        }
    }

    pub(crate) fn copied(&mut self, path: &str, bytes: usize) {
        if let Some(e) = self.entry(path) {
            e.reads += 1;
            e.copied_bytes += bytes as u64;
        }
    }

    /// The first refusal for a path wins.
    pub(crate) fn refused(&mut self, path: &str, reason: &'static str) {
        if let Some(e) = self.entry(path) {
            e.refusal.get_or_insert(reason);
        }
    }

    pub(crate) fn snapshot(&self) -> BTreeMap<String, PathReads> {
        self.by_path.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_totals_its_reads_and_keeps_its_first_refusal() {
        let mut log = ReadLog::default();
        log.refused("a.map", "short read");
        log.refused("a.map", "writable handle");
        log.copied("a.map", 100);
        log.mapped("a.map", 0x20000, 0x1f000);
        log.copied("b.bin", 7);
        let all = log.snapshot();
        let a = &all["a.map"];
        assert_eq!(
            (a.reads, a.mapped_bytes, a.copied_bytes, a.refusal),
            (2, 0x1f000, 100 + 0x1000, Some("short read"))
        );
        assert_eq!(all["b.bin"], PathReads { copied_bytes: 7, reads: 1, ..Default::default() });
    }

    #[test]
    fn the_map_is_bounded() {
        let mut log = ReadLog::default();
        for i in 0..MAX_PATHS + 10 {
            log.copied(&format!("f{i}"), 1);
        }
        assert_eq!(log.snapshot().len(), MAX_PATHS);
    }
}
