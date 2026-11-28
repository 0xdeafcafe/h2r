//! The loadout choice (docs/game/rules-engine.md, "Phases: attack and
//! defend"): a script's `offer(p, picker, count)` opens the first `count`
//! rows of a picker to a player, `choice(p)` reads the row they last
//! picked, and a pick before a spawn is the loadout that spawn gives.
//!
//! The VM keeps each player's window and last row (`rules::vm::Window`,
//! in the `RULES` record, so checkpoints, snapshots and a film's keyframes
//! carry it). On the host, [`plan`] reads the windows after the tick's
//! queue is drained and asks each local player once for each new window
//! (a heavier tier opens, or a new round's): a [`Call::OfferChoice`] the
//! boundary makes with [`open`], which opens the game's select dialog
//! (`crate::ui_kit::picker`, the same one [`super::pick`] opens) over the
//! player's controller with the open rows. A press is [`report`]ed and
//! [`take`]n to the VM at the next boundary, where `Vm::choose_loadout`
//! refuses a row above the window. The dialog never blocks a spawn: a
//! player who picks nothing keeps the row they had, row 0 at first. A
//! spawn asks the player's window again ([`spawned`]).
//!
//! The loadout pickers are not asked as plain pickers
//! (`Vm::plain_picks`), and a film replays the VM's state, not the dialog.
//!
//! [`Call::OfferChoice`]: super::super::runtime::Call::OfferChoice

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use ::rules::ty::MAX_PLAYERS;
use ::rules::vm::Vm;
use guest_layout::entry::Cx;

use super::pick::{Ask, next_ask};

/// A loadout pick made: row `row` of `player`'s window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Report {
    pub player: u8,
    pub row: u8,
}

