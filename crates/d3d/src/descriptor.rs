//! Light-host D3D backend. Keep resource identities and the same synthetic
//! visibility and readback answers as the headless recorder.

use std::collections::{HashMap, HashSet};

use gfx::texture::{DecodedTexture, TextureDesc};
use gfx::{Clear, D3DFormat};

use crate::backend::{Backend, DrawOutcome, GuestLayoutKind, PixelRect, RectClear, ResolvedDraw};

#[derive(Default)]
pub struct DescriptorBackend {
    next_resource: u32,
    surfaces: HashSet<u32>,
    pixels: u64,
    next_ticket: u64,
    reports: HashMap<u64, u64>,
    on_present: Option<fn()>,
}

impl DescriptorBackend {
    pub fn with_present(mut self, on_present: fn()) -> Self {
        self.on_present = Some(on_present);
        self
    }

    fn resource(&mut self) -> u32 {
        self.next_resource += 1;
        self.next_resource
    }
}

impl Backend for DescriptorBackend {
    type Surface = u32;
    type Texture = u32;

    fn create_surface(&mut self, _: D3DFormat, _: u32, _: u32, _: bool) -> Result<u32, String> {
        let id = self.resource();
        self.surfaces.insert(id);
        Ok(id)
    }

    fn destroy_surface(&mut self, id: u32) -> Result<(), String> {
        if !self.surfaces.remove(&id) {
            return Err(format!("surface {id} is not live"));
        }
        Ok(())
    }

    fn upload_texture(&mut self, _: &DecodedTexture) -> Result<u32, String> {
        Err("descriptor backend received texture pixels".into())
    }

    #[inline]
    fn uses_texture_placeholders(&self) -> bool {
        true
    }

    fn texture_placeholder(&mut self, _: &TextureDesc) -> Result<u32, String> {
        Ok(self.resource())
    }

    fn bc_supported(&self) -> bool {
        false
    }

    fn clear(&mut self, _: u32, _: Option<u32>, _: Clear) -> Result<(), String> {
        Ok(())
    }

    fn clear_rects(
        &mut self,
        _: u32,
        _: Option<u32>,
        _: &RectClear,
    ) -> Result<DrawOutcome, String> {
        Ok(DrawOutcome::Drawn)
    }

    fn draw(
        &mut self,
        _: u32,
        _: Option<u32>,
        draw: &ResolvedDraw,
        _: [Option<&u32>; 4],
    ) -> Result<DrawOutcome, String> {
        if draw.hw.zpass_count_enable
            && let Some(clip) = draw.clip
        {
            fn area(r: &PixelRect) -> u64 {
                u64::from(r.width) * u64::from(r.height)
            }
            self.pixels += if draw.clip_rects.is_empty() {
                area(&clip)
            } else {
                draw.clip_rects.iter().map(area).sum()
            };
        }
        Ok(DrawOutcome::Drawn)
    }

    fn present(&mut self, _: u32, _: u32) -> Result<(), String> {
        if let Some(present) = self.on_present {
            present();
        }
        Ok(())
    }

    fn copy_surface(&mut self, _: u32, _: u32) -> Result<bool, String> {
        Ok(true)
    }

    fn write_surface(&mut self, _: u32, _: &TextureDesc, _: &[u8]) -> Result<bool, String> {
        Ok(true)
    }

    fn snapshot_surface(&mut self, _: u32, _: bool) -> Result<Option<u32>, String> {
        Ok(Some(self.resource()))
    }

    fn read_surface(
        &mut self,
        _: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Vec<u8>, String> {
        let bpp = format
            .bytes_per_texel()
            .ok_or_else(|| format!("{format:?} has no texel size"))?;
        let len = match layout {
            GuestLayoutKind::Swizzled => (width * height * bpp) as usize,
            GuestLayoutKind::Linear { pitch } => (pitch * height) as usize,
        };
        Ok(vec![0; len])
    }

    fn read_depth(
        &mut self,
        id: u32,
        format: D3DFormat,
        width: u32,
        height: u32,
        layout: GuestLayoutKind,
    ) -> Result<Option<Vec<u8>>, String> {
        self.read_surface(id, format, width, height, layout)
            .map(Some)
    }

    fn clear_pixel_count(&mut self) -> Result<(), String> {
        self.pixels = 0;
        Ok(())
    }

    fn report_pixel_count(&mut self) -> Result<Option<u64>, String> {
        self.next_ticket += 1;
        self.reports.insert(self.next_ticket, self.pixels);
        Ok(Some(self.next_ticket))
    }

    fn read_pixel_count(&mut self, ticket: u64) -> Result<u64, String> {
        self.reports
            .remove(&ticket)
            .ok_or_else(|| format!("pixel-count ticket {ticket} is not live"))
    }

    fn try_pixel_count(&mut self, ticket: u64) -> Result<Option<u64>, String> {
        self.read_pixel_count(ticket).map(Some)
    }

    fn drop_pixel_count(&mut self, ticket: u64) {
        self.reports.remove(&ticket);
    }
}
