//! The intro, attract and credits movies (docs/audio/bink-hle.md): one
//! decision for every host (`h2 run`, `h2 serve`, `h2 play`, the apps), made
//! from installed transcodes and the disc's attract movie, logged as one line
//! (`movies: playback (3 transcoded: ...)`, `movies: skip (...)`).
//! Playback replaces the Bink API and plays each movie's sound in the
//! DirectSound HLE's mixer; skip stubs the movie wrapper's start.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const ATTRACT_NAME: &str = "attract_60";
#[cfg(feature = "movie-playback")]
const ATTRACT_DISC_PATH: &str = "/bink/attract_60.bik";
pub const ATTRACT_VFS_PATH: &str = "\\Device\\CdRom0\\bink\\attract_60.bik";

/// Availability before asking the decoder to prepare the movie.
#[derive(Clone)]
pub(crate) struct AttractAsset(Arc<Mutex<AssetStatus>>);

#[derive(Clone, serde::Serialize)]
#[serde(tag = "state", content = "reason", rename_all = "snake_case")]
pub(crate) enum AssetStatus {
    Ready,
    Preparing,
    Unavailable(String),
}

impl AttractAsset {
    fn new(status: AssetStatus) -> Self {
        Self(Arc::new(Mutex::new(status)))
    }

    pub(crate) fn status(&self) -> AssetStatus {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    #[cfg(feature = "movie-playback")]
    fn set(&self, status: AssetStatus) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = status;
    }
}

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
pub fn why_off(
    movies: &Movies,
    lockstep: bool,
    level: Option<&str>,
    headless: bool,
) -> Option<String> {
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
    with_source(source, mixer)
}

#[cfg(feature = "movie-playback")]
fn with_source(
    source: Option<Arc<dyn bink_hle::MovieSource>>,
    mixer: Option<Arc<Mutex<audio::Mixer>>>,
) -> Arc<bink_hle::BinkHle> {
    let hle = bink_hle::BinkHle::new(source);
    if let Some(m) = mixer {
        hle.set_audio_out(Arc::new(bink_hle::MixerAudio::new(m)));
    }
    hle
}

#[cfg(feature = "movie-playback")]
struct PreparingAttract {
    regular: Option<Arc<dyn bink_hle::MovieSource>>,
    attract: bink_hle::TranscodedSource,
    asset: AttractAsset,
}

#[cfg(feature = "movie-playback")]
impl bink_hle::MovieSource for PreparingAttract {
    fn open(&self, name: &str) -> Option<Box<dyn bink_hle::MovieStream>> {
        self.try_open(name).ok()
    }

    fn try_open(
        &self,
        name: &str,
    ) -> Result<Box<dyn bink_hle::MovieStream>, bink_hle::MovieOpenError> {
        if name.eq_ignore_ascii_case(ATTRACT_NAME) {
            return match self.asset.status() {
                AssetStatus::Ready => bink_hle::MovieSource::try_open(&self.attract, name),
                _ => Err(bink_hle::MovieOpenError::Unavailable),
            };
        }
        let source = self
            .regular
            .as_ref()
            .ok_or(bink_hle::MovieOpenError::Unavailable)?;
        source.try_open(name)
    }
}

#[cfg(feature = "movie-playback")]
fn prepare_attract(
    file: vfs::HostFile,
    dir: PathBuf,
    regular: Option<Arc<dyn bink_hle::MovieSource>>,
    decoder: Arc<dyn bink_hle::MovieDecoder>,
) -> (Arc<dyn bink_hle::MovieSource>, AttractAsset) {
    use movies::install::{InstallOptions, InstallStatus, install_region};

    let asset = AttractAsset::new(AssetStatus::Preparing);
    let source = Arc::new(PreparingAttract {
        regular,
        attract: bink_hle::TranscodedSource::new(dir.clone(), decoder).only([ATTRACT_NAME]),
        asset: asset.clone(),
    });
    let job = asset.clone();
    if let Err(error) = background::spawn("attract-transcode", move || {
        let options = InstallOptions::new(dir);
        let status = match install_region(
            &file.path,
            file.offset,
            file.len,
            ATTRACT_DISC_PATH,
            &options,
        ) {
            Ok(report) => match report.status {
                InstallStatus::Complete => AssetStatus::Ready,
                InstallStatus::FfmpegAbsent => {
                    AssetStatus::Unavailable("ffmpeg was not found".into())
                }
                InstallStatus::FfmpegUnusable(reason) => AssetStatus::Unavailable(reason),
            },
            Err(error) => AssetStatus::Unavailable(format!("transcoding the disc movie: {error}")),
        };
        job.set(status);
    }) {
        asset.set(AssetStatus::Unavailable(format!(
            "starting the transcode: {error}"
        )));
    }
    (source, asset)
}

