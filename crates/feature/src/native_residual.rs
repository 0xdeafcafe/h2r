//! View scopes for the native renderer's remaining Direct3D calls.

use std::cell::Cell;
use std::sync::OnceLock;

use engine::points;

use crate::{BuildError, Builder, Feature, Manifest, Switches};

pub const SWITCH: &str = "impl.render.native_residual";
pub const ID: &str = "native-residual";

pub fn on(switches: &Switches) -> bool {
    switches.feature(SWITCH, false)
}

pub fn probe_frames() -> &'static [u64] {
    static FRAMES: OnceLock<Vec<u64>> = OnceLock::new();
    FRAMES.get_or_init(|| {
        let Ok(spec) = std::env::var("H2_NATIVE_RESIDUAL_PROBE") else {
            return Vec::new();
        };
        spec.split(',')
            .map(|word| word.trim().parse::<u64>().ok().filter(|&n| n > 0))
            .collect::<Option<Vec<_>>>()
            .unwrap_or_else(|| {
                eprintln!("native-residual: H2_NATIVE_RESIDUAL_PROBE wants comma-separated present numbers");
                Vec::new()
            })
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum ViewScope {
    #[default]
    OutsideView,
    ViewSetup,
    Alternate,
    Scene(u32),
    ViewTail,
}

thread_local! {
    static VIEW: Cell<ViewScope> = const { Cell::new(ViewScope::OutsideView) };
}

pub fn view() -> ViewScope {
    VIEW.get()
}

pub struct NativeResidual;

impl Feature for NativeResidual {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "Native residual scene draws and Direct3D caller census")
            .doc("docs/graphics/native-renderer.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        if probe_frames().is_empty() {
            return Ok(());
        }
        b.raw()
            .observe_before(&points::RENDER_FRAME, "residual_frame_scope", |_| {
                VIEW.set(ViewScope::OutsideView);
            })
            .context_only();
        b.raw()
            .observe_before(&points::RENDER_GAME_VIEW, "residual_view_scope", |_| {
                VIEW.set(ViewScope::ViewSetup);
            })
            .context_only();
        b.raw()
            .observe_before(
                &points::RENDER_ALTERNATE_VIEW,
                "residual_alternate_scope",
                |_| {
                    VIEW.set(ViewScope::Alternate);
                },
            )
            .context_only();
        b.raw()
            .observe_before(&points::RENDER_SCENE_PASSES, "residual_scene_scope", |o| {
                let kind = o.guest().u32(o.cpu.esp.wrapping_add(4)).ok();
                VIEW.set(kind.map_or(ViewScope::ViewSetup, ViewScope::Scene));
            })
            .context_only();
        b.raw()
            .observe_before(&points::RENDER_STENCIL_MODE, "residual_scene_return", |o| {
                // The scene wrapper returns to render_view_scene immediately
                // before this stencil reset. Other callers can be inside the scene.
                const AFTER_SCENE_STENCIL_RETURN_PC: u32 = 0x0002_bc83;
                if matches!(VIEW.get(), ViewScope::Scene(_))
                    && o.guest().u32(o.cpu.esp).ok() == Some(AFTER_SCENE_STENCIL_RETURN_PC)
                {
                    VIEW.set(ViewScope::ViewTail);
                }
            })
            .context_only();
        b.raw()
            .observe_before(&points::RENDER_FRAME_TAIL, "residual_tail_scope", |_| {
                VIEW.set(ViewScope::OutsideView);
            })
            .context_only();
        b.snapshot(crate::SnapshotPolicy::Stateless);
        Ok(())
    }
}
