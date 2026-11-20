//! The `occlusion` feature on the runtime, over the synthetic generated
//! module (no game code): `OBJECT_RENDER_RECORDS` stands at the fixture's
//! `ret` (0x404000) and `OBJECT_RENDER_SETUP` at its leaf (0x402000,
//! `eax = 41; ret`), through `Remap`. The guest holds a synthetic object
//! list, object array and camera at the game's addresses. A skipped setup
//! returns with AL 0, `ret 0x14` and the entry's "not drawn" flag; one
//! that is not skipped reaches the original (EAX 41). `SCENE_TRANSPARENTS`
//! stands at the fixture's `eax = 1; ret` (0x403005): its observer asks the
//! capture function for a grid, and the test answers as the D3D device
//! would: the raw W-buffer of a wall at view depth 20 over the whole view,
//! stored with the game's own w scale (D24S8 over the camera's far plane)
//! and handed over with the device's W parameters (w far 1).

use std::sync::{Arc, Mutex};

use engine::points;
use engine::render_loop::occlusion::DepthMapping;
use engine::render_loop::occlusion::guest::*;
use hooks::{Registry, Remap, Switches};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime};

const RECORDS: u32 = 0x40_4000;
const SETUP: u32 = 0x40_2000;
/// `SCENE_TRANSPARENTS` at the fixture's `eax = 1; ret`.
const TRANSPARENTS: u32 = 0x40_3005;
const STACK: u32 = 0x60_0e00;
const ARRAY: u32 = 0x61_0000;
const DATA: u32 = 0x61_1000;
const BODIES: u32 = 0x61_2000;
const LIST: u32 = 0x61_3000;
const FLAGS: u32 = 0x61_3100;
const HANDLES: u32 = 0x61_3200;
/// The camera's far plane (`+0x44`), a test value.
const FAR: f32 = 1000.0;

fn handle(i: u32) -> u32 {
    0xe000_0000 | (i + 1) << 16 | i
}

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
    g.map_zeroed(0x40_0000, 0x5000, Permissions::RX).unwrap();
    g.map_zeroed(0x4b_9000, 0x2000, Permissions::RW).unwrap();
    g.map_zeroed(0x4e_0000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(0x54_7000, 0x7000, Permissions::RW).unwrap();
    g.map_zeroed(0x60_0000, 0x1_4000, Permissions::RW).unwrap();
    // Objects: 0 the player, 1 behind the wall, 2 in front of it, 3 behind
    // the wall and held by the player.
    let centers = [
        [60.0f32, 0.0, 0.0],
        [60.0, 3.0, 1.0],
        [10.0, 0.0, 0.0],
        [60.0, -1.0, 0.0],
    ];
    g.set_u32(OBJECTS, ARRAY).unwrap();
    g.set_u32(ARRAY + OBJECTS_DATA, DATA).unwrap();
    for (i, c) in centers.iter().enumerate() {
        let i = i as u32;
        let body = BODIES + i * 0x100;
        g.set_u32(DATA + i * HEADER_SIZE + HEADER_BODY, body)
            .unwrap();
        g.set_u32(body + BODY_PARENT, u32::MAX).unwrap();
        for (k, v) in c.iter().enumerate() {
            g.set_u32(body + BODY_SPHERE_CENTER + 4 * k as u32, v.to_bits())
                .unwrap();
        }
        g.set_u32(body + BODY_SPHERE_RADIUS, 0.5f32.to_bits())
            .unwrap();
        g.set_u32(HANDLES + 4 * i, handle(i)).unwrap();
    }
    g.set_u32(BODIES + 0x300 + BODY_PARENT, handle(0)).unwrap();
    g.set_u32(MAIN_VIS + VIS_OBJECTS, LIST).unwrap();
    g.set_u32(LIST + LIST_COUNT, centers.len() as u32).unwrap();
    g.set_u32(LIST + LIST_FLAGS, FLAGS).unwrap();
    g.set_u32(LIST + LIST_HANDLES, HANDLES).unwrap();
    g.set_u32(VIEW_PLAYER, handle(0)).unwrap();
    let cam = [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0];
    for (k, v) in cam.iter().enumerate() {
        g.set_u32(CAMERA + 4 * k as u32, v.to_bits()).unwrap();
    }
    g.set_u32(CAMERA + 0x28, std::f32::consts::FRAC_PI_2.to_bits())
        .unwrap();
    // Viewport top 0, left 0, bottom 480, right 480.
    g.set_u32(CAMERA + 0x30, 0).unwrap();
    g.set_u32(CAMERA + 0x34, 480 | 480 << 16).unwrap();
    // The far plane the game scales its W-buffer by.
    g.set_u32(CAMERA_FAR, FAR.to_bits()).unwrap();
    Runtime::new(g, d)
}

