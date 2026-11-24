//! A fake [`Game`] for tests, `h2 rules run` and Live's workbench: plain
//! tables in, a log of the calls made out. [`FakeGame::apply`] is what the
//! engine does when it reports an event, so every scenario runner shares
//! one model of the game.

use std::collections::{BTreeMap, BTreeSet};

use crate::caps::id;
use crate::vm::{Arg, Call, Game, Input, Placed, ScriptError};

/// One player of the fake game.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FakePlayer {
    pub team: u8,
    pub alive: bool,
    pub score: i32,
    pub lives: i32,
    /// What `kills(p)` and `deaths(p)` read.
    pub kills: i32,
    pub deaths: i32,
}

/// A game held in tables. Actions it can model (score, team, lives,
/// respawn, objective registration) change the tables as the engine
/// would; every action and the script-error end are logged in call order.
#[derive(Clone, Debug, Default)]
pub struct FakeGame {
    pub players: BTreeMap<u8, FakePlayer>,
    pub teams: Vec<u8>,
    pub labelled: BTreeMap<String, Vec<Placed>>,
    /// Retail record fields a scenario sets; the rest read
    /// [`stock_variant`] of [`FakeGame::carrier`].
    pub variant: BTreeMap<String, i32>,
    /// The script's carrier engine index, for the stock record's fields.
    pub carrier: u8,
    pub round_time_left: i32,
    pub squad_alive: [i32; 16],
    /// The objective registry: object to its carrying player.
    pub objectives: BTreeMap<u32, Option<u8>>,
    /// Objects' teams: an objective's, or a flag placement's designator.
    pub object_teams: BTreeMap<u32, u8>,
    /// Labelled objects' label numbers.
    pub numbers: BTreeMap<u32, i32>,
    /// Players' unit objects: what `player_of` reads, and how `enter` and
    /// `leave` move a player in and out of a zone.
    pub units: BTreeMap<u32, u8>,
    /// `(player, zone object)` pairs: a player whose unit is inside the
    /// zone. `inside` and the zone test read the living ones.
    pub inside: BTreeSet<(u8, u32)>,
    /// The area of each zone object a scenario shapes, with its centre
    /// marker (docs/game/gametype-markers.md, "In a script"):
    /// [`FakeGame::move_to`] puts a player inside the zones whose area
    /// holds it. A zone with no area is moved in and out by `enter` and
    /// `leave` alone.
    pub areas: BTreeMap<u32, ([f32; 3], game_markers::Area)>,
    /// Where [`FakeGame::move_to`] last put each player.
    pub positions: BTreeMap<u8, [f32; 3]>,
    /// Actions made, as `(entry name, args)`.
    pub calls: Vec<(&'static str, Vec<Arg>)>,
    /// Reads made, by entry name.
    pub reads: Vec<&'static str>,
    /// Messages shown, in call order (each is in [`FakeGame::calls`] too).
    pub messages: Vec<FakeMessage>,
    /// Sounds played, in call order (each is in [`FakeGame::calls`] too).
    pub sounds: Vec<FakeSound>,
    /// The retail event or original clip posted at each audible round start.
    pub start_lines: Vec<RoundLine>,
    /// What a point is called on the scoreboard and in the post-game, as
    /// the last round start told it (`Game::points_label`).
    pub scoreboard_label: Option<String>,
    /// Each team's spawn rule, as `spawn_areas` and `spawn_near` set it:
    /// the starting locations the engine's spawns of that team may use.
    pub spawns: BTreeMap<u8, FakeSpawns>,
    /// Each team's body, as `set_body` sets it: what its players spawn as.
    /// A team with none spawns in its players' own.
    pub bodies: BTreeMap<u8, crate::bodies::Body>,
    /// The areas shown in play, as `show_area` and `hide_area` keep them:
    /// a zone object and the team of its wall's colour, in the order first
    /// shown (`vm::RulesExt::shown`).
    pub shown: Vec<(u32, Option<u8>)>,
    /// Where each object is, in 16.16 world units: what `object_pos`
    /// reads, set by a scenario, by `teleport` and by a mover's step.
    pub object_positions: BTreeMap<u32, [i32; 3]>,
    /// Each object's velocity in 16.16 world units a second: what
    /// `object_vel` reads, set by a mover's step.
    pub object_velocities: BTreeMap<u32, [i32; 3]>,
    /// The world's half-width in 16.16 world units: a `teleport` with a
    /// coordinate past it is refused, as the engine answers false for a
    /// position outside the world. `None`: the whole 16.16 range is inside.
    pub world_limit: Option<i32>,
    /// Calls the world refused, as `(entry name, why)`: each is in
    /// [`FakeGame::calls`] too, and changed nothing.
    pub refused: Vec<(&'static str, String)>,
    /// The steps the VM's movers made, in order: the object, where it was
    /// put and its velocity.
    pub steps: Vec<(u32, [i32; 3], [i32; 3])>,
    /// The tick [`FakeGame::begin_tick`] last began: a flight starts on it.
    pub tick: u32,
    /// Where placed objects are, in 16.16 world units: a path's points,
    /// and a vehicle as it flies.
    pub object_at: BTreeMap<u32, crate::vehicles::Vec3>,
    /// Which way a flying vehicle faces: the unit tangent of its path.
    pub facing: BTreeMap<u32, crate::vehicles::Vec3>,
    /// The vehicles of the map, by object; any other object is no vehicle
    /// and a vehicle entry called on it is refused.
    pub vehicles: BTreeMap<u32, FakeVehicle>,
    /// Objects whose flight ended since [`FakeGame::take_arrived`], in the
    /// order they ended: what the world API's `on arrive` reports.
    pub arrived: Vec<u32>,
    /// Squads put in vehicles that were then unloaded: `(vehicle, squad,
    /// actors)`, in call order.
    pub unloaded: Vec<(u32, u8, u8)>,
    pub script_error: Option<ScriptError>,
}

/// A sound the script played: the sound's tag path and its audience as a
/// mask of player slots (`sound_to`: the player's bit; `sound_to_team`: the
/// bits of the team's seated players; `sound_to_all`: every slot). A call
/// through `none` plays nothing and is not logged here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeSound {
    pub path: String,
    pub audience: crate::show::Audience,
}

/// A vehicle of the fake game: its seats and what is in them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeVehicle {
    /// The seats, the driver's included.
    pub seats: u8,
    pub driver: Option<u8>,
    /// Players in the other seats, in the order they boarded.
    pub riders: Vec<u8>,
    /// Squads placed in it: `(squad, actors)`, one seat for each actor.
    pub squads: Vec<(u8, u8)>,
    /// `hover(v, true)`.
    pub hovering: bool,
    /// The flight in progress, if any.
    pub flight: Option<crate::vehicles::Flight>,
}

