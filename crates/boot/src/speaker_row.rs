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

// ---- the main Controls screen shows all seven rows ----
//
// A list shows one row per child item widget of its handler, up to the
// visible count in the tag's list block (`0x24bc12` lays them out, one
// item height apart). The Controls handler embeds six item widgets
// (`0x2c84b8`, vector constructor at `+0x88`) and links them as children in
// its "add items" method (`0x24bb60`, vtable slot 17). So for seven rows at
// once we add a seventh item widget of our own after the six, and raise the
// tag's visible count from six to seven. The list then lays it out below
// the sixth, the same distance apart, and never scrolls.

/// The Controls screen.
pub const SCREEN_ID: i16 = 39;
/// The Controls list handler's vtable, which marks it in the shared "add
/// items" method.
pub const HANDLER_VTABLE: u32 = 0x0045_d240;
/// List handler: link its item widgets as children (thiscall, plain `ret`).
pub const ADD_ITEMS: u32 = 0x0024_bb60;
/// Item widget constructor (ECX = item, plain `ret`).
pub const ITEM_CTOR: u32 = 0x0024_abae;
/// Widget: set the local player word at `+8`, recursively (ECX = widget,
/// the word on the stack, `ret 4`).
pub const SET_PLAYER: u32 = 0x0022_ee92;
/// Widget: append EAX to ESI's children (unlinking it first; plain `ret`).
pub const ADD_CHILD: u32 = 0x0022_eb18;
pub const ITEM_SIZE: u32 = 0x80;
const VISIBLE: u32 = 6;

/// A widget's children, in order (`+0x14` first child, `+0x18` next
/// sibling). At most 64; `None` if a link cannot be read.
pub fn children<M: Gm>(m: &M, widget: u32) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    let mut c = m.u32(widget.wrapping_add(0x14))?;
    while c != 0 && out.len() < 64 {
        out.push(c);
        c = m.u32(c.wrapping_add(0x18))?;
    }
    Some(out)
}

/// Raises the visible count of the Controls screen's list in its tag from
/// six to seven, in every pane. Returns how many lists changed (0 for any
/// other screen, or once done). Tag layout as in `layout_glue::extend_panes`;
/// a pane's lists are counted at `+0xc` and pointed to by `+0x10`, and a
/// list block's visible count is the `i16` at `+6`.
pub fn grow_visible<M: Gm>(m: &mut M, wgit: u32) -> Option<u32> {
    if m.u16(wgit.wrapping_add(4))? as i16 != SCREEN_ID {
        return Some(0);
    }
    let (count, panes) = (m.u32(wgit + 0x20)?, m.u32(wgit + 0x24)?);
    let mut changed = 0;
    for p in 0..count.min(16) {
        let pane = panes + p * 0x4c;
        if m.u32(pane + 0xc)? as i32 <= 0 {
            continue;
        }
        let block = m.u32(pane + 0x10)?;
        if u32::from(m.u16(block + 6)?) == VISIBLE {
            if !m.set_u16(block + 6, VISIBLE as u16 + 1) {
                return None;
            }
            changed += 1;
        }
    }
    Some(changed)
}

/// What the "add items" hook does after the original for `handler`:
/// `None` for any other list; `Some(true)` when our item must be built and
/// linked, `Some(false)` when it is already a child and only has to move
/// back to the end (the original re-appends the six retail items).
pub fn extra_item_needed<M: Gm>(m: &M, handler: u32, item: u32) -> Option<bool> {
    if m.u32(handler)? != HANDLER_VTABLE {
        return None;
    }
    Some(!children(m, handler)?.contains(&item))
}

/// The slot for `handler`'s extra item: the one it had, else the next of
/// `slots` in turn (a screen closed long ago gives its slot up).
pub fn extra_slot(table: &mut Vec<(u32, u32)>, next: &mut u32, handler: u32, base: u32, slots: u32) -> u32 {
    if let Some((_, a)) = table.iter().find(|(h, _)| *h == handler) {
        return *a;
    }
    let a = base + (*next % slots) * ITEM_SIZE;
    *next += 1;
    table.retain(|(_, s)| *s != a);
    table.push((handler, a));
    a
}

