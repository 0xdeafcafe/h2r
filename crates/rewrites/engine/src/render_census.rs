//! The render-writes census (docs/engine/sim-render-split.md, slice 0):
//! process counters for the places the frame step reaches the simulation's
//! state, read by `sim.render_writes`. They are host memory, never hashed,
//! never saved, and answer how often each coupling fires before the slices
//! that remove them are sized.
//!
//! A selector call or a pose refusal is counted "in a frame" when it happens
//! outside the tick driver ([`TickScope`]): the render's object setup runs
//! between ticks on the game thread, and the tick body runs inside
//! `sim_tick::run_with`. A load or boot call outside both lands in the frame
//! column too (**S**: the first run's counts show whether that matters).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use guest_layout::memory::GuestMemory;
use guest_layout::points;

static IN_TICK: AtomicBool = AtomicBool::new(false);

static SELECTOR_IN_TICK: AtomicU64 = AtomicU64::new(0);
static SELECTOR_IN_FRAME: AtomicU64 = AtomicU64::new(0);
static GAMEPLAY_DRAWS_IN_FRAME: AtomicU64 = AtomicU64::new(0);
static SETUP_POSE_REFUSALS: AtomicU64 = AtomicU64::new(0);
static TICKS_SCANNED: AtomicU64 = AtomicU64::new(0);
static LAST_LIVE: AtomicU64 = AtomicU64::new(0);
static LAST_BIT30: AtomicU64 = AtomicU64::new(0);
static LAST_PENDING: AtomicU64 = AtomicU64::new(0);
static MAX_PENDING: AtomicU64 = AtomicU64::new(0);
static SUM_PENDING: AtomicU64 = AtomicU64::new(0);

/// The counters at one moment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Variant-selector (`0x001dd630`) calls made inside a tick.
    pub selector_in_tick: u64,
    /// Selector calls made outside a tick: the setup's choices.
    pub selector_in_frame: u64,
    /// Of those, calls on a manager whose gameplay bit is set: draws that
    /// would step the simulation's seed if the render stream were not in.
    pub gameplay_draws_in_frame: u64,
    /// Times the native setup refused a dirty shared pose
    /// (`Refusal::Palette`, `palette::object_pose`): each is a
    /// `0x000bd7b2` evaluation left to the guest at setup time.
    pub setup_pose_refusals: u64,
    /// Ticks whose end scanned the object pool.
    pub ticks_scanned: u64,
    /// The last scan: live objects, objects with body flag bit 30 set, and
    /// those with bit 30 set and bit 29 clear (the ones `0x000bd7b2` would
    /// evaluate).
    pub last_live: u64,
    pub last_bit30: u64,
    pub last_pending: u64,
    /// The most `pending` objects any scan found, and their sum over scans.
    pub max_pending: u64,
    pub sum_pending: u64,
}

pub fn snapshot() -> Snapshot {
    Snapshot {
        selector_in_tick: SELECTOR_IN_TICK.load(Relaxed),
        selector_in_frame: SELECTOR_IN_FRAME.load(Relaxed),
        gameplay_draws_in_frame: GAMEPLAY_DRAWS_IN_FRAME.load(Relaxed),
        setup_pose_refusals: SETUP_POSE_REFUSALS.load(Relaxed),
        ticks_scanned: TICKS_SCANNED.load(Relaxed),
        last_live: LAST_LIVE.load(Relaxed),
        last_bit30: LAST_BIT30.load(Relaxed),
        last_pending: LAST_PENDING.load(Relaxed),
        max_pending: MAX_PENDING.load(Relaxed),
        sum_pending: SUM_PENDING.load(Relaxed),
    }
}

/// Marks the tick body: counts made while it lives are tick-time.
pub struct TickScope(());

impl TickScope {
    pub fn enter() -> Self {
        IN_TICK.store(true, Relaxed);
        Self(())
    }
}

impl Drop for TickScope {
    fn drop(&mut self) {
        IN_TICK.store(false, Relaxed);
    }
}

/// One call of the variant selector (`0x001dd630`), from the channel remap
/// (`sim_anim::manager::tracks::bind`); `gameplay` is the manager's `+0x6c`
/// bit 1, the bit that makes the selector draw from the shared seed.
pub fn selector_call(gameplay: bool) {
    if IN_TICK.load(Relaxed) {
        SELECTOR_IN_TICK.fetch_add(1, Relaxed);
    } else {
        SELECTOR_IN_FRAME.fetch_add(1, Relaxed);
        if gameplay {
            GAMEPLAY_DRAWS_IN_FRAME.fetch_add(1, Relaxed);
        }
    }
}

