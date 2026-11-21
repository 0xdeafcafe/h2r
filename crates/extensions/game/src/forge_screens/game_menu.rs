//! The Forge rows of the game's own GAME MENU (docs/game/forge-firefight-ui.md
//! "The screens", the Forge menu): the multiplayer pause screen (`mp_menu`,
//! screen 195) with **Save**, **Save as**, **Load**, **Rebuild map** and
//! **Forge options...** above the retail rows while a local Forge session
//! runs, added to the retail list the way `crate::pause_menu` adds Video
//! and Voice (docs/game/settings-menu.md "Pause menu"). These are the rows
//! Halo 3's Start has; everything else is on Forge options, so GAME MENU
//! never scrolls.
//!
//! - **Rows.** The list constructor (`MP_MENU_LIST_CTOR`) makes a datum per
//!   retail row with its action word at `+2` (0 Leave Game, 1 Settings, ...).
//!   After it runs, [`put_first`] adds a datum per row of ours and moves
//!   the action words so ours come first: the retail rows keep their own
//!   words, one place down. The rows' array has room for them: its size is
//!   asked at `LIST_DATA_NEW` with the array's name, and [`list_count`]
//!   grows it.
//! - **Text.** Our action words draw our labels (`MP_MENU_ROW_TEXT`).
//! - **Press** (`MP_MENU_ROW_CHOSEN`, which runs action 5's press for any
//!   word it doesn't know): ours are taken first; Leave Game with edits not
//!   saved opens the question instead.

use engine::datum::DatumArray;
use engine::memory::{GuestMemory, MemFault};

/// The GAME MENU's screen id: `ui\screens\multiplayer\mp_menu\mp_menu`.
pub const SCREEN_ID: i16 = 195;
/// Its rows' datum array's name (the constructor's `LIST_DATA_NEW`).
pub const ARRAY_NAME: u32 = 0x0045_93a8;
/// The retail rows' room.
pub const RETAIL_ROOM: u32 = 6;
/// Leave Game's action word.
pub const LEAVE_GAME: u16 = 0;
/// Our first action word, clear of retail's six (0 to 5).
pub const FIRST_ACTION: u16 = 0x20;

/// A row of ours, top first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuRow {
    Save,
    SaveAs,
    Load,
    Rebuild,
    Options,
}

/// The rows, in order. A row beyond these five goes on Forge options
/// (`super::sheet::OPTIONS`).
pub const ROWS: [MenuRow; 5] = [
    MenuRow::Save,
    MenuRow::SaveAs,
    MenuRow::Load,
    MenuRow::Rebuild,
    MenuRow::Options,
];

impl MenuRow {
    pub const fn label(self) -> &'static str {
        match self {
            MenuRow::Save => "Save",
            MenuRow::SaveAs => "Save as",
            MenuRow::Load => "Load",
            MenuRow::Rebuild => "Rebuild map",
            MenuRow::Options => "Forge options...",
        }
    }
    /// The row's text: Rebuild map has the pending count as its value, two
    /// spaces after it as retail's settings rows read.
    pub fn text(self, pending_rebuild: usize) -> String {
        match self {
            MenuRow::Rebuild if pending_rebuild > 0 => {
                format!("{}  {pending_rebuild}", self.label())
            }
            _ => self.label().to_string(),
        }
    }
    pub fn index(self) -> usize {
        ROWS.iter().position(|r| *r == self).expect("a row of ROWS")
    }
    /// Its action word.
    pub fn action(self) -> u16 {
        FIRST_ACTION + self.index() as u16
    }
    pub fn of_action(word: u16) -> Option<MenuRow> {
        ROWS.get(usize::from(word.checked_sub(FIRST_ACTION)?))
            .copied()
    }
}

/// The action words of our rows, top first.
pub fn actions() -> Vec<u16> {
    ROWS.iter().map(|r| r.action()).collect()
}

/// The GAME MENU rows' array grows by our rows while they show; any other
/// array, or none shown, as asked.
pub fn list_count(name: u32, n: u32, shown: bool) -> u32 {
    if shown && name == ARRAY_NAME {
        n + ROWS.len() as u32
    } else {
        n
    }
}

/// A datum's action word, at `+2` of its 4-byte element.
const ACTION_WORD: u32 = 2;

/// Adds a datum per word of `ours` to the rows' array at `array` and moves
/// the action words so ours come first, the retail rows after them in
/// their order: the rows a list draws are its live datums in index order.
/// Returns the rows the list then has. Refused by name when the array
/// cannot take ours (its room was not grown).
pub fn put_first(m: &mut dyn GuestMemory, array: u32, ours: &[u16]) -> Result<usize, String> {
    let a = DatumArray::at(array);
    let fault = |e: MemFault| format!("the GAME MENU rows' array: {e:?}");
    let high = a
        .header_field(m, engine::datum::layout::HIGH_WATER)
        .map_err(fault)?;
    let mut elements = Vec::new();
    for i in 0..high {
        if let Some(e) = a.get_absolute(m, i).map_err(fault)? {
            elements.push(e);
        }
    }
    let mut words = ours.to_vec();
    for e in &elements {
        words.push(m.u16(e + ACTION_WORD).map_err(fault)?);
    }
    for _ in ours {
        let h = a
            .new_datum(m)
            .map_err(fault)?
            .ok_or("the GAME MENU rows' array is full; its room was not grown")?;
        let e = a
            .get_absolute(m, h.index() as i32)
            .map_err(fault)?
            .ok_or("a new GAME MENU row is not live")?;
        elements.push(e);
    }
    elements.sort_unstable();
    for (e, w) in elements.iter().zip(&words) {
        m.set_u16(e + ACTION_WORD, *w).map_err(fault)?;
    }
    Ok(elements.len())
}
