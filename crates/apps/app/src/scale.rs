//! Where the guest frame lands in the window: aspect-correct fitting.
//!
//! The Xbox drives a TV, so a 640x480 back buffer is shown at 4:3 and a
//! 720x480 one is anamorphic (also 4:3, or 16:9 when the dashboard's
//! widescreen setting is on). The display aspect is therefore a setting, not
//! something derived from the back-buffer size.

use settings::NativeTarget;
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

/// Reads whether the live resolution setting is Native.
#[derive(Clone)]
pub struct NativeFit(Arc<dyn Fn() -> bool + Send + Sync>);

impl NativeFit {
    pub fn new(is_native: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Self(Arc::new(is_native))
    }
}

impl std::fmt::Debug for NativeFit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeFit").finish_non_exhaustive()
    }
}

/// Drawable size and fullscreen state shared by the shell and render thread.
/// Publish them together so a reader cannot observe half of a transition.
#[derive(Clone)]
pub(crate) struct NativeSize {
    aspect: DisplayAspect,
    scaling: Scaling,
    pixels: Arc<AtomicU64>,
    native_fit: Option<NativeFit>,
}

impl NativeSize {
    pub(crate) fn new(aspect: DisplayAspect, scaling: Scaling, drawable: (u32, u32)) -> Self {
        let size = Self {
            aspect,
            scaling,
            pixels: Arc::new(AtomicU64::new(0)),
            native_fit: None,
        };
        size.update(drawable, false);
        size
    }

    pub(crate) fn with_native_fit(mut self, native_fit: Option<NativeFit>) -> Self {
        self.native_fit = native_fit;
        self
    }

    // Surface dimensions are bounded by MAX_FRAME_DIMENSION, leaving the
    // high bit free to publish the window mode with the pixels.
    const FULLSCREEN: u64 = 1 << 63;

    pub(crate) fn update(&self, (w, h): (u32, u32), fullscreen: bool) {
        if w != 0 && h != 0 {
            let mode = if fullscreen { Self::FULLSCREEN } else { 0 };
            self.pixels.store(mode | (u64::from(w) << 32) | u64::from(h), Ordering::Relaxed);
        }
    }

    fn surface(&self) -> Option<((u32, u32), bool)> {
        let sample = self.pixels.load(Ordering::Relaxed);
        let pixels = sample & !Self::FULLSCREEN;
        (pixels != 0).then_some((((pixels >> 32) as u32, pixels as u32), sample != pixels))
    }

    pub(crate) fn get(&self) -> NativeTarget {
        let Some((drawable, fullscreen)) = self.surface() else {
            return NativeTarget::windowed((640, 480));
        };
        let viewport = if self.aspect == DisplayAspect::Source || self.scaling == Scaling::Stretch {
            drawable
        } else {
            let fitted = fit(self.aspect.ratio(drawable), drawable);
            (fitted.width, fitted.height)
        };
        NativeTarget { drawable, viewport, fullscreen }
    }

    pub(crate) fn viewport(&self, src: (u32, u32), drawable: (u32, u32), loading: bool) -> Viewport {
        // Native Fit takes its aspect from the drawable. The cached frame
        // can still have the aspect of the last accepted raster resize.
        let scaling = if !loading
            && self.aspect == DisplayAspect::Source
            && self.scaling == Scaling::Fit
            && self.native_fit.as_ref().is_some_and(|fit| (fit.0)())
        {
            Scaling::Stretch
        } else {
            self.scaling
        };
        place(src, self.aspect, scaling, drawable)
    }

