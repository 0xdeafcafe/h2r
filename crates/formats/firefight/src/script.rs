//! Runs the handwritten Firefight director against the host's observations.
//! The wave-set module supplies tuning and tables to the rules compiler.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use ::rules::caps::id;
use ::rules::ir::{Ir, MAX_TABLE_ROWS};
use ::rules::vm::{Arg, Call, Game, Input, Placed, RulesExt, ScriptError, StateError, Vm};
use ::rules::Refusal as IrRefusal;

use crate::director::{
    Command, Observed, Phase, PlayerState, SQUAD_SLOTS, Settings, Slot, State,
};
use crate::scoring::ScoreReason;
use crate::{
    BONUS_SKULLS, Def, Difficulty, Drop, MAX_BONUS_ROWS, MULTIPLIER_SCALE, Outcome, Refusal, ShipKind,
    Skull, WAVES_PER_ROUND,
};

/// The index of `skull` in [`SKULLS`]: its bit, and its row in the tables.
fn skull_index(skull: Skull) -> Option<usize> {
    SKULLS.iter().position(|&entry| entry == skull)
}

const SOURCE: &str = include_str!("../../../../content/modes/rules/firefight.rules");
const WAVE_MODULE: &str = "firefight-waves.rules";
const ZONE_LABEL: &str = "ff_drop";
const SLOT_LABEL: &str = "ff_slots";
const SLOT_OBJECT_BASE: u32 = 256;
const O_DIFFICULTY: u8 = 0;
const O_SKULL0: u8 = 1;

const SKULLS: [Skull; 9] = [
    Skull::Assassins,
    Skull::Thunderstorm,
    Skull::Famine,
    Skull::BlackEye,
    Skull::Ghost,
    Skull::Catch,
    Skull::Mythic,
    Skull::Angry,
    Skull::LowGravity,
];
const DIFFICULTIES: [Difficulty; 4] = [
    Difficulty::Easy,
    Difficulty::Normal,
    Difficulty::Heroic,
    Difficulty::Legendary,
];

// `phase` values.
const WARMUP: i32 = 0;
const SPAWNING: i32 = 1;
const FIGHTING: i32 = 2;
const CLEANUP: i32 = 3;
const INTERMISSION: i32 = 4;
const DONE: i32 = 5;
const BONUS: i32 = 6;

