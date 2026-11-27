//! The view snapshot: the guest's colour copy `0x000352e0`, made on the GPU
//! (docs/graphics/notes/2026-11-27-snapshot-slot.md).
//!
//! `0x000352e0(target, four)` copies the view's colour into a render
//! target (`-1`: target 19, `0x13`) through one or four stages. The
//! camouflage and hologram handlers (`0x00041cc0`, `0x000423c0`) call it at
//! the entry of their sorted entry, so a snapshot is taken in the view's
//! order at that entry, in slot 9 or 10, and holds every pixel drawn before
//! it. Its other callers (the water's `0x0003bea0`, `0x00025820`,
//! `0x0001c0d0`) take the same copy at their own place.
//!
//! [`Snapshots`] keeps one scratch texture per target copy, made again when
//! the extent or format changes; [`ViewSnapshot::record`] downsamples the
//! colour into it before the pass that samples it. Every stage that
//! samples the snapshot binds [`ViewSnapshot::view`].
//!
//! [`ColourCopies`] is the full-size copy beside it: the water edge blend's
//! stages that sample the view's own target (id 5) read the bound colour as
//! the pass found it ([`ColourCopy::record`], a texture copy, no filter),
//! since an attachment cannot be sampled as it is drawn into.

use std::sync::{Arc, Mutex};

/// The guest's snapshot size at render scale 1 (engine texture 17,
/// `object_program::SCENE_COPY_SIZE`).
pub const GUEST_SIZE: (u32, u32) = crate::object_program::SCENE_COPY_SIZE;

/// The snapshot's size for a colour drawn at `size` whose guest surface is
/// `guest`: [`GUEST_SIZE`] times the render scale, at least one texel a
/// side.
pub fn extent(guest: (u32, u32), size: (u32, u32)) -> (u32, u32) {
    let side = |copy: u32, size: u32, guest: u32| -> u32 {
        ((u64::from(copy) * u64::from(size) + u64::from(guest.max(1)) / 2) / u64::from(guest.max(1))).max(1) as u32
    };
    (side(GUEST_SIZE.0, size.0, guest.0), side(GUEST_SIZE.1, size.1, guest.1))
}

/// One snapshot: the downsample of the colour `from` into `view`, to record
/// before the pass that samples it.
pub struct ViewSnapshot {
    pub from: wgpu::TextureView,
    pub view: wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub extent: (u32, u32),
    blitter: Arc<Mutex<Option<gfx::blit::Blitter>>>,
}

impl ViewSnapshot {
    /// Records the downsample into `encoder`: a linear read of the whole
    /// colour into the snapshot, [`Self::extent`] texels (the guest's
    /// four-stage copy averages the same neighbourhood).
    pub fn record(&self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder) {
        let mut guard = self.blitter.lock().unwrap_or_else(|e| e.into_inner());
        let blitter = guard.get_or_insert_with(|| gfx::blit::Blitter::on_device(device));
        blitter.blit_on(
            device, encoder, &self.from, &self.view, self.format,
            gfx::blit::BlitOptions { linear: true, ..Default::default() },
        );
    }
}

/// The snapshots' scratch textures, one per target copy, and the
/// downsample's pipeline.
#[derive(Default)]
pub struct Snapshots {
    scratch: [Option<wgpu::TextureView>; 2],
    blitter: Arc<Mutex<Option<gfx::blit::Blitter>>>,
}

impl Snapshots {
    /// The snapshot of `color` (guest surface `guest`) for target copy
    /// `copy`: its scratch texture ([`extent`]), kept or made again, and
    /// the downsample to record ([`ViewSnapshot::record`]).
    pub fn take(
        &mut self, device: &wgpu::Device, color: &wgpu::TextureView, guest: (u32, u32), copy: gfx::external::TargetCopy,
    ) -> ViewSnapshot {
        let texture = color.texture();
        let format = texture.format();
        let extent = extent(guest, (texture.width(), texture.height()));
        let scratch = match copy {
            gfx::external::TargetCopy::Reference => &mut self.scratch[0],
            gfx::external::TargetCopy::Display => &mut self.scratch[1],
        };
        if scratch.as_ref().is_some_and(|view| {
            let t = view.texture();
            (t.width(), t.height()) != extent || t.format() != format
        }) {
            *scratch = None;
        }
        let view = scratch.get_or_insert_with(|| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("native view snapshot"),
                    size: wgpu::Extent3d { width: extent.0, height: extent.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        });
        ViewSnapshot { from: color.clone(), view: view.clone(), format, extent, blitter: self.blitter.clone() }
    }
}

/// Whether `color` (`size` texels) can be copied: a single-sampled 2D
/// texture of that size that allows copies from it.
pub fn can_copy(color: &wgpu::TextureView, size: (u32, u32)) -> bool {
    let texture = color.texture();
    texture.usage().contains(wgpu::TextureUsages::COPY_SRC)
        && texture.dimension() == wgpu::TextureDimension::D2
        && texture.sample_count() == 1
        && texture.depth_or_array_layers() == 1
        && (texture.width(), texture.height()) == size
}

