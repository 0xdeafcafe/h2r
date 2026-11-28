//! Our Game Setup rows' picture and description (Clanker Options, Halo 1
//! Options), shown while the row has the focus as retail's rows show
//! theirs (docs/game/ui-builder.md "Game Setup's picture and
//! description").
//!
//! **Retail's** (read from the XBE, 2026-11-19). The Game Setup dialog
//! (`squad_settings_dialog`, screen 23) does it in its update
//! (`SQUAD_SETTINGS_UPDATE`, `0x2bbbf6`, vtable `0x45c524`): it asks the
//! focused row's action (`0x2bbaad`, EAX the list at screen `+0x610`),
//! finds its text widgets 2, 3 and 4 and bitmap widget 4 (`0x22edb8`,
//! kinds 6 and 8), and by the action (a jump table of ten at
//! `0x2bbea8`) sets text 2, the description under the list
//! (`change_map_help`'s block, `[-40 -241 -173 0]`), texts 3 and 4 (a
//! "current" label and value, beside the picture) through `TEXT_SET`,
//! and the picture: the map's for Change Map, the game type's for Change
//! Rules, else image N of `xbox_live.bitm` (the Xbox Live menu's
//! pictures, eight images; `0x2b0a14` stores N at bitmap widget `+0x88`
//! when it is under the bitmap's image count). Quick Options shows image
//! 2, Switch To: Co-Op 7, Switch To: Custom Game 6, Matchmaking 0. An
//! action past 9 (ours) gets no texts and image 0.
//!
//! **Ours.** A row of ours registers its action, a picture of that set
//! and its description ([`register`]). The hook lets the update run, then
//! for one of our actions sets text 2 to our description and the picture
//! to our image; texts 3 and 4 stay empty, as retail leaves them for a
//! row with no current value. Clanker Options takes image 1, the one
//! picture of the set no retail row uses (a Spartan over a roster of
//! players); Halo 1 Options takes Quick Options' own (image 2), as it is
//! that row's kind of options.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::entry::{Cx, EntryError, EntryFuture, GuestCall, Ret};
use guest_layout::memory::GuestMemory;
use guest_layout::points;
use hooks::{BuildError, Builder, Flow, Manifest};

/// The feature (`features.game-setup-help` switches it).
pub const FEATURE: &str = "game-setup-help";

/// The focused row's action: EAX the list, plain `ret`, the action in EAX
/// (-1 for none).
pub const FOCUSED_ACTION: u32 = 0x002b_baad;
/// The Game Setup list handler's place in its screen.
pub const SETUP_LIST: u32 = 0x610;
/// The update's widgets: text widget 2 (the description) and bitmap
/// widget 4 (the picture), by kind and index (`ITEM_TEXT`).
pub const TEXT_KIND: u32 = 6;
pub const DESCRIPTION_TEXT: u32 = 2;
pub const BITMAP_KIND: u32 = 8;
pub const PICTURE_BITMAP: u32 = 4;
/// A bitmap widget's image index and [`set_image`], shared with our list
/// class's pictures.
pub use crate::list_class::{WIDGET_IMAGE, set_image};

/// `xbox_live.bitm`'s pictures, as the Game Setup rows use them.
pub mod image {
    /// Matchmaking (and action 9): a squad roster.
    pub const MATCHMAKING: u16 = 0;
    /// A Spartan over a roster of players; no retail Game Setup row uses
    /// it.
    pub const ROSTER: u16 = 1;
    /// Quick Options: the objectives (a flag, a skull, a disc) over a row
    /// of check boxes.
    pub const QUICK_OPTIONS: u16 = 2;
    /// Switch To: Custom Game: the skull and crown over crossed flags.
    pub const CUSTOM_GAME: u16 = 6;
    /// Switch To: Co-Op Game: two Spartans.
    pub const COOP: u16 = 7;
}

/// The description's string id for a row; written to guest memory first
/// when it needs to be (a kit screen's slots).
pub type HelpText = Arc<dyn Fn(&mut dyn GuestMemory) -> Option<u32> + Send + Sync>;

/// One row of ours: its action, its picture, its description.
#[derive(Clone)]
pub struct RowHelp {
    pub action: i16,
    pub image: u16,
    pub text: HelpText,
}

static ROWS: Mutex<Vec<RowHelp>> = Mutex::new(Vec::new());

fn rows() -> std::sync::MutexGuard<'static, Vec<RowHelp>> {
    ROWS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Registers (or replaces, by action) a row's picture and description.
pub fn register(action: i16, image: u16, text: HelpText) {
    let mut v = rows();
    v.retain(|r| r.action != action);
    v.push(RowHelp {
        action,
        image,
        text,
    });
}

/// The row of ours with `action`.
pub fn for_action(action: i16) -> Option<RowHelp> {
    rows().iter().find(|r| r.action == action).cloned()
}

/// The Game Setup screen whose update is being let through to retail's
/// (0: none): its hook passes it on once.
static PASSING: AtomicU32 = AtomicU32::new(0);

