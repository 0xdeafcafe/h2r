use std::collections::BTreeMap;

use live_proto::game::{
    Frame, FrameActor, FrameAnim, FrameHit, FrameItem, FrameMachine, FrameObject, FramePlayer,
    FrameProjectile, FrameVehicle, GameEnded, MAX_FRAME_ACTORS, MAX_FRAME_HITS, MAX_FRAME_ITEMS,
    MAX_FRAME_MACHINES, MAX_FRAME_OBJECTS, MAX_FRAME_PROJECTILES, MAX_FRAME_SEAT, MAX_FRAME_SHIELD,
    MAX_FRAME_SHOTS, MAX_FRAME_SPEED, MAX_FRAME_VEHICLES, MAX_MACHINE_POSITIONS,
    MAX_PROJECTILE_SPEED, MachinePosition, ObjectKind, PlayerKilled, PlayerLeft, ScoreChanged,
};

use crate::event::{
    GameEvent, GameInfo, HitRecord, MachineState, ObjectState, Output, PlacedObject, Placements,
    PlayerState, Pose, Snapshot, VehicleState,
};
use crate::layout;
use crate::memory::yaw;
use crate::motion::{self, UnitMemory};
use crate::tags::TagNames;

/// Shots a player's held weapon fired since their previous frame.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PendingShots {
    weapon: u32,
    fired: Fired,
}

/// Shots since the last frame and the engine ticks they were on.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Fired {
    count: u32,
    /// Bit `k`: a shot on engine tick `at - k`.
    mask: u32,
    at: u64,
}

impl Fired {
    /// The mask as of engine tick `tick`: bit `k` a shot on `tick - k`
    /// (shots past 32 ticks drop off; none before `at`).
    fn mask_at(&self, tick: u64) -> u32 {
        match tick.checked_sub(self.at) {
            Some(d) if d < 32 => self.mask << d,
            _ => 0,
        }
    }

    /// `n` shots on engine tick `tick`.
    fn add(&mut self, tick: u64, n: u32) {
        if n == 0 {
            return;
        }
        self.mask = self.mask_at(tick) | 1;
        self.at = tick;
        self.count = self.count.saturating_add(n);
    }

    /// The frame's `shots` and `fire` at engine tick `tick`.
    fn frame(&self, tick: u64) -> (Option<u32>, Option<u32>) {
        let shots = Some(self.count.min(MAX_FRAME_SHOTS)).filter(|n| *n > 0);
        let fire = shots.and(Some(self.mask_at(tick)).filter(|m| *m != 0));
        (shots, fire)
    }
}

/// Weapons whose shots wait for the next frame's actors; past this many the
/// counts are dropped, not grown.
const MAX_ACTOR_WEAPONS: usize = 512;

/// Name for a vehicle or actor whose tag name is not known.
const UNKNOWN: &str = "unknown";

/// Most dead bodies a frame carries (with the `dead` stance, after the
/// living actors, within [`MAX_FRAME_ACTORS`]), nearest to a player first.
pub const MAX_FRAME_BODIES: usize = 32;

struct Match {
    game_id: String,
    map: String,
    campaign: bool,
    frames_from_host: bool,
    /// The engine's tick at the last sample.
    tick: u64,
    /// Added to the engine's tick for every tick the match sends. A
    /// campaign checkpoint revert or mission restart winds the engine's
    /// clock back within the same game; the game's ticks keep counting
    /// (docs/live/live-telemetry.md, "Campaign"). Even, so frames
    /// stay on even ticks.
    tick_offset: u64,
    /// The latest tick the match has sent (offset applied).
    sent_tick: u64,
    /// The engine tick a campaign revert or restart came back to, for the
    /// next frame's `cut`.
    cut: Option<u64>,
    tick_rate: u64,
    team_play: bool,
    players: Vec<PlayerState>,
    last_emitted_scores: Vec<live_proto::game::PlayerScore>,
    last_emitted_teams: Vec<live_proto::game::TeamScore>,
    last_score_tick: u64,
    ended: bool,
    /// The loaded map's biped and weapon names, when the host found them.
    tags: Option<TagNames>,
    /// By player index; reset when the held weapon changes or the player
    /// has no weapon (dead, or none held).
    shots: BTreeMap<u32, PendingShots>,
    /// Every weapon's shots since the last frame, for actors (read only on
    /// frame ticks, so their held weapon is looked up then).
    actor_shots: BTreeMap<u32, Fired>,
    /// Each player's and sent actor's unit at the previous frame, by unit
    /// datum, for `speed` and `actions`.
    units: BTreeMap<u32, UnitMemory>,
    /// Each machine as last sent, by datum: frames send only those that
    /// moved.
    machines_sent: BTreeMap<u32, MachineState>,
    machine_positions_sent: BTreeMap<u16, u8>,
    /// Damage reports since the last frame (newest [`MAX_FRAME_HITS`]).
    hits: Vec<HitRecord>,
    /// The scenario's placements, for frame `objects`.
    placements: Placements,
    /// Scratch for [`frame_objects`], kept to spare an allocation a frame.
    found: Vec<u32>,
    /// The last sample was in a HaloScript cinematic: kills are not
    /// counted until it ends.
    cinematic: bool,
    /// Each player's unit while alive, and where it stood, by player index:
    /// a dead player is placed at their body (else where they fell).
    player_units: BTreeMap<u32, (u32, [f64; 3], f64)>,
    /// Every unit a player has had whose body is still in the world: those
    /// bodies are the players', not AI ones (the viewer leaves its own).
    player_bodies: std::collections::BTreeSet<u32>,
}

impl Match {
    /// The game's tick for the engine's `tick`.
    fn clock(&self, tick: u64) -> u64 {
        tick.saturating_add(self.tick_offset)
    }

    fn mark_cut(&mut self, tick: u64) {
        if self.clock(tick) <= self.sent_tick {
            let offset = self.sent_tick + 2 - tick;
            self.tick_offset = offset + offset % 2;
        }
        self.tick = tick;
        self.cut = Some(tick);
        self.machines_sent.clear();
        self.machine_positions_sent.clear();
        self.last_score_tick = tick;
        self.shots.clear();
        self.actor_shots.clear();
        self.units.clear();
        self.hits.clear();
    }
}

pub struct Session {
    host: String,
    console: Option<String>,
    /// The Live account signed in on local player slot 0, if any: that
    /// player's roster entries carry its XUID.
    signed_in: Option<live_proto::Xuid>,
    sim: Option<live_proto::game::SimGame>,
    current: Option<Match>,
    /// Set while the console is in a matchmaking game.
    playlist: Option<Playlist>,
    /// The server's bot roster the next multiplayer games run with.
    bots: Option<Bots>,
    /// The `MapId` of the map the next game starts on, when a package
    /// serves it (None: the disc's own map).
    map_id: Option<live_proto::maps::MapId>,
    /// The skulls the next multiplayer games run with (the host's set, as
    /// the boot applied it); campaign games read theirs from the array.
    multiplayer_skulls: Option<Vec<u8>>,
    /// Our own skulls (15 and up) the next campaign games run with; the
    /// array holds only the engine's fifteen.
    campaign_custom_skulls: Option<Vec<u8>>,
    output: Vec<Output>,
    /// Even a joined client can make an offline film of its local view.
    pub(crate) filming: bool,
}

/// A matchmaking game's playlist, as its record names it
/// (`NewGame.playlist`, `playlist_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    /// The hopper's title, when the console knows it (the server fills it
    /// in from its hopper file otherwise).
    pub name: Option<String>,
    /// The hopper descriptor index.
    pub id: String,
    /// The Live session it is played in (16 hex digits), which its
    /// arbitration round is registered under (docs/live/live-matchmaking.md
    /// §8.4).
    pub session_id: Option<String>,
    /// A clanker hopper (docs/game/bots.md §8): the game is a
    /// Clankermatch, recorded as `kind: "clankermatch"`.
    pub clankers: bool,
    /// Live assigned this client to a session where the host uploads the frames.
    pub frames_from_host: bool,
}

/// The host's phantoms, identified by machine ID and recorded with
/// `bot: true`. A bot carries its server-issued XUID only while its
/// roster is in force.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bots {
    /// The Live session a server roster was given for (16 hex digits), while
    /// one is in force: a custom game names it as `NewGame.session_id`,
    /// which the server finds the roster by. `None`: no roster, and no bot
    /// carries an xuid.
    pub session_id: Option<String>,
    /// Each phantom bot's machine id and its xuid. Host-made xuids
    /// (`live_proto::bots::is_local_xuid`) are never sent; a rostered one
    /// is sent only with `session_id`.
    pub machines: Vec<([u8; 6], live_proto::Xuid)>,
}

impl Bots {
    /// A phantom can join after the game starts. Its machine ID identifies
    /// it even if it was absent from the roster at the start.
    fn plays(&self, p: &PlayerState) -> bool {
        p.local_user.is_none()
            && (live_proto::bots::is_phantom_machine(p.machine)
                || self.machines.iter().any(|(m, _)| *m == p.machine))
    }

    /// The rostered xuid of a remote player on `machine`, if one of the
    /// roster's bots plays there and the roster is in force.
    fn xuid(&self, machine: [u8; 6]) -> Option<live_proto::Xuid> {
        self.session_id.as_ref()?;
        self.machines
            .iter()
            .find(|(m, x)| *m == machine && x.0 != 0 && !live_proto::bots::is_local_xuid(*x))
            .map(|(_, x)| *x)
    }
}

impl Session {
    pub fn new(host: impl Into<String>, console: Option<String>) -> Self {
        Self {
            host: host.into(),
            console,
            signed_in: None,
            sim: None,
            current: None,
            playlist: None,
            bots: None,
            map_id: None,
            multiplayer_skulls: None,
            campaign_custom_skulls: None,
            output: Vec::new(),
            filming: false,
        }
    }

    /// Links the player on local slot 0 to the signed-in Live account.
    pub fn with_signed_in(mut self, xuid: Option<live_proto::Xuid>) -> Self {
        self.signed_in = xuid;
        self
    }

    pub fn with_sim(mut self, sim: live_proto::game::SimGame) -> Self {
        self.sim = Some(sim);
        self
    }

    pub fn is_sim(&self) -> bool {
        self.sim.is_some()
    }

    /// Sets the signed-in account's XUID on the player who plays from local
    /// slot 0. Other players keep whatever the game said (nothing, today).
    fn link(&self, players: &mut [PlayerState]) {
        link(self.signed_in, self.bots.as_ref(), players, self.is_sim());
    }

    /// The server's bot roster the next multiplayer games run with
    /// (`None`: no roster; bots, if any, are sent as guests).
    pub fn set_bots(&mut self, bots: Option<Bots>) {
        self.bots = bots;
    }

    /// The matchmaking playlist multiplayer games start from; `None` makes
    /// them custom games.
    pub fn set_playlist(&mut self, playlist: Option<Playlist>) {
        self.playlist = playlist;
    }

    /// The loaded map's `MapId` for the games that start from now on
    /// (`NewGame.map_id`, docs/game/maps.md §5).
    pub fn set_map_id(&mut self, map_id: Option<live_proto::maps::MapId>) {
        self.map_id = map_id;
    }

    /// The skulls multiplayer games run with from now on
    /// (`NewGame.skulls`): only ones that work there are kept.
    pub fn set_multiplayer_skulls(&mut self, skulls: Option<Vec<u8>>) {
        self.multiplayer_skulls = skulls
            .and_then(live_proto::skulls::normalize)
            .filter(|s| live_proto::skulls::multiplayer_ok(s));
    }

    /// Our skulls campaign games run with from now on (`NewGame.skulls`,
    /// beside the array's): only indices past the engine's are kept.
    pub fn set_campaign_custom_skulls(&mut self, skulls: Option<Vec<u8>>) {
        self.campaign_custom_skulls = skulls
            .map(|s| {
                s.into_iter()
                    .filter(|i| *i >= live_proto::skulls::COUNT)
                    .collect::<Vec<_>>()
            })
            .and_then(live_proto::skulls::normalize);
    }

    pub fn take_output(&mut self) -> Vec<Output> {
        std::mem::take(&mut self.output)
    }

    pub(crate) fn live_frames(&self) -> bool {
        self.current.as_ref().is_some_and(|m| !m.frames_from_host)
    }

    pub(crate) fn match_clock(&self) -> Option<u64> {
        self.current.as_ref().filter(|m| !m.ended).map(|m| m.clock(m.tick))
    }

    pub(crate) fn frame_due(&self, tick: u64) -> bool {
        let Some(m) = self.current.as_ref().filter(|_| self.filming) else {
            return tick.is_multiple_of(2);
        };
        // Use the actual engine rate. At 30 Hz this is every second tick;
        // at other rates the integer schedule still gives 15 frames/second.
        let phase = tick % m.tick_rate;
        let samples = m.tick_rate.min(15);
        phase == 0 || phase * samples / m.tick_rate != (phase - 1) * samples / m.tick_rate
    }

    pub(crate) fn match_clock_at(&self, tick: u64) -> Option<u64> {
        self.current.as_ref().filter(|m| !m.ended).map(|m| m.clock(tick))
    }

    pub(crate) fn film_damage(&self, hit: &HitRecord) -> Option<live_proto::film::events::Record> {
        let current = self.current.as_ref().filter(|m| !m.ended && !m.cinematic)?;
        let tick = hit.tick.unwrap_or(current.tick);
        let player = |unit| {
            current.players.iter().find(|p| p.unit == unit).map(|p| p.player.index)
        };
        Some(live_proto::film::events::Record {
            tick: current.clock(tick),
            event: live_proto::film::events::Event::Damage(
                frame_hit(hit, tick, player, current.tags.as_ref())?
            ),
        })
    }

    pub(crate) fn film_weapon(&self, definition: u32) -> Option<String> {
        self.current.as_ref()?.tags.as_ref()?.weapon(definition)
    }

    pub(crate) fn name_scene(&self, scene: &mut live_proto::film::objects::Scene) {
        if let Some(tags) = self.current.as_ref().and_then(|m| m.tags.as_ref()) {
            tags.name_film_scene(scene);
        }
    }

    #[cfg(test)]
    pub fn game_id(&self) -> Option<&str> {
        self.current.as_ref().map(|m| m.game_id.as_str())
    }

    /// The tick the match was last sampled at.
    #[cfg(test)]
    pub fn current_tick(&self) -> Option<u64> {
        self.current.as_ref().map(|m| m.tick)
    }

    #[cfg(test)]
    pub fn start(&mut self, info: &GameInfo, players: Vec<PlayerState>) {
        self.start_with_teams(info, players, Vec::new());
    }

    pub fn start_with_teams(
        &mut self,
        info: &GameInfo,
        players: Vec<PlayerState>,
        teams: Vec<live_proto::game::TeamScore>,
    ) {
        let is_sim = self.is_sim();
        if is_sim && info.mode != "multiplayer" {
            return;
        }
        let mut players = players;
        self.link(&mut players);
        // Campaign re-activates the same map on every checkpoint revert and
        // mission restart (`0x0018e700` calls the activation again with a
        // copy of the active options): that is still the same game.
        if let Some(current) = &mut self.current
            && !current.ended
            && current.map == info.map
            && ((current.campaign && info.mode == "campaign") || is_sim)
        {
            current.mark_cut(info.tick);
            return;
        }
        self.end_previous();
        if !info.is_match() {
            return;
        }
        let sim = self.sim.as_ref();
        let game_id = sim.map_or_else(live_proto::sign::random_id, |sim| {
            live_proto::game::sim_game_id(&sim.run_id)
        });
        let campaign = info.mode == "campaign";
        let playlist = if campaign || sim.is_some() {
            None
        } else {
            self.playlist.clone()
        };
        // Only the account-backed Live uploader has a signed-in player. A
        // local JSONL capture still records the client's view of this game.
        let frames_from_host = self.signed_in.is_some()
            && playlist.as_ref().is_some_and(|p| p.frames_from_host);
        let kind = match (campaign, &playlist) {
            (true, _) => "campaign",
            (false, Some(p)) if p.clankers => "clankermatch",
            (false, Some(_)) => "matchmaking",
            (false, None) => "custom",
        };
        let difficulty = if campaign {
            info.difficulty.clone()
        } else {
            None
        };
        let game = live_proto::game::NewGame {
            sim: sim.cloned(),
            console: self.console.clone(),
            map: info.map.clone(),
            mode: info.mode.clone(),
            variant: info.variant.clone(),
            team_play: info.team_play,
            score_limit: info.score_limit,
            time_limit_s: info.time_limit_s,
            tick: Some(info.tick),
            players: players.iter().map(|p| p.wire_player()).collect(),
            kind: Some(kind.into()),
            playlist: playlist.as_ref().and_then(|p| p.name.clone()),
            // A matchmaking game's session, else the session a bot
            // roster was given for (the server checks `bot: true` entries
            // against it, docs/game/bots.md §9.3).
            session_id: playlist
                .as_ref()
                .and_then(|p| p.session_id.clone())
                .or_else(|| {
                    self.bots
                        .as_ref()
                        .filter(|_| !campaign && sim.is_none())
                        .and_then(|b| b.session_id.clone())
                }),
            playlist_id: playlist.map(|p| p.id),
            difficulty,
            // Live co-op does not exist yet.
            online: None,
            map_id: self.map_id,
            skulls: if campaign {
                live_proto::skulls::normalize(
                    info.skulls
                        .iter()
                        .chain(&self.campaign_custom_skulls)
                        .flatten()
                        .copied(),
                )
            } else {
                self.multiplayer_skulls.clone()
            },
        };
        self.output.push(Output::Event(GameEvent::new(
            game_id.clone(),
            live_proto::game::GAME_STARTED,
            live_proto::game::GameStarted {
                game_id: game_id.clone(),
                host: self.host.clone(),
                dedicated: Default::default(),
                game,
            },
        )));
        self.current = Some(Match {
            game_id,
            map: info.map.clone(),
            campaign,
            frames_from_host,
            tick: info.tick,
            tick_offset: 0,
            sent_tick: info.tick,
            cut: None,
            tick_rate: u64::from(info.tick_rate.max(1)),
            team_play: info.team_play,
            last_emitted_scores: players
                .iter()
                .map(|p| p.stats.wire(p.player.index))
                .collect(),
            last_emitted_teams: teams,
            last_score_tick: info.tick,
            players,
            ended: false,
            tags: None,
            shots: BTreeMap::new(),
            actor_shots: BTreeMap::new(),
            units: BTreeMap::new(),
            machines_sent: BTreeMap::new(),
            machine_positions_sent: BTreeMap::new(),
            hits: Vec::new(),
            placements: Placements::default(),
            found: Vec::new(),
            player_units: BTreeMap::new(),
            player_bodies: std::collections::BTreeSet::new(),
            cinematic: false,
        });
    }

