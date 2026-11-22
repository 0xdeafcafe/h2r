//! Pixel-based detail in the structure record walks. Each of their three
//! callers resets the record counters, builds clusters and placements, then
//! builds object records. Object lighting needs the retail projection.
//! A menu's model widget (the profile and Appearance previews) renders
//! through the same record walk; it keeps the retail factor.

use std::sync::{Arc, Mutex, OnceLock};

use crate::setting_defs::{GUEST_FRAME, LOD_DISTANCE, lod_pixel_multiplier};
use d3d::scaled::ScaleStats;
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy, points};
use render_bridge::native_scene_lists::{NativeSceneLists, ViewLodScale};
use runtime::{Guest, GuestFault, guest::Permissions};

#[derive(Clone)]
pub struct LodDistance {
    scale_stats: Option<Arc<Mutex<ScaleStats>>>,
    distance: Option<settings::Setting<u32>>,
    startup_distance: u32,
    fixed_multiplier: Option<f32>,
    /// Guest address published during registration for the native object walk.
    view_scale: OnceLock<ViewLodScale>,
}

impl LodDistance {
    pub fn new(
        store: Option<&settings::Store>,
        scale_stats: Option<Arc<Mutex<ScaleStats>>>,
    ) -> anyhow::Result<Self> {
        let distance = store.map(|s| s.handle(LOD_DISTANCE.key));
        let mut value = LOD_DISTANCE.default;
        if distance.is_none()
            && let Ok(raw) = std::env::var("H2_LOD_DISTANCE")
        {
            value = LOD_DISTANCE
                .parse_env_text(&raw)
                .map_err(anyhow::Error::msg)?;
        }
        Ok(Self {
            scale_stats,
            view_scale: OnceLock::new(),
            distance,
            fixed_multiplier: None,
            startup_distance: value.as_int().expect("LOD Distance is a numeric choice") as u32,
        })
    }

    /// Keep protected geometry independent of the local display and settings.
    pub fn with_fixed_multiplier(mut self, multiplier: f32) -> Self {
        self.fixed_multiplier = Some(multiplier);
        self
    }

    /// The active target at the last frame boundary. A plain GPU backend
    /// renders the guest frame at its original size.
    fn output_height(&self) -> u32 {
        self.scale_stats.as_ref().map_or(GUEST_FRAME.1, |s| {
            s.lock().unwrap_or_else(|e| e.into_inner()).output.1
        })
    }

    pub fn view_scale(&self) -> Option<ViewLodScale> {
        self.view_scale.get().copied()
    }

    /// Sample the geometry multiplier once at the view boundary.
    fn multiplier(&self) -> f32 {
        if let Some(fixed) = self.fixed_multiplier {
            return fixed;
        }
        let distance = self
            .distance
            .as_ref()
            .map_or(self.startup_distance, settings::Setting::get);
        lod_pixel_multiplier(self.output_height(), distance)
    }
}

