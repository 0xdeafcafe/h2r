//! The pipeline cache across a map change, on synthetic shaders: leaving a
//! level drops what is left of its prewarmed pipelines from the compile
//! queue and queues the menu's again, so the menu's pipelines are never
//! behind the level's. A process of its own: the loaded map
//! (`pipeline_store::set_map`) is process-wide, and the other cache tests
//! expect none.

mod pipeline_draws;

use std::sync::Arc;
use std::time::{Duration, Instant};

use d3d::pipeline_store::{MENU_MAP, set_map};
use d3d::{Backend, GfxBackend};
use gfx::compile::Compiler;
use gfx::{D3DFormat, GfxContext};
use shaders::psh::bits::*;

use pipeline_draws::{draws_with, frame, scratch};

const LEVEL: &str = "pipeline_scope_level";

/// Presents `b` until `done`, at most a minute.
fn present_until(
    b: &mut GfxBackend,
    surface: gfx::surface::SurfaceId,
    done: impl Fn(&GfxBackend) -> bool,
) {
    let start = Instant::now();
    while !done(b) {
        assert!(
            start.elapsed() < Duration::from_secs(60),
            "prewarm never finished"
        );
        b.present(surface, 0).unwrap();
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// A backend on `compiler` (no workers: nothing builds unless a draw
/// needs it, so the queue can be counted) with the cache at `path`.
fn backend(ctx: GfxContext, compiler: &Arc<Compiler>, path: &std::path::Path) -> GfxBackend {
    let mut b = GfxBackend::new(ctx);
    b.renderer.set_compiler(compiler.clone());
    b.with_pipeline_cache(path.to_path_buf())
}

#[test]
fn leaving_a_level_drops_its_prewarms_and_queues_the_menu_s_again() {
    let ctx = || GfxContext::headless();
    if let Err(e) = ctx() {
        eprintln!("skipping GPU test: {e}");
        return;
    }
    let ctx = || ctx().unwrap();
    let path = scratch("scope");
    // Three programs drawn on the menu, three others on the level.
    let combiners: Vec<(u32, bool)> = [
        MAP_UNSIGNED_IDENTITY,
        MAP_UNSIGNED_INVERT,
        MAP_EXPAND_NORMAL,
        MAP_EXPAND_NEGATE,
        MAP_HALFBIAS_NORMAL,
        MAP_HALFBIAS_NEGATE,
    ]
    .map(|m| (inputs(REG_V0 | m, ONE, 0, 0), false))
    .to_vec();
    let menu = draws_with(&combiners[..3]);
    let level = draws_with(&combiners[3..]);

    // First run: records the menu's programs on the menu and the level's
    // on the level.
    set_map(MENU_MAP);
    let compiler = Compiler::new(0);
    let mut b = backend(ctx(), &compiler, &path);
    let want = frame(&mut b, &menu);
    set_map(LEVEL);
    frame(&mut b, &level);
    assert_eq!(b.renderer.cache().len(), 6, "one pipeline per program");
    drop(b);

    // Second run, booted to the menu: the startup scope registers the
    // menu's programs and queues their pipelines.
    set_map(MENU_MAP);
    let compiler = Compiler::new(0);
    let mut b = backend(ctx(), &compiler, &path);
    let warm = b.create_surface(D3DFormat::A8R8G8B8, 8, 8, false).unwrap();
    present_until(&mut b, warm, |b| b.renderer.cache().len() == 3);
    assert_eq!(compiler.queued(), 3);

    // The level loads: the menu's queued pipelines are dropped and the
    // level's queued in their place.
    set_map(LEVEL);
    present_until(&mut b, warm, |b| b.renderer.cache().len() == 6);
    assert_eq!(compiler.queued(), 3, "only the level's are queued");

    // Back to the menu: the level's leftovers go, and the menu's
    // programs, registered already, have their pipelines queued again.
    // Nothing is translated again.
    set_map(MENU_MAP);
    b.present(warm, 0).unwrap();
    assert_eq!(compiler.queued(), 3, "only the menu's are queued");
    assert_eq!(b.renderer.cache().len(), 6);
    assert!(b.renderer.cache().iter().all(|(_, p)| !p.is_ready()));

    // The menu draws as it did, every pipeline a prewarm. Each one its
    // draws need is moved ahead of the prewarms once, however often it
    // is drawn.
    assert_eq!(frame(&mut b, &menu), want);
    assert_eq!(
        b.renderer.cache().misses(),
        0,
        "every pipeline was prewarmed"
    );
    assert_eq!(compiler.queued(), 6, "three prewarms and three draw jobs");
    assert_eq!(frame(&mut b, &menu), want);
    assert_eq!(compiler.queued(), 6, "built pipelines are not queued again");
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}
