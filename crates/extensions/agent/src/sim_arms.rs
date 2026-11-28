//! `sim.arms` (docs/runtime/agent-api.md, "sim.arms"): one read of every
//! native-sim shadow and authority arm, the gate instrument for turning a
//! sim phase on by default (docs/engine/sim-rewrite.md, section 3). A game
//! method, answered at a tick boundary from process counters, so it needs
//! no guest memory; a subsystem whose switch is off reads zeros.
//!
//! Where a source has no counter for a field the field is 0:
//! * physics and collision count `matched` and `mismatched` only, so
//!   `checked` is their sum, and `answered` is 0;
//! * `anim.manager` reports `differed` for `mismatched`;
//! * the shadow ports (`seats`, `effects`) count `skipped` for `declined`
//!   and `answered` 0; the seats' `seats.aim`, `seats.device` and
//!   `seats.leaf.*` arms count their declines and unwatched audits for
//!   `declined`;
//! * `sound.voices` counts ticks for `checked`, `matched` and `mismatched`
//!   and voices for `answered` (`sim_bridge::sim_sound::voice_arm::Stats`).

use crate::{Access, Agent};
use agent_proto::sim_arms::{Arm, SimArms, Subsystem, method};
use engine::sim_anim::graph_walk as walk;
use engine::sim_anim::manager::arm as manager;
use engine::sim_collision as world;
use engine::sim_objects::functions::arm as function;
use engine::sim_objects::functions::parity as objects;
use engine::effect_update::arm as segment_arm;
use engine::sim_objects::native_walk;
use engine::sim_physics::parity as physics;
use engine::sim_seats::aim::arm as aim;
use engine::sim_seats::{player_objects, respawn};
use engine::sim_seats::device::arm as device;
use engine::sim_seats::leaf;
use hooks::Switches;
use hooks::switches::table::{self, Initial};
use sim_bridge::shadow::{SHADOW, Shadow};
use sim_bridge::sim_sound::voice_arm as voices;
use sim_effects::dispatch::Stats as EffectParts;

/// Registers `sim.arms` on `agent`.
pub fn install(agent: &Agent) {
    agent.add_game_method(method::ARMS, Access::Read, |_, _| {
        let switches = Switches::from_env().unwrap_or_default();
        Ok(serde_json::to_value(read(&switches, &SHADOW)).expect("arms serialise"))
    });
}

/// Every subsystem's arms from the process counters and `shadow`'s ports.
fn read(switches: &Switches, shadow: &Shadow) -> SimArms {
    let ports = shadow.status().ports;
    let group = |name: &str| -> Vec<Arm> {
        ports
            .iter()
            .filter(|p| p.group == name)
            .map(|p| Arm {
                name: p.name.to_owned(),
                checked: p.checked,
                matched: p.matched,
                mismatched: p.mismatched,
                answered: 0,
                declined: p.skipped,
            })
            .collect()
    };
    SimArms {
        subsystems: vec![
            subsystem(switches, "physics", physics_arms()),
            subsystem(switches, "objects", {
                let mut arms = vec![objects_wire(objects::status()), function_wire(function::status())];
                arms.extend(walk_wire(native_walk::stats()));
                arms
            }),
            subsystem(switches, "anim", anim_wire(&walk::stats(), manager::stats())),
            subsystem(switches, "collision", collision_arms(&world::stats())),
            subsystem(switches, "seats", {
                let mut arms = group(sim_bridge::sim_seats::GROUP);
                arms.push(aim_wire(aim::status()));
                arms.push(device_wire(device::status()));
                arms.push(respawn_wire(respawn::status()));
                arms.push(player_objects_wire(player_objects::status()));
                arms.extend(leaf::Leaf::ALL.map(|l| leaf_wire(l, leaf::status(l))));
                arms
            }),
            subsystem(switches, "effects", with_outside(group(engine::sim_effects::GROUP), &sim_bridge::sim_effects::outside())),
            subsystem(switches, "sound", vec![sound_wire(&voices::stats())]),
            effects_parts(sim_bridge::sim_effects_parts::status(), engine::effect_update::arm::stats()),
        ],
    }
}

