//! Match files, bot tuning files and batch files (docs/runtime/sim.md §5,
//! §7). TOML, or JSON when the file name ends in `.json`. Every file is
//! read whole and checked before anything runs: [`load_match`] and
//! [`load_batch`] answer a list of problems, not the first one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::clock::Pace;

/// The retail session's 16 peer slots include the playerless host.
pub const MAX_BOTS: usize = 15;

/// A bot's seat. Every bot is a phantom: a remote System Link peer, never
/// a controller seat, which is for a real person. A match file may still
/// write `kind = "phantom"` (the default); `kind = "local"` is refused
/// with an error.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PhantomSeat;

impl Serialize for PhantomSeat {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("phantom")
    }
}

impl<'de> Deserialize<'de> for PhantomSeat {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = PhantomSeat;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(r#""phantom""#)
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<PhantomSeat, E> {
                match v {
                    "phantom" => Ok(PhantomSeat),
                    "local" => Err(E::custom(
                        "kind = \"local\": local bots are gone; every bot is a phantom (a \
                         remote System Link peer), and controller seats are for people",
                    )),
                    other => Err(E::unknown_variant(other, &["phantom"])),
                }
            }
        }
        d.deserialize_str(V)
    }
}

/// Variant settings are numbered `0..0x70` (`variants::record`).
pub const MAX_SETTING: u32 = 0x70;

/// The variant setting that holds the time limit, in seconds.
pub const TIME_LIMIT_SETTING: u32 = 10;
/// The variant setting that turns team play on.
pub const TEAM_PLAY_SETTING: u32 = 91;

/// A game type, by the name a match file gives it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GameType {
    #[default]
    Slayer,
    Oddball,
    Juggernaut,
    King,
    Ctf,
    Assault,
    Territories,
}

impl GameType {
    /// In the game's built-in list order (`variants::GameType::ALL`), which
    /// is the type number `VARIANT_DEFAULTS` (`0x19d220`) takes.
    pub const ALL: [GameType; 7] = [
        GameType::Slayer,
        GameType::Oddball,
        GameType::Juggernaut,
        GameType::King,
        GameType::Ctf,
        GameType::Assault,
        GameType::Territories,
    ];

    /// The built-in list index (0 Slayer .. 6 Territories).
    pub fn list_index(self) -> u32 {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0) as u32
    }

    pub fn name(self) -> &'static str {
        match self {
            GameType::Slayer => "slayer",
            GameType::Oddball => "oddball",
            GameType::Juggernaut => "juggernaut",
            GameType::King => "king",
            GameType::Ctf => "ctf",
            GameType::Assault => "assault",
            GameType::Territories => "territories",
        }
    }

    pub fn parse(s: &str) -> Result<GameType, String> {
        let k: String = s
            .trim()
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        Ok(match k.as_str() {
            "slayer" => GameType::Slayer,
            "oddball" | "ball" => GameType::Oddball,
            "juggernaut" => GameType::Juggernaut,
            "king" | "koth" | "kingofthehill" => GameType::King,
            "ctf" | "capturetheflag" => GameType::Ctf,
            "assault" => GameType::Assault,
            "territories" => GameType::Territories,
            _ => {
                return Err(format!(
                    "unknown game type {s:?} (slayer, oddball, juggernaut, king, ctf, assault, territories)"
                ));
            }
        })
    }

    /// The game is won by something other than kills.
    pub fn objective(self) -> bool {
        !matches!(self, GameType::Slayer | GameType::Juggernaut)
    }

    /// The variant setting that holds this type's score to win (all write
    /// `+0x50`; `variants::GameType::score_setting`).
    pub fn score_setting(self) -> u32 {
        match self {
            GameType::Ctf => 1,
            GameType::Slayer => 2,
            GameType::Oddball => 3,
            GameType::King => 4,
            GameType::Juggernaut => 7,
            GameType::Territories => 8,
            GameType::Assault => 9,
        }
    }
}

/// A bot's skill (`bots::mind::Skill`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Skill {
    Easy,
    #[default]
    Normal,
    Heroic,
    Legendary,
}

impl Skill {
    pub fn parse(s: &str) -> Result<Skill, String> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "easy" => Skill::Easy,
            "normal" => Skill::Normal,
            "heroic" => Skill::Heroic,
            "legendary" => Skill::Legendary,
            _ => {
                return Err(format!(
                    "unknown skill {s:?} (easy, normal, heroic, legendary)"
                ));
            }
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Skill::Easy => "easy",
            Skill::Normal => "normal",
            Skill::Heroic => "heroic",
            Skill::Legendary => "legendary",
        }
    }
}

/// What a bot is tuned by (docs/runtime/sim.md, "Bot parameters from a
/// file"). Every field is optional: one left out keeps the built-in value
/// for the bot's skill.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotParams {
    /// easy, normal, heroic or legendary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    /// Nominal reaction, ticks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reaction: Option<u32>,
    /// The tracking error's spread, radians.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<f32>,
    /// How much of the needed lead it gives a moving target, 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<f32>,
    /// Fraction of full turn rate per radian of aim error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<f32>,
    /// Fires while the aim is this close to the target, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fire_cone_deg: Option<f32>,
    /// `false`: hold the trigger. `true` with `burst_on`/`burst_off`: fire
    /// in bursts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst_on: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burst_off: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grenades_per_minute: Option<f32>,
    /// The decisions' sense of the fight, 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub awareness: Option<f32>,
    /// The personality, each 0..1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggression: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caution: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teamwork: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective_focus: Option<f32>,
    /// World samples are this many game ticks old.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub perception_ticks: Option<u32>,
    /// Nominal ticks from a decision to the pad report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motor_ticks: Option<u32>,
    /// Button-edge timing standard deviation in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub button_sigma_ms: Option<f32>,
    /// Stick travel per tick in stick radii.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stick_step: Option<f32>,
    /// Ticks between right-stick use and face-button use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumb_switch_ticks: Option<u32>,
    /// Phantom round-trip latency in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_ping_ms: Option<f32>,
    /// Maximum phantom one-way variation in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_jitter_ms: Option<f32>,
    /// What turns the hand's view of a target into the right stick:
    /// `code` (the hand-written tracking) or `net` (the aim net).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foot_plan_ticks: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle_plan_ticks: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle_lookahead_ticks: Option<u32>,
    /// Accepted for older tuning files. Player sight is sampled every tick.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player_los_ticks: Option<u32>,
    /// The bounce route's numbers (`[bounce]`); last, as a TOML table
    /// follows the plain values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounce: Option<BounceParams>,
}

