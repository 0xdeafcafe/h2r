//! Firefight as a rules script: a front end that compiles a [`Def`] to
//! rules IR, and [`ScriptDirector`], which runs that IR on the rules VM
//! against what the host observed and returns the director's [`Command`]s.
//!
//! The definition stays the authoring format. Its waves, squads, rounds,
//! actor caps and scores become constant tables; the director's state
//! machine becomes the handlers. The front end builds IR directly rather
//! than rules source: the director's loops over the 16 squad slots unroll
//! at build time, nothing depends on the language's own lowering, and the
//! blob is a pure function of the definition, so its hash is too.
//!
//! The director's tests (`crates/formats/firefight/tests`) drive this over
//! their scenarios and built-in definitions. See `docs/game/firefight.md`,
//! "The director as a rules script".

use std::collections::{BTreeMap, BTreeSet};

use ::rules::caps::id;
use ::rules::ir::{
    ArithOp, CmpOp, Column, Event, Handler, Header, IR_VERSION, Ir, MAX_REGS, MAX_TABLE_ROWS,
    NumTy, Op, OptionDef, Reg, Scope, Set, SetBase, Slot as IrSlot, Table,
};
use ::rules::vm::{Arg, Call, Game, Input, Placed, RulesExt, ScriptError, StateError, Vm};
use ::rules::{Refusal as IrRefusal, Ty};

use crate::director::{
    CLEANUP_ACTORS, CLEANUP_TICKS, Command, Observed, Outcome, PLACE_PER_TICK, Phase, PlayerState,
    SLOT_TIMEOUT_TICKS, SQUAD_SLOTS, Settings, Slot, State,
};
use crate::scoring::ScoreReason;
use crate::{Def, Difficulty, Drop, MAX_TEAMS, MULTIPLIER_SCALE, Refusal, Skull, WAVES_PER_ROUND};

/// The Forge label of the kit's drop zones. The host reports zone `n` as
/// the labelled object with placement id `n`, so the VM visits zones in
/// kit order.
pub const ZONE_LABEL: &str = "ff_drop";

/// Skulls with Firefight effects, in [`Skull`] order: the option order and
/// the bit order of the script's skull mask.
const SKULLS: [Skull; 9] = [
    Skull::Assassins,
    Skull::Thunderstorm,
    Skull::Famine,
    Skull::Blind,
    Skull::Ghost,
    Skull::Catch,
    Skull::Mythic,
    Skull::Angry,
    Skull::LowGravity,
];
const SKULL_NAMES: [&str; 9] = [
    "assassins",
    "thunderstorm",
    "famine",
    "blind",
    "ghost",
    "catch",
    "mythic",
    "angry",
    "low_gravity",
];
const DIFFICULTIES: [Difficulty; 4] = [
    Difficulty::Easy,
    Difficulty::Normal,
    Difficulty::Heroic,
    Difficulty::Legendary,
];

/// Kill points are formed exactly in base-1,000 limbs: at most 1,000 class
/// points times ten factors of at most 2,000 thousandths is under 1000^13.
const LIMBS: usize = 13;
const LIMB: i32 = MULTIPLIER_SCALE as i32;

// `phase` values.
const WARMUP: i32 = 0;
const SPAWNING: i32 = 1;
const FIGHTING: i32 = 2;
const CLEANUP: i32 = 3;
const INTERMISSION: i32 = 4;
const DONE: i32 = 5;

// A squad slot's mode, the low two bits of `slot_NN_info`; then the placed
// count times four (at most 32, so below 256), and the squad's target team
// times 256.
const FREE: i32 = 0;
const PLACED: i32 = 1;
const SEEN: i32 = 2;
const DRAINING: i32 = 3;
/// `slot_NN_info`'s unit of the target team.
const TARGET_UNIT: i32 = 256;

// Tables and their columns.
const T_SQUADS: u8 = 0;
const Q_CLASS: u8 = 0;
const Q_PER_PLAYER: u8 = 1;
const Q_MIN: u8 = 2;
const Q_MAX: u8 = 3;
const Q_PREFER: u8 = 4;
const Q_TIER: u8 = 5;
const T_WAVES: u8 = 1;
const W_FIRST: u8 = 0;
const W_LEN: u8 = 1;
const W_GAP: u8 = 2;
const T_ROUNDS: u8 = 2;
const T_CAPS: u8 = 3;
const T_POINTS: u8 = 4;
const T_DIFFICULTY: u8 = 5;
const T_SKULLS: u8 = 6;
const S_ID: u8 = 0;
const S_BIT: u8 = 1;
const S_MULT: u8 = 2;

/// The label-table index of [`ZONE_LABEL`].
const ZONES: u16 = 0;
/// Option indices.
const O_DIFFICULTY: u8 = 0;
const O_SKULL0: u8 = 1;