impl FakeVehicle {
    /// A parked vehicle with `seats` seats, all free.
    pub fn new(seats: u8) -> FakeVehicle {
        FakeVehicle {
            seats,
            driver: None,
            riders: Vec::new(),
            squads: Vec::new(),
            hovering: false,
            flight: None,
        }
    }

    /// Everyone aboard: the driver, the other players and every actor.
    pub fn aboard(&self) -> i32 {
        i32::from(self.driver.is_some())
            + self.riders.len() as i32
            + self.squads.iter().map(|&(_, n)| i32::from(n)).sum::<i32>()
    }

    /// Seats nobody is in.
    pub fn free(&self) -> i32 {
        (i32::from(self.seats) - self.aboard()).max(0)
    }

    fn leave(&mut self, p: u8) {
        if self.driver == Some(p) {
            self.driver = None;
        }
        self.riders.retain(|&r| r != p);
    }
}

/// The two ways a type announces itself on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundLine {
    Event { kind: u8, index: u8 },
    Clip(crate::types::Type),
}

/// A team's spawn rule in the fake game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FakeSpawns {
    /// The starting locations labelled `label` and numbered `number`.
    Areas { label: String, number: i32 },
    /// The `count` starting locations nearest the zone object `zone`.
    Near { zone: u32, count: i32 },
}

/// A message the script showed: to whom, its audience as a mask of player
/// slots (`docs/game/rules-world-api.md`, "The show channel"), its stored
/// template, the values of its holes, and the line a player reads (a
/// player as `player 2`, a team as `team 1`, a time as `m:ss`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FakeMessage {
    /// A player or a team; `None` for everyone.
    pub to: Option<Arg>,
    /// The audience as a mask of player slots: the player's bit, the bits
    /// of the team's seated players, or every slot for everyone.
    pub audience: crate::show::Audience,
    pub template: String,
    pub values: Vec<Arg>,
    pub line: String,
}

impl FakeMessage {
    /// The message of a `message` call: `[to, text, values...]`, or
    /// `[text, values...]` for everyone.
    fn of(g: &FakeGame, call: &Call) -> Option<FakeMessage> {
        let own = call.entry.args.len();
        let to = (own == 2).then(|| call.args.first().cloned()).flatten();
        let Some(Arg::Text(template)) = call.args.get(own - 1) else {
            return None;
        };
        let values: Vec<Arg> = call.args[own..].to_vec();
        let holes = crate::caps::holes(template).unwrap_or_default();
        let shown: Vec<String> = values
            .iter()
            .zip(holes.iter().copied().map(Some).chain(std::iter::repeat(None)))
            .map(|(v, h)| show(v, h))
            .collect();
        let audience = match &to {
            Some(Arg::Player(Some(p))) => crate::show::player(*p),
            Some(Arg::Team(Some(t))) => g
                .players
                .iter()
                .filter(|(_, q)| q.team == *t)
                .fold(0, |mask, (&slot, _)| mask | crate::show::player(slot)),
            Some(Arg::Player(None) | Arg::Team(None)) => 0,
            None => crate::show::EVERYONE,
            _ => return None,
        };
        Some(FakeMessage {
            to,
            audience,
            template: template.clone(),
            line: crate::caps::fill(template, &shown),
            values,
        })
    }
}

/// A hole's value as the fake shows it.
fn show(v: &Arg, hole: Option<crate::caps::Hole>) -> String {
    match (v, hole) {
        (Arg::Int(n), Some(crate::caps::Hole::Time)) => crate::caps::show_time(*n),
        (Arg::Int(n), _) => n.to_string(),
        (Arg::Player(Some(p)), _) => format!("player {p}"),
        (Arg::Team(Some(t)), _) => format!("team {t}"),
        (Arg::Player(None) | Arg::Team(None), _) => "none".into(),
        (Arg::Text(s), _) => s.clone(),
        (other, _) => format!("{other:?}"),
    }
}

