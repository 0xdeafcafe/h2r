//! Split lights (`impl.render.native_lights_split`, docs/graphics/
//! native-renderer.md, stage 4): a light the host does not draw whole but
//! splits joins the native run and keeps its D3D passes, and those passes'
//! layers 14, 12 and 13 run without the world items of the shaders the
//! host draws natively, which the layer's end sets back. Synthetic guest
//! memory; a test binary of its own, since the host, the run and the
//! split are process globals.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use engine::datum::layout as dl;
use engine::memory::{FlatMemory, GuestMemory};
use engine::render::light;
use feature::native_lights::{self, Host, LIGHT_LIST, SPLIT_LAYERS};
use native_render::camera::{RENDER_CAMERA, RenderCamera};
use native_render::lights::{Light, Run};
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, RENDER_RECORDS};

const ARRAY: u32 = 0x0058_0000;
const DATA: u32 = 0x0059_0000;
const ESP: u32 = 0x0060_0000;
const ITEMS: u32 = 0x005c_0000;
/// Drawn whole by the host.
const WHOLE: u32 = 0xe000_0010;
/// Split by the host: shader 5's surfaces native.
const SPLIT: u32 = 0xe000_0011;
const NATIVE_SHADER: u32 = 0xe000_0005;
const D3D_SHADER: u32 = 0xe000_0006;

#[derive(Default)]
struct Stub {
    runs: Mutex<Vec<Arc<Run>>>,
}

impl Host for Stub {
    fn accepts(&self, _mem: &dyn GuestMemory, light: &Light) -> bool {
        light.definition == WHOLE
    }
    fn open(&self, _camera: &RenderCamera, run: Arc<Run>) -> bool {
        self.runs.lock().unwrap().push(run);
        true
    }
    fn tail(&self, _mem: &mut dyn GuestMemory, _clip: [u32; 2]) -> bool {
        true
    }
    fn target(&self) -> Option<[u32; 8]> {
        Some([0x0080_0000, 0x0080_2000, 0, 0, 640, 480, 0, 1f32.to_bits()])
    }
    fn split(&self, _mem: &dyn GuestMemory, light: &Light) -> Option<Arc<HashSet<u16>>> {
        (light.definition == SPLIT).then(|| Arc::new([NATIVE_SHADER as u16].into_iter().collect()))
    }
}

fn put(m: &mut FlatMemory, at: u32, v: &[f32]) {
    for (i, x) in v.iter().enumerate() {
        m.set_u32(at + 4 * i as u32, x.to_bits()).unwrap();
    }
}

/// Light `index` of definition `definition`; returns its datum.
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

fn call(m: &mut FlatMemory, handle: u32) -> u32 {
    m.set_u32(ESP, 0x0003_1f2b).unwrap();
    m.set_u32(ESP + 4, handle).unwrap();
    m.set_u32(ESP + 8, DATA).unwrap();
    m.set_u32(ESP + 0xc, 0).unwrap();
    m.set_u32(ESP + 0x10, 0.5f32.to_bits()).unwrap();
    ESP
}

const LAYERS: u32 = 1 << 12 | 1 << 13 | 1 << 14;

/// The light's list as its layers see it: a BSP item of the native shader,
/// one of a shader left to D3D, and an object's item of the native shader
/// (render records 2 and 7 BSP, 5 an object's).
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

    m.set_u32(CURRENT_LIST, LIGHT_LIST).unwrap();
    m.set_u32(LIGHT_LIST + 4, 3).unwrap();
    m.set_u32(LIGHT_LIST + 0xc, 0).unwrap();
    m.set_u32(LIGHT_LIST + 0x14, ITEMS).unwrap();
    for (i, (shader, record)) in [(NATIVE_SHADER, 2u32), (D3D_SHADER, 7), (NATIVE_SHADER, 5)].into_iter().enumerate() {
        let a = ITEMS + ITEM_SIZE * i as u32;
        m.set_u32(a, GEOMETRY_BUILD).unwrap();
        m.set_u32(a + 8, shader).unwrap();
        m.set_u32(a + 0xc, record << 8 | 1).unwrap();
        m.set_u32(a + 0x10, LAYERS | 1 << 11).unwrap();
    }
    m.set_u32(RENDER_RECORDS + 0x20 * 2 + 4, u32::MAX).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * 7 + 4, u32::MAX).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * 5 + 4, 0xe1a3_0004).unwrap();
    m
}

