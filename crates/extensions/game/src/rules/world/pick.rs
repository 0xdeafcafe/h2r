//! The pre-round picker (docs/game/rules-world-api.md, "Pickers"): a
//! script's `offer_pick` and `pick_row` on players, and their picks back
//! to the VM.
//!
//! The VM keeps the offers (`rules::vm::RulesExt::picks`) and each player's
//! pick (`Vm::choose`). On the host, [`plan`] reads the offers after the
//! tick's queue is drained and, once for each new set of offers, asks each
//! local player in turn (the next opens when the last one picked, or after
//! [`OPEN_TICKS`]): a [`Call::OfferPick`] the boundary makes with [`open`],
//! which opens the game's value picker (`crate::ui_kit::picker`,
//! `kit.picker.game`, a select dialog over the game: no screen of its own)
//! over the player's controller, with the offer's title and rows. A press
//! on a row is [`report`]ed from the picker's setter; [`take`] hands the
//! reports to the VM at the next boundary, which `picked` then reads.
//!
//! On a client, the offers arrive the other way: the show channel's `Pick`
//! cues ([`client_offer`], `rules::show::Cue::Pick`), and the client asks
//! its own local players from them at each tick boundary
//! ([`client_tick`], while [`client_busy`]). A press goes back up the
//! actor channel as a pick message, and the host's
//! [`receive`](crate::actor_net::pick::receive) hands it to the same
//! reports, so the VM reads a remote pick like a local one.
//!
//! [`Call::OfferPick`]: super::super::runtime::Call::OfferPick

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};

use ::rules::vm::Vm;
use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::{GuestMemory, points};

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
    /// The tick the open picker was asked at and its slot.
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

/// The VM's offers as strings: picker number, title, rows.
fn offers(vm: &Vm) -> Vec<(u8, String, Vec<String>)> {
    let text = |id: u16| vm.string(id).unwrap_or_default().to_owned();
    // The loadout pickers are asked through the player's window
    // (`super::choice`).
    vm.plain_picks()
        .iter()
        .map(|o| (o.slot, text(o.title), o.rows.iter().map(|&r| text(r)).collect()))
        .collect()
}

/// The next player to ask, at `tick`, over this machine's local `seats`
/// (roster slot and controller port). Offers the VM no longer holds (the
/// round ended) reset the asking, so the next round's are asked afresh.
pub fn plan(vm: &Vm, seats: &[(u8, u32)], tick: u32) -> Option<Ask> {
    plan_from(offers(vm), seats, tick)
}

/// [`plan`] from offers read wherever they came from: the VM's on the
/// host, the show channel's cues on a client.
pub fn plan_from(now: Vec<(u8, String, Vec<String>)>, seats: &[(u8, u32)], tick: u32) -> Option<Ask> {
    with_state(|s| {
        if now.is_empty() {
            *s = State { reports: std::mem::take(&mut s.reports), ..State::default() };
            return None;
        }
        if s.key.as_ref() != Some(&now) {
            s.queue.clear();
            s.open = None;
            for &(player, port) in seats {
                for (slot, title, rows) in &now {
                    if !rows.is_empty() {
                        s.queue.push_back(Ask { player, port, slot: *slot, title: title.clone(), rows: rows.clone() });
                    }
                }
            }
            s.key = Some(now);
        }
        next_ask(&mut s.queue, &mut s.open, tick)
    })
}

/// The ask the queue yields at `tick`, held back while the open picker
/// waits for its player.
pub(super) fn next_ask(queue: &mut VecDeque<Ask>, open: &mut Option<(u32, u8)>, tick: u32) -> Option<Ask> {
    if let Some((since, _)) = *open {
        if tick.wrapping_sub(since) < OPEN_TICKS {
            return None;
        }
        *open = None;
    }
    let ask = queue.pop_front()?;
    *open = Some((tick, ask.slot));
    Some(ask)
}

/// A press on a row of the picker `slot` by `player`: kept for the next
/// boundary, and the next player's picker may open. Whether it was kept.
pub fn report(player: u8, slot: u8, row: usize) -> bool {
    let Ok(row) = u8::try_from(row) else { return false };
    with_state(|s| {
        s.reports.push(Report { player, slot, row });
        s.open = None;
    });
    true
}

/// The picks made since the last call, in order.
pub fn take() -> Vec<Report> {
    with_state(|s| std::mem::take(&mut s.reports))
}

