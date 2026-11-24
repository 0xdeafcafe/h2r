//! The lobby's map variant and the build before Start Game
//! (docs/game/forge-map-variants.md, "The lobby" and "Materialising for a
//! match").
//!
//! **The pick.** [`pick`] reads a variant of the lobby's map from the
//! store (`forge::files::variants::load`, with the hash of the installed
//! map), holds its [`MapId`] as `lobby_start::map_variant` and its
//! preparation here. A change of the lobby's map clears it ([`sync`]); so
//! does leaving the lobby ([`clear`]).
//!
//! **The build.** A pick whose package is cached (a package whose receipt
//! matches the id, [`Maps::cached`]) is [`Prep::Ready`] at once. Any other
//! is built once, off the game thread ([`Maps::build`]), and is
//! [`Prep::Building`] until it is done. `game` may not use `tools`, so the
//! composition root supplies the [`Maps`] (`boot`'s `forge_maps`), through
//! [`install`].
//!
//! **The hold.** Start Game is held while the pick builds, and while it is
//! refused: [`hold`] is the status line's sentence, which
//! `crate::lobby_mode::start_refused` and `crate::lobby_status` use. Every
//! refusal is named; none falls back to the original map.
//!
//! **At load.** [`package`] is the built package the session selects for
//! the lobby's map; it is `None` for Original.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use content::{MapId, Sha256};
use forge::files::variants::{self, Error};
use forge_doc::ForgeDoc;

/// A named refusal: the rules table's name and the sentence shown where the
/// player is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub sentence: String,
}

impl Refusal {
    pub fn new(code: &'static str, sentence: impl Into<String>) -> Self {
        Self {
            code,
            sentence: sentence.into(),
        }
    }

    /// A member's refusal, the host's sentence naming whose it is
    /// (`variant_member`).
    pub fn member(gamertag: &str, inner: &Refusal) -> Refusal {
        Refusal::new("variant_member", format!("{gamertag}: {}", inner.sentence))
    }
}

impl From<Error> for Refusal {
    fn from(e: Error) -> Self {
        match &e {
            Error::Missing(_) => Refusal::new("variant_missing", format!("{e}")),
            e => Refusal::new(e.code(), e.to_string()),
        }
    }
}

/// What the game needs of the maps and the package library: this machine's
/// maps by hash, the packages built, and the build.
pub trait Maps: Send {
    /// The SHA-256 of this machine's map of that name.
    fn installed(&self, map: &str) -> Option<Sha256>;
    /// The built package of `id`, when one whose receipt matches is on this
    /// machine.
    fn cached(&self, id: &MapId) -> Option<PathBuf>;
    /// Build `doc` and write its package; the package's directory.
    fn build(&mut self, doc: &ForgeDoc) -> Result<PathBuf, Refusal>;
}

/// The maps, shared with the build's thread.
pub type SharedMaps = Arc<Mutex<Box<dyn Maps>>>;

/// Where a pick's package stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prep {
    /// Building off the game thread.
    Building,
    /// Built, or cached: the package's directory.
    Ready(PathBuf),
    /// A check failed, or the build did.
    Refused(Refusal),
}

/// The lobby's pick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picked {
    /// The base map's name.
    pub map: String,
    /// The lobby's map (its list id) the pick was made on.
    pub lobby_map: Option<u32>,
    /// The variant's file stem in the store.
    pub file: String,
    pub name: String,
    pub id: Option<MapId>,
    pub prep: Prep,
}

type Cell = Arc<Mutex<Option<Picked>>>;

/// The pick, shared with the build's thread. Tests run side by side, each
/// on its own thread: each has its own lobby.
#[cfg(not(test))]
fn cell() -> Cell {
    static PICK: std::sync::OnceLock<Cell> = std::sync::OnceLock::new();
    PICK.get_or_init(Cell::default).clone()
}

#[cfg(test)]
fn cell() -> Cell {
    thread_local! {
        static PICK: Cell = Cell::default();
    }
    PICK.with(Cell::clone)
}

fn with_pick<T>(f: impl FnOnce(&mut Option<Picked>) -> T) -> T {
    f(&mut cell().lock().unwrap_or_else(|e| e.into_inner()))
}

static MAPS: Mutex<Option<SharedMaps>> = Mutex::new(None);

