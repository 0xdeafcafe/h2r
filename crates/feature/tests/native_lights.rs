//! The `native-lights` feature (docs/graphics/native-renderer.md, stage
//! 4): its plan (two context-only hooks the pipelined render runs on its
//! own thread, none with render-timing on) and its skip on synthetic guest
//! memory: the lights the host draws join one run until the device issues
//! its pass, a D3D light between them starts a new run, a new view forgets
//! the open run, and nothing is skipped without a host, for a light the
//! host does not draw, a record that is gone or fails its checks, or when
//! the device cannot draw the pass or do the passes' tail. A skipped light
//! leaves the guest words the tail zeroes at zero, after the host's device
//! side of the tail with the game's clip words (the device side itself:
//! d3d's `light_tail` test); a light left to D3D leaves them alone. A light
//! is skipped only while the device's bound target is the one recorded
//! before the view's lights, and not while its sphere meets a live object's.

use std::sync::{Arc, Mutex};

use engine::datum::layout as dl;
use engine::memory::{FlatMemory, GuestMemory};
use engine::render::light;
use feature::native_lights::{self, Host, NativeLights};
use feature::plan::Base;
use feature::{Registry, Switches};
use native_render::camera::{RENDER_CAMERA, RenderCamera};
use native_render::lights::{Light, Run, TAIL_CLIP, TAIL_ZEROES};

#[test]
fn the_switch_off_places_nothing() {
    let mut r = Registry::new(Switches::parse("").unwrap());
    r.add(&NativeLights).unwrap();
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    assert!(!text.contains("native_lights"), "{text}");
    // Nothing placed, nothing held: no snapshot blocker.
    assert!(plan.snapshot_blockers().is_empty());
}

#[test]
fn its_chains_run_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("impl.render.native_lights=on").unwrap());
    assert!(r.add(&NativeLights).unwrap());
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    assert!(text.contains("native_lights_skip"), "{text}");
    assert!(text.contains("native_lights_view"), "{text}");
    for point in ["RENDER_LIGHT_PASSES", "RENDER_DYNAMIC_LIGHTS"] {
        let chain = plan
            .chains
            .values()
            .find(|c| c.point.name == point)
            .unwrap_or_else(|| panic!("placed at {point}"));
        assert!(!chain.needs_shadow(), "{point}: nothing after the body");
        assert!(chain.render_thread().is_ok(), "{point}");
    }
}

#[test]
fn splitting_places_its_layer_observers_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("impl.render.native_lights=on").unwrap());
    r.add(&NativeLights).unwrap();
    assert!(!r.build(&Base).unwrap().text().contains("native_lights_split"), "off by default");
    let mut r = Registry::new(Switches::parse("impl.render.native_lights=on,impl.render.native_lights_split=on").unwrap());
    r.add(&NativeLights).unwrap();
    let plan = r.build(&Base).unwrap();
    for point in ["LAYER_DRAW", "LAYER_END"] {
        let chain = plan
            .chains
            .values()
            .find(|c| c.point.name == point)
            .unwrap_or_else(|| panic!("placed at {point}"));
        assert!(!chain.needs_shadow(), "{point}: nothing after the body");
        assert!(chain.render_thread().is_ok(), "{point}");
    }
    // Without the lights, nothing splits.
    let mut r = Registry::new(Switches::parse("impl.render.native_lights_split=on").unwrap());
    r.add(&NativeLights).unwrap();
    assert!(!r.build(&Base).unwrap().text().contains("native_lights"));
}

#[test]
fn render_timing_on_places_nothing() {
    let mut r = Registry::new(Switches::parse("impl.render.native_lights=on,render-timing").unwrap());
    r.add(&NativeLights).unwrap();
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    assert!(!text.contains("native_lights"), "{text}");
    // Nothing placed, nothing held: no snapshot blocker.
    assert!(plan.snapshot_blockers().is_empty());
}

const ARRAY: u32 = 0x0058_0000;
const DATA: u32 = 0x0059_0000;
/// Where the hook's ESP points (the return address; the arguments above).
const ESP: u32 = 0x0060_0000;