// ---- the smaller Controls screen (in-game, screen 200) ----
//
// `ui\screens\multiplayer\mp_menu\controller_settings`, the dialog the pause
// menu opens: one text per row, five visible (it already scrolls). Its
// handler (`0x2b48c2`) builds a datum array (`0x45b8b0`, count 6) and gives
// each datum a value word at `+2`, 0 to 5; the row text (`0x2b49e6`) looks
// the value up in a table of string ids, and the row chosen (`0x2b4a4d`)
// switches on it. We add a seventh datum with value 6 after the
// constructor, draw it with our own text and toggle on it.

pub const SMALL_LIST_NAME: u32 = 0x0045_b8b0;
pub const SMALL_CTOR: u32 = 0x002b_48c2;
pub const SMALL_ROW_TEXT: u32 = 0x002b_49e6;
pub const SMALL_ROW_CHOSEN: u32 = 0x002b_4a4d;
/// Datum array: new datum (EDX = array, EAX = handle).
pub const DATUM_NEW: u32 = 0x0016_b840;
pub const SMALL_VALUE: u16 = 6;
pub const SMALL_ON_ID: u32 = crate::layout_glue::ID_BASE + 0x21;
pub const SMALL_OFF_ID: u32 = crate::layout_glue::ID_BASE + 0x22;
pub const SMALL_ON_TEXT: &str = "Controller Speaker: On";
pub const SMALL_OFF_TEXT: &str = "Controller Speaker: Off";

/// The small list's item count: one more for its datum array.
pub fn small_list_count(name: u32, count: u32) -> u32 {
    if name == SMALL_LIST_NAME && count == VANILLA_ROWS { count + 1 } else { count }
}

/// The value word of datum `handle` in `array` (`[array+0x44]` holds
/// four-byte datums; the value is at `+2`).
pub fn datum_value<M: Gm>(m: &M, array: u32, handle: u32) -> Option<u16> {
    if handle == u32::MAX {
        return None;
    }
    let data = m.u32(array.wrapping_add(0x44))?;
    m.u16(data.wrapping_add((handle & 0xffff) * 4 + 2))
}

pub fn set_datum_value<M: Gm>(m: &mut M, array: u32, handle: u32, v: u16) -> bool {
    match m.u32(array.wrapping_add(0x44)) {
        Some(data) if handle != u32::MAX => m.set_u16(data.wrapping_add((handle & 0xffff) * 4 + 2), v),
        _ => false,
    }
}

/// The small row text call (ECX = handler, item at `[esp+4]`): our text
/// for our datum, `None` for the retail rows.
pub fn small_row_text<M: Gm>(m: &M, handler: u32, esp: u32, on: bool) -> Option<(u32, u32)> {
    let item = m.u32(esp.wrapping_add(4))?;
    let value = datum_value(m, m.u32(handler.wrapping_add(0x70))?, m.u32(item.wrapping_add(ITEM_ROW))?)?;
    (value == SMALL_VALUE).then_some((LABEL_TEXT_INDEX, if on { SMALL_ON_ID } else { SMALL_OFF_ID }))
}