fn with_maps<T>(f: impl FnOnce(&mut Option<SharedMaps>) -> T) -> T {
    f(&mut MAPS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The maps and package library the builds use, from the composition root.
pub fn install(maps: Box<dyn Maps>) {
    with_maps(|m| *m = Some(Arc::new(Mutex::new(maps))));
}

/// The lobby's pick.
pub fn picked() -> Option<Picked> {
    with_pick(|p| p.clone())
}

/// Pick the variant `file` of `map`, the lobby's map being `lobby_map`, and
/// start its build. A variant that fails a check is held as picked and
/// refused ([`Prep::Refused`]), so Start Game says why. The thread the build
/// runs on, when there is one.
pub fn pick(
    data: &Path,
    map: &str,
    lobby_map: Option<u32>,
    file: &str,
    maps: &SharedMaps,
) -> Option<JoinHandle<()>> {
    let installed = maps
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .installed(map);
    let mut picked = Picked {
        map: map.to_owned(),
        lobby_map,
        file: file.to_owned(),
        name: file.to_owned(),
        id: None,
        prep: Prep::Building,
    };
    let doc = match variants::load(data, map, file, installed.as_ref()) {
        Ok(doc) => doc,
        Err(e) => {
            picked.prep = Prep::Refused(e.into());
            set(picked);
            return None;
        }
    };
    picked.name = doc.title.clone();
    let id = match doc.id() {
        Ok(document) => MapId::forge(doc.base.sha256, document),
        Err(e) => {
            picked.prep = Prep::Refused(Refusal::new(
                "variant_unreadable",
                format!("{} cannot be read ({e}).", doc.title),
            ));
            set(picked);
            return None;
        }
    };
    picked.id = Some(id);
    let cached = maps.lock().unwrap_or_else(|e| e.into_inner()).cached(&id);
    if let Some(dir) = cached {
        picked.prep = Prep::Ready(dir);
        set(picked);
        return None;
    }
    set(picked);
    let maps = Arc::clone(maps);
    let pick = cell();
    Some(std::thread::spawn(move || {
        let built = maps.lock().unwrap_or_else(|e| e.into_inner()).build(&doc);
        let prep = match built {
            Ok(dir) => Prep::Ready(dir),
            Err(refusal) => Prep::Refused(refusal),
        };
        // A pick that changed while this built keeps its own state.
        let mut held = pick.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = held.as_mut().filter(|p| p.id == Some(id)) {
            p.prep = prep;
        }
    }))
}

/// [`pick`] with the installed [`Maps`]; `None` without them (no builder in
/// this build, which refuses the pick by name).
pub fn pick_installed(
    data: &Path,
    map: &str,
    lobby_map: Option<u32>,
    file: &str,
) -> Option<JoinHandle<()>> {
    let Some(maps) = with_maps(|m| m.clone()) else {
        set(Picked {
            map: map.to_owned(),
            lobby_map,
            file: file.to_owned(),
            name: file.to_owned(),
            id: None,
            prep: Prep::Refused(Refusal::new(
                "variant_build",
                format!("{file} cannot be built in this game."),
            )),
        });
        return None;
    };
    pick(data, map, lobby_map, file, &maps)
}

fn set(picked: Picked) {
    crate::lobby_start::set_map_variant(picked.id);
    with_pick(|p| *p = Some(picked));
}

/// Back to Original: no pick.
pub fn clear() {
    crate::lobby_start::set_map_variant(None);
    forget();
}

/// The lobby's pick, but not the id `lobby_start` holds: the lobby was left
/// (`lobby_start::clear` resets that itself).
pub(crate) fn forget() {
    with_pick(|p| *p = None);
}

/// The lobby's map is `lobby_map` now: a pick made on another map is
/// cleared. Called wherever the lobby's state is read.
pub fn sync(lobby_map: Option<u32>) {
    let stale = with_pick(|p| p.as_ref().is_some_and(|p| p.lobby_map != lobby_map));
    if stale {
        clear();
    }
}

/// The sentence Start Game is held with: the pick is building, or refused.
/// `None` for Original and for a built pick.
pub fn hold() -> Option<String> {
    with_pick(|p| {
        let p = p.as_ref()?;
        match &p.prep {
            Prep::Ready(_) => None,
            Prep::Building => Some(format!("Building {}\u{2026}", p.name)),
            Prep::Refused(r) => Some(r.sentence.clone()),
        }
    })
}

/// The refusal that holds the start, by name.
pub fn refusal() -> Option<Refusal> {
    with_pick(|p| match p.as_ref()?.prep.clone() {
        Prep::Refused(r) => Some(r),
        _ => None,
    })
}

/// The built package the session selects for the lobby's map.
pub fn package() -> Option<(MapId, PathBuf)> {
    with_pick(|p| {
        let p = p.as_ref()?;
        match (&p.prep, p.id) {
            (Prep::Ready(dir), Some(id)) => Some((id, dir.clone())),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests;
