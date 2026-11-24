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

/// One widget of a script's show state as one viewer sees it
/// (`rules::show::WidgetView`): the numbers a script sets with `hud_value`,
/// `hud_style` and `hud_text`, which each HUD below reads its own way.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Widget {
    /// The widget draws at all (`hud_show`).
    pub shown: bool,
    pub value: i32,
    pub style: u8,
    pub text: Option<String>,
    /// `value` names a side the viewer is on: the viewer's team, or the
    /// viewer's own slot in a game without teams (`rules::show::Viewer::on_side`).
    pub mine: bool,
}

/// What a running script feeds the objective HUDs, for one viewer: its
/// widgets by the element id they are at (`koth.hill`), the names of the
/// players its widgets name by slot, and each navpoint element's distance
/// to the thing its waypoint marks. The game builds it each tick
/// (`game::rules::world::hud`); a state, a number or a name the script
/// leaves out is a row that stays hidden.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fed {
    /// The viewer's team, for the HUDs that tell a side's own from the
    /// other's (`ctf`, `assault`).
    pub team: Option<u8>,
    pub widgets: Vec<(String, Widget)>,
    pub names: Vec<(u8, String)>,
    pub distances: Vec<(String, u16)>,
}

impl Fed {
    /// The widget at element `id`, when the script shows it.
    fn widget(&self, id: &str) -> Option<&Widget> {
        self.widgets.iter().find(|(slot, _)| slot == id).map(|(_, w)| w).filter(|w| w.shown)
    }

    /// The seconds a shown widget counts, from its value.
    fn seconds(&self, id: &str) -> Option<u32> {
        self.widget(id).map(|w| w.value.max(0) as u32)
    }

    fn distance(&self, id: &str) -> Option<u16> {
        self.distances.iter().find(|(slot, _)| slot == id).map(|(_, d)| *d)
    }

    /// The banner at element `id`: the shown widget's text, when the
    /// kit's banner takes it.
    fn banner(&self, id: &str) -> Value {
        let text = self.widget(id).and_then(|w| w.text.as_deref());
        banner(text.filter(|t| crate::model::banner_fault(t).is_none()))
    }

    fn name(&self, slot: i32) -> Option<&str> {
        let slot = u8::try_from(slot).ok()?;
        self.names.iter().find(|(s, _)| *s == slot).map(|(_, n)| n.as_str())
    }
}

