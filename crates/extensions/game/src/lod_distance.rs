//! Pixel-based detail in the structure record walks. Each of their three
//! callers resets the record counters, builds clusters and placements, then
//! builds object records. Menu model widgets use the same record walk.
//! The guest selection is always the retail/sim one, so the walk's records,
//! pose evaluation, node matrices and marker data are identical whatever the
//! setting; only a legacy film's recorded multiplier reaches it. The display
//! word carries the local choice to the render paths that substitute
//! geometry without publishing pose, palette or cache writes: the native
//! display passes, and the D3D fallback's draw-boundary swap.

use std::sync::{Arc, OnceLock};

use crate::setting_defs::{GUEST_FRAME, LOD_DISTANCE, LOD_HIGHEST, lod_pixel_multiplier};
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy, points};
use render_bridge::native_scene_lists::{LodDetail, NativeSceneLists, ViewLodScale};
use runtime::{Guest, GuestFault, guest::Permissions};

#[derive(Clone)]
pub struct LodDistance {
    output: Option<crate::run_opts::NativeSize>,
    distance: Option<settings::Setting<u32>>,
    startup_distance: u32,
    reference_multiplier: Option<f32>,
    /// Guest address published during registration for the native object walk.
    view_scale: OnceLock<ViewLodScale>,
}

impl LodDistance {
    pub fn new(
        store: Option<&settings::Store>,
        output: Option<crate::run_opts::NativeSize>,
        viewer_distance: Option<settings::Setting<u32>>,
    ) -> anyhow::Result<Self> {
        let distance = viewer_distance.or_else(|| store.map(|s| s.handle(LOD_DISTANCE.key)));
        let mut value = LOD_DISTANCE.default;
        if distance.is_none()
            && let Ok(raw) = std::env::var("H2_LOD_DISTANCE")
        {
            value = LOD_DISTANCE
                .parse_env_text(&raw)
                .map_err(anyhow::Error::msg)?;
        }
        Ok(Self {
            output,
            view_scale: OnceLock::new(),
            distance,
            reference_multiplier: None,
            startup_distance: value.as_int().expect("LOD Distance is a numeric choice") as u32,
        })
    }

    /// Keep the recorded reference geometry (a legacy film's multiplier)
    /// independent of the local display and settings.
    pub fn with_reference_multiplier(mut self, multiplier: f32) -> Self {
        self.reference_multiplier = Some(multiplier);
        self
    }

    /// The render's output height now; the guest frame's with no renderer.
    fn output_height(&self) -> u32 {
        self.output.as_ref().map_or(GUEST_FRAME.1, |output| output().1)
    }

    pub fn view_scale(&self) -> Option<ViewLodScale> {
        self.view_scale.get().copied()
    }

    /// Sample the display geometry once at the view boundary.
    fn detail(&self) -> LodDetail {
        match self.distance.as_ref().map_or(self.startup_distance, settings::Setting::get) {
            LOD_HIGHEST => LodDetail::Highest,
            units => scaled(lod_pixel_multiplier(self.output_height(), units)),
        }
    }
}

