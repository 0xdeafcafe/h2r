//! The first-person layer observer on synthetic guest memory.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use render_bridge::native_first_person::{self, Host, NativeFirstPerson};
use render_bridge::native_objects::{NativeSet, SetPart};
use hooks::plan::Base;
use hooks::{Registry, Switches};
use native_render::object_nodes::{RECORD_IMPLEMENTATION, RECORD_KEY, RECORD_MATRIX};
use native_render::objects::{
    BODY_RENDER_STATE, CACHE_LIGHTING, CACHE_OWNER, CACHE_RECORD_SIZE, FIRST_PERSON_LAYER3_RETURN,
    ItemDraw, ItemKey, OBJECTS, ObjectFrame, RECORD_LIGHTING, RECORD_MODEL, RENDER_STATE_CACHE,
    SCENE_LAYER3_RETURN,
};
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, RENDER_RECORDS};
use native_render::object_walk::RECORD_STRIP;
use frame_scene::PartFlags;

#[test]
fn the_plan_places_a_render_thread_observer_only_when_on() {
    let mut off = Registry::new(Switches::parse("impl.render.native=off").unwrap());
    off.add(&NativeFirstPerson).unwrap();
    assert!(
        !off.build(&Base)
            .unwrap()
            .text()
            .contains("native_first_person")
    );

    let mut on = Registry::new(Switches::defaults());
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
const MATRIX: u32 = 0x0060_1000;
const TAGS: u32 = 0x005a_0000;
const SHADER: u32 = 0x005b_0000;
/// Per definition, its model's record, its one section and its geometry
/// with parts 0 and 1, both triangle lists ([`model`]).
const MODEL: u32 = 0x005c_0000;

#[derive(Default)]
struct Stub {
    set: Mutex<NativeSet>,
    wanted: Mutex<Vec<u32>>,
    offered: Mutex<Vec<(Vec<ObjectFrame>, Vec<ItemDraw>, HashMap<u16, u32>)>>,
    refuse: Mutex<bool>,
}

impl Host for Stub {
    fn native(&self, _mem: &mut dyn GuestMemory, _prepare_draw: bool) -> Option<Arc<NativeSet>> {
        Some(Arc::new(self.set.lock().unwrap().clone()))
    }

    fn want(&self, model: u32) {
        self.wanted.lock().unwrap().push(model);
    }

    fn draw(
        &self,
        mem: &mut dyn GuestMemory,
        _generation: u64,
        objects: Vec<ObjectFrame>,
        items: Vec<ItemDraw>,
        models: HashMap<u16, u32>,
        masks: native_first_person::ItemMasks,
    ) -> Option<native_render::objects::ClearedObjects> {
        self.offered
            .lock()
            .unwrap()
            .push((objects, items.clone(), models));
        if *self.refuse.lock().unwrap() {
            return None;
        }
        let drawn = vec![true; items.len()];
        masks.apply(mem, &drawn)
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

/// The tag table entries the geometry draw's record read goes through
/// (`object_walk::layer_part`): shaders 0x10 and 0x11, and models A and
/// B, each with one section of two triangle-list parts. Returns the
/// address of `definition`'s part `part`.
fn model(m: &mut FlatMemory, definition: u32) -> u32 {
    use native_render::object_nodes::{MODEL_SECTIONS, TAG_INSTANCES};
    m.set_u32(TAG_INSTANCES, TAGS).unwrap();
    for shader in [0x10u32, 0x11] {
        m.set_u32(TAGS + 16 * shader, u32::from_be_bytes(*b"shad")).unwrap();
        m.set_u32(TAGS + 16 * shader + 8, SHADER).unwrap();
    }
    let at = MODEL + 0x1000 * ((definition >> 8) & 0xf);
    let (section, geometry, parts) = (at + 0x100, at + 0x200, at + 0x300);
    m.set_u32(TAGS + 16 * (definition & 0xffff) + 8, at).unwrap();
    m.set_u32(at + MODEL_SECTIONS - 4, 1).unwrap();
    m.set_u32(at + MODEL_SECTIONS, section).unwrap();
    m.set_u32(section + 0x34, geometry).unwrap();
    m.set_u32(geometry, 2).unwrap();
    m.set_u32(geometry + 4, parts).unwrap();
    for part in 0..2 {
        m.set_u32(parts + 0x48 * part, 8 << 16).unwrap();
    }
    parts
}

fn lighting(handle: u32) -> u32 {
    CACHE_DATA + CACHE_RECORD_SIZE * ((handle & 0xffff) + 5) + CACHE_LIGHTING
}

fn native_part(definition: u32, part: u8, shader: u16) -> ((u16, u16, u8), SetPart) {
    (
        (definition as u16, 0, part),
        SetPart {
            shader,
            implementations: 1,
            texaccum: 0,
        },
    )
}

/// A and B in layer 3, with A using two materials and one BSP item. Body
/// definitions deliberately differ from the record modes.
fn guest() -> (FlatMemory, u32, u32) {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(OBJECTS, ARRAY).unwrap();
    m.set_u32(ARRAY + 0x38, 16).unwrap();
    m.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    m.set_u32(RENDER_STATE_CACHE, CACHE).unwrap();
    m.set_u32(CACHE + 0x38, 16).unwrap();
    m.set_u32(CACHE + 0x44, CACHE_DATA).unwrap();
    let a = object(&mut m, 1, 0xdead_0001, 2);
    let b = object(&mut m, 2, 0xdead_0002, 1);
    model(&mut m, DEF_A);
    model(&mut m, DEF_B);
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
        m.set_u32(at + 0xc, record << 8 | u32::from(record == 2)).unwrap();
        m.set_u32(at + 0x10, 0b1010).unwrap();
        let at = RENDER_RECORDS + 0x20 * record;
        let (model, light) = owner.map_or((u32::MAX, 0), |handle| {
            (if handle == a { DEF_A } else { DEF_B }, lighting(handle))
        });
        m.set_u32(at + RECORD_MODEL, model).unwrap();
        m.set_u32(at + RECORD_LIGHTING, light).unwrap();
        m.set_u32(
            at + RECORD_KEY,
            0x2000_0000 | if record == 2 { 1 } else { 0 },
        )
        .unwrap();
        m.set_u32(at + RECORD_IMPLEMENTATION, 0).unwrap();
        m.set_u32(at + RECORD_MATRIX, MATRIX).unwrap();
        m.set_u32(at + 0x18, 0xff << 16).unwrap(); // fade byte +0x1a
    }
    for word in 0..13u32 {
        m.set_u32(MATRIX + 4 * word, (word as f32).to_bits())
            .unwrap();
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
fn first_person_plans_items_and_restores_exactly_the_unplanned_ones() {
    let all = vec![0b1010; 4];
    let (mut m, a, b) = guest();
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(masks(&m), all, "no host");

    let stub = Arc::new(Stub::default());
    native_first_person::set_host(stub.clone());
    for layer in [1, 2, 7, 8, 20] {
        // B appears only in this layer. Two layer-3 items still require
        // the guest sort. A non-geometry neighbour cannot block discovery.
        m.set_u32(ITEMS + 2 * 0x18 + 0x10, 1 << layer).unwrap();
        m.set_u32(ITEMS + 3 * 0x18, 0).unwrap();
        let before = masks(&m);
        native_first_person::observe_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
        assert_eq!(*stub.wanted.lock().unwrap(), [DEF_A, DEF_B]);
        stub.wanted.lock().unwrap().clear();
        assert!(stub.offered.lock().unwrap().is_empty());
        assert_eq!(masks(&m), before);
    }
    m.set_u32(ITEMS + 2 * 0x18 + 0x10, 0b1010).unwrap();
    m.set_u32(ITEMS + 3 * 0x18, GEOMETRY_BUILD).unwrap();
    {
        let mut set = stub.set.lock().unwrap();
        set.definitions.insert(DEF_A as u16, 2);
        set.definitions.insert(DEF_B as u16, 1);
        set.parts
            .extend([native_part(DEF_A, 0, 0x10), native_part(DEF_B, 0, 0x10)]);
    }
    native_first_person::before_layer(&mut m, SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), all, "the base layer-3 caller is not owned");
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        vec![0b0010, 0b1010, 0b0010, 0b1010],
        "A's supported item and B draw, A's other material stays on D3D"
    );
    {
        let offered = stub.offered.lock().unwrap();
        assert_eq!(offered.len(), 1);
        let (objects, items, models) = &offered[0];
        assert_eq!(
            objects
                .iter()
                .map(|object| object.handle)
                .collect::<Vec<_>>(),
            vec![a, b]
        );
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].key,
            ItemKey {
                record: 1,
                shader: 0xe000_0010,
                section: 0,
                part: 0,
                implementation: 0
            }
        );
        assert_eq!(items[0].rows[0], [1.0, 4.0, 7.0, 10.0]);
        assert_eq!(models.get(&1), Some(&DEF_A));
        assert_eq!(models.get(&3), Some(&DEF_B));
    }
    native_first_person::restore_pending(&mut m);
    assert_eq!(masks(&m), all);

    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        vec![0b0010, 0b1010, 0b0010, 0b1010],
        "A's native part and B draw"
    );
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
        "a host that plans nothing restores every item"
    );
    *stub.refuse.lock().unwrap() = false;

    stub.set
        .lock()
        .unwrap()
        .parts
        .insert(native_part(DEF_A, 1, 0x11).0, native_part(DEF_A, 1, 0x11).1);
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        vec![0b0010, 0b0010, 0b0010, 0b1010],
        "every object item, never the BSP"
    );
    m.set_u32(ITEMS + 0x10, 0b0100).unwrap();
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(masks(&m)[0], 0b0100, "a rebuilt list is not overwritten");

    let (calls, drawn, _, _, refused) = native_first_person::counts();
    assert_eq!(calls, 6);
    assert_eq!(drawn, 4);
    assert_eq!(refused, 1);

    native_first_person::restore_pending(&mut m);
    m.set_u32(RENDER_RECORDS + 0x20 * 3 + RECORD_LIGHTING, 0)
        .unwrap();
    let offers = stub.offered.lock().unwrap().len();
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        vec![0b0100, 0b1010, 0b1010, 0b1010],
        "an item whose owner cannot be established leaves the whole call on D3D"
    );
    assert_eq!(stub.offered.lock().unwrap().len(), offers);

    m.set_u32(RENDER_RECORDS + 0x20 * 3 + RECORD_LIGHTING, lighting(b))
        .unwrap();
    m.set_u32(ITEMS + 3 * 0x18, 0x0004_b5d0).unwrap();
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(
        masks(&m),
        vec![0b0100, 0b1010, 0b1010, 0b1010],
        "a non-geometry item also refuses the call"
    );
    assert_eq!(stub.offered.lock().unwrap().len(), offers);

    // Mixed record modes under one owner are exact per-item draws now.
    m.set_u32(ITEMS + 3 * 0x18, GEOMETRY_BUILD).unwrap();
    m.set_u32(ITEMS + 0x10, 0b1010).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * 2 + RECORD_MODEL, DEF_B)
        .unwrap();
    stub.set
        .lock()
        .unwrap()
        .parts
        .insert(native_part(DEF_B, 1, 0x11).0, native_part(DEF_B, 1, 0x11).1);
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    assert_eq!(masks(&m), vec![0b0010, 0b0010, 0b0010, 0b1010]);
    let offered = stub.offered.lock().unwrap();
    let (_, items, models) = offered.last().unwrap();
    assert_eq!(models.get(&2), Some(&DEF_B));
    assert_eq!(
        items.len(),
        3,
        "the owner's mixed record modes are planned independently"
    );
    assert_eq!((items[0].flags, items[0].subparts.as_ref()), (PartFlags::NONE, None));
    drop(offered);

    // The record's flags as the geometry draw reads them: bit 9 on A's
    // part 0 made a strip part draws a strip; the record's fade byte
    // marks the item faded.
    native_first_person::restore_pending(&mut m);
    let a_parts = model(&mut m, DEF_A);
    m.set_u32(a_parts, 0).unwrap();
    let record_1 = RENDER_RECORDS + 0x20;
    m.set_u32(record_1, RECORD_STRIP).unwrap();
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    {
        let offered = stub.offered.lock().unwrap();
        let (_, items, _) = offered.last().unwrap();
        assert_eq!((items[0].key.record, items[0].flags), (1, PartFlags::STRIP));
    }
    native_first_person::restore_pending(&mut m);
    m.set_u32(record_1 + 0x18, 0x80 << 16).unwrap();
    native_first_person::before_layer(&mut m, FIRST_PERSON_LAYER3_RETURN);
    let offered = stub.offered.lock().unwrap();
    let (_, items, _) = offered.last().unwrap();
    let faded = items.iter().find(|item| item.key.record == 1).expect("a faded record is planned");
    assert!(
        faded.flags.contains(PartFlags::FADE),
        "a fade byte is a part flag; the host decides whether the stipple masks it"
    );
}
