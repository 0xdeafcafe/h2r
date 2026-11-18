//! Our rows on the game's pause menus: Video and Voice, each opening our
//! screen of that name over the pause menu (docs/game/settings-menu.md
//! "Pause menu").
//!
//! **Campaign** (screen 18, `dialog_pause`; loader `0x2320c4`, constructor
//! `0x23203e`, list handler at `screen + 0x614` built by `0x231e88`). Start
//! in a campaign game opens it through `0x149f1e`; its constructor sets the
//! game's paused byte unless it is already set. The list's datum array
//! (its name at `0x459134`) is made with five rows, every one filled, and
//! the rows are told apart by index: the row text (`0x231f6c`) switches on
//! rows 0 to 4 and the row chosen (`0x231fe3`) gives rows 0 to 3 their own
//! press and every other row row 4's. The shared datum array hook makes
//! the array seven rows ([`list_count`]), and `before` hooks on the plan
//! answer rows 5 and 6 (`crate::retail_rows::Extension` over
//! [`List::Pause`]), so the original never sees them. The handler keeps
//! five item widgets; with seven rows the list scrolls.
//!
//! **Multiplayer** (screen 195, `mp_menu`; loader `0x1475c7`, constructor
//! `0x147605`, list handler at `screen + 0x614` built by `0x232708`).
//! Start in a multiplayer game opens it on the player's own stack. Its
//! datum array (`0x4593a8`) has room for six rows; the constructor adds a
//! datum per row it shows, the row's action word (0 to 5) at datum `+2`,
//! some only in some games. The row text (`0x23284e`) looks the action up
//! in a table of six and the row chosen (`0x2328b5`) runs action 5's press
//! for any action past 4. We make the array two rows bigger, add a datum
//! per row of ours after the constructor (actions 6 and 7, [`MP_ACTIONS`]
//! plus the row's place in [`List::MpPause`]), and answer them in `before`
//! hooks ([`MpPauseRows`]).
//!
//! **Which stack.** A row of ours opens its screen with a request built
//! from the pause screen's own `+0x74` and `+0x78`, as the retail
//! Controller Settings row of both menus does (`0x2321ab`, `0x232aff`,
//! through the screen's slots 20 and 21, `0x23012c` and `0x230130`). For
//! the campaign menu that is what `0x149f1e` builds (window 5, slot 4), so
//! its rows go through the usual open; the multiplayer menu is on the
//! player's slot (`0x1910b8`), so its rows build the request themselves.
//!
//! **Pause.** Our screens never touch the paused byte. The game clears it
//! (`0x1482c4`, every UI update) only while no screen is up on any stack
//! (`0x149227`), so a screen of ours over the pause menu keeps the game
//! paused, and B back to the pause menu changes nothing. A multiplayer
//! game is never paused: `mp_menu` sets nothing, and neither do we; the
//! simulation runs while our screens are up. A row press does write
//! `settings.toml` on the game thread ([`settings::Store::set`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use engine::DatumHandle;
use engine::datum::DatumArray;
use engine::entry::{EntryFuture, GuestCall, Ret};
use engine::memory::{GuestMemory, MemFault};
use feature::{BuildError, Builder, Call, Flow, Manifest};
use runtime::GuestFault;
use runtime::guest::Permissions;

use crate::list_class;
use crate::ui::{Chosen, List, Model};

/// The campaign pause menu.
pub const PAUSE_SCREEN_ID: i16 = 18;
pub const PAUSE_LOADER: u32 = 0x0023_20c4;
/// Its rows' datum array name and the rows the game makes.
pub const PAUSE_LIST_NAME: u32 = 0x0045_9134;
pub const PAUSE_ROWS: u32 = 5;
/// Its Controller Settings row (row 3: closes the menu and opens screen
/// 200). Our rows go below it, and row 4 moves down past them.
pub const PAUSE_CONTROLLER_ROW: i16 = 3;

/// The multiplayer pause menu.
pub const MP_SCREEN_ID: i16 = 195;
pub const MP_LOADER: u32 = 0x0014_75c7;
/// Its rows' datum array name, the room the game makes, and the actions
/// it knows (0 to 5): ours follow.
pub const MP_LIST_NAME: u32 = 0x0045_93a8;
pub const MP_ACTIONS: u32 = 6;
/// Its Controller Settings action (opens screen 200, or player settings),
/// shown in every game: our rows go below it.
pub const MP_CONTROLLER_ACTION: i16 = 1;
/// The rows' datum array at handler `+0x70`; a row's action word at datum
/// `+2`; an item widget's row handle at `+0x70`.
pub const LIST_ROWS: u32 = 0x70;
pub const ROW_ACTION: u32 = 2;
pub const ITEM_ROW: u32 = 0x70;

/// The feature id of the multiplayer rows (`features.pause-mp-rows`); the
/// campaign rows are `features.pause-rows`.
pub const MP_FEATURE: &str = "pause-mp-rows";
pub const FEATURE: &str = "pause-rows";

/// Whether our pause rows are on: `H2_NO_PAUSE_ROWS` takes them out.
pub fn rows_on() -> bool {
    std::env::var_os("H2_NO_PAUSE_ROWS").is_none()
}