/// The fields of the stock record of engine `carrier` (the game's
/// defaults writer, `hopper::variant::VariantRecord::stock` in
/// `live-proto`) that a script reads by name. A field it leaves out reads
/// 0, and an index that is no carrier has none.
pub fn stock_variant(carrier: u8) -> BTreeMap<String, i32> {
    if !crate::caps::CARRIERS.iter().any(|&(_, i)| i == carrier) {
        return BTreeMap::new();
    }
    // Team play by type: CTF, Assault and Territories.
    let team = matches!(carrier, 1 | 8 | 9);
    let score = match carrier {
        2 => 25,
        3 | 4 => 120,
        7 => 15,
        1 | 9 => 3,
        8 => 300,
        _ => 0,
    };
    let mut out: BTreeMap<String, i32> = [
        ("team_play", i32::from(team)),
        ("score_limit", score),
        ("time_limit", 480),
        ("lives", 0),
        ("respawn_time", if team { 10 } else { 5 }),
        ("rounds", 0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let area: &[(&str, i32)] = match carrier {
        3 => &[("oddball_ball_count", 1)],
        4 => &[("king_moving_hill", 60)],
        1 => &[("ctf_reset_time", 30)],
        9 => &[("ctf_reset_time", 30), ("assault_arm_time", 5)],
        8 => &[("territories_count", 3), ("territories_contest_time", 5)],
        _ => &[],
    };
    out.extend(area.iter().map(|&(k, v)| (k.to_string(), v)));
    out
}

impl FakeGame {
    /// `n` living players in slots 0.., all on team 0.
    pub fn with_players(n: u8) -> FakeGame {
        let mut g = FakeGame {
            teams: vec![0, 1],
            ..FakeGame::default()
        };
        for slot in 0..n {
            g.players.insert(
                slot,
                FakePlayer {
                    alive: true,
                    ..FakePlayer::default()
                },
            );
        }
        g
    }

    /// The actions made with entry `name`, in order.
    pub fn calls_to(&self, name: &str) -> Vec<&Vec<Arg>> {
        self.calls
            .iter()
            .filter(|(n, _)| *n == name)
            .map(|(_, a)| a)
            .collect()
    }

    /// Declares a labelled object: its label, number and placement. Its
    /// team, when it has one, goes in [`FakeGame::object_teams`].
    pub fn label(&mut self, label: &str, object: u32, number: i32, placement: u32) {
        self.labelled
            .entry(label.to_string())
            .or_default()
            .push(Placed { placement, object });
        self.numbers.insert(object, number);
    }

    /// Gives the zone object `zone` an area round the marker at `centre`.
    pub fn set_area(&mut self, zone: u32, centre: [f32; 3], area: game_markers::Area) {
        self.areas.insert(zone, (centre, area));
    }

    /// Puts player `p` at `at` and returns the `enter` and `leave` inputs
    /// the engine reports for it, `leave`s first, in zone order: the
    /// shaped zones ([`FakeGame::areas`]) whose area holds `at` against
    /// the ones the player was in. A player with no unit is moved and
    /// reports nothing. Apply and fire each input, as [`crate::scenario::step`]
    /// does.
    pub fn move_to(&mut self, p: u8, at: [f32; 3]) -> Vec<Input> {
        self.positions.insert(p, at);
        let Some((&object, _)) = self.units.iter().find(|(_, q)| **q == p) else {
            return Vec::new();
        };
        let mut leaves = Vec::new();
        let mut enters = Vec::new();
        for (&zone, (centre, area)) in &self.areas {
            let was = self.inside.contains(&(p, zone));
            let now = area.contains(*centre, at);
            if was && !now {
                leaves.push(Input::Leave { zone, object });
            } else if now && !was {
                enters.push(Input::Enter { zone, object });
            }
        }
        leaves.extend(enters);
        leaves
    }

    /// A new tick begins: the round clock counts down one, to 0.
    pub fn begin_tick(&mut self, tick: u32) {
        self.tick = tick;
        if tick > 0 {
            self.round_time_left = (self.round_time_left - 1).max(0);
        }
        self.fly(tick);
    }

    /// Moves each flying vehicle to where its path puts it at `tick`,
    /// facing the tangent; a flight that has run its ticks ends there and
    /// the vehicle is reported by [`FakeGame::take_arrived`], in object
    /// order.
    fn fly(&mut self, tick: u32) {
        for (&object, v) in &mut self.vehicles {
            let Some(flight) = &v.flight else { continue };
            let pose = flight.at(tick);
            self.object_at.insert(object, pose.position);
            self.facing.insert(object, pose.facing);
            if flight.done(tick) {
                v.flight = None;
                self.arrived.push(object);
            }
        }
    }

    /// The objects whose flight ended since the last call: the scenario
    /// layer reports each as the world API's `on arrive`.
    pub fn take_arrived(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.arrived)
    }

    /// Puts a parked vehicle of `seats` seats at `object`.
    pub fn add_vehicle(&mut self, object: u32, seats: u8, at: crate::vehicles::Vec3) {
        self.vehicles.insert(object, FakeVehicle::new(seats));
        self.object_at.insert(object, at);
    }

    /// The vehicle player `p` is aboard.
    pub fn vehicle_of(&self, p: u8) -> Option<u32> {
        self.vehicles
            .iter()
            .find(|(_, v)| v.driver == Some(p) || v.riders.contains(&p))
            .map(|(&o, _)| o)
    }

    /// The points of the `path` label in number order, as positions;
    /// `None` when a point has none placed.
    fn path_points(&self, label: &str) -> Option<Vec<crate::vehicles::Vec3>> {
        let mut points: Vec<(i32, u32)> = self
            .labelled
            .get(label)?
            .iter()
            .map(|p| (self.numbers.get(&p.object).copied().unwrap_or(0), p.object))
            .collect();
        points.sort_unstable();
        points.iter().map(|(_, o)| self.object_at.get(o).copied()).collect()
    }

    /// A vehicle entry: the call's effect on the vehicles, or the reason
    /// the game refuses it (a vehicle entry through a handle that is no
    /// vehicle, a flight with no path, a seat that does not exist). A
    /// refused call changes nothing.
    fn drive(&mut self, call: &Call) -> Result<(), &'static str> {
        let object = |i: usize| match call.args.get(i) {
            Some(Arg::Object(Some(o))) => Some(*o),
            _ => None,
        };
        let player = |i: usize| match call.args.get(i) {
            Some(Arg::Player(Some(p))) => Some(*p),
            _ => None,
        };
        match call.entry.id {
            id::FLY => {
                let (Some(o), Some(Arg::Label(label)), Some(&Arg::Int(ticks))) =
                    (object(0), call.args.get(1), call.args.get(2))
                else {
                    return Err("no vehicle");
                };
                if !self.vehicles.contains_key(&o) {
                    return Err("not a vehicle");
                }
                let ticks = u32::try_from(ticks).ok().filter(|&t| t > 0).ok_or("a flight takes at least 1 tick")?;
                let points = self.path_points(label).ok_or("a point of the path is not placed")?;
                let flight = crate::vehicles::Flight::new(points, self.tick, ticks).map_err(|e| match e {
                    crate::vehicles::Refusal::TooFewPoints => "a path has at least 2 points",
                    crate::vehicles::Refusal::TooManyPoints => "a path has too many points",
                    crate::vehicles::Refusal::NoTicks => "a flight takes at least 1 tick",
                })?;
                self.vehicles.get_mut(&o).ok_or("not a vehicle")?.flight = Some(flight);
            }
            id::HOVER => {
                let (Some(o), Some(&Arg::Bool(on))) = (object(0), call.args.get(1)) else {
                    return Err("no vehicle");
                };
                self.vehicles.get_mut(&o).ok_or("not a vehicle")?.hovering = on;
            }
            id::SEAT => {
                let (Some(p), Some(o)) = (player(0), object(1)) else {
                    return Err("no player or vehicle");
                };
                if !self.players.get(&p).is_some_and(|q| q.alive) {
                    return Err("the player is not alive");
                }
                let v = self.vehicles.get(&o).ok_or("not a vehicle")?;
                // Boarding the vehicle the player is already in changes
                // nothing; another vehicle must have a seat before the
                // player leaves the first.
                if v.driver == Some(p) || v.riders.contains(&p) {
                    return Ok(());
                }
                if v.free() == 0 {
                    return Err("the vehicle has no free seat");
                }
                for other in self.vehicles.values_mut() {
                    other.leave(p);
                }
                if let Some(v) = self.vehicles.get_mut(&o) {
                    if v.driver.is_none() {
                        v.driver = Some(p);
                    } else {
                        v.riders.push(p);
                    }
                }
            }
            id::EJECT => {
                let Some(p) = player(0) else {
                    return Err("no player");
                };
                for v in self.vehicles.values_mut() {
                    v.leave(p);
                }
            }
            id::UNLOAD => {
                let o = object(0).ok_or("no vehicle")?;
                let v = self.vehicles.get_mut(&o).ok_or("not a vehicle")?;
                v.driver = None;
                v.riders.clear();
                for (squad, n) in std::mem::take(&mut v.squads) {
                    self.unloaded.push((o, squad, n));
                }
            }
            id::PLACE_SQUAD_IN => {
                let (Some(&Arg::Squad(s)), Some(&Arg::Int(n)), Some(o)) = (call.args.first(), call.args.get(2), object(3))
                else {
                    return Err("no squad or vehicle");
                };
                if n <= 0 {
                    return Err("a squad is at least 1 actor");
                }
                let v = self.vehicles.get_mut(&o).ok_or("not a vehicle")?;
                let n = u8::try_from(n.min(v.free())).ok().filter(|&n| n > 0).ok_or("the vehicle has no free seat")?;
                v.squads.push((s, n));
            }
            _ => {}
        }
        Ok(())
    }

    /// A dead or departed player stands in no zone and carries nothing.
    fn down(&mut self, p: u8) {
        self.inside.retain(|&(q, _)| q != p);
        for v in self.vehicles.values_mut() {
            v.leave(p);
        }
        for carrier in self.objectives.values_mut() {
            if *carrier == Some(p) {
                *carrier = None;
            }
        }
    }

    /// What the engine does when it reports `input`, before the handler
    /// runs: a kill marks its victim dead and counts the victim's death and
    /// the killer's kill (a suicide counts no kill), a death marks the
    /// player dead (either takes the player out of every zone and drops
    /// what they carried, and takes one of the player's lives when the
    /// variant counts them), a spawn alive, a join seats a living player on
    /// team 0, an `enter` or `leave` of a player's unit moves the player
    /// in or out of the zone, and a `pickup` or `drop` of a registered
    /// objective sets or clears its carrier. [`FakeGame::after`] removes a
    /// player who quit.
    ///
    /// Returns whether the event reached the game: the engine holds a dead
    /// player's spawn while the variant's lives setting is positive and the
    /// player has none left (`PLAYER_OUT_OF_LIVES`), so a held spawn
    /// changes nothing and fires no handler. Only the script's
    /// `respawn` lets a held player back.
    pub fn apply(&mut self, input: &Input) -> bool {
        match *input {
            Input::Kill { victim, killer } => {
                let lives = self.variant("lives") > 0;
                if let Some(v) = self.players.get_mut(&victim) {
                    v.alive = false;
                    v.deaths = v.deaths.saturating_add(1);
                    if lives {
                        v.lives = v.lives.saturating_sub(1);
                    }
                }
                if let Some(k) = killer
                    && k != victim
                    && let Some(k) = self.players.get_mut(&k)
                {
                    k.kills = k.kills.saturating_add(1);
                }
                self.down(victim);
            }
            Input::Death(p) => {
                let lives = self.variant("lives") > 0;
                if let Some(v) = self.players.get_mut(&p) {
                    v.alive = false;
                    if lives {
                        v.lives = v.lives.saturating_sub(1);
                    }
                }
                self.down(p);
            }
            Input::Pickup(p, o) => {
                if let Some(carrier) = self.objectives.get_mut(&o) {
                    *carrier = Some(p);
                }
            }
            Input::Drop(p, o) => {
                if let Some(carrier) = self.objectives.get_mut(&o)
                    && *carrier == Some(p)
                {
                    *carrier = None;
                }
            }
            Input::Spawn(p) => {
                if self.holds(p) {
                    return false;
                }
                if let Some(v) = self.players.get_mut(&p) {
                    v.alive = true;
                }
            }
            Input::Join(p) => {
                self.players.entry(p).or_insert(FakePlayer {
                    alive: true,
                    ..FakePlayer::default()
                });
            }
            Input::Enter { zone, object } => {
                if let Some(&p) = self.units.get(&object) {
                    self.inside.insert((p, zone));
                }
            }
            Input::Leave { zone, object } => {
                if let Some(&p) = self.units.get(&object) {
                    self.inside.remove(&(p, zone));
                }
            }
            _ => {}
        }
        true
    }

    /// Whether the engine holds `p`'s respawn: the variant's lives setting
    /// is positive and the player has none left.
    fn holds(&self, p: u8) -> bool {
        self.variant("lives") > 0 && self.players.get(&p).is_some_and(|v| v.lives <= 0)
    }

    /// What the engine does after the handler of `input`: a quit removes
    /// the player and its unit, and the round's end ends the teams' spawn
    /// rules and bodies, as the VM's state does.
    pub fn after(&mut self, input: &Input) {
        if let Input::RoundEnd = *input {
            self.spawns.clear();
            self.bodies.clear();
            self.shown.clear();
            self.object_velocities.clear();
        }
        if let Input::Quit(p) = *input {
            self.players.remove(&p);
            self.units.retain(|_, q| *q != p);
            self.down(p);
        }
    }

    fn player(&self, arg: Option<&Arg>) -> Option<&FakePlayer> {
        match arg {
            Some(Arg::Player(Some(p))) => self.players.get(p),
            _ => None,
        }
    }

    /// Whether player `p` is alive and inside `zone`.
    fn living_inside(&self, p: u8, zone: u32) -> bool {
        self.inside.contains(&(p, zone)) && self.players.get(&p).is_some_and(|q| q.alive)
    }
}