#[derive(Default)]
struct Stub {
    runs: Mutex<Vec<Arc<Run>>>,
    refuse: Mutex<bool>,
    tails: Mutex<Vec<[u32; 2]>>,
    refuse_tail: Mutex<bool>,
    /// The bound target moved off the view's (say, to a shadow target).
    moved: Mutex<bool>,
}

impl Host for Stub {
    fn accepts(&self, _mem: &dyn GuestMemory, light: &Light) -> bool {
        light.definition == 0xe000_0010
    }
    fn open(&self, camera: &RenderCamera, run: Arc<Run>) -> bool {
        assert_eq!(camera.far, 1024.0);
        if *self.refuse.lock().unwrap() {
            return false;
        }
        self.runs.lock().unwrap().push(run);
        true
    }
    fn tail(&self, _mem: &mut dyn GuestMemory, clip: [u32; 2]) -> bool {
        if *self.refuse_tail.lock().unwrap() {
            return false;
        }
        self.tails.lock().unwrap().push(clip);
        true
    }
    fn target(&self) -> Option<[u32; 8]> {
        let rt = if *self.moved.lock().unwrap() { 0x0080_1000 } else { 0x0080_0000 };
        Some([rt, 0x0080_2000, 0, 0, 640, 480, 0, 1f32.to_bits()])
    }
}

/// The tail's words set as a drawn light's passes leave them mid-way.
fn dirty(m: &mut FlatMemory) {
    for &(addr, _) in &TAIL_ZEROES {
        m.set_u8(addr, 0x5a).unwrap();
    }
}

fn zeroed(m: &FlatMemory) -> bool {
    TAIL_ZEROES.iter().all(|&(addr, bytes)| m.bytes(addr, bytes as usize).unwrap().iter().all(|&b| b == 0))
}

fn untouched(m: &FlatMemory) -> bool {
    TAIL_ZEROES.iter().all(|&(addr, _)| m.u8(addr).unwrap() == 0x5a)
}

fn put(m: &mut FlatMemory, at: u32, v: &[f32]) {
    for (i, x) in v.iter().enumerate() {
        m.set_u32(at + 4 * i as u32, x.to_bits()).unwrap();
    }
}

/// Light `index` (salt `0x8000 + index`) of definition `definition`, at
/// x = index; returns its datum.
fn light(m: &mut FlatMemory, index: u32, definition: u32) -> u32 {
    let at = DATA + light::RECORD_SIZE as u32 * index;
    let salt = 0x8000 + index;
    m.set_u32(at, salt | u32::from(light::flags::LIVE) << 16).unwrap();
    m.set_u32(at + 4, definition).unwrap();
    put(m, at + 0x18, &[index as f32, 0.0, 0.0, 5.0]);
    m.set_u32(at + 0x4c, u32::MAX).unwrap();
    m.set_u32(at + 0x58, u32::MAX).unwrap();
    m.set_u32(at + 0x5c, 0xffff).unwrap();
    put(m, at + 0x84, &[index as f32, 0.0, 0.0]);
    put(m, at + 0x90, &[index as f32 + 1.0, 0.0, 0.0]);
    put(m, at + 0xac, &[1.0, 0.0, 0.0]);
    put(m, at + 0xb8, &[0.0, 0.0, 1.0]);
    put(m, at + 0xcc, &[1.0, 0.75]);
    put(m, at + 0xd4, &[1.0, 0.5, 0.25, 0.0, 0.5, 1.0]);
    salt << 16 | index
}

/// The passes' stack for `handle`: the return address, then `(light,
/// &position, shadow, weight)`.
fn call(m: &mut FlatMemory, handle: u32, shadow: bool) -> u32 {
    m.set_u32(ESP, 0x0003_1f2b).unwrap();
    m.set_u32(ESP + 4, handle).unwrap();
    m.set_u32(ESP + 8, DATA).unwrap();
    m.set_u32(ESP + 0xc, u32::from(shadow)).unwrap();
    m.set_u32(ESP + 0x10, 0.5f32.to_bits()).unwrap();
    ESP
}

