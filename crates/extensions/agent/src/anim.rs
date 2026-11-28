//! Animation parity counters at a tick boundary. `anim.graph_walk` reports
//! whole node walks and `anim.manager` reports manager and graph segments.
//! Both read process counters; the Rust answers every call it does not
//! decline, and a decline is counted by reason.

use crate::{Access, Agent};
use agent_proto::anim::{
    Body, GraphWalk, Manager, ManagerEntry, SegmentMismatch, method,
};
use engine::sim_anim::graph_walk::{self as walk, Decline, Stats};
use engine::sim_anim::manager::arm;
use std::collections::BTreeMap;

/// Registers `anim.*` on `agent`.
pub fn install(agent: &Agent) {
    agent.add_game_method(method::GRAPH_WALK, Access::Read, |_, _| {
        Ok(serde_json::to_value(wire(&walk::stats())).expect("stats serialise"))
    });
    agent.add_game_method(method::MANAGER, Access::Read, |_, _| {
        Ok(serde_json::to_value(manager_wire(arm::stats())).expect("stats serialise"))
    });
}

fn manager_wire(stats: Vec<(u32, arm::Stats)>) -> Manager {
    Manager {
        entries: stats
            .into_iter()
            .map(|(addr, s)| {
                let entry = ManagerEntry {
                    matched: s.matched,
                    differed: s.differed,
                    declined: s.declined,
                    interrupted: s.interrupted,
                    answered: s.answered,
                    first: s.first.map(|m| SegmentMismatch {
                        pc: format!("{:08x}", m.pc),
                        what: m.what,
                        plan: m.plan,
                        guest: m.guest,
                    }),
                };
                (format!("{addr:08x}"), entry)
            })
            .collect(),
    }
}

fn reasons(counts: &[u64]) -> BTreeMap<String, u64> {
    Decline::ALL
        .iter()
        .map(|&d| (d.name().to_owned(), counts[d as usize]))
        .collect()
}

fn wire(body: &Stats) -> GraphWalk {
    GraphWalk {
        addr: format!("{:08x}", walk::ENTRY),
        body: Body {
            native: body.native,
            nodes: body.nodes,
            declined: reasons(&body.declined),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_counters_name_every_reason() {
        let mut body = Stats {
            native: 4,
            ..Stats::default()
        };
        body.declined[Decline::Alias as usize] = 3;
        let w = wire(&body);
        assert_eq!(w.addr, "001dd1c0");
        assert_eq!(w.body.declined.len(), Decline::ALL.len());
        assert_eq!((w.body.native, w.body.declined["alias"]), (4, 3));
    }

    #[test]
    fn manager_reports_segment_outcomes_and_the_first_difference() {
        let stats = arm::Stats {
            matched: 7,
            differed: 2,
            declined: 3,
            interrupted: 4,
            answered: 5,
            first: Some(arm::Mismatch {
                pc: 0x10124,
                what: "eax".into(),
                plan: "1".into(),
                guest: "3".into(),
            }),
        };
        let result = serde_json::to_value(manager_wire(vec![(0x10100, stats)])).unwrap();
        assert_eq!(
            result,
            serde_json::json!({
                "entries": { "00010100": {
                    "matched": 7, "differed": 2, "declined": 3, "interrupted": 4, "answered": 5,
                    "first": {"pc": "00010124", "what": "eax", "plan": "1", "guest": "3"}
                }}
            })
        );
    }

    #[test]
    fn the_methods_are_registered_and_need_no_guest() {
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
        for name in method::ALL {
            assert!(line.contains(*name), "{line}");
        }
        a.submit(
            conn,
            r#"{"jsonrpc":"2.0","id":2,"method":"anim.graph_walk"}"#,
        );
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(line.contains("\"001dd1c0\""), "{line}");
        a.submit(conn, r#"{"jsonrpc":"2.0","id":3,"method":"anim.manager"}"#);
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        let _: Manager = serde_json::from_value(reply["result"].clone()).unwrap();
    }
}
