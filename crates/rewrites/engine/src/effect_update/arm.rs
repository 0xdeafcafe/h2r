//! The per-effect bodies' counters (docs/engine/effects-parts.md, "The
//! per-effect bodies"). The Rust bodies of [`super::TABLE`] answer every
//! segment; nothing is handed to the guest. Each entry counts the
//! segments it answered and, of those, the ones that ran outside the
//! float environment the bodies assume (`float_env`, a named error the
//! counters carry; the body still answers).

use super::TABLE;
use crate::sim_objects::{Body, Result};
use runtime::{CpuAbi, Guest, TransferAbi};
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

/// One entry's counters since the map loaded.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    /// Segments the Rust body answered.
    pub answered: u64,
    /// Segments entered outside the float environment the bodies assume.
    pub float_env: u64,
}

static STATE: LazyLock<Mutex<BTreeMap<u32, Stats>>> = LazyLock::new(Mutex::default);

fn state() -> std::sync::MutexGuard<'static, BTreeMap<u32, Stats>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The entry whose body `pc` enters or resumes.
pub fn entry_of(pc: u32) -> Option<u32> {
    TABLE.at(pc).map(|(body, _)| body.point.base)
}

fn count(pc: u32, f: impl FnOnce(&mut Stats)) {
    if let Some(entry) = entry_of(pc) {
        f(state().entry(entry).or_default());
    }
}

/// Whether a fused transfer to `pc` stays in Rust: any body's segment.
pub fn admit(pc: u32) -> bool {
    entry_of(pc).is_some()
}

/// The segment at `pc` was entered outside the bodies' float environment.
pub fn float_env(pc: u32) {
    count(pc, |s| s.float_env += 1);
}

/// Forget the map's counters.
pub fn reset() {
    state().clear();
}

/// Every entry's counters, in entry address order.
pub fn stats() -> Vec<(u32, Stats)> {
    state().iter().map(|(&k, v)| (k, v.clone())).collect()
}

/// A body's segment: counted as answered.
pub fn answer_segment(body: &Body, cpu: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    let transfer = body.segment(cpu, g, pc)?;
    count(pc, |s| s.answered += 1);
    Ok(transfer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_body_segment_is_admitted_and_others_are_not() {
        for body in TABLE.bodies() {
            assert!(admit(body.point.base), "{}", body.point.name);
            for &site in body.sites {
                assert!(admit(site), "{:#x}", site);
            }
        }
        assert!(!admit(0x0010_0000));
    }
}