/// Installs the movie HLE and returns the idle timer's per-movie guard.
#[cfg(feature = "movie-playback")]
pub fn install(
    rt: &runtime::Runtime,
    movies: &Movies,
    disc: &Path,
    attract_file: Option<vfs::HostFile>,
    prepare_missing_attract: bool,
    why_off: Option<&str>,
    mixer: Option<Arc<std::sync::Mutex<audio::Mixer>>>,
) -> (
    bink_hle::Mode,
    Arc<bink_hle::BinkHle>,
    crate::attract::Attract,
) {
    let dir = movies.dir();
    let selection = choose(dir.as_deref(), disc, why_off);
    let decoder = bink_hle::platform_decoder();
    let mut source = decoder
        .as_ref()
        .and_then(|d| selection.source(Arc::clone(d)));
    let current = matches!(&selection, bink_hle::Selection::Playback { names, .. }
        if names.iter().any(|name| name == ATTRACT_NAME));
    let asset = match (why_off, attract_file, dir, decoder) {
        (Some(reason), ..) => AttractAsset::new(AssetStatus::Unavailable(reason.into())),
        (None, _, None, _) => AttractAsset::new(AssetStatus::Unavailable(
            if *movies == Movies::Off {
                "--no-movies"
            } else {
                "no movie directory"
            }
            .into(),
        )),
        (None, None, _, _) => {
            AttractAsset::new(AssetStatus::Unavailable("disc movie not found".into()))
        }
        (None, _, _, None) => AttractAsset::new(AssetStatus::Unavailable(
            "no movie decoder on this platform".into(),
        )),
        (None, Some(_), Some(_), Some(_)) if current => AttractAsset::new(AssetStatus::Ready),
        (None, Some(file), Some(dir), Some(decoder))
            if *movies == Movies::Auto && prepare_missing_attract =>
        {
            let (preparing, asset) = prepare_attract(file, dir, source, decoder);
            source = Some(preparing);
            asset
        }
        _ => AttractAsset::new(AssetStatus::Unavailable(format!(
            "{ATTRACT_NAME} is not current in the movie directory"
        ))),
    };
    let hle = with_source(source, mixer);
    let mut mode = bink_hle::Mode::Skip;
    rt.edit_dispatch(|d| mode = bink_hle::register(d, &hle).0);
    if mode == bink_hle::Mode::Playback && !selection.is_playback() {
        eprintln!("movies: playback (preparing {ATTRACT_NAME} from disc)");
    } else {
        eprintln!("movies: {selection}");
    }
    let attract = crate::attract::Attract::new(Arc::clone(&hle), asset);
    (mode, hle, attract)
}

