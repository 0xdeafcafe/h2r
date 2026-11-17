//! Cutscene hooks: animation names (invented
//! builtins, types and scripts).
use super::*;
use crate::compile::{ANIMATION_NAME, Cutscenes, compile_with};

const SOUND: u16 = 14;
const GRAPH: u16 = 15;
const UNIT: u16 = 16;

/// `(script startup main (begin (sleep (sound_impulse_time snd\line))
/// (custom_animation keyes graphs\x10 "Wave Hello" false)))`.
fn compiled() -> crate::compile::Compiled {
    let mut types = h1_types();
    types.extend(["sound", "animation_graph", "unit"].map(String::from));
    let c = Catalog::new(
        vec![
            f("begin", PASS, 0x2000, &[]),
            f("sleep", VOID, ORDINARY, &[SHORT]),
            f("sound_impulse_time", 8, ORDINARY, &[SOUND]),
            f(
                "custom_animation",
                BOOL,
                ORDINARY,
                &[UNIT, GRAPH, STRING, BOOL],
            ),
        ],
        types,
        ["startup", "dormant", "continuous", "static", "stub"]
            .map(String::from)
            .to_vec(),
    )
    .unwrap();
    let mut h2 = h2_target();
    h2.types
        .extend(["sound", "animation_graph", "unit"].map(String::from));
    let (h2_sound, h2_graph, h2_unit) = (14, 15, 16);
    assert_eq!(h2.types[h2_unit as usize], "unit");
    let _ = h2_sound;
    for (name, b) in [
        (
            "sleep",
            H2Builtin {
                index: 40,
                return_type: VOID,
                parameters: vec![SHORT],
                special: false,
            },
        ),
        (
            "custom_animation",
            H2Builtin {
                index: 201,
                return_type: BOOL,
                parameters: vec![h2_unit, h2_graph, H2_STRING_ID, BOOL],
                special: false,
            },
        ),
    ] {
        h2.builtins.insert(name.into(), b);
    }
    let mut b = Builder::default();
    let snd = b.atom(SOUND, SOUND, "snd\\line", u32::MAX);
    let time = b.call(2, 8, "sound_impulse_time", &[snd]);
    let sleep = b.call(1, VOID, "sleep", &[time]);
    let unit = b.atom(UNIT, UNIT, "keyes", u32::MAX);
    let graph = b.atom(GRAPH, GRAPH, "graphs\\x10", u32::MAX);
    let name = b.atom(STRING, STRING, "Wave Hello", 0);
    let no = b.atom(BOOL, BOOL, "false", 0);
    let anim = b.call(3, BOOL, "custom_animation", &[unit, graph, name, no]);
    let main = b.call(0, VOID, "begin", &[sleep, anim]);
    let t = read(&scenario(&b, &[("main", 0, VOID, main)], &[])).unwrap();
    let table = Table::build(&c, &h2);
    compile_with(
        &t,
        &c,
        &table,
        &h2,
        Missing::Stub,
        Cutscenes::Play,
        &|_, _| true,
        &[],
    )
    .unwrap()
}

#[test]
fn an_animation_name_is_hooked_with_its_graph() {
    let out = compiled();
    let h = out
        .hooks
        .iter()
        .find(|h| h.h1_type == ANIMATION_NAME)
        .unwrap();
    assert_eq!(h.token, "graphs\\x10|Wave Hello");
    assert_eq!(h.type_name, "string_id");
    // The graph itself stays an ordinary hook, resolved by tag name.
    assert!(
        out.hooks
            .iter()
            .any(|h| h.type_name == "animation_graph" && h.token == "graphs\\x10")
    );
}
