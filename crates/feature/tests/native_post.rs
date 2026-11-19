//! The `native-post` feature (docs/graphics/native-renderer.md, stage 5):
//! its plan (one context-only hook the pipelined render runs on its own
//! thread, none with render-timing on, none with the switch off) and its
//! skip on synthetic guest memory: nothing is skipped without a host, for
//! a call the host does not accept, or when the device cannot take the
//! pass; the host sees what the call was asked to draw.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_post::{self, Host, NativePost};
use feature::plan::Base;
use feature::{Registry, Switches};
use native_render::post::{PostFrame, SCENARIO, SCENARIO_SCREEN_EFFECTS, VIEW_KIND};

#[test]
fn the_switch_off_places_nothing() {
    let mut r = Registry::new(Switches::parse("").unwrap());
    r.add(&NativePost).unwrap();
    let text = r.build(&Base).unwrap().text();
    assert!(!text.contains("native_post"), "{text}");
}

#[test]
fn its_chain_runs_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("impl.render.native_post=on").unwrap());
    assert!(r.add(&NativePost).unwrap());
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    assert!(text.contains("native_post_skip"), "{text}");
    let chain = plan
        .chains
        .values()
        .find(|c| c.point.name == "RENDER_SCREEN_EFFECTS")
        .expect("placed at RENDER_SCREEN_EFFECTS");
    assert!(!chain.needs_shadow(), "nothing after the body");
    assert!(chain.render_thread().is_ok());
}

#[test]
fn render_timing_on_places_nothing() {
    let mut r = Registry::new(Switches::parse("impl.render.native_post=on,render-timing").unwrap());
    r.add(&NativePost).unwrap();
    let text = r.build(&Base).unwrap().text();
    assert!(!text.contains("native_post"), "{text}");
}

/// Where the hook's ESP points (the return address; the arguments above).
const ESP: u32 = 0x0060_0000;
const SCENARIO_AT: u32 = 0x0058_0000;

#[derive(Default)]
struct Stub {
    accept: Mutex<bool>,
    open: Mutex<bool>,
    seen: Mutex<Vec<PostFrame>>,
    opened: Mutex<u32>,
}

impl Host for Stub {
    fn accepts(&self, _mem: &dyn GuestMemory, frame: &PostFrame) -> bool {
        self.seen.lock().unwrap().push(*frame);
        *self.accept.lock().unwrap()
    }
    fn open(&self, _frame: &PostFrame) -> bool {
        let open = *self.open.lock().unwrap();
        if open {
            *self.opened.lock().unwrap() += 1;
        }
        open
    }
}

/// The stack at the call: `0x00013870(late, colour)` pushes `colour`, then
/// `late`, then the return address.
fn call(m: &mut FlatMemory, late: bool, colour: bool) -> bool {
    m.set_u32(ESP, 0x0002_c33a).unwrap();
    // Only the low byte counts: the high bytes are whatever the push held.
    m.set_u32(ESP + 4, 0xdead_be00 | u32::from(late)).unwrap();
    m.set_u32(ESP + 8, 0xdead_be00 | u32::from(colour)).unwrap();
    native_post::on_screen_effects(m, ESP)
}

#[test]
fn a_call_is_skipped_only_when_the_host_draws_it() {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(VIEW_KIND, 1).unwrap();
    m.set_u32(SCENARIO, SCENARIO_AT).unwrap();
    m.set_u32(SCENARIO_AT + SCENARIO_SCREEN_EFFECTS, 1).unwrap();
    let before = native_post::counts();

    // No host: the call runs, and is counted.
    assert!(!call(&mut m, false, true));

    let stub = Arc::new(Stub::default());
    native_post::set_host(stub.clone());
    // Refused: the call runs; the host saw what it asked for.
    assert!(!call(&mut m, true, false));
    assert_eq!(
        stub.seen.lock().unwrap().last().copied(),
        Some(PostFrame {
            colour: false,
            late: true,
            view_kind: 1,
            screen_effects: 1,
            screen_effects_gated: true,
        })
    );
    // Accepted but the device takes no pass: the call runs.
    *stub.accept.lock().unwrap() = true;
    assert!(!call(&mut m, false, true));
    assert_eq!(*stub.opened.lock().unwrap(), 0);
    // Accepted and opened: skipped.
    *stub.open.lock().unwrap() = true;
    assert!(call(&mut m, true, true));
    assert_eq!(*stub.opened.lock().unwrap(), 1);
    // A view kind in 5..7 gates the screen effects off; still skipped.
    m.set_u32(VIEW_KIND, 6).unwrap();
    assert!(call(&mut m, false, true));
    assert!(!stub.seen.lock().unwrap().last().unwrap().screen_effects_gated);
    // An unreadable stack: the call runs, the host is not asked.
    let asked = stub.seen.lock().unwrap().len();
    assert!(!native_post::on_screen_effects(&mut m, 0x0001_0000));
    assert_eq!(stub.seen.lock().unwrap().len(), asked);

    let after = native_post::counts();
    assert_eq!(after.calls - before.calls, 6);
    assert_eq!(after.skipped - before.skipped, 2);
    assert_eq!(after.colour - before.colour, 4);
    assert_eq!(after.late - before.late, 2);
    assert_eq!(after.screen_effects - before.screen_effects, 4);
}