/// `impl.sim.native.<name>` with its verdict over `arms`. A subsystem
/// whose switch is gone (anim, objects, effects) is always on, with
/// `switch` empty.
fn subsystem(switches: &Switches, name: &str, arms: Vec<Arm>) -> Subsystem {
    let switch = format!("impl.sim.native.{name}");
    if table::get(&switch).is_none() {
        return verdict(name, String::new(), true, arms);
    }
    let default = table::get(&switch).is_some_and(|s| s.initial == Initial::On);
    let on = switches.implementation(&switch["impl.".len()..], default);
    verdict(name, switch, on, arms)
}

fn verdict(name: &str, switch: String, on: bool, arms: Vec<Arm>) -> Subsystem {
    let checked: u64 = arms.iter().map(|a| a.checked).sum();
    let mismatched: u64 = arms.iter().map(|a| a.mismatched).sum();
    Subsystem {
        name: name.to_owned(),
        switch,
        on,
        arms,
        clean: checked > 0 && mismatched == 0,
    }
}

/// The effects phase in Rust, always on, with no switch (`switch` empty):
/// the parts each class's arm answered (`effects.<class>`), the parts
/// returned from Rust with no leaf (`effects.parts`: `lens`, `MGS2`,
/// `tdtl` and unknown groups), then the per-effect bodies' arm, one arm per
/// entry (`effects.body.<entry>`).
fn effects_parts(parts: EffectParts, bodies: Vec<(u32, segment_arm::Stats)>) -> Subsystem {
    let wire = |name: &str, answered: u64| Arm { name: name.to_owned(), answered, ..Arm::default() };
    let mut arms = vec![
        wire("effects.damage", parts.damage),
        wire("effects.object", parts.object),
        wire("effects.sound", parts.sound),
        wire("effects.character", parts.character),
        wire("effects.coln", parts.coln),
        wire("effects.decal", parts.decal),
        wire("effects.light", parts.light),
        wire("effects.parts", parts.returned + parts.other),
    ];
    arms.extend(segment_wire("effects.body", bodies));
    verdict("effects-parts", String::new(), true, arms)
}

/// The effect ports' calls run outside their `supported` check, added to
/// each port's `declined` (`sim_bridge::sim_effects::outside`).
fn with_outside(mut arms: Vec<Arm>, outside: &[(&str, u64)]) -> Vec<Arm> {
    for arm in &mut arms {
        if let Some((_, n)) = outside.iter().find(|(name, _)| *name == arm.name) {
            arm.declined += n;
        }
    }
    arms
}

fn physics_arms() -> Vec<Arm> {
    physics::status()
        .into_iter()
        .map(|(name, matched, mismatched, declined)| Arm {
            name: name.to_owned(),
            checked: matched + mismatched,
            matched,
            mismatched,
            answered: 0,
            declined,
        })
        .collect()
}

fn objects_wire(s: objects::Status) -> Arm {
    Arm {
        name: "objects.curve".to_owned(),
        answered: s.answered + s.ported_kind + s.ported_float,
        ..Arm::default()
    }
}

/// The aim-assist cone's kernel arm (`impl.sim.aim-assist`); unwatched
/// audits count as declined.
fn aim_wire(s: aim::Status) -> Arm {
    Arm {
        name: "seats.aim".to_owned(),
        checked: s.checked,
        matched: s.matched,
        mismatched: s.mismatched,
        answered: s.answered,
        declined: s.declined_total() + s.dropped,
    }
}

/// The respawn gate's kernel arm (`impl.sim.respawn-ready`); a call the
/// guest answers by name (`post`, `announce`, `every-player`, `fault`)
/// counts as declined.
fn respawn_wire(s: respawn::Status) -> Arm {
    Arm {
        name: "seats.respawn".to_owned(),
        checked: s.checked,
        matched: s.matched,
        mismatched: s.mismatched,
        answered: s.answered,
        declined: s.declined_total() + s.dropped,
    }
}

