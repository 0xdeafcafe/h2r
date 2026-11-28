//! Opening and closing screens of ours, and work that waits for a closed
//! screen to be freed (docs/game/ui-screens.md "Lifecycle").
//!
//! - **Open**: `0x149f1e(player mask, loader)` ([`crate::list_class::open`]).
//! - **Close**: `0x14800c` with EAX the screen's slot (`+0x78`) and ECX its
//!   window (`+0x74`), as the generic back handler (`0x2300ea`) closes on
//!   B. It starts the out transition; the stack frees the screen when that
//!   ends, calling the destructor (screen slot 0) first.
//! - **After close**: anything that must run on the screen underneath (a
//!   retail press, a level start) waits for the destructor
//!   ([`AfterClose`]). The skulls page's first build ran the difficulty
//!   press while the page was still closing; the press stored the
//!   difficulty, the closing page's teardown reset it, and Start seemed
//!   stuck.
//! - **Timing**: a screen of ours comes in within the kit's open budget
//!   and goes within its close budget ([`cap_transition`],
//!   [`crate::ui_kit::animation::Transition::millis`]), whatever the
//!   retail tracks it borrows ask for.
//! - **While closing**: the closing screen still holds its window, so
//!   the dispatcher keeps giving it every press until it is freed. Past
//!   the closing press itself ([`CLOSE_GUARD`]) it hands each press to
//!   the screen beneath ([`while_closing`], [`beneath_for`]), so the lobby
//!   takes input as soon as the close starts.

use std::collections::BTreeMap;
use std::sync::Mutex;

use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::memory::MemFault;

/// Retail's close of a window's top screen.
pub const SCREEN_CLOSE: u32 = 0x0014_800c;
/// Parent of a widget on the channel focus path.
pub const SCREEN_NEXT: u32 = 0x10;
/// A screen's window and slot words.
pub const SCREEN_WINDOW: u32 = 0x74;
pub const SCREEN_SLOT: u32 = 0x78;

/// `screen`'s window and slot.
pub fn window_slot(cx: &Cx, screen: u32) -> Result<(u32, u32), EntryError> {
    Ok(cx.read(|m| -> Result<_, MemFault> {
        Ok((m.u32(screen + SCREEN_WINDOW)?, m.u32(screen + SCREEN_SLOT)?))
    })?)
}

/// Asks the stack to close `screen`, as B does.
pub async fn close(cx: &Cx, screen: u32) -> Result<(), EntryError> {
    let (window, slot) = window_slot(cx, screen)?;
    close_slot(cx, window, slot).await
}

/// The close call alone, for a screen whose window and slot are known.
pub async fn close_slot(cx: &Cx, window: u32, slot: u32) -> Result<(), EntryError> {
    cx.call(GuestCall::new(SCREEN_CLOSE).eax(slot).ecx(window))
        .await?;
    Ok(())
}

/// A screen's message entry: `0x22e957(kind)`, ECX the screen, stdcall.
/// It hands a message of `kind` to the screen and its children. The close
/// (`0x14800c`) ends in the window's `0x2351d4`, which sends kind
/// [`MESSAGE_CLOSE`] to the window's top screen unless it is closing.
pub const SCREEN_MESSAGE: u32 = 0x0022_e957;
/// The close message's kind.
pub const MESSAGE_CLOSE: u32 = 2;
/// A screen's state byte; bit 1 set while it is closing.
pub const SCREEN_STATE: u32 = 0x42;
/// The window's top screen word (`0x2351d4` reads it).
pub const WINDOW_TOP: u32 = 0x8;

/// The window object `0x14800c` closes through for `window` and `slot`:
/// window 2's one at `0x54dba8`, window 3's `0x38`-byte ones from
/// `0x54d76c`, window 5's `0x40`-byte ones from `0x54d62c`. Any other
/// window: none (the close does nothing).
pub fn window_object(window: u32, slot: u32) -> Option<u32> {
    match window {
        2 => Some(0x0054_dba8),
        3 => Some(0x0054_d76c + slot.wrapping_mul(0x38)),
        5 => Some(0x0054_d62c + slot.wrapping_mul(0x40)),
        _ => None,
    }
}

