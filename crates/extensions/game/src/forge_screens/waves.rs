//! The Waves screens (docs/game/forge-firefight-ui.md, "The Waves
//! screens"): WAVES, ROUND, WAVE and SQUAD, list pages on the in-game
//! Controller Settings host, with the lists that can outgrow a page
//! (the rounds, a wave's squads, the problems) on the large dialog, which
//! scrolls; and **Enemy types**.
//!
//! - **Built from facts.** The session publishes the set it plays and what
//!   the pages read with it ([`Facts`], `super::Facts::waves`). A page is
//!   built from them when its row is pressed ([`open`]), and the page it
//!   shows is kept, so it can be built again.
//! - **Each press is one edit.** A value sends [`Choice::Waves`] with an
//!   [`Edit`]; the session applies it to the set it holds ([`apply`]) and
//!   makes one `forge::Session::set_waves` with the whole definition. A
//!   change the validator refuses changes nothing and its sentence is the
//!   notice.
//! - **Built again.** When the published set changes, or an edit was
//!   refused ([`stale`]), each page shown is built again from the set
//!   ([`refresh`]), so a refused value goes back; a page whose round, wave
//!   or squad is gone closes.
//! - **Asked first.** Start from replaces an edited set only after a
//!   question ([`start_from`]); Reset waves, Remove round and Clear wave
//!   ask too ([`WavesAsk`]).
//!
//! Zones are the definition's, numbered from 0 as the kit numbers its
//! `ff_drop_NN` zones and `forge.waves` lists them.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use ::firefight::{Def, Drop, HOST_ACTOR_CAPS, Skull, SquadSpec, Wave};
use forge_doc::waves::{Problem, ProblemKind};

use super::sheet::{
    self, Act, Ask, Choice, Kind, NumberField, PickSpec, Sheet, SheetRow, TypeSpec,
};
use super::{hub, lock, screen};

/// What the session publishes for the Waves screens each tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// The set the session plays: its own, else Holdout.
    pub set: Def,
    /// The session has a set of its own.
    pub own: bool,
    /// The set differs from the one the session loaded: Start from asks
    /// first, and Reset waves is offered.
    pub edited: bool,
    /// The classes the built ground has, which the Enemy picker offers
    /// (the Elite alone with the stage-one ground).
    pub classes: Vec<String>,
    /// The drop zones placed: a squad can prefer zone 0 to one less.
    pub drops: usize,
    /// What the set asks for that the ground lacks
    /// (`forge::Session::wave_problems`).
    pub problems: Vec<Problem>,
    /// What Start from offers: the built-in sets, then the map's own
    /// ([`presets`]), shared, as the session publishes them each tick.
    pub presets: Arc<[Def]>,
    /// A wave test runs: the WAVES page's row reads Stop waves.
    pub testing: bool,
}

/// A class on the Enemy types page: whether the recipe asks for it, and
/// its line ("Grunt. 212 KB.", "Hunter. Cut, no room.").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnemyType {
    pub class: String,
    pub on: bool,
    pub line: String,
}

/// The page a Waves screen shows. Rounds, waves and squads are counted
/// from 0; the titles count them from 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Waves,
    Rounds,
    Round(u8),
    Wave(u8, u8),
    Squads(u8, u8),
    Squad(At),
    Problems,
    /// DROP ZONE n, from 0: what lands at a drop marker and **Add squad
    /// here...** (docs/game/forge-firefight-ui.md, "The AI tool").
    Drop(u8),
    /// The squads that prefer zone n, each opening its SQUAD page.
    DropSquads(u8),
}

/// A squad's place in the set: round, wave and squad, from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    pub round: u8,
    pub wave: u8,
    pub squad: u8,
}

impl Page {
    /// The screen it is shown on.
    pub const fn kind(self) -> Kind {
        match self {
            Page::Waves => Kind::Waves,
            Page::Rounds => Kind::Rounds,
            Page::Round(_) => Kind::Round,
            Page::Wave(..) => Kind::Wave,
            Page::Squads(..) => Kind::Squads,
            Page::Squad(_) => Kind::Squad,
            Page::Problems => Kind::WaveProblems,
            // DROP ZONE is a list page like WAVE, which it replaces on the
            // host: the AI tool never has WAVE up beside it.
            Page::Drop(_) => Kind::Wave,
            Page::DropSquads(_) => Kind::Squads,
        }
    }
}

/// One change of the set: what a value of a Waves page sends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    /// The set's name, as typed.
    Name(String),
    /// Replace the set with preset `i` of [`Facts::presets`].
    StartFrom(usize),
    Teams(u8),
    StartingLives(u16),
    BonusLives(u16),
    SharedLives(bool),
    /// The live enemies allowed with four players; the other tiers scale
    /// with it.
    EnemyLimit(u16),
    /// A round after the last: a copy of it without its skulls.
    AddRound,
    RemoveRound(u8),
    /// Round `round` takes the waves of round `from`.
    CopyRound {
        round: u8,
        from: u8,
    },
    /// The skull a round switches on, or none.
    Skull(u8, Option<Skull>),
    /// The gap before a wave, in seconds.
    StartsAfter {
        round: u8,
        wave: u8,
        seconds: u16,
    },
    /// A squad after the wave's last: a copy of it.
    AddSquad {
        round: u8,
        wave: u8,
    },
    /// Wave `wave` takes the squads and the gap of wave `from` of the
    /// same round.
    CopyWave {
        round: u8,
        wave: u8,
        from: u8,
    },
    /// The wave keeps its first squad alone.
    ClearWave {
        round: u8,
        wave: u8,
    },
    Enemy(At, String),
    PerPlayer(At, u8),
    AtLeast(At, u16),
    AtMost(At, u16),
    Drop(At, Drop),
    /// A squad after the wave's last, a copy of it that prefers zone
    /// `zone` (DROP ZONE's **Add squad here...**).
    AddSquadAt {
        round: u8,
        wave: u8,
        zone: u8,
    },
    /// The same squad again, after it.
    Duplicate(At),
    RemoveSquad(At),
}

