//! The Firefight HUD (docs/game/forge-firefight-ui.md, "The Firefight
//! HUD"; docs/game/firefight.md, "The HUD"): what Halo 2's own HUD shows
//! of a Firefight match, as kit elements.
//!
//! - `ff.banner`: the message routine's timed centred line
//!   ([`Kind::Banner`]): GET READY, WAVE 3, BOSS WAVE, FINISH THEM, WAVE
//!   COMPLETE, ROUND COMPLETE, and the end. Written when its text changes,
//!   then gone.
//! - `ff.lives.pill`, `ff.lives.end`, `ff.lives.count`: a pill cut from
//!   the frag pill's art with an LCD count, the grenade row's next pill:
//!   the lives the viewer has, flashing at 1 and 0.
//! - `ff.enemies.pill`, `ff.enemies.end`, `ff.enemies.count`: the same,
//!   the place after, while a wave is on: the enemies left.
//! - `ff.note`: a notification ([`Kind::Message`]): the round that starts,
//!   a skull switched on, a life lost ([`notes`]). Sent once each, not
//!   published.
//!
//! The round and wave are not permanent lines: they are in the banner when
//! they change and in the notification list. The score is the game's own
//! (kills credit Slayer's score, which the retail score display and
//! scoreboard show). A pill has no icon: the fallback the design allows
//! until a retail widget's art is chosen for each from the disc's census.
//!
//! [`View`] is what the HUD shows; the game (`game::firefight::hud`) makes
//! it from the mode blob alone, so a client draws what the host draws.

use crate::model::{Describe, Hud, Kind, PILL_BODY, PILL_END, Pulse, Style, Value, Values};

pub const NAME: &str = "firefight";

pub const BANNER: &str = "ff.banner";
pub const LIVES_PILL: &str = "ff.lives.pill";
pub const LIVES_END: &str = "ff.lives.end";
pub const LIVES_COUNT: &str = "ff.lives.count";
pub const ENEMIES_PILL: &str = "ff.enemies.pill";
pub const ENEMIES_END: &str = "ff.enemies.end";
pub const ENEMIES_COUNT: &str = "ff.enemies.count";
pub const NOTE: &str = "ff.note";

/// The elements' ids, in draw order.
pub const IDS: [&str; 8] = [
    BANNER,
    LIVES_PILL,
    LIVES_END,
    LIVES_COUNT,
    ENEMIES_PILL,
    ENEMIES_END,
    ENEMIES_COUNT,
    NOTE,
];

/// The widest banner: the longest of the stage words.
pub const BANNER_LONGEST: &str = "ROUND COMPLETE";
/// The widest count a pill shows: two digits, as a grenade count's. A
/// larger number shows as [`COUNT_MAX`].
pub const COUNT_LONGEST: &str = "99";
pub const COUNT_MAX: u16 = 99;
/// The widest notification.
pub const NOTE_LONGEST: &str = "Round 12 of 12";

/// The gap between the kit's two pills: the retail panel's, its count
/// step (53) less a pill's width (50) (V: `grenade_bkd`; docs/game/hud-kit.md,
/// "The pill").
pub const PILL_GAP: i16 = 3;
/// From the lives pill's left edge to the enemies pill's: the kit pill's
/// width ([`PILL_BODY`] and [`PILL_END`]) and the gap.
pub const PILL_STEP: i16 = PILL_BODY + PILL_END + PILL_GAP;

/// Where the match is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Stage {
    /// Before the first wave.
    Warmup = 0,
    /// The wave's squads are being placed.
    Spawning = 1,
    Fighting = 2,
    /// The last few actors hunt the players.
    Cleanup = 3,
    /// Between waves or rounds.
    Intermission = 4,
    Won = 5,
    Lost = 6,
    /// The director is done and no outcome was logged.
    Over = 7,
    /// Two teams: the first (red) team won.
    Team0Won = 8,
    /// Two teams: the second (blue) team won.
    Team1Won = 9,
    /// Two teams: level on score and lives.
    Draw = 10,
}

