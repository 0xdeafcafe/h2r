//! Our rows in the pause menu's Settings: Video and Voice, each opening
//! our screen of that name over it (docs/game/settings-menu.md "Pause
//! menu").
//!
//! **The Settings dialog** (screen 200, `mp_menu\controller_settings`):
//! the campaign pause menu's row 3 closes the menu and opens it (loader
//! `0x231db5`); the multiplayer pause menu's action 1 opens it on the
//! player's own stack. Its list handler (`0x2b48c2`, at `screen + 0x614`)
//! builds a datum array (named at `0x45b8b0`, count 6) and gives each datum
//! a value word at `+2`, 0 to 5. The row text (`0x2b49e6`) looks the value
//! up in a table of six string ids; the row chosen (`0x2b4a4d`) switches on
//! it and does nothing for any other value. The Controller Speaker row
//! (`crate::speaker_row`) is value 6. Ours follow at the end: values from
//! [`FIRST_VALUE`], one datum each, added after the constructor, the array
//! grown to hold them ([`list_count`]). The hooks are `layout_glue`'s, on
//! the same three functions as the speaker row's.
//!
//! **Which stack.** A row of ours opens its screen with a request built
//! from the Settings screen's own `+0x74` and `+0x78`, as the retail
//! menus open their sub-screens: window 5, slot 4 from the campaign menu,
//! the player's slot from the multiplayer one.
//!
//! **Pause.** Our screens never touch the paused byte. The game clears it
//! (`0x1482c4`, every UI update) only while no screen is up on any stack
//! (`0x149227`), so a screen of ours over the Settings dialog keeps a
//! campaign paused, and B back changes nothing. A multiplayer game is
//! never paused.
//!
//! **In a game** our screens are built on screen 200's own tag
//! (`crate::list_class`, "In a game"): the Controls tag is not loaded there.

use crate::layout_glue::Gm;
use crate::ui::{Chosen, List, Model};

/// The Settings dialog's rows' datum array name and the rows it makes.
pub const SETTINGS_LIST_NAME: u32 = crate::speaker_row::SMALL_LIST_NAME;
pub const SETTINGS_ROWS: u32 = 6;
/// Our rows' first value word: after the retail rows (0 to 5) and the
/// Controller Speaker row (6).
pub const FIRST_VALUE: u16 = crate::speaker_row::SMALL_VALUE + 1;
/// The rows' datum array at handler `+0x70`; an item widget's row handle
/// at `+0x70`.
pub const LIST_ROWS: u32 = 0x70;
pub const ITEM_ROW: u32 = 0x70;

/// The feature switch of our rows (`features.pause-rows`).
pub const FEATURE: &str = "pause-rows";

/// Whether our pause rows are on: `H2_NO_PAUSE_ROWS` takes them out.
pub fn rows_on() -> bool {
    std::env::var_os("H2_NO_PAUSE_ROWS").is_none()
}

/// The count the datum array named `name` is made with, `count` being what
/// it would be made with otherwise: the Settings dialog's grows by our rows.
pub fn list_count(m: &Model, name: u32, count: u32) -> u32 {
    match name {
        SETTINGS_LIST_NAME => count + m.rows(List::GameSettings).len() as u32,
        _ => count,
    }
}

/// One row of ours on the Settings dialog, fixed at install: its value
/// word, its label's string id and the screen it opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsRow {
    pub value: u16,
    pub label: u32,
    pub screen: &'static str,
}

/// Our rows, in order, from [`FIRST_VALUE`]. Reads the model's store:
/// never call it with guest memory held.
pub fn settings_rows(m: &Model) -> Vec<SettingsRow> {
    (0..m.rows(List::GameSettings).len() as u16)
        .filter_map(|i| {
            let value = FIRST_VALUE + i;
            let row = m.row(List::GameSettings, value as i16)?;
            let Chosen::Open(screen) = m.press(row) else {
                return None;
            };
            Some(SettingsRow {
                value,
                label: m.texts(row)[0].1,
                screen,
            })
        })
        .collect()
}

/// The value word of the row whose handle is `row` on the list at `list`.
pub fn row_value<M: Gm>(m: &M, list: u32, row: u32) -> Option<u16> {
    crate::speaker_row::datum_value(m, m.u32(list.wrapping_add(LIST_ROWS))?, row)
}

/// Ours among `rows` for a row text call (ECX the list, the item at
/// `[esp+4]`).
pub fn text_row<M: Gm>(m: &M, list: u32, esp: u32, rows: &[SettingsRow]) -> Option<SettingsRow> {
    let item = m.u32(esp.wrapping_add(4))?;
    let value = row_value(m, list, m.u32(item.wrapping_add(ITEM_ROW))?)?;
    rows.iter().copied().find(|r| r.value == value)
}

/// Ours among `rows` for a row chosen call (ECX the list, a pointer to the
/// row's handle at `[esp+8]`).
pub fn chosen_row<M: Gm>(m: &M, list: u32, esp: u32, rows: &[SettingsRow]) -> Option<SettingsRow> {
    let p = m.u32(esp.wrapping_add(8))?;
    let value = row_value(m, list, m.u32(p)?)?;
    rows.iter().copied().find(|r| r.value == value)
}

/// Where the Settings screen keeps its list handler (`0x231df3`).
pub const SCREEN_LIST: u32 = 0x614;

/// The request our row's screen opens with, for a row chosen call (ECX the
/// list, the player chain at `[esp+4]`): the pressing player's mask and
/// the Settings screen's window and slot (`+0x74`, `+0x78`).
pub fn open_request<M: Gm>(m: &M, list: u32, esp: u32, loader: u32) -> Option<[u8; 0x20]> {
    let player = crate::skulls_menu::player_index(m, esp)?;
    let screen = list.wrapping_sub(SCREEN_LIST);
    let window = m.u32(screen + crate::ui_kit::lifecycle::SCREEN_WINDOW)?;
    let slot = m.u32(screen + crate::ui_kit::lifecycle::SCREEN_SLOT)?;
    Some(crate::skulls_mp::open_request(
        1u16 << player,
        window,
        slot,
        loader,
    ))
}

/// The retail rule for the paused byte, as `0x1482c4` applies it each UI
/// update: a paused game resumes only when no screen is up on any stack.
/// (`0x23203e` sets it when the campaign pause menu is made; nothing of
/// ours writes it.)
pub fn resumes(paused: bool, any_screen_up: bool) -> bool {
    paused && !any_screen_up
}

#[cfg(test)]
mod tests;
