//! Watching a film in the running game (docs/game/theater-design.md,
//! "Starting a film in the running game").
//!
//! One session records and watches. `theater.watch` and the Theater menu's
//! Watch plan the film ([`Session::plan_watch`]), queue its options as the
//! next game through the agent's guest calls (`Call::QueueGame`, the path
//! `game.load` takes) and arm it ([`Session::arm_watch`]). When the queued
//! game activates, the session moves from recording to playing at the
//! game boundary: the recorder's state is parked whole and a player state
//! takes its place ([`Session::game_activated`]); the player's state is the
//! one a film player process opens with, so a film plays the same way in
//! both. Nothing is recorded while the film plays: the parked recorder is
//! not touched, and its ticks, tape and film file resume where they were
//! when the watch ends ([`Session::end_watch`]).
//!
//! The watch ends at the next game boundary after the viewer leaves
//! (`theater.end`, End Film, which queue the main menu), or when another
//! game activates. The film's settings are applied when the watch is armed
//! and the viewer's are put back when it ends. The film's end holds its
//! last tick and ends nothing: leaving is the viewer's.
//!
//! A start-route seek (the target is behind, or the viewer asked for the
//! start) queues the film's options again ([`Session::requeue`]); a seek
//! ahead runs forward from the tick shown.
use super::*;
use crate::level_boot;
use agent::sim::calls::{self, GameStep};
use agent_proto::{RpcError, code, theater as api};
use std::path::Path;
use std::sync::OnceLock;

/// What this session was recorded on and with: a film plays only when its
/// stamp is this (`film_play::Play::check_stamp`).
pub(super) struct Own {
    pub(super) xbe_sha256: String,
    pub(super) title_id: u32,
    /// The feature switch spec of the session's header.
    pub(super) features: String,
}

/// What a session holds for watching, beside its state.
pub(super) struct Hub {
    own: Own,
    /// The recorder, while a film plays.
    parked: Mutex<Option<Box<State>>>,
    /// A film whose options are queued and whose game has not activated.
    armed: Mutex<Option<Armed>>,
    /// The agent's guest calls, to queue a game.
    calls: OnceLock<calls::Shared>,
    /// The call that queued the latest game. A call leaves the queue when
    /// its ticket drops, so the ticket is kept until the next game is queued.
    ticket: Mutex<Option<calls::PendingCall>>,
    /// Why the last watched film stopped on its own.
    failed: Mutex<Option<String>>,
    /// Told the film's id when a watch ends by leaving, so the menu can
    /// open the film page again.
    returned: OnceLock<Box<dyn Fn(&str) + Send + Sync>>,
}

impl Hub {
    pub(super) fn new(own: Own) -> Self {
        Self {
            own,
            parked: Mutex::new(None),
            armed: Mutex::new(None),
            calls: OnceLock::new(),
            ticket: Mutex::new(None),
            failed: Mutex::new(None),
            returned: OnceLock::new(),
        }
    }
}

/// What a state holds while a film plays in the session.
pub(super) struct WatchInfo {
    /// The film's id.
    pub(super) film: String,
    /// The guest calls that queue the film's options.
    steps: Vec<GameStep>,
    /// The scenario and mode of the film's game, to tell it from another.
    scenario: String,
    kind: level_boot::Kind,
    /// The viewer's running values of the settings the film set.
    loaned: Vec<(String, settings::Value)>,
    /// The viewer asked to leave: the main menu is queued, and the film no
    /// longer holds its last tick.
    pub(super) leaving: bool,
}

struct Armed {
    state: Box<State>,
}

/// A film checked and ready to queue.
pub struct Planned {
    id: String,
    length_ticks: u64,
    steps: Vec<GameStep>,
    state: Box<State>,
    settings: film::start::Settings,
}

impl Planned {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn length_ticks(&self) -> u64 {
        self.length_ticks
    }

    /// The guest calls that queue the film's game.
    pub fn steps(&self) -> Vec<GameStep> {
        self.steps.clone()
    }
}

fn unavailable(why: impl Into<String>) -> RpcError {
    RpcError::new(code::UNAVAILABLE, why)
}

impl Session {
    /// The agent's guest calls, which queue a game. Set once at boot.
    pub fn attach_calls(&self, calls: calls::Shared) {
        let _ = self.watch.calls.set(calls);
    }

    pub fn calls(&self) -> Option<calls::Shared> {
        self.watch.calls.get().cloned()
    }

    /// Called with the film's id when a watch ends by the viewer leaving.
    pub fn on_return(&self, f: impl Fn(&str) + Send + Sync + 'static) {
        let _ = self.watch.returned.set(Box::new(f));
    }

