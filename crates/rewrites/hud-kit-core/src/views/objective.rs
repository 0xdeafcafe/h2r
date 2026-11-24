//! The display of a scripted game (docs/game/game-types-without-carriers.md,
//! "The display"): a base HUD with the score under the type's `points`
//! label and the clock, and one HUD for each objective type, fed by the
//! script's state: the flag, the bomb, the ball, the hill, the territories.
//!
//! Each HUD is data (`content/modes/huds/<name>.hud.yaml`); a state here
//! is what the script replicates for it, and `values` turns the state into
//! the elements' values: rows of text from the frame's bottom left, as the
//! superbounce readout's, a navpoint over the objective with its distance,
//! and the timed banner (the message routine's centred line) for the
//! moment an objective changes hands.

use crate::model::{Describe, Hud, Value, Values};

/// The widest distance a navpoint shows: the game's whole-number text.
pub const MARK_MAX: u16 = 999;

/// `m:ss` of `seconds` (the clock's own form), at most `99:59`.
pub fn clock(seconds: u32) -> String {
    let s = seconds.min(99 * 60 + 59);
    format!("{}:{:02}", s / 60, s % 60)
}

fn mark(distance: Option<u16>) -> Value {
    match distance {
        Some(d) => Value::text(d.min(MARK_MAX).to_string()),
        None => Value::hidden(),
    }
}

fn banner(text: Option<&str>) -> Value {
    text.map_or_else(Value::hidden, Value::text)
}

fn row(text: Option<String>) -> Value {
    text.map_or_else(Value::hidden, Value::text)
}

/// The base HUD ([`Base`]): what the base shows of any scripted game.
pub mod base {
    use super::*;

    pub const NAME: &str = "base";
    pub const SCORE: &str = "base.score";
    pub const TIME: &str = "base.time";

    /// A player's score and the round's clock.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct State {
        /// The game's `points` label (`Kills`, `Captures`).
        pub points: String,
        pub score: i32,
        /// The score that wins, when the game has a limit.
        pub limit: Option<i32>,
        /// Seconds left in the round, when it is timed.
        pub time_left: Option<u32>,
    }

    pub fn hud() -> Hud {
        crate::data::builtin(NAME)
    }

    pub fn values(s: &State) -> Values {
        let mut v = Values::default();
        let score = match s.limit {
            Some(l) => format!("{} {} of {l}", s.points, s.score),
            None => format!("{} {}", s.points, s.score),
        };
        v.set(SCORE, Value::text(score));
        v.set(TIME, row(s.time_left.map(|t| format!("Time left {}", clock(t)))));
        v
    }

    fn state(points: &str, score: i32, limit: Option<i32>, time_left: Option<u32>) -> State {
        State { points: points.into(), score, limit, time_left }
    }

    pub(super) fn views() -> Vec<(&'static str, State)> {
        vec![
            ("kills", state("Kills", 12, Some(25), Some(754))),
            ("captures", state("Captures", 2, Some(5), None)),
            ("unlimited", state("Points", 340, None, Some(59))),
        ]
    }
}

/// The Capture the Flag HUD ([`Ctf`]).
pub mod ctf {
    use super::*;

    pub const NAME: &str = "ctf";
    pub const OWN: &str = "ctf.own";
    pub const ENEMY: &str = "ctf.enemy";
    pub const MARK: &str = "ctf.mark";
    pub const BANNER: &str = "ctf.banner";

    /// Where a flag is.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Flag {
        Home,
        /// A player of the other side has it (or, for the enemy's flag,
        /// a player of ours).
        Taken,
        /// You have it.
        Yours,
        Dropped,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct State {
        pub own: Flag,
        pub enemy: Flag,
        /// The distance to the flag the waypoint marks.
        pub mark: Option<u16>,
        pub banner: Option<&'static str>,
    }

    pub fn hud() -> Hud {
        crate::data::builtin(NAME)
    }

