//! Check the installed scripts in the same map as the rebuilt cryotube.
use h1campaign::scenario::ScriptReport;
use hs::{Datum, Expression, Scenario};

fn tutorial(name: &str) -> bool {
    name.starts_with("tutorial_")
        || name.starts_with("test_")
        || matches!(
            name,
            "title_training" | "fast_setup" | "player0" | "player1"
        )
}

fn call(s: &Scenario, d: Datum) -> (&str, u16, Vec<Datum>) {
    let Expression::Call {
        name,
        function,
        children,
        ..
    } = s.expression(d).unwrap()
    else {
        panic!("expected a builtin at {d:?}");
    };
    (name, function, children.collect::<hs::Result<_>>().unwrap())
}

pub(super) fn check(s: &Scenario, report: &ScriptReport) {
    for missing in report
        .missing_builtins
        .iter()
        .chain(&report.missing_globals)
    {
        assert!(!tutorial(&missing.definition), "tutorial stub: {missing:?}");
    }
    for unindexed in &report.unindexed {
        let (script, _) = unindexed.split_once(": ").unwrap();
        assert!(!tutorial(script), "unresolved tutorial call: {unindexed}");
    }

    // Check dispatch and type conversions against the executable, including
    // the compiler-generated helpers called from a tutorial definition.
    let mut iso = super::h2_iso();
    let entry = iso.find("default.xbe").unwrap();
    let bytes = iso.read(&entry).unwrap();
    let catalog = hs::Catalog::from_xbe(&xbe::Xbe::parse(&bytes).unwrap()).unwrap();
    let roots = s
        .scripts()
        .iter()
        .filter(|script| tutorial(script.name().unwrap()))
        .map(|script| script.root());
    let slots: std::collections::BTreeSet<_> = s
        .reachable(roots)
        .unwrap()
        .into_iter()
        .map(|d| d.slot())
        .collect();
    for issue in s.unrunnable(&catalog).unwrap() {
        assert!(
            !slots.contains(&issue.slot),
            "unrunnable tutorial: {issue:?}"
        );
    }

    // Check the actual wait condition, its cadence, and the surrounding
    // resets, not just the presence of slot 488 somewhere in the map.
    let (_, _, body) = call(s, s.script("tutorial_action").unwrap().root());
    let pos = body
        .iter()
        .position(|&d| {
            matches!(
                s.expression(d).unwrap(),
                Expression::Call {
                    name: "sleep_until",
                    ..
                }
            )
        })
        .unwrap();
    assert_eq!(call(s, body[pos - 1]).1, 479);
    assert_eq!(call(s, body[pos + 1]).1, 479);
    let (_, _, wait) = call(s, body[pos]);
    let [condition, interval] = wait.as_slice() else {
        panic!("unexpected cryo wait")
    };
    let (name, op, args) = call(s, *condition);
    assert_eq!(
        (name, op, args.len()),
        ("player_action_test_action", 488, 0)
    );
    assert_eq!(s.node(*interval).unwrap().data() as i16, 1);

    let &exit = body[pos + 2..]
        .iter()
        .find(|&&d| {
            matches!(
                s.expression(d).unwrap(),
                Expression::Call { function: 227, .. }
            )
        })
        .expect("scripted dismount after the action test");
    let (_, _, exit) = call(s, exit);
    let [rider, mode] = exit.as_slice() else {
        panic!("unexpected exit arguments")
    };
    let Expression::ScriptCall { script, .. } = s.expression(*rider).unwrap() else {
        panic!("expected the scripted player as the rider");
    };
    assert_eq!(script.name().unwrap(), "player0");
    assert_eq!(s.node(*mode).unwrap().data() as i16, 1);

    for (script, required) in [
        ("tutorial_setup", &[216][..]),
        ("tutorial_looking", &[479, 491, 492, 493, 494, 495]),
        ("test_looking_choose_cycle", &[479, 489, 490]),
        ("tutorial_moving_1", &[33]),
        ("tutorial_moving_2", &[33]),
        ("tutorial_moving_jump", &[33]),
        ("tutorial_moving_crouch", &[33]),
        ("tutorial_hud_shield", &[33, 260, 479, 489]),
        ("tutorial_first_contact", &[33, 254, 256, 257, 263]),
    ] {
        let nodes = s.reachable([s.script(script).unwrap().root()]).unwrap();
        for &op in required {
            assert!(
                nodes
                    .iter()
                    .map(|&d| s.node(d).unwrap())
                    .any(|n| !n.atom() && !n.script_call() && n.opcode() == op),
                "{script} lost builtin {op}"
            );
        }
    }
}
