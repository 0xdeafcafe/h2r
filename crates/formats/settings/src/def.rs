//! What a setting is: [`Def`] and the values it may hold.

use std::fmt;
use std::ops::RangeInclusive;

/// A setting's value. Choices are stored as their number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i64),
}

impl Value {
    pub fn as_bool(self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(b),
            Value::Int(_) => None,
        }
    }

    pub fn as_int(self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(i),
            Value::Bool(_) => None,
        }
    }

    /// The value as the handles hold it: 0 or 1 for a bool.
    pub(crate) fn bits(self) -> i64 {
        match self {
            Value::Bool(b) => i64::from(b),
            Value::Int(i) => i,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
        }
    }
}

/// How an integer reads on a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// `90`.
    Plain,
    /// `90` (the row's label says degrees).
    Degrees,
    /// `2x`.
    Times,
    /// A multiple of a size, shown as the size it gives: `1280x960` for 2
    /// of 640 by 480.
    Pixels { width: u32, height: u32 },
}

/// One value of a [`Kind::Choice`]: its number, how the file, the
/// environment and the CLI spell it, and how a row shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    pub value: i64,
    pub name: &'static str,
    pub label: &'static str,
}

/// What values a setting may hold, and what a row press steps through.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// On or off; a press flips it.
    Bool,
    /// A whole number in `valid` (what the file and the environment may
    /// hold). A press goes to the first of `steps` above the value shown,
    /// wrapping to the first step.
    Int {
        valid: RangeInclusive<i64>,
        steps: &'static [i64],
        unit: Unit,
    },
    /// One of a list, stepped in order. Its name is stored as a TOML float
    /// for canonical decimal names, an integer when it spells the choice's number,
    /// or a string.
    Choice(&'static [Choice]),
}

/// When a change takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    /// At once: the handles see the new value.
    Live,
    /// At the next map load. The running and saved values are kept apart,
    /// as for [`Apply::Restart`].
    NextMap,
    /// At the next launch: the handles keep the value the run started with
    /// and a row shows "(restart)" while the saved value differs.
    Restart,
}

/// What a setting can change (docs/engine/feature-engine.md "Determinism").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Never on the guest thread, never writes guest memory.
    Host,
    /// On the guest thread or writes guest memory; presentation only.
    View,
    /// Changes what the simulation computes or is given.
    Sim,
}

/// Who it is for: which surfaces list it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    User,
    Advanced,
    Diagnostic,
}

/// Where a setting's row is: a screen id and its order there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Menu {
    pub screen: &'static str,
    pub order: u32,
}

/// Reads a value from text.
pub type ParseFn = fn(&str) -> Result<Value, String>;

/// One setting.
#[derive(Clone, Debug)]
pub struct Def {
    /// `table.key` in `settings.toml`: `"graphics.fov"`.
    pub key: &'static str,
    /// The row's label.
    pub label: &'static str,
    pub help: &'static str,
    pub kind: Kind,
    pub default: Value,
    /// The variable that overrides the file, and locks the row.
    pub env: Option<&'static str>,
    /// A dedicated `h2 play` flag that sets it (`--fps`); any setting can
    /// also be given as `--set key=value`.
    pub flag: Option<&'static str>,
    /// Other spellings accepted from the file, the environment and flags:
    /// `("0", Value::Int(70))` makes `0` mean 70.
    pub aliases: &'static [(&'static str, Value)],
    /// Reads the environment variable instead of [`Def::parse`], for a
    /// variable that says more than this setting (`H2_PAD_SPEAKER` may name
    /// a file).
    pub parse_env: Option<ParseFn>,
    pub apply: Apply,
    pub scope: Scope,
    pub tier: Tier,
    pub menu: Option<Menu>,
}

impl Def {
    /// The TOML table and the key in it.
    pub fn table_key(&self) -> (&'static str, &'static str) {
        self.key.split_once('.').unwrap_or(("", self.key))
    }

    /// `v`, if this setting may hold it.
    pub fn check(&self, v: Value) -> Result<Value, String> {
        let ok = match (&self.kind, v) {
            (Kind::Bool, Value::Bool(_)) => true,
            (Kind::Int { valid, .. }, Value::Int(i)) => valid.contains(&i),
            (Kind::Choice(c), Value::Int(i)) => c.iter().any(|c| c.value == i),
            _ => false,
        };
        if ok {
            Ok(v)
        } else {
            Err(format!("{}: expected {}", self.key, self.expected()))
        }
    }