#[derive(Default)]
struct State {
    /// The window (picker, rows open) each player was last asked about.
    asked: [Option<(u8, u8)>; MAX_PLAYERS],
    queue: VecDeque<Ask>,
    /// The tick the open dialog was asked at and its picker.
    open: Option<(u32, u8)>,
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

/// The next local player to ask, at `tick`, over this machine's `seats`
/// (roster slot and controller port). A player whose window is gone (the
/// round ended) is asked afresh when the next one opens.
pub fn plan(vm: &Vm, seats: &[(u8, u32)], tick: u32) -> Option<Ask> {
    with_state(|s| {
        for &(player, port) in seats {
            let Some(w) = vm.window(player) else { continue };
            if w.count == 0 {
                s.asked[player as usize] = None;
                continue;
            }
            if s.asked[player as usize] == Some((w.picker, w.count)) {
                continue;
            }
            let Some(offer) = vm.picks().iter().find(|o| o.slot == w.picker) else { continue };
            let text = |id: u16| vm.string(id).unwrap_or_default().to_owned();
            let rows: Vec<String> = offer.rows.iter().take(w.count as usize).map(|&r| text(r)).collect();
            if rows.is_empty() {
                continue;
            }
            // A window that grew replaces a dialog still waiting for this player.
            s.queue.retain(|a| a.player != player);
            s.asked[player as usize] = Some((w.picker, w.count));
            s.queue.push_back(Ask { player, port, slot: w.picker, title: text(offer.title), rows });
        }
        next_ask(&mut s.queue, &mut s.open, tick)
    })
}

/// `player` spawned: their window is asked again, so the dialog opens over
/// each new body and not only when a window grows. A spawn the film replays
/// asks nobody (`plan` is not called there).
pub fn spawned(player: u8) {
    with_state(|s| {
        if let Some(a) = s.asked.get_mut(player as usize) {
            *a = None;
        }
    });
}

/// A press on a row of `player`'s dialog: kept for the next boundary, and
/// the next player's dialog may open. Whether it was kept.
pub fn report(player: u8, row: usize) -> bool {
    let Ok(row) = u8::try_from(row) else { return false };
    with_state(|s| {
        s.reports.push(Report { player, row });
        s.open = None;
    });
    true
}

/// The picks made since the last call, in order.
pub fn take() -> Vec<Report> {
    with_state(|s| std::mem::take(&mut s.reports))
}

/// Forgets what was asked: a new match starts with every player unasked.
pub fn reset() {
    with_state(|s| *s = State::default());
}

/// Opens the select dialog over `ask.port`'s controller with the player's
/// open rows (a call of the boundary). A press is a report to the VM at the
/// next boundary.
pub async fn open(cx: &Cx, ask: &Ask) -> anyhow::Result<()> {
    let player = ask.player;
    let set = Arc::new(move |row: usize| {
        report(player, row);
        format!("rules loadout: player {player} picked row {row}")
    });
    super::pick::show_picker(cx, ask, set).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::rules::fake::FakeGame;
    use ::rules::vm::{Input, RulesExt};

    /// Each spawn opens `tier` rows of picker 0 to the player and reads the
    /// row they hold.
    const SRC: &str = "rules \"C\" 1\ntype slayer\nrequires player.pick\n\
        global var tier: int = 2\nplayer var row: int = -9\n\
        on round_start { offer_pick(0, \"LOADOUT\")\n pick_row(0, \"A\")\n pick_row(0, \"B\")\n pick_row(0, \"C\")\n pick_row(0, \"D\") }\n\
        on spawn(p) { offer(p, 0, tier)\n p.row = choice(p) }\n";

    fn spawned() -> (Vm, FakeGame) {
        reset();
        let mut vm = Vm::new(::rules::compile(SRC).unwrap(), 0, 0).unwrap();
        let mut g = FakeGame::with_players(2);
        vm.begin_tick(0);
        vm.fire(&mut g, Input::RoundStart);
        vm.fire(&mut g, Input::Spawn(0));
        assert_eq!(g.script_error, None);
        (vm, g)
    }

    #[test]
    fn default_is_row_zero() {
        let (vm, _) = spawned();
        assert_eq!(vm.player_var(0, "row"), Some(0));
        assert_eq!(vm.window(0).map(|w| (w.picker, w.count, w.row)), Some((0, 2, 0)));
    }

    #[test]
    fn a_row_above_the_tier_is_refused() {
        let (mut vm, mut g) = spawned();
        // Two rows are open: row 2 is the next tier's.
        assert!(!vm.choose_loadout(0, 2));
        assert!(vm.choose_loadout(0, 1));
        assert!(!vm.choose_loadout(0, 3), "the last pick stands");
        vm.begin_tick(1);
        vm.fire(&mut g, Input::Spawn(0));
        assert_eq!(vm.player_var(0, "row"), Some(1), "no pick keeps the last one");
        // A player outside the roster has no window.
        assert!(!vm.choose_loadout(200, 0));
    }

    #[test]
    fn a_pick_is_filmed() {
        let (mut vm, mut g) = spawned();
        assert!(vm.choose_loadout(0, 1));
        // The pick is in the state a film keyframe and a checkpoint carry:
        // a VM resumed from its bytes reads the same row.
        let ext = RulesExt::decode(&vm.ext().encode()).unwrap();
        assert_eq!(&ext, vm.ext());
        let mut again = Vm::resume(::rules::compile(SRC).unwrap(), 0, ext).unwrap();
        again.begin_tick(1);
        again.fire(&mut g, Input::Spawn(0));
        assert_eq!(again.player_var(0, "row"), Some(1));
    }

    #[test]
    fn a_spawn_asks_the_same_window_again() {
        let (vm, _) = spawned();
        let seats = [(0u8, 0u32)];
        assert!(plan(&vm, &seats, 5).is_some());
        assert_eq!(plan(&vm, &seats, 6), None);
        assert!(report(0, 0));
        super::spawned(0);
        let again = plan(&vm, &seats, 7).expect("the next spawn opens the dialog again");
        assert_eq!((again.player, again.slot), (0, 0));
        super::spawned(9);
    }

    #[test]
    fn a_new_window_is_asked_once_and_a_press_closes_the_ask() {
        let (vm, _) = spawned();
        let seats = [(0u8, 0u32)];
        let ask = plan(&vm, &seats, 5).unwrap();
        assert_eq!((ask.player, ask.slot, ask.rows.as_slice()), (0, 0, ["A".to_string(), "B".to_string()].as_slice()));
        assert_eq!(plan(&vm, &seats, 6), None, "asked once");
        assert!(report(0, 1));
        assert_eq!(take(), [Report { player: 0, row: 1 }]);
    }
}