/// A question the Waves pages ask before they change the set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavesAsk {
    /// Reset waves: the set as the session loaded it.
    Reset,
    RemoveRound(u8),
    ClearWave(u8, u8),
    /// Start from preset `n` over an edited set.
    StartFrom(u8),
}

/// What a refused index says: the page outlived the part it showed.
pub const GONE: &str = "That part of the waves is gone.";

/// The most rounds a set has (the definition's limit).
pub const MAX_ROUNDS: usize = 12;
/// The most squads a wave has (the definition's limit).
pub const MAX_SQUADS: usize = 16;
/// The Problems list's rows; past them a last row counts the rest.
pub const PROBLEM_ROWS: usize = 24;
/// Ticks a second, as a wave's gap counts them.
const TICKS_PER_SECOND: u32 = 30;
/// The players the enemy limit is set for.
const LIMIT_PLAYERS: u8 = 4;
/// What a class scores when a squad first names it and the set has no
/// points for it: an Elite's in the built-in sets.
const DEFAULT_POINTS: u16 = 100;

/// The Firefight skulls, in the order the Skull picker lists them.
pub const SKULLS: [(Skull, &str); 9] = [
    (Skull::Assassins, "Assassins"),
    (Skull::Thunderstorm, "Thunderstorm"),
    (Skull::Famine, "Famine"),
    (Skull::BlackEye, "Black Eye"),
    (Skull::Ghost, "Ghost"),
    (Skull::Catch, "Catch"),
    (Skull::Mythic, "Mythic"),
    (Skull::Angry, "Angry"),
    (Skull::LowGravity, "Low gravity"),
];

/// The gaps the Starts after picker offers, in seconds.
const GAPS: [u16; 13] = [0, 5, 10, 15, 20, 30, 45, 60, 90, 120, 180, 240, 300];

/// What Start from offers on `map`: the built-in sets, then the sets the
/// `modes` package keeps for that map (`firefight::builtin::map_sets`).
pub fn presets(map: &str) -> Arc<[Def]> {
    let mut all: Vec<Def> = ::firefight::builtin::definitions().into();
    all.extend(::firefight::builtin::map_sets(map));
    all.into()
}

// ---- the pages -------------------------------------------------------

/// The sheet of `page` over `f`; `None` when the round, wave or squad it
/// shows is gone, or a list it shows is empty.
pub fn sheet_for(f: &Facts, page: Page) -> Option<Sheet> {
    match page {
        Page::Waves => Some(waves_sheet(f)),
        Page::Rounds => Some(rounds_sheet(f)),
        Page::Round(r) => round_sheet(f, r),
        Page::Wave(r, w) => wave_sheet(f, r, w),
        Page::Squads(r, w) => squads_sheet(f, r, w),
        Page::Squad(at) => squad_sheet(f, at),
        Page::Problems => problems_sheet(f),
        Page::Drop(zone) => drop_sheet(f, zone),
        Page::DropSquads(zone) => drop_squads_sheet(f, zone),
    }
}

fn waves_choice(edit: Edit) -> Choice {
    Choice::Waves(edit)
}

/// A row whose A opens the value picker on `values`, each sending its
/// choice; the value shown is `values[current]`.
fn pick_row(
    label: &str,
    help: &str,
    values: Vec<String>,
    current: usize,
    choices: Vec<Choice>,
) -> SheetRow {
    let shown = values.get(current).cloned().unwrap_or_default();
    let pick = PickSpec {
        title: label.trim_end_matches("...").into(),
        values,
        current,
        details: Vec::new(),
        description: help.into(),
        choices,
        closes: false,
    };
    SheetRow::new(label, Act::Pick(pick))
        .value(&shown)
        .describe(help)
}

/// A row whose A opens the value picker on `values` with no value of its
/// own: an action with a choice (Copy from).
fn choose_row(label: &str, help: &str, values: Vec<String>, choices: Vec<Choice>) -> SheetRow {
    let mut row = pick_row(label, help, values, 0, choices);
    row.value = None;
    row
}

/// A number row: `values` with `current` among them (added in order when
/// the set holds a number the list does not), each written by `text` and
/// sending `edit` of itself.
fn number_row(
    label: &str,
    help: &str,
    mut values: Vec<u16>,
    current: u16,
    text: impl Fn(u16) -> String,
    edit: impl Fn(u16) -> Edit,
) -> SheetRow {
    if !values.contains(&current) {
        values.push(current);
        values.sort_unstable();
    }
    let at = values.iter().position(|v| *v == current).unwrap_or(0);
    pick_row(
        label,
        help,
        values.iter().map(|v| text(*v)).collect(),
        at,
        values.iter().map(|v| waves_choice(edit(*v))).collect(),
    )
}

/// A row that opens a page above this one; B comes back.
fn page_row(label: &str, help: &str, page: Page) -> SheetRow {
    SheetRow::new(label, Act::Waves(page)).describe(help)
}

fn plain(n: u16) -> String {
    n.to_string()
}

fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The live enemies allowed with four players: the tier that covers
/// four, else the last tier's.
pub fn enemy_limit(def: &Def) -> u16 {
    def.caps
        .for_players(LIMIT_PLAYERS)
        .or_else(|| def.caps.live_actors_by_players.last().map(|t| t.1))
        .unwrap_or(0)
}

/// The host table's budget for a tier ending at `players`.
fn host_cap(players: u8) -> u16 {
    HOST_ACTOR_CAPS
        .iter()
        .find(|(upper, _)| players <= *upper)
        .map_or(HOST_ACTOR_CAPS[HOST_ACTOR_CAPS.len() - 1].1, |t| t.1)
}

/// The values the Enemy limit picker offers: steps of four up to the host
/// table's budget for four players.
fn limit_values() -> Vec<u16> {
    let most = host_cap(LIMIT_PLAYERS);
    (1..=most / 4).map(|i| i * 4).collect()
}

