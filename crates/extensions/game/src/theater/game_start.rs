//! The game start a recording writes into the film
//! (docs/game/theater-design.md, "The start state"; `film::start`).
//!
//! The start has two moments. **Activation**: when `GAME_ACTIVATE` returns
//! success for a game that is not the main menu, the engine's options block
//! is copied whole ([`Starts::activated`]), and a default recording opens
//! the new game's film. **The first update**: at entry
//! to the game's first `SIM_UPDATE_APPLY` the recorder reads the two shared
//! random words and the arena hash ([`first_update`]), and holds the start
//! until the tick's observer has described the game ([`Starts::release`]),
//! which gives the map's identity and digest, the roster and the mode. The
//! start is written just before that tick, the film's tick 1. A game the
//! observer has not described by then is taken again at each following
//! update until it is, and its start sits before that update's tick.
use super::{Mode, Session, metadata};
use anyhow::{Context, Result, ensure};
use film::start::{Game, GameStart, OPTIONS_BYTES, Settings, Stamp};
use runtime::Guest;

/// What the recorder holds for the game in play.
#[derive(Default)]
pub(super) struct Starts {
    /// The options block copied at activation, until the game's first
    /// update takes it.
    activated: Option<Vec<u8>>,
    /// The options of the game whose start was taken. The same block
    /// activating again is the same game (a campaign revert reactivates
    /// the map), not a new one.
    taken: Option<Vec<u8>>,
    /// The start taken at the first update, until the observer has
    /// described the game.
    held: Option<GameStart>,
    /// Updates a game has gone on undescribed, after its first.
    undescribed: u32,
    /// Overrides [`DESCRIBE_UPDATES`]: a test's seam, so a give-up takes
    /// a few updates instead of hundreds of arena hashes.
    #[cfg(test)]
    pub(super) describe_updates: Option<u32>,
}

/// Updates the observer may take to describe a game, counted from its
/// first. A game still unknown after them is not a match.
const DESCRIBE_UPDATES: u32 = 600;

impl Starts {
    /// Whether a map activated with these options is a new game: neither
    /// the game in play nor the one waiting for its first update.
    pub(super) fn is_new(&self, options: &[u8]) -> bool {
        self.taken.as_deref() != Some(options) && self.activated.as_deref() != Some(options)
    }

    /// A map activated with these options. A different block than the
    /// game in play's is a new game, whose start the next first update
    /// takes.
    pub(super) fn activated(&mut self, options: Vec<u8>) {
        if self.taken.as_ref() == Some(&options) {
            return;
        }
        self.taken = None;
        self.held = None;
        self.activated = Some(options);
    }

    /// The options of a game waiting for its first update.
    pub(super) fn waiting(&self) -> bool {
        self.activated.is_some()
    }

    /// Whether the waiting game's start was taken before and not yet
    /// written: a retake at a later update.
    pub(super) fn retaking(&self) -> bool {
        self.undescribed > 0
    }

    /// The first update of the waiting game: its start, taken.
    pub(super) fn take_options(&mut self) -> Option<Vec<u8>> {
        let options = self.activated.take()?;
        self.taken = Some(options.clone());
        Some(options)
    }

    pub(super) fn hold(&mut self, start: GameStart) {
        self.held = Some(start);
    }

    /// The held start, once the tick's observer has described the game:
    /// the map's identity (its digest joined), the mode and the roster. A
    /// game the observer has not described yet keeps waiting: its start is
    /// taken again at the next update, so the start sits just before the
    /// first update that has a description. A game still undescribed after
    /// [`DESCRIBE_UPDATES`] updates has no start. `None` while nothing is
    /// held.
    pub(super) fn release(&mut self, meta: &mut metadata::Capture) -> Result<Option<GameStart>> {
        let Some(mut start) = self.held.take() else {
            return Ok(None);
        };
        if meta.descriptor().map.name.is_empty() {
            self.undescribed += 1;
            #[cfg(test)]
            let limit = self.describe_updates.unwrap_or(DESCRIBE_UPDATES);
            #[cfg(not(test))]
            let limit = DESCRIBE_UPDATES;
            ensure!(
                self.undescribed <= limit,
                "the game was not described in its first {DESCRIBE_UPDATES} updates"
            );
            self.activated = Some(start.options);
            return Ok(None);
        }
        meta.finish_map()?;
        start.game.started_ms = super::autorecord::now_ms();
        start.game.descriptor = meta.descriptor().clone();
        Ok(Some(start))
    }

