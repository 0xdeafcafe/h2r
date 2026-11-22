//! Change Rules on the pause menu's Game Setup: the game types and their
//! variants, as the lobby's Change Rules lists them
//! (docs/game/settings-menu.md "Game Setup from the pause menu").
//!
//! The lobby's list (`crate::tweakers`, "SELECT GAME", screen 207) is in
//! `mainmenu.map` only, so in a game the variants come from the game's
//! built-in list (`[0x51ea14]`, `tweakers::BUILTIN_LIST`): the retail
//! built-ins, the community variants and the game-variant store's
//! (Live, imported `.h2variant` files and the ported saves), which
//! `crate::community_variants` and `crate::game_variants::inject` merge
//! into it at each load of the main menu's tags. A variant is filed as
//! the lobby files it: under its game type, or under one of our rows
//! (Research and Devolution, Party Games) when its name or the rules
//! script its block names says so (`tweakers::record_row`).

use engine::memory::GuestMemory;
use variants::record::{GameType, GameVariant, RECORD_SIZE};

use crate::community_variants::Row;
use crate::tweakers;

/// A row of the game type list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Type(GameType),
    Row(Row),
}

/// The game type list's order (`tweakers::GAME_TYPE_WORDS`), then ours.
pub const ORDER: [Group; 9] = [
    Group::Type(GameType::Slayer),
    Group::Type(GameType::King),
    Group::Type(GameType::Oddball),
    Group::Type(GameType::Juggernaut),
    Group::Type(GameType::Ctf),
    Group::Type(GameType::Assault),
    Group::Type(GameType::Territories),
    Group::Row(Row::Research),
    Group::Row(Row::PartyGames),
];

impl Group {
    pub fn label(self) -> String {
        match self {
            Group::Type(t) => t.label(),
            Group::Row(r) => r.label().to_owned(),
        }
    }

    /// Its place in [`ORDER`].
    pub fn index(self) -> usize {
        ORDER.iter().position(|g| *g == self).unwrap_or(0)
    }
}

/// One variant the list offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub group: Group,
    /// Its name without the list's icon.
    pub name: String,
    pub variant: GameVariant,
}

/// A record's name as a row shows it: the list's icon (a private-use
/// character after the name) and spaces dropped.
pub fn display_name(name: &str) -> String {
    name.trim_end_matches(|c: char| c.is_whitespace() || ('\u{e000}'..='\u{f8ff}').contains(&c))
        .trim()
        .to_owned()
}

/// `variant` as a row of the list: under our `row` when it has one, else
/// under its game type; `None` for an engine the list has no row for.
pub fn listed(variant: GameVariant, row: Option<Row>) -> Option<Listed> {
    let group = match row {
        Some(r) => Group::Row(r),
        None => match variant.game_type() {
            GameType::Other(_) => return None,
            t => Group::Type(t),
        },
    };
    Some(Listed {
        group,
        name: display_name(&variant.name()),
        variant,
    })
}

/// The game types with a variant, in the list's order.
pub fn groups(list: &[Listed]) -> Vec<Group> {
    ORDER
        .into_iter()
        .filter(|g| list.iter().any(|l| l.group == *g))
        .collect()
}

/// The variants of `group`, in the built-in list's order.
pub fn of_group(list: &[Listed], group: Group) -> Vec<Listed> {
    list.iter().filter(|l| l.group == group).cloned().collect()
}

/// The built-in list as the game holds it now; empty when it has none.
pub fn read(m: &dyn GuestMemory) -> Vec<Listed> {
    let Some(list) = m.u32(tweakers::BUILTIN_LIST).ok().filter(|l| *l != 0) else {
        return Vec::new();
    };
    let n = m.u32(list).unwrap_or(0).min(tweakers::MAX_BUILTINS);
    let mut out = Vec::new();
    for i in 0..n {
        let record = list + 4 + i * tweakers::BUILTIN_RECORD + tweakers::BUILTIN_VARIANT;
        let mut raw = [0u8; RECORD_SIZE];
        if m.read(record, &mut raw).is_err() {
            continue;
        }
        let Ok(v) = GameVariant::from_bytes(&raw) else {
            continue;
        };
        if let Some(l) = listed(v, tweakers::record_row(m, record)) {
            out.push(l);
        }
    }
    out
}
