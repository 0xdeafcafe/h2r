//! The `native-objects` feature (docs/graphics/native-renderer.md, stage
//! 2): its plan next to every other hook on the layer draw, and its two
//! observers on synthetic guest memory: the objects the host draws lose
//! layer 3's bit for the base layers' layer 3 and get it back after, with
//! their node matrices handed to the host; nothing changes without a host,
//! from another caller (first person among them), for a shader or an
//! object left to the D3D path, or when the host cannot draw. Each item's
//! object is named as the game names it: the render record's lighting
//! pointer, the render-state cache record it points into, that record's
//! owner, confirmed by the owner's cache handle.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_objects::{self, Host, NativeObjects, NativeSet, SetPart, Stop};
use feature::plan::Base;
use feature::{Registry, Switches};
use native_render::camera::{RENDER_CAMERA, RenderCamera};
use native_render::object_nodes::{RECORD_IMPLEMENTATION, RECORD_KEY, RECORD_MATRIX};
use native_render::objects::{
    BODY_RENDER_STATE, CACHE_LIGHTING, CACHE_OWNER, CACHE_RECORD_SIZE, FIRST_PERSON_LAYER3_RETURN, ItemDraw, ItemKey, OBJECTS,
    ObjectFrame, RECORD_LIGHTING, RECORD_MODEL, RENDER_STATE_CACHE, SCENE_LAYER3_RETURN,
};
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
    assert!(text.contains("native_objects_skip"), "{text}");
    assert!(!text.contains("native_objects_restore"), "nothing after the body: {text}");
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
const CACHE: u32 = 0x0057_0000;
const CACHE_DATA: u32 = 0x0057_1000;
const ITEMS: u32 = 0x0060_0000;
const DEF_A: u32 = 0xe000_0100;
const DEF_B: u32 = 0xe000_0200;
/// The node matrix every record uploads.
const MATRIX: u32 = 0x0060_1000;

#[derive(Default)]
struct Stub {
    set: Mutex<NativeSet>,
    wanted: Mutex<Vec<u32>>,
    drawn: Mutex<Vec<(RenderCamera, Vec<ObjectFrame>, Vec<ItemDraw>)>>,
    refuse: Mutex<bool>,
    /// Records whose items the host refuses to draw.
    refuse_records: Mutex<Vec<u16>>,
    rebind_item: Mutex<Option<u32>>,
}

impl Host for Stub {
    fn native(&self, _mem: &mut dyn GuestMemory) -> Option<Arc<NativeSet>> {
        Some(Arc::new(self.set.lock().unwrap().clone()))
    }
    fn want(&self, definition: u32) {
        self.wanted.lock().unwrap().push(definition);
    }
    fn draw(
        &self, mem: &mut dyn GuestMemory, camera: &RenderCamera, _generation: u64,
        objects: Vec<ObjectFrame>, items: Vec<ItemDraw>, masks: native_objects::ItemMasks,
    ) -> Option<native_render::objects::ClearedObjects> {
        // The fixture's guest mask bits survive both planning and a failed reservation.
        for d in &items {
            let item = ITEMS + 0x18 * (u32::from(d.key.record) - 1);
            assert_ne!(mem.u32(item + 0x10).unwrap() & (1 << 3), 0, "guest mask changed before reservation");
        }
        if *self.refuse.lock().unwrap() {
            return None;
        }
        let refused = self.refuse_records.lock().unwrap().clone();
        let out: Vec<_> = items.iter().map(|d| !d.shadow && !refused.contains(&d.key.record)).collect();
        self.drawn.lock().unwrap().push((*camera, objects, items));
        if let Some(item) = self.rebind_item.lock().unwrap().take() {
            mem.set_u32(item + 8, 0xe000_ffff).unwrap();
        }
        masks.apply(mem, &out)
    }
}

/// Object `slot` with definition `def` and `nodes` node matrices (node `n`
/// at x = n), its render-state cache record `slot + 5`; returns its handle.
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
    let handle = salt << 16 | slot;
    let index = slot + 5;
    m.set_u32(CACHE_DATA + CACHE_RECORD_SIZE * index + CACHE_OWNER, handle).unwrap();
    m.set_u32(body + BODY_RENDER_STATE, (0x4000 + index) << 16 | index).unwrap();
    handle
}

