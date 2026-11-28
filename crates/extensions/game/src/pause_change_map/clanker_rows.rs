//! Clanker Options on the pause menu's Game Setup: the Clankers page's
//! settings (`crate::clankers::DEFS`) as a list of ours, one line a row,
//! A on a row opening the value picker (docs/game/settings-menu.md "Game
//! Setup from the pause menu").
//!
//! The lobby's page is a dialog on Quick Options' tag, which is in
//! `mainmenu.map` only; in a game its rows would have no value column, so
//! the pause menu lists the same settings on screen 200 as the other
//! in-game lists of ours do. A change is stored in the settings store,
//! as the lobby's page stores it, so the Clankers driver applies it
//! ([`applies`]): the bots are added, taken off or retuned at once, in the
//! game being played.

use std::sync::Arc;

use settings::{Def, Store};

use crate::list_class::Rows;
use crate::ui::Chosen;
use crate::ui_kit::choice;
use crate::ui_kit::picker::{self, Pick};

use super::quick::Applies;
use super::{CLANKER_SLOTS, CLANKER_TITLE, CLANKER_TITLE_SLOT, IDS, TEXTS};

/// When a change to `def` takes effect: at once for every setting the
/// driver follows (`clankers::LIVE_KEYS`); Teams has no reader, so it
/// waits for the next game's seating at best.
pub fn applies(def: &Def) -> Applies {
    if crate::clankers::LIVE_KEYS.contains(&def.key) {
        Applies::Live
    } else {
        Applies::AtRestart
    }
}

/// Setting `def` as one line: its label, two spaces, the held choice.
pub fn line(def: &Def, store: &Store) -> String {
    let cs = choice::choices(def);
    let held = cs
        .get(choice::index(def, store))
        .map_or_else(String::new, |(_, label)| label.clone());
    format!("{}  {held}", def.label)
}

/// The value picker for `def`: its choices, the held one focused, each
/// pick stored at once.
pub fn pick(def: &'static Def, store: &Arc<Store>) -> Pick {
    let values: Vec<String> = choice::choices(def).into_iter().map(|(_, l)| l).collect();
    let store = Arc::clone(store);
    let current = choice::index(def, &store);
    let set = Arc::new(move |i: usize| format!("pause clanker options: {}", choice::set(def, &store, i)));
    Pick::new(def.label, values, current, def.help, set)
}

/// The page's rows.
pub struct ClankerRows(pub Arc<Store>);

impl ClankerRows {
    fn def(row: i16) -> Option<&'static Def> {
        usize::try_from(row)
            .ok()
            .and_then(|r| crate::clankers::DEFS.get(r))
            .copied()
    }
}

impl Rows for ClankerRows {
    fn title(&self) -> u32 {
        IDS.id(CLANKER_TITLE_SLOT)
    }
    fn count(&self) -> usize {
        TEXTS.set(CLANKER_TITLE_SLOT, CLANKER_TITLE);
        crate::clankers::DEFS.len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        match Self::def(row) {
            Some(d) => vec![(0, TEXTS.set(CLANKER_SLOTS + row as u32, &line(d, &self.0)))],
            None => Vec::new(),
        }
    }
    fn line(&self, row: i16) -> Option<String> {
        Self::def(row).map(|d| line(d, &self.0))
    }
    fn press(&self, row: i16) -> Chosen {
        let Some(d) = Self::def(row) else {
            return Chosen::Nothing;
        };
        picker::offer_list(pick(d, &self.0)).map_or(Chosen::Nothing, Chosen::Open)
    }
    fn write_texts(&self, m: &mut dyn guest_layout::memory::GuestMemory) -> Result<(), guest_layout::memory::MemFault> {
        TEXTS.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let lines: Vec<String> = crate::clankers::DEFS.iter().map(|d| line(d, &self.0)).collect();
        Some(format!("{CLANKER_TITLE}: {count} rows ({})", lines.join(", ")))
    }
}