impl Game for FakeGame {
    fn points_label(&mut self, label: &str) {
        self.scoreboard_label = Some(label.to_string());
    }

    fn start_line(&mut self, ty: crate::types::Type) {
        use crate::types::StartLine;
        match ty.row().start {
            StartLine::Retail(kind) => self.start_lines.push(RoundLine::Event { kind, index: 0 }),
            StartLine::Ours => self.start_lines.push(RoundLine::Clip(ty)),
            StartLine::None => {}
        }
    }

    fn players(&self) -> Vec<u8> {
        // Reverse order, so the VM's own ordering is what tests see.
        self.players.keys().rev().copied().collect()
    }

    fn teams(&self) -> Vec<u8> {
        self.teams.iter().rev().copied().collect()
    }

    fn labelled(&self, label: &str) -> Vec<Placed> {
        let holds = |t: &str| self.labelled.get(t).is_some_and(|l| !l.is_empty());
        let mut list = self
            .labelled
            .get(crate::labels::reads(label, holds))
            .cloned()
            .unwrap_or_default();
        list.reverse();
        list
    }

    fn objectives(&self) -> Vec<u32> {
        self.objectives.keys().rev().copied().collect()
    }

    fn variant(&self, field: &str) -> i32 {
        self.variant
            .get(field)
            .copied()
            .or_else(|| stock_variant(self.carrier).get(field).copied())
            .unwrap_or(0)
    }