/// The count the datum array named `name` is made with, `count` being what
/// the game asks for: a pause menu's grows by our rows.
pub fn list_count(m: &Model, name: u32, count: u32) -> u32 {
    match name {
        PAUSE_LIST_NAME => m.count(List::Pause, count),
        MP_LIST_NAME => m.count(List::MpPause, count),
        _ => count,
    }
}

/// The actions our multiplayer rows take, in order.
pub fn mp_actions(m: &Model) -> Vec<i16> {
    let n = m.rows(List::MpPause).len() as i16;
    (0..n).map(|i| MP_ACTIONS as i16 + i).collect()
}

/// Whether `action` is one of ours on the multiplayer menu.
pub fn is_ours_mp(m: &Model, action: i16) -> bool {
    action >= MP_ACTIONS as i16 && m.row(List::MpPause, action).is_some()
}

/// The actions of the list at `list`, in slot order.
pub fn actions(m: &dyn GuestMemory, list: u32) -> Result<Vec<i16>, MemFault> {
    let array = DatumArray::at(m.u32(list.wrapping_add(LIST_ROWS))?);
    let hw = array.header_field(m, engine::datum::layout::HIGH_WATER)?;
    let mut out = Vec::new();
    for i in 0..hw.max(0) {
        if let Some(elem) = array.get_absolute(m, i)? {
            out.push(m.u16(elem.wrapping_add(ROW_ACTION))? as i16);
        }
    }
    Ok(out)
}

/// Adds a row per action of `ours` the list at `list` lacks; the actions
/// added.
pub fn add_actions(m: &mut dyn GuestMemory, list: u32, ours: &[i16]) -> Result<Vec<i16>, String> {
    let have = actions(m, list).map_err(|e| format!("{e:?}"))?;
    let array = DatumArray::at(
        m.u32(list.wrapping_add(LIST_ROWS))
            .map_err(|e| format!("{e:?}"))?,
    );
    let mut added = Vec::new();
    for &a in ours.iter().filter(|a| !have.contains(a)) {
        let h = array
            .new_datum(m)
            .map_err(|e| format!("{e:?}"))?
            .ok_or("the list is full")?;
        let elem = array
            .try_get(m, h)
            .map_err(|e| format!("{e:?}"))?
            .ok_or("the new row is not there")?;
        m.set_u16(elem.wrapping_add(ROW_ACTION), a as u16)
            .map_err(|e| format!("{e:?}"))?;
        added.push(a);
    }
    Ok(added)
}

/// The actions in `have` (the list's, in row order) with `ours` moved to
/// just below action `after`; `have` unchanged without it.
pub fn ordered(have: &[i16], ours: &[i16], after: i16) -> Vec<i16> {
    let Some(at) = have.iter().position(|a| *a == after) else {
        return have.to_vec();
    };
    let mut out: Vec<i16> = have.iter().copied().filter(|a| !ours.contains(a)).collect();
    let at = out
        .iter()
        .position(|a| *a == have[at])
        .map_or(out.len(), |i| i + 1);
    let mine: Vec<i16> = ours.iter().copied().filter(|a| have.contains(a)).collect();
    out.splice(at..at, mine);
    out
}

/// Puts our rows below action `after` on the list at `list`: the rows are
/// told apart by their action word only, so the words are written again
/// in the new order, row by row. Returns the order.
pub fn order_actions(
    m: &mut dyn GuestMemory,
    list: u32,
    ours: &[i16],
    after: i16,
) -> Result<Vec<i16>, MemFault> {
    let have = actions(m, list)?;
    let want = ordered(&have, ours, after);
    if want == have {
        return Ok(want);
    }
    let array = DatumArray::at(m.u32(list.wrapping_add(LIST_ROWS))?);
    let hw = array.header_field(m, engine::datum::layout::HIGH_WATER)?;
    let mut next = want.iter();
    for i in 0..hw.max(0) {
        if let Some(elem) = array.get_absolute(m, i)?
            && let Some(a) = next.next()
        {
            m.set_u16(elem.wrapping_add(ROW_ACTION), *a as u16)?;
        }
    }
    Ok(want)
}

/// The action of the row whose handle is `row` on the list at `list`.
pub fn row_action(m: &dyn GuestMemory, list: u32, row: u32) -> Option<i16> {
    let array = DatumArray::at(m.u32(list.wrapping_add(LIST_ROWS)).ok()?);
    let elem = array.try_get(m, DatumHandle::from_raw(row)?).ok()??;
    m.u16(elem.wrapping_add(ROW_ACTION)).ok().map(|v| v as i16)
}

/// The retail rule for the paused byte, as `0x1482c4` applies it each UI
/// update: a paused game resumes only when no screen is up on any stack.
/// (`0x23203e` sets it when the campaign pause menu is made; nothing of
/// ours writes it.)
pub fn resumes(paused: bool, any_screen_up: bool) -> bool {
    paused && !any_screen_up
}

/// The multiplayer menu's rows of ours.
pub struct MpPauseRows {
    pub model: Arc<Model>,
}

