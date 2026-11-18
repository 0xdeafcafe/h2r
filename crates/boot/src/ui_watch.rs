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
//! A channel whose list has no screen for the controller passes the input
//! to the controller's own slot, not to the next window; with no screen at
//! all the line says why (`ui: screen none (slot 4: window 1 active ...
//! list [mask/id ...]; slot 0: ...)`), so a run shows what was there.
//!
//! **The screens seen in runs** (20261118-092343, traced statically):
//!
//! | id | window | what |
//! |---|---|---|
//! | none | | the main menu (pushed as id 9 on window 5, slot 4 by `0x1483c3`, loader `0x22f11e`), read as no screen; why is open: the none line now says |
//! | `0xb6` | 1 | a moment on the way to the main menu (constructor at `0x1a2e32`) |
//! | `0xee` | 1 | A on Campaign (constructor at `0x2acb02`) |
//! | `0xd1` | 5 | the campaign's next screen, then the level loads (`0x2b14b2`) |
//! | `0x1e` | 5 | the players' screen (`0x2524cc`) in a mode (`+0x610`, now on the line): 1 Split Screen (`0x25240c`, main menu row 2), 2 System Link (`0x252433`, row 3), 3 Xbox Live (`0x25245a`, row 1, when signed in: `0x2308e0`), 4 an accepted invitation (`0x252481`, from the invitation dialog's first button `0x22f36f`), 0 from `0x2524a8` |
//! | `0xba` | | the Xbox Live menu |
//!
//! On the players' screen, A or START (event buttons 0 and 0xc,
//! `0x25286d`) goes on by mode: 3 opens the Live menu (`0x2310b7`), 2 the
//! System Link browser, 4 joins; B or Back with one player goes back.
//! The main menu's rows (`0x230827`, the chosen row kept at `0x510a14`):
//! 0 Campaign, 1 Xbox Live, 2 Split Screen, 3 System Link, 4 Settings.

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
/// The main menu's screen id (docs/live/live-coop.md 12.1). [guess] Runs
/// read it as no screen (run 20261118-092343); see the module docs.
pub const MAIN_MENU: u32 = 9;
/// The players' screen (constructor `0x2524cc`, vtable `0x45a490`) and
/// its mode word.
pub const JOIN_SCREEN: u32 = 0x1e;
const JOIN_MODE: u32 = 0x610;
/// No real list is longer; a cycle in a torn read stops here.
const MAX_LIST: usize = 32;

/// The screen player one's input goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Top {
    pub window: u32,
    pub slot: u32,
    pub id: u32,
    /// The screen's address (for the players' screen, its join mask at
    /// `+8`).
    pub screen: u32,
    /// The Xbox Live menu's restored row, on that menu.
    pub live_row: Option<u32>,
    /// The players' screen's mode ([`JOIN_SCREEN`], `+0x610`): what A on
    /// it goes on to.
    pub mode: Option<u32>,
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
        if let Some(m) = self.mode {
            s.push_str(&format!(", mode {m}"));
        }
        s
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "screen": self.id,
            "window": self.window,
            "slot": self.slot,
            "live_menu_row": self.live_row,
            "mode": self.mode,
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

/// The first screen in `ch`'s list whose mask holds `controller`
/// (`0x23515d`); `seen` gets each screen's mask and raw id on the way.
fn screen_in<M: Gm + ?Sized>(
    m: &M,
    ch: u32,
    controller: u32,
    seen: &mut Vec<String>,
) -> Option<u32> {
    let mut at = m.u32(ch + LIST).unwrap_or(0);
    for _ in 0..MAX_LIST {
        if at == 0 {
            return None;
        }
        let mask = m.u16(at + MASK)?;
        let id = m.u32(at + ID)?;
        seen.push(format!("{mask:#x}/{id:#x}"));
        if (mask as i16 as i32) & (1 << controller) != 0 {
            return Some(at);
        }
        at = m.u32(at + NEXT)?;
    }
    None
}

/// What one read found: the screen, or why none (each slot the
/// dispatcher tried: its first occupied window, the channel's active and
/// pending screens, and its list as mask/id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Read {
    pub top: Option<Top>,
    pub why_none: String,
}

