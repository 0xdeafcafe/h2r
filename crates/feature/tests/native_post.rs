//! The `native-post` feature (docs/graphics/native-post-hud.md, stage 5):
//! Its entry and exit hooks and the call identity reported to the host.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_post::{self, CallSite, Fp, Host, NativePost, PostCall, ScreenEffect};
use feature::plan::Base;
use feature::{Registry, Switches};

fn placed(switches: &str) -> Option<feature::plan::Plan> {
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    r.add(&NativePost).unwrap();
    let plan = r.build(&Base).unwrap();
    plan.text().contains("native_post_colour").then_some(plan)
}

#[test]
fn the_master_switch_places_it() {
    assert!(placed("impl.render.native=off").is_none(), "off: nothing");
    assert!(placed("").is_some(), "on by default");
    assert!(placed("render-timing").is_some(), "the call still runs: timing is fine");
}

/// Off or on, it holds nothing a load must drop: never a blocker.
#[test]
fn its_snapshot_policy_is_stateless_on_or_off() {
    for switches in ["impl.render.native=off", ""] {
        let mut r = Registry::new(Switches::parse(switches).unwrap());
        assert!(r.add(&NativePost).unwrap());
        let plan = r.build(&Base).unwrap();
        let policies: Vec<_> = plan
            .snapshot
            .iter()
            .map(|f| (f.id, f.policy.name()))
            .collect();
        assert_eq!(
            policies,
            vec![(native_post::ID, "stateless")],
            "{switches:?}"
        );
        assert!(plan.snapshot_blockers().is_empty(), "{switches:?}");
    }
}

#[test]
fn the_colour_pass_has_a_return_boundary() {
    let plan = placed("").expect("placed");
    let chain = plan
        .chains
        .values()
        .find(|c| c.point.name == "COLOUR_PASS")
        .expect("placed at COLOUR_PASS");
    assert!(chain.needs_shadow(), "the return continuation closes the call");
}

const ESP: u32 = 0x0060_0000;

#[derive(Default)]
struct Stub {
    arm: Mutex<bool>,
    seen: Mutex<Vec<(CallSite, [u32; 7], Fp)>>,
    effects: Mutex<Vec<(CallSite, ScreenEffect, Fp)>>,
    finished: Mutex<Vec<(PostCall, u32)>>,
}

impl Host for Stub {
    fn colour_pass(&self, _mem: &mut dyn GuestMemory, site: CallSite, args: [u32; 7], fp: Fp) -> bool {
        self.seen.lock().unwrap().push((site, args, fp));
        *self.arm.lock().unwrap()
    }

    fn screen_effect(&self, _mem: &mut dyn GuestMemory, site: CallSite, call: ScreenEffect, fp: Fp) -> bool {
        self.effects.lock().unwrap().push((site, call, fp));
        *self.arm.lock().unwrap()
    }

    fn finish(&self, _: &mut dyn GuestMemory, kind: PostCall, esp: u32) {
        self.finished.lock().unwrap().push((kind, esp));
    }
}

#[test]
fn the_host_gets_the_calls_words() {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(ESP, 0x0001_39ba).unwrap();
    let site = CallSite { kind: PostCall::Colour, esp: ESP, ret: 0x0001_39ba };
    let args = [0x407f_7cee, 0x3f00_0000, 0x3e00_0000, 0x3e80_0000, 0, 1, 0x0000_0100];
    for (i, a) in args.iter().enumerate() {
        m.set_u32(ESP + 4 + 4 * i as u32, *a).unwrap();
    }
    let fp = Fp { mxcsr: 0x1f80, x87_control: 0x027f };
    let before = native_post::counts();

    // No host: nothing armed, still counted.
    assert!(!native_post::on_colour_pass(&mut m, ESP, fp));

    let stub = Arc::new(Stub::default());
    native_post::set_host(stub.clone());
    assert!(!native_post::on_colour_pass(&mut m, ESP, fp));
    assert_eq!(stub.seen.lock().unwrap().last().copied(), Some((site, args, fp)));
    *stub.arm.lock().unwrap() = true;
    assert!(native_post::on_colour_pass(&mut m, ESP, fp));
    assert_eq!(stub.seen.lock().unwrap().last().copied(), Some((site, args, fp)));
    // An unreadable stack: the host is not asked.
    let asked = stub.seen.lock().unwrap().len();
    assert!(!native_post::on_colour_pass(&mut m, 0x0001_0000, fp));
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
    let site = CallSite { kind: PostCall::Screen, esp: ESP, ret: 0x0001_3b0c };
    assert_eq!(stub.effects.lock().unwrap().last().copied(), Some((site, want, fp)));
    native_post::finish(&mut m, PostCall::Colour, ESP + 32);
    native_post::finish(&mut m, PostCall::Screen, ESP + 20);
    assert_eq!(*stub.finished.lock().unwrap(), [(PostCall::Colour, ESP), (PostCall::Screen, ESP)]);
    assert!(!native_post::on_screen_effect(&mut m, 0, 0x0001_0000, fp), "an unreadable stack");

    let after = native_post::counts();
    assert!(after.calls >= before.calls + 4);
    assert!(after.armed > before.armed);
    assert!(after.effects >= before.effects + 2);
    assert!(after.effects_armed > before.effects_armed);
}

#[test]
fn the_screen_effect_hook_is_the_switchs_only() {
    let on = placed("").expect("placed");
    assert!(on.text().contains("native_post_screen_effect"));
    let chain = on
        .chains
        .values()
        .find(|c| c.point.name == "SCREEN_EFFECT")
        .expect("placed at SCREEN_EFFECT");
    assert!(chain.needs_shadow(), "the return continuation closes the call");
}
