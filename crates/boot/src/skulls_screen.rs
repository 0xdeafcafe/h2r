//! The Skulls screen on our own list class, and its row on the campaign
//! difficulty list as plan hooks (docs/skulls.md "Menu").
//!
//! The screen shows the skulls of the profile whose player pressed the
//! row ([`skulls_menu::open_profile`], set before it opens; one is open at a
//! time). Its rows, title and presses are [`skulls_menu`]'s. The difficulty
//! list's fifth row (the count is still the shared list hook's) is drawn
//! and pressed by `before` hooks: a press opens the class's loader, never
//! the original (which would clamp the row and start the campaign).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use engine::entry::{EntryFuture, GuestCall, Ret};
use feature::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use settings::Store;

use crate::layout_glue::chosen_row;
use crate::list_class::{self, ListClass, Rows};
use crate::skulls;
use crate::skulls_menu::{self, ROW, TITLE_ID};
use crate::ui::Chosen;

/// The screen id `Open` rows and the loader registry know it by.
pub const SCREEN: &str = "skulls";

/// The open profile's skulls.
pub struct SkullsRows(pub Arc<Store>);

impl Rows for SkullsRows {
    fn title(&self) -> u32 {
        TITLE_ID
    }
    fn count(&self) -> usize {
        skulls_menu::open_profile().map_or(0, |p| skulls::screen_for(&self.0, &p).rows().len())
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        skulls_menu::open_profile()
            .and_then(|p| skulls_menu::row_texts(&self.0, &p, row))
            .map_or_else(Vec::new, |t| t.to_vec())
    }
    fn press(&self, row: i16) -> Chosen {
        match skulls_menu::open_profile().and_then(|p| skulls_menu::press(&self.0, &p, row)) {
            Some(msg) => Chosen::Stepped(msg),
            None => Chosen::Nothing,
        }
    }
}

/// The screen's class.
pub fn class(store: Arc<Store>) -> ListClass {
    ListClass::with_rows("skulls-screen", SCREEN, Arc::new(SkullsRows(store)))
}

/// What opening the screen for a press by local player `player` (with
/// `profile` on its slot) does: the mask and profile to open with, or the
/// line that says why not.
pub fn open_for(
    store: &Store,
    player: Option<u32>,
    profile: Option<String>,
) -> Result<(u32, String), String> {
    let (Some(player), Some(profile)) = (player, profile) else {
        return Err("skulls menu: the pressing player's index is unreadable; not opened".into());
    };
    if skulls::screen_for(store, &profile).rows().is_empty() {
        return Err(format!(
            "skulls menu: {profile} has no skulls yet; not opened"
        ));
    }
    Ok((1 << player, profile))
}

/// The Skulls row on the difficulty list.
pub struct DifficultyRow(pub Arc<Store>);

struct Seen {
    text: AtomicBool,
    chosen: AtomicBool,
}

fn once(seen: &AtomicBool, what: &str) {
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("{what}");
    }
}

fn row_text(c: &mut Call, seen: &Seen) -> Result<Flow, GuestFault> {
    let (item, row) = {
        let mem = c.ctx.runtime().memory();
        let item = mem.u32(c.cpu.esp + 4).ok();
        (item, crate::video_screen::text_row(&*mem, c.cpu.esp))
    };
    let (Some(item), Some(ROW)) = (item, row) else {
        return Ok(Flow::Continue);
    };
    once(&seen.text, "skulls menu: difficulty row text");
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            list_class::draw_texts(&cx, item, vec![(0, TITLE_ID)]).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn row_chosen(c: &mut Call, store: &Store, seen: &Seen) -> Result<Flow, GuestFault> {
    let (row, player, profile) = {
        let mem = c.ctx.runtime().memory();
        let player = skulls_menu::player_index(&*mem, c.cpu.esp);
        (
            chosen_row(&*mem, c.cpu.esp),
            player,
            player.and_then(|p| skulls_menu::profile_of(&*mem, p)),
        )
    };
    if row != Some(ROW) {
        return Ok(Flow::Continue);
    }
    once(&seen.chosen, "skulls menu: difficulty row chosen");
    // The store is read with guest memory released.
    let (mask, profile) = match open_for(store, player, profile) {
        Ok(v) => v,
        Err(why) => {
            eprintln!("{why}");
            return c.ret(8).map(Flow::Return);
        }
    };
    let Some(loader) = list_class::loader_of(SCREEN) else {
        eprintln!("skulls menu: its list class is off; not opened");
        return c.ret(8).map(Flow::Return);
    };
    skulls_menu::set_open(Some(profile.clone()));
    eprintln!("skulls menu: opening for {profile}");
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.call(GuestCall::new(engine::points::OPEN_SCREEN.base).args([mask, loader]))
                .await?;
            Ok(Ret::new(0, 8))
        })
    })
}

impl feature::Feature for DifficultyRow {
    fn manifest(&self) -> Manifest {
        Manifest::new("skulls-row", "The Skulls row on the difficulty list").doc("docs/skulls.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let seen = Arc::new(Seen {
            text: AtomicBool::new(false),
            chosen: AtomicBool::new(false),
        });
        let s = Arc::clone(&seen);
        b.raw().before(
            &engine::points::DIFFICULTY_ROW_TEXT,
            "the Skulls row's text",
            move |c| row_text(c, &s),
        );
        let store = Arc::clone(&self.0);
        b.raw().before(
            &engine::points::DIFFICULTY_ROW_CHOSEN,
            "the Skulls row's press",
            move |c| row_chosen(c, &store, &seen),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Arc<Store> {
        Arc::new(Store::defaults(crate::setting_defs::ALL))
    }

    #[test]
    fn nothing_opens_without_a_player_or_a_skull() {
        let s = store();
        assert!(open_for(&s, None, Some("a".into())).is_err());
        assert!(open_for(&s, Some(0), None).is_err());
        // A fresh profile has collected nothing.
        let e = open_for(&s, Some(1), Some("fresh".into())).unwrap_err();
        assert!(e.contains("fresh has no skulls yet"), "{e}");
    }

    #[test]
    fn with_no_open_profile_the_screen_has_no_rows() {
        skulls_menu::set_open(None);
        let r = SkullsRows(store());
        assert_eq!(r.count(), 0);
        assert!(r.texts(0).is_empty());
        assert_eq!(r.press(0), Chosen::Nothing);
        assert_eq!(r.title(), TITLE_ID);
    }

    #[test]
    fn the_row_and_the_class_are_on_the_plan() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        let c = class(store());
        r.add(&c).unwrap();
        r.add(&DifficultyRow(store())).unwrap();
        let text = r.build(&feature::Base).unwrap().text();
        assert!(text.contains(
            "0x002c9ce7 DIFFICULTY_ROW_TEXT: before [skulls-row:the Skulls row's text] -> original\n"
        ), "{text}");
        assert!(text.contains(
            "0x002c9d38 DIFFICULTY_ROW_CHOSEN: before [skulls-row:the Skulls row's press] -> original\n"
        ));
        assert!(text.contains("entry skulls-screen:loader"));
        assert_eq!(list_class::loader_of(SCREEN), Some(c.loader()));
    }
}
