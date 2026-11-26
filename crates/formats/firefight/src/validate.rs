use std::collections::BTreeSet;

use crate::{
    ALL_ZONE_ROLES, Bonus, Caps, Def, Drop, FORMAT, MAX_BONUS_TICKS, MAX_CLASSES, MAX_DOC_BYTES, MAX_PLAYERS,
    MAX_ROUNDS, MAX_SET_SCALE, MAX_SET_SKULLS, MAX_SKULLS, MAX_SQUADS, MAX_TEAMS, MULTIPLIER_SCALE, Refusal,
    Result, SET_ROUNDS, Scoring, Skull, WAVES_PER_ROUND, Wave, wire,
};

/// The provisional host budget in the AI networking design, by inclusive
/// upper bound on player count.
pub const HOST_ACTOR_CAPS: [(u8, u16); 3] = [(4, 32), (8, 24), (16, 16)];

const MAX_NAME_BYTES: usize = 128;
const MAX_CLASS_BYTES: usize = 32;
const MAX_ACTORS: u16 = HOST_ACTOR_CAPS[0].1;
const MAX_LIVES: u16 = 1_000;
const MAX_GAP_TICKS: u32 = 5 * 60 * 30;
const MAX_DROP_ZONES: u8 = 12;
const MAX_WEAPON_TIER: u8 = 3;
const MAX_CLASS_POINTS: u16 = 1_000;
const MAX_PLAYER_KILL_POINTS: u16 = 1_000;
const MAX_BONUS_POINTS: u32 = 1_000_000;
const MAX_MULTIPLIER: u16 = 2 * MULTIPLIER_SCALE;

/// Classes provided by the target kit. An empty kit admits no definitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    pub classes: BTreeSet<String>,
    /// Classes the kit's build dropped to fit the map's tag headroom: a
    /// definition naming one is refused as cut, not as unknown.
    pub cut: BTreeSet<String>,
    /// The roles ([`crate::ZoneRole::bit`]) at least one zone of the kit
    /// carries. A squad's `Drop::Role` needs its role here.
    pub zone_roles: u8,
}

impl Limits {
    pub fn new(classes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            classes: classes.into_iter().map(Into::into).collect(),
            cut: BTreeSet::new(),
            zone_roles: ALL_ZONE_ROLES,
        }
    }

    /// The same limits for a kit that cut `classes` to fit.
    pub fn with_cut(mut self, classes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.cut = classes.into_iter().map(Into::into).collect();
        self
    }

    /// The same limits for a kit whose zones carry only `roles`
    /// (a mask of [`crate::ZoneRole::bit`]s).
    pub fn with_zone_roles(mut self, roles: u8) -> Self {
        self.zone_roles = roles;
        self
    }
}

/// Check the format, the worst wave at each player count, and kit admission.
pub fn validate(def: &Def, limits: &Limits) -> Result<()> {
    definition(def)?;
    // Every squad's class and every scored class must be in the kit: one
    // the build cut is named as cut, one it never had as unknown.
    let squad_classes = def
        .rounds
        .iter()
        .flat_map(|round| &round.waves)
        .flat_map(|wave| &wave.squads)
        .map(|squad| &squad.class);
    for class in def.scoring.by_class.keys().chain(squad_classes) {
        if limits.cut.contains(class) {
            return Err(Refusal::CutClass(class.clone()));
        }
        if !limits.classes.contains(class) {
            return Err(Refusal::UnknownClass(class.clone()));
        }
    }
    // A role squad no zone can host would never be placed: refuse it here.
    for squad in def
        .rounds
        .iter()
        .flat_map(|round| &round.waves)
        .flat_map(|wave| &wave.squads)
    {
        if let Drop::Role(role) = squad.drop
            && limits.zone_roles & role.bit() == 0
        {
            return Err(invalid(
                "drop",
                "a squad's zone role is carried by no zone of the kit",
            ));
        }
    }
    wire::measure(def, MAX_DOC_BYTES)
}