/// WAVES: the set's own settings, the rounds and the problems. The header
/// is the set's name.
pub fn waves_sheet(f: &Facts) -> Sheet {
    let def = &f.set;
    let mut rows = vec![
        SheetRow::new(
            "Name",
            Act::Type(TypeSpec {
                field: NumberField::WavesName,
                initial: def.name.clone(),
            }),
        )
        .value(&def.name)
        .describe("The set's name, as the lobby names it."),
    ];
    if !f.presets.is_empty() {
        let pick = PickSpec {
            title: "Start from".into(),
            values: f.presets.iter().map(|p| p.name.clone()).collect(),
            current: 0,
            details: f.presets.iter().map(preset_line).collect(),
            description: String::new(),
            choices: (0..f.presets.len())
                .map(|i| waves_choice(Edit::StartFrom(i)))
                .collect(),
            closes: false,
        };
        rows.push(
            SheetRow::new("Start from...", Act::Pick(pick))
                .describe("Replace the waves with a built-in set or one of this map's."),
        );
    }
    rows.extend([
        pick_row(
            "Teams",
            "Everyone against the enemies, or two teams that fight each other too.",
            vec!["One team".into(), "Two teams".into()],
            usize::from(def.teams.saturating_sub(1)).min(1),
            vec![waves_choice(Edit::Teams(1)), waves_choice(Edit::Teams(2))],
        ),
        number_row(
            "Starting lives",
            "The lives the players start with.",
            (0..=30).collect(),
            def.lives.pool_start,
            plain,
            Edit::StartingLives,
        ),
        number_row(
            "Bonus lives a round",
            "The lives each round adds.",
            (0..=10).collect(),
            def.lives.per_round_bonus,
            plain,
            Edit::BonusLives,
        ),
        pick_row(
            "Shared lives",
            "One pool for the team, or each player's own.",
            vec!["On".into(), "Off".into()],
            usize::from(!def.lives.shared),
            vec![
                waves_choice(Edit::SharedLives(true)),
                waves_choice(Edit::SharedLives(false)),
            ],
        ),
        number_row(
            "Enemy limit",
            "The live enemies allowed with four players. Fewer players allow fewer.",
            limit_values(),
            enemy_limit(def),
            plain,
            Edit::EnemyLimit,
        ),
        page_row(
            "Rounds...",
            "Each round's skull and waves, and a round to add.",
            Page::Rounds,
        )
        .value(&def.rounds.len().to_string()),
    ]);
    if !f.problems.is_empty() {
        rows.push(
            page_row(
                "Problems",
                "What the waves ask for that the AI ground lacks.",
                Page::Problems,
            )
            .value(&f.problems.len().to_string()),
        );
    }
    rows.push(if f.testing {
        SheetRow::new("Stop waves", Act::Choose(Choice::StopWaves))
            .describe("Erase the test's enemies and stop the waves.")
    } else {
        SheetRow::new("Test waves", Act::Choose(Choice::TestWaves))
            .describe("Fight the waves here, building the AI ground first if needed.")
    });
    if f.edited {
        rows.push(
            SheetRow::new(
                "Reset waves",
                Act::Dialog(Box::new(sheet::question(Ask::Waves(WavesAsk::Reset), ""))),
            )
            .describe("Go back to the waves as this map was loaded."),
        );
    }
    Sheet {
        kind: Kind::Waves,
        title: def.name.to_uppercase(),
        body: String::new(),
        rows,
        focus: 0,
    }
}

/// A preset's line in the Start from picker.
fn preset_line(def: &Def) -> String {
    let teams = if def.teams > 1 { "two teams" } else { "one team" };
    format!("{}, {teams}.", count(def.rounds.len(), "round", "rounds"))
}

/// The skulls a round switches on, by name, "and" between them.
fn skull_names(skulls: &[Skull]) -> Option<String> {
    let names: Vec<&str> = skulls
        .iter()
        .filter_map(|s| SKULLS.iter().find(|(k, _)| k == s).map(|(_, n)| *n))
        .collect();
    (!names.is_empty()).then(|| names.join(" and "))
}

/// A round's line in the Rounds list: its waves and its skull.
pub fn round_line(round: &::firefight::Round) -> String {
    let skull = skull_names(&round.skulls_on).unwrap_or_else(|| "no skull".into());
    format!("{}, {skull}.", count(round.waves.len(), "wave", "waves"))
}

/// The Rounds list: a row a round, each opening ROUND, then **Add round**.
pub fn rounds_sheet(f: &Facts) -> Sheet {
    let mut rows: Vec<SheetRow> = f
        .set
        .rounds
        .iter()
        .enumerate()
        .map(|(r, round)| {
            page_row(
                &format!("Round {}", r + 1),
                &round_line(round),
                Page::Round(r as u8),
            )
        })
        .collect();
    if f.set.rounds.len() < MAX_ROUNDS {
        rows.push(
            SheetRow::new("Add round", Act::Choose(waves_choice(Edit::AddRound)))
                .describe("A round after the last, a copy of it without its skull."),
        );
    }
    Sheet {
        kind: Kind::Rounds,
        title: "ROUNDS".into(),
        body: String::new(),
        rows,
        focus: 0,
    }
}

/// The enemies a wave sends with `players` players.
pub fn enemies(wave: &Wave, players: u8) -> u32 {
    wave.squads
        .iter()
        .map(|s| u32::from(s.count.for_players(players)))
        .sum()
}

/// A wave's value on ROUND: its enemies with one player, then with four
/// ("6 to 14 enemies").
pub fn wave_line(wave: &Wave) -> String {
    let (one, four) = (enemies(wave, 1), enemies(wave, 4));
    if one == four {
        count(one as usize, "enemy", "enemies")
    } else {
        format!("{one} to {four} enemies")
    }
}

/// A wave's enemies against the limit at one, two and four players
/// ("6, 10 and 14 of 16, 24 and 32 enemies."): WAVE's help line.
pub fn wave_help(def: &Def, wave: &Wave) -> String {
    let at = |p: u8| enemies(wave, p);
    let cap = |p: u8| def.caps.for_players(p).unwrap_or_else(|| enemy_limit(def));
    format!(
        "{}, {} and {} of {}, {} and {} enemies with one, two and four players.",
        at(1),
        at(2),
        at(4),
        cap(1),
        cap(2),
        cap(4)
    )
}