    /// The scenario's placements for the current match's frame `objects`;
    /// frames carry none until this is set.
    pub fn set_placements(&mut self, placements: Placements) {
        if let Some(current) = &mut self.current {
            current.placements = placements;
        }
    }

    /// Names for the current match's frame `species` and `weapon`; frames
    /// leave both out until (and unless) this is set.
    pub fn set_tag_names(&mut self, tags: TagNames) {
        if let Some(current) = &mut self.current {
            current.tags = Some(tags);
        }
    }

    pub(crate) fn end_previous(&mut self) {
        let Some(current) = self.current.take() else {
            return;
        };
        if current.ended {
            return;
        }
        let game_id = current.game_id.clone();
        Self::event_to(
            &mut self.output,
            game_id,
            live_proto::game::GAME_ENDED,
            GameEnded {
                tick: current.clock(current.tick),
                reason: "quit".into(),
                winner: None,
                players: current
                    .players
                    .iter()
                    .map(|p| p.stats.wire(p.player.index))
                    .collect(),
            },
        );
    }

    fn event_to(
        output: &mut Vec<Output>,
        game_id: String,
        kind: &str,
        data: impl serde::Serialize,
    ) {
        output.push(Output::Event(GameEvent::new(game_id, kind, data)));
    }

    pub fn sample(&mut self, snapshot: &Snapshot) {
        let signed_in = self.signed_in;
        let bots = self.bots.as_ref();
        let sim = self.is_sim();
        let render_frame = self.frame_due(snapshot.tick);
        let Some(current) = &mut self.current else {
            return;
        };
        if current.ended {
            return;
        }
        if sim && snapshot.tick < current.tick {
            // `h2 sim batch --fork` can load a snapshot without another
            // map start. Keep the recording's ticks monotonic across it.
            current.mark_cut(snapshot.tick);
        }
        current.tick = snapshot.tick;
        let clock = current.clock(snapshot.tick);
        // Most ticks the roster is as it was: nothing joined or left, and
        // nothing to copy.
        if !same_roster(signed_in, bots, &current.players, &snapshot.players, sim) {
            let mut linked = snapshot.players.clone();
            link(signed_in, bots, &mut linked, sim);
            Self::roster_changed(&mut self.output, current, &snapshot.players, linked);
        }
        let scores = snapshot.scores();
        if snapshot.has_stats
            && (self.filming || snapshot.tick >= current.last_score_tick + current.tick_rate)
            && (scores != current.last_emitted_scores
                || snapshot.teams != current.last_emitted_teams)
        {
            Self::event_to(
                &mut self.output,
                current.game_id.clone(),
                live_proto::game::SCORE_CHANGED,
                ScoreChanged {
                    tick: clock,
                    players: scores.clone(),
                    teams: (!snapshot.teams.is_empty()).then(|| snapshot.teams.clone()),
                },
            );
            current.last_emitted_scores = scores;
            current.last_emitted_teams = snapshot.teams.clone();
            current.last_score_tick = snapshot.tick;
            current.sent_tick = current.sent_tick.max(clock);
        }

        current.cinematic = snapshot.cinematic;
        // dh-identity: the joined client's one frame-upload gate. Scores,
        // joins, leaves and the end of the game still go through as usual.
        if current.frames_from_host && !self.filming {
            return;
        }
        for (player, object) in snapshot.players.iter().zip(&snapshot.objects) {
            if let Some(o) = object.as_ref().filter(|_| player.unit != u32::MAX) {
                current
                    .player_units
                    .insert(player.player.index, (player.unit, o.pos, yaw(o.forward)));
                current.player_bodies.insert(player.unit);
            }
        }

        let mut shots = BTreeMap::new();
        for (player, object) in snapshot.players.iter().zip(&snapshot.objects) {
            let Some(weapon) = object.as_ref().and_then(|o| o.weapon) else {
                continue;
            };
            let index = player.player.index;
            let mut pending = current
                .shots
                .get(&index)
                .copied()
                .filter(|p| p.weapon == weapon.datum)
                .unwrap_or(PendingShots {
                    weapon: weapon.datum,
                    fired: Fired::default(),
                });
            let fired = snapshot.shots.get(&weapon.datum).copied().unwrap_or(0);
            pending.fired.add(snapshot.tick, fired);
            shots.insert(index, pending);
        }
        current.shots = shots;
        for (weapon, n) in &snapshot.shots {
            if current.actor_shots.len() >= MAX_ACTOR_WEAPONS
                && !current.actor_shots.contains_key(weapon)
            {
                current.actor_shots.clear();
            }
            current
                .actor_shots
                .entry(*weapon)
                .or_default()
                .add(snapshot.tick, *n);
        }

        if snapshot.cinematic {
            // A cutscene: the scripts move puppets about, teleport the
            // players and kill whom they like. Nothing it does is play, so
            // nothing is counted: no shots, no hits, and frames carry only
            // the tick and the BSP, marked, for viewers to show a cutscene.
            // The units' last frames are forgotten, so the first frame
            // after it works out no speed or actions across the cut.
            current.shots.clear();
            current.actor_shots.clear();
            current.hits.clear();
            current.units.clear();
            if render_frame {
                current.sent_tick = current.sent_tick.max(clock);
                self.output.push(Output::Frame(Frame {
                    game_id: current.game_id.clone(),
                    tick: clock,
                    bsp: snapshot.bsp,
                    cut: current.cut.take(),
                    cinematic: true,
                    ..Frame::default()
                }));
            }
            return;
        }

        current.hits.extend(snapshot.hits.iter().cloned());
        let over = current.hits.len().saturating_sub(MAX_FRAME_HITS);
        current.hits.drain(..over);

        if render_frame {
            let game_id = current.game_id.clone();
            let tags = current.tags.as_ref();
            let hits = std::mem::take(&mut current.hits);
            let player_of_unit: BTreeMap<u32, u32> = snapshot
                .players
                .iter()
                .filter(|p| p.unit != u32::MAX)
                .map(|p| (p.unit, p.player.index))
                .collect();
            let frame_hits = frame_hits(&hits, snapshot.tick, &player_of_unit, tags);
            // Units that dealt melee damage since the last frame.
            let meleed: std::collections::BTreeSet<u32> = hits
                .iter()
                .filter(|h| layout::damage_reporting_type(h.code).0 == "melee")
                .map(|h| h.owner_object)
                .collect();
            let previous = std::mem::take(&mut current.units);
            let tick = snapshot.tick;
            let tick_rate = current.tick_rate;
            let units = &mut current.units;
            let mut observe = |datum: u32, object: &ObjectState, alive: bool| {
                let (observed, memory) = motion::observe(
                    previous.get(&datum),
                    object,
                    alive,
                    tick,
                    tick_rate,
                    meleed.contains(&datum),
                );
                if alive {
                    units.insert(datum, memory);
                }
                observed
            };
            // A body the game has cleared away is no one's any more.
            let living: std::collections::BTreeSet<u32> = current
                .player_units
                .values()
                .map(|(unit, _, _)| *unit)
                .collect();
            current
                .player_bodies
                .retain(|d| living.contains(d) || snapshot.bodies.iter().any(|b| b.datum == *d));
            let player_units = &current.player_units;
            let players = snapshot
                .players
                .iter()
                .zip(&snapshot.objects)
                .filter_map(|(player, object)| {
                    let alive = player.unit != u32::MAX;
                    // The dead lie at their body while it lasts, else where they fell.
                    let fallen = (!alive)
                        .then(|| player_units.get(&player.player.index))
                        .flatten()
                        .map(|(unit, pos, yaw)| {
                            snapshot
                                .bodies
                                .iter()
                                .find(|b| b.datum == *unit)
                                .map_or((*pos, *yaw), |b| {
                                    (b.object.pos, self::yaw(b.object.forward))
                                })
                        });
                    let index = player.player.index;
                    // Where they are: their unit, else (dead) their body or
                    // where they fell, else (a unit that cannot be read this
                    // tick) where that unit was last seen. Nowhere known: left
                    // out, never sent at the origin.
                    let (pos, yaw) = object
                        .as_ref()
                        .map(|o| (o.pos, yaw(o.forward)))
                        .or(fallen)
                        .or_else(|| {
                            player_units
                                .get(&index)
                                .filter(|(unit, _, _)| alive && *unit == player.unit)
                                .map(|(_, pos, yaw)| (*pos, *yaw))
                        })?;
                    let (shots, fire) = current
                        .shots
                        .get_mut(&index)
                        .map(|p| std::mem::take(&mut p.fired).frame(tick))
                        .unwrap_or_default();
                    let observed = match object {
                        Some(o) => observe(player.unit, o, alive),
                        None if !alive => motion::Observed {
                            stance: Some("dead".into()),
                            ..Default::default()
                        },
                        None => motion::Observed::default(),
                    };
                    Some(FramePlayer {
                        index,
                        alive,
                        pos,
                        yaw,
                        vehicle: object.as_ref().and_then(|o| o.vehicle).map(hex),
                        species: object
                            .as_ref()
                            .and_then(|o| o.definition)
                            .zip(tags)
                            .and_then(|(d, t)| t.species(d)),
                        aim: object.as_ref().and_then(|o| o.aim),
                        weapon: object
                            .as_ref()
                            .and_then(|o| o.weapon)
                            .zip(tags)
                            .and_then(|(w, t)| t.weapon(w.definition)),
                        shots,
                        fire,
                        stance: observed.stance,
                        speed: observed.speed,
                        actions: observed.actions,
                        grenade: observed.grenade,
                        anim: object
                            .as_ref()
                            .and_then(|o| frame_anim(o.motion.animation.as_ref(), tags)),
                        vel: object.as_ref().and_then(frame_vel),
                        seat: object.as_ref().and_then(frame_seat),
                        body: object.as_ref().and_then(|o| frame_vitality(o).0),
                        shield: object.as_ref().and_then(|o| frame_vitality(o).1),
                        // Not read yet (docs/live/replay-fidelity.md).
                        actions_ago: None,
                    })
                })
                .collect();
            let actor_shots = std::mem::take(&mut current.actor_shots);
            let mut actors = frame_actors(snapshot, tags, current.campaign, &actor_shots, tick);
            for (actor, object, datum) in &mut actors {
                let observed = observe(*datum, object, true);
                actor.stance = observed.stance;
                actor.speed = observed.speed;
                actor.actions = observed.actions;
                actor.grenade = observed.grenade;
                actor.anim = frame_anim(object.motion.animation.as_ref(), tags);
            }
            let vehicles = frame_vehicles(snapshot, &actors, tags);
            let bodies = frame_bodies(
                snapshot,
                &current.player_bodies,
                tags,
                current.campaign,
                MAX_FRAME_ACTORS.saturating_sub(actors.len()),
            );
            let machines = frame_machines(snapshot, &mut current.machines_sent, tags);
            let machine_positions =
                frame_machine_positions(snapshot, &mut current.machine_positions_sent);
            let objects = frame_objects(&current.placements, &snapshot.placed, &mut current.found);
            let projectiles = frame_projectiles(snapshot, tags);
            let items = frame_items(snapshot, tags);
            current.sent_tick = current.sent_tick.max(clock);
            self.output.push(Output::Frame(Frame {
                game_id,
                tick: clock,
                players,
                vehicles,
                actors: actors
                    .into_iter()
                    .map(|(a, _, _)| a)
                    .chain(bodies)
                    .collect(),
                hits: frame_hits,
                bsp: snapshot.bsp,
                machines,
                machine_positions,
                objects,
                cut: current.cut.take(),
                cinematic: false,
                projectiles,
                items,
            }));
        }
    }

    /// Joins and leaves between the current roster and `linked` (the
    /// fresh one, XUIDs linked), which then becomes the current one.
    fn roster_changed(
        output: &mut Vec<Output>,
        current: &mut Match,
        fresh: &[PlayerState],
        linked: Vec<PlayerState>,
    ) {
        let before: BTreeMap<u32, u32> = current
            .players
            .iter()
            .map(|p| (p.player.index, p.datum))
            .collect();
        let after: BTreeMap<u32, u32> = fresh.iter().map(|p| (p.player.index, p.datum)).collect();
        let left: Vec<u32> = before
            .iter()
            .filter(|(index, datum)| after.get(index) != Some(datum))
            .map(|(index, _)| *index)
            .collect();
        let joined: Vec<PlayerState> = linked
            .iter()
            .filter(|p| before.get(&p.player.index) != Some(&p.datum))
            .cloned()
            .collect();
        for index in left {
            Self::event_to(
                output,
                current.game_id.clone(),
                live_proto::game::PLAYER_LEFT,
                PlayerLeft { index },
            );
        }
        for player in joined {
            Self::event_to(
                output,
                current.game_id.clone(),
                live_proto::game::PLAYER_JOINED,
                live_proto::game::PlayerJoined {
                    player: player.wire_player(),
                },
            );
        }
        current.players = linked;
    }

    #[cfg(test)]
    fn is_campaign(&self) -> bool {
        self.current.as_ref().is_some_and(|m| m.campaign)
    }

    /// A player was killed; `cinematic` when a cinematic was playing at
    /// the time, which (like a kill after the end) is not counted.
    ///
    /// A betrayal is a teammate killed in a game with teams
    /// ([`live_proto::game::is_betrayal`]), by the players' teams as last
    /// sampled. The kill function's third argument is not a betrayal flag
    /// (it is set in free-for-all too), so it is not read.
    pub fn kill(
        &mut self,
        tick: u64,
        attacker: Option<u32>,
        victim: u32,
        source: Option<(&'static str, bool)>,
        object: Option<ObjectState>,
        cinematic: bool,
    ) {
        let Some(current) = &mut self.current else {
            return;
        };
        let betrayal = live_proto::game::is_betrayal(
            current.team_play || current.campaign,
            attacker,
            victim,
            |index| {
                current
                    .players
                    .iter()
                    .find(|p| p.player.index == index)
                    .map(|p| p.player.team)
            },
        );
        let kind = live_proto::game::kill_kind(attacker, victim, betrayal);
        // Post-end kills, and the scripts' kills in a cutscene, do not count.
        if current.ended || cinematic || current.cinematic {
            return;
        }
        let tick = current.clock(tick);
        current.sent_tick = current.sent_tick.max(tick);
        Self::event_to(
            &mut self.output,
            current.game_id.clone(),
            live_proto::game::PLAYER_KILLED,
            PlayerKilled {
                tick,
                victim,
                killer: attacker,
                weapon: source.map(|(weapon, _)| weapon.to_owned()),
                headshot: source.map(|(_, headshot)| headshot),
                kind: kind.into(),
                pos: object.map(|o| o.pos),
            },
        );
    }

    pub(crate) fn end_without_scoreboard(&mut self, tick: Option<u64>) {
        if let Some(current) = &self.current {
            self.end(&Snapshot { tick: tick.unwrap_or(current.tick), ..Snapshot::default() });
        }
    }

    pub fn end(&mut self, snapshot: &Snapshot) {
        let Some(current) = &mut self.current else {
            return;
        };
        if current.ended {
            return;
        }
        current.ended = true;
        current.tick = snapshot.tick;
        let winner = snapshot.winner(current.team_play);
        Self::event_to(
            &mut self.output,
            current.game_id.clone(),
            live_proto::game::GAME_ENDED,
            GameEnded {
                tick: current.clock(snapshot.tick),
                reason: "finished".into(),
                winner,
                players: snapshot.scores(),
            },
        );
    }
}

/// Who `p` is to the server: a bot is a bot (`bot: true`, with its
/// rostered XUID when it has one). Local player 0 uses the signed-in
/// account. Everyone else keeps the identity reported by the game.
fn identity(
    signed_in: Option<live_proto::Xuid>,
    bots: Option<&Bots>,
    p: &PlayerState,
    sim: bool,
) -> (Option<live_proto::Xuid>, Option<bool>) {
    if sim {
        return (None, Some(true));
    }
    if let Some(b) = bots.filter(|b| b.plays(p)) {
        return (b.xuid(p.machine), Some(true));
    }
    if let Some(x) = signed_in.filter(|_| p.local_user == Some(0)) {
        return (Some(x), p.player.bot);
    }
    (p.player.xuid, p.player.bot)
}

/// Gives local player 0 the signed-in account's XUID, and marks each bot
/// (a rostered phantom with its own xuid).
fn link(
    signed_in: Option<live_proto::Xuid>,
    bots: Option<&Bots>,
    players: &mut [PlayerState],
    sim: bool,
) {
    for p in players.iter_mut() {
        (p.player.xuid, p.player.bot) = identity(signed_in, bots, p, sim);
    }
}

/// Whether `current` is `fresh` once [`link`]ed, without copying it.
fn same_roster(
    signed_in: Option<live_proto::Xuid>,
    bots: Option<&Bots>,
    current: &[PlayerState],
    fresh: &[PlayerState],
    sim: bool,
) -> bool {
    current.len() == fresh.len()
        && current.iter().zip(fresh).all(|(c, f)| {
            // Every field named, so a new one cannot be missed.
            let PlayerState {
                datum,
                player,
                appearance,
                local_user,
                controller,
                machine,
                unit,
                stats,
            } = f;
            let live_proto::game::Player {
                index,
                name,
                // Both as `identity` makes them.
                xuid: _,
                team,
                local,
                appearance: wire_appearance,
                bot: _,
            } = player;
            let (xuid, bot) = identity(signed_in, bots, f, sim);
            c.datum == *datum
                && c.appearance == *appearance
                && c.local_user == *local_user
                && c.controller == *controller
                && c.unit == *unit
                && c.stats == *stats
                && c.player.index == *index
                && c.player.name == *name
                && c.player.xuid == xuid
                && c.player.bot == bot
                && c.machine == *machine
                && c.player.team == *team
                && c.player.local == *local
                && c.player.appearance == *wire_appearance
        })
}

/// A datum as the frame's object ids write it: eight lower-case hex
/// digits (`format!("{d:08x}")`, without the formatter).
fn hex(d: u32) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    (0..8)
        .rev()
        .map(|i| char::from(DIGITS[(d >> (i * 4)) as usize & 0xf]))
        .collect()
}

