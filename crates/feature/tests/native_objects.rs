//! The `native-objects` feature (docs/graphics/native-renderer.md, stage
//! 2): its plan next to every other hook on the layer draw, and its two
//! observers on synthetic guest memory: the objects the host draws lose
//! layer 3's bit for the base layers' layer 3 and get it back after, with
//! their node matrices handed to the host; nothing changes without a host,
//! from another caller, for a shader or an object left to the D3D path,
//! or when the host cannot draw.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_objects::{self, Host, NativeObjects, NativeSet};
use feature::plan::Base;
use feature::{Registry, Switches};
use native_render::camera::{RENDER_CAMERA, RenderCamera};
use native_render::objects::{ObjectFrame, OBJECTS, SCENE_LAYER3_RETURN};
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, RENDER_RECORDS};

#[test]
fn the_plan_orders_it_inside_render_timing_and_the_world() {
    let switches = "impl.render.native_objects=on,impl.render.native_world=on,render-timing=on,render-timing.deep=on,impl.render.layer_draw=on";
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    assert!(r.add(&feature::render_timing::RenderTiming::new()).unwrap());
    assert!(r.add(&feature::render_loop::RenderLoop).unwrap());
    assert!(r.add(&feature::native_world::NativeWorld).unwrap());
    assert!(r.add(&NativeObjects).unwrap());
    let plan = match r.build(&Base) {
        Ok(p) => p,
        Err(c) => panic!("refused: {c:?}"),
    };
    let text = plan.text();
    assert!(text.contains("native_objects_skip") && text.contains("native_objects_restore"), "{text}");
}

#[test]
fn the_switch_off_places_nothing() {
    let mut r = Registry::new(Switches::parse("").unwrap());
    r.add(&NativeObjects).unwrap();
    let text = r.build(&Base).unwrap().text();
    assert!(!text.contains("native_objects"), "{text}");
}

const LIST: u32 = 0x004c_1a48;
const ARRAY: u32 = 0x0058_0000;
const HEADERS: u32 = 0x0058_1000;
const BODIES: u32 = 0x0059_0000;
const ITEMS: u32 = 0x0060_0000;
const DEF_A: u32 = 0xe000_0100;
const DEF_B: u32 = 0xe000_0200;

#[derive(Default)]
struct Stub {
    set: Mutex<NativeSet>,
    wanted: Mutex<Vec<u32>>,
    drawn: Mutex<Vec<(RenderCamera, Vec<ObjectFrame>)>>,
    refuse: Mutex<bool>,
}

impl Host for Stub {
    fn native(&self, _mem: &dyn GuestMemory) -> Option<Arc<NativeSet>> {
        Some(Arc::new(self.set.lock().unwrap().clone()))
    }
    fn want(&self, definition: u32) {
        self.wanted.lock().unwrap().push(definition);
    }
    fn draw(&self, camera: &RenderCamera, objects: Vec<ObjectFrame>) -> bool {
        if *self.refuse.lock().unwrap() {
            return false;
        }
        self.drawn.lock().unwrap().push((*camera, objects));
        true
    }
}

/// Object `slot` with definition `def` and `nodes` node matrices (node `n`
/// at x = n); returns its handle.
fn object(m: &mut FlatMemory, slot: u32, def: u32, nodes: u32) -> u32 {
    let salt = 0x8000 + slot;
    let header = HEADERS + 0xc * slot;
    let body = BODIES + 0x1000 * slot;
    m.set_u32(header, salt).unwrap();
    m.set_u32(header + 8, body).unwrap();
    m.set_u32(body, def).unwrap();
    m.set_u32(body + 0x3c, 1f32.to_bits()).unwrap();
    m.set_u32(body + 0x114, (0x34 * nodes) | (0x200 << 16)).unwrap();
    for n in 0..nodes {
        let at = body + 0x200 + 0x34 * n;
        let words = [1.0f32, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, n as f32, 0.0, 0.0];
        for (k, w) in words.into_iter().enumerate() {
            m.set_u32(at + 4 * k as u32, w.to_bits()).unwrap();
        }
    }
    salt << 16 | slot
}

