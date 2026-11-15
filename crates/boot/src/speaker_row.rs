//! The "Controller Speaker" row on Halo 2's Settings, Controls screen
//! (screen 39), under the vibration row: its label, its on/off value and
//! what pressing it does. See docs/playstation-audio.md "In-game setting"
//! for the addresses and why the row lives here.
//!
//! The Controls list has six rows, built by its screen constructor through
//! the datum array constructor with the count in EAX. The row text function
//! (`0x2c8548`, `ret 8`, item widget at `[esp+4]`, row at `[item+0x70]`)
//! fills a label and a value text; row 5 is vibration, and anything past it
//! gets the null id. The row-chosen handler (`0x2c8719`, thiscall, `ret 8`,
//! a pointer to the row at `[esp+8]`) opens a dialog per row and the
//! vibration dialog for anything past row 4. So for row 6 we fill the texts
//! ourselves and toggle the setting in place, and never run the original
//! for that row. Rows 0 to 5 always go to the original.
//!
//! The hooks are registered by `layout_glue::install` (both features share
//! the list constructor, the row text function and the string lookup); the
//! logic is here, over [`Gm`], so it is tested without a guest.

use crate::layout_glue::Gm;
#[cfg(test)]
use crate::layout_glue::chosen_row;
use crate::speaker_setting::SpeakerSetting;

/// The Controls screen's row-chosen handler.
pub const ROW_CHOSEN: u32 = 0x002c_8719;
/// Name of the Controls list's item array, which marks it as that list.
pub const LIST_NAME: u32 = 0x0045_d298;
pub const VANILLA_ROWS: u32 = 6;
/// Our row, after the vibration row.
pub const ROW: i16 = 6;
/// The item widget's row index.
const ITEM_ROW: u32 = 0x70;
/// The game's own value strings, as the vibration row uses them, so the
/// value is in the player's language.
pub const ON_ID: u32 = 0x0200_0181;
pub const OFF_ID: u32 = 0x0300_0182;
/// Our label, in the private range of `layout_glue::is_ours`.
pub const LABEL_ID: u32 = crate::layout_glue::ID_BASE + 0x20;
pub const LABEL_TEXT: &str = "Controller Speaker";
/// The skin's text widgets of a row: label, then value.
pub const LABEL_TEXT_INDEX: u32 = 0;
pub const VALUE_TEXT_INDEX: u32 = 1;

/// The list's item count: one more for the Controls list, unchanged for any
/// other.
pub fn list_count(name: u32, count: u32) -> u32 {
    if name == LIST_NAME && count == VANILLA_ROWS {
        count + 1
    } else {
        count
    }
}

/// The row an item widget shows, if it can be read.
pub fn item_row<M: Gm>(m: &M, item: u32) -> Option<i16> {
    m.u16(item.wrapping_add(ITEM_ROW)).map(|v| v as i16)
}

/// The texts to set for the row text call with the stack at `esp`:
/// `(text widget index, string id)` for the label and the value, or `None`
/// for any row but ours (the original runs).
pub fn row_texts<M: Gm>(m: &M, esp: u32, on: bool) -> Option<[(u32, u32); 2]> {
    let item = m.u32(esp.wrapping_add(4))?;
    (item_row(m, item)? == ROW).then_some([
        (LABEL_TEXT_INDEX, LABEL_ID),
        (VALUE_TEXT_INDEX, if on { ON_ID } else { OFF_ID }),
    ])
}

