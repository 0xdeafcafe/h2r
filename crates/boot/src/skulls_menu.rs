//! The in-game Skulls menu (docs/skulls.md "Menu"): a "Skulls" row on the
//! campaign difficulty list that opens a screen of the pressing player's
//! collected skulls, each On or Off.
//!
//! **The row.** The difficulty screen (screen id 12, loaders `0x2bb3ed` and
//! `0x2bb432`) builds its list handler with `0x2c9c12`: a four-row datum
//! array named "difficulty list" (`0x45d6e0`) through the shared
//! constructor (`0x1a480d`, count in EAX). Its row text `0x2c9ce7` (item at
//! `[esp+4]`, row at `[item+0x70]`, `ret 8`) gives anything past row 3 the
//! null id, and its row chosen `0x2c9d38` (`ret 8`, a pointer to the row at
//! `[esp+8]`, `[[esp+4]+4]` the player index, as every list callback of
//! this kind gets) clamps the row to 0..3, stores the difficulty and starts
//! the campaign. So we grow the list to five when a signed-in profile has a
//! skull, draw row 4 ourselves and, for a press on row 4, open our screen
//! and never run the original. The handler embeds four item widgets, so the
//! list scrolls to the fifth row as lists with more rows than items do
//! (the Live menu's Custom Games row does the same). Moving onto row 4 would
//! set pane 4 of a tag that has four (`0x230427`, no bounds check), so the
//! pane change is skipped for it and the previous difficulty's pane stays.
//!
//! **The screen** is a second Controls screen, opened as the Video screen
//! is (`0x149f1e(1 << player, 0x2b7714)`): while our open call runs the list
//! hook names its datum array [`LIST_NAME_TEXT`] and gives it one row per
//! collected skull of the profile on the pressing player's slot. Its rows go
//! through the Controls row text and row chosen hooks, its title through
//! the header hook, as the Video screen's do.
//!
//! Rows are [`skulls::screen_for`]'s, so they come from the `ui` model's
//! screen of that profile; a press is [`skulls::toggle`].

use std::sync::Mutex;

use settings::Store;

use crate::layout_glue::{Gm, ID_BASE};
use crate::skulls::{self, COUNT, SKULLS};
use crate::speaker_row;
use crate::ui::RowKind;

/// The difficulty list: its array name, retail rows and our row.
pub const DIFFICULTY_LIST_NAME: u32 = 0x0045_d6e0;
pub const DIFFICULTY_ROWS: u32 = 4;
pub const ROW: i16 = 4;
/// The difficulty screen's id (`screen + 0x70`).
pub const DIFFICULTY_SCREEN_ID: i16 = 12;
/// The difficulty list's row text (`ret 8`) and row chosen (`ret 8`).
pub const DIFFICULTY_ROW_TEXT: u32 = 0x002c_9ce7;
pub const DIFFICULTY_ROW_CHOSEN: u32 = 0x002c_9d38;
/// The name our list's datum array is made with.
pub const LIST_NAME_TEXT: &str = "h2r skulls";

/// Our strings: a label per skull, the title (also the row's label) and the
/// array name. `layout_glue::is_ours` looks at the middle bytes only, so a
/// base one higher in the top byte than [`ID_BASE`] is ours too and clear of
/// every id the model and the other rows take (`ID_BASE + 0x00..0xff`).
pub const SKULLS_ID_BASE: u32 = ID_BASE + 0x0100_0000;
const LABEL_BASE: u32 = SKULLS_ID_BASE;
pub const TITLE_ID: u32 = SKULLS_ID_BASE + 0x10;
pub const LIST_NAME_ID: u32 = SKULLS_ID_BASE + 0x11;
pub const TITLE: &str = "Skulls";

/// The label id of skull `i`.
pub fn label_id(i: usize) -> u32 {
    LABEL_BASE + i as u32
}

