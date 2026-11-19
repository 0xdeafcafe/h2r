//! The first-person layer observer on synthetic guest memory.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_first_person::{self, Host, NativeFirstPerson};
use feature::native_objects::NativeSet;
use feature::plan::Base;
use feature::{Registry, Switches};
use native_render::objects::{
    BODY_RENDER_STATE, CACHE_LIGHTING, CACHE_OWNER, CACHE_RECORD_SIZE, FIRST_PERSON_LAYER3_RETURN,
    OBJECTS, ObjectFrame, RECORD_LIGHTING, RECORD_MODEL, RENDER_STATE_CACHE, SCENE_LAYER3_RETURN,
};
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, RENDER_RECORDS};

#[test]
fn the_plan_places_a_render_thread_observer_only_when_on() {
    let mut off = Registry::new(Switches::parse("").unwrap());
    off.add(&NativeFirstPerson).unwrap();
    assert!(
        !off.build(&Base)
            .unwrap()
            .text()
            .contains("native_first_person")
    );

    let mut on = Registry::new(Switches::parse("impl.render.native_first_person=on").unwrap());
    on.add(&NativeFirstPerson).unwrap();
    let plan = on.build(&Base).unwrap();
    assert!(plan.text().contains("native_first_person_skip"));
    let chain = plan
        .chains
        .values()
        .find(|c| c.point.name == "LAYER_DRAW")
        .unwrap();
    assert!(!chain.needs_shadow());
    assert!(chain.render_thread().is_ok());
}

const LIST: u32 = 0x004c_1a48;
const ARRAY: u32 = 0x0058_0000;
const HEADERS: u32 = 0x0058_1000;
const BODIES: u32 = 0x0059_0000;
const CACHE: u32 = 0x0057_0000;
const CACHE_DATA: u32 = 0x0057_1000;
const ITEMS: u32 = 0x004c_8740;
const DEF_A: u32 = 0xe000_0100;
const DEF_B: u32 = 0xe000_0200;

#[derive(Default)]
struct Stub {
    set: Mutex<NativeSet>,
    wanted: Mutex<Vec<u32>>,
    drawn: Mutex<Vec<Vec<ObjectFrame>>>,
    refuse: Mutex<bool>,
}

impl Host for Stub {
    fn native(&self, _mem: &dyn GuestMemory) -> Option<Arc<NativeSet>> {
        Some(Arc::new(self.set.lock().unwrap().clone()))
    }

    fn want(&self, definition: u32) {
        self.wanted.lock().unwrap().push(definition);
    }

    fn draw(&self, objects: Vec<ObjectFrame>) -> bool {
        if *self.refuse.lock().unwrap() {
            return false;
        }
        self.drawn.lock().unwrap().push(objects);
        true
    }
}

fn object(m: &mut FlatMemory, slot: u32, definition: u32, nodes: u32) -> u32 {
    let salt = 0x8000 + slot;
    let header = HEADERS + 0xc * slot;
    let body = BODIES + 0x1000 * slot;
    m.set_u32(header, salt).unwrap();
    m.set_u32(header + 8, body).unwrap();
    m.set_u32(body, definition).unwrap();
    m.set_u32(body + 0x3c, 1f32.to_bits()).unwrap();
    m.set_u32(body + 0x114, (0x34 * nodes) | (0x200 << 16))
        .unwrap();
    for n in 0..nodes {
        let at = body + 0x200 + 0x34 * n;
        let words = [
            1.0f32, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, n as f32, 0.0, 0.0,
        ];
        for (k, word) in words.into_iter().enumerate() {
            m.set_u32(at + 4 * k as u32, word.to_bits()).unwrap();
        }
    }
    let handle = salt << 16 | slot;
    let index = slot + 5;
    m.set_u32(CACHE_DATA + CACHE_RECORD_SIZE * index + CACHE_OWNER, handle)
        .unwrap();
    m.set_u32(body + BODY_RENDER_STATE, (0x4000 + index) << 16 | index)
        .unwrap();
    handle
}

fn lighting(handle: u32) -> u32 {
    CACHE_DATA + CACHE_RECORD_SIZE * ((handle & 0xffff) + 5) + CACHE_LIGHTING
}