pub(crate) fn definition(def: &Def) -> Result<()> {
    if def.format != FORMAT {
        return Err(Refusal::Format(def.format));
    }
    wire::check_size(def.name.len(), "name bytes", MAX_NAME_BYTES)?;
    if def.name.trim().is_empty() || def.name.chars().any(char::is_control) {
        return Err(invalid(
            "name",
            "must contain text without control characters",
        ));
    }
    teams(def)?;
    wire::check_size(def.rounds.len(), "rounds", MAX_ROUNDS)?;
    if def.rounds.is_empty() {
        return Err(invalid("rounds", "at least one round is required"));
    }
    if def.lives.pool_start > MAX_LIVES || def.lives.per_round_bonus > MAX_LIVES {
        return Err(invalid(
            "lives",
            "starting lives and the round bonus must be at most 1,000 each",
        ));
    }

    caps(&def.caps)?;
    scoring(&def.scoring)?;
    if let Some(bonus) = &def.bonus {
        self::bonus(bonus, def)?;
    }
    let mut enabled = BTreeSet::new();
    for (round_index, round) in def.rounds.iter().enumerate() {
        wire::check_size(round.skulls_on.len(), "skulls_on", MAX_SKULLS)?;
        for &skull in &round.skulls_on {
            if skull == Skull::Iron {
                return Err(invalid("skulls_on", "Iron is only on in the bonus round"));
            }
            if !enabled.insert(skull) {
                return Err(invalid("skulls_on", "each skull may be enabled once"));
            }
        }
        if round.waves.len() != WAVES_PER_ROUND {
            return Err(invalid("waves", "each round requires five waves"));
        }
        for (wave_index, wave) in round.waves.iter().enumerate() {
            if wave.boss != (wave_index + 1 == WAVES_PER_ROUND) {
                return Err(invalid(
                    "boss",
                    "only the last wave of each round is a boss wave",
                ));
            }
            let actors_by_player = wave_actors(wave, &def.scoring)?;
            for (index, actors) in actors_by_player.into_iter().enumerate() {
                let players = (index + 1) as u8;
                let Some(cap) = def.caps.for_players(players) else {
                    break;
                };
                if actors > u32::from(cap) {
                    return Err(Refusal::WaveOverCap {
                        round: round_index + 1,
                        wave: wave_index + 1,
                        players,
                        actors,
                        cap,
                    });
                }
            }
        }
    }
    sets(def, &enabled)
}

/// Endless play and the set skulls: an endless definition has one team
/// and a set of [`SET_ROUNDS`] rounds; a set skull is listed once, is not a
/// round skull (a new set would switch it off again), and only an endless
/// definition has any.
fn sets(def: &Def, round_skulls: &BTreeSet<crate::Skull>) -> Result<()> {
    let set = &def.set;
    wire::check_size(set.skulls.len(), "set skulls", MAX_SET_SKULLS)?;
    if set.scale > MAX_SET_SCALE {
        return Err(invalid("scale", "must be in 0..=500 thousandths per set"));
    }
    if !set.endless {
        if set.scale != 0 || !set.skulls.is_empty() {
            return Err(invalid(
                "set",
                "a scale and set skulls need an endless definition",
            ));
        }
        return Ok(());
    }
    if def.teams != 1 {
        return Err(invalid("set", "an endless definition has one team"));
    }
    if def.rounds.len() != SET_ROUNDS {
        return Err(invalid("rounds", "a set is three rounds"));
    }
    let mut arriving = BTreeSet::new();
    for &skull in &set.skulls {
        if round_skulls.contains(&skull) || !arriving.insert(skull) {
            return Err(invalid(
                "set skulls",
                "a set skull is listed once and is not a round skull",
            ));
        }
    }
    Ok(())
}

/// One or two player teams; points for a player kill only with two, and
/// two teams need a lobby of at least two.
fn teams(def: &Def) -> Result<()> {
    if !(1..=MAX_TEAMS as u8).contains(&def.teams) {
        return Err(invalid("teams", "a definition has one or two player teams"));
    }
    if def.scoring.player_kill > MAX_PLAYER_KILL_POINTS {
        return Err(invalid("player_kill", "must be in 0..=1,000"));
    }
    if def.teams == 1 && def.scoring.player_kill != 0 {
        return Err(invalid(
            "player_kill",
            "player kills score only with two teams",
        ));
    }
    if def.teams > 1
        && def
            .caps
            .live_actors_by_players
            .last()
            .is_some_and(|&(players, _)| players < 2)
    {
        return Err(invalid(
            "teams",
            "two teams need an actor cap tier for at least two players",
        ));
    }
    Ok(())
}

fn caps(caps: &Caps) -> Result<()> {
    let tiers = &caps.live_actors_by_players;
    wire::check_size(tiers.len(), "actor cap tiers", usize::from(MAX_PLAYERS))?;
    if tiers.is_empty() {
        return Err(invalid("actor cap tiers", "at least one tier is required"));
    }
    let mut previous = 0;
    for &(players, actors) in tiers {
        if players <= previous || players > MAX_PLAYERS {
            return Err(invalid(
                "actor cap tiers",
                "player thresholds must increase from 1 to 16",
            ));
        }
        // The host budget never increases, so the upper endpoint is the
        // tightest budget for every player count covered by this tier.
        let host_cap = HOST_ACTOR_CAPS
            .iter()
            .find_map(|&(upper, cap)| (players <= upper).then_some(cap));
        if actors == 0 || !host_cap.is_some_and(|cap| actors <= cap) {
            return Err(invalid(
                "actor cap tiers",
                "actor caps must be positive and within the host budget",
            ));
        }
        previous = players;
    }
    Ok(())
}