/// The aim models a tuning can name (`bots::tuning::AimModel`).
pub const AIM_MODELS: [&str; 2] = ["code", "net"];

/// A number a bounce tuning field holds: read from and written as `f64`
/// by the sweep ([`crate::sweep`]).
pub trait BounceNumber: Copy {
    fn to_f64(self) -> f64;
    fn from_f64(v: f64) -> Self;
}

impl BounceNumber for f32 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
    fn from_f64(v: f64) -> Self {
        v as f32
    }
}

impl BounceNumber for u32 {
    fn to_f64(self) -> f64 {
        f64::from(self)
    }
    fn from_f64(v: f64) -> Self {
        v.round().clamp(0.0, f64::from(u32::MAX)) as u32
    }
}

/// One bounce tuning field as the schema has it: its name, the built-in
/// value and the range a file may set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BounceField {
    pub name: &'static str,
    pub default: f64,
    pub min: f64,
    pub max: f64,
    /// Ticks or tries: a whole number.
    pub whole: bool,
}

macro_rules! bounce_params {
    (@whole u32) => { true };
    (@whole f32) => { false };
    ($( $(#[$m:meta])* $name:ident : $ty:ident = $default:expr, $lo:expr, $hi:expr ;)*) => {
        /// The bounce route's numbers (`bots::tuning::BounceTuning`;
        /// docs/game/bots.md, "Tuning the bounce"), each optional: one left
        /// out keeps the built-in value. `[bounce]` in a tuning file.
        #[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct BounceParams {
            $(
                $(#[$m])*
                #[serde(default, skip_serializing_if = "Option::is_none")]
                pub $name: Option<$ty>,
            )*
        }

        /// Every bounce tuning field: name, built-in value, range. The
        /// defaults are `bots::tuning::BOUNCE_DEFAULT`'s (a test in the
        /// game's `sim_glue` holds them equal).
        pub const BOUNCE_FIELDS: &[BounceField] = &[
            $(
                BounceField {
                    name: stringify!($name),
                    default: $default as f64,
                    min: $lo as f64,
                    max: $hi as f64,
                    whole: bounce_params!(@whole $ty),
                },
            )*
        ];

        impl BounceParams {
            /// `self` with every field `over` sets replaced by `over`'s.
            pub fn overlay(&self, over: &BounceParams) -> BounceParams {
                BounceParams {
                    $( $name: over.$name.or(self.$name), )*
                }
            }

            /// The field `name`'s value, if set.
            pub fn get(&self, name: &str) -> Option<f64> {
                match name {
                    $( stringify!($name) => self.$name.map(BounceNumber::to_f64), )*
                    _ => None,
                }
            }

            /// Sets the field `name` (rounded for a whole-number field).
            pub fn set(&mut self, name: &str, v: f64) -> Result<(), String> {
                match name {
                    $( stringify!($name) => {
                        self.$name = Some(<$ty as BounceNumber>::from_f64(v));
                        Ok(())
                    } )*
                    _ => Err(format!("bounce.{name}: no such field ({})", bounce_names())),
                }
            }

            /// Whether it sets nothing.
            pub fn is_empty(&self) -> bool {
                $( self.$name.is_none() && )* true
            }
        }
    };
}

bounce_params! {
    /// The walk to a standing site ends this far behind it (world units).
    approach: f32 = 0.8, 0.0, 3.0;
    /// The stick on the walk to the approach point.
    approach_speed: f32 = 1.0, 0.1, 1.0;
    /// Within this of the site it stands (world units).
    settle: f32 = 0.05, 0.0, 0.5;
    /// Settling gives up after this long and jumps (ticks).
    settle_ticks: u32 = 90, 0, 600;
    /// The settling stick: gain a unit, held between min and max.
    settle_gain: f32 = 4.0, 0.0, 20.0;
    settle_min: f32 = 0.12, 0.0, 1.0;
    settle_max: f32 = 0.6, 0.0, 1.0;
    /// Still this long before the jump (ticks).
    still_ticks: u32 = 4, 0, 60;
    /// A held this long for the jump (ticks).
    jump_ticks: u32 = 3, 1, 15;
    /// The stick toward the drift in the air: the aim onto the crease.
    drift: f32 = 0.15, 0.0, 1.0;
    /// Jumps at a site before giving it up.
    bounce_tries: u32 = 8, 1, 30;
    /// The prime: crouch first standing still, crouch under the lip,
    /// pushing in after letting go (ticks); the stick in, how far past the
    /// stand, how far out a try starts (world units), the second and third
    /// tries' turn (degrees), the stick out.
    crouch_first: u32 = 6, 0, 60;
    crouch_ticks: u32 = 12, 1, 60;
    release_ticks: u32 = 6, 0, 60;
    in_speed: f32 = 0.6, 0.0, 1.0;
    push_in: f32 = 0.06, 0.0, 0.5;
    start_out: f32 = 0.35, 0.0, 2.0;
    approach_turn_deg: f32 = 25.0, 0.0, 90.0;
    out_speed: f32 = 1.0, 0.0, 1.0;
    /// The drop: lined up within (world units) or after (ticks), the stick
    /// of the slower run, the air steering's correction over just enough,
    /// drops before giving up.
    line_up: f32 = 0.15, 0.0, 1.0;
    line_up_ticks: u32 = 45, 0, 300;
    run_off_speed: f32 = 0.6, 0.0, 1.0;
    air_gain: f32 = 1.2, 0.0, 20.0;
    drop_tries: u32 = 3, 1, 30;
    /// The stick toward the goal riding a launch.
    fly_speed: f32 = 1.0, 0.0, 1.0;
}

/// The bounce tuning fields' names, for messages.
pub fn bounce_names() -> String {
    BOUNCE_FIELDS
        .iter()
        .map(|f| f.name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The schema of the bounce field `name`.
pub fn bounce_field(name: &str) -> Option<&'static BounceField> {
    BOUNCE_FIELDS.iter().find(|f| f.name == name)
}

impl BounceParams {
    /// What is wrong with it, if anything: a value out of its field's
    /// range, or a settling stick whose least is over its most.
    pub fn problems(&self, at: &str) -> Vec<String> {
        let mut out = Vec::new();
        for f in BOUNCE_FIELDS {
            if let Some(v) = self.get(f.name)
                && !(v.is_finite() && (f.min..=f.max).contains(&v))
            {
                out.push(format!(
                    "{at}: bounce.{} = {v}: {} to {}",
                    f.name, f.min, f.max
                ));
            }
        }
        if let (Some(lo), Some(hi)) = (self.settle_min, self.settle_max)
            && lo > hi
        {
            out.push(format!(
                "{at}: bounce.settle_min = {lo} is over settle_max = {hi}"
            ));
        }
        out
    }
}

impl BotParams {
    /// `self` with every field `over` sets replaced by `over`'s.
    pub fn overlay(&self, over: &BotParams) -> BotParams {
        fn pick<T: Clone>(a: &Option<T>, b: &Option<T>) -> Option<T> {
            b.clone().or_else(|| a.clone())
        }
        BotParams {
            skill: pick(&self.skill, &over.skill),
            reaction: pick(&self.reaction, &over.reaction),
            error: pick(&self.error, &over.error),
            lead: pick(&self.lead, &over.lead),
            gain: pick(&self.gain, &over.gain),
            fire_cone_deg: pick(&self.fire_cone_deg, &over.fire_cone_deg),
            burst: pick(&self.burst, &over.burst),
            burst_on: pick(&self.burst_on, &over.burst_on),
            burst_off: pick(&self.burst_off, &over.burst_off),
            grenades_per_minute: pick(&self.grenades_per_minute, &over.grenades_per_minute),
            awareness: pick(&self.awareness, &over.awareness),
            aggression: pick(&self.aggression, &over.aggression),
            caution: pick(&self.caution, &over.caution),
            teamwork: pick(&self.teamwork, &over.teamwork),
            objective_focus: pick(&self.objective_focus, &over.objective_focus),
            perception_ticks: pick(&self.perception_ticks, &over.perception_ticks),
            motor_ticks: pick(&self.motor_ticks, &over.motor_ticks),
            button_sigma_ms: pick(&self.button_sigma_ms, &over.button_sigma_ms),
            stick_step: pick(&self.stick_step, &over.stick_step),
            thumb_switch_ticks: pick(&self.thumb_switch_ticks, &over.thumb_switch_ticks),
            phantom_ping_ms: pick(&self.phantom_ping_ms, &over.phantom_ping_ms),
            phantom_jitter_ms: pick(&self.phantom_jitter_ms, &over.phantom_jitter_ms),
            aim_model: pick(&self.aim_model, &over.aim_model),
            foot_plan_ticks: pick(&self.foot_plan_ticks, &over.foot_plan_ticks),
            vehicle_plan_ticks: pick(&self.vehicle_plan_ticks, &over.vehicle_plan_ticks),
            vehicle_lookahead_ticks: pick(
                &self.vehicle_lookahead_ticks,
                &over.vehicle_lookahead_ticks,
            ),
            player_los_ticks: pick(&self.player_los_ticks, &over.player_los_ticks),
            bounce: match (&self.bounce, &over.bounce) {
                (Some(a), Some(b)) => Some(a.overlay(b)),
                (a, b) => pick(a, b),
            },
        }
    }

    /// What is wrong with it, if anything (`at` names it in the messages).
    pub fn problems(&self, at: &str) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(s) = &self.skill
            && let Err(e) = Skill::parse(s)
        {
            out.push(format!("{at}: {e}"));
        }
        let unit = [
            ("lead", self.lead),
            ("awareness", self.awareness),
            ("aggression", self.aggression),
            ("caution", self.caution),
            ("teamwork", self.teamwork),
            ("objective_focus", self.objective_focus),
        ];
        for (name, v) in unit {
            if let Some(v) = v
                && !(0.0..=1.0).contains(&v)
            {
                out.push(format!("{at}: {name} = {v}: 0 to 1"));
            }
        }
        let positive = [
            ("error", self.error),
            ("gain", self.gain),
            ("fire_cone_deg", self.fire_cone_deg),
            ("grenades_per_minute", self.grenades_per_minute),
        ];
        for (name, v) in positive {
            if let Some(v) = v
                && !(v.is_finite() && v >= 0.0)
            {
                out.push(format!("{at}: {name} = {v}: a number, 0 or more"));
            }
        }
        if let Some(c) = self.fire_cone_deg
            && c > 180.0
        {
            out.push(format!("{at}: fire_cone_deg = {c}: at most 180"));
        }
        if self.reaction.is_some_and(|r| r > 300) {
            out.push(format!("{at}: reaction: at most 300 ticks"));
        }
        for (name, value, lo, hi) in [
            (
                "perception_ticks",
                self.perception_ticks.map(|v| v as f32),
                0.0,
                30.0,
            ),
            ("motor_ticks", self.motor_ticks.map(|v| v as f32), 0.0, 30.0),
            ("button_sigma_ms", self.button_sigma_ms, 0.0, 12.0),
            ("stick_step", self.stick_step, 0.1, 0.6),
            (
                "thumb_switch_ticks",
                self.thumb_switch_ticks.map(|v| v as f32),
                1.0,
                6.0,
            ),
            ("phantom_ping_ms", self.phantom_ping_ms, 0.0, 1000.0),
            ("phantom_jitter_ms", self.phantom_jitter_ms, 0.0, 50.0),
        ] {
            if let Some(v) = value
                && !(lo..=hi).contains(&v)
            {
                out.push(format!("{at}: {name} = {v}: {lo} to {hi}"));
            }
        }
        if let Some(m) = &self.aim_model
            && !AIM_MODELS.contains(&m.as_str())
        {
            out.push(format!("{at}: aim_model = {m:?}: code or net"));
        }
        for (name, ticks) in [
            ("foot_plan_ticks", self.foot_plan_ticks),
            ("vehicle_plan_ticks", self.vehicle_plan_ticks),
            ("vehicle_lookahead_ticks", self.vehicle_lookahead_ticks),
            ("player_los_ticks", self.player_los_ticks),
        ] {
            if let Some(ticks) = ticks
                && !(1..=30).contains(&ticks)
            {
                out.push(format!("{at}: {name} = {ticks}: 1 to 30 ticks"));
            }
        }
        let on_off = self.burst_on.is_some() || self.burst_off.is_some();
        if self.burst == Some(false) && on_off {
            out.push(format!(
                "{at}: burst = false holds the trigger; burst_on/burst_off make no sense with it"
            ));
        }
        if self.burst_on == Some(0) {
            out.push(format!("{at}: burst_on: at least 1 tick"));
        }
        if let Some(b) = &self.bounce {
            out.extend(b.problems(at));
        }
        out
    }

    /// The skill, `normal` when unset (call after [`problems`](Self::problems)).
    pub fn skill(&self) -> Skill {
        self.skill
            .as_deref()
            .and_then(|s| Skill::parse(s).ok())
            .unwrap_or_default()
    }

    /// Reads a tuning file (TOML or JSON).
    pub fn load(path: &Path) -> Result<BotParams, String> {
        parse_file(path)
    }
}

/// One `[[bot]]` of a match file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BotSpec {
    /// Every bot's seat; kept so old match files that name it still read.
    #[serde(default)]
    pub kind: PhantomSeat,
    /// Shown in the report (default `Clanker <seat>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Overrides the tuning's skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    /// The team in a team game (0 red, 1 blue, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<u8>,
    /// Which arm of an A/B batch the bot plays for (set by the batch).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<String>,
    /// A tuning file, relative to the match file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<PathBuf>,
    /// Tuning inline; over the file's. Last: a TOML table follows the
    /// plain values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<BotParams>,
}

fn default_game_type() -> String {
    "slayer".into()
}

fn default_pace() -> String {
    "unpaced".into()
}

/// How much state a report records (the match and batch key `detail`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Detail {
    /// Per-tick digests, and components unless `components = false`.
    #[default]
    Full,
    /// No components; one digest row a game second and a rolling digest
    /// over every tick (`h2 sim diff` compares those).
    Training,
}

