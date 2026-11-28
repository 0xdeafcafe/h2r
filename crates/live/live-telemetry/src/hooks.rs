mod film;
use film::FilmProjection;
pub use film::{FilmObserver, install_film};

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::points;
#[cfg(test)]
use runtime::Runtime;

use crate::config::{TelemetryConfig, TelemetrySinkConfig};
use crate::event::{PlayerState, Shots, World};
use crate::layout;
use crate::memory;
use crate::pipeline::{Kill, Names, Pipeline, Start, Work};
use crate::session::{Playlist, Session};
use crate::sink::Sink;
#[cfg(test)]
use crate::sink::JsonlSink;
use crate::tags::{BotsLocator, HostedSessionLocator, MapIdLocator, MapLocator, PlaylistLocator};

/// Weapons with uncollected shots are few (only those fired since the last
/// completed tick); past this many the counts are dropped, not grown.
const MAX_FIRING_WEAPONS: usize = 256;
/// Damage reports kept between samples (one tick).
const MAX_PENDING_HITS: usize = 256;

pub(super) type CombatSource = Box<dyn Fn() -> Vec<live_proto::game::SimCombatSample> + Send + Sync>;
pub(super) type CombatFeed = Arc<std::sync::OnceLock<CombatSource>>;

pub struct Telemetry {
    /// Shared only with the transport of an immediate, private simulation.
    combat: CombatFeed,
    /// Legacy projection and transport run on the worker. During a film the
    /// game thread owns projection and queues only legacy transport work.
    pub(crate) pipeline: Pipeline,
    film: Option<FilmProjection>,
    pub(crate) hooks: live_proto::film::TelemetryHooks,
    disabled: AtomicBool,
    /// A film can retry a match start whose options or roster were still loading.
    retry_start: AtomicBool,
    last_tick: Mutex<Option<u64>>,
    /// Shots by weapon object datum since the last sample, from the weapon
    /// fire hook.
    shots: Mutex<Shots>,
    /// Damage reports since the last sample, from the damage hook (oldest
    /// dropped past [`MAX_PENDING_HITS`]).
    hits: Mutex<Vec<crate::event::HitRecord>>,
    /// The world scan's buffers, and a second roster list for
    /// [`memory::snapshot_into`], kept between ticks.
    scan: Mutex<(memory::ScanBuffers, Vec<PlayerState>)>,
    /// Finds the loaded map's file for tag names; set by the boot once its
    /// VFS exists.
    maps: Mutex<Option<MapLocator>>,
    /// Names the loaded map's `MapId`; set by the boot with its VFS.
    map_ids: Mutex<Option<MapIdLocator>>,
    /// Names the starting game's playlist; set by the boot with Live.
    playlists: Mutex<Option<PlaylistLocator>>,
    hosted_session: Mutex<Option<HostedSessionLocator>>,
    /// Names the starting game's bot roster; set by the boot with Live.
    bots: Mutex<Option<BotsLocator>>,
}

impl Telemetry {
    pub(crate) fn new(session: Session, sink: impl Sink) -> Self {
        Self {
            pipeline: Pipeline::new(session, sink),
            combat: Default::default(),
            film: None,
            hooks: live_proto::film::TelemetryHooks::EVENTS,
            disabled: AtomicBool::new(false),
            retry_start: AtomicBool::new(false),
            last_tick: Mutex::new(None),
            shots: Mutex::default(),
            hits: Mutex::new(Vec::new()),
            scan: Mutex::default(),
            maps: Mutex::new(None),
            map_ids: Mutex::new(None),
            playlists: Mutex::new(None),
            hosted_session: Mutex::new(None),
            bots: Mutex::new(None),
        }
    }

    /// Finish the ordinary game stream and wait for Live to store its report.
    pub fn finish_sim(&self, report: serde_json::Value) -> Result<(), String> {
        self.submit(Work::FinishSim(report))?;
        self.pipeline.settle();
        self.pipeline.worker_error().map_or(Ok(()), Err)
    }

    /// Sampled on the telemetry worker alongside an immediate simulation's frames.
    pub fn set_sim_combat(&self, read: impl Fn() -> Vec<live_proto::game::SimCombatSample> + Send + Sync + 'static) {
        let _ = self.combat.set(Box::new(read));
    }

    /// Lets each match look up its map's biped and weapon tag names (frame
    /// `species` and `weapon`). Without it those fields are never sent.
    pub fn set_map_locator(&self, locator: MapLocator) {
        *self.maps.lock().unwrap() = Some(locator);
    }

    /// Lets each game say which map exactly it is on (`NewGame.map_id`).
    /// Without it every game names only its map's name.
    pub fn set_map_id_locator(&self, locator: MapIdLocator) {
        *self.map_ids.lock().unwrap() = Some(locator);
    }

    /// The matchmaking playlist the next multiplayer game comes from, or
    /// `None` for custom games. A playlist locator, once set, answers at
    /// each multiplayer start instead.
    pub fn set_playlist(&self, playlist: Option<Playlist>) {
        self.configure(move |s| s.set_playlist(playlist));
    }

    /// Lets each multiplayer game say which playlist it came from
    /// (`NewGame.playlist_id`, docs/live/live-matchmaking.md §8): asked at
    /// the start, so a ranked game is recorded as one.
    pub fn set_playlist_locator(&self, locator: PlaylistLocator) {
        *self.playlists.lock().unwrap() = Some(locator);
    }

    /// Reads the current advertisement when a multiplayer game starts.
    pub fn set_hosted_session_locator(&self, locator: HostedSessionLocator) {
        *self.hosted_session.lock().unwrap() = Some(locator);
    }

    /// Lets each multiplayer game know its Clankers (docs/game/bots.md §6,
    /// stage 6c): asked at the start, so every bot goes out with `bot:
    /// true` and a game with a server roster names the roster's session.
    pub fn set_bots_locator(&self, locator: BotsLocator) {
        *self.bots.lock().unwrap() = Some(locator);
    }