/// The frame's `vel`: the object's velocity (world units a second, as the
/// engine holds it), absent when not read or faster than a frame takes.
fn frame_vel(o: &ObjectState) -> Option<[f64; 3]> {
    o.velocity.filter(|v| {
        let speed = v.iter().map(|c| c * c).sum::<f64>().sqrt();
        speed.is_finite() && speed <= MAX_FRAME_SPEED
    })
}

/// The frame's `seat`: only while riding a vehicle, at most
/// [`MAX_FRAME_SEAT`].
fn frame_seat(o: &ObjectState) -> Option<u32> {
    o.vehicle
        .and(o.seat)
        .map(|s| u32::from(s).min(MAX_FRAME_SEAT))
}

/// The frame's `body` and `shield`, clamped to what a frame takes (body 0
/// to 1, shield 0 to [`MAX_FRAME_SHIELD`]).
fn frame_vitality(o: &ObjectState) -> (Option<f64>, Option<f64>) {
    o.vitality.map_or((None, None), |(body, shield)| {
        (
            Some(body.clamp(0.0, 1.0)),
            Some(shield.clamp(0.0, MAX_FRAME_SHIELD)),
        )
    })
}

fn finite_direction(v: [f64; 3]) -> bool {
    v.iter().all(|c| c.is_finite()) && v.iter().map(|c| c * c).sum::<f64>() > 1e-12
}

fn team_name(team: i16, campaign: bool) -> String {
    campaign
        .then(|| layout::campaign_team(team))
        .flatten()
        .map_or_else(|| team.to_string(), str::to_owned)
}

/// The frame's actors, nearest to any live player first, capped; each with
/// its object state (for vehicles).
fn frame_actors<'a>(
    snapshot: &'a Snapshot,
    tags: Option<&TagNames>,
    campaign: bool,
    shots: &BTreeMap<u32, Fired>,
    tick: u64,
) -> Vec<(FrameActor, &'a ObjectState, u32)> {
    let players: Vec<[f64; 3]> = snapshot.objects.iter().flatten().map(|o| o.pos).collect();
    let distance = |pos: [f64; 3]| {
        players
            .iter()
            .map(|p| (0..3).map(|i| (p[i] - pos[i]).powi(2)).sum::<f64>())
            .fold(f64::INFINITY, f64::min)
    };
    let mut actors: Vec<_> = snapshot
        .actors
        .iter()
        .filter(|a| finite_direction(a.object.forward))
        .map(|a| (distance(a.object.pos), a))
        .collect();
    actors.sort_by(|a, b| a.0.total_cmp(&b.0));
    actors
        .into_iter()
        .take(MAX_FRAME_ACTORS)
        .map(|(_, a)| {
            let o = &a.object;
            let (shots, fire) = o
                .weapon
                .and_then(|w| shots.get(&w.datum))
                .map(|f| f.frame(tick))
                .unwrap_or_default();
            let (body, shield) = frame_vitality(o);
            let actor = FrameActor {
                id: hex(a.datum),
                species: o
                    .definition
                    .zip(tags)
                    .and_then(|(d, t)| t.actor_species(d))
                    .unwrap_or_else(|| UNKNOWN.into()),
                pos: o.pos,
                yaw: yaw(o.forward),
                aim: o.aim,
                weapon: o.weapon.zip(tags).and_then(|(w, t)| t.weapon(w.definition)),
                shots,
                fire,
                team: o.team.map(|t| team_name(t, campaign)),
                vehicle: o.vehicle.map(hex),
                vel: frame_vel(o),
                seat: frame_seat(o),
                body,
                shield,
                ..FrameActor::default()
            };
            (actor, o, a.datum)
        })
        .collect()
}

/// The dead bodies that are not players' (AI, and bodies the level starts
/// with), nearest to any live player first, at most [`MAX_FRAME_BODIES`]
/// and `room`: actors with the `dead` stance, there until the game clears
/// the body away.
fn frame_bodies(
    snapshot: &Snapshot,
    players: &std::collections::BTreeSet<u32>,
    tags: Option<&TagNames>,
    campaign: bool,
    room: usize,
) -> Vec<FrameActor> {
    let near: Vec<[f64; 3]> = snapshot.objects.iter().flatten().map(|o| o.pos).collect();
    let distance = |pos: [f64; 3]| {
        near.iter()
            .map(|p| (0..3).map(|i| (p[i] - pos[i]).powi(2)).sum::<f64>())
            .fold(f64::INFINITY, f64::min)
    };
    let mut bodies: Vec<_> = snapshot
        .bodies
        .iter()
        .filter(|b| {
            !players.contains(&b.datum)
                && finite_direction(b.object.forward)
                && b.object.pos.iter().all(|c| c.is_finite())
        })
        .map(|b| (distance(b.object.pos), b))
        .collect();
    bodies.sort_by(|a, b| a.0.total_cmp(&b.0));
    bodies
        .into_iter()
        .take(MAX_FRAME_BODIES.min(room))
        .map(|(_, b)| {
            let o = &b.object;
            FrameActor {
                id: hex(b.datum),
                species: o
                    .definition
                    .zip(tags)
                    .and_then(|(d, t)| t.actor_species(d))
                    .unwrap_or_else(|| UNKNOWN.into()),
                pos: o.pos,
                yaw: yaw(o.forward),
                team: o.team.map(|t| team_name(t, campaign)),
                stance: Some("dead".into()),
                ..FrameActor::default()
            }
        })
        .collect()
}

/// Every vehicle, occupied ones first, capped: those a player or a sent
/// actor rides (their transform as the rider read it), then the world
/// scan's, occupied when a biped is parented to it.
fn frame_vehicles(
    snapshot: &Snapshot,
    actors: &[(FrameActor, &ObjectState, u32)],
    tags: Option<&TagNames>,
) -> Vec<FrameVehicle> {
    let kind = |definition: Option<u32>| {
        definition
            .zip(tags)
            .and_then(|(d, t)| t.vehicle(d))
            .unwrap_or_else(|| UNKNOWN.into())
    };
    // The world scan's word on each vehicle: what it is mounted on and
    // where it aims (a turret), also for one a rider read.
    let scanned: BTreeMap<u32, &VehicleState> =
        snapshot.vehicles.iter().map(|v| (v.datum, v)).collect();
    let aim = |d: u32| {
        scanned
            .get(&d)
            .and_then(|v| v.aim)
            .filter(|a| finite_direction(*a))
    };
    let mut seen = std::collections::BTreeSet::new();
    let ridden = snapshot
        .objects
        .iter()
        .flatten()
        .chain(actors.iter().map(|(_, o, _)| *o))
        .filter_map(|o| {
            let id = o.vehicle?;
            if !finite_direction(o.forward) || !finite_direction(o.up) || !seen.insert(id) {
                return None;
            }
            Some(FrameVehicle {
                id: hex(id),
                kind: kind(o.vehicle_definition),
                pos: o.pos,
                forward: o.forward,
                up: o.up,
                occupied: true,
                parent: o
                    .mount
                    .or_else(|| scanned.get(&id).and_then(|v| v.parent))
                    .map(hex),
                aim: aim(id),
                // The rider read what it rides: the vehicle's, or for a
                // gunner the mount's.
                vel: frame_vel(o),
            })
        })
        .collect::<Vec<_>>();
    let mut world: Vec<_> = snapshot
        .vehicles
        .iter()
        .filter(|v| {
            v.pos.iter().all(|c| c.is_finite())
                && finite_direction(v.forward)
                && finite_direction(v.up)
                && seen.insert(v.datum)
        })
        .map(|v| FrameVehicle {
            id: hex(v.datum),
            kind: kind(v.definition),
            pos: v.pos,
            forward: v.forward,
            up: v.up,
            occupied: v.occupied,
            parent: v.parent.map(hex),
            aim: aim(v.datum),
            vel: v.velocity.filter(|v| speed_of(*v) <= MAX_FRAME_SPEED),
        })
        .collect();
    world.sort_by_key(|v| !v.occupied);
    ridden
        .into_iter()
        .chain(world)
        .take(MAX_FRAME_VEHICLES)
        .collect()
}

/// A device sample only changes when its quantized value does.
fn frame_machine_positions(
    snapshot: &Snapshot,
    sent: &mut BTreeMap<u16, u8>,
) -> Vec<MachinePosition> {
    let mut changed = Vec::new();
    for m in &snapshot.machines {
        let Some((i, position)) = m.placement.zip(m.position) else {
            continue;
        };
        let p = (position * f64::from(u8::MAX)).round() as u8;
        if sent.get(&i) == Some(&p) {
            continue;
        }
        if changed.len() == MAX_MACHINE_POSITIONS {
            break;
        }
        let at = changed.binary_search_by_key(&i, |m: &MachinePosition| m.i);
        if let Err(at) = at {
            changed.insert(at, MachinePosition { i, p });
            sent.insert(i, p);
        }
    }
    changed
}

fn speed_of(v: [f64; 3]) -> f64 {
    v.iter().map(|c| c * c).sum::<f64>().sqrt()
}

/// The squared distance from `pos` to the nearest player (any position a
/// frame has for one), for nearest-first caps; 0 without players.
fn nearest_player(snapshot: &Snapshot) -> impl Fn([f64; 3]) -> f64 + '_ {
    move |pos| {
        snapshot
            .objects
            .iter()
            .flatten()
            .map(|o| (0..3).map(|i| (o.pos[i] - pos[i]).powi(2)).sum::<f64>())
            .fold(None, |m: Option<f64>, d| Some(m.map_or(d, |m| m.min(d))))
            .unwrap_or(0.0)
    }
}

/// Projectiles a viewer can see fly: slower than [`MAX_PROJECTILE_SPEED`]
/// (a bullet lives a tick or two; the viewer draws it from `fire`), or
/// stuck to something. Past the cap the nearest to a player are kept;
/// `unknown` kind without the map's names.
fn frame_projectiles(snapshot: &Snapshot, tags: Option<&TagNames>) -> Vec<FrameProjectile> {
    let near = nearest_player(snapshot);
    let mut out: Vec<(f64, FrameProjectile)> = snapshot
        .projectiles
        .iter()
        .filter(|p| p.pos.iter().all(|c| c.is_finite()))
        .filter(|p| {
            p.parent.is_some()
                || p.velocity
                    .is_some_and(|v| speed_of(v) < MAX_PROJECTILE_SPEED)
        })
        .map(|p| {
            (
                near(p.pos),
                FrameProjectile {
                    id: hex(p.datum),
                    kind: p
                        .definition
                        .zip(tags)
                        .and_then(|(d, t)| t.projectile(d))
                        .unwrap_or_else(|| UNKNOWN.into()),
                    pos: p.pos,
                    vel: p.velocity,
                    parent: p.parent.map(hex),
                },
            )
        })
        .collect();
    if out.len() > MAX_FRAME_PROJECTILES {
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.truncate(MAX_FRAME_PROJECTILES);
        out.sort_by(|a, b| a.1.id.cmp(&b.1.id));
    }
    // In object table order, as the scan found them, so a delta matches
    // each in place.
    out.into_iter().map(|(_, p)| p).collect()
}

/// Loose weapons and equipment (dropped guns, a dropped flag, ball or
/// bomb). Past the cap the nearest to a player are kept, in object table
/// order so a delta matches each in place.
fn frame_items(snapshot: &Snapshot, tags: Option<&TagNames>) -> Vec<FrameItem> {
    let near = nearest_player(snapshot);
    let mut out: Vec<(f64, &crate::event::ItemState)> = snapshot
        .items
        .iter()
        .filter(|i| finite_direction(i.forward) && finite_direction(i.up))
        .map(|i| (near(i.pos), i))
        .collect();
    if out.len() > MAX_FRAME_ITEMS {
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.truncate(MAX_FRAME_ITEMS);
    }
    out.sort_by_key(|(_, i)| i.datum & 0xffff);
    out.into_iter()
        .map(|(_, i)| FrameItem {
            id: hex(i.datum),
            kind: i
                .definition
                .zip(tags)
                .and_then(|(d, t)| {
                    if i.equipment {
                        t.equipment(d)
                    } else {
                        t.weapon(d)
                    }
                })
                .unwrap_or_else(|| UNKNOWN.into()),
            pos: i.pos,
            forward: i.forward,
            up: i.up,
            equipment: i.equipment,
        })
        .collect()
}

/// The machines that moved since they were last sent (all of them the
/// first time), capped; remembers what it sends.
fn frame_machines(
    snapshot: &Snapshot,
    sent: &mut BTreeMap<u32, MachineState>,
    tags: Option<&TagNames>,
) -> Vec<FrameMachine> {
    const EPSILON: f64 = 1e-3;
    let near = |a: &[f64; 3], b: &[f64; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < EPSILON);
    let moved = |m: &MachineState| match sent.get(&m.datum) {
        None => true,
        Some(s) => !near(&s.pos, &m.pos) || !near(&s.forward, &m.forward) || !near(&s.up, &m.up),
    };
    let changed: Vec<MachineState> = snapshot
        .machines
        .iter()
        .filter(|m| {
            m.pos.iter().all(|c| c.is_finite())
                && finite_direction(m.forward)
                && finite_direction(m.up)
                && moved(m)
        })
        .take(MAX_FRAME_MACHINES)
        .cloned()
        .collect();
    changed
        .into_iter()
        .map(|m| {
            let kind = m
                .definition
                .zip(tags)
                .and_then(|(d, t)| t.machine(d))
                .unwrap_or_else(|| UNKNOWN.into());
            let node = m
                .placement
                .filter(|_| kind != UNKNOWN)
                .map(|i| format!("{kind}_{i}"))
                .filter(|n| live_proto::game::valid_node(n));
            let frame = FrameMachine {
                node,
                id: hex(m.datum),
                kind,
                pos: m.pos,
                forward: m.forward,
                up: m.up,
                position: None,
            };
            sent.insert(m.datum, m);
            frame
        })
        .collect()
}

/// How far an object may be from its placement and still count as there:
/// a fiftieth of a world unit, and two degrees (cosine) of either axis.
const PLACED_DISTANCE: f64 = 0.02;
const PLACED_COS: f64 = 0.999_390_827;

/// Every placement whose object is not where the scenario put it, by kind
/// then index: moved by more than [`PLACED_DISTANCE`] or turned more than
/// two degrees (with its pose), or gone: no object, or (weapons, equipment
/// and netgame spawns) the object is held. Other parented objects are
/// left out (their transform is parent-relative).
fn frame_objects(
    placements: &Placements,
    placed: &[PlacedObject],
    found: &mut Vec<u32>,
) -> Vec<FrameObject> {
    frame_objects_with(placements, placed, found, true)
}

/// Inside the two degrees by more than this (in cosine) needs no exact
/// test: rounding is some 1e-15.
const PLACED_COS_MARGIN: f64 = 1e-9;

/// Whether `o` is where `pose` puts it beyond doubt, without normalising
/// its axes: then [`frame_objects`]' exact test (normalise, then compare)
/// finds it there too. Its distance test is the exact one; each axis must
/// be finite, longer than `1e-6` with room, and within the cosine with
/// [`PLACED_COS_MARGIN`] to spare (`v·p / |v|`, squared: no square root).
fn surely_in_place(o: &PlacedObject, pose: &Pose) -> bool {
    let dot = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let cos = PLACED_COS + PLACED_COS_MARGIN;
    let within = |v: [f64; 3], p: [f64; 3]| {
        let n = dot(v, v);
        let q = dot(v, p);
        n.is_finite() && n > 2e-12 && q > 0.0 && q * q >= cos * cos * n
    };
    let d = [0, 1, 2].map(|k| o.pos[k] - pose.pos[k]);
    o.pos.iter().all(|c| c.is_finite())
        // The exact test's `dot(d, d) > PLACED_DISTANCE²`, negated.
        && dot(d, d).partial_cmp(&(PLACED_DISTANCE * PLACED_DISTANCE))
            != Some(std::cmp::Ordering::Greater)
        && within(o.forward, pose.forward)
        && within(o.up, pose.up)
}