/// Whether `screen` takes a close message: not already closing.
pub fn takes_close(m: &dyn guest_layout::memory::GuestMemory, screen: u32) -> Result<bool, MemFault> {
    Ok(m.u8(screen + SCREEN_STATE)? & 2 == 0)
}

// --- How long a screen of ours takes to come and go -------------------------

/// A screen's transition words (`crate::ui_transition` reads them for the
/// agent API's `layers`): the delay before it starts (`+0x44`), when it
/// started on the UI clock (`+0x4c`) and how long it plays (`+0x54`), all
/// in milliseconds. The state byte's bit 0 is set while it comes in, bit 1
/// while it goes out ([`SCREEN_STATE`]). The window frees a closing screen
/// once `delay + length` has passed, and until then the input dispatcher
/// (`0x147dbe`, `crate::ui_watch`) still gives the closing screen every
/// press: the first window holding a screen wins.
pub const SCREEN_DELAY: u32 = 0x44;
pub const SCREEN_STARTED: u32 = 0x4c;
pub const SCREEN_LENGTH: u32 = 0x54;
/// The state byte's bits: coming in, going out.
pub const STATE_IN: u8 = 1;
pub const STATE_OUT: u8 = 2;

/// `(delay, length)` held to `budget` milliseconds in all: unchanged when
/// they fit, else the delay kept up to the budget and the length the rest.
/// A transition is only ever shortened.
pub fn capped(delay: u32, length: u32, budget: u32) -> (u32, u32) {
    if delay.saturating_add(length) <= budget {
        return (delay, length);
    }
    let delay = delay.min(budget);
    (delay, budget - delay)
}

/// Holds `screen`'s running transition to the kit's budget
/// ([`crate::ui_kit::animation::Transition::millis`]): an opening one to
/// the open's, a closing one to the close's, so the closing screen is
/// gone (and freed) by then whatever its tracks ask for. A screen in
/// neither is left alone. Returns the transition it held, if any.
pub fn cap_transition(
    m: &mut dyn guest_layout::memory::GuestMemory,
    screen: u32,
) -> Result<Option<crate::ui_kit::animation::Transition>, MemFault> {
    use crate::ui_kit::animation::Transition;
    let state = m.u8(screen + SCREEN_STATE)?;
    let t = if state & STATE_OUT != 0 {
        Transition::Close
    } else if state & STATE_IN != 0 {
        Transition::Open
    } else {
        return Ok(None);
    };
    let (delay, length) = (m.u32(screen + SCREEN_DELAY)?, m.u32(screen + SCREEN_LENGTH)?);
    let (d, l) = capped(delay, length, t.millis());
    if (d, l) != (delay, length) {
        m.set_u32(screen + SCREEN_DELAY, d)?;
        m.set_u32(screen + SCREEN_LENGTH, l)?;
    }
    Ok(Some(t))
}

// --- Input while a screen of ours closes ------------------------------------

/// A press this soon after a screen of ours started closing is the press
/// that closed it, seen again: dropped, not handed down. The same reading
/// as the opener's guard ([`crate::ui_kit::input::OPENER_GUARD`]).
pub const CLOSE_GUARD: std::time::Duration = crate::ui_kit::input::OPENER_GUARD;

/// What a closing screen of ours does with a press `since` after it
/// started closing: drop it inside [`CLOSE_GUARD`] (the closing press),
/// else hand it to the screen beneath ([`beneath_for`]), which takes it as
/// if the closing one were already gone.
pub fn hands_down(since: std::time::Duration) -> bool {
    since >= CLOSE_GUARD
}

/// When each closing screen of ours started closing, by address. A screen
/// is told once ([`closing_since`] keeps the first), and forgotten when
/// it is freed ([`forget_closing`]) or when the list grows past what any
/// stack holds.
static CLOSING: Mutex<Vec<(u32, std::time::Instant)>> = Mutex::new(Vec::new());
const CLOSING_KEPT: usize = 16;

fn closing() -> std::sync::MutexGuard<'static, Vec<(u32, std::time::Instant)>> {
    CLOSING.lock().unwrap_or_else(|e| e.into_inner())
}

/// `screen` started closing at `at` (kept: the first time counts).
pub fn started_closing(screen: u32, at: std::time::Instant) {
    let mut v = closing();
    if v.iter().any(|(s, _)| *s == screen) {
        return;
    }
    v.push((screen, at));
    if v.len() > CLOSING_KEPT {
        v.remove(0);
    }
}

