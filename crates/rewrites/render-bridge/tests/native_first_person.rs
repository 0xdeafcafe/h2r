//! The first person is a cut (`0x0003fdf0`, both calls) whose parts draw
//! from the scene in slot 10, on synthetic scenes and a stub host.
#![cfg(feature = "native-render")]

use std::sync::{Arc, Mutex};

use guest_layout::memory::{FlatMemory, GuestMemory};
use frame_scene::{
    FrameScene, InstanceInput, InstanceKey, ItemKey, RenderCamera, SceneBuilder, SceneId, Stamp, ViewKey, ViewKind,
    ViewSlot,
};
use hooks::plan::Base;
use hooks::{Registry, Switches};
use render_bridge::cut::{self, CUTS};
use render_bridge::native_first_person::{self, Dropped, Host, NativeFirstPerson, Outcome};
use render_bridge::native_objects::{NativeSet, SetPart};
use render_bridge::producers::{self, REGISTRY, Status};

const MODEL: u32 = 0xe200_0001;
const SHADER: u32 = 0x10;

#[test]
fn the_plan_places_the_cut_when_on() {
    let mut on = Registry::new(Switches::defaults());
    on.add(&NativeFirstPerson).unwrap();
    let plan = on.build(&Base).unwrap();
    // The entry is the feature's, and the body never runs.
    assert!(plan.text().contains("native_first_person_skip"));
    let chain = plan
        .chains
        .values()
        .find(|c| c.point.base == 0x0003_fdf0)
        .expect("the cut's point is in the plan");
    assert!(chain.render_thread().is_ok());
}

#[test]
fn the_cut_row_is_in_the_table_and_the_census() {
    cut::check(CUTS).unwrap();
    let row = CUTS.iter().find(|c| c.name == "first_person").expect("a cut row");
    assert_eq!((row.entry, row.pop, row.abi_result), (0x0003_fdf0, 4, 1));
    assert!(cut::OWN_PRODUCER.contains(&"first_person"));
    let census = &REGISTRY[producers::row("first_person").unwrap()];
    // Cut from the first frame: no held check runs the body.
    assert_eq!((census.entry, census.status), (0x0003_fdf0, Status::Native));
}

#[test]
fn the_counters_name_no_held_check() {
    let counters = native_first_person::counters();
    assert!(counters.iter().any(|(k, _)| k == "first_person_dropped_effects"));
    assert!(counters.iter().all(|(k, _)| !k.contains("held") && !k.contains("check")));
}

fn camera() -> RenderCamera {
    RenderCamera {
        position: [1.0, 2.0, 3.0],
        forward: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        fov: 70f32.to_radians(),
        projection_xy: None,
        viewport: [0, 0, 480, 640],
        near: 0.0625,
        far: 1024.0,
    }
}

/// A first-person view with one object whose parts draw in `layers`, part
/// `n` in `layers[n]`.
fn scene(layers: &[u8]) -> Arc<FrameScene> {
    let mut b = SceneBuilder::default();
    b.begin(SceneId::default());
    let key = ViewKey { kind: ViewKind::FirstPerson, player: 0, view: 0 };
    b.open_view(key, camera()).unwrap();
    let instance = b
        .push_instance(InstanceInput {
            key: InstanceKey(1),
            definition: 0xe100_0001,
            model: MODEL,
            nodes: &[],
            sphere: ([0.0; 3], 1.0),
            lighting: None,
            defaults: None,
        })
        .unwrap();
    for (n, &layer) in layers.iter().enumerate() {
        let item = ItemKey {
            record: 0,
            shader: SHADER,
            section: 0,
            part: n as u8,
            implementation: 0,
        };
        b.push_part(instance, n as u32, item, layer, &[]).unwrap();
    }
    b.seal(Stamp::default()).unwrap()
}

