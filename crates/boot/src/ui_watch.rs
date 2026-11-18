//! `H2_UI_WATCH=1`: which menu screen takes player one's input, read on
//! every game tick and printed when it changes (`ui: screen 0x09 (window
//! 5, slot 4)`), and answered over the agent API as `ui.screen`. `h2 coop
//! test --drive` sets it and presses through the menus on what it says
//! (docs/live/live-coop.md section 12.6).
//!
//! **The read** (static, from the input dispatcher `0x147dbe` and the push
//! `0x147f6d`). Screens sit on six windows of channels; each channel has
//! an active screen at `+8`, a pending one at `+0xc` (`0x1473b6` asks
//! whether either is set) and its screens in a list at `+0x34`, linked
//! through `+0x10`, each with a controller mask (word, `+8`) and its id at
//! `+0x70`. Windows 0, 2 and 4 are single channels (`0x54d99c`,
//! `0x54dba8`, `0x54d9e0`) used only from slot 4 (shared); windows 1, 3
//! and 5 are arrays by slot (`0x54d884` and `0x54d76c` with stride 0x38,
//! `0x54d62c` with stride 0x40). The dispatcher tries, for the shared slot
//! 4 and then the controller's own: window 0, 1, 2, 3, 4, 5, first with a
//! screen wins, and gives the input to the first screen in that channel's
//! list whose mask holds the controller (`0x23515d`). This reads the same
//! way for controller 0.
//!
//! On the Xbox Live menu (`custom_games::SCREEN_ID`) the line adds the row
//! the menu restores its cursor to (`0x510a18`, read when 0 to 3).
//!
//! Not confirmed by a game run: the ids of the boot screens (the intro,
//! the profile picker) and the lobby, which the lines will show.

use std::sync::Mutex;
use std::time::Instant;

use engine::points;

use crate::layout_glue::Gm;

pub const FEATURE: &str = "ui-watch";

/// The single channels, by window.
const WINDOW_0: u32 = 0x0054_d99c;
const WINDOW_2: u32 = 0x0054_dba8;
const WINDOW_4: u32 = 0x0054_d9e0;
/// The arrays by slot, by window, and their strides.
const WINDOW_1: u32 = 0x0054_d884;
const WINDOW_3: u32 = 0x0054_d76c;
const WINDOW_5: u32 = 0x0054_d62c;
const STRIDE_13: u32 = 0x38;
const STRIDE_5: u32 = 0x40;
/// The shared slot.
pub const SHARED: u32 = 4;
/// A channel's fields.
const ACTIVE: u32 = 8;
const PENDING: u32 = 0xc;
const LIST: u32 = 0x34;
/// A screen's fields.
const NEXT: u32 = 0x10;
const MASK: u32 = 8;
const ID: u32 = 0x70;
/// Where the Xbox Live menu keeps the row it restores.
const LIVE_MENU_ROW: u32 = 0x0051_0a18;
/// The main menu's screen id (docs/live/live-coop.md 12.1).
pub const MAIN_MENU: u32 = 9;
/// No real list is longer; a cycle in a torn read stops here.
const MAX_LIST: usize = 32;

/// The screen player one's input goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Top {
    pub window: u32,
    pub slot: u32,
    pub id: u32,
    /// The Xbox Live menu's restored row, on that menu.
    pub live_row: Option<u32>,
}

impl Top {
    pub fn line(&self) -> String {
        let mut s = format!(
            "ui: screen 0x{:02x} (window {}, slot {})",
            self.id, self.window, self.slot
        );
        if let Some(r) = self.live_row {
            s.push_str(&format!(", live menu row {r}"));
        }
        s
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "screen": self.id,
            "window": self.window,
            "slot": self.slot,
            "live_menu_row": self.live_row,
        })
    }
}

/// The channel of `window` for `slot`, if that window has one there.
fn channel(window: u32, slot: u32) -> Option<u32> {
    match window {
        0 => (slot == SHARED).then_some(WINDOW_0),
        1 => Some(WINDOW_1 + slot * STRIDE_13),
        2 => (slot == SHARED).then_some(WINDOW_2),
        3 => Some(WINDOW_3 + slot * STRIDE_13),
        4 => (slot == SHARED).then_some(WINDOW_4),
        5 => Some(WINDOW_5 + slot * STRIDE_5),
        _ => None,
    }
}

