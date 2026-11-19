use super::*;

fn datum(slot: usize) -> Datum {
    Datum(((0x8000 + slot as u32) << 16) | slot as u32)
}
fn name(s: &str) -> [u8; 32] {
    let mut b = [0; 32];
    b[..s.len()].copy_from_slice(s.as_bytes());
    b
}
fn atom(slot: usize, ty: u16, text: u32, data: u32) -> Node {
    Node {
        salt: datum(slot).salt(),
        opcode: ty,
        value_type: ty,
        flags: 9,
        next: Datum::NULL,
        string_offset: text,
        data,
    }
}
fn fixture() -> (Scenario, Catalog) {
    let strings = b"sequence\0emit\0hello \"world\"\n\x93\0worker\0flag\0".to_vec();
    let offset = |s: &[u8]| strings.windows(s.len()).position(|w| w == s).unwrap() as u32;
    let mut nodes = vec![
        atom(0, 4, 0, datum(1).0),
        atom(1, 2, 0, 0),
        atom(2, 4, 0, datum(3).0),
        atom(3, 2, offset(b"emit"), 0),
        atom(4, 9, offset(b"hello"), 0xdeadbeef),
        atom(5, 6, 0, datum(6).0),
        atom(6, 2, offset(b"worker"), 0),
        atom(7, 6, u32::MAX, (-0.0f32).to_bits()),
        atom(8, 5, u32::MAX, 1),
        Node::decode(&[0xba; 20]).unwrap(),
    ];
    nodes[0].opcode = 0;
    nodes[0].flags = 8;
    nodes[1].opcode = 0;
    nodes[1].next = datum(2);
    nodes[2].opcode = 1;
    nodes[2].flags = 8;
    nodes[2].next = datum(5);
    nodes[3].opcode = 1;
    nodes[3].next = datum(4);
    nodes[5].opcode = 1;
    nodes[5].flags = 10;
    nodes[6].opcode = 1;
    nodes[9].salt = 0;
    let scenario = Scenario {
        nodes,
        strings,
        scripts: vec![
            Script {
                name_bytes: name("entry"),
                kind: 0,
                return_type: 4,
                root: datum(0),
            },
            Script {
                name_bytes: name("worker"),
                kind: 3,
                return_type: 6,
                root: datum(7),
            },
        ],
        globals: vec![Global {
            name_bytes: name("flag"),
            value_type: 5,
            reserved: 0x1234,
            root: datum(8),
        }],
        references: vec![],
    };
    let mut types: Vec<_> = (0..62).map(|i| format!("invented_type_{i}")).collect();
    for (i, n) in [(4, "void"), (5, "boolean"), (6, "real"), (9, "string")] {
        types[i] = n.into();
    }
    let catalog = Catalog {
        functions: (0..2)
            .map(|_| Builtin {
                descriptor: 0,
                return_type: 4,
                flags: 2,
                evaluate: 1,
                usage: None,
                parameters: vec![],
                name: None,
            })
            .collect(),
        types,
        script_kinds: [
            "startup",
            "dormant",
            "continuous",
            "static",
            "stub",
            "command_script",
        ]
        .map(str::to_owned)
        .to_vec(),
        external_global_types: vec![5],
    };
    (scenario, catalog)
}
#[test]
fn marked_vehicle_count_keeps_the_retail_grenade_count_name() {
    use crate::h1_ai_slots::{CONTROL_TOKEN, Operation, VEHICLE_COUNT};

    let (mut scenario, _) = fixture();
    let name_offset = scenario.strings.len() as u32;
    scenario.strings.extend_from_slice(b"unit_get_total_grenade_count\0");
    let marker_offset = scenario.strings.len() as u32;
    scenario.strings.extend_from_slice(CONTROL_TOKEN.as_bytes());
    scenario.strings.push(0);

    for node in &mut scenario.nodes[..4] {
        node.opcode = VEHICLE_COUNT;
    }
    scenario.nodes[1].string_offset = marker_offset;
    scenario.nodes[1].data = Operation::VehicleCount.marker();
    scenario.nodes[3].string_offset = name_offset;
    assert_eq!(scenario.builtin_names().unwrap()[&VEHICLE_COUNT], "unit_get_total_grenade_count");

    scenario.nodes[1].data = 0;
    assert!(scenario.builtin_names().is_err());
}