/// The small row-chosen call (ECX = handler, a pointer to the datum handle
/// at `[esp+8]`): true for our datum.
pub fn small_is_ours<M: Gm>(m: &M, handler: u32, esp: u32) -> bool {
    let value = (|| {
        let p = m.u32(esp.wrapping_add(8))?;
        datum_value(m, m.u32(handler.wrapping_add(0x70))?, m.u32(p)?)
    })();
    value == Some(SMALL_VALUE)
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

    /// A screen tag with `panes` panes, each with one list of `visible`
    /// rows, the screen id at `+4`.
    fn tag(f: &mut Fake, id: i16, panes: u32, visible: u16) -> u32 {
        let (wgit, pane0, blocks) = (0x10_0000u32, 0x11_0000u32, 0x12_0000u32);
        f.set_u16(wgit + 4, id as u16);
        f.set_u32(wgit + 0x20, panes);
        f.set_u32(wgit + 0x24, pane0);
        for p in 0..panes {
            let pane = pane0 + p * 0x4c;
            let block = blocks + p * 0x20;
            f.set_u32(pane + 0xc, 1);
            f.set_u32(pane + 0x10, block);
            f.set_u16(block + 4, 2);
            f.set_u16(block + 6, visible);
        }
        wgit
    }

    #[test]
    fn the_controls_list_shows_seven_rows_once() {
        let mut f = Fake::default();
        let wgit = tag(&mut f, SCREEN_ID, 1, 6);
        assert_eq!(grow_visible(&mut f, wgit), Some(1));
        assert_eq!(f.u16(0x12_0000 + 6), Some(7));
        assert_eq!(f.u16(0x12_0000 + 4), Some(2), "the skin stays");
        // Again: already seven, nothing changes.
        assert_eq!(grow_visible(&mut f, wgit), Some(0));
        assert_eq!(f.u16(0x12_0000 + 6), Some(7));
    }

    #[test]
    fn other_screens_and_other_counts_keep_their_lists() {
        let mut f = Fake::default();
        let wgit = tag(&mut f, 40, 1, 6);
        assert_eq!(grow_visible(&mut f, wgit), Some(0));
        assert_eq!(f.u16(0x12_0000 + 6), Some(6));
        let mut f = Fake::default();
        let wgit = tag(&mut f, SCREEN_ID, 2, 5);
        assert_eq!(grow_visible(&mut f, wgit), Some(0));
        assert_eq!(f.u16(0x12_0000 + 6), Some(5));
        // A pane without a list is skipped.
        let mut f = Fake::default();
        let wgit = tag(&mut f, SCREEN_ID, 1, 6);
        f.set_u32(0x11_0000 + 0xc, 0);
        assert_eq!(grow_visible(&mut f, wgit), Some(0));
        assert_eq!(f.u16(0x12_0000 + 6), Some(6));
    }

    /// A handler with `n` children, linked through +0x14 and +0x18.
    fn handler(f: &mut Fake, vtable: u32, kids: &[u32]) -> u32 {
        let h = 0x20_0000;
        f.set_u32(h, vtable);
        f.set_u32(h + 0x14, kids.first().copied().unwrap_or(0));
        for (i, k) in kids.iter().enumerate() {
            f.set_u32(k + 0x18, kids.get(i + 1).copied().unwrap_or(0));
        }
        h
    }

    #[test]
    fn our_item_is_built_once_then_kept_last() {
        let retail: Vec<u32> = (0..6).map(|i| 0x20_0088 + i * ITEM_SIZE).collect();
        let ours = 0xa2f2_0000;
        let mut f = Fake::default();
        let h = handler(&mut f, HANDLER_VTABLE, &retail);
        assert_eq!(children(&f, h).unwrap(), retail);
        assert_eq!(extra_item_needed(&f, h, ours), Some(true));
        let mut with_ours = retail.clone();
        with_ours.push(ours);
        let h = handler(&mut f, HANDLER_VTABLE, &with_ours);
        assert_eq!(extra_item_needed(&f, h, ours), Some(false));
        // Any other list handler is left alone.
        let h = handler(&mut f, 0x0045_cdc0, &retail);
        assert_eq!(extra_item_needed(&f, h, ours), None);
    }

    #[test]
    fn each_open_screen_gets_its_own_slot() {
        let (mut table, mut next) = (Vec::new(), 0);
        let a = extra_slot(&mut table, &mut next, 0x100, 0x1000, 2);
        assert_eq!(a, 0x1000);
        assert_eq!(extra_slot(&mut table, &mut next, 0x100, 0x1000, 2), a, "same handler, same slot");
        let b = extra_slot(&mut table, &mut next, 0x200, 0x1000, 2);
        assert_eq!(b, 0x1000 + ITEM_SIZE);
        // A third screen takes the oldest slot back.
        assert_eq!(extra_slot(&mut table, &mut next, 0x300, 0x1000, 2), a);
        assert_eq!(table.len(), 2);
        assert!(!table.iter().any(|(h, _)| *h == 0x100));
    }

    /// The smaller screen: a datum array at `ARRAY` with data at `DATA`
    /// holding values 0 to 6, its handler at `HANDLER`, and an item showing
    /// datum `row`.
    const HANDLER: u32 = 0x30_0000;
    const ARRAY: u32 = 0x31_0000;
    const DATA: u32 = 0x32_0000;
    fn small(row: u32) -> Fake {
        let mut f = Fake::default();
        f.set_u32(HANDLER + 0x70, ARRAY);
        f.set_u32(ARRAY + 0x44, DATA);
        for i in 0..7u32 {
            f.set_u16(DATA + i * 4, 0x8000 | i as u16);
            assert!(set_datum_value(&mut f, ARRAY, 0x0001_0000 | i, i as u16));
        }
        f.set_u32(ESP + 4, ITEM);
        f.set_u32(ITEM + 0x70, 0x0001_0000 | row);
        f.set_u32(ESP + 8, ROW_PTR);
        f.set_u32(ROW_PTR, 0x0001_0000 | row);
        f
    }

    #[test]
    fn the_small_list_grows_by_one() {
        assert_eq!(small_list_count(SMALL_LIST_NAME, 6), 7);
        assert_eq!(small_list_count(SMALL_LIST_NAME, 5), 5);
        assert_eq!(small_list_count(LIST_NAME, 6), 7 - 1);
        assert_eq!(list_count(SMALL_LIST_NAME, 6), 6);
    }

    #[test]
    fn the_small_row_shows_and_toggles() {
        let f = small(6);
        assert_eq!(datum_value(&f, ARRAY, 0x0001_0006), Some(6));
        assert_eq!(small_row_text(&f, HANDLER, ESP, true), Some((0, SMALL_ON_ID)));
        assert_eq!(small_row_text(&f, HANDLER, ESP, false), Some((0, SMALL_OFF_ID)));
        assert!(small_is_ours(&f, HANDLER, ESP));
        let s = SpeakerSetting::new(None, None);
        for want in [false, true, false] {
            assert!(small_is_ours(&f, HANDLER, ESP));
            s.toggle();
            assert_eq!(small_row_text(&f, HANDLER, ESP, s.enabled()).unwrap().1, if want { SMALL_ON_ID } else { SMALL_OFF_ID });
        }
        assert!(crate::layout_glue::is_ours(SMALL_ON_ID) && crate::layout_glue::is_ours(SMALL_OFF_ID));
    }

    #[test]
    fn retail_small_rows_go_to_the_original() {
        for row in 0..6 {
            let f = small(row);
            assert_eq!(small_row_text(&f, HANDLER, ESP, true), None, "{row}");
            assert!(!small_is_ours(&f, HANDLER, ESP), "{row}");
        }
        // A free item (handle -1) and unreadable memory.
        let mut f = small(6);
        f.set_u32(ITEM + 0x70, u32::MAX);
        f.set_u32(ROW_PTR, u32::MAX);
        assert_eq!(small_row_text(&f, HANDLER, ESP, true), None);
        assert!(!small_is_ours(&f, HANDLER, ESP));
        assert_eq!(small_row_text(&Fake::default(), HANDLER, ESP, true), None);
        assert!(!small_is_ours(&Fake::default(), HANDLER, ESP));
        assert!(!set_datum_value(&mut Fake::default(), ARRAY, 0, 6));
    }
}
