//! The add-point call `0x00302c70` as the game calls it
//! (docs/engine/havok-convex.md, "The add-point arm"): [`answer`] runs
//! [`manifold::add_point`] when the new point moves a slot it matches,
//! which makes no call out. A point that needs a new slot needs the
//! manager's add (`vt[+0x04]`, the contact commit with Halo's contact
//! callback), which stays the guest's ([`Fallback::Commit`]). On a
//! [`Fallback`] the journal is discarded and nothing has been written.
use crate::convex;
use crate::jacobian::Fpu;
use crate::manifold::{self, Added, PointArgs, PointCalls};
use crate::mem::Journal;
use std::sync::atomic::{AtomicU64, Ordering};

/// Why an add-point call runs the guest. Each is counted ([`fallbacks`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// A word the call reads is unmapped or unaligned, a vector is not
    /// 16-byte aligned, a store is not plain writable memory, or the count
    /// is past 256.
    Memory,
    /// No slot matches: the manager adds the contact.
    Commit,
    /// An unmasked float exception or an x87 stack not empty at the entry.
    Float,
    /// The guest answered an audited call whose return could not be
    /// observed, so it went uncompared.
    Unobserved,
}

impl Fallback {
    pub const ALL: [Fallback; 4] = [Self::Memory, Self::Commit, Self::Float, Self::Unobserved];

    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Commit => "commit",
            Self::Float => "float",
            Self::Unobserved => "unobserved",
        }
    }
}

static FALLBACKS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

/// Count a call sent to the guest.
pub fn count(reason: Fallback) {
    FALLBACKS[reason as usize].fetch_add(1, Ordering::Relaxed);
}

/// The add-point call's declines since start, by reason, in [`Fallback::ALL`] order.
pub fn fallbacks() -> [(&'static str, u64); 4] {
    Fallback::ALL.map(|f| (f.name(), FALLBACKS[f as usize].load(Ordering::Relaxed)))
}

/// The manager's calls, which the arm leaves to the guest.
struct Guest {
    called: bool,
}

impl PointCalls for Guest {
    fn add(&mut self, _: &mut Journal, _: u32, _: [u32; 4], _: u32) -> Result<u32, convex::Fallback> {
        self.called = true;
        Err(convex::Fallback::Memory)
    }

    fn remove(&mut self, _: &mut Journal, _: u32, _: u32, _: u32) -> Result<(), convex::Fallback> {
        self.called = true;
        Err(convex::Fallback::Memory)
    }
}

/// The moved slot (eax) and ecx and edx at the return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moved {
    pub slot: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// `0x00302c70` on a matching point, or the reason it declines.
pub fn answer(j: &mut Journal, args: &PointArgs, fpu: Fpu) -> Result<Moved, Fallback> {
    let mut calls = Guest { called: false };
    match manifold::add_point(j, args, fpu, &mut calls) {
        Ok(Added::Moved { slot, ecx, edx }) => Ok(Moved { slot, ecx, edx }),
        // The other exits come after a manager call, which declines.
        Ok(_) => Err(Fallback::Commit),
        Err(_) if calls.called => Err(Fallback::Commit),
        Err(convex::Fallback::Float) => Err(Fallback::Float),
        Err(_) => Err(Fallback::Memory),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declines_are_counted_by_reason_in_order() {
        let before = fallbacks();
        count(Fallback::Commit);
        let after = fallbacks();
        assert_eq!(after.map(|f| f.0), ["memory", "commit", "float", "unobserved"]);
        assert!(after[1].1 > before[1].1);
    }
}
