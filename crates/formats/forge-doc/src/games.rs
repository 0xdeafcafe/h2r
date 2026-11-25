//! Game types as a set of engines (docs/game/forge-map-variants.md,
//! "Game types"): what an item spawner's four game-type slots mean, and
//! the game type a netgame flag is by its type.
//!
//! An engine is named by its index in the variant record (King of the
//! Hill 4, Capture the Flag 1, Slayer 2, Oddball 3, Juggernaut 7,
//! Territories 8, Assault 9: the index `forge::tool::in_game` takes); the
//! slots hold the engine's code (King of the Hill 0, the rest as the
//! index) or a group value. The matcher is
//! `map::scenario_layout::game_types::slot_matches`; [`encode`] and
//! [`decode`] are its inverse. Capture the Flag and Assault match each
//! other's slot, so a set has both or neither.

use std::fmt;

use map::scenario_layout::game_types as slots;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// The engines, by index and name, in the order the slots list them.
pub const ENGINES: [(i32, &str); 7] = [
    (4, "King of the Hill"),
    (1, "Capture the Flag"),
    (2, "Slayer"),
    (3, "Oddball"),
    (7, "Juggernaut"),
    (8, "Territories"),
    (9, "Assault"),
];

/// A set of engines, by index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GameSet(u16);

/// Capture the Flag and Assault, which one slot names together.
const FLAGS: u16 = 1 << 1 | 1 << 9;

impl GameSet {
    pub const NONE: GameSet = GameSet(0);
    pub const ALL: GameSet = GameSet(1 << 4 | 1 << 1 | 1 << 2 | 1 << 3 | 1 << 7 | 1 << 8 | 1 << 9);

    /// The set of these engine indices; an index that is no engine's is
    /// left out.
    pub fn of(indices: &[i32]) -> GameSet {
        GameSet(
            indices
                .iter()
                .filter(|i| ENGINES.iter().any(|(e, _)| e == *i))
                .fold(0u16, |bits, i| bits | 1u16 << *i),
        )
    }

    pub fn has(self, index: i32) -> bool {
        (0..16).contains(&index) && self.0 >> index & 1 == 1
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The engines in the set, in [`ENGINES`]' order.
    pub fn engines(self) -> Vec<i32> {
        ENGINES
            .iter()
            .map(|(i, _)| *i)
            .filter(|i| self.has(*i))
            .collect()
    }
}

impl fmt::Display for GameSet {
    /// `All game types`, or the engines' names by commas.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == GameSet::ALL {
            return f.write_str("All game types");
        }
        let names: Vec<&str> = ENGINES
            .iter()
            .filter(|(i, _)| self.has(*i))
            .map(|(_, n)| *n)
            .collect();
        f.write_str(&names.join(", "))
    }
}

/// A set is written as its engine indices in ascending order, `[1, 9]`,
/// one spelling a set: a list out of order, with a repeat or with an index
/// that is no engine's is refused.
impl Serialize for GameSet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut indices: Vec<i32> = self.engines();
        indices.sort_unstable();
        indices.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for GameSet {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let indices = Vec::<i32>::deserialize(d)?;
        if indices.len() > ENGINES.len() || !indices.windows(2).all(|w| w[0] < w[1]) {
            return Err(de::Error::custom(
                "a game set lists engine indices once each, in ascending order",
            ));
        }
        let set = GameSet::of(&indices);
        if set.engines().len() != indices.len() {
            return Err(de::Error::custom("a game set names engines by index"));
        }
        Ok(set)
    }
}

/// Why a set has no slots.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GamesError {
    #[error("An item can name four game types at most.")]
    TooMany,
    #[error("Capture the Flag and Assault go together.")]
    FlagPair,
    #[error("An item needs a game type.")]
    Empty,
}

/// The engine's code for slots: King of the Hill's is 0.
fn code(index: i32) -> i32 {
    if index == 4 { 0 } else { index }
}

/// The slots for `set`: a group value when one equals the set, else one
/// slot an engine (Capture the Flag and Assault share theirs), the unused
/// slots repeating the first.
pub fn encode(set: GameSet) -> Result<[i16; slots::SLOT_COUNT], GamesError> {
    if set.is_empty() {
        return Err(GamesError::Empty);
    }
    let one = |slot: i16| [slot; slots::SLOT_COUNT];
    if set == GameSet::ALL {
        return Ok(one(slots::ALL));
    }
    let flags = set.0 & FLAGS;
    if flags != 0 && flags != FLAGS {
        return Err(GamesError::FlagPair);
    }
    if set.0 == GameSet::ALL.0 & !FLAGS {
        return Ok(one(slots::ALL_BUT_1_AND_9[0]));
    }
    let mut out: Vec<i16> = ENGINES
        .iter()
        .map(|(i, _)| *i)
        .filter(|i| set.has(*i) && *i != 9)
        .map(|i| code(i) as i16)
        .collect();
    if out.len() > slots::SLOT_COUNT {
        return Err(GamesError::TooMany);
    }
    let first = out[0];
    out.resize(slots::SLOT_COUNT, first);
    Ok(std::array::from_fn(|n| out[n]))
}

/// Whether the matcher knows the slot's value.
fn known(slot: i16) -> bool {
    slots::ENGINE_CODES.contains(&i32::from(slot))
        || slot == slots::ALL
        || slots::ALL_BUT_1_AND_9.contains(&slot)
}

/// The set `slots` name, or none when a slot holds a value the table does
/// not know: such an entry is shown as it is and not edited.
pub fn decode(slot_values: &[i16]) -> Option<GameSet> {
    if !slot_values.iter().copied().all(known) {
        return None;
    }
    let engines: Vec<i32> = ENGINES
        .iter()
        .map(|(i, _)| *i)
        .filter(|i| slots::first_match(slot_values, Some(code(*i))).is_some())
        .collect();
    Some(GameSet::of(&engines))
}

/// The game type a netgame flag of type `ty` is in: Capture the Flag's
/// spawn and return (0, 1), Assault's bomb spawn and return (2, 3),
/// Oddball's ball (4), a territory (10) and a hill corner (11 to 18). A
/// flag of another type is in every game type, which is `None`.
pub fn flag_games(ty: i64) -> Option<GameSet> {
    Some(match ty {
        0 | 1 => GameSet::of(&[1]),
        2 | 3 => GameSet::of(&[9]),
        4 => GameSet::of(&[3]),
        10 => GameSet::of(&[8]),
        11..=18 => GameSet::of(&[4]),
        _ => return None,
    })
}
