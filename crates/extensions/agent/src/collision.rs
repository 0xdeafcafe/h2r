//! `collision.stats` (docs/runtime/agent-api.md, "collision"): the native
//! collision world's call, bail, decode and float-environment counters.
//! A game method, answered at a tick boundary. The counters are process
//! globals, so the answer needs no guest memory.

use crate::{Access, Agent};
use agent_proto::collision::{EntryStats, Precision, RayParity, Stats, method};
use engine::sim_collision::{self as world, Bail, Entry};

/// Registers `collision.*` on `agent`.
pub fn install(agent: &Agent) {
    agent.add_game_method(method::STATS, Access::Read, |_, _| {
        Ok(serde_json::to_value(wire(&world::stats())).expect("stats serialise"))
    });
}

fn parity(stats: &world::VectorParity) -> agent_proto::collision::VectorParity {
    agent_proto::collision::VectorParity {
        matched: stats.matched,
        mismatched: stats.mismatched,
        unverified: Bail::ALL.iter().map(|&b| (b.name().to_owned(), stats.unverified[b as usize])).collect(),
        first_mismatch: stats.first_mismatch.clone(),
        exits_matched: stats.exits_matched,
        answered: stats.answered,
    }
}

fn wire(s: &world::Stats) -> Stats {
    Stats {
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
                    mxcsr: c.mxcsr.iter().map(|(w, n)| (format!("{w:08x}"), *n)).collect(),
                    x87_control: c.x87_control
                        .iter()
                        .map(|(w, n)| (format!("{w:04x}"), *n))
                        .collect(),
                }
            })
            .collect(),
        decodes: s.decodes,
        decode_failures: s.decode_failures,
        flushes: s.flushes,
        bsps: s.bsps,
        mopps: s.mopps,
        bytes: s.bytes,
        array_bytes: s.array_bytes,
        budget_bytes: s.budget_bytes,
        ring_fallbacks: s.ring_fallbacks,
        cluster_budget_reads: s.cluster_budget_reads,
        precision: Precision {
            single: s.precision[0],
            reserved: s.precision[1],
            double: s.precision[2],
            extended: s.precision[3],
        },
        vector: parity(&s.vector),
        context: parity(&s.context),
        instance: parity(&s.instance),
        flood: parity(&s.flood),
        visit: parity(&s.visit),
        query: parity(&s.query),
        ray: RayParity {
            matched: s.ray.matched,
            mismatched: s.ray.mismatched,
            unverified: Bail::ALL
                .iter()
                .map(|&b| (b.name().to_owned(), s.ray.unverified[b as usize]))
                .collect(),
            fraction_moved: s.ray.fraction_moved,
            first_mismatch: s.ray.first_mismatch.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_name_every_entry_and_reason() {
        let mut s = world::Stats::default();
        s.entries[Entry::PointToLeaf as usize].native = 7;
        s.entries[Entry::Location as usize].bails[Bail::Alias as usize] = 2;
        s.entries[Entry::InstanceTrace as usize].mxcsr.insert(0x1fa0, 3);
        s.entries[Entry::InstanceTrace as usize].x87_control.insert(0x037f, 3);
        s.decodes = 3;
        s.mopps = 2;
        s.entries[Entry::MoppBox as usize].bails[Bail::Grow as usize] = 5;
        s.bytes = 950;
        s.array_bytes = 246;
        s.budget_bytes = 8 << 20;
        s.ring_fallbacks = 4;
        s.cluster_budget_reads = 19;
        s.entries[Entry::Location as usize].bails[Bail::Budget as usize] = 6;
        s.precision = [1, 0, 5, 0];
        s.context.matched = 5;
        s.instance.mismatched = 6;
        s.flood.matched = 3;
        s.visit.unverified[Bail::Alias as usize] = 2;
        s.query.matched = 7;
        s.vector.matched = 9;
        s.vector.mismatched = 1;
        s.vector.first_mismatch = Some("AL: expected 1, guest 0".into());
        s.vector.unverified[Bail::Trap as usize] = 3;
        s.ray.matched = 11;
        s.ray.unverified[Bail::Float as usize] = 2;
        s.ray.first_mismatch = Some("different step at observation 3".into());
        let w = wire(&s);
        let names: Vec<&str> = w.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "point_to_leaf",
                "location",
                "vector_test",
                "vector_context",
                "instance_trace",
                "cluster_flood",
                "object_visit",
                "mopp_box",
                "mopp_ray",
                "object_sphere",
                "query",
            ]
        );
        assert_eq!(w.entries[8].addr, "00301800");
        assert_eq!(w.entries[9].addr, "0011e5e0");
        assert_eq!(w.entries[0].addr, "0014a280");
        assert_eq!(w.entries[0].native, 7);
        assert_eq!(w.entries[1].bails["alias"], 2);
        assert_eq!(w.entries[1].bails.len(), Bail::ALL.len());
        assert_eq!((w.decodes, w.precision.double), (3, 5));
        assert_eq!((w.mopps, w.entries[5].addr.as_str()), (2, "0014a5b0"));
        assert_eq!(w.entries[7].bails["grow"], 5);
        assert_eq!(w.entries[1].bails["budget"], 6);
        assert_eq!((w.bytes, w.array_bytes), (950, 246));
        assert_eq!((w.budget_bytes, w.ring_fallbacks, w.cluster_budget_reads), (8 << 20, 4, 19));
        assert_eq!(w.entries[4].mxcsr["00001fa0"], 3);
        assert_eq!(w.entries[4].x87_control["037f"], 3);
        assert_eq!((w.ray.matched, w.ray.unverified["float"]), (11, 2));
        assert_eq!((w.vector.matched, w.vector.mismatched, w.vector.unverified["trap"]), (9, 1, 3));
        assert_eq!(w.context.matched, 5);
        assert_eq!(w.instance.mismatched, 6);
        assert_eq!(w.flood.matched, 3);
        assert_eq!(w.visit.unverified["alias"], 2);
        assert_eq!(w.query.matched, 7);
        assert_eq!(w.vector.first_mismatch, s.vector.first_mismatch);
        assert_eq!(w.ray.unverified.len(), Bail::ALL.len());
        assert_eq!(
            w.ray.first_mismatch.as_deref(),
            Some("different step at observation 3")
        );
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
    }
}