fn payload(s: &Scenario) -> (Vec<u8>, u32) {
    let base = 0x81000000;
    let mut b = vec![0; 0x240];
    let tables = [
        (
            0x238,
            s.nodes.len(),
            s.nodes.iter().flat_map(|n| n.encode()).collect::<Vec<_>>(),
        ),
        (0x1b0, s.strings.len(), s.strings.clone()),
        (
            0x1b8,
            s.scripts.len(),
            s.scripts.iter().flat_map(|n| n.encode()).collect(),
        ),
        (
            0x1c0,
            s.globals.len(),
            s.globals.iter().flat_map(|n| n.encode()).collect(),
        ),
        (
            0x1c8,
            s.references.len(),
            s.references.iter().flat_map(|n| n.encode()).collect(),
        ),
    ];
    for (field, count, data) in tables {
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
        let address = base + b.len() as u32;
        b[field..field + 4].copy_from_slice(&(count as u32).to_le_bytes());
        b[field + 4..field + 8].copy_from_slice(&address.to_le_bytes());
        b.extend(data);
    }
    (b, base)
}

#[test]
fn codec_symmetry_keeps_dead_slots_padding_and_references() {
    let (mut s, c) = fixture();
    s.scripts[0].name_bytes[31] = 0xcd;
    s.references.push(Reference {
        group: map::FourCC(*b"fake"),
        id: map::TagId(0x12340002),
    });
    let (bytes, base) = payload(&s);
    let decoded = Scenario::decode(&bytes, base).unwrap();
    decoded.verify_codec_symmetry(&bytes, base).unwrap();
    assert_eq!(decoded.nodes, s.nodes);
    assert_eq!(decoded.scripts, s.scripts);
    assert_eq!(decoded.references, s.references);
    let stats = decoded.validate(&c).unwrap();
    assert_eq!(
        (stats.slots, stats.live_nodes, stats.free_slots),
        (10, 9, 1)
    );
    assert_eq!(stats.unrooted_live_nodes, 0);
}
#[test]
fn source_node_count_preserves_bits_bytes_and_script_calls() {
    let (s, mut c) = fixture();
    c.observe(&s).unwrap();
    assert_eq!(c.function(1).unwrap().name.as_deref(), Some("emit"));
    let output = s.decompile(&c, None).unwrap();
    assert!(output.source.contains("hello \\\"world\\\"\\n\\x93"));
    assert!(output.source.contains("-0.0"));
    assert!(output.source.contains("(worker)"));
    let parsed = source_counts(&output.source).unwrap();
    assert_eq!((parsed.scripts, parsed.globals, parsed.nodes), (2, 1, 9));
    assert_eq!(output.emitted_nodes, parsed.nodes);
    assert_eq!(
        s.reached_builtins([datum(0)])
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        [0, 1]
    );
    let selected = s.decompile(&c, Some("worker")).unwrap();
    assert_eq!(
        source_counts(&selected.source).unwrap(),
        SourceCounts {
            scripts: 1,
            globals: 0,
            nodes: 1
        }
    );
    assert!(s.decompile(&c, Some("missing")).is_err());
}
#[test]
fn stale_salts_free_links_and_cycles_are_rejected() {
    for bad in [Datum(datum(4).0 ^ 0x10000), datum(9), datum(100), datum(0)] {
        let (mut s, _) = fixture();
        s.nodes[3].next = bad;
        assert!(s.validate_graph().is_err(), "accepted {bad:?}");
    }
    let (mut s, _) = fixture();
    s.nodes[0].data = datum(4).0;
    assert!(s.validate_graph().is_err());
}
#[test]
fn corrupt_tables_and_legacy_blob_are_rejected() {
    let (s, _) = fixture();
    let (bytes, base) = payload(&s);
    for (field, value) in [
        (0x238, u32::MAX),
        (0x23c, base - 4),
        (0x23c, base + 1),
        (0x1a8, 1),
    ] {
        let mut bad = bytes.clone();
        bad[field..field + 4].copy_from_slice(&value.to_le_bytes());
        assert!(Scenario::decode(&bad, base).is_err());
    }
    assert!(Scenario::decode(&bytes[..0x23f], base).is_err());
    assert!(Node::decode(&[0; 19]).is_err());
}
#[test]
fn invalid_symbol_offsets_types_and_opcodes_are_rejected() {
    let (s, c) = fixture();
    let mut bad = s.clone();
    bad.nodes[3].string_offset = u32::MAX;
    assert!(bad.validate(&c).is_err());
    let mut bad = s.clone();
    bad.nodes[7].value_type = 100;
    assert!(bad.validate(&c).is_err());
    let mut bad = s.clone();
    bad.nodes[0].opcode = 911;
    bad.nodes[1].opcode = 911;
    assert!(bad.validate(&c).is_err());
    let mut bad = s;
    bad.nodes[5].opcode = 300;
    bad.nodes[6].opcode = 300;
    assert!(bad.validate(&c).is_err());
}
#[test]
fn global_indices_are_bounded_and_name_conflicts_are_transactional() {
    let (mut s, mut c) = fixture();
    s.nodes[8].flags = 13;
    s.nodes[8].string_offset = s.strings.len() as u32 - 5;
    s.nodes[8].data = 0x8000;
    s.validate(&c).unwrap();
    s.nodes[8].data = 0x8001;
    assert!(s.validate(&c).is_err());
    s.nodes[8].data = 0;
    s.validate(&c).unwrap();
    s.nodes[8].data = 1;
    assert!(s.validate(&c).is_err());
    let (s, mut catalog) = fixture();
    catalog.functions[1].name = Some("different".into());
    assert!(catalog.observe(&s).is_err());
    assert!(catalog.functions[0].name.is_none());
    c.observe(&s).unwrap();
}
#[test]
fn deep_source_walk_uses_no_recursion() {
    let (mut s, c) = fixture();
    s.nodes.clear();
    s.globals.clear();
    s.scripts.truncate(1);
    let depth = 3000;
    for i in 0..depth {
        let slot = i * 2;
        let mut call = atom(slot, 4, u32::MAX, datum(slot + 1).0);
        call.flags = 8;
        call.opcode = 0;
        let mut head = atom(slot + 1, 2, 0, 0);
        head.opcode = 0;
        if i + 1 < depth {
            head.next = datum(slot + 2);
        }
        s.nodes.extend([call, head]);
    }
    let out = s.decompile(&c, None).unwrap();
    assert_eq!(source_counts(&out.source).unwrap().nodes, depth * 2);
}
#[test]
fn source_counter_rejects_malformed_lists_and_strings() {
    for bad in [
        "(script startup x (call)",
        "(global boolean x)",
        "(script startup x (() 1))",
        "(script startup x \"oops)",
        "(global long x 1 2)",
    ] {
        assert!(source_counts(bad).is_err(), "accepted {bad}");
    }
}

