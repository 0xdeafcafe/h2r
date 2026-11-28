//! `H2_UI_WATCH=1`: which menu screen takes player one's input, read on
//! every UI update and printed when it changes (`ui: screen 0x09 (window
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
//! Dialogs (windows 0 to 4) read as no screen: their list entries carry no
//! plausible id at `+0x70` (the account picker's reads `0xe1670000`, its
//! low half the cursor's row). Their active screen's id does read, so a
//! none line with a dialog up names it ([`dialog`], [`dialog_name`]):
//! `ui: screen none (...), dialog 0x14 dialog_live_signin (window 3, slot
//! 4)`. This distinguishes the account picker from other dialogues so the
//! menu driver can choose the account row.
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
//! The retail main menu rows (`0x230827`, the chosen row kept at `0x510a14`):
//! 0 Campaign, 1 Xbox Live, 2 Split Screen, 3 System Link, 4 Settings.
//! With Theater installed, it occupies row 4 and Settings moves to row 5;
//! the rows an agent presses keep their retail places.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use guest_layout::points;

use crate::layout_glue::Gm;
use crate::ui_audit::ObservedFindings;
use crate::ui_kit::lifecycle::SCREEN_NEXT;
use crate::ui_kit::preview::{Fonts, SyntheticSource};
use crate::ui_kit::view::Metrics;

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
pub(crate) const ACTIVE: u32 = 8;
pub(crate) const PENDING: u32 = 0xc;
const LIST: u32 = 0x34;
/// A screen's fields.
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
pub(crate) const JOIN_MODE: u32 = 0x610;
/// No real list is longer; a cycle in a torn read stops here.
const MAX_LIST: usize = 32;

/// The screen player one's input goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Top {
    pub window: u32,
    pub slot: u32,
    pub id: u32,
    /// The list class name when it uses a shared retail skin.
    pub class_name: Option<&'static str>,
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
        if let Some(class) = self.class_name {
            s.push_str(&format!(" {class}"));
        }
        if let Some(r) = self.live_row {
            s.push_str(&format!(", live menu row {r}"));
        }
        if let Some(m) = self.mode {
            s.push_str(&format!(", mode {m}"));
        }
        s
    }

    pub fn json(&self) -> serde_json::Value {
        let mut json = serde_json::json!({
            "screen": self.id,
            "window": self.window,
            "slot": self.slot,
            "live_menu_row": self.live_row,
            "mode": self.mode,
        });
        if let Some(class) = self.class_name {
            json["class"] = serde_json::json!(class);
        }
        json
    }
}

/// The channel of `window` for `slot`, if that window has one there.
pub(crate) fn channel(window: u32, slot: u32) -> Option<u32> {
    if slot > SHARED {
        return None;
    }
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
        at = m.u32(at + SCREEN_NEXT)?;
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
    /// The first screen on windows 0 to 4 ([`dialog`]), if any.
    pub dialog: Option<Dialog>,
}

impl Read {
    /// A none line names the dialog up, if any (module docs); a screen's
    /// line is as it was.
    pub fn line(&self) -> String {
        match (&self.top, &self.dialog) {
            (Some(t), _) => t.line(),
            (None, Some(d)) => format!("ui: screen none ({}), {}", self.why_none, d.line()),
            (None, None) => format!("ui: screen none ({})", self.why_none),
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
    let dialog = dialog(m);
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
                    class_name: m.u32(screen).and_then(crate::list_class::class_name),
                    screen,
                    live_row,
                    mode,
                }),
                why_none: String::new(),
                dialog,
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
        dialog,
    }
}

/// One window channel's screens, for a log line: its active and pending
/// screens and its list, top first, each with its id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stack {
    pub window: u32,
    pub slot: u32,
    pub active: u32,
    pub pending: u32,
    /// The list's screens and ids (`None`: no plausible id).
    pub list: Vec<(u32, Option<u32>)>,
}

impl Stack {
    /// Whether a screen with `id` is the channel's active or pending one,
    /// or in its list.
    pub fn holds(&self, id: u32) -> bool {
        self.list.iter().any(|(_, i)| *i == Some(id))
    }
    pub fn line(&self) -> String {
        let list: Vec<String> = self
            .list
            .iter()
            .map(|(at, id)| match id {
                Some(id) => format!("{at:#x}/{id:#x}"),
                None => format!("{at:#x}/?"),
            })
            .collect();
        format!(
            "window {} slot {}: active {:#x}, pending {:#x}, list [{}]",
            self.window,
            self.slot,
            self.active,
            self.pending,
            list.join(" ")
        )
    }
}