/// How long ago `screen` started closing, as of `now`; `None` when it was
/// never seen closing (its close came from the game itself, not through
/// us: it counts from its first press seen closing).
pub fn closing_since(screen: u32, now: std::time::Instant) -> Option<std::time::Duration> {
    closing()
        .iter()
        .find(|(s, _)| *s == screen)
        .map(|(_, at)| now.saturating_duration_since(*at))
}

/// `screen` was freed (or reused): its closing time goes.
pub fn forget_closing(screen: u32) {
    closing().retain(|(s, _)| *s != screen);
}

/// The first window which would receive the event after the closing
/// screen goes away. The channel stores a focused widget at +0x34.
/// Its +0x10 chain leads to the active screen (0x235246). Dispatch to
/// the first widget accepting the controller only after verifying that
/// root, as 0x23515d does. Pending replacements consume the press.
fn beneath_for(
    m: &dyn guest_layout::GuestMemory,
    screen: u32,
    mask: u16,
) -> Result<Option<u32>, MemFault> {
    use crate::ui_watch::{ACTIVE, PENDING, channel};
    let window = m.u32(screen + SCREEN_WINDOW)?;
    let slot = m.u32(screen + SCREEN_SLOT)?;
    for w in window..=5 {
        let Some(ch) = channel(w, slot) else { continue };
        let active = m.u32(ch + ACTIVE)?;
        if m.u32(ch + PENDING)? != 0 {
            return Ok(None);
        }
        if active == 0 || active == screen {
            continue;
        }
        if m.u8(active + SCREEN_STATE)? & (STATE_IN | STATE_OUT) != 0 {
            return Ok(None);
        }
        let mut at = m.u32(ch + 0x34)?;
        let mut receiver = None;
        for _ in 0..32 {
            if at == 0 { break; }
            if receiver.is_none() && m.u16(at + 8)? & mask != 0 {
                receiver = Some(at);
            }
            if at == active {
                return Ok(receiver);
            }
            at = m.u32(at + SCREEN_NEXT)?;
        }
        return Ok(None);
    }
    Ok(None)
}

/// What a press on screen `screen` of ours, closing, becomes: `None` when
/// the screen is not closing (it handles the press itself), else the
/// screen to hand it to, or `Some(None)` to drop it (inside the close
/// guard, or nothing beneath). `now` is when the press came.
pub fn while_closing(
    m: &dyn guest_layout::memory::GuestMemory,
    screen: u32,
    now: std::time::Instant,
    controller: u32,
) -> Result<Option<Option<u32>>, MemFault> {
    if m.u8(screen + SCREEN_STATE)? & STATE_OUT == 0 {
        return Ok(None);
    }
    started_closing(screen, now);
    let since = closing_since(screen, now).unwrap_or_default();
    if !hands_down(since) {
        return Ok(Some(None));
    }
    let mask = 1u16.checked_shl(controller).unwrap_or(0) & m.u16(screen + 8)?;
    Ok(Some(beneath_for(m, screen, mask)?))
}

/// Hands event `ev` to screen `to` through its own event slot (screen
/// slot 10, thiscall, the event, `ret 4`), as the dispatcher would.
pub async fn hand_down(cx: &Cx, to: u32, ev: u32) -> Result<u32, EntryError> {
    let handler = cx.read(|m| -> Result<u32, MemFault> {
        let vtable = m.u32(to)?;
        m.u32(vtable + 4 * crate::list_class::SCREEN_EVENT_SLOT)
    })?;
    Ok(cx.call(GuestCall::new(handler).ecx(to).args([ev])).await?.eax)
}

/// The row handle on the channel's focused widget path for `list`, as
/// LIST_FOCUSED_ROW finds it via the focused item's membership in that path.
pub(crate) fn focused_row_handle(
    m: &dyn guest_layout::GuestMemory,
    channel: u32,
    list: u32,
) -> Option<u32> {
    let mut child = m.u32(channel + 0x34).ok()?;
    for _ in 0..32 {
        if child == 0 {
            return None;
        }
        let parent = m.u32(child.checked_add(0x10)?).ok()?;
        if parent == list {
            return m
                .u32(child.checked_add(0x70)?)
                .ok()
                .filter(|&row| row != u32::MAX);
        }
        child = parent;
    }
    None
}

