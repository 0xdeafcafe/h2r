//! The strip's bumpers (docs/game/party-lobby.md, "Switching").
//!
//! A `before` on `PREGAME_LOBBY_INPUT` (the lobby's input handler: ECX the
//! screen, the event on the stack, AL handled, `ret 4`) takes Black and
//! White, buttons 4 and 5 of a kind 5 event (`crate::ui_kit::input`), and
//! answers handled; every other event goes on to retail's handler. The
//! handler runs only while the lobby is the input screen, so a dialog over
//! it keeps its own buttons, and in a game the bumpers are the game's.
//!
//! [`decide`] is the whole choice: a member's press does nothing; a press
//! toward the end of the strip does nothing; a press toward only tabs that
//! are off shows the nearest one's reason; any other switches to the next
//! tab that is on ([`super::step`]), showing the reason of the first one
//! it skipped. The switch itself is [`super::switch::run`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use engine::entry::{EntryFuture, Ret};
use hooks::{Call, Flow};
use runtime::GuestFault;
use settings::Store;

use super::strip::{self, Shown};
use super::{Dir, Reason, Step, Tab, notice, step};
use crate::list_class::BUTTON_EVENT;

/// Black and White's button words (`+8` of a kind 5 event).
pub const BLACK_BUTTON: u32 = 4;
pub const WHITE_BUTTON: u32 = 5;

/// The bumper an event is, if it is one.
pub fn bumper(kind: u32, button: u32) -> Option<Dir> {
    match (kind, button) {
        (BUTTON_EVENT, WHITE_BUTTON) => Some(Dir::Prev),
        (BUTTON_EVENT, BLACK_BUTTON) => Some(Dir::Next),
        _ => None,
    }
}

/// What a bumper press does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Nothing: a member's press, or the end of the strip.
    Nothing,
    /// Only a reason on the status line.
    Notice(Reason),
    /// A switch from `from` to `to`, with the first skipped tab's reason.
    Switch {
        from: Tab,
        to: Tab,
        notice: Option<Reason>,
    },
}

/// The press of `dir` in the lobby `shown`.
pub fn decide(dir: Dir, shown: &Shown) -> Act {
    if !shown.leader {
        return Act::Nothing;
    }
    match step(&shown.row, shown.selected, dir) {
        Step::Stay(None) => Act::Nothing,
        Step::Stay(Some(r)) => Act::Notice(r),
        Step::Move { to, skipped } => Act::Switch {
            from: shown.selected,
            to,
            notice: skipped,
        },
    }
}

static UNREAD: AtomicBool = AtomicBool::new(false);

fn handled(c: &mut Call) -> Result<Flow, GuestFault> {
    c.cpu.eax = (c.cpu.eax & !0xff) | 1;
    c.ret(4).map(Flow::Return)
}