impl Detail {
    pub fn parse(s: &str) -> Result<Detail, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "full" => Ok(Detail::Full),
            "training" => Ok(Detail::Training),
            other => Err(format!("detail = \"{other}\": \"full\" or \"training\"")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Detail::Full => "full",
            Detail::Training => "training",
        }
    }
}

/// A match file's `bot_threads`: a worker count, or `"auto"` or `"off"`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BotThreads {
    Count(u32),
    Word(String),
}

impl MatchConfig {
    /// The `detail` key, `Full` when unset or invalid ([`resolve`] refuses
    /// an invalid one).
    pub fn detail(&self) -> Detail {
        self.detail
            .as_deref()
            .and_then(|d| Detail::parse(d).ok())
            .unwrap_or_default()
    }
}

impl BotThreads {
    /// The value for `H2_BOT_THREADS`.
    pub fn env_value(&self) -> String {
        match self {
            BotThreads::Count(n) => n.to_string(),
            BotThreads::Word(w) => w.trim().to_ascii_lowercase(),
        }
    }
}

/// A match file (docs/runtime/sim.md, "Match file").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MatchConfig {
    /// A multiplayer map on the disc, by file name (`lockout`).
    pub map: String,
    #[serde(default = "default_game_type")]
    pub game_type: String,
    /// Set by an `arm_swap` batch. True on the leg where every bot plays
    /// for the other arm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arms_swapped: Option<bool>,
    /// Seconds; 0 or unset: the type's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_limit_s: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_limit: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_play: Option<bool>,
    #[serde(default)]
    pub seed: u64,
    /// Stop after this many game ticks (30 a second).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_ticks: Option<u64>,
    /// Stop after this long on the wall clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_wall_s: Option<u64>,
    /// Run the world render every Nth world frame (0: never).
    #[serde(default)]
    pub render_every: u32,
    /// `unpaced`, `realtime` or `x<factor>`.
    #[serde(default = "default_pace")]
    pub pace: String,
    /// The superbounce practice: the superbounce mode on for the match,
    /// so every bot practises bounces instead of fighting
    /// (docs/runtime/sim.md, "Bounce practice"), and the report carries
    /// the bounce tally. (Before the tables: TOML
    /// writes plain values first.)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superbounce: Option<bool>,
    /// `false` leaves `state_components_v1` out of the report, for timing
    /// arms; the per-tick gameplay digests stay. Unset: recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub components: Option<bool>,
    /// How much of the state a report records: `"full"` (the default: the
    /// per-tick digests and components) or `"training"` (no components;
    /// sparse digests and a rolling digest over every tick).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Bot think workers for this process (`H2_BOT_THREADS`'s values): a
    /// count, `"auto"` or `"off"`. Unset: `h2 sim run` picks
    /// (docs/runtime/sim.md, "Bot thinking across cores"). The
    /// environment variable wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_threads: Option<BotThreads>,
    /// Overlap the thinks with the guest's tick (`H2_BOT_PIPELINE`): a
    /// different, repeatable match. Unset: off. The environment variable
    /// wins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_pipeline: Option<bool>,
    /// More variant settings, by id (`"83" = 1`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, i32>,
    /// The bounce drill: with `superbounce = true`, the bots try one site
    /// only, each try from a start near it with the word already at -3
    /// (docs/runtime/sim.md, "The bounce drill").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drill: Option<DrillConfig>,
    /// Bounce site training: this run is one shard of a batch
    /// (docs/runtime/sim.md, "Training the bounce sites").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub train: Option<crate::train::TrainConfig>,
    /// Seam drops: every seat dropped onto candidate collision edges with
    /// the pair word held at -3 (docs/runtime/sim.md, "Seam drops").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drops: Option<crate::drops::DropsConfig>,
    #[serde(default, rename = "bot")]
    pub bots: Vec<BotSpec>,
}