/// Work waiting for screens to be freed, by screen address: queued by
/// [`AfterClose::then`] when a screen is closed, taken by
/// [`AfterClose::freed`] from the class's destructor slot. A screen freed
/// with nothing queued (closed by B) takes nothing.
pub struct AfterClose<T> {
    waiting: Mutex<BTreeMap<u32, T>>,
}

impl<T> Default for AfterClose<T> {
    fn default() -> Self {
        AfterClose {
            waiting: Mutex::new(BTreeMap::new()),
        }
    }
}

impl<T> AfterClose<T> {
    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<u32, T>> {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Runs `work` once `screen` is freed. `false` when the screen already
    /// has work waiting (a second confirm while closing): the first stays.
    pub fn then(&self, screen: u32, work: T) -> bool {
        let mut w = self.lock();
        if w.contains_key(&screen) {
            return false;
        }
        w.insert(screen, work);
        true
    }
    /// Whether `screen` is closing with work waiting.
    pub fn closing(&self, screen: u32) -> bool {
        self.lock().contains_key(&screen)
    }
    /// Whether any screen is closing with work waiting.
    pub fn waiting(&self) -> bool {
        !self.lock().is_empty()
    }
    /// `screen` was freed: its waiting work, if any.
    pub fn freed(&self, screen: u32) -> Option<T> {
        self.lock().remove(&screen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::memory::GuestMemory;

    /// The crash picking a map in Select Map's second step: the categories
    /// were closed by window and slot from the maps screen's destructor.
    /// The stack (`0x2351ea`) had zeroed the window's top word for the
    /// screen it was freeing, so `0x2351d4` read the state byte at
    /// `0 + 0x42` (`MemoryFault ... addr: 42`, ESI `0x54d84c`: window 3,
    /// slot 4). The screen itself still reads.
    #[test]
    fn a_screen_under_one_being_freed_closes_by_itself_not_by_its_window() {
        let mut g = guest_layout::testing::FakeGuest::new();
        let window = window_object(3, 4).unwrap();
        assert_eq!(window, 0x0054_d84c, "the ESI of the crash");
        g.map(0x0054_d000, 0x1000);
        let categories = 0x8253_0000;
        g.map(categories, 0x1000);
        // Mid-free: the top word is 0.
        g.set_u32(window + WINDOW_TOP, 0).unwrap();
        let top = g.u32(window + WINDOW_TOP).unwrap();
        assert!(
            g.u8(top + SCREEN_STATE).is_err(),
            "the close by window reads 0x42"
        );
        // By the screen: it takes the close, once.
        assert_eq!(takes_close(&g, categories), Ok(true));
        g.set_u8(categories + SCREEN_STATE, 2).unwrap();
        assert_eq!(takes_close(&g, categories), Ok(false), "already closing");
        assert_eq!(window_object(4, 0), None);
        assert_eq!(window_object(2, 7), Some(0x0054_dba8));
        assert_eq!(window_object(5, 1), Some(0x0054_d66c));
    }

    /// A transition is held to its budget and never lengthened.
    #[test]
    fn a_transition_is_held_to_its_budget() {
        for (delay, length, budget, held) in [
            (0, 150, 200, (0, 150)),
            (50, 150, 200, (50, 150)),
            (0, 900, 200, (0, 200)),
            (100, 900, 200, (100, 100)),
            (400, 900, 200, (200, 0)),
            (u32::MAX, u32::MAX, 200, (200, 0)),
        ] {
            assert_eq!(capped(delay, length, budget), held, "{delay} + {length} in {budget}");
        }
    }

    const SCREEN: u32 = 0x8000_1000;

    fn screen_with(state: u8, delay: u32, length: u32) -> guest_layout::testing::FakeGuest {
        let mut g = guest_layout::testing::FakeGuest::new();
        g.map(SCREEN, 0x1000);
        g.put_u8(SCREEN + SCREEN_STATE, state)
            .put_u32(SCREEN + SCREEN_DELAY, delay)
            .put_u32(SCREEN + SCREEN_LENGTH, length);
        g
    }

    /// A screen of ours coming in is held to the open budget, going out
    /// to the close budget, and at rest it is left alone.
    #[test]
    fn a_screen_of_ours_comes_and_goes_within_the_kits_budgets() {
        use crate::ui_kit::animation::Transition;
        let mut g = screen_with(STATE_OUT, 0, 1200);
        assert_eq!(cap_transition(&mut g, SCREEN), Ok(Some(Transition::Close)));
        assert_eq!(g.u32(SCREEN + SCREEN_LENGTH), Ok(Transition::Close.millis()));
        let mut g = screen_with(STATE_IN, 0, 1200);
        assert_eq!(cap_transition(&mut g, SCREEN), Ok(Some(Transition::Open)));
        assert_eq!(g.u32(SCREEN + SCREEN_LENGTH), Ok(Transition::Open.millis()));
        let mut g = screen_with(0, 0, 1200);
        assert_eq!(cap_transition(&mut g, SCREEN), Ok(None));
        assert_eq!(g.u32(SCREEN + SCREEN_LENGTH), Ok(1200), "at rest: untouched");
        let mut g = screen_with(STATE_OUT, 0, 90);
        cap_transition(&mut g, SCREEN).unwrap();
        assert_eq!(g.u32(SCREEN + SCREEN_LENGTH), Ok(90), "a short one stays short");
    }

    /// The press that closed a screen is dropped; a press after the guard
    /// goes down to the screen beneath.
    #[test]
    fn a_press_after_the_close_guard_goes_to_the_screen_beneath() {
        use std::time::Duration;
        for (ms, down) in [(0, false), (20, false), (49, false), (50, true), (100, true), (1000, true)] {
            assert_eq!(hands_down(Duration::from_millis(ms)), down, "{ms} ms after the close");
        }
    }

    /// A closing dialog over Game Setup, with focus in a controller-specific child.
    #[test]
    fn a_closing_dialog_only_forwards_to_the_next_active_widgets() {
        use std::time::{Duration, Instant};
        let mut m = guest_layout::testing::FakeGuest::new();
        let (dialog, setup, item) = (0x8100_0000, 0x8101_0000, 0x8102_0000);
        let slot = crate::ui_watch::SHARED;
        let channel = crate::ui_watch::channel(3, slot).unwrap();
        m.map(0x0054_d000, 0x1000)
            .put_u32(dialog + SCREEN_WINDOW, 1)
            .put_u32(dialog + SCREEN_SLOT, slot)
            .put_u16(dialog + 8, 3)
            .put_u8(dialog + SCREEN_STATE, STATE_OUT)
            .put_u32(channel + crate::ui_watch::ACTIVE, setup)
            .put_u32(channel + 0x34, item)
            .put_u32(item + SCREEN_NEXT, setup)
            .put_u16(item + 8, 2)
            .put_u16(setup + 8, 1);
        let start = Instant::now();
        forget_closing(dialog);
        started_closing(dialog, start);
        let later = start + Duration::from_millis(60);
        assert_eq!(while_closing(&m, dialog, start, 0), Ok(Some(None)));
        assert_eq!(while_closing(&m, dialog, later, 0), Ok(Some(Some(setup))));
        assert_eq!(while_closing(&m, dialog, later, 1), Ok(Some(Some(item))));
        m.put_u32(item + SCREEN_NEXT, dialog);
        assert_eq!(while_closing(&m, dialog, later, 1), Ok(Some(None)), "stale focus");
        m.put_u32(item + SCREEN_NEXT, setup)
            .put_u32(channel + crate::ui_watch::PENDING, dialog);
        assert_eq!(while_closing(&m, dialog, later, 0), Ok(Some(None)), "pending");
        forget_closing(dialog);
    }

    #[test]
    fn work_runs_once_for_the_screen_closed_and_only_it() {
        let a: AfterClose<&str> = AfterClose::default();
        assert!(a.then(0x1000, "start"));
        assert!(!a.then(0x1000, "again"), "a second confirm while closing");
        assert!(a.closing(0x1000));
        assert_eq!(a.freed(0x2000), None, "another screen (B elsewhere)");
        assert_eq!(a.freed(0x1000), Some("start"));
        assert_eq!(a.freed(0x1000), None, "once");
    }
}
