//! A trial run on the live Guest: Rust code stores as it likes, then
//! every byte goes back (docs/runtime/second-executor.md, "Trial stores").
//!
//! While a trial is on, [`Guest::write_bytes_from`] saves the bytes under
//! each store before it lands, on the fast path and the slow one, and
//! tells neither the write watch nor an HLE capture: when the trial ends
//! the bytes are as they were, so there is nothing to report. Only stores
//! through `Guest` are seen; generated code must not run inside a trial
//! (its inline stores bypass this path). Mapping changes are not undone.

use super::Guest;

/// One store's place and the bytes it replaced.
struct Saved {
    /// Physical offset in RAM.
    at: usize,
    /// Virtual address it was made through.
    addr: u32,
    /// Index of its old bytes in [`Trial::old`].
    from: usize,
    len: usize,
}

#[derive(Default)]
pub(crate) struct Trial {
    saved: Vec<Saved>,
    old: Vec<u8>,
}

impl Trial {
    #[inline]
    pub(super) fn save(&mut self, at: usize, addr: u32, bytes: &[u8]) {
        self.saved.push(Saved { at, addr, from: self.old.len(), len: bytes.len() });
        self.old.extend_from_slice(bytes);
    }
}

/// What a trial stored: each byte's final value, by the virtual address
/// it was stored through, in address order. A byte stored twice appears
/// once, with its last value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrialStores {
    pub bytes: Vec<(u32, u8)>,
}

impl Guest {
    /// Run `f` on this Guest, then put back every byte it stored through
    /// `Guest` (module docs), on every path out, a panic included. Returns
    /// `f`'s result and the stores' final bytes. Nested trials are not
    /// supported: an inner call runs `f` inside the outer trial.
    pub fn trial<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> (T, TrialStores) {
        if self.trial.is_some() {
            let value = f(self);
            return (value, TrialStores::default());
        }
        self.invalidate_hle_capture();
        self.trial = Some(Box::default());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self)));
        let trial = self.trial.take().expect("the trial is still on");
        let mut finals = std::collections::BTreeMap::new();
        for s in &trial.saved {
            for i in 0..s.len {
                finals.insert(s.addr.wrapping_add(i as u32), self.ram[s.at + i]);
            }
        }
        // Newest first, so a byte stored twice gets its first old value.
        for s in trial.saved.iter().rev() {
            self.ram[s.at..s.at + s.len].copy_from_slice(&trial.old[s.from..s.from + s.len]);
        }
        let value = result.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        (value, TrialStores { bytes: finals.into_iter().collect() })
    }

    /// Whether a [`trial`](Self::trial) is on.
    pub fn in_trial(&self) -> bool {
        self.trial.is_some()
    }
}

#[cfg(test)]
mod tests {
    use crate::guest::{Guest, Permissions};

    fn guest() -> Guest {
        let mut g = Guest::new();
        g.map_zeroed(0x10000, 0x3000, Permissions::RW).unwrap();
        g
    }

    #[test]
    fn every_store_goes_back_and_the_final_bytes_are_reported() {
        let mut g = guest();
        g.set_u32(0x10000, 0x1122_3344).unwrap();
        let ((), stores) = g.trial(|g| {
            g.set_u32(0x10000, 0xaabb_ccdd).unwrap();
            g.set_u8(0x10001, 0x55).unwrap();
            // Across a page boundary: the slow path.
            g.write_bytes(0x10ffe, &[1, 2, 3, 4], 1).unwrap();
            assert_eq!(g.u32(0x10000).unwrap(), 0xaabb_55dd);
        });
        assert_eq!(g.u32(0x10000).unwrap(), 0x1122_3344);
        assert_eq!(g.u32(0x10ffe).unwrap(), 0);
        assert_eq!(
            stores.bytes,
            [(0x10000, 0xdd), (0x10001, 0x55), (0x10002, 0xbb), (0x10003, 0xaa),
             (0x10ffe, 1), (0x10fff, 2), (0x11000, 3), (0x11001, 4)]
        );
        assert!(!g.in_trial());
    }

    #[test]
    fn the_write_watch_sees_nothing_of_a_trial() {
        const PA: u32 = 0x0004_0000;
        let mut g = Guest::new();
        g.map(0x30000, 0x1000, PA, Permissions::RW).unwrap();
        let id = g.subscribe_writes("trial test");
        g.watch_writes(id, crate::watch::CONTIGUOUS_BASE | PA, 0x1000).unwrap();
        let ((), _) = g.trial(|g| g.set_u32(0x30010, 7).unwrap());
        assert!(!g.has_dirty(id));
        assert_eq!(g.u32(0x30010).unwrap(), 0);
        g.set_u32(0x30010, 7).unwrap();
        assert!(g.has_dirty(id));
    }

    #[test]
    fn a_faulting_store_changes_nothing_and_a_panic_still_undoes() {
        let mut g = guest();
        let (r, stores) = g.trial(|g| {
            g.set_u32(0x10020, 9).unwrap();
            g.set_u32(0x20000, 1)
        });
        assert!(r.is_err());
        assert_eq!(stores.bytes.len(), 4);
        assert_eq!(g.u32(0x10020).unwrap(), 0);
        let mut g = std::panic::AssertUnwindSafe(g);
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            g.trial(|g| {
                g.set_u32(0x10030, 5).unwrap();
                panic!("inside");
            })
        }));
        assert!(caught.is_err());
        assert_eq!(g.u32(0x10030).unwrap(), 0);
        assert!(!g.in_trial());
    }
}
