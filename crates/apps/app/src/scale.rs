//! Where the guest frame lands in the window: aspect-correct fitting.
//!
//! The Xbox drives a TV, so a 640x480 back buffer is shown at 4:3 and a
//! 720x480 one is anamorphic (also 4:3, or 16:9 when the dashboard's
//! widescreen setting is on). The display aspect is therefore a setting, not
//! something derived from the back-buffer size.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The shape the guest frame is shown at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DisplayAspect {
    /// 4:3, what a retail Xbox outputs for Halo 2 by default.
    Standard,
    /// 16:9 (the game's widescreen mode renders anamorphic 640x480).
    Widescreen,
    /// Square pixels: the frame's own width / height.
    Source,
    /// Any other width / height ratio. Non-finite or non-positive values
    /// fall back to 4:3.
    Custom(f32),
}

impl DisplayAspect {
    /// Width / height of the displayed image for a frame of `src` pixels.
    pub fn ratio(self, src: (u32, u32)) -> f32 {
        let r = match self {
            DisplayAspect::Standard => 4.0 / 3.0,
            DisplayAspect::Widescreen => 16.0 / 9.0,
            DisplayAspect::Source if src.0 > 0 && src.1 > 0 => src.0 as f32 / src.1 as f32,
            DisplayAspect::Source => 4.0 / 3.0,
            DisplayAspect::Custom(r) => r,
        };
        if r.is_finite() && r > 0.0 {
            r
        } else {
            4.0 / 3.0
        }
    }
}

/// Preserve the drawable aspect when it exceeds a texture limit.
pub(crate) fn limit_drawable(size: (u32, u32), limit: u32) -> (u32, u32) {
    let longest = size.0.max(size.1);
    if longest <= limit || size.0 == 0 || size.1 == 0 {
        return size;
    }
    let scale = |axis: u32| {
        let rounded = (u64::from(axis) * u64::from(limit) + u64::from(longest / 2))
            / u64::from(longest);
        rounded.max(1) as u32
    };
    (scale(size.0), scale(size.1))
}

/// Physical render extent shared by the shell and the render thread. Publish
/// both axes together so a reader cannot observe half of a resize.
#[derive(Clone)]
pub(crate) struct NativeSize {
    aspect: DisplayAspect,
    scaling: Scaling,
    pixels: Arc<AtomicU64>,
}

impl NativeSize {
    pub(crate) fn new(aspect: DisplayAspect, scaling: Scaling, drawable: (u32, u32)) -> Self {
        let size = Self {
            aspect,
            scaling,
            pixels: Arc::new(AtomicU64::new(0)),
        };
        size.update(drawable);
        size
    }

    pub(crate) fn update(&self, (w, h): (u32, u32)) {
        if w != 0 && h != 0 {
            self.pixels.store((u64::from(w) << 32) | u64::from(h), Ordering::Relaxed);
        }
    }

    fn drawable(&self) -> Option<(u32, u32)> {
        let size = self.pixels.load(Ordering::Relaxed);
        (size != 0).then_some(((size >> 32) as u32, size as u32))
    }

    /// Raster size for Native. Stretch fills the drawable, other modes
    /// fit the requested aspect.
    pub(crate) fn get(&self) -> (u32, u32) {
        let Some(drawable) = self.drawable() else {
            return (640, 480);
        };
        if self.aspect == DisplayAspect::Source || self.scaling == Scaling::Stretch {
            return drawable;
        }
        let viewport = fit(self.aspect.ratio(drawable), drawable);
        (viewport.width, viewport.height)
    }

    /// Compositor's physical destination for a source of this size.
    pub(crate) fn display_size(&self, src: (u32, u32)) -> (u32, u32) {
        let Some(drawable) = self.drawable() else {
            return src;
        };
        let viewport = place(src, self.aspect, self.scaling, drawable);
        (viewport.width, viewport.height)
    }
}

/// How the frame is scaled into the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scaling {
    /// As large as fits, aspect kept, black bars on the rest.
    Fit,
    /// The largest whole multiple of the frame height that fits (aspect
    /// kept); falls back to `Fit` when not even 1x fits.
    Integer,
    /// Fill the window, aspect ignored.
    Stretch,
}