fn wave_title(wave: &Wave, w: usize) -> String {
    if wave.boss {
        "Boss wave".into()
    } else {
        format!("Wave {}", w + 1)
    }
}

/// ROUND n: its skull, its five waves, Copy from and Remove round.
pub fn round_sheet(f: &Facts, r: u8) -> Option<Sheet> {
    let def = &f.set;
    let round = def.rounds.get(usize::from(r))?;
    let taken: Vec<Skull> = def
        .rounds
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != usize::from(r))
        .flat_map(|(_, o)| o.skulls_on.iter().copied())
        .collect();
    let free: Vec<&(Skull, &str)> = SKULLS
        .iter()
        .filter(|(s, _)| !taken.contains(s) || round.skulls_on.contains(s))
        .collect();
    let mut values = vec!["None".to_string()];
    values.extend(free.iter().map(|(_, n)| n.to_string()));
    let mut choices = vec![waves_choice(Edit::Skull(r, None))];
    choices.extend(free.iter().map(|(s, _)| waves_choice(Edit::Skull(r, Some(*s)))));
    let current = round
        .skulls_on
        .first()
        .and_then(|s| free.iter().position(|(k, _)| k == s))
        .map_or(0, |i| i + 1);
    let mut skull = pick_row(
        "Skull",
        "The skull this round switches on, on to the end.",
        values,
        current,
        choices,
    );
    if round.skulls_on.len() > 1
        && let Some(names) = skull_names(&round.skulls_on)
    {
        skull = skull.value(&names);
    }
    let mut rows = vec![skull];
    for (w, wave) in round.waves.iter().enumerate() {
        rows.push(
            page_row(
                &wave_title(wave, w),
                &format!("{}.", count(wave.squads.len(), "squad", "squads")),
                Page::Wave(r, w as u8),
            )
            .value(&wave_line(wave)),
        );
    }
    if def.rounds.len() > 1 {
        let others: Vec<usize> = (0..def.rounds.len()).filter(|i| *i != usize::from(r)).collect();
        rows.push(choose_row(
            "Copy from...",
            "This round takes another round's waves.",
            others.iter().map(|i| format!("Round {}", i + 1)).collect(),
            others
                .iter()
                .map(|i| waves_choice(Edit::CopyRound { round: r, from: *i as u8 }))
                .collect(),
        ));
        rows.push(
            SheetRow::new(
                "Remove round",
                Act::Dialog(Box::new(sheet::question(
                    Ask::Waves(WavesAsk::RemoveRound(r)),
                    "",
                ))),
            )
            .describe("Take this round and its waves out of the set."),
        );
    }
    Some(Sheet {
        kind: Kind::Round,
        title: format!("ROUND {}", u16::from(r) + 1),
        body: String::new(),
        rows,
        focus: 0,
    })
}

fn seconds(n: u16) -> String {
    count(usize::from(n), "second", "seconds")
}

/// WAVE n: the gap before it, its squads, Copy from and Clear wave. The
/// Squads row's help is the wave's enemies against the limit.
pub fn wave_sheet(f: &Facts, r: u8, w: u8) -> Option<Sheet> {
    let def = &f.set;
    let round = def.rounds.get(usize::from(r))?;
    let wave = round.waves.get(usize::from(w))?;
    let gap = u16::try_from(wave.gap_ticks / TICKS_PER_SECOND).unwrap_or(u16::MAX);
    let mut rows = vec![
        number_row(
            "Starts after",
            "The gap before the wave.",
            GAPS.to_vec(),
            gap,
            seconds,
            |s| Edit::StartsAfter {
                round: r,
                wave: w,
                seconds: s,
            },
        ),
        page_row("Squads...", &wave_help(def, wave), Page::Squads(r, w))
            .value(&wave.squads.len().to_string()),
    ];
    let others: Vec<usize> = (0..round.waves.len()).filter(|i| *i != usize::from(w)).collect();
    if !others.is_empty() {
        rows.push(choose_row(
            "Copy from...",
            "This wave takes another wave's squads and gap.",
            others
                .iter()
                .map(|i| wave_title(&round.waves[*i], *i))
                .collect(),
            others
                .iter()
                .map(|i| {
                    waves_choice(Edit::CopyWave {
                        round: r,
                        wave: w,
                        from: *i as u8,
                    })
                })
                .collect(),
        ));
    }
    if wave.squads.len() > 1 {
        rows.push(
            SheetRow::new(
                "Clear wave",
                Act::Dialog(Box::new(sheet::question(
                    Ask::Waves(WavesAsk::ClearWave(r, w)),
                    "",
                ))),
            )
            .describe("The wave keeps its first squad alone."),
        );
    }
    Some(Sheet {
        kind: Kind::Wave,
        title: wave_title(wave, usize::from(w)).to_uppercase(),
        body: String::new(),
        rows,
        focus: 0,
    })
}

/// A class's name as the pages write it ("Elite").
pub fn class_title(class: &str) -> String {
    let mut c = class.chars();
    c.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(c).collect()
    })
}

fn drop_text(drop: Drop) -> String {
    match drop {
        Drop::Edge => "Any drop".into(),
        Drop::PreferZone(z) => format!("Drop zone {z}"),
        Drop::Role(role) => format!("{role:?} zones"),
    }
}

/// A squad's line in the Squads list ("Elites, drop zone 3. 1 for each
/// player, 1 to 5.").
pub fn squad_line(squad: &SquadSpec) -> String {
    let c = squad.count;
    format!(
        "{}s, {}. {} for each player, {} to {}.",
        class_title(&squad.class),
        drop_text(squad.drop).to_lowercase(),
        c.per_player,
        c.min,
        c.max
    )
}

