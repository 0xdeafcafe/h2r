//! `sim.ledger` (docs/runtime/agent-api.md, "sim"): the guest phases of the
//! game tick the Rust driver still calls. Read from `sim_bridge::tick`'s
//! process ledger, so it needs no guest reads and answers on the
//! connection's thread.

use agent_proto::sim::ledger::{LedgerReply, PhaseRow};
use agent_proto::sim::method;
use agent_proto::{RpcError, code};
use engine::sim_tick::{self, Ledger, Owner, SYSTEMS};

use crate::{Access, Agent};

/// The answer for `ledger`, with the sound table's counters.
pub fn reply(ledger: &Ledger, sound: std::collections::BTreeMap<String, u64>) -> LedgerReply {
    LedgerReply {
        tick: ledger.tick(),
        ticks: ledger.ticks(),
        phases: SYSTEMS
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
                ns: ledger.phase_ns(i),
                last_ns: ledger.phase_last_ns(i),
            })
            .collect(),
        tick_ns: ledger.tick_ns(),
        tick_last_ns: ledger.tick_last_ns(),
        loop_ns: ledger.loop_ns(),
        loop_last_ns: ledger.loop_last_ns(),
        loops: ledger.loops(),
        guest_total: sim_tick::guest_phases() as u32,
        sound,
    }
}

/// Adds `sim.ledger` over the tick driver's ledger.
pub fn install(agent: &Agent) {
    agent.add_method(method::LEDGER, Access::Read, |_| {
        serde_json::to_value(reply(&sim_bridge::tick::ledger(), sim_bridge::sim_sound::entries::counters()))
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
        let mut ns = [0u64; 29];
        ns[0] = 500;
        ns[1] = 250;
        l.finish_tick(7, &ns, 1_000);
        let r = reply(&l, Default::default());
        assert_eq!((r.tick, r.ticks, r.phases.len()), (7, 1, 29));
        assert_eq!(r.phases[0].entry, "0x0014d3a0");
        assert_eq!((r.phases[0].owner.as_str(), r.phases[0].calls), ("guest", 2));
        assert_eq!((r.phases[0].ns, r.phases[0].last_ns, r.phases[1].ns), (500, 500, 250));
        assert_eq!((r.tick_ns, r.tick_last_ns, r.loops), (1_000, 1_000, 0));
        assert!(r.phases.iter().map(|p| p.ns).sum::<u64>() <= r.tick_ns);
        assert_eq!(r.guest_total, sim_tick::guest_phases() as u32);
        assert!(r.sound.is_empty());
    }
}
