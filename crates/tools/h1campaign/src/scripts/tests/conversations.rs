use crate::ai::conversations::{Conversation, Line, Participant, SpeakerScopes};
use hs::h1_conversation::{BUILTINS, Op};

use super::*;
use crate::scripts::compile::{Cutscenes, compile_with_conversations};

#[test]
fn conversation_calls_reference_the_generated_worker_and_state() {
    const CONVERSATION: u16 = 14;
    const UNIT_NAME: u16 = 15;
    const REAL: u16 = 6;
    const LONG: u16 = 8;
    const H2_OBJECT: u16 = 50;
    const H2_UNIT_NAME: u16 = 57;
    const H2_SOUND: u16 = 32;

    let mut source_types = h1_types();
    source_types.extend(["conversation".into(), "unit_name".into()]);
    let builtins = [
        ("ai_conversation", BOOL),
        ("ai_conversation_stop", VOID),
        ("ai_conversation_status", SHORT),
        ("ai_conversation_line", SHORT),
        ("ai_conversation_advance", VOID),
    ];
    let mut functions: Vec<_> = builtins
        .into_iter()
        .map(|(name, ty)| f(name, ty, ORDINARY, &[CONVERSATION]))
        .collect();
    functions.push(f("object_get_health", REAL, ORDINARY, &[OBJECT]));
    let source = Catalog::new(
        functions,
        source_types,
        ["startup", "dormant", "continuous", "static", "stub"]
            .map(String::from)
            .to_vec(),
    )
    .unwrap();

    let mut target = Target {
        types: (0..62).map(|i| format!("type_{i}")).collect(),
        ..Default::default()
    };
    target.types[H2_OBJECT as usize] = "object".into();
    target.types[H2_UNIT_NAME as usize] = "unit_name".into();
    target.types[H2_SOUND as usize] = "sound".into();
    target.types[56] = "object_name".into();
    for (i, name) in h1_types().iter().take(11).enumerate() {
        target.types[i] = name.clone();
    }
    for (name, index, return_type, parameters, special) in [
        ("begin", 0, PASS, &[][..], true),
        ("if", 2, PASS, &[], true),
        ("set", 4, PASS, &[], true),
        ("and", 5, BOOL, &[], true),
        ("or", 6, BOOL, &[], true),
        ("+", 7, REAL, &[], true),
        ("=", 13, BOOL, &[], true),
        (">", 15, BOOL, &[], true),
        ("<", 16, BOOL, &[], true),
        ("<=", 18, BOOL, &[], true),
        ("sleep", 19, VOID, &[], true),
        ("sleep_until", 21, VOID, &[], true),
        ("not", 26, BOOL, &[BOOL], false),
        ("object_get_health", 69, REAL, &[H2_OBJECT], false),
        (BUILTINS[0].0, BUILTINS[0].1, VOID, &[LONG; 4], false),
        ("cs_stack_command_script", 367, VOID, &[19, 21], false),
        ("cs_command_script_running", 370, BOOL, &[19, 21], false),
        ("cs_command_script_queued", 371, BOOL, &[19, 21], false),
        ("cs_look_object", 394, VOID, &[BOOL, H2_OBJECT], false),
        ("cs_face_object", 400, VOID, &[BOOL, H2_OBJECT], false),
        (
            "cs_approach",
            429,
            VOID,
            &[H2_OBJECT, REAL, REAL, REAL],
            false,
        ),
    ] {
        target.builtins.insert(
            name.into(),
            H2Builtin {
                index,
                return_type,
                parameters: parameters.to_vec(),
                special,
            },
        );
    }

    let mut b = Builder::default();
    let mut scripts = Vec::new();
    for (i, (name, ty)) in builtins.into_iter().enumerate() {
        let scene = b.atom(CONVERSATION, CONVERSATION, "test_scene", 0xffff_0000);
        let expr = b.call(i as u16, ty, name, &[scene]);
        scripts.push((name.strip_prefix("ai_").unwrap(), 3, ty, expr));
    }
    let unit = b.atom(UNIT_NAME, OBJECT, "new_name", 0xffff_0001);
    let expr = b.call(5, REAL, "object_get_health", &[unit]);
    scripts.push(("actor_health", 3, REAL, expr));
    let t = read(&scenario(&b, &scripts, &[])).unwrap();
    let sounds = [
        Some("sound\\first".into()),
        None,
        Some("sound\\second".into()),
        None,
        None,
        None,
    ];
    let scenes = [Conversation {
        name: "test_scene".into(),
        flags: 0x20,
        trigger_distance: 0.0,
        run_to_player_distance: 3.0,
        participants: vec![Participant {
            flags: 0,
            selection: 0,
            actor_type: 7,
            existing_name: Some("actor".into()),
            assigned_name: Some("new_name".into()),
            voices: [3, -1, 8, -1, -1, -1],
            encounter: None,
        }],
        lines: vec![Line {
            flags: 0x18,
            speaker: 0,
            addressee_kind: 1,
            addressee: -1,
            pause_after: 0.5,
            sounds,
        }],
    }];
    let table = Table::build(&source, &target);
    let mut out = compile_with_conversations(
        &t,
        &source,
        &table,
        &target,
        Missing::Stub,
        Cutscenes::Play,
        &|_, _| true,
        &[],
        &scenes,
        &SpeakerScopes::default(),
    )
    .unwrap();
    assert!(out.missing.is_empty());
    let left = out.resolve_hooks(|h| match h.token.as_str() {
        "actor" => Some(0xffff_0000),
        "new_name" => Some(0xffff_0001),
        "sound\\first" => Some(0x1234_0002),
        "sound\\second" => Some(0x1234_0003),
        _ => None,
    });
    assert!(left.is_empty());
    let bytes = out.scenario_bytes(BASE);
    let scnr = hs::Scenario::decode(&bytes, BASE).unwrap();
    scnr.validate_graph().unwrap();
    assert_eq!(scnr.reachable(scnr.roots()).unwrap().len(), out.nodes.len());

    // Generated globals with the same literal initial value share its node;
    // each keeps its own definition and value cell.
    let state_root = |purpose: &str| {
        scnr.globals()
            .iter()
            .find(|g| g.name().unwrap().starts_with(&format!("__h1cv0_{purpose}_")))
            .unwrap()
            .root()
    };
    assert_eq!(state_root("status"), state_root("count"));
    assert_eq!(state_root("request"), state_root("working"));
    assert_ne!(state_root("status"), state_root("line"));
    assert_ne!(state_root("status"), state_root("request"));

    let root = |name| scnr.node(scnr.script(name).unwrap().root()).unwrap();
    for name in ["conversation", "conversation_stop", "conversation_advance"] {
        assert!(
            root(name).script_call(),
            "{name} should call a generated script"
        );
    }
    for name in ["conversation_status", "conversation_line"] {
        assert!(
            root(name).global(),
            "{name} should read the worker's live global"
        );
    }
    let health = root("actor_health");
    let head = scnr.node(Datum(health.data())).unwrap();
    let alias = scnr.node(head.next()).unwrap();
    assert_eq!(
        alias.opcode(),
        2,
        "use the captured actor with a placement fallback"
    );
    assert_eq!(scnr.scripts().iter().filter(|s| s.kind() == 2).count(), 1);
    let ops: std::collections::HashSet<_> = scnr
        .nodes()
        .iter()
        .filter(|n| !n.atom() && !n.script_call() && n.opcode() == BUILTINS[0].1)
        .map(|n| {
            let head = scnr.node(Datum(n.data())).unwrap();
            scnr.node(head.next()).unwrap().data()
        })
        .collect();
    for op in [
        Op::StartSound,
        Op::PollSound,
        Op::StopSound,
        Op::Actor,
        Op::Focus,
        Op::Score,
        Op::RememberVoice,
        Op::CancelCommand,
    ] {
        assert!(ops.contains(&(op as u32)));
    }
    let command = |name: &str| {
        scnr.scripts()
            .iter()
            .enumerate()
            .find(|(_, script)| {
                script.kind() == 5
                    && script
                        .name()
                        .unwrap()
                        .starts_with(&format!("__h1cv0_{name}_"))
            })
            .unwrap()
    };
    for (index, script, expected) in [
        (command("approach").0, command("approach").1, &[429u16][..]),
        (command("face").0, command("face").1, &[394u16, 400][..]),
    ] {
        let builtins = scnr.reached_builtins([script.root()]).unwrap();
        for opcode in expected {
            assert!(builtins.contains(opcode));
        }
        assert!(
            scnr.nodes()
                .iter()
                .any(|n| n.atom() && n.opcode() == 21 && n.data() as u16 == index as u16)
        );
    }
    let actor = out.hooks.iter().find(|h| h.token == "actor").unwrap();
    assert_eq!(scnr.nodes()[actor.slot as usize].value_type(), H2_OBJECT);
    for tag in [0x1234_0002, 0x1234_0003] {
        assert!(
            scnr.nodes()
                .iter()
                .any(|n| n.atom() && n.opcode() == H2_SOUND && n.data() == tag)
        );
    }
}