    fn zone_holder(&self, zone: u32) -> Option<u8> {
        let mut holder = None;
        for (&p, player) in &self.players {
            if !self.living_inside(p, zone) {
                continue;
            }
            match holder {
                None => holder = Some(player.team),
                Some(t) if t == player.team => {}
                Some(_) => return None,
            }
        }
        holder
    }

    fn read(&mut self, call: &Call) -> i32 {
        self.reads.push(call.entry.name);
        let p = self.player(call.args.first());
        let object = |i: usize| match call.args.get(i) {
            Some(Arg::Object(Some(o))) => Some(*o),
            _ => None,
        };
        match call.entry.id {
            id::OBJECT_POS | id::OBJECT_VEL => {
                let table = if call.entry.id == id::OBJECT_POS {
                    &self.object_positions
                } else {
                    &self.object_velocities
                };
                match (object(0), call.args.get(1)) {
                    (Some(o), Some(Arg::Int(axis))) => usize::try_from(*axis)
                        .ok()
                        .and_then(|a| table.get(&o)?.get(a).copied())
                        .unwrap_or(0),
                    _ => 0,
                }
            }
            id::IS_OBJECTIVE => object(0).is_some_and(|o| self.objectives.contains_key(&o)) as i32,
            id::CARRIER_OF => object(0)
                .and_then(|o| self.objectives.get(&o).copied().flatten())
                .map_or(-1, i32::from),
            id::OBJECT_TEAM => object(0)
                .and_then(|o| self.object_teams.get(&o))
                .map_or(-1, |&t| i32::from(t)),
            id::PLAYER_OF => object(0)
                .and_then(|o| self.units.get(&o))
                .map_or(-1, |&p| i32::from(p)),
            id::VEHICLE_RIDERS => object(0)
                .and_then(|o| self.vehicles.get(&o))
                .map_or(0, FakeVehicle::aboard),
            id::VEHICLE_DRIVER => object(0)
                .and_then(|o| self.vehicles.get(&o))
                .and_then(|v| v.driver)
                .map_or(-1, i32::from),
            id::LABEL_NUMBER => object(0).and_then(|o| self.numbers.get(&o).copied()).unwrap_or(0),
            id::INSIDE => match (call.args.first(), object(1)) {
                (Some(Arg::Player(Some(p))), Some(z)) => self.living_inside(*p, z) as i32,
                _ => 0,
            },
            id::PLAYER_SCORE => p.map_or(0, |p| p.score),
            id::ALIVE => p.is_some_and(|p| p.alive) as i32,
            id::TEAM_OF => p.map_or(-1, |p| p.team as i32),
            id::SLOT_OF => match call.args.first() {
                Some(Arg::Player(Some(s))) => i32::from(*s),
                _ => -1,
            },
            id::SIDE_OF => match (call.args.first(), p) {
                (Some(Arg::Player(Some(s))), Some(p)) => {
                    if self.variant("team_play") != 0 {
                        i32::from(p.team)
                    } else {
                        16 + i32::from(*s)
                    }
                }
                _ => -1,
            },
            id::LIVES => p.map_or(0, |p| p.lives),
            id::KILLS => p.map_or(0, |p| p.kills),
            id::DEATHS => p.map_or(0, |p| p.deaths),
            id::ROUND_TIME_LEFT => self.round_time_left,
            id::SQUAD_ALIVE => match call.args.first() {
                Some(Arg::Squad(s)) => self.squad_alive.get(*s as usize).copied().unwrap_or(0),
                _ => 0,
            },
            id::TEAM_SCORE => match call.args.first() {
                Some(Arg::Team(Some(t))) => self
                    .players
                    .values()
                    .filter(|p| p.team == *t)
                    .map(|p| p.score)
                    .sum(),
                _ => 0,
            },
            _ => 0,
        }
    }