impl Read {
    pub fn line(&self) -> String {
        match &self.top {
            Some(t) => t.line(),
            None => format!("ui: screen none ({})", self.why_none),
        }
    }
}

/// The screen controller 0's input goes to, as the dispatcher `0x147dbe`
/// picks it: for the shared slot 4, then the controller's own (taken as
/// slot 0), the first window holding a screen (0 to 5; windows 0, 2 and 4
/// only for slot 4), and in it the first screen whose mask holds the
/// controller. A window whose list has none for it passes the input on to
/// the next slot, not the next window.
pub fn read<M: Gm + ?Sized>(m: &M) -> Read {
    let mut why = Vec::new();
    for slot in [SHARED, 0] {
        let Some((window, ch)) = (0..6)
            .filter_map(|w| channel(w, slot).map(|c| (w, c)))
            .find(|(_, c)| occupied(m, *c))
        else {
            why.push(format!("slot {slot}: no window"));
            continue;
        };
        let mut seen = Vec::new();
        let found = screen_in(m, ch, 0, &mut seen);
        if let Some(screen) = found
            && let Some(id) = id_of(m, screen)
        {
            let live_row = (id == crate::custom_games::SCREEN_ID as u32)
                .then(|| m.u32(LIVE_MENU_ROW))
                .flatten();
            let mode = (id == JOIN_SCREEN)
                .then(|| m.u32(screen + JOIN_MODE))
                .flatten();
            return Read {
                top: Some(Top {
                    window,
                    slot,
                    id,
                    screen,
                    live_row,
                    mode,
                }),
                why_none: String::new(),
            };
        }
        why.push(format!(
            "slot {slot}: window {window} active {:#x} pending {:#x} list [{}]",
            m.u32(ch + ACTIVE).unwrap_or(0),
            m.u32(ch + PENDING).unwrap_or(0),
            seen.join(" ")
        ));
    }
    Read {
        top: None,
        why_none: why.join("; "),
    }
}

/// A screen on windows 0 to 4 (see [`dialog`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dialog {
    pub window: u32,
    pub slot: u32,
    /// The channel's active screen's id, when it has a plausible one.
    pub id: Option<u32>,
}

/// The first screen on windows 0 to 4, any slot (4, the shared one, and
/// the controllers' 0 to 3), as `0x14917c(window)` asks one window over
/// every slot: whatever sits over the window-5 menus. The players screen
/// ignores every button while window 1 has one (`0x252ad7` asks
/// `0x14917c(1)`), and the profile chooser it opens for a controller
/// (`0x24b6ea` and `0x1905bf`, a screen request with words 3 and 4 and that
/// controller's mask) is one of these. `None` when the menus are bare.
pub fn dialog<M: Gm + ?Sized>(m: &M) -> Option<Dialog> {
    for window in 0..5 {
        for slot in 0..=SHARED {
            let Some(ch) = channel(window, slot) else {
                continue;
            };
            if occupied(m, ch) {
                let id = m
                    .u32(ch + ACTIVE)
                    .filter(|&s| s != 0)
                    .and_then(|s| id_of(m, s));
                return Some(Dialog { window, slot, id });
            }
        }
    }
    None
}

/// The screen alone ([`read`]).
pub fn top<M: Gm + ?Sized>(m: &M) -> Option<Top> {
    read(m).top
}

/// The newest read, for `ui.screen`, and when it was made.
static LAST: Mutex<Option<(Instant, Option<Top>)>> = Mutex::new(None);

/// The newest read and its age, if the watch runs.
pub fn last() -> Option<(std::time::Duration, Option<Top>)> {
    let l = LAST.lock().unwrap_or_else(|e| e.into_inner());
    l.map(|(at, top)| (at.elapsed(), top))
}

/// The line said last, so a line is said only on a change.
static SAID: Mutex<String> = Mutex::new(String::new());

