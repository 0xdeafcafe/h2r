//! The `occlusion` feature on the runtime, over the synthetic generated
//! module (no game code): `OBJECT_RENDER_RECORDS` stands at the fixture's
//! `ret` (0x404000) and `OBJECT_RENDER_SETUP` at its leaf (0x402000,
//! `eax = 41; ret`), through `Remap`. The guest holds a synthetic object
//! list, object array and camera at the game's addresses. A skipped setup
//! returns with AL 0, `ret 0x14` and the entry's "not drawn" flag; one
//! that is not skipped reaches the original (EAX 41).

use std::sync::Arc;

use engine::points;
use engine::render_loop::occlusion::guest::*;
use engine::render_loop::occlusion::{Camera, DepthGrid, Frame, HiZ};
use feature::{Registry, Remap, Switches};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime};

const RECORDS: u32 = 0x40_4000;
const SETUP: u32 = 0x40_2000;
const STACK: u32 = 0x60_0e00;
const ARRAY: u32 = 0x61_0000;
const DATA: u32 = 0x61_1000;
const BODIES: u32 = 0x61_2000;
const LIST: u32 = 0x61_3000;
const FLAGS: u32 = 0x61_3100;
const HANDLES: u32 = 0x61_3200;

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
        [20.0f32, 0.0, 0.0],
        [20.0, 3.0, 1.0],
        [3.0, 0.0, 0.0],
        [20.0, -1.0, 0.0],
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
    Runtime::new(g, d)
}

fn wall(frame: u64) -> Frame {
    let mut g = DepthGrid::far(64, 32);
    g.depth.iter_mut().for_each(|d| *d = 5.0);
    Frame {
        hiz: HiZ::build(g),
        camera: Camera::from_fov(
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            std::f32::consts::FRAC_PI_2,
            1.0,
            0.05,
        ),
        frame,
    }
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
    assert!(r.add(&feature::occlusion::OcclusionCull).unwrap());
    let plan = r
        .build(&Remap::new(&[
            (&points::OBJECT_RENDER_RECORDS, RECORDS),
            (&points::OBJECT_RENDER_SETUP, SETUP),
        ]))
        .unwrap();
    let text = plan.text();
    assert!(
        text.contains("occlusion_begin") && text.contains("occlusion_skip"),
        "{text}"
    );
    plan.install(&rt).unwrap();

    let mut skipped = Vec::new();
    for frame in 1..=3u32 {
        rt.memory().set_u32(FRAME, frame).unwrap();
        if frame > 1 {
            feature::occlusion::MAILBOX.publish(0, wall(frame as u64 - 1));
        }
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
    }
    assert_eq!(skipped, vec![vec![], vec![], vec![1]]);
    let flags: Vec<u8> = (0..4)
        .map(|i| rt.memory().u8(FLAGS + 2 * i).unwrap())
        .collect();
    assert_eq!(flags, vec![0, FLAG_NOT_DRAWN, 0, 0]);
}

#[test]
fn occlusion_is_off_unless_switched_exactly() {
    for switches in ["", "impl.render=on"] {
        let mut r = Registry::new(Switches::parse(switches).unwrap());
        r.add(&feature::occlusion::OcclusionCull).unwrap();
        let text = r.build(&feature::Base).unwrap().text();
        assert_eq!(text, "feature occlusion: on\n", "{switches:?}");
    }
}
