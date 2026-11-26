//! The Firefight HUD (docs/game/firefight.md, "The HUD"): the block at the
//! bottom right, above Halo 2's score plate, as kit elements, in Halo 2's
//! own HUD font and colour.
//!
//! - `ff.score`, `ff.lives`, `ff.progress`: three right-justified lines,
//!   14 pixels apart, from the frame's bottom right corner up;
//! - `ff.enemies`: a strip meter (`Art::StripMeter`, 92 by 8) over them,
//!   the enemies left over the wave's total, or the bonus round's time;
//! - `ff.skull.0..6`: seven gauges of two steps, off and on, one fixed slot
//!   a skull ([`SKULL_SLOTS`]), the pictures drawn from the skulls' strokes
//!   (`font::skulls`, from `game::skull_glyphs`'s strokes);
//! - `ff.banner`: the centred line at the top, the set, the round, the
//!   skull or the wave that begins, and the ends.
//!
//! One layout: a state never adds, removes or moves an element, so the
//! skull slots and the meter keep their places. The quarter screen (three
//! or four players) draws the lives and progress lines alone, one line
//! lower. Where the lines' words and the places differ by screen the game
//! (`game::firefight::hud::layout`) is the source and the HUD takes its
//! numbers; the game gives one [`State`] for each local user's screen.
//!
//! A skull switched on blinks three times ([`Motion::Arrive`]) and stays
//! lit; the last life blinks the same way; the meter eases. The banner's
//! ease in and out is not drawn: a fade needs the colour hook
//! (docs/game/hud-kit.md, "Colours"), so the banner is at full strength
//! for the three seconds it is up.
//!
//! The forge monitor's pills ([`super::forge`]) keep this module's count
//! constants ([`COUNT_MAX`], [`COUNT_LONGEST`], [`PILL_STEP`]).

use crate::model::{Describe, Glyphs, Hud, PILL_BODY, PILL_END, Pulse, Style, Value, Values};
use crate::motion::Motion;

pub const NAME: &str = "firefight";

pub const SCORE: &str = "ff.score";
pub const LIVES: &str = "ff.lives";
pub const PROGRESS: &str = "ff.progress";
pub const ENEMIES: &str = "ff.enemies";
pub const BANNER: &str = "ff.banner";

/// How many skull slots: Ghost, Catch, Black Eye, Thunderstorm, Famine,
/// Mythic, Iron, in that order (`font::skulls::SLOT_SKULLS`;
/// `game::firefight::hud::layout::SKULL_SLOTS`).
pub const SKULL_SLOTS: usize = font::skulls::SLOTS;
/// The skull slots' ids, in slot order.
pub const SKULLS: [&str; SKULL_SLOTS] = [
    "ff.skull.0",
    "ff.skull.1",
    "ff.skull.2",
    "ff.skull.3",
    "ff.skull.4",
    "ff.skull.5",
    "ff.skull.6",
];

/// The elements' ids, in draw order.
pub const IDS: [&str; 4 + SKULL_SLOTS + 1] = [
    SCORE, LIVES, PROGRESS, ENEMIES, SKULLS[0], SKULLS[1], SKULLS[2], SKULLS[3], SKULLS[4],
    SKULLS[5], SKULLS[6], BANNER,
];

/// The widest score line: two teams at the most points.
pub const SCORE_LONGEST: &str = "RED 9999999 BLUE 9999999";
/// The widest lives line.
pub const LIVES_LONGEST: &str = "LIVES 999";
/// The widest progress line: an endless set's.
pub const PROGRESS_LONGEST: &str = "SET 256 ROUND 12 WAVE 16";
/// The widest banner.
pub const BANNER_LONGEST: &str = "BONUS OVER 99 LIVES";

/// The skull pictures' first code unit, U+E600 (`font::skulls::FIRST`).
pub const SKULL_FIRST: u16 = font::skulls::FIRST;
/// The skull pictures' size, wide and tall: inside the full screen's 16
/// pixel slots and the half screen's 14.
pub const SKULL_PICTURE: i16 = font::skulls::PICTURE as i16;