fn synthetic_xbe() -> Vec<u8> {
    fn put(b: &mut [u8], at: usize, value: u32) {
        b[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut b = vec![0; 0x280400];
    b[..4].copy_from_slice(b"XBEH");
    for (at, value) in [
        (0x104, 0x10000),
        (0x108, 0x400),
        (0x10c, 0x480000),
        (0x118, 0x10200),
        (0x11c, 1),
        (0x120, 0x10300),
        (0x128, 0x002d0aee ^ 0xa8fc57ab),
        (0x208, 0x4d530064),
        (0x2ac, 3),
        (0x300, 4),
        (0x304, 0x200000),
        (0x308, 0x280000),
        (0x30c, 0x400),
        (0x310, 0x280000),
        (0x314, 0x10380),
    ] {
        put(&mut b, at, value);
    }
    let off = |a: u32| (a - 0x200000 + 0x400) as usize;
    b[off(0x20994d)..off(0x20994d) + 7].copy_from_slice(&[0x8b, 0x14, 0x8d, 0xe0, 0x44, 0x47, 0]);
    put(&mut b, off(FUNCTION_COUNT), 911);
    for i in 0..911 {
        put(&mut b, off(FUNCTION_TABLE + i * 4), 0x460000);
    }
    for i in 0..62 {
        put(&mut b, off(TYPE_NAMES + i * 4), 0x460040);
    }
    for i in 0..6 {
        put(&mut b, off(SCRIPT_KIND_NAMES + i * 4), 0x460060);
    }
    for i in 0..0x41d {
        put(&mut b, off(EXTERNAL_GLOBAL_TABLE + i * 4), 0x460020);
    }
    put(&mut b, off(0x460000), 4); // return u16 and flags u16
    put(&mut b, off(0x460004), 0x200010); // synthetic executable pointer
    put(&mut b, off(0x460008), 0x460080); // help string, not name
    put(&mut b, off(0x46000c), 1 | (9 << 16)); // count and first parameter
    put(&mut b, off(0x460020), 5);
    for (address, s) in [
        (0x460040, "invented_type"),
        (0x460060, "invented_kind"),
        (0x460080, "invented argument help"),
    ] {
        b[off(address)..off(address) + s.len()].copy_from_slice(s.as_bytes());
    }
    b
}
#[test]
fn catalog_reads_descriptors_without_mistaking_help_for_names() {
    let bytes = synthetic_xbe();
    let c = Catalog::from_xbe(&xbe::Xbe::parse(&bytes).unwrap()).unwrap();
    assert_eq!(
        (
            c.functions.len(),
            c.types.len(),
            c.external_global_types.len()
        ),
        (911, 62, 1053)
    );
    assert_eq!(c.functions[0].parameters, [9]);
    assert_eq!(
        c.functions[0].usage.as_deref(),
        Some("invented argument help")
    );
    assert_eq!(c.functions[0].name, None);
    for (address, value) in [
        (FUNCTION_COUNT, 912u32),
        (FUNCTION_TABLE, 0xfffffff0),
        (0x460004, 0x600000),
        (0x46000c, 33),
    ] {
        let mut bad = bytes.clone();
        let at = (address - 0x200000 + 0x400) as usize;
        bad[at..at + 4].copy_from_slice(&value.to_le_bytes());
        assert!(Catalog::from_xbe(&xbe::Xbe::parse(&bad).unwrap()).is_err());
    }
}

#[test]
fn huge_and_unterminated_tokens_are_bounded() {
    let (mut s, _) = fixture();
    s.strings = vec![b'x'; MAX_STRING_BYTES];
    s.strings[MAX_STRING_BYTES - 1] = 0;
    assert!(s.string_bytes(0).is_err());
    // A bounded suffix remains accessible without rejecting an entire blob.
    assert_eq!(s.string_bytes((MAX_STRING_BYTES - 2) as u32).unwrap(), b"x");
    s.strings = vec![b'x'; MAX_TOKEN_BYTES];
    assert!(s.string_bytes(0).is_err());
    s.strings.push(0);
    assert_eq!(s.string(0).unwrap().len(), MAX_TOKEN_BYTES);
    s.strings.insert(0, b'x');
    assert!(s.string_bytes(0).is_err());
}
#[test]
fn output_budget_covers_escaping_headers_and_final_delimiters() {
    let (mut s, c) = fixture();
    let expected = s.decompile(&c, None).unwrap().source;
    assert_eq!(
        s.decompile_with_budget(&c, None, expected.len())
            .unwrap()
            .source,
        expected
    );
    assert!(
        s.decompile_with_budget(&c, None, expected.len() - 1)
            .is_err()
    );
    assert!(s.decompile_with_budget(&c, None, 1).is_err());
    s.nodes[4].string_offset = s.strings.len() as u32;
    s.strings.extend(vec![0x93; MAX_TOKEN_BYTES]);
    s.strings.push(0);
    assert!(
        s.decompile_with_budget(&c, None, MAX_TOKEN_BYTES * 2)
            .is_err()
    );
    let out = s.decompile(&c, None).unwrap();
    assert!(out.source.len() > MAX_TOKEN_BYTES * 4);
}
#[test]
fn table_counts_fail_before_decoding_or_span_checks() {
    let (s, _) = fixture();
    let (bytes, base) = payload(&s);
    for (field, limit) in [
        (0x1b8, MAX_DEFINITIONS),
        (0x1c0, MAX_DEFINITIONS),
        (0x1c8, MAX_REFERENCES),
        (0x238, MAX_NODES),
        (0x1b0, MAX_STRING_BYTES),
    ] {
        let mut bad = bytes.clone();
        bad[field..field + 4].copy_from_slice(&((limit + 1) as u32).to_le_bytes());
        let error = Scenario::decode(&bad, base).unwrap_err().to_string();
        assert!(
            error.contains("count") && error.contains("exceeds limit"),
            "{error}"
        );
    }
}
#[test]
fn literal_validation_and_rendering_use_source_type() {
    let (mut s, c) = fixture();
    // Result type numeric, source type string: missing text must be rejected.
    s.nodes[7].opcode = 9;
    assert!(s.validate(&c).is_err());
    assert!(s.decompile(&c, None).is_err());
    s.nodes[7].string_offset = s.nodes[4].string_offset;
    s.validate(&c).unwrap();
    assert!(matches!(
        s.expression(datum(7)).unwrap(),
        Expression::Atom {
            literal: Literal::String(_),
            ..
        }
    ));
    // Result type string, source numeric: obsolete text offsets are not read.
    s.nodes[7].opcode = 6;
    s.nodes[7].value_type = 9;
    s.nodes[7].string_offset = u32::MAX;
    s.validate(&c).unwrap();
    assert!(s.decompile(&c, None).unwrap().source.contains("-0.0"));
}
#[test]
fn globals_reject_arbitrary_high_bits_but_accept_canonical_sign_extension() {
    let (mut s, c) = fixture();
    s.nodes[8].flags = 13;
    s.nodes[8].string_offset = s.strings.len() as u32 - 5;
    for bad in [0x10000, 0x10001, 0x00018000, 0xffff0000, 0x12348000] {
        s.nodes[8].data = bad;
        assert!(s.validate(&c).is_err());
        assert!(s.expression(datum(8)).is_err());
    }
    for valid in [0x8000, 0xffff8000] {
        s.nodes[8].data = valid;
        s.validate(&c).unwrap();
        assert!(matches!(
            s.expression(datum(8)).unwrap(),
            Expression::Global {
                index: GlobalIndex::External(0),
                ..
            }
        ));
    }
}
#[test]
fn ordinary_builtin_arguments_match_descriptor_arity_and_types() {
    let (s, mut c) = fixture();
    c.functions[1].flags = 0;
    c.functions[1].parameters = vec![9];
    s.validate(&c).unwrap();
    for parameters in [vec![], vec![9, 9], vec![6]] {
        c.functions[1].parameters = parameters;
        assert!(s.validate(&c).is_err());
    }
    c.functions[1].flags = 2; // Custom forms do not use this signature contract.
    s.validate(&c).unwrap();
}
#[test]
fn dag_sharing_counts_occurrences_separately_from_unique_nodes() {
    let (mut s, c) = fixture();
    s.nodes[5].flags = 8;
    s.nodes[5].data = datum(3).0; // Same function head and argument as call 2.
    let (bytes, base) = payload(&s);
    let s = Scenario::decode(&bytes, base).unwrap();
    let source = s.decompile(&c, None).unwrap();
    assert_eq!(source.emitted_nodes, source.unique_nodes + 2);
    assert_eq!(
        source_counts(&source.source).unwrap().nodes,
        source.emitted_nodes
    );
    let Expression::Call { children, .. } = s.expression(datum(0)).unwrap() else {
        panic!("call expected")
    };
    assert_eq!(
        children.collect::<Result<Vec<_>>>().unwrap(),
        [datum(2), datum(5)]
    );
}
#[test]
fn typed_views_expose_calls_literals_globals_and_script_targets() {
    let (s, _) = fixture();
    let (bytes, base) = payload(&s);
    let s = Scenario::decode(&bytes, base).unwrap();
    assert_eq!(s.scripts()[0].root(), datum(0));
    assert_eq!(s.nodes()[7].opcode(), 6);
    assert!(matches!(
        s.expression(datum(0)).unwrap(),
        Expression::Call { function: 0, .. }
    ));
    let Expression::ScriptCall {
        script, children, ..
    } = s.expression(datum(5)).unwrap()
    else {
        panic!("script call expected")
    };
    assert_eq!(script.name().unwrap(), "worker");
    assert_eq!(children.count(), 0);
    assert!(matches!(
        s.expression(datum(7)).unwrap(),
        Expression::Atom {
            literal: Literal::RealBits(0x80000000),
            ..
        }
    ));
    assert!(matches!(
        s.expression(datum(8)).unwrap(),
        Expression::Atom {
            literal: Literal::Boolean(true),
            ..
        }
    ));
    assert!(s.expression(datum(9)).is_err());
}
#[test]
fn menu_documentation_is_only_a_runtime_inspection_pointer() {
    let doc = include_str!("../../../docs/formats/haloscript.md");
    let section = doc
        .split("## Menu requirements")
        .nth(1)
        .unwrap()
        .split("## ")
        .next()
        .unwrap();
    assert!(section.len() < 500);
    assert!(section.contains("--builtins"));
    assert!(!section.contains('|') && !section.contains("0x"));
    assert!(!doc.contains("mainmenu_"));
}

/// The cast rules of `0x0020a290` and the table `0x0020a1a0` fills.
#[test]
fn castable_follows_the_retail_converter() {
    use crate::castable;
    // Same type, passthrough, and anything to a name or object type.
    assert!(castable(12, 12));
    assert!(castable(12, 3));
    assert!(castable(56, 9));
    assert!(castable(51, 50));
    assert!(castable(50, 57));
    // The table's entries.
    assert!(castable(4, 9) && castable(4, 61) && !castable(4, 0));
    assert!(castable(5, 9) && castable(5, 6) && !castable(5, 11));
    assert!(castable(6, 7) && castable(6, 44) && castable(6, 49) && !castable(6, 50));
    assert!(castable(7, 6) && castable(8, 7) && !castable(7, 5));
    assert!(castable(31, 19) && castable(31, 55) && castable(31, 61) && !castable(31, 9));
    // b30's fault: no string to unit_seat_mapping (nor string_id: the
    // compiler writes those as string ID literals).
    assert!(!castable(12, 9));
    assert!(!castable(11, 9));
}