    /// The skulls the next multiplayer games run with (the host's set as
    /// the boot applied it at the game's start; docs/game/skulls.md "In
    /// multiplayer"). Campaign games read the skull array instead.
    pub fn set_multiplayer_skulls(&self, skulls: Option<Vec<u8>>) {
        self.configure(move |s| s.set_multiplayer_skulls(skulls));
    }

    /// Our own skulls (indices 15 and up) the next campaign games run with,
    /// as the boot applied them at the campaign start: they have no byte in
    /// the skull array, so the array alone would miss them.
    pub fn set_campaign_custom_skulls(&self, skulls: Option<Vec<u8>>) {
        self.configure(move |s| s.set_campaign_custom_skulls(skulls));
    }

    /// A Firefight `Score` command as the host's director credited it, at
    /// the guest's `tick`. Live records it as a public game event and the
    /// film as an `ObjectiveScore` of game type `firefight`; with no game
    /// running it is dropped.
    pub fn firefight_score(&self, score: live_proto::firefight::FirefightScore) {
        self.configure(move |s| s.firefight_score(score));
    }

    /// A Firefight match's result, sent before the game ends: the outcome
    /// (a two-team match's winning team or a draw), the team count and
    /// each team's score.
    pub fn firefight_ended(&self, end: live_proto::firefight::FirefightEnded) {
        self.configure(move |s| s.firefight_ended(end));
    }

    /// A setting for the session, after everything already queued. Once
    /// telemetry has failed there is no session to set.
    pub(crate) fn configure(&self, f: impl FnOnce(&mut Session) + Send + 'static) {
        let _ = self.submit(Work::Configure(Box::new(f)));
    }

    /// Where the loaded map's names are, if the host has its file; the
    /// worker loads them and checks they are the guest's.
    fn names(&self, guest: &runtime::Guest, map: &str) -> Option<Names> {
        if !live_proto::game::valid_id(map) {
            return None;
        }
        let file = {
            let maps = self.maps.lock().unwrap();
            (maps.as_ref()?)(guest)?
        };
        Some(Names {
            map: map.to_owned(),
            file,
            scenario: memory::tag_index_scenario(guest).ok(),
        })
    }

    /// The weapon fire hook: one shot of the weapon object `weapon`.
    pub(crate) fn fired(&self, weapon: u32) {
        if self.disabled() {
            return;
        }
        let mut shots = self.shots.lock().unwrap();
        if shots.len() >= MAX_FIRING_WEAPONS && !shots.contains_key(&weapon) {
            shots.clear();
        }
        shots.add(weapon, 1);
    }

    /// The damage hook: one damage report.
    pub(crate) fn damaged(&self, hit: crate::event::HitRecord) {
        if self.disabled() {
            return;
        }
        if let Some(film) = &self.film && film.observer.active()
            && let Some(damage) = film.session.lock().unwrap().film_damage(&hit)
            && film.observer.record(damage).is_err()
        {
            self.dropped_film_sample("damage");
        }
        let mut hits = self.hits.lock().unwrap();
        if hits.len() >= MAX_PENDING_HITS {
            hits.remove(0);
        }
        hits.push(hit);
    }