    /// Whether a film is playing in this session now.
    pub fn watching(&self) -> bool {
        self.lock().watch.is_some()
    }

    /// This session can watch a film: it records games itself and is not a
    /// film player.
    pub fn can_watch(&self) -> bool {
        !self.replaying && self.player_features()
    }

    /// Check that the film at `path` can be watched here, and build the
    /// state that plays it. Nothing changes in the session.
    pub fn plan_watch(&self, path: &Path, view: api::InitialView) -> Result<Planned> {
        ensure!(self.can_watch(), "this session does not watch films");
        ensure!(
            !self.watching(),
            "a film is already being watched; leave it first (theater.end)"
        );
        let own = &self.watch.own;
        let play = film_play::Play::open(path)?.context(NO_GAME_START)?;
        play.check_stamp(&own.xbe_sha256)?;
        ensure!(
            play.start().settings.switches == own.features,
            "the film was recorded with other feature switches than this session runs"
        );
        let request = level_boot::Request::recorded(&play.start().options)?;
        let steps = level_boot::game_steps(&request)
            .context("the film's options do not fit the options block")?;
        let reader = Reader::open(path)?;
        let mut header = reader.header.clone();
        ensure!(
            header.xbe_sha256 == own.xbe_sha256 && header.title_id == own.title_id,
            "film boot facts differ"
        );
        let length = play.length();
        header.meta.length_ticks = length;
        let id = header.meta.id.clone();
        let settings = play.start().settings.clone();
        let first_update = play.start().game.first_update;
        let mut presentation = Presentation::default();
        presentation.start_view(view);
        presentation.camera.initialized = true;
        let replay = Replay {
            verification: None,
            path: path.to_owned(),
            cache: None,
            unpaced: false,
            // The game draws its own view; the camera rig is for a rendered
            // replay, which has its own window.
            render_every: 0,
            speed: 1.,
            seek_tick: 0,
            from_tick: None,
            seek_source: None,
            paused_at_target: false,
            presentation: presentation.clone(),
            restart: PathBuf::new(),
            camera: Default::default(),
            window_closed: None,
        };
        replay.camera.restore(&presentation);
        let start_at = view
            .tick
            .map(u64::from)
            .filter(|&tick| tick > 0)
            .map(|tick| tick.max(first_update).min(length));
        let seek = start_at.map(|target| SeekProgress {
            target,
            from_tick: 0,
            source: api::SeekSource::Boot,
            requested_source: None,
            warm: false,
            restore_ms: None,
        });
        let state = State {
            verify: None,
            mode: Mode::Replay(Playback::empty(reader, length), replay),
            engine: None,
            play: Some(play),
            header,
            tick: 0,
            calls: 0,
            paused: false,
            speed: 1.,
            rate: Default::default(),
            step_until: None,
            pacing: Some(Default::default()),
            seeking: start_at,
            seek,
            capture_points: Vec::new(),
            measurements: None,
            presentation,
            restart: None,
            ring_status: Default::default(),
            camera_redraw: None,
            warm_check: None,
            failure: None,
            complete: false,
            outcome: Outcome::default(),
            watch: Some(WatchInfo {
                film: id.clone(),
                steps: steps.clone(),
                scenario: request.scenario.clone(),
                kind: request.kind,
                loaned: Vec::new(),
                leaving: false,
            }),
        };
        Ok(Planned {
            id,
            length_ticks: length,
            steps,
            state: Box::new(state),
            settings,
        })
    }

    /// The film's options are queued: apply its simulation-scope settings
    /// and wait for its game to activate. A film already waiting is
    /// replaced, and its settings given back first.
    pub fn arm_watch(&self, planned: Planned) {
        self.disarm_watch();
        let Planned { mut state, settings, .. } = planned;
        let loaned = self.lend_settings(&settings);
        if let Some(info) = &mut state.watch {
            info.loaned = loaned;
        }
        *self.watch.armed.lock().unwrap_or_else(|e| e.into_inner()) = Some(Armed { state });
    }

    /// Give up a film that is waiting for its game: the options could not
    /// be queued, or another game came first.
    pub fn disarm_watch(&self) {
        let armed = self.watch.armed.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(Armed { state }) = armed
            && let Some(info) = &state.watch
        {
            self.give_back(&info.loaned);
        }
    }

