//! Films of the games a player plays, recorded by default
//! (docs/game/film-format.md, "Recording every game").
//!
//! `h2 play` records each session's games into the film library when no
//! `--film` or `H2_FILM_OUT` names a destination. Each game is a film of
//! its own ("One film per game"): `<id>.h2film`, the film file, and
//! `<id>.h2stats`, its stats container, opened at the game's activation
//! with tick 1 its first update and sealed at its end. Nothing is recorded
//! outside a game. A film plays from its game start (`film_play`). The
//! recording keeps the session's player features on (Live session links,
//! browser advertisements, Firefight's director, the rules start gate,
//! editor reloads, Forge), and its header says so, so the replay installs
//! the same plan. Something that changes the guest from outside the film
//! (a session link joined, an editor write) is noted and the recording
//! carries on.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// `H2_FEATURES=-film-autorecord` records nothing unless asked.
pub const FEATURE: &str = "film-autorecord";

/// Automatic films kept in the library, newest first. The Recent tab
/// shows 25 films, so older automatic films would never be seen.
pub const KEEP: usize = 25;

/// An unsealed automatic film untouched for this long belongs to a session
/// that stopped without sealing it: the prune seals it at the ticks it
/// holds, or removes it when it does not read. A running recording writes
/// a checkpoint every few seconds.
pub const ABANDONED_AFTER: Duration = Duration::from_secs(60 * 60);

const PREFIX: &str = "auto-";

/// What the host asked for before boot (`Host::film_auto`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Request {
    /// Not a player session (`h2 run`, `h2 serve`, hosts, sims, Theater).
    #[default]
    Off,
    /// A player session with no explicit destination.
    On,
    /// A player session that must not record; the reason is logged once.
    Skip(&'static str),
}

/// The boot facts that decide whether this session records.
#[derive(Clone, Copy, Debug)]
pub struct Inputs {
    pub request: Request,
    /// `H2_FEATURES` keeps `film-autorecord` on.
    pub feature_on: bool,
    /// `--film` or `H2_FILM_OUT` named a destination.
    pub explicit: bool,
    pub replaying: bool,
    pub host_light: bool,
    pub sim: bool,
    /// `H2_LOCKSTEP`, `H2_INPUT_RECORD` or `H2_INPUT_REPLAY` own the pad tap.
    pub lockstep: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Record,
    /// Nothing to say: not a player session, an explicit film, or the switch.
    Quiet,
    /// A player session that goes unrecorded, with the line to log.
    Skip(&'static str),
}

pub fn decide(i: &Inputs) -> Decision {
    if i.explicit || i.replaying || i.host_light || i.sim || !i.feature_on {
        return Decision::Quiet;
    }
    match i.request {
        Request::Off => Decision::Quiet,
        Request::Skip(why) => Decision::Skip(why),
        Request::On if i.lockstep => Decision::Skip(
            "lockstep or input recording owns the controller tap (H2_LOCKSTEP, H2_INPUT_RECORD, H2_INPUT_REPLAY)",
        ),
        Request::On => Decision::Record,
    }
}

/// `auto-YYYYMMDD-HHMMSS-mmm.h2film` in UTC, the film file of a game that
/// started then: names sort oldest to newest, and the library sorts Recent
/// by the header's start time.
pub fn file_name(at: SystemTime) -> String {
    let ms = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let ms = i64::try_from(ms).unwrap_or(i64::MAX);
    let stamp = chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%Y%m%d-%H%M%S-%3f");
    format!("{PREFIX}{stamp}.h2film")
}

/// A new film file in `dir` for a game started at `at`, with no stats
/// container beside it either. Two games started in the same millisecond
/// take numbered names.
pub fn destination(dir: &Path, at: SystemTime) -> PathBuf {
    let free = |p: &PathBuf| !p.exists() && !film::file::stats_of(p).exists();
    let name = file_name(at);
    let first = dir.join(&name);
    if free(&first) {
        return first;
    }
    let stem = name.trim_end_matches(".h2film");
    (1u32..)
        .map(|n| dir.join(format!("{stem}-{n}.h2film")))
        .find(free)
        .expect("a free film name")
}

/// The header of a game's film, from the session's: an ID of its own and
/// the game's start, with no map, players or length until it is sealed.
pub(super) fn game_header(session: &film::Header) -> film::Header {
    let mut header = session.clone();
    header.meta.id = live_proto::sign::random_id();
    header.meta.started_ms = now_ms();
    header.meta.map.clear();
    header.meta.variant.clear();
    header.meta.players.clear();
    header.meta.length_ticks = 0;
    header
}

/// What a feature calls just before it changes the guest from outside the
/// film. The session's default recording carries on and says so
/// ([`super::Session::outside_change`]); `drops_game` also drops the game
/// in play, which its film can no longer replay. `None` for any other film
/// and for no film.
pub fn stopper(
    film: Option<&Arc<super::Session>>,
    why: &'static str,
    drops_game: bool,
) -> Option<Arc<dyn Fn() + Send + Sync>> {
    let film = film.filter(|f| !f.replaying && f.player_features())?.clone();
    Some(Arc::new(move || film.outside_change(why, drops_game)))
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

pub fn is_auto(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "h2film")
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(PREFIX))
}

fn saved(path: &Path) -> bool {
    path.with_extension("h2film.saved").is_file()
}

/// Delete one automatic film: its film file, its stats container and its
/// pin.
pub fn remove(path: &Path) -> anyhow::Result<()> {
    for extra in ["h2stats", "h2film.saved"] {
        match fs::remove_file(path.with_extension(extra)) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            r => r?,
        }
    }
    match fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => Ok(r?),
    }
}

/// Make room for one more automatic film: keep the `keep - 1` newest
/// unsaved automatic films and every saved one. An unsealed automatic film
/// untouched for [`ABANDONED_AFTER`] (its session was killed) is sealed at
/// the ticks it holds up to its last checkpoint ([`film::file::recover`])
/// and kept, or removed when it does not read; a newer unsealed film is a
/// recording in progress and is left alone. Films named by `--film` and
/// downloaded archives are never touched. Returns the removed paths.
pub fn prune(dir: &Path, keep: usize, now: SystemTime) -> anyhow::Result<Vec<PathBuf>> {
    let mut sealed = Vec::new();
    let mut removed = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(removed),
        Err(e) => return Err(e.into()),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type()?.is_file() || !is_auto(&path) || saved(&path) {
            continue;
        }
        let film = fs::File::open(&path)
            .map_err(anyhow::Error::from)
            .and_then(film::file::FilmReader::open);
        if film.as_ref().is_ok_and(|reader| reader.sealed) {
            sealed.push(path);
            continue;
        }
        let idle = entry
            .metadata()?
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= ABANDONED_AFTER);
        if !idle {
            continue;
        }
        // A killed recording's film: sealed at what it holds when that is a
        // game start and at least one tick, else removed.
        let ticks = film
            .ok()
            .filter(|reader| !reader.game_starts.is_empty())
            .map(|reader| reader.length_ticks)
            .filter(|&ticks| ticks > 0);
        match ticks.map(|ticks| film::file::recover(&path, ticks)) {
            Some(Ok(())) => sealed.push(path),
            _ => {
                remove(&path)?;
                removed.push(path);
            }
        }
    }
    // Names carry the start time, so the newest sort last.
    sealed.sort();
    let excess = sealed.len().saturating_sub(keep.saturating_sub(1));
    for path in sealed.into_iter().take(excess) {
        remove(&path)?;
        removed.push(path);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests;
