//! The multiplayer Skulls screen and its row in the game variant editor
//! (docs/game/skulls.md "In multiplayer").
//!
//! **Where.** The variant editor's category list (`variant_editing_options`,
//! screen 178: Match, Player, Team, Game Type, Vehicle and Equipment
//! Options) gets a seventh row, "Skulls". The list is grown by the shared
//! datum array hook ([`crate::skulls_mp::variant_count`]); retail's row
//! text leaves word 6 blank and its row chosen ignores it, so our `before`
//! hooks draw it and, on a press, open the multiplayer Skulls screen. The
//! row has no pane (the tag has six), so the pane hook keeps the last
//! category's ([`crate::skulls_mp::skip_pane`]).
//!
//! **Which screen.** A screen of its own: a second [`SkullsScreen`]
//! ([`SkullsScreen::multiplayer`]) on its own list class (`skulls-mp-screen`,
//! screen id [`MP_SCREEN`]), so nothing about it is the campaign screen's.
//!
//! **Which stack.** It opens on the pressing screen's own screen stack: the
//! request's window and slot are the variant editor's (`+0x74`, `+0x78`).
//! The Game Setup row it replaces opened through `0x149f1e`, which always
//! says window 5 (the main menu's stack), from a dialog on window 3; the
//! screen went onto the main menu's stack under the lobby and B closed it
//! there (docs/game/skulls.md "Why the Game Setup row broke").

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use guest_layout::entry::{EntryFuture, GuestCall, Ret};
use hooks::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;

use crate::layout_glue::chosen_row;
use crate::list_class::{self, ListClass, ListLayout, PictureSpec, TextBlock};
use crate::skulls_menu::{self, SkullsScreen, TITLE_ID};
use crate::skulls_mp::{self, SCREEN_SLOT, SCREEN_WINDOW, VARIANT_ROW};

/// The multiplayer screen's id for `Open` rows and the loader registry.
pub const MP_SCREEN: &str = "skulls-mp";
/// Its list class (`features.skulls-mp-screen` switches it).
pub const MP_FEATURE: &str = "skulls-mp-screen";
/// The row (`features.skulls-mp-row` switches it).
pub const ROW_FEATURE: &str = "skulls-mp-row";

/// The multiplayer screen's class, with the campaign page's layout,
/// description block and picture.
pub fn mp_class(
    screen: Arc<SkullsScreen>,
    block: TextBlock,
    layout: ListLayout,
    picture: PictureSpec,
) -> ListClass {
    skulls_menu::level_select_list(ListClass::with_rows(MP_FEATURE, MP_SCREEN, screen), layout)
        .description(block)
        .picture(picture)
        // The campaign page's legends: `SkullsScreen::focused_footer` sets
        // the focused row's as the focus moves.
        .footer(skulls_menu::SETTING_FOOTER_ID)
}

/// Whether the row's hooks are on the plan: the list is grown only then.
static ROW_ON: AtomicBool = AtomicBool::new(false);

/// Whether the variant editor's list should get the Skulls row.
pub fn row_on() -> bool {
    ROW_ON.load(Ordering::SeqCst)
}

/// What a press on our row opens: for the player and profile, on the
/// pressing screen's stack.
#[derive(Debug, PartialEq, Eq)]
pub struct Opening {
    pub profile: String,
    pub player: u32,
    pub request: [u8; 0x20],
    pub loader: u32,
}

/// A press on the variant editor's Skulls row by local player `player`
/// (with `profile` on its slot) on a screen whose window and slot are
/// `stack`: what to open, or the line that says why not.
pub fn opening(
    player: Option<u32>,
    profile: Option<String>,
    stack: Option<(u32, u32)>,
    loader: Option<u32>,
) -> Result<Opening, String> {
    let (Some(player), Some(profile)) = (player, profile) else {
        return Err("skulls menu: the pressing player's index is unreadable; not opened".into());
    };
    let Some(loader) = loader else {
        return Err("skulls menu: the multiplayer screen's class is off; not opened".into());
    };
    let Some((window, slot)) = stack else {
        return Err(
            "skulls menu: the variant editor's screen stack is unreadable; not opened".into(),
        );
    };
    Ok(Opening {
        profile,
        player,
        request: skulls_mp::open_request(1 << player, window, slot, loader),
        loader,
    })
}