/// `window`'s channel for `slot` as it is now; `None` when there is no
/// such channel or it is unreadable. The active and pending screens are
/// added to the list when it does not hold them.
pub fn stack<M: Gm + ?Sized>(m: &M, window: u32, slot: u32) -> Option<Stack> {
    let ch = channel(window, slot)?;
    let (active, pending) = (m.u32(ch + ACTIVE)?, m.u32(ch + PENDING)?);
    let mut list = Vec::new();
    let mut at = m.u32(ch + LIST)?;
    while at != 0 && list.len() < MAX_LIST {
        list.push((at, id_of(m, at)));
        at = m.u32(at + SCREEN_NEXT).unwrap_or(0);
    }
    for s in [active, pending] {
        if s != 0 && !list.iter().any(|(at, _)| *at == s) {
            list.push((s, id_of(m, s)));
        }
    }
    Some(Stack {
        window,
        slot,
        active,
        pending,
        list,
    })
}

/// A screen on windows 0 to 4 (see [`dialog`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dialog {
    /// Kit dialogs share retail screen ids. Their class identifies them.
    pub class_name: Option<&'static str>,
    pub window: u32,
    pub slot: u32,
    /// The channel's active screen's id, when it has a plausible one.
    pub id: Option<u32>,
}

/// The account picker, `dialog_live_signin` (screen 20, loader
/// `0x24b4a9`): row 0 is "no account" while the profile is signed out.
pub const ACCOUNT_PICKER: u32 = 0x14;

/// A dialog screen's tag name, by id, for the ones read off `mainmenu.map`
/// (docs/game/ui-builder.md, "The modal dialog"). `None` for the others
/// (the new profile's boot dialogs `0x0a` and `0xa7` are not named yet).
pub fn dialog_name(id: u32) -> Option<&'static str> {
    Some(match id {
        0x08 => "error_dialog_ok",
        ACCOUNT_PICKER => "dialog_live_signin",
        0x15 => "dialog_live_password",
        0x21 => "confirmation_dialog",
        0xb1 => "party_leader_picker",
        0xee => "tcr_dialog",
        0xf0 => "error_dialog_large",
        _ => return None,
    })
}

impl Dialog {
    /// `dialog 0x14 dialog_live_signin (window 3, slot 4)`; `dialog 0x0a
    /// (window 3, slot 4)` without a name, `dialog ? (...)` without an id.
    pub fn line(&self) -> String {
        let id = match self.id {
            Some(i) => match self.class_name.or_else(|| dialog_name(i)) {
                Some(n) => format!("0x{i:02x} {n}"),
                None => format!("0x{i:02x}"),
            },
            None => "?".to_owned(),
        };
        format!("dialog {id} (window {}, slot {})", self.window, self.slot)
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "name": self.class_name.or_else(|| self.id.and_then(dialog_name)),
            "window": self.window,
            "slot": self.slot,
        })
    }
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
                let active = m.u32(ch + ACTIVE).filter(|&s| s != 0);
                let id = active.and_then(|s| id_of(m, s));
                let class_name = active
                    .and_then(|s| m.u32(s))
                    .and_then(crate::ui_kit::dialog_tag::by_vtable)
                    .map(|d| d.name)
                    .filter(|n| {
                        matches!(
                            *n,
                            crate::clanker_card::SCREEN
                                | crate::lab_takeover::SCREEN
                                | crate::lab_takeover::ALONGSIDE_SCREEN
                                | crate::lab_takeover::RUNNING_SCREEN
                                | crate::ui_kit::picker::NAME
                                | crate::ui_kit::picker::GAME_NAME
                        )
                    });
                return Some(Dialog {
                    window,
                    slot,
                    id,
                    class_name,
                });
            }
        }
    }
    None
}

/// The screen alone ([`read`]).
pub fn top<M: Gm + ?Sized>(m: &M) -> Option<Top> {
    read(m).top
}