/// Whether a channel holds a screen (`0x1473b6`).
fn occupied<M: Gm + ?Sized>(m: &M, ch: u32) -> bool {
    m.u32(ch + ACTIVE).is_some_and(|v| v != 0) || m.u32(ch + PENDING).is_some_and(|v| v != 0)
}

/// A plausible screen id at `screen`.
fn id_of<M: Gm + ?Sized>(m: &M, screen: u32) -> Option<u32> {
    m.u32(screen + ID).filter(|id| *id < 0x400)
}

/// The screen in `ch` that takes `controller`'s input: the first in its
/// list whose mask holds it, else the active screen.
fn screen_in<M: Gm + ?Sized>(m: &M, ch: u32, controller: u32) -> Option<u32> {
    let mut at = m.u32(ch + LIST).unwrap_or(0);
    for _ in 0..MAX_LIST {
        if at == 0 {
            break;
        }
        let mask = m.u16(at + MASK)? as i16 as i32;
        if mask & (1 << controller) != 0 {
            return Some(at);
        }
        at = m.u32(at + NEXT)?;
    }
    m.u32(ch + ACTIVE).filter(|s| *s != 0)
}

/// The screen controller 0's input goes to, as the dispatcher picks it.
pub fn top<M: Gm + ?Sized>(m: &M) -> Option<Top> {
    for slot in [SHARED, 0] {
        for window in 0..6 {
            let Some(ch) = channel(window, slot) else {
                continue;
            };
            if !occupied(m, ch) {
                continue;
            }
            let screen = screen_in(m, ch, 0)?;
            let id = id_of(m, screen)?;
            let live_row = (id == crate::custom_games::SCREEN_ID as u32)
                .then(|| m.u32(LIVE_MENU_ROW))
                .flatten();
            return Some(Top {
                window,
                slot,
                id,
                live_row,
            });
        }
    }
    None
}

/// The newest read, for `ui.screen`, and when it was made.
static LAST: Mutex<Option<(Instant, Option<Top>)>> = Mutex::new(None);

/// The newest read and its age, if the watch runs.
pub fn last() -> Option<(std::time::Duration, Option<Top>)> {
    let l = LAST.lock().unwrap_or_else(|e| e.into_inner());
    l.map(|(at, top)| (at.elapsed(), top))
}

fn note(top: Option<Top>) -> Option<String> {
    let mut l = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let before = l.and_then(|(_, t)| t);
    let first = l.is_none();
    *l = Some((Instant::now(), top));
    match top {
        Some(t) if first || before != Some(t) => Some(t.line()),
        None if !first && before.is_some() => Some("ui: screen none".into()),
        _ => None,
    }
}

/// `H2_UI_WATCH`: on with 1, on, true or yes.
pub fn wanted(v: Option<&str>) -> bool {
    matches!(
        v.map(|s| s.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "on" | "true" | "yes")
    )
}

pub struct UiWatch;

impl feature::Feature for UiWatch {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new(
            FEATURE,
            "H2_UI_WATCH=1: the menu screen player one's input goes to, a line when it changes, and ui.screen",
        )
        .doc("docs/live/live-coop.md")
    }

    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        b.raw()
            .observe_after(&points::GAME_TICK, "ui: watch the screen", |o| {
                let g = o.guest();
                if let Some(line) = note(top(&*g)) {
                    eprintln!("{line}");
                }
            })
            .optional();
        Ok(())
    }
}