/// The values the script feeds the objective HUD named `name`; `None` for
/// a name that is no objective HUD.
pub fn feed(name: &str, fed: &Fed) -> Option<Values> {
    Some(match name {
        ctf::NAME => ctf::feed(fed),
        assault::NAME => assault::feed(fed),
        oddball::NAME => oddball::feed(fed),
        koth::NAME => koth::feed(fed),
        territories::NAME => territories::feed(fed),
        _ => return None,
    })
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
    /// The script's widgets for each team's flag: the HUD's own flag is
    /// the viewer's team's, the enemy's the other's (a viewer on no team
    /// reads team 0's as its own).
    pub const FLAGS: [&str; 2] = ["ctf.flag0", "ctf.flag1"];

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

    /// The HUD a script feeds: each team's flag has a widget ([`FLAGS`]),
    /// style 0 at home, 1 carried (the value is 16 and the carrier's
    /// roster slot, so it is "yours" when you carry it) or 2 dropped; a
    /// flag with no shown widget has no row.
    pub fn feed(f: &Fed) -> Values {
        let flag = |id| {
            f.widget(id).map(|w| match w.style {
                1 if w.mine => Flag::Yours,
                1 => Flag::Taken,
                2 => Flag::Dropped,
                _ => Flag::Home,
            })
        };
        let mine = usize::from(f.team == Some(1));
        let (own, enemy) = (flag(FLAGS[mine]), flag(FLAGS[1 - mine]));
        let state = State {
            own: own.unwrap_or(Flag::Home),
            enemy: enemy.unwrap_or(Flag::Home),
            mark: f.distance(MARK),
            banner: None,
        };
        let mut v = values(&state);
        if own.is_none() {
            v.set(OWN, Value::hidden());
        }
        if enemy.is_none() {
            v.set(ENEMY, Value::hidden());
        }
        v.set(BANNER, f.banner(BANNER));
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
    /// The script's widgets for each team's bomb: the HUD shows the
    /// viewer's team's (a viewer on no team reads team 0's).
    pub const BOMBS: [&str; 2] = ["asl.bomb0", "asl.bomb1"];

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

    /// The HUD a script feeds: each team's bomb has a widget ([`BOMBS`]),
    /// style 0 at base, 1 carried, 2 dropped or 3 armed, with the seconds
    /// to detonation in its value.
    pub fn feed(f: &Fed) -> Values {
        let bomb = f.widget(BOMBS[usize::from(f.team == Some(1))]).map(|w| match w.style {
            1 => Bomb::Carried,
            2 => Bomb::Dropped,
            3 => Bomb::Armed(w.value.max(0) as u32),
            _ => Bomb::Home,
        });
        let state = State {
            bomb: bomb.unwrap_or(Bomb::Home),
            mark: f.distance(MARK),
            banner: None,
        };
        let mut v = values(&state);
        if bomb.is_none() {
            v.set(BOMB, Value::hidden());
        }
        v.set(BANNER, f.banner(BANNER));
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

    /// The HUD a script feeds: the holder's widget (`odd.holder`) is style
    /// 0 for a loose ball or 1 carried, its value `16` and the carrier's
    /// roster slot (the player's name comes from the roster); `odd.time`
    /// counts the seconds you have held it.
    pub fn feed(f: &Fed) -> Values {
        let holder = f.widget(HOLDER).map(|w| match w.style {
            1 if w.mine => Holder::You,
            1 => Holder::Other(f.name(w.value - 16).unwrap_or("another player").to_string()),
            _ => Holder::Nobody,
        });
        let state = State {
            holder: holder.clone().unwrap_or(Holder::Nobody),
            held: f.seconds(TIME),
            mark: f.distance(MARK),
            banner: None,
        };
        let mut v = values(&state);
        if holder.is_none() {
            v.set(HOLDER, Value::hidden());
        }
        v.set(BANNER, f.banner(BANNER));
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

    /// The HUD a script feeds: the hill's widget (`koth.hill`) is style 0
    /// open, 1 held (the value is the holding side, so it is "yours" when
    /// you are on it) or 2 contested; `koth.time` counts the seconds your
    /// side has held it.
    pub fn feed(f: &Fed) -> Values {
        let hill = f.widget(HILL).map(|w| match w.style {
            1 if w.mine => Hill::Yours,
            1 => Hill::Theirs,
            2 => Hill::Contested,
            _ => Hill::Open,
        });
        let state = State {
            hill: hill.unwrap_or(Hill::Open),
            held: f.seconds(TIME),
            mark: f.distance(MARK),
            banner: None,
        };
        let mut v = values(&state);
        if hill.is_none() {
            v.set(HILL, Value::hidden());
        }
        v.set(BANNER, f.banner(BANNER));
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

    /// The HUD a script feeds: the count's widget (`ter.owned`) holds the
    /// territories your side holds in its value and the total in its
    /// style.
    pub fn feed(f: &Fed) -> Values {
        let owned = f.widget(OWNED).map(|w| (w.value.clamp(0, 9) as u8, w.style));
        let state = State {
            owned: owned.map_or(0, |o| o.0),
            total: owned.map_or(0, |o| o.1),
            mark: f.distance(MARK),
            banner: None,
        };
        let mut v = values(&state);
        if owned.is_none() {
            v.set(OWNED, Value::hidden());
        }
        v.set(BANNER, f.banner(BANNER));
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

    fn widget(value: i32, style: u8, mine: bool) -> Widget {
        Widget { shown: true, value, style, text: None, mine }
    }

    fn banner_widget(text: &str) -> Widget {
        Widget { shown: true, text: Some(text.into()), ..Widget::default() }
    }

    fn at(v: &Values, id: &str) -> Option<String> {
        let x = v.get(id).unwrap();
        x.shown.then(|| x.text.clone())
    }

    /// Each HUD reads the script's widgets its own way, a hidden widget
    /// has no row, and every state a feed makes fits the HUD's layout.
    #[test]
    fn a_fed_hud_reads_the_scripts_widgets_and_leaves_the_rest_hidden() {
        let fed = |widgets: &[(&str, Widget)], distances: &[(&str, u16)]| Fed {
            team: None,
            widgets: widgets.iter().map(|(s, w)| (s.to_string(), w.clone())).collect(),
            names: vec![(3, "Ann".into())],
            distances: distances.iter().map(|(s, d)| (s.to_string(), *d)).collect(),
        };
        let koth = feed(
            "koth",
            &fed(
                &[
                    (koth::HILL, widget(1, 1, true)),
                    (koth::TIME, widget(41, 0, false)),
                    (koth::BANNER, banner_widget("HILL MOVED")),
                ],
                &[(koth::MARK, 75)],
            ),
        )
        .unwrap();
        assert_eq!(at(&koth, koth::HILL).as_deref(), Some("Hill: Yours"));
        assert_eq!(at(&koth, koth::TIME).as_deref(), Some("Held 0:41"));
        assert_eq!(at(&koth, koth::MARK).as_deref(), Some("75"));
        assert_eq!(at(&koth, koth::BANNER).as_deref(), Some("HILL MOVED"));
        let theirs = feed("koth", &fed(&[(koth::HILL, widget(1, 1, false))], &[])).unwrap();
        assert_eq!(at(&theirs, koth::HILL).as_deref(), Some("Hill: Held by the enemy"));
        assert_eq!(at(&theirs, koth::TIME), None);
        assert_eq!(at(&theirs, koth::MARK), None);
        assert_eq!(at(&theirs, koth::BANNER), None);

        let flags = feed(
            "ctf",
            &fed(
                &[(ctf::FLAGS[0], widget(0, 1, false)), (ctf::FLAGS[1], widget(16, 1, true))],
                &[],
            ),
        )
        .unwrap();
        assert_eq!(at(&flags, ctf::OWN).as_deref(), Some("Your flag: Taken"));
        assert_eq!(at(&flags, ctf::ENEMY).as_deref(), Some("Enemy flag: Carried by you"));

        let bomb = feed("assault", &fed(&[(assault::BOMBS[0], widget(14, 3, false))], &[])).unwrap();
        assert_eq!(at(&bomb, assault::BOMB).as_deref(), Some("Bomb: Armed 0:14"));
        // A viewer on the second team reads the second team's flag and bomb
        // as its own.
        let mut second = fed(
            &[(ctf::FLAGS[0], widget(0, 1, false)), (ctf::FLAGS[1], widget(0, 2, false))],
            &[],
        );
        second.team = Some(1);
        let second = feed("ctf", &second).unwrap();
        assert_eq!(at(&second, ctf::OWN).as_deref(), Some("Your flag: Dropped"));
        assert_eq!(at(&second, ctf::ENEMY).as_deref(), Some("Enemy flag: Taken"));

        let ball = feed("oddball", &fed(&[(oddball::HOLDER, widget(16 + 3, 1, false))], &[])).unwrap();
        assert_eq!(at(&ball, oddball::HOLDER).as_deref(), Some("Ball: Carried by Ann"));
        let you = feed("oddball", &fed(&[(oddball::HOLDER, widget(16 + 3, 1, true))], &[])).unwrap();
        assert_eq!(at(&you, oddball::HOLDER).as_deref(), Some("Ball: Carried by you"));

        let held = feed("territories", &fed(&[(territories::OWNED, widget(2, 5, false))], &[])).unwrap();
        assert_eq!(at(&held, territories::OWNED).as_deref(), Some("Territories 2 of 5"));

        // A script that feeds nothing shows nothing; a name that is no
        // objective HUD is none.
        let none = feed("koth", &Fed::default()).unwrap();
        assert!(koth::hud().elements.iter().all(|e| at(&none, e.id).is_none()));
        assert!(feed("firefight", &Fed::default()).is_none());
        // A banner the kit refuses stays hidden.
        let long = feed("koth", &fed(&[(koth::BANNER, banner_widget("A\nB"))], &[])).unwrap();
        assert_eq!(at(&long, koth::BANNER), None);
    }

    #[test]
    fn the_clock_is_minutes_and_seconds_and_stops_at_the_widest() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(754), "12:34");
        assert_eq!(clock(999_999), "99:59");
    }
}