    pub fn disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
            || self.film.as_ref().is_some_and(|f| !f.transport && !f.observer.active())
    }

    fn disable(&self, reason: impl AsRef<str>) {
        if !self.disabled.swap(true, Ordering::AcqRel) {
            self.dropped_film_sample("telemetry_disabled");
            self.pipeline.log_error(reason.as_ref());
        }
    }

    pub(crate) fn protect(&self, f: impl FnOnce() -> Result<(), String>) {
        if self.disabled() {
            return;
        }
        if self.film.is_none() && let Some(error) = self.pipeline.worker_error() {
            self.disable(error);
            return;
        }
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(Ok(())) => {}
            Ok(Err(_)) if self.film.is_some() => self.dropped_film_sample("telemetry_observer"),
            Ok(Err(error)) => self.disable(error),
            Err(_) => self.disable("panic in observer"),
        }
    }

    /// Guest observations are supplemental to the machine tape. Keep the
    /// original error behaviour of a standalone telemetry session.
    fn missing_sample<T: Default>(
        &self,
        source: &'static str,
        fault: runtime::GuestFault,
    ) -> Result<T, String> {
        if self.film.is_some() {
            self.dropped_film_sample(source);
            Ok(T::default())
        } else {
            Err(format!("read {source}: {fault}"))
        }
    }

    pub(crate) fn start(&self, guest: &runtime::Guest) -> Result<(), String> {
        let read = || {
            let info = memory::game_info(guest)?;
            let (players, teams) = if info.is_match() {
                let players = memory::players(guest)?;
                let teams = memory::team_scores(guest, &players)?;
                (players, teams)
            } else {
                (Vec::new(), Vec::new())
            };
            Ok((info, players, teams))
        };
        let (mut info, players, teams) = match read() {
            Ok(start) => start,
            Err(fault) => {
                self.retry_start.store(self.film.is_some(), Ordering::Relaxed);
                return self.missing_sample("match_start", fault);
            }
        };
        self.retry_start.store(false, Ordering::Relaxed);
        let names = if info.is_match() {
            self.names(guest, &info.map)
        } else {
            None
        };
        let identity = if info.is_match() && live_proto::game::valid_id(&info.map) {
            self.map_ids
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|locate| locate(guest))
        } else {
            None
        };
        let map_id = identity.map(|(id, base)| {
            if let Some(base) = base.filter(|name| live_proto::game::valid_id(name)) {
                info.map = base;
            }
            id
        });
        // The scenario's placements, to tell moved and gone objects from
        // those where the scenario put them; without them frames carry no
        // objects.
        let placements = if info.is_match() {
            memory::placements(guest).unwrap_or_default()
        } else {
            Default::default()
        };
        if info.mode == "multiplayer" {
            let hosted = self.hosted_session.lock().unwrap().as_ref().and_then(|get| get());
            self.configure(move |s| s.hosted_session = hosted);
            let playlist = self
                .playlists
                .lock()
                .unwrap()
                .as_ref()
                .map(|locate| locate());
            if let Some(playlist) = playlist {
                self.configure(move |s| s.set_playlist(playlist));
            }
            let bots = self.bots.lock().unwrap().as_ref().map(|locate| locate());
            // Asked every start, so a game without a roster names none.
            if let Some(bots) = bots {
                self.configure(move |s| s.set_bots(bots));
            }
        }
        let tick = info.tick;
        self.submit(Work::Start(Box::new(Start {
            info,
            players,
            teams,
            map_id,
            placements,
            names,
        })))?;
        self.shots.lock().unwrap().clear();
        self.hits.lock().unwrap().clear();
        *self.last_tick.lock().unwrap() = Some(tick);
        Ok(())
    }

    pub(crate) fn sample(&self, guest: &runtime::Guest) -> Result<(), String> {
        self.observe_tick(guest).map(|_| ())
    }

    /// The tick's observation, queued for the worker; false when the game
    /// tick has not changed since the last one.
    ///
    /// Runs on the game thread every tick (docs/live/live-telemetry.md,
    /// "Threads"): guest reads into a recycled snapshot and a queue push,
    /// no allocation and no system call once steady (`bench.rs`'s
    /// `the_game_threads_tick_does_not_allocate`).
    pub(crate) fn observe_tick(&self, guest: &runtime::Guest) -> Result<bool, String> {
        if self.retry_start.load(Ordering::Relaxed) {
            self.start(guest)?;
            return Ok(false);
        }
        // The in-game clock also advances in the main menu. A film has no
        // roster or active world to observe there.
        if self.film.as_ref().is_some_and(|f| f.session.lock().unwrap().match_clock().is_none()) {
            return Ok(false);
        }
        // An unchanged tick reads nothing else.
        let tick = match memory::game_tick(guest) {
            Ok(tick) => tick,
            Err(fault) => return self.missing_sample("tick_clock", fault),
        };
        if *self.last_tick.lock().unwrap() == Some(tick) {
            return Ok(false);
        }
        let mut scan = self.scan.lock().unwrap();
        let (buffers, roster) = &mut *scan;
        // A snapshot the worker is done with: its lists are empty but keep
        // their allocations, and its roster names this one's.
        let mut snapshot = self.pipeline.snapshot();
        if let Err(e) = memory::snapshot_into(guest, &mut snapshot, roster) {
            self.pipeline.recycle(snapshot);
            return self.missing_sample("tick_snapshot", e);
        }
        {
            let mut last = self.last_tick.lock().unwrap();
            if *last == Some(snapshot.tick) {
                self.pipeline.recycle(snapshot);
                return Ok(false);
            }
            *last = Some(snapshot.tick);
        }
        std::mem::swap(&mut snapshot.shots, &mut *self.shots.lock().unwrap());
        std::mem::swap(&mut snapshot.hits, &mut *self.hits.lock().unwrap());
        // The film's full scene and supplemental legacy frame describe
        // the same simulation boundary, including on non-30 Hz maps.
        let frame_due = self.film.as_ref().map_or_else(
            || snapshot.tick.is_multiple_of(2),
            |f| f.session.lock().unwrap().frame_due(snapshot.tick),
        );
        if frame_due && !snapshot.cinematic {
            let mut world = World {
                actors: std::mem::take(&mut snapshot.actors),
                vehicles: std::mem::take(&mut snapshot.vehicles),
                machines: std::mem::take(&mut snapshot.machines),
                placed: std::mem::take(&mut snapshot.placed),
                bodies: std::mem::take(&mut snapshot.bodies),
                projectiles: std::mem::take(&mut snapshot.projectiles),
                items: std::mem::take(&mut snapshot.items),
            };
            // A failed scan sends none rather than stopping telemetry.
            if memory::world_into(guest, buffers, &mut world).is_err() {
                self.dropped_film_sample("telemetry_world");
            }
            snapshot.actors = world.actors;
            snapshot.vehicles = world.vehicles;
            snapshot.machines = world.machines;
            snapshot.placed = world.placed;
            snapshot.bodies = world.bodies;
            snapshot.projectiles = world.projectiles;
            snapshot.items = world.items;
        }
        // Only a recording needs the full salted object census. The web
        // metadata frame and its object scene are gathered at the same tick.
        if frame_due
            && let Some(film) = &self.film
            && film.observer.active()
        {
            let session = film.session.lock().unwrap();
            if session.match_clock().is_some() {
                // An unreadable/loading object array is not an empty world.
                match film.scene.lock().unwrap().read(guest) {
                    Ok(mut scene) => {
                        session.name_scene(&mut scene);
                        film.observer.scene(scene)?;
                    }
                    Err(_) => film.observer.dropped("scene_census"),
                }
            }
        }
        self.submit(Work::Tick(snapshot))?;
        Ok(true)
    }

    fn end(&self, guest: &runtime::Guest) -> Result<(), String> {
        let latch = guest.u32(layout::GAME_GLOBALS_PTR).and_then(|globals| {
            if globals == 0 { Ok(0) } else { guest.u8(globals.wrapping_add(layout::GAME_ENDED)) }
        });
        match latch {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(fault) => return self.missing_sample("end_latch", fault),
        }
        let snapshot = match memory::snapshot(guest) {
            Ok(snapshot) => snapshot,
            Err(fault) => {
                self.missing_sample::<()>("final_scoreboard", fault)?;
                // Retail has latched game over. Close the real match, leaving
                // the winner and final scores unknown. Live keeps the last good
                // scoreboard in that case.
                let tick = memory::game_tick(guest).ok();
                return self.submit(Work::Configure(Box::new(move |session| {
                    session.end_without_scoreboard(tick);
                })));
            }
        };
        self.submit(Work::End(Box::new(snapshot)))
    }

    fn actor_killed(&self, guest: &runtime::Guest, report: u32, victim: u32) -> Result<(), String> {
        use guest_layout::Datum;
        use sim_objects::{SimEvent, damage};

        let Some(SimEvent::ActorKilled { unit, character, killer, weapon }) =
            damage::actor_killed(guest, report, Datum(victim))
        else {
            return Ok(());
        };
        if memory::cinematic(guest).unwrap_or(false) {
            return Ok(());
        }
        let tick = match memory::game_tick(guest) {
            Ok(tick) => tick,
            Err(fault) => return self.missing_sample("actor_kill_clock", fault),
        };
        self.submit(Work::ActorKilled {
            tick,
            unit: unit.raw(),
            character: character.get(),
            owner: killer.get(),
            weapon,
        })
    }

    /// A bounce off a superbounce crease by the player in slot `player`,
    /// `chain` bounces into its chain, a Monster when `monster`, at the
    /// guest's game tick (docs/game/superbounce.md, "Recording"). The
    /// superbounce mode calls it on the console that records the game; the
    /// session sends it as a `Superbounce` game event and the film records
    /// it with its medals.
    pub fn superbounce(&self, guest: &runtime::Guest, player: u32, chain: u32, monster: bool) {
        self.bounce_work(guest, |tick| Work::Superbounce {
            tick,
            player,
            chain,
            monster,
        });
    }

    /// The player in slot `player` died falling out of or through the map
    /// while trying a bounce, ending a chain of `chain` (0 if none), as the
    /// superbounce mode decides: a `Bounced`
    /// game event, and the `bounced` medal in the film.
    pub fn bounced(&self, guest: &runtime::Guest, player: u32, chain: u32) {
        self.bounce_work(guest, |tick| Work::Bounced {
            tick,
            player,
            chain,
        });
    }

    /// The player in slot `player` teabagged the body of the player in slot
    /// `victim`, as the game's teabag watch decides (docs/game/teabag.md):
    /// a `Teabag` game event, which takes the `incel` medal from both in
    /// the film and on Live. The watch calls it on the console that
    /// records the game.
    pub fn teabag(&self, guest: &runtime::Guest, player: u32, victim: u32) {
        self.bounce_work(guest, |tick| Work::Teabag {
            tick,
            player,
            victim,
        });
    }

    /// The game awarded medal `index` (its row in the game's medal table,
    /// `live_proto::film::events::medals::OFFICIAL`) to the player in
    /// absolute slot `player`, `count` times: a `GameMedal` game event for
    /// each, as the game names the medal. An index past the table (the
    /// game's unused rows) or a count of 0 sends nothing.
    fn medal(&self, guest: &runtime::Guest, index: u32, player: u32, count: u32) {
        let Some(medal) = live_proto::film::events::medals::official_at(index) else {
            return;
        };
        for _ in 0..count.min(16) {
            self.bounce_work(guest, |tick| Work::Medal {
                tick,
                player: player & 0xffff,
                medal: medal.id,
            });
        }
    }

    /// Queues a medal event at the game tick, as a kill: none in a
    /// cinematic.
    fn bounce_work(&self, guest: &runtime::Guest, work: impl FnOnce(u64) -> Work) {
        self.protect(|| {
            if memory::cinematic(guest).unwrap_or(false) {
                return Ok(());
            }
            let tick = match memory::game_tick(guest) {
                Ok(tick) => tick,
                Err(fault) => return self.missing_sample("superbounce_clock", fault),
            };
            self.submit(work(tick))
        });
    }

    fn kill(&self, guest: &runtime::Guest, esp: u32) -> Result<(), String> {
        use guest_layout::damage::{KillingDamage, killing_damage};

        let (Ok(attacker), Ok(victim)) = (
            guest.u32(esp.wrapping_add(4)),
            guest.u32(esp.wrapping_add(8)),
        ) else {
            self.dropped_film_sample("kill");
            return Ok(());
        };
        // Only the low byte of the fourth argument is initialized.
        let credited_damage = guest.u8(esp.wrapping_add(0x10)).ok();
        let tick = match memory::game_tick(guest) {
            Ok(tick) => tick,
            Err(fault) => return self.missing_sample("kill_clock", fault),
        };
        let victim_index = victim & 0xffff;
        let details = memory::players(guest).and_then(|players| {
            let unit = players.iter()
                .find(|p| p.player.index == victim_index)
                .map_or(u32::MAX, |p| p.unit);
            memory::object(guest, unit)
        });
        let object = match details {
            Ok(object) => object,
            Err(fault) => self.missing_sample("kill_position", fault)?,
        };
        let damage = killing_damage(
            guest,
            esp,
            credited_damage.map(|code| KillingDamage {
                code,
                owner_player: guest_layout::Datum(attacker),
            }),
        );
        let source = damage.and_then(|d| {
            let other = d.owner_player.get().is_some_and(|owner| owner != victim);
            layout::kill_source(d.code, other)
        });
        self.submit(Work::Kill(Box::new(Kill {
            tick,
            attacker: (attacker != u32::MAX).then_some(attacker & 0xffff),
            victim: victim_index,
            source,
            object,
            // The scripts' kills in a cutscene are not play.
            cinematic: memory::cinematic(guest).unwrap_or(false),
        })))
    }
}