/// Skull slot `slot`'s pictures: step 0 off, step 1 on.
pub const fn skull_glyphs(slot: usize) -> Glyphs {
    Glyphs {
        first: font::skulls::unit(slot, false),
        steps: 1,
        size: (SKULL_PICTURE, SKULL_PICTURE),
    }
}

/// The widest count a pill shows: two digits, as a grenade count's. A
/// larger number shows as [`COUNT_MAX`]. (The Forge monitor's pills.)
pub const COUNT_LONGEST: &str = "99";
pub const COUNT_MAX: u16 = 99;

/// The gap between the kit's two pills: the retail panel's, its count
/// step (53) less a pill's width (50) (V: `grenade_bkd`; docs/game/hud-kit.md,
/// "The pill").
pub const PILL_GAP: i16 = 3;
/// From one pill's left edge to the next's: the kit pill's width
/// ([`PILL_BODY`] and [`PILL_END`]) and the gap.
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

/// How the lives line is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LivesLook {
    Normal,
    /// The last life: the line blinks three times as it appears.
    Last,
    /// None left: the warning colour, steady.
    Out,
}

/// What the HUD shows one local user.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct State {
    /// The score line's words; none when it is hidden.
    pub score: Option<String>,
    /// The lives line's words and look; none for a viewer with no count.
    pub lives: Option<(String, LivesLook)>,
    pub progress: Option<String>,
    /// The enemies meter's fill, 0 to 1; none when it is hidden.
    pub enemies: Option<f32>,
    /// Which skull slots are on.
    pub skulls: [bool; SKULL_SLOTS],
    /// The banner's words.
    pub banner: Option<String>,
}

/// Every element hidden: no match runs.
pub fn hidden() -> Values {
    values(&State::default())
}

/// The values for `state`: one for each element.
pub fn values(state: &State) -> Values {
    let mut out = Values::default();
    let text = |t: &Option<String>| t.as_ref().map_or_else(Value::hidden, |t| Value::text(t.clone()));
    out.set(SCORE, text(&state.score));
    out.set(
        LIVES,
        match &state.lives {
            None => Value::hidden(),
            Some((t, LivesLook::Normal)) => Value::text(t.clone()),
            Some((t, LivesLook::Last)) => Value::text(t.clone()).moving(Motion::Arrive),
            Some((t, LivesLook::Out)) => {
                Value::text(t.clone()).styled(Style::Flash(Pulse::TimeWarning))
            }
        },
    );
    out.set(PROGRESS, text(&state.progress));
    out.set(
        ENEMIES,
        match state.enemies {
            Some(f) => Value::fill(f.clamp(0.0, 1.0)).moving(Motion::Ease),
            None => Value::hidden(),
        },
    );
    for (id, on) in SKULLS.into_iter().zip(state.skulls) {
        out.set(
            id,
            if on {
                Value::fill(1.0).moving(Motion::Arrive)
            } else {
                Value::hidden()
            },
        );
    }
    out.set(BANNER, text(&state.banner));
    out
}

/// The elements, in draw order ([`IDS`]).
pub fn hud() -> Hud {
    crate::data::builtin(NAME)
}

fn some(t: &str) -> Option<String> {
    Some(t.to_owned())
}

