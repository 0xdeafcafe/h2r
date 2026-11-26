//! Playing a film from its game start (docs/game/theater-design.md,
//! "Playback"). The player takes the film file's game start
//! (`film::start::GameStart`), has the game load the map with the film's
//! options as `level_boot` answers the first map request, and plays from
//! the game's first update:
//!
//! - **Before the first update** the ticks are the game's own (menus,
//!   loading) and are not film ticks.
//! - **At the first update** the two shared random words are written, the
//!   map is checked against the film's, and the arena hash is compared
//!   with the film's start hash. A difference stops the film before the
//!   update is applied.
//! - **Each update after** has the film's actions and body for its film
//!   tick written over the engine's at `SIM_UPDATE_APPLY`
//!   (`actions::at_update`), and the gameplay digest of the film tick a
//!   game tick ends on is compared at the tick's end ([`tick_done`]). A
//!   film tick is one application of the update: a game tick that applies
//!   it twice takes two. The film ends after its last tick.
//! - **Extension records** a feature's outside input
//!   ([`super::extension`]): the records that come before a film tick are
//!   handed to their features when the tick before it ends (the first
//!   tick's, at the game's first update), and each feature applies them in
//!   the game tick that follows. A film with records of a feature this
//!   build does not have does not open.
//! - **Forge monitor looks and poses** are presentation and sit in the
//!   stats container beside the film file, one container tick for each
//!   game tick ([`Looks`]): when a game tick ends, the container's ticks up
//!   to it are handed to `forge_film`, which the camera rig and the
//!   renderer read. A film with no stats container here plays without them.
//!
//! A film with no game start is refused ([`super::NO_GAME_START`]). The
//! player restores nothing of the recording machine: the data directory
//! is empty and private, and the guest's storage writes go to a scratch
//! (`vfs::Vfs::guard_writes`). A seek behind the tick shown queues the
//! film's options again and runs forward (`Session::requeue`).
use super::{Session, State, forge_film, metadata};
use anyhow::{Context, Result, bail, ensure};
use film::blam::{TrackReader, stream};
use film::file::FilmActions;
use film::start::GameStart;
use runtime::Guest;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The options' count of local players (`OPTIONS_PLAYERS` writes it at
/// `+0x2cc`; docs/runtime/level-boot.md).
const OPT_PLAYER_COUNT: usize = 0x2cc;
/// Controller ports the guest has.
const PORTS: u32 = 4;

pub(super) struct Play {
    actions: FilmActions,
    start: GameStart,
    length: u64,
    /// The game has taken its first update and the film is being played.
    begun: bool,
    /// The last film tick a game tick ended on.
    at: u64,
    /// Updates applied in the game tick in progress: each took the film
    /// tick after the last.
    applied: u64,
    /// The game ticks the film's game has ended since its first update
    /// (a session ring entry keeps it).
    game_ticks: u64,
    /// The film's game has been queued and has not activated: a start-route
    /// seek in the running game (`watch`). Updates before the activation
    /// are the old game's own and are not the film's.
    awaiting: bool,
    /// The mounted maps, to check the game's map against the film's.
    pub(super) map_vfs: Option<Arc<Mutex<vfs::Vfs>>>,
    /// The Forge monitor records of the film's stats container, when it has
    /// one here.
    looks: Option<Looks>,
    /// The build of the session watching the film (its image stamp's first
    /// line), when it is another than the film's: a start that does not
    /// reproduce says so.
    other_build: Option<String>,
}

/// What a film recorded by another build is refused with, or what a start
/// that does not reproduce on another build says first.
pub(super) const ANOTHER_BUILD: &str = "This film was recorded by another build.";

/// The first line of a build stamp, which names the build.
fn build_line(stamp: &str) -> &str {
    stamp.lines().next().unwrap_or_default().trim()
}

/// The film's Forge monitor looks and poses (`forge_film`), read from its
/// stats container a game tick at a time. The container holds one tick for
/// each game tick, where the film file holds one for each application of
/// the update, so it is counted in game ticks from the game's first update.
/// A film whose container is not here (a host's film read from Live) or
/// cannot be read (a recording killed before its seal) plays without them;
/// a record that cannot be read is a named film error.
struct Looks {
    stats: PathBuf,
    track: Option<TrackReader<File>>,
    /// The container tick the last game tick ended on.
    at: u64,
    /// The container is read to its end, or could not be opened.
    done: bool,
    /// A monitor record has been handed over: the playback's clock follows
    /// every game tick from here, and the playback is cleared when the film
    /// starts over or ends. A film with no monitor record leaves it alone.
    fed: bool,
}

