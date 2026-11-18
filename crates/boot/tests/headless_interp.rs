//! In-between frames on the headless GPU backend (`H2_BOOT_GFX`), as an
//! off-screen `h2 serve` runs them (docs/game/high-framerate.md "E1",
//! "Headless"): with `impl.render.reproject` and the frame rate above 30,
//! the run has an interp and the agent's `render.stats` reads its counters
//! (every hold reason, zero or not) instead of `null`. One test in its own
//! binary: it sets the process's environment. Skips without a GPU adapter.

use runtime::guest::Permissions;
use runtime::{Dispatch, Guest, Runtime};

#[test]
fn the_headless_gpu_backend_runs_reprojection_and_reports_its_counters() {
    if let Err(e) = gfx::GfxContext::headless() {
        eprintln!("skipping GPU test: {e}");
        return;
    }
    let dir = std::env::temp_dir().join(format!("h2-headless-interp-{}", std::process::id()));
    // SAFETY: this test binary has one test and no other threads yet.
    unsafe {
        std::env::set_var("H2_BOOT_GFX", &dir);
        std::env::set_var("H2_BOOT_GFX_EVERY", "never");
        std::env::set_var("H2_FEATURES", "impl.render.reproject");
        std::env::set_var("H2_FPS", "60");
        std::env::set_var("H2_PIPELINE_CACHE", dir.join("pipelines.bin"));
        for k in [
            "H2_RENDER_THREAD",
            "H2_PIPELINE_RENDER",
            "H2_RENDER_SCALE",
            "H2_LIGHTING",
            "H2_BOOT_GFX_LIGHTING_AB",
        ] {
            std::env::remove_var(k);
        }
    }
    let sources = boot::render_probe::sources();
    assert!((sources.interp)().is_none(), "nothing registered yet");

    // What boot's run asks: no window, H2_BOOT_GFX set, the render thread on.
    let render_thread = boot::d3d_glue::interp_render_thread(
        false,
        false,
        true,
        boot::d3d_glue::render_thread_requested(),
    );
    assert_eq!(render_thread, Some(true));
    let interp =
        boot::d3d_glue::interpolation(None, true).expect("the switch and H2_FPS=60 turn it on");
    assert_eq!(interp.mode, d3d::interp::Mode::Reproject { ahead: false });
    assert_eq!((interp.rate)(), 60, "H2_FPS without a settings store");

    let rt = Runtime::new(Guest::new(), Dispatch::default());
    {
        let mut m = rt.memory();
        m.map_zeroed(0x003f_0000, 0x14000, Permissions::RX).unwrap();
        m.map_zeroed(0x0100_0000, 0x4000, Permissions::RW).unwrap();
    }
    let installed = boot::d3d_glue::install_headless(&rt, Some(interp)).unwrap();

    let v = (sources.interp)().expect("render.stats' interp is not null");
    assert_eq!(v["frames"], 0, "{v}");
    for reason in [
        "size_mismatch",
        "mismatch_extent",
        "mismatch_mapping",
        "mismatch_screen",
        "mismatch_overlay",
        "unsure",
        "unsure_camera",
        "unsure_unpinned",
        "unpaired",
        "unpaired_gap",
        "unpaired_camera",
        "unpaired_views",
        "unpaired_counting",
        "warps",
        "captures",
        "replays",
    ] {
        assert_eq!(v[reason], 0, "{reason} in {v}");
    }
    drop(installed);
    let _ = std::fs::remove_dir_all(&dir);
}