/// A match's `[drill]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DrillConfig {
    /// The site to try: the known site nearest it, within half a unit.
    pub site: [f32; 3],
    /// Where each try starts, primed. Unset: the site's approach (a
    /// drop's takeoff or launch floor), so a try is the jump alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<[f32; 3]>,
}

/// A bot as a run uses it: everything resolved. Every bot plays as a
/// phantom peer; there is no port or seat kind to resolve.
#[derive(Clone, Debug, PartialEq)]
pub struct Bot {
    pub seat: u8,
    pub name: String,
    pub skill: Skill,
    pub team: Option<u8>,
    /// The tuning file and the inline tuning, the skill folded in.
    pub params: BotParams,
    pub arm: Option<String>,
    /// The brain's random seed: from the match seed and the logical seat.
    pub seed: u64,
}

/// A match file, read and checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub config: MatchConfig,
    pub game_type: GameType,
    pub pace: Pace,
    pub bots: Vec<Bot>,
    /// Every variant setting the match writes, by id: the time limit, the
    /// score limit and team play first, then `settings`.
    pub variant: Vec<(u32, i32)>,
    pub team_play: bool,
    /// The bots practise superbounces ([`MatchConfig::superbounce`]).
    pub superbounce: bool,
}

impl Match {
    /// The match as JSON for the report: the file with every bot's tuning
    /// resolved inline (no paths), so a report says what was played.
    pub fn resolved_json(&self) -> serde_json::Value {
        let mut c = self.config.clone();
        c.game_type = self.game_type.name().into();
        c.team_play = Some(self.team_play);
        if c.detail.is_some() {
            c.detail = Some(self.config.detail().name().into());
        }
        c.bots = self
            .bots
            .iter()
            .map(|b| BotSpec {
                kind: PhantomSeat,
                name: Some(b.name.clone()),
                skill: Some(b.skill.name().into()),
                team: b.team,
                arm: b.arm.clone(),
                params: None,
                tuning: Some(b.params.clone()),
            })
            .collect();
        serde_json::to_value(c).unwrap_or(serde_json::Value::Null)
    }
}