impl Looks {
    fn of(film: &Path) -> Option<Self> {
        let stats = film::file::stats_of(film);
        stats.is_file().then_some(Self {
            stats,
            track: None,
            at: 0,
            done: false,
            fed: false,
        })
    }

    /// The film's game takes its first update after container tick `before`.
    fn begin(&mut self, before: u64) {
        self.at = before;
    }

    /// The film starts over (a start-route seek): what the playback read
    /// goes, and the container is read again from its start.
    fn restart(&mut self) {
        self.track = None;
        self.at = 0;
        self.done = false;
        if std::mem::take(&mut self.fed) {
            forge_film::clear();
        }
    }

    /// A game tick ended: hand `forge_film` the records of the container's
    /// ticks up to it, each with its tick.
    fn tick_ended(&mut self) -> Result<()> {
        self.at += 1;
        if !self.done && self.track.is_none() {
            match TrackReader::open(&self.stats) {
                Ok(track) => self.track = Some(track.without_actions()),
                Err(error) => {
                    eprintln!("theater: the film's Forge monitor records are not read: {error:#}");
                    self.done = true;
                }
            }
        }
        if !self.done
            && let Some(track) = self.track.as_mut()
        {
            loop {
                let Some(tick) = track.step()? else {
                    self.done = true;
                    break;
                };
                let monitors = tick.forge.iter().any(|payload| {
                    matches!(payload.first(), Some(&stream::FORGE_LOOK | &stream::FORGE_POSE))
                });
                if monitors || self.fed {
                    self.fed = true;
                    forge_film::deliver(tick.tick, &tick.forge)?;
                }
                if tick.tick >= self.at {
                    return Ok(());
                }
            }
        }
        // Past the container's end the playback's clock goes on.
        if self.fed {
            forge_film::deliver(self.at, &[])?;
        }
        Ok(())
    }
}

impl Drop for Play {
    /// The film ends: the monitors it showed go with it.
    fn drop(&mut self) {
        if self.looks.as_ref().is_some_and(|looks| looks.fed) {
            forge_film::clear();
        }
    }
}

impl Play {
    /// The film file at `path`, if it has a game start: the film plays
    /// from it. `None` for no film file or a film file with no game start,
    /// which does not play. An unsealed film (its recording was killed)
    /// plays up to its last checkpoint.
    pub(super) fn open(path: &Path) -> Result<Option<Self>> {
        if !path.try_exists()? {
            return Ok(None);
        }
        let mut actions = FilmActions::open(path)?;
        let Some(start) = actions.game_start()? else {
            return Ok(None);
        };
        super::extension::check(&actions.extension_tags())?;
        super::extension::clear();
        let length = actions.length_ticks();
        ensure!(
            start.game.first_update >= 1 && start.game.first_update <= length,
            "the film's game starts at tick {}, outside the film's {length} ticks",
            start.game.first_update
        );
        Ok(Some(Self {
            actions,
            start,
            length,
            begun: false,
            at: 0,
            applied: 0,
            game_ticks: 0,
            awaiting: false,
            map_vfs: None,
            looks: Looks::of(path),
            other_build: None,
        }))
    }

    /// The film's options are queued again: the game that runs until they
    /// load is not the film's, and the film starts over at the game's first
    /// update (`Session::requeue`).
    pub(super) fn await_activation(&mut self) {
        self.begun = false;
        self.at = 0;
        self.applied = 0;
        self.game_ticks = 0;
        self.awaiting = true;
        super::extension::clear();
        if let Some(looks) = &mut self.looks {
            looks.restart();
        }
    }

    /// The queued game activated.
    pub(super) fn activated(&mut self) {
        self.awaiting = false;
    }

    pub(super) fn awaiting(&self) -> bool {
        self.awaiting
    }

    /// The film's length in ticks.
    pub(super) fn length(&self) -> u64 {
        self.length
    }

    /// Whether the game has begun, so that the film's ticks are counted.
    pub(super) fn begun(&self) -> bool {
        self.begun
    }

    pub(super) fn start(&self) -> &GameStart {
        &self.start
    }

    /// What a film must have been recorded on to be played by this build:
    /// the XBE and the simulation ABI of its stamp. The guest image and its
    /// hooks are checked against the film's header as for every film.
    pub(super) fn check_stamp(&self, xbe_sha256: &str) -> Result<()> {
        let abi = super::compatibility::SIMULATION_ABI;
        ensure!(
            self.start.stamp.xbe_sha256 == xbe_sha256,
            "film requires a different XBE"
        );
        ensure!(
            self.start.stamp.simulation_abi == abi,
            "Film predates a breaking simulation change. Simulation ABI differs (recorded {}, installed {abi}).",
            self.start.stamp.simulation_abi
        );
        Ok(())
    }