/// Runs `at` as the game would call it and returns the CPU after.
fn call(rt: &Arc<Runtime>, at: u32, cpu: CpuAbi) -> CpuAbi {
    let mut cpu = CpuAbi { esp: STACK, ..cpu };
    rt.memory().set_u32(STACK, HARNESS_EXIT).unwrap();
    let mut ctx = ExecCtx::new(Arc::clone(rt), 1);
    let t = Runner::default().run(&mut cpu, &mut ctx, at);
    assert!(
        matches!(ctx.event(t.event_id), Some(Event::HarnessExit)),
        "{:?}",
        ctx.event(t.event_id)
    );
    cpu
}

fn setup(rt: &Arc<Runtime>, index: u32) -> CpuAbi {
    rt.memory().set_u32(STACK + 4, 0xabcd_0000 | index).unwrap();
    call(
        rt,
        SETUP,
        CpuAbi {
            esi: MAIN_VIS,
            eax: 0x7777_7701,
            ebx: 0xb0b0,
            edi: 0xd1d1,
            ebp: 0xe0e0,
            ..Default::default()
        },
    )
}

#[test]
fn the_feature_skips_occluded_objects_from_the_second_frame() {
    let rt = runtime();
    let mut r = Registry::new(Switches::parse("impl.render.occlusion=on").unwrap());
    assert!(r.add(&hooks::occlusion::OcclusionCull).unwrap());
    let plan = r
        .build(&Remap::new(&[
            (&points::OBJECT_RENDER_RECORDS, RECORDS),
            (&points::OBJECT_RENDER_SETUP, SETUP),
            (&points::SCENE_TRANSPARENTS, TRANSPARENTS),
        ]))
        .unwrap();
    let text = plan.text();
    assert!(
        text.contains("occlusion_begin") && text.contains("occlusion_skip"),
        "{text}"
    );
    assert!(text.contains("occlusion_capture"), "{text}");
    plan.install(&rt).unwrap();
    let asked: Arc<Mutex<Vec<(u64, u32, u32)>>> = Arc::default();
    {
        let asked = Arc::clone(&asked);
        hooks::occlusion::set_capture(Box::new(move |tag, w, h| {
            asked.lock().unwrap().push((tag, w, h));
        }));
    }

    let mut skipped = Vec::new();
    for frame in 1..=3u32 {
        rt.memory().set_u32(FRAME, frame).unwrap();
        call(
            &rt,
            RECORDS,
            CpuAbi {
                eax: MAIN_VIS,
                ..Default::default()
            },
        );
        let mut this = Vec::new();
        for i in 0..4 {
            let cpu = setup(&rt, i);
            assert_eq!(
                [cpu.ebx, cpu.edi, cpu.ebp, cpu.esi],
                [0xb0b0, 0xd1d1, 0xe0e0, MAIN_VIS]
            );
            if cpu.eax == 41 {
                assert_eq!(cpu.esp, STACK + 4, "the original ran");
            } else {
                assert_eq!(cpu.eax, 0x7777_7700, "AL 0, no retry");
                assert_eq!(cpu.esp, STACK + 4 + 0x14, "ret 0x14");
                assert_eq!(cpu.ecx, FLAGS + 2 * i);
                this.push(i);
            }
        }
        skipped.push(this);
        // The opaque layers are drawn: the transparents' observer asks for
        // this frame's depth, and the device answers (here, at once).
        call(&rt, TRANSPARENTS, CpuAbi::default());
        let asks: Vec<_> = asked.lock().unwrap().drain(..).collect();
        assert_eq!(asks.len(), 1, "one capture per main view and frame");
        let (tag, w, h) = asks[0];
        assert_eq!((w, h), hooks::occlusion::GRID);
        // A second transparents call in the frame asks for nothing.
        call(&rt, TRANSPARENTS, CpuAbi::default());
        assert!(asked.lock().unwrap().is_empty());
        // The device's grid as the game leaves it: w stored with the
        // game's own scale (`z_scale / far`, far the camera's `+0x44`),
        // handed over with the device's W parameters, which stay at the
        // identity projection's (`+0x944` 1, `+0x948` D24's full scale).
        let z_scale = 16_777_215.0f32;
        let raw = vec![DepthMapping::w_buffer_store(20.0, 1.0 / FAR, z_scale); (w * h) as usize];
        hooks::occlusion::deliver(tag, w, h, &raw, DepthMapping::w_buffer(1.0, z_scale));
    }
    assert_eq!(hooks::occlusion::published(), 3);
    let (grid, map) = hooks::occlusion::last_grid().unwrap();
    assert_eq!((grid.cells, grid.far, grid.dropped), (2048, 0, 0));
    assert!((20.0..20.01).contains(&grid.nearest), "{grid:?}");
    assert_eq!(map, DepthMapping::w_buffer(1.0 / FAR, 16_777_215.0));
    assert_eq!(skipped, vec![vec![], vec![], vec![1]]);
    let flags: Vec<u8> = (0..4)
        .map(|i| rt.memory().u8(FLAGS + 2 * i).unwrap())
        .collect();
    assert_eq!(flags, vec![0, FLAG_NOT_DRAWN, 0, 0]);

    // A snapshot load: its section drops the streaks, the depth and the
    // last grid, so the next frame (later, the camera where it was, which
    // the tracker alone would not take for a cut) culls nothing.
    let policies: Vec<_> = plan
        .snapshot
        .iter()
        .map(|f| (f.id, f.policy.name()))
        .collect();
    assert_eq!(policies, vec![("occlusion", "section")]);
    assert!(plan.snapshot_blockers().is_empty());
    let saved: Vec<(String, Vec<u8>)> = plan
        .save_sections()
        .into_iter()
        .map(|(id, b)| (id.to_string(), b))
        .collect();
    assert_eq!(saved, vec![("occlusion".to_string(), vec![])]);
    plan.load_sections(&saved, &rt).unwrap();
    assert!(hooks::occlusion::last_grid().is_none());
    assert!(hooks::occlusion::MAILBOX.latest(0).is_none());
    rt.memory().set_u32(FRAME, 4).unwrap();
    call(
        &rt,
        RECORDS,
        CpuAbi {
            eax: MAIN_VIS,
            ..Default::default()
        },
    );
    for i in 0..4 {
        rt.memory().set_u8(FLAGS + 2 * i, 0).unwrap();
        assert_eq!(setup(&rt, i).eax, 41, "object {i} drawn after the load");
    }
}

#[test]
fn occlusion_is_off_unless_switched_exactly() {
    for switches in ["", "impl.render=on"] {
        let mut r = Registry::new(Switches::parse(switches).unwrap());
        r.add(&hooks::occlusion::OcclusionCull).unwrap();
        let plan = r.build(&hooks::Base).unwrap();
        assert_eq!(plan.text(), "feature occlusion: on\n", "{switches:?}");
        // Nothing placed, nothing held: no snapshot blocker.
        assert!(plan.snapshot_blockers().is_empty(), "{switches:?}");
    }
}