/// The newest screen and widget findings sampled on the same UI update.
#[derive(Clone)]
struct Snapshot {
    at: Instant,
    top: Option<Top>,
    dialog: Option<Dialog>,
    findings: ObservedFindings,
    fonts: Option<Arc<Fonts>>,
}

static LAST: Mutex<Option<Snapshot>> = Mutex::new(None);

pub fn last_json() -> Option<serde_json::Value> {
    let mut read = LAST.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    let metrics: &dyn Metrics = match read.fonts.as_deref() {
        Some(fonts) => fonts,
        None => &SyntheticSource,
    };
    read.findings.resolve_authored(metrics);
    let mut json = screen_json(read.at.elapsed(), read.top, read.dialog);
    json["layout_findings"] = serde_json::json!(read.findings.layout);
    json["content_findings"] = serde_json::json!(read.findings.content);
    Some(json)
}

/// `ui.screen`'s answer: the screen's fields (`"screen": null` for none),
/// `dialog` (`null`, or its `id`, `name`, `window` and `slot`), `age_ms`.
pub fn screen_json(
    age: std::time::Duration,
    top: Option<Top>,
    dialog: Option<Dialog>,
) -> serde_json::Value {
    let mut v = top.map_or_else(|| serde_json::json!({ "screen": null }), |t| t.json());
    v["dialog"] = dialog.map_or(serde_json::Value::Null, |d| d.json());
    v["age_ms"] = serde_json::json!(age.as_millis() as u64);
    v
}

/// The line said last, so a line is said only on a change.
static SAID: Mutex<String> = Mutex::new(String::new());

