//! `sim.ledger` (docs/runtime/agent-api.md, "sim"): the guest phases of the
//! game tick the Rust driver still calls. Read from `sim_bridge::tick`'s
//! process ledger, so it needs no guest reads and answers on the
//! connection's thread.

use agent_proto::sim::ledger::{LedgerReply, PhaseRow};
use agent_proto::sim::method;
use agent_proto::{RpcError, code};
use engine::sim_tick::{self, Ledger, Owner, PHASES};

use crate::{Access, Agent};

/// The answer for `ledger`.
pub fn reply(ledger: &Ledger) -> LedgerReply {
    LedgerReply {
        tick: ledger.tick(),
        ticks: ledger.ticks(),
        phases: PHASES
            .iter()
            .enumerate()
            .map(|(i, p)| PhaseRow {
                entry: format!("{:#010x}", p.entry),
                name: p.name.to_owned(),
                owner: match p.owner {
                    Owner::Guest => "guest",
                    Owner::Rust => "rust",
                }
                .to_owned(),
                calls: ledger.calls(i),
            })
            .collect(),
        guest_total: sim_tick::guest_phases() as u32,
    }
}

/// Adds `sim.ledger` over the tick driver's ledger.
pub fn install(agent: &Agent) {
    agent.add_method(method::LEDGER, Access::Read, |_| {
        serde_json::to_value(reply(&sim_bridge::tick::ledger()))
            .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reply_lists_the_phases_in_order_with_their_calls() {
        let mut l = Ledger::new();
        l.record(0);
        l.record(0);
        l.finish_tick(7);
        let r = reply(&l);
        assert_eq!((r.tick, r.ticks, r.phases.len()), (7, 1, 29));
        assert_eq!(r.phases[0].entry, "0x0014d3a0");
        assert_eq!((r.phases[0].owner.as_str(), r.phases[0].calls), ("guest", 2));
        assert_eq!(r.guest_total, sim_tick::guest_phases() as u32);
    }
}