impl Stage {
    pub const ALL: [Stage; 11] = [
        Stage::Warmup,
        Stage::Spawning,
        Stage::Fighting,
        Stage::Cleanup,
        Stage::Intermission,
        Stage::Won,
        Stage::Lost,
        Stage::Over,
        Stage::Team0Won,
        Stage::Team1Won,
        Stage::Draw,
    ];

    /// The stage of a code (the mode blob's byte).
    pub fn from_code(code: u8) -> Option<Stage> {
        Stage::ALL.into_iter().find(|s| *s as u8 == code)
    }

    /// The stage of a match `team` won.
    pub fn team_won(team: u8) -> Stage {
        if team == 0 {
            Stage::Team0Won
        } else {
            Stage::Team1Won
        }
    }

    /// A stage only two teams reach.
    pub fn two_teams_only(self) -> bool {
        matches!(self, Stage::Team0Won | Stage::Team1Won | Stage::Draw)
    }

    /// The wave is on: enemies are being placed, fought or hunted.
    pub fn wave_on(self) -> bool {
        matches!(self, Stage::Spawning | Stage::Fighting | Stage::Cleanup)
    }

    /// The match is over.
    pub fn over(self) -> bool {
        matches!(
            self,
            Stage::Won
                | Stage::Lost
                | Stage::Over
                | Stage::Team0Won
                | Stage::Team1Won
                | Stage::Draw
        )
    }
}

/// What the HUD shows, for one viewer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub stage: Stage,
    /// Zero-based; the round and wave that are on or next.
    pub round: u8,
    pub rounds: u8,
    pub wave: u8,
    pub boss: bool,
    /// Actors alive and still to be placed, while a wave is on.
    pub left: u16,
    /// The skulls on, one bit a session skull index.
    pub skulls: u32,
    /// The lives the viewer has: the shared pool, its own, its team's;
    /// none when it has no count.
    pub lives: Option<u16>,
}

/// The banner for `v`'s stage, if it has one: every stage but the fight
/// itself.
pub fn banner_text(v: &View) -> Option<String> {
    Some(match v.stage {
        Stage::Warmup => "GET READY".into(),
        Stage::Spawning if v.boss => "BOSS WAVE".into(),
        Stage::Spawning => format!("WAVE {}", u16::from(v.wave) + 1),
        Stage::Fighting => return None,
        Stage::Cleanup => "FINISH THEM".into(),
        Stage::Intermission if v.wave == 0 && v.round > 0 => "ROUND COMPLETE".into(),
        Stage::Intermission => "WAVE COMPLETE".into(),
        Stage::Won => "VICTORY".into(),
        Stage::Lost | Stage::Over => "GAME OVER".into(),
        Stage::Team0Won => "RED TEAM WINS".into(),
        Stage::Team1Won => "BLUE TEAM WINS".into(),
        Stage::Draw => "DRAW".into(),
    })
}

/// A count as a pill shows it: at most [`COUNT_MAX`].
pub fn count_text(n: u16) -> String {
    n.min(COUNT_MAX).to_string()
}

/// The elements, in draw order ([`IDS`]).
pub fn hud() -> Hud {
    crate::data::builtin(NAME)
}

/// Every element hidden: no match runs.
pub fn hidden() -> Values {
    let mut out = Values::default();
    for id in IDS {
        out.set(id, Value::hidden());
    }
    out
}

/// The values for `v`. The note is hidden: a notification is sent
/// ([`notes`]), not published.
pub fn values(v: &View) -> Values {
    let mut out = hidden();
    if let Some(t) = banner_text(v) {
        out.set(BANNER, Value::text(t));
    }
    if let Some(n) = v.lives.filter(|_| !v.stage.over()) {
        // The last life, and none: the time warning's flash.
        let style = if n <= 1 {
            Style::Flash(Pulse::TimeWarning)
        } else {
            Style::Normal
        };
        out.set(LIVES_PILL, Value::shown().styled(style));
        out.set(LIVES_END, Value::shown().styled(style));
        out.set(LIVES_COUNT, Value::text(count_text(n)).styled(style));
    }
    if v.stage.wave_on() {
        out.set(ENEMIES_PILL, Value::shown());
        out.set(ENEMIES_END, Value::shown());
        out.set(ENEMIES_COUNT, Value::text(count_text(v.left)));
    }
    out
}

