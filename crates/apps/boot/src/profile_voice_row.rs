//! The "Voice Chat" row on a player profile's Edit Profile screen, right
//! after Voice Masking, opening our Voice screen (`crate::voice_settings`;
//! docs/game/settings-menu.md "Edit Profile").
//!
//! **The list** (screen 38, `player_profile\edit_profile_menu`; constructor
//! `0x237758`, list handler constructor `0x2b7900`, vtable `0x45bea0`):
//! the rows' datum array is named at `0x45bfd8` and made with
//! [`RETAIL_ROWS`] rows (`0x1a480d`), and the constructor adds a datum for
//! every row the array holds (its loop runs to the array's maximum,
//! `+0x20`). The row text (`0x2b7990`) and row chosen (`0x2b79f9`) tell
//! the rows apart by index: 0 Controls, 1 Appearance, 2 Voice Output,
//! 3 Voice Masking, 4 Online Status, 5 Subtitles. Any other index gets the
//! null text and a press that does nothing.
//!
//! **Our row** is the `ui` model's row of [`List::Profile`] at index
//! [`OURS`] (`ui::Extension::at`), answered by `crate::retail_rows`
//! `before` hooks on the plan at the row text and row chosen
//! ([`extension`]). The array grows by one ([`list_count`], through
//! `layout_glue`'s datum array hook), so the constructor's own loop makes
//! the seventh datum. Online Status and Subtitles then show at 5 and 6,
//! past the indexes the original knows, so their text and press go to the
//! original with the handle of the retail row they show
//! (`retail_rows::moved`): the item's row word (`+0x70`) swapped for the
//! call and put back, or the press's row word pointing at a word holding
//! the retail handle.
//!
//! **The pane.** Each retail row has its own pane (`0x230427` with the
//! row): our row keeps the one shown ([`pane`]), as the Settings list's
//! Video row does, and the moved retail rows ask for their own.
//!
//! The Voice settings are the install's (`Scope::Host`), not the
//! profile's: every profile's row opens the same screen.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::ui::{List, Model};

/// The feature (`features.profile-voice-row` switches it).
pub const FEATURE: &str = "profile-voice-row";
/// The Edit Profile screen's id.
pub const SCREEN_ID: i16 = 38;
/// The rows' datum array name (`0x2b7933` pushes it).
pub const LIST_NAME: u32 = 0x0045_bfd8;
/// The retail rows: Controls, Appearance, Voice Output, Voice Masking,
/// Online Status, Subtitles.
pub const RETAIL_ROWS: u32 = 6;
pub const RETAIL: [&str; RETAIL_ROWS as usize] = [
    "Controls",
    "Appearance",
    "Voice Output",
    "Voice Masking",
    "Online Status",
    "Subtitles",
];
/// Voice Masking's index: ours goes right after it.
pub const VOICE_MASKING: i16 = 3;
/// Our row's index.
pub const OURS: i16 = VOICE_MASKING + 1;
/// Our row's label: beside Voice Output (where voice plays) and Voice
/// Masking (how it sounds), the screen that sets up the chat itself.
pub const LABEL: &str = "Voice Chat";
/// The screen the row opens.
pub const OPENS: &str = crate::video_screen::VOICE_SCREEN;

/// Whether the feature's hooks are on the plan (`layout_glue` sets it from
/// the registry's answer). The array and the panes change only then, so
/// without them the list stays retail.
static ON: AtomicBool = AtomicBool::new(false);

pub fn set_on(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// The model's rows for the Edit Profile list: Voice Chat after Voice
/// Masking.
pub fn rows() -> crate::ui::Extension {
    crate::ui::extend(List::Profile, RETAIL_ROWS)
        .at(OURS as u32)
        .row(crate::ui::Row::open(OPENS).label(LABEL))
}

/// The row's hooks on the plan: the shared retail-rows extension over
/// [`List::Profile`].
pub fn extension(model: Arc<Model>) -> crate::retail_rows::Extension {
    crate::retail_rows::Extension {
        id: FEATURE,
        title: "The Voice Chat row on Edit Profile",
        list: List::Profile,
        text: &engine::points::EDIT_PROFILE_ROW_TEXT,
        chosen: &engine::points::EDIT_PROFILE_ROW_CHOSEN,
        label_only: true,
        model,
    }
}

/// The count the datum array named `name` is made with, `count` what it
/// would be made with otherwise: Edit Profile's grows by our row while the
/// feature is on.
pub fn list_count(m: &Model, name: u32, count: u32) -> u32 {
    grown(m, ON.load(Ordering::Relaxed), name, count)
}

fn grown(m: &Model, on: bool, name: u32, count: u32) -> u32 {
    if on && name == LIST_NAME {
        m.count(List::Profile, count)
    } else {
        count
    }
}

/// The pane call for row `row` of screen `id`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    /// Keep the pane shown (our row has none).
    Keep,
    /// Show retail row `n`'s pane.
    Show(i16),
}

/// What the pane hook does for row `row` of screen `id`: `None` leaves the
/// call to the original as it is.
pub fn pane(m: &Model, id: i16, row: i16) -> Option<Pane> {
    pane_when(m, ON.load(Ordering::Relaxed), id, row)
}