/// [`frame_objects`]; `quick` lets [`surely_in_place`] skip the exact test
/// (the tests compare both).
fn frame_objects_with(
    placements: &Placements,
    placed: &[PlacedObject],
    found: &mut Vec<u32>,
    quick: bool,
) -> Vec<FrameObject> {
    const NONE: u32 = u32::MAX;
    let dot = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let unit = |v: [f64; 3]| {
        let len = dot(v, v).sqrt();
        (len.is_finite() && len > 1e-6).then(|| [v[0] / len, v[1] / len, v[2] / len])
    };
    // Each kind's run of `found` (one slot a placement, `placements.kinds`
    // holds each kind once), by kind; one pass over `placed` fills them all.
    const KINDS: usize = 32;
    let mut runs = [(0usize, 0usize); KINDS];
    let mut total = 0;
    for (kind, poses) in &placements.kinds {
        if let Some(run) = runs.get_mut(*kind as usize) {
            *run = (total, poses.len());
            total += poses.len();
        }
    }
    found.clear();
    found.resize(total, NONE);
    for (n, p) in placed.iter().enumerate() {
        let Some(&(start, len)) = runs.get(p.kind as usize) else {
            continue;
        };
        if usize::from(p.index) >= len {
            continue;
        }
        let slot = &mut found[start + usize::from(p.index)];
        // One on its spawn beats one a player holds.
        if *slot == NONE || (placed[*slot as usize].parented && !p.parented) {
            *slot = n as u32;
        }
    }
    let mut out = Vec::new();
    for (kind, poses) in &placements.kinds {
        let Some(&(start, len)) = runs.get(*kind as usize) else {
            continue;
        };
        let found = &found[start..start + len];
        let held_is_gone = matches!(
            kind,
            ObjectKind::Weapon | ObjectKind::Equipment | ObjectKind::NetgameEquipment
        );
        for (i, (pose, slot)) in poses.iter().zip(found.iter()).enumerate() {
            let object = (*slot != NONE).then(|| &placed[*slot as usize]);
            let entry = match object {
                Some(o) if o.parented && !held_is_gone => None,
                None | Some(PlacedObject { parented: true, .. }) => Some(FrameObject {
                    k: *kind,
                    i: i as u32,
                    p: None,
                    f: None,
                    u: None,
                    gone: true,
                }),
                Some(o) if quick && pose.as_ref().is_some_and(|p| surely_in_place(o, p)) => None,
                Some(o) => {
                    let (Some(forward), Some(up)) = (unit(o.forward), unit(o.up)) else {
                        continue;
                    };
                    if !o.pos.iter().all(|c| c.is_finite()) {
                        continue;
                    }
                    let moved = pose.is_none_or(|pose| {
                        let d = [0, 1, 2].map(|k| o.pos[k] - pose.pos[k]);
                        dot(d, d) > PLACED_DISTANCE * PLACED_DISTANCE
                            || dot(forward, pose.forward) < PLACED_COS
                            || dot(up, pose.up) < PLACED_COS
                    });
                    moved.then_some(FrameObject {
                        k: *kind,
                        i: i as u32,
                        p: Some(o.pos),
                        f: Some(forward),
                        u: Some(up),
                        gone: false,
                    })
                }
            };
            if let Some(entry) = entry {
                if out.len() == MAX_FRAME_OBJECTS {
                    return out;
                }
                out.push(entry);
            }
        }
    }
    out
}

/// The animation a unit plays, named through the map's graphs; none when
/// the graph or the animation has no name the frames take.
fn frame_anim(
    animation: Option<&crate::event::Animation>,
    tags: Option<&TagNames>,
) -> Option<FrameAnim> {
    let a = animation?;
    Some(FrameAnim {
        name: tags?.animation(a)?,
        frame: f64::from(a.frame).clamp(0.0, live_proto::game::MAX_ANIM_FRAME),
    })
}

/// The frame's hits: attacker and victim as `p<index>` for a player's unit
/// (or the reporting player), else the object datum; the weapon from the
/// damage reporting type, else the damage effect's stem.
fn frame_hits(
    hits: &[HitRecord],
    tick: u64,
    player_of_unit: &BTreeMap<u32, u32>,
    tags: Option<&TagNames>,
) -> Vec<FrameHit> {
    hits.iter()
        .filter_map(|h| frame_hit(h, tick, |unit| player_of_unit.get(&unit).copied(), tags))
        .take(MAX_FRAME_HITS)
        .collect()
}

