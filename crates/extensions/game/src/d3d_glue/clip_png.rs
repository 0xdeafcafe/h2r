use gfx::GfxContext;
use gfx::blit::{BlitOptions, Blitter};
use gfx::presenter::OffscreenTarget;
use gfx::surface::{SurfaceId, SurfaceStore};

use super::{read_rgba, write_png};

pub(super) struct ClipPng {
    size: Option<(u32, u32)>,
    output: Option<(Blitter, OffscreenTarget)>,
    present: u64,
    slot: u32,
}

impl ClipPng {
    pub(super) fn new(size: Option<(u32, u32)>) -> Self {
        Self {
            size,
            output: None,
            present: 0,
            slot: 0,
        }
    }

    pub(super) fn guest(
        &mut self,
        present: u64,
        ctx: &GfxContext,
        store: &SurfaceStore,
        id: SurfaceId,
    ) {
        self.present = present;
        self.slot = 0;
        self.save(ctx, store, id);
    }

    pub(super) fn replay(&mut self, ctx: &GfxContext, store: &SurfaceStore, id: SurfaceId) {
        self.slot += 1;
        self.save(ctx, store, id);
    }

    fn save(&mut self, ctx: &GfxContext, store: &SurfaceStore, id: SurfaceId) {
        agent::render::clip::CLIPS.write(self.present, self.slot, |path| {
            let (pixels, width, height) = match self.size {
                Some(size) => self.read_sized(ctx, store, id, size)?,
                None => read_rgba(ctx, store, id).map_err(|e| e.to_string())?,
            };
            write_png(path, &pixels, width, height).map_err(|e| e.to_string())
        });
    }

    fn read_sized(
        &mut self,
        ctx: &GfxContext,
        store: &SurfaceStore,
        id: SurfaceId,
        (width, height): (u32, u32),
    ) -> Result<(Vec<u8>, u32, u32), String> {
        let surface = store.get(id).map_err(|e| format!("{e:?}"))?;
        let src = surface.desc();
        if src.width < width || src.height < height {
            return Err(format!(
                "the {}x{} rendered view cannot cover {width}x{height}; raise H2_RENDER_SCALE",
                src.width, src.height
            ));
        }
        let (blitter, target) = self.output.get_or_insert_with(|| {
            let target = OffscreenTarget::new(
                &ctx.device,
                (width, height),
                wgpu::TextureFormat::Rgba8Unorm,
            );
            (Blitter::new(ctx), target)
        });
        let dst = target.target();
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        store
            .resolve_msaa(&mut enc, id)
            .map_err(|e| format!("{e:?}"))?;
        blitter.blit(
            ctx,
            &mut enc,
            surface.view(),
            dst.color,
            dst.color_format,
            BlitOptions {
                src_rect: center_crop((src.width, src.height), (width, height)),
                linear: true,
                force_opaque: true,
                gamma: None,
            },
        );
        ctx.queue.submit([enc.finish()]);
        Ok((target.read_pixels(ctx), width, height))
    }
}

pub(super) fn parse_size(raw: &str, max: u32) -> Result<(u32, u32), String> {
    let parse = |value: &str| value.parse::<u32>().ok().filter(|n| (1..=max).contains(n));
    raw.split_once('x')
        .and_then(|(w, h)| Some((parse(w)?, parse(h)?)))
        .ok_or_else(|| {
            format!("H2_BOOT_GFX_SIZE must be WxH with each dimension between 1 and {max}")
        })
}

fn center_crop((src_w, src_h): (u32, u32), (dst_w, dst_h): (u32, u32)) -> [f32; 4] {
    let ratio = (dst_w as f32 / dst_h as f32) / (src_w as f32 / src_h as f32);
    let w = ratio.min(1.0);
    let h = ratio.recip().min(1.0);
    [(1.0 - w) / 2.0, (1.0 - h) / 2.0, w, h]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uhd_uses_the_center_2160_rows_of_the_6x_game_view() {
        let uhd = parse_size("3840x2160", 4096).unwrap();
        assert_eq!(uhd, (3840, 2160));
        assert_eq!(center_crop((3840, 2880), uhd), [0.0, 0.125, 1.0, 0.75]);
        for bad in ["", "3840X2160", "0x2160", "3840x-1", "4097x2160"] {
            assert!(parse_size(bad, 4096).is_err(), "{bad}");
        }
    }
}