/// The lighting pointer a render record of object `handle` holds: its
/// render-state cache record's `+0x44`.
fn lighting(handle: u32) -> u32 {
    CACHE_DATA + CACHE_RECORD_SIZE * ((handle & 0xffff) + 5) + CACHE_LIGHTING
}

/// Layer 3 current; items: object A (shader 0x10), A (0x11), object B
/// (0x10), a BSP item (0x10); all in layers 1 and 3. A usable camera.
fn guest() -> (FlatMemory, u32, u32) {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(OBJECTS, ARRAY).unwrap();
    m.set_u32(ARRAY + 0x38, 16).unwrap();
    m.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    m.set_u32(RENDER_STATE_CACHE, CACHE).unwrap();
    m.set_u32(CACHE + 0x38, 16).unwrap();
    m.set_u32(CACHE + 0x44, CACHE_DATA).unwrap();
    let a = object(&mut m, 1, DEF_A, 2);
    let b = object(&mut m, 2, DEF_B, 1);
    m.set_u32(CURRENT_LAYER, 3).unwrap();
    m.set_u32(CURRENT_LIST, LIST).unwrap();
    m.set_u32(LIST + 4, 4).unwrap();
    m.set_u32(LIST + 0xc, 0).unwrap();
    m.set_u32(LIST + 0x14, ITEMS).unwrap();
    for (i, (shader, record, owner)) in [(0x10u32, 1u32, Some(a)), (0x11, 2, Some(a)), (0x10, 3, Some(b)), (0x10, 4, None)]
        .into_iter()
        .enumerate()
    {
        let at = ITEMS + 0x18 * i as u32;
        m.set_u32(at, GEOMETRY_BUILD).unwrap();
        m.set_u32(at + 8, 0xe000_0000 | shader).unwrap();
        m.set_u32(at + 0xc, record << 8).unwrap();
        m.set_u32(at + 0x10, 0b1010).unwrap();
        // The record's +4 is the model datum (-1: the BSP), never the
        // handle; the object comes from the lighting pointer at +0x10.
        let at = RENDER_RECORDS + 0x20 * record;
        let (model, light) = owner.map_or((u32::MAX, 0), |h| (0xe100_0000 | record, lighting(h)));
        m.set_u32(at + RECORD_MODEL, model).unwrap();
        m.set_u32(at + RECORD_LIGHTING, light).unwrap();
        // A model record of section 0, part 0, implementation 0, its
        // node matrix at MATRIX.
        m.set_u32(at + RECORD_KEY, 0x2000_0000).unwrap();
        m.set_u32(at + RECORD_IMPLEMENTATION, 0).unwrap();
        m.set_u32(at + RECORD_MATRIX, MATRIX).unwrap();
    }
    for k in 0..13u32 {
        m.set_u32(MATRIX + 4 * k, (k as f32).to_bits()).unwrap();
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
    native_objects::restore_pending(&mut m);

    let stub = Arc::new(Stub::default());
    {
        let mut set = stub.set.lock().unwrap();
        set.definitions.insert(DEF_A as u16, 2);
        set.parts.insert((DEF_A as u16, 0, 0), SetPart { shader: 0x10, implementations: 1, texaccum: 0 });
    }
    native_objects::set_host(stub.clone());

    // Another caller of the layer draw (another view kind), and the
    // first-person draw's: both left to D3D.
    native_objects::before_layer(&mut m, 0x0002_afa5);
    assert_eq!(masks(&m), all);
    native_objects::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
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
        // The item, as its record picked it, with its record's rows.
        let items = &drawn[0].2;
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].index, items[0].object, items[0].shadow), (0, 0, false));
        assert_eq!(items[0].key, ItemKey { record: 1, shader: 0xe000_0010, section: 0, part: 0, implementation: 0 });
        assert_eq!(items[0].rows[0], [1.0, 4.0, 7.0, 10.0]);
    }
    native_objects::restore_pending(&mut m);
    assert_eq!(masks(&m), all);

    // B's model loaded with another node count than the object has: B
    // stays on the D3D path.
    {
        let mut set = stub.set.lock().unwrap();
        set.definitions.insert(DEF_B as u16, 5);
        set.parts.insert((DEF_B as u16, 0, 0), SetPart { shader: 0x10, implementations: 1, texaccum: 0 });
    }
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), vec![0b0010, 0b1010, 0b1010, 0b1010]);
    assert_eq!(stub.drawn.lock().unwrap()[1].1.len(), 1);
    native_objects::restore_pending(&mut m);
    assert_eq!(masks(&m), all);

    // The device cannot draw: every bit back at once.
    *stub.refuse.lock().unwrap() = true;
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), all);
    native_objects::restore_pending(&mut m);
    assert_eq!(masks(&m), all);

    // Where each layer-3 call ended: no host, the wrong caller, first
    // person, drawn twice, refused once; B wanted once.
    assert_eq!(native_objects::stop_count(Stop::NoHost), 1);
    assert_eq!(native_objects::stop_count(Stop::WrongCaller), 1);
    assert_eq!(native_objects::stop_count(Stop::FirstPerson), 1);
    assert_eq!(native_objects::stop_count(Stop::Drawn), 2);
    assert_eq!(native_objects::stop_count(Stop::DrawRefused), 1);
    assert_eq!(native_objects::diagnostics(), (6, 1));
    // No model loaded for anything yet: live object items, none accepted;
    // every item of an unknown definition asks (A twice, B once; the host
    // dedupes).
    let saved = std::mem::take(&mut *stub.set.lock().unwrap());
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), all);
    assert_eq!(native_objects::stop_count(Stop::NoneAccepted), 1);
    assert_eq!(native_objects::diagnostics(), (7, 4));
    *stub.set.lock().unwrap() = saved;
    // No live object behind any item: counted apart, so a live run can
    // tell "no models yet" from "no objects found".
    m.set_u32(OBJECTS, 0).unwrap();
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(native_objects::stop_count(Stop::NoObjectItems), 1);
    m.set_u32(OBJECTS, ARRAY).unwrap();
    // Nor when the records' lighting pointers lead nowhere (no cache).
    m.set_u32(RENDER_STATE_CACHE, 0).unwrap();
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(native_objects::stop_count(Stop::NoObjectItems), 2);
    assert_eq!(masks(&m), all);
    m.set_u32(RENDER_STATE_CACHE, CACHE).unwrap();
    let summary = native_objects::summary();
    assert!(summary.contains("drawn 2") && summary.contains("first person 1"), "{summary}");
    assert!(summary.contains("object records"), "{summary}");

    // Another layer: nothing.
    *stub.refuse.lock().unwrap() = false;
    m.set_u32(CURRENT_LAYER, 1).unwrap();
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    native_objects::restore_pending(&mut m);
    assert_eq!(masks(&m), all);
    assert_eq!(stub.drawn.lock().unwrap().len(), 2);

    // The next layer draw, whoever calls it, sets the bits back first.
    m.set_u32(CURRENT_LAYER, 3).unwrap();
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), vec![0b0010, 0b1010, 0b1010, 0b1010]);
    m.set_u32(CURRENT_LAYER, 5).unwrap();
    native_objects::before_layer(&mut m, 0x0002_afa5);
    assert_eq!(masks(&m), all);

    // A list built again before it (another mask on the cleared item):
    // left alone.
    m.set_u32(CURRENT_LAYER, 3).unwrap();
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    m.set_u32(ITEMS + 0x10, 0b0100).unwrap();
    native_objects::restore_pending(&mut m);
    assert_eq!(masks(&m)[0], 0b0100);

    // Both objects resolve. The host accepts only B's record. The two
    // accepted candidates are separated by a guest-owned part of A.
    m.set_u32(ITEMS + 0x10, 0b1010).unwrap();
    stub.set.lock().unwrap().definitions.insert(DEF_B as u16, 1);
    *stub.refuse_records.lock().unwrap() = vec![1];
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), vec![0b1010, 0b1010, 0b0010, 0b1010]);
    native_objects::restore_pending(&mut m);
    assert_eq!(masks(&m), all);

    // Rebinding B's item after planning invalidates the entire mask commit.
    stub.refuse_records.lock().unwrap().clear();
    *stub.rebind_item.lock().unwrap() = Some(ITEMS + 2 * 0x18);
    native_objects::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), all);
    native_objects::restore_pending(&mut m);
    assert_eq!(masks(&m), all);
}

