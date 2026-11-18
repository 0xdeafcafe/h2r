//! Which mode a run gets: playback when transcoded movies for this disc are
//! installed and a decoder exists, skip otherwise. Hosts (`boot`) call
//! [`select`] once at startup and log its [`Display`](std::fmt::Display)
//! (`movies: playback (3 transcoded)`, `movies: skip (none installed ...)`).
use crate::{MovieDecoder, MovieSource, TranscodedSource};
use movies::status::{Availability, DiscMovie, availability};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The outcome of [`select`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Turned off (`--no-movies`, or a run that must stay deterministic).
    Off(String),
    /// No manifest in the directory.
    NoneInstalled(PathBuf),
    /// A manifest, but nothing current for this disc.
    Stale(String),
    /// Movies are installed but this platform has no decoder for them.
    NoDecoder,
    /// Play these movies (engine names) from `dir`.
    Playback { dir: PathBuf, names: Vec<String> },
}

/// Decide from what is installed in `dir` (`None`: off) for the disc's
/// movies. `decoder` is the platform's ([`platform_decoder`]).
pub fn select(dir: Option<&Path>, disc: &[DiscMovie], decoder: bool) -> Selection {
    let Some(dir) = dir else {
        return Selection::Off("--no-movies".into());
    };
    match availability(dir, disc) {
        Availability::NoneInstalled => Selection::NoneInstalled(dir.to_path_buf()),
        Availability::Stale(why) => Selection::Stale(why),
        Availability::Current(_) if !decoder => Selection::NoDecoder,
        Availability::Current(names) => Selection::Playback {
            dir: dir.to_path_buf(),
            names,
        },
    }
}

impl Selection {
    pub fn is_playback(&self) -> bool {
        matches!(self, Selection::Playback { .. })
    }
    /// The source for [`crate::BinkHle::new`]: `Some` only for playback.
    /// It opens only the movies found current.
    pub fn source(&self, decoder: Arc<dyn MovieDecoder>) -> Option<Arc<dyn MovieSource>> {
        match self {
            Selection::Playback { dir, names } => Some(Arc::new(
                TranscodedSource::new(dir.clone(), decoder).only(names),
            )),
            _ => None,
        }
    }
}

impl fmt::Display for Selection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Selection::Off(why) => write!(f, "skip (off: {why})"),
            Selection::NoneInstalled(dir) => write!(
                f,
                "skip (none installed in {}; `h2 movies <disc.iso>` installs them)",
                dir.display()
            ),
            Selection::Stale(why) => write!(f, "skip (stale: {why})"),
            Selection::NoDecoder => write!(f, "skip (no movie decoder on this platform)"),
            Selection::Playback { names, .. } => {
                write!(
                    f,
                    "playback ({} transcoded: {})",
                    names.len(),
                    names.join(", ")
                )
            }
        }
    }
}

/// The platform's decoder for transcoded movies: AVFoundation on macOS,
/// none elsewhere yet.
pub fn platform_decoder() -> Option<Arc<dyn MovieDecoder>> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(crate::AvFoundationDecoder))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}
