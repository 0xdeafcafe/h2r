use crate::run_opts::{NativeSize, NativeTarget};
use settings::Resolution;
use std::sync::Arc;

/// The game and native Theater share the same raster selection policy.
pub fn output_size(
    store: Option<&settings::Store>,
    native: NativeTarget,
) -> anyhow::Result<NativeSize> {
    let choice: Box<dyn Fn() -> Resolution + Send + Sync> = if let Some(store) = store {
        let handle = store.handle::<Resolution>(crate::setting_defs::RESOLUTION.key);
        Box::new(move || handle.get())
    } else {
        let fixed = crate::setting_defs::resolution_from_env().map_err(anyhow::Error::msg)?;
        Box::new(move || fixed)
    };
    // Use the latest published target at boot and at each frame boundary.
    Ok(Arc::new(move || choice().size(native())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setting_defs::{ALL, RESOLUTION};
    use settings::NativeTarget as Target;
    use std::sync::Mutex;

    #[test]
    fn native_tracks_each_retina_size_and_mode_on_the_first_sample() {
        let opening = Target::windowed((2560, 1440));
        let target = Arc::new(Mutex::new(opening));
        let read = Arc::clone(&target);
        let store = settings::Store::defaults(ALL);
        let output = output_size(Some(&store), Arc::new(move || *read.lock().unwrap())).unwrap();

        assert_eq!(output(), (2560, 1440));
        for (next, raster) in [
            (Target::windowed((2784, 1728)), (2784, 1728)),
            (Target::windowed((5120, 2800)), (5120, 2800)),
            (Target { fullscreen: true, ..Target::windowed((5120, 2880)) }, (3840, 2160)),
            (Target::windowed((2784, 1728)), (2784, 1728)),
        ] {
            *target.lock().unwrap() = next;
            assert_eq!(output(), raster);
        }

        store.set(RESOLUTION.key, Resolution::Fixed(5120, 2880).value()).unwrap();
        *target.lock().unwrap() = opening;
        assert_eq!(output(), (5120, 2880));
        store.set(RESOLUTION.key, Resolution::Native.value()).unwrap();
        assert_eq!(output(), (2560, 1440));
    }
}
