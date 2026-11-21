use std::collections::BTreeSet;

use crate::{
    Caps, Def, Drop, FORMAT, MAX_CLASSES, MAX_DOC_BYTES, MAX_PLAYERS, MAX_ROUNDS, MAX_SKULLS,
    MAX_SQUADS, MULTIPLIER_SCALE, Refusal, Result, Scoring, WAVES_PER_ROUND, Wave, wire,
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
const MAX_MULTIPLIER: u16 = 2 * MULTIPLIER_SCALE;

/// Classes provided by the target kit. An empty kit admits no definitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    pub classes: BTreeSet<String>,
}

impl Limits {
    pub fn new(classes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            classes: classes.into_iter().map(Into::into).collect(),
        }
    }
}

/// Check the format, the worst wave at each player count, and kit admission.
pub fn validate(def: &Def, limits: &Limits) -> Result<()> {
    definition(def)?;
    for class in def.scoring.by_class.keys() {
        if !limits.classes.contains(class) {
            return Err(Refusal::UnknownClass(class.clone()));
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
    let mut enabled = BTreeSet::new();
    for (round_index, round) in def.rounds.iter().enumerate() {
        wire::check_size(round.skulls_on.len(), "skulls_on", MAX_SKULLS)?;
        for &skull in &round.skulls_on {
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