/// The notifications the change from `before` to `now` sends: a round
/// that starts ("Round 2 of 3"), each skull switched on, lowest index
/// first ("Mythic skull on", its name from `skull_name`; a skull with no
/// name sends none), lives lost ("A life lost", "2 lives lost") and lives
/// gained, the round's bonus ("Bonus life", "4 bonus lives"). The match's
/// end sends one note, the rounds cleared ([`results_note`]), and the
/// notes stop after it. None from no view (the match's first: the skulls
/// on from the start are the lobby's).
pub fn notes(
    before: Option<&View>,
    now: &View,
    skull_name: impl Fn(u8) -> Option<&'static str>,
) -> Vec<String> {
    let Some(b) = before else {
        return Vec::new();
    };
    if now.stage.over() {
        return if b.stage.over() {
            Vec::new()
        } else {
            results_note(now).into_iter().collect()
        };
    }
    let mut out = Vec::new();
    if now.round > b.round {
        out.push(format!(
            "Round {} of {}",
            u16::from(now.round) + 1,
            now.rounds
        ));
    }
    let on = now.skulls & !b.skulls;
    for n in (0..32u8).filter(|n| on & (1 << n) != 0) {
        if let Some(name) = skull_name(n) {
            out.push(format!("{name} skull on"));
        }
    }
    if let (Some(was), Some(is)) = (b.lives, now.lives) {
        if is < was {
            out.push(match was - is {
                1 => "A life lost".to_owned(),
                n => format!("{n} lives lost"),
            });
        } else if is > was {
            out.push(match is - was {
                1 => "Bonus life".to_owned(),
                n => format!("{n} bonus lives"),
            });
        }
    }
    out
}

/// The note the match's end sends beside its banner: the rounds cleared of
/// the rounds there are ("Cleared 2/3"), all of them for a one-team win.
/// None for the two-team ends, where the banner names the winner and a
/// team can win a round the other never finished, and for a match the
/// director left without an outcome.
pub fn results_note(v: &View) -> Option<String> {
    let cleared = match v.stage {
        Stage::Won => v.rounds,
        Stage::Lost => v.round.min(v.rounds),
        _ => return None,
    };
    Some(format!("Cleared {cleared}/{}", v.rounds))
}

/// The named states: each stage, a boss wave, a round's end, the lives at
/// their extremes, the viewer without a count, and the widest numbers.
pub fn views() -> Vec<(&'static str, View)> {
    let base = View {
        stage: Stage::Fighting,
        round: 0,
        rounds: 3,
        wave: 1,
        boss: false,
        left: 7,
        skulls: 0,
        lives: Some(12),
    };
    let with = |f: &dyn Fn(&mut View)| {
        let mut v = base.clone();
        f(&mut v);
        v
    };
    vec![
        (
            "warmup",
            with(&|v| {
                v.stage = Stage::Warmup;
                v.wave = 0;
                v.left = 0;
            }),
        ),
        ("spawning", with(&|v| v.stage = Stage::Spawning)),
        (
            "boss",
            with(&|v| {
                v.stage = Stage::Spawning;
                v.wave = 4;
                v.boss = true;
            }),
        ),
        ("fighting", base.clone()),
        (
            "cleanup",
            with(&|v| {
                v.stage = Stage::Cleanup;
                v.left = 2;
            }),
        ),
        (
            "wave-complete",
            with(&|v| {
                v.stage = Stage::Intermission;
                v.left = 0;
            }),
        ),
        (
            "round-complete",
            with(&|v| {
                v.stage = Stage::Intermission;
                v.round = 1;
                v.wave = 0;
                v.left = 0;
            }),
        ),
        ("won", with(&|v| v.stage = Stage::Won)),
        ("lost", with(&|v| v.stage = Stage::Lost)),
        ("over", with(&|v| v.stage = Stage::Over)),
        ("last-life", with(&|v| v.lives = Some(1))),
        ("skull-on", with(&|v| v.skulls = 1 << 12)),
        ("no-lives", with(&|v| v.lives = Some(0))),
        ("no-own-lives", with(&|v| v.lives = None)),
        (
            "widest",
            with(&|v| {
                v.round = 11;
                v.rounds = 12;
                v.wave = 15;
                v.left = u16::MAX;
                v.lives = Some(u16::MAX);
            }),
        ),
        ("red-wins", with(&|v| v.stage = Stage::Team0Won)),
        ("blue-wins", with(&|v| v.stage = Stage::Team1Won)),
        ("draw", with(&|v| v.stage = Stage::Draw)),
    ]
}