/// The Squads list: a row a squad, each opening SQUAD, then **Add squad**.
pub fn squads_sheet(f: &Facts, r: u8, w: u8) -> Option<Sheet> {
    let wave = f.set.rounds.get(usize::from(r))?.waves.get(usize::from(w))?;
    let mut rows: Vec<SheetRow> = wave
        .squads
        .iter()
        .enumerate()
        .map(|(s, squad)| {
            page_row(
                &format!("Squad {}", s + 1),
                &squad_line(squad),
                Page::Squad(At {
                    round: r,
                    wave: w,
                    squad: s as u8,
                }),
            )
        })
        .collect();
    if wave.squads.len() < MAX_SQUADS {
        rows.push(
            SheetRow::new(
                "Add squad",
                Act::Choose(waves_choice(Edit::AddSquad { round: r, wave: w })),
            )
            .describe("A squad after the last, a copy of it."),
        );
    }
    Some(Sheet {
        kind: Kind::Squads,
        title: "SQUADS".into(),
        body: String::new(),
        rows,
        focus: 0,
    })
}

/// SQUAD n: its enemy, its count, its drop, Duplicate and Remove squad.
pub fn squad_sheet(f: &Facts, at: At) -> Option<Sheet> {
    let wave = f
        .set
        .rounds
        .get(usize::from(at.round))?
        .waves
        .get(usize::from(at.wave))?;
    let squad = wave.squads.get(usize::from(at.squad))?;
    let class_at = f.classes.iter().position(|c| *c == squad.class);
    let mut enemy = pick_row(
        "Enemy",
        "The enemy the squad is made of: a class the AI ground has.",
        f.classes.iter().map(|c| class_title(c)).collect(),
        class_at.unwrap_or(0),
        f.classes
            .iter()
            .map(|c| waves_choice(Edit::Enemy(at, c.clone())))
            .collect(),
    );
    if class_at.is_none() {
        enemy = enemy.value(&class_title(&squad.class));
    }
    let c = squad.count;
    let mut drops = vec![Drop::Edge];
    drops.extend((0..f.drops.min(forge_doc::waves::MAX_DROPS)).map(|z| Drop::PreferZone(z as u8)));
    let drop_at = drops.iter().position(|d| *d == squad.drop);
    let mut drop = pick_row(
        "Drop",
        "Where the squad comes in: any drop, or the drop zone it prefers.",
        drops.iter().map(|d| drop_text(*d)).collect(),
        drop_at.unwrap_or(0),
        drops
            .iter()
            .map(|d| waves_choice(Edit::Drop(at, *d)))
            .collect(),
    );
    if drop_at.is_none() {
        drop = drop.value(&drop_text(squad.drop));
    }
    let mut rows = vec![
        enemy,
        number_row(
            "For each player",
            "How many come for each player.",
            (1..=8).collect(),
            u16::from(c.per_player),
            plain,
            |n| Edit::PerPlayer(at, n as u8),
        ),
        number_row(
            "At least",
            "The fewest that come, with any number of players.",
            (1..=32).collect(),
            c.min,
            plain,
            |n| Edit::AtLeast(at, n),
        ),
        number_row(
            "At most",
            "The most that come, with any number of players.",
            (1..=32).collect(),
            c.max,
            plain,
            |n| Edit::AtMost(at, n),
        ),
        drop,
    ];
    if wave.squads.len() < MAX_SQUADS {
        rows.push(
            SheetRow::new("Duplicate", Act::Choose(waves_choice(Edit::Duplicate(at))))
                .describe("The same squad again in this wave."),
        );
    }
    if wave.squads.len() > 1 {
        rows.push(
            SheetRow::new("Remove squad", Act::Choose(waves_choice(Edit::RemoveSquad(at))))
                .describe("Take this squad out of the wave."),
        );
    }
    Some(Sheet {
        kind: Kind::Squad,
        title: format!("SQUAD {}", u16::from(at.squad) + 1),
        body: String::new(),
        rows,
        focus: 0,
    })
}

/// The squads that prefer zone `zone`, in set order.
pub fn squads_at_zone(def: &Def, zone: u8) -> Vec<(At, &SquadSpec)> {
    let mut out = Vec::new();
    for (r, round) in def.rounds.iter().enumerate() {
        for (w, wave) in round.waves.iter().enumerate() {
            for (s, squad) in wave.squads.iter().enumerate() {
                if squad.drop == Drop::PreferZone(zone) {
                    let at = At {
                        round: r as u8,
                        wave: w as u8,
                        squad: s as u8,
                    };
                    out.push((at, squad));
                }
            }
        }
    }
    out
}

/// "2 squads land here": the readout's fact for a drop zone.
pub fn lands_here(n: usize) -> String {
    match n {
        0 => "No squads land here".into(),
        1 => "1 squad lands here".into(),
        n => format!("{n} squads land here"),
    }
}

/// DROP ZONE n: the squads that land here, **Add squad here...** and
/// **Coordinates...**. `None` when the zone is not placed.
pub fn drop_sheet(f: &Facts, zone: u8) -> Option<Sheet> {
    if usize::from(zone) >= f.drops {
        return None;
    }
    let here = squads_at_zone(&f.set, zone).len();
    let squads = if here == 0 {
        SheetRow::new("Squads that land here", Act::Info).value("None")
    } else {
        page_row(
            "Squads that land here",
            "The squads that prefer this drop zone; A opens one.",
            Page::DropSquads(zone),
        )
        .value(&here.to_string())
    };
    let mut labels = Vec::new();
    let mut choices = Vec::new();
    for (r, round) in f.set.rounds.iter().enumerate() {
        for w in 0..round.waves.len() {
            labels.push(format!("Round {} Wave {}", r + 1, w + 1));
            choices.push(waves_choice(Edit::AddSquadAt {
                round: r as u8,
                wave: w as u8,
                zone,
            }));
        }
    }
    Some(Sheet {
        kind: Kind::Wave,
        title: format!("DROP ZONE {zone}"),
        body: String::new(),
        rows: vec![
            squads,
            choose_row(
                "Add squad here...",
                "A copy of a wave's last squad that lands at this drop zone.",
                labels,
                choices,
            ),
            SheetRow::new("Coordinates...", Act::Open(Kind::Coordinates))
                .describe("Where the drop zone stands and which way it faces."),
        ],
        focus: 0,
    })
}

