//! The `native-post` feature (docs/graphics/native-renderer.md, stage 5):
//! its plan (one context-only hook on the colour pass that the pipelined
//! render runs on its own thread, render-timing or not; none with the
//! switch off unless the A/B asks for it) and its body on synthetic guest
//! memory: the host gets the call's seven words, the caller's
//! floating-point state and the mode; the call always runs.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_post::{self, Fp, Host, NativePost, ScreenEffect};
use feature::plan::Base;
use feature::{Registry, Switches};

fn placed(switches: &str) -> Option<feature::plan::Plan> {
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    r.add(&NativePost).unwrap();
    let plan = r.build(&Base).unwrap();
    plan.text().contains("native_post_colour").then_some(plan)
}

#[test]
fn the_switch_places_it_and_the_ab_places_it_observing() {
    // One test: the A/B's placement is process-wide.
    assert!(placed("").is_none(), "off: nothing");
    native_post::place_for_capture(true);
    assert!(placed("").is_some(), "the A/B's D3D side");
    native_post::place_for_capture(false);
    assert!(placed("").is_none());
    assert!(placed("impl.render.native_post=on,render-timing").is_some(), "the call still runs: timing is fine");
}

#[test]
fn its_chain_runs_on_the_render_thread() {
    let plan = placed("impl.render.native_post=on").expect("placed");
    let chain = plan
        .chains
        .values()
        .find(|c| c.point.name == "COLOUR_PASS")
        .expect("placed at COLOUR_PASS");
    assert!(!chain.needs_shadow(), "nothing after the body");
    assert!(chain.render_thread().is_ok());
}

const ESP: u32 = 0x0060_0000;

#[derive(Default)]
struct Stub {
    arm: Mutex<bool>,
    seen: Mutex<Vec<([u32; 7], Fp, bool)>>,
    effects: Mutex<Vec<(ScreenEffect, Fp)>>,
}

impl Host for Stub {
    fn colour_pass(&self, _mem: &mut dyn GuestMemory, args: [u32; 7], fp: Fp, native: bool) -> bool {
        self.seen.lock().unwrap().push((args, fp, native));
        *self.arm.lock().unwrap()
    }

    fn screen_effect(&self, _mem: &mut dyn GuestMemory, call: ScreenEffect, fp: Fp) -> bool {
        self.effects.lock().unwrap().push((call, fp));
        *self.arm.lock().unwrap()
    }
}

#[test]
fn the_host_gets_the_calls_words() {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(ESP, 0x0001_39ba).unwrap();
    let args = [0x407f_7cee, 0x3f00_0000, 0x3e00_0000, 0x3e80_0000, 0, 1, 0x0000_0100];
    for (i, a) in args.iter().enumerate() {
        m.set_u32(ESP + 4 + 4 * i as u32, *a).unwrap();
    }
    let fp = Fp { mxcsr: 0x1f80, x87_control: 0x027f };
    let before = native_post::counts();

    // No host: nothing armed, still counted.
    assert!(!native_post::on_colour_pass(&mut m, ESP, fp, true));

    let stub = Arc::new(Stub::default());
    native_post::set_host(stub.clone());
    assert!(!native_post::on_colour_pass(&mut m, ESP, fp, false));
    assert_eq!(stub.seen.lock().unwrap().last().copied(), Some((args, fp, false)));
    *stub.arm.lock().unwrap() = true;
    assert!(native_post::on_colour_pass(&mut m, ESP, fp, true));
    assert_eq!(stub.seen.lock().unwrap().last().copied(), Some((args, fp, true)));
    // An unreadable stack: the host is not asked.
    let asked = stub.seen.lock().unwrap().len();
    assert!(!native_post::on_colour_pass(&mut m, 0x0001_0000, fp, true));
    assert_eq!(stub.seen.lock().unwrap().len(), asked);

    // A screen effect: EAX the datum, then the four words.
    m.set_u32(ESP, 0x0001_3b0c).unwrap();
    for (i, w) in [0x0060_1000u32, 2, 2, 0x0060_2000].iter().enumerate() {
        m.set_u32(ESP + 4 + 4 * i as u32, *w).unwrap();
    }
    assert!(native_post::on_screen_effect(&mut m, 0xe001_0002, ESP, fp));
    let want = ScreenEffect {
        datum: 0xe001_0002,
        values: 0x0060_1000,
        columns: 2,
        rows: 2,
        corners: 0x0060_2000,
    };
    assert_eq!(stub.effects.lock().unwrap().last().copied(), Some((want, fp)));
    assert!(!native_post::on_screen_effect(&mut m, 0, 0x0001_0000, fp), "an unreadable stack");

    let after = native_post::counts();
    assert!(after.calls >= before.calls + 4);
    assert!(after.armed > before.armed);
    assert!(after.effects >= before.effects + 2);
    assert!(after.effects_armed > before.effects_armed);
}

#[test]
fn the_screen_effect_hook_is_the_switchs_only() {
    let on = placed("impl.render.native_post=on").expect("placed");
    assert!(on.text().contains("native_post_screen_effect"));
    let chain = on
        .chains
        .values()
        .find(|c| c.point.name == "SCREEN_EFFECT")
        .expect("placed at SCREEN_EFFECT");
    assert!(!chain.needs_shadow(), "nothing after the body");
    assert!(chain.render_thread().is_ok());
}

/// `H2_POST_TRACE`: a count (empty: the default), `,after` to note the
/// returns too; anything else traces nothing.
#[test]
fn the_trace_variable_is_a_count_and_after() {
    use native_post::{TraceSpec, TRACE_CALLS};
    let spec = |calls, after| Some(TraceSpec { calls, after });
    assert_eq!(TraceSpec::parse(""), spec(TRACE_CALLS, false));
    assert_eq!(TraceSpec::parse("8"), spec(8, false));
    assert_eq!(TraceSpec::parse("8,after"), spec(8, true));
    assert_eq!(TraceSpec::parse(" 2 , after "), spec(2, true));
    assert_eq!(TraceSpec::parse(",after"), spec(TRACE_CALLS, true));
    for bad in ["0", "-1", "x", "8,before", "8,after,x"] {
        assert_eq!(TraceSpec::parse(bad), None, "{bad}");
    }
    assert_eq!(native_post::return_line("0x0002d140", 0xffff_ff01), "0x0002d140 returned, AL 1");
}