    /// Compositor's physical destination for a source of this size.
    pub(crate) fn display_size(&self, src: (u32, u32)) -> (u32, u32) {
        let Some((drawable, _)) = self.surface() else {
            return src;
        };
        let viewport = self.viewport(src, drawable, false);
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

/// Fit the opening play window in logical points using the monitor's shape.
pub(crate) fn initial_monitor_window(bounds: (u32, u32), monitor: (u32, u32)) -> (u32, u32) {
    if monitor.0 == 0 || monitor.1 == 0 {
        return bounds;
    }
    let available = (bounds.0.min(monitor.0), bounds.1.min(monitor.1));
    let window = fit(DisplayAspect::Source.ratio(monitor), available);
    (window.width, window.height)
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
    fn play_window_opens_at_the_monitor_aspect_in_logical_points() {
        let bounds = (1280, 960);
        assert_eq!(initial_monitor_window(bounds, (2560, 1440)), (1280, 720));
        assert_eq!(initial_monitor_window(bounds, (1200, 1920)), (600, 960));
        assert_eq!(initial_monitor_window(bounds, (800, 600)), (800, 600));
        assert_eq!(initial_monitor_window(bounds, (0, 0)), bounds);
    }

    #[test]
    fn oversized_drawables_fit_the_texture_limit_in_both_axes() {
        assert_eq!(limit_drawable((11520, 2160), 8192), (8192, 1536));
        assert_eq!(limit_drawable((2160, 11520), 4096), (768, 4096));
        assert_eq!(limit_drawable((100_000, 1), 4096), (4096, 1));
        assert_eq!(limit_drawable((5120, 2880), 8192), (5120, 2880));
    }

    #[cfg(not(any(target_os = "tvos", target_os = "ios")))]
    #[test]
    fn retina_native_target_tracks_physical_pixels_and_window_mode() {
        use settings::Resolution;
        use winit::dpi::LogicalSize;

        let backing = |(w, h)| {
            let pixels = LogicalSize::new(w, h).to_physical::<u32>(2.0);
            (pixels.width, pixels.height)
        };
        let opening = initial_monitor_window((1280, 960), (2560, 1440));
        let size = NativeSize::new(DisplayAspect::Source, Scaling::Fit, backing(opening))
            .with_native_fit(Some(NativeFit::new(|| true)));
        let reader = size.clone();
        assert_eq!(reader.get(), NativeTarget::windowed((2560, 1440)));
        for (points, fullscreen, raster) in [
            ((1392, 864), false, (2784, 1728)), // user resize
            ((2560, 1400), false, (5120, 2800)), // maximize
            ((2560, 1440), true, (3840, 2160)), // fullscreen
            ((2560, 1440), false, (5120, 2880)), // mode-only change
            ((1392, 864), false, (2784, 1728)), // restore
        ] {
            let drawable = backing(points);
            size.update(drawable, fullscreen);
            let sample = reader.get();
            assert_eq!(
                sample,
                NativeTarget { drawable, viewport: drawable, fullscreen },
            );
            assert_eq!(Resolution::Native.size(sample), raster);
            assert_eq!(reader.display_size(raster), drawable);
        }
        size.update((0, 1728), false);
        assert_eq!(reader.get(), NativeTarget::windowed((2784, 1728)));
    }

    #[test]
    fn native_drawable_fits_an_explicit_aspect() {
        let size = NativeSize::new(DisplayAspect::Standard, Scaling::Fit, (0, 0));
        assert_eq!(size.get(), NativeTarget::windowed((640, 480)));
        size.update((5120, 2880), true);
        assert_eq!(size.get(), NativeTarget {
            drawable: (5120, 2880), viewport: (3840, 2880), fullscreen: true,
        });
        let wide = NativeSize::new(DisplayAspect::Widescreen, Scaling::Fit, (2560, 1600));
        assert_eq!(wide.get().viewport, (2560, 1440));
    }

    #[test]
    fn display_size_tracks_the_compositor_for_fixed_and_native_resolutions() {
        let wide = NativeSize::new(DisplayAspect::Widescreen, Scaling::Fit, (3440, 1440));
        assert_eq!(wide.display_size((640, 480)), (2560, 1440));
        wide.update((1920, 1200), false);
        assert_eq!(wide.display_size(wide.get().viewport), (1920, 1080));

        let stretched = NativeSize::new(DisplayAspect::Standard, Scaling::Stretch, (3440, 1440));
        assert_eq!(stretched.display_size((640, 480)), (3440, 1440));
        assert_eq!(stretched.get().viewport, (3440, 1440));

        let integer = NativeSize::new(DisplayAspect::Source, Scaling::Integer, (1920, 1080));
        assert_eq!(integer.display_size((640, 480)), (1280, 960));

        let headless = NativeSize::new(DisplayAspect::Source, Scaling::Fit, (0, 0));
        assert_eq!(headless.display_size((5120, 2880)), (5120, 2880));
    }

    #[test]
    fn native_fit_reuses_the_same_frame_at_each_new_drawable_aspect() {
        let cached = (1280, 960);
        let size = NativeSize::new(DisplayAspect::Source, Scaling::Fit, cached)
            .with_native_fit(Some(NativeFit::new(|| true)));
        for (target, rebuilt) in [
            ((1600, 900), (1600, 900)),
            ((1920, 1080), (1920, 1080)),
            ((900, 1200), (900, 1200)),
            ((5120, 2880), (3840, 2160)),
        ] {
            size.update(target, false);
            let viewport = Viewport {
                x: 0,
                y: 0,
                width: target.0,
                height: target.1,
            };
            assert_eq!(size.viewport(cached, target, false), viewport);
            assert_eq!(size.viewport(rebuilt, target, false), viewport);
            assert_eq!(size.display_size(cached), target);
        }
    }

    #[test]
    fn native_fit_preserves_loading_fixed_aspect_and_integer_placement() {
        use std::sync::atomic::AtomicBool;

        let native = Arc::new(AtomicBool::new(true));
        let mode = Arc::clone(&native);
        let control = Some(NativeFit::new(move || mode.load(Ordering::Relaxed)));
        let src = (640, 480);
        let target = (1920, 1080);
        let size = NativeSize::new(DisplayAspect::Source, Scaling::Fit, target)
            .with_native_fit(control.clone());
        let fitted = fit(4.0 / 3.0, target);
        assert_eq!(size.viewport(src, target, true), fitted);
        assert_eq!(size.display_size(src), target);

        let forced = NativeSize::new(DisplayAspect::Standard, Scaling::Fit, target)
            .with_native_fit(control.clone());
        assert_eq!(forced.viewport(src, target, false), fitted);
        let integer = NativeSize::new(DisplayAspect::Source, Scaling::Integer, target)
            .with_native_fit(control);
        assert_eq!(integer.display_size(src), (1280, 960));

        native.store(false, Ordering::Relaxed);
        assert_eq!(size.viewport(src, target, false), fitted);
        assert_eq!(size.display_size(src), (1440, 1080));
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
