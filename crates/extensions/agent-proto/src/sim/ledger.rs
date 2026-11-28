//! `sim.ledger`: the guest phases of the game tick the Rust driver still
//! calls (`engine::sim_tick`, docs/engine/sim-rewrite.md, "The tick
//! driver"). The goal is `guest_total` 0.

use std::collections::BTreeMap;

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
    /// Nanoseconds in this phase over every tick (monotonic clock).
    pub ns: u64,
    /// Nanoseconds in this phase in the last tick; 0 for a tick that
    /// skipped its phases.
    pub last_ns: u64,
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
    /// Nanoseconds in the tick body over every tick. The phases' `ns` sum
    /// to at most this.
    pub tick_ns: u64,
    /// Nanoseconds in the last tick body.
    pub tick_last_ns: u64,
    /// Nanoseconds from one tick's entry to the next's, summed over `loops`
    /// spans: the tick plus what the game thread does between ticks (sound
    /// runs outside the 29 phases). Ms per tick is `loop_ns / loops / 1e6`.
    pub loop_ns: u64,
    /// The last such span.
    pub loop_last_ns: u64,
    /// Entry-to-entry spans measured.
    pub loops: u64,
    /// Phases owned by the guest: what `tick-ledger.toml` bounds, per tick.
    pub guest_total: u32,
    /// The sound table's counters by name (`sound.admitted`,
    /// `sound.phase_runs`, ...; docs/engine/sound-instances.md, "Counters"):
    /// the tick's 30th phase, the sound phase. Empty while the table is off.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sound: BTreeMap<String, u64>,
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
                    ns: 82_000_000,
                    last_ns: 2_100_000,
                },
                PhaseRow {
                    entry: "0x0016f200".into(),
                    name: "countdown_requests".into(),
                    owner: "rust".into(),
                    calls: 0,
                    ns: 0,
                    last_ns: 0,
                },
            ],
            tick_ns: 120_000_000,
            tick_last_ns: 3_000_000,
            loop_ns: 600_000_000,
            loop_last_ns: 15_000_000,
            loops: 40,
            guest_total: 1,
            sound: [("sound.phase_runs".to_owned(), 40)].into(),
        };
        let json = serde_json::to_value(&reply).unwrap();
        assert_eq!(json["phases"][0]["entry"], "0x0014d3a0");
        assert_eq!(json["guest_total"], 1);
        assert_eq!(json["sound"]["sound.phase_runs"], 40);
        assert_eq!(json["phases"][0]["ns"], 82_000_000);
        assert_eq!(json["phases"][0]["last_ns"], 2_100_000);
        assert_eq!((json["tick_ns"].as_u64(), json["loops"].as_u64()), (Some(120_000_000), Some(40)));
        assert_eq!(serde_json::from_value::<LedgerReply>(json).unwrap(), reply);
        assert_eq!(crate::sim::method::LEDGER, "sim.ledger");
        assert!(crate::sim::method::ALL.contains(&crate::sim::method::LEDGER));
    }
}