    fn act(&mut self, call: &Call) {
        self.calls.push((call.entry.name, call.args.clone()));
        if crate::caps::is_message(call.entry.id) {
            let message = FakeMessage::of(self, call);
            self.messages.extend(message);
            return;
        }
        if matches!(call.entry.id, id::SOUND_TO | id::SOUND_TO_TEAM | id::SOUND_TO_ALL) {
            if let Some(Arg::Tag(Some(path))) = call.args.first() {
                let audience = match (call.entry.id, call.args.get(1)) {
                    (id::SOUND_TO, Some(Arg::Player(Some(p)))) => Some(crate::show::player(*p)),
                    (id::SOUND_TO_TEAM, Some(Arg::Team(Some(t)))) => Some(
                        self.players
                            .iter()
                            .filter(|(_, q)| q.team == *t)
                            .fold(0, |mask, (&slot, _)| mask | crate::show::player(slot)),
                    ),
                    (id::SOUND_TO_ALL, _) => Some(crate::show::EVERYONE),
                    _ => None,
                };
                if let Some(audience) = audience {
                    self.sounds.push(FakeSound {
                        path: path.clone(),
                        audience,
                    });
                }
            }
            return;
        }
        if matches!(
            call.entry.id,
            id::FLY | id::HOVER | id::SEAT | id::EJECT | id::UNLOAD | id::PLACE_SQUAD_IN
        ) {
            if let Err(why) = self.drive(call) {
                self.refused.push((call.entry.name, why.into()));
            }
            return;
        }
        if call.entry.id == id::REGISTER_OBJECTIVE
            && let Some(Arg::Object(Some(o))) = call.args.first()
        {
            self.objectives.insert(*o, None);
            if let Some(Arg::Team(Some(t))) = call.args.get(1) {
                self.object_teams.insert(*o, *t);
            }
            return;
        }
        if let Some(Arg::Team(Some(team))) = call.args.first()
            && matches!(call.entry.id, id::SPAWN_AREAS | id::SPAWN_NEAR)
        {
            // As the VM keeps it (`vm::Spawns`): `spawn_near` with no zone
            // or a count of 0 or less lifts the team's rule.
            let rule = match (call.entry.id, call.args.get(1), call.args.get(2)) {
                (id::SPAWN_AREAS, Some(Arg::Label(label)), Some(Arg::Int(n))) => Some(FakeSpawns::Areas {
                    label: label.clone(),
                    number: *n,
                }),
                (id::SPAWN_NEAR, Some(Arg::Object(Some(zone))), Some(Arg::Int(n))) if *n > 0 => {
                    Some(FakeSpawns::Near {
                        zone: *zone,
                        count: (*n).min(crate::vm::MAX_SPAWN_NEAR),
                    })
                }
                _ => None,
            };
            match rule {
                Some(r) => {
                    self.spawns.insert(*team, r);
                }
                None => {
                    self.spawns.remove(team);
                }
            }
            return;
        }
        if matches!(call.entry.id, id::SHOW_AREA | id::HIDE_AREA)
            && let Some(Arg::Object(Some(zone))) = call.args.first()
        {
            if call.entry.id == id::HIDE_AREA {
                self.shown.retain(|(z, _)| z != zone);
            } else {
                let team = match call.args.get(1) {
                    Some(Arg::Team(Some(t))) => Some(*t),
                    _ => None,
                };
                match self.shown.iter_mut().find(|(z, _)| z == zone) {
                    Some(row) => row.1 = team,
                    None => self.shown.push((*zone, team)),
                }
            }
            return;
        }
        if call.entry.id == id::TELEPORT {
            // The engine answers false for a position outside the world:
            // a refused call, which moves nothing.
            if let [Arg::Object(Some(o)), Arg::Fixed(x), Arg::Fixed(y), Arg::Fixed(z)] = call.args[..] {
                let at = [x, y, z];
                if self.world_limit.is_some_and(|w| at.iter().any(|c| c.unsigned_abs() > w.unsigned_abs())) {
                    self.refused.push(("teleport", format!("{at:?} is outside the world")));
                } else {
                    self.object_positions.insert(o, at);
                }
            }
            return;
        }
        if matches!(call.entry.id, id::MOVE_TO | id::STOP) {
            // The VM keeps the mover; the steps arrive as `step_object`.
            return;
        }
        if call.entry.id == id::SET_BODY
            && let Some(Arg::Team(Some(team))) = call.args.first()
        {
            match call.args.get(1) {
                Some(Arg::Tag(Some(path))) => {
                    if let Some(b) = crate::bodies::of_path(path) {
                        self.bodies.insert(*team, b);
                    }
                }
                _ => {
                    self.bodies.remove(team);
                }
            }
            return;
        }
        let slot = match call.args.first() {
            Some(Arg::Player(Some(p))) => Some(*p),
            _ => None,
        };
        // `respawn` gives one life, which the next death takes again.
        let counts_lives = self.variant("lives") > 0;
        let Some(player) = slot.and_then(|s| self.players.get_mut(&s)) else {
            return;
        };
        match (call.entry.id, call.args.get(1)) {
            (id::SCORE, Some(Arg::Int(n))) => player.score = player.score.saturating_add(*n),
            (id::SET_TEAM, Some(Arg::Team(Some(t)))) => player.team = *t,
            (id::SET_LIVES, Some(Arg::Int(n))) => player.lives = *n,
            (id::RESPAWN, _) => {
                player.alive = true;
                if counts_lives {
                    player.lives = player.lives.max(1);
                }
            }
            _ => {}
        }
    }