impl Feature for LodDistance {
    fn manifest(&self) -> Manifest {
        Manifest::new("lod-distance", "Output-resolution level of detail")
            .doc("docs/graphics/occlusion-culling.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        // A worker snapshot carries the original projection, the guest
        // selection and the display selection with the model-view bit.
        b.snapshot(SnapshotPolicy::Stateless);
        let saved = b.guest_alloc(12, Permissions::RW)?;
        let view_scale = ViewLodScale::new(saved + 4);
        self.view_scale
            .set(view_scale)
            .map_err(|_| BuildError("LOD distance already registered".into()))?;
        NativeSceneLists::install_guest_lod(b, view_scale, false);
        let setting = self.clone();
        b.raw()
            .before(
                &points::RESET_SCENE_RECORDS,
                "size structure records",
                move |c| {
                    if c.addr == points::RESET_SCENE_RECORDS.base {
                        let detail = setting.detail();
                        let mut mem = c.ctx.runtime().memory();
                        let caller = caller(mem.u32(c.cpu.esp)?);
                        reset_view(&mut mem, saved, caller, detail, setting.reference_multiplier)?;
                    }
                    Ok(Flow::Continue)
                },
            )
            .context_only();
        // Preserve the exact reference's original model-widget policy
        // even when a legacy film supplies a fixed world multiplier.
        b.raw()
            .before(&points::ui_render::MODEL_VIEW_CALL, "mark the UI model view", move |c| {
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
            .before(&points::ui_render::MODEL_VIEW_RETURN, "end the UI model view", move |c| {
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

/// The guest return address of a `RESET_SCENE_RECORDS` call. The light
/// frame body (docs/graphics/native-light-gather.md, L2) makes the call
/// in place of `0x00031c80` with a continuation as its return address,
/// and names the guest's own site while it does.
fn caller(return_address: u32) -> u32 {
    use engine::render::light_gather::frame::{call_site, site};
    match call_site() {
        Some(at) if at == site::RESET_RECORDS => at,
        _ => return_address,
    }
}

fn scaled(multiplier: f32) -> LodDetail {
    if multiplier == 1.0 { LodDetail::Retail } else { LodDetail::Scaled(multiplier) }
}

pub(super) fn reset_view(
    mem: &mut Guest, saved: u32, caller: u32, display: LodDetail, reference: Option<f32>,
) -> Result<(), GuestFault> {
    // A prior walk may have exited before OBJECT_RENDER_RECORDS restored the projection.
    end_structure(mem, saved)?;
    // Only these callers bracket both structure walks with the reset and
    // OBJECT_RENDER_RECORDS. The model widget reaches 0x31ef5.
    let world = matches!(caller, 0x0002_cbcc | 0x0003_1ef5 | 0x0003_f3b9);
    let model_view = mem.u32(saved + 8)? & ViewLodScale::MODEL_VIEW;
    let (guest, display) = if world {
        // The guest walk keeps the retail/sim selection whatever the local
        // display choice: the record walk, its pose evaluation and the
        // matrices the sim's marker lookup reads never follow the setting.
        // A legacy film's recorded multiplier is the one exception: its
        // playback must reproduce the walk that recorded it.
        let guest = match reference {
            Some(_) if model_view != 0 => LodDetail::Retail,
            Some(multiplier) => scaled(multiplier),
            None => LodDetail::Retail,
        };
        (guest, display)
    } else {
        (LodDetail::Retail, LodDetail::Retail)
    };
    mem.set_u32(saved + 4, guest.guest_word())?;
    mem.set_u32(saved + 8, display.guest_word() | model_view)?;
    let factor = guest.pixel_multiplier();
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

fn set_model_view(mem: &mut Guest, saved: u32, inside: bool) -> Result<(), GuestFault> {
    let value = if inside { ViewLodScale::MODEL_VIEW } else { 0 };
    mem.set_u32(saved + 8, value)
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
        let stats = Arc::new(std::sync::Mutex::new(GUEST_FRAME));
        let read = Arc::clone(&stats);
        let output: crate::run_opts::NativeSize = Arc::new(move || *read.lock().unwrap());
        let lod = LodDistance::new(Some(&store), Some(output), None).unwrap();
        store
            .set(
                crate::setting_defs::RESOLUTION.key,
                settings::Resolution::Fixed(2560, 1600).value(),
            )
            .unwrap();
        assert_eq!(lod.detail(), LodDetail::Highest);
        store.set(LOD_DISTANCE.key, LOD_DISTANCE.parse("1").unwrap()).unwrap();
        assert_eq!(lod.detail(), LodDetail::Retail);
        *stats.lock().unwrap() = (2560, 1600);
        assert_eq!(lod.detail(), LodDetail::Scaled(1600.0 / 480.0));
        store.set(LOD_DISTANCE.key, LOD_DISTANCE.parse("1.5").unwrap()).unwrap();
        assert_eq!(lod.detail(), LodDetail::Scaled(5.0));
        store.set(LOD_DISTANCE.key, LOD_DISTANCE.parse("retail").unwrap()).unwrap();
        assert_eq!(lod.detail(), LodDetail::Retail);
    }

    #[test]
    fn a_viewer_chooses_detail_independently_of_the_recorded_settings() {
        let recorded = settings::Store::defaults(crate::setting_defs::ALL);
        recorded.set(LOD_DISTANCE.key, LOD_DISTANCE.parse("retail").unwrap()).unwrap();
        let viewer = settings::Store::defaults(crate::setting_defs::ALL);
        let handle = viewer.handle(LOD_DISTANCE.key);
        let lod = LodDistance::new(Some(&recorded), None, Some(handle)).unwrap()
            .with_reference_multiplier(1.0);
        assert_eq!(lod.detail(), LodDetail::Highest);

        viewer.set(LOD_DISTANCE.key, LOD_DISTANCE.parse("2").unwrap()).unwrap();
        assert_eq!(lod.detail(), LodDetail::Scaled(2.0));
        assert_eq!(lod.reference_multiplier, Some(1.0));
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
        // A legacy film's recorded multiplier scales the guest projection and
        // survives an interrupted walk.
        for (next_caller, factor, pixels, kept) in [
            (caller, 4.0, 120.0, 4.0),
            (caller, 1.5, 45.0, 1.5),
            (caller, 1.0, 30.0, 1.0),
            (caller, 2.0, 60.0, 2.0),
            // Not a world caller: retail detail, for its objects too.
            (0, 3.0, 30.0, 1.0),
            (caller, 3.0, 90.0, 3.0),
        ] {
            reset_view(&mut mem, saved, next_caller, scaled(factor), Some(factor)).unwrap();
            assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), pixels);
            assert_eq!(cached.read(&mem), scaled(kept));
        }
        end_structure(&mut mem, saved).unwrap();
        assert_eq!(mem.u32(saved).unwrap(), 0);
        assert_eq!(
            cached.read(&mem),
            LodDetail::Scaled(3.0),
            "object records retain this view's factor"
        );
        // The local display detail never scales the guest projection.
        for display in [LodDetail::Highest, scaled(4.0), LodDetail::Retail] {
            reset_view(&mut mem, saved, caller, display, None).unwrap();
            assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), 30.0);
            assert_eq!(cached.read(&mem), LodDetail::Retail);
            assert_eq!(cached.display(&mem), display);
        }
        end_structure(&mut mem, saved).unwrap();
        assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), 30.0);
    }

    #[test]
    fn outside_the_light_frame_body_the_caller_is_the_return_address() {
        assert_eq!(caller(0x0002_cbcc), 0x0002_cbcc);
        assert_eq!(caller(0x1234), 0x1234);
    }

    #[test]
    fn a_menu_model_view_keeps_the_retail_walk_and_the_requested_display() {
        let mut mem = Guest::new();
        let saved = 0x1000;
        let cached = ViewLodScale::new(saved + 4);
        let caller = 0x0003_1ef5;
        mem.map_zeroed(saved, 0x1000, Permissions::RW).unwrap();
        mem.map_zeroed(points::PIXEL_SCALE & !0xfff, 0x1000, Permissions::RW).unwrap();
        mem.set_f32(points::PIXEL_SCALE, 30.0).unwrap();

        set_model_view(&mut mem, saved, true).unwrap();
        reset_view(&mut mem, saved, caller, LodDetail::Highest, None).unwrap();
        assert_eq!(cached.read(&mem), LodDetail::Retail);
        assert_eq!(cached.display(&mem), LodDetail::Highest);

        reset_view(&mut mem, saved, caller, LodDetail::Scaled(4.0), Some(2.5)).unwrap();
        assert_eq!(cached.read(&mem), LodDetail::Retail);
        assert_eq!(cached.display(&mem), LodDetail::Scaled(4.0));
        assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), 30.0);

