use crate::run_opts::NativeSize;
use settings::Resolution;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Keep presenting the existing raster during a window drag. The compositor
/// continues to place that raster into the current physical drawable.
const RESIZE_QUIET: Duration = Duration::from_millis(150);

struct RasterSize {
    choice: Resolution,
    native: (u32, u32),
    raster: (u32, u32),
    pending: Option<((u32, u32), Instant)>,
}

impl RasterSize {
    fn new(choice: Resolution, native: (u32, u32)) -> Self {
        Self {
            choice,
            native,
            raster: choice.size(native),
            pending: None,
        }
    }

    fn sample(&mut self, choice: Resolution, native: (u32, u32), now: Instant) -> (u32, u32) {
        if choice != self.choice {
            // A menu selection takes effect on the next frame, including a
            // switch back to Native while a resize is in progress.
            *self = Self::new(choice, native);
        } else if choice == Resolution::Native {
            match self.pending {
                _ if self.native == native => self.pending = None,
                Some((size, since)) if size == native => {
                    if now.duration_since(since) >= RESIZE_QUIET {
                        *self = Self::new(choice, native);
                    }
                }
                _ => self.pending = Some((native, now)),
            }
        }
        self.raster
    }
}

/// The game and native Theater share the same raster selection policy.
pub fn output_size(
    store: Option<&settings::Store>,
    native: NativeSize,
) -> anyhow::Result<NativeSize> {
    let choice: Box<dyn Fn() -> Resolution + Send + Sync> = if let Some(store) = store {
        let handle = store.handle::<Resolution>(crate::setting_defs::RESOLUTION.key);
        Box::new(move || handle.get())
    } else {
        let fixed = crate::setting_defs::resolution_from_env().map_err(anyhow::Error::msg)?;
        Box::new(move || fixed)
    };
    let size = Mutex::new(RasterSize::new(choice(), native()));
    Ok(Arc::new(move || {
        size.lock()
            .unwrap_or_else(|e| e.into_inner())
            .sample(choice(), native(), Instant::now())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_resize_waits_for_the_last_stable_drawable() {
        let start = Instant::now();
        let at = |millis| start + Duration::from_millis(millis);
        let old = (1280, 960);
        let mut s = RasterSize::new(Resolution::Native, old);
        for (t, native) in [(0, (1600, 900)), (100, (1920, 1080)), (200, (5120, 2880))] {
            assert_eq!(s.sample(Resolution::Native, native, at(t)), old);
        }
        assert_eq!(s.sample(Resolution::Native, (5120, 2880), at(349)), old);
        assert_eq!(
            s.sample(Resolution::Native, (5120, 2880), at(350)),
            (3840, 2160)
        );
    }

    #[test]
    fn settings_apply_immediately_and_cancel_pending_resizes() {
        let start = Instant::now();
        let native = (3440, 1440);
        let drag = (1560, 950);
        let mut s = RasterSize::new(Resolution::Native, native);
        assert_eq!(s.sample(Resolution::Native, drag, start), native);
        assert_eq!(s.sample(Resolution::Native, native, start), native);
        assert!(s.pending.is_none());
        assert_eq!(s.sample(Resolution::Native, drag, start), native);
        let fixed = Resolution::Fixed(5120, 2880);
        assert_eq!(s.sample(fixed, drag, start), (5120, 2880));
        assert_eq!(s.sample(fixed, native, start), (5120, 2880));
        assert_eq!(s.sample(Resolution::Native, drag, start), drag);
        assert!(s.pending.is_none());
    }
}