/// Why a definition did not become a director script.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScriptRefusal {
    #[error(transparent)]
    Def(#[from] Refusal),
    #[error("the definition has {rows} squads; a rules table holds {MAX_TABLE_ROWS}")]
    Squads { rows: usize },
    #[error("on {0} needs more than {MAX_REGS} registers")]
    Registers(&'static str),
    #[error("the director script does not verify: {0}")]
    Verify(#[from] IrRefusal),
    #[error(transparent)]
    State(#[from] StateError),
}

fn info_name(slot: usize) -> String {
    format!("slot_{slot:02}_info")
}

fn alive_name(slot: usize) -> String {
    format!("slot_{slot:02}_alive")
}

fn tick_name(slot: usize) -> String {
    format!("slot_{slot:02}_tick")
}

/// Compiles a structurally valid definition to the director script. The
/// same definition always gives the same bytes.
pub fn compile(def: &Def) -> Result<Ir, ScriptRefusal> {
    crate::validate::definition(def)?;
    let mut st = Strings::default();

    // Slots.
    let mut slots = Vec::new();
    let pool_start = i32::from(def.lives.pool_start);
    let mut global = |st: &mut Strings, name: &str, ty: Ty, init: i32| {
        slot(&mut slots, st, name, Scope::Global, ty, init)
    };
    let phase = global(&mut st, "phase", Ty::Int, WARMUP);
    let phase_tick = global(&mut st, "phase_tick", Ty::Int, 0);
    let round = global(&mut st, "round", Ty::Int, 0);
    let wave = global(&mut st, "wave", Ty::Int, 0);
    let next = global(&mut st, "next_squad", Ty::Int, 0);
    let wave_actors = global(&mut st, "wave_actors", Ty::Int, 0);
    let pool = [
        global(&mut st, "pool_0", Ty::Int, pool_start),
        global(
            &mut st,
            "pool_1",
            Ty::Int,
            if def.teams > 1 { pool_start } else { 0 },
        ),
    ];
    let score = [
        global(&mut st, "score_0", Ty::Int, 0),
        global(&mut st, "score_1", Ty::Int, 0),
    ];
    let skulls = global(&mut st, "skulls", Ty::Int, 0);
    let started = global(&mut st, "started", Ty::Bool, 0);
    let mut info = [0u16; SQUAD_SLOTS];
    let mut alive = [0u16; SQUAD_SLOTS];
    let mut tick = [0u16; SQUAD_SLOTS];
    for s in 0..SQUAD_SLOTS {
        info[s] = global(&mut st, &info_name(s), Ty::Int, FREE);
        alive[s] = global(&mut st, &alive_name(s), Ty::Int, 0);
        tick[s] = global(&mut st, &tick_name(s), Ty::Int, 0);
    }
    let registered = slot(&mut slots, &mut st, "registered", Scope::Player, Ty::Bool, 0);
    let lives = slot(&mut slots, &mut st, "lives", Scope::Player, Ty::Int, pool_start);
    let respawning = slot(&mut slots, &mut st, "respawning", Scope::Player, Ty::Bool, 0);
    // The player's Firefight team, 0 or 1, fixed by `on join`.
    let team = slot(&mut slots, &mut st, "team", Scope::Player, Ty::Int, 0);

    // Options: the difficulty, then one switch per skull on from the start.
    let mut options = vec![OptionDef {
        name: st.get("difficulty"),
        label: st.get("Difficulty"),
        description: None,
        ty: Ty::Int,
        lo: 0,
        hi: 3,
        default: 1,
    }];
    for name in SKULL_NAMES {
        let key = st.get(&format!("skull_{name}"));
        options.push(OptionDef {
            name: key,
            label: st.get(name),
            description: None,
            ty: Ty::Bool,
            lo: 0,
            hi: 1,
            default: 0,
        });
    }

    // Tables.
    let skull_index = |skull: Skull| SKULLS.iter().position(|&s| s == skull).unwrap_or(0) as i32;
    let mut squad_cells = Vec::new();
    let mut wave_cells = Vec::new();
    let mut squad_rows = 0usize;
    for r in &def.rounds {
        for w in &r.waves {
            wave_cells.extend([squad_rows as i32, w.squads.len() as i32, w.gap_ticks as i32]);
            for q in &w.squads {
                let prefer = match q.drop {
                    Drop::Edge => -1,
                    Drop::PreferZone(zone) => i32::from(zone),
                };
                squad_cells.extend([
                    i32::from(st.get(&q.class)),
                    i32::from(q.count.per_player),
                    i32::from(q.count.min),
                    i32::from(q.count.max),
                    prefer,
                    i32::from(q.weapon_tier),
                ]);
                squad_rows += 1;
            }
        }
    }
    if squad_rows > MAX_TABLE_ROWS {
        return Err(ScriptRefusal::Squads { rows: squad_rows });
    }
    let skull_cols = def
        .rounds
        .iter()
        .map(|r| r.skulls_on.len())
        .max()
        .unwrap_or(0)
        .max(1);
    let mut round_cells = Vec::new();
    for r in &def.rounds {
        for k in 0..skull_cols {
            round_cells.push(r.skulls_on.get(k).map_or(-1, |&s| skull_index(s)));
        }
    }
    let caps_cells: Vec<i32> = def
        .caps
        .live_actors_by_players
        .iter()
        .flat_map(|&(upper, actors)| [i32::from(upper), i32::from(actors)])
        .collect();
    let points_cells: Vec<i32> = def
        .scoring
        .by_class
        .iter()
        .flat_map(|(class, &points)| [i32::from(st.get(class)), i32::from(points)])
        .collect();
    let difficulty_cells: Vec<i32> = DIFFICULTIES
        .iter()
        .map(|&d| i32::from(def.scoring.difficulty_multiplier.get(d)))
        .collect();
    let skull_cells: Vec<i32> = SKULLS
        .iter()
        .enumerate()
        .flat_map(|(k, skull)| {
            let multiplier = def
                .scoring
                .skull_multipliers
                .get(skull)
                .copied()
                .unwrap_or(MULTIPLIER_SCALE);
            [*skull as i32, 1 << k, i32::from(multiplier)]
        })
        .collect();
    let int = Ty::Int;
    let tables = vec![
        table(
            &mut st,
            "squads",
            &[
                ("class", Ty::Class),
                ("per_player", int),
                ("min", int),
                ("max", int),
                ("prefer_zone", int),
                ("weapon_tier", int),
            ],
            squad_cells,
        ),
        table(
            &mut st,
            "waves",
            &[("first_squad", int), ("squads", int), ("gap_ticks", int)],
            wave_cells,
        ),
        table(
            &mut st,
            "rounds",
            &(0..skull_cols).map(|_| ("skull", int)).collect::<Vec<_>>(),
            round_cells,
        ),
        table(&mut st, "caps", &[("players", int), ("actors", int)], caps_cells),
        table(&mut st, "points", &[("class", Ty::Class), ("points", int)], points_cells),
        table(&mut st, "difficulty", &[("multiplier", int)], difficulty_cells),
        table(
            &mut st,
            "skulls",
            &[("id", int), ("bit", int), ("multiplier", int)],
            skull_cells,
        ),
    ];

    let g = Gen {
        tys: slots.iter().map(|s| s.ty).collect(),
        v: Vars {
            phase,
            phase_tick,
            round,
            wave,
            next,
            wave_actors,
            pool,
            score,
            skulls,
            started,
            info,
            alive,
            tick,
            registered,
            lives,
            respawning,
            team,
        },
        two_teams: def.teams > 1,
        player_kill: i32::from(def.scoring.player_kill),
        shared: def.lives.shared,
        pool_start,
        bonus: i32::from(def.lives.per_round_bonus),
        rounds: def.rounds.len() as i32,
        largest: def
            .caps
            .live_actors_by_players
            .last()
            .map_or(1, |&(upper, _)| i32::from(upper))
            .max(1),
        assist: i32::from(def.scoring.assist),
        caps_rows: def.caps.live_actors_by_players.len(),
        skull_cols: skull_cols as u8,
    };
    let mut handlers = vec![
        on_init(&g)?,
        on_join(&g)?,
        on_actor_kill(&g)?,
        on_actor_assist(&g)?,
    ];
    if g.two_teams && g.player_kill > 0 {
        handlers.push(on_kill(&g)?);
    }
    handlers.push(on_tick(&g)?);

    let ir = Ir {
        header: Header {
            ir_version: IR_VERSION,
            api_version: ::rules::caps::API_VERSION,
            name: def.name.clone(),
            version: 1,
            // The Firefight type rides Slayer's engine, on one or two
            // player teams; the actors' team is the host's.
            game_type: ::rules::types::Type::Firefight,
            uses: None,
            teams: def.teams,
            points: ::rules::types::Type::Firefight.points().into(),
            about: String::new(),
            requires: vec!["ai.squads".into(), "firefight.director".into()],
        },
        strings: st.list,
        options,
        settings: Vec::new(),
        quick: Vec::new(),
        enums: Vec::new(),
        slots,
        // The kit's drop zones: a set of objects, at most 256, that the
        // host reports from the kit rather than a Forge map.
        labels: vec![::rules::ir::LabelDef::objects(ZONE_LABEL)],
        tags: Vec::new(),
        palettes: Vec::new(),
        tables,
        handlers,
    };
    ::rules::verify(&ir)?;
    Ok(ir)
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
        let ir = compile(&def)?;
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
        let ir = compile(&def)?;
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
            _ => Phase::Done,
        };
        let slots = std::array::from_fn(|s| {
            let info = g(&info_name(s));
            let since = g(&tick_name(s)) as u32;
            match info % 4 {
                PLACED | SEEN => Slot::Placed {
                    count: (info % TARGET_UNIT / 4) as u16,
                    alive: g(&alive_name(s)) as u16,
                    seen: info % 4 == SEEN,
                    placed_at: since,
                    target: (info / TARGET_UNIT) as u8,
                },
                DRAINING => Slot::Draining { since },
                _ => Slot::Free,
            }
        });
        let mut players = BTreeMap::new();
        for slot in 0..16u8 {
            if self.vm.player_var(slot, "registered") == Some(1) {
                players.insert(
                    slot,
                    PlayerState {
                        lives: self.vm.player_var(slot, "lives").unwrap_or(0) as u16,
                        respawning: self.vm.player_var(slot, "respawning") == Some(1),
                        team: self.vm.player_var(slot, "team").unwrap_or(0) as u8,
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
            .collect();
        State {
            phase,
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
                if self.vm.set_player_var(slot, "lives", i32::from(pool)) {
                    Ok(())
                } else {
                    Err(format!("the script keeps no lives for player {slot}"))
                }
            }
        }
    }

    /// Actors the script counts as alive.
    pub fn live_actors(&self) -> u16 {
        self.state()
            .slots
            .iter()
            .map(|slot| match *slot {
                Slot::Placed { alive, .. } => alive,
                Slot::Free | Slot::Draining { .. } => 0,
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

impl Game for Host<'_> {
    fn players(&self) -> Vec<u8> {
        self.present.keys().copied().collect()
    }

    fn teams(&self) -> Vec<u8> {
        (0..self.teams).collect()
    }

    fn labelled(&self, label: &str) -> Vec<Placed> {
        if label != ZONE_LABEL {
            return Vec::new();
        }
        (0..self.seen.zones.len().min(usize::from(u8::MAX) + 1) as u32)
            .map(|zone| Placed {
                placement: zone,
                object: zone,
            })
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
            (id::SQUAD_ALIVE, [Arg::Squad(s)]) => self
                .seen
                .actors_alive_by_squad
                .get(usize::from(*s))
                .map_or(0, |&n| i32::from(n)),
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
        (id::HUNT, [Arg::Squad(slot)]) => Command::Hunt { slot: *slot },
        (id::ERASE_SQUAD, [Arg::Squad(slot)]) => Command::EraseSquad { slot: *slot },
        (id::FF_ROUND, [Arg::Int(index)]) => Command::Round {
            index: narrow_u8(*index)?,
        },
        (id::FF_WAVE, [Arg::Int(index)]) => Command::Wave {
            index: narrow_u8(*index)?,
        },
        (id::FF_SKULL, [Arg::Int(skull), Arg::Bool(on)]) => Command::Skull {
            id: *SKULLS.iter().find(|&&s| s as i32 == *skull)?,
            on: *on,
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

// --- The IR builder -------------------------------------------------------

#[derive(Default)]
struct Strings {
    list: Vec<String>,
    index: BTreeMap<String, u16>,
}

impl Strings {
    fn get(&mut self, text: &str) -> u16 {
        if let Some(&i) = self.index.get(text) {
            return i;
        }
        let i = self.list.len() as u16;
        self.list.push(text.to_string());
        self.index.insert(text.to_string(), i);
        i
    }
}

fn slot(
    slots: &mut Vec<IrSlot>,
    st: &mut Strings,
    name: &str,
    scope: Scope,
    ty: Ty,
    init: i32,
) -> u16 {
    slots.push(IrSlot {
        name: st.get(name),
        scope,
        ty,
        init,
    });
    (slots.len() - 1) as u16
}

fn table(st: &mut Strings, name: &str, columns: &[(&str, Ty)], cells: Vec<i32>) -> Table {
    Table {
        name: st.get(name),
        columns: columns
            .iter()
            .map(|&(name, ty)| Column {
                name: st.get(name),
                ty,
            })
            .collect(),
        cells,
    }
}

/// Slot numbers of the script's variables.
struct Vars {
    phase: u16,
    /// The phase's tick: Warmup and Intermission until, Cleanup since.
    phase_tick: u16,
    round: u16,
    wave: u16,
    next: u16,
    wave_actors: u16,
    /// Each team's shared pool.
    pool: [u16; MAX_TEAMS],
    /// Each team's points.
    score: [u16; MAX_TEAMS],
    /// Skulls on, one bit per [`SKULLS`] entry.
    skulls: u16,
    started: u16,
    /// Per squad slot: mode plus four times the placed count.
    info: [u16; SQUAD_SLOTS],
    alive: [u16; SQUAD_SLOTS],
    /// Per squad slot: placed at, or draining since.
    tick: [u16; SQUAD_SLOTS],
    registered: u16,
    lives: u16,
    respawning: u16,
    team: u16,
}

/// What every handler generator reads.
struct Gen {
    tys: Vec<Ty>,
    v: Vars,
    two_teams: bool,
    player_kill: i32,
    shared: bool,
    pool_start: i32,
    bonus: i32,
    rounds: i32,
    largest: i32,
    assist: i32,
    caps_rows: usize,
    skull_cols: u8,
}

/// One handler's code under construction: registers drawn from per-type
/// pools and released by scope, and forward labels patched at the end.
struct Code {
    tys: Vec<Ty>,
    regs: Vec<Ty>,
    free: BTreeMap<Ty, Vec<Reg>>,
    held: Vec<Reg>,
    ops: Vec<Op>,
    targets: Vec<Option<u16>>,
    patches: Vec<(usize, usize)>,
    end: usize,
    overflow: bool,
}

impl Code {
    fn new(g: &Gen, args: &[Ty]) -> Code {
        let mut code = Code {
            tys: g.tys.clone(),
            regs: args.to_vec(),
            free: BTreeMap::new(),
            held: Vec::new(),
            ops: Vec::new(),
            targets: Vec::new(),
            patches: Vec::new(),
            end: 0,
            overflow: false,
        };
        code.end = code.label();
        code
    }

    fn finish(mut self, event: Event) -> Result<Handler, ScriptRefusal> {
        let end = self.end;
        self.bind(end);
        if self.overflow {
            return Err(ScriptRefusal::Registers(event.name()));
        }
        for &(at, label) in &self.patches {
            let target = self.targets[label].expect("every label is bound");
            match &mut self.ops[at] {
                Op::Jump { to } | Op::JumpIf { to, .. } | Op::JumpIfNot { to, .. } => *to = target,
                _ => unreachable!("only jumps are patched"),
            }
        }
        Ok(Handler {
            event,
            timer: None,
            regs: self.regs,
            code: self.ops,
        })
    }

    /// A register of `ty` the caller writes before reading.
    fn tmp(&mut self, ty: Ty) -> Reg {
        let r = match self.free.get_mut(&ty).and_then(Vec::pop) {
            Some(r) => r,
            None => {
                if self.regs.len() >= MAX_REGS {
                    self.overflow = true;
                    return 0;
                }
                self.regs.push(ty);
                (self.regs.len() - 1) as Reg
            }
        };
        self.held.push(r);
        r
    }

    fn mark(&self) -> usize {
        self.held.len()
    }

    fn release(&mut self, mark: usize) {
        for r in self.held.drain(mark..) {
            self.free.entry(self.regs[r as usize]).or_default().push(r);
        }
    }

    fn emit(&mut self, op: Op) {
        self.ops.push(op);
    }

    fn kt(&mut self, ty: Ty, value: i32) -> Reg {
        let dst = self.tmp(ty);
        self.emit(Op::Const { dst, value });
        dst
    }

    fn k(&mut self, value: i32) -> Reg {
        self.kt(Ty::Int, value)
    }

    fn set(&mut self, dst: Reg, value: i32) {
        self.emit(Op::Const { dst, value });
    }

    fn mov(&mut self, dst: Reg, src: Reg) {
        self.emit(Op::Move { dst, src });
    }

    fn arith_to(&mut self, dst: Reg, op: ArithOp, a: Reg, b: Reg) {
        self.emit(Op::Arith {
            op,
            ty: NumTy::Int,
            dst,
            a,
            b,
        });
    }

    fn arith(&mut self, op: ArithOp, a: Reg, b: Reg) -> Reg {
        let dst = self.tmp(Ty::Int);
        self.arith_to(dst, op, a, b);
        dst
    }

    fn add_k(&mut self, a: Reg, value: i32) -> Reg {
        let b = self.k(value);
        self.arith(ArithOp::Add, a, b)
    }

    fn inc(&mut self, r: Reg) {
        let mark = self.mark();
        let one = self.k(1);
        self.arith_to(r, ArithOp::Add, r, one);
        self.release(mark);
    }

    fn cmp(&mut self, op: CmpOp, a: Reg, b: Reg) -> Reg {
        let dst = self.tmp(Ty::Bool);
        self.emit(Op::Cmp { op, dst, a, b });
        dst
    }

    fn cmp_k(&mut self, op: CmpOp, a: Reg, value: i32) -> Reg {
        let ty = self.regs[a as usize];
        let b = self.kt(ty, value);
        self.cmp(op, a, b)
    }

    fn not(&mut self, a: Reg) -> Reg {
        let dst = self.tmp(Ty::Bool);
        self.emit(Op::Not { dst, a });
        dst
    }

    fn or(&mut self, a: Reg, b: Reg) -> Reg {
        let dst = self.tmp(Ty::Bool);
        self.mov(dst, a);
        let done = self.label();
        self.jump_if(a, done);
        self.mov(dst, b);
        self.bind(done);
        dst
    }

    fn and(&mut self, a: Reg, b: Reg) -> Reg {
        let dst = self.tmp(Ty::Bool);
        self.mov(dst, a);
        let done = self.label();
        self.jump_if_not(a, done);
        self.mov(dst, b);
        self.bind(done);
        dst
    }

    fn load(&mut self, slot: u16) -> Reg {
        let dst = self.tmp(self.tys[slot as usize].register());
        self.emit(Op::Load {
            dst,
            slot,
            owner: None,
        });
        dst
    }

    fn load_of(&mut self, slot: u16, owner: Reg) -> Reg {
        let dst = self.tmp(self.tys[slot as usize].register());
        self.emit(Op::Load {
            dst,
            slot,
            owner: Some(owner),
        });
        dst
    }

    fn store(&mut self, slot: u16, src: Reg) {
        self.emit(Op::Store {
            slot,
            owner: None,
            src,
        });
    }

    fn store_of(&mut self, slot: u16, owner: Reg, src: Reg) {
        self.emit(Op::Store {
            slot,
            owner: Some(owner),
            src,
        });
    }

    fn store_k(&mut self, slot: u16, value: i32) {
        let mark = self.mark();
        let src = self.kt(self.tys[slot as usize].register(), value);
        self.store(slot, src);
        self.release(mark);
    }

    fn opt(&mut self, index: u8, ty: Ty) -> Reg {
        let dst = self.tmp(ty);
        self.emit(Op::Opt { dst, index });
        dst
    }

    fn cell_to(&mut self, dst: Reg, table: u8, row: Reg, col: u8) {
        self.emit(Op::TableRead {
            dst,
            table,
            row,
            col,
        });
    }

    fn cell(&mut self, table: u8, row: Reg, col: u8, ty: Ty) -> Reg {
        let dst = self.tmp(ty);
        self.cell_to(dst, table, row, col);
        dst
    }

    fn find(&mut self, table: u8, key: Reg) -> Reg {
        let dst = self.tmp(Ty::Int);
        self.emit(Op::TableFind { dst, table, key });
        dst
    }

    /// A call with a result: a read or a VM built-in.
    fn call(&mut self, entry: u16, args: Vec<Reg>, ty: Ty) -> Reg {
        let dst = self.tmp(ty);
        self.call_to(dst, entry, args);
        dst
    }

    fn call_to(&mut self, dst: Reg, entry: u16, args: Vec<Reg>) {
        self.emit(Op::Call {
            entry,
            args,
            dst: Some(dst),
        });
    }

    fn act(&mut self, entry: u16, args: Vec<Reg>) {
        self.emit(Op::Call {
            entry,
            args,
            dst: None,
        });
    }

    fn label(&mut self) -> usize {
        self.targets.push(None);
        self.targets.len() - 1
    }

    fn bind(&mut self, label: usize) {
        self.targets[label] = Some(self.ops.len() as u16);
    }

    fn jump_op(&mut self, op: Op, label: usize) {
        self.patches.push((self.ops.len(), label));
        self.emit(op);
    }

    fn jump(&mut self, label: usize) {
        self.jump_op(Op::Jump { to: 0 }, label);
    }

    fn jump_if(&mut self, cond: Reg, label: usize) {
        self.jump_op(Op::JumpIf { cond, to: 0 }, label);
    }

    fn jump_if_not(&mut self, cond: Reg, label: usize) {
        self.jump_op(Op::JumpIfNot { cond, to: 0 }, label);
    }

    /// Ends the handler. Only outside a `for each` body.
    fn exit_if(&mut self, cond: Reg) {
        let end = self.end;
        self.jump_if(cond, end);
    }

    fn exit(&mut self) {
        let end = self.end;
        self.jump(end);
    }

    fn when(&mut self, cond: Reg, then: impl FnOnce(&mut Code)) {
        let skip = self.label();
        self.jump_if_not(cond, skip);
        let mark = self.mark();
        then(self);
        self.release(mark);
        self.bind(skip);
    }

    fn when_else(&mut self, cond: Reg, then: impl FnOnce(&mut Code), other: impl FnOnce(&mut Code)) {
        let skip = self.label();
        let done = self.label();
        self.jump_if_not(cond, skip);
        let mark = self.mark();
        then(self);
        self.release(mark);
        self.jump(done);
        self.bind(skip);
        other(self);
        self.release(mark);
        self.bind(done);
    }

    fn each(&mut self, base: SetBase, body: impl FnOnce(&mut Code, Reg)) {
        let dst = self.tmp(base.elem());
        let at = self.ops.len();
        self.emit(Op::ForEach {
            set: Set { base, pick: None },
            dst,
            body: 0,
        });
        let mark = self.mark();
        body(self, dst);
        self.release(mark);
        let len = (self.ops.len() - at - 1) as u16;
        if let Op::ForEach { body, .. } = &mut self.ops[at] {
            *body = len;
        }
    }
}

fn exit_if_done(c: &mut Code, g: &Gen) {
    let phase = c.load(g.v.phase);
    let done = c.cmp_k(CmpOp::Eq, phase, DONE);
    c.exit_if(done);
}

fn phase_is(c: &mut Code, g: &Gen, value: i32) -> Reg {
    let phase = c.load(g.v.phase);
    c.cmp_k(CmpOp::Eq, phase, value)
}

/// Whether squad slot `s` holds a placed squad.
fn placed(c: &mut Code, g: &Gen, s: usize) -> Reg {
    let info = c.load(g.v.info[s]);
    let four = c.k(4);
    let mode = c.arith(ArithOp::Rem, info, four);
    let unseen = c.cmp_k(CmpOp::Eq, mode, PLACED);
    let seen = c.cmp_k(CmpOp::Eq, mode, SEEN);
    c.or(unseen, seen)
}

/// Actors the director counts as alive: a freed or draining slot holds 0.
fn live_sum(c: &mut Code, g: &Gen) -> Reg {
    let live = c.k(0);
    for s in 0..SQUAD_SLOTS {
        let mark = c.mark();
        let alive = c.load(g.v.alive[s]);
        c.arith_to(live, ArithOp::Add, live, alive);
        c.release(mark);
    }
    live
}

/// The waves-table row of the current round and wave.
fn wave_row(c: &mut Code, g: &Gen) -> Reg {
    let round = c.load(g.v.round);
    let wave = c.load(g.v.wave);
    let five = c.k(WAVES_PER_ROUND as i32);
    let base = c.arith(ArithOp::Mul, round, five);
    c.arith(ArithOp::Add, base, wave)
}

fn on_init(g: &Gen) -> Result<Handler, ScriptRefusal> {
    let mut c = Code::new(g, Event::Init.args());
    let yes = c.kt(Ty::Bool, 1);
    c.store(g.v.started, yes);
    let tick = c.call(id::NOW, vec![], Ty::Int);
    let zero = c.k(0);
    let gap = c.cell(T_WAVES, zero, W_GAP, Ty::Int);
    let until = c.arith(ArithOp::Add, tick, gap);
    c.store_k(g.v.phase, WARMUP);
    c.store(g.v.phase_tick, until);
    if g.shared {
        if g.two_teams {
            for t in 0..MAX_TEAMS {
                let mark = c.mark();
                let team = team_k(&mut c, t);
                let pool = c.load(g.v.pool[t]);
                c.act(id::FF_TEAM_LIVES, vec![team, pool]);
                c.release(mark);
            }
        } else {
            let none = c.kt(Ty::Player, -1);
            let pool = c.load(g.v.pool[0]);
            c.act(id::FF_LIVES, vec![none, pool]);
        }
    }
    let mask = c.k(0);
    for k in 0..SKULLS.len() {
        let on = c.opt(O_SKULL0 + k as u8, Ty::Bool);
        c.when(on, |c| {
            let bit = c.k(1 << k);
            c.arith_to(mask, ArithOp::Add, mask, bit);
        });
    }
    c.store(g.v.skulls, mask);
    c.finish(Event::Init)
}

fn on_join(g: &Gen) -> Result<Handler, ScriptRefusal> {
    let mut c = Code::new(g, Event::Join.args());
    let p: Reg = 0;
    exit_if_done(&mut c, g);
    let nobody = c.cmp_k(CmpOp::Eq, p, -1);
    c.exit_if(nobody);
    let known = c.load_of(g.v.registered, p);
    c.exit_if(known);
    let yes = c.kt(Ty::Bool, 1);
    c.store_of(g.v.registered, p, yes);
    // The Firefight team, fixed here: game team 0 is the first, every
    // other the second.
    if g.two_teams {
        let game_team = c.call(id::TEAM_OF, vec![p], Ty::Team);
        let first = c.cmp_k(CmpOp::Eq, game_team, 0);
        let side = c.k(1);
        c.when(first, |c| c.set(side, 0));
        c.store_of(g.v.team, p, side);
    }
    if !g.shared {
        let lives = c.k(g.pool_start);
        c.store_of(g.v.lives, p, lives);
        c.act(id::FF_LIVES, vec![p, lives]);
    }
    c.finish(Event::Join)
}

fn on_actor_kill(g: &Gen) -> Result<Handler, ScriptRefusal> {
    let mut c = Code::new(g, Event::ActorKill.args());
    let (killer, class): (Reg, Reg) = (0, 1);
    exit_if_done(&mut c, g);
    let nobody = c.cmp_k(CmpOp::Eq, killer, -1);
    c.exit_if(nobody);
    let row = c.find(T_POINTS, class);
    let unscored = c.cmp_k(CmpOp::Eq, row, -1);
    c.exit_if(unscored);
    let points = kill_points(&mut c, g, row);
    let kill = c.kt(Ty::Bool, 0);
    c.act(id::FF_SCORE, vec![killer, points, kill]);
    let team = c.load_of(g.v.team, killer);
    credit_team(&mut c, g, team, points);
    c.finish(Event::ActorKill)
}

fn on_actor_assist(g: &Gen) -> Result<Handler, ScriptRefusal> {
    let mut c = Code::new(g, Event::ActorAssist.args());
    let (killer, assist, class): (Reg, Reg, Reg) = (0, 1, 2);
    exit_if_done(&mut c, g);
    let nobody = c.cmp_k(CmpOp::Eq, killer, -1);
    c.exit_if(nobody);
    let no_assist = c.cmp_k(CmpOp::Eq, assist, -1);
    c.exit_if(no_assist);
    let same = c.cmp(CmpOp::Eq, assist, killer);
    c.exit_if(same);
    let row = c.find(T_POINTS, class);
    let unscored = c.cmp_k(CmpOp::Eq, row, -1);
    c.exit_if(unscored);
    let points = kill_points(&mut c, g, row);
    let share = c.k(g.assist);
    let product = c.arith(ArithOp::Mul, points, share);
    let scale = c.k(LIMB);
    let credit = c.arith(ArithOp::Div, product, scale);
    let some = c.cmp_k(CmpOp::Gt, credit, 0);
    c.when(some, |c| {
        let yes = c.kt(Ty::Bool, 1);
        c.act(id::FF_SCORE, vec![assist, credit, yes]);
        let team = c.load_of(g.v.team, assist);
        credit_team(c, g, team, credit);
    });
    c.finish(Event::ActorAssist)
}

/// Player kills across the teams: with two teams, a kill of a player on the
/// other team scores the fixed `player_kill`.
fn on_kill(g: &Gen) -> Result<Handler, ScriptRefusal> {
    let mut c = Code::new(g, Event::Kill.args());
    let (victim, killer): (Reg, Reg) = (0, 1);
    exit_if_done(&mut c, g);
    let nobody = c.cmp_k(CmpOp::Eq, killer, -1);
    c.exit_if(nobody);
    let suicide = c.cmp(CmpOp::Eq, killer, victim);
    c.exit_if(suicide);
    let ours = c.load_of(g.v.team, killer);
    let theirs = c.load_of(g.v.team, victim);
    let team_kill = c.cmp(CmpOp::Eq, ours, theirs);
    c.exit_if(team_kill);
    let points = c.k(g.player_kill);
    c.act(id::FF_PLAYER_SCORE, vec![killer, points]);
    credit_team(&mut c, g, ours, points);
    c.finish(Event::Kill)
}

/// A `team` register holding Firefight team `t`.
fn team_k(c: &mut Code, t: usize) -> Reg {
    c.kt(Ty::Team, t as i32)
}

/// Adds `value` to the global at `slot`.
fn add_to(c: &mut Code, slot: u16, value: Reg) {
    let mark = c.mark();
    let x = c.load(slot);
    c.arith_to(x, ArithOp::Add, x, value);
    c.store(slot, x);
    c.release(mark);
}

/// Adds `points` to the score of the Firefight team in `team` (0 or 1).
fn credit_team(c: &mut Code, g: &Gen, team: Reg, points: Reg) {
    let first = c.cmp_k(CmpOp::Eq, team, 0);
    c.when_else(
        first,
        |c| add_to(c, g.v.score[0], points),
        |c| add_to(c, g.v.score[1], points),
    );
}

/// `scoring::kill_points` for the points-table row: class points times the
/// difficulty multiplier and every skull's (1,000 for a skull that is
/// off), over 1,000^10, formed exactly in limbs and rounded down once.
fn kill_points(c: &mut Code, g: &Gen, row: Reg) -> Reg {
    let base = c.cell(T_POINTS, row, 1, Ty::Int);
    let thousand = c.k(LIMB);
    let limbs: [Reg; LIMBS] = std::array::from_fn(|_| c.k(0));
    c.arith_to(limbs[0], ArithOp::Rem, base, thousand);
    c.arith_to(limbs[1], ArithOp::Div, base, thousand);
    let difficulty = c.opt(O_DIFFICULTY, Ty::Int);
    let factor = c.cell(T_DIFFICULTY, difficulty, 0, Ty::Int);
    mul_limbs(c, &limbs, factor, thousand);
    let mask = c.load(g.v.skulls);
    for k in 0..SKULLS.len() {
        let mark = c.mark();
        let bit = c.k(1 << k);
        let shifted = c.arith(ArithOp::Div, mask, bit);
        let two = c.k(2);
        let low = c.arith(ArithOp::Rem, shifted, two);
        let on = c.cmp_k(CmpOp::Eq, low, 1);
        let factor = c.k(LIMB);
        c.when(on, |c| {
            let row = c.k(k as i32);
            c.cell_to(factor, T_SKULLS, row, S_MULT);
        });
        mul_limbs(c, &limbs, factor, thousand);
        c.release(mark);
    }
    let points = c.k(0);
    let million = c.k(LIMB * LIMB);
    let high = c.arith(ArithOp::Mul, limbs[12], million);
    let mid = c.arith(ArithOp::Mul, limbs[11], thousand);
    c.arith_to(points, ArithOp::Add, high, mid);
    c.arith_to(points, ArithOp::Add, points, limbs[10]);
    points
}

/// Multiplies the base-1,000 number in `limbs` by `factor` (at most 2,000).
fn mul_limbs(c: &mut Code, limbs: &[Reg; LIMBS], factor: Reg, thousand: Reg) {
    let mark = c.mark();
    let carry = c.k(0);
    let t = c.tmp(Ty::Int);
    for &limb in limbs {
        c.arith_to(t, ArithOp::Mul, limb, factor);
        c.arith_to(t, ArithOp::Add, t, carry);
        c.arith_to(limb, ArithOp::Rem, t, thousand);
        c.arith_to(carry, ArithOp::Div, t, thousand);
    }
    c.release(mark);
}

fn on_tick(g: &Gen) -> Result<Handler, ScriptRefusal> {
    let mut c = Code::new(g, Event::Tick.args());
    let v = &g.v;
    exit_if_done(&mut c, g);
    let tick = c.call(id::NOW, vec![], Ty::Int);

    // Present players; a living one is no longer respawning.
    let present = c.k(0);
    c.each(SetBase::Players, |c, p| {
        c.inc(present);
        let alive = c.call(id::ALIVE, vec![p], Ty::Bool);
        c.when(alive, |c| {
            let no = c.kt(Ty::Bool, 0);
            c.store_of(v.respawning, p, no);
        });
    });

    observe_slots(&mut c, g, tick);
    respawn(&mut c, g);
    out_of_lives(&mut c, g, present);

    // Cleanup: at the deadline, erase what is left; then the wave ends.
    let ending = c.kt(Ty::Bool, 0);
    let cleanup = phase_is(&mut c, g, CLEANUP);
    c.when(cleanup, |c| {
        let live = live_sum(c, g);
        let any = c.cmp_k(CmpOp::Gt, live, 0);
        c.when(any, |c| {
            let since = c.load(v.phase_tick);
            let deadline = c.add_k(since, CLEANUP_TICKS as i32);
            let early = c.cmp(CmpOp::Lt, tick, deadline);
            c.exit_if(early);
            for s in 0..SQUAD_SLOTS {
                let mark = c.mark();
                let held = placed(c, g, s);
                c.when(held, |c| {
                    let squad = c.kt(Ty::Squad, s as i32);
                    c.act(id::ERASE_SQUAD, vec![squad]);
                    c.store_k(v.info[s], DRAINING);
                    c.store(v.tick[s], tick);
                    c.store_k(v.alive[s], 0);
                });
                c.release(mark);
            }
        });
        c.set(ending, 1);
    });
    // Fighting with nobody left: the wave ends.
    let fighting = phase_is(&mut c, g, FIGHTING);
    c.when(fighting, |c| {
        let live = live_sum(c, g);
        let none = c.cmp_k(CmpOp::Eq, live, 0);
        c.when(none, |c| c.set(ending, 1));
    });
    c.when(ending, |c| end_wave(c, g, tick));

    // Warmup or intermission over: the next wave starts.
    let warmup = phase_is(&mut c, g, WARMUP);
    let between = phase_is(&mut c, g, INTERMISSION);
    let waiting = c.or(warmup, between);
    c.when(waiting, |c| {
        let until = c.load(v.phase_tick);
        let early = c.cmp(CmpOp::Lt, tick, until);
        c.exit_if(early);
        start_wave(c, g);
    });

    // Spawning: place from the reserve; fighting once it is empty.
    let spawning = phase_is(&mut c, g, SPAWNING);
    c.when(spawning, |c| {
        place(c, g, tick, present);
        let next = c.load(v.next);
        let row = wave_row(c, g);
        let squads = c.cell(T_WAVES, row, W_LEN, Ty::Int);
        let more = c.cmp(CmpOp::Lt, next, squads);
        c.exit_if(more);
        c.store_k(v.phase, FIGHTING);
    });

    // Fighting: the last few actors hunt the players.
    let fighting = phase_is(&mut c, g, FIGHTING);
    c.when(fighting, |c| {
        let live = live_sum(c, g);
        let any = c.cmp_k(CmpOp::Gt, live, 0);
        let few = c.cmp_k(CmpOp::Le, live, i32::from(CLEANUP_ACTORS));
        let wave_actors = c.load(v.wave_actors);
        let fewer = c.cmp(CmpOp::Lt, live, wave_actors);
        let a = c.and(any, few);
        let hunt = c.and(a, fewer);
        c.when(hunt, |c| {
            for s in 0..SQUAD_SLOTS {
                let mark = c.mark();
                let held = placed(c, g, s);
                c.when(held, |c| {
                    let squad = c.kt(Ty::Squad, s as i32);
                    c.act(id::HUNT, vec![squad]);
                });
                c.release(mark);
            }
            c.store_k(v.phase, CLEANUP);
            c.store(v.phase_tick, tick);
        });
    });
    c.finish(Event::Tick)
}

/// Which Firefight teams have a player present (`here`), and which have
/// one alive or with a respawn coming (`up`), by the players' fixed teams.
fn team_presence(c: &mut Code, g: &Gen) -> ([Reg; MAX_TEAMS], [Reg; MAX_TEAMS]) {
    let v = &g.v;
    let here = [c.kt(Ty::Bool, 0), c.kt(Ty::Bool, 0)];
    let up = [c.kt(Ty::Bool, 0), c.kt(Ty::Bool, 0)];
    c.each(SetBase::Players, |c, p| {
        let team = c.load_of(v.team, p);
        let alive = c.call(id::ALIVE, vec![p], Ty::Bool);
        let respawning = c.load_of(v.respawning, p);
        let either = c.or(alive, respawning);
        let first = c.cmp_k(CmpOp::Eq, team, 0);
        c.when_else(
            first,
            |c| {
                c.set(here[0], 1);
                c.when(either, |c| c.set(up[0], 1));
            },
            |c| {
                c.set(here[1], 1);
                c.when(either, |c| c.set(up[1], 1));
            },
        );
    });
    (here, up)
}

/// Out of lives: a team present with nobody alive and no
/// respawn coming is out. One team (or one of two present) out: lost. Both
/// present: the other team wins, and both out at once is decided.
fn out_of_lives(c: &mut Code, g: &Gen, present: Reg) {
    let v = &g.v;
    if !g.two_teams {
        let up = c.kt(Ty::Bool, 0);
        c.each(SetBase::Players, |c, p| {
            let alive = c.call(id::ALIVE, vec![p], Ty::Bool);
            let respawning = c.load_of(v.respawning, p);
            let either = c.or(alive, respawning);
            c.when(either, |c| c.set(up, 1));
        });
        let some = c.cmp_k(CmpOp::Gt, present, 0);
        let down = c.not(up);
        let lost = c.and(some, down);
        c.when(lost, |c| {
            let no = c.kt(Ty::Bool, 0);
            c.act(id::FF_FINISH, vec![no]);
            c.store_k(v.phase, DONE);
            c.exit();
        });
        return;
    }
    let (here, up) = team_presence(c, g);
    let out: [Reg; MAX_TEAMS] = std::array::from_fn(|t| {
        let down = c.not(up[t]);
        c.and(here[t], down)
    });
    let any = c.or(out[0], out[1]);
    let both_here = c.and(here[0], here[1]);
    c.when(any, |c| {
        c.when_else(
            both_here,
            |c| {
                let both_out = c.and(out[0], out[1]);
                let winner = c.kt(Ty::Team, 1);
                c.when_else(
                    both_out,
                    |c| {
                        let decided = decide(c, g);
                        c.mov(winner, decided);
                    },
                    |c| c.when(out[1], |c| c.set(winner, 0)),
                );
                c.act(id::FF_TEAM_FINISH, vec![winner]);
            },
            |c| {
                let no = c.kt(Ty::Bool, 0);
                c.act(id::FF_FINISH, vec![no]);
            },
        );
        c.store_k(v.phase, DONE);
        c.exit();
    });
}

/// The decided winner: the winning team by score, then lives left; `none`
/// for a draw.
fn decide(c: &mut Code, g: &Gen) -> Reg {
    let winner = c.kt(Ty::Team, -1);
    let mark = c.mark();
    let scores = [c.load(g.v.score[0]), c.load(g.v.score[1])];
    let lives = [lives_left(c, g, 0), lives_left(c, g, 1)];
    let ahead = c.cmp(CmpOp::Gt, scores[0], scores[1]);
    let behind = c.cmp(CmpOp::Lt, scores[0], scores[1]);
    let more = c.cmp(CmpOp::Gt, lives[0], lives[1]);
    let fewer = c.cmp(CmpOp::Lt, lives[0], lives[1]);
    c.when_else(
        ahead,
        |c| c.set(winner, 0),
        |c| {
            c.when_else(
                behind,
                |c| c.set(winner, 1),
                |c| {
                    c.when(more, |c| c.set(winner, 0));
                    c.when(fewer, |c| c.set(winner, 1));
                },
            );
        },
    );
    c.release(mark);
    winner
}

/// Lives left: team `t`'s pool, or its registered players'
/// own counts.
fn lives_left(c: &mut Code, g: &Gen, t: usize) -> Reg {
    let v = &g.v;
    if g.shared {
        return c.load(v.pool[t]);
    }
    let sum = c.k(0);
    for slot in 0..16 {
        let mark = c.mark();
        let p = c.kt(Ty::Player, slot);
        let known = c.load_of(v.registered, p);
        c.when(known, |c| {
            let team = c.load_of(v.team, p);
            let ours = c.cmp_k(CmpOp::Eq, team, t as i32);
            c.when(ours, |c| {
                let lives = c.load_of(v.lives, p);
                c.arith_to(sum, ArithOp::Add, sum, lives);
            });
        });
        c.release(mark);
    }
    sum
}

/// The slot pass, unrolled over the 16 slots.
fn observe_slots(c: &mut Code, g: &Gen, tick: Reg) {
    let v = &g.v;
    for s in 0..SQUAD_SLOTS {
        let mark = c.mark();
        let squad = c.kt(Ty::Squad, s as i32);
        let observed = c.call(id::SQUAD_ALIVE, vec![squad], Ty::Int);
        let info = c.load(v.info[s]);
        let four = c.k(4);
        let mode = c.arith(ArithOp::Rem, info, four);
        // The placed count and the target, kept as they are.
        let count4 = c.arith(ArithOp::Sub, info, mode);
        let unseen = c.cmp_k(CmpOp::Eq, mode, PLACED);
        let was_seen = c.cmp_k(CmpOp::Eq, mode, SEEN);
        let held = c.or(unseen, was_seen);
        c.when(held, |c| {
            let alive = c.load(v.alive[s]);
            let obs = c.tmp(Ty::Int);
            c.mov(obs, alive);
            let lower = c.cmp(CmpOp::Lt, observed, alive);
            c.when(lower, |c| c.mov(obs, observed));
            let seen = c.cmp_k(CmpOp::Eq, mode, SEEN);
            let reported = c.cmp_k(CmpOp::Gt, obs, 0);
            c.when(reported, |c| c.set(seen, 1));
            c.when_else(
                seen,
                |c| c.mov(alive, obs),
                |c| {
                    let placed_at = c.load(v.tick[s]);
                    let deadline = c.add_k(placed_at, SLOT_TIMEOUT_TICKS as i32);
                    let late = c.cmp(CmpOp::Ge, tick, deadline);
                    c.when(late, |c| c.set(alive, 0));
                },
            );
            c.store(v.alive[s], alive);
            let gone = c.cmp_k(CmpOp::Eq, alive, 0);
            let next = c.tmp(Ty::Int);
            c.when_else(
                gone,
                |c| c.set(next, FREE),
                |c| {
                    c.when_else(
                        seen,
                        |c| {
                            let m = c.k(SEEN);
                            c.arith_to(next, ArithOp::Add, count4, m);
                        },
                        |c| {
                            let m = c.k(PLACED);
                            c.arith_to(next, ArithOp::Add, count4, m);
                        },
                    );
                },
            );
            c.store(v.info[s], next);
        });
        let draining = c.cmp_k(CmpOp::Eq, mode, DRAINING);
        c.when(draining, |c| {
            let empty = c.cmp_k(CmpOp::Eq, observed, 0);
            let since = c.load(v.tick[s]);
            let deadline = c.add_k(since, SLOT_TIMEOUT_TICKS as i32);
            let late = c.cmp(CmpOp::Ge, tick, deadline);
            let free = c.or(empty, late);
            c.when(free, |c| c.store_k(v.info[s], FREE));
        });
        c.release(mark);
    }
}

/// Respawn: a life for each dead player not yet respawning,
/// from the team's pool or the player's own count; with none left, a free
/// respawn only between waves.
fn respawn(c: &mut Code, g: &Gen) {
    let v = &g.v;
    let warmup = phase_is(c, g, WARMUP);
    let intermission = phase_is(c, g, INTERMISSION);
    let between = c.or(warmup, intermission);
    c.each(SetBase::Players, |c, p| {
        let alive = c.call(id::ALIVE, vec![p], Ty::Bool);
        let respawning = c.load_of(v.respawning, p);
        let up = c.or(alive, respawning);
        let down = c.not(up);
        c.when(down, |c| {
            if !g.shared {
                spend(c, g, p, between, None);
            } else if g.two_teams {
                let team = c.load_of(v.team, p);
                let first = c.cmp_k(CmpOp::Eq, team, 0);
                c.when_else(
                    first,
                    |c| spend(c, g, p, between, Some(0)),
                    |c| spend(c, g, p, between, Some(1)),
                );
            } else {
                spend(c, g, p, between, Some(0));
            }
        });
    });
}

/// One dead player's respawn: from team `pool`'s shared pool, or (none)
/// the player's own lives.
fn spend(c: &mut Code, g: &Gen, p: Reg, between: Reg, pool: Option<usize>) {
    let v = &g.v;
    let lives = match pool {
        Some(t) => c.load(v.pool[t]),
        None => c.load_of(v.lives, p),
    };
    let left = c.cmp_k(CmpOp::Gt, lives, 0);
    c.when_else(
        left,
        |c| {
            let one = c.k(1);
            c.arith_to(lives, ArithOp::Sub, lives, one);
            match pool {
                Some(t) => c.store(v.pool[t], lives),
                None => c.store_of(v.lives, p, lives),
            }
            let yes = c.kt(Ty::Bool, 1);
            c.store_of(v.respawning, p, yes);
            c.act(id::FF_RESPAWN, vec![p]);
            match pool {
                Some(t) if g.two_teams => {
                    let team = team_k(c, t);
                    c.act(id::FF_TEAM_LIVES, vec![team, lives]);
                }
                Some(_) => {
                    let none = c.kt(Ty::Player, -1);
                    c.act(id::FF_LIVES, vec![none, lives]);
                }
                None => c.act(id::FF_LIVES, vec![p, lives]),
            }
        },
        |c| {
            c.when(between, |c| {
                let yes = c.kt(Ty::Bool, 1);
                c.store_of(v.respawning, p, yes);
                c.act(id::FF_RESPAWN, vec![p]);
            });
        },
    );
}

/// The end of a wave: respawn the dead, then the next wave, the next
/// round with its lives bonus, or the end.
fn end_wave(c: &mut Code, g: &Gen, tick: Reg) {
    let v = &g.v;
    c.each(SetBase::Players, |c, p| {
        let alive = c.call(id::ALIVE, vec![p], Ty::Bool);
        let respawning = c.load_of(v.respawning, p);
        let up = c.or(alive, respawning);
        let down = c.not(up);
        c.when(down, |c| {
            let yes = c.kt(Ty::Bool, 1);
            c.store_of(v.respawning, p, yes);
            c.act(id::FF_RESPAWN, vec![p]);
        });
    });
    let won = c.label();
    let wave = c.load(v.wave);
    let next_wave = c.add_k(wave, 1);
    let more_waves = c.cmp_k(CmpOp::Lt, next_wave, WAVES_PER_ROUND as i32);
    c.when_else(
        more_waves,
        |c| c.store(v.wave, next_wave),
        |c| {
            let round = c.load(v.round);
            let next_round = c.add_k(round, 1);
            let more_rounds = c.cmp_k(CmpOp::Lt, next_round, g.rounds);
            c.when_else(
                more_rounds,
                |c| {
                    c.store(v.round, next_round);
                    c.store_k(v.wave, 0);
                    if g.shared {
                        let teams = if g.two_teams { MAX_TEAMS } else { 1 };
                        for t in 0..teams {
                            let mark = c.mark();
                            let pool = c.load(v.pool[t]);
                            let bonus = c.k(g.bonus);
                            c.arith_to(pool, ArithOp::Add, pool, bonus);
                            c.store(v.pool[t], pool);
                            if g.two_teams {
                                let team = team_k(c, t);
                                c.act(id::FF_TEAM_LIVES, vec![team, pool]);
                            } else {
                                let none = c.kt(Ty::Player, -1);
                                c.act(id::FF_LIVES, vec![none, pool]);
                            }
                            c.release(mark);
                        }
                    } else {
                        for slot in 0..16 {
                            let mark = c.mark();
                            let p = c.kt(Ty::Player, slot);
                            let known = c.load_of(v.registered, p);
                            c.when(known, |c| {
                                let lives = c.load_of(v.lives, p);
                                let bonus = c.k(g.bonus);
                                c.arith_to(lives, ArithOp::Add, lives, bonus);
                                c.store_of(v.lives, p, lives);
                                c.act(id::FF_LIVES, vec![p, lives]);
                            });
                            c.release(mark);
                        }
                    }
                },
                |c| {
                    if g.two_teams {
                        let winner = last_round_winner(c, g);
                        c.act(id::FF_TEAM_FINISH, vec![winner]);
                    } else {
                        let yes = c.kt(Ty::Bool, 1);
                        c.act(id::FF_FINISH, vec![yes]);
                    }
                    c.store_k(v.phase, DONE);
                    c.jump(won);
                },
            );
        },
    );
    let row = wave_row(c, g);
    let gap = c.cell(T_WAVES, row, W_GAP, Ty::Int);
    let until = c.arith(ArithOp::Add, tick, gap);
    c.store_k(v.phase, INTERMISSION);
    c.store(v.phase_tick, until);
    c.bind(won);
}

/// The last round's outcome with two teams: the only team present,
/// else decided by score and lives (`none` for a draw).
fn last_round_winner(c: &mut Code, g: &Gen) -> Reg {
    let (here, _) = team_presence(c, g);
    let winner = c.kt(Ty::Team, -1);
    let away = [c.not(here[0]), c.not(here[1])];
    let only = [c.and(here[0], away[1]), c.and(away[0], here[1])];
    c.when_else(
        only[0],
        |c| c.set(winner, 0),
        |c| {
            c.when_else(
                only[1],
                |c| c.set(winner, 1),
                |c| {
                    let decided = decide(c, g);
                    c.mov(winner, decided);
                },
            );
        },
    );
    winner
}

/// The start of a wave: a round's first wave announces the round and
/// switches its skulls on, in schedule order.
fn start_wave(c: &mut Code, g: &Gen) {
    let v = &g.v;
    let wave = c.load(v.wave);
    let first = c.cmp_k(CmpOp::Eq, wave, 0);
    c.when(first, |c| {
        let round = c.load(v.round);
        c.act(id::FF_ROUND, vec![round]);
        let mask = c.load(v.skulls);
        for k in 0..g.skull_cols {
            let mark = c.mark();
            let index = c.cell(T_ROUNDS, round, k, Ty::Int);
            let scheduled = c.cmp_k(CmpOp::Ge, index, 0);
            c.when(scheduled, |c| {
                let bit = c.cell(T_SKULLS, index, S_BIT, Ty::Int);
                let shifted = c.arith(ArithOp::Div, mask, bit);
                let two = c.k(2);
                let low = c.arith(ArithOp::Rem, shifted, two);
                let off = c.cmp_k(CmpOp::Eq, low, 0);
                c.when(off, |c| {
                    c.arith_to(mask, ArithOp::Add, mask, bit);
                    let skull = c.cell(T_SKULLS, index, S_ID, Ty::Int);
                    let on = c.kt(Ty::Bool, 1);
                    c.act(id::FF_SKULL, vec![skull, on]);
                });
            });
            c.release(mark);
        }
        c.store(v.skulls, mask);
    });
    c.act(id::FF_WAVE, vec![wave]);
    c.store_k(v.next, 0);
    c.store_k(v.wave_actors, 0);
    c.store_k(v.phase, SPAWNING);
}

/// The target team: the only team present, else the one fewer
/// live actors are sent at, else even squads at team 0. An int, 0 or 1.
fn choose_target(c: &mut Code, g: &Gen, here: &[Reg; MAX_TEAMS], next: Reg) -> Reg {
    let v = &g.v;
    let target = c.k(0);
    let away = [c.not(here[0]), c.not(here[1])];
    let only = [c.and(here[0], away[1]), c.and(away[0], here[1])];
    c.when_else(
        only[0],
        |c| c.set(target, 0),
        |c| {
            c.when_else(
                only[1],
                |c| c.set(target, 1),
                |c| {
                    let load = [c.k(0), c.k(0)];
                    for s in 0..SQUAD_SLOTS {
                        let mark = c.mark();
                        let held = placed(c, g, s);
                        c.when(held, |c| {
                            let info = c.load(v.info[s]);
                            let unit = c.k(TARGET_UNIT);
                            let at = c.arith(ArithOp::Div, info, unit);
                            let alive = c.load(v.alive[s]);
                            let first = c.cmp_k(CmpOp::Eq, at, 0);
                            c.when_else(
                                first,
                                |c| c.arith_to(load[0], ArithOp::Add, load[0], alive),
                                |c| c.arith_to(load[1], ArithOp::Add, load[1], alive),
                            );
                        });
                        c.release(mark);
                    }
                    let less = c.cmp(CmpOp::Lt, load[0], load[1]);
                    let more = c.cmp(CmpOp::Gt, load[0], load[1]);
                    c.when_else(
                        less,
                        |c| c.set(target, 0),
                        |c| {
                            c.when_else(
                                more,
                                |c| c.set(target, 1),
                                |c| {
                                    let two = c.k(2);
                                    c.arith_to(target, ArithOp::Rem, next, two);
                                },
                            );
                        },
                    );
                },
            );
        },
    );
    target
}

/// Placement: up to two squads from the reserve, in definition
/// order, while the next fits under the cap and a slot and a zone are free.
fn place(c: &mut Code, g: &Gen, tick: Reg, present: Reg) {
    let v = &g.v;
    let one = c.k(1);
    let largest = c.k(g.largest);
    let players = c.call(id::CLAMP, vec![present, one, largest], Ty::Int);
    let cap = c.k(0);
    for row in (0..g.caps_rows).rev() {
        let mark = c.mark();
        let row = c.k(row as i32);
        let upper = c.cell(T_CAPS, row, 0, Ty::Int);
        let within = c.cmp(CmpOp::Le, players, upper);
        c.when(within, |c| c.cell_to(cap, T_CAPS, row, 1));
        c.release(mark);
    }
    let here = g.two_teams.then(|| team_presence(c, g).0);
    let live = live_sum(c, g);
    let wave = wave_row(c, g);
    let first = c.cell(T_WAVES, wave, W_FIRST, Ty::Int);
    let squads = c.cell(T_WAVES, wave, W_LEN, Ty::Int);
    let stop = c.label();
    for _ in 0..PLACE_PER_TICK {
        let mark = c.mark();
        let next = c.load(v.next);
        let more = c.cmp(CmpOp::Lt, next, squads);
        c.jump_if_not(more, stop);
        let row = c.arith(ArithOp::Add, first, next);
        let per_player = c.cell(T_SQUADS, row, Q_PER_PLAYER, Ty::Int);
        let min = c.cell(T_SQUADS, row, Q_MIN, Ty::Int);
        let max = c.cell(T_SQUADS, row, Q_MAX, Ty::Int);
        let scaled = c.arith(ArithOp::Mul, per_player, players);
        let count = c.call(id::CLAMP, vec![scaled, min, max], Ty::Int);
        let total = c.arith(ArithOp::Add, live, count);
        let over = c.cmp(CmpOp::Gt, total, cap);
        c.jump_if(over, stop);

        let free = c.k(-1);
        for s in (0..SQUAD_SLOTS).rev() {
            let inner = c.mark();
            let info = c.load(v.info[s]);
            let four = c.k(4);
            let mode = c.arith(ArithOp::Rem, info, four);
            let empty = c.cmp_k(CmpOp::Eq, mode, FREE);
            c.when(empty, |c| c.set(free, s as i32));
            c.release(inner);
        }
        let full = c.cmp_k(CmpOp::Eq, free, -1);
        c.jump_if(full, stop);

        let target = here.map(|here| choose_target(c, g, &here, next));
        let prefer = c.cell(T_SQUADS, row, Q_PREFER, Ty::Int);
        let zone = choose_zone(c, prefer, target);
        let nowhere = c.cmp_k(CmpOp::Eq, zone, -1);
        c.jump_if(nowhere, stop);

        let four = c.k(4);
        let count4 = c.arith(ArithOp::Mul, count, four);
        let info = c.add_k(count4, PLACED);
        if let Some(target) = target {
            let unit = c.k(TARGET_UNIT);
            let at = c.arith(ArithOp::Mul, target, unit);
            c.arith_to(info, ArithOp::Add, info, at);
        }
        for s in 0..SQUAD_SLOTS {
            let inner = c.mark();
            let chosen = c.cmp_k(CmpOp::Eq, free, s as i32);
            c.when(chosen, |c| {
                c.store(v.info[s], info);
                c.store(v.alive[s], count);
                c.store(v.tick[s], tick);
            });
            c.release(inner);
        }
        let squad = c.call(id::SQUAD, vec![free], Ty::Squad);
        let class = c.cell(T_SQUADS, row, Q_CLASS, Ty::Class);
        let tier = c.cell(T_SQUADS, row, Q_TIER, Ty::Int);
        c.act(id::PLACE_SQUAD_AT, vec![squad, class, count, zone, tier]);
        if let Some(target) = target {
            let team = team_of_int(c, target);
            c.act(id::FF_TARGET, vec![squad, team]);
        }
        let after = c.add_k(next, 1);
        c.store(v.next, after);
        let placed = c.load(v.wave_actors);
        c.arith_to(placed, ArithOp::Add, placed, count);
        c.store(v.wave_actors, placed);
        c.arith_to(live, ArithOp::Add, live, count);
        c.release(mark);
    }
    c.bind(stop);
}

/// A `team` register for the Firefight team in the int register `t` (0
/// or 1).
fn team_of_int(c: &mut Code, t: Reg) -> Reg {
    let team = c.kt(Ty::Team, 0);
    let second = c.cmp_k(CmpOp::Eq, t, 1);
    c.when(second, |c| c.set(team, 1));
    team
}

/// `director::choose_zone` over the labelled drop zones, in kit order: an
/// unseen preferred zone, else a seeded pick among the farther half of the
/// unseen zones (of all zones when every one is in sight). With a target
/// team (an int register, 0 or 1), the candidates narrow to the zones at
/// least as near that team as the other, when any is, and rank by the
/// target team's distance. -1 for none.
fn choose_zone(c: &mut Code, prefer: Reg, target: Option<Reg>) -> Reg {
    let zones = SetBase::Labelled(ZONES);
    let zone = c.k(-1);
    let done = c.label();
    let all = c.k(0);
    let unseen = c.k(0);
    let preferred = c.kt(Ty::Bool, 0);
    c.each(zones, |c, z| {
        let seen = c.call(id::ZONE_SEEN, vec![z], Ty::Bool);
        let hidden = c.not(seen);
        c.when(hidden, |c| {
            c.inc(unseen);
            let here = c.cmp(CmpOp::Eq, all, prefer);
            c.when(here, |c| c.set(preferred, 1));
        });
        c.inc(all);
    });
    c.when(preferred, |c| {
        c.mov(zone, prefer);
        c.jump(done);
    });
    let use_all = c.cmp_k(CmpOp::Eq, unseen, 0);
    let n = c.tmp(Ty::Int);
    c.mov(n, unseen);
    c.when(use_all, |c| c.mov(n, all));

    // With a target: the target team's side, and whether to keep to it.
    let side = target.map(|t| {
        let ours = team_of_int(c, t);
        let other = c.k(1);
        c.arith_to(other, ArithOp::Sub, other, t);
        let theirs = team_of_int(c, other);
        let near = c.k(0);
        c.each(zones, |c, z| {
            let seen = c.call(id::ZONE_SEEN, vec![z], Ty::Bool);
            let hidden = c.not(seen);
            let candidate = c.or(use_all, hidden);
            c.when(candidate, |c| {
                let mine = c.call(id::ZONE_TEAM_DISTANCE, vec![z, ours], Ty::Int);
                let yours = c.call(id::ZONE_TEAM_DISTANCE, vec![z, theirs], Ty::Int);
                let nearer = c.cmp(CmpOp::Le, mine, yours);
                c.when(nearer, |c| c.inc(near));
            });
        });
        let use_near = c.cmp_k(CmpOp::Gt, near, 0);
        c.when(use_near, |c| c.mov(n, near));
        (ours, theirs, use_near)
    });

    let empty = c.cmp_k(CmpOp::Eq, n, 0);
    c.jump_if(empty, done);
    let n1 = c.add_k(n, 1);
    let two = c.k(2);
    let farther = c.arith(ArithOp::Div, n1, two);
    let pick = c.k(0);
    let draw = c.cmp_k(CmpOp::Gt, farther, 1);
    c.when(draw, |c| c.call_to(pick, id::RANDOM, vec![farther]));

    // Whether a zone is a candidate, and its rank key.
    let candidate_of = |c: &mut Code, z: Reg| -> Reg {
        let seen = c.call(id::ZONE_SEEN, vec![z], Ty::Bool);
        let hidden = c.not(seen);
        let candidate = c.or(use_all, hidden);
        match side {
            None => candidate,
            Some((ours, theirs, use_near)) => {
                let mine = c.call(id::ZONE_TEAM_DISTANCE, vec![z, ours], Ty::Int);
                let yours = c.call(id::ZONE_TEAM_DISTANCE, vec![z, theirs], Ty::Int);
                let nearer = c.cmp(CmpOp::Le, mine, yours);
                let anywhere = c.not(use_near);
                let allowed = c.or(anywhere, nearer);
                c.and(candidate, allowed)
            }
        }
    };
    let distance_of = |c: &mut Code, z: Reg| -> Reg {
        match side {
            None => c.call(id::ZONE_DISTANCE, vec![z], Ty::Int),
            Some((ours, _, _)) => c.call(id::ZONE_TEAM_DISTANCE, vec![z, ours], Ty::Int),
        }
    };
    let i = c.k(0);
    c.each(zones, |c, z| {
        let candidate = candidate_of(c, z);
        c.when(candidate, |c| {
            let dz = distance_of(c, z);
            let rank = c.k(0);
            let j = c.k(0);
            c.each(zones, |c, w| {
                let other = candidate_of(c, w);
                c.when(other, |c| {
                    let dw = distance_of(c, w);
                    let farther = c.cmp(CmpOp::Gt, dw, dz);
                    c.when_else(
                        farther,
                        |c| c.inc(rank),
                        |c| {
                            let tie = c.cmp(CmpOp::Eq, dw, dz);
                            let before = c.cmp(CmpOp::Lt, j, i);
                            let earlier = c.and(tie, before);
                            c.when(earlier, |c| c.inc(rank));
                        },
                    );
                });
                c.inc(j);
            });
            let hit = c.cmp(CmpOp::Eq, rank, pick);
            c.when(hit, |c| c.mov(zone, i));
        });
        c.inc(i);
    });
    c.bind(done);
    zone
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin;

    #[test]
    fn every_builtin_compiles_verifies_and_hashes_stably() {
        let mut ids = BTreeSet::new();
        for def in builtin::definitions() {
            let ir = compile(&def).unwrap();
            assert_eq!(compile(&def).unwrap().encode(), ir.encode());
            assert_eq!(Ir::decode(&ir.encode()).unwrap(), ir);
            assert!(::rules::verify::unhostable(&ir).is_none());
            assert!(ir.encode().len() <= ::rules::ir::MAX_IR_BYTES);
            ids.insert(ir.id());
        }
        assert_eq!(ids.len(), 4);
    }

    #[test]
    fn the_drop_zones_are_a_set_of_objects_a_map_need_not_hold() {
        let ir = compile(&builtin::definitions()[0]).unwrap();
        let zones = &ir.labels[ZONES as usize];
        assert_eq!(*zones, ::rules::ir::LabelDef::objects(ZONE_LABEL));
        assert_eq!(zones.kind, ::rules::ir::LabelKind::Object);
        assert_eq!(zones.max as usize, ::rules::ty::MAX_LABELLED);
        let empty = ::rules::labels::Census::default();
        assert_eq!(::rules::labels::check(&ir, &empty, &[0]), vec![]);
    }

    #[test]
    fn options_carry_the_difficulty_and_the_starting_skulls() {
        let ir = compile(&builtin::definitions()[0]).unwrap();
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
        assert_eq!(compile(&def), Err(ScriptRefusal::Squads { rows: 270 }));
    }
}
