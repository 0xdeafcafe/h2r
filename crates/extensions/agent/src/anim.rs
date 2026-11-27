//! Animation parity counters at a tick boundary. `anim.graph_walk` reports
//! whole node walks and `anim.manager` reports manager and graph segments.
//! Both read process counters under `impl.sim.native.anim`.

use crate::{Access, Agent};
use agent_proto::anim::{
    Arm, Body, GraphWalk, Manager, ManagerEntry, Mismatch, SegmentMismatch, method,
};
use engine::sim_anim::graph_walk::{self as walk, ArmStats, Decline, Stats};
use engine::sim_anim::manager::arm;
use std::collections::BTreeMap;

/// Registers `anim.*` on `agent`.
pub fn install(agent: &Agent) {
    agent.add_game_method(method::GRAPH_WALK, Access::Read, |_, _| {
        Ok(
            serde_json::to_value(wire(&walk::arm_stats(), &walk::stats()))
                .expect("stats serialise"),
        )
    });
    agent.add_game_method(method::MANAGER, Access::Read, |_, _| {
        Ok(serde_json::to_value(manager_wire(arm::stats())).expect("stats serialise"))
    });
}

fn manager_wire(stats: Vec<(u32, arm::Stats)>) -> Manager {
    Manager {
        switch: "impl.sim.native.anim".into(),
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

fn wire(arm: &ArmStats, body: &Stats) -> GraphWalk {
    GraphWalk {
        switch: "impl.sim.native.anim".into(),
        addr: format!("{:08x}", walk::ENTRY),
        arm: Arm {
            checked: arm.checked,
            matched: arm.matched,
            mismatched: arm.mismatched,
            dropped: arm.dropped,
            declined: reasons(&arm.declined),
            first: arm.first.as_ref().map(|(m, c)| Mismatch {
                what: m.what.clone(),
                plan: m.plan.clone(),
                guest: m.guest.clone(),
                entry: [
                    ("eax", c.eax),
                    ("ecx", c.ecx),
                    ("edx", c.edx),
                    ("ebx", c.ebx),
                    ("esp", c.esp),
                    ("ebp", c.ebp),
                    ("esi", c.esi),
                    ("edi", c.edi),
                    ("mxcsr", c.mxcsr),
                ]
                .into_iter()
                .map(|(name, v)| (name.to_owned(), format!("{v:08x}")))
                .collect(),
            }),
        },
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
    use runtime::CpuAbi;

    #[test]
    fn the_counters_name_every_reason_and_the_first_mismatch() {
        let mut arm = ArmStats {
            checked: 9,
            matched: 8,
            mismatched: 1,
            ..ArmStats::default()
        };
        arm.declined[Decline::Queue as usize] = 2;
        arm.first = Some((
            walk::Mismatch {
                what: "matrix 2 +0x10".into(),
                plan: "0x01".into(),
                guest: "0x00".into(),
            },
            CpuAbi {
                esp: 0x007f_0000,
                ..CpuAbi::default()
            },
        ));
        let mut body = Stats {
            native: 4,
            ..Stats::default()
        };
        body.declined[Decline::Alias as usize] = 3;
        let w = wire(&arm, &body);
        assert_eq!(w.addr, "001dd1c0");
        assert_eq!((w.arm.checked, w.arm.matched, w.arm.mismatched), (9, 8, 1));
        assert_eq!(w.arm.declined.len(), Decline::ALL.len());
        assert_eq!(w.arm.declined["queue"], 2);
        let first = w.arm.first.unwrap();
        assert_eq!(first.what, "matrix 2 +0x10");
        assert_eq!(first.entry["esp"], "007f0000");
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
                "switch": "impl.sim.native.anim",
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
        assert!(line.contains("impl.sim.native.anim"), "{line}");
        a.submit(conn, r#"{"jsonrpc":"2.0","id":3,"method":"anim.manager"}"#);
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        let counters: Manager = serde_json::from_value(reply["result"].clone()).unwrap();
        assert_eq!(counters.switch, "impl.sim.native.anim");
    }
}