/// After retail's update on `screen`: our description and picture when
/// the focused row is one of ours.
async fn update(cx: Cx, screen: u32) -> Result<Ret, EntryError> {
    PASSING.store(screen, Ordering::SeqCst);
    let r = cx
        .call(GuestCall::new(points::SQUAD_SETTINGS_UPDATE.base).ecx(screen))
        .await;
    PASSING.store(0, Ordering::SeqCst);
    let eax = r?.eax;
    let action = cx
        .call(GuestCall::new(FOCUSED_ACTION).eax(screen.wrapping_add(SETUP_LIST)))
        .await?
        .eax as i32;
    let Some(row) = i16::try_from(action).ok().and_then(for_action) else {
        return Ok(Ret::new(eax, 0));
    };
    let text = cx
        .call(GuestCall::new(points::ITEM_TEXT.base).ecx(screen).args([
            TEXT_KIND,
            DESCRIPTION_TEXT,
            0,
        ]))
        .await?
        .eax;
    let picture = cx
        .call(GuestCall::new(points::ITEM_TEXT.base).ecx(screen).args([
            BITMAP_KIND,
            PICTURE_BITMAP,
            0,
        ]))
        .await?
        .eax;
    let id = cx.write(|m| (row.text)(m));
    if let (Some(id), true) = (id, text != 0) {
        cx.call(
            GuestCall::new(points::TEXT_SET.base)
                .edi(text)
                .esi(screen)
                .args([id]),
        )
        .await?;
    }
    if picture != 0 {
        cx.write(|m| set_image(m, picture, row.image))?;
    }
    Ok(Ret::new(eax, 0))
}

/// The hook on the Game Setup dialog's update (see the module docs).
pub struct GameSetupHelp;

impl hooks::Feature for GameSetupHelp {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            FEATURE,
            "A picture and a description for our Game Setup rows",
        )
        .doc("docs/game/ui-builder.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        b.raw().before(
            &points::SQUAD_SETTINGS_UPDATE,
            "our rows' picture and description",
            |c| {
                let screen = c.cpu.ecx;
                // Retail's own update, called from ours: let it run.
                if screen != 0
                    && PASSING
                        .compare_exchange(screen, 0, Ordering::SeqCst, Ordering::SeqCst)
                        .is_ok()
                {
                    return Ok(Flow::Continue);
                }
                if rows().is_empty() {
                    return Ok(Flow::Continue);
                }
                c.answer_async(move |cx| -> EntryFuture { Box::pin(update(cx, screen)) })
            },
        );
        // What it shows is read afresh at each update.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds [`GameSetupHelp`].
pub fn install(registry: &mut hooks::Registry) -> anyhow::Result<()> {
    registry
        .add(&GameSetupHelp)
        .map_err(|e| anyhow::anyhow!("game setup help: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_class::{BITMAP_IMAGES, WIDGET_BLOCK};
    use guest_layout::testing::FakeGuest;

    /// A bitmap block's `bitm` datum.
    const BLOCK_BITMAP: u32 = 0x1c;
    const WIDGET: u32 = 0x0010_0000;
    const BLOCK: u32 = 0x0010_1000;
    const ENTRIES: u32 = 0x0010_2000;
    const TAG: u32 = 0x0010_3000;

    /// A bitmap widget on a block naming a bitmap of `images` images.
    fn guest(images: u32) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(WIDGET, 0x1000)
            .map(BLOCK, 0x1000)
            .map(ENTRIES, 0x1000)
            .map(TAG, 0x1000)
            .map(points::TAG_ENTRIES.base & !0xfff, 0x1000);
        g.put_u32(WIDGET + WIDGET_BLOCK, BLOCK)
            .put_u32(BLOCK + BLOCK_BITMAP, 0xe5b5_0003)
            .put_u32(points::TAG_ENTRIES.base, ENTRIES)
            .put_u32(ENTRIES + 3 * 0x10 + 8, TAG)
            .put_u32(TAG + BITMAP_IMAGES, images);
        g
    }

    /// The image is set as `0x2b0a14` sets it: at `+0x88`, under the
    /// bitmap's image count only.
    #[test]
    fn the_picture_is_an_image_of_the_rows_bitmap() {
        let mut g = guest(8);
        assert!(set_image(&mut g, WIDGET, image::ROSTER).unwrap());
        assert_eq!(g.u16(WIDGET + WIDGET_IMAGE).unwrap(), image::ROSTER);
        assert!(!set_image(&mut g, WIDGET, 8).unwrap(), "past the count");
        assert_eq!(g.u16(WIDGET + WIDGET_IMAGE).unwrap(), image::ROSTER);
        let mut g = guest(8);
        g.put_u32(BLOCK + BLOCK_BITMAP, u32::MAX);
        assert!(!set_image(&mut g, WIDGET, 1).unwrap(), "no bitmap");
        let mut g = guest(8);
        g.put_u32(WIDGET + WIDGET_BLOCK, 0);
        assert!(!set_image(&mut g, WIDGET, 1).unwrap(), "no block");
    }

    /// A row registers once by action; its text comes back.
    #[test]
    fn rows_register_by_action() {
        const A: i16 = 0x7f00;
        register(A, image::COOP, Arc::new(|_: &mut dyn GuestMemory| Some(1)));
        register(
            A,
            image::ROSTER,
            Arc::new(|_: &mut dyn GuestMemory| Some(2)),
        );
        let r = for_action(A).expect("registered");
        assert_eq!(r.image, image::ROSTER, "the second replaced the first");
        let mut g = FakeGuest::new();
        assert_eq!((r.text)(&mut g), Some(2));
        assert!(for_action(A + 1).is_none());
        rows().retain(|r| r.action != A);
    }

    /// Our rows' pictures are of the set's eight, and none is a retail
    /// row's except Halo 1 Options' (Quick Options', its kind).
    #[test]
    fn our_pictures_are_the_sets() {
        for i in [image::ROSTER, image::QUICK_OPTIONS] {
            assert!(i < 8);
        }
        for retail in [image::MATCHMAKING, image::CUSTOM_GAME, image::COOP] {
            assert_ne!(image::ROSTER, retail);
        }
    }

    #[test]
    fn the_hook_is_on_the_plan() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        r.add(&GameSetupHelp).unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
        assert!(
            text.contains("SQUAD_SETTINGS_UPDATE: before [game-setup-help:"),
            "{text}"
        );
    }
}
