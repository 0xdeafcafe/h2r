//! Synthetic tables only: invented scripts, builtins and types.
use hs::Datum;

use crate::catalog::{Catalog, Function};
use crate::compile::{Missing, compile, field};
use crate::decompile::decompile;
use crate::layout::*;
use crate::translate::{Adapter, H2Builtin, Kind, Table, Target};

const BASE: u32 = 0x8000_0000;
const ORDINARY: u32 = 0x1000;

// Halo 1 type indices of the invented catalog (scalars as in Halo 1).
const PASS: u16 = 3;
const VOID: u16 = 4;
const BOOL: u16 = 5;
const SHORT: u16 = 7;
const STRING: u16 = 9;
const AI: u16 = 11;
const OBJECT: u16 = 12;
const ACTOR_VARIANT: u16 = 13;

fn h1_types() -> Vec<String> {
    [
        "unparsed",
        "special form",
        "function name",
        "passthrough",
        "void",
        "boolean",
        "real",
        "short",
        "long",
        "string",
        "script",
        "ai",
        "object",
        "actor_variant",
    ]
    .map(String::from)
    .to_vec()
}

fn f(name: &str, ret: u16, parse: u32, params: &[u16]) -> Function {
    Function {
        name: name.into(),
        return_type: ret,
        parse,
        evaluate: 0,
        parameters: params.to_vec(),
    }
}

/// 0 begin, 1 print, 2 ai_place, 3 !=, 4 game_save_totally_unsafe,
/// 5 gone, 6 objects_attach, 7 actor_thing.
fn h1_catalog() -> Catalog {
    Catalog::new(
        vec![
            f("begin", PASS, 0x2000, &[]),
            f("print", VOID, ORDINARY, &[STRING]),
            f("ai_place", VOID, ORDINARY, &[AI]),
            f("!=", BOOL, 0x3000, &[]),
            f("game_save_totally_unsafe", VOID, ORDINARY, &[]),
            f("gone", BOOL, ORDINARY, &[SHORT]),
            f("objects_attach", VOID, ORDINARY, &[OBJECT, STRING]),
            f("actor_thing", VOID, ORDINARY, &[ACTOR_VARIANT]),
        ],
        h1_types(),
        ["startup", "dormant", "continuous", "static", "stub"]
            .map(String::from)
            .to_vec(),
    )
    .unwrap()
}

// Halo 2 indices of the invented target.
const H2_STRING_ID: u16 = 11;
const H2_AI: u16 = 12;
const H2_OBJECT: u16 = 13;

fn h2_target() -> Target {
    let b = |index, ret, params: &[u16], special| H2Builtin {
        index,
        return_type: ret,
        parameters: params.to_vec(),
        special,
    };
    let mut t = Target {
        types: [
            "unparsed",
            "special form",
            "function name",
            "passthrough",
            "void",
            "boolean",
            "real",
            "short",
            "long",
            "string",
            "script",
            "string_id",
            "ai",
            "object",
        ]
        .map(String::from)
        .to_vec(),
        ..Target::default()
    };
    for (name, builtin) in [
        ("begin", b(0, PASS, &[], true)),
        ("print", b(5, VOID, &[STRING], false)),
        ("ai_place", b(9, VOID, &[H2_AI, SHORT], false)),
        ("=", b(13, BOOL, &[], true)),
        ("not", b(25, BOOL, &[BOOL], false)),
        ("game_save_immediate", b(30, VOID, &[], false)),
        (
            "objects_attach",
            b(31, VOID, &[H2_OBJECT, H2_STRING_ID], false),
        ),
        ("actor_thing", b(32, VOID, &[H2_OBJECT], false)),
    ] {
        t.builtins.insert(name.into(), builtin);
    }
    t
}

/// Halo 1 syntax nodes and strings, built like the compiler lays them out.
#[derive(Default)]
struct Builder {
    nodes: Vec<[u8; 20]>,
    strings: Vec<u8>,
}