/// The native setup refused a dirty shared pose.
pub fn setup_pose_refusal() {
    SETUP_POSE_REFUSALS.fetch_add(1, Relaxed);
}

/// What one pool scan found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoseScan {
    pub live: u64,
    pub bit30: u64,
    pub pending: u64,
}

/// Object body flags (`body + 4`): bit 30 is the node matrices' dirty bit,
/// bit 29 the "do not evaluate" bit (`palette::object_pose`).
const BODY_FLAGS: u32 = 4;

/// Walk the object pool and count the bodies whose dirty-pose bit is set.
/// `None` where the pool is not readable (before a map is loaded).
pub fn scan_poses(m: &dyn GuestMemory) -> Option<PoseScan> {
    let header = m.u32(points::OBJECTS.base).ok().filter(|&h| h != 0)?;
    let end = m.u32(header.wrapping_add(0x38)).ok().filter(|&e| e <= 0x10000)?;
    let data = m.u32(header.wrapping_add(0x44)).ok().filter(|&d| d != 0)?;
    let mut scan = PoseScan::default();
    for i in 0..end {
        let slot = data.wrapping_add(i * 12);
        if m.u16(slot).ok()? == 0 {
            continue;
        }
        let Ok(body) = m.u32(slot.wrapping_add(8)) else { continue };
        if body == 0 {
            continue;
        }
        let Ok(flags) = m.u32(body.wrapping_add(BODY_FLAGS)) else { continue };
        scan.live += 1;
        if flags & (1 << 30) != 0 {
            scan.bit30 += 1;
            if flags & (1 << 29) == 0 {
                scan.pending += 1;
            }
        }
    }
    Some(scan)
}

/// Record a tick-end scan.
pub fn record_scan(scan: PoseScan) {
    TICKS_SCANNED.fetch_add(1, Relaxed);
    LAST_LIVE.store(scan.live, Relaxed);
    LAST_BIT30.store(scan.bit30, Relaxed);
    LAST_PENDING.store(scan.pending, Relaxed);
    MAX_PENDING.fetch_max(scan.pending, Relaxed);
    SUM_PENDING.fetch_add(scan.pending, Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const HEADER: u32 = 0x1000;
    const DATA: u32 = 0x2000;

    fn pool(bodies: &[(u16, u32)]) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.put_u32(points::OBJECTS.base, HEADER);
        g.put_u32(HEADER + 0x38, bodies.len() as u32);
        g.put_u32(HEADER + 0x44, DATA);
        for (i, &(salt, flags)) in bodies.iter().enumerate() {
            let slot = DATA + i as u32 * 12;
            let body = 0x4000 + i as u32 * 0x100;
            g.put(slot, &salt.to_le_bytes());
            g.put_u32(slot + 8, body);
            g.put_u32(body + BODY_FLAGS, flags);
        }
        g
    }

    #[test]
    fn the_scan_counts_bit_30_with_and_without_bit_29() {
        let g = pool(&[(1, 1 << 30), (2, 1 << 30 | 1 << 29), (0, 1 << 30), (3, 0)]);
        assert_eq!(scan_poses(&g), Some(PoseScan { live: 3, bit30: 2, pending: 1 }));
    }

    #[test]
    fn an_unloaded_pool_is_none() {
        assert_eq!(scan_poses(&FakeGuest::new()), None);
    }

    #[test]
    fn counts_split_by_the_tick_scope() {
        let before = snapshot();
        selector_call(true);
        {
            let _tick = TickScope::enter();
            selector_call(true);
        }
        selector_call(false);
        let after = snapshot();
        assert_eq!(after.selector_in_frame - before.selector_in_frame, 2);
        assert_eq!(after.gameplay_draws_in_frame - before.gameplay_draws_in_frame, 1);
        assert_eq!(after.selector_in_tick - before.selector_in_tick, 1);
    }

    #[test]
    fn a_scan_keeps_the_last_and_the_most() {
        record_scan(PoseScan { live: 9, bit30: 5, pending: 4 });
        record_scan(PoseScan { live: 8, bit30: 1, pending: 1 });
        let s = snapshot();
        assert_eq!((s.last_live, s.last_bit30, s.last_pending), (8, 1, 1));
        assert!(s.max_pending >= 4 && s.ticks_scanned >= 2);
    }
}