/// The squads of DROP ZONE n, a row each, opening its SQUAD page.
pub fn drop_squads_sheet(f: &Facts, zone: u8) -> Option<Sheet> {
    let rows: Vec<SheetRow> = squads_at_zone(&f.set, zone)
        .into_iter()
        .map(|(at, squad)| {
            page_row(
                &format!(
                    "Round {} Wave {}: {}s",
                    at.round + 1,
                    at.wave + 1,
                    class_title(&squad.class)
                ),
                &squad_line(squad),
                Page::Squad(at),
            )
        })
        .collect();
    (!rows.is_empty()).then(|| Sheet {
        kind: Kind::Squads,
        title: format!("SQUADS AT ZONE {zone}"),
        body: String::new(),
        rows,
        focus: 0,
    })
}

/// A problem's row: where it is, and what is missing.
fn problem_row(def: &Def, p: &Problem) -> SheetRow {
    let squad = def
        .rounds
        .get(usize::from(p.round).wrapping_sub(1))
        .and_then(|r| r.waves.get(usize::from(p.wave).wrapping_sub(1)))
        .and_then(|w| w.squads.get(usize::from(p.squad).wrapping_sub(1)));
    let what = match (p.what, squad) {
        (ProblemKind::ClassNotBuilt, Some(s)) => {
            format!("No {}s on this map.", class_title(&s.class))
        }
        (ProblemKind::ZoneNotPlaced, Some(s)) => {
            format!("{} is not placed.", drop_text(s.drop))
        }
        (ProblemKind::ClassNotBuilt, None) => "Its enemy is not on this map.".into(),
        (ProblemKind::ZoneNotPlaced, None) => "Its drop zone is not placed.".into(),
    };
    page_row(
        &format!("Round {}, wave {}, squad {}", p.round, p.wave, p.squad),
        &what,
        Page::Squad(At {
            round: p.round.saturating_sub(1),
            wave: p.wave.saturating_sub(1),
            squad: p.squad.saturating_sub(1),
        }),
    )
}

/// The Problems list: a row a problem, each opening its squad; `None`
/// with none.
pub fn problems_sheet(f: &Facts) -> Option<Sheet> {
    if f.problems.is_empty() {
        return None;
    }
    let mut rows: Vec<SheetRow> = f
        .problems
        .iter()
        .take(PROBLEM_ROWS)
        .map(|p| problem_row(&f.set, p))
        .collect();
    if f.problems.len() > PROBLEM_ROWS {
        let more = f.problems.len() - PROBLEM_ROWS;
        rows.push(
            SheetRow::new(&count(more, "more problem", "more problems"), Act::Info)
                .describe("Fix the ones above to see them."),
        );
    }
    Some(Sheet {
        kind: Kind::WaveProblems,
        title: "PROBLEMS".into(),
        body: String::new(),
        rows,
        focus: 0,
    })
}

/// The Enemy types page: a row a class, On or Off, its line the cost from
/// the last build. `None` while the recipe names no classes.
pub fn enemy_types_sheet(types: &[EnemyType]) -> Option<Sheet> {
    if types.is_empty() {
        return None;
    }
    let rows = types
        .iter()
        .map(|t| {
            pick_row(
                &class_title(&t.class),
                &t.line,
                vec!["On".into(), "Off".into()],
                usize::from(!t.on),
                vec![
                    Choice::EnemyType(t.class.clone(), true),
                    Choice::EnemyType(t.class.clone(), false),
                ],
            )
        })
        .collect();
    Some(Sheet {
        kind: Kind::EnemyTypes,
        title: "ENEMY TYPES".into(),
        body: String::new(),
        rows,
        focus: 0,
    })
}

/// A Waves question's title and body; `name` is the preset Start from
/// would play.
pub fn question_text(ask: WavesAsk, name: &str) -> (&'static str, String) {
    match ask {
        WavesAsk::Reset => (
            "RESET WAVES",
            "Go back to the waves as this map was loaded? Your edits to them are lost.".into(),
        ),
        WavesAsk::RemoveRound(r) => (
            "REMOVE ROUND",
            format!("Remove round {} and its waves?", u16::from(r) + 1),
        ),
        WavesAsk::ClearWave(r, w) => (
            "CLEAR WAVE",
            format!(
                "Clear wave {} of round {}? It keeps its first squad.",
                u16::from(w) + 1,
                u16::from(r) + 1
            ),
        ),
        WavesAsk::StartFrom(_) => (
            "START FROM",
            format!("Replace the edited waves with {name}? Your edits to them are lost."),
        ),
    }
}

// ---- the edits -------------------------------------------------------

/// What Start from does with preset `i`: replace the set at once, or ask
/// first when the set was edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Start {
    Apply(Edit),
    Ask(Sheet),
}

/// Start from preset `i` of `f`.
pub fn start_from(f: &Facts, i: usize) -> Result<Start, String> {
    let preset = f.presets.get(i).ok_or(GONE)?;
    if !f.edited {
        return Ok(Start::Apply(Edit::StartFrom(i)));
    }
    let n = u8::try_from(i).map_err(|_| GONE.to_string())?;
    Ok(Start::Ask(sheet::question(
        Ask::Waves(WavesAsk::StartFrom(n)),
        &preset.name,
    )))
}

/// The edit Yes to a Waves question makes; `None` for Reset waves, which
/// sets the set as loaded.
pub fn answered(ask: WavesAsk) -> Option<Edit> {
    match ask {
        WavesAsk::Reset => None,
        WavesAsk::RemoveRound(r) => Some(Edit::RemoveRound(r)),
        WavesAsk::ClearWave(round, wave) => Some(Edit::ClearWave { round, wave }),
        WavesAsk::StartFrom(i) => Some(Edit::StartFrom(usize::from(i))),
    }
}