fn once(seen: &AtomicBool, what: &str) {
    if !seen.swap(true, Ordering::Relaxed) {
        eprintln!("{what}");
    }
}

fn mp_row_text(c: &mut Call, model: &Model, seen: &AtomicBool) -> Result<Flow, GuestFault> {
    let (item, action) = {
        let mem = c.ctx.runtime().memory();
        let item = mem.u32(c.cpu.esp + 4).ok();
        let action = item
            .and_then(|i| mem.u32(i + ITEM_ROW).ok())
            .and_then(|row| row_action(&*mem, c.cpu.ecx, row));
        (item, action)
    };
    // The model reads the store: never with guest memory held.
    let (Some(item), Some(texts)) = (
        item,
        action
            .filter(|a| is_ours_mp(model, *a))
            .and_then(|a| crate::retail_rows::texts(model, List::MpPause, a, true)),
    ) else {
        return Ok(Flow::Continue);
    };
    once(seen, "pause menu: multiplayer row text");
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            list_class::draw_texts(&cx, item, texts).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

fn mp_row_chosen(c: &mut Call, model: &Model, request_at: u32) -> Result<Flow, GuestFault> {
    let (action, player, stack) = {
        let mem = c.ctx.runtime().memory();
        let row = mem.u32(c.cpu.esp + 8).ok().and_then(|p| mem.u32(p).ok());
        let action = row.and_then(|row| row_action(&*mem, c.cpu.ecx, row));
        let player = crate::skulls_menu::player_index(&*mem, c.cpu.esp);
        let stack = crate::h1_options_row::screen_of(&*mem, c.cpu.ecx).and_then(|s| {
            Some((
                mem.u32(s + crate::ui_kit::lifecycle::SCREEN_WINDOW).ok()?,
                mem.u32(s + crate::ui_kit::lifecycle::SCREEN_SLOT).ok()?,
            ))
        });
        (action, player, stack)
    };
    let Some(action) = action.filter(|a| is_ours_mp(model, *a)) else {
        return Ok(Flow::Continue);
    };
    // The store is read with no guest memory held.
    let Some(Chosen::Open(screen)) = model.row(List::MpPause, action).map(|r| model.press(r))
    else {
        return c.ret(8).map(Flow::Return);
    };
    let loader = list_class::loader_of(screen);
    let (Some(player), Some((window, slot)), Some(loader)) = (player, stack, loader) else {
        eprintln!(
            "pause menu: {screen} not opened (player {player:?}, stack {stack:?}, class {})",
            if loader.is_some() { "on" } else { "off" }
        );
        return c.ret(8).map(Flow::Return);
    };
    eprintln!("pause menu: opening {screen} (player {player}) on window {window}, slot {slot}");
    let request = crate::skulls_mp::open_request(1 << player, window, slot, loader);
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.write(|m| m.write(request_at, &request))?;
            cx.call(GuestCall::new(loader).args([request_at])).await?;
            Ok(Ret::new(0, 8))
        })
    })
}

impl feature::Feature for MpPauseRows {
    fn manifest(&self) -> Manifest {
        Manifest::new(MP_FEATURE, "Our rows on the multiplayer pause menu")
            .doc("docs/game/settings-menu.md")
    }
    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        use engine::points::{MP_MENU_LIST_CTOR as CTOR, MP_MENU_ROW_CHOSEN, MP_MENU_ROW_TEXT};
        let request_at = b.guest_alloc(0x1000, Permissions::RW)?;
        // The handler being built (ESI at entry); a stack in case of nesting.
        let building: Arc<std::sync::Mutex<Vec<u32>>> = Arc::default();
        let b2 = Arc::clone(&building);
        b.raw().before(&CTOR, "note the pause list", move |c| {
            b2.lock().unwrap_or_else(|e| e.into_inner()).push(c.cpu.esi);
            Ok(Flow::Continue)
        });
        let ours = mp_actions(&self.model);
        b.raw().after(&CTOR, "add our pause rows", move |c| {
            let Some(list) = building.lock().unwrap_or_else(|e| e.into_inner()).pop() else {
                return Ok(());
            };
            let mut m = c.ctx.runtime().memory();
            match add_actions(&mut *m, list, &ours) {
                Ok(added) if !added.is_empty() => {
                    eprintln!("pause menu: multiplayer list gets rows with actions {added:?}")
                }
                Ok(_) => {}
                Err(why) => eprintln!("pause menu: multiplayer rows not added: {why}"),
            }
            if let Err(e) = order_actions(&mut *m, list, &ours, MP_CONTROLLER_ACTION) {
                eprintln!("pause menu: multiplayer rows left last: {e:?}");
            }
            Ok(())
        });
        let (m, seen) = (Arc::clone(&self.model), Arc::new(AtomicBool::new(false)));
        b.raw()
            .before(&MP_MENU_ROW_TEXT, "our pause rows' text", move |c| {
                mp_row_text(c, &m, &seen)
            });
        let m = Arc::clone(&self.model);
        b.raw()
            .before(&MP_MENU_ROW_CHOSEN, "our pause rows' press", move |c| {
                mp_row_chosen(c, &m, request_at)
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