/// Every string we serve (the array name is served apart, by
/// `layout_glue`, for its address).
pub fn strings() -> Vec<(u32, &'static str)> {
    let mut v: Vec<(u32, &'static str)> = SKULLS
        .iter()
        .enumerate()
        .map(|(i, d)| (label_id(i), d.label))
        .collect();
    v.push((TITLE_ID, TITLE));
    v
}

/// Whether the menu is on (`H2_NO_SKULLS` or `H2_NO_SKULLS_MENU` take it
/// out) for a store that has the skull settings.
pub fn enabled(store: &Store) -> bool {
    std::env::var_os("H2_NO_SKULLS").is_none()
        && std::env::var_os("H2_NO_SKULLS_MENU").is_none()
        && store.def(SKULLS[0].key).is_some()
}

/// Whether any of `profiles` has collected a skull: the difficulty list
/// then gets the row.
pub fn any_collected(store: &Store, profiles: &[String]) -> bool {
    profiles
        .iter()
        .any(|p| !skulls::collected(store, p).is_empty())
}

/// The difficulty list's count when the game builds the array `name` with
/// `retail` rows: one more when `row` (some profile has a skull), only for
/// that list at its retail size.
pub fn difficulty_count(name: u32, retail: u32, row: bool) -> u32 {
    if row && name == DIFFICULTY_LIST_NAME && retail == DIFFICULTY_ROWS {
        retail + 1
    } else {
        retail
    }
}

/// The count for the list our open call builds (the Controls list at its
/// retail six): `rows`, or `None` for any other list.
pub fn skulls_count(name: u32, retail: u32, opening: bool, rows: usize) -> Option<u32> {
    (opening && name == speaker_row::LIST_NAME && retail == speaker_row::VANILLA_ROWS && rows > 0)
        .then_some(rows as u32)
}

/// Whether the list `handler`'s datum array (`[handler+0x70]`) is named
/// [`LIST_NAME_TEXT`].
pub fn is_skulls_list<M: Gm>(m: &M, handler: u32) -> bool {
    let Some(array) = m.u32(handler.wrapping_add(0x70)) else {
        return false;
    };
    let want = LIST_NAME_TEXT.as_bytes();
    let mut got = vec![0; want.len() + 1];
    m.rd(array, &mut got) && got[..want.len()] == *want && got[want.len()] == 0
}

/// Whether `screen` is one of ours (its list handler at `+0x614`).
pub fn is_skulls_screen<M: Gm>(m: &M, screen: u32) -> bool {
    is_skulls_list(m, screen.wrapping_add(crate::video_screen::SCREEN_LIST))
}

/// The local player index of a list callback (`[[esp+4]]+4`), as
/// `video_screen::player_mask` reads it.
pub fn player_index<M: Gm>(m: &M, esp: u32) -> Option<u32> {
    let p = m.u32(esp.wrapping_add(4))?;
    let record = m.u32(p)?;
    let index = m.u32(record.wrapping_add(4))?;
    (index < skulls::LOCAL_PLAYERS).then_some(index)
}

/// A pane change on screen `screen_id` for list row `row` that must be
/// skipped: our row on the difficulty screen, which has no pane for it.
pub fn skip_pane(screen_id: i16, row: i16) -> bool {
    screen_id == DIFFICULTY_SCREEN_ID && row == ROW
}

/// The skull of row `row` of `profile`'s screen.
pub fn row_skull(store: &Store, profile: &str, row: i16) -> Option<usize> {
    let screen = skulls::screen_for(store, profile);
    let r = screen.rows().get(usize::try_from(row).ok()?)?;
    match r.kind {
        RowKind::Setting(k) => skulls::index_of(k),
        RowKind::Open(_) => None,
    }
}

/// `(text slot, string id)` for row `row` of `profile`'s screen: the
/// skull's name and the game's own On or Off.
pub fn row_texts(store: &Store, profile: &str, row: i16) -> Option<[(u32, u32); 2]> {
    let i = row_skull(store, profile, row)?;
    let on = store.profile_get(profile, SKULLS[i].key)?.as_bool()?;
    Some([
        (speaker_row::LABEL_TEXT_INDEX, label_id(i)),
        (
            speaker_row::VALUE_TEXT_INDEX,
            if on {
                speaker_row::ON_ID
            } else {
                speaker_row::OFF_ID
            },
        ),
    ])
}

/// A press on row `row` of `profile`'s screen: flip that skull. Returns
/// what to log.
pub fn press(store: &Store, profile: &str, row: i16) -> Option<String> {
    let i = row_skull(store, profile, row)?;
    Some(skulls::toggle(store, profile, SKULLS[i].key).unwrap_or_else(|e| e))
}

/// The profile whose skulls a press by local player `index` shows: the
/// one signed in on that player's slot. `None` when that slot has no named
/// profile.
pub fn profile_of<M: Gm>(m: &M, index: u32) -> Option<String> {
    skulls::slot_name(m, index)
}

/// The profile of the open Skulls screen. One is open at a time: the
/// screen is modal over the difficulty screen.
static OPEN: Mutex<Option<String>> = Mutex::new(None);

pub fn set_open(profile: Option<String>) {
    *OPEN.lock().unwrap_or_else(|e| e.into_inner()) = profile;
}

pub fn open_profile() -> Option<String> {
    OPEN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

const _: () = assert!(COUNT <= (TITLE_ID - LABEL_BASE) as usize);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_glue::is_ours;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake(BTreeMap<u32, u8>);
    impl Gm for Fake {
        fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
            for (i, b) in out.iter_mut().enumerate() {
                match self.0.get(&(addr + i as u32)) {
                    Some(v) => *b = *v,
                    None => return false,
                }
            }
            true
        }
        fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
            for (i, b) in data.iter().enumerate() {
                self.0.insert(addr + i as u32, *b);
            }
            true
        }
    }

    fn store() -> Store {
        let defs: Vec<&'static settings::Def> = SKULLS.iter().collect();
        Store::defaults(&defs)
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_difficulty_list_grows_only_when_a_profile_has_a_skull() {
        let s = store();
        let players = names(&["Chief", "Arbiter"]);
        assert!(!any_collected(&s, &players));
        assert_eq!(
            difficulty_count(DIFFICULTY_LIST_NAME, 4, any_collected(&s, &players)),
            4
        );
        skulls::record_pickup(&s, &names(&["Arbiter"]), 12);
        assert!(any_collected(&s, &players), "either co-op player counts");
        assert_eq!(difficulty_count(DIFFICULTY_LIST_NAME, 4, true), 5);
        assert_eq!(
            difficulty_count(DIFFICULTY_LIST_NAME, 5, true),
            5,
            "not at its retail size"
        );
        assert_eq!(
            difficulty_count(speaker_row::LIST_NAME, 4, true),
            4,
            "another list"
        );
        assert!(!any_collected(&s, &[]));
    }

    #[test]
    fn only_the_list_our_open_call_builds_is_the_skulls_list() {
        assert_eq!(skulls_count(speaker_row::LIST_NAME, 6, true, 3), Some(3));
        assert_eq!(skulls_count(speaker_row::LIST_NAME, 6, false, 3), None);
        assert_eq!(
            skulls_count(speaker_row::LIST_NAME, 6, true, 0),
            None,
            "no skulls, no screen"
        );
        assert_eq!(skulls_count(DIFFICULTY_LIST_NAME, 4, true, 3), None);
        let mut f = Fake::default();
        f.set_u32(0x3070, 0x4000);
        let mut name = LIST_NAME_TEXT.as_bytes().to_vec();
        name.resize(0x20, 0);
        f.wr(0x4000, &name);
        assert!(is_skulls_list(&f, 0x3000));
        assert!(is_skulls_screen(
            &f,
            0x3000 - crate::video_screen::SCREEN_LIST
        ));
        f.wr(0x4000, b"h2r video settings\0");
        assert!(!is_skulls_list(&f, 0x3000));
        assert!(!is_skulls_list(&Fake::default(), 0x3000));
    }

    #[test]
    fn rows_are_the_profiles_collected_skulls_with_on_and_off() {
        let s = store();
        skulls::record_pickup(&s, &names(&["Chief"]), 12);
        skulls::record_pickup(&s, &names(&["Chief"]), 1);
        skulls::record_pickup(&s, &names(&["Arbiter"]), 3);
        assert_eq!(row_skull(&s, "Chief", 0), Some(1));
        assert_eq!(row_skull(&s, "Chief", 1), Some(12));
        assert_eq!(row_skull(&s, "Chief", 2), None);
        assert_eq!(row_skull(&s, "Chief", -1), None);
        assert_eq!(
            row_texts(&s, "Chief", 1),
            Some([(0, label_id(12)), (1, speaker_row::ON_ID)])
        );
        assert_eq!(
            press(&s, "Chief", 1).unwrap(),
            "skulls: Mythic off for Chief"
        );
        assert_eq!(
            row_texts(&s, "Chief", 1),
            Some([(0, label_id(12)), (1, speaker_row::OFF_ID)])
        );
        assert_eq!(row_texts(&s, "Chief", 2), None);
        assert_eq!(press(&s, "Chief", 5), None);
        // The other co-op player's screen is theirs.
        assert_eq!(row_skull(&s, "Arbiter", 0), Some(3));
        assert_eq!(row_skull(&s, "Arbiter", 1), None);
        assert_eq!(row_skull(&s, "Nobody", 0), None);
    }

    #[test]
    fn the_pressing_players_profile_is_the_one_shown() {
        let mut f = Fake::default();
        // [esp+4] -> p -> record; record+4 is the player index.
        f.set_u32(0x1004, 0x5000);
        f.set_u32(0x5000, 0x6000);
        f.set_u32(0x6004, 1);
        assert_eq!(player_index(&f, 0x1000), Some(1));
        let base = skulls::SLOTS + skulls::SLOT_STRIDE;
        f.set_u32(base, skulls::SLOT_SIGNED_IN);
        let mut name = vec![0u8; skulls::NAME_UNITS * 2];
        for (i, u) in "Arbiter".encode_utf16().enumerate() {
            name[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        f.wr(base + skulls::SLOT_NAME, &name);
        assert_eq!(profile_of(&f, 1).as_deref(), Some("Arbiter"));
        assert_eq!(profile_of(&f, 0), None);
        f.set_u32(0x6004, 4);
        assert_eq!(player_index(&f, 0x1000), None);
        set_open(Some("Arbiter".into()));
        assert_eq!(open_profile().as_deref(), Some("Arbiter"));
        set_open(None);
    }

    #[test]
    fn only_our_row_on_the_difficulty_screen_skips_its_pane() {
        assert!(skip_pane(DIFFICULTY_SCREEN_ID, ROW));
        assert!(!skip_pane(DIFFICULTY_SCREEN_ID, 3));
        assert!(!skip_pane(40, ROW));
    }

    #[test]
    fn strings_are_ours_distinct_and_clear_of_the_models() {
        let all = strings();
        assert_eq!(all.len(), COUNT + 1);
        let mut ids: Vec<u32> = all.iter().map(|(i, _)| *i).collect();
        ids.push(LIST_NAME_ID);
        assert!(ids.iter().all(|i| is_ours(*i)));
        assert!(
            ids.iter()
                .all(|i| !crate::video_screen::MODEL_IDS.contains(i))
        );
        assert!(
            ids.iter().all(|i| *i >= ID_BASE + 0x100),
            "clear of the other rows' ids"
        );
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
        assert!(all.contains(&(label_id(12), "Mythic")));
        assert!(all.contains(&(TITLE_ID, "Skulls")));
        assert!(LIST_NAME_TEXT.len() < 0x20);
    }
}
