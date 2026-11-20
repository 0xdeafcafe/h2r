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

use std::collections::BTreeMap;
use std::sync::Mutex;

use engine::entry::{Cx, EntryError, GuestCall};
use engine::memory::MemFault;

/// Retail's close of a window's top screen.
pub const SCREEN_CLOSE: u32 = 0x0014_800c;
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
pub fn takes_close(m: &dyn engine::memory::GuestMemory, screen: u32) -> Result<bool, MemFault> {
    Ok(m.u8(screen + SCREEN_STATE)? & 2 == 0)
}

/// Closes `screen` itself, whether or not it is its window's top: the
/// close message `0x2351d4` sends, sent to the screen directly. For a
/// screen under one being freed: while the stack frees a screen
/// (`0x2351ea`) the window's top word is 0 until the window's next update,
/// and the close by window and slot ([`close_slot`]) reads `0 + 0x42`.
pub async fn close_screen(cx: &Cx, screen: u32) -> Result<(), EntryError> {
    if !cx.read(|m| takes_close(m, screen))? {
        return Ok(());
    }
    cx.call(
        GuestCall::new(SCREEN_MESSAGE)
            .ecx(screen)
            .args([MESSAGE_CLOSE]),
    )
    .await?;
    Ok(())
}

/// The row handle on the channel's focused widget path for `list`, as
/// LIST_FOCUSED_ROW finds it via the focused item's membership in that path.
pub(crate) fn focused_row_handle(
    m: &dyn engine::GuestMemory,
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
    use engine::memory::GuestMemory;

    /// The crash picking a map in Select Map's second step: the categories
    /// were closed by window and slot from the maps screen's destructor.
    /// The stack (`0x2351ea`) had zeroed the window's top word for the
    /// screen it was freeing, so `0x2351d4` read the state byte at
    /// `0 + 0x42` (`MemoryFault ... addr: 42`, ESI `0x54d84c`: window 3,
    /// slot 4). The screen itself still reads.
    #[test]
    fn a_screen_under_one_being_freed_closes_by_itself_not_by_its_window() {
        let mut g = engine::testing::FakeGuest::new();
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