    /// Run every setting the film set with the film's value. Returns what
    /// the viewer's session was running, for [`Self::give_back`]. A setting
    /// this build does not have, or a value it does not take, is named and
    /// left.
    fn lend_settings(&self, film: &film::start::Settings) -> Vec<(String, settings::Value)> {
        let mut loaned = Vec::new();
        let Some(store) = self.video_menu.get() else {
            return loaned;
        };
        for (key, &bits) in &film.values {
            let Some(def) = store.def(key).filter(|d| d.scope == settings::Scope::Sim) else {
                eprintln!("theater: the film's setting {key} is not one of this build's");
                continue;
            };
            let value = match store.get(def.key) {
                Some(settings::Value::Bool(_)) => settings::Value::Bool(bits != 0),
                _ => settings::Value::Int(bits),
            };
            match store.run_as(def.key, value) {
                Ok(before) => loaned.push((key.clone(), before)),
                Err(e) => eprintln!("theater: the film's setting {key}: {e}"),
            }
        }
        loaned
    }

    fn give_back(&self, loaned: &[(String, settings::Value)]) {
        let Some(store) = self.video_menu.get() else {
            return;
        };
        for (key, before) in loaned {
            if let Err(e) = store.run_as(key, *before) {
                eprintln!("theater: putting back {key}: {e}");
            }
        }
    }

    /// The watch starts or ends at a game boundary. `true` when the
    /// activation was the watch's own and the recorder is to ignore it.
    pub(super) fn watch_boundary(&self, g: &runtime::Guest) -> bool {
        let options = game_start::copy_options(g);
        if options.is_some() {
            let armed = self.watch.armed.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(armed) = armed {
                return self.begin_watch(armed, options.as_deref());
            }
        }
        let mut s = self.lock();
        let Some(info) = &s.watch else {
            return false;
        };
        let Some(options) = options else {
            // The main menu (or no game): the film's game is gone.
            drop(s);
            self.finish_watch(true);
            return true;
        };
        let same = level_boot::Request::recorded(&options)
            .is_ok_and(|r| r.scenario == info.scenario && r.kind == info.kind);
        if !same {
            // Another game started: the film is not what is playing.
            drop(s);
            self.finish_watch(false);
            return false;
        }
        // The film's game again: its queued start, or a revert's
        // reactivation of the map, which the film replays itself.
        if let Some(play) = &mut s.play {
            play.activated();
        }
        true
    }

    fn begin_watch(&self, armed: Armed, options: Option<&[u8]>) -> bool {
        let Armed { state } = armed;
        let same = options
            .and_then(|o| level_boot::Request::recorded(o).ok())
            .zip(state.watch.as_ref())
            .is_some_and(|(r, info)| r.scenario == info.scenario && r.kind == info.kind);
        if !same {
            // Not the film's game. Its settings go back and the recorder
            // goes on.
            if let Some(info) = &state.watch {
                self.give_back(&info.loaned);
            }
            eprintln!("theater: a game started that is not the film's; the film is not watched");
            return false;
        }
        *self.watch.failed.lock().unwrap_or_else(|e| e.into_inner()) = None;
        let mut s = self.lock();
        // The game in play, if any, is left as it is when its player quits
        // to the menu: no film is made of it.
        s.drop_game("left to watch a film".into());
        let id = state.watch.as_ref().map(|w| w.film.clone());
        let recorder = std::mem::replace(&mut *s, *state);
        *self.watch.parked.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(recorder));
        eprintln!("theater: watching film {}", id.unwrap_or_default());
        true
    }

    /// End the watch: the recorder's state returns and the viewer's
    /// settings are put back. `left` says the viewer left (the film's page
    /// is opened again) rather than another game starting. The film's id,
    /// when a watch was on.
    pub(super) fn finish_watch(&self, left: bool) -> Option<String> {
        let recorder = self.watch.parked.lock().unwrap_or_else(|e| e.into_inner()).take()?;
        let mut s = self.lock();
        let info = s.watch.take();
        *s = *recorder;
        drop(s);
        let info = info?;
        self.give_back(&info.loaned);
        if left && let Some(told) = self.watch.returned.get() {
            told(&info.film);
        }
        Some(info.film)
    }

    /// The session is ending: a film still playing is dropped, so the
    /// recorder seals its own games.
    pub(super) fn drop_watch(&self) {
        self.disarm_watch();
        self.finish_watch(false);
    }

    /// Why the last film watched here stopped before the viewer left it.
    pub(super) fn watch_failure(&self) -> Option<String> {
        self.watch.failed.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// A film being watched has diverged (the game's start hash, a digest,
    /// an update the film cannot apply). The film stops and the viewer
    /// leaves it as End Film does: the main menu is queued, the settings
    /// are put back at its activation and the film's page opens with the
    /// reason, which `theater.status` keeps as `watch_failure`. The game
    /// stays alive: the failure is the film's, not the guest's. Returns
    /// whether a watch was ended this way.
    pub(super) fn watch_diverged(&self) -> bool {
        let (film, why) = {
            let mut s = self.lock();
            let Some(why) = s.failure.clone() else { return false };
            let Some(info) = &mut s.watch else { return false };
            let film = info.film.clone();
            info.leaving = true;
            // The film is stopped: its ticks no longer count and the game
            // runs on its own while the menu loads.
            s.failure = None;
            s.complete = true;
            s.seeking = None;
            (film, why)
        };
        eprintln!("theater: film {film} stopped: {why}; leaving it for the main menu");
        *self.watch.failed.lock().unwrap_or_else(|e| e.into_inner()) = Some(why);
        if let Err(e) = self.queue(crate::game_menu::steps()) {
            // No way to the menu: the game in play is the film's, and the
            // recorder takes the session back as it is.
            eprintln!("theater: could not queue the main menu: {e}");
            self.finish_watch(true);
        }
        true
    }

    /// Leave the film for the main menu (`theater.end`, End Film). The game
    /// goes back at the next boundary, which is the menu's activation.
    pub fn end_film(&self) -> Result<(), RpcError> {
        {
            let mut s = self.lock();
            let Some(info) = &mut s.watch else {
                return Err(unavailable("no film is being watched"));
            };
            if info.leaving {
                return Ok(());
            }
            info.leaving = true;
            // The film's ticks stop counting and the game runs on its own
            // while the menu loads.
            s.complete = true;
        }
        if let Err(why) = self.queue(crate::game_menu::steps()) {
            let mut s = self.lock();
            if let Some(info) = &mut s.watch {
                info.leaving = false;
            }
            s.complete = false;
            return Err(unavailable(why));
        }
        Ok(())
    }

    fn queue(&self, steps: Vec<GameStep>) -> Result<(), String> {
        let calls = self
            .watch
            .calls
            .get()
            .ok_or("the game's own functions aren't reachable here: the agent-calls feature isn't installed")?;
        let mut held = self.watch.ticket.lock().unwrap_or_else(|e| e.into_inner());
        // A game queued before and not yet started is superseded.
        drop(held.take());
        *held = Some(calls::submit(calls, calls::Call::QueueGame { steps })?);
        Ok(())
    }

    /// A start-route seek: the film's options are queued again and the film
    /// starts over at the new game's first update, running forward to
    /// `target`.
    pub(super) fn requeue(&self, target: u64) -> Result<(), RpcError> {
        let steps = {
            let mut s = self.lock();
            let Some(info) = &s.watch else {
                return Err(unavailable("no film is being watched"));
            };
            let steps = info.steps.clone();
            s.restart_play(target);
            steps
        };
        self.queue(steps).map_err(unavailable)
    }
}