fn round_mut(def: &mut Def, r: u8) -> Result<&mut ::firefight::Round, String> {
    def.rounds.get_mut(usize::from(r)).ok_or_else(|| GONE.into())
}

fn wave_mut(def: &mut Def, r: u8, w: u8) -> Result<&mut Wave, String> {
    round_mut(def, r)?
        .waves
        .get_mut(usize::from(w))
        .ok_or_else(|| GONE.into())
}

fn squad_mut(def: &mut Def, at: At) -> Result<&mut SquadSpec, String> {
    wave_mut(def, at.round, at.wave)?
        .squads
        .get_mut(usize::from(at.squad))
        .ok_or_else(|| GONE.into())
}

/// Every tier of `def` scaled so four players are allowed `limit`, each
/// at least 1 and within the host table's budget for its tier.
fn scale_caps(def: &mut Def, limit: u16) {
    let old = enemy_limit(def).max(1);
    for (players, actors) in &mut def.caps.live_actors_by_players {
        let scaled = (u32::from(*actors) * u32::from(limit) + u32::from(old) / 2) / u32::from(old);
        let scaled = u16::try_from(scaled).unwrap_or(u16::MAX);
        *actors = scaled.clamp(1, host_cap(*players));
    }
}

/// `def` with `edit` made, checked as a document's set is
/// (`forge_doc::waves::check`): the validator's refusal, or a part of the
/// set that is gone, by name. `presets` are Start from's.
pub fn apply(def: &Def, edit: &Edit, presets: &[Def]) -> Result<Def, String> {
    let mut d = def.clone();
    match edit {
        Edit::Name(text) => {
            let text = text.trim();
            if text.is_empty() {
                return Err("Not renamed: the name was empty.".into());
            }
            d.name = text.into();
        }
        Edit::StartFrom(i) => d = presets.get(*i).cloned().ok_or(GONE)?,
        Edit::Teams(n) => {
            d.teams = *n;
            d.scoring.player_kill = if *n > 1 {
                ::firefight::builtin::PLAYER_KILL_POINTS
            } else {
                0
            };
        }
        Edit::StartingLives(n) => d.lives.pool_start = *n,
        Edit::BonusLives(n) => d.lives.per_round_bonus = *n,
        Edit::SharedLives(on) => d.lives.shared = *on,
        Edit::EnemyLimit(n) => scale_caps(&mut d, *n),
        Edit::AddRound => {
            if d.rounds.len() >= MAX_ROUNDS {
                return Err(format!("A set has {MAX_ROUNDS} rounds at most."));
            }
            let mut next = d.rounds.last().cloned().ok_or(GONE)?;
            next.skulls_on.clear();
            d.rounds.push(next);
        }
        Edit::RemoveRound(r) => {
            if d.rounds.len() <= 1 {
                return Err("A set keeps one round at least.".into());
            }
            round_mut(&mut d, *r)?;
            d.rounds.remove(usize::from(*r));
        }
        Edit::CopyRound { round, from } => {
            let waves = d.rounds.get(usize::from(*from)).ok_or(GONE)?.waves.clone();
            round_mut(&mut d, *round)?.waves = waves;
        }
        Edit::Skull(r, skull) => round_mut(&mut d, *r)?.skulls_on = skull.iter().copied().collect(),
        Edit::StartsAfter {
            round,
            wave,
            seconds,
        } => wave_mut(&mut d, *round, *wave)?.gap_ticks = u32::from(*seconds) * TICKS_PER_SECOND,
        Edit::AddSquad { round, wave } => {
            let w = wave_mut(&mut d, *round, *wave)?;
            if w.squads.len() >= MAX_SQUADS {
                return Err(format!("A wave has {MAX_SQUADS} squads at most."));
            }
            let last = w.squads.last().cloned().ok_or(GONE)?;
            w.squads.push(last);
        }
        Edit::CopyWave { round, wave, from } => {
            let source = d
                .rounds
                .get(usize::from(*round))
                .and_then(|r| r.waves.get(usize::from(*from)))
                .cloned()
                .ok_or(GONE)?;
            let w = wave_mut(&mut d, *round, *wave)?;
            w.squads = source.squads;
            w.gap_ticks = source.gap_ticks;
        }
        Edit::ClearWave { round, wave } => wave_mut(&mut d, *round, *wave)?.squads.truncate(1),
        Edit::Enemy(at, class) => {
            squad_mut(&mut d, *at)?.class = class.clone();
            let points = presets
                .iter()
                .find_map(|p| p.scoring.by_class.get(class).copied())
                .unwrap_or(DEFAULT_POINTS);
            d.scoring.by_class.entry(class.clone()).or_insert(points);
        }
        Edit::PerPlayer(at, n) => squad_mut(&mut d, *at)?.count.per_player = *n,
        Edit::AtLeast(at, n) => {
            let c = &mut squad_mut(&mut d, *at)?.count;
            c.min = *n;
            c.max = c.max.max(*n);
        }
        Edit::AtMost(at, n) => {
            let c = &mut squad_mut(&mut d, *at)?.count;
            c.max = *n;
            c.min = c.min.min(*n);
        }
        Edit::Drop(at, drop) => squad_mut(&mut d, *at)?.drop = *drop,
        Edit::AddSquadAt { round, wave, zone } => {
            let w = wave_mut(&mut d, *round, *wave)?;
            if w.squads.len() >= MAX_SQUADS {
                return Err(format!("A wave has {MAX_SQUADS} squads at most."));
            }
            let mut squad = w.squads.last().cloned().ok_or(GONE)?;
            squad.drop = Drop::PreferZone(*zone);
            w.squads.push(squad);
        }
        Edit::Duplicate(at) => {
            let copy = squad_mut(&mut d, *at)?.clone();
            let w = wave_mut(&mut d, at.round, at.wave)?;
            if w.squads.len() >= MAX_SQUADS {
                return Err(format!("A wave has {MAX_SQUADS} squads at most."));
            }
            w.squads.insert(usize::from(at.squad) + 1, copy);
        }
        Edit::RemoveSquad(at) => {
            squad_mut(&mut d, *at)?;
            let w = wave_mut(&mut d, at.round, at.wave)?;
            if w.squads.len() <= 1 {
                return Err("A wave keeps one squad at least.".into());
            }
            w.squads.remove(usize::from(at.squad));
        }
    }
    forge_doc::waves::check(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

/// The first problem as a notice: what **Test waves** and a save say
/// (docs/game/forge-firefight.md, "The wave set"). `None` with none.
pub fn first_problem(def: &Def, problems: &[Problem]) -> Option<String> {
    problems.first().map(|p| p.sentence(def))
}

// ---- the pages shown -------------------------------------------------

/// Shows `page`'s sheet on its screen, built from the published facts,
/// and keeps the page so [`refresh`] builds it again. Refused by name
/// when the session publishes no waves or the part is gone.
pub fn open(page: Page) -> Result<(), String> {
    let f = super::facts()
        .waves
        .ok_or("This session has no waves to edit.")?;
    let sheet = sheet_for(&f, page).ok_or(GONE)?;
    let kind = page.kind();
    screen(kind).show(sheet)?;
    let mut pages = lock(&hub().wave_pages);
    pages.retain(|(k, _)| *k != kind);
    pages.push((kind, page));
    Ok(())
}

/// Opens `page` as the first screen over the world (X on a drop zone):
/// [`open`] with the screen asked for. Refused while another is up.
pub fn show(page: Page) -> Result<(), String> {
    if super::ours_up() {
        return Err("A Forge screen is up.".into());
    }
    open(page)?;
    *lock(&hub().request) = Some(page.kind());
    Ok(())
}

/// The page `kind`'s screen was last opened on.
pub fn shown(kind: Kind) -> Option<Page> {
    lock(&hub().wave_pages)
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, p)| *p)
}

