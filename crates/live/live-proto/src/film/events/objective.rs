//! State edges observed in the retail game-engine objective registry. Item
//! and unit ids preserve datum salts. Player ids are roster slots.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Item {
    Flag,
    Bomb,
    Ball,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemChange {
    Registered,
    CarrierChanged,
    Unregistered,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Change {
    Item {
        item: Item,
        change: ItemChange,
        object: u32,
        /// The flag/bomb's team (8 for neutral), or the Oddball number.
        designator: i16,
        unit: Option<u32>,
        player: Option<u32>,
    },
    BombState {
        team: u8,
        arming: bool,
        armed: bool,
        planted: bool,
    },
    Hill {
        index: u8,
        /// Bit `i` is set when player slot `i` is in the scoring mask.
        occupants: u16,
    },
    Territory {
        index: u8,
        owner: Option<u32>,
        team: Option<i16>,
        occupants: u16,
    },
    Juggernauts {
        players: u16,
    },
}