        set_model_view(&mut mem, saved, false).unwrap();
        reset_view(&mut mem, saved, caller, LodDetail::Highest, Some(2.5)).unwrap();
        assert_eq!(cached.read(&mem), LodDetail::Scaled(2.5));
        assert_eq!(cached.display(&mem), LodDetail::Highest);
        end_structure(&mut mem, saved).unwrap();
        assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), 30.0);
    }

    #[test]
    fn the_display_detail_never_reaches_the_guest_walk() {
        // Whatever the setting, the words the record walk reads — the guest
        // selection and the projection's pixel scale — stay at their retail
        // values, so the walk's published records, pose evaluation, the live
        // node matrices the marker lookup reads and the lockstep and film
        // digests are the same for Retail and Highest.
        let mut mem = Guest::new();
        let saved = 0x1000;
        let cached = ViewLodScale::new(saved + 4);
        let caller = 0x0003_f3b9;
        mem.map_zeroed(saved, 0x1000, Permissions::RW).unwrap();
        mem.map_zeroed(points::PIXEL_SCALE & !0xfff, 0x1000, Permissions::RW)
            .unwrap();
        mem.set_f32(points::PIXEL_SCALE, 30.0).unwrap();
        reset_view(&mut mem, saved, caller, LodDetail::Retail, None).unwrap();
        let guest_word = cached.read(&mem);
        let pixels = mem.f32(points::PIXEL_SCALE).unwrap();
        for display in [LodDetail::Highest, scaled(2.0), LodDetail::Retail] {
            reset_view(&mut mem, saved, caller, display, None).unwrap();
            assert_eq!(cached.read(&mem), guest_word);
            assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), pixels);
            assert_eq!(cached.display(&mem), display);
            end_structure(&mut mem, saved).unwrap();
            assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), pixels);
        }
    }

    #[test]
    fn projected_structure_pixels_follow_the_recorded_reference_and_keep_object_lighting_retail() {
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
            // The multiplier a legacy film recorded at this raster size.
            let multiplier = lod_pixel_multiplier(output_height, half_units);
            reset_view(
                &mut mem,
                saved,
                0x0003_f3b9,
                scaled(multiplier),
                Some(multiplier),
            )
            .unwrap();
            assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), want);
            // The same display choice without a recorded reference leaves
            // the guest projection untouched.
            reset_view(&mut mem, saved, 0x0003_f3b9, scaled(multiplier), None).unwrap();
            assert_eq!(mem.f32(points::PIXEL_SCALE).unwrap(), guest_y);
            end_structure(&mut mem, saved).unwrap();
            assert_eq!(mem.u32(points::PIXEL_SCALE).unwrap(), guest_y.to_bits());
            assert_eq!(mem.u32(saved).unwrap(), 0);
        }
    }
}