fn wave_actors(wave: &Wave, scoring: &Scoring) -> Result<[u32; MAX_PLAYERS as usize]> {
    wire::check_size(wave.squads.len(), "squads", MAX_SQUADS)?;
    if wave.squads.is_empty() {
        return Err(invalid("squads", "a wave needs at least one squad"));
    }
    if wave.gap_ticks > MAX_GAP_TICKS {
        return Err(invalid("gap_ticks", "the gap must be at most 9,000 ticks"));
    }

    let mut actors = [0; MAX_PLAYERS as usize];
    for squad in &wave.squads {
        class_name(&squad.class)?;
        if !scoring.by_class.contains_key(&squad.class) {
            return Err(invalid("class", "each squad class needs a scoring entry"));
        }
        let count = squad.count;
        if count.min == 0 || count.min > count.max || count.max > MAX_ACTORS {
            return Err(invalid("count", "requires 1 <= min <= max <= 32"));
        }
        if let Drop::PreferZone(zone) = squad.drop
            && zone >= MAX_DROP_ZONES
        {
            return Err(invalid("drop", "the preferred zone must be in 0..12"));
        }
        if squad.weapon_tier > MAX_WEAPON_TIER {
            return Err(invalid("weapon_tier", "the weapon tier must be in 0..=3"));
        }

        for (players, total) in (1..=MAX_PLAYERS).zip(&mut actors) {
            *total += u32::from(count.for_players(players));
        }
    }
    Ok(actors)
}

/// The bonus round's time, its squad against the actor caps, and its
/// points-to-lives table (ascending in points, positive lives).
fn bonus(bonus: &Bonus, def: &Def) -> Result<()> {
    if !(1..=MAX_BONUS_TICKS).contains(&bonus.ticks) {
        return Err(invalid("bonus", "the time must be 1 to 9,000 ticks"));
    }
    let wave = Wave {
        squads: vec![bonus.squad.clone()],
        boss: false,
        gap_ticks: 0,
    };
    for (index, actors) in wave_actors(&wave, &def.scoring)?.into_iter().enumerate() {
        let Some(cap) = def.caps.for_players((index + 1) as u8) else {
            break;
        };
        if actors > u32::from(cap) {
            return Err(invalid(
                "bonus",
                "the squad needs more actors than the cap at some player count",
            ));
        }
    }
    wire::check_size(
        bonus.lives_by_points.len(),
        "bonus rows",
        crate::MAX_BONUS_ROWS,
    )?;
    let mut previous = None;
    for &(points, lives) in &bonus.lives_by_points {
        if points > MAX_BONUS_POINTS || previous.is_some_and(|p| points <= p) {
            return Err(invalid(
                "bonus",
                "points must increase down the table, to at most 1,000,000",
            ));
        }
        if lives == 0 || lives > MAX_LIVES {
            return Err(invalid("bonus", "each row grants 1 to 1,000 lives"));
        }
        previous = Some(points);
    }
    Ok(())
}

fn scoring(scoring: &Scoring) -> Result<()> {
    wire::check_size(scoring.by_class.len(), "scoring classes", MAX_CLASSES)?;
    for (class, &points) in &scoring.by_class {
        class_name(class)?;
        if !(1..=MAX_CLASS_POINTS).contains(&points) {
            return Err(invalid("by_class", "class points must be in 1..=1,000"));
        }
    }

    let difficulty = scoring.difficulty_multiplier;
    if [
        difficulty.easy,
        difficulty.normal,
        difficulty.heroic,
        difficulty.legendary,
    ]
    .iter()
    .chain(scoring.skull_multipliers.values())
    .any(|value| !(1..=MAX_MULTIPLIER).contains(value))
    {
        return Err(invalid(
            "scoring multiplier",
            "must be in 1..=2,000 thousandths",
        ));
    }
    if scoring.skull_multipliers.contains_key(&Skull::Iron) {
        return Err(invalid("skull_multipliers", "Iron has no multiplier"));
    }
    if scoring.assist > MULTIPLIER_SCALE {
        return Err(invalid("assist", "must be in 0..=1,000 thousandths"));
    }
    Ok(())
}

fn class_name(class: &str) -> Result<()> {
    wire::check_size(class.len(), "class bytes", MAX_CLASS_BYTES)?;
    if !class.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        || !class
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(invalid("class", "expected a lowercase ASCII identifier"));
    }
    Ok(())
}

fn invalid(field: &'static str, reason: &'static str) -> Refusal {
    Refusal::Invalid { field, reason }
}