/// Opens the value picker over `ask.port`'s controller with the offer's
/// title and rows (a call of the boundary). A press is a report to the
/// VM at the next boundary.
pub async fn open(cx: &Cx, ask: &Ask) -> anyhow::Result<()> {
    let (player, slot) = (ask.player, ask.slot);
    let set = Arc::new(move |row: usize| {
        report(player, slot, row);
        format!("rules pick: player {player} picked row {row} of picker {slot}")
    });
    show_picker(cx, ask, set).await
}

/// Opens the value picker for a client machine's local player: a press
/// goes back up the actor channel (`crate::actor_net::pick`), not into
/// this machine's reports, and closes the asking's wait either way.
pub async fn open_remote(cx: &Cx, ask: &Ask) -> anyhow::Result<()> {
    let (player, slot) = (ask.player, ask.slot);
    let set = Arc::new(move |row: usize| {
        let row = u8::try_from(row).unwrap_or(u8::MAX);
        if !crate::actor_net::pick::send(crate::actor_net::pick::Pick {
            player,
            slot,
            row,
        }) {
            eprintln!("rules: player {player}'s pick of picker {slot} is not carried: the outbox is full");
        }
        remote_reported();
        format!("rules pick: player {player} picked row {row} of picker {slot}, sent upstream")
    });
    show_picker(cx, ask, set).await
}

