//! The intro, attract and credits movies (docs/audio/bink-hle.md): one
//! decision for every host (`h2 run`, `h2 serve`, `h2 play`, the apps), made
//! from what `h2 movies` installed and logged as one line
//! (`movies: playback (3 transcoded: ...)`, `movies: skip (...)`).
//! Playback replaces the Bink API and plays each movie's sound in the
//! DirectSound HLE's mixer; skip stubs the movie wrapper's start.

use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(feature = "movie-playback")]
/// Where `h2 movies` installs by default (the app data directory), if the
/// platform has one.
pub fn default_dir() -> Option<PathBuf> {
    movies::install::app_movies_dir().ok()
}

#[cfg(not(feature = "movie-playback"))]
pub fn default_dir() -> Option<PathBuf> {
    None
}

/// Which movies a run may play.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Movies {
    /// None (`--no-movies`).
    Off,
    /// Those installed in [`default_dir`], unless the run has no use for
    /// them (see [`why_off`]).
    Auto,
    /// Those installed in this directory, whatever the run (`--movies DIR`).
    Dir(PathBuf),
}

impl Movies {
    /// The directory to look in (`None`: off).
    pub fn dir(&self) -> Option<PathBuf> {
        match self {
            Movies::Off => None,
            Movies::Auto => default_dir(),
            Movies::Dir(d) => Some(d.clone()),
        }
    }
}

/// Why this run skips every movie, if it does: a lockstep run always (its
/// schedule must not depend on wall time); by default (`Movies::Auto`) a
/// direct-to-level boot (`--level`, H2_LEVEL) and a headless run (`h2 run`,
/// `h2 serve`, benchmarks), where nothing wants the intro and its wall-clock
/// pacing only costs time. `--movies DIR` still plays them there.
pub fn why_off(movies: &Movies, lockstep: bool, level: Option<&str>, headless: bool) -> Option<String> {
    if !cfg!(feature = "movie-playback") {
        return Some("movie playback disabled at build time".into());
    }
    if lockstep {
        return Some("lockstep".into());
    }
    if *movies != Movies::Auto {
        return None;
    }
    if let Some(l) = level {
        return Some(format!("level boot {l}; --movies DIR plays them"));
    }
    headless.then(|| "headless run; --movies DIR plays them".into())
}

#[cfg(feature = "movie-playback")]
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

#[cfg(feature = "movie-playback")]
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

#[cfg(feature = "movie-playback")]
/// [`choose`], [`hle`] and registration, with the log line.
pub fn install(
    rt: &runtime::Runtime,
    movies: &Movies,
    disc: &Path,
    why_off: Option<&str>,
    mixer: Option<Arc<std::sync::Mutex<audio::Mixer>>>,
) -> (bink_hle::Mode, Arc<bink_hle::BinkHle>) {
    let selection = choose(movies.dir().as_deref(), disc, why_off);
    let hle = hle(&selection, mixer);
    let mut mode = bink_hle::Mode::Skip;
    rt.edit_dispatch(|d| mode = bink_hle::register(d, &hle).0);
    eprintln!("movies: {selection}");
    (mode, hle)
}

/// Registers the wrapper's skip stub in a build without movie playback.
#[cfg(not(feature = "movie-playback"))]
pub fn install(
    rt: &runtime::Runtime,
    _movies: &Movies,
    _disc: &Path,
    why_off: Option<&str>,
    _mixer: Option<Arc<std::sync::Mutex<audio::Mixer>>>,
) -> (bink_hle::Mode, Arc<bink_hle::BinkHle>) {
    let hle = bink_hle::BinkHle::skip_only();
    rt.edit_dispatch(|d| {
        bink_hle::register_skip(d, &hle);
    });
    eprintln!("movies: skip ({})", why_off.unwrap_or("movie playback disabled at build time"));
    (bink_hle::Mode::Skip, hle)
}

#[cfg(all(test, feature = "movie-playback"))]
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
        let (mode, hle) = install(&rt, &Movies::Dir(dir.clone()), &disc, None, None);
        assert_eq!(mode, bink_hle::Mode::Skip);
        assert!(!hle.has_source());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn level_boots_and_headless_runs_skip_unless_a_dir_is_named() {
        let dir = Movies::Dir("/m".into());
        // A windowed menu boot plays what is installed.
        assert_eq!(why_off(&Movies::Auto, false, None, false), None);
        // A level boot or a headless run skips by default, with the reason.
        let level = why_off(&Movies::Auto, false, Some("01a_tutorial"), false).unwrap();
        assert!(level.starts_with("level boot 01a_tutorial"), "{level}");
        let headless = why_off(&Movies::Auto, false, None, true).unwrap();
        assert!(headless.starts_with("headless run"), "{headless}");
        // --movies DIR plays them there.
        assert_eq!(why_off(&dir, false, Some("01a_tutorial"), true), None);
        // Lockstep always skips.
        assert_eq!(why_off(&dir, true, None, false).as_deref(), Some("lockstep"));
        assert_eq!(why_off(&Movies::Off, false, None, true), None);
        assert_eq!(Movies::Off.dir(), None);
        assert_eq!(dir.dir(), Some("/m".into()));
    }
}