fn set(parts: &[u8]) -> NativeSet {
    let mut set = NativeSet::default();
    set.definitions.insert(MODEL as u16, 1);
    for &part in parts {
        set.parts.insert(
            (MODEL as u16, 0, part),
            SetPart { shader: SHADER as u16, implementations: 1, texaccum: 0 },
        );
    }
    set
}

#[derive(Default)]
struct Stub {
    set: Mutex<Option<NativeSet>>,
    wanted: Mutex<Vec<u32>>,
    /// Each draw: its slot, call and parts.
    drew: Mutex<Vec<(ViewSlot, u32, Vec<u32>)>>,
}

impl Host for Stub {
    fn native(&self, _mem: &mut dyn GuestMemory, _prepare_draw: bool) -> Option<Arc<NativeSet>> {
        self.set.lock().unwrap().clone().map(Arc::new)
    }

    fn want(&self, model: u32) {
        self.wanted.lock().unwrap().push(model);
    }

    fn draw(
        &self,
        _mem: &mut dyn GuestMemory,
        scene: &FrameScene,
        _view: &frame_scene::View,
        slot: ViewSlot,
        mode: u32,
        parts: &[u32],
    ) -> Outcome {
        assert!(parts.iter().all(|&p| (p as usize) < scene.parts.len()));
        self.drew.lock().unwrap().push((slot, mode, parts.to_vec()));
        Outcome { drawn: parts.len(), dropped: Vec::new() }
    }
}

fn run(stub: &Stub, scene: &FrameScene, mode: u32) -> Outcome {
    let mut mem = FlatMemory::new(0x1000, 4);
    native_first_person::offer(stub, &mut mem, scene, &scene.views[0], mode)
}

#[test]
fn first_person_draws_after_the_world_transparents() {
    // Slot 10, after the world's transparents (slot 9) and before the
    // flare query, bloom and post.
    assert_eq!(native_first_person::SLOT, ViewSlot::FirstPerson);
    assert_eq!(native_first_person::SLOT.number(), Some(10));
    assert!(native_first_person::SLOT > ViewSlot::Transparents);
    assert!(native_first_person::SLOT < ViewSlot::FlareQuery);

    let stub = Stub::default();
    *stub.set.lock().unwrap() = Some(set(&[0, 1]));
    let s = scene(&[1, 3]);
    let out = run(&stub, &s, 0);
    assert_eq!(out, Outcome { drawn: 2, dropped: vec![] });
    let drew = stub.drew.lock().unwrap();
    assert_eq!(drew.len(), 1);
    assert_eq!(drew[0].0, native_first_person::SLOT, "no pass of its own, and none before slot 10");
    assert!(drew[0].0 > ViewSlot::Transparents);
}

#[test]
fn each_call_draws_its_layers() {
    let stub = Stub::default();
    *stub.set.lock().unwrap() = Some(set(&[0, 1, 2, 3]));
    // The guest's split (`0x0003fdf0`): argument 0 draws layers 1, 3, 8,
    // 20, 7 and 2; argument 1 draws layer 16 only.
    let s = scene(&[1, native_first_person::LATE_LAYER, 7, 2]);
    let first = run(&stub, &s, 0);
    let second = run(&stub, &s, 1);
    assert_eq!((first.drawn, second.drawn), (3, 1), "both calls, argument 0 and 1");
    let drew = stub.drew.lock().unwrap();
    assert_eq!(drew[0], (ViewSlot::FirstPerson, 0, vec![s.order[0], s.order[2], s.order[3]]));
    assert_eq!(drew[1], (ViewSlot::FirstPerson, 1, vec![s.order[1]]));
}