/// Why a definition did not load into the director script.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScriptRefusal {
    #[error(transparent)]
    Def(#[from] Refusal),
    #[error("the definition has {rows} squads; a rules table holds {MAX_TABLE_ROWS}")]
    Squads { rows: usize },
    #[error("the director script could not be loaded: {0}")]
    Source(String),
    #[error("the director script does not verify: {0}")]
    Verify(#[from] IrRefusal),
    #[error(transparent)]
    State(#[from] StateError),
}

/// Loads the handwritten director with a validated wave set as its data.
pub fn load(def: &Def) -> Result<Ir, ScriptRefusal> {
    crate::validate::definition(def)?;
    let data = wave_module(def)?;

    // Names may contain quotes and backslashes. Definition validation
    // rejects control characters, so these are the only escapes needed.
    let name = def.name.replace('\\', "\\\\").replace('"', r#"\""#);
    let source = SOURCE.replacen("rules \"Firefight\" 1", &format!("rules \"{name}\" 1"), 1);
    let (ir, _) = ::rules::ship_files("firefight.rules", &source, &|file| {
        (file == WAVE_MODULE).then(|| data.clone())
    })
    .map_err(|error| ScriptRefusal::Source(error.to_string()))?;
    Ok(ir)
}

fn wave_module(def: &Def) -> Result<String, ScriptRefusal> {
    let ally_rows = def.rounds.iter().map(|round| round.allies.len()).sum::<usize>();
    let squad_rows = def
        .rounds
        .iter()
        .flat_map(|round| &round.waves)
        .map(|wave| wave.squads.len())
        .sum::<usize>()
        + usize::from(def.bonus.is_some())
        + ally_rows;
    if squad_rows > MAX_TABLE_ROWS {
        return Err(ScriptRefusal::Squads { rows: squad_rows });
    }

    let mut data = format!("teams {}\n", def.teams);
    let largest = def
        .caps
        .live_actors_by_players
        .last()
        .map_or(1, |&(n, _)| n);
    let pool = def.lives.pool_start;
    let constants = [
        ("team_count", u32::from(def.teams)),
        ("shared_lives", u32::from(def.lives.shared)),
        ("start_pool", u32::from(pool)),
        ("second_pool", if def.teams == 2 { u32::from(pool) } else { 0 }),
        ("bonus_lives", u32::from(def.lives.per_round_bonus)),
        ("player_kill_points", u32::from(def.scoring.player_kill)),
        ("assist_share", u32::from(def.scoring.assist)),
        ("largest_roster", u32::from(largest)),
        ("waves_per_round", WAVES_PER_ROUND as u32),
        ("endless", u32::from(def.set.endless)),
        ("set_scale", u32::from(def.set.scale)),
        ("set_skull_count", def.set.skulls.len() as u32),
        ("zero_holds", u32::from(def.lives.zero_holds)),
        ("bonus_on", u32::from(def.bonus.is_some())),
        ("bonus_ticks", def.bonus.as_ref().map_or(0, |b| b.ticks)),
        ("set_rounds", set_rounds(def)),
    ];
    for (name, value) in constants {
        writeln!(data, "const {name} = {value}").unwrap();
    }

    data.push_str("table squads: (kind: Class, per_player: int, minimum: int, maximum: int, preferred: int, tier: int, zone_role: int) = [\n");
    for squad in def.rounds.iter().flat_map(|r| &r.waves).flat_map(|w| &w.squads) {
        let (preferred, role) = drop_columns(squad.drop);
        writeln!(
            data,
            "(\"{}\", {}, {}, {}, {preferred}, {}, {role}),",
            squad.class,
            squad.count.per_player,
            squad.count.min,
            squad.count.max,
            squad.weapon_tier,
        )
        .unwrap();
    }
    // The bonus squad is the last row, after every wave's.
    if let Some(bonus) = &def.bonus {
        let squad = &bonus.squad;
        let (preferred, role) = drop_columns(squad.drop);
        writeln!(
            data,
            "(\"{}\", {}, {}, {}, {preferred}, {}, {role}),",
            squad.class, squad.count.per_player, squad.count.min, squad.count.max, squad.weapon_tier,
        )
        .unwrap();
    }
    // The rounds' allies: a row each, and each round's first row and count.
    // A table needs a row, so a definition with none has one that no
    // round's count reaches.
    data.push_str("]\ntable ally_squads: (kind: Class, per_player: int, minimum: int, maximum: int, preferred: int, tier: int, zone_role: int) = [\n");
    for squad in def.rounds.iter().flat_map(|round| &round.allies) {
        let (preferred, role) = drop_columns(squad.drop);
        writeln!(
            data,
            "(\"{}\", {}, {}, {}, {preferred}, {}, {role}),",
            squad.class,
            squad.count.per_player,
            squad.count.min,
            squad.count.max,
            squad.weapon_tier,
        )
        .unwrap();
    }
    if ally_rows == 0 {
        let class = &def.rounds[0].waves[0].squads[0].class;
        writeln!(data, "(\"{class}\", 0, 1, 1, -1, 0, 0),").unwrap();
    }
    data.push_str("]\ntable ally_rounds: (first: int, count: int) = [\n");
    let mut first = 0;
    for round in &def.rounds {
        writeln!(data, "({first}, {}),", round.allies.len()).unwrap();
        first += round.allies.len();
    }
    data.push_str("]\ntable waves: (first: int, count: int, gap: int) = [\n");
    let mut first = 0;
    for wave in def.rounds.iter().flat_map(|r| &r.waves) {
        let count = wave.squads.len();
        writeln!(data, "({first}, {count}, {}),", wave.gap_ticks).unwrap();
        first += count;
    }
    if def.bonus.is_some() {
        writeln!(data, "({first}, 1, 0),").unwrap();
    }

    data.push_str("]\ntable round_data: (s0: int, s1: int, s2: int, s3: int, s4: int, s5: int, s6: int, s7: int, s8: int) = [\n");
    for round in &def.rounds {
        let row = std::array::from_fn::<_, 9, _>(|i| {
            round.skulls_on.get(i)
                .and_then(|&skull| skull_index(skull))
                .map_or(-1, |index| index as i32)
                .to_string()
        });
        writeln!(data, "({}),", row.join(", ")).unwrap();
    }

    // The set skulls, by `skull_data` row: sets 2, 3 and 4. A table needs a
    // row, so an empty list is one `-1` that `set_skull_count` (0) never reads.
    data.push_str("]\ntable set_skulls: (which: int) = [\n");
    for skull in &def.set.skulls {
        let index = skull_index(*skull).expect("a set skull is one the director knows");
        writeln!(data, "({index}),").unwrap();
    }
    if def.set.skulls.is_empty() {
        data.push_str("(-1),\n");
    }
    data.push_str("]\ntable actor_caps: (actors: int) = [\n");
    for players in 1..=largest {
        writeln!(data, "({}),", def.caps.for_players(players).unwrap_or(0)).unwrap();
    }
    data.push_str("]\ntable class_points: (kind: Class, value: int) = [\n");
    for (class, points) in &def.scoring.by_class {
        writeln!(data, "(\"{class}\", {points}),").unwrap();
    }
    data.push_str("]\ntable difficulties: (multiplier: int) = [\n");
    for difficulty in DIFFICULTIES {
        writeln!(data, "({}),", def.scoring.difficulty_multiplier.get(difficulty)).unwrap();
    }

    data.push_str("]\ntable skull_data: (id: int, bit: int, multiplier: int, scheduled: int) = [\n");
    for (index, skull) in SKULLS.iter().enumerate() {
        let multiplier = def
            .scoring
            .skull_multipliers
            .get(skull)
            .copied()
            .unwrap_or(MULTIPLIER_SCALE);
        // `scheduled`: some round switches it on, so a new set switches it off.
        let scheduled = def.rounds.iter().any(|round| round.skulls_on.contains(skull));
        writeln!(
            data,
            "({}, {}, {multiplier}, {}),",
            *skull as i32,
            1 << index,
            u8::from(scheduled)
        )
        .unwrap();
    }

    // The skulls the bonus round switches on, as places in the skull list.
    data.push_str("]\ntable bonus_skulls: (s0: int, s1: int, s2: int, s3: int, s4: int, s5: int, s6: int, s7: int, s8: int) = [\n");
    let listed = BONUS_SKULLS.map(|skull| SKULLS.iter().position(|entry| *entry == skull));
    let row = std::array::from_fn::<_, 9, _>(|i| {
        listed
            .get(i)
            .copied()
            .flatten()
            .map_or(-1, |index| index as i32)
            .to_string()
    });
    writeln!(data, "({}),", row.join(", ")).unwrap();

    // The points-to-lives table in one row, unused places -1.
    data.push_str("]\ntable bonus_rows: (");
    let names: Vec<String> = (0..MAX_BONUS_ROWS)
        .flat_map(|i| [format!("need{i}: int"), format!("extra{i}: int")])
        .collect();
    data.push_str(&names.join(", "));
    data.push_str(") = [\n(");
    let cells: Vec<String> = (0..MAX_BONUS_ROWS)
        .flat_map(|i| {
            let row = def
                .bonus
                .as_ref()
                .and_then(|bonus| bonus.lives_by_points.get(i));
            match row {
                Some(&(points, lives)) => [points.to_string(), lives.to_string()],
                None => ["-1".to_string(), "0".to_string()],
            }
        })
        .collect();
    data.push_str(&cells.join(", "));
    data.push_str("),\n]\n");
    Ok(data)
}

/// A squad's preferred zone and zone-role mask columns.
fn drop_columns(drop: Drop) -> (i32, i32) {
    match drop {
        Drop::Edge => (-1, 0),
        Drop::PreferZone(zone) => (i32::from(zone), 0),
        Drop::Role(role) => (-1, i32::from(role.bit())),
    }
}

/// Rounds in a set: the bonus round follows each set's last round. A
/// definition is one set until it grows its own (`Def.set`).
fn set_rounds(def: &Def) -> u32 {
    def.rounds.len().max(1) as u32
}

/// The option bits for a match's settings.
pub fn options(ir: &Ir, settings: &Settings) -> u32 {
    let offsets = ir.option_offsets();
    let at = |index: u8| offsets.get(usize::from(index)).copied().unwrap_or(0);
    let difficulty = DIFFICULTIES
        .iter()
        .position(|&d| d == settings.difficulty)
        .unwrap_or(1) as u32;
    let mut bits = difficulty << at(O_DIFFICULTY);
    for (k, skull) in SKULLS.iter().enumerate() {
        if settings.skulls.contains(skull) {
            bits |= 1 << at(O_SKULL0 + k as u8);
        }
    }
    bits
}

/// The match seed to hand the VM so that its generator starts where the
/// Rust director's does (the settings' seed mixed with the definition's
/// hash): the VM mixes in the blob's hash itself ([`::rules::vm::seed`]).
pub fn match_seed(def: &Def, ir: &Ir, seed: u64) -> Result<u64, ScriptRefusal> {
    let prefix = |bytes: &[u8]| {
        let mut b = [0u8; 8];
        b.copy_from_slice(&bytes[..8]);
        u64::from_le_bytes(b)
    };
    Ok(seed ^ prefix(&def.id()?.0) ^ prefix(&ir.id().0))
}

/// The Firefight director run as a rules script on the VM.
pub struct ScriptDirector {
    def: Def,
    vm: Vm,
    teams: u8,
}

impl ScriptDirector {
    /// Start a match. The definition must be structurally valid.
    pub fn new(def: Def, settings: Settings) -> Result<Self, ScriptRefusal> {
        let ir = load(&def)?;
        let seed = match_seed(&def, &ir, settings.seed)?;
        let bits = options(&ir, &settings);
        Ok(Self {
            teams: def.teams,
            vm: Vm::new(ir, bits, seed)?,
            def,
        })
    }

    /// Resume from saved VM state, e.g. a film key or a host handover.
    pub fn resume(def: Def, settings: &Settings, ext: RulesExt) -> Result<Self, ScriptRefusal> {
        let ir = load(&def)?;
        let bits = options(&ir, settings);
        Ok(Self {
            teams: def.teams,
            vm: Vm::resume(ir, bits, ext)?,
            def,
        })
    }

    /// The definition the script was built from.
    pub fn def(&self) -> &Def {
        &self.def
    }

    /// The definition's player teams: 1, or 2.
    pub fn teams(&self) -> u8 {
        self.teams
    }

    pub fn vm(&self) -> &Vm {
        &self.vm
    }

    /// The VM state: what `SimExt` carries for the mode.
    pub fn ext(&self) -> &RulesExt {
        self.vm.ext()
    }

    /// The script error that stopped the VM, if one did.
    pub fn error(&self) -> Option<ScriptError> {
        self.vm.ext().stopped
    }

    /// Advance one tick. The host fires
    /// `on init` once, `on join` for each player present for the first
    /// time (in slot order), `on actor_kill` and `on actor_assist` for each
    /// kill in event order, `on kill` for each player killed by a player,
    /// then the per-tick group; every command is a queued call, made in
    /// program order at the end.
    pub fn step(&mut self, tick: u32, seen: &Observed) -> Vec<Command> {
        let mut host = Host {
            seen,
            present: present(seen),
            teams: self.teams,
            out: Vec::new(),
        };
        self.vm.begin_tick(tick);
        if self.vm.global("started") == Some(0) {
            self.vm.fire(&mut host, Input::Init);
        }
        let joining: Vec<u8> = host
            .present
            .keys()
            .copied()
            .filter(|&slot| self.vm.player_var(slot, "registered") == Some(0))
            .collect();
        for slot in joining {
            self.vm.fire(&mut host, Input::Join(slot));
        }
        for kill in &seen.kills {
            self.vm.fire(
                &mut host,
                Input::ActorKill {
                    killer: kill.killer,
                    class: kill.class.clone(),
                },
            );
            if kill.assist.is_some() {
                self.vm.fire(
                    &mut host,
                    Input::ActorAssist {
                        killer: kill.killer,
                        assist: kill.assist,
                        class: kill.class.clone(),
                    },
                );
            }
        }
        for kill in &seen.player_kills {
            self.vm.fire(
                &mut host,
                Input::Kill {
                    victim: kill.victim,
                    killer: kill.killer,
                },
            );
        }
        self.vm.update(&mut host);
        self.vm.flush_queue(&mut host);
        host.out
    }

    /// The director's state, read back from the script's variables.
    pub fn state(&self) -> State {
        let g = |name: &str| self.vm.global(name).unwrap_or(0);
        let at = g("phase_tick") as u32;
        let phase = match g("phase") {
            WARMUP => Phase::Warmup { until: at },
            SPAWNING => Phase::Spawning,
            FIGHTING => Phase::Fighting,
            CLEANUP => Phase::Cleanup { since: at },
            INTERMISSION => Phase::Intermission { until: at },
            BONUS => Phase::Bonus { until: at },
            DONE => Phase::Done,
            // A phase the script never writes reads as finished.
            _ => Phase::Done,
        };
        let slots = std::array::from_fn(|s| {
            let object = SLOT_OBJECT_BASE + s as u32;
            let field = |name| self.vm.object_var(object, name).unwrap_or(0);
            let mode = field("squad_state");
            let since = field("squad_tick") as u32;
            match mode {
                1 | 2 => Slot::Placed {
                    count: field("squad_count") as u16,
                    alive: field("squad_remaining") as u16,
                    seen: mode == 2,
                    placed_at: since,
                    target: field("squad_side") as u8,
                    ally: field("squad_ally") > 0,
                },
                3 => Slot::Draining { since },
                _ => Slot::Free,
            }
        });
        let mut players = BTreeMap::new();
        for slot in 0..16u8 {
            if self.vm.player_var(slot, "registered") == Some(1) {
                players.insert(
                    slot,
                    PlayerState {
                        lives: self.vm.player_var(slot, "remaining_lives").unwrap_or(0) as u16,
                        respawning: self.vm.player_var(slot, "respawning") == Some(1),
                        team: self.vm.player_var(slot, "side").unwrap_or(0) as u8,
                    },
                );
            }
        }
        let mask = g("skulls");
        let skulls: BTreeSet<Skull> = SKULLS
            .iter()
            .enumerate()
            .filter(|&(k, _)| mask & (1 << k) != 0)
            .map(|(_, &s)| s)
            .chain((g("iron") == 1).then_some(Skull::Iron))
            .collect();
        State {
            phase,
            set: g("set_index") as u8,
            round: g("round") as u8,
            wave: g("wave") as u8,
            next_squad: g("next_squad") as u8,
            wave_actors: g("wave_actors") as u16,
            slots,
            pools: [g("pool_0") as u16, g("pool_1") as u16],
            scores: [g("score_0") as u32, g("score_1") as u32],
            players,
            skulls,
            rng: self.vm.ext().rng,
            started: g("started") == 1,
        }
    }

    /// Set a team's shared pool from outside the script: a `TeamLives` the
    /// host applies, an operator's among them. The engine keeps no team
    /// pool, so the script's `pool_<team>` is the pool: the next step
    /// spends respawns from it, and a team at 0 waits for the wave to end.
    /// Refused for a team the definition does not have, and when lives are
    /// not shared.
    pub fn set_team_pool(&mut self, team: u8, pool: u16) -> std::result::Result<(), String> {
        if team >= self.def.teams {
            return Err(format!(
                "the match has {} player team(s), not team {team}",
                self.def.teams
            ));
        }
        if !self.def.lives.shared {
            return Err("lives are each player's own, not a team's pool".into());
        }
        if self.vm.set_global(&format!("pool_{team}"), i32::from(pool)) {
            Ok(())
        } else {
            Err(format!("the script keeps no pool for team {team}"))
        }
    }

    /// Set the shared pool of one team (`player` none) or a player's own
    /// lives from outside the script: a `Lives` the host applies. Refused
    /// where the definition keeps no such count: a shared pool with two
    /// teams (that is `TeamLives`), a player's own lives when lives are
    /// shared, and a player the script has not seen.
    pub fn set_lives(&mut self, player: Option<u8>, pool: u16) -> std::result::Result<(), String> {
        let shared = self.def.lives.shared;
        match player {
            None if !shared => Err("lives are each player's own: name the player".into()),
            None if self.teams > 1 => {
                Err("with two teams each team has its own pool: set it with team_lives".into())
            }
            None => self
                .vm
                .set_global("pool_0", i32::from(pool))
                .then_some(())
                .ok_or_else(|| "the script keeps no shared pool".to_string()),
            Some(_) if shared => Err("lives are a shared pool, not each player's own".into()),
            Some(slot) => {
                if self.vm.player_var(slot, "registered") != Some(1) {
                    return Err(format!("the director has not seen player {slot}"));
                }
                if self.vm.set_player_var(slot, "remaining_lives", i32::from(pool)) {
                    Ok(())
                } else {
                    Err(format!("the script keeps no lives for player {slot}"))
                }
            }
        }
    }

    /// Enemy actors the script counts as alive: an ally is counted apart
    /// ([`ScriptDirector::live_allies`]).
    pub fn live_actors(&self) -> u16 {
        self.live(false)
    }

    /// Ally actors the script counts as alive.
    pub fn live_allies(&self) -> u16 {
        self.live(true)
    }

    fn live(&self, allies: bool) -> u16 {
        self.state()
            .slots
            .iter()
            .map(|slot| match *slot {
                Slot::Placed { alive, ally, .. } if ally == allies => alive,
                _ => 0,
            })
            .sum()
    }

    /// Actors of the current wave not yet placed, at `players`.
    pub fn reserve(&self, players: u8) -> u16 {
        let state = self.state();
        let players = self.supported_players(players);
        let current = || {
            self.def
                .rounds
                .get(usize::from(state.round))?
                .waves
                .get(usize::from(state.wave))
        };
        current().map_or(0, |wave| {
            wave.squads[usize::from(state.next_squad).min(wave.squads.len())..]
                .iter()
                .map(|squad| squad.count.for_players(players))
                .sum()
        })
    }

    /// The live actor cap at a player count, clamped to the definition's tiers.
    pub fn cap(&self, players: u8) -> u16 {
        let players = self.supported_players(players);
        self.def.caps.for_players(players).unwrap_or(0)
    }

    /// Clamp a player count to the definition's tiers (1 to the last bound).
    fn supported_players(&self, players: u8) -> u8 {
        let largest = self
            .def
            .caps
            .live_actors_by_players
            .last()
            .map_or(1, |&(upper, _)| upper);
        players.clamp(1, largest.max(1))
    }
}

/// A present player: alive, and the game team of its first entry.
#[derive(Clone, Copy)]
struct Seen {
    alive: bool,
    team: u8,
}

/// Present players by slot; a slot listed twice counts as alive if either
/// entry is, and keeps its first entry's team.
fn present(seen: &Observed) -> BTreeMap<u8, Seen> {
    let mut players = BTreeMap::new();
    for player in &seen.players {
        players
            .entry(player.slot)
            .or_insert(Seen {
                alive: false,
                team: player.team,
            })
            .alive |= player.alive;
    }
    players
}

/// The `Game` over one tick's `Observed`: reads answer from it, and each
/// call the script makes becomes a [`Command`].
struct Host<'a> {
    seen: &'a Observed,
    present: BTreeMap<u8, Seen>,
    teams: u8,
    out: Vec<Command>,
}

impl Host<'_> {
    fn kind_seen(&self, kind: i32) -> Option<&crate::director::KindSeen> {
        self.seen.ship.kinds.get(ShipKind::from_index(kind)?.index())
    }
}

impl Game for Host<'_> {
    fn players(&self) -> Vec<u8> {
        self.present.keys().copied().collect()
    }

    fn teams(&self) -> Vec<u8> {
        (0..self.teams).collect()
    }

    fn labelled(&self, label: &str) -> Vec<Placed> {
        let (count, base) = match label {
            ZONE_LABEL => (self.seen.zones.len().min(12), 0),
            SLOT_LABEL => (SQUAD_SLOTS, SLOT_OBJECT_BASE),
            _ => return Vec::new(),
        };
        (0..count as u32)
            .map(|index| Placed { placement: index, object: base + index })
            .collect()
    }

    fn variant(&self, _field: &str) -> i32 {
        0
    }

    fn read(&mut self, call: &Call) -> i32 {
        let zone = |z: &u32| self.seen.zones.get(*z as usize);
        match (call.entry.id, call.args.as_slice()) {
            (id::ALIVE, [Arg::Player(Some(p))]) => {
                i32::from(self.present.get(p).is_some_and(|seen| seen.alive))
            }
            // A game team is 0 to 7; a present player's, else none.
            (id::TEAM_OF, [Arg::Player(Some(p))]) => self
                .present
                .get(p)
                .map_or(-1, |seen| i32::from(seen.team.min(7))),
            (id::PLAYER_OF, [Arg::Object(Some(slot))])
                if (SLOT_OBJECT_BASE..SLOT_OBJECT_BASE + SQUAD_SLOTS as u32).contains(slot) =>
            {
                (*slot - SLOT_OBJECT_BASE) as i32
            }
            (id::SQUAD_ALIVE, [Arg::Squad(s)]) => self
                .seen
                .actors_alive_by_squad
                .get(usize::from(*s))
                .map_or(0, |&n| i32::from(n)),
            // A mask of 0 names no role: every zone fits.
            (id::ZONE_HAS_ROLE, [Arg::Object(Some(z)), Arg::Int(mask)]) => zone(z)
                .map_or(0, |z| i32::from(*mask == 0 || i32::from(z.roles) & *mask != 0)),
            (id::ZONE_PATH, [Arg::Int(z), Arg::Int(kind)]) => i32::from(
                self.kind_seen(*kind)
                    .zip(usize::try_from(*z).ok())
                    .and_then(|(seen, z)| seen.paths.get(z))
                    .copied()
                    .unwrap_or(false),
            ),
            (id::SHIP_READY, [Arg::Int(kind)]) => {
                i32::from(self.kind_seen(*kind).is_some_and(|seen| seen.ready))
            }
            // The kind the class arrives by, -1 for none.
            (id::SQUAD_SHIP, [Arg::Class(Some(class))]) => self
                .seen
                .ship
                .ship_of(class)
                .map_or(-1, |kind| kind.index() as i32),
            (id::ZONE_SEEN, [Arg::Object(Some(z))]) => zone(z).map_or(0, |z| i32::from(z.seen)),
            (id::ZONE_DISTANCE, [Arg::Object(Some(z))]) => {
                zone(z).map_or(0, |z| i32::try_from(z.distance).unwrap_or(i32::MAX))
            }
            (id::ZONE_TEAM_DISTANCE, [Arg::Object(Some(z)), Arg::Team(Some(t))]) => zone(z)
                .and_then(|z| z.team_distance.get(usize::from(*t)))
                .map_or(0, |&d| i32::try_from(d).unwrap_or(i32::MAX)),
            _ => 0,
        }
    }

    fn act(&mut self, call: &Call) {
        let command = command(call);
        debug_assert!(command.is_some(), "no command for {call:?}");
        self.out.extend(command);
    }

    fn script_error(&mut self, _error: &ScriptError) {}
}

