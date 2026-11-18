//! The `native-world` feature (docs/graphics/native-renderer.md): its plan
//! next to every other hook on the layer draw, and its two observers on
//! synthetic guest memory: the BSP items the host draws lose layer 3's bit
//! for the layer and get it back after; nothing changes without a host,
//! in another layer, or for a shader the host leaves to the D3D path.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_world::{self, Host, NativeWorld, ShaderSet};
use feature::plan::Base;
use feature::{Registry, Switches};
use native_render::camera::{RENDER_CAMERA, RenderCamera};
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, RENDER_RECORDS};

#[test]
fn the_plan_orders_it_inside_render_timing_with_the_layer_ports() {
    let switches = "impl.render.native_world=on,render-timing=on,render-timing.deep=on,impl.render.layer_draw=on";
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    assert!(r.add(&feature::render_timing::RenderTiming::new()).unwrap());
    assert!(r.add(&feature::render_loop::RenderLoop).unwrap());
    assert!(r.add(&NativeWorld).unwrap());
    let plan = match r.build(&Base) {
        Ok(p) => p,
        Err(c) => panic!("refused: {c:?}"),
    };
    let text = plan.text();
    assert!(text.contains("native_world_skip") && text.contains("native_world_restore"), "{text}");
}

#[test]
fn the_switch_off_places_nothing() {
    let mut r = Registry::new(Switches::parse("").unwrap());
    r.add(&NativeWorld).unwrap();
    let text = r.build(&Base).unwrap().text();
    assert!(!text.contains("native_world"), "{text}");
}

const LIST: u32 = 0x004c_1a48;
const ITEMS: u32 = 0x0060_0000;
const BSP: u32 = 0x0080_0000;

struct Stub {
    asked: Mutex<Vec<(u32, RenderCamera)>>,
    shaders: Arc<ShaderSet>,
}

impl Host for Stub {
    fn ask(&self, _mem: &dyn GuestMemory, bsp: u32, camera: &RenderCamera) -> Option<Arc<ShaderSet>> {
        self.asked.lock().unwrap().push((bsp, *camera));
        Some(Arc::clone(&self.shaders))
    }
}

/// Layer 3 current; two BSP items (shaders 0x10 and 0x11) and an object
/// item, all in layers 1 and 3; a usable camera; the active BSP.
fn guest() -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(CURRENT_LAYER, 3).unwrap();
    m.set_u32(CURRENT_LIST, LIST).unwrap();
    m.set_u32(LIST + 4, 3).unwrap();
    m.set_u32(LIST + 0xc, 0).unwrap();
    m.set_u32(LIST + 0x14, ITEMS).unwrap();
    for (i, (shader, record, handle)) in [(0x10u32, 1u32, u32::MAX), (0x11, 2, u32::MAX), (0x10, 3, 0xe123_0001)]
        .into_iter()
        .enumerate()
    {
        let a = ITEMS + 0x18 * i as u32;
        m.set_u32(a, GEOMETRY_BUILD).unwrap();
        m.set_u32(a + 8, 0xe000_0000 | shader).unwrap();
        m.set_u32(a + 0xc, record << 8).unwrap();
        m.set_u32(a + 0x10, 0b1010).unwrap();
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
    m.set_u32(native_world::ACTIVE_BSP, BSP).unwrap();
    m
}

fn masks(m: &FlatMemory) -> Vec<u32> {
    (0..3).map(|i| m.u32(ITEMS + 0x18 * i + 0x10).unwrap()).collect()
}

#[test]
fn the_world_items_skip_layer_3_and_get_it_back() {
    // No host yet: nothing changes.
    let mut m = guest();
    native_world::before_layer(&mut m);
    assert_eq!(masks(&m), vec![0b1010; 3]);
    native_world::after_layer(&mut m);

    let stub = Arc::new(Stub {
        asked: Mutex::default(),
        shaders: Arc::new([0x10u16].into_iter().collect()),
    });
    native_world::set_host(stub.clone());

    native_world::before_layer(&mut m);
    assert_eq!(
        masks(&m),
        vec![0b0010, 0b1010, 0b1010],
        "the drawn shader's BSP item only: not the D3D-path shader, not the object"
    );
    {
        let asked = stub.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0, BSP);
        assert_eq!(asked[0].1.far, 1024.0);
    }
    native_world::after_layer(&mut m);
    assert_eq!(masks(&m), vec![0b1010; 3]);

    // Another layer: the host is not asked.
    m.set_u32(CURRENT_LAYER, 1).unwrap();
    native_world::before_layer(&mut m);
    native_world::after_layer(&mut m);
    assert_eq!(masks(&m), vec![0b1010; 3]);
    assert_eq!(stub.asked.lock().unwrap().len(), 1);
}
