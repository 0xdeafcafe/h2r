//! The pre-round picker on the host (docs/game/rules-world-api.md,
//! "Pickers"): a script's `offer_pick` and `pick_row` on this machine's
//! local players, and their picks back to the VM.
//!
//! The VM keeps the offers (`rules::vm::RulesExt::picks`) and each player's
//! pick (`Vm::choose`); this module is the host's part. [`plan`] reads the
//! offers after the tick's queue is drained and, once for each new set of
//! offers, asks each local player in turn (the next opens when the last
//! one picked, or after [`OPEN_TICKS`]): a [`Call::OfferPick`] the boundary
//! makes with [`open`], which opens the game's value picker
//! (`crate::ui_kit::picker`, `kit.picker.game`, a select dialog over the
//! game: no screen of its own) over the player's controller, with the
//! offer's title and rows. A press on a row is [`report`]ed from the
//! picker's setter; [`take`] hands the reports to the VM at the next
//! boundary, which `picked` then reads.
//!
//! **Not built.** A player on another machine is not asked: it has no
//! local user here, and carrying a pick from the machine it sat at needs a
//! message on the actor channel. Such a player reads -1 from `picked`, so
//! its script falls back to what the lobby chose. A player who joins after
//! the offers were made is not asked either.
//!
//! [`Call::OfferPick`]: super::super::runtime::Call::OfferPick

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use ::rules::vm::Vm;
use engine::entry::{Cx, EntryError, GuestCall};
use engine::{GuestMemory, points};

use crate::list_class;
use crate::ui_kit::dialog_tag;
use crate::ui_kit::picker::{self, Pick};

/// How long an opened picker holds the next player's back, in ticks (30
/// seconds): B on it reports nothing, so the queue would wait for ever.
pub const OPEN_TICKS: u32 = 900;

/// The picker's description, on the right.
const HELP: &str = "Your pick holds from your next spawn.";

/// One player to ask: the roster slot, the controller port it plays on,
/// and the offer's picker number, title and rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub player: u8,
    pub port: u32,
    pub slot: u8,
    pub title: String,
    pub rows: Vec<String>,
}

/// A pick made: row `row` of picker `slot` by `player`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub player: u8,
    pub slot: u8,
    pub row: u8,
}

#[derive(Default)]
struct State {
    /// The offers last asked for, as their strings.
    key: Option<Vec<(u8, String, Vec<String>)>>,
    queue: VecDeque<Ask>,
    /// The tick the open picker was asked at.
    open_since: Option<u32>,
    reports: Vec<Report>,
}

#[cfg(not(test))]
static STATE: Mutex<Option<State>> = Mutex::new(None);

#[cfg(not(test))]
fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    f(STATE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(State::default))
}

// Tests run side by side, each on its own thread: each has its own state.
#[cfg(test)]
thread_local! {
    static STATE: Mutex<Option<State>> = const { Mutex::new(None) };
}

#[cfg(test)]
fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|s| f(s.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(State::default)))
}

/// The VM's offers as strings: picker number, title, rows.
fn offers(vm: &Vm) -> Vec<(u8, String, Vec<String>)> {
    let text = |id: u16| vm.string(id).unwrap_or_default().to_owned();
    vm.picks()
        .iter()
        .map(|o| (o.slot, text(o.title), o.rows.iter().map(|&r| text(r)).collect()))
        .collect()
}

/// The next player to ask, at `tick`, over this machine's local `seats`
/// (roster slot and controller port). Offers the VM no longer holds (the
/// round ended) reset the asking, so the next round's are asked afresh.
pub fn plan(vm: &Vm, seats: &[(u8, u32)], tick: u32) -> Option<Ask> {
    let now = offers(vm);
    with_state(|s| {
        if now.is_empty() {
            *s = State { reports: std::mem::take(&mut s.reports), ..State::default() };
            return None;
        }
        if s.key.as_ref() != Some(&now) {
            s.queue.clear();
            s.open_since = None;
            for &(player, port) in seats {
                for (slot, title, rows) in &now {
                    if !rows.is_empty() {
                        s.queue.push_back(Ask { player, port, slot: *slot, title: title.clone(), rows: rows.clone() });
                    }
                }
            }
            s.key = Some(now);
        }
        if let Some(since) = s.open_since {
            if tick.saturating_sub(since) < OPEN_TICKS {
                return None;
            }
            s.open_since = None;
        }
        let ask = s.queue.pop_front()?;
        s.open_since = Some(tick);
        Some(ask)
    })
}

/// A press on a row of the picker `slot` by `player`: kept for the next
/// boundary, and the next player's picker may open.
pub fn report(player: u8, slot: u8, row: usize) {
    let Ok(row) = u8::try_from(row) else { return };
    with_state(|s| {
        s.reports.push(Report { player, slot, row });
        s.open_since = None;
    });
}

