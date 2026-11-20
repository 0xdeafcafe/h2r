//! The `native-hud` feature (docs/graphics/native-renderer.md, stage 5):
//! its plan (four context-only hooks the pipelined render runs on its own
//! thread; none with the switch off) and its bodies on synthetic guest
//! memory: the host gets each call's words, and a quad it answers returns
//! in place of the original.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_hud::{self, Host, HudQuadCall, NativeHud};
use feature::plan::Base;
use feature::{Registry, Switches};

fn plan(switches: &str) -> feature::plan::Plan {
    let mut registry = Registry::new(Switches::parse(switches).unwrap());
    registry.add(&NativeHud).unwrap();
    registry.build(&Base).unwrap()
}

#[test]
fn the_native_ui_switch_defaults_on_and_the_master_disables_it() {
    for (spec, expected) in [
        ("", true),
        ("impl.render.native_ui=on", true),
        ("impl.render.native_ui=off", false),
        ("impl.render.native_hud=off", false),
        ("impl.render.native=off,impl.render.native_ui=on", false),
        ("impl.render.native_ui=on,impl.render.native=off", false),
    ] {
        assert_eq!(native_hud::on(&Switches::parse(spec).unwrap()), expected);
    }
    assert!(plan("impl.render.native=off,impl.render.native_ui=on").chains.is_empty());
}

#[test]
fn the_ui_hooks_can_run_on_the_render_thread() {
    let plan = plan("");
    let points: &[&str] = if native_hud::compare_spec().is_some() {
        &["SCREEN_QUADS_BEGIN", "SCREEN_QUAD", "SCREEN_QUADS_END", "HUD_SHADER_QUAD"]
    } else {
        &[
            "UI_RENDER_LAYERS", "UI_ITEM_CALL", "UI_ITEM_RETURN", "UI_SCREEN_QUAD_READY",
            "UI_LOADING_RETURN", "UI_LOADING_EMPTY_RETURN", "UI_FADE_FAN_READY", "HUD_QUAD_READY",
        ]
    };
    for point in points {
        let chain = plan.chains.values().find(|c| c.point.name == *point)
            .unwrap_or_else(|| panic!("placed at {point}"));
        assert!(!chain.needs_shadow(), "{point}: nothing after the body");
        assert!(chain.render_thread().is_ok(), "{point}");
    }
}

#[test]
fn the_compare_count_parses() {
    assert_eq!(
        native_hud::parse_compare(""),
        Some(native_hud::COMPARE_WINDOWS)
    );
    assert_eq!(native_hud::parse_compare(" 3 "), Some(3));
    assert_eq!(native_hud::parse_compare("0"), None);
    assert_eq!(native_hud::parse_compare("all"), None);
}

const ESP: u32 = 0x0060_0000;

#[derive(Default)]
struct Stub {
    answer: Mutex<bool>,
    calls: Mutex<Vec<String>>,
}

impl Host for Stub {
    fn begin(&self, _mem: &mut dyn GuestMemory, record: u32) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("begin {record:#x}"));
    }
    fn screen_quad(&self, _mem: &mut dyn GuestMemory, corners: u32) -> bool {
        self.calls
            .lock()
            .unwrap()
            .push(format!("quad {corners:#x}"));
        *self.answer.lock().unwrap()
    }
    fn end(&self, _mem: &mut dyn GuestMemory) {
        self.calls.lock().unwrap().push("end".into());
    }
    fn hud_quad(&self, _mem: &mut dyn GuestMemory, call: HudQuadCall) {
        self.calls.lock().unwrap().push(format!("hud {call:x?}"));
    }
}

#[test]
fn the_host_gets_each_calls_words_and_answers_quads() {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(ESP, 0x0001_f275).unwrap();
    m.set_u32(ESP + 4, 0x0050_0000).unwrap();
    m.set_u32(ESP + 8, 3).unwrap();
    m.set_u32(ESP + 12, 0xe004_0005).unwrap();
    let before = native_hud::counts();
    // No host: nothing answered.
    assert!(!native_hud::on_screen_quad(&mut m, ESP));
    let stub = Arc::new(Stub::default());
    native_hud::set_host(stub.clone());
    native_hud::on_begin(&mut m, ESP);
    assert!(!native_hud::on_screen_quad(&mut m, ESP));
    *stub.answer.lock().unwrap() = true;
    assert!(native_hud::on_screen_quad(&mut m, ESP));
    native_hud::on_end(&mut m);
    native_hud::on_hud_quad(&mut m, 0x0051_0000, 0x0052_0000, ESP);
    native_hud::on_hud_quad(&mut m, 0, 0, 0x0001_0000);
    assert!(
        !native_hud::on_screen_quad(&mut m, 0x0001_0000),
        "an unreadable stack"
    );
    let calls = stub.calls.lock().unwrap().clone();
    assert_eq!(calls[0], "begin 0x500000");
    assert_eq!(calls[1], "quad 0x500000");
    assert_eq!(calls[3], "end");
    assert_eq!(
        calls[4],
        "hud HudQuadCall { corners: 510000, rect: 520000, args: [500000, 3, e0040005] }"
    );
    assert_eq!(calls.len(), 5, "{calls:?}");
    let after = native_hud::counts();
    assert!(after.quads >= before.quads + 4);
    assert!(after.answered > before.answered);
    assert!(after.hud_quads >= before.hud_quads + 2);
}
