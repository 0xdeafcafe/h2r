//! Pixel-based detail in the structure record walks. Each of their three
//! callers resets the record counters, builds clusters and placements, then
//! builds object records. Object lighting needs the retail projection.

use std::sync::{Arc, Mutex};

use crate::setting_defs::{GUEST_FRAME, LOD_DISTANCE, lod_pixel_multiplier};
use d3d::scaled::ScaleStats;
use hooks::{BuildError, Builder, Feature, Flow, Manifest, SnapshotPolicy, points};
use runtime::{Guest, GuestFault, guest::Permissions};

#[derive(Clone)]
pub struct LodDistance {
    scale_stats: Option<Arc<Mutex<ScaleStats>>>,
    distance: Option<settings::Setting<u32>>,
    startup_distance: u32,
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
            distance,
            startup_distance: value.as_int().expect("LOD Distance is a numeric choice") as u32,
        })
    }

    /// The active target at the last frame boundary. A plain GPU backend
    /// renders the guest frame at its original size.
    fn output_height(&self) -> u32 {
        self.scale_stats.as_ref().map_or(GUEST_FRAME.1, |s| {
            s.lock().unwrap_or_else(|e| e.into_inner()).scale * GUEST_FRAME.1
        })
    }

    /// Multiplier for a visual size already expressed in guest pixels.
    pub fn multiplier(&self) -> f32 {
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
        // Save the exact retail bits in this render context's guest RAM.
        // The worker snapshot carries them if a cold resource hands one of
        // the walks back to the game thread halfway through.
        b.snapshot(SnapshotPolicy::Stateless);
        let objects = self.clone();
        render_bridge::native_scene_lists::NativeSceneLists::install_guest_lod(
            b,
            Arc::new(move || objects.multiplier()),
        );
        let saved = b.guest_alloc(4, Permissions::RW)?;
        let setting = self.clone();
        b.raw()
            .before(
                &points::RESET_SCENE_RECORDS,
                "size structure records",
                move |c| {
                    let factor = setting.multiplier();
                    if c.addr == points::RESET_SCENE_RECORDS.base && factor != 1.0 {
                        let mut mem = c.ctx.runtime().memory();
                        // These callers unconditionally reach OBJECT_RENDER_RECORDS
                        // immediately after the cluster and placement walks.
                        if matches!(mem.u32(c.cpu.esp)?, 0x0002_cbcc | 0x0003_1ef5 | 0x0003_f3b9) {
                            begin_structure(&mut mem, saved, factor)?;
                        }
                    }
                    Ok(Flow::Continue)
                },
            )
            .context_only();
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

fn begin_structure(mem: &mut Guest, saved: u32, multiplier: f32) -> Result<(), GuestFault> {
    if multiplier == 1.0 {
        return Ok(());
    }
    let vertical = mem.f32(points::PIXEL_SCALE)?;
    let scaled = vertical * multiplier;
    if vertical > 0.0 && scaled.is_finite() && scaled != vertical {
        mem.set_f32(saved, vertical)?;
        mem.set_f32(points::PIXEL_SCALE, scaled)?;
    }
    Ok(())
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
            scale: 1,
            ..Default::default()
        }));
        let lod = LodDistance::new(Some(&store), Some(stats.clone())).unwrap();
        store
            .set(
                crate::setting_defs::RENDER_SCALE.key,
                settings::Value::Int(4),
            )
            .unwrap();
        assert_eq!(lod.multiplier(), 1.0);
        stats.lock().unwrap().scale = 4;
        assert_eq!(lod.multiplier(), 4.0);
        store
            .set(LOD_DISTANCE.key, LOD_DISTANCE.parse("1.5").unwrap())
            .unwrap();
        assert_eq!(lod.multiplier(), 6.0);
        store
            .set(LOD_DISTANCE.key, LOD_DISTANCE.parse("retail").unwrap())
            .unwrap();
        assert_eq!(lod.multiplier(), 1.0);
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
            begin_structure(
                &mut mem,
                saved,
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