/// The named states: the match as it goes, each look of the lives line, a
/// bonus round, two teams, the end, and the widest words.
pub fn states() -> Vec<(&'static str, State)> {
    let fighting = State {
        score: some("SCORE 3400"),
        lives: Some(("LIVES 4".into(), LivesLook::Normal)),
        progress: some("SET 2 ROUND 2 WAVE 3"),
        enemies: Some(0.75),
        skulls: [true, false, false, false, false, true, false],
        banner: None,
    };
    let with = |f: &dyn Fn(&mut State)| {
        let mut s = fighting.clone();
        f(&mut s);
        s
    };
    vec![
        ("fighting", fighting.clone()),
        ("wave-banner", with(&|s| s.banner = some("WAVE 3"))),
        ("skull-banner", with(&|s| s.banner = some("MYTHIC ON"))),
        ("between-waves", with(&|s| {
            s.enemies = None;
            s.banner = some("WAVE CLEAR");
        })),
        ("last-life", with(&|s| s.lives = Some(("LAST LIFE".into(), LivesLook::Last)))),
        ("no-lives", with(&|s| s.lives = Some(("NO LIVES".into(), LivesLook::Out)))),
        ("no-own-lives", with(&|s| s.lives = None)),
        ("bonus", with(&|s| {
            s.progress = some("BONUS 0:42");
            s.enemies = Some(0.4);
            s.banner = some("BONUS ROUND");
        })),
        ("two-teams", with(&|s| {
            s.score = some("RED 450 BLUE 1100");
            s.progress = some("ROUND 2 OF 3 WAVE 3");
        })),
        ("all-skulls", with(&|s| s.skulls = [true; SKULL_SLOTS])),
        ("game-over", State {
            score: some("SCORE 3400"),
            banner: some("GAME OVER"),
            ..State::default()
        }),
        ("none", State::default()),
        ("widest", State {
            score: some(SCORE_LONGEST),
            lives: Some((LIVES_LONGEST.into(), LivesLook::Normal)),
            progress: some(PROGRESS_LONGEST),
            enemies: Some(1.0),
            skulls: [true; SKULL_SLOTS],
            banner: some(BANNER_LONGEST),
        }),
    ]
}

/// The HUD's description.
pub struct Firefight;

impl Describe for Firefight {
    fn hud(&self) -> Hud {
        hud()
    }

    fn states(&self) -> Vec<(&'static str, Values)> {
        states().into_iter().map(|(n, s)| (n, values(&s))).collect()
    }

