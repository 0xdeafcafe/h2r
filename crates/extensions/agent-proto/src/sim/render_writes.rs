//! `sim.render_writes`: the render-writes census
//! (docs/engine/sim-render-split.md, slice 0). Process counters since the
//! game started; diff two answers for a span.

use serde::{Deserialize, Serialize};

/// `sim.render_writes`'s answer (it takes no parameters).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderWritesReply {
    /// Variant-selector calls inside a tick, since the game started.
    pub selector_in_tick: u64,
    /// Variant-selector calls outside a tick: the render setup's choices.
    /// Zero once slice 3 lands.
    pub selector_in_frame: u64,
    /// Of those, calls with the manager's gameplay bit set (draws from the
    /// shared seed). Zero once slice 5 lands.
    pub gameplay_draws_in_frame: u64,
    /// Times the native setup refused a dirty shared pose, leaving the
    /// `0x000bd7b2` evaluation to the guest at setup time. Zero once
    /// slice 4 lands.
    pub setup_pose_refusals: u64,
    /// Ticks whose end scanned the object pool.
    pub ticks_scanned: u64,
    /// The last scan: live objects, bodies with flag bit 30 set, and those
    /// with bit 30 set and bit 29 clear.
    pub live_objects: u64,
    pub bit30_objects: u64,
    pub pending_objects: u64,
    /// The largest `pending_objects` of any scan, and their sum over every
    /// scan (mean per tick is `sum / ticks_scanned`).
    pub pending_max: u64,
    pub pending_sum: u64,
    /// Hashed pages the frame step dirtied: the frame guard's count
    /// (slice 1). 0 until the guard exists.
    pub guarded_pages_dirtied: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_render_writes_round_trips() {
        let reply = RenderWritesReply {
            selector_in_tick: 12,
            selector_in_frame: 3,
            gameplay_draws_in_frame: 1,
            setup_pose_refusals: 2,
            ticks_scanned: 600,
            live_objects: 200,
            bit30_objects: 7,
            pending_objects: 5,
            pending_max: 9,
            pending_sum: 2400,
            guarded_pages_dirtied: 0,
        };
        let json = serde_json::to_value(&reply).unwrap();
        assert_eq!(json["selector_in_frame"], 3);
        assert_eq!(json["pending_sum"], 2400);
        assert_eq!(serde_json::from_value::<RenderWritesReply>(json).unwrap(), reply);
        assert_eq!(crate::sim::method::RENDER_WRITES, "sim.render_writes");
        assert!(crate::sim::method::ALL.contains(&crate::sim::method::RENDER_WRITES));
    }
}