/// An edit was refused: the pages are built again at the next publish, so
/// a value the picker showed goes back to the set's.
pub fn stale() {
    hub().waves_stale.store(true, Ordering::SeqCst);
}

/// Whether an edit was refused since the last ask.
pub(crate) fn take_stale() -> bool {
    hub().waves_stale.swap(false, Ordering::SeqCst)
}

/// Builds each page kept again from `f`, its focus kept; a page whose
/// part is gone is dropped and its screen, if up, closes.
pub fn refresh(f: Option<&Facts>) {
    let pages: Vec<(Kind, Page)> = lock(&hub().wave_pages).clone();
    let mut kept = Vec::with_capacity(pages.len());
    for (kind, page) in pages {
        let s = screen(kind);
        match f.and_then(|f| sheet_for(f, page)) {
            Some(mut next) => {
                let focus = s.sheet().focus;
                next.focus = focus.min(next.rows.len().saturating_sub(1));
                if let Err(why) = s.show(next) {
                    eprintln!("forge screens: {why}");
                }
                kept.push((kind, page));
            }
            None => {
                if s.is_up() {
                    s.close_with_dialog();
                }
            }
        }
    }
    *lock(&hub().wave_pages) = kept;
}

/// An invented set for the pages' samples and tests: two rounds, the first
/// with Catch, its first wave an Elite squad from any drop and a Grunt
/// squad that prefers drop zone 3, on a map with two drops placed.
pub fn sample_facts() -> Facts {
    let squad = |class: &str, drop: serde_json::Value| {
        serde_json::json!({
            "class": class,
            "count": {"per_player": 1, "min": 2, "max": 4},
            "drop": drop,
            "weapon_tier": 0,
        })
    };
    let first = serde_json::json!([squad("elite", "edge".into()), squad("grunt", serde_json::json!({"prefer_zone": 3}))]);
    let one = serde_json::json!([squad("elite", "edge".into())]);
    let waves = |squads: &serde_json::Value| -> Vec<serde_json::Value> {
        (0..5)
            .map(|i| {
                let squads = if i == 0 { squads.clone() } else { one.clone() };
                serde_json::json!({"squads": squads, "boss": i == 4, "gap_ticks": 300})
            })
            .collect()
    };
    let set: Def = serde_json::from_value(serde_json::json!({
        "format": 3,
        "name": "Yard",
        "teams": 1,
        "rounds": [
            {"waves": waves(&first), "skulls_on": ["catch"]},
            {"waves": waves(&one), "skulls_on": []},
        ],
        "set": {"endless": false, "scale": 0, "skulls": []},
        "lives": {"pool_start": 7, "per_round_bonus": 2, "shared": true},
        "caps": {"live_actors_by_players": [[4, 16], [8, 16], [16, 16]]},
        "scoring": {
            "by_class": {"elite": 100, "grunt": 20},
            "player_kill": 0,
            "difficulty_multiplier": {"easy": 1000, "normal": 1000, "heroic": 1000, "legendary": 1000},
            "skull_multipliers": {},
            "assist": 500,
        },
    }))
    .expect("the sample set is a definition");
    let classes: Vec<String> = forge_doc::waves::GROUND_CLASSES.map(String::from).to_vec();
    let drops = 2;
    let problems = forge_doc::waves::problems(&set, &classes, drops);
    Facts {
        set,
        own: true,
        edited: true,
        classes,
        drops,
        problems,
        presets: presets("yard"),
        testing: false,
    }
}

/// Sample Enemy types rows: one kept with its cost, one cut.
pub fn sample_enemy_types() -> Vec<EnemyType> {
    vec![
        EnemyType {
            class: "elite".into(),
            on: true,
            line: "Elite. Always built.".into(),
        },
        EnemyType {
            class: "grunt".into(),
            on: true,
            line: "Grunt. 212 KB.".into(),
        },
        EnemyType {
            class: "hunter".into(),
            on: false,
            line: "Hunter. Cut, no room.".into(),
        },
    ]
}

#[cfg(test)]
#[path = "waves_tests.rs"]
mod tests;
