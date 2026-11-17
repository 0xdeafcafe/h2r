//! `render-census` end to end on a real runtime and the synthetic fixture
//! module (no game code): the fixture's `MAIN` (it calls the leaf at
//! `LEAF`) stands in for `render_frame_and_present`, the leaf for
//! `render_scene_passes`. A second feature's observer on the leaf plays
//! the render front-end: guest-code accesses through the ABI's pc-carrying
//! accessors, a D3D entry's write under the D3D phase, a host write, and a
//! slice of another guest thread. Between frames the test plays the sim.
//! Its own binary: frame stats are process-wide.

use std::sync::Arc;

use engine::points;
use feature::render_census::{Options, RenderCensus};
use feature::{Registry, Remap, Switches, from_fn};
use runtime::exec::HARNESS_EXIT;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Dispatch, Event, ExecCtx, Guest, Runner, Runtime};
use timeline::phase::{self, Phase};

const MAIN: u32 = 0x40_1000;
const LEAF: u32 = 0x40_2000;
const STACK: u32 = 0x60_0e00;
/// Data pages: render-written by guest code, by D3D only, by the host
/// only; render-read and sim-written; written by another thread.
const A: u32 = 0x61_0000;
const B: u32 = 0x61_1000;
const C: u32 = 0x61_2000;
const D: u32 = 0x61_3000;
const E: u32 = 0x61_4000;

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
    g.map_zeroed(A, 0x5000, Permissions::RW).unwrap();
    g.prepare_fast_pages();
    Runtime::new(g, d)
}

fn call(rt: &Arc<Runtime>, at: u32) {
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
}

#[test]
fn the_census_pairs_render_and_sim_accesses_on_a_real_runtime() {
    let rt = runtime();
    let out = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("render-census.tsv");
    let _ = std::fs::remove_file(&out);
    let census = RenderCensus::with_options(Options {
        every: 2,
        top: 10,
        lines: 4,
        out: Some(out.clone()),
        print: false,
    })
    .with_region(B, B + 0x1000, "push_buffer");
    let mut r = Registry::new(Switches::parse("render-census").unwrap());
    assert!(r.add(&census).unwrap());
    r.add(&from_fn("front-end", |b| {
        b.raw()
            .observe_before(&points::RENDER_SCENE_PASSES, "work", |o| {
                let mut g = o.guest();
                g.write_bytes_from(0x0003_0000, A + 8, &[1, 2, 3, 4], 1)
                    .unwrap();
                let mut v = [0; 4];
                g.read_bytes_from(0x0003_0004, D, &mut v, 1).unwrap();
                {
                    let _d3d = phase::enter(Phase::D3d);
                    g.set_u32(B, 0xd3d).unwrap();
                }
                g.set_u32(C, 0xc).unwrap();
                phase::guest_thread(Some(2));
                g.write_bytes_from(0x0009_0000, E, &[1], 1).unwrap();
                phase::guest_thread(Some(1));
            });
        Ok(())
    }))
    .unwrap();
    let plan = r
        .build(&Remap::new(&[
            (&points::RENDER_FRAME, MAIN),
            (&points::RENDER_SCENE_PASSES, LEAF),
        ]))
        .unwrap();
    plan.install(&rt).unwrap();
    phase::start(phase::Config::default()).unwrap();
    phase::frame_end(0);
    phase::guest_thread(Some(1));
    let sim = || {
        let mut g = rt.memory();
        let mut v = [0; 4];
        g.read_bytes_from(0x0005_0000, A + 8, &mut v, 1).unwrap();
        assert_eq!(v, [1, 2, 3, 4], "the sim sees the render's bytes");
        g.read_bytes_from(0x0005_0004, B, &mut v, 1).unwrap();
        g.write_bytes_from(0x0005_0008, D, &[7], 1).unwrap();
    };
    for _ in 0..3 {
        call(&rt, MAIN);
        sim();
    }
    // The fourth entry pairs the third frame and writes the report.
    call(&rt, MAIN);
    phase::guest_thread(None);
    let _ = phase::stop();

    let t = census.with_census(|c| c.totals());
    assert_eq!((t.frames, t.pairs), (4, 3));
    assert_eq!(t.other_thread, 4, "thread 2's write, every frame");
    // Render writes: A (guest), B (D3D only), C (host only), and the stack
    // (MAIN's call pushes its return address).
    assert_eq!(t.rw_guest, 4 * 2, "A and the stack page");
    assert_eq!((t.rw_d3d_only, t.rw_host_only), (4, 4));
    assert_eq!(t.rs, 3 * 2, "A and B, after each of the first three frames");
    assert_eq!(t.rs_core, 3, "A: B is our D3D's");
    assert_eq!(
        t.sr,
        3 * 2,
        "before frames 2-4: D, and the stack page the harness writes"
    );
    assert_eq!(t.ww, 3, "the stack page: MAIN's call, the harness's write");
    assert_eq!(t.dropped, 0);
    census.with_census(|c| {
        let a = &c.pages()[&(A >> 12)];
        assert_eq!(a.render_writer.unwrap().pc, 0x0003_0000);
        assert_eq!(a.rs_reader.unwrap().pc, 0x0005_0000);
        assert_eq!(c.regions.classify(B), "push_buffer");
        assert_eq!(c.regions.classify(STACK), "stack");
        assert!(!c.pages().contains_key(&(E >> 12)));
        let s = &c.pages()[&(STACK >> 12)];
        assert!(s.rw_guest != 0, "the fixture's own call writes the stack");
        assert!(
            s.render_writer.unwrap().pc != 0,
            "attributed to the fixture code's pc"
        );
        assert_eq!(c.sticky(), [A >> 12, B >> 12], "picked at the report");
        // Line detail on A after the report: frame 4 wrote line 0, and
        // nothing has read it since.
        assert_eq!(a.rw_lines, 1);
    });
    let tsv = std::fs::read_to_string(&out).unwrap();
    // Written at the third entry, after the second pair.
    assert!(
        tsv.starts_with("# render-census frames=2 pairs=2\n"),
        "{tsv}"
    );
    assert!(
        tsv.lines()
            .any(|l| l.starts_with("0x00611000\tpush_buffer\t2\t0\t2\t0\td3d\t")),
        "{tsv}"
    );
    // The trap stays installed; nothing the census armed broke the bytes.
    assert_eq!(rt.memory().u32(B).unwrap(), 0xd3d);
}

/// Off unless switched on; on, it observes `render_frame_and_present`'s
/// entry and return and the game's end.
#[test]
fn the_census_is_off_by_default() {
    let mut r = Registry::new(Switches::defaults());
    assert!(!r.add(&RenderCensus::new()).unwrap());
    let mut r = Registry::new(Switches::parse("render-census").unwrap());
    assert!(r.add(&RenderCensus::new()).unwrap());
    let text = r.build(&feature::Base).unwrap().text();
    for want in [
        "RENDER_FRAME: observe_before [render-census:census enter] -> original -> observe_after [render-census:census leave]",
        "GAME_END",
    ] {
        assert!(text.contains(want), "{want}: {text}");
    }
}