/// Telemetry's listeners, as a feature (`live-telemetry`): match start,
/// each tick and the end on the `game` and `frame` bridges, and two raw
/// observers (a kill, a weapon firing). All are read-only and run beside
/// the original, so they share their points with the level boot's loaded
/// mark and lockstep's tick hash (docs/engine/feature-engine.md).
pub struct TelemetryFeature(pub Arc<Telemetry>);

impl ::hooks::Feature for TelemetryFeature {
    fn manifest(&self) -> ::hooks::Manifest {
        ::hooks::Manifest::new("live-telemetry", "Live telemetry")
            .doc("docs/live/live-telemetry.md")
    }

    fn build(&self, b: &mut ::hooks::Builder) -> Result<(), ::hooks::BuildError> {
        if self.0.hooks.observers == live_proto::film::TelemetryObservers::EventsAndImpactsV1 {
            crate::impacts::install(b, self.0.clone());
        }
        let t = Arc::clone(&self.0);
        b.game()
            .on_start("match start", move |o| t.protect(|| t.start(&o.guest())));
        let t = Arc::clone(&self.0);
        b.frame()
            .on_tick("sample", move |o| {
                let _cpu = timeline::host::Subsystem::Telemetry.span();
                t.protect(|| t.sample(&o.guest()));
            });
        let t = Arc::clone(&self.0);
        b.game()
            .on_end("end", move |o| t.protect(|| t.end(&o.guest())));
        // stdcall (weapon object datum, barrel index): the first argument.
        // The point is optional: without it frames carry no `shots`.
        if self.0.hooks.weapon_fire {
            let t = Arc::clone(&self.0);
            b.raw()
                .observe_before(&points::WEAPON_FIRE, "shots", move |o| {
                    if let Some(weapon) = o.arg_u32("weapon") {
                        t.fired(weapon);
                    }
                })
                .optional();
        }
        // Custom ABI: EDI = the damage report, the victim object datum on
        // the stack; read before the original applies it. Optional: without
        // it frames carry no `hits`.
        if self.0.hooks.damage_report {
            let t = Arc::clone(&self.0);
            b.raw()
                .observe_before(&points::DAMAGE_REPORT, "hits", move |o| {
                    if t.disabled() { return; }
                    let (Some(report), Some(victim)) = (o.arg_u32("report"), o.arg_u32("victim"))
                    else {
                        t.dropped_film_sample("damage");
                        return;
                    };
                    t.protect(|| {
                        let guest = o.guest();
                        match memory::hit(&guest, report, victim) {
                            Ok(hit) => t.damaged(hit),
                            Err(_) => t.dropped_film_sample("damage"),
                        }
                        t.actor_killed(&guest, report, victim)
                    });
                })
                .optional();
        }
        let t = Arc::clone(&self.0);
        b.raw()
            .observe_before(&points::PLAYER_KILLED, "kill", move |o| {
                if t.disabled() { return; }
                t.protect(|| t.kill(&o.guest(), o.cpu.esp));
            });
        // Custom ABI: EAX the medal's index, ECX the player's slot, EBX the
        // count; read before the original counts it. Every medal the game
        // awards passes here (docs/live/live-gamestate.md, "Medals").
        // Optional: without it a film and Live have no Halo 2 medals.
        let t = Arc::clone(&self.0);
        b.raw()
            .observe_before(&points::GAME_MEDAL, "medal", move |o| {
                if t.disabled() { return; }
                let (Some(index), Some(player), Some(count)) =
                    (o.arg_u32("medal"), o.arg_u32("player"), o.arg_u32("count"))
                else {
                    t.dropped_film_sample("medal");
                    return;
                };
                t.medal(&o.guest(), index, player, count);
            })
            .optional();
        // Observers only: nothing here writes the guest or feeds back into
        // it. The session is an outbound record of the run; a load winds
        // the engine's tick, which the session reads as a campaign revert
        // (a `cut`) or a jump, and the run goes on (docs/runtime/snapshots.md).
        b.snapshot(::hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds the configured observer to `registry`. `Ok(None)` means disabled.
/// Nothing is hooked until the plan is installed.
pub fn install(
    registry: &mut ::hooks::Registry,
    config: &TelemetryConfig,
) -> Result<Option<Arc<Telemetry>>, String> {
    use live_proto::film::TelemetryHooks;
    let hooks = if config.enabled { TelemetryHooks::EVENTS } else { TelemetryHooks::OFF };
    install_film(registry, config, None, hooks)
}

/// The Live account the game is signed in with: the one whose key signs the
/// events. Offline capture has none.
fn signed_in(config: &TelemetryConfig) -> Option<live_proto::Xuid> {
    match &config.sink {
        TelemetrySinkConfig::Server { account, .. } => live_proto::Xuid::parse(&account.xuid).ok(),
        TelemetrySinkConfig::Discard { .. }
        | TelemetrySinkConfig::File(_)
        | TelemetrySinkConfig::Simulation { .. }
        | TelemetrySinkConfig::Host { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{GameInfo, PlayerState};
    use crate::memory::fixture;

    #[test]
    fn host_telemetry_has_no_signed_in_player() {
        let secret = "test-private-host-credential";
        let config = TelemetryConfig::for_host("https://live.example".into(), secret.into(), None)
            .unwrap();
        assert!(config.enabled);
        assert!(signed_in(&config).is_none());
        assert!(!format!("{config:?}").contains(secret));
        assert!(TelemetryConfig::for_host("http://live.example".into(), secret.into(), None).is_err());
    }

    #[test]
    fn observer_failure_disables_without_panicking() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-failure-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            JsonlSink::create(&path).unwrap(),
        );
        let guest = runtime::Guest::new();
        telemetry.protect(|| telemetry.start(&guest));
        assert!(telemetry.disabled());
        telemetry.protect(|| telemetry.start(&guest));
        drop(telemetry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn successful_start_publishes_current_roster() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-start-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            JsonlSink::create(&path).unwrap(),
        );
        let mut guest = fixture::memory();
        guest
            .set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 2)
            .unwrap();
        guest
            .set_u16(
                fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE,
                30,
            )
            .unwrap();
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 4).unwrap();
        guest
            .write_bytes(layout::CACHE_MAP_NAME, b"lockout\0", 1)
            .unwrap();
        fixture::put_player(&mut guest, 0, "A", 0, true, u32::MAX);
        telemetry.protect(|| telemetry.start(&guest));
        assert!(!telemetry.disabled());
        assert!(telemetry.pipeline.session().game_id().is_some());
        drop(telemetry);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("GameStarted"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_map_id_locator_names_the_loaded_map() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-mapid-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            JsonlSink::create(&path).unwrap(),
        );
        let id = live_proto::maps::MapId([0x5c; 32]);
        let asked = std::sync::Arc::new(Mutex::new(Vec::new()));
        let seen = asked.clone();
        telemetry.set_map_id_locator(Box::new(move |guest| {
            let map = memory::loaded_map_name(guest).ok()?;
            seen.lock().unwrap().push(map.clone());
            (map == "mod-lockout").then_some((id, Some("lockout".into())))
        }));
        let mut guest = fixture::memory();
        guest
            .set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 2)
            .unwrap();
        guest
            .set_u16(
                fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE,
                30,
            )
            .unwrap();
        guest
            .write_bytes(layout::CACHE_MAP_NAME, b"mod-lockout\0", 1)
            .unwrap();
        fixture::put_player(&mut guest, 0, "A", 0, true, u32::MAX);
        telemetry.protect(|| telemetry.start(&guest));
        drop(telemetry);
        assert_eq!(*asked.lock().unwrap(), ["mod-lockout"]);
        let text = std::fs::read_to_string(&path).unwrap();
        let started = text
            .lines()
            .find(|l| l.contains("GameStarted"))
            .expect("a GameStarted");
        assert!(started.contains(&id.to_string()), "{started}");
        assert!(started.contains("\"map\":\"lockout\""), "{started}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_playlist_locator_makes_a_multiplayer_start_a_matchmaking_game() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-playlist-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            JsonlSink::create(&path).unwrap(),
        );
        let asked = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let count = asked.clone();
        telemetry.set_playlist_locator(Box::new(move || {
            count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Some(Playlist {
                name: None,
                id: "1".into(),
                session_id: Some("0123456789abcdef".into()),
                clankers: false,
                frames_from_host: false,
            })
        }));
        let mut guest = fixture::memory();
        guest
            .set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 2)
            .unwrap();
        guest
            .set_u16(
                fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE,
                30,
            )
            .unwrap();
        guest
            .write_bytes(layout::CACHE_MAP_NAME, b"lockout\0", 1)
            .unwrap();
        fixture::put_player(&mut guest, 0, "A", 0, true, u32::MAX);
        telemetry.protect(|| telemetry.start(&guest));
        drop(telemetry);
        assert_eq!(asked.load(std::sync::atomic::Ordering::Relaxed), 1);
        let text = std::fs::read_to_string(&path).unwrap();
        let started: serde_json::Value = text
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .find(|v| v["type"] == "GameStarted")
            .expect("a GameStarted");
        let game = &started["data"];
        assert_eq!(game["kind"], "matchmaking", "{started}");
        assert_eq!(game["playlist_id"], "1");
        assert_eq!(game["session_id"], "0123456789abcdef");
        assert!(game.get("playlist").is_none_or(|p| p.is_null()));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn salted_kill_datums_become_player_slots() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-kill-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            JsonlSink::create(&path).unwrap(),
        );
        let mut guest = fixture::memory();
        guest
            .set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 2)
            .unwrap();
        guest
            .set_u16(
                fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE,
                30,
            )
            .unwrap();
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 9).unwrap();
        guest
            .write_bytes(layout::CACHE_MAP_NAME, b"lockout\0", 1)
            .unwrap();
        fixture::put_player(&mut guest, 0, "A", 0, true, 0x90010000);
        fixture::put_player(&mut guest, 1, "B", 0, false, fixture::object_datum(1));
        fixture::put_object(&mut guest, 0, u32::MAX);
        fixture::put_object(&mut guest, 1, u32::MAX);
        let object = memory::object(&guest, fixture::object_datum(1))
            .unwrap()
            .unwrap();
        assert_eq!(object.pos, [1.0, 2.0, 3.0]);
        telemetry.protect(|| telemetry.start(&guest));
        let esp = 0x1a0000;
        guest.set_u32(esp, 0x2222).unwrap();
        guest.set_u32(esp + 4, 0x80010000).unwrap();
        guest.set_u32(esp + 8, 0x80010001).unwrap();
        guest.set_u8(esp + 0x10, (1 << 6) | 10).unwrap();
        telemetry.protect(|| telemetry.kill(&guest, esp));
        guest.set_u32(esp + 8, 0x80010000).unwrap();
        guest.set_u8(esp + 0x10, 21).unwrap();
        telemetry.protect(|| telemetry.kill(&guest, esp));
        // A different player earns credit for earlier rifle damage, but
        // the final hit was an unowned collision with the world.
        use guest_layout::damage::{DEPLETING, DamageReport};
        use points::kill_credit::{RETURN_TO_ATTRIBUTION, RETURN_TO_DAMAGE_REPORT};
        let report = 0x1b0000;
        guest.set_u32(esp + 8, 0x80010001).unwrap();
        guest.set_u8(esp + 0x10, (1 << 6) | 10).unwrap();
        guest.set_u32(esp, RETURN_TO_ATTRIBUTION).unwrap();
        guest.set_u32(esp + 0x74, RETURN_TO_DAMAGE_REPORT).unwrap();
        guest.set_u32(esp + 0x1c, report).unwrap();
        guest.set_u32(report + DamageReport::FLAGS.offset, DEPLETING).unwrap();
        guest.set_u8(report + DamageReport::CODE.offset, 2).unwrap();
        guest.set_u32(report + DamageReport::OWNER_PLAYER.offset, u32::MAX).unwrap();
        telemetry.protect(|| telemetry.kill(&guest, esp));
        // In a cutscene the scripts' kills are not counted, and the tick's
        // frame is a marked, empty one.
        fixture::set_cinematic(&mut guest, true);
        guest
            .set_u32(fixture::TIME + layout::TIME_TICK, 10)
            .unwrap();
        telemetry.protect(|| telemetry.kill(&guest, esp));
        telemetry.protect(|| telemetry.sample(&guest));
        assert!(!telemetry.disabled());
        drop(telemetry);
        let lines: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let frames: Vec<&serde_json::Value> = lines
            .iter()
            .filter(|event| event["type"] == "GameFrame")
            .collect();
        assert_eq!(frames.len(), 1, "{lines:?}");
        assert_eq!(frames[0]["data"]["cinematic"], true);
        assert_eq!(frames[0]["data"]["tick"], 10);
        assert_eq!(frames[0]["data"]["players"], serde_json::json!([]));
        let events: Vec<serde_json::Value> = lines
            .into_iter()
            .filter(|event: &serde_json::Value| event["type"] == "PlayerKilled")
            .collect();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0]["data"]["victim"], 1);
        assert_eq!(events[0]["data"]["kind"], "kill");
        assert_eq!(events[0]["data"]["weapon"], "battle_rifle");
        assert_eq!(events[0]["data"]["headshot"], true);
        assert_eq!(events[0]["data"]["pos"], serde_json::json!([1.0, 2.0, 3.0]));
        assert_eq!(events[1]["data"]["victim"], 0);
        assert_eq!(events[1]["data"]["kind"], "suicide");
        assert_eq!(events[1]["data"]["weapon"], "frag_grenade");
        assert_eq!(events[1]["data"]["headshot"], false);
        assert_eq!(events[2]["data"]["killer"], 0);
        assert_eq!(events[2]["data"]["weapon"], "collision");
        assert!(events[2]["data"].get("headshot").is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn fired_shots_reach_the_holders_frame() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-shots-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            JsonlSink::create(&path).unwrap(),
        );
        let mut guest = fixture::memory();
        guest
            .set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 2)
            .unwrap();
        guest
            .set_u16(
                fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE,
                30,
            )
            .unwrap();
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 1).unwrap();
        guest
            .write_bytes(layout::CACHE_MAP_NAME, b"lockout\0", 1)
            .unwrap();
        fixture::put_player(&mut guest, 0, "A", 0, true, fixture::object_datum(1));
        fixture::put_object(&mut guest, 1, u32::MAX);
        fixture::put_unit(&mut guest, 1, 0xe100_0010, [1.0, 0.0, 0.0], 0);
        fixture::put_weapon(&mut guest, 5, 1, 0, 0xe100_0020);
        telemetry.protect(|| telemetry.start(&guest));
        // Shots before the match's first sample are the previous match's.
        telemetry.fired(fixture::object_datum(5));
        telemetry.protect(|| telemetry.start(&guest));
        telemetry.fired(fixture::object_datum(5));
        telemetry.fired(fixture::object_datum(5));
        telemetry.fired(fixture::object_datum(9)); // nobody's weapon
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 2).unwrap();
        telemetry.protect(|| telemetry.sample(&guest));
        assert!(telemetry.shots.lock().unwrap().is_empty(), "drained");
        telemetry.fired(fixture::object_datum(5));
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 4).unwrap();
        telemetry.protect(|| telemetry.sample(&guest));
        assert!(!telemetry.disabled());
        drop(telemetry);
        let frames: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .filter(|row: &serde_json::Value| row["type"] == "GameFrame")
            .collect();
        // The worker may fold one tick into the other; shots then add up.
        let shots: u64 = frames
            .iter()
            .map(|f| f["data"]["players"][0]["shots"].as_u64().unwrap_or(0))
            .sum();
        assert_eq!(shots, 3, "{frames:?}");
        assert_eq!(
            frames[0]["data"]["players"][0]["aim"],
            serde_json::json!([1.0, 0.0, 0.0])
        );
        assert!(
            frames[0]["data"]["players"][0].get("weapon").is_none(),
            "no names without a map file"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn campaign_play_makes_one_game_with_actor_frames() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-campaign-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            JsonlSink::create(&path).unwrap(),
        );
        let mut guest = fixture::memory();
        guest
            .set_u32(fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TYPE, 1)
            .unwrap();
        guest
            .set_u16(
                fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_TICK_RATE,
                30,
            )
            .unwrap();
        guest
            .set_u16(
                fixture::GLOBALS + layout::OPTIONS + layout::OPTIONS_DIFFICULTY,
                3,
            )
            .unwrap();
        // Campaign has no statborg.
        guest
            .set_u32(fixture::ENGINE + layout::ENGINE_STATBORG, 0)
            .unwrap();
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 1).unwrap();
        guest
            .write_bytes(layout::CACHE_MAP_NAME, b"03a_oldmombasa\0", 1)
            .unwrap();
        fixture::put_player(&mut guest, 0, "Chief", 1, true, fixture::object_datum(1));
        fixture::put_object(&mut guest, 1, u32::MAX);
        fixture::put_actor(&mut guest, 2, 3, [4.0, 0.0, 0.0]);
        fixture::put_vehicle(&mut guest, 3);
        fixture::put_machine(&mut guest, 4, 2, 0.5);
        guest
            .set_u16(fixture::GLOBALS + layout::GLOBALS_ACTIVE_BSP, 1)
            .unwrap();
        telemetry.protect(|| telemetry.start(&guest));
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 2).unwrap();
        telemetry.protect(|| telemetry.sample(&guest));
        // A checkpoint revert re-activates the same map.
        telemetry.protect(|| telemetry.start(&guest));
        guest.set_u32(fixture::TIME + layout::TIME_TICK, 4).unwrap();
        // The AI hits the player for a fifth of their body.
        telemetry.damaged(crate::event::HitRecord {
            code: 7,
            effect: 0,
            owner_player: u32::MAX,
            owner_object: fixture::object_datum(2),
            victim: fixture::object_datum(1),
            pos: Some([0.0, 0.0, 0.0]),
            body: 0.2,
            shield: 0.0,
            tick: None,
        });
        telemetry.protect(|| telemetry.sample(&guest));
        assert!(!telemetry.disabled());
        drop(telemetry);
        let rows: Vec<serde_json::Value> = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let started: Vec<_> = rows.iter().filter(|r| r["type"] == "GameStarted").collect();
        assert_eq!(started.len(), 1, "{rows:?}");
        assert_eq!(started[0]["data"]["mode"], "campaign");
        assert_eq!(started[0]["data"]["kind"], "campaign");
        assert_eq!(started[0]["data"]["difficulty"], "legendary");
        assert!(rows.iter().all(|r| r["type"] != "GameEnded"));
        let frame = rows
            .iter()
            .rev()
            .find(|r| r["type"] == "GameFrame")
            .unwrap();
        let actor = &frame["data"]["actors"][0];
        assert_eq!(actor["id"], format!("{:08x}", fixture::object_datum(2)));
        assert_eq!(actor["team"], "covenant");
        assert_eq!(actor["pos"], serde_json::json!([4.0, 0.0, 0.0]));
        assert_eq!(actor["species"], "unknown", "no map file in tests");
        let data = &frame["data"];
        assert_eq!(data["bsp"], 1);
        assert_eq!(data["vehicles"][0]["occupied"], false);
        // Machines go out in the game's first frame (and then only when they
        // move); the sink may have folded that frame into a later one.
        let frame = &rows.iter().find(|r| r["type"] == "GameFrame").unwrap()["data"];
        assert_eq!(
            frame["machine_positions"],
            serde_json::json!([{ "i": 2, "p": 128 }])
        );
        assert_eq!(frame["machines"][0].get("node"), None, "no name, no node");
        assert_eq!(
            data["hits"][0]["from"],
            format!("{:08x}", fixture::object_datum(2))
        );
        assert_eq!(data["hits"][0]["to"], "p0");
        assert_eq!(data["hits"][0]["weapon"], "needler");
        assert_eq!(data["players"][0]["stance"], "stand");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn fire_counts_are_bounded() {
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-bounded-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let telemetry = Telemetry::new(
            Session::new("test", None),
            JsonlSink::create(&path).unwrap(),
        );
        for weapon in 0..(MAX_FIRING_WEAPONS as u32 + 10) {
            telemetry.fired(weapon);
        }
        assert!(telemetry.shots.lock().unwrap().len() <= MAX_FIRING_WEAPONS);
        drop(telemetry);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn disabled_config_is_a_noop() {
        assert!(!TelemetryConfig::default().enabled);
        let info = GameInfo {
            map: "lockout".into(),
            mode: "main_menu".into(),
            difficulty: None,
            skulls: None,
            variant: None,
            team_play: false,
            score_limit: None,
            time_limit_s: None,
            tick_rate: 30,
            tick: 0,
        };
        let mut session = Session::new("test", None);
        session.start(&info, Vec::<PlayerState>::new());
        assert!(session.take_output().is_empty());
    }

    fn fixture_runtime() -> Arc<Runtime> {
        use runtime::guest::Guest;
        use runtime::{Dispatch, RegisteredModule};
        let mut dispatch = Dispatch::default();
        // SAFETY: the fixture's tables, handlers and probes follow its ABI.
        let registered = unsafe {
            dispatch.register_generated_module(
                &runtime_test_probe::h2gen_fixture_module,
                runtime_test_probe::h2gen_fixture_module_abi_version,
            )
        };
        assert!(matches!(registered, Ok(RegisteredModule { .. })));
        Runtime::new(Guest::new(), dispatch)
    }

    /// Installs telemetry's plan on the fixture, with `fire` standing in for
    /// the weapon fire entry; returns the override names at the five points
    /// and the install notes.
    fn install_on_fixture(fire: u32) -> (Vec<Option<String>>, Vec<String>) {
        let runtime = fixture_runtime();
        let path = std::env::temp_dir().join(format!(
            "h2-telemetry-install-{}.jsonl",
            live_proto::sign::random_id()
        ));
        let config = TelemetryConfig {
            enabled: true,
            sink: TelemetrySinkConfig::File(path.clone()),
        };
        // The fixture module's entries stand in for the retail hooks.
        let at = [
            (&points::GAME_ACTIVATE, 0x40_1000),
            (&points::GAME_TICK, 0x40_2000),
            (&points::GAME_END, 0x40_3000),
            (&points::PLAYER_KILLED, 0x40_4000),
            (&points::WEAPON_FIRE, fire),
            // Not an entry of the fixture module: the optional damage point
            // is dropped with a note.
            (&points::DAMAGE_REPORT, 0x40_6000),
            // Nor is the optional medal point.
            (&points::GAME_MEDAL, 0x40_7000),
        ];
        let (done, wait) = std::sync::mpsc::channel();
        let rt = Arc::clone(&runtime);
        std::thread::spawn(move || {
            let mut r = ::hooks::Registry::new(::hooks::Switches::defaults());
            assert!(install(&mut r, &config).unwrap().is_some());
            let plan = r.build(&::hooks::Remap::new(&at)).unwrap();
            let installed = plan.install(&rt).unwrap();
            let names = rt.edit_dispatch(|d| {
                at.iter()
                    .map(|(_, a)| d.override_name(*a).map(str::to_owned))
                    .collect()
            });
            let _ = done.send((names, installed.notes));
        });
        let out = wait
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("install deadlocked");
        let _ = std::fs::remove_file(path);
        out
    }

    /// Regression: install used to resolve originals under the dispatch write
    /// lock and hung boot forever whenever Live was on.
    #[test]
    fn install_on_a_real_runtime_returns() {
        let (names, notes) = install_on_fixture(0x40_3005);
        assert_eq!(
            names,
            [
                Some("plan:GAME_ACTIVATE[live-telemetry]".into()),
                Some("plan:GAME_TICK[live-telemetry]".into()),
                Some("plan:GAME_END[live-telemetry]".into()),
                Some("plan:PLAYER_KILLED[live-telemetry]".into()),
                Some("plan:WEAPON_FIRE[live-telemetry]".into()),
                None,
                None,
            ]
        );
        // The fixture has neither the damage point nor the medal point.
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("hits dropped")), "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("medal dropped")), "{notes:?}");
    }

    /// The weapon fire hook is optional: an image without that entry still
    /// gets the rest of telemetry.
    #[test]
    fn install_without_the_fire_entry_still_installs() {
        let (names, notes) = install_on_fixture(0x40_5000);
        assert!(names[..4].iter().all(Option::is_some), "{names:?}");
        assert_eq!(names[4], None);
        assert_eq!(notes.len(), 3, "{notes:?}");
        assert!(
            notes.iter().any(|n| n.contains("shots dropped")),
            "{notes:?}"
        );
    }
}