fn masks(m: &FlatMemory) -> Vec<u32> {
    (0..3).map(|i| m.u32(ITEMS + ITEM_SIZE * i + 0x10).unwrap()).collect()
}

const ALL: u32 = LAYERS | 1 << 11;

#[test]
fn a_split_light_lights_each_surface_once() {
    let mut m = guest();
    let whole = light(&mut m, 1, WHOLE);
    let split = light(&mut m, 2, SPLIT);
    let d3d = light(&mut m, 3, 0xe000_0012);
    let stub = Arc::new(Stub::default());
    native_lights::set_host(stub.clone());

    // Splitting off: the light the host splits keeps all its passes and
    // joins nothing.
    native_lights::set_split(false);
    native_lights::new_view();
    let esp = call(&mut m, split);
    assert!(!native_lights::on_light(&mut m, esp));
    assert!(stub.runs.lock().unwrap().is_empty());

    native_lights::set_split(true);
    // Skipped whole, then split: both join the run, and the split light's
    // passes run (its own tail too).
    let esp = call(&mut m, whole);
    assert!(native_lights::on_light(&mut m, esp));
    let esp = call(&mut m, split);
    assert!(!native_lights::on_light(&mut m, esp), "its D3D passes run");
    assert_eq!(stub.runs.lock().unwrap().len(), 1);
    let run = stub.runs.lock().unwrap()[0].seal();
    assert_eq!(run.iter().map(|l| l.handle).collect::<Vec<_>>(), vec![whole, split]);
    assert_eq!(native_lights::split_counts().0, 1);

    // Its layers 14, 12 and 13: the native shader's BSP item goes without
    // the layer; the D3D shader's item and the object's keep it; the
    // layer's end sets it back.
    for (layer, ret) in SPLIT_LAYERS {
        m.set_u32(CURRENT_LAYER, layer).unwrap();
        native_lights::split_layer(&mut m, ret);
        assert_eq!(masks(&m), vec![ALL & !(1 << layer), ALL, ALL], "layer {layer}");
        native_lights::layer_end(&mut m);
        assert_eq!(masks(&m), vec![ALL; 3], "layer {layer} back");
    }
    assert_eq!(native_lights::split_counts(), (1, 3, 3));

    // Not one of its split layers: layer 11 (shadowed lights), another
    // call site (the first-person lights draw the same list), another list.
    m.set_u32(CURRENT_LAYER, 11).unwrap();
    native_lights::split_layer(&mut m, 0x0003_134d);
    assert_eq!(masks(&m), vec![ALL; 3]);
    m.set_u32(CURRENT_LAYER, 12).unwrap();
    native_lights::split_layer(&mut m, 0x0004_0642);
    assert_eq!(masks(&m), vec![ALL; 3]);
    m.set_u32(CURRENT_LIST, 0x004c_1a48).unwrap();
    native_lights::split_layer(&mut m, 0x0003_144f);
    assert_eq!(masks(&m), vec![ALL; 3]);
    m.set_u32(CURRENT_LIST, LIGHT_LIST).unwrap();

    // A layer left out and not ended (no `LAYER_END` came): the next
    // layer draw sets it back first.
    native_lights::split_layer(&mut m, 0x0003_144f);
    assert_eq!(masks(&m)[0], ALL & !(1 << 12));
    m.set_u32(CURRENT_LAYER, 11).unwrap();
    native_lights::split_layer(&mut m, 0x0003_134d);
    assert_eq!(masks(&m), vec![ALL; 3]);

    // The next light's passes: the split is over.
    let esp = call(&mut m, d3d);
    assert!(!native_lights::on_light(&mut m, esp));
    m.set_u32(CURRENT_LAYER, 12).unwrap();
    native_lights::split_layer(&mut m, 0x0003_144f);
    assert_eq!(masks(&m), vec![ALL; 3]);
    assert_eq!(native_lights::split_counts().0, 1);

    // So is it at a new view.
    let esp = call(&mut m, split);
    assert!(!native_lights::on_light(&mut m, esp));
    assert_eq!(native_lights::split_counts().0, 2);
    native_lights::new_view();
    native_lights::split_layer(&mut m, 0x0003_144f);
    assert_eq!(masks(&m), vec![ALL; 3]);
}
