//! Keyboard+mouse binding table (serde TOML).
//!
//! ```toml
//! [convert]                 # optional, shared deadzone settings
//! stick_deadzone = 0.08
//! trigger_deadzone = 0.03
//!
//! [mouse]                   # optional
//! look = true               # mouse motion drives the right stick
//! sensitivity = 0.04        # stick deflection per mouse count per 1/60 s
//! invert_y = false
//!
//! [bind]                    # control = [inputs]; omitted controls keep the defaults
//! a = ["Space"]
//! right_trigger = ["MouseLeft"]
//! left_stick_up = ["W"]
//! ```
//!
//! Input names are case-insensitive and follow W3C `KeyboardEvent.code`
//! (`KeyW`/`W`, `Digit1`/`1`, `ArrowUp`, `ShiftLeft`, `Space`, `Escape`, ...),
//! plus `MouseLeft`, `MouseRight`, `MouseMiddle`, `MouseBack`,
//! `MouseForward`. Several inputs can be bound to one control. A `[bind]`
//! entry replaces that control's default list (an empty list unbinds it).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::host::{ConfigError, ConvertConfig};

/// A control on the virtual Xbox pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    A,
    B,
    X,
    Y,
    Black,
    White,
    LeftTrigger,
    RightTrigger,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    Start,
    Back,
    LeftThumb,
    RightThumb,
    LeftStickUp,
    LeftStickDown,
    LeftStickLeft,
    LeftStickRight,
    RightStickUp,
    RightStickDown,
    RightStickLeft,
    RightStickRight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

/// One physical input a control can be bound to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Input {
    /// Canonical lowercase key name, see [`canonical_key`].
    Key(String),
    Mouse(MouseButton),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum BindingError {
    #[error("unknown input name `{0}`")]
    UnknownInput(String),
    #[error("invalid bindings TOML: {0}")]
    Toml(String),
    #[error("invalid value: {0}")]
    Invalid(#[from] ConfigError),
}

const NAMED_KEYS: &[&str] = &[
    "space", "enter", "escape", "tab", "backspace", "capslock", "delete", "insert", "home", "end",
    "pageup", "pagedown", "arrowup", "arrowdown", "arrowleft", "arrowright", "shiftleft",
    "shiftright", "controlleft", "controlright", "altleft", "altright", "comma", "period", "slash",
    "semicolon", "quote", "bracketleft", "bracketright", "backslash", "minus", "equal", "backquote",
];

/// Canonical (lowercase, no `Key`/`Digit` prefix) key name, or `None` if the
/// name is not a key we know.
pub fn canonical_key(name: &str) -> Option<String> {
    let l = name.trim().to_ascii_lowercase();
    let l = match l.strip_prefix("key") {
        Some(r) if r.len() == 1 => r.to_string(),
        _ => l,
    };
    let l = match l.strip_prefix("digit") {
        Some(r) if r.len() == 1 => r.to_string(),
        _ => l,
    };
    let single = l.len() == 1 && l.as_bytes()[0].is_ascii_alphanumeric();
    let func = l
        .strip_prefix('f')
        .and_then(|n| n.parse::<u8>().ok())
        .is_some_and(|n| (1..=12).contains(&n));
    (single || func || NAMED_KEYS.contains(&l.as_str())).then_some(l)
}

impl std::str::FromStr for Input {
    type Err = BindingError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let l = s.trim().to_ascii_lowercase();
        let m = match l.as_str() {
            "mouseleft" => Some(MouseButton::Left),
            "mouseright" => Some(MouseButton::Right),
            "mousemiddle" => Some(MouseButton::Middle),
            "mouseback" => Some(MouseButton::Back),
            "mouseforward" => Some(MouseButton::Forward),
            _ => None,
        };
        if let Some(m) = m {
            return Ok(Input::Mouse(m));
        }
        canonical_key(s).map(Input::Key).ok_or_else(|| BindingError::UnknownInput(s.to_string()))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MouseConfig {
    pub look: bool,
    pub sensitivity: f32,
    pub invert_y: bool,
}

impl MouseConfig {
    /// Sensitivity must be finite and non-negative.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.sensitivity.is_finite() && self.sensitivity >= 0.0 {
            Ok(())
        } else {
            Err(ConfigError { field: "mouse.sensitivity", expected: "a finite number >= 0" })
        }
    }
}

impl Default for MouseConfig {
    fn default() -> Self {
        MouseConfig { look: true, sensitivity: 0.04, invert_y: false }
    }
}

