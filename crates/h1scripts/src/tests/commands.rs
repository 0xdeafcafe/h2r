use super::*;

/// Two lists with the same name and conflicting stale operands. All list,
/// atom and point records use H1's wire layout, independent of its reader.
#[test]
fn command_roots_keep_indices_points_and_blocking_order() {
    let mut b = Builder::default();
    let who = b.atom(AI, AI, "squad", 0);
    let list_type = h1_types().len() as u16;
    let which = b.atom(list_type, list_type, "walk", 1);
    let start = b.call(0, VOID, "ai_command_list", &[who, which]);
    let mut bytes = scenario(&b, &[("start", 0, VOID, start)], &[]);
    let mut block = |field: usize, n: usize, stride: usize| {
        let offset = bytes.len();
        bytes.resize(offset + n * stride, 0);
        bytes[field..field + 4].copy_from_slice(&(n as u32).to_le_bytes());
        bytes[field + 4..field + 8].copy_from_slice(&(BASE + offset as u32).to_le_bytes());
        offset
    };
    let lists = block(0x438, 2, 0x60);
    let atoms = block(lists + 0x60 + 0x30, 4, 0x20);
    block(lists + 0x60 + 0x3c, 2, 0x14);
    for i in 0..2 {
        let at = lists + i * 0x60;
        bytes[at..at + 4].copy_from_slice(b"walk");
        bytes[at + 0x2e..at + 0x30].copy_from_slice(&(-1i16).to_le_bytes());
    }
    bytes[atoms..atoms + 2].copy_from_slice(&1u16.to_le_bytes());
    bytes[atoms + 12..atoms + 14].copy_from_slice(&1u16.to_le_bytes());
    bytes[atoms + 0x24..atoms + 0x28].copy_from_slice(&1.25f32.to_le_bytes());

    bytes[atoms + 0x40..atoms + 0x42].copy_from_slice(&4u16.to_le_bytes());
    bytes[atoms + 0x42..atoms + 0x44].copy_from_slice(&3i16.to_le_bytes());
    bytes[atoms + 0x44..atoms + 0x48].copy_from_slice(&0.125f32.to_le_bytes());
    bytes[atoms + 0x60..atoms + 0x62].copy_from_slice(&7u16.to_le_bytes());
    bytes[atoms + 0x64..atoms + 0x68].copy_from_slice(&0.75f32.to_le_bytes());
    bytes[atoms + 0x6c..atoms + 0x6e].copy_from_slice(&1i16.to_le_bytes());
    let t = read(&bytes).unwrap();
    let mut types = h1_types();
    types.push("ai_command_list".into());
    let h1 = Catalog::new(
        vec![f("ai_command_list", VOID, ORDINARY, &[AI, list_type])],
        types,
        vec!["startup".into()],
    )
    .unwrap();
    let mut h2 = h2_target();
    h2.types.resize(62, "unused".into());
    h2.types[H2_AI as usize] = "unused".into();
    h2.types[19] = "ai".into();
    h2.types[21] = "ai_command_script".into();
    h2.types[28] = "point reference".into();
    for (name, index, params) in [
        ("cs_run_command_script", 365, vec![19, 21]),
        ("cs_go_to", 384, vec![28]),
        ("cs_pause", 402, vec![6]),
        ("cs_look", 392, vec![5, 28]),
        ("cs_aim", 395, vec![5, 28]),
        ("cs_move_in_direction", 401, vec![6, 6, 6]),
        ("cs_shoot_point", 405, vec![5, 28]),
        ("cs_abort_on_alert", 438, vec![5]),
        ("cs_enable_targeting", 441, vec![5]),
        ("cs_enable_looking", 442, vec![5]),
        ("cs_enable_dialogue", 444, vec![5]),
    ] {
        h2.builtins.insert(
            name.into(),
            H2Builtin {
                index,
                return_type: VOID,
                parameters: params,
                special: false,
            },
        );
    }
    let tr = Table::build(&h1, &h2);
    assert_eq!(tr.builtins[0].kind, Kind::Adapter);
    let c = compile(&t, &h1, &tr, &h2, Missing::Stub, &[]).unwrap();
    let s = hs::Scenario::decode(&c.scenario_bytes(BASE), BASE).unwrap();
    assert!(c.missing.is_empty());
    assert_eq!(s.scripts().len(), 3);
    assert_eq!(s.scripts()[1].name().unwrap(), "h1_cmd_0");
    assert_eq!(s.scripts()[2].name().unwrap(), "h1_cmd_1");
    assert_eq!(s.scripts()[2].kind(), 5);
    let args = |root: Datum| {
        let head = s.node(Datum(s.node(root).unwrap().data())).unwrap();
        let mut cursor = head.next();
        let mut out = Vec::new();
        while cursor != Datum::NULL {
            out.push(cursor);
            cursor = s.node(cursor).unwrap().next();
        }
        out
    };
    let call = s.scripts()[0].root();
    assert_eq!(s.node(call).unwrap().opcode(), 365);
    assert_eq!(s.node(args(call)[1]).unwrap().data(), 2);
    let actions = args(s.scripts()[2].root());
    let ops: Vec<_> = actions
        .iter()
        .map(|&a| s.node(a).unwrap().opcode())
        .collect();
    let walk = actions.len() - 4;
    assert_eq!(ops[walk..walk + 2], [384, 402]);
    let last = walk + 1;
    assert_eq!(
        s.node(args(actions[last - 1])[0]).unwrap().data(),
        0x0001_0001
    );
    assert_eq!(
        s.node(args(actions[last])[0]).unwrap().data(),
        1.25f32.to_bits()
    );
    let gaze = args(actions[walk + 2]);
    let shot = args(actions[walk + 3]);
    let ops = |nodes: &[Datum]| nodes.iter().map(|d| s.node(*d).unwrap().opcode()).collect::<Vec<_>>();
    assert_eq!(ops(&gaze), [392, 401]);
    assert_eq!(ops(&shot), [405, 401, 405, 395]);
    let data = |d: Datum, i: usize| s.node(args(d)[i]).unwrap().data();
    assert_eq!(data(gaze[1], 0), (-327683.0f32).to_bits());
    assert_eq!(data(gaze[1], 1), 0.125f32.to_bits());
    assert_eq!(data(shot[0], 0), 1);
    assert_eq!(data(shot[1], 0), (-524289.0f32).to_bits());
    assert_eq!(data(shot[1], 1), 0.75f32.to_bits());
    assert_eq!(data(shot[2], 0), 0);
    assert_eq!(data(shot[0], 1), data(shot[2], 1));
}