fn guest() -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(light::ARRAY_POINTER, ARRAY).unwrap();
    m.set_i32(ARRAY + dl::MAXIMUM_COUNT, light::MAXIMUM_COUNT).unwrap();
    m.set_i32(ARRAY + dl::ELEMENT_SIZE, light::RECORD_SIZE as i32).unwrap();
    m.set_i32(ARRAY + dl::HIGH_WATER, 8).unwrap();
    m.set_u32(ARRAY + dl::DATA, DATA).unwrap();
    let c = RENDER_CAMERA;
    put(&mut m, c, &[0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);
    put(&mut m, c + 0x28, &[1.2]);
    m.set_u32(c + 0x34, 480 | (640 << 16)).unwrap();
    put(&mut m, c + 0x40, &[0.0625, 1024.0]);
    m.set_u32(TAIL_CLIP[0], 0x3f80_0000).unwrap();
    m.set_u32(TAIL_CLIP[1], 0x4b7f_ffff).unwrap();
    m
}

/// One test, since the host and the open run are process globals.
#[test]
fn native_lights_join_runs_between_the_d3d_lights() {
    let mut m = guest();
    let a = light(&mut m, 1, 0xe000_0010);
    let b = light(&mut m, 2, 0xe000_0010);
    let d3d = light(&mut m, 3, 0xe000_0011);
    let c = light(&mut m, 4, 0xe000_0010);

    // No host yet: every light keeps its passes.
    dirty(&mut m);
    let esp = call(&mut m, a, false);
    assert!(!native_lights::on_light(&mut m, esp));
    assert!(untouched(&m));

    let stub = Arc::new(Stub::default());
    native_lights::set_host(stub.clone());
    native_lights::new_view();

    // A (shadowed) and B: one run, one pass asked for; the record is copied,
    // and the tail done: the device side with the game's clip words, then
    // the guest words zeroed.
    let esp = call(&mut m, a, true);
    assert!(native_lights::on_light(&mut m, esp));
    assert!(zeroed(&m));
    assert_eq!(*stub.tails.lock().unwrap(), vec![[0x3f80_0000, 0x4b7f_ffff]]);
    let esp = call(&mut m, b, false);
    assert!(native_lights::on_light(&mut m, esp));
    assert_eq!(stub.runs.lock().unwrap().len(), 1);
    // A definition the host does not draw: its passes run, and their first
    // draw is the device's next entry, which issues the pass (seals the run).
    dirty(&mut m);
    let esp = call(&mut m, d3d, false);
    assert!(!native_lights::on_light(&mut m, esp));
    assert!(untouched(&m), "a D3D light's own tail zeroes them");
    assert_eq!(stub.tails.lock().unwrap().len(), 2);
    let first = stub.runs.lock().unwrap()[0].seal();
    assert_eq!(first.iter().map(|l| l.handle).collect::<Vec<_>>(), vec![a, b]);
    let l = first[0];
    assert_eq!((l.definition, l.shadow, l.weight), (0xe000_0010, true, 0.5));
    assert_eq!((l.position, l.forward, l.up), ([1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]));
    assert_eq!((l.colour_a, l.colour_b, l.intensity), ([1.0, 0.5, 0.25], [0.0, 0.5, 1.0], 0.75));
    assert_eq!(l.bounds, ([1.0, 0.0, 0.0], 5.0));
    assert!(!first[1].shadow);
    // C comes after the D3D light: a run of its own.
    let esp = call(&mut m, c, false);
    assert!(native_lights::on_light(&mut m, esp));
    assert_eq!(stub.runs.lock().unwrap().len(), 2);

    // The next view's lights: the open run is not joined.
    native_lights::new_view();
    let esp = call(&mut m, a, false);
    assert!(native_lights::on_light(&mut m, esp));
    assert_eq!(stub.runs.lock().unwrap().len(), 3);
    assert_eq!(stub.runs.lock().unwrap()[1].len(), 1);

    // The device cannot draw: the light keeps its passes, and no run is
    // left open.
    native_lights::new_view();
    *stub.refuse.lock().unwrap() = true;
    let esp = call(&mut m, b, false);
    assert!(!native_lights::on_light(&mut m, esp));
    *stub.refuse.lock().unwrap() = false;
    let esp = call(&mut m, b, false);
    assert!(native_lights::on_light(&mut m, esp));
    assert_eq!(stub.runs.lock().unwrap().len(), 4);

    // A stale datum (another salt) and a record that fails its checks (a
    // colour out of range): not read, so D3D.
    let esp = call(&mut m, a ^ 0x0001_0000, false);
    assert!(!native_lights::on_light(&mut m, esp));
    put(&mut m, DATA + light::RECORD_SIZE as u32 * 4 + 0xd4, &[2.0]);
    let esp = call(&mut m, c, false);
    assert!(!native_lights::on_light(&mut m, esp));

    // The device cannot do the tail: the light keeps its passes, the words
    // are left alone and no run opens.
    native_lights::new_view();
    dirty(&mut m);
    *stub.refuse_tail.lock().unwrap() = true;
    let esp = call(&mut m, a, false);
    assert!(!native_lights::on_light(&mut m, esp));
    assert!(untouched(&m));
    assert_eq!(stub.runs.lock().unwrap().len(), 4);
    *stub.refuse_tail.lock().unwrap() = false;

    // The bound target is not the view's any more: the light keeps its
    // passes (their tail sets the target back), with no tail of ours.
    native_lights::new_view();
    *stub.moved.lock().unwrap() = true;
    dirty(&mut m);
    let tails = stub.tails.lock().unwrap().len();
    let esp = call(&mut m, a, false);
    assert!(!native_lights::on_light(&mut m, esp));
    assert!(untouched(&m));
    assert_eq!(stub.tails.lock().unwrap().len(), tails);
    assert_eq!(native_lights::target_refusals(), 1);
    // Back on the view's target: skipped again.
    *stub.moved.lock().unwrap() = false;
    let esp = call(&mut m, a, false);
    assert!(native_lights::on_light(&mut m, esp));
    assert_eq!(stub.runs.lock().unwrap().len(), 5);

    let (lights, skipped, runs) = native_lights::counts();
    assert_eq!((lights, skipped, runs), (13, 6, 5));

    // A live object whose sphere meets the light's (A: (1, 0, 0), r 5):
    // the light keeps its D3D passes, which light the object; counted.
    let objects = 0x005a_0000;
    let body = objects + 0x1000;
    m.set_u32(native_render::objects::OBJECTS, objects).unwrap();
    m.set_u32(objects + 0x38, 3).unwrap();
    m.set_u32(objects + 0x44, objects + 0x100).unwrap();
    m.set_u32(objects + 0x100 + 0xc * 2, 0x8002).unwrap();
    m.set_u32(objects + 0x100 + 0xc * 2 + 8, body).unwrap();
    put(&mut m, body + 0x30, &[7.0, 0.0, 0.0, 1.5]);
    dirty(&mut m);
    let esp = call(&mut m, a, false);
    assert!(!native_lights::on_light(&mut m, esp));
    assert!(untouched(&m));
    assert_eq!(native_lights::object_refusals(), 1);
    // The object moves out of reach: skipped again (joins the open run).
    put(&mut m, body + 0x30, &[7.0, 0.0, 0.0, 0.5]);
    let esp = call(&mut m, a, false);
    assert!(native_lights::on_light(&mut m, esp));
    assert_eq!(native_lights::object_refusals(), 1);
    assert_eq!(native_lights::counts(), (15, 7, 5));

    // A snapshot load forgets the open run (it is the saving render's):
    // the next light opens one of its own.
    native_lights::forget_runs();
    let esp = call(&mut m, a, false);
    assert!(native_lights::on_light(&mut m, esp));
    assert_eq!(stub.runs.lock().unwrap().len(), 6);
    assert_eq!(native_lights::counts(), (16, 8, 6));
}

/// Placed, its section (no bytes) forgets the open run and the split
/// light at a load.
#[test]
fn placed_its_snapshot_policy_is_a_section() {
    let mut r = Registry::new(Switches::parse("impl.render.native_lights=on").unwrap());
    assert!(r.add(&NativeLights).unwrap());
    let plan = r.build(&Base).unwrap();
    let policies: Vec<_> = plan
        .snapshot
        .iter()
        .map(|f| (f.id, f.policy.name()))
        .collect();
    assert_eq!(policies, vec![(native_lights::ID, "section")]);
    assert!(plan.snapshot_blockers().is_empty());
}
