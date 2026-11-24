//! `anim.graph_walk` (docs/runtime/agent-api.md, "anim"): the counters of
//! the animation graph's node walk `1dd1c0`, its arm and its body
//! (docs/engine/skeleton-walk.md, "The graph walk `1dd1c0`"). A game
//! method, answered at a tick boundary. The counters are process globals,
//! so the answer needs no guest memory; with `impl.sim.native.anim` off
//! they stay zero.

use crate::{Access, Agent};
use agent_proto::anim::{Arm, Body, GraphWalk, Mismatch, method};
use engine::sim_anim::graph_walk::{self as walk, ArmStats, Decline, Stats};
use std::collections::BTreeMap;

/// Registers `anim.*` on `agent`.
pub fn install(agent: &Agent) {
    agent.add_game_method(method::GRAPH_WALK, Access::Read, |_, _| {
        Ok(
            serde_json::to_value(wire(&walk::arm_stats(), &walk::stats()))
                .expect("stats serialise"),
        )
    });
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
        assert!(line.contains(method::GRAPH_WALK), "{line}");
        a.submit(
            conn,
            r#"{"jsonrpc":"2.0","id":2,"method":"anim.graph_walk"}"#,
        );
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(line.contains("\"001dd1c0\""), "{line}");
        assert!(line.contains("impl.sim.native.anim"), "{line}");
    }
}