fn frame_hit(
    h: &HitRecord,
    tick: u64,
    player_of_unit: impl Fn(u32) -> Option<u32>,
    tags: Option<&TagNames>,
) -> Option<FrameHit> {
    let object = |datum: u32| {
        (datum != u32::MAX).then(|| match player_of_unit(datum) {
            Some(index) => format!("p{index}"),
            None => hex(datum),
        })
    };
    let (kind, weapon) = layout::damage_reporting_type(h.code);
    let from = if h.owner_player != u32::MAX {
        Some(format!("p{}", h.owner_player & 0xffff))
    } else {
        object(h.owner_object)
    };
    let amount = f64::from(h.body.max(0.0)) + f64::from(h.shield.max(0.0));
    Some(FrameHit {
        from,
        to: object(h.victim),
        weapon: weapon.map(str::to_owned).or_else(|| {
            tags.and_then(|t| t.damage_effect(h.effect))
                .filter(|s| live_proto::game::valid_model_id(s))
        }),
        pos: h.pos?,
        amount: amount.is_finite().then_some(amount),
        kind: Some(kind.into()),
        ago: h.tick.and_then(|t| tick.checked_sub(t))
            .and_then(|a| u32::try_from(a).ok())
            .filter(|a| *a <= live_proto::game::MAX_FRAME_AGO),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Appearance, ObjectState, PlayerStats};

    fn info() -> GameInfo {
        GameInfo {
            map: "lockout".into(),
            mode: "multiplayer".into(),
            difficulty: None,
            skulls: None,
            variant: Some("Slayer".into()),
            team_play: false,
            score_limit: Some(25),
            time_limit_s: Some(600),
            tick_rate: 30,
            tick: 0,
        }
    }

    fn player(index: u32, score: i64) -> PlayerState {
        PlayerState {
            datum: 0x80010000 | index,
            player: live_proto::game::Player {
                index,
                name: format!("P{index}"),
                xuid: None,
                team: 0,
                local: Some(index == 0),
                appearance: None,
                bot: None,
            },
            appearance: Appearance::default(),
            local_user: (index == 0).then_some(0),
            controller: (index == 0).then_some(0),
            machine: [0; 6],
            unit: u32::MAX,
            stats: PlayerStats {
                score,
                ..PlayerStats::ZERO
            },
        }
    }

    fn snapshot(tick: u64, players: Vec<PlayerState>) -> Snapshot {
        Snapshot {
            tick,
            has_stats: true,
            objects: players.iter().map(|_| None).collect(),
            players,
            teams: Vec::new(),
            shots: Default::default(),
            actors: Vec::new(),
            vehicles: Vec::new(),
            machines: Vec::new(),
            placed: Vec::new(),
            bodies: Vec::new(),
            projectiles: Vec::new(),
            items: Vec::new(),
            bsp: None,
            hits: Vec::new(),
            cinematic: false,
        }
    }

    fn kinds(session: &mut Session) -> Vec<String> {
        session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) => Some(e.kind),
                Output::Frame(_) => None,
            })
            .collect()
    }

    #[test]
    fn hex_writes_a_datum_as_the_formatter_does() {
        for d in [0, 1, 0xa, 0xe280_0001, 0x9001_0000, u32::MAX] {
            assert_eq!(hex(d), format!("{d:08x}"));
        }
    }

    #[test]
    fn start_roster_score_and_end_events() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameStarted"]);
        session.sample(&snapshot(2, vec![player(0, 0), player(1, 0)]));
        assert_eq!(kinds(&mut session), ["PlayerJoined"]);
        session.sample(&snapshot(4, vec![player(0, 0)]));
        assert_eq!(kinds(&mut session), ["PlayerLeft"]);

        session.sample(&snapshot(15, vec![player(0, 1)]));
        assert!(
            kinds(&mut session).is_empty(),
            "score changes wait one second"
        );
        session.sample(&snapshot(30, vec![player(0, 2)]));
        assert_eq!(kinds(&mut session), ["ScoreChanged"]);
        session.sample(&snapshot(31, vec![player(0, 3)]));
        assert!(kinds(&mut session).is_empty());
        session.sample(&snapshot(60, vec![player(0, 4)]));
        assert_eq!(kinds(&mut session), ["ScoreChanged"]);

        session.end(&snapshot(61, vec![player(0, 5)]));
        let outputs = session.take_output();
        let Output::Event(event) = &outputs[0] else {
            panic!()
        };
        assert_eq!(event.kind, "GameEnded");
        assert_eq!(event.data["winner"]["player"], 0);
        assert_eq!(event.data["players"][0]["score"], 5);
        session.end(&snapshot(62, vec![player(0, 6)]));
        assert!(session.take_output().is_empty(), "end is idempotent");
    }

    #[test]
    fn roster_events_carry_appearance() {
        let mut session = Session::new("test", None);
        let mut host = player(0, 0);
        host.appearance = Appearance {
            primary: Some(5),
            secondary: Some(9),
            tertiary: Some(2),
            quaternary: Some(11),
            emblem_foreground: Some(30),
            emblem_background: Some(7),
        };
        session.start(&info(), vec![host, player(1, 0)]);
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        assert_eq!(started.kind, "GameStarted");
        assert_eq!(
            started.data["players"][0]["appearance"],
            serde_json::json!({
                "primary": 5, "secondary": 9, "tertiary": 2, "quaternary": 11,
                "emblem_foreground": 30, "emblem_background": 7
            })
        );
        assert!(
            started.data["players"][1].get("appearance").is_none(),
            "an unknown appearance is omitted entirely"
        );

        // A join carries the new player's appearance; an unchanged roster
        // never re-sends one, and there is no appearance anywhere else.
        let mut joiner = player(2, 0);
        joiner.appearance = Appearance {
            primary: Some(1),
            ..Appearance::default()
        };
        session.sample(&snapshot(2, vec![player(0, 0), player(1, 0), joiner]));
        let events = session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) => Some(e),
                Output::Frame(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "PlayerJoined");
        assert_eq!(events[0].data["player"]["index"], 2);
        assert_eq!(events[0].data["player"]["appearance"]["primary"], 1);

        session.sample(&snapshot(
            30,
            vec![player(0, 1), player(1, 0), player(2, 0)],
        ));
        let events = session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) => Some(e),
                Output::Frame(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "ScoreChanged");
        assert!(
            events[0].data.get("appearance").is_none(),
            "appearance never rides score or frame events"
        );
    }

    #[test]
    fn kills_include_kind_victim_position_and_attribution() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0), player(1, 0)]);
        session.take_output();
        let object = ObjectState {
            pos: [4.0, 5.0, 6.0],
            forward: [1.0, 0.0, 0.0],
            ..ObjectState::default()
        };
        session.kill(9, Some(0), 1, None, Some(object.clone()), false);
        session.kill(10, Some(0), 0, None, Some(object.clone()), false);
        session.kill(12, None, 1, None, Some(object), false);
        let events = session.take_output();
        let expected = [("kill", 9), ("suicide", 10), ("world", 12)];
        assert_eq!(events.len(), expected.len());
        for (output, (kind, tick)) in events.into_iter().zip(expected) {
            let Output::Event(event) = output else {
                panic!()
            };
            assert_eq!(event.kind, "PlayerKilled");
            assert_eq!(event.data["kind"], kind);
            assert!(event.data.get("weapon").is_none());
            assert!(event.data.get("headshot").is_none());
            assert_eq!(event.data["tick"], tick);
            assert_eq!(event.data["pos"], serde_json::json!([4.0, 5.0, 6.0]));
        }
    }

    fn kill_kinds(session: &mut Session) -> Vec<String> {
        session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) if e.kind == "PlayerKilled" => {
                    Some(e.data["kind"].as_str().unwrap().to_string())
                }
                _ => None,
            })
            .collect()
    }

    fn on_team(index: u32, team: i32) -> PlayerState {
        let mut p = player(index, 0);
        p.player.team = team;
        p
    }

    /// Free-for-all (a Clankers game: the host and phantoms, all team 0)
    /// has no betrayals: every player kill is a kill.
    #[test]
    fn free_for_all_kills_are_never_betrayals() {
        let mut session = Session::new("test", None);
        let players = (0..4).map(|i| on_team(i, 0)).collect();
        session.start(&info(), players);
        session.take_output();
        session.kill(9, Some(0), 3, None, None, false);
        session.kill(10, Some(1), 0, None, None, false);
        session.kill(11, Some(2), 1, None, None, false);
        assert_eq!(kill_kinds(&mut session), ["kill", "kill", "kill"]);
    }

    /// In a team game a betrayal is a teammate killed; an enemy killed is
    /// a kill.
    #[test]
    fn team_game_betrayals_are_teammates_killed() {
        let mut session = Session::new("test", None);
        let team = GameInfo {
            team_play: true,
            ..info()
        };
        let players = vec![on_team(0, 0), on_team(1, 0), on_team(2, 1)];
        session.start(&team, players);
        session.take_output();
        session.kill(9, Some(0), 1, None, None, false);
        session.kill(10, Some(0), 2, None, None, false);
        session.kill(11, Some(2), 2, None, None, false);
        assert_eq!(kill_kinds(&mut session), ["betrayal", "kill", "suicide"]);
    }

    #[test]
    fn post_end_kills_are_ignored() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0), player(1, 0)]);
        session.end(&snapshot(30, vec![player(0, 0), player(1, 0)]));
        session.take_output();
        session.kill(31, Some(0), 1, None, None, false);
        assert!(session.take_output().is_empty());
    }

    #[test]
    fn replacement_and_nonmatch_close_the_previous_game() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        session.take_output();
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameEnded", "GameStarted"]);
        let mut menu = info();
        menu.mode = "main_menu".into();
        session.start(&menu, Vec::new());
        assert_eq!(kinds(&mut session), ["GameEnded"]);
        session.sample(&snapshot(2, vec![player(0, 0)]));
        assert!(session.take_output().is_empty());
    }

    #[test]
    fn frames_are_every_second_tick_with_yaw_and_vehicle() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let mut state = snapshot(1, vec![p.clone()]);
        state.objects[0] = Some(ObjectState {
            pos: [1.0, 2.0, 3.0],
            forward: [0.0, -1.0, 0.0],
            vehicle: Some(0x90020001),
            ..ObjectState::default()
        });
        session.sample(&state);
        assert!(session.take_output().is_empty());
        state.tick = 2;
        session.sample(&state);
        let outputs = session.take_output();
        let Output::Frame(frame) = &outputs[0] else {
            panic!()
        };
        assert_eq!(frame.players[0].yaw, -std::f64::consts::FRAC_PI_2);
        assert_eq!(frame.players[0].vehicle.as_deref(), Some("90020001"));
        assert!(frame.players[0].alive);
    }

    #[test]
    fn sim_session_marks_snapshot_rewinds_as_cuts() {
        let mut session = Session::new("test", None).with_sim(live_proto::game::SimGame {
            run_id: "forked".into(),
            batch_id: None,
            seed: "42".into(),
            gametype: "slayer".into(),
            pace: "unpaced".into(),
            bots: vec![],
            deferred: false,
        });
        let players = vec![player(0, 1)];
        session.start(&info(), players.clone());
        let mut state = snapshot(34, players);
        state.machines.push(machine(7, [1.0, 2.0, 3.0], Some(0.0)));
        session.sample(&state);
        state.tick = 30;
        session.sample(&state);
        let got = frames(&mut session);
        assert_eq!((got[0].tick, got[0].cut), (34, None));
        assert_eq!((got[1].tick, got[1].cut), (36, Some(30)));
        for f in &got {
            assert_eq!(f.machine_positions, [MachinePosition { i: 7, p: 0 }]);
            assert_eq!(f.machines.len(), 1);
        }
    }

    fn frames(session: &mut Session) -> Vec<Frame> {
        session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Frame(f) => Some(f),
                Output::Event(_) => None,
            })
            .collect()
    }

    fn armed(weapon: u32, definition: u32) -> ObjectState {
        ObjectState {
            pos: [1.0, 2.0, 3.0],
            forward: [1.0, 0.0, 0.0],
            definition: Some(0xe100_0010),
            aim: Some([0.0, 0.6, 0.8]),
            weapon: Some(crate::event::Weapon {
                datum: weapon,
                definition,
            }),
            ..ObjectState::default()
        }
    }

    fn names() -> TagNames {
        TagNames::new(
            0xe000_0001,
            [
                (
                    0xe100_0010,
                    *b"bipd",
                    "objects\\characters\\elite\\elite_mp".to_string(),
                ),
                (
                    0xe100_0020,
                    *b"weap",
                    "objects\\weapons\\rifle\\battle_rifle\\battle_rifle".to_string(),
                ),
            ],
        )
    }

    #[test]
    fn frames_carry_the_animation_by_name() {
        use crate::event::Animation;
        use crate::tags::Graph;
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let mut state = snapshot(2, vec![p]);
        let mut object = armed(0x9005_0005, 0xe100_0020);
        object.motion.animation = Some(Animation {
            graph: 0xe100_0030,
            inherited: -1,
            index: 1,
            frame: 7.25,
        });
        state.objects[0] = Some(object);
        // No names yet: no animation, never a guess.
        session.sample(&state);
        assert_eq!(frames(&mut session)[0].players[0].anim, None);
        session.set_tag_names(names().with_graphs([(
            0xe100_0030,
            Graph {
                names: vec![None, Some("combat:rifle:any:idle".into())],
                parents: vec![],
            },
        )]));
        state.tick = 4;
        session.sample(&state);
        let frame = &frames(&mut session)[0];
        assert_eq!(
            frame.players[0].anim,
            Some(FrameAnim {
                name: "combat:rifle:any:idle".into(),
                frame: 7.25
            })
        );
        assert_eq!(live_proto::game::validate_frame(frame), Ok(()));
        // An animation the graph doesn't name: none.
        state.objects[0]
            .as_mut()
            .unwrap()
            .motion
            .animation
            .as_mut()
            .unwrap()
            .index = 0;
        state.tick = 6;
        session.sample(&state);
        assert_eq!(frames(&mut session)[0].players[0].anim, None);
    }

    #[test]
    fn frames_carry_species_aim_and_weapon_from_tag_names() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&info(), vec![p.clone(), player(1, 0)]);
        session.take_output();
        let mut state = snapshot(2, vec![p.clone(), player(1, 0)]);
        state.objects[0] = Some(armed(0x9005_0005, 0xe100_0020));

        // Without names, only the aim (read straight from memory) is sent.
        session.sample(&state);
        let frame = &frames(&mut session)[0];
        assert_eq!(frame.players[0].aim, Some([0.0, 0.6, 0.8]));
        assert_eq!(frame.players[0].species, None);
        assert_eq!(frame.players[0].weapon, None);

        session.set_tag_names(names());
        state.tick = 4;
        session.sample(&state);
        let frame = &frames(&mut session)[0];
        assert_eq!(frame.players[0].species.as_deref(), Some("elite"));
        assert_eq!(frame.players[0].weapon.as_deref(), Some("battle_rifle"));
        // A dead player who has never stood anywhere is nowhere to draw.
        assert_eq!(frame.players.len(), 1);
        assert_eq!(
            serde_json::to_value(&frame.players[0]).unwrap()["species"],
            "elite"
        );

        // An unknown definition sends nothing rather than a guess.
        state.objects[0] = Some(armed(0x9005_0005, 0xe100_0099));
        state.tick = 6;
        session.sample(&state);
        assert_eq!(frames(&mut session)[0].players[0].weapon, None);

        // A new match forgets the old map's names.
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let mut state = snapshot(8, vec![p]);
        state.objects[0] = Some(armed(0x9005_0005, 0xe100_0020));
        session.sample(&state);
        assert_eq!(frames(&mut session)[0].players[0].weapon, None);
    }

    #[test]
    fn shots_add_up_between_frames_and_reset_on_weapon_change_and_death() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let rifle = 0x9005_0005;
        let pistol = 0x9006_0006;
        let sample =
            |session: &mut Session, tick: u64, weapon: Option<u32>, fired: &[(u32, u32)]| {
                let mut state = snapshot(tick, vec![p.clone()]);
                state.objects[0] = weapon.map(|w| armed(w, 0xe100_0020));
                state.shots = fired.iter().copied().collect();
                session.sample(&state);
                frames(session).first().map(|f| f.players[0].shots)
            };

        // Odd ticks add up into the next even tick's frame.
        assert_eq!(sample(&mut session, 1, Some(rifle), &[(rifle, 2)]), None);
        assert_eq!(
            sample(&mut session, 2, Some(rifle), &[(rifle, 1), (pistol, 4)]),
            Some(Some(3)),
            "only the held weapon's shots count"
        );
        assert_eq!(
            sample(&mut session, 4, Some(rifle), &[]),
            Some(None),
            "0 is absent"
        );
        // Switching weapons drops shots counted for the old one.
        assert_eq!(sample(&mut session, 5, Some(rifle), &[(rifle, 5)]), None);
        assert_eq!(
            sample(&mut session, 6, Some(pistol), &[(pistol, 1)]),
            Some(Some(1))
        );
        // Death drops pending shots, and a dead player sends none.
        assert_eq!(sample(&mut session, 7, Some(pistol), &[(pistol, 2)]), None);
        assert_eq!(sample(&mut session, 8, None, &[(pistol, 1)]), Some(None));
        // Counts are capped at the protocol's limit.
        assert_eq!(
            sample(&mut session, 10, Some(rifle), &[(rifle, 500)]),
            Some(Some(MAX_FRAME_SHOTS))
        );
    }

    #[test]
    fn fire_masks_move_older_and_drop_off_after_32_ticks() {
        let mut f = Fired::default();
        f.add(100, 1);
        f.add(102, 0);
        f.add(103, 2);
        assert_eq!(f.count, 3);
        assert_eq!(f.mask_at(103), 0b1001, "ticks 103 and 100");
        assert_eq!(f.mask_at(110), 0b1001 << 7);
        assert_eq!(f.mask_at(131), (1 << 31) | (1 << 28));
        assert_eq!(f.mask_at(132), 1 << 29, "tick 100 is past 32 back");
        assert_eq!(f.mask_at(135), 0);
        assert_eq!(f.mask_at(99), 0, "none before the last shot's tick");
        assert_eq!(f.frame(104), (Some(3), Some(0b10010)));
        assert_eq!(Fired::default().frame(104), (None, None));
    }

    #[test]
    fn frames_carry_velocity_seat_vitality_and_fire_ticks() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let rifle = 0x9005_0005;
        let mut unit = armed(rifle, 0xe100_0020);
        unit.velocity = Some([3.0, -4.0, 0.5]);
        unit.seat = Some(2);
        unit.vitality = Some((0.5, 3.5));
        let mut state = snapshot(1, vec![p.clone()]);
        state.objects[0] = Some(unit.clone());
        state.shots = [(rifle, 1)].into_iter().collect();
        session.sample(&state);
        state.tick = 2;
        state.shots = [(rifle, 2)].into_iter().collect();
        session.sample(&state);
        let f = frames(&mut session);
        let sent = &f[0].players[0];
        assert_eq!(sent.vel, Some([3.0, -4.0, 0.5]), "units a second, as read");
        assert_eq!(sent.seat, None, "a seat only while in a vehicle");
        assert_eq!((sent.body, sent.shield), (Some(0.5), Some(3.5)));
        assert_eq!(sent.shots, Some(3));
        assert_eq!(sent.fire, Some(0b11), "ticks 2 and 1");
        // No shots: neither count nor mask.
        state.shots.clear();
        state.tick = 3;
        session.sample(&state);
        state.tick = 4;
        session.sample(&state);
        let f = frames(&mut session);
        assert_eq!((f[0].players[0].shots, f[0].players[0].fire), (None, None));
        // A shot on the odd tick lands a tick before the frame. In a
        // vehicle, a seat, vitality and speed past what a frame takes:
        // clamped, and a velocity too fast is left out.
        state.tick = 5;
        state.shots = [(rifle, 1)].into_iter().collect();
        session.sample(&state);
        unit.vehicle = Some(0x9002_0001);
        unit.seat = Some(70);
        unit.vitality = Some((1.5, 9.0));
        unit.velocity = Some([2000.0, 0.0, 0.0]);
        state.objects[0] = Some(unit.clone());
        state.tick = 6;
        state.shots.clear();
        session.sample(&state);
        let f = frames(&mut session);
        let sent = &f[0].players[0];
        assert_eq!((sent.shots, sent.fire), (Some(1), Some(0b10)));
        assert_eq!(sent.seat, Some(MAX_FRAME_SEAT));
        assert_eq!(
            (sent.body, sent.shield),
            (Some(1.0), Some(MAX_FRAME_SHIELD))
        );
        assert_eq!(sent.vel, None);
        live_proto::game::validate_frame(&f[0]).unwrap();
        // Actors: the same, by the weapon each holds.
        let gun = 0x9007_0007;
        let mut grunt = armed(gun, 0xe100_0020);
        grunt.velocity = Some([0.0, 1.0, 0.0]);
        grunt.vitality = Some((0.25, 0.0));
        grunt.vehicle = Some(0x9002_0001);
        grunt.seat = Some(1);
        state.tick = 7;
        state.shots = [(gun, 1)].into_iter().collect();
        session.sample(&state);
        state.tick = 8;
        state.shots.clear();
        state.actors = vec![crate::event::ActorState {
            datum: 0x9008_0008,
            object: grunt,
        }];
        session.sample(&state);
        let f = frames(&mut session);
        let actor = &f[0].actors[0];
        assert_eq!((actor.shots, actor.fire), (Some(1), Some(0b10)));
        assert_eq!(actor.vel, Some([0.0, 1.0, 0.0]));
        assert_eq!(actor.seat, Some(1));
        assert_eq!((actor.body, actor.shield), (Some(0.25), Some(0.0)));
        live_proto::game::validate_frame(&f[0]).unwrap();
    }

    fn campaign() -> GameInfo {
        GameInfo {
            map: "03a_oldmombasa".into(),
            mode: "campaign".into(),
            difficulty: Some("heroic".into()),
            skulls: None,
            variant: None,
            team_play: false,
            score_limit: None,
            time_limit_s: None,
            tick_rate: 30,
            tick: 0,
        }
    }

    /// A cutscene sends marked frames with no one in them, on the game's
    /// clock, and counts nothing the scripts do: no kills, hits or shots.
    /// The first frame after it has the players back where the scripts left
    /// them, with no speed or actions worked out across the cut.
    #[test]
    fn a_cinematic_sends_marked_empty_frames_and_counts_nothing() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x9001_0001;
        let rifle = 0xe400_0001;
        session.start(&campaign(), vec![p.clone()]);
        session.take_output();
        let at = |pos: [f64; 3]| ObjectState {
            pos,
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            weapon: Some(crate::event::Weapon {
                datum: rifle,
                definition: 0xe100_0002,
            }),
            ..ObjectState::default()
        };
        let state = |tick: u64, pos: [f64; 3], cinematic: bool| {
            let mut s = snapshot(tick, vec![p.clone()]);
            s.objects[0] = Some(at(pos));
            s.actors = vec![actor(3, [9.0, 9.0, 0.0], 1)];
            s.bsp = Some(1);
            s.shots.add(rifle, 2);
            s.hits = vec![HitRecord {
                code: 7,
                effect: 0,
                owner_player: 0,
                owner_object: p.unit,
                victim: 0x9000_0003,
                pos: Some([9.0, 9.0, 0.0]),
                body: 0.5,
                shield: 0.0,
                tick: Some(tick),
            }];
            s.cinematic = cinematic;
            s
        };
        session.sample(&state(10, [0.0, 0.0, 0.0], false));
        let before = frames(&mut session);
        assert_eq!(before.len(), 1);
        assert!(!before[0].cinematic);
        assert_eq!(before[0].players.len(), 1);
        assert_eq!(before[0].actors.len(), 1);
        assert_eq!(before[0].hits.len(), 1);

        // The cutscene: the scripts walk the player off and kill them.
        for tick in 11..=14 {
            session.sample(&state(tick, [tick as f64, 0.0, 0.0], true));
        }
        session.kill(14, None, 0, None, None, false);
        session.kill(14, None, 0, None, None, true);
        let output = session.take_output();
        assert!(
            output.iter().all(|o| matches!(o, Output::Frame(_))),
            "no kills: {output:?}"
        );
        let during: Vec<Frame> = output
            .into_iter()
            .filter_map(|o| match o {
                Output::Frame(f) => Some(f),
                Output::Event(_) => None,
            })
            .collect();
        assert_eq!(
            during.iter().map(|f| f.tick).collect::<Vec<_>>(),
            [12, 14],
            "frames keep the game's clock"
        );
        for f in &during {
            assert!(f.cinematic);
            assert_eq!(f.bsp, Some(1));
            assert!(f.players.is_empty() && f.actors.is_empty() && f.vehicles.is_empty());
            assert!(f.hits.is_empty() && f.machines.is_empty() && f.objects.is_empty());
            assert_eq!(live_proto::game::validate_frame(f), Ok(()));
        }

        // After it: the player is back where the scripts left them, the
        // shots and hits are this tick's only, and nothing moved "fast".
        // 10 units in 0.2 s from the frame before the cutscene: a speed,
        // were the cut not forgotten.
        session.sample(&state(15, [10.0, 0.0, 0.0], false));
        session.sample(&state(16, [10.0, 0.0, 0.0], false));
        let after = frames(&mut session);
        assert_eq!(after.len(), 1);
        let f = &after[0];
        assert!(!f.cinematic);
        assert_eq!(f.tick, 16);
        assert_eq!(f.players[0].pos, [10.0, 0.0, 0.0]);
        assert_eq!(f.players[0].speed, None, "no speed across the cut");
        assert_eq!(f.players[0].actions, None);
        assert_eq!(f.players[0].shots, Some(4), "ticks 15 and 16 only");
        assert_eq!(f.hits.len(), 2, "ticks 15 and 16 only");
        assert_eq!(f.actors.len(), 1);

        // A kill outside the cutscene counts again.
        session.kill(17, None, 0, None, None, false);
        assert_eq!(kinds(&mut session), ["PlayerKilled"]);
    }

    #[test]
    fn campaign_reverts_continue_the_same_game() {
        let mut session = Session::new("test", None);
        session.start(&campaign(), vec![player(0, 0)]);
        assert!(session.is_campaign());
        let id = session.game_id().unwrap().to_owned();
        assert_eq!(kinds(&mut session), ["GameStarted"]);
        session.sample(&snapshot(600, vec![player(0, 0)]));
        session.take_output();
        // A checkpoint revert: same map, the clock goes back.
        let mut revert = campaign();
        revert.tick = 400;
        session.start(&revert, vec![player(0, 0)]);
        assert!(kinds(&mut session).is_empty(), "no new game");
        assert_eq!(session.game_id(), Some(id.as_str()));
        session.sample(&snapshot(402, vec![player(0, 0)]));
        assert!(
            frames(&mut session)[0].tick > 600,
            "the game's clock keeps counting"
        );

        // The next mission is a new game; so is a multiplayer map.
        let mut next = campaign();
        next.map = "03b_newmombasa".into();
        session.start(&next, vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameEnded", "GameStarted"]);
        assert_ne!(session.game_id(), Some(id.as_str()));
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameEnded", "GameStarted"]);
        // Multiplayer re-activations stay separate games, as before.
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(kinds(&mut session), ["GameEnded", "GameStarted"]);
    }

    /// Regression: a checkpoint revert or mission restart winds the
    /// engine's clock back, and the frames after it used to carry the
    /// rewound ticks. A replay orders frames by tick, so the two runs
    /// interleaved and the player flicked between them.
    #[test]
    fn a_campaign_revert_does_not_wind_the_games_ticks_back() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x9001_0001;
        session.start(&campaign(), vec![p.clone()]);
        session.take_output();
        let at = |tick: u64, x: f64| {
            let mut s = snapshot(tick, vec![p.clone()]);
            s.has_stats = false;
            s.objects[0] = Some(ObjectState {
                pos: [x, 0.0, 0.0],
                forward: [1.0, 0.0, 0.0],
                ..ObjectState::default()
            });
            s
        };
        let mut sent = Vec::new();
        for tick in (600..=660).step_by(2) {
            session.sample(&at(tick, 100.0));
            sent.extend(frames(&mut session));
        }
        // The player dies and the game reverts to a checkpoint 20 seconds
        // back, then to the mission's start.
        session.kill(661, None, 0, None, None, false);
        let mut revert = campaign();
        revert.tick = 61;
        session.start(&revert, vec![p.clone()]);
        let mut events = session.take_output();
        for tick in (62..=120).step_by(2) {
            session.sample(&at(tick, 5.0));
            sent.extend(frames(&mut session));
        }
        revert.tick = 3;
        session.start(&revert, vec![p.clone()]);
        for tick in (4..=30).step_by(2) {
            session.sample(&at(tick, -50.0));
            sent.extend(frames(&mut session));
        }
        session.kill(31, None, 0, None, None, false);
        session.end(&at(32, -50.0));
        events.extend(session.take_output());

        let ticks: Vec<u64> = sent.iter().map(|f| f.tick).collect();
        // The first frame of each new run says where the engine came back to.
        let cuts: Vec<(usize, u64)> = sent
            .iter()
            .enumerate()
            .filter_map(|(i, f)| f.cut.map(|c| (i, c)))
            .collect();
        let starts: Vec<usize> = (1..sent.len())
            .filter(|i| sent[*i].players[0].pos[0] != sent[i - 1].players[0].pos[0])
            .collect();
        assert_eq!(cuts, vec![(starts[0], 61), (starts[1], 3)]);
        assert!(
            ticks.windows(2).all(|w| w[0] < w[1]),
            "frame ticks only go up: {ticks:?}"
        );
        assert!(
            ticks.iter().all(|t| t % 2 == 0),
            "frames stay on even ticks"
        );
        // Sorted by tick, each run stays whole: no flicking between them.
        let mut by_tick = sent.clone();
        by_tick.sort_by_key(|f| f.tick);
        let xs: Vec<f64> = by_tick.iter().map(|f| f.players[0].pos[0]).collect();
        let runs = xs.windows(2).filter(|w| w[0] != w[1]).count();
        assert_eq!(runs, 2, "three runs, two cuts: {xs:?}");
        // Events are on the same clock as the frames.
        let tick_of = |kind: &str, n: usize| {
            events
                .iter()
                .filter_map(|o| match o {
                    Output::Event(e) if e.kind == kind => e.data["tick"].as_u64(),
                    _ => None,
                })
                .nth(n)
                .unwrap()
        };
        assert_eq!(tick_of("PlayerKilled", 0), 661);
        let last = *ticks.last().unwrap();
        assert!(tick_of("PlayerKilled", 1) > last);
        assert_eq!(tick_of("GameEnded", 0), tick_of("PlayerKilled", 1) + 1);
        assert_eq!(tick_of("PlayerKilled", 1) - last, 1);
    }

    /// A player whose unit cannot be read (a stale datum while the game
    /// swaps the BSP, a cutscene) was sent at (0, 0, 0); a dead player who
    /// never stood anywhere too.
    #[test]
    fn a_player_with_no_known_place_is_left_out_not_sent_at_the_origin() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x9001_0001;
        let mut q = player(1, 0);
        q.unit = u32::MAX;
        session.start(&campaign(), vec![p.clone(), q.clone()]);
        session.take_output();
        let mut state = snapshot(2, vec![p.clone(), q.clone()]);
        state.objects[0] = Some(ObjectState {
            pos: [7.0, 8.0, 9.0],
            forward: [0.0, 1.0, 0.0],
            ..ObjectState::default()
        });
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert_eq!(frame.players.len(), 1, "player 1 has never stood anywhere");
        assert_eq!(frame.players[0].pos, [7.0, 8.0, 9.0]);

        // The unit is still the player's, but unreadable this frame: it
        // stays where it was last seen.
        let mut state = snapshot(4, vec![p.clone(), q.clone()]);
        state.objects[0] = None;
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert_eq!(frame.players.len(), 1);
        assert_eq!(frame.players[0].pos, [7.0, 8.0, 9.0]);
        assert!((frame.players[0].yaw - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert!(frame.players[0].alive);

        // A new unit (a respawn) that cannot be read yet is nowhere known.
        p.unit = 0x9002_0002;
        let state = snapshot(6, vec![p.clone(), q.clone()]);
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert!(frame.players.is_empty(), "{:?}", frame.players);
    }

    fn started(session: &mut Session) -> live_proto::game::NewGame {
        let event = session
            .take_output()
            .into_iter()
            .find_map(|o| match o {
                Output::Event(e) if e.kind == "GameStarted" => Some(e),
                _ => None,
            })
            .expect("a GameStarted");
        let game: live_proto::game::GameStarted = serde_json::from_value(event.data).unwrap();
        assert_eq!(live_proto::game::validate_new(&game.game), Ok(()));
        game.game
    }

    #[test]
    fn games_carry_the_loaded_maps_id() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(started(&mut session).map_id, None, "the disc's map");
        let id = live_proto::maps::MapId([3; 32]);
        session.set_map_id(Some(id));
        session.start(&info(), vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.map_id, Some(id));
        assert_eq!(game.map, info().map, "map stays the base map's name");
    }

    #[test]
    fn games_carry_their_skulls() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(started(&mut session).skulls, None, "none on is left out");
        // Multiplayer: the host's set as the boot applied it.
        session.set_multiplayer_skulls(Some(vec![6, 4]));
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(started(&mut session).skulls, Some(vec![4, 6]));
        // A skull that does nothing in multiplayer is never claimed.
        session.set_multiplayer_skulls(Some(vec![12]));
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(started(&mut session).skulls, None);
        // Campaign: the array as read, whatever the multiplayer set.
        session.set_multiplayer_skulls(Some(vec![6]));
        let campaign = GameInfo {
            map: "03a_oldmombasa".into(),
            mode: "campaign".into(),
            skulls: Some(vec![0, 12]),
            ..info()
        };
        session.start(&campaign, vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.skulls, Some(vec![0, 12]));
        assert_eq!(live_proto::game::validate_new(&game), Ok(()));
        // Our own skulls join the array's in a campaign, never a
        // multiplayer game; engine indices handed in that way are ignored.
        session.set_campaign_custom_skulls(Some(vec![20, 3, 15]));
        // (The next mission: the same one again continues its game.)
        let next = GameInfo {
            map: "03b_newmombasa".into(),
            ..campaign.clone()
        };
        session.start(&next, vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.skulls, Some(vec![0, 12, 15, 20]));
        assert_eq!(live_proto::game::validate_new(&game), Ok(()));
        session.start(&info(), vec![player(0, 0)]);
        assert_eq!(started(&mut session).skulls, Some(vec![6]));
        session.set_campaign_custom_skulls(Some(vec![17]));
        let bare = GameInfo {
            skulls: None,
            ..campaign
        };
        session.start(&bare, vec![player(0, 0)]);
        assert_eq!(started(&mut session).skulls, Some(vec![17]));
    }

    #[test]
    fn games_say_their_kind_playlist_and_difficulty() {
        let mut session = Session::new("test", None);
        session.start(&campaign(), vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.kind.as_deref(), Some("campaign"));
        assert_eq!(game.difficulty.as_deref(), Some("heroic"));
        assert_eq!((game.playlist, game.online), (None, None));

        session.start(&info(), vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.kind.as_deref(), Some("custom"));
        assert_eq!((game.playlist, game.difficulty), (None, None));

        session.set_playlist(Some(Playlist {
            name: Some("Team Slayer".into()),
            id: "3".into(),
            session_id: Some("0123456789abcdef".into()),
            clankers: false,
            frames_from_host: false,
        }));
        session.start(&info(), vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.kind.as_deref(), Some("matchmaking"));
        assert_eq!(game.playlist.as_deref(), Some("Team Slayer"));
        assert_eq!(game.playlist_id.as_deref(), Some("3"));
        assert_eq!(game.session_id.as_deref(), Some("0123456789abcdef"));

        // A clanker hopper's game is a Clankermatch, with its playlist and
        // session as any matchmade game.
        session.set_playlist(Some(Playlist {
            name: None,
            id: "9".into(),
            session_id: Some("00000000000000c1".into()),
            clankers: true,
            frames_from_host: false,
        }));
        session.start(&info(), vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.kind.as_deref(), Some("clankermatch"));
        assert_eq!(game.playlist_id.as_deref(), Some("9"));
        assert_eq!(game.session_id.as_deref(), Some("00000000000000c1"));

        // A playlist never makes a campaign game a matchmaking one.
        session.start(&campaign(), vec![player(0, 0)]);
        let game = started(&mut session);
        assert_eq!(game.kind.as_deref(), Some("campaign"));
        assert_eq!(game.playlist, None);
    }

    #[test]
    fn dedicated_join_skips_frames_only_for_the_joined_game() {
        let mut session = Session::new("test", None).with_signed_in(Some(live_proto::Xuid(1)));
        let playlist = Playlist {
            name: None,
            id: "3".into(),
            session_id: Some("0123456789abcdef".into()),
            clankers: false,
            frames_from_host: true,
        };
        session.set_playlist(Some(playlist.clone()));
        session.start(&info(), vec![player(0, 0)]);
        session.take_output();

        // The next game's decision cannot change delivery mid-match.
        session.set_playlist(None);
        let scored = snapshot(30, vec![player(0, 1), player(1, 0)]);
        session.sample(&scored);
        let output = session.take_output();
        assert!(output.iter().all(|o| matches!(o, Output::Event(_))));
        let event_kinds: Vec<_> = output.iter().filter_map(|o| match o {
            Output::Event(e) => Some(e.kind.as_str()),
            Output::Frame(_) => None,
        }).collect();
        assert!(event_kinds.contains(&live_proto::game::SCORE_CHANGED));
        assert!(event_kinds.contains(&live_proto::game::PLAYER_JOINED));
        let mut cinematic = scored.clone();
        cinematic.tick = 32;
        cinematic.cinematic = true;
        session.sample(&cinematic);
        assert!(frames(&mut session).is_empty());
        session.end(&scored);
        assert!(kinds(&mut session).iter().any(|k| k == live_proto::game::GAME_ENDED));

        session.set_playlist(Some(Playlist {
            frames_from_host: false,
            ..playlist.clone()
        }));
        session.start(&info(), vec![]);
        session.sample(&snapshot(2, vec![]));
        assert_eq!(frames(&mut session).len(), 1, "the next peer game uploads frames");

        let mut local_capture = Session::new("test", None);
        local_capture.set_playlist(Some(playlist));
        local_capture.start(&info(), vec![]);
        local_capture.sample(&snapshot(2, vec![]));
        assert_eq!(frames(&mut local_capture).len(), 1, "local capture still records");
    }

    fn actor(slot: u32, pos: [f64; 3], team: i16) -> crate::event::ActorState {
        crate::event::ActorState {
            datum: 0x9000_0000 | (slot << 16) | slot,
            object: ObjectState {
                pos,
                forward: [0.0, 1.0, 0.0],
                up: [0.0, 0.0, 1.0],
                definition: Some(0xe100_0011),
                team: Some(team),
                ..ObjectState::default()
            },
        }
    }

    fn campaign_names() -> TagNames {
        TagNames::new(
            1,
            [
                (
                    0xe100_0011,
                    *b"bipd",
                    "objects\\characters\\grunt\\grunt".to_string(),
                ),
                (
                    0xe100_0020,
                    *b"weap",
                    "objects\\weapons\\pistol\\plasma_pistol\\plasma_pistol".to_string(),
                ),
                (
                    0xe100_0030,
                    *b"vehi",
                    "objects\\vehicles\\ghost\\ghost".to_string(),
                ),
            ],
        )
    }

    #[test]
    fn the_dead_lie_at_their_bodies_and_ai_bodies_stay_until_cleared() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x9001_0001;
        session.start(&campaign(), vec![p.clone()]);
        session.set_tag_names(campaign_names());
        session.take_output();
        let standing = |pos| ObjectState {
            pos,
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            ..ObjectState::default()
        };
        let mut state = snapshot(2, vec![p.clone()]);
        state.objects[0] = Some(standing([3.0, 4.0, 5.0]));
        session.sample(&state);
        frames(&mut session);

        // The player dies; their body has slid a little. A grunt's body and
        // a living grunt are about.
        let mut dead = p.clone();
        dead.unit = u32::MAX;
        let mut state = snapshot(4, vec![dead.clone()]);
        let mut own = actor(1, [3.5, 4.0, 5.0], 0);
        own.datum = 0x9001_0001;
        own.object.forward = [0.0, 1.0, 0.0];
        let grunt_body = actor(2, [9.0, 0.0, 0.0], 3);
        state.bodies = vec![own, grunt_body];
        state.actors = vec![actor(3, [20.0, 0.0, 0.0], 3)];
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        let me = &frame.players[0];
        assert!(!me.alive);
        assert_eq!(me.pos, [3.5, 4.0, 5.0], "at the body");
        assert!((me.yaw - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(me.stance.as_deref(), Some("dead"));
        // The living first, then the AI body as a dead actor; the player's
        // own body is not an actor.
        assert_eq!(frame.actors.len(), 2);
        assert_eq!(frame.actors[0].id, "90030003");
        assert_ne!(frame.actors[0].stance.as_deref(), Some("dead"));
        let body = &frame.actors[1];
        assert_eq!(body.id, "90020002");
        assert_eq!(body.stance.as_deref(), Some("dead"));
        assert_eq!(body.species, "grunt");
        assert_eq!(body.pos, [9.0, 0.0, 0.0]);
        assert_eq!(body.team.as_deref(), Some("covenant"));
        assert_eq!(live_proto::game::validate_frame(&frame), Ok(()));

        // The game clears the player's body away: they lie where they fell.
        let mut state = snapshot(6, vec![dead]);
        state.bodies = vec![];
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert_eq!(frame.players[0].pos, [3.0, 4.0, 5.0]);
        assert!(frame.actors.is_empty(), "the grunt's body is gone too");
    }

    #[test]
    fn bodies_are_capped_within_the_actors() {
        let mut session = Session::new("test", None);
        session.start(&campaign(), vec![player(0, 0)]);
        session.take_output();
        let mut state = snapshot(2, vec![player(0, 0)]);
        state.actors = (0..MAX_FRAME_ACTORS as u32 - 4)
            .map(|i| actor(i, [0.0; 3], 3))
            .collect();
        state.bodies = (0..10)
            .map(|i| actor(200 + i, [i as f64, 0.0, 0.0], 3))
            .collect();
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert_eq!(frame.actors.len(), MAX_FRAME_ACTORS);
        let dead = frame
            .actors
            .iter()
            .filter(|a| a.stance.as_deref() == Some("dead"))
            .count();
        assert_eq!(dead, 4, "the nearest bodies fill what room is left");
        state.actors.clear();
        state.tick = 4;
        state.bodies = (0..100)
            .map(|i| actor(200 + i, [i as f64, 0.0, 0.0], 3))
            .collect();
        session.sample(&state);
        assert_eq!(
            frames(&mut session).remove(0).actors.len(),
            MAX_FRAME_BODIES
        );
    }

    #[test]
    fn campaign_frames_carry_actors_and_occupied_vehicles() {
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x90010000;
        session.start(&campaign(), vec![p.clone()]);
        session.set_tag_names(campaign_names());
        session.take_output();

        let mut state = snapshot(1, vec![p.clone()]);
        state.objects[0] = Some(ObjectState {
            pos: [0.0; 3],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            ..ObjectState::default()
        });
        // An odd tick's shots wait for the frame.
        state.shots = [(0x9009_0009, 2)].into();
        session.sample(&state);
        assert!(frames(&mut session).is_empty());

        let far = actor(3, [50.0, 0.0, 0.0], 3);
        let mut near = actor(4, [5.0, 0.0, 0.0], 2);
        near.object.weapon = Some(crate::event::Weapon {
            datum: 0x9009_0009,
            definition: 0xe100_0020,
        });
        near.object.aim = Some([1.0, 0.0, 0.0]);
        let mut driver = actor(5, [20.0, 0.0, 1.0], 3);
        driver.object.vehicle = Some(0x9006_0006);
        driver.object.vehicle_definition = Some(0xe100_0030);
        driver.object.up = [0.0, 0.6, 0.8];
        let mut nameless = actor(7, [30.0, 0.0, 0.0], 12);
        nameless.object.definition = Some(0xe100_0099);
        state.tick = 2;
        state.shots = [(0x9009_0009, 1)].into();
        state.actors = vec![far, near, driver, nameless];
        session.sample(&state);
        let frame = frames(&mut session).remove(0);

        let ids: Vec<&str> = frame.actors.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            ["90040004", "90050005", "90070007", "90030003"],
            "nearest first"
        );
        let near = &frame.actors[0];
        assert_eq!(near.species, "grunt");
        assert_eq!(near.team.as_deref(), Some("human"));
        assert_eq!(near.weapon.as_deref(), Some("plasma_pistol"));
        assert_eq!(near.shots, Some(3), "both ticks' shots");
        assert_eq!(near.aim, Some([1.0, 0.0, 0.0]));
        assert!((near.yaw - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
        assert_eq!(frame.actors[1].team.as_deref(), Some("covenant"));
        assert_eq!(frame.actors[1].vehicle.as_deref(), Some("90060006"));
        assert_eq!(frame.actors[2].species, "unknown");
        assert_eq!(frame.actors[2].team.as_deref(), Some("12"), "raw index");
        assert_eq!(
            frame.vehicles,
            [FrameVehicle {
                id: "90060006".into(),
                kind: "ghost".into(),
                pos: [20.0, 0.0, 1.0],
                forward: [0.0, 1.0, 0.0],
                up: [0.0, 0.6, 0.8],
                occupied: true,
                parent: None,
                aim: None,
                vel: None,
            }]
        );
        assert_eq!(live_proto::game::validate_frame(&frame), Ok(()));

        // Shots were handed out; the next frame has none.
        state.tick = 4;
        state.shots.clear();
        session.sample(&state);
        assert_eq!(frames(&mut session)[0].actors[0].shots, None);
    }

    #[test]
    fn actor_count_is_capped_and_vehicles_are_listed_once() {
        let mut session = Session::new("test", None);
        session.start(&campaign(), vec![player(0, 0)]);
        session.take_output();
        let mut state = snapshot(2, vec![player(0, 0)]);
        state.actors = (0..MAX_FRAME_ACTORS as u32 + 20)
            .map(|i| {
                let mut a = actor(i + 1, [i as f64, 0.0, 0.0], 3);
                // Every actor rides the same vehicle; one has a bad up.
                a.object.vehicle = Some(0x9fff_0fff);
                if i == 0 {
                    a.object.up = [f64::NAN, 0.0, 0.0];
                }
                a
            })
            .collect();
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert_eq!(frame.actors.len(), MAX_FRAME_ACTORS);
        assert_eq!(frame.vehicles.len(), 1);
        assert_eq!(frame.vehicles[0].kind, "unknown", "no names set");
        assert_eq!(frame.vehicles[0].up, [0.0, 0.0, 1.0]);
        assert_eq!(live_proto::game::validate_frame(&frame), Ok(()));
    }

    #[test]
    fn multiplayer_teams_stay_numbers() {
        assert_eq!(team_name(3, false), "3");
        assert_eq!(team_name(3, true), "covenant");
        assert_eq!(team_name(1, true), "player");
    }

    #[test]
    fn unchanged_initial_teams_do_not_create_a_score_event() {
        let mut session = Session::new("test", None);
        let teams = vec![live_proto::game::TeamScore { team: 0, score: 0 }];
        session.start_with_teams(&info(), vec![player(0, 0)], teams.clone());
        session.take_output();
        let mut state = snapshot(30, vec![player(0, 0)]);
        state.teams = teams;
        session.sample(&state);
        assert!(kinds(&mut session).is_empty());
    }

    #[test]
    fn team_winner_and_ties() {
        let mut state = snapshot(1, vec![player(0, 5), player(1, 2)]);
        state.teams = vec![
            live_proto::game::TeamScore { team: 0, score: 7 },
            live_proto::game::TeamScore { team: 1, score: 2 },
        ];
        assert_eq!(
            state.winner(true),
            Some(live_proto::game::Winner::Team { team: 0 })
        );
        state.teams[1].score = 7;
        assert_eq!(state.winner(true), None);
    }

    #[test]
    fn an_unchanged_roster_is_the_same_once_linked() {
        let xuid = live_proto::Xuid::parse("0009a1b2c3d4e5f6").unwrap();
        let fresh = vec![player(0, 1), player(1, 2)];
        let mut current = fresh.clone();
        link(Some(xuid), None, &mut current, false);
        assert!(same_roster(Some(xuid), None, &current, &fresh, false));
        assert!(
            !same_roster(None, None, &current, &fresh, false),
            "the link is part of it"
        );
        let mut renamed = fresh.clone();
        renamed[1].player.name = "Q".into();
        assert!(!same_roster(Some(xuid), None, &current, &renamed, false));
        let mut scored = fresh.clone();
        scored[0].stats.kills = 1;
        assert!(!same_roster(Some(xuid), None, &current, &scored, false));
        assert!(!same_roster(Some(xuid), None, &current, &fresh[..1], false));
    }

    #[test]
    fn a_stats_change_reaches_the_roster_the_end_reports() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        session.take_output();
        session.sample(&snapshot(2, vec![player(0, 5)]));
        assert_eq!(session.current.as_ref().unwrap().players[0].stats.score, 5);
    }

    #[test]
    fn the_signed_in_account_is_the_player_on_local_slot_zero() {
        let xuid = live_proto::Xuid::parse("0009a1b2c3d4e5f6").unwrap();
        let mut session = Session::new("test", None).with_signed_in(Some(xuid));
        // Slot 0 is the signed-in local player; 1 is remote; 2 is a second
        // local player on another controller (not the signed-in account).
        let mut guest = player(2, 0);
        guest.local_user = Some(1);
        session.start(&info(), vec![player(0, 0), player(1, 0)]);
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        assert_eq!(started.data["players"][0]["xuid"], "0009a1b2c3d4e5f6");
        assert_eq!(
            started.data["players"][0]["name"], "P0",
            "the in-game name stays"
        );
        assert!(started.data["players"][1].get("xuid").is_none());

        session.sample(&snapshot(2, vec![player(0, 0), player(1, 0), guest]));
        let outputs = session.take_output();
        let joined: Vec<_> = outputs
            .iter()
            .filter_map(|o| match o {
                Output::Event(e) if e.kind == "PlayerJoined" => Some(e),
                _ => None,
            })
            .collect();
        assert_eq!(
            joined.len(),
            1,
            "linking does not make the roster look changed"
        );
        assert!(joined[0].data["player"].get("xuid").is_none());

        // A player who rejoins on slot 0 (a new datum) is linked again.
        let mut back = player(0, 0);
        back.datum ^= 0x0001_0000;
        session.sample(&snapshot(4, vec![back]));
        let rejoin = session
            .take_output()
            .into_iter()
            .find_map(|o| match o {
                Output::Event(e) if e.kind == "PlayerJoined" => Some(e),
                _ => None,
            })
            .unwrap();
        assert_eq!(rejoin.data["player"]["xuid"], "0009a1b2c3d4e5f6");
    }

    /// Phantom bots from a server roster (docs/game/bots.md stage 6c):
    /// found by machine id, each carries its rostered xuid and `bot:
    /// true`, on the game's start and when it joins later; the game names
    /// the roster's session. A local-port Clanker and a phantom with a
    /// host-made xuid are bots too, with no xuid.
    #[test]
    fn rostered_phantoms_are_bots_with_their_xuids() {
        use live_proto::Xuid;
        let me = Xuid::parse("0009a1b2c3d4e5f6").unwrap();
        let (a, b, c) = (
            [0xc1, 0xa4, 0, 0, 0, 1],
            [0xc1, 0xa4, 0, 0, 0, 2],
            [0xc1, 0xa4, 0, 0, 0, 3],
        );
        let (bot_a, bot_c) = (
            Xuid(live_proto::bots::SERVER_XUID_MARK | 1),
            Xuid(live_proto::bots::SERVER_XUID_MARK | 3),
        );
        let mut session = Session::new("test", None).with_signed_in(Some(me));
        session.set_bots(Some(Bots {
            session_id: Some("0123456789abcdef".into()),
            machines: vec![(a, bot_a), (b, live_proto::bots::local_xuid(2)), (c, bot_c)],
        }));
        let mut port = player(1, 0);
        port.local_user = Some(1);
        port.player.local = Some(true);
        port.controller = Some(1);
        port.machine = [0x00, 0x50, 0xf2, 1, 2, 3];
        let mut human = player(0, 0);
        human.machine = port.machine;
        let mut pa = player(2, 0);
        pa.machine = a;
        let roster = vec![human, port, pa];
        session.start(&info(), roster.clone());
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        let game = &started.data;
        assert_eq!(game["session_id"], "0123456789abcdef");
        assert_eq!(game["kind"], "custom");
        let players = &game["players"];
        assert_eq!(players[0]["xuid"], me.to_string());
        assert!(players[0].get("bot").is_none());
        assert!(players[1].get("xuid").is_none(), "a second local player");
        assert!(players[1].get("bot").is_none());
        assert_eq!(players[2]["xuid"], bot_a.to_string());
        assert_eq!(players[2]["bot"], true);

        // Two more phantoms join once the game runs: one rostered, one
        // with a host-made xuid, which is never sent.
        let mut pb = player(3, 0);
        pb.machine = b;
        let mut pc = player(4, 0);
        pc.machine = c;
        let mut later = roster.clone();
        later.extend([pb, pc]);
        session.sample(&snapshot(2, later.clone()));
        let joined: Vec<_> = session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) if e.kind == "PlayerJoined" => Some(e.data["player"].clone()),
                _ => None,
            })
            .collect();
        assert_eq!(joined.len(), 2, "{joined:?}");
        assert!(joined[0].get("xuid").is_none());
        assert_eq!(joined[0]["bot"], true, "a host-made phantom");
        assert_eq!(joined[1]["xuid"], bot_c.to_string());
        assert_eq!(joined[1]["bot"], true);
        for p in joined.iter().chain(players.as_array().unwrap()) {
            let x = p.get("xuid").and_then(|x| x.as_str()).unwrap_or("");
            assert!(!x.starts_with("000e"), "a host-made xuid sent: {x}");
        }
        // Marking does not make the roster look changed.
        session.sample(&snapshot(4, later));
        assert!(!kinds(&mut session).contains(&"PlayerJoined".to_string()));
    }

    /// The run 20261119-020015: "6 players, 3 bots". The roster changed
    /// mid-game, so its phantoms were new machines the start's list did
    /// not name, and they went out as humans. A phantom's machine id says
    /// it is a bot (`live_proto::bots::is_phantom_machine`), listed or not;
    /// only a listed one, with its roster in force, carries an xuid. A
    /// remote human stays a human.
    #[test]
    fn a_phantom_the_start_did_not_list_is_still_a_bot() {
        let listed = [0xc1, 0xa4, 0, 0, 0, 1];
        let rostered = live_proto::Xuid(live_proto::bots::SERVER_XUID_MARK | 1);
        let mut session = Session::new("test", None);
        session.set_bots(Some(Bots {
            session_id: Some("0123456789abcdef".into()),
            machines: vec![(listed, rostered)],
        }));
        // Player 0 is this console's own in `player`; here it is a
        // remote phantom.
        let mut pa = player(0, 0);
        pa.machine = listed;
        pa.local_user = None;
        pa.controller = None;
        pa.player.local = Some(false);
        let mut human = player(1, 0);
        human.machine = [0x00, 0x50, 0xf2, 9, 9, 9];
        let roster = vec![pa, human];
        session.start(&info(), roster.clone());
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        let players = &started.data["players"];
        assert_eq!(players[0]["bot"], true);
        assert_eq!(players[0]["xuid"], rostered.to_string());
        assert!(players[1].get("bot").is_none(), "a remote human");

        // Joins after the start on a machine the list never had.
        let mut late = player(2, 0);
        late.machine = [0xc1, 0xa4, 0, 0, 0, 7];
        let mut later = roster;
        later.push(late);
        session.sample(&snapshot(2, later));
        let joined: Vec<_> = session
            .take_output()
            .into_iter()
            .filter_map(|o| match o {
                Output::Event(e) if e.kind == "PlayerJoined" => Some(e.data["player"].clone()),
                _ => None,
            })
            .collect();
        assert_eq!(joined.len(), 1, "{joined:?}");
        assert_eq!(joined[0]["bot"], true);
        assert!(
            joined[0].get("xuid").is_none(),
            "no account it was listed with"
        );
    }

    /// Host-made phantoms carry no account. They still appear in the
    /// roster, scores, positions and final standings under their slots.
    #[test]
    fn host_made_phantoms_are_bots_without_an_account() {
        use live_proto::Xuid;
        let me = Xuid::parse("0009a1b2c3d4e5f6").unwrap();
        let mut session = Session::new("test", None).with_signed_in(Some(me));
        session.set_bots(Some(Bots::default()));
        let mut roster = vec![player(0, 0)];
        for slot in 1..=2 {
            let mut bot = player(slot, 0);
            bot.player.name = format!("Clanker {slot}");
            bot.machine = [0xc1, 0xa4, 0, 0, 0, slot as u8];
            bot.unit = 0xe000_0000 | slot;
            roster.push(bot);
        }
        session.start(&info(), roster.clone());
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        let game = &started.data;
        assert!(game.get("session_id").is_none(), "no roster, no session");
        let players = game["players"].as_array().unwrap();
        assert_eq!(players[0]["xuid"], me.to_string());
        assert!(players[0].get("bot").is_none());
        for (p, name) in players[1..].iter().zip(["Clanker 1", "Clanker 2"]) {
            assert_eq!(p["name"], name);
            assert_eq!(p["bot"], true);
            assert!(p.get("xuid").is_none(), "a Clanker has no account");
        }
        // The server takes the start as it is.
        let new: live_proto::game::NewGame = serde_json::from_value(game.clone()).unwrap();
        assert_eq!(live_proto::game::validate_new(&new), Ok(()));

        // Clanker 1 scores and moves: its score and position go out under
        // its slot like anyone's.
        let mut later = roster.clone();
        later[1].stats.kills = 2;
        later[1].stats.score = 2;
        later[0].stats.deaths = 2;
        let mut snap = snapshot(30, later.clone());
        snap.objects[1] = Some(ObjectState {
            pos: [4.0, 5.0, 6.0],
            forward: [1.0, 0.0, 0.0],
            ..ObjectState::default()
        });
        session.sample(&snap);
        let out = session.take_output();
        assert!(
            !out.iter()
                .any(|o| matches!(o, Output::Event(e) if e.kind == "PlayerJoined")),
            "marking a bot does not make the roster look changed"
        );
        let score = out
            .iter()
            .find_map(|o| match o {
                Output::Event(e) if e.kind == "ScoreChanged" => Some(e.data.clone()),
                _ => None,
            })
            .expect("a score change");
        let row = score["players"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["index"] == 1)
            .unwrap()
            .clone();
        assert_eq!(row["kills"], 2);
        let frame = out
            .iter()
            .find_map(|o| match o {
                Output::Frame(f) => Some(f.clone()),
                _ => None,
            })
            .expect("a frame");
        let bot = frame
            .players
            .iter()
            .find(|p| p.index == 1)
            .expect("the bot in the frame");
        assert_eq!(bot.pos, [4.0, 5.0, 6.0]);

        session.end(&snapshot(60, later));
        let ended = session
            .take_output()
            .into_iter()
            .find_map(|o| match o {
                Output::Event(e) if e.kind == "GameEnded" => Some(e.data),
                _ => None,
            })
            .expect("the game ends");
        let final_row = ended["players"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["index"] == 1)
            .unwrap()
            .clone();
        assert_eq!(final_row["kills"], 2);
    }

    /// A rostered phantom's xuid goes out only with its roster's session:
    /// without one the server would refuse the record.
    #[test]
    fn a_phantom_out_of_its_roster_is_a_bot_without_an_xuid() {
        let m = [0xc1, 0xa4, 0, 0, 0, 1];
        let mut session = Session::new("test", None);
        session.set_bots(Some(Bots {
            session_id: None,
            machines: vec![(m, live_proto::Xuid(live_proto::bots::SERVER_XUID_MARK | 1))],
        }));
        let mut p = player(1, 0);
        p.machine = m;
        session.start(&info(), vec![player(0, 0), p]);
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        assert!(started.data.get("session_id").is_none());
        let bot = &started.data["players"][1];
        assert_eq!(bot["bot"], true);
        assert!(bot.get("xuid").is_none());
    }

    #[test]
    fn without_a_bot_roster_a_custom_game_names_no_session() {
        let mut session = Session::new("test", None);
        let mut p = player(1, 0);
        p.machine = [0xc1, 0xa4, 0, 0, 0, 1];
        session.start(&info(), vec![player(0, 0), p]);
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        assert!(started.data.get("session_id").is_none());
        assert_eq!(started.data["kind"], "custom");
        assert!(started.data["players"][1].get("bot").is_none());
    }

    #[test]
    fn without_a_signed_in_account_players_have_no_xuid() {
        let mut session = Session::new("test", None);
        session.start(&info(), vec![player(0, 0)]);
        let outputs = session.take_output();
        let Output::Event(started) = &outputs[0] else {
            panic!()
        };
        assert!(started.data["players"][0].get("xuid").is_none());
    }

    fn machine(slot: u32, pos: [f64; 3], position: Option<f64>) -> MachineState {
        MachineState {
            datum: 0x9000_0000 | (slot << 16) | slot,
            definition: Some(0xe100_0030),
            placement: Some(slot as u16),
            pos,
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            position,
        }
    }

    fn world_names() -> TagNames {
        TagNames::new(
            0xe000_0001,
            [
                (
                    0xe100_0030,
                    *b"mach",
                    "scenarios\\objects\\covenant\\military\\scarab\\scarab".to_string(),
                ),
                (
                    0xe100_0040,
                    *b"jpt!",
                    "objects\\weapons\\melee\\big_punch".to_string(),
                ),
                (
                    0xe100_0050,
                    *b"vehi",
                    "objects\\vehicles\\warthog\\warthog".to_string(),
                ),
            ],
        )
    }

    #[test]
    fn frames_carry_slow_projectiles_and_loose_items() {
        use crate::event::{ItemState, ProjectileState};
        let names = TagNames::new(
            0,
            [
                (
                    0xe100_0060,
                    *b"proj",
                    "objects\\weapons\\grenade\\plasma_grenade\\plasma_grenade".to_string(),
                ),
                (
                    0xe100_0070,
                    *b"weap",
                    "objects\\weapons\\multiplayer\\flag\\flag".to_string(),
                ),
                (
                    0xe100_0080,
                    *b"eqip",
                    "objects\\powerups\\overshield\\overshield".to_string(),
                ),
            ],
        );
        let mut state = snapshot(2, Vec::new());
        let datum = |slot: u32| 0x9000_0000 | (slot << 16) | slot;
        let projectile = |slot: u32, velocity, parent| ProjectileState {
            datum: datum(slot),
            definition: Some(0xe100_0060),
            pos: [slot as f64, 0.0, 1.0],
            velocity,
            parent,
        };
        state.projectiles = vec![
            projectile(1, Some([5.0, 0.0, 2.0]), None),
            // A bullet: the viewer draws it from its shooter's fire.
            projectile(2, Some([400.0, 0.0, 0.0]), None),
            projectile(3, None, Some(0x9001_0001)),
            // No velocity read and not stuck: left out.
            projectile(4, None, None),
        ];
        let item = |slot: u32, definition: u32, equipment: bool| ItemState {
            datum: datum(slot),
            definition: Some(definition),
            equipment,
            pos: [slot as f64, 1.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        };
        state.items = vec![
            item(5, 0xe100_0070, false),
            item(6, 0xe100_0080, true),
            item(7, 0xe100_0099, false),
        ];
        let projectiles = frame_projectiles(&state, Some(&names));
        let seen: Vec<_> = projectiles
            .iter()
            .map(|p| (p.id.as_str(), p.kind.as_str(), p.parent.as_deref()))
            .collect();
        assert_eq!(
            seen,
            [
                ("90010001", "plasma_grenade", None),
                ("90030003", "plasma_grenade", Some("90010001")),
            ]
        );
        assert_eq!(projectiles[0].vel, Some([5.0, 0.0, 2.0]));
        let items = frame_items(&state, Some(&names));
        let seen: Vec<_> = items
            .iter()
            .map(|i| (i.id.as_str(), i.kind.as_str(), i.equipment))
            .collect();
        assert_eq!(
            seen,
            [
                ("90050005", "flag", false),
                ("90060006", "overshield", true),
                ("90070007", "unknown", false),
            ]
        );
        let frame = Frame {
            game_id: "g".into(),
            projectiles,
            items,
            ..Frame::default()
        };
        assert_eq!(live_proto::game::validate_frame(&frame), Ok(()));
        // Without the map's names every kind is unknown.
        assert!(
            frame_projectiles(&state, None)
                .iter()
                .all(|p| p.kind == "unknown")
        );
    }

    #[test]
    fn frames_carry_bsp_hits_vehicles_and_moved_machines() {
        use crate::event::{HitRecord, VehicleState};
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x9001_0001;
        session.start(&campaign(), vec![p.clone()]);
        session.set_tag_names(world_names());
        session.take_output();
        let mut state = snapshot(2, vec![p.clone()]);
        state.objects[0] = Some(ObjectState {
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            ..ObjectState::default()
        });
        state.bsp = Some(1);
        let vehicle = |slot: u32, occupied| VehicleState {
            datum: 0x9000_0000 | (slot << 16) | slot,
            definition: Some(0xe100_0050),
            pos: [slot as f64, 0.0, 0.0],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            occupied,
            parent: None,
            aim: None,
            velocity: Some([0.0, 2.0, 0.0]),
        };
        // Vehicle 9 is vehicle 6's turret, aiming up and left.
        let mut turret = vehicle(9, true);
        turret.parent = Some(0x9006_0006);
        turret.aim = Some([0.0, 0.6, 0.8]);
        state.vehicles = vec![vehicle(5, false), vehicle(6, true), turret];
        state.machines = vec![
            machine(7, [1.0, 1.0, 1.0], Some(0.0)),
            machine(8, [2.0, 2.0, 2.0], None),
        ];
        state.hits = vec![
            // Player 0 shoots an AI with a battle rifle.
            HitRecord {
                code: 10,
                effect: 0xe100_0099,
                owner_player: 0x8001_0000,
                owner_object: 0x9001_0001,
                victim: 0x9003_0003,
                pos: Some([3.0, 0.0, 0.0]),
                body: 0.25,
                shield: 0.5,
                tick: Some(1),
            },
            // An AI punches player 0: a melee for the AI and a weapon from
            // the damage effect's name.
            HitRecord {
                code: 3,
                effect: 0xe100_0040,
                owner_player: u32::MAX,
                owner_object: 0x9003_0003,
                victim: 0x9001_0001,
                pos: Some([0.0, 0.0, 0.0]),
                body: 0.1,
                shield: 0.0,
                tick: None,
            },
            // No position: not sent.
            HitRecord {
                code: 1,
                effect: 0,
                owner_player: u32::MAX,
                owner_object: u32::MAX,
                victim: 0x900a_000a,
                pos: None,
                body: 0.1,
                shield: 0.0,
                tick: None,
            },
        ];
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert_eq!(live_proto::game::validate_frame(&frame), Ok(()));
        assert_eq!(frame.bsp, Some(1));
        assert_eq!(frame.hits.len(), 2);
        let shot = &frame.hits[0];
        assert_eq!(shot.from.as_deref(), Some("p0"));
        assert_eq!(shot.to.as_deref(), Some("90030003"));
        assert_eq!(shot.weapon.as_deref(), Some("battle_rifle"));
        assert_eq!(shot.kind.as_deref(), Some("bullet"));
        assert_eq!(shot.amount, Some(0.75));
        assert_eq!(shot.ago, Some(1), "a tick before the frame's");
        assert_eq!(frame.hits[1].ago, None, "no tick read");
        let punch = &frame.hits[1];
        assert_eq!(punch.from.as_deref(), Some("90030003"));
        assert_eq!(punch.to.as_deref(), Some("p0"));
        assert_eq!(punch.weapon.as_deref(), Some("big_punch"));
        assert_eq!(punch.kind.as_deref(), Some("melee"));
        let vehicles: Vec<_> = frame
            .vehicles
            .iter()
            .map(|v| (v.id.as_str(), v.kind.as_str(), v.occupied))
            .collect();
        assert_eq!(
            vehicles,
            [
                ("90060006", "warthog", true),
                ("90090009", "warthog", true),
                ("90050005", "warthog", false)
            ],
            "occupied first"
        );
        assert_eq!(frame.vehicles[0].parent, None);
        assert_eq!(frame.vehicles[0].aim, None);
        assert_eq!(frame.vehicles[1].parent.as_deref(), Some("90060006"));
        assert_eq!(frame.vehicles[1].aim, Some([0.0, 0.6, 0.8]));
        let machines: Vec<_> = frame
            .machines
            .iter()
            .map(|m| (m.node.as_deref(), m.kind.as_str(), m.position))
            .collect();
        assert_eq!(
            machines,
            [
                (Some("scarab_7"), "scarab", None),
                (Some("scarab_8"), "scarab", None)
            ],
            "all of them in the first frame"
        );

        assert_eq!(frame.machine_positions, [MachinePosition { i: 7, p: 0 }]);

        // The device animates without moving its object origin.
        state.tick = 4;
        state.hits.clear();
        state.machines = vec![
            machine(7, [1.0, 1.0, 1.0], Some(0.5)),
            machine(8, [2.0, 2.0, 2.0], None),
        ];
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert!(frame.hits.is_empty());
        assert!(frame.machines.is_empty());
        assert_eq!(frame.machine_positions, [MachinePosition { i: 7, p: 128 }]);

        state.tick = 6;
        state.machines[0].position = Some(0.501);
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert!(frame.machines.is_empty());
        assert!(frame.machine_positions.is_empty(), "same quantized value");

        state.tick = 8;
        state.machines[1].pos[2] = 3.0;
        state.machines[1].forward = [0.0, 1.0, 0.0];
        session.sample(&state);
        let frame = frames(&mut session).remove(0);
        assert_eq!(frame.machines.len(), 1);
        assert_eq!(frame.machines[0].pos, [2.0, 2.0, 3.0]);
        assert_eq!(frame.machines[0].forward, [0.0, 1.0, 0.0]);
        assert!(frame.machine_positions.is_empty());
    }

    #[test]
    fn machine_positions_capture_keeps_more_devices_than_world_poses() {
        let mut state = snapshot(2, vec![]);
        state.machines = (0..195)
            .map(|slot| machine(slot, [0.0; 3], Some(0.5)))
            .collect();
        let mut sent = BTreeMap::new();
        let positions = frame_machine_positions(&state, &mut sent);
        assert_eq!(positions.len(), 195);
        assert_eq!(positions[194], MachinePosition { i: 194, p: 128 });
        assert!(frame_machine_positions(&state, &mut sent).is_empty());
    }

    #[test]
    fn frames_carry_stance_speed_actions_and_grenade() {
        use crate::event::{Motion, Weapon};
        let mut session = Session::new("test", None);
        let mut p = player(0, 0);
        p.unit = 0x9001_0001;
        session.start(&info(), vec![p.clone()]);
        session.take_output();
        let unit = |pos: [f64; 3], grenades: u32, crouch: f64| ObjectState {
            pos,
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            weapon: Some(Weapon {
                datum: 0x9002_0002,
                definition: 1,
            }),
            motion: Motion {
                crouch: Some(crouch),
                physics_mode: Some(layout::BIPED_MODE_GROUND),
                grenade: Some(1),
                grenades: Some(grenades),
                weapons: Some([0x9002_0002, u32::MAX, u32::MAX, u32::MAX]),
                ..Motion::default()
            },
            ..ObjectState::default()
        };
        let mut state = snapshot(2, vec![p.clone()]);
        state.objects[0] = Some(unit([0.0; 3], 2, 0.0));
        session.sample(&state);
        let first = &frames(&mut session)[0].players[0];
        assert_eq!(first.stance.as_deref(), Some("stand"));
        assert_eq!(first.speed, None, "no previous frame");
        assert_eq!(first.actions, None);
        assert_eq!(first.grenade.as_deref(), Some("plasma"));

        state.tick = 4;
        state.objects[0] = Some(unit([0.4, 0.0, 0.0], 1, 1.0));
        session.sample(&state);
        let second = &frames(&mut session)[0].players[0];
        assert_eq!(second.stance.as_deref(), Some("crouch"));
        assert_eq!(second.speed, Some(6.0));
        assert_eq!(second.actions, Some(vec!["grenade".to_string()]));
    }

    fn pose(pos: [f64; 3]) -> Option<crate::event::Pose> {
        Some(crate::event::Pose {
            pos,
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        })
    }

    fn placed(kind: ObjectKind, index: u16, pos: [f64; 3], parented: bool) -> PlacedObject {
        PlacedObject {
            kind,
            index,
            parented,
            pos,
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
        }
    }

    /// An invented scenario: four crates, three weapons, two vehicles and
    /// two netgame spawns.
    fn table() -> Placements {
        Placements {
            kinds: vec![
                (
                    ObjectKind::Vehicle,
                    vec![pose([0.0; 3]), pose([5.0, 0.0, 0.0])],
                ),
                (
                    ObjectKind::Weapon,
                    vec![
                        pose([1.0, 0.0, 0.0]),
                        pose([2.0, 0.0, 0.0]),
                        pose([3.0, 0.0, 0.0]),
                    ],
                ),
                (
                    ObjectKind::Crate,
                    vec![pose([0.0; 3]), pose([1.0; 3]), pose([2.0; 3]), None],
                ),
                (
                    ObjectKind::NetgameEquipment,
                    vec![pose([7.0; 3]), pose([8.0; 3])],
                ),
            ],
        }
    }

    fn objects(placed: &[PlacedObject]) -> Vec<(ObjectKind, u32, bool)> {
        frame_objects(&table(), placed, &mut Vec::new())
            .into_iter()
            .map(|o| (o.k, o.i, o.gone))
            .collect()
    }

    #[test]
    fn objects_where_the_scenario_put_them_are_not_sent() {
        let all = [
            placed(ObjectKind::Vehicle, 0, [0.0; 3], false),
            placed(ObjectKind::Vehicle, 1, [5.0, 0.0, 0.0], false),
            placed(ObjectKind::Weapon, 0, [1.0, 0.0, 0.0], false),
            placed(ObjectKind::Weapon, 1, [2.0, 0.0, 0.0], false),
            placed(ObjectKind::Weapon, 2, [3.0, 0.0, 0.0], false),
            // Jitter under the threshold.
            placed(ObjectKind::Crate, 0, [0.01, 0.0, 0.0], false),
            placed(ObjectKind::Crate, 1, [1.0; 3], false),
            placed(ObjectKind::Crate, 2, [2.0; 3], false),
            placed(ObjectKind::NetgameEquipment, 0, [7.0; 3], false),
            placed(ObjectKind::NetgameEquipment, 1, [8.0; 3], false),
        ];
        // Crate 3's placement did not read: present means moved.
        assert_eq!(objects(&all), [(ObjectKind::Crate, 3, true)]);
    }

    #[test]
    fn moved_turned_gone_and_held_objects_are_sent_in_order() {
        let mut turned = placed(ObjectKind::Crate, 1, [1.0; 3], false);
        // Three degrees about z.
        let (s, c) = 3f64.to_radians().sin_cos();
        turned.forward = [c, s, 0.0];
        let mut tipped = placed(ObjectKind::Crate, 2, [2.0; 3], false);
        tipped.up = [0.0, 0.1, 1.0];
        let world = [
            // Vehicle 0 driven off; vehicle 1 has a parent (ignored, as
            // a parented vehicle's transform is not world space).
            placed(ObjectKind::Vehicle, 0, [0.0, 3.0, 0.0], false),
            placed(ObjectKind::Vehicle, 1, [0.0; 3], true),
            // Weapon 0 held by a player: gone. Weapon 1 dropped elsewhere
            // and another copy held: the dropped one wins. Weapon 2 absent.
            placed(ObjectKind::Weapon, 0, [9.0; 3], true),
            placed(ObjectKind::Weapon, 1, [0.0; 3], true),
            placed(ObjectKind::Weapon, 1, [2.0, 1.0, 0.0], false),
            placed(ObjectKind::Crate, 0, [0.0; 3], false),
            turned,
            tipped,
            placed(ObjectKind::Crate, 3, [4.0; 3], false),
            // Picked up from spawn 1; spawn 0 respawned in place. An index
            // past the block is ignored.
            placed(ObjectKind::NetgameEquipment, 0, [7.0; 3], false),
            placed(ObjectKind::NetgameEquipment, 9, [7.0; 3], false),
        ];
        assert_eq!(
            objects(&world),
            [
                (ObjectKind::Vehicle, 0, false),
                (ObjectKind::Weapon, 0, true),
                (ObjectKind::Weapon, 1, false),
                (ObjectKind::Weapon, 2, true),
                (ObjectKind::Crate, 1, false),
                (ObjectKind::Crate, 2, false),
                (ObjectKind::Crate, 3, false),
                (ObjectKind::NetgameEquipment, 1, true),
            ]
        );
        let sent = frame_objects(&table(), &world, &mut Vec::new());
        assert_eq!(sent[2].p, Some([2.0, 1.0, 0.0]));
        assert_eq!(sent[0].f, Some([1.0, 0.0, 0.0]));
        let up = sent[5].u.unwrap();
        assert!(
            (up.iter().map(|c| c * c).sum::<f64>() - 1.0).abs() < 1e-9,
            "sent normalised"
        );
        assert!(sent[1].p.is_none() && sent[1].f.is_none() && sent[1].u.is_none());
        // The frame validates.
        let mut frame = Frame {
            objects: sent,
            ..Frame::default()
        };
        frame.tick = 2;
        live_proto::game::validate_frame(&frame).unwrap();
    }

    /// Objects at and about every boundary of the in-place test (distance,
    /// two degrees on either axis, axis length, non-finite values), around
    /// placements of many orientations: the quick path decides as the
    /// exact one, object for object.
    #[test]
    fn the_quick_in_place_test_decides_as_the_exact_one() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let angle = PLACED_COS.acos();
        let mut poses = Vec::new();
        let mut placed = Vec::new();
        for i in 0..4000u16 {
            let (forward, up) =
                crate::memory::rotation_axes([next() * 6.3, next() * 3.0 - 1.5, next() * 6.3]);
            let pos = [next() * 100.0, next() * 100.0, next() * 10.0];
            poses.push(Some(Pose { pos, forward, up }));
            // Turn by an angle at the two-degree boundary (give or take a
            // few ulps, or the margin), or well inside or out.
            let turn = match i % 6 {
                0 => angle * (1.0 + (next() - 0.5) * 1e-12),
                1 => angle + (next() - 0.5) * 4e-9,
                2 => angle * next(),
                3 => angle * (1.0 + next()),
                4 => 0.0,
                _ => angle * (1.0 - 1e-15 * next()),
            };
            let (sn, cs) = turn.sin_cos();
            // About the axis forward × up's normal: forward toward up's side.
            let side = [
                forward[1] * up[2] - forward[2] * up[1],
                forward[2] * up[0] - forward[0] * up[2],
                forward[0] * up[1] - forward[1] * up[0],
            ];
            let turned = |a: [f64; 3]| [0, 1, 2].map(|k| a[k] * cs + side[k] * sn);
            let scale = match i % 7 {
                0 => 1e-6 * (1.0 + (next() - 0.5) * 1e-9),
                1 => 1.414e-6 + next() * 1e-9,
                2 => 1e3,
                _ => 0.5 + next(),
            };
            let mut f = turned(forward).map(|c| c * scale);
            let mut u = if i % 2 == 0 { up } else { turned(up) };
            let r = PLACED_DISTANCE
                * match i % 5 {
                    0 => 1.0 + (next() - 0.5) * 1e-12,
                    1 => next(),
                    2 => 1.5,
                    _ => 0.0,
                };
            let mut p = [pos[0] + r, pos[1], pos[2]];
            match i % 97 {
                0 => f[1] = f64::NAN,
                1 => u[2] = f64::INFINITY,
                2 => p[0] = f64::NAN,
                3 => f = [0.0; 3],
                4 => u = u.map(|c| -c),
                _ => {}
            }
            placed.push(PlacedObject {
                kind: ObjectKind::Scenery,
                index: i,
                parented: false,
                pos: p,
                forward: f,
                up: u,
            });
        }
        let mut exact = Vec::new();
        // In runs of a hundred placements, under the frame's cap.
        for (poses, run) in poses.chunks(100).zip(placed.chunks_mut(100)) {
            for (i, o) in run.iter_mut().enumerate() {
                o.index = i as u16;
            }
            let table = Placements {
                kinds: vec![(ObjectKind::Scenery, poses.to_vec())],
            };
            let quick = frame_objects_with(&table, run, &mut Vec::new(), true);
            let b = frame_objects_with(&table, run, &mut Vec::new(), false);
            assert_eq!(quick, b);
            exact.extend(b);
        }
        // Both sides of the boundary were tried.
        let sent = exact.iter().filter(|o| !o.gone).count();
        assert!(sent > 500 && sent < 3500, "{sent}");
    }

    #[test]
    fn no_placements_no_objects() {
        let world = [placed(ObjectKind::Crate, 0, [5.0; 3], false)];
        assert!(frame_objects(&Placements::default(), &world, &mut Vec::new()).is_empty());
    }

    #[test]
    fn frames_carry_objects_once_placements_are_set() {
        let mut session = Session::new("test", None);
        let p = player(0, 0);
        session.start(&campaign(), vec![p.clone()]);
        session.set_placements(table());
        session.take_output();
        let mut state = snapshot(2, vec![p]);
        state.placed = vec![placed(ObjectKind::Crate, 0, [0.0, 0.0, 1.0], false)];
        session.sample(&state);
        let frame = frames(&mut session).pop().unwrap();
        let sent: Vec<_> = frame.objects.iter().map(|o| (o.k, o.i, o.gone)).collect();
        assert!(sent.contains(&(ObjectKind::Crate, 0, false)));
        assert!(sent.contains(&(ObjectKind::Weapon, 0, true)));
        assert_eq!(frame.objects[0].k, ObjectKind::Vehicle);
    }

    /// Game-thread cost of a frame for a ~60-actor campaign scene: the
    /// object table scan, the frame build, and its JSON or packed form.
    /// `cargo test --release -p live-telemetry measure_frame_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure_frame_cost() {
        use crate::memory::{fixture, world};
        use std::time::Instant;
        let mut guest = fixture::memory();
        let mut slot = 1;
        for i in 0..60 {
            fixture::put_actor(&mut guest, slot, 2, [i as f32, 1.0, 2.0]);
            slot += 1;
        }
        for _ in 0..8 {
            fixture::put_vehicle(&mut guest, slot);
            slot += 1;
        }
        for i in 0..12 {
            fixture::put_machine(&mut guest, slot, i, 0.5);
            slot += 1;
        }
        for i in 0..90 {
            let kind = [2u8, 6, 11][i % 3];
            fixture::put_placed(
                &mut guest,
                slot,
                kind,
                i as i16,
                -1,
                [i as f32, 0.0, 0.0],
                u32::MAX,
            );
            slot += 1;
        }
        let poses = |n: usize| (0..n).map(|i| pose([i as f64 * 3.0, 0.0, 0.0])).collect();
        let table = Placements {
            kinds: vec![
                (ObjectKind::Weapon, poses(40)),
                (ObjectKind::Scenery, poses(40)),
                (ObjectKind::Crate, poses(40)),
            ],
        };
        let mut session = Session::new("test", None);
        let p = player(0, 0);
        session.start(&campaign(), vec![p.clone()]);
        session.set_placements(table);
        session.take_output();
        let runs = 2000u32;
        let (mut scan, mut build, mut json, mut packed) = (0.0, 0.0, 0.0, 0.0);
        let mut encoder = live_proto::pack::Encoder::new(0, 150);
        let mut bytes = (0, 0);
        for n in 0..runs {
            let start = Instant::now();
            let found = world(&guest).unwrap();
            scan += start.elapsed().as_secs_f64();
            let mut state = snapshot(2 + 2 * u64::from(n), vec![p.clone()]);
            state.actors = found.actors;
            state.vehicles = found.vehicles;
            state.machines = found.machines;
            state.placed = found.placed;
            let start = Instant::now();
            session.sample(&state);
            let frame = frames(&mut session).pop().unwrap();
            build += start.elapsed().as_secs_f64();
            let start = Instant::now();
            let text = serde_json::to_string(&frame).unwrap();
            json += start.elapsed().as_secs_f64();
            let start = Instant::now();
            let (record, _) = encoder.encode(&frame).unwrap();
            packed += start.elapsed().as_secs_f64();
            bytes = (text.len(), record.len());
        }
        let us = |t: f64| t * 1e6 / f64::from(runs);
        println!(
            "scan {:.1} us, build {:.1} us, json {:.1} us ({} B), packed {:.1} us ({} B)",
            us(scan),
            us(build),
            us(json),
            bytes.0,
            us(packed),
            bytes.1
        );
    }
}
