use super::*;
use crate::compile::{Cutscenes, compile_with_ai};
use h1ai::control::{Action, Control, Operation, Platoon, Request};

#[test]
fn platoon_actions_share_a_static_body_and_use_a_persistent_switch() {
    let mut types = h1_types();
    types.resize(18, "unused".into());
    types[17] = "ai".into();
    // Keep the same type names as the invented target, each at one index.
    types[usize::from(AI)] = "old_ai".into();
    let h1 = Catalog::new(
        vec![
            f("begin", PASS, 0x2000, &[]),
            f("ai_maneuver_enable", VOID, ORDINARY, &[17, BOOL]),
            f("ai_retreat", VOID, ORDINARY, &[17]),
        ],
        types,
        vec!["startup".into()],
    )
    .unwrap();
    let mut h2 = h2_target();
    for (name, index, special) in [
        ("set", 4, true),
        ("if", 2, true),
        ("ai_migrate", 308, false),
    ] {
        h2.builtins.insert(
            name.into(),
            H2Builtin {
                index,
                return_type: if special { PASS } else { VOID },
                parameters: if special { vec![] } else { vec![H2_AI; 2] },
                special,
            },
        );
    }
    let mut builder = Builder::default();
    let mut ai = || builder.atom(17, 17, "enc", 0);
    let enable = ai();
    let first = ai();
    let second = ai();
    let disabled = builder.atom(BOOL, BOOL, "false", 0);
    let disable = builder.call(1, VOID, "ai_maneuver_enable", &[enable, disabled]);
    let first = builder.call(2, VOID, "ai_retreat", &[first]);
    let second = builder.call(2, VOID, "ai_retreat", &[second]);
    let main = builder.call(0, VOID, "begin", &[disable, first, second]);
    let bytes = scenario(&builder, &[("main", 0, VOID, main)], &[]);
    let tables = read(&bytes).unwrap();
    let platoon = Platoon {
        encounter: 0,
        index: 0,
    };
    let enable = Request {
        operation: Operation::ManeuverEnable,
        reference: 0,
        target: None,
    };
    let retreat = Request {
        operation: Operation::Maneuver,
        ..enable
    };
    let mut controls = Control::default();
    controls
        .actions
        .insert(enable, vec![Action::Enable(platoon)]);
    controls.actions.insert(
        retreat,
        vec![
            Action::Migrate {
                from: 0,
                to: 1,
                platoon: Some(platoon),
            },
            Action::Migrate {
                from: 2,
                to: 3,
                platoon: Some(platoon),
            },
        ],
    );
    controls.switches.insert(platoon);
    let compiled = compile_with_ai(
        &tables,
        &h1,
        &Table::build(&h1, &h2),
        &h2,
        Missing::Stub,
        Cutscenes::Play,
        &|_, _| true,
        &[],
        Some(&controls),
    )
    .unwrap();
    assert!(compiled.missing.is_empty());
    let scenario = hs::Scenario::decode(&compiled.scenario_bytes(BASE), BASE).unwrap();
    assert_eq!(scenario.globals().len(), 1);
    assert_eq!(scenario.scripts().len(), 2);
    assert_eq!(scenario.scripts()[1].kind(), 3);
    let calls: Vec<_> = scenario.nodes().iter().filter(|n| !n.atom()).collect();
    for opcode in [2, 308] {
        assert_eq!(calls.iter().filter(|n| n.opcode() == opcode).count(), 2);
    }
    assert_eq!(
        calls
            .iter()
            .filter(|n| n.script_call() && n.opcode() == 1)
            .count(),
        2
    );
    assert_eq!(
        scenario.reachable(scenario.roots()).unwrap().len(),
        scenario.nodes().len()
    );
}

#[test]
fn rebuilt_transport_seat_substrings_keep_the_row_and_side() {
    use crate::seats::h2_label_matches;

    assert!(h2_label_matches("riderLM", "p_riderlm"));
    assert!(!h2_label_matches("riderLM", "p_riderlb"));
    assert!(!h2_label_matches("riderLM", "p_riderrm"));
}

#[test]
fn a_follow_radius_is_folded_only_when_its_setter_precedes_the_only_follow() {
    let mut types = h1_types();
    types.resize(18, "unused".into());
    types[17] = "ai".into();
    let h1 = Catalog::new(
        vec![
            f("begin", PASS, 0x2000, &[]),
            f("ai_follow_distance", VOID, ORDINARY, &[17, 6]),
            f("ai_follow_target_players", VOID, ORDINARY, &[17]),
        ],
        types,
        vec!["startup".into()],
    )
    .unwrap();
    for setter_first in [true, false] {
        let mut builder = Builder::default();
        let before = builder.atom(17, 17, "enc", 0);
        let after = builder.atom(17, 17, "enc", 0);
        let radius = builder.atom(6, 6, "5", 5.0_f32.to_bits());
        let setter = builder.call(1, VOID, "ai_follow_distance", &[before, radius]);
        let follower = builder.call(2, VOID, "ai_follow_target_players", &[after]);
        let children = if setter_first {
            [setter, follower]
        } else {
            [follower, setter]
        };
        let root = builder.call(0, VOID, "begin", &children);
        let bytes = scenario(&builder, &[("main", 0, VOID, root)], &[]);
        let tables = read(&bytes).unwrap();
        let controls = crate::aicontrol::requests(&tables, &h1);
        assert_eq!(
            controls
                .iter()
                .any(|r| r.operation == Operation::FollowDistance),
            setter_first,
        );
    }
}