    /// What the setting may hold, in words.
    pub fn expected(&self) -> String {
        match &self.kind {
            Kind::Bool => "0 or 1".into(),
            Kind::Int { valid, .. } => {
                format!("a whole number from {} to {}", valid.start(), valid.end())
            }
            Kind::Choice(c) => {
                let names: Vec<&str> = c.iter().map(|c| c.name).collect();
                match names.split_last() {
                    Some((last, [])) => (*last).to_string(),
                    Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
                    None => "nothing".into(),
                }
            }
        }
    }

    /// A value from text (the environment, a flag, `h2 settings set`):
    /// trimmed; an alias, else the kind's own spelling.
    pub fn parse(&self, text: &str) -> Result<Value, String> {
        let t = text.trim();
        if let Some((_, v)) = self.aliases.iter().find(|(a, _)| a.eq_ignore_ascii_case(t)) {
            return Ok(*v);
        }
        let v = match &self.kind {
            Kind::Bool => match t.to_ascii_lowercase().as_str() {
                "1" | "true" | "on" | "yes" => Some(Value::Bool(true)),
                "0" | "false" | "off" | "no" => Some(Value::Bool(false)),
                _ => None,
            },
            Kind::Int { valid, .. } => t
                .parse::<i64>()
                .ok()
                .filter(|i| valid.contains(i))
                .map(Value::Int),
            Kind::Choice(c) => c
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(t))
                .map(|c| Value::Int(c.value)),
        };
        v.ok_or_else(|| format!("{t:?}: expected {}", self.expected()))
    }

    /// The environment variable's text, through [`Def::parse_env`] when
    /// there is one.
    pub fn parse_env_text(&self, text: &str) -> Result<Value, String> {
        match self.parse_env {
            Some(f) => f(text),
            None => self.parse(text),
        }
    }

    /// A value read from `settings.toml`. A bool must be a TOML boolean;
    /// an integer a TOML integer. Choices accept their name as text or a number.
    pub fn from_toml(&self, v: &toml::Value) -> Result<Value, String> {
        let text = match (&self.kind, v) {
            (Kind::Bool, toml::Value::Boolean(b)) => return Ok(Value::Bool(*b)),
            (Kind::Bool, _) => return Err("must be true or false".into()),
            (Kind::Int { .. } | Kind::Choice(_), toml::Value::Integer(i)) => i.to_string(),
            // Keep the decimal point for whole floats: an integer choice
            // such as "60" still requires a TOML integer or string.
            (Kind::Choice(_), toml::Value::Float(f)) => format!("{f:?}"),
            (Kind::Choice(_), toml::Value::String(s)) => s.clone(),
            (Kind::Int { .. }, _) => return Err("must be a whole number".into()),
            (Kind::Choice(_), _) => return Err(format!("must be {}", self.expected())),
        };
        self.parse(&text)
            .map_err(|_| format!("must be {}", self.expected()))
    }

    /// `v` as `settings.toml` holds it.
    pub fn to_toml(&self, v: Value) -> toml::Value {
        match (&self.kind, v) {
            (_, Value::Bool(b)) => toml::Value::Boolean(b),
            (Kind::Choice(c), Value::Int(i)) => match c.iter().find(|c| c.value == i) {
                Some(c) if c.name.parse::<i64>().ok() != Some(i) => match c.name.parse::<f64>() {
                    Ok(f) if f.is_finite() && format!("{f:?}") == c.name => toml::Value::Float(f),
                    _ => toml::Value::String(c.name.into()),
                },
                _ => toml::Value::Integer(i),
            },
            (_, Value::Int(i)) => toml::Value::Integer(i),
        }
    }

    /// `v` as the environment, the CLI and the log spell it: `uncapped`,
    /// `90`, `true`.
    pub fn name(&self, v: Value) -> String {
        match (&self.kind, v) {
            (Kind::Choice(c), Value::Int(i)) => c
                .iter()
                .find(|c| c.value == i)
                .map_or_else(|| i.to_string(), |c| c.name.to_string()),
            _ => v.to_string(),
        }
    }

    /// `v` as a row shows it: `Uncapped`, `2x`, `1280x960`, `90`, `On`.
    pub fn label(&self, v: Value) -> String {
        match (&self.kind, v) {
            (_, Value::Bool(b)) => if b { "On" } else { "Off" }.into(),
            (Kind::Choice(c), Value::Int(i)) => c
                .iter()
                .find(|c| c.value == i)
                .map_or_else(|| i.to_string(), |c| c.label.to_string()),
            (
                Kind::Int {
                    unit: Unit::Times, ..
                },
                Value::Int(i),
            ) => format!("{i}x"),
            (
                Kind::Int {
                    unit: Unit::Pixels { width, height },
                    ..
                },
                Value::Int(i),
            ) => format!("{}x{}", i64::from(*width) * i, i64::from(*height) * i),
            (_, Value::Int(i)) => i.to_string(),
        }
    }

    /// The value after `shown` on a row press.
    pub fn next(&self, shown: Value) -> Value {
        match (&self.kind, shown) {
            (_, Value::Bool(b)) => Value::Bool(!b),
            (Kind::Int { steps, .. }, Value::Int(i)) => {
                Value::Int(steps.iter().copied().find(|s| *s > i).unwrap_or(steps[0]))
            }
            (Kind::Choice(c), Value::Int(i)) => {
                let at = c.iter().position(|c| c.value == i);
                Value::Int(at.map_or(c[0].value, |p| c[(p + 1) % c.len()].value))
            }
            (Kind::Bool, Value::Int(_)) => self.default,
        }
    }

    /// Every value the setting may hold, in order: what a row must have a
    /// string for.
    pub fn values(&self) -> Vec<Value> {
        match &self.kind {
            Kind::Bool => vec![Value::Bool(true), Value::Bool(false)],
            Kind::Int { valid, .. } => valid.clone().map(Value::Int).collect(),
            Kind::Choice(c) => c.iter().map(|c| Value::Int(c.value)).collect(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const STEPS: &[i64] = &[70, 75, 80, 85, 90, 95, 100, 105, 110, 115, 120];

    pub static FOV: Def = Def {
        key: "graphics.fov",
        label: "Field of View",
        help: "",
        kind: Kind::Int {
            valid: 30..=150,
            steps: STEPS,
            unit: Unit::Degrees,
        },
        default: Value::Int(70),
        env: Some("H2_FOV"),
        flag: None,
        aliases: &[("0", Value::Int(70))],
        parse_env: None,
        apply: Apply::Live,
        scope: Scope::View,
        tier: Tier::User,
        menu: Some(Menu {
            screen: "video",
            order: 10,
        }),
    };

    pub static FPS: Def = Def {
        key: "graphics.fps",
        label: "Frame Rate",
        help: "",
        kind: Kind::Choice(&[
            Choice {
                value: 30,
                name: "30",
                label: "30",
            },
            Choice {
                value: 60,
                name: "60",
                label: "60",
            },
            Choice {
                value: 120,
                name: "120",
                label: "120",
            },
            Choice {
                value: 0,
                name: "uncapped",
                label: "Uncapped",
            },
        ]),
        default: Value::Int(30),
        env: Some("H2_FPS"),
        flag: Some("--fps"),
        aliases: &[("unlimited", Value::Int(0)), ("max", Value::Int(0))],
        parse_env: None,
        apply: Apply::Restart,
        scope: Scope::Host,
        tier: Tier::User,
        menu: Some(Menu {
            screen: "video",
            order: 20,
        }),
    };

    pub static SPEAKER: Def = Def {
        key: "pad_speaker.enabled",
        label: "Controller Speaker",
        help: "",
        kind: Kind::Bool,
        default: Value::Bool(true),
        env: Some("H2_PAD_SPEAKER"),
        flag: None,
        aliases: &[],
        parse_env: None,
        apply: Apply::Live,
        scope: Scope::Host,
        tier: Tier::User,
        menu: None,
    };

    #[test]
    fn text_parses_as_each_kind_spells_it() {
        assert_eq!(FOV.parse(" 93 "), Ok(Value::Int(93)));
        assert_eq!(FOV.parse("0"), Ok(Value::Int(70)), "the alias");
        assert!(FOV.parse("20").unwrap_err().contains("30 to 150"));
        assert!(FOV.parse("-5").is_err());
        assert!(FOV.parse("wide").is_err());
        assert_eq!(FPS.parse("Uncapped"), Ok(Value::Int(0)));
        assert_eq!(FPS.parse("MAX"), Ok(Value::Int(0)));
        assert_eq!(FPS.parse("60"), Ok(Value::Int(60)));
        assert!(FPS.parse("0").is_err(), "uncapped is spelled, not 0");
        assert!(
            FPS.parse("45")
                .unwrap_err()
                .contains("30, 60, 120 or uncapped")
        );
        assert_eq!(SPEAKER.parse("0"), Ok(Value::Bool(false)));
        assert_eq!(SPEAKER.parse("on"), Ok(Value::Bool(true)));
        assert!(SPEAKER.parse("maybe").is_err());
    }

    #[test]
    fn toml_reads_and_writes_as_today() {
        let t = |s: &str| -> toml::Value { s.parse::<toml::Table>().unwrap()["v"].clone() };
        assert_eq!(FPS.from_toml(&t("v = 60")), Ok(Value::Int(60)));
        assert_eq!(FPS.from_toml(&t("v = \"uncapped\"")), Ok(Value::Int(0)));
        assert_eq!(FPS.from_toml(&t("v = \"60\"")), Ok(Value::Int(60)));
        assert!(FPS.from_toml(&t("v = 0")).is_err());
        assert!(FPS.from_toml(&t("v = 45")).is_err());
        assert_eq!(FOV.from_toml(&t("v = 0")), Ok(Value::Int(70)));
        assert!(
            FOV.from_toml(&t("v = 90.5"))
                .unwrap_err()
                .contains("whole number")
        );
        assert!(FOV.from_toml(&t("v = \"90\"")).is_err());
        assert!(
            SPEAKER
                .from_toml(&t("v = \"yes\""))
                .unwrap_err()
                .contains("true or false")
        );
        assert_eq!(
            FPS.to_toml(Value::Int(0)),
            toml::Value::String("uncapped".into())
        );
        assert_eq!(FPS.to_toml(Value::Int(60)), toml::Value::Integer(60));
        assert_eq!(
            SPEAKER.to_toml(Value::Bool(false)),
            toml::Value::Boolean(false)
        );
    }

    #[test]
    fn presses_step_and_wrap() {
        let mut v = FOV.default;
        let mut seen = vec![v];
        for _ in 0..11 {
            v = FOV.next(v);
            seen.push(v);
        }
        let ints: Vec<i64> = seen.iter().map(|v| v.as_int().unwrap()).collect();
        assert_eq!(ints, [70, 75, 80, 85, 90, 95, 100, 105, 110, 115, 120, 70]);
        // Off the steps: the next step above, else the first.
        for (from, to) in [(93, 95), (119, 120), (121, 70), (150, 70), (40, 70)] {
            assert_eq!(FOV.next(Value::Int(from)), Value::Int(to), "{from}");
        }
        let mut v = FPS.default;
        let mut seen = vec![v];
        for _ in 0..4 {
            v = FPS.next(v);
            seen.push(v);
        }
        let ints: Vec<i64> = seen.iter().map(|v| v.as_int().unwrap()).collect();
        assert_eq!(ints, [30, 60, 120, 0, 30]);
        assert_eq!(SPEAKER.next(Value::Bool(true)), Value::Bool(false));
    }

    #[test]
    fn labels_and_names() {
        assert_eq!(FPS.label(Value::Int(0)), "Uncapped");
        assert_eq!(FPS.name(Value::Int(0)), "uncapped");
        assert_eq!(FOV.label(Value::Int(95)), "95");
        assert_eq!(SPEAKER.label(Value::Bool(false)), "Off");
        let scale = Def {
            kind: Kind::Int {
                valid: 1..=6,
                steps: &[1, 2, 3, 4],
                unit: Unit::Pixels {
                    width: 640,
                    height: 480,
                },
            },
            ..FOV
        };
        let shown: Vec<String> = (1..=4).map(|i| scale.label(Value::Int(i))).collect();
        assert_eq!(shown, ["640x480", "1280x960", "1920x1440", "2560x1920"]);
        assert_eq!(scale.label(Value::Int(6)), "3840x2880");
        assert_eq!(
            scale.name(Value::Int(2)),
            "2",
            "the file and CLI keep the scale"
        );
        assert_eq!(FOV.values().len(), 121);
        assert_eq!(FPS.values().len(), 4);
        assert_eq!(FOV.table_key(), ("graphics", "fov"));
        assert_eq!(
            FPS.check(Value::Int(45)).unwrap_err(),
            "graphics.fps: expected 30, 60, 120 or uncapped"
        );
        assert!(FOV.check(Value::Bool(true)).is_err());
    }
}