/// The pickup scan's kernel arm (`impl.sim.player-objects`); a candidate
/// or pickup the guest answers counts as declined.
fn player_objects_wire(s: player_objects::Status) -> Arm {
    Arm {
        name: "seats.player-objects".to_owned(),
        checked: s.checked,
        matched: s.matched,
        mismatched: s.mismatched,
        answered: s.answered,
        declined: s.declined_total() + s.dropped,
    }
}

/// The bound controller's device arm (under `impl.sim.native.seats`);
/// audits whose transfer never came count as declined.
fn device_wire(s: device::Status) -> Arm {
    Arm {
        name: "seats.device".to_owned(),
        checked: s.checked,
        matched: s.matched,
        mismatched: s.mismatched,
        answered: s.answered,
        declined: s.declined_total() + s.dropped,
    }
}

/// One seat leaf callee's arm (`seats.leaf.<name>`); audits whose
/// return never came count as declined.
fn leaf_wire(l: leaf::Leaf, s: leaf::Status) -> Arm {
    Arm {
        name: format!("seats.leaf.{}", l.name()),
        checked: s.checked,
        matched: s.matched,
        mismatched: s.mismatched,
        answered: s.answered,
        declined: s.declined_total() + s.dropped,
    }
}

/// The function value's arm; `dropped` audits count as declined.
fn function_wire(s: function::Status) -> Arm {
    Arm {
        name: "objects.function".to_owned(),
        checked: s.checked,
        matched: s.matched,
        mismatched: s.mismatched,
        answered: s.answered,
        declined: s.declined_total() + s.dropped,
    }
}

/// The walks in Rust, one arm per body (`objects.walk.<entry>`): its
/// segments answered, and its refused calls (every reason) declined.
/// Nothing is compared.
fn walk_wire(bodies: Vec<native_walk::Stats>) -> Vec<Arm> {
    bodies
        .into_iter()
        .map(|s| Arm {
            name: format!("objects.walk.{:08x}", s.entry),
            checked: 0,
            matched: 0,
            mismatched: 0,
            answered: s.segments,
            declined: s.refused.iter().sum(),
        })
        .collect()
}

/// A segment arm's entries as `<prefix>.<entry>`; faulted, stopped and
/// yielded guest segments count as declined.
fn segment_wire(prefix: &str, entries: Vec<(u32, segment_arm::Stats)>) -> Vec<Arm> {
    entries
        .into_iter()
        .map(|(entry, s)| Arm {
            name: format!("{prefix}.{entry:08x}"),
            answered: s.answered,
            // Answered all the same; the float environment is the error.
            declined: s.float_env,
            ..Arm::default()
        })
        .collect()
}

/// The graph walk compares nothing: its body answers or declines by reason.
fn anim_wire(body: &walk::Stats, segments: Vec<(u32, manager::Stats)>) -> Vec<Arm> {
    let mut arms = vec![Arm {
        name: "anim.graph_walk".to_owned(),
        answered: body.native,
        declined: body.declined.iter().sum(),
        ..Arm::default()
    }];
    arms.extend(segments.into_iter().map(|(addr, s)| Arm {
        name: format!("anim.manager.{addr:08x}"),
        checked: s.matched + s.differed,
        matched: s.matched,
        mismatched: s.differed,
        answered: s.answered,
        declined: s.declined,
    }));
    arms
}

/// Sampled ticks with voices are checked; quiet and skipped ticks are not.
fn sound_wire(s: &voices::Stats) -> Arm {
    Arm {
        name: "sound.voices".to_owned(),
        checked: s.matched + s.mismatched,
        matched: s.matched,
        mismatched: s.mismatched,
        answered: s.answered,
        declined: s.declined,
    }
}

