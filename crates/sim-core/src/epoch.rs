//! The derived-state invalidation counter (docs/engine/sim-rewrite.md
//! section 2, "Who owns state"). Hooks bump it when guest memory changes
//! under Rust's feet: map load, checkpoint revert `0x123f60`, full restore
//! `0x124380` and snapshot load. A store of derived or static state reads
//! it on each use and drops everything when it moved. Derived state never
//! changes a result, so a spurious bump only costs time.
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Epoch(AtomicU64);

impl Epoch {
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Acquire)
    }

    /// Invalidate everything keyed to the previous value.
    pub fn bump(&self) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

impl Default for Epoch {
    fn default() -> Self {
        Self::new()
    }
}

/// The process-wide epoch for guest-derived simulation state.
pub static SIM: Epoch = Epoch::new();

#[cfg(test)]
mod tests {
    #[test]
    fn bump_moves_the_counter() {
        let e = super::Epoch::new();
        let before = e.get();
        e.bump();
        assert_ne!(e.get(), before);
    }
}
