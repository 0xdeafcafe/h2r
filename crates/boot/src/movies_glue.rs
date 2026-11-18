//! The intro, attract and credits movies (docs/audio/bink-hle.md): one
//! decision for every host (`h2 run`, `h2 serve`, `h2 play`, the apps), made
//! from what `h2 movies` installed and logged as one line
//! (`movies: playback (3 transcoded: ...)`, `movies: skip (...)`).
//! Playback replaces the Bink API and plays each movie's sound in the
//! DirectSound HLE's mixer; skip stubs the movie wrapper's start.

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Where `h2 movies` installs by default (the app data directory), if the
/// platform has one.
pub fn default_dir() -> Option<PathBuf> {
    movies::install::app_movies_dir().ok()
}

/// Pick the mode for `disc` from `dir` (`None`: off). `why_off` names the
/// reason when the run must skip regardless (a lockstep run, whose
/// schedule must not depend on wall time).
pub fn choose(dir: Option<&Path>, disc: &Path, why_off: Option<&str>) -> bink_hle::Selection {
    if let Some(why) = why_off {
        return bink_hle::Selection::Off(why.into());
    }
    let listed = match dir {
        // A disc that can't be listed is checked by the manifest alone.
        Some(_) => bink_hle::disc_movies(disc).unwrap_or_default(),
        None => Vec::new(),
    };
    bink_hle::select(dir, &listed, bink_hle::platform_decoder().is_some())
}

/// The Bink HLE for `selection`, with movie sound in `mixer` when there is
/// one (`None`: movies play silent).
pub fn hle(
    selection: &bink_hle::Selection,
    mixer: Option<Arc<std::sync::Mutex<audio::Mixer>>>,
) -> Arc<bink_hle::BinkHle> {
    let source = bink_hle::platform_decoder().and_then(|d| selection.source(d));
    let hle = bink_hle::BinkHle::new(source);
    if let Some(m) = mixer {
        hle.set_audio_out(Arc::new(bink_hle::MixerAudio::new(m)));
    }
    hle
}

/// [`choose`], [`hle`] and registration, with the log line.
pub fn install(
    rt: &runtime::Runtime,
    dir: Option<&Path>,
    disc: &Path,
    why_off: Option<&str>,
    mixer: Option<Arc<std::sync::Mutex<audio::Mixer>>>,
) -> (bink_hle::Mode, Arc<bink_hle::BinkHle>) {
    let selection = choose(dir, disc, why_off);
    let hle = hle(&selection, mixer);
    let mut mode = bink_hle::Mode::Skip;
    rt.edit_dispatch(|d| mode = bink_hle::register(d, &hle).0);
    eprintln!("movies: {selection}");
    (mode, hle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_and_none_installed_skip() {
        let dir = std::env::temp_dir().join(format!("boot-movies-glue-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let disc = dir.join("no-such.iso");
        assert_eq!(choose(None, &disc, None), bink_hle::Selection::Off("--no-movies".into()));
        assert_eq!(choose(Some(&dir), &disc, Some("lockstep")), bink_hle::Selection::Off("lockstep".into()));
        assert_eq!(choose(Some(&dir), &disc, None), bink_hle::Selection::NoneInstalled(dir.clone()));
        let rt = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
        let (mode, hle) = install(&rt, Some(&dir), &disc, None, None);
        assert_eq!(mode, bink_hle::Mode::Skip);
        assert!(!hle.has_source());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