/// A row-chosen call for `row` (read with `layout_glue::chosen_row`): for
/// our row, toggle the setting and return true (the original must not run);
/// false otherwise. Call it with guest memory released: the toggle takes
/// DirectSound's lock.
pub fn chosen(row: Option<i16>, setting: &SpeakerSetting) -> bool {
    if row != Some(ROW) {
        return false;
    }
    setting.toggle();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
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

    const ESP: u32 = 0x1000;
    const ITEM: u32 = 0x2000;
    const ROW_PTR: u32 = 0x3000;

    /// A row text call for an item showing `row`.
    fn text_call(row: i16) -> Fake {
        let mut f = Fake::default();
        f.set_u32(ESP + 4, ITEM);
        f.set_u16(ITEM + 0x70, row as u16);
        f
    }

    /// A row-chosen call for `row`.
    fn chosen_call(row: i16) -> Fake {
        let mut f = Fake::default();
        f.set_u32(ESP + 4, 0xdead_0000);
        f.set_u32(ESP + 8, ROW_PTR);
        f.set_u16(ROW_PTR, row as u16);
        f
    }

    #[test]
    fn only_the_controls_list_grows_by_one() {
        assert_eq!(list_count(LIST_NAME, 6), 7);
        assert_eq!(list_count(LIST_NAME, 4), 4);
        assert_eq!(list_count(crate::layout_glue::LIST_NAME, 6), 6);
        assert_eq!(list_count(0x1234, 6), 6);
    }

    #[test]
    fn the_added_row_shows_our_label_and_the_setting() {
        let f = text_call(ROW);
        assert_eq!(row_texts(&f, ESP, true), Some([(0, LABEL_ID), (1, ON_ID)]));
        assert_eq!(
            row_texts(&f, ESP, false),
            Some([(0, LABEL_ID), (1, OFF_ID)])
        );
        assert!(crate::layout_glue::is_ours(LABEL_ID));
        assert!(!crate::layout_glue::is_ours(ON_ID) && !crate::layout_glue::is_ours(OFF_ID));
    }

    #[test]
    fn vanilla_rows_go_to_the_original() {
        for row in [0i16, 1, 2, 3, 4, 5, 7, -1] {
            assert_eq!(row_texts(&text_call(row), ESP, true), None, "{row}");
            let s = SpeakerSetting::new(None, None);
            assert!(!chosen(chosen_row(&chosen_call(row), ESP), &s), "{row}");
            assert!(s.enabled(), "row {row} must not toggle");
        }
        // Unreadable memory: the original runs.
        assert_eq!(row_texts(&Fake::default(), ESP, true), None);
        assert!(!chosen(
            chosen_row(&Fake::default(), ESP),
            &SpeakerSetting::new(None, None)
        ));
    }

    #[test]
    fn choosing_the_row_toggles_on_off_on_and_the_text_follows() {
        let s = SpeakerSetting::new(None, None);
        let press = chosen_call(ROW);
        let text = text_call(ROW);
        let value = |s: &SpeakerSetting| row_texts(&text, ESP, s.enabled()).unwrap()[1].1;
        assert_eq!(value(&s), ON_ID);
        assert!(chosen(chosen_row(&press, ESP), &s));
        assert_eq!(value(&s), OFF_ID);
        assert!(chosen(chosen_row(&press, ESP), &s));
        assert_eq!(value(&s), ON_ID);
        assert!(chosen(chosen_row(&press, ESP), &s));
        assert_eq!(value(&s), OFF_ID);
    }

    #[test]
    fn a_locked_row_is_handled_but_changes_nothing() {
        // H2_PAD_SPEAKER=0: the row shows off; pressing it must not open
        // the vibration dialog (the original's fall-through) or turn it on.
        let s = SpeakerSetting::new(Some(false), None);
        assert!(chosen(chosen_row(&chosen_call(ROW), ESP), &s));
        assert!(!s.enabled());
        assert_eq!(
            row_texts(&text_call(ROW), ESP, s.enabled()).unwrap()[1].1,
            OFF_ID
        );
    }

    #[test]
    fn the_row_argument_is_the_second_one() {
        // [esp+4] is the handler's first argument (not a row); a first
        // argument that happens to point at a 6 must not count.
        let mut f = chosen_call(2);
        f.set_u32(ESP + 4, 0x4000);
        f.set_u16(0x4000, ROW as u16);
        assert_eq!(chosen_row(&f, ESP), Some(2));
    }
}