/// The sample notification a state's preview shows: what [`notes`] sends
/// on the way into it.
fn sample_note(state: &str) -> Option<&'static str> {
    match state {
        "round-complete" => Some("Round 2 of 3"),
        "last-life" => Some("A life lost"),
        "skull-on" => Some("Mythic skull on"),
        "widest" => Some(NOTE_LONGEST),
        _ => None,
    }
}

/// The HUD's description.
pub struct Firefight;

impl Describe for Firefight {
    fn hud(&self) -> Hud {
        hud()
    }

    fn states(&self) -> Vec<(&'static str, Values)> {
        views()
            .into_iter()
            .map(|(name, v)| {
                let mut out = values(&v);
                if let Some(t) = sample_note(name) {
                    out.set(NOTE, Value::text(t));
                }
                (name, out)
            })
            .collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Some("Firefight runs on multiplayer maps; a campaign player HUD may have no grenade row")
    }

    /// Firefight is a game of its own: never on the screen with a
    /// superbounce game's HUDs or Forge's readout.
    fn never_with(&self) -> &'static [&'static str] {
        &[
            super::superbounce::NAME,
            super::superbounce_readout::NAME,
            super::forge_readout::NAME,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Origin;
    use crate::model::{BANNER_UNITS, banner_fault, message_fault};

    fn view(stage: Stage) -> View {
        View {
            stage,
            round: 1,
            rounds: 3,
            wave: 2,
            boss: false,
            left: 9,
            skulls: 0,
            lives: Some(4),
        }
    }

    fn name(n: u8) -> Option<&'static str> {
        match n {
            12 => Some("Mythic"),
            16 => Some("Low Gravity"),
            _ => None,
        }
    }

    fn shown(v: &Values, id: &str) -> Option<String> {
        v.get(id).filter(|x| x.shown).map(|x| x.text.clone())
    }

    /// Every stage has its banner or none, each one line the game keeps
    /// whole, within the element's longest.
    #[test]
    fn every_stage_has_a_banner_or_none() {
        let table: [(Stage, bool, Option<&str>); 13] = [
            (Stage::Warmup, false, Some("GET READY")),
            (Stage::Spawning, false, Some("WAVE 3")),
            (Stage::Spawning, true, Some("BOSS WAVE")),
            (Stage::Fighting, false, None),
            (Stage::Fighting, true, None),
            (Stage::Cleanup, false, Some("FINISH THEM")),
            (Stage::Intermission, false, Some("WAVE COMPLETE")),
            (Stage::Won, false, Some("VICTORY")),
            (Stage::Lost, false, Some("GAME OVER")),
            (Stage::Over, false, Some("GAME OVER")),
            (Stage::Team0Won, false, Some("RED TEAM WINS")),
            (Stage::Team1Won, false, Some("BLUE TEAM WINS")),
            (Stage::Draw, false, Some("DRAW")),
        ];
        for (stage, boss, want) in table {
            let v = View {
                boss,
                ..view(stage)
            };
            assert_eq!(banner_text(&v).as_deref(), want, "{stage:?} boss {boss}");
            assert_eq!(shown(&values(&v), BANNER).as_deref(), want, "{stage:?}");
            if let Some(t) = want {
                assert_eq!(banner_fault(t), None);
                assert!(t.len() <= BANNER_LONGEST.len(), "{t}");
                assert!(t.len() <= BANNER_UNITS);
            }
        }
        // A round's first wave between rounds is the round's end.
        let between = View {
            wave: 0,
            ..view(Stage::Intermission)
        };
        assert_eq!(banner_text(&between).as_deref(), Some("ROUND COMPLETE"));
        let first = View {
            round: 0,
            ..between
        };
        assert_eq!(banner_text(&first).as_deref(), Some("WAVE COMPLETE"));
        // Every stage is in the table.
        for s in Stage::ALL {
            assert!(table.iter().any(|(t, ..)| *t == s), "{s:?}");
            assert_eq!(Stage::from_code(s as u8), Some(s));
        }
        assert_eq!(Stage::from_code(11), None);
    }

    /// The lives pill shows while the match is on and the viewer has a
    /// count, flashing at 1 and 0; the enemies pill while a wave is on;
    /// the note is never published.
    #[test]
    fn the_pills_follow_the_view() {
        let on = values(&view(Stage::Fighting));
        assert_eq!(shown(&on, LIVES_COUNT).as_deref(), Some("4"));
        assert_eq!(shown(&on, ENEMIES_COUNT).as_deref(), Some("9"));
        for id in [LIVES_PILL, LIVES_END, ENEMIES_PILL, ENEMIES_END] {
            let x = on.get(id).unwrap();
            assert!(x.shown && x.style == Style::Normal, "{id}");
        }
        assert!(!on.get(NOTE).unwrap().shown);
        for n in [0, 1] {
            let v = values(&View {
                lives: Some(n),
                ..view(Stage::Fighting)
            });
            for id in [LIVES_PILL, LIVES_END, LIVES_COUNT] {
                assert_eq!(
                    v.get(id).unwrap().style,
                    Style::Flash(Pulse::TimeWarning),
                    "{n} {id}"
                );
            }
        }
        let between = values(&view(Stage::Intermission));
        assert!(shown(&between, LIVES_COUNT).is_some());
        for id in [ENEMIES_PILL, ENEMIES_END, ENEMIES_COUNT] {
            assert!(!between.get(id).unwrap().shown, "{id}: no wave is on");
        }
        let over = values(&view(Stage::Won));
        for id in [LIVES_PILL, LIVES_COUNT, ENEMIES_COUNT] {
            assert!(!over.get(id).unwrap().shown, "{id}: the match is over");
        }
        let none = values(&View {
            lives: None,
            ..view(Stage::Fighting)
        });
        assert!(!none.get(LIVES_PILL).unwrap().shown, "no count, no pill");
        // A count past two digits shows the most.
        let many = values(&View {
            left: 300,
            lives: Some(100),
            ..view(Stage::Fighting)
        });
        assert_eq!(shown(&many, ENEMIES_COUNT).as_deref(), Some("99"));
        assert_eq!(shown(&many, LIVES_COUNT).as_deref(), Some("99"));
        // Every state is the one layout.
        let h = hud();
        assert_eq!(h.problems(), Vec::<String>::new());
        for (name, v) in Firefight.states() {
            assert_eq!(v.layout_problems(&h), Vec::<String>::new(), "{name}");
        }
        assert_eq!(hidden().layout_problems(&h), Vec::<String>::new());
    }

    /// A round that starts and lives lost are notes, once each; the first
    /// view and the end send none.
    #[test]
    fn a_new_round_and_a_lost_life_are_notes() {
        let a = view(Stage::Fighting);
        assert!(notes(None, &a, name).is_empty());
        assert!(notes(Some(&a), &a, name).is_empty());
        let next = View {
            round: 2,
            wave: 0,
            stage: Stage::Spawning,
            ..a.clone()
        };
        assert_eq!(notes(Some(&a), &next, name), ["Round 3 of 3"]);
        let hit = View {
            lives: Some(3),
            ..a.clone()
        };
        assert_eq!(notes(Some(&a), &hit, name), ["A life lost"]);
        let two = View {
            lives: Some(2),
            ..a.clone()
        };
        assert_eq!(notes(Some(&a), &two, name), ["2 lives lost"]);
        // Skulls switched on, lowest index first; one with no name and one
        // switched off send none.
        let skulls = View {
            skulls: 1 << 16 | 1 << 12 | 1 << 3,
            ..a.clone()
        };
        assert_eq!(
            notes(Some(&a), &skulls, name),
            ["Mythic skull on", "Low Gravity skull on"]
        );
        assert!(notes(Some(&skulls), &a, name).is_empty());
        let both = View {
            round: 2,
            skulls: 1 << 12,
            lives: Some(3),
            ..a.clone()
        };
        assert_eq!(
            notes(Some(&a), &both, name),
            ["Round 3 of 3", "Mythic skull on", "A life lost"]
        );
        // Lives gained are the round's bonus, one note.
        assert_eq!(notes(Some(&hit), &a, name), ["Bonus life"]);
        let more = View {
            round: 2,
            lives: Some(8),
            ..a.clone()
        };
        assert_eq!(
            notes(Some(&a), &more, name),
            ["Round 3 of 3", "4 bonus lives"]
        );
        // The end sends the rounds cleared once, then nothing.
        let end = View {
            lives: Some(0),
            ..view(Stage::Lost)
        };
        assert_eq!(notes(Some(&a), &end, name), ["Cleared 1/3"]);
        assert!(notes(Some(&end), &end, name).is_empty());
        let won = View {
            round: 2,
            ..view(Stage::Won)
        };
        assert_eq!(notes(Some(&a), &won, name), ["Cleared 3/3"]);
        for stage in [Stage::Team0Won, Stage::Team1Won, Stage::Draw, Stage::Over] {
            assert!(notes(Some(&a), &view(stage), name).is_empty(), "{stage:?}");
        }
        for t in [
            "Round 12 of 12",
            "A life lost",
            "65535 lives lost",
            "65535 bonus lives",
            "Cleared 12/12",
            "Unlimited Grenades skull on",
        ] {
            assert_eq!(message_fault(t), None, "{t}");
        }
    }

    /// The pills are the grenade row's next place and the one after, a
    /// kit pill's width and the gap apart, in every screen mode; the
    /// counts on them; the channels in every mode.
    #[test]
    fn the_pills_are_the_grenade_rows_next_places() {
        let h = hud();
        assert_eq!(h.elements.iter().map(|e| e.id).collect::<Vec<_>>(), IDS);
        assert!(h.uses_grenade_row() && h.has_text());
        let at = |id: &str| {
            let p = h.element(id).unwrap().at;
            assert_eq!(p.full, p.half.unwrap(), "{id}");
            assert_eq!(p.full, p.quarter.unwrap(), "{id}");
            (p.origin, p.full)
        };
        let (pill, count) = (Origin::NextGrenadePill, Origin::NextGrenadeCount);
        assert_eq!(at(LIVES_PILL), (pill, (0, 0)));
        assert_eq!(at(LIVES_END), (pill, (20, 0)));
        assert_eq!(at(LIVES_COUNT), (count, (0, 0)));
        assert_eq!(at(ENEMIES_PILL), (pill, (30, 0)));
        assert_eq!(at(ENEMIES_END), (pill, (50, 0)));
        assert_eq!(at(ENEMIES_COUNT), (count, (30, 0)));
        assert_eq!(h.element(LIVES_COUNT).unwrap().over, Some(LIVES_PILL));
        assert_eq!(h.element(ENEMIES_COUNT).unwrap().over, Some(ENEMIES_PILL));
        for id in [BANNER, NOTE] {
            let e = h.element(id).unwrap();
            assert!(e.kind.is_channel(), "{id}");
            for m in crate::Mode::ALL {
                assert!(e.at.offset(m).is_some(), "{id} {m:?}");
            }
        }
        assert!(Firefight.multiplayer_only().is_some());
        assert!(
            Firefight
                .never_with()
                .contains(&super::super::superbounce::NAME)
        );
    }
}