    /// A film watched in a session that runs other simulation hooks than
    /// the film was recorded with does not start as it did: it is refused
    /// by name. `hooks` is the session's hook digest and `build` its build
    /// stamp. A digest either side does not know is not compared. A film
    /// of another build with the same hooks plays; if its start does not
    /// reproduce, the reason names the build ([`Play::begin`]).
    pub(super) fn check_build(&mut self, hooks: Option<&str>, build: &str) -> Result<()> {
        let film = &self.start.stamp;
        if let (Some(theirs), Some(ours)) = (film.hooks_sha256.as_deref(), hooks) {
            ensure!(
                theirs == ours,
                "{ANOTHER_BUILD} Its simulation hooks differ from this build's, so its game would not start as it did (recorded on {}).",
                build_line(&film.build)
            );
        }
        self.other_build = (!build.is_empty() && !film.build.is_empty() && film.build != build)
            .then(|| build_line(build).to_owned());
        Ok(())
    }

    /// The controller ports the guest has a neutral pad on: one for each
    /// local player the options name, at most the guest's four.
    pub(super) fn ports(&self) -> usize {
        ports(&self.start.options) as usize
    }

    /// The game ticks the film's game has ended since its first update.
    pub(super) fn game_ticks(&self) -> u64 {
        self.game_ticks
    }

    /// A session ring entry was loaded into the game: the state at the end
    /// of film tick `tick`, which the film's game reached in `game_ticks`
    /// game ticks. The film goes on from the tick after it: records handed
    /// to their features and not applied go, and the next tick's are handed
    /// over again, as when the tick ended ([`tick_done`]).
    pub(super) fn resume_at(&mut self, tick: u64, game_ticks: u64) -> Result<()> {
        ensure!(self.begun && !self.awaiting, "the film's game is not running");
        ensure!(
            tick >= self.start.game.first_update && tick <= self.length,
            "tick {tick} is outside the film's game ({} to {})",
            self.start.game.first_update,
            self.length
        );
        self.at = tick;
        self.applied = 0;
        self.game_ticks = game_ticks;
        super::extension::clear();
        if tick < self.length {
            self.deliver(tick + 1)?;
        }
        Ok(())
    }

    /// Hand the features the extension records that come before film tick
    /// `tick` ([`super::extension::deliver`]).
    fn deliver(&mut self, tick: u64) -> Result<()> {
        for (tag, bytes) in self.actions.extensions_at(tick)? {
            super::extension::deliver(tag, bytes)?;
        }
        Ok(())
    }

    /// The game's first update. Writes the shared random words, checks the
    /// map and compares the arena hash with the film's; the film has begun
    /// only when they agree.
    fn begin(&mut self, g: &mut Guest) -> Result<()> {
        let first = self.start.game.first_update;
        let at = g
            .u32(crate::sim_glue::RANDOM_SEED_PTR)
            .ok()
            .filter(|&p| p != 0)
            .context("the shared random words are not set")?;
        for (offset, word) in [(0, self.start.random[0]), (4, self.start.random[1])] {
            g.set_u32(at.wrapping_add(offset), word)
                .map_err(|e| anyhow::anyhow!("writing the shared random words: {e:?}"))?;
        }
        if let Some(vfs) = &self.map_vfs {
            // The descriptor names the tick the match began at; the check
            // is made at this game's first update.
            let mut descriptor = self.start.game.descriptor.clone();
            descriptor.match_start_tick = Some(first);
            metadata::verify_start(g, first, &descriptor, Some(vfs))?;
        }
        super::game_start::dump_arena(g, "watch", first);
        let hash = lockstep::hash_gameplay(g, lockstep::SimRegion::HALO2)
            .context("the arena has no hash at the game's first update")?;
        if hash != self.start.start_hash {
            // Name what differs, when the film says what it was made of:
            // the pages, and the data arrays that own them.
            let detail = self
                .start
                .hash_detail
                .as_ref()
                .zip(super::game_start::hash_detail(g))
                .map(|(film, now)| {
                    let arena = super::game_start::arena_bytes(g);
                    let why = super::game_start::start_differences(film, &now, arena.as_deref());
                    format!(" ({why})")
                })
                .unwrap_or_default();
            let build = self
                .other_build
                .as_ref()
                .map(|ours| {
                    format!(
                        "{ANOTHER_BUILD} It was recorded on {}; this is {ours}. ",
                        build_line(&self.start.stamp.build)
                    )
                })
                .unwrap_or_default();
            bail!(
                "{build}the game's start hash {hash:016x} differs from the film's {:016x}{detail}",
                self.start.start_hash
            );
        }
        self.begun = true;
        Ok(())
    }
}