/// The picks made since the last call, in order.
pub fn take() -> Vec<Report> {
    with_state(|s| std::mem::take(&mut s.reports))
}

/// Opens the value picker over `ask.port`'s controller with the offer's
/// title and rows (a call of the boundary).
pub async fn open(cx: &Cx, ask: &Ask) -> anyhow::Result<()> {
    let (player, slot) = (ask.player, ask.slot);
    let set = Arc::new(move |row: usize| {
        report(player, slot, row);
        format!("rules pick: player {player} picked row {row} of picker {slot}")
    });
    let pick = Pick::new(&ask.title, ask.rows.clone(), 0, HELP, set);
    let name = picker::offer_list(pick).ok_or_else(|| anyhow::anyhow!("the picker {} has no rows", ask.slot))?;
    let ingame = cx.read(list_class::ingame).unwrap_or(true);
    let name = picker::screen_for(name, ingame);
    let loader = list_class::loader_of(name).ok_or_else(|| anyhow::anyhow!("{name} is off; the picker is not opened"))?;
    let mask = 1u32 << ask.port.min(3);
    if let Some(d) = dialog_tag::by_loader(loader) {
        return dialog_tag::open_over(cx, &d, mask).await.map_err(guest);
    }
    cx.call(GuestCall::new(points::OPEN_SCREEN.base).args([mask, loader]))
        .await
        .map_err(guest)?;
    Ok(())
}

fn guest(e: impl Into<EntryError>) -> anyhow::Error {
    anyhow::anyhow!("{}", e.into())
}

/// This machine's local players: roster slot and controller port.
pub fn seats(m: &dyn GuestMemory) -> Vec<(u8, u32)> {
    super::show::local_seats(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rules::fake::FakeGame;
    use ::rules::vm::Input;

    const SRC: &str = "rules \"P\" 1\ntype slayer\nrequires player.pick\n\
        on round_start { offer_pick(0, \"LOADOUT\")\n pick_row(0, \"A\")\n pick_row(0, \"B\") }\n";

    fn vm_with_offers() -> Vm {
        let mut vm = Vm::new(::rules::compile(SRC).unwrap(), 0, 0).unwrap();
        let mut g = FakeGame::with_players(2);
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        vm
    }

    #[test]
    fn local_players_are_asked_one_at_a_time_and_a_press_opens_the_next() {
        let vm = vm_with_offers();
        let seats = [(0u8, 0u32), (1, 1)];
        let first = plan(&vm, &seats, 5).unwrap();
        assert_eq!((first.player, first.port, first.slot), (0, 0, 0));
        assert_eq!((first.title.as_str(), first.rows.as_slice()), ("LOADOUT", ["A".to_string(), "B".to_string()].as_slice()));
        // The first picker is open: nobody else is asked yet.
        assert_eq!(plan(&vm, &seats, 6), None);
        report(0, 0, 1);
        assert_eq!(take(), [Report { player: 0, slot: 0, row: 1 }]);
        let second = plan(&vm, &seats, 7).unwrap();
        assert_eq!((second.player, second.port), (1, 1));
        report(1, 0, 0);
        assert_eq!(plan(&vm, &seats, 8), None, "everyone was asked once");
    }

    #[test]
    fn a_picker_nobody_answers_gives_way_after_the_wait() {
        let vm = vm_with_offers();
        let seats = [(0u8, 0u32), (1, 1)];
        assert_eq!(plan(&vm, &seats, 0).unwrap().player, 0);
        assert_eq!(plan(&vm, &seats, OPEN_TICKS - 1), None);
        assert_eq!(plan(&vm, &seats, OPEN_TICKS).unwrap().player, 1);
    }

    #[test]
    fn the_next_round_is_asked_afresh_once_the_offers_were_taken() {
        let mut vm = vm_with_offers();
        let seats = [(0u8, 0u32)];
        assert!(plan(&vm, &seats, 1).is_some());
        report(0, 0, 0);
        assert_eq!(plan(&vm, &seats, 2), None);
        // The round ends: the VM drops its offers, and the asking resets.
        let mut g = FakeGame::with_players(2);
        vm.begin_tick(3);
        vm.fire(&mut g, Input::RoundEnd);
        assert_eq!(plan(&vm, &seats, 3), None);
        assert_eq!(take().len(), 1, "a report made before the reset is still handed over");
        vm.begin_tick(4);
        vm.fire(&mut g, Input::RoundStart);
        assert!(plan(&vm, &seats, 4).is_some());
    }

    #[test]
    fn a_remote_player_is_not_asked() {
        let vm = vm_with_offers();
        assert_eq!(plan(&vm, &[], 1), None);
    }

    #[test]
    fn a_row_past_a_byte_is_not_reported() {
        report(0, 0, 300);
        assert!(take().is_empty());
    }
}