#[test]
fn an_unreadable_part_is_a_named_drop() {
    let stub = Stub::default();
    *stub.set.lock().unwrap() = Some(set(&[0, 1]));
    let mut s = (*scene(&[1, 1])).clone();
    s.parts[0].instance = 99;
    let out = run(&stub, &s, 0);
    assert_eq!(out.drawn, 1, "the readable part still draws");
    assert_eq!(out.dropped, vec![(Dropped::Unreadable, 1)]);
    assert_eq!(Dropped::Unreadable.name(), "unreadable");

    // A part range outside the scene is dropped whole, by name.
    let mut s = (*scene(&[1, 1])).clone();
    s.order.clear();
    let out = run(&stub, &s, 0);
    assert_eq!((out.drawn, out.dropped_total()), (0, 2));
    assert_eq!(out.dropped, vec![(Dropped::Unreadable, 2)]);
    assert_eq!(stub.drew.lock().unwrap().len(), 1, "nothing was offered for it");
}

#[test]
fn a_part_the_host_cannot_draw_is_dropped_by_cause_and_never_left_to_the_guest() {
    let stub = Stub::default();
    // The model is not resident: asked for, dropped.
    *stub.set.lock().unwrap() = Some(NativeSet::default());
    let s = scene(&[1]);
    let out = run(&stub, &s, 0);
    assert_eq!(out.dropped, vec![(Dropped::Unresident, 1)]);
    assert_eq!(*stub.wanted.lock().unwrap(), vec![MODEL]);
    // The model is resident but not part 0.
    *stub.set.lock().unwrap() = Some(set(&[5]));
    let out = run(&stub, &s, 0);
    assert_eq!(out.dropped, vec![(Dropped::Unsupported, 1)]);
    // No set at all.
    *stub.set.lock().unwrap() = None;
    assert_eq!(run(&stub, &s, 0).dropped, vec![(Dropped::Unresident, 1)]);
    assert!(stub.drew.lock().unwrap().is_empty(), "no draw was made for any");
}

/// The first person has no camera of its own: `0x0003fdf0` and its call
/// site (`0x0002c1d0`..`0x0002c2c2`) set none, so its arms and weapon
/// draw with the render camera record (`0x004b9da0`) and its projection,
/// the game view's. Answering the call opens the first person's view with
/// that camera, and the call's epilogue gives list 0 its layers back.
#[test]
fn the_first_persons_view_takes_the_render_camera_the_game_view_drew_with() {
    use engine::render::light_gather::first_person_items::{LIST, RECORD_COUNT, SAVED_LAYERS};
    use native_render::camera::RENDER_CAMERA;

    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    let c = RENDER_CAMERA;
    for (i, v) in [0.0f32, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0].into_iter().enumerate() {
        m.set_u32(c + 4 * i as u32, v.to_bits()).unwrap();
    }
    m.set_u32(c + 0x28, 1.2f32.to_bits()).unwrap();
    m.set_u32(c + 0x34, 480 | (640 << 16)).unwrap();
    m.set_u32(c + 0x40, 0.0625f32.to_bits()).unwrap();
    m.set_u32(c + 0x44, 1024f32.to_bits()).unwrap();
    // List 0 with the world's items; no first-person record.
    m.set_u32(LIST, 16).unwrap();
    m.set_u32(LIST + 4, 3).unwrap();
    m.set_u32(LIST + 8, 0b1010).unwrap();
    m.set_u32(LIST + 0x14, 0x0050_0000).unwrap();
    m.set_u32(RECORD_COUNT, 0).unwrap();

    let game = render_bridge::scene_step::open_view(&m).expect("the game view").camera;
    native_first_person::answer(&mut m, 0, 0);
    render_bridge::scene_step::with_open(|open| {
        let (_, view) = open.expect("the first person's view");
        assert_eq!(view.key.kind, ViewKind::FirstPerson);
        assert_eq!(view.camera, game, "the game view's camera and projection");
        assert_eq!(view.camera.fov, 1.2);
        assert_eq!(view.camera.position, [0.0, 0.0, 1.0]);
    });
    assert_eq!(m.u32(SAVED_LAYERS).unwrap(), 0b1010, "the prologue saved list 0's layers");
    assert_eq!(m.u32(LIST + 8).unwrap(), 0b1010, "the epilogue set them back");
    assert_eq!(m.u32(LIST + 0xc).unwrap(), 0);
}