fn note(r: Read) -> Option<String> {
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), r.top));
    let line = r.line();
    let mut said = SAID.lock().unwrap_or_else(|e| e.into_inner());
    if *said == line {
        return None;
    }
    *said = line.clone();
    Some(line)
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
                if let Some(line) = note(read(&*g)) {
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
    fn a_dialog_is_any_screen_below_window_5_in_any_slot() {
        let mut f = Fake::default();
        screen(
            &mut f,
            WINDOW_5 + 4 * STRIDE_5,
            0x8000_0000,
            JOIN_SCREEN,
            0x1,
        );
        assert_eq!(dialog(&f), None, "the players screen alone");
        // A profile chooser for controller 1: the shared slot's first
        // window is now 3, which has nothing for player one, so player
        // one's input goes nowhere; a bot must not press A either.
        screen(&mut f, WINDOW_3 + 4 * STRIDE_13, 0x8000_1000, 0x17, 0x2);
        assert_eq!(top(&f), None);
        assert_eq!(
            dialog(&f),
            Some(Dialog {
                window: 3,
                slot: 4,
                id: Some(0x17)
            })
        );
        // One in a controller's own slot counts too, the lower window first.
        let mut f = Fake::default();
        screen(&mut f, WINDOW_1 + 2 * STRIDE_13, 0x8000_2000, 0x40, 0x4);
        f.set_u32(WINDOW_3 + 4 * STRIDE_13 + PENDING, 0x8000_3000);
        assert_eq!(dialog(&f).map(|d| (d.window, d.slot)), Some((1, 2)));
        // A pending screen alone is one, with no id yet.
        let mut f = Fake::default();
        f.set_u32(WINDOW_0 + PENDING, 0x8000_3000);
        assert_eq!(
            dialog(&f),
            Some(Dialog {
                window: 0,
                slot: 4,
                id: None
            })
        );
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
    fn a_window_without_player_one_passes_to_its_own_slot_not_the_next_window() {
        let mut f = Fake::default();
        // The main menu on window 5, under a window 1 screen for player
        // two only: the dispatcher never reaches window 5 for slot 4.
        screen(&mut f, WINDOW_5 + 4 * STRIDE_5, 0x8000_0000, MAIN_MENU, 0xf);
        screen(&mut f, WINDOW_1 + 4 * STRIDE_13, 0x8000_1000, 0xb6, 0x2);
        let r = read(&f);
        assert_eq!(r.top, None);
        assert_eq!(
            r.line(),
            "ui: screen none (slot 4: window 1 active 0x80001000 pending 0x0 list [0x2/0xb6]; \
             slot 0: no window)"
        );
        // A screen in player one's own slot takes it.
        screen(&mut f, WINDOW_5, 0x8000_2000, 0x1e, 0x1);
        assert_eq!(top(&f).map(|t| (t.slot, t.id)), Some((0, 0x1e)));
    }

    #[test]
    fn the_players_screen_says_its_mode() {
        let mut f = Fake::default();
        screen(
            &mut f,
            WINDOW_5 + 4 * STRIDE_5,
            0x8000_0000,
            JOIN_SCREEN,
            0xffff,
        );
        f.set_u32(0x8000_0000 + JOIN_MODE, 3);
        let t = top(&f).unwrap();
        assert_eq!(t.mode, Some(3));
        assert_eq!(t.screen, 0x8000_0000, "where its join mask is");
        assert_eq!(t.line(), "ui: screen 0x1e (window 5, slot 4), mode 3");
    }

    #[test]
    fn lines_only_on_change() {
        // One test owns LAST and SAID.
        let a = Top {
            window: 5,
            slot: 4,
            id: 9,
            screen: 0x8000_0000,
            live_row: None,
            mode: None,
        };
        let b = Top {
            id: 0xba,
            live_row: Some(0),
            ..a
        };
        let r = |top: Option<Top>| Read {
            top,
            why_none: "slot 4: no window; slot 0: no window".into(),
        };
        assert!(note(r(Some(a))).is_some());
        assert_eq!(note(r(Some(a))), None);
        assert_eq!(note(r(Some(b))).as_deref(), Some(b.line().as_str()));
        assert_eq!(
            note(r(None)).as_deref(),
            Some("ui: screen none (slot 4: no window; slot 0: no window)")
        );
        assert_eq!(note(r(None)), None);
        assert!(last().unwrap().1.is_none());
        assert!(wanted(Some("1")) && wanted(Some("on")) && !wanted(None) && !wanted(Some("0")));
    }
}
