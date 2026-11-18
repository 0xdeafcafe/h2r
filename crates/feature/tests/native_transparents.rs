//! The `native-transparents` feature (docs/graphics/native-renderer.md,
//! stage 3): its plan (two context-only hooks the pipelined render runs on
//! its own thread) and its skip on synthetic guest memory: the geometry
//! entries the host draws join one run until the device issues its pass,
//! a D3D entry between them starts a new run, a new view forgets the open
//! run, and nothing is skipped without a host, for an entry the host does
//! not draw, or when the device cannot.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_transparents::{self, Host, NativeTransparents};
use feature::plan::Base;
use feature::{Registry, Switches};
use native_render::camera::{RENDER_CAMERA, RenderCamera};
use native_render::skip::RENDER_RECORDS;
use native_render::transparent::{DATA_OFFSET, Entry, GEOMETRY_HANDLER, Run, Source};

#[test]
fn the_switch_off_places_nothing() {
    let mut r = Registry::new(Switches::parse("").unwrap());
    r.add(&NativeTransparents).unwrap();
    let text = r.build(&Base).unwrap().text();
    assert!(!text.contains("native_transparents"), "{text}");
}

#[test]
fn its_chains_run_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("impl.render.native_transparents=on").unwrap());
    assert!(r.add(&NativeTransparents).unwrap());
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    assert!(text.contains("native_transparents_skip"), "{text}");
    assert!(text.contains("native_transparents_view"), "{text}");
    for point in ["TRANSPARENT_GEOMETRY", "SCENE_TRANSPARENTS"] {
        let chain = plan
            .chains
            .values()
            .find(|c| c.point.name == point)
            .unwrap_or_else(|| panic!("placed at {point}"));
        assert!(!chain.needs_shadow(), "{point}: nothing after the body");
        assert!(chain.render_thread().is_ok(), "{point}");
    }
}

const ENTRIES: u32 = 0x0060_0000;

#[derive(Default)]
struct Stub {
    runs: Mutex<Vec<Arc<Run>>>,
    refuse: Mutex<bool>,
}

impl Host for Stub {
    fn accepts(&self, _mem: &dyn GuestMemory, entry: &Entry) -> bool {
        entry.shader as u16 == 0x10
    }
    fn open(&self, camera: &RenderCamera, run: Arc<Run>) -> bool {
        assert_eq!(camera.far, 1024.0);
        if *self.refuse.lock().unwrap() {
            return false;
        }
        self.runs.lock().unwrap().push(run);
        true
    }
}

/// Entry `i` (a BSP cluster's part 1 with shader `0xe000_0000 | shader`,
/// render record `i + 1`); returns the handler's argument.
fn entry(m: &mut FlatMemory, i: u32, shader: u32) -> u32 {
    let at = ENTRIES + 0x70 * i;
    let record = i + 1;
    m.set_u32(at + 4, (-(i as f32)).to_bits()).unwrap();
    m.set_u32(at + 0x20, GEOMETRY_HANDLER).unwrap();
    m.set_u32(at + 0x24, 0xe000_0000 | shader).unwrap();
    m.set_u32(at + 0x28, record << 8 | 1).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * record + 4, u32::MAX).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * record + 0xc, 0x7ff << 18 | (20 + i) << 9).unwrap();
    at + DATA_OFFSET
}

fn guest() -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    let c = RENDER_CAMERA;
    for (i, v) in [0.0f32, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0].into_iter().enumerate() {
        m.set_u32(c + 4 * i as u32, v.to_bits()).unwrap();
    }
    m.set_u32(c + 0x28, 1.2f32.to_bits()).unwrap();
    m.set_u32(c + 0x34, 480 | (640 << 16)).unwrap();
    m.set_u32(c + 0x40, 0.0625f32.to_bits()).unwrap();
    m.set_u32(c + 0x44, 1024f32.to_bits()).unwrap();
    m
}

/// One test, since the host and the open run are process globals.
#[test]
fn native_entries_join_runs_between_the_d3d_draws() {
    let mut m = guest();
    let a = entry(&mut m, 0, 0x10);
    let b = entry(&mut m, 1, 0x10);
    let d3d = entry(&mut m, 2, 0x11);
    let c = entry(&mut m, 3, 0x10);

    // No host yet: every entry keeps its handler.
    assert!(!native_transparents::on_entry(&m, a));

    let stub = Arc::new(Stub::default());
    native_transparents::set_host(stub.clone());
    native_transparents::new_view();

    // A and B: one run, one pass asked for.
    assert!(native_transparents::on_entry(&m, a));
    assert!(native_transparents::on_entry(&m, b));
    assert_eq!(stub.runs.lock().unwrap().len(), 1);
    // A shader the host does not draw: the handler runs, and its draw is
    // the device's next entry, which issues the pass (seals the run).
    assert!(!native_transparents::on_entry(&m, d3d));
    let first = stub.runs.lock().unwrap()[0].seal();
    assert_eq!(first.len(), 2);
    assert_eq!(first[0].source, Source::Cluster(20));
    assert_eq!((first[1].source, first[1].part), (Source::Cluster(21), 1));
    // C sorts after the D3D draw: a run of its own.
    assert!(native_transparents::on_entry(&m, c));
    assert_eq!(stub.runs.lock().unwrap().len(), 2);
    assert_eq!(stub.runs.lock().unwrap()[1].len(), 1);

    // The next view's transparents: the open run is not joined.
    native_transparents::new_view();
    assert!(native_transparents::on_entry(&m, a));
    assert_eq!(stub.runs.lock().unwrap().len(), 3);
    assert_eq!(stub.runs.lock().unwrap()[1].len(), 1);

    // The device cannot draw: the entry keeps its handler, and no run is
    // left open.
    native_transparents::new_view();
    *stub.refuse.lock().unwrap() = true;
    assert!(!native_transparents::on_entry(&m, b));
    *stub.refuse.lock().unwrap() = false;
    assert!(native_transparents::on_entry(&m, b));
    assert_eq!(stub.runs.lock().unwrap().len(), 4);

    // An entry without a render record: not geometry the hook reads.
    m.set_u32(c + 4, 0xffff_ff00).unwrap();
    assert!(!native_transparents::on_entry(&m, c));

    let (entries, skipped, runs) = native_transparents::counts();
    assert_eq!((entries, skipped, runs), (9, 5, 4));
}