/// A pixel rectangle in the window surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Viewport {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Viewport {
    /// True when nothing would be drawn.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// The largest `aspect`-shaped rectangle centred in `target`.
pub fn fit(aspect: f32, target: (u32, u32)) -> Viewport {
    let (tw, th) = target;
    if tw == 0 || th == 0 || !(aspect.is_finite() && aspect > 0.0) {
        return Viewport::default();
    }
    let (w, h) = if (tw as f64) / (th as f64) > aspect as f64 {
        (((th as f64) * aspect as f64).round() as u32, th)
    } else {
        (tw, ((tw as f64) / aspect as f64).round() as u32)
    };
    let (w, h) = (w.clamp(1, tw), h.clamp(1, th));
    Viewport {
        x: (tw - w) / 2,
        y: (th - h) / 2,
        width: w,
        height: h,
    }
}

/// Where a frame of `src` pixels goes in a `target`-sized surface.
pub fn place(
    src: (u32, u32),
    aspect: DisplayAspect,
    scaling: Scaling,
    target: (u32, u32),
) -> Viewport {
    let ratio = aspect.ratio(src);
    match scaling {
        Scaling::Stretch if target.0 > 0 && target.1 > 0 => Viewport {
            x: 0,
            y: 0,
            width: target.0,
            height: target.1,
        },
        Scaling::Stretch => Viewport::default(),
        Scaling::Fit => fit(ratio, target),
        Scaling::Integer => {
            if src.1 == 0 {
                return fit(ratio, target);
            }
            let by_height = target.1 / src.1;
            // Width of the image at k times the frame height.
            let width_at = |k: u32| ((k * src.1) as f64 * ratio as f64).round() as u32;
            let mut k = by_height;
            while k > 0 && width_at(k) > target.0 {
                k -= 1;
            }
            if k == 0 {
                return fit(ratio, target);
            }
            let (w, h) = (width_at(k), k * src.1);
            Viewport {
                x: (target.0 - w) / 2,
                y: (target.1 - h) / 2,
                width: w,
                height: h,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_drawables_fit_the_texture_limit_in_both_axes() {
        assert_eq!(limit_drawable((11520, 2160), 8192), (8192, 1536));
        assert_eq!(limit_drawable((2160, 11520), 4096), (768, 4096));
        assert_eq!(limit_drawable((100_000, 1), 4096), (4096, 1));
        assert_eq!(limit_drawable((5120, 2880), 8192), (5120, 2880));
    }

    #[test]
    fn native_drawable_tracks_physical_pixels_across_resizes() {
        let size = NativeSize::new(DisplayAspect::Source, Scaling::Fit, (5120, 2880));
        let reader = size.clone();
        assert_eq!(reader.get(), (5120, 2880));
        size.update((3440, 1440));
        assert_eq!(reader.get(), (3440, 1440));
        size.update((0, 1440));
        size.update((3440, 0));
        assert_eq!(reader.get(), (3440, 1440));
        size.update((1233, 857));
        assert_eq!(reader.get(), (1233, 857));
    }

    #[test]
    fn native_drawable_fits_an_explicit_aspect() {
        let size = NativeSize::new(DisplayAspect::Standard, Scaling::Fit, (0, 0));
        assert_eq!(size.get(), (640, 480));
        size.update((5120, 2880));
        assert_eq!(size.get(), (3840, 2880));
        let wide = NativeSize::new(DisplayAspect::Widescreen, Scaling::Fit, (2560, 1600));
        assert_eq!(wide.get(), (2560, 1440));
    }

    #[test]
    fn display_size_tracks_the_compositor_for_fixed_and_native_resolutions() {
        let wide = NativeSize::new(DisplayAspect::Widescreen, Scaling::Fit, (3440, 1440));
        assert_eq!(wide.display_size((640, 480)), (2560, 1440));
        wide.update((1920, 1200));
        assert_eq!(wide.display_size(wide.get()), (1920, 1080));

        let stretched = NativeSize::new(DisplayAspect::Standard, Scaling::Stretch, (3440, 1440));
        assert_eq!(stretched.display_size((640, 480)), (3440, 1440));
        assert_eq!(stretched.get(), (3440, 1440));

        let integer = NativeSize::new(DisplayAspect::Source, Scaling::Integer, (1920, 1080));
        assert_eq!(integer.display_size((640, 480)), (1280, 960));

        let headless = NativeSize::new(DisplayAspect::Source, Scaling::Fit, (0, 0));
        assert_eq!(headless.display_size((5120, 2880)), (5120, 2880));
    }

    #[test]
    fn exact_fit_has_no_bars() {
        assert_eq!(
            fit(4.0 / 3.0, (640, 480)),
            Viewport {
                x: 0,
                y: 0,
                width: 640,
                height: 480
            }
        );
        assert_eq!(
            fit(4.0 / 3.0, (1280, 960)),
            Viewport {
                x: 0,
                y: 0,
                width: 1280,
                height: 960
            }
        );
    }

    #[test]
    fn wide_window_pillarboxes() {
        // 1920x1080 at 4:3: 1440 wide, 240 px bars each side.
        assert_eq!(
            fit(4.0 / 3.0, (1920, 1080)),
            Viewport {
                x: 240,
                y: 0,
                width: 1440,
                height: 1080
            }
        );
    }

    #[test]
    fn tall_window_letterboxes() {
        assert_eq!(
            fit(4.0 / 3.0, (800, 1000)),
            Viewport {
                x: 0,
                y: 200,
                width: 800,
                height: 600
            }
        );
    }

    #[test]
    fn widescreen_in_a_16_10_window() {
        // 2560x1600, 16:9: 2560x1440, 80 px top and bottom.
        assert_eq!(
            fit(16.0 / 9.0, (2560, 1600)),
            Viewport {
                x: 0,
                y: 80,
                width: 2560,
                height: 1440
            }
        );
    }

    #[test]
    fn degenerate_targets_draw_nothing() {
        assert!(fit(4.0 / 3.0, (0, 480)).is_empty());
        assert!(fit(4.0 / 3.0, (640, 0)).is_empty());
        assert!(fit(f32::NAN, (640, 480)).is_empty());
        // A 1-pixel-tall window still gets a 1 px image, never zero.
        let v = fit(4.0 / 3.0, (1000, 1));
        assert_eq!((v.width, v.height), (1, 1));
    }

    #[test]
    fn aspect_ratios() {
        assert_eq!(DisplayAspect::Standard.ratio((720, 480)), 4.0 / 3.0);
        assert_eq!(DisplayAspect::Source.ratio((720, 480)), 1.5);
        assert_eq!(DisplayAspect::Source.ratio((0, 0)), 4.0 / 3.0);
        assert_eq!(DisplayAspect::Custom(-1.0).ratio((640, 480)), 4.0 / 3.0);
        assert_eq!(DisplayAspect::Custom(2.0).ratio((640, 480)), 2.0);
    }

    #[test]
    fn integer_scaling() {
        // 1920x1080: 2x of 480 is 960 high, 1280 wide.
        assert_eq!(
            place(
                (640, 480),
                DisplayAspect::Standard,
                Scaling::Integer,
                (1920, 1080)
            ),
            Viewport {
                x: 320,
                y: 60,
                width: 1280,
                height: 960
            }
        );
        // Too small for 1x: fit instead.
        assert_eq!(
            place(
                (640, 480),
                DisplayAspect::Standard,
                Scaling::Integer,
                (320, 240)
            ),
            Viewport {
                x: 0,
                y: 0,
                width: 320,
                height: 240
            }
        );
        // Width-limited: 3x height fits 1600 high but 3x width (1920) does not fit 1700.
        assert_eq!(
            place(
                (640, 480),
                DisplayAspect::Standard,
                Scaling::Integer,
                (1700, 1600)
            ),
            Viewport {
                x: 210,
                y: 320,
                width: 1280,
                height: 960
            }
        );
    }

    #[test]
    fn stretch_fills() {
        assert_eq!(
            place(
                (640, 480),
                DisplayAspect::Standard,
                Scaling::Stretch,
                (1000, 300)
            ),
            Viewport {
                x: 0,
                y: 0,
                width: 1000,
                height: 300
            }
        );
    }
}
