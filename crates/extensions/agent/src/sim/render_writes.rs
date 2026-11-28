//! `sim.render_writes` (docs/runtime/agent-api-sim.md): the render-writes
//! census of docs/engine/sim-render-split.md, slice 0. Read from
//! `engine::render_census`'s process counters, so it needs no guest reads
//! and answers on the connection's thread.

use agent_proto::sim::method;
use agent_proto::sim::render_writes::RenderWritesReply;
use agent_proto::{RpcError, code};
use engine::render_census::Snapshot;

use crate::{Access, Agent};

/// The answer for a snapshot of the counters.
pub fn reply(s: Snapshot) -> RenderWritesReply {
    RenderWritesReply {
        selector_in_tick: s.selector_in_tick,
        selector_in_frame: s.selector_in_frame,
        gameplay_draws_in_frame: s.gameplay_draws_in_frame,
        setup_pose_refusals: s.setup_pose_refusals,
        ticks_scanned: s.ticks_scanned,
        live_objects: s.last_live,
        bit30_objects: s.last_bit30,
        pending_objects: s.last_pending,
        pending_max: s.max_pending,
        pending_sum: s.sum_pending,
        // The frame guard (slice 1) fills this.
        guarded_pages_dirtied: 0,
    }
}

/// Adds `sim.render_writes` over the census counters.
pub fn install(agent: &Agent) {
    agent.add_method(method::RENDER_WRITES, Access::Read, |_| {
        serde_json::to_value(reply(engine::render_census::snapshot()))
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reply_carries_the_snapshot() {
        let s = Snapshot {
            selector_in_frame: 4,
            gameplay_draws_in_frame: 2,
            setup_pose_refusals: 1,
            ticks_scanned: 10,
            last_pending: 3,
            max_pending: 6,
            sum_pending: 20,
            ..Snapshot::default()
        };
        let r = reply(s);
        assert_eq!((r.selector_in_frame, r.gameplay_draws_in_frame, r.setup_pose_refusals), (4, 2, 1));
        assert_eq!((r.pending_objects, r.pending_max, r.pending_sum, r.guarded_pages_dirtied), (3, 6, 20, 0));
    }
}