    fn multiplayer_only(&self) -> Option<&'static str> {
        Some("Firefight runs on multiplayer maps; its enemies meter is the energy sword's battery strip, which a campaign player HUD may lack")
    }

    /// Firefight is a game of its own: never on the screen with a
    /// superbounce game's HUDs, and its block, at the bottom right, never
    /// meets the Forge readout's.
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
    use crate::model::{Anchor, Art, Font, Justify, Kind};
    use crate::records::{self, NO_FONT, WidgetKind, text as tf};
    use crate::templates::{ArtTemplate, GrenadeRow, TextTemplate, Templates};
    use crate::{Mode, compile};
    use map::{FourCC, TagId};

    fn u16_at(b: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([b[at], b[at + 1]])
    }

    fn templates() -> Templates {
        let shad = FourCC(*b"shad");
        Templates {
            nhdt: TagId(0xe000_0001),
            text: Ok(TextTemplate {
                shader: (shad, TagId(0xe000_0003)),
                string: 0x0400_0123,
            }),
            art: vec![(
                Art::StripMeter,
                Ok(ArtTemplate {
                    bitmap: TagId(0xe000_0010),
                    shader: (shad, TagId(0xe000_0002)),
                    sequences: [3, 4, 5],
                    crop: None,
                    ramp: None,
                }),
            )],
            row: Ok(GrenadeRow {
                pill_anchor: 1,
                pill: [(106, 0), (80, 1), (60, 2)],
                count_anchor: 1,
                count: [(118, -1), (90, -2), (70, -3)],
                panel: 103.0,
                frag: [53.0, 50.0],
                step: [(53, 0), (40, 0), (30, 0)],
            }),
        }
    }

    /// The HUD compiles against the player's templates: eleven text
    /// records (three lines, seven skull gauges, the banner), the meter's
    /// bitmap, no channel; each text has its font in the modes it has an
    /// offset in and none in the others (the quarter screen draws the
    /// lives and progress lines alone); every place is from the bottom
    /// right corner but the banner's, from the top left.
    #[test]
    fn the_firefight_hud_compiles_to_records_in_every_mode() {
        let hud = hud();
        assert_eq!(hud.problems(), Vec::<String>::new());
        assert_eq!(hud.elements.iter().map(|e| e.id).collect::<Vec<_>>(), IDS);
        assert!(!hud.uses_grenade_row() && hud.has_text());
        let c = compile(&hud, &templates()).unwrap();
        assert!(c.channels.is_empty());
        assert_eq!(c.widgets.len(), 12);
        let meter = c.widgets.iter().find(|w| w.id == ENEMIES).unwrap();
        assert_eq!(meter.kind, WidgetKind::Bitmap);
        let font = |w: &records::Widget, mode: Mode| w.bytes[tf::FONTS + mode.index()] as i8;
        let shown = |id: &str| -> [bool; 3] {
            let w = c.widgets.iter().find(|w| w.id == id).unwrap();
            assert_eq!(w.kind, WidgetKind::Text, "{id}");
            Mode::ALL.map(|m| font(w, m) != NO_FONT)
        };
        assert_eq!(shown(SCORE), [true, true, false]);
        assert_eq!(shown(LIVES), [true, true, true]);
        assert_eq!(shown(PROGRESS), [true, true, true]);
        assert_eq!(shown(BANNER), [true, true, false]);
        for id in SKULLS {
            assert_eq!(shown(id), [true, true, false], "{id}");
        }
        let at = |id: &str| {
            let w = c.widgets.iter().find(|w| w.id == id).unwrap();
            (
                u16_at(&w.bytes, tf::ANCHOR),
                tf::OFFSETS.map(|o| (u16_at(&w.bytes, o) as i16, u16_at(&w.bytes, o + 2) as i16)),
            )
        };
        let corner = Anchor::BottomRight as u16;
        assert_eq!(at(SCORE).0, corner);
        assert_eq!(at(SCORE).1[0], (0, -58));
        assert_eq!(at(LIVES).1, [(0, -72), (0, -72), (0, -58)]);
        assert_eq!(at(PROGRESS).1, [(0, -86), (0, -86), (0, -72)]);
        assert_eq!(at(BANNER).0, Anchor::TopLeft as u16);
        assert_eq!(at(BANNER).1[0], (272, 60));
        assert_eq!(at(BANNER).1[1], (272, 48));
        // Every line is right-justified at the corner, the banner centred.
        for (id, justify) in [(SCORE, Justify::End), (LIVES, Justify::End), (BANNER, Justify::Centre)] {
            let e = hud.element(id).unwrap();
            assert!(matches!(e.kind, Kind::Text { font: Font::Hud, justify: j, .. } if j == justify), "{id}");
        }
        for (i, id) in SKULLS.into_iter().enumerate() {
            assert_eq!(at(id).0, corner, "{id}");
            // A fixed slot a skull: 18 pixels on at full size, 14 at half.
            let (x, y) = (-(18 * i as i16 + 15), -113);
            let (hx, hy) = (-(14 * i as i16 + 14), -110);
            assert_eq!(at(id).1[..2], [(x, y), (hx, hy)], "{id}");
            let e = hud.element(id).unwrap();
            assert_eq!(e.kind, Kind::Gauge { glyphs: skull_glyphs(i) });
        }
    }

    /// Every state is the one layout, and a skull's slot is the same
    /// whatever is on.
    #[test]
    fn every_state_is_the_one_layout() {
        let h = hud();
        for (name, v) in Firefight.states() {
            assert_eq!(v.layout_problems(&h), Vec::<String>::new(), "{name}");
        }
        assert_eq!(hidden().layout_problems(&h), Vec::<String>::new());
        for (id, v) in &hidden().0 {
            assert!(!v.shown, "{id}");
        }
        // Slots never move: the places do not read the state.
        let before = hud();
        for (_, s) in states() {
            let _ = values(&s);
        }
        assert_eq!(before, hud());
        assert_eq!(skull_glyphs(0).first, SKULL_FIRST);
        assert_eq!(skull_glyphs(6).unit(1.0), SKULL_FIRST + 13);
        assert_eq!(skull_glyphs(3).unit(0.0), SKULL_FIRST + 6);
        for i in 0..SKULL_SLOTS {
            let g = skull_glyphs(i);
            assert_eq!(g.units().count(), 2);
            assert_eq!((g.size.0, g.size.1), (SKULL_PICTURE, SKULL_PICTURE));
        }
    }

    /// A skull on arrives blinking and stays lit; the last life blinks as
    /// it appears; none left is the warning colour; the meter eases.
    #[test]
    fn the_looks_move_as_designed() {
        let s = State {
            lives: Some(("LAST LIFE".into(), LivesLook::Last)),
            enemies: Some(2.0),
            skulls: [false, false, true, false, false, false, false],
            ..State::default()
        };
        let v = values(&s);
        let skull = v.get(SKULLS[2]).unwrap();
        assert!(skull.shown && skull.fill == 1.0 && skull.motion == Motion::Arrive);
        assert!(!v.get(SKULLS[3]).unwrap().shown);
        assert_eq!(v.get(LIVES).unwrap().motion, Motion::Arrive);
        let meter = v.get(ENEMIES).unwrap();
        assert_eq!((meter.fill, meter.motion), (1.0, Motion::Ease));
        let out = values(&State {
            lives: Some(("NO LIVES".into(), LivesLook::Out)),
            ..State::default()
        });
        assert_eq!(out.get(LIVES).unwrap().style, Style::Flash(Pulse::TimeWarning));
        assert_eq!(out.get(LIVES).unwrap().motion, Motion::None);
    }

    /// A skull switched on blinks three times in half a second and stays
    /// lit; one already on does not blink again when another comes on.
    #[test]
    fn a_skull_blinks_three_times_when_switched_on() {
        use crate::motion::{self, BLINK_MS, BLINKS};
        let skulls = |on: [bool; SKULL_SLOTS]| values(&State { skulls: on, ..State::default() });
        let off = skulls([false; SKULL_SLOTS]);
        let mut one = [false; SKULL_SLOTS];
        one[2] = true;
        let on = skulls(one);
        let (before, now) = (off.get(SKULLS[2]).unwrap(), on.get(SKULLS[2]).unwrap());
        assert!(motion::changed(before, now), "off to on starts the motion");
        let stamped = motion::restamp(Some((before, 60_000.0)), now, true).unwrap();
        let slot = BLINK_MS / (2 * BLINKS) as f32;
        let seen: Vec<bool> = (0..2 * BLINKS)
            .map(|k| motion::animate(&stamped, k as f32 * slot + 1.0, true).shown)
            .collect();
        assert_eq!(seen, [true, false, true, false, true, false]);
        assert_eq!(seen.iter().filter(|s| !**s).count(), BLINKS as usize);
        for ms in [BLINK_MS, BLINK_MS + 1.0, 2_000.0, 600_000.0] {
            let v = motion::animate(&stamped, ms, true);
            assert!(v.shown && v.fill == 1.0, "{ms}: steady after the blinks");
        }
        // The same value keeps its stamp; a second skull on restamps its
        // own value alone.
        assert_eq!(motion::restamp(Some((&stamped, 3_000.0)), now, true), None);
        let mut two = one;
        two[0] = true;
        let both = skulls(two);
        assert!(!motion::changed(now, both.get(SKULLS[2]).unwrap()));
        assert!(motion::changed(off.get(SKULLS[0]).unwrap(), both.get(SKULLS[0]).unwrap()));
    }

    #[test]
    fn the_firefight_hud_is_never_shown_with_superbounce() {
        assert!(Firefight.never_with().contains(&super::super::superbounce::NAME));
        assert!(Firefight.never_with().contains(&super::super::superbounce_readout::NAME));
        // The other side says it too, or the runtime's check finds one
        // direction enough.
        let ours = Firefight.hud().name;
        for d in [
            Box::new(super::super::superbounce::Superbounce) as Box<dyn Describe>,
            Box::new(super::super::superbounce_readout::Readout),
        ] {
            let meets = d.never_with().contains(&ours) || Firefight.never_with().contains(&d.hud().name);
            assert!(meets, "{}", d.hud().name);
        }
        assert!(Firefight.multiplayer_only().is_some());
    }

    /// The stage codes read back, and the stages say what they are.
    #[test]
    fn the_stages_read_back() {
        for s in Stage::ALL {
            assert_eq!(Stage::from_code(s as u8), Some(s));
        }
        assert_eq!(Stage::from_code(11), None);
        assert!(Stage::Fighting.wave_on() && !Stage::Intermission.wave_on());
        assert!(Stage::Draw.over() && Stage::Draw.two_teams_only());
        assert_eq!(Stage::team_won(1), Stage::Team1Won);
    }
}
