//! The persistent pipeline cache end to end, on synthetic shaders: a run
//! records every pipeline its draws needed; the next run, from the same
//! file, translates those programs and builds their pipelines ahead of time
//! on worker threads, so its draws translate nothing, miss no pipeline, build
//! nothing on the drawing thread and render the same pixels.

mod pipeline_draws;

use std::time::{Duration, Instant};

use d3d::{Backend, GfxBackend, ResolvedDraw};
use gfx::{D3DFormat, GfxContext};
use shaders::psh::bits::*;

use pipeline_draws::{draws_with, frame, scratch};

/// Three draws, two programs with different combiners over one vertex program that writes
/// every output from a register: a triangle in window coordinates, coloured
/// by `v1`.
fn draws() -> Vec<ResolvedDraw> {
    let combiners = [(REG_V0, false), (REG_V1, true), (REG_V0, false)]
        .map(|(src, kills)| (inputs(src, ONE, 0, 0), kills));
    draws_with(&combiners)
}

/// `b` with the exact merge rule: every discarding draw starts a render pass.
fn exact(mut b: GfxBackend) -> GfxBackend {
    b.set_merge_masks(false);
    b
}

#[test]
fn second_run_builds_nothing_on_the_drawing_thread() {
    let ctx = || GfxContext::headless();
    if let Err(e) = ctx() {
        eprintln!("skipping GPU test: {e}");
        return;
    }
    let ctx = || ctx().unwrap();
    let path = scratch("second-run");
    let draws = draws();
    let me = std::thread::current().id();

    // First run: cold. Every pipeline is a miss and gets recorded.
    let mut b = exact(GfxBackend::new(ctx())).with_pipeline_cache(path.clone());
    let want = frame(&mut b, &draws);
    assert!(
        want.chunks(4).any(|p| p != [0, 0, 0, 0]),
        "the draws drew something"
    );
    let passes = b.batch_stats().render_passes;
    let built = b.renderer.cache().len();
    assert!(built >= 2, "{built} pipelines");
    assert_eq!(b.renderer.cache().misses() as usize, built);
    let text = std::fs::read_to_string(&path).unwrap();
    let records = |kind: &str| {
        let tag = format!(",\"r\":{{\"{kind}\":");
        text.lines().filter(|l| l.contains(&tag)).count()
    };
    assert_eq!(records("Pipeline"), built);
    assert_eq!(records("Program"), 2);
    // Both drew before any map was published: one sighting each.
    assert_eq!(records("Seen"), 2);
    drop(b);

    // Second run: warm. Prewarming finishes before the frame (presents
    // drive it, as frames do in the game).
    let mut b = exact(GfxBackend::new(ctx())).with_pipeline_cache(path.clone());
    let warm = b.create_surface(D3DFormat::A8R8G8B8, 8, 8, false).unwrap();
    let start = Instant::now();
    while b.renderer.cache().len() < built || b.renderer.cache().iter().any(|(_, p)| !p.is_ready())
    {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "prewarm never finished"
        );
        b.present(warm, 0).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(frame(&mut b, &draws), want);
    assert_eq!(
        b.renderer.cache().misses(),
        0,
        "every pipeline was prewarmed"
    );
    assert!(
        b.renderer
            .cache()
            .iter()
            .all(|(_, p)| p.built_on().is_some_and(|t| t != me))
    );
    // The prewarmed discarding program still gets its own pass.
    assert_eq!(b.batch_stats().render_passes, passes);
    // Nothing new to record.
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);

    // Without the cache: same pixels, built at draw time, and the same as
    // one render pass per draw.
    let mut b = exact(GfxBackend::new(ctx()));
    assert_eq!(frame(&mut b, &draws), want);
    assert_eq!(b.batch_stats().render_passes, passes);
    let mut b = GfxBackend::new(ctx());
    b.set_merge_passes(false);
    assert_eq!(frame(&mut b, &draws), want);
    assert!(b.batch_stats().render_passes >= passes);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A warm start registers at most `PREWARM_PER_PRESENT` programs a present,
/// however many the prewarm thread has ready, so presents never stall on
/// it; the rest follow on later presents.
#[test]
fn prewarm_registers_a_bounded_number_a_present() {
    let ctx = || GfxContext::headless();
    if let Err(e) = ctx() {
        eprintln!("skipping GPU test: {e}");
        return;
    }
    let ctx = || ctx().unwrap();
    let path = scratch("paced");
    let per_present = d3d::nv2a::PREWARM_PER_PRESENT;
    // Three presents' worth of programs, one pipeline each.
    let maps = [
        MAP_UNSIGNED_IDENTITY,
        MAP_UNSIGNED_INVERT,
        MAP_EXPAND_NORMAL,
        MAP_EXPAND_NEGATE,
        MAP_HALFBIAS_NORMAL,
        MAP_HALFBIAS_NEGATE,
        MAP_SIGNED_IDENTITY,
        MAP_SIGNED_NEGATE,
    ];
    let combiners: Vec<(u32, bool)> = [REG_V0, REG_V1]
        .into_iter()
        .flat_map(|src| [ONE, REG_V1].map(move |b| (src, b)))
        .flat_map(|(src, b)| maps.map(move |m| (inputs(src | m, b, 0, 0), false)))
        .take(3 * per_present)
        .collect();
    assert_eq!(combiners.len(), 3 * per_present);
    let draws = draws_with(&combiners);

    let mut b = GfxBackend::new(ctx()).with_pipeline_cache(path.clone());
    let want = frame(&mut b, &draws);
    let built = b.renderer.cache().len();
    assert_eq!(built, draws.len(), "one pipeline per program");
    drop(b);

    let mut b = GfxBackend::new(ctx()).with_pipeline_cache(path.clone());
    let warm = b.create_surface(D3DFormat::A8R8G8B8, 8, 8, false).unwrap();
    let before = b.renderer.cache().len();
    assert!(before <= per_present, "{before} registered at open");
    // Let the thread fill its backlog, then one present takes a bounded
    // share of it.
    std::thread::sleep(Duration::from_millis(500));
    b.present(warm, 0).unwrap();
    let after = b.renderer.cache().len();
    assert!(
        after - before <= per_present,
        "{} in one present",
        after - before
    );
    let start = Instant::now();
    while b.renderer.cache().len() < built || b.renderer.cache().iter().any(|(_, p)| !p.is_ready())
    {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "prewarm never finished"
        );
        b.present(warm, 0).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(frame(&mut b, &draws), want);
    assert_eq!(
        b.renderer.cache().misses(),
        0,
        "every pipeline was prewarmed"
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