/// The brain seed of the bot in logical `seat` in a match with `seed`
/// (SplitMix64 of the two, so neighbouring seeds and seats differ).
pub fn bot_seed(seed: u64, seat: u8) -> u64 {
    let mut x = seed ^ (u64::from(seat) + 1).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn parse_text<T: serde::de::DeserializeOwned>(text: &str, json: bool) -> Result<T, String> {
    if json {
        serde_json::from_str(text).map_err(|e| e.to_string())
    } else {
        toml::from_str(text).map_err(|e| e.to_string())
    }
}

fn is_json(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

pub(crate) fn parse_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_text(&text, is_json(path)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parses a match file's text (TOML, or JSON with `json`).
pub fn parse_match(text: &str, json: bool) -> Result<MatchConfig, String> {
    parse_text(text, json)
}

/// Reads and checks a match file; tuning files are read relative to it.
pub fn load_match(path: &Path) -> Result<Match, String> {
    let config: MatchConfig = parse_file(path)?;
    let base = path.parent().unwrap_or(Path::new("."));
    resolve(config, &|p: &Path| BotParams::load(&base.join(p)))
}

/// Checks `config` and resolves its bots; `read` loads a tuning file by
/// the path the match file gives. Every problem is listed.
pub fn resolve(
    mut config: MatchConfig,
    read: &dyn Fn(&Path) -> Result<BotParams, String>,
) -> Result<Match, String> {
    let mut problems = Vec::new();
    if let Some(d) = &config.drops {
        // A drops file names its map in the table or the match, has a
        // seat for every phantom unless it lists bots, plays with no
        // time limit as teams and keeps the report light.
        if config.map.trim().is_empty()
            && let Some(m) = &d.map
        {
            config.map = m.clone();
        }
        if config.bots.is_empty() {
            config.bots = d.seat_bots();
        }
        if config.time_limit_s.is_none() {
            config.time_limit_s = Some(0);
        }
        // Free-for-all sims end at tick 0 (a sim bug), so team play.
        if config.team_play.is_none() {
            config.team_play = Some(true);
        }
        if config.detail.is_none() {
            config.detail = Some(Detail::Training.name().into());
        }
    }
    if config.map.trim().is_empty() {
        problems.push("map: empty".to_string());
    }
    let game_type = GameType::parse(&config.game_type).unwrap_or_else(|e| {
        problems.push(format!("game_type: {e}"));
        GameType::Slayer
    });
    let pace = Pace::parse(&config.pace).unwrap_or_else(|e| {
        problems.push(format!("pace: {e}"));
        Pace::Unpaced
    });
    match &config.bot_threads {
        Some(BotThreads::Word(w))
            if !matches!(w.trim().to_ascii_lowercase().as_str(), "auto" | "off") =>
        {
            problems.push(format!("bot_threads = \"{w}\": a count, \"auto\" or \"off\""));
        }
        Some(BotThreads::Count(n)) if *n > 32 => {
            problems.push(format!("bot_threads = {n}: at most 32"));
        }
        _ => {}
    }
    if let Some(d) = &config.detail
        && let Err(e) = Detail::parse(d)
    {
        problems.push(e);
    }
    if config.detail() == Detail::Training && config.components == Some(true) {
        problems.push("components = true: a training report has no components".into());
    }
    if config.bots.is_empty() {
        problems.push("a match needs at least one bot".into());
    }
    if config.bots.len() > MAX_BOTS {
        problems.push("at most 15 phantoms: the host occupies one of 16 peers".into());
    }
    if config.max_ticks == Some(0) {
        problems.push("max_ticks = 0: leave it out for no limit".to_string());
    }
    let team_play = config.team_play.unwrap_or(false);
    let mut variant = Vec::new();
    if let Some(t) = config.time_limit_s {
        if t > 0x7fff {
            problems.push(format!("time_limit_s = {t}: at most 32767"));
        }
        variant.push((TIME_LIMIT_SETTING, t.min(0x7fff) as i32));
    }
    if let Some(s) = config.score_limit {
        variant.push((game_type.score_setting(), s.min(i32::MAX as u32) as i32));
    }
    if config.team_play.is_some() {
        variant.push((TEAM_PLAY_SETTING, i32::from(team_play)));
    }
    for (k, v) in &config.settings {
        match k.trim().parse::<u32>() {
            Ok(id) if id < MAX_SETTING => {
                if variant.iter().any(|(i, _)| *i == id) {
                    problems.push(format!("settings.{k}: set by the match's own fields too"));
                }
                variant.push((id, *v));
            }
            _ => problems.push(format!(
                "settings.{k}: a setting id, 0 to {}",
                MAX_SETTING - 1
            )),
        }
    }
    let mut bots = Vec::new();
    for (i, spec) in config.bots.iter().enumerate().take(MAX_BOTS) {
        let at = format!("bot {}", i + 1);
        let seat = i as u8;
        let file = match &spec.params {
            Some(p) => read(p).unwrap_or_else(|e| {
                problems.push(format!("{at}: params: {e}"));
                BotParams::default()
            }),
            None => BotParams::default(),
        };
        let mut params = match &spec.tuning {
            Some(t) => file.overlay(t),
            None => file,
        };
        if let Some(s) = &spec.skill {
            params.skill = Some(s.clone());
        }
        problems.extend(params.problems(&at));
        let skill = params.skill();
        params.skill = Some(skill.name().into());
        if let Some(t) = spec.team
            && t >= 8
        {
            problems.push(format!("{at}: team {t}: 0 to 7"));
        }
        let team = if team_play {
            Some(spec.team.unwrap_or(seat % 2))
        } else {
            spec.team
        };
        bots.push(Bot {
            seat,
            name: spec
                .name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| format!("Clanker {seat}")),
            skill,
            team,
            params,
            arm: spec.arm.clone(),
            seed: bot_seed(config.seed, seat),
        });
    }
    if team_play && bots.len() >= 2 {
        let first = bots[0].team;
        if bots.iter().all(|b| b.team == first) {
            problems.push("team_play: every bot is on one team; nobody to fight".to_string());
        }
    }
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    let superbounce = config.superbounce.unwrap_or(false);
    if let Some(d) = &config.drill {
        if !superbounce {
            problems.push("drill: needs superbounce = true".into());
        }
        let finite = |p: &[f32; 3]| p.iter().all(|c| c.is_finite());
        if !finite(&d.site) || d.start.is_some_and(|s| !finite(&s)) {
            problems.push("drill: site and start are world coordinates".into());
        }
        if !problems.is_empty() {
            return Err(problems.join("\n"));
        }
    }
    if let Some(t) = &config.train {
        problems.extend(t.problems(superbounce, config.drill.is_some()));
        if !problems.is_empty() {
            return Err(problems.join("\n"));
        }
    }
    if let Some(d) = &config.drops {
        problems.extend(d.problems(
            &config.map,
            superbounce,
            config.drill.is_some(),
            config.train.is_some(),
        ));
        if !problems.is_empty() {
            return Err(problems.join("\n"));
        }
    }
    Ok(Match {
        config,
        game_type,
        pace,
        bots,
        variant,
        team_play,
        superbounce,
    })
}

// ---- batches ----

/// One arm of an A/B batch.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Arm {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A tuning file, relative to the batch file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tuning: Option<BotParams>,
}

/// How a batch splits the bots between its arms.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// A and B in the same match: teams A and B in a team game, every
    /// other bot in free-for-all.
    #[default]
    Versus,
    /// Every match twice: all bots A, then all bots B.
    Separate,
    /// Versus, twice: the second leg gives every bot the other arm, so
    /// each arm plays every seat, team and brain seed once.
    ArmSwap,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Versus => "versus",
            Self::Separate => "separate",
            Self::ArmSwap => "arm_swap",
        }
    }

    pub fn parse(s: &str) -> Result<Mode, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "versus" | "vs" => Ok(Mode::Versus),
            "separate" => Ok(Mode::Separate),
            "arm_swap" => Ok(Mode::ArmSwap),
            _ => Err(format!(
                "unknown mode {s:?} (versus, separate, arm_swap)"
            )),
        }
    }
}