    fn word(f: Flag) -> &'static str {
        match f {
            Flag::Home => "At home",
            Flag::Taken => "Taken",
            Flag::Yours => "Carried by you",
            Flag::Dropped => "Dropped",
        }
    }

    pub fn values(s: &State) -> Values {
        let mut v = Values::default();
        v.set(OWN, Value::text(format!("Your flag: {}", word(s.own))));
        v.set(ENEMY, Value::text(format!("Enemy flag: {}", word(s.enemy))));
        v.set(MARK, mark(s.mark));
        v.set(BANNER, banner(s.banner));
        v
    }

    pub(super) fn views() -> Vec<(&'static str, State)> {
        let s = |own, enemy, mark, banner| State { own, enemy, mark, banner };
        vec![
            ("both-home", s(Flag::Home, Flag::Home, Some(120), None)),
            ("you-carry", s(Flag::Home, Flag::Yours, Some(87), Some("FLAG TAKEN"))),
            ("ours-taken", s(Flag::Taken, Flag::Home, Some(310), None)),
            ("dropped", s(Flag::Dropped, Flag::Dropped, Some(14), Some("FLAG DROPPED"))),
            ("captured", s(Flag::Home, Flag::Home, None, Some("FLAG CAPTURED"))),
        ]
    }
}

/// The Assault HUD ([`Assault`]).
pub mod assault {
    use super::*;

    pub const NAME: &str = "assault";
    pub const BOMB: &str = "asl.bomb";
    pub const MARK: &str = "asl.mark";
    pub const BANNER: &str = "asl.banner";

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Bomb {
        Home,
        Carried,
        Dropped,
        /// Armed, with the seconds to detonation.
        Armed(u32),
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct State {
        pub bomb: Bomb,
        pub mark: Option<u16>,
        pub banner: Option<&'static str>,
    }

    pub fn hud() -> Hud {
        crate::data::builtin(NAME)
    }

    pub fn values(s: &State) -> Values {
        let mut v = Values::default();
        let word = match s.bomb {
            Bomb::Home => "At base".to_string(),
            Bomb::Carried => "Carried".to_string(),
            Bomb::Dropped => "Dropped".to_string(),
            Bomb::Armed(t) => format!("Armed {}", clock(t)),
        };
        v.set(BOMB, Value::text(format!("Bomb: {word}")));
        v.set(MARK, mark(s.mark));
        v.set(BANNER, banner(s.banner));
        v
    }

    pub(super) fn views() -> Vec<(&'static str, State)> {
        let s = |bomb, mark, banner| State { bomb, mark, banner };
        vec![
            ("at-base", s(Bomb::Home, Some(200), None)),
            ("carried", s(Bomb::Carried, Some(95), Some("BOMB TAKEN"))),
            ("dropped", s(Bomb::Dropped, Some(30), Some("BOMB DROPPED"))),
            ("armed", s(Bomb::Armed(14), Some(6), Some("BOMB ARMED"))),
            ("detonated", s(Bomb::Home, None, Some("BOMB DETONATED"))),
        ]
    }
}

/// The Oddball HUD ([`Oddball`]).
pub mod oddball {
    use super::*;

    pub const NAME: &str = "oddball";
    pub const HOLDER: &str = "odd.holder";
    pub const TIME: &str = "odd.time";
    pub const MARK: &str = "odd.mark";
    pub const BANNER: &str = "odd.banner";

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Holder {
        Nobody,
        You,
        /// Another player, by name.
        Other(String),
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct State {
        pub holder: Holder,
        /// Seconds you have held the ball, once you have held it.
        pub held: Option<u32>,
        pub mark: Option<u16>,
        pub banner: Option<&'static str>,
    }

    pub fn hud() -> Hud {
        crate::data::builtin(NAME)
    }

    pub fn values(s: &State) -> Values {
        let mut v = Values::default();
        let who = match &s.holder {
            Holder::Nobody => "Loose".to_string(),
            Holder::You => "Carried by you".to_string(),
            Holder::Other(n) => format!("Carried by {n}"),
        };
        v.set(HOLDER, Value::text(format!("Ball: {who}")));
        v.set(TIME, row(s.held.map(|t| format!("Held {}", clock(t)))));
        v.set(MARK, mark(s.mark));
        v.set(BANNER, banner(s.banner));
        v
    }

    pub(super) fn views() -> Vec<(&'static str, State)> {
        let s = |holder, held, mark, banner| State { holder, held, mark, banner };
        vec![
            ("loose", s(Holder::Nobody, None, Some(40), None)),
            ("you-hold", s(Holder::You, Some(23), None, Some("YOU HAVE THE BALL"))),
            ("other-holds", s(Holder::Other("Ann".into()), Some(23), Some(65), None)),
            ("dropped", s(Holder::Nobody, Some(23), Some(8), Some("BALL DROPPED"))),
        ]
    }
}

/// The King of the Hill HUD ([`Koth`]).
pub mod koth {
    use super::*;

