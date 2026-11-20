//! `collision.stats` (docs/runtime/agent-api.md, "collision"): the native
//! collision world's call, bail, decode and float-environment counters.
//! A game method, answered at a tick boundary. The counters are process
//! globals, so the answer needs no guest memory; with
//! `impl.sim.native.collision` off they stay zero.

use crate::{Access, Agent};
use agent_proto::collision::{EntryStats, Precision, Stats, method};
use engine::sim_collision::{self as world, Bail, Entry};

/// Registers `collision.*` on `agent`.
pub fn install(agent: &Agent) {
    agent.add_game_method(method::STATS, Access::Read, |_, _| {
        Ok(serde_json::to_value(wire(&world::stats())).expect("stats serialise"))
    });
}

fn wire(s: &world::Stats) -> Stats {
    Stats {
        switch: "impl.sim.native.collision".into(),
        entries: Entry::ALL
            .iter()
            .map(|&e| {
                let c = &s.entries[e as usize];
                EntryStats {
                    name: e.name().into(),
                    addr: format!("{:08x}", e.addr()),
                    native: c.native,
                    bails: Bail::ALL
                        .iter()
                        .map(|&b| (b.name().to_owned(), c.bails[b as usize]))
                        .collect(),
                }
            })
            .collect(),
        decodes: s.decodes,
        decode_failures: s.decode_failures,
        flushes: s.flushes,
        bsps: s.bsps,
        bytes: s.bytes,
        precision: Precision {
            single: s.precision[0],
            reserved: s.precision[1],
            double: s.precision[2],
            extended: s.precision[3],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_name_every_entry_and_reason() {
        let mut s = world::Stats::default();
        s.entries[Entry::VectorTest as usize].native = 7;
        s.entries[Entry::Location as usize].bails[Bail::Alias as usize] = 2;
        s.decodes = 3;
        s.precision = [1, 0, 5, 0];
        let w = wire(&s);
        let names: Vec<&str> = w.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["point_to_leaf", "location", "vector_test", "vector_context"]
        );
        assert_eq!(w.entries[0].addr, "0014a280");
        assert_eq!(w.entries[2].native, 7);
        assert_eq!(w.entries[1].bails["alias"], 2);
        assert_eq!(w.entries[1].bails.len(), Bail::ALL.len());
        assert_eq!((w.decodes, w.precision.double), (3, 5));
    }

    #[test]
    fn the_method_is_registered_and_needs_no_guest() {
        use agent_proto::Object;
        let a = Agent::new();
        install(&a);
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        let conn = a.connect(tx);
        struct NoGuest;
        impl crate::Reader for NoGuest {
            fn status(&mut self) -> crate::GameStatus {
                crate::GameStatus::default()
            }
            fn objects(&mut self) -> Result<Vec<Object>, String> {
                Ok(Vec::new())
            }
        }
        a.submit(
            conn,
            r#"{"jsonrpc":"2.0","id":1,"method":"session.methods"}"#,
        );
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(line.contains(method::STATS), "{line}");
        a.submit(
            conn,
            r#"{"jsonrpc":"2.0","id":2,"method":"collision.stats"}"#,
        );
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(line.contains("\"vector_context\""), "{line}");
        assert!(line.contains("impl.sim.native.collision"), "{line}");
    }
}