/// Registers the wrapper's skip stub in a build without movie playback.
#[cfg(not(feature = "movie-playback"))]
pub fn install(
    rt: &runtime::Runtime,
    _movies: &Movies,
    _disc: &Path,
    _attract_file: Option<vfs::HostFile>,
    _prepare_missing_attract: bool,
    why_off: Option<&str>,
    _mixer: Option<Arc<std::sync::Mutex<audio::Mixer>>>,
) -> (
    bink_hle::Mode,
    Arc<bink_hle::BinkHle>,
    crate::attract::Attract,
) {
    let hle = bink_hle::BinkHle::skip_only();
    rt.edit_dispatch(|d| {
        bink_hle::register_skip(d, &hle);
    });
    let reason = why_off.unwrap_or("movie playback disabled at build time");
    eprintln!("movies: skip ({reason})");
    let asset = AttractAsset::new(AssetStatus::Unavailable(reason.into()));
    let attract = crate::attract::Attract::new(Arc::clone(&hle), asset);
    (bink_hle::Mode::Skip, hle, attract)
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
        assert_eq!(
            choose(None, &disc, None),
            bink_hle::Selection::Off("--no-movies".into())
        );
        assert_eq!(
            choose(Some(&dir), &disc, Some("lockstep")),
            bink_hle::Selection::Off("lockstep".into())
        );
        assert_eq!(
            choose(Some(&dir), &disc, None),
            bink_hle::Selection::NoneInstalled(dir.clone())
        );
        let rt = runtime::Runtime::new(runtime::Guest::new(), runtime::Dispatch::default());
        let (mode, hle, _) = install(
            &rt,
            &Movies::Dir(dir.clone()),
            &disc,
            None,
            false,
            None,
            None,
        );
        assert_eq!(mode, bink_hle::Mode::Skip);
        assert!(!hle.has_source());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ui_attract_reads_the_menu_timer_at_a_slice() {
        use agent_proto::{Request, code, method};
        use guest_layout::points::{
            SCENARIO, UI_ATTRACT_RESET, UI_ATTRACT_TIMEOUT, UI_CLOCK, UI_LAST_INPUT,
            UI_MUSIC_GATE, UI_MUSIC_STATE,
        };
        use serde_json::{Value, json};

        let mut guest = runtime::Guest::new();
        let scenario = 0x1_0000;
        for addr in [
            scenario,
            SCENARIO.base,
            UI_CLOCK.base,
            UI_ATTRACT_RESET.base,
            UI_MUSIC_GATE.base,
            UI_MUSIC_STATE.base,
            bink_hle::addr::ST_BINK,
        ] {
            guest
                .map_zeroed(addr & !0xfff, 0x1000, runtime::guest::Permissions::RW)
                .unwrap();
        }
        guest.set_u32(SCENARIO.base, scenario).unwrap();
        guest.set_u16(scenario + 0x10, 2).unwrap();
        guest.set_u32(UI_CLOCK.base, 4_000).unwrap();
        guest.set_u32(UI_LAST_INPUT.base, 500).unwrap();
        guest.set_u32(UI_ATTRACT_RESET.base, 1_000).unwrap();
        guest.set_u32(UI_ATTRACT_TIMEOUT.base, 80_000).unwrap();
        guest.set_u8(UI_MUSIC_GATE.base, 1).unwrap();
        let rt = runtime::Runtime::new(guest, runtime::Dispatch::default());
        let agent = agent::Agent::new();
        let mut registry = hooks::Registry::new(hooks::Switches::defaults());
        let attract = crate::attract::Attract::new(
            bink_hle::BinkHle::skip_only(),
            AttractAsset::new(AssetStatus::Preparing),
        );
        attract.install(&mut registry, Some(&agent)).unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let conn = agent.connect(tx);
        let call = |params| -> Value {
            let request = Request::new(1, method::UI_ATTRACT, params);
            agent.submit(conn, &serde_json::to_string(&request).unwrap());
            agent.between_slices(&mut agent::GuestReader::new(|| rt.memory()), 0);
            serde_json::from_str(&rx.try_recv().unwrap()).unwrap()
        };
        let reply = call(json!({}));
        let state = &reply["result"];
        assert_eq!(state["enabled"], true);
        assert_eq!(state["state"], "idle");
        assert_eq!(state["asset"], json!({"state": "preparing"}));
        assert_eq!(state["timer"]["idle_ms"], 3_000);
        assert_eq!(state["timer"]["music_allowed"], true);
        assert_eq!(state["timer"]["attract_timeout_ms"], 80_000);
        assert_eq!(
            call(json!({"force": true}))["error"]["code"],
            code::INVALID_PARAMS
        );
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
        assert_eq!(
            why_off(&dir, true, None, false).as_deref(),
            Some("lockstep")
        );
        assert_eq!(why_off(&Movies::Off, false, None, true), None);
        assert_eq!(Movies::Off.dir(), None);
        assert_eq!(dir.dir(), Some("/m".into()));
    }
}