/// Layer 3 current; items: object A (shader 0x10), A (0x11), object B
/// (0x10), a BSP item (0x10); all in layers 1 and 3. A usable camera.
fn guest() -> (FlatMemory, u32, u32) {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(OBJECTS, ARRAY).unwrap();
    m.set_u32(ARRAY + 0x38, 16).unwrap();
    m.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    let a = object(&mut m, 1, DEF_A, 2);
    let b = object(&mut m, 2, DEF_B, 1);
    m.set_u32(CURRENT_LAYER, 3).unwrap();
    m.set_u32(CURRENT_LIST, LIST).unwrap();
    m.set_u32(LIST + 4, 4).unwrap();
    m.set_u32(LIST + 0xc, 0).unwrap();
    m.set_u32(LIST + 0x14, ITEMS).unwrap();
    for (i, (shader, record, handle)) in [(0x10u32, 1u32, a), (0x11, 2, a), (0x10, 3, b), (0x10, 4, u32::MAX)]
        .into_iter()
        .enumerate()
    {
        let at = ITEMS + 0x18 * i as u32;
        m.set_u32(at, GEOMETRY_BUILD).unwrap();
        m.set_u32(at + 8, 0xe000_0000 | shader).unwrap();
        m.set_u32(at + 0xc, record << 8).unwrap();
        m.set_u32(at + 0x10, 0b1010).unwrap();
        m.set_u32(RENDER_RECORDS + 0x20 * record + 4, handle).unwrap();
    }
    let c = RENDER_CAMERA;
    for (i, v) in [0.0f32, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0].into_iter().enumerate() {
        m.set_u32(c + 4 * i as u32, v.to_bits()).unwrap();
    }
    m.set_u32(c + 0x28, 1.2f32.to_bits()).unwrap();
    m.set_u32(c + 0x34, 480 | (640 << 16)).unwrap();
    m.set_u32(c + 0x40, 0.0625f32.to_bits()).unwrap();
    m.set_u32(c + 0x44, 1024f32.to_bits()).unwrap();
    (m, a, b)
}

fn masks(m: &FlatMemory) -> Vec<u32> {
    (0..4).map(|i| m.u32(ITEMS + 0x18 * i + 0x10).unwrap()).collect()
}

/// One test, since the host and the cleared items are process globals.
#[test]
fn objects_skip_layer_3_hand_over_their_nodes_and_get_their_bits_back() {
    let all = vec![0b1010; 4];
    // No host yet: nothing changes.
    let (mut m, a, _b) = guest();
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), all);
    native_objects::after_layer(&mut m);

    let stub = Arc::new(Stub::default());
    {
        let mut set = stub.set.lock().unwrap();
        set.definitions.insert(DEF_A as u16, 2);
        set.shaders.insert(0x10);
    }
    native_objects::set_host(stub.clone());

    // Another caller of the layer draw (first person, another view kind).
    native_objects::before_layer(&mut m, 0x0002_afa5);
    assert_eq!(masks(&m), all);
    assert!(stub.drawn.lock().unwrap().is_empty());

    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        vec![0b0010, 0b1010, 0b1010, 0b1010],
        "A's native material only: not A's D3D-path shader, not B (no model yet), not the BSP"
    );
    assert_eq!(*stub.wanted.lock().unwrap(), vec![DEF_B], "B's model is asked for");
    {
        let drawn = stub.drawn.lock().unwrap();
        assert_eq!(drawn.len(), 1);
        assert_eq!(drawn[0].0.far, 1024.0);
        let objects = &drawn[0].1;
        assert_eq!(objects.len(), 1);
        assert_eq!((objects[0].handle, objects[0].definition, objects[0].nodes.len()), (a, DEF_A, 2));
        assert_eq!(objects[0].nodes[1].position, [1.0, 0.0, 0.0]);
    }
    native_objects::after_layer(&mut m);
    assert_eq!(masks(&m), all);

    // B's model loaded with another node count than the object has: B
    // stays on the D3D path.
    stub.set.lock().unwrap().definitions.insert(DEF_B as u16, 5);
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), vec![0b0010, 0b1010, 0b1010, 0b1010]);
    assert_eq!(stub.drawn.lock().unwrap()[1].1.len(), 1);
    native_objects::after_layer(&mut m);
    assert_eq!(masks(&m), all);

    // The device cannot draw: every bit back at once.
    *stub.refuse.lock().unwrap() = true;
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), all);
    native_objects::after_layer(&mut m);
    assert_eq!(masks(&m), all);

    // Another layer: nothing.
    *stub.refuse.lock().unwrap() = false;
    m.set_u32(CURRENT_LAYER, 1).unwrap();
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    native_objects::after_layer(&mut m);
    assert_eq!(masks(&m), all);
    assert_eq!(stub.drawn.lock().unwrap().len(), 2);
}
