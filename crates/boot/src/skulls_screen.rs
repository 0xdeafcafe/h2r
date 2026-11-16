//! The Skulls screen on our own list class, and its row on the campaign
//! difficulty list as plan hooks (docs/skulls.md "Menu").
//!
//! The screen is a [`SkullsScreen`]: all fifteen skulls, Locked, On or Off
//! for the profile whose player pressed the row ([`SkullsScreen::open`],
//! set before it opens; one is open at a time), with a description text of
//! the focused skull. Its rows, title, presses and descriptions are
//! [`skulls_menu`]'s. The difficulty
//! list's fifth row (the count is still the shared list hook's) is drawn
//! and pressed by `before` hooks: a press opens the class's loader, never
//! the original (which would clamp the row and start the campaign).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::layout_glue::chosen_row;
use crate::list_class::{self, ListClass, TextBlock};
use crate::skulls_menu::{self, ROW, SkullsScreen, TITLE_ID};
use engine::entry::{EntryFuture, GuestCall, Ret};
use feature::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;

/// The screen id `Open` rows and the loader registry know it by.
pub const SCREEN: &str = "skulls";

/// The screen's class, with its description text `block`.
pub fn class(screen: Arc<SkullsScreen>, block: TextBlock) -> ListClass {
    ListClass::with_rows(skulls_menu::FEATURE, SCREEN, screen).description(block)
}

/// What opening the screen for a press by local player `player` (with
/// `profile` on its slot) does: the mask and profile to open with, or the
/// line that says why not. A profile with nothing collected still opens:
/// every skull is listed, locked.
pub fn open_for(player: Option<u32>, profile: Option<String>) -> Result<(u32, String), String> {
    let (Some(player), Some(profile)) = (player, profile) else {
        return Err("skulls menu: the pressing player's index is unreadable; not opened".into());
    };
    Ok((1 << player, profile))
}

/// The Skulls row on the difficulty list.
pub struct DifficultyRow(pub Arc<SkullsScreen>);

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

fn row_chosen(c: &mut Call, screen: &SkullsScreen, seen: &Seen) -> Result<Flow, GuestFault> {
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
    let (mask, profile) = match open_for(player, profile) {
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
    eprintln!(
        "skulls menu: opening the Skulls screen for {profile} (player {})",
        mask.trailing_zeros()
    );
    screen.open(profile);
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
        let screen = Arc::clone(&self.0);
        b.raw().before(
            &engine::points::DIFFICULTY_ROW_CHOSEN,
            "the Skulls row's press",
            move |c| row_chosen(c, &screen, &seen),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_class::Rows;
    use crate::skulls;
    use settings::Store;

    fn screen() -> Arc<SkullsScreen> {
        Arc::new(SkullsScreen::new(Arc::new(Store::defaults(
            crate::setting_defs::ALL,
        ))))
    }

    #[test]
    fn a_profile_with_nothing_collected_still_opens() {
        assert!(open_for(None, Some("a".into())).is_err());
        assert!(open_for(Some(0), None).is_err());
        assert_eq!(
            open_for(Some(1), Some("fresh".into())),
            Ok((2, "fresh".into()))
        );
    }

    #[test]
    fn with_no_open_profile_every_skull_is_locked() {
        let r = screen();
        assert_eq!(r.count(), skulls::COUNT);
        assert_eq!(r.texts(0)[1].1, skulls_menu::LOCKED_ID);
        assert!(matches!(r.press(0), crate::ui::Chosen::Stepped(m) if m.contains("no profile")));
        assert_eq!(r.title(), TITLE_ID);
    }

    #[test]
    fn the_row_and_the_class_are_on_the_plan() {
        let mut r = feature::Registry::new(feature::Switches::defaults());
        let s = screen();
        let c = class(Arc::clone(&s), skulls_menu::DESCRIPTION_TEXT);
        r.add(&c).unwrap();
        r.add(&DifficultyRow(s)).unwrap();
        let text = r.build(&feature::Base).unwrap().text();
        assert!(
            text.contains(
                "0x002c9ce7 DIFFICULTY_ROW_TEXT: before [skulls-row:the Skulls row's text] -> original\n"
            ),
            "{text}"
        );
        assert!(text.contains(
            "0x002c9d38 DIFFICULTY_ROW_CHOSEN: before [skulls-row:the Skulls row's press] -> original\n"
        ));
        assert!(text.contains("entry skulls-screen:loader"));
        assert_eq!(list_class::loader_of(SCREEN), Some(c.loader()));
    }
}