    fn step_object(&mut self, object: u32, at: [i32; 3], velocity: [i32; 3]) {
        self.steps.push((object, at, velocity));
        self.object_positions.insert(object, at);
        self.object_velocities.insert(object, velocity);
    }

    fn script_error(&mut self, error: &ScriptError) {
        self.script_error = Some(*error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kill_counts_and_a_spawn_revives() {
        let mut g = FakeGame::with_players(2);
        assert!(g.apply(&Input::Kill { victim: 1, killer: Some(0) }));
        assert!(!g.players[&1].alive);
        assert_eq!((g.players[&0].kills, g.players[&1].deaths), (1, 1));
        assert!(g.apply(&Input::Kill { victim: 0, killer: Some(0) }));
        assert_eq!(g.players[&0].kills, 1, "a suicide counts no kill");
        assert!(g.apply(&Input::Spawn(1)));
        assert!(g.players[&1].alive);
        assert!(g.apply(&Input::Join(4)));
        assert!(g.players[&4].alive);
        g.after(&Input::Quit(4));
        assert!(!g.players.contains_key(&4));
    }

    #[test]
    fn a_variant_that_counts_lives_holds_respawns() {
        let mut g = FakeGame::with_players(2);
        g.variant.insert("lives".into(), 1);
        for p in g.players.values_mut() {
            p.lives = 1;
        }
        // A death takes the life; the engine's own respawn try is held,
        // changes nothing and reports nothing.
        assert!(g.apply(&Input::Kill { victim: 0, killer: Some(1) }));
        assert!(!g.players[&0].alive);
        assert_eq!(g.players[&0].lives, 0);
        assert!(!g.apply(&Input::Spawn(0)), "out of lives");
        assert!(!g.players[&0].alive);
        // The script's `respawn` gives one life back; the next death
        // takes it again.
        let respawn = Call {
            entry: crate::caps::entry(id::RESPAWN, crate::caps::API_VERSION).unwrap(),
            args: vec![Arg::Player(Some(0))],
        };
        g.act(&respawn);
        assert!(g.players[&0].alive);
        assert_eq!(g.players[&0].lives, 1);
        assert!(g.apply(&Input::Death(0)));
        assert!(!g.players[&0].alive);
        assert_eq!(g.players[&0].lives, 0);
        // A variant that does not count lives never holds a spawn.
        g.variant.insert("lives".into(), 0);
        assert!(g.apply(&Input::Spawn(0)));
        assert!(g.players[&0].alive);
    }

    #[test]
    fn an_objective_is_carried_until_dropped_or_its_carrier_dies() {
        let mut g = FakeGame::with_players(2);
        g.objectives.insert(9, None);
        g.apply(&Input::Pickup(0, 9));
        assert_eq!(g.objectives[&9], Some(0));
        // Another player's drop is not this one's.
        g.apply(&Input::Drop(1, 9));
        assert_eq!(g.objectives[&9], Some(0));
        g.apply(&Input::Drop(0, 9));
        assert_eq!(g.objectives[&9], None);
        g.apply(&Input::Pickup(1, 9));
        g.apply(&Input::Kill { victim: 1, killer: Some(0) });
        assert_eq!(g.objectives[&9], None);
        // An object that is no objective has no carrier.
        g.apply(&Input::Pickup(0, 77));
        assert!(!g.objectives.contains_key(&77));
    }

    #[test]
    fn salted_datums_are_keys_like_any_other() {
        let mut g = FakeGame::with_players(2);
        let ball = 0xE1A3_0012u32;
        g.objectives.insert(ball, None);
        assert_eq!(g.objectives(), [ball]);
        g.apply(&Input::Pickup(1, ball));
        assert_eq!(g.objectives[&ball], Some(1));
        let is = Call {
            entry: crate::caps::entry_named("is_objective").unwrap(),
            args: vec![Arg::Object(Some(ball))],
        };
        assert_eq!(g.read(&is), 1);
        let carrier = Call {
            entry: crate::caps::entry_named("carrier_of").unwrap(),
            args: vec![Arg::Object(Some(ball))],
        };
        assert_eq!(g.read(&carrier), 1);
    }

    #[test]
    fn a_message_is_logged_with_its_values() {
        let mut g = FakeGame::with_players(2);
        let call = Call {
            entry: crate::caps::entry(id::MESSAGE, crate::caps::API_VERSION).unwrap(),
            args: vec![
                Arg::Player(Some(0)),
                Arg::Text("{player} scored, {time} left".into()),
                Arg::Player(Some(1)),
                Arg::Int(95 * crate::caps::TICKS_PER_SECOND),
            ],
        };
        g.act(&call);
        assert_eq!(
            g.messages,
            [FakeMessage {
                to: Some(Arg::Player(Some(0))),
                audience: crate::show::player(0),
                template: "{player} scored, {time} left".into(),
                values: vec![Arg::Player(Some(1)), Arg::Int(95 * crate::caps::TICKS_PER_SECOND)],
                line: "player 1 scored, 1:35 left".into(),
            }]
        );
        assert_eq!(g.calls_to("message").len(), 1);
        // Everyone's form has no `to`; an enum's value is its name.
        let all = Call {
            entry: crate::caps::entry(id::MESSAGE_ALL, crate::caps::API_VERSION).unwrap(),
            args: vec![Arg::Text("{{phase}} {enum}".into()), Arg::Text("attack".into())],
        };
        g.act(&all);
        assert_eq!(g.messages[1].to, None);
        assert_eq!(g.messages[1].line, "{phase} attack");
    }

    #[test]
    fn the_fake_logs_each_message_with_its_audience() {
        let mut g = FakeGame::with_players(4);
        g.players.get_mut(&1).unwrap().team = 1;
        g.players.get_mut(&3).unwrap().team = 1;
        let call = |entry: u16, args: Vec<Arg>| Call {
            entry: crate::caps::entry(entry, crate::caps::API_VERSION).unwrap(),
            args,
        };
        // A player: their bit.
        g.act(&call(
            id::MESSAGE,
            vec![Arg::Player(Some(2)), Arg::Text("hi".into())],
        ));
        assert_eq!(g.messages[0].audience, crate::show::player(2));
        // A team: the bits of its seated players.
        g.act(&call(
            id::MESSAGE_TEAM,
            vec![Arg::Team(Some(1)), Arg::Text("push".into())],
        ));
        assert_eq!(
            g.messages[1].audience,
            crate::show::player(1) | crate::show::player(3)
        );
        // A team with no players: no audience.
        g.act(&call(
            id::MESSAGE_TEAM,
            vec![Arg::Team(Some(5)), Arg::Text("push".into())],
        ));
        assert_eq!(g.messages[2].audience, 0);
        // Everyone: every slot.
        g.act(&call(id::MESSAGE_ALL, vec![Arg::Text("go".into())]));
        assert_eq!(g.messages[3].audience, crate::show::EVERYONE);
        // No player: no audience.
        g.act(&call(
            id::MESSAGE,
            vec![Arg::Player(None), Arg::Text("hi".into())],
        ));
        assert_eq!(g.messages[4].audience, 0);
    }

    #[test]
    fn the_dead_leave_their_zones() {
        let mut g = FakeGame::with_players(1);
        g.units.insert(100, 0);
        g.apply(&Input::Enter { zone: 7, object: 100 });
        g.apply(&Input::Kill { victim: 0, killer: None });
        g.apply(&Input::Spawn(0));
        assert_eq!(g.zone_holder(7), None, "a respawn is not where the death was");
    }

    #[test]
    fn the_zone_test_counts_living_units_of_one_team() {
        let mut g = FakeGame::with_players(3);
        g.players.get_mut(&2).unwrap().team = 1;
        for p in 0..3u8 {
            g.units.insert(100 + u32::from(p), p);
        }
        assert_eq!(g.zone_holder(7), None);
        g.apply(&Input::Enter { zone: 7, object: 100 });
        g.apply(&Input::Enter { zone: 7, object: 101 });
        assert_eq!(g.zone_holder(7), Some(0));
        g.apply(&Input::Enter { zone: 7, object: 102 });
        assert_eq!(g.zone_holder(7), None, "contested");
        g.apply(&Input::Kill { victim: 2, killer: Some(0) });
        assert_eq!(g.zone_holder(7), Some(0), "the dead do not hold");
        g.apply(&Input::Leave { zone: 7, object: 100 });
        g.apply(&Input::Leave { zone: 7, object: 101 });
        assert_eq!(g.zone_holder(7), None);
    }

    #[test]
    fn variant_fields_read_the_scenario_then_the_stock_record() {
        let mut g = FakeGame {
            carrier: 1,
            ..FakeGame::default()
        };
        assert_eq!(g.variant("team_play"), 1);
        assert_eq!(g.variant("score_limit"), 3);
        g.variant.insert("score_limit".into(), 5);
        assert_eq!(g.variant("score_limit"), 5);
        g.carrier = 2;
        assert_eq!(g.variant("team_play"), 0);
        assert_eq!(g.variant("friendly_fire"), 0);
    }

    #[test]
    fn the_round_clock_counts_down_to_zero() {
        let mut g = FakeGame {
            round_time_left: 2,
            ..FakeGame::default()
        };
        g.begin_tick(0);
        assert_eq!(g.round_time_left, 2);
        for t in 1..5 {
            g.begin_tick(t);
        }
        assert_eq!(g.round_time_left, 0);
    }

    #[test]
    fn a_sound_to_reaches_only_its_audience() {
        let mut g = FakeGame::with_players(4);
        g.players.get_mut(&1).unwrap().team = 1;
        g.players.get_mut(&3).unwrap().team = 1;
        let call = |entry: u16, args: Vec<Arg>| Call {
            entry: crate::caps::entry(entry, crate::caps::API_VERSION).unwrap(),
            args,
        };
        let tag = || Arg::Tag(Some("sound/stinger".into()));
        g.act(&call(id::SOUND_TO, vec![tag(), Arg::Player(Some(2))]));
        g.act(&call(id::SOUND_TO_TEAM, vec![tag(), Arg::Team(Some(1))]));
        g.act(&call(id::SOUND_TO_TEAM, vec![tag(), Arg::Team(Some(5))]));
        g.act(&call(id::SOUND_TO_ALL, vec![tag()]));
        // A call through `none` plays nothing.
        g.act(&call(id::SOUND_TO, vec![tag(), Arg::Player(None)]));
        g.act(&call(id::SOUND_TO, vec![Arg::Tag(None), Arg::Player(Some(0))]));
        let heard: Vec<_> = g.sounds.iter().map(|s| s.audience).collect();
        assert_eq!(
            heard,
            [
                crate::show::player(2),
                crate::show::player(1) | crate::show::player(3),
                0,
                crate::show::EVERYONE,
            ]
        );
        assert!(g.sounds.iter().all(|s| s.path == "sound/stinger"));
        assert_eq!(g.calls_to("sound_to").len(), 3);
    }

    #[test]
    fn a_script_plays_a_sound_at_the_boundary() {
        let src = "rules \"Stinger\" 1\ntype slayer\nrequires audio.play\n\
                   on kill(victim, killer) { sound_to_all(\"sound/stinger\") }\n";
        let (ir, _) = crate::build(src).unwrap();
        assert_eq!(ir.tags[0].group, crate::ir::TagGroup::Sound);
        let mut vm = crate::vm::Vm::new(ir, 0, 0).unwrap();
        let mut g = FakeGame::with_players(2);
        vm.begin_tick(1);
        vm.fire(&mut g, Input::Kill { victim: 0, killer: Some(1) });
        assert!(g.sounds.is_empty(), "a queued call is made at the next boundary");
        vm.begin_tick(2);
        vm.flush_queue(&mut g);
        assert_eq!(
            g.sounds,
            [FakeSound {
                path: "sound/stinger".into(),
                audience: crate::show::EVERYONE,
            }]
        );
    }
}