/// A batch file (docs/runtime/sim.md §7).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchConfig {
    /// The results directory, relative to the batch file.
    pub out: PathBuf,
    /// Processes at once (default: half the cores).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<usize>,
    /// The match file every run starts from, relative to the batch file.
    #[serde(rename = "match")]
    pub match_file: PathBuf,
    /// One run per map (default: the match's).
    #[serde(default)]
    pub maps: Vec<String>,
    /// One run per game type (default: the match's).
    #[serde(default)]
    pub game_types: Vec<String>,
    /// Write a coverage report per child and one merged report.
    #[serde(default)]
    pub audit: bool,
    /// One run per seed (default: the match's).
    #[serde(default)]
    pub seeds: Vec<u64>,
    /// One run per skill, every bot at it (default: the bots' own). The
    /// comparison then has a table per skill as well.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// `detail` for every run (`"full"` or `"training"`); over the match
    /// file's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// Legacy batch option. Only false is accepted.
    #[serde(default, skip_serializing)]
    pub fork: bool,
    pub a: Arm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<Arm>,
}

/// An arm with its tuning read.
#[derive(Clone, Debug, PartialEq)]
pub struct ArmParams {
    pub name: String,
    pub params: BotParams,
}

/// One run of a batch.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    /// `<map>[-<gametype>][-<skill>]-s<seed>` plus the arm name in
    /// separate mode or `-swapped` on a second leg. Used as the
    /// directory name under `<out>/runs/`.
    pub id: String,
    pub map: String,
    pub seed: u64,
    /// The match file the run plays: self-contained (tunings inline).
    pub config: MatchConfig,
}