/// Alone at the layer draw, and with the world, the chain runs on the
/// pipelined render thread: context-only observers before, nothing after.
#[test]
fn its_layer_draw_chain_runs_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("impl.render.native_world=on,impl.render.native_objects=on").unwrap());
    assert!(r.add(&feature::native_world::NativeWorld).unwrap());
    assert!(r.add(&NativeObjects).unwrap());
    let plan = r.build(&Base).unwrap();
    let chain = plan
        .chains
        .values()
        .find(|c| c.point.name == "LAYER_DRAW")
        .expect("placed at the layer draw");
    assert_eq!(chain.observe_before.len(), 2, "the world's and the objects'");
    assert!(!chain.needs_shadow());
    assert!(chain.render_thread().is_ok());
}

/// Guest words for [`native_objects::item_refusal`].
struct Words(std::collections::HashMap<u32, u32>);

impl native_render::camera::GuestRead for Words {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        Some(self.0.get(&addr).copied().unwrap_or(0))
    }
}

fn key(section: u16, part: u8, shader: u32, implementation: usize) -> ItemKey {
    ItemKey { record: 7, shader, section, part, implementation }
}

/// An item whose part's implementation samples the texture accumulation
/// target is drawn only while the target is bound (`0x0001dcc0`'s
/// check): decided at the hook, so with none bound it keeps its D3D draw
/// rather than go undrawn. Its object and rows must read too.
#[test]
fn an_item_sampling_the_texture_accumulation_target_waits_for_it() {
    use native_render::object_nodes::TEXACCUM_TARGET;
    let mut set = NativeSet::default();
    set.definitions.insert(DEF_A as u16, 1);
    set.parts.insert((DEF_A as u16, 2, 1), SetPart { shader: 0x10, implementations: 0b11, texaccum: 0b10 });
    let rows = Ok(vec![[0.0; 4]; 3]);
    let mut mem = Words(Default::default());
    let refusal = |mem: &Words, implementation| native_objects::item_refusal(mem, &set, DEF_A, &key(2, 1, 0x10, implementation), true, &rows);
    assert_eq!(refusal(&mem, 0), None, "implementation 0 does not sample it");
    assert!(refusal(&mem, 1).unwrap().contains("texture accumulation"), "no target");
    mem.0.insert(TEXACCUM_TARGET + 0x88, 0x0123_4560);
    assert_eq!(refusal(&mem, 1), None, "bound");
    mem.0.insert(TEXACCUM_TARGET + 0x94, 1 << 8);
    assert!(refusal(&mem, 1).is_some(), "the byte at +0x95 set");
    let no_rows = Err("record 7: no rows".to_string());
    assert_eq!(native_objects::item_refusal(&mem, &set, DEF_A, &key(2, 1, 0x10, 0), true, &no_rows).as_deref(), Some("record 7: no rows"));
    assert!(native_objects::item_refusal(&mem, &set, DEF_A, &key(2, 1, 0x10, 0), false, &rows).is_some(), "its object did not read");
}

