//! `sim.ledger`: the guest phases of the game tick the Rust driver still
//! calls (`engine::sim_tick`, docs/engine/sim-rewrite.md, "The tick
//! driver"). The goal is `guest_total` 0.

use serde::{Deserialize, Serialize};

/// One phase of the tick, in retail order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseRow {
    /// The phase's guest entry, `0x` and eight hex digits.
    pub entry: String,
    pub name: String,
    /// `guest` while the driver calls the lifted original, `rust` once a
    /// Rust body runs it.
    pub owner: String,
    /// Guest calls to this phase since the game started.
    pub calls: u32,
}

/// `sim.ledger`'s answer (it takes no parameters).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerReply {
    /// The game tick count after the last tick the driver ran; 0 before
    /// the first.
    pub tick: u32,
    /// Ticks the driver has run.
    pub ticks: u64,
    pub phases: Vec<PhaseRow>,
    /// Phases owned by the guest: what `tick-ledger.toml` bounds, per tick.
    pub guest_total: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sim_ledger_round_trips() {
        let reply = LedgerReply {
            tick: 42,
            ticks: 41,
            phases: vec![
                PhaseRow {
                    entry: "0x0014d3a0".into(),
                    name: "players_before".into(),
                    owner: "guest".into(),
                    calls: 41,
                },
                PhaseRow {
                    entry: "0x0016f200".into(),
                    name: "unknown_16f200".into(),
                    owner: "rust".into(),
                    calls: 0,
                },
            ],
            guest_total: 1,
        };
        let json = serde_json::to_value(&reply).unwrap();
        assert_eq!(json["phases"][0]["entry"], "0x0014d3a0");
        assert_eq!(json["guest_total"], 1);
        assert_eq!(serde_json::from_value::<LedgerReply>(json).unwrap(), reply);
        assert_eq!(crate::sim::method::LEDGER, "sim.ledger");
        assert!(crate::sim::method::ALL.contains(&crate::sim::method::LEDGER));
    }
}