/// One full-size copy of the bound colour: `from` into `to` (`extent`),
/// to record before the pass; `view` is `to`'s.
pub struct ColourCopy {
    pub from: wgpu::Texture,
    pub to: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub extent: wgpu::Extent3d,
}

impl ColourCopy {
    /// Records the copy into `encoder`.
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        encoder.copy_texture_to_texture(self.from.as_image_copy(), self.to.as_image_copy(), self.extent);
    }
}

/// The full-size copies' scratch textures, one per target copy.
#[derive(Default)]
pub struct ColourCopies {
    scratch: [Option<wgpu::TextureView>; 2],
}

impl ColourCopies {
    /// The copy of `color` for target copy `copy`: its scratch texture, of
    /// `color`'s extent and format, kept or made again.
    pub fn take(&mut self, device: &wgpu::Device, color: &wgpu::TextureView, copy: gfx::external::TargetCopy) -> ColourCopy {
        let texture = color.texture();
        let extent = texture.size();
        let format = texture.format();
        let scratch = match copy {
            gfx::external::TargetCopy::Reference => &mut self.scratch[0],
            gfx::external::TargetCopy::Display => &mut self.scratch[1],
        };
        if scratch.as_ref().is_some_and(|view| {
            let t = view.texture();
            t.size() != extent || t.format() != format
        }) {
            *scratch = None;
        }
        let view = scratch.get_or_insert_with(|| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("native view colour copy"),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        });
        ColourCopy { from: texture.clone(), to: view.texture().clone(), view: view.clone(), extent }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_snapshot_is_the_guest_size_times_the_render_scale() {
        assert_eq!(extent((640, 480), (640, 480)), (320, 240));
        assert_eq!(extent((640, 480), (1280, 960)), (640, 480));
        assert_eq!(extent((640, 480), (1920, 1080)), (960, 540));
        assert_eq!(extent((640, 480), (1, 1)), (1, 1), "never empty");
    }

    /// A camouflaged biped's layer 18 samples the snapshot of its entry:
    /// the colour drawn before it, downsampled, one per target copy and made
    /// again at another scale.
    #[test]
    fn a_camouflaged_biped_samples_the_snapshot() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let mut snapshots = Snapshots::default();
        let color = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("color"),
            size: wgpu::Extent3d { width: 1280, height: 960, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let color = color.create_view(&Default::default());
        let first = snapshots.take(&ctx.device, &color, (640, 480), gfx::external::TargetCopy::Reference);
        assert_eq!(first.extent, (640, 480));
        assert_eq!((first.view.texture().width(), first.view.texture().height()), (640, 480));
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        first.record(&ctx.device, &mut encoder);
        ctx.queue.submit([encoder.finish()]);
        let again = snapshots.take(&ctx.device, &color, (640, 480), gfx::external::TargetCopy::Reference);
        assert_eq!(again.view.texture(), first.view.texture(), "kept for the same copy and size");
        let display = snapshots.take(&ctx.device, &color, (640, 480), gfx::external::TargetCopy::Display);
        assert_ne!(display.view.texture(), first.view.texture(), "one per target copy");
        let other = snapshots.take(&ctx.device, &color, (1280, 960), gfx::external::TargetCopy::Reference);
        assert_eq!((other.view.texture().width(), other.view.texture().height()), (320, 240));
    }

    /// The water's colour copy is the bound colour at full size, one per
    /// target copy, made again when the colour's size changes; a colour
    /// that does not allow copies, or of another size, cannot be copied.
    #[test]
    fn the_colour_copy_is_full_size_and_one_per_target_copy() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let texture = |size: (u32, u32), usage: wgpu::TextureUsages| {
            ctx.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("color"),
                    size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let color = texture((64, 32), wgpu::TextureUsages::COPY_SRC);
        assert!(can_copy(&color, (64, 32)));
        assert!(!can_copy(&color, (32, 32)), "another size");
        assert!(!can_copy(&texture((64, 32), wgpu::TextureUsages::empty()), (64, 32)), "no copies from it");
        let mut copies = ColourCopies::default();
        let first = copies.take(&ctx.device, &color, gfx::external::TargetCopy::Reference);
        assert_eq!((first.to.width(), first.to.height()), (64, 32));
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        first.record(&mut encoder);
        ctx.queue.submit([encoder.finish()]);
        assert_eq!(copies.take(&ctx.device, &color, gfx::external::TargetCopy::Reference).to, first.to, "kept");
        assert_ne!(copies.take(&ctx.device, &color, gfx::external::TargetCopy::Display).to, first.to, "one per target copy");
        let larger = texture((128, 64), wgpu::TextureUsages::COPY_SRC);
        assert_eq!(copies.take(&ctx.device, &larger, gfx::external::TargetCopy::Reference).to.width(), 128);
    }
}
