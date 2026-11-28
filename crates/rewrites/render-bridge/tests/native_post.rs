//! The `native-post` feature (docs/graphics/native-post-hud.md, stage 5;
//! docs/graphics/retire-d3d-display.md, "The post and the screen
//! effects"): the colour pass and the screen effect as Rust bodies, the
//! guest's writes they make, and what they hand the host.
#![cfg(feature = "native-render")]

use std::sync::{Arc, Mutex};

use engine::colour_pass::Parameters;
use engine::memory::{FlatMemory, GuestMemory};
use render_bridge::native_post::{self, Fp, Host, NativePost, ScreenEffect};
use hooks::plan::Base;
use hooks::{Registry, Switches};

fn placed(switches: &str) -> Option<hooks::plan::Plan> {
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
        let policies: Vec<_> = plan.snapshot.iter().map(|f| (f.id, f.policy.name())).collect();
        assert_eq!(policies, vec![(native_post::ID, "stateless")], "{switches:?}");
        assert!(plan.snapshot_blockers().is_empty(), "{switches:?}");
    }
}

/// Both calls are Rust: an implementation at each point, nothing after
/// a guest body.
#[test]
fn both_calls_are_implemented_in_rust() {
    let plan = placed("").expect("placed");
    for point in ["COLOUR_PASS", "SCREEN_EFFECT"] {
        let chain = plan.chains.values().find(|c| c.point.name == point).expect(point);
        assert!(chain.implement.is_some(), "{point}: no guest body runs");
        assert!(!chain.needs_shadow(), "{point}: no return continuation");
    }
}

const ESP: u32 = 0x0060_0000;

#[derive(Default)]
struct Stub {
    glows: Mutex<Vec<(Parameters, Fp)>>,
    effects: Mutex<Vec<(ScreenEffect, Fp)>>,
    ended: Mutex<bool>,
}

impl Host for Stub {
    fn glow(&self, _: &mut dyn GuestMemory, p: &Parameters, fp: Fp) -> Result<(), String> {
        self.glows.lock().unwrap().push((*p, fp));
        Ok(())
    }

    fn screen_effect(&self, _: &mut dyn GuestMemory, call: ScreenEffect, fp: Fp) -> Result<bool, String> {
        self.effects.lock().unwrap().push((call, fp));
        Ok(*self.ended.lock().unwrap())
    }
}