fn note(r: Read, findings: ObservedFindings, fonts: Option<Arc<Fonts>>) -> Option<String> {
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(Snapshot {
        at: Instant::now(),
        top: r.top,
        dialog: r.dialog,
        findings,
        fonts,
    });
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

#[derive(Default)]
pub struct UiWatch {
    fonts: Option<Arc<Fonts>>,
}

impl hooks::Feature for UiWatch {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            FEATURE,
            "H2_UI_WATCH=1: the menu screen player one's input goes to, a line when it changes, and ui.screen",
        )
        .doc("docs/live/live-coop.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        let fonts = self.fonts.clone();
        b.raw()
            .observe_after(&points::UI_UPDATE, "ui: watch the screen", move |o| {
                let g = o.guest();
                let r = read(&*g);
                let mut findings = r.top.map_or_else(ObservedFindings::default, |t| {
                    crate::ui_audit::observe(&*g, t.screen, Some(t.id), channel(t.window, t.slot))
                });
                if let Some(d) = r.dialog
                    && let Some(ch) = channel(d.window, d.slot)
                    && let Some(active) = Gm::u32(&*g, ch + ACTIVE).filter(|&s| s != 0)
                    && r.top.is_none_or(|t| t.screen != active)
                {
                    findings.append(crate::ui_audit::observe(&*g, active, d.id, Some(ch)));
                }
                drop(g);
                if let Some(line) = note(r, findings, fonts.clone()) {
                    eprintln!("{line}");
                }
            })
            .optional();
        // An observer: `LAST` and `SAID` are the host's log of guest
        // memory, read again at the next UI update.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Adds the watch when `H2_UI_WATCH` asks for it.
pub fn install(registry: &mut hooks::Registry, disc: &Path) -> anyhow::Result<bool> {
    if !wanted(std::env::var("H2_UI_WATCH").ok().as_deref()) {
        return Ok(false);
    }
    let fonts = match Fonts::open(disc) {
        Ok(fonts) => Some(Arc::new(fonts)),
        Err(e) => {
            eprintln!("ui: font metrics unavailable: {e:#}");
            None
        }
    };
    registry
        .add(&UiWatch { fonts })
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
        f.set_u32(at + SCREEN_NEXT, old);
        f.set_u32(ch + LIST, at);
        f.set_u16(at + MASK, mask);
        f.set_u32(at + ID, id);
    }

    /// The skulls page's log of window 5 slot 4 around the difficulty
    /// press: every screen in the list, the pending one too.
    #[test]
    fn a_stack_lists_its_screens_with_their_ids() {
        let mut f = Fake::default();
        let ch = WINDOW_5 + 4 * STRIDE_5;
        screen(&mut f, ch, 0x8000_0000, 0x0c, 0xf);
        screen(&mut f, ch, 0x8000_1000, 0x2a, 0xf);
        f.set_u32(ch + PENDING, 0x8000_2000);
        f.set_u32(0x8000_2000 + ID, 0xd1);
        let s = stack(&f, 5, 4).unwrap();
        assert_eq!(s.active, 0x8000_1000);
        assert!(s.holds(0xd1) && s.holds(0x0c) && !s.holds(0x09));
        assert_eq!(
            s.line(),
            "window 5 slot 4: active 0x80001000, pending 0x80002000, \
             list [0x80001000/0x2a 0x80000000/0xc 0x80002000/0xd1]"
        );
        assert_eq!(stack(&f, 9, 4), None, "no such window");
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
                class_name: None,
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
                class_name: None,
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
        f.set_u32(0x8000_1000 + SCREEN_NEXT, 0x8000_1000);
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
        // The window 1 screen is the dialog up ([`dialog`]: any screen on
        // windows 0 to 4, whoever's mask), and why player one's input
        // goes nowhere, so the line names it.
        assert_eq!(
            r.line(),
            "ui: screen none (slot 4: window 1 active 0x80001000 pending 0x0 list [0x2/0xb6]; \
             slot 0: no window), dialog 0xb6 (window 1, slot 4)"
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

    /// Run 20261119-115144: the account picker over the main menu reads as
    /// no screen (its list entry's `+0x70` is `0xe1670000`), and the line
    /// names it by its active screen's id.
    #[test]
    fn a_none_line_names_the_dialog_up() {
        let mut f = Fake::default();
        screen(
            &mut f,
            WINDOW_5 + 4 * STRIDE_5,
            0x8000_0000,
            0xe16e_0001,
            0x1,
        );
        let ch = WINDOW_3 + 4 * STRIDE_13;
        screen(&mut f, ch, 0x8000_1000, 0xe167_0000, 0x1);
        // The active screen is another record than the list entry.
        f.set_u32(ch + ACTIVE, 0x8000_2000);
        f.set_u32(0x8000_2000 + ID, ACCOUNT_PICKER);
        let r = read(&f);
        assert_eq!(r.top, None);
        assert_eq!(
            r.line(),
            "ui: screen none (slot 4: window 3 active 0x80002000 pending 0x0 list \
             [0x1/0xe1670000]; slot 0: no window), dialog 0x14 dialog_live_signin \
             (window 3, slot 4)"
        );
        let d = r.dialog.unwrap();
        assert_eq!(d.json()["name"], "dialog_live_signin");
        assert_eq!(d.json()["id"], 0x14);
        // An unnamed one gives its id, an unread one `?`.
        let d = Dialog {
            class_name: None,
            window: 3,
            slot: 4,
            id: Some(0x0a),
        };
        assert_eq!(d.line(), "dialog 0x0a (window 3, slot 4)");
        assert_eq!(d.json()["name"], serde_json::Value::Null);
        let d = Dialog { id: None, ..d };
        assert_eq!(d.line(), "dialog ? (window 3, slot 4)");
        // A screen's line is as it was, dialog or not.
        screen(&mut f, WINDOW_1 + 4 * STRIDE_13, 0x8000_3000, 0xb6, 0x1);
        assert_eq!(read(&f).line(), "ui: screen 0xb6 (window 1, slot 4)");
    }

    #[test]
    fn ui_screen_names_the_party_leader_picker() {
        let mut f = Fake::default();
        screen(&mut f, WINDOW_3 + 4 * STRIDE_13, 0x8000_2000, 0xb1, 1);
        let v = screen_json(std::time::Duration::ZERO, None, dialog(&f));
        assert_eq!(v["dialog"]["id"], 0xb1);
        assert_eq!(v["dialog"]["name"], "party_leader_picker");
    }

    #[test]
    fn ui_screen_distinguishes_the_card_and_value_pickers_sharing_the_controls_id() {
        use crate::ui_kit::{dialog::DialogBase, dialog_tag, picker};
        for (i, (name, base)) in [
            (crate::clanker_card::SCREEN, DialogBase::Large),
            (picker::NAME, DialogBase::ValuePicker),
            (picker::GAME_NAME, DialogBase::Large),
        ]
        .into_iter()
        .enumerate()
        {
            let offset = i as u32 * 0x100;
            let d = dialog_tag::Registered {
                name,
                vtable: 0x7154_2000 + offset,
                page: 0x7154_4000 + offset,
                loader: 0x7154_3000 + offset,
                base,
                list: dialog_tag::base_layout(base, 4),
            };
            dialog_tag::register(d);
            let mut f = Fake::default();
            let active = 0x8000_2000;
            screen(&mut f, WINDOW_1 + 4 * STRIDE_13, active, 39, 1);
            f.set_u32(active, d.vtable);
            let v = screen_json(std::time::Duration::ZERO, None, dialog(&f));
            assert_eq!(v["dialog"]["name"], name);
            assert_eq!(v["dialog"]["id"], 39);
        }
    }

    #[test]
    fn ui_screen_answers_with_the_dialog() {
        let age = std::time::Duration::from_millis(40);
        let v = screen_json(
            age,
            None,
            Some(Dialog {
                class_name: None,
                window: 3,
                slot: 4,
                id: Some(ACCOUNT_PICKER),
            }),
        );
        assert_eq!(v["screen"], serde_json::Value::Null);
        assert_eq!(v["dialog"]["name"], "dialog_live_signin");
        assert_eq!(v["dialog"]["window"], 3);
        assert_eq!(v["age_ms"], 40);
        let top = Top {
            window: 5,
            slot: 4,
            id: 0xba,
            class_name: None,
            screen: 0x8000_0000,
            live_row: Some(2),
            mode: None,
        };
        let v = screen_json(age, Some(top), None);
        assert_eq!(v["screen"], 0xba);
        assert_eq!(v["dialog"], serde_json::Value::Null);
    }

    #[test]
    fn in_game_list_classes_distinguish_screens_with_the_same_retail_id() {
        for class in ["video", "voice"] {
            let top = Top {
                window: 5,
                slot: 1,
                id: 200,
                class_name: Some(class),
                screen: 0x8000_0000,
                live_row: None,
                mode: None,
            };
            let wire = screen_json(std::time::Duration::ZERO, Some(top), None);
            assert_eq!(wire["screen"], 200);
            assert_eq!(wire["class"], class);
            assert_eq!(wire["slot"], 1);
            assert_eq!(top.line(), format!("ui: screen 0xc8 (window 5, slot 1) {class}"));
        }
    }

    #[test]
    fn the_watch_observes_ui_updates_during_a_simulation_pause() {
        let mut r = hooks::Registry::new(hooks::Switches::defaults());
        assert!(r.add(&UiWatch::default()).unwrap());
        let plan = r.build(&hooks::Base).unwrap().text();
        assert!(plan.contains("UI_UPDATE"), "{plan}");
        assert!(!plan.contains("GAME_TICK"), "{plan}");
    }

    #[test]
    fn lines_only_on_change() {
        // One test owns LAST and SAID.
        let a = Top {
            window: 5,
            slot: 4,
            id: 9,
            class_name: None,
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
            dialog: None,
        };
        assert!(note(r(Some(a)), ObservedFindings::default(), None).is_some());
        assert_eq!(note(r(Some(a)), ObservedFindings::default(), None), None);
        assert_eq!(note(r(Some(b)), ObservedFindings::default(), None).as_deref(), Some(b.line().as_str()));
        assert_eq!(
            note(r(None), ObservedFindings::default(), None).as_deref(),
            Some("ui: screen none (slot 4: no window; slot 0: no window)")
        );
        assert_eq!(note(r(None), ObservedFindings::default(), None), None);
        let finding = serde_json::json!({"issue": {"kind": "unsafe"}});
        let content = serde_json::json!({"issue": {"kind": "placeholder"}});
        let mut findings = ObservedFindings::default();
        findings.layout.push(finding.clone());
        findings.content.push(content.clone());
        assert_eq!(note(r(None), findings, None), None);
        let wire = last_json().unwrap();
        assert!(wire["screen"].is_null());
        assert_eq!(wire["layout_findings"][0], finding);
        assert_eq!(wire["content_findings"][0], content);
        assert_eq!(note(r(None), ObservedFindings::default(), None), None);
        assert_eq!(last_json().unwrap()["layout_findings"], serde_json::json!([]));
        assert!(wanted(Some("1")) && wanted(Some("on")) && !wanted(None) && !wanted(Some("0")));
    }
}