/// Opens the value picker over `ask.port`'s controller with the offer's
/// title and rows (a call of the boundary); `set` stores the row.
pub(super) async fn show_picker(cx: &Cx, ask: &Ask, set: Arc<dyn Fn(usize) -> String + Send + Sync>) -> anyhow::Result<()> {
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

// ---- The client half ----
//
// A machine that runs no VM is offered nothing: its offers arrive as the
// show channel's `Pick` cues (`super::show`), and its local players are
// asked from them; a press goes back up the actor channel
// (`crate::actor_net::pick`), and the host's `Vm::choose` validates it
// like a local one.

/// The asking on a client: the offers last applied, the players not yet
/// asked and the tick the open picker was asked at.
#[derive(Default)]
struct ClientState {
    offers: BTreeMap<u8, (String, Vec<String>)>,
    queue: VecDeque<Ask>,
    open: Option<(u32, u8)>,
}

#[cfg(not(test))]
static CLIENT: Mutex<Option<ClientState>> = Mutex::new(None);

#[cfg(not(test))]
fn with_client<T>(f: impl FnOnce(&mut ClientState) -> T) -> T {
    f(CLIENT.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(ClientState::default))
}

// Tests run side by side, each on its own thread: each has its own half.
#[cfg(test)]
thread_local! {
    static CLIENT: Mutex<Option<ClientState>> = const { Mutex::new(None) };
    static RECEIVED: Mutex<Option<Vec<(u8, String, Vec<String>)>>> = const { Mutex::new(None) };
}

#[cfg(test)]
fn with_client<T>(f: impl FnOnce(&mut ClientState) -> T) -> T {
    CLIENT.with(|s| f(s.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(ClientState::default)))
}

/// The offer cues received since the last tick boundary, in arrival
/// order: applied as a set at the boundary, so the cues of one tick
/// change the asking once. `None` is no cue this tick, which changes
/// nothing.
#[cfg(not(test))]
static RECEIVED: Mutex<Option<Vec<(u8, String, Vec<String>)>>> = Mutex::new(None);

#[cfg(not(test))]
fn with_received<T>(f: impl FnOnce(&mut Option<Vec<(u8, String, Vec<String>)>>) -> T) -> T {
    f(&mut RECEIVED.lock().unwrap_or_else(|e| e.into_inner()))
}

#[cfg(test)]
fn with_received<T>(f: impl FnOnce(&mut Option<Vec<(u8, String, Vec<String>)>>) -> T) -> T {
    RECEIVED.with(|r| f(&mut r.lock().unwrap_or_else(|e| e.into_inner())))
}

/// A picker's offer a client received (the show channel's `Pick` cue,
/// `super::show`): kept for the next tick boundary, where the asking is
/// planned from the whole set of them. An offer with no rows takes the
/// picker's offer away.
pub fn client_offer(slot: u8, title: String, rows: Vec<String>) {
    with_received(|r| {
        r.get_or_insert_with(Vec::new).push((slot, title, rows));
    });
}

/// Forgets what the client half holds: a new match, a lost game, a
/// snapshot restore or a channel closed for the match starts it empty.
pub fn client_reset() {
    with_client(|s| *s = ClientState::default());
    with_received(|r| *r = None);
}

/// Whether picks travel upstream, as the transport says once a tick
/// (`crate::actor_net_link::pick_upstream`): a tunnel that negotiated the
/// actor channel does. The client asks nobody otherwise, so nobody is
/// offered a pick that cannot travel.
static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Sets whether the actor channel carries picks upstream; the transport
/// answers once a tick (`crate::actor_net::client`'s caller).
pub fn client_ready(ready: bool) {
    READY.store(ready, std::sync::atomic::Ordering::Relaxed);
}

fn ready() -> bool {
    READY.load(std::sync::atomic::Ordering::Relaxed)
}

/// Whether anything waits: an offer received, a player not yet asked or
/// a picker open. The client's tick boundary runs while it is
/// (`crate::actor_net::client`), so the asking advances.
pub fn client_busy() -> bool {
    let busy = |s: &mut ClientState| !s.queue.is_empty() || s.open.is_some();
    ready() && (with_received(|r| r.is_some()) || with_client(busy))
}

/// The asking at the tick boundary: the offers received since the last
/// one are applied as a set, and the next local player is asked. An
/// offer that changed is a new set, asked afresh as on the host; only
/// pickers added or taken away extend or trim the asking in place, so
/// the cues of one tick do not reopen the first picker.
pub fn client_plan(tick: u32, seats: &[(u8, u32)]) -> Option<Ask> {
    let Some(received) = with_received(std::mem::take) else {
        return with_client(|s| next_ask(&mut s.queue, &mut s.open, tick));
    };
    with_client(|s| {
        // Cues for separate slots can arrive at different boundaries.
        let mut now = s.offers.clone();
        for (slot, title, rows) in received {
            if rows.is_empty() {
                now.remove(&slot);
            } else {
                now.insert(slot, (title, rows));
            }
        }

        let replaced = now.iter().any(|(slot, offer)| {
            s.offers.get(slot).is_some_and(|previous| previous != offer)
        });
        if replaced {
            s.queue.clear();
            s.open = None;
        } else {
            s.queue.retain(|ask| now.contains_key(&ask.slot));
            if s.open.is_some_and(|(_, slot)| !now.contains_key(&slot)) {
                s.open = None;
            }
        }
        for &(player, port) in seats {
            for (&slot, (title, rows)) in &now {
                if replaced || !s.offers.contains_key(&slot) {
                    s.queue.push_back(Ask { player, port, slot, title: title.clone(), rows: rows.clone() });
                }
            }
        }
        s.offers = now;
        next_ask(&mut s.queue, &mut s.open, tick)
    })
}

/// The client's tick boundary: asks the next local player, whose picker
/// sends its press upstream — while [`client_ready`] says the channel
/// carries it. Runs while [`client_busy`]
/// (`crate::actor_net::client`).
pub async fn client_tick(cx: &Cx, tick: u32) {
    let seats = cx.read(|m| seats(m));
    // The id the host finds this machine's players by (`actor_net::pick`).
    crate::actor_net::pick::set_local_machine(cx.read(|m| crate::actor_net::pick::read_local_machine(m)));
    let ask = client_plan(tick, &seats);
    if !ready() {
        return;
    }
    if let Some(ask) = ask
        && let Err(error) = open_remote(cx, &ask).await
    {
        eprintln!("rules: player {}'s picker did not open: {error}", ask.player);
    }
}

/// A press on a client: the picker closed, so the next player's may open
/// whether or not the send went out.
fn remote_reported() {
    with_client(|s| s.open = None);
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

    /// The whole host path of a client's pick: the tunnel's message lands
    /// in the reports (`actor_net::pick::receive`), the runtime hands each
    /// to the VM at the boundary, and the VM holds it where `picked(p, n)`
    /// reads it. A pick for a player outside the sender's seats never gets
    /// there.
    #[test]
    fn a_clients_pick_reaches_the_vm_where_picked_reads_it() {
        use crate::actor_net::pick::{Pick, receive};
        let mut vm = vm_with_offers();
        let _ = take();
        // Player 1 sits on the sending machine; player 0 does not.
        assert!(receive(&Pick { player: 1, slot: 0, row: 1 }.encode(), &[1]));
        assert!(!receive(&Pick { player: 0, slot: 0, row: 1 }.encode(), &[1]));
        for pick in take() {
            assert!(vm.choose(pick.player, pick.slot, pick.row));
        }
        assert_eq!(vm.choices(), [::rules::vm::Choice { player: 1, slot: 0, row: 1 }]);
        // A row the picker does not hold is refused by the VM, not kept.
        assert!(receive(&Pick { player: 1, slot: 0, row: 5 }.encode(), &[1]));
        let held = take();
        assert!(held.iter().all(|p| !vm.choose(p.player, p.slot, p.row)));
        assert_eq!(vm.choices().len(), 1);
    }

    #[test]
    fn a_remote_player_is_not_asked() {
        let vm = vm_with_offers();
        assert_eq!(plan(&vm, &[], 1), None);
    }

    #[test]
    fn a_row_past_a_byte_is_not_reported() {
        assert!(!report(0, 0, 300));
        assert!(take().is_empty());
    }

    // The client half: the offers arrive as cues, and the asking plans
    // from the set of them.

    fn offered(slot: u8, title: &str, rows: &[&str]) {
        client_offer(
            slot,
            title.into(),
            rows.iter().map(|r| (*r).into()).collect(),
        );
    }

    #[test]
    fn a_client_asks_its_local_players_from_the_received_offers() {
        client_ready(true);
        offered(0, "LOADOUT", &["A", "B"]);
        let seats = [(2u8, 1u32)];
        let ask = client_plan(5, &seats).unwrap();
        assert_eq!((ask.player, ask.port, ask.slot), (2, 1, 0));
        assert_eq!((ask.title.as_str(), ask.rows.as_slice()), ("LOADOUT", ["A".to_string(), "B".to_string()].as_slice()));
        // The picker is open: nobody else is asked yet, and nothing waits
        // once the press closed it.
        assert_eq!(client_plan(6, &seats), None);
        assert!(client_busy());
        remote_reported();
        assert!(!client_busy());
    }

    #[test]
    fn a_changed_offer_asks_afresh_and_an_added_one_only_extends_the_asking() {
        let seats = [(0u8, 0u32)];
        offered(0, "LOADOUT", &["A", "B"]);
        let first = client_plan(1, &seats).unwrap();
        assert_eq!(first.slot, 0);
        // A second picker's cue arrives the tick after: the asking grows
        // at the back, and the open picker keeps its wait.
        offered(1, "WEAPON", &["RIFLE"]);
        assert_eq!(client_plan(2, &seats), None);
        remote_reported();
        let second = client_plan(3, &seats).unwrap();
        assert_eq!((second.slot, second.title.as_str()), (1, "WEAPON"));
        // The offer itself changed: asked players are asked again, as on
        // the host.
        offered(0, "LOADOUT", &["A", "B", "C"]);
        let again = client_plan(4, &seats).unwrap();
        assert_eq!(again.slot, 0);
        assert_eq!(again.rows.len(), 3);
        // A repeated cue leaves both the open picker and the queue alone.
        offered(0, "LOADOUT", &["A", "B", "C"]);
        assert_eq!(client_plan(5, &seats), None);
        remote_reported();
        assert_eq!(client_plan(6, &seats).unwrap().slot, 1);
    }

    #[test]
    fn an_offer_taken_away_ends_its_asking_and_an_emptied_set_asks_afresh_later() {
        client_ready(true);
        let seats = [(0u8, 0u32), (1, 1)];
        offered(0, "LOADOUT", &["A"]);
        offered(1, "WEAPON", &["RIFLE"]);
        let ask = client_plan(1, &seats).unwrap();
        assert_eq!((ask.player, ask.slot), (0, 0));
        // Retracting the open picker releases its wait and removes its
        // queued asks while leaving the other picker in order.
        offered(0, "", &[]);
        let next = client_plan(2, &seats).unwrap();
        assert_eq!((next.player, next.slot), (0, 1));
        // The round ended: the host took both pickers away, and the
        // asking resets so the next round's are asked afresh.
        offered(1, "", &[]);
        assert!(client_busy(), "the retracts wait for the boundary");
        assert_eq!(client_plan(3, &seats), None);
        assert!(!client_busy());
        offered(0, "LOADOUT", &["A"]);
        assert_eq!(client_plan(4, &seats).unwrap().slot, 0);
    }
}