fn panic_text(f: impl FnOnce()) -> String {
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("a panic");
    err.downcast_ref::<String>().cloned().or_else(|| err.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
}

/// The host gets each call's words; a glow the guest would draw
/// somewhere the native pass does not is a named panic, as is a call
/// whose stack does not read.
#[test]
fn the_host_gets_the_calls_words() {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(ESP, 0x0001_39ba).unwrap();
    let args = [0x407f_7cee, 0x3f00_0000, 0x3e00_0000, 0x3e80_0000, 0, 1, 0x0000_0100];
    for (i, a) in args.iter().enumerate() {
        m.set_u32(ESP + 4 + 4 * i as u32, *a).unwrap();
    }
    let fp = Fp { mxcsr: 0x1f80, x87_control: 0x027f };
    let stub = Arc::new(Stub::default());
    native_post::set_host(stub.clone());
    let before = native_post::counts();

    // The colour pass draws as its parameters say.
    for one in [0.0f32, 1.0] {
        m.set_u32(0x0045_dbc0, one.to_bits()).unwrap();
        let p = engine::colour_pass::parameters(&m, args, fp.mxcsr).unwrap();
        let asked = stub.glows.lock().unwrap().len();
        m.set_u32(native_post::EXIT_TARGET, u32::MAX).unwrap();
        assert_eq!(native_post::colour_pass(&mut m, ESP, fp), p.draws);
        assert_eq!(stub.glows.lock().unwrap().len(), asked + usize::from(p.draws));
        if p.draws {
            assert_eq!(stub.glows.lock().unwrap().last().map(|(q, f)| (q.args, *f)), Some((args, fp)));
            // Another target bound at the end: not where the guest draws.
            m.set_u32(native_post::EXIT_TARGET, 3).unwrap();
            let text = panic_text(|| {
                native_post::colour_pass(&mut m, ESP, fp);
            });
            assert!(text.contains("post: glow target"), "{text}");
        }
    }
    let text = panic_text(|| {
        let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
        native_post::colour_pass(&mut m, 0x0001_0000, fp);
    });
    assert!(text.contains("post: colour pass args"), "{text}");

    // A screen effect: EAX the datum, then the four words; its values
    // copied where its passes' constants read them.
    m.set_u32(ESP, 0x0001_3b0c).unwrap();
    for (i, w) in [0x0060_1000u32, 2, 2, 0x0060_2000].iter().enumerate() {
        m.set_u32(ESP + 4 + 4 * i as u32, *w).unwrap();
    }
    for (i, v) in [1u32, 2, 3, 4].iter().enumerate() {
        m.set_u32(0x0060_1000 + 4 * i as u32, *v).unwrap();
    }
    assert_eq!(native_post::screen_effect(&mut m, 0xe001_0002, ESP, fp), 0xe001_0002, "returned at once: EAX kept");
    let want = ScreenEffect { datum: 0xe001_0002, values: 0x0060_1000, columns: 2, rows: 2, corners: 0x0060_2000 };
    assert_eq!(stub.effects.lock().unwrap().last().copied(), Some((want, fp)));
    for i in 0..4 {
        assert_eq!(m.u32(native_post::SCREEN_VALUES + 4 * i).unwrap(), i + 1);
    }
    *stub.ended.lock().unwrap() = true;
    assert_eq!(native_post::screen_effect(&mut m, 0xe001_0002, ESP, fp), u32::MAX, "the end's `or eax, -1`");
    let text = panic_text(|| {
        native_post::screen_effect(&mut m, 0, 0x0001_0000, fp);
    });
    assert!(text.contains("post: screen effect args"), "{text}");

    let after = native_post::counts();
    assert!(after.calls >= before.calls + 3);
    assert!(after.glows > before.glows);
    assert!(after.effects >= before.effects + 2);
    assert!(after.effects_ended > before.effects_ended);
}

/// `0x0001ddd0`: the table's size and both headers' size words, zero
/// unless `+0x94` is set.
#[test]
fn a_resize_writes_the_table_and_both_headers() {
    let mut m = FlatMemory::new(0x0040_0000, 0x0020_0000);
    let e = native_post::TARGETS + native_post::TARGET_STRIDE;
    native_post::resize(&mut m, 1, 320, 240).unwrap();
    assert_eq!((m.u32(e + 0x80).unwrap(), m.u32(e + 0x84).unwrap()), (320, 240));
    assert_eq!((m.u32(e + 0x14).unwrap(), m.u32(e + 0x28).unwrap()), (0, 0), "+0x94 clear");
    m.write(e + 0x94, &[1]).unwrap();
    native_post::resize(&mut m, 1, 320, 240).unwrap();
    // Pitch 320 * 4 / 64 = 20, less one at bit 24; height and width less one.
    let word = (19 << 24) | (239 << 12) | 319;
    assert_eq!((m.u32(e + 0x14).unwrap(), m.u32(e + 0x28).unwrap()), (word, word));
}

/// The call's end gives targets 1, 0x19 and 0x1b their saved sizes, 0x19
/// a second time 0x1a's, and clears the template cache and the results.
#[test]
fn the_end_writes_the_saved_sizes_back_in_the_originals_order() {
    let mut m = FlatMemory::new(0x0040_0000, 0x0020_0000);
    let e = |t: u32| native_post::TARGETS + t * native_post::TARGET_STRIDE;
    for (t, w) in [(1u32, 640u32), (0x19, 160), (0x1a, 80), (0x1b, 40)] {
        m.set_u32(e(t) + 0x88, 0x0050_0000).unwrap();
        native_post::resize(&mut m, t, w, w).unwrap();
    }
    m.write(e(0x1b) + 0x95, &[1]).unwrap();
    let saved = native_post::save_sizes(&m).unwrap();
    assert_eq!(saved, [(640, 640), (160, 160), (80, 80), (0, 0)], "a hidden size saves as 0");
    for t in native_post::SAVED {
        native_post::resize(&mut m, t, 8, 8).unwrap();
    }
    m.set_u32(0x0045_dbd4, 0x3f80_0000).unwrap();
    native_post::end_writes(&mut m, &saved).unwrap();
    let size = |t: u32| (m.u32(e(t) + 0x80).unwrap(), m.u32(e(t) + 0x84).unwrap());
    assert_eq!(size(1), (640, 640));
    assert_eq!(size(0x19), (80, 80), "0x1a's saved size, written second");
    assert_eq!(size(0x1a), (8, 8), "what the call set");
    assert_eq!(size(0x1b), (0, 0));
    assert_eq!(m.u8(native_post::END_FLAG).unwrap(), 1);
    let c = native_post::TEMPLATE_CACHE;
    assert_eq!((m.u32(c).unwrap(), m.u32(c + 0x40).unwrap()), (u32::MAX, u32::MAX));
    assert_eq!(m.u32(c + 0x1330).unwrap(), 0x3f80_0000);
    assert_eq!(native_post::RESULTS.map(|r| m.u32(r).unwrap()), [u32::MAX; 2]);
    m.set_u32(c + 0x24, 0x0050_0000).unwrap();
    assert_eq!(native_post::end_writes(&mut m, &saved), Err("template_restore".into()), "restores are D3D calls");
}

/// `0x00014f60`'s texture for a target: the two buffer cases by the frame
/// count's parity, the fixed ones by header.
#[test]
fn a_targets_texture_is_its_case() {
    let mut m = FlatMemory::new(0x0001_0000, 0x0060_0000);
    let case = |m: &mut FlatMemory, code: u32| {
        m.write(native_post::TEXTURE_CASE, &[1]).unwrap();
        m.set_u32(native_post::TEXTURE_JUMP + 4, code).unwrap();
    };
    m.set_u32(native_post::BUFFER_TEXTURES, 0xa0).unwrap();
    m.set_u32(native_post::BUFFER_TEXTURES + 4, 0xa1).unwrap();
    m.set_u32(0x0050_9374, 0xb0).unwrap();
    m.set_u32(native_post::FRAME_COUNT, 5).unwrap();
    case(&mut m, 0x0001_4fd4);
    assert_eq!(native_post::target_texture(&m, 0), Ok(0xa1), "5 % 2");
    case(&mut m, 0x0001_4fa7);
    assert_eq!(native_post::target_texture(&m, 0), Ok(0xa0), "(5 - 1) % 2");
    case(&mut m, 0x0001_4ffc);
    assert_eq!(native_post::target_texture(&m, 0), Ok(0xb0));
    case(&mut m, 0x0001_5100);
    assert_eq!(native_post::target_texture(&m, 0), Err("texture case"));
}