    pub const NAME: &str = "koth";
    pub const HILL: &str = "koth.hill";
    pub const TIME: &str = "koth.time";
    pub const MARK: &str = "koth.mark";
    pub const BANNER: &str = "koth.banner";

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Hill {
        Open,
        Yours,
        Theirs,
        Contested,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct State {
        pub hill: Hill,
        /// Seconds your side has held the hill.
        pub held: Option<u32>,
        pub mark: Option<u16>,
        pub banner: Option<&'static str>,
    }

    pub fn hud() -> Hud {
        crate::data::builtin(NAME)
    }

    pub fn values(s: &State) -> Values {
        let mut v = Values::default();
        let word = match s.hill {
            Hill::Open => "Open",
            Hill::Yours => "Yours",
            Hill::Theirs => "Held by the enemy",
            Hill::Contested => "Contested",
        };
        v.set(HILL, Value::text(format!("Hill: {word}")));
        v.set(TIME, row(s.held.map(|t| format!("Held {}", clock(t)))));
        v.set(MARK, mark(s.mark));
        v.set(BANNER, banner(s.banner));
        v
    }

    pub(super) fn views() -> Vec<(&'static str, State)> {
        let s = |hill, held, mark, banner| State { hill, held, mark, banner };
        vec![
            ("open", s(Hill::Open, None, Some(150), Some("HILL MOVED"))),
            ("yours", s(Hill::Yours, Some(41), Some(0), None)),
            ("theirs", s(Hill::Theirs, Some(41), Some(75), None)),
            ("contested", s(Hill::Contested, Some(41), Some(3), None)),
        ]
    }
}

/// The Territories HUD ([`Territories`]).
pub mod territories {
    use super::*;

    pub const NAME: &str = "territories";
    pub const OWNED: &str = "ter.owned";
    pub const MARK: &str = "ter.mark";
    pub const BANNER: &str = "ter.banner";

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct State {
        /// Territories your side holds, of `total`.
        pub owned: u8,
        pub total: u8,
        pub mark: Option<u16>,
        pub banner: Option<&'static str>,
    }

    pub fn hud() -> Hud {
        crate::data::builtin(NAME)
    }

    pub fn values(s: &State) -> Values {
        let mut v = Values::default();
        v.set(OWNED, Value::text(format!("Territories {} of {}", s.owned.min(9), s.total.min(9))));
        v.set(MARK, mark(s.mark));
        v.set(BANNER, banner(s.banner));
        v
    }

