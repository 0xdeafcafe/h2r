//! The `native-objects` feature (docs/graphics/native-objects.md, stage
//! 2): its plan next to every other hook on the layer draw, its snapshot
//! policy, and which items of the native set a host is offered.
#![cfg(feature = "native-render")]

use render_bridge::native_objects::{self, NativeObjects, NativeSet, SetPart};
use hooks::plan::Base;
use hooks::{Registry, Switches};
use native_render::objects::ItemKey;

#[test]
fn the_plan_orders_it_inside_render_timing_and_the_world() {
    let switches = "render-timing=on,render-timing.deep=on";
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    assert!(r.add(&render_bridge::render_timing::RenderTiming::new()).unwrap());
    assert!(r.add(&render_bridge::native_world::NativeWorld).unwrap());
    assert!(r.add(&NativeObjects).unwrap());
    let plan = match r.build(&Base) {
        Ok(p) => p,
        Err(c) => panic!("refused: {c:?}"),
    };
    let text = plan.text();
    assert!(text.contains("native_objects_skip"), "{text}");
    assert!(!text.contains("native_objects_restore"), "nothing after the body: {text}");
}

/// On, its section (no bytes) drops the walk's layer at a load.
#[test]
fn on_its_snapshot_policy_is_a_section() {
    let mut r = Registry::new(Switches::defaults());
    assert!(r.add(&NativeObjects).unwrap());
    let plan = r.build(&Base).unwrap();
    let policies: Vec<_> = plan
        .snapshot
        .iter()
        .map(|f| (f.id, f.policy.name()))
        .collect();
    assert_eq!(policies, vec![(native_objects::ID, "section")]);
    assert!(plan.snapshot_blockers().is_empty());
}

const DEF_A: u32 = 0xe000_0100;
const DEF_B: u32 = 0xe000_0200;

/// Alone at the layer draw, and with the world, the chain runs on a second
/// executor: context-only observers before, nothing after.
#[test]
fn its_layer_draw_chain_runs_on_the_render_thread() {
    let mut r = Registry::new(Switches::defaults());
    assert!(r.add(&render_bridge::native_world::NativeWorld).unwrap());
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

/// Objects draw from the scene in the opaque base's slot. A host that
/// takes nothing says so by a named cause, and consumes nothing: the run
/// keeps its guest handlers' draws.
#[test]
fn objects_draw_in_the_opaque_base_slot_and_a_declined_run_consumes_nothing() {
    use guest_layout::memory::FlatMemory;
    use frame_scene::ViewSlot;
    use native_objects::Host;
    use timeline::native::ObjectWalk;

    assert_eq!(native_objects::SLOT, ViewSlot::OpaqueBase);
    struct Takes;
    impl Host for Takes {}
    let mut mem = FlatMemory::new(0x1000, 16);
    let mut consumed = false;
    let out = Takes.draw_walk(&mut mem, None, native_objects::SLOT, 0, &[], None, &mut |_| {
        consumed = true;
        true
    });
    assert!(matches!(out, Err(ObjectWalk::Reservation)), "the default host declines by name");
    assert!(!consumed, "a declined run is not consumed");
}

/// Only the first slot of the next admitted run goes to the host. Every
/// other handler call draws as the guest's: an object of a span the walk
/// did not admit (an unreadable part, an interleaved span), the later
/// slots of a run the host refused, and calls outside the list. A Grunt
/// whose part the walk cannot read is drawn by its handler, not dropped.
#[test]
fn a_handler_the_walk_does_not_take_draws_as_the_guests() {
    use native_objects::{HandlerAt, handler_at};
    use native_render::object_walk::Run;

    let run = Run { slots: 3..5, objects: 1 };
    assert_eq!(handler_at(6, Some(&run), Some(3)), HandlerAt::Run);
    for slot in [0, 1, 2, 4, 5] {
        assert_eq!(handler_at(6, Some(&run), Some(slot)), HandlerAt::Guest, "slot {slot}");
    }
    assert_eq!(handler_at(6, None, Some(3)), HandlerAt::Guest, "no run left: the refused run's later slots");
    assert_eq!(handler_at(6, Some(&run), None), HandlerAt::Guest, "outside the list");
    assert_eq!(handler_at(3, Some(&run), Some(3)), HandlerAt::Guest, "past the walk's entries");
}

/// Objects the walk does not draw are counted kept, by cause and view,
/// in `render.scene`'s `scene_kept_<site>3_object_<cause>` rows.
#[test]
fn objects_the_walk_keeps_are_counted_by_cause() {
    use frame_scene::ViewKind;
    use native_objects::{keep, kept_site};
    use timeline::native::ObjectWalk;

    let kept = |name: &str| {
        render_bridge::native_layers::counters()
            .into_iter()
            .find(|(k, _)| k == name)
            .map_or(0, |(_, v)| v)
    };
    let (part, order, alt) = (
        kept("scene_kept_3_object_unsupported"),
        kept("scene_kept_3_object_order"),
        kept("scene_kept_alt3_object_reservation"),
    );
    keep(kept_site(ViewKind::Game), ObjectWalk::Unsupported, 2);
    keep(kept_site(ViewKind::Game), ObjectWalk::Order, 1);
    keep(kept_site(ViewKind::Alternate), ObjectWalk::Reservation, 3);
    keep(kept_site(ViewKind::Game), ObjectWalk::State, 0);
    assert!(kept("scene_kept_3_object_unsupported") >= part + 2);
    assert!(kept("scene_kept_3_object_order") > order);
    assert!(kept("scene_kept_alt3_object_reservation") >= alt + 3);
    assert_eq!(ObjectWalk::PartLayout.name(), "part_layout");
}
