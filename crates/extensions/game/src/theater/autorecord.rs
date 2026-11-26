//! Films of the games a player plays, recorded by default
//! (docs/game/film-format.md, "Recording every game").
//!
//! `h2 play` records each session into the film library when no `--film`
//! or `H2_FILM_OUT` names a destination. The session's tape runs from boot
//! to exit and is never listed; each game it plays is cut from it into a
//! film of its own ([`cut`], "One film per game"), which plays from its
//! game's start (`film_play`). The recording keeps the
//! session's player features on (Live session links, browser
//! advertisements, Firefight's director, the rules start gate, editor
//! reloads, Forge), and its header says so, so the replay installs the same
//! plan. Something that changes the guest from outside the tape (a session
//! link joined, an editor write) is noted and the recording carries on.
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

/// An unreadable automatic film untouched for this long belongs to a
/// session that stopped without sealing it, and the prune removes it. A
/// running recording writes a chunk every few seconds.
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

/// `auto-YYYYMMDD-HHMMSS-mmm.h2film` in UTC: names sort oldest to newest,
/// and the library sorts Recent by the header's start time.
pub fn file_name(at: SystemTime) -> String {
    let ms = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let ms = i64::try_from(ms).unwrap_or(i64::MAX);
    let stamp = chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .format("%Y%m%d-%H%M%S-%3f");
    format!("{PREFIX}{stamp}.h2film")
}

/// A new file in `dir` for a recording started at `at`. Two sessions
/// started in the same millisecond take numbered names.
pub fn destination(dir: &Path, at: SystemTime) -> PathBuf {
    let name = file_name(at);
    let first = dir.join(&name);
    if !first.exists() {
        return first;
    }
    let stem = name.trim_end_matches(".h2film");
    (1u32..)
        .map(|n| dir.join(format!("{stem}-{n}.h2film")))
        .find(|p| !p.exists())
        .expect("a free film name")
}

/// What a feature calls just before it changes the guest from outside the
/// tape. The session's default recording carries on and says so
/// ([`super::Session::outside_change`]); `drops_game` also drops the game
/// in play, which the tape can no longer replay. `None` for any other film
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

/// Seal the game that ended at `tick` as a film of its own, cut from the
/// running tape `rec` (docs/game/film-format.md, "One film per game"). The
/// cut is a copy of the tape so far, closed with its own `HEAD`, the game's
/// description and scores, and an end mark here; it opens at the game's
/// start. Before it is written the library keeps the `KEEP - 1` newest
/// default films. Returns the cut's path.
pub(super) fn cut(
    rec: &mut super::Recording,
    tape: &film::Header,
    slices: u64,
    tick: u64,
    calls: u64,
) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    rec.meta.finish_map()?;
    let mut summary = rec.meta.summary(tick);
    rec.unified.summary_medals(&mut summary.players);
    summary.dropped_samples = rec.unified.drop_counts();
    let header = cut_header(tape, &summary, tick, rec.game_started_ms);
    let dir = rec.file.parent().unwrap_or(Path::new(".")).to_owned();
    let now = SystemTime::now();
    if let Err(error) = prune(&dir, KEEP, now) {
        eprintln!("theater: could not make room in the film library: {error:#}");
    }
    let path = destination(&dir, now);
    let out = rec.out.as_mut().context("the tape is closed")?;
    // A running tape's file holds exactly what it wrote so far; on APFS
    // the copy is a clone.
    fs::copy(&rec.file, &path).with_context(|| path.display().to_string())?;
    // The cut's film file: this game's film from the tape's own, as the
    // container is cut from the tape's.
    let film_path = film::file::beside(&path);
    let sealed = (|| -> anyhow::Result<()> {
        let file = fs::File::options().write(true).open(&path)?;
        let mut writer = out.fork(file, &header)?;
        let actions = match &rec.film {
            Some(tape) => {
                let seed = film::file::Seed {
                    header: header.clone(),
                    descriptor: rec.meta.descriptor().clone(),
                };
                let file = fs::File::options()
                    .write(true)
                    .create_new(true)
                    .open(&film_path)
                    .with_context(|| film_path.display().to_string())?;
                let cut = tape
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .fork(file, &seed)?;
                let cut = std::sync::Arc::new(std::sync::Mutex::new(cut));
                writer.set_mirror(Some(film::file::machine_mirror(std::sync::Arc::clone(&cut))));
                Some(cut)
            }
            None => None,
        };
        let mut track = rec.track.clone();
        writer.chunk(film::HEAD, &header)?;
        track.finish(&mut writer, rec.meta.descriptor(), &summary)?;
        writer.chunk(film::MARK, &film::tape::Mark::End { slices, tick, calls })?;
        writer.finish()?.sync_all()?;
        if let Some(cut) = actions {
            let cut = std::sync::Arc::try_unwrap(cut)
                .map_err(|_| anyhow::anyhow!("the cut's film file is still shared"))?
                .into_inner()
                .unwrap_or_else(|e| e.into_inner());
            cut.finish()?.sync_all()?;
        }
        Ok(())
    })();
    if let Err(error) = sealed {
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&film_path);
        return Err(error);
    }
    Ok(path)
}

/// The final header of a game cut from `tape`: an ID of its own, the
/// tape's ID, the game's start, length and scoreboard.
pub(super) fn cut_header(
    tape: &film::Header,
    summary: &film::blam::Summary,
    tick: u64,
    game_started_ms: Option<u64>,
) -> film::Header {
    let mut header = tape.clone();
    header.meta.id = live_proto::sign::random_id();
    header.tape_id = Some(tape.meta.id.clone());
    header.meta.started_ms = game_started_ms.unwrap_or(tape.meta.started_ms);
    header.meta.length_ticks = tick;
    if !summary.level.is_empty() {
        header.meta.map.clone_from(&summary.level);
        header.meta.variant.clone_from(&summary.variant_name);
        header.meta.players = summary
            .players
            .iter()
            .map(|player| film::Player {
                name: player.name.clone(),
                team: player.team,
                score: player.score,
            })
            .collect();
    }
    header
}

/// Where Theater opens a film with no seek or clip asked for: a game cut
/// from a session's tape opens at its game's start, any other film at boot.
pub fn opening_tick(header: &film::Header, front: Option<&film::range::Front>) -> u64 {
    if header.tape_id.is_none() {
        return 0;
    }
    front
        .and_then(|f| f.match_start_tick)
        .filter(|&tick| tick <= header.meta.length_ticks)
        .unwrap_or(0)
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

/// Delete one automatic film with its stats, film file and pin.
pub fn remove(path: &Path) -> anyhow::Result<()> {
    for extra in ["h2stats", "h2f", "h2film.saved"] {
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
/// unsaved automatic films and every saved one, and remove automatic films
/// a stopped session left unsealed. Films named by `--film` and downloaded
/// archives are never touched. Returns the removed paths.
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
        if film::LibraryEntry::read(&path).is_ok() {
            sealed.push(path);
            continue;
        }
        let idle = entry
            .metadata()?
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= ABANDONED_AFTER);
        if idle {
            remove(&path)?;
            removed.push(path);
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