/// How many local players the options name, as controller ports.
pub(super) fn ports(options: &[u8]) -> u32 {
    options
        .get(OPT_PLAYER_COUNT..OPT_PLAYER_COUNT + 4)
        .map_or(1, |b| u32::from_le_bytes(b.try_into().unwrap()))
        .clamp(1, PORTS)
}

/// An update entered `SIM_UPDATE_APPLY`: the film's actions and body for
/// its film tick are written over the engine's, the film's head and stamp
/// untouched. The game's first update starts the film ([`Play::begin`]); a
/// failure there leaves the update as the engine built it. A first-person
/// camera has each local user's look set to the film's facing. Each update the
/// game tick applies takes the next film tick.
pub(super) fn update(state: &mut State, g: &mut Guest, update: u32) -> Result<()> {
    let first_person = matches!(&state.mode, super::Mode::Replay(o) if o.camera.reads_look());
    let play = state.play.as_mut().context("no film is being played")?;
    if play.awaiting {
        return Ok(());
    }
    if !play.begun {
        let first = play.start.game.first_update;
        state.tick = first - 1;
        play.begin(g)?;
        play.at = first - 1;
        if let Some(looks) = &mut play.looks {
            looks.begin(first - 1);
        }
        play.deliver(first)?;
    }
    let tick = play.at + play.applied + 1;
    ensure!(
        tick <= play.length,
        "the game ran past the film's last tick {}",
        play.length
    );
    let row = play.actions.row(tick)?;
    play.applied += 1;
    super::actions::apply(g, update, &row.inputs)?;
    super::actions::apply_body(g, update, &row.body)?;
    super::actions::apply_look(g, update, first_person);
    Ok(())
}

/// A game tick ended (`Session::checkpoint`, with `state.tick` already
/// advanced by it): it must have taken the updates the film has for the
/// film ticks it covers, and the gameplay digest it ended on must be the
/// film's. A game tick that applied no update covers one film tick, which
/// must have none. The film's last tick completes it.
pub(super) fn tick_done(state: &mut State, hash: Option<u64>) {
    let Some(play) = state.play.as_mut() else {
        return;
    };
    let applied = std::mem::take(&mut play.applied);
    let tick = play.at + applied.max(1);
    state.tick = tick;
    let mut digested = false;
    let checked = (|| -> Result<bool> {
        let row = play.actions.row(tick)?;
        ensure!(
            applied > 0 || row.inputs.is_empty(),
            "simulation skipped the recorded player update"
        );
        if let Some(low) = row.digest {
            let hash = hash.context("the game has no gameplay digest at this tick")?;
            ensure!(
                hash as u16 == low,
                "expected gameplay digest {low:04x}, observed {:04x} ({hash:016x})",
                hash as u16
            );
            digested = true;
        }
        if let Some(whole) = row.closing {
            ensure!(
                hash == Some(whole),
                "expected gameplay {whole:016x}, observed {hash:?}"
            );
        }
        if let Some(looks) = &mut play.looks {
            looks
                .tick_ended()
                .context("the film's Forge monitor records")?;
        }
        if tick < play.length {
            play.deliver(tick + 1)?;
        }
        Ok(tick >= play.length)
    })();
    play.at = tick;
    play.game_ticks += 1;
    if digested && let Some(v) = &mut state.verify {
        v.digest(tick);
    }
    match checked {
        Ok(true) => state.complete = true,
        Ok(false) => {}
        Err(error) => state.fail(format!("{error:#}")),
    }
}

/// The film player process's environment (`h2 theater play` starts it):
/// the film's feature switches, and none of the viewer's overrides of a
/// setting the simulation reads. The film's settings are its game start's.
pub fn configure_replay_child(header: &film::Header, child: &mut std::process::Command) -> Result<()> {
    let switches = hooks::Switches::parse(&header.features).map_err(anyhow::Error::msg)?;
    child.env("H2_FEATURES", switches.spec());
    for def in crate::setting_defs::defs() {
        // The viewer's display settings stay theirs.
        if def.scope != settings::Scope::Host
            && def.key != crate::setting_defs::LOD_DISTANCE.key
            && let Some(name) = def.env
        {
            child.env_remove(name);
        }
    }
    Ok(())
}

impl Session {
    /// The game start being played.
    pub fn game_start(&self) -> Option<GameStart> {
        self.lock().play.as_ref().map(|play| play.start().clone())
    }
}

#[cfg(test)]
mod tests;