impl Builder {
    fn string(&mut self, s: &str) -> u32 {
        let at = self.strings.len() as u32;
        self.strings.extend_from_slice(s.as_bytes());
        self.strings.push(0);
        at
    }
    fn node(&mut self, op: u16, ty: u16, flags: u16, string: u32, data: u32) -> Datum {
        let slot = self.nodes.len() as u32;
        let salt = 0xe000 + slot;
        let mut b = [0u8; 20];
        b[0..2].copy_from_slice(&(salt as u16).to_le_bytes());
        b[2..4].copy_from_slice(&op.to_le_bytes());
        b[4..6].copy_from_slice(&ty.to_le_bytes());
        b[6..8].copy_from_slice(&flags.to_le_bytes());
        b[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        b[12..16].copy_from_slice(&string.to_le_bytes());
        b[16..20].copy_from_slice(&data.to_le_bytes());
        self.nodes.push(b);
        Datum(salt << 16 | slot)
    }
    fn link(&mut self, a: Datum, next: Datum) {
        self.nodes[a.slot()][8..12].copy_from_slice(&next.0.to_le_bytes());
    }
    fn atom(&mut self, source: u16, ty: u16, token: &str, data: u32) -> Datum {
        let s = self.string(token);
        self.node(source, ty, 9, s, data)
    }
    fn call_with(
        &mut self,
        op: u16,
        head_op: u16,
        ty: u16,
        flags: u16,
        name: &str,
        args: &[Datum],
    ) -> Datum {
        let s = self.string(name);
        let head = self.node(head_op, 2, 9, s, 0);
        let mut prev = head;
        for &a in args {
            self.link(prev, a);
            prev = a;
        }
        self.node(op, ty, flags, 0, head.0)
    }
    fn call(&mut self, op: u16, ty: u16, name: &str, args: &[Datum]) -> Datum {
        self.call_with(op, op, ty, 8, name, args)
    }
}

/// The scenario and its tag space: scnr at `BASE`, tables after it.
fn scenario(
    b: &Builder,
    scripts: &[(&str, u16, u16, Datum)],
    globals: &[(&str, u16, Datum)],
) -> Vec<u8> {
    let mut space = vec![0u8; SCNR_MIN_LEN];
    let put = |s: &mut Vec<u8>, at: usize, v: u32| s[at..at + 4].copy_from_slice(&v.to_le_bytes());
    let append = |s: &mut Vec<u8>, bytes: &[u8]| -> u32 {
        let at = BASE + s.len() as u32;
        s.extend_from_slice(bytes);
        at
    };
    let mut syntax = vec![0u8; SYNTAX_HEADER_LEN];
    syntax[..12].copy_from_slice(b"script node\0");
    syntax[0x20..0x22].copy_from_slice(&(b.nodes.len() as u16).to_le_bytes());
    syntax[0x22..0x24].copy_from_slice(&20u16.to_le_bytes());
    syntax[0x28..0x2c].copy_from_slice(&DATA_SIGNATURE.to_le_bytes());
    syntax[0x2e..0x30].copy_from_slice(&(b.nodes.len() as u16).to_le_bytes());
    syntax.extend(b.nodes.concat());
    let at = append(&mut space, &syntax);
    put(&mut space, SCNR_SYNTAX_DATA, syntax.len() as u32);
    put(&mut space, SCNR_SYNTAX_DATA + 0xc, at);
    let at = append(&mut space, &b.strings);
    put(&mut space, SCNR_STRING_DATA, b.strings.len() as u32);
    put(&mut space, SCNR_STRING_DATA + 0xc, at);
    let mut defs = Vec::new();
    for &(name, kind, ret, root) in scripts {
        let mut e = vec![0u8; SCRIPT_LEN];
        e[..name.len()].copy_from_slice(name.as_bytes());
        e[0x20..0x22].copy_from_slice(&kind.to_le_bytes());
        e[0x22..0x24].copy_from_slice(&ret.to_le_bytes());
        e[0x24..0x28].copy_from_slice(&root.0.to_le_bytes());
        defs.extend(e);
    }
    let at = append(&mut space, &defs);
    put(&mut space, SCNR_SCRIPTS, scripts.len() as u32);
    put(&mut space, SCNR_SCRIPTS + 4, at);
    let mut defs = Vec::new();
    for &(name, ty, root) in globals {
        let mut e = vec![0u8; GLOBAL_LEN];
        e[..name.len()].copy_from_slice(name.as_bytes());
        e[0x20..0x22].copy_from_slice(&ty.to_le_bytes());
        e[0x28..0x2c].copy_from_slice(&root.0.to_le_bytes());
        defs.extend(e);
    }
    let at = append(&mut space, &defs);
    put(&mut space, SCNR_GLOBALS, globals.len() as u32);
    put(&mut space, SCNR_GLOBALS + 4, at);
    let mut r = vec![0u8; REFERENCE_LEN];
    r[0x18..0x1c].copy_from_slice(&u32::from_be_bytes(*b"snd!").to_le_bytes());
    r[0x24..0x28].copy_from_slice(&0xe001_0002u32.to_le_bytes());
    let at = append(&mut space, &r);
    put(&mut space, SCNR_REFERENCES, 1);
    put(&mut space, SCNR_REFERENCES + 4, at);
    space
}

fn read(space: &[u8]) -> crate::Result<Tables> {
    Tables::decode(&space[..SCNR_MIN_LEN], &h1map::Space::new(space, BASE))
}

/// `(global short g_count 5)` and
/// `(script static boolean main (begin (print "hi") (ai_place squad_a)
///   (game_save_totally_unsafe) (objects_attach obj "head") (wake_helper)
///   (!= g_count 2) (gone 3)))` plus `(script dormant helper (print "x"))`.
type Scripts = Vec<(&'static str, u16, u16, Datum)>;
type Globals = Vec<(&'static str, u16, Datum)>;

fn sample() -> (Builder, Scripts, Globals) {
    let mut b = Builder::default();
    let five = b.atom(SHORT, SHORT, "5", 5);
    let hi = b.atom(STRING, STRING, "hi", 0);
    let print = b.call(1, VOID, "print", &[hi]);
    let squad = b.atom(AI, AI, "squad_a", 0x0001_0002);
    let place = b.call(2, VOID, "ai_place", &[squad]);
    let save = b.call(4, VOID, "game_save_totally_unsafe", &[]);
    let obj = b.atom(OBJECT, OBJECT, "obj", 3);
    let marker = b.atom(STRING, STRING, "head", 0);
    let attach = b.call(6, VOID, "objects_attach", &[obj, marker]);
    let helper = b.call_with(1, 1, VOID, 10, "helper", &[]);
    let g = b.string("g_count");
    let gref = b.node(SHORT, SHORT, 13, g, 0);
    let two = b.atom(SHORT, SHORT, "2", 2);
    let ne = b.call(3, BOOL, "!=", &[gref, two]);
    let three = b.atom(SHORT, SHORT, "3", 3);
    let gone = b.call(5, BOOL, "gone", &[three]);
    let root = b.call(
        0,
        BOOL,
        "begin",
        &[print, place, save, attach, helper, ne, gone],
    );
    let x = b.atom(STRING, STRING, "x", 0);
    let print_x = b.call(1, VOID, "print", &[x]);
    let helper_root = b.call(0, VOID, "begin", &[print_x]);
    (
        b,
        vec![("main", 3, BOOL, root), ("helper", 1, VOID, helper_root)],
        vec![("g_count", SHORT, five)],
    )
}

#[test]
fn tables_decode_every_field() {
    let (b, s, g) = sample();
    let t = read(&scenario(&b, &s, &g)).unwrap();
    assert_eq!(t.nodes.len(), b.nodes.len());
    assert_eq!(t.header.capacity as usize, b.nodes.len());
    assert_eq!(
        t.scripts[0],
        Script {
            name: "main".into(),
            kind: 3,
            return_type: BOOL,
            root: s[0].3
        }
    );
    assert_eq!(
        t.globals[0],
        Global {
            name: "g_count".into(),
            value_type: SHORT,
            root: g[0].2
        }
    );
    assert_eq!(t.references[0].group, h1map::Group::new(b"snd!"));
    assert_eq!(t.references[0].id, h1map::TagId(0xe001_0002));
}

#[test]
fn syntax_data_must_be_a_data_array() {
    let (b, s, g) = sample();
    let mut space = scenario(&b, &s, &g);
    let at = (u32::from_le_bytes(space[SCNR_SYNTAX_DATA + 0xc..][..4].try_into().unwrap()) - BASE)
        as usize;
    space[at + 0x28] ^= 1;
    assert!(read(&space).unwrap_err().to_string().contains("data array"));
}

#[test]
fn decompiles_to_source_and_reaches_every_node() {
    let (b, s, g) = sample();
    let t = read(&scenario(&b, &s, &g)).unwrap();
    let d = decompile(&t, &h1_catalog()).unwrap();
    let expected = "(global short g_count 5)\n\n\
(script static boolean main\n    (begin\n        (print \"hi\")\n        (ai_place squad_a)\n        \
(game_save_totally_unsafe)\n        (objects_attach obj \"head\")\n        (helper)\n        \
(!= g_count 2)\n        (gone 3)))\n\n\
(script dormant helper\n    (begin\n        (print \"x\")))\n\n";
    assert_eq!(d.source, expected);
    let st = &d.stats;
    assert_eq!(st.live_nodes, b.nodes.len());
    assert_eq!(st.reached_nodes, st.live_nodes);
    assert_eq!(
        (st.builtin_calls, st.script_calls, st.global_refs),
        (9, 1, 1)
    );
    assert_eq!((st.checked_calls, st.special_calls), (6, 3));
    assert_eq!(st.builtin_uses[&1], 2);
}

#[test]
fn decompile_rejects_bad_nodes() {
    let c = h1_catalog();
    let check = |edit: &dyn Fn(&mut Builder, Datum), want: &str| {
        let (mut b, s, g) = sample();
        edit(&mut b, s[1].3);
        let err = decompile(&read(&scenario(&b, &s, &g)).unwrap(), &c)
            .unwrap_err()
            .to_string();
        assert!(err.contains(want), "{err}");
    };
    // helper's root is `(begin (print "x"))`: begin at the last slot,
    // print two before it, its "x" atom before that.
    let slot = |d: Datum| d.slot();
    let both = |b: &mut Builder, root: Datum, op: u16| {
        for k in [2, 3] {
            b.nodes[slot(root) - k][2..4].copy_from_slice(&op.to_le_bytes());
        }
    };
    check(&|b, root| both(b, root, 999), "unknown builtin");
    check(&|b, root| both(b, root, 5), "head says print");
    check(
        &|b, root| b.nodes[slot(root) - 4][4..6].copy_from_slice(&SHORT.to_le_bytes()),
        "argument type short",
    );
    check(&|b, root| b.nodes[slot(root) - 4][0] ^= 1, "stale node");
    check(
        &|b, root| b.nodes[slot(root) - 3][2..4].copy_from_slice(&7u16.to_le_bytes()),
        "has head 7",
    );
}

#[test]
fn a_head_one_index_behind_its_call_is_accepted() {
    let (mut b, s, g) = sample();
    // ai_place (2) with a head that says 1: the call wins, so the name
    // must be the catalog's name for 2.
    let squad = b.atom(AI, AI, "squad_a", 0);
    let call = b.call_with(2, 1, VOID, 8, "ai_place", &[squad]);
    let mut s = s;
    s.push(("extra", 0, VOID, call));
    let d = decompile(&read(&scenario(&b, &s, &g)).unwrap(), &h1_catalog()).unwrap();
    assert_eq!(d.stats.stale_heads, 1);
    assert!(
        d.source
            .contains("(script startup extra\n    (ai_place squad_a))")
    );
}

#[test]
fn catalog_rejects_repeated_names_and_bad_types() {
    let types = h1_types();
    let kinds = vec!["startup".to_owned()];
    assert!(
        Catalog::new(
            vec![f("a", 0, 1, &[]), f("a", 0, 1, &[])],
            types.clone(),
            kinds.clone()
        )
        .is_err()
    );
    assert!(Catalog::new(vec![f("a", 99, 1, &[])], types.clone(), kinds.clone()).is_err());
    let c = h1_catalog();
    assert!(c.fixed_signature(1) && !c.fixed_signature(0) && !c.fixed_signature(3));
    assert_eq!(c.index_of("gone"), Some(5));
}

#[test]
fn translation_kinds() {
    let t = Table::build(&h1_catalog(), &h2_target());
    let kind = |i: u16| t.builtin(i).unwrap().kind;
    assert_eq!(kind(0), Kind::Direct);
    assert_eq!(kind(1), Kind::Direct);
    assert_eq!(t.builtin(2).unwrap().adapter, Some(Adapter::OmitOptional));
    assert_eq!(t.builtin(3).unwrap().adapter, Some(Adapter::Negate));
    assert_eq!(kind(4), Kind::Renamed);
    assert_eq!(
        t.builtin(4).unwrap().target.as_ref().unwrap().0,
        "game_save_immediate"
    );
    assert_eq!(kind(5), Kind::Missing);
    assert_eq!(t.builtin(6).unwrap().adapter, Some(Adapter::Coerce));
    assert_eq!(kind(7), Kind::Missing);
    assert_eq!(
        t.builtin(7).unwrap().reason,
        Some("a parameter type Halo 2 lacks")
    );
    assert_eq!(t.ty(AI), Some(H2_AI));
    assert_eq!(t.ty(ACTOR_VARIANT), None);
    let uses = [(0u16, 2usize), (1, 3), (4, 1), (5, 4)]
        .into_iter()
        .collect();
    let c = t.coverage(&uses);
    assert_eq!(
        (
            c.total(),
            c.of(Kind::Direct),
            c.of(Kind::Renamed),
            c.of(Kind::Missing)
        ),
        (10, 5, 1, 4)
    );
    assert_eq!(c.missing.get(&5), Some(&4));
}

fn compiled(policy: Missing) -> (Tables, crate::compile::Compiled) {
    let (b, s, g) = sample();
    let t = read(&scenario(&b, &s, &g)).unwrap();
    let c = h1_catalog();
    let h2 = h2_target();
    let table = Table::build(&c, &h2);
    let out = compile(&t, &c, &table, &h2, policy, &["sound\\x".into()]).unwrap();
    (t, out)
}

#[test]
fn compiled_tables_round_trip_through_the_halo2_decoder() {
    for policy in [Missing::Stub, Missing::Extension] {
        let (t, out) = compiled(policy);
        let bytes = out.scenario_bytes(0x8100_0000);
        let s = hs::Scenario::decode(&bytes, 0x8100_0000).unwrap();
        s.verify_codec_symmetry(&bytes, 0x8100_0000).unwrap();
        assert_eq!(s.nodes(), &out.nodes[..]);
        assert_eq!(s.strings(), &out.strings[..]);
        assert_eq!(s.scripts().len(), t.scripts.len());
        assert_eq!(s.script("main").unwrap().return_type(), BOOL);
        assert_eq!(s.script("helper").unwrap().kind(), 1);
        assert_eq!(s.globals()[0].name().unwrap(), "g_count");
        assert_eq!(s.reachable(s.roots()).unwrap().len(), out.nodes.len());
        assert_eq!(s.references().len(), 1);
        let names = s.builtin_names().unwrap();
        assert_eq!(names[&30], "game_save_immediate");
        assert_eq!(names[&25], "not");
        assert_eq!(names[&13], "=");
        assert_eq!(names[&9], "ai_place");
    }
}

#[test]
fn compile_rewrites_adapters_and_stubs_missing_calls() {
    let (_, out) = compiled(Missing::Stub);
    let s = hs::Scenario::decode(&out.scenario_bytes(BASE), BASE).unwrap();
    let call_args = |name: &str| -> Vec<hs::Node> {
        let n = s
            .nodes()
            .iter()
            .find(|n| {
                !n.atom()
                    && s.string(s.node(Datum(n.data())).unwrap().string_offset())
                        .unwrap()
                        == name
            })
            .unwrap();
        let mut a = s.node(Datum(n.data())).unwrap().next();
        let mut v = Vec::new();
        while a != Datum::NULL {
            let x = *s.node(a).unwrap();
            v.push(x);
            a = x.next();
        }
        v
    };
    // ai_place keeps one argument (Halo 2's count is optional); its
    // squad is a hook.
    let place = call_args("ai_place");
    assert_eq!(place.len(), 1);
    assert_eq!((place[0].value_type(), place[0].data()), (H2_AI, u32::MAX));
    // != becomes (not (= g_count 2)).
    let not = call_args("not");
    assert_eq!(not.len(), 1);
    assert!(!not[0].atom() && not[0].value_type() == BOOL);
    assert_eq!(call_args("=").len(), 2);
    // The marker string becomes a string ID literal, hooked.
    let attach = call_args("objects_attach");
    assert_eq!(
        (attach[1].opcode(), attach[1].value_type()),
        (H2_STRING_ID, H2_STRING_ID)
    );
    let hook = out.hooks.iter().find(|h| h.token == "head").unwrap();
    assert_eq!(
        (hook.type_name.as_str(), hook.h1_type.as_str()),
        ("string_id", "string")
    );
    assert!(
        out.hooks
            .iter()
            .any(|h| h.token == "squad_a" && h.type_name == "ai")
    );
    assert!(
        out.hooks
            .iter()
            .any(|h| h.token == "obj" && h.type_name == "object")
    );
    // gone is missing: a `false` literal stands in.
    assert_eq!(out.missing.len(), 1);
    assert_eq!(out.missing[0].name, "gone");
    let begin = call_args("begin");
    let last = begin.last().unwrap();
    assert!(last.atom() && last.opcode() == BOOL && last.data() == 0);
    // The script call keeps its index and name.
    assert!(begin.iter().any(|n| n.script_call() && n.opcode() == 1));
}

#[test]
fn extension_policy_keeps_missing_calls_for_the_vm() {
    let (_, out) = compiled(Missing::Extension);
    let s = hs::Scenario::decode(&out.scenario_bytes(BASE), BASE).unwrap();
    let names = s.builtin_names().unwrap();
    assert_eq!(names[&(crate::compile::EXTENSION_BASE + 5)], "gone");
}

#[test]
fn hooks_resolve_into_atoms() {
    let (_, mut out) = compiled(Missing::Stub);
    let left = out.resolve_hooks(|h| (h.token == "squad_a").then_some(0x0003_0004));
    assert_eq!(left.len(), out.hooks.len() - 1);
    let h = out.hooks.iter().find(|h| h.token == "squad_a").unwrap();
    assert_eq!(out.nodes[h.slot as usize].data(), 0x0003_0004);
}

#[test]
fn install_writes_the_halo2_scenario_fields() {
    let (_, out) = compiled(Missing::Stub);
    let mut scnr = map::build::Block::zeroed(field::MIN_LEN);
    out.install(&mut scnr, |_, name| {
        (name == "sound\\x").then_some((map::FourCC(*b"snd!"), map::TagId(0xe000_0001)))
    })
    .unwrap();
    let at: Vec<u32> = scnr.fixups.iter().map(|f| f.offset).collect();
    assert_eq!(
        at,
        [
            field::NODES,
            field::STRINGS,
            field::SCRIPTS,
            field::GLOBALS,
            field::REFERENCES
        ]
    );
    let map::build::FixupKind::Block { count, child } = &scnr.fixups[0].kind else {
        panic!()
    };
    assert_eq!(*count as usize, out.nodes.len());
    assert_eq!(child.bytes.len(), out.nodes.len() * hs::Node::SIZE);
    let map::build::FixupKind::Block { count, child } = &scnr.fixups[4].kind else {
        panic!()
    };
    assert_eq!(*count, 1);
    assert_eq!(
        &child.bytes[..4],
        &u32::from_be_bytes(*b"snd!").to_le_bytes()
    );
    assert!(matches!(
        child.fixups[0].kind,
        map::build::FixupKind::TagId(map::TagId(0xe000_0001))
    ));
}

#[test]
fn startup_placements_decode_as_a_startup_script() {
    let (_, mut out) = compiled(Missing::Stub);
    let hooks = out.hooks.len();
    let scripts = out.scripts.len();
    crate::compile::add_startup_placements(
        &mut out,
        &h2_target(),
        "h1_initial",
        &["first".into(), "second".into()],
    )
    .unwrap();
    let s = hs::Scenario::decode(&out.scenario_bytes(BASE), BASE).unwrap();
    assert_eq!(s.scripts().len(), scripts + 1);
    let script = s.script("h1_initial").unwrap();
    assert_eq!((script.kind(), script.return_type()), (0, VOID));
    assert_eq!(s.reachable(s.roots()).unwrap().len(), out.nodes.len());
    // Two one-argument ai_place calls inside begin, each squad hooked.
    let places: Vec<_> = s
        .nodes()
        .iter()
        .filter(|n| {
            !n.atom()
                && s.string(s.node(Datum(n.data())).unwrap().string_offset())
                    .unwrap()
                    == "ai_place"
        })
        .collect();
    assert_eq!(places.len(), 3, "main's call and the two new ones");
    let new: Vec<_> = out.hooks[hooks..]
        .iter()
        .map(|h| h.token.as_str())
        .collect();
    assert_eq!(new, ["first", "second"]);
    assert!(out.hooks[hooks..].iter().all(|h| h.h2_type == H2_AI));
    // Nothing to place: no script.
    let n = out.scripts.len();
    crate::compile::add_startup_placements(&mut out, &h2_target(), "x", &[]).unwrap();
    assert_eq!(out.scripts.len(), n);
}

mod level_and_seats {
    use crate::compile::Hook;
    use crate::level::{Level, h1, h2};
    use crate::seats::{self, Table, Unit, h2_label_matches};
    use map::TagId;
    use map::build::FixupKind;

    const BASE: u32 = 0x8000_0000;

    fn hook(ty: &str, token: &str) -> Hook {
        Hook {
            slot: 0,
            h2_type: 0,
            type_name: ty.into(),
            h1_type: String::new(),
            token: token.into(),
        }
    }

    /// An invented scenario with two flags, a camera point, a title and two
    /// device groups.
    fn level() -> Level {
        let mut space = vec![0u8; 0x600];
        let add = |space: &mut Vec<u8>, (field, len): (usize, usize), els: Vec<Vec<u8>>| {
            let at = BASE + space.len() as u32;
            space[field..field + 4].copy_from_slice(&(els.len() as u32).to_le_bytes());
            space[field + 4..field + 8].copy_from_slice(&at.to_le_bytes());
            for mut e in els {
                e.resize(len, 0);
                space.extend(e);
            }
        };
        let named = |at: usize, n: &str, floats: &[(usize, f32)]| {
            let mut e = vec![0u8; 0x100];
            e[at..at + n.len()].copy_from_slice(n.as_bytes());
            for &(o, v) in floats {
                e[o..o + 4].copy_from_slice(&v.to_le_bytes());
            }
            e
        };
        add(
            &mut space,
            h1::FLAGS,
            vec![
                named(4, "first_flag", &[(0x24, 1.0), (0x30, 0.5)]),
                named(4, "second_flag", &[(0x28, 2.0)]),
            ],
        );
        add(
            &mut space,
            h1::CAMERA_POINTS,
            vec![named(4, "cam", &[(0x28, 3.0), (0x34, 0.25)])],
        );
        add(&mut space, h1::TITLES, vec![named(4, "arrival", &[])]);
        add(
            &mut space,
            h1::DEVICE_GROUPS,
            vec![
                named(0, "door", &[(0x20, 1.0)]),
                named(0, "bridge_control_position", &[]),
            ],
        );
        let scnr = space[..0x600].to_vec();
        Level::decode(&scnr, &h1map::Space::new(&space, BASE)).unwrap()
    }

    #[test]
    fn halo1_level_data_reads_and_resolves() {
        let l = level();
        assert_eq!(l.flags[0].position, [1.0, 0.0, 0.0]);
        assert_eq!(l.flags[0].facing, [0.5, 0.0]);
        assert_eq!(l.flags[1].position, [0.0, 2.0, 0.0]);
        assert_eq!(l.camera_points[0].position, [3.0, 0.0, 0.0]);
        assert_eq!(l.camera_points[0].orientation, [0.25, 0.0, 0.0]);
        assert_eq!(l.titles, ["arrival"]);
        assert_eq!(l.device_groups[0].initial, 1.0);
        assert_eq!(l.resolve(&hook("cutscene_flag", "second_flag")), Some(1));
        assert_eq!(l.resolve(&hook("cutscene_camera_point", "cam")), Some(0));
        assert_eq!(l.resolve(&hook("cutscene_title", "arrival")), Some(0));
        // A device group atom is the runtime datum (retail: 0xe5650001 for
        // group 1).
        assert_eq!(
            l.resolve(&hook("device_group", "bridge_control_position")),
            Some(0xe565_0001)
        );
        assert_eq!(l.resolve(&hook("navpoint", "default_red")), Some(0));
        assert_eq!(l.resolve(&hook("cutscene_flag", "nope")), None);
        assert_eq!(l.resolve(&hook("ai", "first_flag")), None);
    }

    #[test]
    fn halo2_blocks_take_halo2_layouts() {
        let blocks = level().blocks();
        let field = |at: u32| blocks.iter().find(|b| b.0 == at).unwrap();
        let (_, stride, flags) = field(h2::FLAGS.0);
        assert_eq!(*stride, 0x38);
        assert_eq!(&flags[1].bytes[4..15], b"second_flag");
        assert_eq!(
            f32::from_le_bytes(flags[1].bytes[0x28..0x2c].try_into().unwrap()),
            2.0
        );
        let (_, _, cams) = field(h2::CAMERA_POINTS.0);
        assert_eq!(
            f32::from_le_bytes(cams[0].bytes[0x28..0x2c].try_into().unwrap()),
            3.0
        );
        assert_eq!(
            f32::from_le_bytes(cams[0].bytes[0x34..0x38].try_into().unwrap()),
            0.25
        );
        let (_, _, titles) = field(h2::TITLES.0);
        assert!(matches!(&titles[0].fixups[0].kind, FixupKind::StringId(s) if s == "arrival"));
        let (_, stride, groups) = field(h2::DEVICE_GROUPS.0);
        assert_eq!(*stride, 0x28);
        assert_eq!(&groups[0].bytes[..4], b"door");
        assert_eq!(
            f32::from_le_bytes(groups[0].bytes[0x20..0x24].try_into().unwrap()),
            1.0
        );
        for (_, stride, els) in &blocks {
            assert!(els.iter().all(|e| e.bytes.len() == *stride));
        }
    }

    #[test]
    fn halo1_seat_roles_match_halo2_labels() {
        assert!(h2_label_matches("driver", "warthog_d"));
        assert!(h2_label_matches("W-driver", "scorpion_d"));
        assert!(!h2_label_matches("driver", "warthog_b_d"));
        assert!(h2_label_matches("gunner", "warthog_g"));
        assert!(h2_label_matches("gunner", "pelican_g_rear"));
        assert!(h2_label_matches("passenger", "warthog_p"));
        assert!(h2_label_matches("passenger", "scorpion_p_lf"));
        assert!(!h2_label_matches("passenger", "warthog_b_p"));
        assert!(h2_label_matches("p-riderlf", "pelican_p_l01"));
        assert!(!h2_label_matches("p-riderlf", "pelican_p_r01"));
        assert!(!h2_label_matches("p-riderlf", "warthog_p"));
        assert!(h2_label_matches("cargo", "pelican_lc"));
        // Retail's own rule for anything else: the label starts with it.
        assert!(h2_label_matches("pelican_p", "pelican_p_l03"));
        assert!(!h2_label_matches("pelican_p", "pelican_g"));
    }

    #[test]
    fn seat_table_is_runs_of_unit_and_mask() {
        let hog = TagId(0xe000_0001);
        let pelican = TagId(0xe001_0002);
        let units = [
            Unit {
                id: hog,
                labels: vec!["warthog_d".into(), "warthog_p".into(), "warthog_b_d".into()],
            },
            Unit {
                id: pelican,
                labels: vec![
                    "pelican_d".into(),
                    "pelican_p_l01".into(),
                    "pelican_p_r01".into(),
                ],
            },
        ];
        let t = Table::build(["driver", "p-riderlf", "driver", "nothing"], &units);
        assert_eq!(t.entries, [(hog, 1), (pelican, 1), (pelican, 2)]);
        assert_eq!(t.values["driver"], 2 << 16);
        assert_eq!(t.values["p-riderlf"], 1 << 16 | 2);
        assert!(!t.values.contains_key("nothing"));
        let els = t.elements();
        assert_eq!(els[2].bytes.len(), seats::MAPPINGS.1);
        assert_eq!(&els[2].bytes[4..8], &2u32.to_le_bytes());
        assert!(matches!(els[2].fixups[0].kind, FixupKind::TagId(id) if id == pelican));
    }

    #[test]
    fn seat_labels_read_from_pinned_and_built_vehicles() {
        use map::build::{Block, TagPayload, TagSet};
        let mut set = TagSet::new(map::CacheType::Campaign, "t", "s\\t");
        set.strings = vec![None, Some("warthog_d".into()), Some("warthog_p".into())];
        let (field, stride, label) = seats::UNIT_SEATS;
        // Pinned: the seats block inside the tag's own bytes.
        let address: u32 = 0x8010_0000;
        let mut bytes = vec![0u8; 0x200 + 2 * stride];
        bytes[field..field + 4].copy_from_slice(&2u32.to_le_bytes());
        bytes[field + 4..field + 8].copy_from_slice(&(address + 0x200).to_le_bytes());
        for (i, sid) in [1u32 | 9 << 24, 2 | 9 << 24].iter().enumerate() {
            let at = 0x200 + i * stride + label;
            bytes[at..at + 4].copy_from_slice(&sid.to_le_bytes());
        }
        set.add_group(map::FourCC(*b"vehi"), None, None);
        let pinned = set
            .add_tag(
                map::FourCC(*b"vehi"),
                "hog",
                TagPayload::Pinned { address, bytes },
            )
            .unwrap();
        // Built: a seats block child with a string ID fixup.
        let mut root = Block::zeroed(0x200);
        let mut child = Block::zeroed(stride);
        child.string_id(label as u32, "pelican_d");
        root.block(field as u32, 1, child);
        let built = set
            .add_tag(map::FourCC(*b"vehi"), "pelican", TagPayload::Built(root))
            .unwrap();
        let u = seats::units(&set);
        assert_eq!(
            u[0],
            Unit {
                id: pinned,
                labels: vec!["warthog_d".into(), "warthog_p".into()]
            }
        );
        assert_eq!(
            u[1],
            Unit {
                id: built,
                labels: vec!["pelican_d".into()]
            }
        );
    }
}

/// `(script static boolean cinematic_skip_start true)`, two dormant
/// scripts `tutorial_setup` and `fast_setup`, and `(script startup main
/// (begin (cinematic_skip_start) (wake tutorial_setup)
/// (ai_conversation_status squad_a)))`, compiled with `cutscenes`.
fn cutscene_level(cutscenes: crate::compile::Cutscenes) -> hs::Scenario {
    const SCRIPT: u16 = 10;
    let c = Catalog::new(
        vec![
            f("begin", PASS, 0x2000, &[]),
            f("wake", VOID, ORDINARY, &[SCRIPT]),
            f("ai_conversation_status", SHORT, ORDINARY, &[AI]),
            f("print", VOID, ORDINARY, &[STRING]),
        ],
        h1_types(),
        ["startup", "dormant", "continuous", "static", "stub"]
            .map(String::from)
            .to_vec(),
    )
    .unwrap();
    let mut h2 = h2_target();
    h2.builtins.insert(
        "wake".into(),
        H2Builtin {
            index: 33,
            return_type: VOID,
            parameters: vec![SCRIPT],
            special: false,
        },
    );
    let mut b = Builder::default();
    let yes = b.atom(BOOL, BOOL, "true", 1);
    let guard = b.call(0, BOOL, "begin", &[yes]);
    let dormant = |b: &mut Builder, s: &str| {
        let text = b.atom(STRING, STRING, s, 0);
        let p = b.call(3, VOID, "print", &[text]);
        b.call(0, VOID, "begin", &[p])
    };
    let tutorial = dormant(&mut b, "t");
    let fast = dormant(&mut b, "f");
    let skip = b.call_with(0, 0, BOOL, 10, "cinematic_skip_start", &[]);
    let target = b.atom(SCRIPT, SCRIPT, "tutorial_setup", 1);
    let wake = b.call(1, VOID, "wake", &[target]);
    let squad = b.atom(AI, AI, "squad_a", 0);
    let status = b.call(2, SHORT, "ai_conversation_status", &[squad]);
    let main = b.call(0, VOID, "begin", &[skip, wake, status]);
    let scripts = vec![
        ("cinematic_skip_start", 3, BOOL, guard),
        ("tutorial_setup", 1, VOID, tutorial),
        ("fast_setup", 1, VOID, fast),
        ("main", 0, VOID, main),
    ];
    let t = read(&scenario(&b, &scripts, &[])).unwrap();
    let table = Table::build(&c, &h2);
    let out =
        crate::compile::compile_with(&t, &c, &table, &h2, Missing::Stub, cutscenes, &[]).unwrap();
    let bytes = out.scenario_bytes(BASE);
    let s = hs::Scenario::decode(&bytes, BASE).unwrap();
    s.verify_codec_symmetry(&bytes, BASE).unwrap();
    assert_eq!(s.reachable(s.roots()).unwrap().len(), out.nodes.len());
    s
}

/// The arguments of `main`'s `begin`.
fn main_body(s: &hs::Scenario) -> Vec<hs::Node> {
    let root = s.script("main").unwrap().root();
    let call = s.node(root).unwrap();
    let mut a = s.node(Datum(call.data())).unwrap().next();
    let mut v = Vec::new();
    while a != Datum::NULL {
        let n = *s.node(a).unwrap();
        v.push(n);
        a = n.next();
    }
    v
}

#[test]
fn skipped_cutscenes_take_halo1_s_skip_path() {
    use crate::compile::Cutscenes;
    let wake_arg = |s: &hs::Scenario, n: &hs::Node| {
        let head = s.node(Datum(n.data())).unwrap();
        let arg = *s.node(head.next()).unwrap();
        (
            arg.data(),
            s.string(arg.string_offset()).unwrap().to_owned(),
        )
    };
    // Played: the guard is called and the tutorial woken.
    let s = cutscene_level(Cutscenes::Play);
    let body = main_body(&s);
    assert!(body[0].script_call() && body[0].opcode() == 0);
    assert_eq!(wake_arg(&s, &body[1]), (1, "tutorial_setup".into()));
    // Skipped: the guard is `false`, the short path woken (script 2).
    let s = cutscene_level(Cutscenes::Skip);
    let body = main_body(&s);
    assert!(body[0].atom() && body[0].opcode() == BOOL && body[0].data() == 0);
    assert_eq!(s.string(body[0].string_offset()).unwrap(), "false");
    assert_eq!(wake_arg(&s, &body[1]), (2, "fast_setup".into()));
    // Either way the stubbed conversation status reads as finished.
    for s in [cutscene_level(Cutscenes::Play), s] {
        let status = main_body(&s)[2];
        assert!(status.atom());
        assert_eq!((status.opcode(), status.data()), (SHORT, 5));
    }
}