impl State {
    /// The film starts over: its game is queued again and ticks are the
    /// film's again from that game's first update, which runs forward to
    /// `target`. The viewer's speed, pause and view stay.
    pub(super) fn restart_play(&mut self, target: u64) {
        let first = self.play.as_ref().map_or(1, |play| play.start().game.first_update);
        if let Some(play) = &mut self.play {
            play.await_activation();
        }
        self.tick = 0;
        self.restart = None;
        self.complete = false;
        self.failure = None;
        self.step_until = None;
        self.rate = Default::default();
        if let Some(clock) = &mut self.pacing {
            clock.debt = 0.;
        }
        self.seeking = Some(target.max(first));
        if let Some(seek) = &mut self.seek {
            seek.source = api::SeekSource::Boot;
        }
    }

    /// A seek of a film watched in the running game. Ahead of the tick
    /// shown it runs forward; otherwise (or when the start was asked for)
    /// it is a restart, which the session turns into a queued game.
    pub(super) fn seek_in_game(
        &mut self,
        target: u64,
        from: u64,
        requested: Option<api::SeekRoute>,
    ) {
        let begun = self.play.as_ref().is_some_and(|play| play.begun());
        let first = self.play.as_ref().map_or(1, |play| play.start().game.first_update);
        if begun && target > self.tick && requested != Some(api::SeekRoute::Boot) {
            self.seek = Some(SeekProgress {
                target,
                from_tick: from,
                source: api::SeekSource::Forward,
                requested_source: requested,
                warm: false,
                restore_ms: None,
            });
            self.seeking = Some(target.max(first));
            self.step_until = None;
            self.rate = Default::default();
        } else {
            self.queue_jump(target, from, requested, api::SeekSource::Boot);
        }
    }

    /// The film has been given its last tick, or has stopped, and holds
    /// there until the viewer leaves.
    pub(super) fn holds_in_game(&self) -> bool {
        self.watch.as_ref().is_some_and(|info| !info.leaving)
            && (self.complete || self.failure.is_some())
    }
}

#[cfg(test)]
mod tests;
