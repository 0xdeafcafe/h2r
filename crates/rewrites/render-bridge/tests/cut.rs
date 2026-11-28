//! The cut table and the frame step (docs/graphics/framescene.md, 5.2 and
//! 8.2) on the generated fixture module: the fixture's leaf (`eax = 41;
//! ret`) at 0x402000 stands in for a producer's guest body, through `Remap`.

use std::sync::Arc;

use engine::memory::{FlatMemory, GuestMemory};
use hooks::{Registry, Remap, Switches, from_fn};
use render_bridge::cut::{self, CUTS, Cut, Refused};
use render_bridge::frame_step::{self, Addrs, Owner, ROWS, Step};
use render_bridge::producers::{self, REGISTRY, Status};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime};

const LEAF: u32 = 0x40_2000;
const STACK: u32 = 0x60_0e00;
const A: Addrs = Addrs::HALO2;

static GAME_VIEW: [Cut; 1] = [Cut {
    name: "game_view",
    entry: 0x0002_b790,
    abi_result: 7,
    pop: 0,
}];

fn runtime() -> Arc<Runtime> {
    let mut d = Dispatch::default();
    // SAFETY: the fixture's tables, handlers and probes follow its ABI.
    unsafe {
        d.register_generated_module(
            &runtime_test_probe::h2gen_fixture_module,
            runtime_test_probe::h2gen_fixture_module_abi_version,
        )
        .unwrap();
    }
    let mut g = Guest::new();
    g.map_zeroed(0x60_0000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(0x40_0000, 0x5000, Permissions::RX).unwrap();
    // The words the frame step writes.
    for page in [A.present_counter, A.frame_counter, A.one_shot] {
        g.map_zeroed(page & !0xfff, 0x1000, Permissions::RW).unwrap();
    }
    Runtime::new(g, d)
}

fn call(rt: &Arc<Runtime>, at: u32) -> CpuAbi {
    rt.memory().set_u32(STACK, HARNESS_EXIT).unwrap();
    let mut cpu = CpuAbi {
        esp: STACK,
        ..Default::default()
    };
    let mut ctx = ExecCtx::new(Arc::clone(rt), 1);
    let t = Runner::default().run(&mut cpu, &mut ctx, at);
    assert!(
        matches!(ctx.event(t.event_id), Some(Event::HarnessExit)),
        "{:?}",
        ctx.event(t.event_id)
    );
    assert_eq!(cpu.esp, STACK + 4, "the caller's stack is as a return leaves it");
    cpu
}

fn install(rt: &Arc<Runtime>, cuts: &'static [Cut]) {
    let mut r = Registry::new(Switches::defaults());
    r.add(&from_fn("cuts", move |b| {
        cut::install(b, cuts);
        Ok(())
    }))
    .unwrap();
    let map: Vec<_> = cuts.iter().map(|c| (cut::point(c), LEAF)).collect();
    r.build(&Remap::new(&map)).unwrap().install(rt).unwrap();
}

fn frame_memory() -> FlatMemory {
    FlatMemory::new(0x0048_0000, 0x0007_0000)
}

#[test]
fn a_cut_producer_never_runs_its_guest_body() {
    let rt = runtime();
    {
        let mut g = rt.memory();
        g.set_u32(A.frame_counter, 5).unwrap();
        g.set_u8(A.one_shot, 1).unwrap();
    }
    assert_eq!(call(&rt, LEAF).eax, 41, "the body, before the cut");
    install(&rt, &GAME_VIEW);
    assert_eq!(
        call(&rt, LEAF).eax,
        7,
        "the entry answers with its ABI result, not the body's"
    );
    let g = rt.memory();
    assert_eq!(
        (g.u32(A.frame_counter).unwrap(), g.u8(A.one_shot).unwrap()),
        (5, 1),
        "a producer's turn makes none of the frame owner's writes"
    );
}

#[test]
fn an_uncut_producer_is_untouched() {
    let rt = runtime();
    install(&rt, &[]);
    assert_eq!(call(&rt, LEAF).eax, 41, "the body runs");
    let g = rt.memory();
    assert_eq!(g.u32(A.frame_counter).unwrap(), 0);
    assert_eq!(g.u32(A.present_counter).unwrap(), 0);
    drop(g);
    let mut m = frame_memory();
    assert_eq!(
        frame_step::writes(&mut m, Step::Producer("game_view")).unwrap(),
        Vec::<&str>::new(),
        "no row belongs to a producer yet"
    );
    assert!(CUTS.iter().all(|c| c.name != "hud"), "the HUD is not cut here");
}

#[test]
fn the_frame_step_makes_the_ticks_writes_in_guest_order() {
    let mut m = frame_memory();
    m.set_u32(A.frame_counter, 9).unwrap();
    m.set_u8(A.one_shot, 1).unwrap();
    m.set_u32(A.present_counter, 0xffff_ffff).unwrap();
    m.set_u32(A.present_counter + 4, 3).unwrap();
    let made = frame_step::writes(&mut m, Step::Frame).unwrap();
    assert_eq!(
        made,
        ["render_frame_counter", "one_shot_byte", "present_counter"],
        "render_views' counter and byte, then the present's counter"
    );
    assert_eq!(m.u32(A.frame_counter).unwrap(), 10);
    assert_eq!(m.u8(A.one_shot).unwrap(), 0);
    assert_eq!(m.u32(A.present_counter).unwrap(), 0);
    assert_eq!(m.u32(A.present_counter + 4).unwrap(), 4, "the 64-bit add carries");
    // The no-op rows come first in the order, so a slice that fills one
    // lands before the counters, as the guest's object and particle code
    // runs inside the views.
    let names: Vec<_> = ROWS.iter().map(|r| r.name).collect();
    assert_eq!(
        names[..5],
        ["shared_seed", "effects_word", "particle_stamps", "animation_stamps", "residency"]
    );
    // Every row names a test in this file.
    let this = include_str!("cut.rs");
    for r in ROWS {
        assert!(this.contains(&format!("fn {}()", r.test)), "{}: {}", r.name, r.test);
    }
}

#[test]
fn the_cut_table_names_every_registry_producer() {
    cut::check(CUTS).unwrap();
    for (i, c) in CUTS.iter().enumerate() {
        assert!(CUTS[..i].iter().all(|o| o.name != c.name), "{} twice", c.name);
    }
    // Native rows with no entry of their own: the guest body is called
    // only from a cut producer, `dynamic_lights` (`render_light_passes`,
    // once per frame, docs/graphics/native-lights.md), so it never runs.
    const INSIDE_A_CUT: &[&str] = &["light_passes"];
    for row in REGISTRY
        .iter()
        .filter(|r| r.status == Status::Native && !INSIDE_A_CUT.contains(&r.name))
    {
        assert!(
            CUTS.iter().any(|c| c.name == row.name),
            "{} is native in the census and not cut",
            row.name
        );
    }
    for r in ROWS {
        if let Owner::Producers(names) = r.owner {
            for n in names {
                assert!(producers::row(n).is_some(), "{n} is not a registry row");
            }
        }
    }
    cut::check(&GAME_VIEW).unwrap();
    let ghost = [Cut { name: "ghost", ..GAME_VIEW[0] }];
    assert_eq!(cut::check(&ghost), Err(Refused::NotARow("ghost")));
    let moved = [Cut { entry: 0x1234, ..GAME_VIEW[0] }];
    assert_eq!(
        cut::check(&moved),
        Err(Refused::WrongEntry { name: "game_view", cut: 0x1234, row: 0x0002_b790 })
    );
}

/// The object setup's residency step makes the guest's requests, as
/// writes in its output (`native_scene_lists::residency_request`), so the
/// cut makes them with the records: a resident block stamps its cache
/// entry, a request already queued stamps and counts the structure miss,
/// and one that needs a new queue entry is the guest cache's own service.
#[test]
#[cfg(feature = "native-render")]
fn object_setup_makes_the_guests_residency_requests() {
    use engine::testing::FakeGuest;
    use render_bridge::native_scene_lists::{ResidencyRequest, residency_request};
    const CACHE: u32 = 0x004e_649c;
    const REQUESTS: u32 = 0x004e_648c;
    const NONBLOCKING: u32 = 0x0046_8c4c;
    const STRUCTURE_MISSES: u32 = 0x004e_64a8;
    const TAGS: u32 = 0x004e_3b44;
    let (resource, cache, pool, slots) = (0x10000, 0x20000, 0x21000, 0x22000);
    let word = |v: u32| v.to_le_bytes().to_vec();
    let mut mem = FakeGuest::new();
    mem.put_u32(CACHE, cache)
        .put_u32(cache + 0x64, pool)
        .put_u32(cache + 0x38, 324)
        .put_u32(pool + 0x24, 0x18)
        .put_u32(pool + 0x38, 2)
        .put_u32(pool + 0x44, slots);

    // Resident: the cache's current epoch into the block's entry.
    mem.put_u8(resource + 0x1e, 1).put_u16(resource + 0x20, 1);
    assert_eq!(
        residency_request(&mem, resource, true),
        ResidencyRequest::Made { resident: true, writes: vec![(slots + 0x18 + 0x14, word(324))] }
    );

    // Not resident, its request already queued at the priority the
    // nonblocking path asks for: stamped, and a structure miss counted.
    let (handle, requests, queue, tags) = (0x8900_0000u32, 0x23000, 0x24000, 0x25000);
    mem.put_u8(resource + 0x1e, 0)
        .put_u32(resource + 8, 12)
        .put_u32(resource + 0x20, handle)
        .put_u32(REQUESTS, requests)
        .put_u32(requests + 0x24, 12)
        .put_u32(requests + 0x38, 1)
        .put_u32(requests + 0x44, queue)
        .put_u16(queue, (handle >> 16) as u16)
        .put_u8(queue + 3, 2)
        .put_u32(TAGS, tags)
        .put_u32(tags, u32::from_be_bytes(*b"sbsp"))
        .put_u32(STRUCTURE_MISSES, 3)
        .put_u8(NONBLOCKING, 1);
    assert_eq!(
        residency_request(&mem, resource, true),
        ResidencyRequest::Made {
            resident: false,
            writes: vec![(slots + 0x14, word(324)), (STRUCTURE_MISSES, word(4))],
        }
    );

    // No queue entry yet: a new request is the guest cache's service.
    mem.put_u32(resource + 0x20, u32::MAX);
    assert_eq!(residency_request(&mem, resource, true), ResidencyRequest::NeedsGuestService);

    // The object setup is not cut: a call its build refuses (a light
    // sample due, a menu's model view, a locked pose) runs the guest body.
    assert!(CUTS.iter().all(|c| c.name != "object_setup"));
    let row = producers::row("object_setup").expect("a registry row");
    assert_eq!(REGISTRY[row].status, Status::Partial);
    assert!(cut::OWN_PRODUCER.iter().all(|n| CUTS.iter().any(|c| c.name == *n)));
    assert!(CUTS.iter().any(|c| c.name == "object_shadows" && c.entry == 0x0003_35e0 && c.pop == 4));
}

/// The world layers cut draw in the slots the design traces, and the
/// layers whose kept guest draws are counted are the table's.
#[cfg(feature = "native-render")]
#[test]
fn the_world_layers_are_cut_in_their_slots() {
    use render_bridge::cut::LAYER_CUTS;
    for c in LAYER_CUTS {
        assert_eq!(native_render::layer_items::world_slot(c.layer), Some(c.slot), "layer {}", c.layer);
    }
    let mut layers: Vec<u8> = LAYER_CUTS.iter().map(|c| c.layer).collect();
    layers.sort_unstable();
    assert_eq!(layers, native_render::layer_items::WORLD_LAYERS.to_vec(), "one row per world layer");
    for l in render_bridge::native_layers::SCENE_ONLY {
        assert!(layers.contains(&(l as u8)), "layer {l}");
    }
}
