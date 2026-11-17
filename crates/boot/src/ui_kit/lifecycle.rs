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
        Ok((
            m.u32(screen + SCREEN_WINDOW)?,
            m.u32(screen + SCREEN_SLOT)?,
        ))
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
    /// `screen` was freed: its waiting work, if any.
    pub fn freed(&self, screen: u32) -> Option<T> {
        self.lock().remove(&screen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