fn collision_arms(s: &world::Stats) -> Vec<Arm> {
    let vector = |name: &str, p: &world::VectorParity| Arm {
        name: name.to_owned(),
        checked: p.matched + p.mismatched,
        matched: p.matched,
        mismatched: p.mismatched,
        answered: 0,
        declined: p.unverified.iter().sum(),
    };
    vec![
        vector("collision.vector", &s.vector),
        vector("collision.context", &s.context),
        vector("collision.instance", &s.instance),
        vector("collision.query", &s.query),
        vector("collision.visit", &s.visit),
        vector("collision.flood", &s.flood),
        Arm {
            name: "collision.ray".to_owned(),
            checked: s.ray.matched + s.ray.mismatched,
            matched: s.ray.matched,
            mismatched: s.ray.mismatched,
            answered: 0,
            declined: s.ray.unverified.iter().sum(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arm(name: &str, checked: u64, mismatched: u64) -> Arm {
        Arm {
            name: name.into(),
            checked,
            matched: checked - mismatched,
            mismatched,
            ..Arm::default()
        }
    }

    fn registry() -> &'static Shadow {
        Box::leak(Box::new(Shadow::new()))
    }

    #[test]
    fn an_idle_process_reads_zeros_and_is_not_clean() {
        let shadow = registry();
        shadow.register("sim.seats_a", sim_bridge::sim_seats::GROUP, "A", 0x1000, true);
        let r = read(&Switches::default(), shadow);
        let names: Vec<_> = r.subsystems.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["physics", "objects", "anim", "collision", "seats", "effects", "sound", "effects-parts"]);
        for s in &r.subsystems {
            if ["objects", "anim", "effects", "effects-parts"].contains(&s.name.as_str()) {
                assert_eq!(s.switch, "", "{}: no switch", s.name);
                assert!(s.on, "{}: always on", s.name);
            } else {
                assert_eq!(s.switch, format!("impl.sim.native.{}", s.name));
            }
            assert!(!s.clean, "{}", s.name);
            for a in &s.arms {
                assert_eq!(
                    (a.checked, a.matched, a.mismatched, a.answered, a.declined),
                    (0, 0, 0, 0, 0),
                    "{}",
                    a.name
                );
            }
        }
        let seats = &r.subsystems[4];
        assert_eq!(seats.arms.len(), 5 + leaf::Leaf::ALL.len());
        assert_eq!(seats.arms[0].name, "sim.seats_a");
        assert_eq!(seats.arms[1].name, "seats.aim");
        assert_eq!(seats.arms[2].name, "seats.device");
        assert_eq!(seats.arms[3].name, "seats.respawn");
        assert_eq!(seats.arms[4].name, "seats.player-objects");
        assert_eq!(seats.arms[5].name, "seats.leaf.visible");
        assert!(r.subsystems[5].arms.is_empty());
    }

    #[test]
    fn the_switch_reads_from_the_features() {
        let s = Switches::parse("impl.sim.native.physics=on").unwrap();
        let r = read(&s, registry());
        assert!(r.subsystems[0].on);
        assert!(!r.subsystems[3].on, "collision");
    }

    #[test]
    fn a_mismatch_clears_clean_and_a_match_sets_it() {
        let switch = || "impl.sim.native.x".to_owned();
        let clean = verdict("x", switch(), true, vec![arm("a", 5, 0), arm("b", 0, 0)]);
        assert!(clean.clean);
        let dirty = verdict("x", switch(), true, vec![arm("a", 5, 0), arm("b", 3, 1)]);
        assert!(!dirty.clean);
        assert!(!verdict("x", switch(), true, vec![arm("a", 0, 0)]).clean);
    }

    #[test]
    fn sources_map_onto_the_arm_fields() {
        let o = objects_wire(objects::Status { answered: 7, ported_kind: 2, ported_float: 1 });
        assert_eq!((o.checked, o.answered, o.declined), (0, 10, 0));
        let f = function_wire(function::Status {
            checked: 8,
            matched: 8,
            mismatched: 0,
            answered: 5,
            declined: [1, 2, 3, 0, 0, 1],
            dropped: 2,
        });
        assert_eq!((f.name.as_str(), f.checked, f.answered, f.declined), ("objects.function", 8, 5, 9));
        let mut body = walk::Stats {
            native: 3,
            ..Default::default()
        };
        body.declined[0] = 2;
        let seg = manager::Stats {
            matched: 5,
            differed: 1,
            declined: 2,
            answered: 9,
            ..Default::default()
        };
        let arms = anim_wire(&body, vec![(0x10100, seg)]);
        assert_eq!((arms[0].checked, arms[0].answered, arms[0].declined), (0, 3, 2));
        assert_eq!(arms[1].name, "anim.manager.00010100");
        assert_eq!((arms[1].checked, arms[1].mismatched, arms[1].declined, arms[1].answered), (6, 1, 2, 9));
        let mut stats = world::Stats::default();
        stats.vector.matched = 2;
        stats.ray.mismatched = 1;
        let c = collision_arms(&stats);
        assert_eq!(c.len(), 7);
        assert_eq!((c[0].checked, c[6].checked, c[6].mismatched), (2, 1, 1));
        let w = walk_wire(vec![native_walk::Stats { entry: 0xbd090, segments: 4, refused: [2], ..Default::default() }]);
        assert_eq!(w[0].name, "objects.walk.000bd090");
        assert_eq!((w[0].checked, w[0].mismatched, w[0].answered, w[0].declined), (0, 0, 4, 2));
        let e = effects_parts(
            EffectParts { returned: 4, other: 2, damage: 3, object: 9, decal: 5, ..EffectParts::default() },
            vec![(0x1794e0, segment_arm::Stats { answered: 2, float_env: 1 })],
        );
        let names: Vec<&str> = e.arms.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "effects.damage",
                "effects.object",
                "effects.sound",
                "effects.character",
                "effects.coln",
                "effects.decal",
                "effects.light",
                "effects.parts",
                "effects.body.001794e0"
            ]
        );
        assert_eq!(e.arms[5].answered, 5);
        assert_eq!((e.arms[0].checked, e.arms[0].answered, e.arms[0].declined), (0, 3, 0));
        assert_eq!((e.arms[1].answered, e.arms[7].answered, e.arms[7].declined), (9, 6, 0));
        assert_eq!((e.arms[8].checked, e.arms[8].answered, e.arms[8].declined), (0, 2, 1));
        assert!(!e.clean);
        let v = sound_wire(&voices::Stats { matched: 4, mismatched: 1, quiet: 9, declined: 2, answered: 7, ..Default::default() });
        assert_eq!((v.checked, v.mismatched, v.answered, v.declined), (5, 1, 7, 2));
    }

    #[test]
    fn the_wire_shape_is_stable() {
        let v = serde_json::to_value(SimArms {
            subsystems: vec![verdict("seats", "impl.sim.native.seats".into(), true, vec![arm("a", 2, 0)])],
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"subsystems": [{
                "name": "seats", "switch": "impl.sim.native.seats", "on": true, "clean": true,
                "arms": [{"name": "a", "checked": 2, "matched": 2, "mismatched": 0, "answered": 0, "declined": 0}]
            }]})
        );
    }

    #[test]
    fn the_method_is_registered_and_answers_without_a_guest() {
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
        a.submit(conn, r#"{"jsonrpc":"2.0","id":1,"method":"session.methods"}"#);
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(line.contains("sim.arms"), "{line}");
        a.submit(conn, r#"{"jsonrpc":"2.0","id":2,"method":"sim.arms"}"#);
        a.between_slices(&mut NoGuest, 0);
        let line = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&line).unwrap();
        let arms: SimArms = serde_json::from_value(reply["result"].clone()).unwrap();
        assert_eq!(arms.subsystems.len(), 8);
    }
}