/// An item is offered by the part its record names: the definition's
/// part at that section and index, with the item's shader and the
/// record's implementation. Another part of the same section, another
/// shader or an implementation the part lacks keeps its D3D draw.
#[test]
fn an_item_is_accepted_only_for_the_part_and_implementation_its_record_picked() {
    let mut set = NativeSet::default();
    set.definitions.insert(DEF_A as u16, 2);
    set.definitions.insert(DEF_B as u16, 2);
    set.parts.insert((DEF_A as u16, 4, 0), SetPart { shader: 0x10, implementations: 0b101, texaccum: 0 });
    assert!(set.accepts(DEF_A, &key(4, 0, 0xe000_0010, 0)));
    assert!(set.accepts(DEF_A, &key(4, 0, 0xe000_0010, 2)));
    assert!(!set.accepts(DEF_A, &key(4, 0, 0xe000_0010, 1)), "an implementation the part lacks");
    assert!(!set.accepts(DEF_A, &key(4, 1, 0xe000_0010, 0)), "another part of the section");
    assert!(!set.accepts(DEF_A, &key(5, 0, 0xe000_0010, 0)), "another section");
    assert!(!set.accepts(DEF_A, &key(4, 0, 0xe000_0011, 0)), "another shader");
    assert!(!set.accepts(DEF_B, &key(4, 0, 0xe000_0010, 0)), "B's model refused the material");
}