/// Adds the watch when `H2_UI_WATCH` asks for it.
pub fn install(registry: &mut feature::Registry) -> anyhow::Result<bool> {
    if !wanted(std::env::var("H2_UI_WATCH").ok().as_deref()) {
        return Ok(false);
    }
    registry
        .add(&UiWatch)
        .map_err(|e| anyhow::anyhow!("ui-watch: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake(BTreeMap<u32, u8>);
    impl Gm for Fake {
        fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
            for (i, b) in out.iter_mut().enumerate() {
                match self.0.get(&(addr + i as u32)) {
                    Some(v) => *b = *v,
                    None => *b = 0,
                }
            }
            true
        }
        fn wr(&mut self, addr: u32, data: &[u8]) -> bool {
            for (i, b) in data.iter().enumerate() {
                self.0.insert(addr + i as u32, *b);
            }
            true
        }
    }

    /// A screen with `id` and `mask` at `at`, first in `ch`'s list.
    fn screen(f: &mut Fake, ch: u32, at: u32, id: u32, mask: u16) {
        f.set_u32(ch + ACTIVE, at);
        let old = f.u32(ch + LIST).unwrap();
        f.set_u32(at + NEXT, old);
        f.set_u32(ch + LIST, at);
        f.set_u16(at + MASK, mask);
        f.set_u32(at + ID, id);
    }

    #[test]
    fn nothing_open_reads_none() {
        assert_eq!(top(&Fake::default()), None);
    }

    #[test]
    fn the_main_menu_on_window_5_shared() {
        let mut f = Fake::default();
        screen(&mut f, WINDOW_5 + 4 * STRIDE_5, 0x8000_0000, MAIN_MENU, 0xf);
        let t = top(&f).unwrap();
        assert_eq!((t.window, t.slot, t.id, t.live_row), (5, 4, 9, None));
        assert_eq!(t.line(), "ui: screen 0x09 (window 5, slot 4)");
    }

    #[test]
    fn a_dialog_on_window_3_comes_before_the_menu() {
        let mut f = Fake::default();
        screen(&mut f, WINDOW_5 + 4 * STRIDE_5, 0x8000_0000, MAIN_MENU, 0xf);
        screen(&mut f, WINDOW_3 + 4 * STRIDE_13, 0x8000_1000, 0x17, 0xf);
        assert_eq!(top(&f).unwrap().id, 0x17);
        // The modal window 0 before both.
        screen(&mut f, WINDOW_0, 0x8000_2000, 0x2a, 0xf);
        assert_eq!(top(&f).unwrap().window, 0);
    }

    #[test]
    fn the_list_gives_the_screen_whose_mask_holds_player_one() {
        let mut f = Fake::default();
        let ch = WINDOW_5 + 4 * STRIDE_5;
        screen(&mut f, ch, 0x8000_0000, 0x09, 0x1);
        // Pushed later, first in the list, but for player two only.
        screen(&mut f, ch, 0x8000_1000, 0x33, 0x2);
        assert_eq!(top(&f).unwrap().id, 0x09);
    }

    #[test]
    fn players_own_slot_after_the_shared_one() {
        let mut f = Fake::default();
        screen(&mut f, WINDOW_5, 0x8000_0000, 0x44, 0x1);
        let t = top(&f).unwrap();
        assert_eq!((t.window, t.slot, t.id), (5, 0, 0x44));
    }

    #[test]
    fn the_live_menu_says_its_row() {
        let mut f = Fake::default();
        screen(&mut f, WINDOW_5 + 4 * STRIDE_5, 0x8000_0000, 0xba, 0xf);
        f.set_u32(LIVE_MENU_ROW, 3);
        let t = top(&f).unwrap();
        assert_eq!(t.live_row, Some(3));
        assert_eq!(
            t.line(),
            "ui: screen 0xba (window 5, slot 4), live menu row 3"
        );
        assert_eq!(t.json()["live_menu_row"], 3);
    }

    #[test]
    fn a_torn_read_is_no_screen() {
        let mut f = Fake::default();
        screen(
            &mut f,
            WINDOW_5 + 4 * STRIDE_5,
            0x8000_0000,
            0x1234_5678,
            0xf,
        );
        assert_eq!(top(&f), None);
        // A cycle in the list stops.
        let ch = WINDOW_1 + 4 * STRIDE_13;
        screen(&mut f, ch, 0x8000_1000, 0x10, 0x2);
        f.set_u32(0x8000_1000 + NEXT, 0x8000_1000);
        f.set_u32(ch + ACTIVE, 0);
        f.set_u32(ch + PENDING, 1);
        assert_eq!(top(&f), None);
    }

    #[test]
    fn lines_only_on_change() {
        // One test owns LAST.
        let a = Top {
            window: 5,
            slot: 4,
            id: 9,
            live_row: None,
        };
        let b = Top {
            id: 0xba,
            live_row: Some(0),
            ..a
        };
        assert!(note(Some(a)).is_some());
        assert_eq!(note(Some(a)), None);
        assert_eq!(note(Some(b)).as_deref(), Some(b.line().as_str()));
        assert_eq!(note(None).as_deref(), Some("ui: screen none"));
        assert_eq!(note(None), None);
        assert!(last().unwrap().1.is_none());
        assert!(wanted(Some("1")) && wanted(Some("on")) && !wanted(None) && !wanted(Some("0")));
    }
}