/// Whose skulls the multiplayer screen shows, as a machine snapshot keeps
/// it (docs/runtime/snapshots.md): the profile the last press opened it
/// for. Its options are always none.
pub fn save_state(screen: &SkullsScreen) -> Vec<u8> {
    let w = &mut snapshot::codec::Writer::new();
    match screen.profile() {
        Some(p) => w.bool(true).str(&p),
        None => w.bool(false),
    };
    std::mem::take(w).into_bytes()
}

/// Puts [`save_state`]'s bytes back; nothing changes unless all of them
/// decode. A screen saved never opened is left as it is: a press opens it
/// again before it shows.
pub fn load_state(screen: &SkullsScreen, bytes: &[u8]) -> Result<(), String> {
    let e = |e: snapshot::Error| e.to_string();
    let r = &mut snapshot::codec::Reader::new(bytes, "skulls-mp-row");
    let profile = if r.bool().map_err(e)? {
        Some(r.str().map_err(e)?.to_owned())
    } else {
        None
    };
    r.finish().map_err(e)?;
    if let Some(profile) = profile {
        screen.open(profile);
    }
    Ok(())
}

/// The Skulls row on the variant editor's category list.
pub struct VariantRow(pub Arc<SkullsScreen>);

fn once(seen: &AtomicBool, what: &str) {
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("{what}");
    }
}