    /// The game ended or was left: the next one starts afresh.
    pub(super) fn next_game(&mut self) {
        *self = Self::default();
    }
}

impl Session {
    /// A map activated (`GAME_ACTIVATE` returned success): a recording
    /// copies the options block the game starts from, a default recording
    /// opens a new game's film (sealing the game before it, if any), and a
    /// film player's requeued game begins.
    pub(super) fn game_activated(&self, g: &Guest) {
        // A film's game, or the end of one, is the watch's boundary: the
        // recorder is parked while a film plays and sees none of it.
        if self.watch_boundary(g) {
            return;
        }
        let mut s = self.lock();
        // A film player's game queued again by a start-route seek
        // (`Session::requeue`): the film starts over at its first update.
        if let Some(play) = &mut s.play
            && play.awaiting()
            && copy_options(g).is_some()
        {
            play.activated();
            return;
        }
        if s.complete || s.failure.is_some() {
            return;
        }
        if !matches!(s.mode, Mode::Record(_)) {
            return;
        }
        let Some(options) = copy_options(g) else { return };
        if matches!(&s.mode, Mode::Record(rec) if rec.library().is_some() && rec.starts.is_new(&options)) {
            s.open_game();
        }
        if let Mode::Record(rec) = &mut s.mode {
            rec.starts.activated(options);
        }
    }
}

/// The active game's options block, copied whole; `None` unless a campaign
/// or multiplayer game is active (the main menu is neither).
pub(super) fn copy_options(g: &Guest) -> Option<Vec<u8>> {
    let at = metadata::active_options(g)?;
    let mut block = vec![0u8; OPTIONS_BYTES];
    g.read_bytes(at, &mut block, 1).ok()?;
    Some(block)
}

/// The shared random words (`sim_glue::RANDOM_SEED_PTR` points at them).
pub(super) fn random_words(g: &Guest) -> Result<[u32; 2]> {
    let at = g
        .u32(crate::sim_glue::RANDOM_SEED_PTR)
        .ok()
        .filter(|&p| p != 0)
        .context("the shared random words are not set")?;
    Ok([
        g.u32(at).context("reading the first random word")?,
        g.u32(at.wrapping_add(4)).context("reading the second random word")?,
    ])
}

/// The stamp a build must equal to play the film.
pub(super) fn stamp(header: &film::Header) -> Stamp {
    Stamp {
        xbe_sha256: header.xbe_sha256.clone(),
        simulation_abi: header.runtime_stamp.clone(),
        contract_sha256: header
            .guest_image
            .as_ref()
            .and_then(|image| image.contract_sha256.clone()),
        hooks_sha256: header.simulation_hooks.clone(),
        build: header.meta.image_stamp.clone(),
    }
}

/// Every setting whose scope is `Sim`, by value, and the feature switch
/// spec. Without a store (a headless recording) no setting is listed.
pub(super) fn settings(store: Option<&settings::Store>, switches: &str) -> Settings {
    let mut values = std::collections::BTreeMap::new();
    if let Some(store) = store {
        for def in store.defs() {
            if def.scope != settings::Scope::Sim {
                continue;
            }
            if let Some(value) = store.get(def.key) {
                let bits = match value {
                    settings::Value::Bool(b) => i64::from(b),
                    settings::Value::Int(i) => i,
                };
                values.insert(def.key.to_owned(), bits);
            }
        }
    }
    Settings {
        values,
        switches: switches.to_owned(),
    }
}

/// The rules script of the active variant, by value: its eight-byte prefix
/// and then its blob. `None` for a variant that plays as its own game type.
pub(super) fn script(g: &Guest) -> Option<Vec<u8>> {
    let block = crate::rules::guest::block(g).ok().flatten()?;
    let script = variants::ext::Ext::in_effect(&block).script;
    if script.mode != variants::ext::Mode::Rules {
        return None;
    }
    let blob = crate::rules::feature::local_blob(script.prefix)?;
    let mut out = script.prefix.to_vec();
    out.extend_from_slice(&blob);
    Some(out)
}

