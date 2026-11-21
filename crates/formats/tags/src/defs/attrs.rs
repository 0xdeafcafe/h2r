//! The editor attributes of a definition: units, titles and the words that
//! give a label or an axis. Each has a closed word list or a single-line
//! text check, so a wrong word fails the parse
//! (`docs/formats/tag-definitions.md`, "Editor attributes").

/// The unit of a numeric field (`unit=<word>`). The value is stored in the
/// tag as this unit says; `Degrees` and `DegreesPerSecond` are stored in
/// radians and shown in degrees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Unit {
    Seconds,
    Ticks,
    WorldUnits,
    WorldUnitsPerSecond,
    Degrees,
    DegreesPerSecond,
    Fraction,
    Percent,
    Pixels,
    Count,
    Index,
}

impl Unit {
    /// Every unit, in the order the docs list them.
    pub const ALL: [Unit; 11] = [
        Unit::Seconds,
        Unit::Ticks,
        Unit::WorldUnits,
        Unit::WorldUnitsPerSecond,
        Unit::Degrees,
        Unit::DegreesPerSecond,
        Unit::Fraction,
        Unit::Percent,
        Unit::Pixels,
        Unit::Count,
        Unit::Index,
    ];

    /// The word the definition files use.
    pub fn word(self) -> &'static str {
        match self {
            Unit::Seconds => "seconds",
            Unit::Ticks => "ticks",
            Unit::WorldUnits => "world_units",
            Unit::WorldUnitsPerSecond => "world_units_per_second",
            Unit::Degrees => "degrees",
            Unit::DegreesPerSecond => "degrees_per_second",
            Unit::Fraction => "fraction",
            Unit::Percent => "percent",
            Unit::Pixels => "pixels",
            Unit::Count => "count",
            Unit::Index => "index",
        }
    }

    /// The unit a definition word names.
    pub fn from_word(word: &str) -> Option<Unit> {
        Unit::ALL.into_iter().find(|u| u.word() == word)
    }

    /// Whether the value is an angle stored in radians.
    pub fn is_angle(self) -> bool {
        matches!(self, Unit::Degrees | Unit::DegreesPerSecond)
    }
}

/// What names an element of a block, or the kind of a group or struct
/// (`title=`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Title {
    /// Fixed words: the kind of a group or struct (`Weapon`).
    Text(String),
    /// The element field whose value titles each element of a block.
    Field(String),
}

/// The words a group's `applies=` may take, the live-write timings.
pub fn applies_word(word: &str) -> Option<&'static str> {
    ["frame", "use", "creation"]
        .into_iter()
        .find(|w| *w == word)
}

/// Whether `text` is fit for a label or title: not empty, no space at
/// either end, no double space, no control character.
pub fn check_text(text: &str) -> Result<(), &'static str> {
    if text.is_empty() {
        Err("is empty")
    } else if text.trim() != text {
        Err("has a space at an end")
    } else if text.contains("  ") {
        Err("has a double space")
    } else if text.chars().any(char::is_control) {
        Err("has a control character")
    } else {
        Ok(())
    }
}

/// Whether `word` can stand as an axis letter or a field name: ASCII
/// letters, digits and underscores, not starting with a digit.
pub fn check_word(word: &str) -> bool {
    let mut chars = word.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}