/// Parsed binding table.
#[derive(Clone, Debug, PartialEq)]
pub struct Bindings {
    pub convert: ConvertConfig,
    pub mouse: MouseConfig,
    pub bind: BTreeMap<Control, Vec<Input>>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct Raw {
    convert: ConvertConfig,
    mouse: MouseConfig,
    bind: BTreeMap<Control, Vec<String>>,
}

impl Default for Bindings {
    /// Halo-oriented defaults: WASD + mouse, Space jump, left mouse fire.
    fn default() -> Self {
        use Control::*;
        let table: &[(Control, &[&str])] = &[
            (A, &["Space"]),
            (B, &["V", "MouseMiddle"]),
            (X, &["E"]),
            (Y, &["Q"]),
            (Black, &["F"]),
            (White, &["R"]),
            (LeftTrigger, &["G", "MouseBack"]),
            (RightTrigger, &["MouseLeft"]),
            (DpadUp, &["ArrowUp"]),
            (DpadDown, &["ArrowDown"]),
            (DpadLeft, &["ArrowLeft"]),
            (DpadRight, &["ArrowRight"]),
            (Start, &["Escape"]),
            (Back, &["Tab"]),
            (LeftThumb, &["ControlLeft", "C"]),
            (RightThumb, &["MouseRight"]),
            (LeftStickUp, &["W"]),
            (LeftStickDown, &["S"]),
            (LeftStickLeft, &["A"]),
            (LeftStickRight, &["D"]),
        ];
        Bindings {
            convert: ConvertConfig::default(),
            mouse: MouseConfig::default(),
            bind: table
                .iter()
                .map(|(c, names)| (*c, names.iter().map(|n| n.parse().expect("default binding")).collect()))
                .collect(),
        }
    }
}

impl Bindings {
    /// Parse a TOML table on top of the defaults.
    pub fn from_toml(text: &str) -> Result<Self, BindingError> {
        let raw: Raw = toml::from_str(text).map_err(|e| BindingError::Toml(e.to_string()))?;
        raw.convert.validate()?;
        raw.mouse.validate()?;
        let mut b = Bindings { convert: raw.convert, mouse: raw.mouse, ..Default::default() };
        for (c, names) in raw.bind {
            let inputs = names.iter().map(|n| n.parse()).collect::<Result<Vec<Input>, _>>()?;
            b.bind.insert(c, inputs);
        }
        Ok(b)
    }

    pub fn inputs(&self, c: Control) -> &[Input] {
        self.bind.get(&c).map_or(&[], Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names() {
        assert_eq!("KeyW".parse::<Input>(), Ok(Input::Key("w".into())));
        assert_eq!("digit1".parse::<Input>(), Ok(Input::Key("1".into())));
        assert_eq!("F12".parse::<Input>(), Ok(Input::Key("f12".into())));
        assert_eq!("ShiftLeft".parse::<Input>(), Ok(Input::Key("shiftleft".into())));
        assert_eq!("MouseRight".parse::<Input>(), Ok(Input::Mouse(MouseButton::Right)));
        assert!("F13".parse::<Input>().is_err());
        assert!("Banana".parse::<Input>().is_err());
    }

    #[test]
    fn defaults_cover_every_button_and_left_stick() {
        let b = Bindings::default();
        assert_eq!(b.inputs(Control::A), &[Input::Key("space".into())]);
        assert_eq!(b.inputs(Control::LeftStickUp).len(), 1);
        // Right stick is driven by the mouse by default.
        assert!(b.inputs(Control::RightStickUp).is_empty());
    }

    #[test]
    fn toml_overrides_defaults_per_control() {
        let b = Bindings::from_toml(
            r#"
            [convert]
            stick_deadzone = 0.2
            [mouse]
            sensitivity = 0.1
            invert_y = true
            [bind]
            a = ["Enter", "KeyJ"]
            b = []
            right_stick_left = ["ArrowLeft"]
            "#,
        )
        .unwrap();
        assert_eq!(b.convert.stick_deadzone, 0.2);
        assert_eq!(b.convert.trigger_deadzone, ConvertConfig::default().trigger_deadzone);
        assert_eq!(b.mouse.sensitivity, 0.1);
        assert!(b.mouse.invert_y && b.mouse.look);
        assert_eq!(b.inputs(Control::A), &[Input::Key("enter".into()), Input::Key("j".into())]);
        assert!(b.inputs(Control::B).is_empty());
        assert_eq!(b.inputs(Control::X), &[Input::Key("e".into())]); // untouched default
        assert_eq!(b.inputs(Control::RightStickLeft).len(), 1);
    }

    #[test]
    fn config_values_are_validated() {
        for bad in [
            "[convert]\nstick_deadzone = 1.0",
            "[convert]\nstick_deadzone = -0.5",
            "[convert]\ntrigger_deadzone = nan",
            "[convert]\ntrigger_deadzone = inf",
            "[mouse]\nsensitivity = -1.0",
            "[mouse]\nsensitivity = nan",
        ] {
            assert!(matches!(Bindings::from_toml(bad), Err(BindingError::Invalid(_))), "{bad}");
        }
        assert!(Bindings::from_toml("[convert]\nstick_deadzone = 0.0").is_ok());
        // Unknown keys inside [convert] and [mouse] are rejected too.
        assert!(matches!(Bindings::from_toml("[convert]\nstick_deadzon = 0.1"), Err(BindingError::Toml(_))));
        assert!(matches!(Bindings::from_toml("[mouse]\nlok = false"), Err(BindingError::Toml(_))));
    }

    #[test]
    fn bad_toml_is_rejected() {
        assert!(matches!(Bindings::from_toml("[bind]\na = [\"Nope\"]"), Err(BindingError::UnknownInput(_))));
        assert!(matches!(Bindings::from_toml("[bind]\nzz = [\"W\"]"), Err(BindingError::Toml(_))));
        assert!(matches!(Bindings::from_toml("bogus = 1"), Err(BindingError::Toml(_))));
    }
}