impl Feature for LodDistance {
    fn manifest(&self) -> Manifest {
        Manifest::new("lod-distance", "Output-resolution level of detail")
            .doc("docs/graphics/occlusion-culling.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // Both words belong to the current render context. A worker snapshot
        // or a mid-view restore carries the factor and original projection together.
        // The third word marks the UI model view, so a snapshot taken inside
        // a widget's view carries it with the factor.
        b.snapshot(SnapshotPolicy::Stateless);
        let saved = b.guest_alloc(12, Permissions::RW)?;
        let view_scale = ViewLodScale::new(saved + 4);
        self.view_scale
            .set(view_scale)
            .map_err(|_| BuildError("LOD distance already registered".into()))?;
        NativeSceneLists::install_guest_lod(b, view_scale);
        let setting = self.clone();
        b.raw()
            .before(
                &points::RESET_SCENE_RECORDS,
                "size structure records",
                move |c| {
                    if c.addr == points::RESET_SCENE_RECORDS.base {
                        let factor = setting.multiplier();
                        let mut mem = c.ctx.runtime().memory();
                        let caller = mem.u32(c.cpu.esp)?;
                        reset_view(&mut mem, saved, caller, factor)?;
                    }
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        // A model widget's view (0x2b0e12 calls 0x2c560, which reaches the
        // world caller 0x31ef5 through 0x2bcd0 and 0x31c80) keeps retail
        // detail. The mark is set only when the return site is generated,
        // so it cannot be left set for the next world view.
        b.raw()
            .before(&points::ui_render::MODEL_VIEW_CALL, "retail LOD in the UI model view", move |c| {
                let rt = c.ctx.runtime();
                if rt.read_dispatch(|d| d.has_generated(points::ui_render::MODEL_VIEW_RETURN.base)) {
                    set_model_view(&mut rt.memory(), saved, true)?;
                }
                Ok(Flow::Continue)
            })
            .context_only()
            .after_feature("native-hud")
            .optional();
        b.raw()
            .before(&points::ui_render::MODEL_VIEW_RETURN, "end the UI model view's retail LOD", move |c| {
                set_model_view(&mut c.ctx.runtime().memory(), saved, false)?;
                Ok(Flow::Continue)
            })
            .context_only()
            .after_feature("native-hud")
            .optional();
        b.raw()
            .before(
                &points::OBJECT_RENDER_RECORDS,
                "restore object projection",
                move |c| {
                    if c.addr == points::OBJECT_RENDER_RECORDS.base {
                        end_structure(&mut c.ctx.runtime().memory(), saved)?;
                    }
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        Ok(())
    }
}

fn reset_view(mem: &mut Guest, saved: u32, caller: u32, factor: f32) -> Result<(), GuestFault> {
    // A prior walk may have exited before OBJECT_RENDER_RECORDS restored the
    // projection. Recover it even when switching to Retail or another caller.
    end_structure(mem, saved)?;
    // Only these three callers bracket both structure walks with the reset
    // and OBJECT_RENDER_RECORDS. Any other view, and a menu's model widget
    // (which reaches 0x31ef5), records retail detail.
    let world = matches!(caller, 0x0002_cbcc | 0x0003_1ef5 | 0x0003_f3b9);
    let factor = if world && !in_model_view(mem, saved)? { factor } else { 1.0 };
    mem.set_f32(saved + 4, factor)?;
    if factor == 1.0 {
        return Ok(());
    }
    let vertical = mem.f32(points::PIXEL_SCALE)?;
    let scaled = vertical * factor;
    if vertical > 0.0 && scaled.is_finite() && scaled != vertical {
        mem.set_f32(saved, vertical)?;
        mem.set_f32(points::PIXEL_SCALE, scaled)?;
    }
    Ok(())
}

const MODEL_VIEW: u32 = 8;

fn set_model_view(mem: &mut Guest, saved: u32, inside: bool) -> Result<(), GuestFault> {
    mem.set_u32(saved + MODEL_VIEW, u32::from(inside))
}

fn in_model_view(mem: &Guest, saved: u32) -> Result<bool, GuestFault> {
    Ok(mem.u32(saved + MODEL_VIEW)? != 0)
}

fn end_structure(mem: &mut Guest, saved: u32) -> Result<(), GuestFault> {
    let original = mem.u32(saved)?;
    if original != 0 {
        mem.set_u32(points::PIXEL_SCALE, original)?;
        mem.set_u32(saved, 0)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detail_follows_the_active_render_target_and_live_distance() {
        let store = settings::Store::defaults(crate::setting_defs::ALL);
        let stats = Arc::new(Mutex::new(ScaleStats {
            output: GUEST_FRAME,
            ..Default::default()
        }));
        let lod = LodDistance::new(Some(&store), Some(stats.clone())).unwrap();
        store
            .set(
                crate::setting_defs::RESOLUTION.key,
                settings::Resolution::Fixed(2560, 1600).value(),
            )
            .unwrap();
        assert_eq!(lod.multiplier(), 1.0);
        let recorded = lod.clone().with_fixed_multiplier(2.5);
        stats.lock().unwrap().output = (2560, 1600);
        assert_eq!(lod.multiplier(), 1600.0 / 480.0);
        store
            .set(LOD_DISTANCE.key, LOD_DISTANCE.parse("1.5").unwrap())
            .unwrap();
        assert_eq!(lod.multiplier(), 5.0);
        assert_eq!(recorded.multiplier(), 2.5);
        store
            .set(LOD_DISTANCE.key, LOD_DISTANCE.parse("retail").unwrap())
            .unwrap();
        assert_eq!(lod.multiplier(), 1.0);
    }

    #[test]
    fn interrupted_structure_walk_restores_before_the_next_view() {
        let mut mem = Guest::new();
        let saved = 0x1000;
        let cached = ViewLodScale::new(saved + 4);
        let caller = 0x0003_f3b9;
        mem.map_zeroed(saved, 0x1000, Permissions::RW).unwrap();
        mem.map_zeroed(points::PIXEL_SCALE & !0xfff, 0x1000, Permissions::RW)
            .unwrap();
        mem.set_f32(points::PIXEL_SCALE, 30.0).unwrap();
        for (next_caller, factor, pixels, kept) in [
            (caller, 4.0, 120.0, 4.0),
            (caller, 1.5, 45.0, 1.5),
            (caller, 1.0, 30.0, 1.0),
            (caller, 2.0, 60.0, 2.0),
            // Not a world caller: retail detail, for its objects too.
            (0, 3.0, 30.0, 1.0),
            (caller, 3.0, 90.0, 3.0),
        ] {
            reset_view(&mut mem, saved, next_caller, factor).unwrap();
            assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), pixels);
            assert_eq!(cached.read(&mem), kept);
        }
        end_structure(&mut mem, saved).unwrap();
        assert_eq!(mem.u32(saved).unwrap(), 0);
        assert_eq!(
            cached.read(&mem),
            3.0,
            "object records retain this view's factor"
        );
    }

    #[test]
    fn a_menu_model_view_keeps_retail_detail_and_the_world_still_scales() {
        let mut mem = Guest::new();
        let saved = 0x1000;
        let cached = ViewLodScale::new(saved + 4);
        let world = 0x0003_1ef5; // The caller the UI model view reaches.
        mem.map_zeroed(saved, 0x1000, Permissions::RW).unwrap();
        mem.map_zeroed(points::PIXEL_SCALE & !0xfff, 0x1000, Permissions::RW)
            .unwrap();
        mem.set_f32(points::PIXEL_SCALE, 30.0).unwrap();

        set_model_view(&mut mem, saved, true).unwrap();
        reset_view(&mut mem, saved, world, 4.0).unwrap();
        assert_eq!(cached.read(&mem), 1.0);
        assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), 30.0);
        assert_eq!(mem.u32(saved).unwrap(), 0, "no projection saved to restore");

        set_model_view(&mut mem, saved, false).unwrap();
        reset_view(&mut mem, saved, world, 4.0).unwrap();
        assert_eq!(cached.read(&mem), 4.0);
        assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), 120.0);
        end_structure(&mut mem, saved).unwrap();
        assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), 30.0);
    }

    #[test]
    fn projected_structure_pixels_follow_the_view_and_object_lighting_keeps_the_retail_bits() {
        let mut mem = Guest::new();
        let saved = 0x1000;
        mem.map_zeroed(saved, 0x1000, Permissions::RW).unwrap();
        mem.map_zeroed(points::PIXEL_SCALE & !0xfff, 0x1000, Permissions::RW)
            .unwrap();
        for (output_height, half_units, guest_y, want) in [
            (480, 2, 30.0f32, 30.0f32),
            (1920, 2, 30.0, 120.0),
            (1920, 3, 15.0, 90.0), // A shorter split-screen viewport.
            (2160, 0, 24.0, 24.0), // Retail after a scaled view.
        ] {
            mem.set_f32(points::PIXEL_SCALE, guest_y).unwrap();
            reset_view(
                &mut mem,
                saved,
                0x0003_f3b9,
                lod_pixel_multiplier(output_height, half_units),
            )
            .unwrap();
            assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), want);
            end_structure(&mut mem, saved).unwrap();
            assert_eq!(mem.u32(points::PIXEL_SCALE).unwrap(), guest_y.to_bits());
            assert_eq!(mem.u32(saved).unwrap(), 0);
        }
    }
}
