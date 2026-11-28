//! X in the pregame lobby (docs/game/party-lobby.md, "Changing mode").
//!
//! A `before` on `PREGAME_LOBBY_INPUT` (the lobby's input handler: ECX the
//! screen, the event on the stack, AL handled, `ret 4`) takes X, button 2
//! of a kind 5 event (`crate::ui_kit::input`), from the party leader: it
//! opens the GAME MODE picker ([`super::picker`]) over the lobby, on the
//! lobby's own window and slot, for the player who pressed it, and answers
//! handled. Every other event, and a member's X, goes on to retail's
//! handler. The handler runs only while the lobby is the input screen, so
//! a dialog over it keeps its own buttons.
//!
//! [`decide`] is the whole choice.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use guest_layout::entry::{EntryFuture, GuestCall, Ret};
use hooks::{Call, Flow};
use runtime::GuestFault;
use settings::Store;

use super::header::{self, Shown};
use crate::list_class::BUTTON_EVENT;
use crate::ui_kit::input::X_BUTTON;

/// What an event in the lobby does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Retail's: not X, or a member's X.
    Pass,
    /// The leader's X: the GAME MODE picker.
    Open,
}

/// The event `(kind, button)` in the lobby `shown`.
pub fn decide(kind: u32, button: u32, shown: &Shown) -> Act {
    if (kind, button) == (BUTTON_EVENT, X_BUTTON) && shown.leader {
        Act::Open
    } else {
        Act::Pass
    }
}

/// The local players an event's controller can be (`+4`).
const LOCAL_PLAYERS: u32 = 4;

/// The guest page the picker's open request is written to ([`install`]).
static REQUEST: AtomicU32 = AtomicU32::new(0);

/// The party lobby feature (`crate::lobby_mode`) gives a page of its own
/// for the open request.
pub fn install(request: u32) {
    REQUEST.store(request, Ordering::SeqCst);
}

static UNREAD: AtomicBool = AtomicBool::new(false);
static NO_PICKER: AtomicBool = AtomicBool::new(false);

/// The hook on `PREGAME_LOBBY_INPUT`: X, with `store` the settings
/// Firefight's choice is held in.
pub fn press(c: &mut Call, store: Option<Arc<Store>>) -> Result<Flow, GuestFault> {
    let rt = Arc::clone(c.ctx.runtime());
    let screen = c.cpu.ecx;
    let (shown, player, stack) = {
        let mem = rt.memory();
        let event = mem.u32(c.cpu.esp + 4).ok();
        let word = |o: u32| event.and_then(|e| mem.u32(e.checked_add(o)?).ok());
        let (Some(kind), Some(button)) = (word(0), word(8)) else {
            return Ok(Flow::Continue);
        };
        if (kind, button) != (BUTTON_EVENT, X_BUTTON) {
            return Ok(Flow::Continue);
        }
        let Some(shown) = header::shown_now(&*mem) else {
            if !UNREAD.swap(true, Ordering::Relaxed) {
                eprintln!("lobby tabs: X with the lobby's session unreadable: left to retail");
            }
            return Ok(Flow::Continue);
        };
        if decide(kind, button, &shown) == Act::Pass {
            return Ok(Flow::Continue);
        }
        let player = word(4).filter(|&p| p < LOCAL_PLAYERS);
        let stack = (|| {
            Some((
                mem.u32(screen.checked_add(crate::ui_kit::lifecycle::SCREEN_WINDOW)?).ok()?,
                mem.u32(screen.checked_add(crate::ui_kit::lifecycle::SCREEN_SLOT)?).ok()?,
            ))
        })();
        (shown, player, stack)
    };
    let present = super::present();
    let pick = super::picker::pick(&shown, present).after(super::picker::after(present, store));
    let loader = crate::ui_kit::picker::offer_list(pick).and_then(crate::list_class::loader_of);
    let request_at = REQUEST.load(Ordering::SeqCst);
    let (Some(player), Some((window, slot)), Some(loader)) = (player, stack, loader) else {
        if !NO_PICKER.swap(true, Ordering::Relaxed) {
            eprintln!(
                "lobby tabs: the GAME MODE picker not opened (player {player:?}, stack {stack:?}, picker {})",
                if loader.is_some() { "on" } else { "off" }
            );
        }
        return Ok(Flow::Continue);
    };
    if request_at == 0 {
        return Ok(Flow::Continue);
    }
    eprintln!("lobby tabs: opening the GAME MODE picker (player {player}) on window {window}, slot {slot}");
    let request = crate::skulls_mp::open_request(1 << player, window, slot, loader);
    c.answer_async(move |cx| -> EntryFuture {
        Box::pin(async move {
            cx.write(|m| m.write(request_at, &request))?;
            cx.call(GuestCall::new(loader).args([request_at])).await?;
            Ok(Ret::new(1, 4))
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lobby_tabs::{Facts, Place, Present, Tab, row};
    use crate::ui_kit::input::Input;

    fn lobby(leader: bool) -> Shown {
        Shown {
            place: Place::SplitScreen,
            leader,
            selected: Tab::CustomGames,
            row: row(Present::ALL, Place::SplitScreen, Facts::default()),
            clanker: None,
        }
    }

    /// X opens the picker for the leader only; a member's X, and every
    /// other button, is retail's.
    #[test]
    fn x_opens_the_picker_for_the_leader_only() {
        let (kind, button) = Input::X.event();
        assert_eq!(decide(kind, button, &lobby(true)), Act::Open);
        assert_eq!(decide(kind, button, &lobby(false)), Act::Pass, "a member's X");
        for input in [
            Input::A,
            Input::B,
            Input::Y,
            Input::Start,
            Input::Back,
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
        ] {
            let (kind, button) = input.event();
            assert_eq!(decide(kind, button, &lobby(true)), Act::Pass, "{input:?}");
        }
        // A direction event's word 8 is not a button.
        assert_eq!(decide(crate::list_class::LEFT_EVENT, X_BUTTON, &lobby(true)), Act::Pass);
    }

    /// The bumpers are retail's again: no mode moves on Black or White.
    #[test]
    fn the_bumpers_are_left_to_retail() {
        const BLACK: u32 = 4;
        const WHITE: u32 = 5;
        for button in [BLACK, WHITE] {
            assert_eq!(decide(BUTTON_EVENT, button, &lobby(true)), Act::Pass, "{button}");
        }
    }
}