/// A and B in layer 3, with A using two materials and one BSP item.
fn guest() -> (FlatMemory, u32, u32) {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(OBJECTS, ARRAY).unwrap();
    m.set_u32(ARRAY + 0x38, 16).unwrap();
    m.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    m.set_u32(RENDER_STATE_CACHE, CACHE).unwrap();
    m.set_u32(CACHE + 0x38, 16).unwrap();
    m.set_u32(CACHE + 0x44, CACHE_DATA).unwrap();
    // The body definitions deliberately do not name the record modes:
    // first-person geometry follows RECORD_MODEL, not the third-person body.
    let a = object(&mut m, 1, 0xdead_0001, 2);
    let b = object(&mut m, 2, 0xdead_0002, 1);
    m.set_u32(CURRENT_LAYER, 3).unwrap();
    m.set_u32(CURRENT_LIST, LIST).unwrap();
    m.set_u32(LIST + 4, 4).unwrap();
    m.set_u32(LIST + 0xc, 0).unwrap();
    m.set_u32(LIST + 0x14, ITEMS).unwrap();
    m.set_u32(0x0050_943c, 16).unwrap();
    for (i, (shader, record, owner)) in [
        (0x10u32, 1u32, Some(a)),
        (0x11, 2, Some(a)),
        (0x10, 3, Some(b)),
        (0x10, 4, None),
    ]
    .into_iter()
    .enumerate()
    {
        let at = ITEMS + 0x18 * i as u32;
        m.set_u32(at, GEOMETRY_BUILD).unwrap();
        m.set_u32(at + 8, 0xe000_0000 | shader).unwrap();
        m.set_u32(at + 0xc, record << 8).unwrap();
        m.set_u32(at + 0x10, 0b1010).unwrap();
        let at = RENDER_RECORDS + 0x20 * record;
        let (model, light) = owner.map_or((u32::MAX, 0), |h| {
            (if h == a { DEF_A } else { DEF_B }, lighting(h))
        });
        m.set_u32(at + RECORD_MODEL, model).unwrap();
        m.set_u32(at + RECORD_LIGHTING, light).unwrap();
    }
    (m, a, b)
}

fn masks(m: &FlatMemory) -> Vec<u32> {
    (0..4)
        .map(|i| m.u32(ITEMS + 0x18 * i + 0x10).unwrap())
        .collect()
}

/// One behaviour test because the installed host and pending restore are
/// process globals, like the native-objects suite's.
#[test]
fn first_person_skips_whole_objects_and_restores_them_between_draws() {
    let all = vec![0b1010; 4];
    let (mut m, a, b) = guest();
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(masks(&m), all, "no host");

    let stub = Arc::new(Stub::default());
    {
        let mut set = stub.set.lock().unwrap();
        set.definitions.insert(DEF_A as u16, 2);
        set.definitions.insert(DEF_B as u16, 1);
        set.shaders.insert((DEF_A as u16, 0x10));
        set.shaders.insert((DEF_B as u16, 0x10));
        set.shadowed.insert(DEF_B as u16);
    }
    native_first_person::set_host(stub.clone());

    native_first_person::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), all, "the base layer-3 caller is not owned");
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        all,
        "A's unaccepted material vetoes all of A; compared B stays on D3D"
    );
    assert_eq!(
        stub.drawn.lock().unwrap().len(),
        1,
        "the compared object still reaches the host"
    );
    assert_eq!(stub.drawn.lock().unwrap()[0][0].handle, b);

    stub.set.lock().unwrap().shadowed.clear();
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        vec![0b1010, 0b1010, 0b0010, 0b1010],
        "only B is native"
    );
    assert_eq!(stub.drawn.lock().unwrap()[1][0].handle, b);
    native_first_person::restore_pending(&mut m);
    assert_eq!(
        masks(&m),
        all,
        "restored before the light step's next layer"
    );

    *stub.refuse.lock().unwrap() = true;
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        all,
        "a refused queue request restores immediately"
    );
    *stub.refuse.lock().unwrap() = false;

    stub.set
        .lock()
        .unwrap()
        .shaders
        .insert((DEF_A as u16, 0x11));
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        vec![0b0010, 0b0010, 0b0010, 0b1010],
        "both complete objects, never the BSP"
    );
    let drawn = stub.drawn.lock().unwrap();
    let last = drawn.last().unwrap();
    assert_eq!(
        last.iter().map(|o| o.handle).collect::<Vec<_>>(),
        vec![a, b]
    );
    assert_eq!(
        last[0].nodes.len(),
        2,
        "the first-person object's own nodes"
    );
    assert_eq!(
        last.iter()
            .map(|object| object.definition)
            .collect::<Vec<_>>(),
        vec![DEF_A, DEF_B],
        "the record modes, not the body definitions"
    );
    drop(drawn);
    m.set_u32(ITEMS + 0x10, 0b0100).unwrap();
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(masks(&m)[0], 0b0100, "a rebuilt list is not overwritten");

    let (calls, drawn, _, _, refused) = native_first_person::counts();
    assert_eq!(calls, 6);
    assert_eq!(drawn, 4, "comparison, B, A and B, then the rebuilt list");
    assert_eq!(refused, 1);

    native_first_person::restore_pending(&mut m);
    m.set_u32(ITEMS + 3 * 0x18, 0x0004_b5d0).unwrap();
    let draws = stub.drawn.lock().unwrap().len();
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        vec![0b0100, 0b1010, 0b1010, 0b1010],
        "an item whose owner cannot be established leaves the whole call on D3D"
    );
    assert_eq!(stub.drawn.lock().unwrap().len(), draws);

    m.set_u32(ITEMS + 3 * 0x18, GEOMETRY_BUILD).unwrap();
    m.set_u32(ITEMS + 0x10, 0b1010).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * 2 + RECORD_MODEL, DEF_B)
        .unwrap();
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        vec![0b1010, 0b1010, 0b0010, 0b1010],
        "one owner with two record modes is vetoed; the complete other object still draws"
    );
    assert_eq!(
        stub.drawn.lock().unwrap().last().unwrap()[0].definition,
        DEF_B
    );
}