fn row_text(c: &mut Call, seen: &AtomicBool) -> Result<Flow, GuestFault> {
    let (item, row) = {
        let mem = c.ctx.runtime().memory();
        let item = mem.u32(c.cpu.esp + 4).ok();
        (item, crate::video_screen::text_row(&*mem, c.cpu.esp))
    };
    let (Some(item), Some(VARIANT_ROW)) = (item, row) else {
        return Ok(Flow::Continue);
    };
    once(seen, "skulls menu: variant editor row text");
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            list_class::draw_texts(&cx, item, vec![(0, TITLE_ID)]).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn row_chosen(c: &mut Call, screen: &SkullsScreen, request_at: u32) -> Result<Flow, GuestFault> {
    let (row, player, profile, stack, host) = {
        let mem = c.ctx.runtime().memory();
        let player = skulls_menu::player_index(&*mem, c.cpu.esp);
        // The list handler is at screen + 0x614 (the constructor's ESI).
        let at = c.cpu.ecx.wrapping_sub(guest_layout::view::ui::LIST);
        let stack = mem
            .u32(at + SCREEN_WINDOW)
            .ok()
            .zip(mem.u32(at + SCREEN_SLOT).ok());
        (
            chosen_row(&*mem, c.cpu.esp),
            player,
            player.and_then(|p| skulls_menu::profile_of(&*mem, p)),
            stack,
            crate::skulls::signed_in(&*mem),
        )
    };
    if row != Some(VARIANT_ROW) {
        return Ok(Flow::Continue);
    }
    let o = match opening(player, profile, stack, list_class::loader_of(MP_SCREEN)) {
        Ok(o) => o,
        Err(why) => {
            eprintln!("{why}");
            return c.ret(8).map(Flow::Return);
        }
    };
    if let Some(mp) = skulls_mp::shared() {
        // The host's profiles as of now, for what a lobby advertises.
        mp.set_profiles(if host.is_empty() {
            vec![crate::skulls::DEFAULT_PROFILE.to_owned()]
        } else {
            host
        });
    }
    let window = u32::from_le_bytes(o.request[4..8].try_into().unwrap_or_default());
    eprintln!(
        "skulls menu: opening the multiplayer Skulls screen for {} (player {}) on window {window}",
        o.profile, o.player
    );
    screen.open(o.profile);
    let (request, loader) = (o.request, o.loader);
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.write(|m| m.write(request_at, &request))?;
            cx.call(GuestCall::new(loader).args([request_at])).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

impl hooks::Feature for VariantRow {
    fn manifest(&self) -> Manifest {
        Manifest::new(ROW_FEATURE, "The Skulls row in the game variant editor")
            .doc("docs/game/skulls.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use guest_layout::points::{VARIANT_OPTIONS_ROW_CHOSEN, VARIANT_OPTIONS_ROW_TEXT};
        // The request a press hands the loader (it is read during the call
        // only, as retail's are on the caller's stack).
        let request_at = b.guest_alloc(0x1000, Permissions::RW)?;
        let seen = Arc::new(AtomicBool::new(false));
        b.raw().before(
            &VARIANT_OPTIONS_ROW_TEXT,
            "the Skulls row's text",
            move |c| row_text(c, &seen),
        );
        let screen = Arc::clone(&self.0);
        b.raw().before(
            &VARIANT_OPTIONS_ROW_CHOSEN,
            "the Skulls row's press",
            move |c| row_chosen(c, &screen, request_at),
        );
        ROW_ON.store(true, Ordering::SeqCst);
        // `ROW_ON` is fixed by the build; the screen's profile is the press's.
        let (save, load) = (Arc::clone(&self.0), Arc::clone(&self.0));
        b.snapshot(hooks::SnapshotPolicy::section(
            move || save_state(&save),
            move |bytes, _| load_state(&load, bytes),
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_class::Rows;
    use settings::Store;

    fn screen() -> Arc<SkullsScreen> {
        Arc::new(SkullsScreen::multiplayer(Arc::new(Store::defaults(
            crate::setting_defs::ALL,
        ))))
    }

    /// The press opens on the variant editor's own stack (window 5, slot 4
    /// in the main menu; a lobby's window 3 stays 3), for the pressing
    /// player, with the multiplayer screen's loader.
    #[test]
    fn a_press_opens_the_multiplayer_screen_on_the_pressing_screens_stack() {
        let o = opening(
            Some(1),
            Some("Chief".into()),
            Some((5, 4)),
            Some(0xa0f0_5000),
        )
        .unwrap();
        assert_eq!(o.profile, "Chief");
        assert_eq!(o.request, skulls_mp::open_request(2, 5, 4, 0xa0f0_5000));
        let o = opening(Some(0), Some("Chief".into()), Some((3, 4)), Some(1)).unwrap();
        assert_eq!(u32::from_le_bytes(o.request[4..8].try_into().unwrap()), 3);
        assert!(opening(None, Some("a".into()), Some((5, 4)), Some(1)).is_err());
        assert!(opening(Some(0), None, Some((5, 4)), Some(1)).is_err());
        assert!(
            opening(Some(0), Some("a".into()), Some((5, 4)), None)
                .unwrap_err()
                .contains("class is off")
        );
        assert!(opening(Some(0), Some("a".into()), None, Some(1)).is_err());
    }

    /// Its own screen: the multiplayer rows whoever opens it.
    #[test]
    fn the_multiplayer_screen_is_its_own_instance() {
        let s = screen();
        assert_eq!(s.count(), skulls_mp::MP.len());
        s.open("Chief".into());
        assert_eq!(s.count(), skulls_mp::MP.len());
        assert_eq!(s.title(), skulls_menu::PAGE_TITLE_ID);
    }

    #[test]
    fn the_row_and_the_class_are_on_the_plan() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        let s = screen();
        r.add(&mp_class(
            Arc::clone(&s),
            skulls_menu::DESCRIPTION_TEXT,
            skulls_menu::LIST_LAYOUT,
            skulls_menu::PICTURE,
        ))
        .unwrap();
        r.add(&VariantRow(s)).unwrap();
        let text = r.build(&hooks::Base).unwrap().text();
        for line in [
            "0x002b7581 VARIANT_OPTIONS_ROW_TEXT: before [skulls-mp-row:the Skulls row's text] -> original\n",
            "0x002b75e8 VARIANT_OPTIONS_ROW_CHOSEN: before [skulls-mp-row:the Skulls row's press] -> original\n",
            "entry skulls-mp-screen:loader",
        ] {
            assert!(text.contains(line), "{line}\n{text}");
        }
        assert!(row_on());
        assert!(list_class::loader_of(MP_SCREEN).is_some());
    }

    #[test]
    fn a_snapshot_carries_whose_skulls_the_screen_shows() {
        let (a, b) = (screen(), screen());
        let never = save_state(&a);
        a.open("Chief".into());
        let bytes = save_state(&a);
        load_state(&b, &bytes).unwrap();
        assert_eq!(b.profile().as_deref(), Some("Chief"));
        assert_eq!(save_state(&b), bytes, "the same state again");
        load_state(&b, &never).unwrap();
        assert_eq!(b.profile().as_deref(), Some("Chief"), "left as it was");
        assert!(load_state(&b, &bytes[..bytes.len() - 1]).is_err());
        let mut odd = bytes.clone();
        odd.push(0);
        assert!(load_state(&b, &odd).is_err(), "trailing bytes");
    }
}