/// The command a script call stands for.
fn command(call: &Call) -> Option<Command> {
    let narrow_u8 = |v: i32| u8::try_from(v).ok();
    let narrow_u16 = |v: i32| u16::try_from(v).ok();
    Some(match (call.entry.id, call.args.as_slice()) {
        (
            id::PLACE_SQUAD_AT,
            [
                Arg::Squad(slot),
                Arg::Class(Some(class)),
                Arg::Int(count),
                Arg::Int(zone),
                Arg::Int(tier),
            ],
        ) => Command::PlaceSquad {
            slot: *slot,
            class: class.clone(),
            count: narrow_u16(*count)?,
            zone: narrow_u8(*zone)?,
            weapon_tier: narrow_u8(*tier)?,
        },
        (
            id::FF_DROP_SQUAD,
            [
                Arg::Squad(slot),
                Arg::Class(Some(class)),
                Arg::Int(count),
                Arg::Int(zone),
                Arg::Int(tier),
                Arg::Int(ship),
            ],
        ) => Command::DropSquad {
            slot: *slot,
            class: class.clone(),
            count: narrow_u16(*count)?,
            zone: narrow_u8(*zone)?,
            weapon_tier: narrow_u8(*tier)?,
            ship: ShipKind::from_index(*ship)?,
        },
        (id::FF_SHIP_HOVER, [Arg::Int(ship), Arg::Bool(on)]) => Command::ShipHover {
            ship: ShipKind::from_index(*ship)?,
            on: *on,
        },
        (id::FF_SHIP_UNLOAD, [Arg::Int(ship)]) => Command::ShipUnload {
            ship: ShipKind::from_index(*ship)?,
        },
        (id::FF_SHIP_LEAVE, [Arg::Int(ship), Arg::Int(zone)]) => Command::ShipLeave {
            ship: ShipKind::from_index(*ship)?,
            zone: narrow_u8(*zone)?,
        },
        (id::HUNT, [Arg::Squad(slot)]) => Command::Hunt { slot: *slot },
        (id::ERASE_SQUAD, [Arg::Squad(slot)]) => Command::EraseSquad { slot: *slot },
        (id::FF_SET, [Arg::Int(index)]) => Command::Set {
            index: narrow_u8(*index)?,
        },
        (id::FF_ROUND, [Arg::Int(index)]) => Command::Round {
            index: narrow_u8(*index)?,
        },
        (id::FF_WAVE, [Arg::Int(index)]) => Command::Wave {
            index: narrow_u8(*index)?,
        },
        (id::FF_SKULL, [Arg::Int(skull), Arg::Bool(on)]) => Command::Skull {
            id: SKULLS
                .iter()
                .chain([&Skull::Iron])
                .find(|&&s| s as i32 == *skull)
                .copied()?,
            on: *on,
        },
        (id::FF_BONUS, [Arg::Bool(on), Arg::Int(ticks)]) => Command::Bonus {
            on: *on,
            ticks: u32::try_from(*ticks).ok()?,
        },
        (id::FF_LIVES, [Arg::Player(player), Arg::Int(pool)]) => Command::Lives {
            player: *player,
            pool: narrow_u16(*pool)?,
        },
        (id::FF_SCORE, [Arg::Player(Some(player)), Arg::Int(points), Arg::Bool(assist)]) => {
            Command::Score {
                player: *player,
                points: u32::try_from(*points).ok()?,
                reason: if *assist {
                    ScoreReason::Assist
                } else {
                    ScoreReason::Kill
                },
            }
        }
        (id::FF_RESPAWN, [Arg::Player(Some(player))]) => Command::Respawn { player: *player },
        (id::FF_FINISH, [Arg::Bool(won)]) => Command::Finish {
            outcome: if *won { Outcome::Won } else { Outcome::Lost },
        },
        (id::FF_TARGET, [Arg::Squad(slot), Arg::Team(Some(team))]) => Command::Target {
            slot: *slot,
            team: *team,
        },
        (id::FF_ALLY, [Arg::Squad(slot), Arg::Team(Some(team))]) => Command::Ally {
            slot: *slot,
            team: *team,
        },
        (id::FF_TEAM_LIVES, [Arg::Team(Some(team)), Arg::Int(pool)]) => Command::TeamLives {
            team: *team,
            pool: narrow_u16(*pool)?,
        },
        (id::FF_PLAYER_SCORE, [Arg::Player(Some(player)), Arg::Int(points)]) => Command::Score {
            player: *player,
            points: u32::try_from(*points).ok()?,
            reason: ScoreReason::PlayerKill,
        },
        (id::FF_TEAM_FINISH, [Arg::Team(team)]) => Command::Finish {
            outcome: match *team {
                Some(team) => Outcome::TeamWon { team },
                None => Outcome::Draw,
            },
        },
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rules::ir::LabelKind;

    use crate::builtin;

    #[test]
    fn every_builtin_loads_verifies_and_hashes_stably() {
        let mut ids = BTreeSet::new();
        for def in builtin::definitions() {
            let ir = load(&def).unwrap();
            assert_eq!(load(&def).unwrap().encode(), ir.encode());
            assert_eq!(Ir::decode(&ir.encode()).unwrap(), ir);
            assert!(::rules::verify::unhostable(&ir).is_none());
            assert!(ir.encode().len() <= ::rules::ir::MAX_IR_BYTES);
            ids.insert(ir.id());
        }
        assert_eq!(ids.len(), builtin::definitions().len());
    }

    #[test]
    fn the_director_bounds_the_kit_zones_and_has_one_record_per_slot() {
        let ir = load(&builtin::definitions()[0]).unwrap();
        let [zones, slots] = ir.labels.as_slice() else {
            panic!("expected drop-zone and squad-slot labels");
        };
        assert_eq!(
            (zones.text.as_str(), zones.kind, zones.min, zones.max),
            (ZONE_LABEL, LabelKind::Object, 0, 12)
        );
        assert_eq!(
            (slots.text.as_str(), slots.kind, slots.min, slots.max),
            (SLOT_LABEL, LabelKind::Object, SQUAD_SLOTS as u16, SQUAD_SLOTS as u16)
        );
    }

    #[test]
    fn options_carry_the_difficulty_and_the_starting_skulls() {
        let ir = load(&builtin::definitions()[0]).unwrap();
        let settings = Settings {
            seed: 0,
            difficulty: Difficulty::Legendary,
            skulls: [Skull::Assassins, Skull::LowGravity].into(),
        };
        assert_eq!(options(&ir, &settings), 3 | 1 << 2 | 1 << 10);
        let normal = Settings {
            seed: 0,
            difficulty: Difficulty::Normal,
            skulls: BTreeSet::new(),
        };
        assert_eq!(options(&ir, &normal), ir.default_options());
    }

    #[test]
    fn a_definition_with_more_squads_than_a_table_holds_is_refused() {
        let mut def = builtin::definitions()[0].clone();
        let mut squad = def.rounds[0].waves[0].squads[0].clone();
        squad.count = crate::Count {
            per_player: 0,
            min: 1,
            max: 1,
        };
        let mut round = def.rounds[0].clone();
        for wave in &mut round.waves {
            wave.squads = vec![squad.clone(); 6];
        }
        def.rounds = vec![round; 9];
        def.rounds.iter_mut().for_each(|r| r.skulls_on.clear());
        assert_eq!(load(&def), Err(ScriptRefusal::Squads { rows: 270 }));
    }
}