    pub(super) fn views() -> Vec<(&'static str, State)> {
        let s = |owned, mark, banner| State { owned, total: 5, mark, banner };
        vec![
            ("none", s(0, Some(220), None)),
            ("two", s(2, Some(90), Some("TERRITORY CAPTURED"))),
            ("all", s(5, None, Some("TERRITORY CAPTURED"))),
        ]
    }
}

macro_rules! describe {
    ($(#[$doc:meta])* $name:ident, $m:ident) => {
        $(#[$doc])*
        pub struct $name;

        impl Describe for $name {
            fn hud(&self) -> Hud {
                $m::hud()
            }
            fn states(&self) -> Vec<(&'static str, Values)> {
                $m::views().into_iter().map(|(n, s)| (n, $m::values(&s))).collect()
            }
        }
    };
}

describe!(
    /// The base HUD's description.
    Base, base
);
describe!(
    /// The Capture the Flag HUD's description.
    Ctf, ctf
);
describe!(
    /// The Assault HUD's description.
    Assault, assault
);
describe!(
    /// The Oddball HUD's description.
    Oddball, oddball
);
describe!(
    /// The King of the Hill HUD's description.
    Koth, koth
);
describe!(
    /// The Territories HUD's description.
    Territories, territories
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;

    fn all() -> Vec<Box<dyn Describe>> {
        vec![
            Box::new(Base),
            Box::new(Ctf),
            Box::new(Assault),
            Box::new(Oddball),
            Box::new(Koth),
            Box::new(Territories),
        ]
    }

    /// Each HUD reads, has no problems, and every one of its states values
    /// each element once; its strings fit the longest its element names.
    #[test]
    fn each_display_hud_compiles_and_every_state_fits_it() {
        for d in all() {
            let hud = d.hud();
            assert_eq!(hud.problems(), Vec::<String>::new(), "{}", hud.name);
            assert!(!d.states().is_empty(), "{}", hud.name);
            for (name, v) in d.states() {
                assert_eq!(v.layout_problems(&hud), Vec::<String>::new(), "{} {name}", hud.name);
                for e in &hud.elements {
                    let value = v.get(e.id).unwrap();
                    let longest = match &e.kind {
                        Kind::Text { longest, .. } | Kind::Banner { longest } => *longest,
                        Kind::Navpoint { longest } => *longest,
                        other => panic!("{}: {other:?}", e.id),
                    };
                    if value.shown {
                        assert!(
                            value.text.chars().count() <= longest.chars().count(),
                            "{} {name}: {:?} is wider than {longest:?}",
                            e.id,
                            value.text
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_hud_names_are_the_yaml_files_and_are_registered() {
        let names: Vec<_> = all().iter().map(|d| d.hud().name).collect();
        assert_eq!(names, ["base", "ctf", "assault", "oddball", "koth", "territories"]);
        for n in names {
            assert!(crate::views::find(n).is_some(), "{n}");
        }
    }

    fn text(d: &dyn Describe, state: &str, id: &str) -> Option<String> {
        let v = d.states().into_iter().find(|(n, _)| *n == state).unwrap().1;
        let x = v.get(id).unwrap();
        x.shown.then(|| x.text.clone())
    }

    /// An invented script state reaches the elements: the base's score
    /// is under the game's label, the flag rows read each flag's place, the
    /// ball's time is hidden until held, the navpoint is the distance.
    #[test]
    fn an_invented_script_state_shows_in_the_elements() {
        assert_eq!(text(&Base, "kills", base::SCORE).as_deref(), Some("Kills 12 of 25"));
        assert_eq!(text(&Base, "kills", base::TIME).as_deref(), Some("Time left 12:34"));
        assert_eq!(text(&Base, "captures", base::TIME), None);
        assert_eq!(text(&Base, "unlimited", base::SCORE).as_deref(), Some("Points 340"));
        assert_eq!(text(&Ctf, "you-carry", ctf::ENEMY).as_deref(), Some("Enemy flag: Carried by you"));
        assert_eq!(text(&Ctf, "ours-taken", ctf::OWN).as_deref(), Some("Your flag: Taken"));
        assert_eq!(text(&Ctf, "captured", ctf::MARK), None);
        assert_eq!(text(&Ctf, "captured", ctf::BANNER).as_deref(), Some("FLAG CAPTURED"));
        assert_eq!(text(&Assault, "armed", assault::BOMB).as_deref(), Some("Bomb: Armed 0:14"));
        assert_eq!(text(&Oddball, "loose", oddball::TIME), None);
        assert_eq!(text(&Oddball, "other-holds", oddball::HOLDER).as_deref(), Some("Ball: Carried by Ann"));
        assert_eq!(text(&Oddball, "you-hold", oddball::TIME).as_deref(), Some("Held 0:23"));
        assert_eq!(text(&Koth, "contested", koth::HILL).as_deref(), Some("Hill: Contested"));
        assert_eq!(text(&Koth, "contested", koth::MARK).as_deref(), Some("3"));
        assert_eq!(text(&Territories, "two", territories::OWNED).as_deref(), Some("Territories 2 of 5"));
    }

    #[test]
    fn the_clock_is_minutes_and_seconds_and_stops_at_the_widest() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(754), "12:34");
        assert_eq!(clock(999_999), "99:59");
    }
}
