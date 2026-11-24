//! The game types a script names with its `type` line: a closed table.
//!
//! A type is a category, not an engine. It picks the row of the game type
//! list a script's variants are filed under, the announcer's line when the
//! game starts, and what a point is called when the script gives no
//! `points` line. Two scripts of one type share nothing else. See
//! `docs/game/game-types-without-carriers.md`, "The header".

/// A game type ([`TABLE`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Type {
    Slayer,
    Ctf,
    Assault,
    Oddball,
    Koth,
    Juggernaut,
    Territories,
    Research,
    Party,
    Firefight,
    Siege,
}

/// Where a type's line at the start of a game comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartLine {
    /// The retail announcer line, played from the disc.
    Retail,
    /// A clip of ours (`art/announcer/`).
    Ours,
    /// No line.
    None,
}

/// One row of [`TABLE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub ty: Type,
    /// The word the `type` line takes.
    pub name: &'static str,
    /// The game type list's row, as a player reads it.
    pub label: &'static str,
    /// What a point is when the script has no `points` line.
    pub points: &'static str,
    pub start: StartLine,
    /// The retail engine index a record of this type is on. Every script
    /// record moves to index 2 (Slayer) in GT2 of the design page; until
    /// then a retail type keeps its own engine's index and our types ride
    /// Slayer's.
    pub engine: u8,
}

/// Every game type, in the game type list's order.
pub const TABLE: &[Row] = &[
    row(
        Type::Slayer,
        "slayer",
        "Slayer",
        "Kills",
        StartLine::Retail,
        2,
    ),
    row(
        Type::Ctf,
        "ctf",
        "Capture the Flag",
        "Captures",
        StartLine::Retail,
        1,
    ),
    row(
        Type::Assault,
        "assault",
        "Assault",
        "Detonations",
        StartLine::Retail,
        9,
    ),
    row(
        Type::Oddball,
        "oddball",
        "Oddball",
        "Time",
        StartLine::Retail,
        3,
    ),
    row(
        Type::Koth,
        "koth",
        "King of the Hill",
        "Time",
        StartLine::Retail,
        4,
    ),
    row(
        Type::Juggernaut,
        "juggernaut",
        "Juggernaut",
        "Kills",
        StartLine::Retail,
        7,
    ),
    row(
        Type::Territories,
        "territories",
        "Territories",
        "Time",
        StartLine::Retail,
        8,
    ),
    row(
        Type::Research,
        "research",
        "Research and Devolution",
        "Points",
        StartLine::None,
        2,
    ),
    row(
        Type::Party,
        "party",
        "Party Games",
        "Points",
        StartLine::None,
        2,
    ),
    row(
        Type::Firefight,
        "firefight",
        "Firefight",
        "Points",
        StartLine::Ours,
        2,
    ),
    row(Type::Siege, "siege", "Siege", "Points", StartLine::Ours, 2),
];

const fn row(
    ty: Type,
    name: &'static str,
    label: &'static str,
    points: &'static str,
    start: StartLine,
    engine: u8,
) -> Row {
    Row {
        ty,
        name,
        label,
        points,
        start,
        engine,
    }
}

/// The most UTF-8 bytes of a `points` label.
pub const MAX_POINTS_BYTES: usize = 32;

impl Type {
    /// The type's row of [`TABLE`].
    pub fn row(self) -> &'static Row {
        // `TABLE` holds every type once, in declaration order.
        &TABLE[self.code() as usize]
    }

    /// The word the `type` line takes.
    pub fn name(self) -> &'static str {
        self.row().name
    }

    /// The type a `type` line names.
    pub fn named(name: &str) -> Option<Type> {
        TABLE.iter().find(|r| r.name == name).map(|r| r.ty)
    }

    /// What a point is when the script has no `points` line.
    pub fn points(self) -> &'static str {
        self.row().points
    }

    /// The retail engine index a record of this type is on ([`Row::engine`]).
    pub fn engine(self) -> u8 {
        self.row().engine
    }

    /// Whether the type is one of the seven retail ones, each with a
    /// built-in script a `uses` line may name (`crate::builtin::ALL`).
    pub fn is_retail(self) -> bool {
        self.row().start == StartLine::Retail
    }

    /// The header byte.
    pub fn code(self) -> u8 {
        match self {
            Type::Slayer => 0,
            Type::Ctf => 1,
            Type::Assault => 2,
            Type::Oddball => 3,
            Type::Koth => 4,
            Type::Juggernaut => 5,
            Type::Territories => 6,
            Type::Research => 7,
            Type::Party => 8,
            Type::Firefight => 9,
            Type::Siege => 10,
        }
    }

    /// The type a header byte names.
    pub fn from_code(c: u8) -> Option<Type> {
        TABLE.get(c as usize).map(|r| r.ty)
    }

    /// The retail type of a retail engine index.
    pub fn of_engine(engine: u8) -> Option<Type> {
        TABLE
            .iter()
            .find(|r| r.start == StartLine::Retail && r.engine == engine)
            .map(|r| r.ty)
    }
}

/// Every `type` word, comma separated, for messages.
pub fn names() -> String {
    TABLE.iter().map(|r| r.name).collect::<Vec<_>>().join(", ")
}

/// The retail types' words, comma separated: what a `uses` line takes.
pub fn retail_names() -> String {
    TABLE
        .iter()
        .filter(|r| r.start == StartLine::Retail)
        .map(|r| r.name)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_holds_each_type_once_at_its_code() {
        for (i, r) in TABLE.iter().enumerate() {
            assert_eq!(r.ty.code() as usize, i, "{}", r.name);
            assert_eq!(Type::from_code(i as u8), Some(r.ty));
            assert_eq!(Type::named(r.name), Some(r.ty));
            assert!(!r.points.is_empty() && r.points.len() <= MAX_POINTS_BYTES);
        }
        assert_eq!(Type::from_code(TABLE.len() as u8), None);
        assert_eq!(Type::named("king"), None);
    }

    #[test]
    fn the_seven_retail_types_are_the_seven_engines() {
        let retail: Vec<u8> = TABLE
            .iter()
            .filter(|r| r.ty.is_retail())
            .map(|r| r.engine)
            .collect();
        assert_eq!(retail, [2, 1, 9, 3, 4, 7, 8]);
        for r in TABLE {
            if r.ty.is_retail() {
                assert_eq!(Type::of_engine(r.engine), Some(r.ty));
            } else {
                assert_eq!(r.engine, 2, "{}: our types ride Slayer", r.name);
            }
        }
        assert_eq!(Type::of_engine(5), None);
        assert_eq!(
            retail_names(),
            "slayer, ctf, assault, oddball, koth, juggernaut, territories"
        );
    }
}