/// What the recorder knows at entry to a game's first update: the options
/// copied at activation, the random words, the arena hash, the stamp and
/// the settings. `film_tick` is the tick of the film file the update's
/// observer will push, 1-based.
pub(super) fn first_update(
    g: &Guest,
    options: Vec<u8>,
    header: &film::Header,
    store: Option<&settings::Store>,
    film_tick: u64,
) -> Result<GameStart> {
    ensure!(
        options.len() == OPTIONS_BYTES,
        "the options block is {} bytes",
        options.len()
    );
    dump_arena(g, "recorded", film_tick);
    Ok(GameStart {
        stamp: stamp(header),
        game: Game {
            started_ms: 0,
            first_update: film_tick,
            first_game_tick: bots::guest::game_tick(g).map_or(0, u64::from),
            descriptor: Default::default(),
        },
        options,
        random: random_words(g)?,
        start_hash: lockstep::hash_gameplay(g, lockstep::SimRegion::HALO2)
            .context("the arena has no hash at the game's first update")?,
        hash_detail: hash_detail(g),
        settings: settings(store, &header.features),
        variant: script(g),
        map_variant: None,
    })
}

/// With `H2_THEATER_ARENA_DUMP=<dir>`, write the arena as it stands to
/// `<dir>/arena-<role>-<tick>-<pid>.bin`: a recording's start, or a
/// watched film's start, to be compared byte for byte.
pub(super) fn dump_arena(g: &Guest, role: &str, first_update: u64) {
    let Some(dir) = std::env::var_os("H2_THEATER_ARENA_DUMP") else { return };
    let (mut bytes, mut pages) = (Vec::new(), Vec::new());
    let Some((base, _)) = lockstep::hash_sim_blocks(
        g,
        lockstep::SimRegion::HALO2,
        film::start::DETAIL_PAGE,
        &mut bytes,
        &mut pages,
    ) else {
        return;
    };
    let path = std::path::Path::new(&dir)
        .join(format!("arena-{role}-{first_update}-{}.bin", std::process::id()));
    match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &bytes)) {
        Ok(()) => eprintln!("theater: arena at the {role} start ({base:#x}) written to {}", path.display()),
        Err(e) => eprintln!("theater: could not write {}: {e}", path.display()),
    }
}

/// What the arena hash at this moment is made of: the extension records'
/// digest and a hash of each page of the arena. `None` when the arena is
/// not readable.
pub(super) fn hash_detail(g: &Guest) -> Option<film::start::HashDetail> {
    let (mut bytes, mut pages) = (Vec::new(), Vec::new());
    lockstep::hash_gameplay_blocks(
        g,
        lockstep::SimRegion::HALO2,
        film::start::DETAIL_PAGE,
        &mut bytes,
        &mut pages,
    )?;
    Some(film::start::HashDetail { ext: sim_world::ext::digest(), pages })
}

/// How many owners a start's report names.
const OWNERS_SHOWN: usize = 8;

/// What differs between the film's start and the game's (`now`): the
/// pages and the extension digest ([`film::start::HashDetail::differences`]),
/// and the arena's data arrays that own the differing pages, read from
/// `arena`, the game's arena as it stands (unmasked, so every array's
/// header is there: `lockstep::pools::page_owners`).
pub(super) fn start_differences(
    film: &film::start::HashDetail,
    now: &film::start::HashDetail,
    arena: Option<&[u8]>,
) -> String {
    let mut out = film.differences(now);
    let pages = film.differing_pages(now);
    if let Some(arena) = arena
        && !pages.is_empty()
    {
        let owners = lockstep::pools::page_owners(arena, film::start::DETAIL_PAGE, &pages);
        if !owners.is_empty() {
            out.push_str("; in ");
            out.push_str(&lockstep::pools::owners_line(&owners, OWNERS_SHOWN));
        }
    }
    out
}

/// The game's arena as it stands, unmasked; `None` when it is not
/// readable.
pub(super) fn arena_bytes(g: &Guest) -> Option<Vec<u8>> {
    let (mut bytes, mut pages) = (Vec::new(), Vec::new());
    lockstep::hash_sim_blocks(
        g,
        lockstep::SimRegion::HALO2,
        film::start::DETAIL_PAGE,
        &mut bytes,
        &mut pages,
    )?;
    Some(bytes)
}

#[cfg(test)]
pub(in crate::theater) mod tests;