/// The hook on `PREGAME_LOBBY_INPUT`: the bumpers, `store` the settings
/// Firefight's choice is held in.
pub fn press(c: &mut Call, store: Option<Arc<Store>>) -> Result<Flow, GuestFault> {
    let rt = Arc::clone(c.ctx.runtime());
    let (dir, shown) = {
        let mem = rt.memory();
        let event = mem.u32(c.cpu.esp + 4).ok();
        let word = |o: u32| event.and_then(|e| mem.u32(e.checked_add(o)?).ok());
        let Some(dir) = word(0).zip(word(8)).and_then(|(k, b)| bumper(k, b)) else {
            return Ok(Flow::Continue);
        };
        let shown = strip::read(&*mem)
            .map(|r| strip::shown(&r, super::present(), crate::lobby_start::mode()));
        (dir, shown)
    };
    let Some(shown) = shown else {
        if !UNREAD.swap(true, Ordering::Relaxed) {
            eprintln!("lobby tabs: a bumper with the lobby's session unreadable: left to retail");
        }
        return Ok(Flow::Continue);
    };
    let place = shown.place;
    match decide(dir, &shown) {
        Act::Nothing => handled(c),
        Act::Notice(r) => {
            notice(r);
            handled(c)
        }
        Act::Switch {
            from,
            to,
            notice: skipped,
        } => {
            if let Some(r) = skipped {
                notice(r);
            }
            c.answer_async(move |cx| -> EntryFuture {
                Box::pin(async move {
                    super::switch::run(&cx, from, to, place, store).await?;
                    Ok(Ret::new(1, 4))
                })
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lobby_tabs::{Facts, NOTICE_FOR, Notice, Place, Present, row};
    use crate::ui_kit::input::Input;
    use std::time::{Duration, Instant};

    fn lobby(leader: bool, selected: Tab) -> Shown {
        Shown {
            place: Place::SplitScreen,
            leader,
            selected,
            row: row(Present::ALL, Place::SplitScreen, Facts::default()),
            clanker: None,
        }
    }

    #[test]
    fn black_and_white_are_handled_and_nothing_else_is() {
        assert_eq!(bumper(BUTTON_EVENT, WHITE_BUTTON), Some(Dir::Prev));
        assert_eq!(bumper(BUTTON_EVENT, BLACK_BUTTON), Some(Dir::Next));
        for input in [
            Input::A,
            Input::B,
            Input::X,
            Input::Y,
            Input::Start,
            Input::Back,
            Input::Up,
            Input::Down,
            Input::Left,
            Input::Right,
        ] {
            let (kind, button) = input.event();
            assert_eq!(bumper(kind, button), None, "{input:?}");
        }
        // A direction event's word 8 is not a button.
        assert_eq!(bumper(crate::list_class::LEFT_EVENT, WHITE_BUTTON), None);
        // The kit's screens never see them as an input of theirs.
        assert_eq!(Input::from_event(BUTTON_EVENT, BLACK_BUTTON), None);
        assert_eq!(Input::from_event(BUTTON_EVENT, WHITE_BUTTON), None);
    }

    #[test]
    fn a_member_cannot_switch() {
        for dir in [Dir::Prev, Dir::Next] {
            assert_eq!(decide(dir, &lobby(false, Tab::CustomGames)), Act::Nothing);
        }
        assert_eq!(
            decide(Dir::Next, &lobby(true, Tab::CustomGames)),
            Act::Switch {
                from: Tab::CustomGames,
                to: Tab::Forge,
                notice: None
            }
        );
    }

    #[test]
    fn a_press_toward_a_tab_that_is_off_shows_its_reason_for_four_seconds() {
        let act = decide(Dir::Prev, &lobby(true, Tab::CustomGames));
        assert_eq!(act, Act::Notice(Reason::MatchmakingNeedsLive));
        // Skipped on the way to a tab that is on: the move, and the reason.
        assert_eq!(
            decide(Dir::Next, &lobby(true, Tab::Forge)),
            Act::Switch {
                from: Tab::Forge,
                to: Tab::CoOp,
                notice: Some(Reason::NoKitMap)
            }
        );
        let at = Instant::now();
        let n = Notice {
            reason: Reason::MatchmakingNeedsLive,
            at,
        };
        assert_eq!(
            n.shown(at + Duration::from_secs(3)),
            Some(Reason::MatchmakingNeedsLive)
        );
        assert_eq!(n.shown(at + NOTICE_FOR), None, "gone after four seconds");
        assert_eq!(NOTICE_FOR, Duration::from_secs(4));
    }

    #[test]
    fn the_ends_of_the_strip_do_nothing() {
        let mut s = lobby(true, Tab::CoOp);
        s.place = Place::XboxLive;
        s.row = row(
            Present::ALL,
            Place::XboxLive,
            Facts {
                playlists: true,
                kit_maps: true,
                remote_members: false,
            },
        );
        assert_eq!(decide(Dir::Next, &s), Act::Nothing);
        s.selected = Tab::Multiplayer;
        assert_eq!(decide(Dir::Prev, &s), Act::Nothing);
    }

    /// Every tab the availability table has on is reached by the bumpers,
    /// in each place: Xbox Live from CUSTOM GAMES to MULTIPLAYER and CO-OP,
    /// Split Screen to CO-OP. None is silently dead: a tab passed by is
    /// named on the status line.
    #[test]
    fn every_tab_that_is_on_is_reached_by_the_bumpers() {
        let facts = Facts {
            playlists: true,
            kit_maps: true,
            remote_members: false,
        };
        for place in [Place::SplitScreen, Place::SystemLink, Place::XboxLive] {
            let r = row(Present::ALL, place, facts);
            let on: Vec<Tab> = r
                .iter()
                .filter(|(_, a)| a.is_on())
                .map(|(t, _)| *t)
                .collect();
            let mut s = Shown {
                place,
                leader: true,
                selected: Tab::CustomGames,
                row: r.clone(),
                clanker: None,
            };
            let mut seen = vec![Tab::CustomGames];
            for dir in [Dir::Prev, Dir::Next] {
                s.selected = Tab::CustomGames;
                while let Act::Switch { to, .. } = decide(dir, &s) {
                    seen.push(to);
                    s.selected = to;
                }
            }
            seen.sort_by_key(|t| *t as u8);
            assert_eq!(seen, on, "{place:?}");
        }
        // Split Screen with no kit map: FORGE to CO-OP passes FIREFIGHT and
        // names it.
        assert_eq!(
            decide(Dir::Next, &lobby(true, Tab::Forge)),
            Act::Switch {
                from: Tab::Forge,
                to: Tab::CoOp,
                notice: Some(Reason::NoKitMap)
            }
        );
    }
}