fn pane_when(m: &Model, on: bool, id: i16, row: i16) -> Option<Pane> {
    if !on || id != SCREEN_ID {
        return None;
    }
    if m.row(List::Profile, row).is_some() {
        return Some(Pane::Keep);
    }
    match m.retail_index(List::Profile, row)? {
        r if r != row => Some(Pane::Show(r)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retail_rows;
    use crate::setting_defs::ALL;
    use crate::ui::{Chosen, RowKind};
    use crate::video_screen::{self, Switches};
    use engine::testing::FakeGuest;
    use settings::Store;

    fn model() -> Arc<Model> {
        Arc::new(video_screen::model(Arc::new(Store::defaults(ALL)), Switches::ALL).unwrap())
    }

    fn label(m: &Model, id: u32) -> String {
        m.strings()
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, t)| t.clone())
            .unwrap_or_default()
    }

    /// The list as a player reads it, top to bottom.
    #[test]
    fn voice_chat_sits_right_after_voice_masking() {
        let m = model();
        let shown: Vec<String> = (0..m.count(List::Profile, RETAIL_ROWS) as i16)
            .map(|i| match m.retail_index(List::Profile, i) {
                Some(r) => RETAIL[r as usize].to_string(),
                None => {
                    let t = retail_rows::texts(&m, List::Profile, i, true).expect("ours");
                    label(&m, t[0].1)
                }
            })
            .collect();
        assert_eq!(
            shown,
            [
                "Controls",
                "Appearance",
                "Voice Output",
                "Voice Masking",
                "Voice Chat",
                "Online Status",
                "Subtitles",
            ]
        );
        assert_eq!(m.index_of(List::Profile, &RowKind::Open(OPENS)), Some(OURS));
        assert_eq!(OPENS, crate::voice_settings::SCREEN);
    }

    #[test]
    fn a_press_opens_the_voice_screen_and_retail_rows_go_to_the_original() {
        let m = model();
        assert_eq!(m.press(m.row(List::Profile, OURS).unwrap()), Chosen::Open(OPENS));
        for r in 0..OURS {
            assert_eq!(m.retail_index(List::Profile, r), Some(r), "unmoved {r}");
            assert_eq!(retail_rows::moved(&m, List::Profile, r), None);
        }
        assert_eq!(retail_rows::moved(&m, List::Profile, 5), Some(4), "Online Status");
        assert_eq!(retail_rows::moved(&m, List::Profile, 6), Some(5), "Subtitles");
        assert_eq!(m.retail_index(List::Profile, 7), None);
        assert_eq!(m.retail_index(List::Profile, -1), None);
        assert!(retail_rows::texts(&m, List::Profile, 5, true).is_none(), "retail's");
    }

    #[test]
    fn only_edit_profiles_array_grows_and_only_with_the_feature() {
        let m = model();
        assert_eq!(grown(&m, true, LIST_NAME, RETAIL_ROWS), RETAIL_ROWS + 1);
        assert_eq!(grown(&m, false, LIST_NAME, RETAIL_ROWS), RETAIL_ROWS);
        assert_eq!(grown(&m, true, 0x0045_b8b0, 6), 6, "the in-game Settings dialog");
        assert_eq!(grown(&m, true, crate::speaker_row::LIST_NAME, 6), 6, "Controls");
        assert_eq!(grown(&m, true, LIST_NAME, 4), 4, "not the retail count");
        // No Voice screen: no row, and the list stays retail.
        let none = video_screen::model(
            Arc::new(Store::defaults(ALL)),
            Switches {
                voice_screen: false,
                ..Switches::ALL
            },
        )
        .unwrap();
        assert!(none.rows(List::Profile).is_empty());
        assert_eq!(grown(&none, true, LIST_NAME, RETAIL_ROWS), RETAIL_ROWS);
    }

    #[test]
    fn our_row_keeps_the_pane_and_moved_rows_ask_for_their_own() {
        let m = model();
        assert_eq!(pane_when(&m, true, SCREEN_ID, OURS), Some(Pane::Keep));
        assert_eq!(pane_when(&m, true, SCREEN_ID, 5), Some(Pane::Show(4)));
        assert_eq!(pane_when(&m, true, SCREEN_ID, 6), Some(Pane::Show(5)));
        for r in 0..OURS {
            assert_eq!(pane_when(&m, true, SCREEN_ID, r), None, "row {r} is retail's own");
        }
        assert_eq!(pane_when(&m, true, 39, OURS), None, "another screen");
        assert_eq!(pane_when(&m, false, SCREEN_ID, OURS), None, "feature off");
    }

    #[test]
    fn a_moved_row_takes_the_retail_rows_handle() {
        use engine::datum::DatumArray;
        let mut g = FakeGuest::new();
        let (list, array, name) = (0x1000, 0x5000_0000, 0x5000_0800);
        g.map(list, 0x100).map(array, 0x1000);
        let a = DatumArray::at(array);
        // Seven rows of four bytes, as the constructor makes them with ours.
        a.initialize_contiguous(&mut g, name, 7, 4, 0, 0).unwrap();
        a.make_valid(&mut g).unwrap();
        a.delete_all(&mut g).unwrap();
        for _ in 0..7 {
            a.new_datum(&mut g).unwrap().unwrap();
        }
        g.put_u32(list + retail_rows::LIST_ROWS, array);
        let h = retail_rows::retail_handle(&g, list, 4).expect("Online Status's handle");
        assert_eq!(h & 0xffff, 4, "the original reads the low word");
        assert_eq!(Some(h), a.handle_of(&g, 4).unwrap().map(|d| d.0));
    }

    #[test]
    fn the_hooks_are_on_the_plan_before_the_original() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        r.add(&extension(model())).unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
        assert!(
            text.contains(
                "0x002b7990 EDIT_PROFILE_ROW_TEXT: before [profile-voice-row:our rows' text] -> original\n"
            ),
            "{text}"
        );
        assert!(
            text.contains(
                "0x002b79f9 EDIT_PROFILE_ROW_CHOSEN: before [profile-voice-row:our rows' press] -> original\n"
            ),
            "{text}"
        );
    }
}