/// A batch, read and planned.
#[derive(Clone, Debug, PartialEq)]
pub struct Batch {
    pub config: BatchConfig,
    pub out: PathBuf,
    pub mode: Mode,
    pub arms: Vec<ArmParams>,
    pub runs: Vec<Run>,
}

/// Reads a batch file, its match file and its arms' tuning files, and plans
/// its runs.
pub fn load_batch(path: &Path) -> Result<Batch, String> {
    let config: BatchConfig = parse_file(path)?;
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let match_path = base.join(&config.match_file);
    let base_match: MatchConfig = parse_file(&match_path)?;
    let match_dir = match_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let read_arm = |p: &Path| BotParams::load(&base.join(p));
    let read_bot = |p: &Path| BotParams::load(&match_dir.join(p));
    plan(config, base_match, &read_arm, &read_bot, &base)
}

/// Plans a batch's runs: maps × game types × skills × seeds (× arms in
/// separate mode, × two legs in the swap modes). Every run's match file
/// has its bots' tunings inline, the arm's over the bot's own, so a run
/// directory is self-contained.
pub fn plan(
    config: BatchConfig,
    base_match: MatchConfig,
    read_arm: &dyn Fn(&Path) -> Result<BotParams, String>,
    read_bot: &dyn Fn(&Path) -> Result<BotParams, String>,
    base: &Path,
) -> Result<Batch, String> {
    let mut problems = Vec::new();
    if config.fork {
        problems.push(
            "remove fork = true: phantom bots require a fresh System Link session per run".into(),
        );
    }
    let mode = match config.mode.as_deref() {
        Some(m) => Mode::parse(m).unwrap_or_else(|e| {
            problems.push(e);
            Mode::Versus
        }),
        None => Mode::Versus,
    };
    let arm = |a: &Arm, default: &str, problems: &mut Vec<String>| {
        let file = match &a.params {
            Some(p) => read_arm(p).unwrap_or_else(|e| {
                problems.push(format!("arm {default}: params: {e}"));
                BotParams::default()
            }),
            None => BotParams::default(),
        };
        let params = match &a.tuning {
            Some(t) => file.overlay(t),
            None => file,
        };
        problems.extend(params.problems(&format!("arm {default}")));
        ArmParams {
            name: a.name.clone().unwrap_or_else(|| default.to_string()),
            params,
        }
    };
    let mut arms = vec![arm(&config.a, "a", &mut problems)];
    if let Some(b) = &config.b {
        arms.push(arm(b, "b", &mut problems));
    }
    if arms.len() == 2 && arms[0].name == arms[1].name {
        problems.push(format!("arms a and b are both named {:?}", arms[0].name));
    }
    if base_match.bots.is_empty() {
        problems.push("the match file has no [[bot]]".to_string());
    }
    if mode == Mode::Versus && arms.len() == 2 && base_match.bots.len() < 2 {
        problems.push("versus needs at least two bots, one per arm".to_string());
    }
    if mode == Mode::ArmSwap && arms.len() < 2 {
        problems.push("arm_swap needs two arms, [a] and [b]".to_string());
    }
    if mode == Mode::ArmSwap && base_match.bots.len() < 2 {
        problems.push("arm_swap needs at least two bots, one per arm".to_string());
    }
    if config.parallel == Some(0) {
        problems.push("parallel = 0: at least 1".to_string());
    }
    let skills: Vec<Option<String>> = if config.skills.is_empty() {
        vec![None]
    } else {
        config
            .skills
            .iter()
            .map(|s| match Skill::parse(s) {
                Ok(skill) => Some(skill.name().to_string()),
                Err(e) => {
                    problems.push(format!("skills: {e}"));
                    None
                }
            })
            .collect()
    };
    // The bots' own tuning files, read once.
    let mut own = Vec::new();
    for (i, b) in base_match.bots.iter().enumerate() {
        let file = match &b.params {
            Some(p) => read_bot(p).unwrap_or_else(|e| {
                problems.push(format!("bot {}: params: {e}", i + 1));
                BotParams::default()
            }),
            None => BotParams::default(),
        };
        own.push(match &b.tuning {
            Some(t) => file.overlay(t),
            None => file,
        });
    }
    let maps = if config.maps.is_empty() {
        vec![base_match.map.clone()]
    } else {
        config.maps.clone()
    };
    let seeds = if config.seeds.is_empty() {
        vec![base_match.seed]
    } else {
        config.seeds.clone()
    };
    let game_types = if config.game_types.is_empty() {
        vec![(base_match.game_type.clone(), base_match.team_play)]
    } else {
        config
            .game_types
            .iter()
            .filter_map(|name| match GameType::parse(name) {
                Ok(kind) => {
                    let team_play = match kind {
                        GameType::Ctf | GameType::Assault => Some(true),
                        GameType::Juggernaut => Some(false),
                        _ => base_match.team_play,
                    };
                    Some((kind.name().to_string(), team_play))
                }
                Err(e) => {
                    problems.push(e);
                    None
                }
            })
            .collect()
    };
    let mut runs = Vec::new();
    let kinds: Vec<Option<usize>> = match (mode, arms.len()) {
        (Mode::Separate, 2) => vec![Some(0), Some(1)],
        _ => vec![None],
    };
    // Apply the selected arm's tuning and then the skill axis. In a
    // team game the versus split follows teams, including implicit ones.
    let lineup = |kind: Option<usize>, flip: bool, skill: &Option<String>, team_play: bool| {
        base_match
            .bots
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let mut spec = b.clone();
                let team = b.team.unwrap_or((i % 2) as u8);
                if team_play {
                    spec.team = Some(team);
                }
                let a = kind.unwrap_or_else(|| {
                    if arms.len() < 2 {
                        0
                    } else if team_play {
                        usize::from(team % 2)
                    } else {
                        i % 2
                    }
                });
                let a = if flip && arms.len() == 2 { 1 - a } else { a };
                let arm = &arms[a];
                spec.params = None;
                spec.tuning = Some(own[i].overlay(&arm.params));
                if let Some(s) = skill.as_ref().or(arm.params.skill.as_ref()) {
                    spec.skill = Some(s.clone());
                }
                spec.arm = Some(arm.name.clone());
                spec
            })
            .collect::<Vec<_>>()
    };
    for map in &maps {
        for (game_type, team_play) in &game_types {
            let name = if config.game_types.is_empty() {
                safe(map)
            } else {
                format!("{}-{game_type}", safe(map))
            };
            for skill in &skills {
                let name = match skill {
                    Some(s) => format!("{name}-{s}"),
                    None => name.clone(),
                };
                for &seed in &seeds {
                    for &kind in &kinds {
                        let id = match kind {
                            Some(a) => format!("{name}-s{seed}-{}", safe(&arms[a].name)),
                            None => format!("{name}-s{seed}"),
                        };
                        let mut c = base_match.clone();
                        c.map = map.clone();
                        c.game_type = game_type.clone();
                        c.team_play = *team_play;
                        c.seed = seed;
                        if config.detail.is_some() {
                            c.detail = config.detail.clone();
                        }
                        let team_play = team_play.unwrap_or(false);
                        c.bots = lineup(kind, false, skill, team_play);
                        if let Err(e) = resolve(c.clone(), &|_| Ok(BotParams::default())) {
                            problems.push(format!("run {id}: {}", e.replace('\n', "; ")));
                        }
                        let swapped_run = match mode {
                            Mode::ArmSwap => {
                                c.arms_swapped = Some(false);
                                let mut swapped = c.clone();
                                swapped.arms_swapped = Some(true);
                                swapped.bots = lineup(kind, true, skill, team_play);
                                Some(swapped)
                            }
                            Mode::Versus | Mode::Separate => None,
                        }
                        .map(|config| {
                            let id = format!("{id}-swapped");
                            if let Err(e) = resolve(config.clone(), &|_| Ok(BotParams::default())) {
                                problems.push(format!("run {id}: {e}"));
                            }
                            Run {
                                id,
                                map: map.clone(),
                                seed,
                                config,
                            }
                        });
                        runs.push(Run {
                            id,
                            map: map.clone(),
                            seed,
                            config: c,
                        });
                        runs.extend(swapped_run);
                    }
                }
            }
        }
    }
    let mut ids: Vec<&str> = runs.iter().map(|r| r.id.as_str()).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|w| w[0] == w[1]) {
        problems.push("two runs share an id: repeated maps, game types, skills or seeds".to_string());
    }
    if !problems.is_empty() {
        problems.dedup();
        return Err(problems.join("\n"));
    }
    let out = base.join(&config.out);
    Ok(Batch {
        config,
        out,
        mode,
        arms,
        runs,
    })
}

/// `[A-Za-z0-9_-]` of a name, for directory names.
pub fn safe(s: &str) -> String {
    let t: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if t.is_empty() { "_".into() } else { t }
}

#[cfg(test)]
mod tests;
